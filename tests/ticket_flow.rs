//! Tickets created through `ui_tickets::create_tickets` against a fake Jira, recorded in a temp
//! ticket history and listed by "My tickets" (`IssueBrowser`). Never the real Jira or %APPDATA%.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::TimeZone;
use common::MockJira;
use serde_json::{Value, json};
use worklog::config::TicketConfig;
use worklog::drafting::WorkItem;
use worklog::history::{History, TicketRecord};
use worklog::jira::{Issue, Jira, NewIssue};
use worklog::ui_issues::IssueBrowser;
use worklog::ui_tickets::{self, Creator, Draft};

const DAY: i64 = 86_400;

/// Wall clock: `IssueBrowser` lists what is recent against `Local::now()`, so creation times are
/// placed relative to it, far from the seven-day edge.
fn now() -> i64 {
    chrono::Local::now().timestamp()
}

/// Jira that creates issues as `<project>-<n>` (n counting up from 11 over successful creations)
/// and refuses a summary starting with "Broken" with a 400 field error.
fn creating_jira() -> MockJira {
    let next = AtomicUsize::new(11);
    MockJira::start(move |method, url, body| {
        if (method, url) != ("POST", "/rest/api/2/issue") {
            return (404, String::new());
        }
        let v: Value = serde_json::from_str(body).unwrap();
        let summary = v["fields"]["summary"].as_str().unwrap_or_default();
        if summary.starts_with("Broken") {
            return (400, json!({"errorMessages": [], "errors": {"components": "Component/s is required."}}).to_string());
        }
        let key = format!("{}-{}", v["fields"]["project"]["key"].as_str().unwrap(), next.fetch_add(1, Ordering::SeqCst));
        (201, json!({"id": "10001", "key": key, "self": "http://jira.example.test/rest/api/2/issue/10001"}).to_string())
    })
}

/// Every request answered with `status` and `body`.
fn failing_jira(status: u16, body: Value) -> MockJira {
    MockJira::start(move |_, _, _| (status, body.to_string()))
}

/// (project, summary) of each POST the fake Jira received, in order.
fn posted(mock: &MockJira) -> Vec<(String, String)> {
    mock.requests()
        .iter()
        .map(|r| {
            assert_eq!((r.method.as_str(), r.url.as_str()), ("POST", "/rest/api/2/issue"));
            let v: Value = serde_json::from_str(&r.body).unwrap();
            (v["fields"]["project"]["key"].as_str().unwrap().to_owned(), v["fields"]["summary"].as_str().unwrap().to_owned())
        })
        .collect()
}

fn wi(line: &str, change: Option<&str>) -> WorkItem {
    WorkItem { line: line.into(), change: change.map(str::to_owned) }
}

fn new_issue(project: &str, summary: &str, description: &str) -> NewIssue {
    NewIssue {
        project: project.into(),
        issue_type: "Task".into(),
        summary: summary.into(),
        description: description.into(),
        assignee: Some("jdoe".into()),
        extra_fields: serde_json::Map::new(),
    }
}

fn record(key: &str, project: &str, created: i64, title: &str, description: &str, language: &str, sources: Vec<WorkItem>) -> TicketRecord {
    TicketRecord {
        key: key.into(),
        project: project.into(),
        created,
        title: title.into(),
        description: description.into(),
        language: language.into(),
        sources,
    }
}

/// "My tickets" shows a record as its key, its title and "created <local date time>".
fn listed(key: &str, title: &str, created: i64) -> Issue {
    let at = chrono::Local.timestamp_opt(created, 0).single().unwrap();
    Issue { key: key.into(), summary: title.into(), status: at.format("created %Y-%m-%d %H:%M").to_string() }
}

type Results = Vec<(usize, Result<String, String>)>;

/// Runs `create_tickets`, collecting every `on_result` call; `stop_after` calls return false from
/// that call on.
fn create(jira: &MockJira, history: Option<&History>, jobs: Vec<(usize, NewIssue)>, sources: Vec<Vec<WorkItem>>, language: &str, at: i64, stop_after: usize) -> Results {
    let mut results = Vec::new();
    ui_tickets::create_tickets(&Jira::new(&jira.base, "tok"), history, jobs, sources, language, || at, |i, res| {
        results.push((i, res));
        results.len() < stop_after
    });
    results
}

