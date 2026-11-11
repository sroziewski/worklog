//! Pure helpers of the main window against a local tiny_http Jira mock; nothing is drawn.

mod common;

use common::MockJira;
use serde_json::json;
use worklog::config::Config;
use worklog::jira::Jira;
use worklog::ui;

#[test]
fn picker_projects_lists_every_configured_project_in_order_with_name_or_key() {
    let mock = MockJira::start(|_, url, _| match url {
        "/rest/api/2/project/XY" => (200, json!({"key": "XY", "name": "Empty Project"}).to_string()),
        "/rest/api/2/project/ABC" => (200, json!({"key": "ABC", "name": "Alpha"}).to_string()),
        "/rest/api/2/project/XYZ" => (404, json!({"errorMessages": ["No project could be found with key 'XYZ'."]}).to_string()),
        "/rest/api/2/project/AAA" => (500, "boom".into()),
        "/rest/api/2/project/NN" => (200, json!({"key": "NN"}).to_string()),
        _ => (418, String::new()),
    });
    let cfg = Config {
        project_keys: vec!["XY".into(), "ABC".into(), "XYZ".into(), "AAA".into(), "NN".into()],
        ctx_env_file: None,
        jira_url: None,
        ..Config::default()
    };
    let projects = ui::picker_projects(&cfg, &Jira::new(&mock.base, "tok-ui"));
    let pairs = |v: &[(&str, &str)]| v.iter().map(|(k, l)| (k.to_string(), l.to_string())).collect::<Vec<_>>();
    assert_eq!(
        projects,
        pairs(&[("XY", "Empty Project (XY)"), ("ABC", "Alpha (ABC)"), ("XYZ", "XYZ"), ("AAA", "AAA"), ("NN", "NN")])
    );
    let urls: Vec<String> = mock.requests().iter().map(|r| format!("{} {}", r.method, r.url)).collect();
    assert_eq!(
        urls,
        vec![
            "GET /rest/api/2/project/XY",
            "GET /rest/api/2/project/ABC",
            "GET /rest/api/2/project/XYZ",
            "GET /rest/api/2/project/AAA",
            "GET /rest/api/2/project/NN",
        ]
    );
}

mod load {
    use super::*;
    use common::{commit, day, init_repo, local_at, query_param};
    use std::sync::Mutex;
    use worklog::jira::Issue;
    use worklog::report::Progress;

    /// Repo `alpha` with "ABC-1 api" on 2026-11-04 and, with `unticketed`, also "cleanup".
    fn repo(tmp: &tempfile::TempDir, unticketed: bool) -> std::path::PathBuf {
        let d = day(2026, 11, 4);
        let setup = local_at(day(2026, 11, 1), 8, 0, 0);
        let root = tmp.path().join("root");
        let alpha = root.join("alpha");
        init_repo(&alpha, &setup);
        commit(&alpha, &setup, "init", "");
        commit(&alpha, &local_at(d, 10, 0, 0), "ABC-1 api", "");
        if unticketed {
            commit(&alpha, &local_at(d, 11, 0, 0), "cleanup", "");
        }
        root
    }

    fn cfg(root: &std::path::Path) -> Config {
        Config {
            repo_roots: vec![root.to_path_buf()],
            author_emails: vec![common::ME.into()],
            project_keys: vec!["ABC".into(), "XYZ".into()],
            ctx_env_file: None,
            jira_url: None,
            ..Config::default()
        }
    }

    /// Jira knowing ABC-1, with ABC-50 open for me (the candidates search answers
    /// `candidates_status`), projects Alpha (ABC) and Xylophone (XYZ).
    fn jira(candidates_status: u16) -> MockJira {
        let open = if candidates_status == 200 {
            json!({"issues": [{"key": "ABC-50", "fields": {"summary": "Pick me", "status": {"name": "Open"}}}]})
        } else {
            json!({"errorMessages": ["search down"]})
        };
        jira_answering_open_tickets(candidates_status, open)
    }

    /// As `jira`, the open-ticket search answering `status` with `body`.
    fn jira_answering_open_tickets(status: u16, body: serde_json::Value) -> MockJira {
        let body = body.to_string();
        MockJira::start(move |_, url, _| match url.split('?').next().unwrap() {
            "/rest/api/2/myself" => (200, json!({"name": "jdoe"}).to_string()),
            "/rest/api/2/search" if query_param(url, "jql").unwrap().starts_with("worklogAuthor") => (200, json!({"issues": []}).to_string()),
            "/rest/api/2/search" => (status, body.clone()),
            "/rest/api/2/issue/ABC-1" => (200, json!({"key": "ABC-1", "fields": {"summary": "API", "status": {"name": "In Progress"}}}).to_string()),
            "/rest/api/2/project/ABC" => (200, json!({"key": "ABC", "name": "Alpha"}).to_string()),
            "/rest/api/2/project/XYZ" => (200, json!({"key": "XYZ", "name": "Xylophone"}).to_string()),
            _ => (404, json!({"errorMessages": ["Issue Does Not Exist"]}).to_string()),
        })
    }

    type Loaded = (anyhow::Result<(worklog::report::Report, ui::Projects)>, Vec<Progress>);

