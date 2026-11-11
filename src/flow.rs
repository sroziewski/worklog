//! The decisions behind the popup, kept free of GUI types so they can be tested: which day a
//! run asks about, whether it asks at all, which rows Accept posts and when they start.

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Weekday};

use crate::plan;
use crate::state::Decision;

fn is_workday(d: NaiveDate) -> bool {
    !matches!(d.weekday(), Weekday::Sat | Weekday::Sun)
}

/// The day a scheduled run reports on: the latest Mon–Fri whose `popup_time` has passed. A run
/// caught up after a missed start (machine off at 17:00, started next morning) thus asks about
/// the day that was missed, not about the new day that has barely begun.
pub fn due_date(now: NaiveDateTime, popup_time: NaiveTime) -> NaiveDate {
    let mut d = now.date();
    if now.time() < popup_time {
        d -= TimeDelta::days(1);
    }
    while !is_workday(d) {
        d -= TimeDelta::days(1);
    }
    d
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    Ask,
    /// Why the window stays closed, for the console and the log.
    Skip(String),
}

/// Whether to open the window for a day. Jira's total decides, not an earlier Accept: a day
/// accepted with 4h of 8h is still open. An explicit Deny is respected; `force` overrides all.
/// `logged_minutes` is `None` when Jira could not be asked, which opens the window rather than
/// silently skipping a day.
pub fn prompt_gate(decision: Option<Decision>, logged_minutes: Option<u32>, workday_minutes: u32, force: bool) -> Gate {
    if force {
        return Gate::Ask;
    }
    if decision == Some(Decision::Denied) {
        return Gate::Skip("you denied this day; use --force to ask again".into());
    }
    match logged_minutes {
        Some(m) if m >= workday_minutes => {
            Gate::Skip(format!("{} already logged (workday {}); use --force to ask anyway", crate::report::fmt_minutes(m), crate::report::fmt_minutes(workday_minutes)))
        }
        _ => Gate::Ask,
    }
}