struct TempHistory {
    history: Arc<History>,
    // Dropped after the history has closed the file.
    _tmp: tempfile::TempDir,
}

fn temp_history() -> TempHistory {
    let tmp = tempfile::tempdir().unwrap();
    let history = Arc::new(History::open(&tmp.path().join("worklog").join("tickets.redb")).unwrap());
    TempHistory { history, _tmp: tmp }
}

mod end_to_end {
    use super::*;

    fn draft(project: &str, title: &str, description: &str, selected: bool, sources: Vec<WorkItem>) -> Draft {
        let mut d = Draft::new(project, title, description, vec![]);
        d.selected = selected;
        d.sources = sources;
        d
    }

    /// Created a week and a day ago (ABC-11, not listed), an hour ago from the New ticket window
    /// (ABC-12 and XYZ-13; draft 1 unselected, draft 2 refused by Jira), and a minute ago (ABC-14).
    #[test]
    fn created_tickets_are_recorded_with_their_own_sources_and_listed_in_my_tickets() {
        let h = temp_history();
        let jira = creating_jira();
        let n = now();
        let (week_ago, hour_ago, minute_ago) = (n - 8 * DAY, n - 3600, n - 60);

        let old = create(&jira, Some(&h.history), vec![(0, new_issue("ABC", "Old import", "Stary opis."))], vec![vec![wi("[alpha] ABC-3 import", None)]], "Polish", week_ago, usize::MAX);
        assert_eq!(old, vec![(0, Ok("ABC-11".to_string()))]);

        let mut c = Creator::new("XYZ".into(), None);
        c.drafts = vec![
            draft("ABC", "  Export CSV  ", "\n Eksport wierszy do CSV. \n", true, vec![wi("[alpha] ABC-1 export rows", Some("+fn export()")), wi("[alpha] src/csv.rs", None)]),
            draft("XYZ", "Not selected", "never sent", false, vec![wi("[xyz] skipped line", Some("+skipped"))]),
            draft("XYZ", "Broken draft", "refused", true, vec![wi("[xyz] refused line", Some("+refused"))]),
            draft("XYZ", "Fix upload", "Naprawiono wysyłanie.", true, vec![wi("[xyz] XYZ-4 upload", Some("+fn upload()"))]),
        ];
        let cfg = TicketConfig { opencode_config: None, ..TicketConfig::default() };
        let jobs = c.create_jobs(&cfg, "jdoe");
        assert_eq!(jobs.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![0, 2, 3], "fixture: draft 1 is not sendable");
        let sources = c.job_sources(&jobs);
        let window = create(&jira, Some(&h.history), jobs, sources, "English", hour_ago, usize::MAX);
        assert_eq!(
            window,
            vec![
                (0, Ok("ABC-12".to_string())),
                (2, Err("Jira 400 on /rest/api/2/issue: components: Component/s is required.".to_string())),
                (3, Ok("XYZ-13".to_string())),
            ]
        );

        let latest = create(&jira, Some(&h.history), vec![(5, new_issue("ABC", "Newest one", ""))], vec![vec![wi("typo fix", None)]], "Polish", minute_ago, usize::MAX);
        assert_eq!(latest, vec![(5, Ok("ABC-14".to_string()))]);

        assert_eq!(
            posted(&jira),
            vec![
                ("ABC".to_string(), "Old import".to_string()),
                ("ABC".to_string(), "Export CSV".to_string()),
                ("XYZ".to_string(), "Broken draft".to_string()),
                ("XYZ".to_string(), "Fix upload".to_string()),
                ("ABC".to_string(), "Newest one".to_string()),
            ]
        );

        let abc11 = record("ABC-11", "ABC", week_ago, "Old import", "Stary opis.", "Polish", vec![wi("[alpha] ABC-3 import", None)]);
        let abc12 = record(
            "ABC-12",
            "ABC",
            hour_ago,
            "Export CSV",
            "Eksport wierszy do CSV.",
            "English",
            vec![wi("[alpha] ABC-1 export rows", Some("+fn export()")), wi("[alpha] src/csv.rs", None)],
        );
        let xyz13 = record("XYZ-13", "XYZ", hour_ago, "Fix upload", "Naprawiono wysyłanie.", "English", vec![wi("[xyz] XYZ-4 upload", Some("+fn upload()"))]);
        let abc14 = record("ABC-14", "ABC", minute_ago, "Newest one", "", "Polish", vec![wi("typo fix", None)]);
        // Read as of the oldest creation's seventh day, the window still holds all of them: four
        // records, none for the refused draft.
        assert_eq!(h.history.recent(week_ago + 7 * DAY).unwrap(), vec![abc14.clone(), abc12.clone(), xyz13.clone(), abc11.clone()]);

        let browser = IssueBrowser::new(Ok(Arc::clone(&h.history)));
        assert_eq!(browser.list_error, None);
        // Newest first; ABC-12 and XYZ-13 were created in the same second, so by key.
        assert_eq!(browser.records, vec![abc14, abc12, xyz13]);
        assert_eq!(
            browser.list,
            vec![listed("ABC-14", "Newest one", minute_ago), listed("ABC-12", "Export CSV", hour_ago), listed("XYZ-13", "Fix upload", hour_ago)]
        );
    }
}

