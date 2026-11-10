use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{Local, NaiveDate};

use crate::config::Config;
use crate::git::{self, Collector};
use crate::jira::{Issue, Jira};
use crate::keys::KeyMatcher;
use crate::plan::{self, Bucket, Params, Suggestion};

#[derive(Debug, Clone)]
pub struct Row {
    pub suggestion: Suggestion,
    /// Jira's view of the key; `None` for unassigned rows and keys Jira does not know.
    pub issue: Option<Issue>,
    pub already_logged_minutes: u32,
}

#[derive(Debug, Clone)]
pub struct Report {
    pub date: NaiveDate,
    pub me: String,
    pub rows: Vec<Row>,
    pub already_logged_minutes: u32,
    pub budget_minutes: u32,
    pub candidates: Vec<Issue>,
    /// Problems that did not stop the report (unreadable repo, unknown key, ...).
    pub warnings: Vec<String>,
    /// Project of each scanned repo, by repo name (as in `Suggestion::repos`); repos no
    /// `repo_projects` root covers are missing.
    pub repo_projects: BTreeMap<String, String>,
    /// Every scanned repo by name (as in `Suggestion::repos`); one name may cover several repos.
    pub repo_paths: BTreeMap<String, Vec<std::path::PathBuf>>,
}

impl Report {
    pub fn has_activity(&self) -> bool {
        !self.rows.is_empty()
    }
}

pub fn build(cfg: &Config, jira: &Jira, date: NaiveDate) -> Result<Report> {
    build_at(cfg, jira, date, Local::now().date_naive(), git::global_user_email())
}

/// `build` with the clock and the git-config fallback email passed in: uncommitted files count
/// only when `date == today`, and `fallback_email` stands in when `author_emails` is empty.
pub fn build_at(cfg: &Config, jira: &Jira, date: NaiveDate, today: NaiveDate, fallback_email: Option<String>) -> Result<Report> {
    let started = std::time::Instant::now();
    log::info!("building suggestions for {date}");
    let me = jira.myself()?;
    log::info!("Jira user {me} ({:.2}s)", started.elapsed().as_secs_f64());
    let mut emails = cfg.author_emails.clone();
    if emails.is_empty() {
        emails.extend(fallback_email);
    }
    log::info!("author emails: {emails:?}");
    let mut warnings = Vec::new();
    if emails.is_empty() {
        warnings.push("no author email configured and git user.email is unset; no commits can be matched".into());
    }

    let matcher = KeyMatcher::new(&cfg.project_keys);
    let collector = Collector { matcher: &matcher, author_emails: &emails, date, include_uncommitted: date == today };
    let repos = git::discover_repos(&cfg.repo_roots);
    if repos.is_empty() {
        warnings.push(format!("no git repos found under {:?}", cfg.repo_roots));
    }
    let repo_projects = repo_projects(cfg, &repos);
    let mut repo_paths: BTreeMap<String, Vec<std::path::PathBuf>> = BTreeMap::new();
    for r in &repos {
        if let Some(name) = r.file_name() {
            repo_paths.entry(name.to_string_lossy().into_owned()).or_default().push(r.clone());
        }
    }
    log::info!("scanning {} repos under {:?} (uncommitted files: {})", repos.len(), cfg.repo_roots, date == today);
    let t = std::time::Instant::now();
    let (events, errors) = collector.collect(&repos);
    log::info!("git: {} events in {:.2}s", events.len(), t.elapsed().as_secs_f64());
    for e in &errors {
        log::warn!("{e}");
    }
    warnings.extend(errors);

    let t = std::time::Instant::now();
    let logged = jira.logged_on(&me, date)?;
    let already: u32 = logged.iter().map(|(_, m)| m).sum();
    log::info!("already logged on {date}: {already} min on {} issues ({:.2}s)", logged.len(), t.elapsed().as_secs_f64());
    let budget = cfg.workday_minutes.saturating_sub(already);
    let suggestions = plan::allocate(
        &events,
        Params {
            budget_minutes: budget,
            round_minutes: cfg.round_minutes,
            max_gap_minutes: cfg.max_gap_minutes,
            session_start_minutes: cfg.session_start_minutes,
        },
    );

    for s in &suggestions {
        log::info!("suggest {:?}: {} min (raw {} min, {} evidence)", s.bucket, s.minutes, s.raw_minutes, s.evidence.len());
    }
    let mut rows = Vec::new();
    for s in suggestions {
        let issue = match &s.bucket {
            Bucket::Issue(k) => match jira.issue(k) {
                Ok(Some(i)) => Some(i),
                Ok(None) => {
                    warnings.push(format!("{k} is not a Jira issue visible to you"));
                    None
                }
                Err(e) => {
                    warnings.push(format!("{k}: {e:#}"));
                    None
                }
            },
            Bucket::Unassigned(_) => None,
        };
        let already_logged_minutes = match &s.bucket {
            Bucket::Issue(k) => logged.iter().filter(|(lk, _)| lk == k).map(|(_, m)| *m).sum(),
            Bucket::Unassigned(_) => 0,
        };
        rows.push(Row { suggestion: s, issue, already_logged_minutes });
    }

    let candidates = if rows.iter().any(|r| r.issue.is_none()) {
        jira.candidates(&cfg.project_keys, 30).unwrap_or_else(|e| {
            warnings.push(format!("listing your open issues: {e:#}"));
            Vec::new()
        })
    } else {
        Vec::new()
    };

    log::info!("done in {:.2}s: {} rows, {} warnings", started.elapsed().as_secs_f64(), rows.len(), warnings.len());
    Ok(Report { date, me, rows, already_logged_minutes: already, budget_minutes: budget, candidates, warnings, repo_projects, repo_paths })
}

/// Project of each repo by directory name, the key activity rows use. A name found under roots of
/// different projects (`backend` in both trees) is left out: its merged row belongs to neither.
pub fn repo_projects(cfg: &Config, repos: &[std::path::PathBuf]) -> BTreeMap<String, String> {
    let mut map: BTreeMap<String, Option<String>> = BTreeMap::new();
    for r in repos {
        let Some(name) = r.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        let project = cfg.project_of_repo(r);
        match map.get(&name) {
            Some(seen) if *seen != project => {
                map.insert(name, None);
            }
            Some(_) => {}
            None => {
                map.insert(name, project);
            }
        }
    }
    map.into_iter().filter_map(|(name, p)| Some((name, p?))).collect()
}

pub fn fmt_minutes(m: u32) -> String {
    match (m / 60, m % 60) {
        (0, mm) => format!("{mm}m"),
        (h, 0) => format!("{h}h"),
        (h, mm) => format!("{h}h {mm}m"),
    }
}

/// `flow::prompt_gate` fed from the state file and Jira; a Jira failure opens the window.
pub fn gate(cfg: &Config, jira: &Jira, date: NaiveDate, force: bool, state: &crate::state::State) -> crate::flow::Gate {
    let decision = state.days.get(&date).copied();
    let logged = if force {
        None
    } else {
        match jira.myself().and_then(|me| jira.logged_on(&me, date)) {
            Ok(l) => {
                let m: u32 = l.iter().map(|(_, m)| m).sum();
                log::info!("{date}: {} logged in Jira", fmt_minutes(m));
                Some(m)
            }
            Err(e) => {
                log::warn!("checking time already logged on {date}: {e:#}");
                None
            }
        }
    };
    crate::flow::prompt_gate(decision, logged, cfg.workday_minutes, force)
}
