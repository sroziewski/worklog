//! The New ticket window's "Work from" choice: the report's day (the main window's rows), or
//! the user's own work from a chosen moment up to now, read from git apart from the report.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, TimeZone};

use crate::config::Config;
use crate::git::{Collector, Event, Span};
use crate::keys::KeyMatcher;
use crate::plan::{self, Bucket, Params};
use crate::report;
use crate::ui_tickets::WorkGroup;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// The main window's rows: the report's day, worklog minutes and all.
    ReportDay,
    SinceYesterday,
    Last3Days,
    ThisWeek,
    Last7Days,
    Custom,
}

impl Preset {
    pub const ALL: [Preset; 6] = [Preset::ReportDay, Preset::SinceYesterday, Preset::Last3Days, Preset::ThisWeek, Preset::Last7Days, Preset::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Preset::ReportDay => "Report day",
            Preset::SinceYesterday => "Since yesterday",
            Preset::Last3Days => "Last 3 days",
            Preset::ThisWeek => "This week",
            Preset::Last7Days => "Last 7 days",
            Preset::Custom => "Custom…",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Preset::ReportDay => "the rows of the main window: the day the report is for",
            Preset::SinceYesterday => "from yesterday 00:00 until now",
            Preset::Last3Days => "from 00:00 two days ago until now (today and the two days before)",
            Preset::ThisWeek => "from Monday 00:00 of this week until now",
            Preset::Last7Days => "from 00:00 six days ago until now (today and the six days before)",
            Preset::Custom => "from a date and time you pick until now",
        }
    }
}

/// Where `preset` starts at `now`; `custom` is the user's own start. `None` for the report's day,
/// which is not a start but the main window's rows.
pub fn start_of(preset: Preset, now: NaiveDateTime, custom: NaiveDateTime) -> Option<NaiveDateTime> {
    let today = now.date();
    let midnight = |d: NaiveDate| d.and_time(NaiveTime::MIN);
    let days_back = |n: u64| midnight(today.checked_sub_days(chrono::Days::new(n)).unwrap_or(today));
    match preset {
        Preset::ReportDay => None,
        Preset::SinceYesterday => Some(days_back(1)),
        Preset::Last3Days => Some(days_back(2)),
        Preset::ThisWeek => Some(days_back(u64::from(today.weekday().num_days_from_monday()))),
        Preset::Last7Days => Some(days_back(6)),
        Preset::Custom => Some(custom),
    }
}

/// Why work cannot be read from `start`: it is not before `now`.
pub fn check_start(start: NaiveDateTime, now: NaiveDateTime) -> Result<(), String> {
    if start >= now { Err(format!("{} is not in the past; pick an earlier start", start.format("%a %Y-%m-%d %H:%M"))) } else { Ok(()) }
}

/// "since Mon 2026-11-16 09:00 (1 d 3 h ago)".
pub fn describe_start(start: NaiveDateTime, now: NaiveDateTime) -> String {
    let mins = (now - start).num_minutes().max(0);
    let (d, h, m) = (mins / (24 * 60), mins / 60 % 24, mins % 60);
    let ago = match (d, h) {
        (0, 0) => format!("{m} min"),
        (0, _) => format!("{h} h {m} min"),
        (_, 0) => format!("{d} d"),
        _ => format!("{d} d {h} h"),
    };
    format!("since {} ({ago} ago)", start.format("%a %Y-%m-%d %H:%M"))
}

/// The local instant of a wall-clock `start`; a time a DST jump skipped moves to the jump's end.
pub fn local(start: NaiveDateTime) -> DateTime<Local> {
    (0..=120)
        .find_map(|m| Local.from_local_datetime(&(start + TimeDelta::minutes(m))).earliest())
        .unwrap_or_else(|| Local.from_utc_datetime(&start))
}

/// Ids of the groups no main-window row stands for: far above the main window's own ids (which
/// count up from 0), so a created ticket's rows never name a main row by accident.
pub const RANGE_ROW_BASE: u64 = 1 << 48;