mod create_tickets {
    use super::*;

    fn jobs() -> Vec<(usize, NewIssue)> {
        vec![(3, new_issue("ABC", "First", "one")), (6, new_issue("XYZ", "Second", "two")), (8, new_issue("ABC", "Third", "three"))]
    }

    fn sources() -> Vec<Vec<WorkItem>> {
        vec![vec![wi("line one", Some("+1"))], vec![wi("line two", None)], vec![wi("line three", Some("+3"))]]
    }

    fn recorded_keys(h: &History) -> Vec<String> {
        h.recent(now()).unwrap().into_iter().map(|r| r.key).collect()
    }

    #[test]
    fn on_result_returning_false_stops_the_remaining_jobs() {
        let h = temp_history();
        let jira = creating_jira();
        let results = create(&jira, Some(&h.history), jobs(), sources(), "Polish", now() - 120, 1);
        assert_eq!(results, vec![(3, Ok("ABC-11".to_string()))]);
        assert_eq!(posted(&jira), vec![("ABC".to_string(), "First".to_string())]);
        assert_eq!(recorded_keys(&h.history), vec!["ABC-11".to_string()]);
    }

    #[test]
    fn stopping_after_a_failure_sends_nothing_more() {
        let h = temp_history();
        let jira = creating_jira();
        let jobs = vec![(4, new_issue("ABC", "Broken first", "x")), (7, new_issue("ABC", "Never sent", "y"))];
        let results = create(&jira, Some(&h.history), jobs, sources(), "Polish", now() - 120, 1);
        assert_eq!(results, vec![(4, Err("Jira 400 on /rest/api/2/issue: components: Component/s is required.".to_string()))]);
        assert_eq!(posted(&jira), vec![("ABC".to_string(), "Broken first".to_string())]);
        assert_eq!(recorded_keys(&h.history), Vec::<String>::new());
    }

    #[test]
    fn without_a_history_every_result_is_still_reported() {
        let jira = creating_jira();
        let results = create(&jira, None, jobs(), sources(), "Polish", now(), usize::MAX);
        assert_eq!(results, vec![(3, Ok("ABC-11".to_string())), (6, Ok("XYZ-12".to_string())), (8, Ok("ABC-13".to_string()))]);
        assert_eq!(
            posted(&jira),
            vec![("ABC".to_string(), "First".to_string()), ("XYZ".to_string(), "Second".to_string()), ("ABC".to_string(), "Third".to_string())]
        );
    }

    /// A clock that moves on 90 s each time it is read, starting at `start`; counts its reads.
    struct Clock {
        start: i64,
        reads: std::cell::Cell<i64>,
    }

    impl Clock {
        fn read(&self) -> i64 {
            let n = self.reads.get();
            self.reads.set(n + 1);
            self.start + 90 * n
        }
    }

