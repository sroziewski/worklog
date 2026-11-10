use std::path::PathBuf;

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate};
use clap::{Parser, Subcommand};
use worklog::config::{self, Config};
use worklog::jira::Jira;
use worklog::plan::Bucket;
use worklog::report::{self, fmt_minutes};
use worklog::flow::{self, Gate};
use worklog::{schedule, state, ui};

#[derive(Parser)]
#[command(version, about = "Suggests Jira worklogs from the day's git activity")]
struct Cli {
    /// Config file (default: <config dir>/worklog/config.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Show every git command and Jira request with its duration (always in the log file).
    #[arg(short, long, global = true)]
    verbose: bool,
    /// No progress output on the console (the log file still gets everything).
    #[arg(short, long, global = true, conflicts_with = "verbose")]
    quiet: bool,
    /// Print the hours logged in Jira for the current working week (Mon–Fri) per day and issue,
    /// against the workday target. Read-only.
    #[arg(long)]
    check_weekly: bool,
    /// With --check-weekly: a day inside the week to check (default: today).
    #[arg(long, requires = "check_weekly")]
    date: Option<NaiveDate>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print the suggestions for a day without logging anything.
    Suggest {
        #[arg(long)]
        date: Option<NaiveDate>,
    },
    /// Open the Accept/Deny window for a day (default: today). It stays closed when Jira already
    /// shows a full workday or the day was denied. The scheduled run instead asks about the latest
    /// weekday whose popup_time has passed, to catch up a missed day.
    Popup {
        #[arg(long)]
        date: Option<NaiveDate>,
        /// Open the window even when the day is full or was denied.
        #[arg(long)]
        force: bool,
    },
    /// Schedule the popup Mon-Fri at `popup_time` (Task Scheduler / systemd user timer).
    Install,
    Uninstall,
    /// Print the effective configuration and where it came from.
    Config,
}

impl Cmd {
    fn name(&self) -> &'static str {
        match self {
            Cmd::Suggest { .. } => "suggest",
            Cmd::Popup { .. } => "popup",
            Cmd::Install => "install",
            Cmd::Uninstall => "uninstall",
            Cmd::Config => "config",
        }
    }
}

fn main() -> Result<()> {
    let res = run();
    // Errors end the run; put them in the log file too, not only on the console.
    if let Err(e) = &res {
        log::error!("{e:#}");
    }
    res
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    // clap's args_conflicts_with_subcommands would also forbid the global --config/-v/-q before
    // a subcommand, so only this pairing is rejected, by hand, with clap's usage-error exit.
    if cli.check_weekly
        && let Some(cmd) = &cli.cmd
    {
        use clap::CommandFactory;
        Cli::command()
            .error(clap::error::ErrorKind::ArgumentConflict, format!("the subcommand '{}' cannot be used with '--check-weekly'", cmd.name()))
            .exit();
    }
    let level = match (cli.verbose, cli.quiet) {
        (true, _) => Some(log::LevelFilter::Debug),
        (_, true) => Some(log::LevelFilter::Warn),
        _ => Some(log::LevelFilter::Info),
    };
    let log_path = worklog::logging::init(level);
    log::debug!("worklog {} {:?}, log file {log_path:?}", env!("CARGO_PKG_VERSION"), std::env::args().skip(1).collect::<Vec<_>>());
    let cfg_path = cli.config.clone().or_else(Config::default_path);
    let cfg = Config::load(cli.config.as_deref())?;
    if cli.check_weekly {
        return check_weekly(&cfg, cli.date.unwrap_or_else(|| Local::now().date_naive()));
    }
    let Some(cmd) = cli.cmd else {
        use clap::CommandFactory;
        Cli::command().print_help()?;
        return Ok(());
    };
    match cmd {
        Cmd::Suggest { date } => suggest(&cfg, date.unwrap_or_else(|| Local::now().date_naive())),
        Cmd::Popup { date, force } => worklog_popup(cfg, date, force),
        Cmd::Install => {
            let exe = popup_exe()?;
            println!("{}", schedule::install(&exe, cfg.popup_time_parsed()?)?);
            Ok(())
        }
        Cmd::Uninstall => {
            println!("{}", schedule::uninstall()?);
            Ok(())
        }
        Cmd::Config => {
            println!("# {}", cfg_path.map(|p| p.display().to_string()).unwrap_or_default());
            println!("{cfg:#?}");
            let creds = config::jira_credentials(&cfg, |k| std::env::var(k).ok());
            println!("jira: {creds:?}");
            Ok(())
        }
    }
}

