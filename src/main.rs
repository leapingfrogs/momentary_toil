extern crate directories;
#[macro_use]
extern crate rocket;

use std::fs::create_dir_all;
use std::io;
use std::io::{stdin, stdout};
use std::path::{Path, PathBuf};
use std::string::ToString;
use std::sync::mpsc;
use std::sync::mpsc::Sender;

use chrono::prelude::*;
use clap::Parser;
use directories::ProjectDirs;
use figment::{Figment, providers::{Format, Serialized, Toml}};
use futures::executor::block_on;
use google_calendar::{Client, events::Events};
use mockall::automock;
use rocket::figment::Figment as RocketFigment;
use rocket::State;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Default, Clone)]
struct AccountConfig {
    name: String,
    refresh_token: Option<String>,
    default: bool,
}

#[derive(Serialize, Deserialize, Clone)]
struct ToilConfig {
    #[serde(skip)]
    args: Args,
    #[serde(skip)]
    config_path: PathBuf,
    client_id: Option<String>,
    client_secret: Option<String>,
    redirect_uri: Option<String>,
    refresh_token: Option<String>,
    // accounts: Vec<AccountConfig>,
}

impl Default for ToilConfig {
    fn default() -> Self {
        Self {
            args: Args::default(),
            config_path: PathBuf::default(),
            client_id: None,
            client_secret: None,
            redirect_uri: Some("http://localhost:8000/callback".to_string()),
            refresh_token: None,
        }
    }
}

impl ToilConfig {
    fn new(args: Args) -> Self
    {
        let config_path = match ProjectDirs::from("com", "leapingfrogs", "Momentary Toil") {
            Some(proj_dirs) => Path::join(proj_dirs.config_local_dir(), "Config2.toml"),
            None => PathBuf::from("Config.toml"),
        };

        ToilConfig {
            args,
            config_path: config_path.clone(),
            ..Figment::from(Serialized::defaults(ToilConfig::default()))
                .merge(Toml::file(config_path))
                .extract().expect("Valid configuration")
        }
    }

    fn save(&self) {
        if !cfg!(test) {
            if let Some(folder) = self.config_path.parent() {
                create_dir_all(folder).unwrap_or_else(|_| panic!("Config dir created: {:?}", self.config_path.clone()));
            }
            let _ = toml::to_string_pretty(&self).map(|toml| std::fs::write(self.config_path.clone(), &toml).unwrap_or_else(|_| panic!("Saved config ({:?})", self.config_path.clone())));
        }
    }
}


#[derive(Parser, Debug, Default, Clone)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value_t = false)]
    details: bool,

    #[arg(short, long, default_value_t = 1)]
    weeks: u8,

    #[arg(short, long, default_value = "default")]
    account_name: String,

    #[arg(short, long, default_value = None)]
    config: Option<String>,

    #[arg(long, default_value_t = true)]
    persist_configuration: bool,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let time_util = TimeUtil::new();
    let start = time_util.start_of_week();
    let end = time_util.end_of_week();

    let mut cfg = get_checked_configuration(
        args,
        Box::new(CmdLine::new(stdin().lock(), stdout())),
    );

    let events = block_on(retreive_events(&mut cfg, start, end));
    let (summary, total_duration_minutes) = build_results(events, &cfg);

    let work_hours_per_week = 40.0;
    let meeting_hours = total_duration_minutes as f32 / 60.0;
    let non_meeting_hours = work_hours_per_week - meeting_hours;
    let meeting_percentage = meeting_hours * 100.0 / work_hours_per_week;

    let mut helper = CmdLine::new(stdin().lock(), stdout());

    helper.output(format!("Week:: {:?} -> {:?}", start.date_naive(), end.date_naive()));

    helper.output(format!("  Total meeting time this week: {meeting_hours:.2} / {work_hours_per_week:.2} [{meeting_percentage:.2}%]"));
    helper.output(format!("  Non-Meeting time: {non_meeting_hours:.2}"));
    for s in summary {
        helper.output(format!("    > {}", s));
    }
}

fn build_results(events: Vec<ToilEvent>, cfg: &ToilConfig) -> (Vec<String>, i64) {
    let mut summary: Vec<String> = Vec::new();
    let total_duration: i64 = events.iter().map(|e| {
        let duration_minutes = e.end.signed_duration_since(e.start).num_minutes();
        if e.start.hour() < 18 && e.end.hour() > 7
        {
            if cfg.args.details {
                summary.push(format!(
                    "{} {}: [{:.2} minutes] {}",
                    e.start.weekday(),
                    e.start.time(),
                    duration_minutes,
                    e.summary,
                ));
            }
            duration_minutes
        } else {
            0
        }
    }).sum();
    (summary, total_duration)
}

fn get_checked_configuration(args: Args, mut helper: Box<dyn CmdLineHelper>) -> ToilConfig {
    let mut cfg =
        if cfg!(test) {
            Figment::from(Serialized::defaults(ToilConfig::default()))
                .extract().expect("Valid configuration")
        } else {
            ToilConfig::new(args)
        };
    if cfg.client_id.is_none() {
        cfg.client_id = helper.get_user_input("Enter your client_id: ").ok();
    }
    if cfg.client_secret.is_none() {
        cfg.client_secret = helper.get_user_input("Enter your client_secret: ").ok();
    }
    cfg.save();
    cfg
}

#[automock]
trait DateTimeProvider {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

struct TimeUtil {}

impl DateTimeProvider for TimeUtil {}

impl TimeUtil {
    fn new() -> Self {
        Self {}
    }
    fn end_of_week(&self) -> DateTime<Utc> {
        self.day_of_week(Weekday::Sat)
    }