/// Monday to Sunday of the ISO week holding `date`.
pub fn week_of(date: NaiveDate) -> Vec<NaiveDate> {
    let monday = date - TimeDelta::days(i64::from(date.weekday().num_days_from_monday()));
    (0..7).map(|i| monday + TimeDelta::days(i)).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct DayLine {
    pub date: NaiveDate,
    pub logged: u32,
    /// 0 for weekend days.
    pub target: u32,
    /// Workday not yet over (after `today`): its gap isn't counted as missing.
    pub future: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WeekSummary {
    /// Mon–Fri always; Sat/Sun only when something was logged on them.
    pub days: Vec<DayLine>,
    /// Minutes per issue key over the week, most first, then by key.
    pub per_issue: Vec<(String, u32)>,
    pub total: u32,
    /// Target of the workdays up to and including `today`.
    pub target_so_far: u32,
    pub target_week: u32,
    /// Sum of the shortfalls of past and current workdays (overtime on one day doesn't cover
    /// another).
    pub missing_so_far: u32,
}

/// Rolls worklogs `(date, key, minutes)` up into a week report. Entries outside `week` are
/// ignored.
pub fn week_summary(entries: &[(NaiveDate, String, u32)], week: &[NaiveDate], today: NaiveDate, workday_minutes: u32) -> WeekSummary {
    let in_week: Vec<&(NaiveDate, String, u32)> = entries.iter().filter(|(d, _, _)| week.contains(d)).collect();
    let mut days = Vec::new();
    let (mut target_so_far, mut target_week, mut missing_so_far) = (0, 0, 0);
    for &d in week {
        let logged: u32 = in_week.iter().filter(|(e, _, _)| *e == d).map(|(_, _, m)| m).sum();
        let target = if is_workday(d) { workday_minutes } else { 0 };
        let future = d > today;
        target_week += target;
        if !future {
            target_so_far += target;
            missing_so_far += target.saturating_sub(logged);
        }
        if is_workday(d) || logged > 0 {
            days.push(DayLine { date: d, logged, target, future });
        }
    }
    let mut per_issue: Vec<(String, u32)> = Vec::new();
    for (_, key, m) in &in_week {
        match per_issue.iter_mut().find(|(k, _)| k == key) {
            Some((_, sum)) => *sum += m,
            None => per_issue.push((key.clone(), *m)),
        }
    }
    per_issue.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let total = in_week.iter().map(|(_, _, m)| m).sum();
    WeekSummary { days, per_issue, total, target_so_far, target_week, missing_so_far }
}

/// What the window knows about one row when Accept is pressed.
#[derive(Debug, Clone, PartialEq)]
pub struct RowInput {
    pub checked: bool,
    pub minutes: u32,
    /// The ticket to log on: the row's own key, or the one picked for an unassigned row.
    pub target: Option<String>,
    pub posted_ok: bool,
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PostJob {
    pub row: usize,
    pub key: String,
    pub started: NaiveDateTime,
    pub minutes: u32,
    pub comment: String,
}

pub fn postable(rows: &[RowInput]) -> Vec<usize> {
    (0..rows.len())
        .filter(|&i| {
            let r = &rows[i];
            r.checked && r.minutes > 0 && !r.posted_ok && r.target.is_some()
        })
        .collect()
}

/// Worklogs to post, back to back from `day_start` after what was logged before the window
/// opened and what an earlier Accept of this window already posted (a retry never re-posts those).
pub fn plan_posts(date: NaiveDate, day_start: NaiveTime, already_logged_minutes: u32, rows: &[RowInput]) -> Vec<PostJob> {
    let todo = postable(rows);
    let done: u32 = rows.iter().filter(|r| r.posted_ok).map(|r| r.minutes).sum();
    let durations: Vec<u32> = todo.iter().map(|&i| rows[i].minutes).collect();
    let starts = plan::start_times(date, day_start, already_logged_minutes + done, &durations);
    todo.into_iter()
        .zip(starts)
        .map(|(i, started)| PostJob {
            row: i,
            key: rows[i].target.clone().expect("postable rows have a target"),
            started,
            minutes: rows[i].minutes,
            comment: rows[i].comment.clone(),
        })
        .collect()
}

/// What to record after posting: `Accepted` once anything was logged, nothing when every post
/// failed. A record only; whether the window opens again is decided by `prompt_gate` from Jira.
pub fn decision_after_posting(any_posted_ok: bool) -> Option<Decision> {
    any_posted_ok.then_some(Decision::Accepted)
}

/// Applies every posting result already sent, without blocking. `results[row]` receives each
/// outcome and `remaining` counts down. Returns true when posting is over: all expected results
/// arrived, or the sender is gone (thread finished or died).
pub fn drain<T>(rx: &std::sync::mpsc::Receiver<(usize, T)>, results: &mut [Option<T>], remaining: &mut usize) -> bool {
    use std::sync::mpsc::TryRecvError;
    loop {
        match rx.try_recv() {
            Ok((row, res)) => {
                results[row] = Some(res);
                *remaining = remaining.saturating_sub(1);
            }
            Err(TryRecvError::Empty) => return *remaining == 0,
            Err(TryRecvError::Disconnected) => return true,
        }
    }
}

/// The project a row's work belongs to in the New ticket window: its ticket's project, `None`
/// for a row added by hand (it has no work), else its repo's project from `repo_projects`.
pub fn group_project(bucket: &crate::plan::Bucket, manual: bool, repo_projects: &std::collections::BTreeMap<String, String>) -> Option<String> {
    match bucket {
        crate::plan::Bucket::Issue(k) => Some(crate::jira::project_of(k).to_owned()),
        crate::plan::Bucket::Unassigned(_) if manual => None,
        crate::plan::Bucket::Unassigned(repo) => repo_projects.get(repo).cloned(),
    }
}

/// Current indices of the rows a created ticket was drafted from (`wanted`, by row id), in
/// `wanted` order; rows removed since are left out.
pub fn created_targets(ids: &[u64], wanted: &[u64]) -> Vec<usize> {
    wanted.iter().filter_map(|&id| row_index(ids, id)).collect()
}

/// A row's ticket-picker state, as far as a newly created ticket concerns it.
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    /// Index into the candidate list.
    pub picked: Option<usize>,
    pub checked: bool,
    pub posted_ok: bool,
    /// The row's activity names its ticket itself; its picker is unused.
    pub own_key: bool,
}

/// Puts a newly created ticket at the top of `candidates` and keeps every existing pick on the
/// same issue (indices shift by one). Each row in `targets` (the rows whose work the ticket was
/// drafted from) picks and ticks it, unless the row has a ticket of its own, already picked
/// one (a ticket created earlier, or the user's choice, is never overridden) or is logged.
/// Targets past `picks` are ignored.
pub fn apply_created(candidates: &mut Vec<crate::jira::Issue>, picks: &mut [Pick], issue: crate::jira::Issue, targets: &[usize]) {
    candidates.insert(0, issue);
    for p in picks.iter_mut() {
        if let Some(i) = &mut p.picked {
            *i += 1;
        }
    }
    for &t in targets {
        if let Some(p) = picks.get_mut(t)
            && !p.posted_ok
            && !p.own_key
            && p.picked.is_none()
        {
            p.picked = Some(0);
            p.checked = true;
        }
    }
}

/// Hands out row ids that are never reused, so a ticket requested from a removed row can't land
/// on a row added after it.
#[derive(Debug, Default)]
pub struct RowIds {
    next: u64,
}

impl RowIds {
    pub fn take(&mut self) -> u64 {
        let id = self.next;
        self.next += 1;
        id
    }
}

/// Current index of the row with `id`; `None` once that row is gone.
pub fn row_index(ids: &[u64], id: u64) -> Option<usize> {
    ids.iter().position(|&i| i == id)
}

/// The `--check-weekly` text: one line per shown day with logged / target and a note, the
/// per-issue totals, and the totals line.
pub fn render_week(s: &WeekSummary, me: &str, week: &[NaiveDate], today: NaiveDate) -> String {
    use crate::report::fmt_minutes;
    use std::fmt::Write;
    let mut out = String::new();
    if let (Some(first), Some(last)) = (week.first(), week.last()) {
        let _ = writeln!(out, "Week {first} – {last} for {me}\n");
    }
    for d in &s.days {
        let note = if !is_workday(d.date) {
            "weekend".to_owned()
        } else if d.future {
            "upcoming".to_owned()
        } else {
            match d.target.saturating_sub(d.logged) {
                0 => "ok".to_owned(),
                g => format!("missing {}", fmt_minutes(g)),
            }
        };
        let marker = if d.date == today { " ← today" } else { "" };
        let _ = writeln!(out, "  {} {}  {:>7} / {:<4} {note}{marker}", d.date.format("%a"), d.date, fmt_minutes(d.logged), fmt_minutes(d.target));
    }
    if !s.per_issue.is_empty() {
        let _ = writeln!(out, "\nBy issue:");
        for (key, m) in &s.per_issue {
            let _ = writeln!(out, "  {key:<14} {:>7}", fmt_minutes(*m));
        }
    }
    let _ = writeln!(
        out,
        "\nTotal {} · target so far {} (missing {}) · full week {}",
        fmt_minutes(s.total),
        fmt_minutes(s.target_so_far),
        fmt_minutes(s.missing_so_far),
        fmt_minutes(s.target_week)
    );
    out
}

/// A ticket saved from "My tickets": its new summary and status wherever the report shows it
/// (the pickers' list and the rows of that ticket).
pub fn apply_updated(report: &mut crate::report::Report, issue: &crate::jira::Issue) {
    for c in report.candidates.iter_mut().filter(|c| c.key == issue.key) {
        c.summary = issue.summary.clone();
        c.status = issue.status.clone();
    }
    for i in report.rows.iter_mut().filter_map(|r| r.issue.as_mut()).filter(|i| i.key == issue.key) {
        i.summary = issue.summary.clone();
        i.status = issue.status.clone();
    }
}

/// A day without git activity starts with one row to pick a ticket for, holding the time still
/// to log; a day already full gets none.
pub fn needs_starter_row(activity_rows: usize, budget_minutes: u32) -> bool {
    activity_rows == 0 && budget_minutes > 0
}

/// What the main panel shows below the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayView {
    /// Rows from git activity.
    Activity,
    /// No git activity: the rows added by hand, if any, and "+ Add row" to add (more).
    NoActivity,
    /// No rows and no time left to log: nothing to do.
    Full,
}

/// `activity_rows` of `rows` come from git; `unplanned_minutes` is the time no ticked row covers.
/// Removing every row by hand on a day with time left must still leave "+ Add row" reachable.
pub fn day_view(activity_rows: usize, rows: usize, unplanned_minutes: u32) -> DayView {
    if activity_rows > 0 {
        DayView::Activity
    } else if rows == 0 && unplanned_minutes == 0 {
        DayView::Full
    } else {
        DayView::NoActivity
    }
}
