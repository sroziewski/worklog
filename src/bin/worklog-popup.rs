// The scheduled entry point: a GUI-subsystem binary, so Windows opens no console window.
#![windows_subsystem = "windows"]

use chrono::Local;
use worklog::config::{self, Config};
use worklog::flow::{self, Gate};
use worklog::jira::Jira;
use worklog::report;
use worklog::state::{self, State};

fn main() {
    worklog::logging::init(None);
    log::info!("scheduled popup started");
    let result = Config::load(None).and_then(|cfg| {
        let date = flow::due_date(Local::now().naive_local(), cfg.popup_time_parsed()?);
        let state_path = state::default_path();
        let state = state_path.as_deref().map(State::load).unwrap_or_default();
        let creds = config::jira_credentials(&cfg, |k| std::env::var(k).ok())?;
        let jira = Jira::new(&creds.url, &creds.token);
        if let Gate::Skip(why) = report::gate(&cfg, &jira, date, false, &state) {
            log::info!("{date}: not asking — {why}");
            return Ok(());
        }
        log::info!("asking about {date}");
        worklog::ui::run(cfg, creds, date, state_path)
    });
    if let Err(e) = result {
        log::error!("{e:#}");
        // No console to print to: leave the reason where the user can find it.
        if let Some(dir) = dirs::config_dir() {
            let _ = std::fs::create_dir_all(dir.join("worklog"));
            let _ = std::fs::write(dir.join("worklog").join("last-error.txt"), format!("{} {e:#}\n", Local::now()));
        }
        std::process::exit(1);
    }
}