fn worklog_popup(cfg: Config, date: Option<NaiveDate>, force: bool) -> Result<()> {
    let date = date.unwrap_or_else(|| Local::now().date_naive());
    let state_path = state::default_path();
    let state = state_path.as_deref().map(state::State::load).unwrap_or_default();
    let creds = config::jira_credentials(&cfg, |k| std::env::var(k).ok())?;
    let jira = Jira::new(&creds.url, &creds.token);
    if let Gate::Skip(why) = report::gate(&cfg, &jira, date, force, &state) {
        log::debug!("{date}: not opening — {why}");
        println!("{date}: not opening — {why}");
        return Ok(());
    }
    ui::run(cfg, creds, date, state_path)
}

fn check_weekly(cfg: &Config, date: NaiveDate) -> Result<()> {
    let creds = config::jira_credentials(cfg, |k| std::env::var(k).ok())?;
    let jira = Jira::new(&creds.url, &creds.token);
    let me = jira.myself()?;
    let week = flow::week_of(date);
    let entries = jira.logged_between(&me, week[0], week[6])?;
    let today = Local::now().date_naive();
    let s = flow::week_summary(&entries, &week, today, cfg.workday_minutes);
    print!("{}", flow::render_week(&s, &me, &week, today));
    Ok(())
}

/// The GUI-subsystem twin next to this executable, so the scheduled run opens no console.
fn popup_exe() -> Result<PathBuf> {
    let me = std::env::current_exe().context("locating own executable")?;
    let twin = me.with_file_name(format!("worklog-popup{}", std::env::consts::EXE_SUFFIX));
    anyhow::ensure!(twin.exists(), "{} not found; build both binaries (cargo build --release)", twin.display());
    Ok(twin)
}

fn suggest(cfg: &Config, date: NaiveDate) -> Result<()> {
    let creds = config::jira_credentials(cfg, |k| std::env::var(k).ok())?;
    let jira = Jira::new(&creds.url, &creds.token);
    let r = report::build(cfg, &jira, date)?;
    println!(
        "{} for {} — already logged {}, {} to fill",
        r.date,
        r.me,
        fmt_minutes(r.already_logged_minutes),
        fmt_minutes(r.budget_minutes)
    );
    for w in &r.warnings {
        println!("  ! {w}");
    }
    if !r.has_activity() {
        println!("no git activity");
        return Ok(());
    }
    for row in &r.rows {
        let s = &row.suggestion;
        let (key, title) = match (&s.bucket, &row.issue) {
            (Bucket::Issue(k), Some(i)) => (k.clone(), format!("{} [{}]", i.summary, i.status)),
            (Bucket::Issue(k), None) => (k.clone(), "(not found in Jira)".into()),
            (Bucket::Unassigned(repo), _) => ("?".into(), format!("no ticket key in {repo}")),
        };
        println!("\n{key:<12} {:>7}  (raw {})  {title}", fmt_minutes(s.minutes), fmt_minutes(s.raw_minutes));
        for e in s.evidence.iter().take(8) {
            println!("{:14}- {e}", "");
        }
        if s.evidence.len() > 8 {
            println!("{:14}… {} more", "", s.evidence.len() - 8);
        }
    }
    Ok(())
}
