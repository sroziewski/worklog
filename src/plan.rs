use std::collections::BTreeMap;

use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeDelta};

use crate::git::{Event, EventKind};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Bucket {
    Issue(String),
    /// Activity in this repo that names no ticket; the user picks one or it is not logged.
    Unassigned(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub bucket: Bucket,
    pub minutes: u32,
    /// Unscaled credit from the activity gaps, kept to show how the estimate came about.
    pub raw_minutes: u32,
    pub repos: Vec<String>,
    /// Commit subjects first, then checked-out branches and changed files; no duplicates.
    pub evidence: Vec<String>,
    /// How many leading `evidence` entries are commit subjects.
    pub commits: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct Params {
    pub budget_minutes: u32,
    pub round_minutes: u32,
    pub max_gap_minutes: u32,
    pub session_start_minutes: u32,
}

/// Credits each event with the time since the previous one (the work that led to it), or with
/// the session-start credit when it opens a session, sums per bucket, and scales the sums to the
/// budget in `round_minutes` steps. The budget is shared only among ticket rows when there are
/// any (a single ticket gets all of it), otherwise among the unassigned rows; the returned
/// minutes add up to the budget rounded down to a step, unless there is no activity at all.
pub fn allocate(events: &[Event], p: Params) -> Vec<Suggestion> {
    let mut sorted: Vec<&Event> = events.iter().collect();
    sorted.sort_by_key(|e| e.time);

    // seconds credited, repos, (evidence rank, evidence text)
    type Acc = (i64, Vec<String>, Vec<(u8, String)>);
    let mut raw: BTreeMap<Bucket, Acc> = BTreeMap::new();
    let mut prev: Option<&Event> = None;
    for e in sorted {
        let gap = prev.map(|pe| (e.time - pe.time).num_seconds());
        let credit = match gap {
            Some(g) if g <= i64::from(p.max_gap_minutes) * 60 => g,
            _ => i64::from(p.session_start_minutes) * 60,
        };
        prev = Some(e);
        let bucket = match &e.key {
            Some(k) => Bucket::Issue(k.clone()),
            None => Bucket::Unassigned(e.repo.clone()),
        };
        let entry = raw.entry(bucket).or_default();
        entry.0 += credit;
        if !entry.1.contains(&e.repo) {
            entry.1.push(e.repo.clone());
        }
        let rank = match e.kind {
            EventKind::Commit => 0,
            EventKind::Checkout => 1,
            EventKind::Uncommitted => 2,
        };
        let text = match e.kind {
            EventKind::Commit => e.what.clone(),
            EventKind::Checkout => format!("checkout {}", e.what),
            EventKind::Uncommitted => format!("uncommitted {}", e.what),
        };
        if !entry.2.iter().any(|(_, t)| *t == text) {
            entry.2.push((rank, text));
        }
    }

    let mut items: Vec<Suggestion> = raw
        .into_iter()
        .map(|(bucket, (secs, repos, mut ev))| {
            ev.sort_by_key(|(rank, _)| *rank); // stable: keeps time order within a kind
            Suggestion {
                bucket,
                minutes: 0,
                raw_minutes: u32::try_from(secs / 60).unwrap_or(u32::MAX),
                repos,
                commits: ev.iter().filter(|(rank, _)| *rank == 0).count(),
                evidence: ev.into_iter().map(|(_, t)| t).collect(),
            }
        })
        .collect();

    // The budget goes to rows with a ticket: one ticket gets the whole day. Rows without one start
    // at zero for the user to fill in after picking a ticket, unless no activity names a ticket.
    let any_issue = items.iter().any(|s| matches!(s.bucket, Bucket::Issue(_)));
    let shares = |s: &Suggestion| !any_issue || matches!(s.bucket, Bucket::Issue(_));
    let idx: Vec<usize> = (0..items.len()).filter(|&i| shares(&items[i])).collect();
    let weights: Vec<u64> = idx.iter().map(|&i| u64::from(items[i].raw_minutes)).collect();
    let shared = distribute(&weights, p.budget_minutes / p.round_minutes.max(1));
    let mut units = vec![0; items.len()];
    for (&i, u) in idx.iter().zip(shared) {
        units[i] = u;
    }
    for (s, u) in items.iter_mut().zip(units) {
        s.minutes = u * p.round_minutes;
    }
    items.sort_by(|a, b| b.minutes.cmp(&a.minutes).then_with(|| a.bucket.cmp(&b.bucket)));
    items
}

/// Splits `units` proportionally to `weights` (largest remainder). Every item with work gets at
/// least one unit while there are enough units to go round, taken from the largest share.
/// All weights zero (activity without measurable gaps): split evenly.
pub fn distribute(weights: &[u64], units: u32) -> Vec<u32> {
    let n = weights.len();
    if n == 0 || units == 0 {
        return vec![0; n];
    }
    let total: u64 = weights.iter().sum();
    let weights: Vec<u64> = if total == 0 { vec![1; n] } else { weights.to_vec() };
    let total: u64 = weights.iter().sum();

    let units64 = u64::from(units);
    let mut out: Vec<u32> = weights.iter().map(|w| u32::try_from(w * units64 / total).unwrap_or(units)).collect();
    let mut rest = units - out.iter().sum::<u32>();
    let mut order: Vec<usize> = (0..n).collect();
    // Largest fractional part first; ties go to the heavier item, then the earlier one.
    order.sort_by(|&a, &b| {
        let fa = weights[a] * units64 % total;
        let fb = weights[b] * units64 % total;
        fb.cmp(&fa).then(weights[b].cmp(&weights[a])).then(a.cmp(&b))
    });
    for &i in order.iter().cycle() {
        if rest == 0 {
            break;
        }
        out[i] += 1;
        rest -= 1;
    }

    if units as usize >= n {
        while let Some(z) = out.iter().position(|&u| u == 0) {
            let donor = (0..n).max_by_key(|&i| (out[i], std::cmp::Reverse(i))).unwrap();
            out[donor] -= 1;
            out[z] += 1;
        }
    }
    out
}

/// Back-to-back start times from `day_start`, after the minutes already logged today, so the
/// new worklogs neither overlap each other nor sit on top of earlier ones.
pub fn start_times(date: NaiveDate, day_start: NaiveTime, already_logged_minutes: u32, durations: &[u32]) -> Vec<NaiveDateTime> {
    let mut t = date.and_time(day_start) + TimeDelta::minutes(i64::from(already_logged_minutes));
    durations
        .iter()
        .map(|&d| {
            let s = t;
            t += TimeDelta::minutes(i64::from(d));
            s
        })
        .collect()
}

/// Worklog comment: the commit subjects (other evidence when there are none), at most ten lines.
pub fn comment(s: &Suggestion) -> String {
    const MAX: usize = 10;
    let source: &[String] = if s.commits > 0 { &s.evidence[..s.commits.min(s.evidence.len())] } else { &s.evidence };
    let mut out: String = source.iter().take(MAX).map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n");
    if source.len() > MAX {
        out.push_str(&format!("\n… and {} more", source.len() - MAX));
    }
    if out.is_empty() {
        out = format!("Work in {}", s.repos.join(", "));
    }
    out
}
