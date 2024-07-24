#[macro_use]
extern crate rocket;

use chrono::prelude::*;
use clap::Parser;
use futures::executor::block_on;
use google_calendar::{events::Events, Client};
use rocket::figment::Figment;
use rocket::State;
use serde::{Deserialize, Serialize};
use std::io;
use std::string::ToString;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};

use ratatui::{
    buffer::Buffer,
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind},
    layout::{Alignment, Rect},
    style::Stylize,
    symbols::border,
    text::{Line, Text},
    widgets::{
        block::{Position, Title},
        Block, Paragraph, Widget,
    },
    Frame,
};

mod tui;

const REQUIRED_SCOPES: [&str; 3] = [
    "https://www.googleapis.com/auth/calendar.readonly",
    "https://www.googleapis.com/auth/calendar.events.readonly",
    "https://www.googleapis.com/auth/calendar.settings.readonly",
];
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

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value_t = false)]
    gui: bool,

    #[arg(short, long, default_value_t = false)]
    details: bool,

    #[arg(short, long, default_value_t = 1)]
    weeks: u8,
}

#[derive(Debug, Default)]
pub struct App {
    counter: u8,
    exit: bool,
}

impl App {
    /// runs the application's main loop until the user quits
    pub fn run(&mut self, terminal: &mut tui::Tui) -> io::Result<()> {
        while !self.exit {
            terminal.draw(|frame| self.render_frame(frame))?;
            self.handle_events()?;
        }
        Ok(())
    }

    fn render_frame(&self, frame: &mut Frame) {
        frame.render_widget(self, frame.size());
    }

    fn handle_events(&mut self) -> io::Result<()> {
        match event::read()? {
            // it's important to check that the event is a key press event as
            // crossterm also emits key release and repeat events on Windows.
            Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                self.handle_key_event(key_event)
            }
            _ => {}
        };
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Char('q') => self.exit(),
            KeyCode::Left => self.decrement_counter(),
            KeyCode::Right => self.increment_counter(),
            _ => {}
        }
    }

    fn exit(&mut self) {
        self.exit = true;
    }

    fn increment_counter(&mut self) {
        self.counter += 1;
    }

    fn decrement_counter(&mut self) {
        self.counter -= 1;
    }
}

impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title = Title::from(" Counter App Tutorial ".bold());
        let instructions = Title::from(Line::from(vec![
            " Decrement ".into(),
            "<Left>".blue().bold(),
            " Increment ".into(),
            "<Right>".blue().bold(),
            " Quit ".into(),
            "<Q> ".blue().bold(),
        ]));
        let block = Block::bordered()
            .title(title.alignment(Alignment::Center))
            .title(
                instructions
                    .alignment(Alignment::Center)
                    .position(Position::Bottom),
            )
            .border_set(border::THICK);

        let counter_text = Text::from(vec![Line::from(vec![
            "Value: ".into(),
            self.counter.to_string().yellow(),
        ])]);

        Paragraph::new(counter_text)
            .centered()
            .block(block)
            .render(area, buf);
    }
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let args = Args::parse();

    let mut terminal = tui::init()?;
    let app_result = App::default().run(&mut terminal);
    tui::restore()?;

    // Start: should move to in ratatui state?
    let (tx, rx) = mpsc::channel();

    let mut cfg = ToilConfig::new();

    let start_week = start_of_week();
    let end_week = end_of_week();
    // cfg.client_id.get_or_insert_with(|| prompt(None, "Enter your client_id: "));
    // cfg.client_secret.get_or_insert_with(|| prompt(None, "Enter your client_secret: "));
    // cfg.redirect_uri.get_or_insert_with(|| prompt(None, "Enter your redirect_uri: "));
    // confy::store("momentary_toil", None, cfg.clone()).unwrap();

    println!(
        "Week:: {:?} -> {:?}",
        start_week.date_naive(),
        end_week.date_naive()
    );

    let _join_handle = tokio::spawn(async move {
        rocket::custom(Figment::from(rocket::Config::default()).join(("log_level", "off")))
            .manage(MyState { tx })
            .mount("/", routes![callback])
            .launch()
            .await
    });

    block_on(do_call(rx, &mut cfg, &args, start_week, end_week));

    // End: should move to in ratatui state?

    app_result
}

fn end_of_week() -> DateTime<Utc> {
    day_of_week(Weekday::Sat)
}

fn start_of_week() -> DateTime<Utc> {
    day_of_week(Weekday::Mon)
}

