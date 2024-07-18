#[macro_use]
extern crate rocket;

use chrono::prelude::*;
use futures::executor::block_on;
use google_calendar::{events::Events, Client};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, io, sync::Mutex, thread, time};
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread::sleep;
use std::time::Duration;
use rocket::{Rocket, State};
use tokio::task::spawn_blocking;

#[derive(Serialize, Deserialize, Default, Clone)]
struct ToilConfig {
    client_id: Option<String>,
    client_secret: Option<String>,
    redirect_uri: Option<String>,
    refresh_token: Option<String>,
}

impl ToilConfig {
    fn new() -> Self {
        let mut cfg: ToilConfig = confy::load("momentary_toil", None).unwrap();

        if cfg.client_id.is_none() {
            cfg.client_id = Some(get_user_input(None, "Enter your client_id: "));
        }
        if cfg.client_secret.is_none() {
            cfg.client_secret = Some(get_user_input(None, "Enter your client_secret: "));
        }
        if cfg.redirect_uri.is_none() {
            cfg.redirect_uri = Some(get_user_input(None, "Enter your redirect_uri: "));
        }
        confy::store("momentary_toil", None, cfg.clone()).unwrap();
        cfg
    }
}

static HANDLE: Lazy<Mutex<Option<CallbackData>>> = Lazy::new(Default::default);

#[derive(Clone, Debug)]
struct CallbackData {
    state: String,
    code: String,
}

struct MyState {
    tx: Sender<CallbackData>,
}

#[get("/callback?<state>&<code>", format = "*/*")]
fn callback(code: &str, state: &str, st: &State<MyState>) -> &'static str {
    let callback_data = CallbackData {
        state: state.to_string(),
        code: code.to_string(),
    };

    st.tx.send(callback_data).unwrap();

    "Thank you, you may now close this window."
}



#[tokio::main]
// #[launch]
async fn main() {
    let (tx, rx) = mpsc::channel();

    let mut cfg = ToilConfig::new();

    let start_week = start_of_week();
    let end_week = end_of_week();

    println!(
        "Week:: {:?} -> {:?}",
        start_week.date_naive(),
        end_week.date_naive()
    );

    let join_handle =
        tokio::spawn(async move { rocket::build().manage(MyState {tx}).mount("/", routes![callback]).launch().await });

    block_on(do_call(rx, &mut cfg, start_week, end_week));
    // let _ = join_handle.await;
}

fn end_of_week() -> DateTime<Utc> {
    day_of_week(Weekday::Sat)
}

fn start_of_week() -> DateTime<Utc> {
    day_of_week(Weekday::Mon)
}

fn day_of_week(weekday: Weekday) -> DateTime<Utc> {
    let current_week = Utc::now().iso_week();
    let result =
        NaiveDate::from_isoywd_opt(current_week.year(), current_week.week(), weekday)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .unwrap();
    Utc.from_local_datetime(&result).single().unwrap()
}

/// A function to display messages and get user input.
/// It accepts an optional instruction and a mandatory message.
/// If an instruction is given, it's displayed before the message.
/// After displaying the messages, it takes user input,
/// trims it down and returns it.
fn get_user_input(optional_instruction: Option<&str>, mandatory_message: &str) -> String {
    if let Some(instruction) = optional_instruction {
        println!("{}", instruction);
    }
    println!("{}", mandatory_message);
    let mut input_buffer = String::new();
    io::stdin()
        .read_line(&mut input_buffer)
        .expect("Failed to read from stdin");
    input_buffer.trim().to_string()
}
async fn do_call<'a>(rx: Receiver<CallbackData>, cfg: &mut ToilConfig, start: DateTime<Utc>, end: DateTime<Utc>) {
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

        // launch a server to receive the response...
        println!("Server started!");
        println!(
            "Please open the following Url: {user_consent_url}"
        );

        // In your redirect URL capture the code sent and our state.
        // Send it along to the request for the token.
        let result = rx.recv().unwrap();
        let code = result.code;
        let state = result.state;
        let access_token = gcal.get_access_token(&code, &state).await.unwrap();
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