    #[test]
    fn each_created_ticket_takes_the_time_of_its_own_creation_and_a_failure_reads_no_clock() {
        let h = temp_history();
        let jira = creating_jira();
        let clock = Clock { start: now() - 3600, reads: std::cell::Cell::new(0) };
        let jobs = vec![
            (1, new_issue("ABC", "First", "one")),
            (2, new_issue("XYZ", "Broken middle", "x")),
            (4, new_issue("XYZ", "Second", "two")),
            (7, new_issue("ABC", "Third", "three")),
        ];
        let sources = vec![vec![wi("s1", None)], vec![wi("s2", None)], vec![wi("s4", Some("+4"))], vec![wi("s7", None)]];
        let mut results = Vec::new();
        ui_tickets::create_tickets(&Jira::new(&jira.base, "tok"), Some(&h.history), jobs, sources, "Polish", || clock.read(), |i, res| {
            results.push((i, res));
            true
        });
        assert_eq!(
            results,
            vec![
                (1, Ok("ABC-11".to_string())),
                (2, Err("Jira 400 on /rest/api/2/issue: components: Component/s is required.".to_string())),
                (4, Ok("XYZ-12".to_string())),
                (7, Ok("ABC-13".to_string())),
            ]
        );
        // Three tickets, three reads: start, start + 90, start + 180; the refused job takes none.
        assert_eq!(clock.reads.get(), 3);
        let s = clock.start;
        assert_eq!(
            h.history.recent(now()).unwrap(),
            vec![
                record("ABC-13", "ABC", s + 180, "Third", "three", "Polish", vec![wi("s7", None)]),
                record("XYZ-12", "XYZ", s + 90, "Second", "two", "Polish", vec![wi("s4", Some("+4"))]),
                record("ABC-11", "ABC", s, "First", "one", "Polish", vec![wi("s1", None)]),
            ]
        );
    }

    #[test]
    fn no_jobs_sends_and_reports_nothing() {
        let h = temp_history();
        let jira = creating_jira();
        assert_eq!(create(&jira, Some(&h.history), vec![], vec![], "Polish", now(), usize::MAX), vec![]);
        assert_eq!(jira.requests().len(), 0);
        assert_eq!(recorded_keys(&h.history), Vec::<String>::new());
    }

    // The documented contract pairs jobs and sources by position; a job past the end of `sources`
    // gets none (the code's `unwrap_or_default`), not a neighbour's.
    #[test]
    fn a_job_without_sources_is_recorded_with_none() {
        let h = temp_history();
        let jira = creating_jira();
        let at = now() - 300;
        let results = create(&jira, Some(&h.history), jobs()[..2].to_vec(), vec![vec![wi("only one", None)]], "Polish", at, usize::MAX);
        assert_eq!(results, vec![(3, Ok("ABC-11".to_string())), (6, Ok("XYZ-12".to_string()))]);
        assert_eq!(h.history.get("ABC-11").unwrap().map(|r| r.sources), Some(vec![wi("only one", None)]));
        assert_eq!(h.history.get("XYZ-12").unwrap(), Some(record("XYZ-12", "XYZ", at, "Second", "two", "Polish", vec![])));
    }

    #[test]
    fn failure_messages_and_the_timeout_hint() {
        const HINT: &str = " (Jira may still have created it; check before trying again)";
        // (name, status, body, expected error). The hint goes with every message that reads as a
        // timeout ("Timeout" / "timed out"), whatever the status.
        let rows: [(&str, u16, Value, String); 4] = [
            ("field error", 400, json!({"errors": {"summary": "You must specify a summary of the issue."}}), "Jira 400 on /rest/api/2/issue: summary: You must specify a summary of the issue.".into()),
            ("server error", 500, json!({"errorMessages": ["Internal server error"]}), "Jira 500 on /rest/api/2/issue: Internal server error".into()),
            ("request timeout", 408, json!({"errorMessages": ["Request Timeout"]}), format!("Jira 408 on /rest/api/2/issue: Request Timeout{HINT}")),
            ("gateway timed out", 504, json!({"errorMessages": ["Gateway timed out"]}), format!("Jira 504 on /rest/api/2/issue: Gateway timed out{HINT}")),
        ];
        for (name, status, body, expected) in rows {
            let h = temp_history();
            let jira = failing_jira(status, body);
            let results = create(&jira, Some(&h.history), vec![(2, new_issue("XYZ", "Any", "d"))], vec![vec![wi("l", None)]], "Polish", now(), usize::MAX);
            assert_eq!(results, vec![(2, Err(expected))], "{name}");
            assert_eq!(recorded_keys(&h.history), Vec::<String>::new(), "{name}");
        }
    }
}