/// The title of a repo's work that names no ticket, in the main window and the New ticket window.
pub fn unassigned_title(repo: &str) -> String {
    format!("{repo} — no ticket yet")
}

/// A main-window row a range group may stand for: same bucket, its id and title.
#[derive(Debug, Clone, PartialEq)]
pub struct MainRow {
    pub bucket: Bucket,
    pub row: u64,
    pub title: String,
}

/// The events as the New ticket window lists them: one group per ticket key, and per repo for
/// work that names none, most worked-on first. A group whose bucket a main-window row also has
/// takes that row's id and title, so the row picks the ticket created from it; the others get
/// ids from `RANGE_ROW_BASE` up. "Most worked-on" is the activity time the report would credit
/// with `max_gap_minutes` and `session_start_minutes`; no minutes are allocated.
pub fn work_groups(events: &[Event], repo_projects: &BTreeMap<String, String>, main: &[MainRow], max_gap_minutes: u32, session_start_minutes: u32) -> Vec<WorkGroup> {
    let params = Params { budget_minutes: 0, round_minutes: 1, max_gap_minutes, session_start_minutes };
    let mut suggestions = plan::allocate(events, params);
    suggestions.sort_by(|a, b| b.raw_minutes.cmp(&a.raw_minutes).then_with(|| a.bucket.cmp(&b.bucket)));
    suggestions
        .into_iter()
        .enumerate()
        .map(|(i, s)| {
            let same = main.iter().find(|m| m.bucket == s.bucket);
            let title = match (same, &s.bucket) {
                (Some(m), _) => m.title.clone(),
                (None, Bucket::Issue(k)) => k.clone(),
                (None, Bucket::Unassigned(repo)) => unassigned_title(repo),
            };
            WorkGroup {
                row: same.map_or(RANGE_ROW_BASE + i as u64, |m| m.row),
                title,
                project: crate::flow::group_project(&s.bucket, false, repo_projects),
                repos: s.repos,
                lines: s.evidence,
            }
        })
        .collect()
}

/// What a scan of a span found.
#[derive(Debug, Clone, PartialEq)]
pub struct Scanned {
    pub span: Span,
    /// The wall-clock start the user chose, for display.
    pub start: NaiveDateTime,
    pub groups: Vec<WorkGroup>,
    pub repo_paths: BTreeMap<String, Vec<PathBuf>>,
    pub warnings: Vec<String>,
    /// Commits found (after the cross-repo dedup), and repos read.
    pub commits: usize,
    pub repos: usize,
}

/// Reads the user's work in `span` from the configured repos, the way the report reads a day
/// (same repos, author emails and disabled-project filter), with uncommitted files when
/// `include_uncommitted` (the span ends now).
pub fn scan(cfg: &Config, emails: &[String], start: NaiveDateTime, span: Span, include_uncommitted: bool, main: &[MainRow]) -> Scanned {
    let mut warnings = Vec::new();
    if emails.is_empty() {
        warnings.push("no author email configured and git user.email is unset; no commits can be matched".into());
    }
    let all_keys: Vec<String> = cfg.project_keys.iter().chain(&cfg.inactive_keys).cloned().collect();
    let matcher = KeyMatcher::new(&all_keys);
    let repos = report::active_repos(cfg, &mut warnings);
    let t = std::time::Instant::now();
    let collector = Collector { matcher: &matcher, author_emails: emails, span, include_uncommitted };
    let (mut events, errors) = collector.collect(&repos);
    warnings.extend(errors);
    report::drop_inactive(cfg, &mut events);
    let commits = events.iter().filter(|e| e.kind == crate::git::EventKind::Commit).count();
    log::info!("range scan {} – {}: {} events ({commits} commits) in {} repos, {:.2}s", span.from, span.to, events.len(), repos.len(), t.elapsed().as_secs_f64());
    let groups = work_groups(&events, &report::repo_projects(cfg, &repos), main, cfg.max_gap_minutes, cfg.session_start_minutes);
    Scanned { span, start, groups, repo_paths: report::repo_paths(&repos), warnings, commits, repos: repos.len() }
}
