use chrono::prelude::*;
use futures::executor::block_on;
use google_calendar::{events::Events, Client};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io};

#[derive(Serialize, Deserialize, Default, Clone)]
struct ToilConfig {
    client_id: Option<String>,
    client_secret: Option<String>,
    redirect_uri: Option<String>,
    refresh_token: Option<String>,
}

#[tokio::main]
async fn main() {
    let mut cfg: ToilConfig = confy::load("momentary_toil", None).unwrap();

    if cfg.client_id.is_none() {
        cfg.client_id = Some(prompt(None, "Enter your client_id: "));
    }
    if cfg.client_secret.is_none() {
        cfg.client_secret = Some(prompt(None, "Enter your client_secret: "));
    }
    if cfg.redirect_uri.is_none() {
        cfg.redirect_uri = Some(prompt(None, "Enter your redirect_uri: "));
    }
    confy::store("momentary_toil", None, cfg.clone()).unwrap();

    let current_week = Utc::now().iso_week();
    let start_week =
        NaiveDate::from_isoywd_opt(current_week.year(), current_week.week(), Weekday::Mon)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .unwrap();
    let start_week = Utc.from_local_datetime(&start_week).single().unwrap();
    let end_week =
        NaiveDate::from_isoywd_opt(current_week.year(), current_week.week(), Weekday::Sat)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .unwrap();
    let end_week = Utc.from_local_datetime(&end_week).single().unwrap();
    println!(
        "Week:: {:?} -> {:?}",
        start_week.date_naive(),
        end_week.date_naive()
    );
    block_on(do_call(&mut cfg, start_week, end_week));
}

fn prompt(instr: Option<&str>, msg: &str) -> String {
    if let Some(inst) = instr {
        println!("{inst}");
    }
    println!("{msg}");
    let mut buffer = String::new();
    io::stdin()
        .read_line(&mut buffer)
        .expect("User to enter a value");
    buffer
}

async fn do_call<'a>(cfg: &mut ToilConfig, start: DateTime<Utc>, end: DateTime<Utc>) {
    let access = "".to_string();
    let refresh = cfg.refresh_token.clone().unwrap_or("".to_string());

    let mut gcal = Client::new(
        cfg.client_id.as_ref().expect("Config required a client_id"),
        cfg.client_secret
            .as_ref()
            .expect("Config required a client_secret"),
        cfg.redirect_uri
            .as_ref()
            .expect("Config required a redirect_uri"),
        &access,
        &refresh,
    );

    if access.eq("") && refresh.eq("") {
        // Get the URL to request consent from the user.
        // You can optionally pass in scopes. If none are provided, then the
        // resulting URL will not have any scopes.
        let user_consent_url = gcal.user_consent_url(&[
            "https://www.googleapis.com/auth/calendar.readonly".to_string(),
            "https://www.googleapis.com/auth/calendar.events.readonly".to_string(),
            "https://www.googleapis.com/auth/calendar.settings.readonly".to_string(),
        ]);

        // TODO replace by runnng a background server to capture the response?
        let buffer = prompt(
            Some(&format!(
                "Please open the following Url: {user_consent_url}"
            )),
            ".. and paste the response URL below:",
        );
        let mut params: HashMap<String, String> = HashMap::new();
        url::Url::parse(&buffer)
            .expect("a valid url")
            .query_pairs()
            .filter(|(k, _v)| k == "code" || k == "state")
            .for_each(|(k, v)| {
                params.insert(k.into_owned(), v.into_owned());
            });
        // println!("Params: {:?}", params);

        // In your redirect URL capture the code sent and our state.
        // Send it along to the request for the token.
        let code = params.get("code").expect("A code to be available");
        let state = params.get("state").expect("A state to be available");
        let access_token = gcal.get_access_token(code, state).await.unwrap();
        // println!(
        //     "Refresh Token: {:?} expires_in: {:?}\nAccess Token: {:?} expires_in: {:?}",
        //     access_token.refresh_token,
        //     access_token.refresh_token_expires_in,
        //     access_token.access_token,
        //     access_token.expires_in
        // );

        if !access_token.refresh_token.eq("") {
            cfg.refresh_token = Some(access_token.refresh_token);
            confy::store("momentary_toil", None, cfg.clone()).unwrap();
        }
    } else if !refresh.eq("") {
        // You can additionally refresh the access token with the following.
        // You must have a refresh token to be able to call this function.
        let access_token = gcal.refresh_access_token().await.unwrap();
        // println!(
        //     "Refresh Token: {:?} expires_in: {:?}\nAccess Token: {:?} expires_in: {:?}",
        //     access_token.refresh_token,
        //     access_token.refresh_token_expires_in,
        //     access_token.access_token,
        //     access_token.expires_in
        // );
        if !access_token.refresh_token.eq("") {
            cfg.refresh_token = Some(access_token.refresh_token);
            confy::store("momentary_toil", None, cfg.clone()).unwrap();
        }
    }

    let e = Events::new(gcal);
    let events = e
        .list_all(
            "primary",
            "",
            0,
            google_calendar::types::OrderBy::StartTime,
            &[String::from(""); 0],
            "",
            &[String::from(""); 0],
            false,
            false,
            true,
            &end.to_rfc3339(),
            &start.to_rfc3339(),
            "",
            "",
        )
        .await
        .expect("A list of events");

    let total_duration = events
        .body
        .iter()
        .filter(|e| {
            !e.event_type.eq("workingLocation")
                && e.start.as_ref().is_some_and(|s| s.date.is_none())
                && e.end.as_ref().is_some_and(|e| e.date.is_none())
        })
        .fold(0, |acc, e| {
            if let (Some(start), Some(end)) = (
                &e.start.as_ref().and_then(|s| s.date_time),
                &e.end.as_ref().and_then(|e| e.date_time),
            ) {
                // println!("{:?}", e.summary);
                let duration = end.signed_duration_since(start);
                let minutes = duration.num_minutes();
                acc + minutes
            } else {
                acc
            }
        });
    let meeting_hours = total_duration as f32 / 60.0;
    let full_week = 40.0;
    println!(
        "  Total meeting time this week: {:.2} / {:.2} [{:.2}%]\n  Non-Meeting time: {:.2}",
        meeting_hours,
        full_week,
        meeting_hours * 100.0 / full_week,
        full_week - meeting_hours,
    );
}