    fn start_of_week(&self) -> DateTime<Utc> {
        self.day_of_week(Weekday::Mon)
    }

    fn day_of_week(&self, weekday: Weekday) -> DateTime<Utc> {
        let current_week = self.now().iso_week();
        let result = NaiveDate::from_isoywd_opt(current_week.year(), current_week.week(), weekday)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .unwrap();
        Utc.from_local_datetime(&result)
            .single()
            .expect("A valid date for the week")
    }
}

struct CmdLine<R, W> {
    reader: R,
    writer: W,
}

impl<R, W> CmdLine<R, W>
where
    R: io::BufRead,
    W: io::Write,
{
    fn output(&mut self, msg: String) {
        writeln!(&mut self.writer, "{}", msg).expect("should be able to write to stdout");
    }
}

impl<R, W> CmdLine<R, W>
where
    R: io::BufRead,
    W: io::Write,
{
    fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }
}

struct ToilEvent {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    summary: String,
}

#[automock]
trait CmdLineHelper {
    fn get_user_input(&mut self, mandatory_message: &str) -> io::Result<String>;
}
impl<R, W> CmdLineHelper for CmdLine<R, W>
where
    R: io::BufRead,
    W: io::Write,
{
    /// A function to display messages and get user input.
    /// It accepts an optional instruction and a mandatory message.
    /// If an instruction is given, it's displayed before the message.
    /// After displaying the messages, it takes user input,
    /// trims it down and returns it.
    fn get_user_input(&mut self, mandatory_message: &str) -> io::Result<String>
    where
        R: io::BufRead,
        W: io::Write,
    {
        writeln!(&mut self.writer, "{}", mandatory_message)?;
        let mut input_buffer = String::new();
        self.reader.read_line(&mut input_buffer)?;
        Ok(input_buffer.trim().to_string())
    }
}

async fn retreive_events<'a>(
    cfg: &mut ToilConfig,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Vec<ToilEvent>
{
    let (tx, rx) = mpsc::channel();
    let _join_handle = tokio::spawn(async move {
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

        rocket::custom(RocketFigment::from(rocket::Config::default()).join(("log_level", "off")))
            .manage(MyState { tx })
            .mount("/", routes![callback])
            .launch()
            .await
    });

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
        let scopes = [
            "https://www.googleapis.com/auth/calendar.readonly".to_string(),
            "https://www.googleapis.com/auth/calendar.events.readonly".to_string(),
            "https://www.googleapis.com/auth/calendar.settings.readonly".to_string(),
        ];
        let user_consent_url = gcal.user_consent_url(&scopes);

        // launch a server to receive the response...
        println!("Please open the following Url: {}", user_consent_url.replace(" ", "%20"));

        // In your redirect URL capture the code sent and our state.
        // Send it along to the request for the token.
        let result = rx.recv().unwrap();
        let code = result.code;
        let state = result.state;
        let access_token = gcal.get_access_token(&code, &state).await.unwrap();

        println!("Got token: {:?}", access_token);
        if !access_token.refresh_token.eq("") {
            cfg.refresh_token = Some(access_token.refresh_token);
            cfg.save();
        }
    } else if !refresh.eq("") {
        // You can additionally refresh the access token with the following.
        // You must have a refresh token to be able to call this function.
        let access_token = gcal.refresh_access_token().await.unwrap();

        if !access_token.refresh_token.eq("") {
            cfg.refresh_token = Some(access_token.refresh_token);
            cfg.save();
        }
    }

    let e = Events::new(gcal);
    let events = e
        .list_all("primary", "", 0, google_calendar::types::OrderBy::StartTime,
                  &[], "", &[], false, false,
                  true, &end.to_rfc3339(), &start.to_rfc3339(), "", "",
        )
        .await
        .expect("A list of events");

    events
        .body
        .iter()
        .filter(|e| {
            !e.event_type.eq("workingLocation")
                && e.start.as_ref().map(|s| s.date.is_none()).unwrap_or(false)
                && e.end.as_ref().map(|e| e.date.is_none()).unwrap_or(false)
                && (e.attendees.iter().any(|a| a.self_ && a.response_status != "declined") || e.organizer.as_ref().map_or(false, |o| o.self_))
        })
        .filter_map(|e| {
            match (
                e.start.as_ref().and_then(|x| x.date_time),
                e.end.as_ref().and_then(|x| x.date_time)
            ) {
                (Some(start), Some(end)) => Some(
                    ToilEvent { start, end, summary: e.summary.clone() }
                ),
                _ => None,
            }
        }).collect()
}

#[cfg(test)]
mod tests {
    use mockall::predicate;

    use super::*;

    #[test]
    fn test_get_checked_configuration() {
        let args = Args {
            details: false,
            weeks: 1,
            account_name: "default".to_string(),
            config: Some("test".to_string()),
            persist_configuration: false,
        };
        let mut mock = MockCmdLineHelper::new();
        mock.expect_get_user_input().with(predicate::eq("Enter your client_id: ")).times(1).returning(|_| Ok("client_id".to_string()));
        mock.expect_get_user_input().with(predicate::eq("Enter your client_secret: ")).times(1).returning(|_| Ok("client_secret".to_string()));

        let config = get_checked_configuration(args, Box::new(mock));
        assert_eq!(config.client_id, Some("client_id".to_string()));
        assert_eq!(config.client_secret, Some("client_secret".to_string()));
        assert_eq!(config.redirect_uri, Some("http://localhost:8000/callback".to_string()));
    }
}