    fn run(cfg: &Config, mock: &MockJira) -> Loaded {
        common::empty_git_config();
        let heard = Mutex::new(Vec::new());
        let d = day(2026, 11, 4);
        let r = ui::load(cfg, &Jira::new(&mock.base, "tok-ui"), d, day(2026, 11, 10), None, &|p| heard.lock().unwrap().push(p));
        (r, heard.into_inner().unwrap())
    }

    /// The progress without the git scan events.
    fn steps(heard: &[Progress]) -> Vec<Progress> {
        heard.iter().filter(|p| !matches!(p, Progress::Scan(_))).cloned().collect()
    }

    fn step(s: &str) -> Progress {
        Progress::Step(s.into())
    }

    fn candidate_searches(mock: &MockJira) -> usize {
        mock.requests().iter().filter(|r| query_param(&r.url, "jql").is_some_and(|q| q.starts_with("assignee"))).count()
    }

    fn named_projects() -> ui::Projects {
        vec![("ABC".to_string(), "Alpha (ABC)".to_string()), ("XYZ".to_string(), "Xylophone (XYZ)".to_string())]
    }

    fn abc50() -> Issue {
        Issue { key: "ABC-50".into(), summary: "Pick me".into(), status: "Open".into() }
    }

    #[test]
    fn every_row_ticketed_lists_open_tickets_after_the_report_then_reads_project_names() {
        let tmp = tempfile::tempdir().unwrap();
        let mock = jira(200);
        let (r, heard) = run(&cfg(&repo(&tmp, false)), &mock);
        let (report, projects) = r.unwrap();
        assert!(report.rows.iter().all(|x| x.issue.is_some()), "fixture: every row has its issue");
        assert_eq!(
            steps(&heard),
            vec![
                step("asking Jira who you are"),
                Progress::Scanning(1),
                step("reading what you logged on 2026-11-04"),
                step("looking up 1 tickets in Jira"),
                step("listing your open tickets"),
                step("reading project names"),
            ]
        );
        assert_eq!(report.candidates, vec![abc50()]);
        assert_eq!(projects, named_projects());
        assert_eq!(candidate_searches(&mock), 1);
    }

    #[test]
    fn a_row_without_issue_lists_open_tickets_once_in_the_report() {
        let tmp = tempfile::tempdir().unwrap();
        let mock = jira(200);
        let (r, heard) = run(&cfg(&repo(&tmp, true)), &mock);
        let (report, projects) = r.unwrap();
        assert!(report.rows.iter().any(|x| x.issue.is_none()), "fixture: the cleanup row has no issue");
        assert_eq!(
            steps(&heard),
            vec![
                step("asking Jira who you are"),
                Progress::Scanning(1),
                step("reading what you logged on 2026-11-04"),
                step("looking up 1 tickets in Jira"),
                step("listing your open tickets"),
                step("reading project names"),
            ]
        );
        assert_eq!(report.candidates, vec![abc50()]);
        assert_eq!(projects, named_projects());
        assert_eq!(candidate_searches(&mock), 1);
    }

    #[test]
    fn a_row_without_issue_and_an_empty_or_failed_report_list_is_not_asked_again() {
        // (name, open-ticket search status, body). The report's own search ran because the cleanup
        // row has no issue; whatever it found, load must not search a second time.
        let rows = [("no open tickets", 200, json!({"issues": []})), ("search down", 500, json!({"errorMessages": ["search down"]}))];
        for (name, status, body) in rows {
            let tmp = tempfile::tempdir().unwrap();
            let mock = jira_answering_open_tickets(status, body);
            let (r, heard) = run(&cfg(&repo(&tmp, true)), &mock);
            let (report, projects) = r.unwrap();
            assert!(report.rows.iter().any(|x| x.issue.is_none()), "{name}: fixture: the cleanup row has no issue");
            assert_eq!(candidate_searches(&mock), 1, "{name}");
            assert_eq!(report.candidates, vec![], "{name}");
            assert_eq!(heard.iter().filter(|p| **p == step("listing your open tickets")).count(), 1, "{name}: {heard:?}");
            assert_eq!(steps(&heard).last(), Some(&step("reading project names")), "{name}");
            assert_eq!(projects, named_projects(), "{name}");
        }
    }