fn day_of_week(weekday: Weekday) -> DateTime<Utc> {
    let current_week = Utc::now().iso_week();
    let result = NaiveDate::from_isoywd_opt(current_week.year(), current_week.week(), weekday)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .unwrap();
    Utc.from_local_datetime(&result)
        .single()
        .expect("A valid date for the week")
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
async fn do_call<'a>(
    rx: Receiver<CallbackData>,
    cfg: &mut ToilConfig,
    args: &Args,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) {
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
        let scopes: [String; 3] = REQUIRED_SCOPES
            .into_iter()
            .map(String::from)
            .collect::<Vec<String>>()
            .try_into()
            .unwrap();
        let user_consent_url = gcal.user_consent_url(&scopes);

        // launch a server to receive the response...
        println!("Server started!");
        println!("Please open the following Url: {user_consent_url}");

        // In your redirect URL capture the code sent and our state.
        // Send it along to the request for the token.
        let result = rx.recv().unwrap();
        let code = result.code;
        let state = result.state;
        let access_token = gcal.get_access_token(&code, &state).await.unwrap();

        if !access_token.refresh_token.eq("") {
            cfg.refresh_token = Some(access_token.refresh_token);
            confy::store("momentary_toil", None, cfg.clone()).unwrap();
        }
    } else if !refresh.eq("") {
        // You can additionally refresh the access token with the following.
        // You must have a refresh token to be able to call this function.
        let access_token = gcal.refresh_access_token().await.unwrap();

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
            &[],
            "",
            &[],
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

    let mut summary: Vec<String> = Vec::new();
    let total_duration: i64 = events
        .body
        .iter()
        .filter(|e| {
            !e.event_type.eq("workingLocation")
                && e.start.as_ref().map(|s| s.date.is_none()).unwrap_or(false)
                && e.end.as_ref().map(|e| e.date.is_none()).unwrap_or(false)
        })
        .map(|e| {
            if let (Some(start), Some(end)) = (
                e.start.as_ref().and_then(|s| s.date_time),
                e.end.as_ref().and_then(|e| e.date_time),
            ) {
                let duration = end.signed_duration_since(start);
                let attendee = e
                    .attendees
                    .iter()
                    .find(|a| a.self_ && a.response_status != "declined");

                let start_hour = e
                    .start
                    .as_ref()
                    .and_then(|s| s.date_time.map(|d| d.hour()))
                    .unwrap_or(0);
                let end_hour = e
                    .end
                    .as_ref()
                    .and_then(|s| s.date_time.map(|d| d.hour()))
                    .unwrap_or(0);
                if (start_hour < 18 && end_hour > 7)
                    && (attendee.is_some() || e.organizer.as_ref().map_or(false, |o| o.self_))
                {
                    summary.push(format!(
                        "{} {}: [{:.2}mins] {}",
                        start.weekday(),
                        start.time(),
                        duration.num_minutes(),
                        e.summary,
                    ));
                    duration.num_minutes()
                } else {
                    0
                }
            } else {
                0
            }
        })
        .sum();

    let work_hours_per_week = 40.0;
    let meeting_hours = total_duration as f32 / 60.0;
    let non_meeting_hours = work_hours_per_week - meeting_hours;
    let meeting_percentage = meeting_hours * 100.0 / work_hours_per_week;

    println!("  Total meeting time this week: {meeting_hours:.2} / {work_hours_per_week:.2} [{meeting_percentage:.2}%]");
    println!("  Non-Meeting time: {non_meeting_hours:.2}");
    if args.details {
        for s in summary {
            println!("    > {}", s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    #[test]
    fn tests_available() {
        assert!(true);
    }

    #[test]
    fn render() {
        let app = App::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 50, 4));

        app.render(buf.area, &mut buf);

        let mut expected = Buffer::with_lines(vec![
            "┏━━━━━━━━━━━━━ Counter App Tutorial ━━━━━━━━━━━━━┓",
            "┃                    Value: 0                    ┃",
            "┃                                                ┃",
            "┗━ Decrement <Left> Increment <Right> Quit <Q> ━━┛",
        ]);
        let title_style = Style::new().bold();
        let counter_style = Style::new().yellow();
        let key_style = Style::new().blue().bold();
        expected.set_style(Rect::new(14, 0, 22, 1), title_style);
        expected.set_style(Rect::new(28, 1, 1, 1), counter_style);
        expected.set_style(Rect::new(13, 3, 6, 1), key_style);
        expected.set_style(Rect::new(30, 3, 7, 1), key_style);
        expected.set_style(Rect::new(43, 3, 4, 1), key_style);

        // note ratatui also has an assert_buffer_eq! macro that can be used to
        // compare buffers and display the differences in a more readable way
        assert_eq!(buf, expected);
    }

    #[test]
    fn handle_key_event() -> io::Result<()> {
        let mut app = App::default();
        app.handle_key_event(KeyCode::Right.into());
        assert_eq!(app.counter, 1);

        app.handle_key_event(KeyCode::Left.into());
        assert_eq!(app.counter, 0);

        let mut app = App::default();
        app.handle_key_event(KeyCode::Char('q').into());
        assert_eq!(app.exit, true);

        Ok(())
    }
}