    #[test]
    fn a_day_without_git_activity_has_no_rows_the_whole_day_left_and_the_open_tickets_listed() {
        // Either a repo with commits only on the days around 2026-11-04, or no repo at all.
        for quiet_repo in [true, false] {
            let tmp = tempfile::tempdir().unwrap();
            let root = tmp.path().join("root");
            std::fs::create_dir_all(&root).unwrap();
            if quiet_repo {
                let alpha = root.join("alpha");
                init_repo(&alpha, &local_at(day(2026, 11, 1), 8, 0, 0));
                commit(&alpha, &local_at(day(2026, 11, 3), 17, 0, 0), "ABC-1 the day before", "");
                commit(&alpha, &local_at(day(2026, 11, 5), 9, 0, 0), "ABC-1 the day after", "");
            }
            let (scanned, warnings) = if quiet_repo { (1, vec![]) } else { (0, vec![format!("no git repos found under {:?}", vec![root.clone()])]) };
            let mock = jira(200);
            let cfg = Config { workday_minutes: 450, ..cfg(&root) };

            let (r, heard) = run(&cfg, &mock);

            let (report, projects) = r.unwrap();
            let name = if quiet_repo { "repo without commits on the day" } else { "no repos" };
            assert_eq!(report.rows.len(), 0, "{name}");
            assert_eq!((report.already_logged_minutes, report.budget_minutes), (0, 450), "{name}");
            assert!(worklog::flow::needs_starter_row(report.rows.len(), report.budget_minutes), "{name}");
            assert_eq!(report.candidates, vec![abc50()], "{name}");
            assert_eq!(candidate_searches(&mock), 1, "{name}");
            assert_eq!(report.warnings, warnings, "{name}");
            assert_eq!(
                steps(&heard),
                vec![
                    step("asking Jira who you are"),
                    Progress::Scanning(scanned),
                    step("reading what you logged on 2026-11-04"),
                    step("looking up 0 tickets in Jira"),
                    step("listing your open tickets"),
                    step("reading project names"),
                ],
                "{name}"
            );
            assert_eq!(projects, named_projects(), "{name}");
        }
    }

    #[test]
    fn a_failed_open_ticket_list_leaves_it_empty_and_loading_goes_on() {
        let tmp = tempfile::tempdir().unwrap();
        let mock = jira(500);
        let (r, heard) = run(&cfg(&repo(&tmp, false)), &mock);
        let (report, projects) = r.unwrap();
        assert_eq!(report.candidates, vec![]);
        assert_eq!(report.warnings, Vec::<String>::new());
        assert_eq!(steps(&heard).last(), Some(&step("reading project names")));
        assert_eq!(projects, named_projects());
    }

    #[test]
    fn a_failed_report_is_the_error_and_nothing_more_is_asked() {
        let tmp = tempfile::tempdir().unwrap();
        let mock = MockJira::start(|_, _, _| (500, json!({"errorMessages": ["down"]}).to_string()));
        let (r, heard) = run(&cfg(&repo(&tmp, false)), &mock);
        assert_eq!(r.err().map(|e| format!("{e:#}")).as_deref(), Some("Jira 500 on /rest/api/2/myself: down"));
        assert_eq!(heard, vec![step("asking Jira who you are")]);
        assert_eq!(mock.requests().len(), 1);
    }
}

mod drain_progress {
    use std::path::PathBuf;
    use std::sync::mpsc::{TryRecvError, channel};
    use worklog::git::ScanEvent;
    use worklog::report::{LoadProgress, Progress};
    use worklog::ui;

    #[test]
    fn applies_every_waiting_message_in_order_and_leaves_the_channel_empty() {
        let (tx, rx) = channel();
        for p in [
            Progress::Scanning(9),
            Progress::Step("asking Jira who you are".into()),
            Progress::Scanning(2),
            Progress::Scan(ScanEvent::Started(PathBuf::from(r"D:\work\alpha"))),
        ] {
            tx.send(p).unwrap();
        }
        let mut p = LoadProgress::default();
        ui::drain_progress(&rx, &mut p);
        assert_eq!(p, LoadProgress { step: "reading git".into(), scanning: true, total: 2, done: 0, active: vec!["alpha".into()] });
        assert_eq!(rx.try_recv().err(), Some(TryRecvError::Empty));

        tx.send(Progress::Step("reading project names".into())).unwrap();
        ui::drain_progress(&rx, &mut p);
        assert_eq!(p, LoadProgress { step: "reading project names".into(), scanning: false, total: 2, done: 0, active: vec![] });
    }

    #[test]
    fn nothing_waiting_changes_nothing() {
        let (tx, rx) = channel::<Progress>();
        let before = LoadProgress { step: "reading git".into(), scanning: true, total: 3, done: 1, active: vec!["beta".into()] };
        let mut p = before.clone();
        ui::drain_progress(&rx, &mut p);
        assert_eq!(p, before);
        drop(tx);
        ui::drain_progress(&rx, &mut p);
        assert_eq!(p, before);
    }

    #[test]
    fn messages_sent_before_the_sender_went_away_are_still_applied() {
        let (tx, rx) = channel();
        tx.send(Progress::Step("listing your open tickets".into())).unwrap();
        drop(tx);
        let mut p = LoadProgress::default();
        ui::drain_progress(&rx, &mut p);
        assert_eq!(p.label(), "listing your open tickets…");
        assert_eq!(rx.try_recv().err(), Some(TryRecvError::Disconnected));
    }
}

#[test]
fn picker_projects_with_one_project() {
    let mock = MockJira::start(|_, _, _| (200, json!({"name": "Alpha"}).to_string()));
    let cfg = Config { project_keys: vec!["ABC".into()], ctx_env_file: None, jira_url: None, ..Config::default() };
    assert_eq!(ui::picker_projects(&cfg, &Jira::new(&mock.base, "tok-ui")), vec![("ABC".to_string(), "Alpha (ABC)".to_string())]);
}
