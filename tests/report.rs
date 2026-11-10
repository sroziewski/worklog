mod common;

use common::{MockJira, commit, day, init_repo, local_at, query_param};
use serde_json::json;
use worklog::config::Config;
use worklog::jira::{self, Issue, Jira};
use worklog::plan::Bucket;
use worklog::report;

#[test]
fn fmt_minutes_per_contract() {
    let cases = [(0, "0m"), (1, "1m"), (45, "45m"), (59, "59m"), (60, "1h"), (61, "1h 1m"), (135, "2h 15m"), (600, "10h")];
    for (m, expected) in cases {
        assert_eq!(report::fmt_minutes(m), expected, "{m}");
    }
}

/// One repo with three commits on 2026-11-04 (a past day, so uncommitted files are not read), a
/// Jira that knows ABC-1 (60 min already logged by jdoe) but not ABC-9.
#[test]
fn build_combines_git_activity_with_jira() {
    let d = day(2026, 11, 4);
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("svc");
    let setup = local_at(day(2026, 11, 1), 8, 0, 0);
    init_repo(&repo, &setup);
    commit(&repo, &setup, "init", "");
    commit(&repo, &local_at(d, 10, 0, 0), "ABC-1 api", "");
    commit(&repo, &local_at(d, 11, 0, 0), "ABC-9 typo", "");
    commit(&repo, &local_at(d, 12, 0, 0), "cleanup", "");

    let logged_start = jira::format_started(&local_at(d, 8, 0, 0));
    let mock = MockJira::start(move |_, url, _| {
        let path = url.split('?').next().unwrap();
        match path {
            "/rest/api/2/myself" => (200, json!({"name": "jdoe"}).to_string()),
            "/rest/api/2/search" => {
                let jql = query_param(url, "jql").unwrap();
                if jql.starts_with("worklogAuthor") {
                    (200, json!({"issues": [{"key": "ABC-1", "fields": {"summary": "API", "status": {"name": "In Progress"}}}]}).to_string())
                } else {
                    (200, json!({"issues": [{"key": "ABC-50", "fields": {"summary": "Pick me", "status": {"name": "Open"}}}]}).to_string())
                }
            }
            "/rest/api/2/issue/ABC-1/worklog" => (
                200,
                json!({"worklogs": [{"author": {"name": "jdoe"}, "started": logged_start, "timeSpentSeconds": 3600}]}).to_string(),
            ),
            "/rest/api/2/issue/ABC-1" => {
                (200, json!({"key": "ABC-1", "fields": {"summary": "API", "status": {"name": "In Progress"}}}).to_string())
            }
            _ => (404, json!({"errorMessages": ["Issue Does Not Exist"]}).to_string()),
        }
    });

    let cfg = Config {
        repo_roots: vec![repo.clone()],
        author_emails: vec![common::ME.into()],
        project_keys: vec!["ABC".into(), "XYZ".into()],
        ctx_env_file: None,
        jira_url: None,
        ..Config::default()
    };
    common::empty_git_config();
    let r = report::build(&cfg, &Jira::new(&mock.base, "tok"), d).unwrap();

    assert_eq!(r.date, d);
    assert_eq!(r.me, "jdoe");
    assert_eq!(r.already_logged_minutes, 60);
    // Default workday 480 - 60 already logged.
    assert_eq!(r.budget_minutes, 420);
    // Credits: ABC-1 30 (session start), ABC-9 60, svc unassigned 60. 420/15 = 28 steps shared by
    // the tickets 30:60 -> 9.33 / 18.67; largest remainder gives the spare step to ABC-9 -> 9 / 19.
    let rows: Vec<_> = r
        .rows
        .iter()
        .map(|x| (x.suggestion.bucket.clone(), x.suggestion.minutes, x.issue.clone(), x.already_logged_minutes))
        .collect();
    assert_eq!(
        rows,
        vec![
            (Bucket::Issue("ABC-9".into()), 285, None, 0),
            (
                Bucket::Issue("ABC-1".into()),
                135,
                Some(Issue { key: "ABC-1".into(), summary: "API".into(), status: "In Progress".into() }),
                60
            ),
            (Bucket::Unassigned("svc".into()), 0, None, 0),
        ]
    );
    assert_eq!(r.warnings, vec!["ABC-9 is not a Jira issue visible to you".to_string()]);
    assert_eq!(r.candidates, vec![Issue { key: "ABC-50".into(), summary: "Pick me".into(), status: "Open".into() }]);
    assert!(r.has_activity());
}

const D: (i32, u32, u32) = (2026, 11, 4);

/// Repo `svc` with an initial commit three days earlier and `(subject, hour)` commits on D.
fn repo_with(tmp: &tempfile::TempDir, commits: &[(&str, u32)]) -> std::path::PathBuf {
    let d = day(D.0, D.1, D.2);
    let repo = tmp.path().join("svc");
    let setup = local_at(day(2026, 11, 1), 8, 0, 0);
    init_repo(&repo, &setup);
    commit(&repo, &setup, "init", "");
    for (subject, hour) in commits {
        commit(&repo, &local_at(d, *hour, 0, 0), subject, "");
    }
    repo
}

fn cfg_for(repo: &std::path::Path, emails: &[&str]) -> Config {
    Config {
        repo_roots: vec![repo.to_path_buf()],
        author_emails: emails.iter().map(|e| e.to_string()).collect(),
        project_keys: vec!["ABC".into(), "XYZ".into()],
        ctx_env_file: None,
        jira_url: None,
        ..Config::default()
    }
}

fn path_of(url: &str) -> &str {
    url.split('?').next().unwrap()
}

#[test]
fn build_without_author_email_warns() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo_with(&tmp, &[("ABC-1 mine", 10)]);
    let mock = MockJira::start(|_, url, _| match path_of(url) {
        "/rest/api/2/myself" => (200, json!({"name": "jdoe"}).to_string()),
        _ => (200, json!({"issues": []}).to_string()),
    });
    // The fallback reads `git config --global user.email`; the global config is an empty file.
    common::empty_git_config();
    let r = report::build(&cfg_for(&repo, &[]), &Jira::new(&mock.base, "tok"), day(D.0, D.1, D.2)).unwrap();
    assert_eq!(r.warnings, vec!["no author email configured and git user.email is unset; no commits can be matched".to_string()]);
    assert!(!r.has_activity());
}

#[test]
fn build_turns_issue_and_candidate_errors_into_warnings() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo_with(&tmp, &[("ABC-1 api", 10), ("cleanup", 11)]);
    let mock = MockJira::start(|_, url, _| match path_of(url) {
        "/rest/api/2/myself" => (200, json!({"name": "jdoe"}).to_string()),
        "/rest/api/2/search" if query_param(url, "jql").unwrap().starts_with("worklogAuthor") => (200, json!({"issues": []}).to_string()),
        "/rest/api/2/search" => (500, json!({"errorMessages": ["search down"]}).to_string()),
        _ => (500, json!({"errorMessages": ["boom"]}).to_string()),
    });
    common::empty_git_config();
    let r = report::build(&cfg_for(&repo, &[common::ME]), &Jira::new(&mock.base, "tok"), day(D.0, D.1, D.2)).unwrap();

    let candidates_jql = r#"assignee = currentUser() AND resolution = Unresolved AND project in ("ABC", "XYZ") ORDER BY updated DESC"#;
    assert_eq!(
        r.warnings,
        vec![
            "ABC-1: Jira 500 on /rest/api/2/issue/ABC-1?fields=summary,status: boom".to_string(),
            format!(
                "listing your open issues: Jira 500 on /rest/api/2/search?jql={}&fields=summary,status&maxResults=30: search down",
                jira::url_encode(candidates_jql)
            ),
        ]
    );
    let rows: Vec<_> = r.rows.iter().map(|x| (x.suggestion.bucket.clone(), x.suggestion.minutes, x.issue.clone())).collect();
    assert_eq!(rows, vec![(Bucket::Issue("ABC-1".into()), 480, None), (Bucket::Unassigned("svc".into()), 0, None)]);
    assert_eq!(r.candidates, vec![]);
}

#[test]
fn build_skips_candidates_when_every_row_has_an_issue() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo_with(&tmp, &[("ABC-1 api", 10), ("ABC-2 db", 11)]);
    let mock = MockJira::start(|_, url, _| match path_of(url) {
        "/rest/api/2/myself" => (200, json!({"name": "jdoe"}).to_string()),
        "/rest/api/2/search" => (200, json!({"issues": []}).to_string()),
        p => {
            let key = p.rsplit('/').next().unwrap().to_owned();
            (200, json!({"key": key, "fields": {"summary": "S", "status": {"name": "Open"}}}).to_string())
        }
    });
    common::empty_git_config();
    let r = report::build(&cfg_for(&repo, &[common::ME]), &Jira::new(&mock.base, "tok"), day(D.0, D.1, D.2)).unwrap();
    assert_eq!(r.warnings, Vec::<String>::new());
    assert_eq!(r.candidates, vec![]);
    let reqs = mock.requests();
    let paths: Vec<&str> = reqs.iter().map(|x| path_of(&x.url)).collect();
    // ABC-2 has the larger credit (60 vs 30), so its row and issue lookup come first.
    assert_eq!(paths, vec!["/rest/api/2/myself", "/rest/api/2/search", "/rest/api/2/issue/ABC-2", "/rest/api/2/issue/ABC-1"]);
    assert!(query_param(&reqs[1].url, "jql").unwrap().starts_with("worklogAuthor"), "{}", reqs[1].url);
}

fn mock_all_known() -> MockJira {
    MockJira::start(|_, url, _| match path_of(url) {
        "/rest/api/2/myself" => (200, json!({"name": "jdoe"}).to_string()),
        "/rest/api/2/search" => (200, json!({"issues": []}).to_string()),
        p => {
            let key = p.rsplit('/').next().unwrap().to_owned();
            (200, json!({"key": key, "fields": {"summary": "S", "status": {"name": "Open"}}}).to_string())
        }
    })
}

/// (bucket, evidence) per row.
fn evidence_rows(r: &report::Report) -> Vec<(Bucket, Vec<String>)> {
    r.rows.iter().map(|x| (x.suggestion.bucket.clone(), x.suggestion.evidence.clone())).collect()
}

mod build_at {
    use super::*;

    /// Repo with "ABC-1 api" committed on D at 10:00 and an untracked notes.txt last written on D.
    fn repo_with_untracked(tmp: &tempfile::TempDir) -> std::path::PathBuf {
        let repo = repo_with(tmp, &[("ABC-1 api", 10)]);
        let notes = repo.join("notes.txt");
        std::fs::write(&notes, "todo").unwrap();
        common::set_mtime(&notes, &local_at(day(D.0, D.1, D.2), 12, 0, 0));
        repo
    }

    #[test]
    fn uncommitted_files_count_when_date_is_today() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo_with_untracked(&tmp);
        let mock = mock_all_known();
        common::empty_git_config();
        let d = day(D.0, D.1, D.2);
        let r = report::build_at(&cfg_for(&repo, &[common::ME]), &Jira::new(&mock.base, "tok"), d, d, None).unwrap();
        assert_eq!(
            evidence_rows(&r),
            vec![
                (Bucket::Issue("ABC-1".into()), vec!["ABC-1 api".to_string()]),
                (Bucket::Unassigned("svc".into()), vec!["uncommitted notes.txt".to_string()]),
            ]
        );
    }

    #[test]
    fn uncommitted_files_are_ignored_for_another_day() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo_with_untracked(&tmp);
        let mock = mock_all_known();
        common::empty_git_config();
        let d = day(D.0, D.1, D.2);
        let r = report::build_at(&cfg_for(&repo, &[common::ME]), &Jira::new(&mock.base, "tok"), d, day(2026, 11, 5), None).unwrap();
        assert_eq!(evidence_rows(&r), vec![(Bucket::Issue("ABC-1".into()), vec!["ABC-1 api".to_string()])]);
    }

    #[test]
    fn fallback_email_is_used_when_no_author_emails_are_configured() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo_with(&tmp, &[("ABC-1 api", 10)]);
        let mock = mock_all_known();
        common::empty_git_config();
        let d = day(D.0, D.1, D.2);
        let r = report::build_at(&cfg_for(&repo, &[]), &Jira::new(&mock.base, "tok"), d, d, Some(common::ME.into())).unwrap();
        assert_eq!(r.warnings, Vec::<String>::new());
        assert_eq!(evidence_rows(&r), vec![(Bucket::Issue("ABC-1".into()), vec!["ABC-1 api".to_string()])]);
    }

    #[test]
    fn configured_author_emails_win_over_the_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo_with(&tmp, &[("ABC-1 api", 10)]);
        let mock = mock_all_known();
        common::empty_git_config();
        let d = day(D.0, D.1, D.2);
        let cfg = cfg_for(&repo, &["colleague@example.test"]);
        let r = report::build_at(&cfg, &Jira::new(&mock.base, "tok"), d, d, Some(common::ME.into())).unwrap();
        assert_eq!(r.warnings, Vec::<String>::new());
        assert_eq!(evidence_rows(&r), vec![]);
    }

    #[test]
    fn no_configured_and_no_fallback_email_warns() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo_with(&tmp, &[("ABC-1 api", 10)]);
        let mock = mock_all_known();
        common::empty_git_config();
        let d = day(D.0, D.1, D.2);
        let r = report::build_at(&cfg_for(&repo, &[]), &Jira::new(&mock.base, "tok"), d, d, None).unwrap();
        assert_eq!(r.warnings, vec!["no author email configured and git user.email is unset; no commits can be matched".to_string()]);
        assert_eq!(evidence_rows(&r), vec![]);
    }
}

mod gate {
    use super::*;
    use chrono::NaiveDate;
    use std::collections::BTreeMap;
    use worklog::flow::Gate;
    use worklog::state::{Decision, State};

    /// Tue 2026-11-10, the day asked about.
    fn d() -> NaiveDate {
        day(2026, 11, 10)
    }

    fn cfg(workday_minutes: u32) -> Config {
        Config { repo_roots: vec![], ctx_env_file: None, jira_url: None, workday_minutes, ..Config::default() }
    }

    fn state(entries: &[(NaiveDate, Decision)]) -> State {
        State { days: entries.iter().copied().collect::<BTreeMap<_, _>>() }
    }

    /// Jira where jdoe logged `minutes` per issue on `d()` at 09:00 local; `search_status` != 200
    /// fails the worklog search.
    fn mock(per_issue: &'static [(&'static str, u32)], search_status: u16) -> MockJira {
        let started = jira::format_started(&local_at(d(), 9, 0, 0));
        MockJira::start(move |_, url, _| match path_of(url) {
            "/rest/api/2/myself" => (200, json!({"name": "jdoe"}).to_string()),
            "/rest/api/2/search" if search_status != 200 => (search_status, json!({"errorMessages": ["search down"]}).to_string()),
            "/rest/api/2/search" => {
                let issues: Vec<_> = per_issue.iter().map(|(k, _)| json!({"key": k, "fields": {}})).collect();
                (200, json!({ "issues": issues }).to_string())
            }
            p => match per_issue.iter().find(|(k, _)| p == format!("/rest/api/2/issue/{k}/worklog")) {
                Some((_, m)) => (
                    200,
                    json!({"worklogs": [{"author": {"name": "jdoe"}, "started": started, "timeSpentSeconds": m * 60}]}).to_string(),
                ),
                None => (404, String::new()),
            },
        })
    }

    #[test]
    fn accepted_half_day_is_asked_again() {
        let mock = mock(&[("ABC-1", 240)], 200);
        let s = state(&[(d(), Decision::Accepted)]);
        assert_eq!(report::gate(&cfg(480), &Jira::new(&mock.base, "tok"), d(), false, &s), Gate::Ask);

        let r = mock.requests();
        let paths: Vec<&str> = r.iter().map(|x| path_of(&x.url)).collect();
        assert_eq!(paths, vec!["/rest/api/2/myself", "/rest/api/2/search", "/rest/api/2/issue/ABC-1/worklog"]);
        assert_eq!(
            query_param(&r[1].url, "jql").as_deref(),
            Some(r#"worklogAuthor = currentUser() AND worklogDate = "2026-11-10""#)
        );
    }

    #[test]
    fn full_day_summed_over_issues_is_skipped() {
        // 200 + 280 = 480 = the workday.
        let mock = mock(&[("ABC-1", 200), ("XYZ-3", 280)], 200);
        let s = state(&[(d(), Decision::Accepted)]);
        assert_eq!(
            report::gate(&cfg(480), &Jira::new(&mock.base, "tok"), d(), false, &s),
            Gate::Skip("8h already logged (workday 8h); use --force to ask anyway".into())
        );
    }

    #[test]
    fn uses_the_configured_workday() {
        // 470 logged: full against a 450 workday, short of the default 480.
        let short_workday = mock(&[("ABC-1", 470)], 200);
        assert_eq!(
            report::gate(&cfg(450), &Jira::new(&short_workday.base, "tok"), d(), false, &State::default()),
            Gate::Skip("7h 50m already logged (workday 7h 30m); use --force to ask anyway".into())
        );
        let default_workday = mock(&[("ABC-1", 470)], 200);
        assert_eq!(report::gate(&cfg(480), &Jira::new(&default_workday.base, "tok"), d(), false, &State::default()), Gate::Ask);
    }

    #[test]
    fn denied_day_is_skipped_even_when_short() {
        let mock = mock(&[("ABC-1", 60)], 200);
        let s = state(&[(d(), Decision::Denied)]);
        assert_eq!(
            report::gate(&cfg(480), &Jira::new(&mock.base, "tok"), d(), false, &s),
            Gate::Skip("you denied this day; use --force to ask again".into())
        );
    }

    #[test]
    fn decision_of_another_day_does_not_count() {
        let mock = mock(&[("ABC-1", 60)], 200);
        let s = state(&[(day(2026, 11, 9), Decision::Denied), (day(2026, 11, 11), Decision::Denied)]);
        assert_eq!(report::gate(&cfg(480), &Jira::new(&mock.base, "tok"), d(), false, &s), Gate::Ask);
    }

    #[test]
    fn jira_search_error_opens_the_window() {
        let mock = mock(&[("ABC-1", 480)], 500);
        let s = state(&[(d(), Decision::Accepted)]);
        assert_eq!(report::gate(&cfg(480), &Jira::new(&mock.base, "tok"), d(), false, &s), Gate::Ask);
        let paths: Vec<String> = mock.requests().iter().map(|x| path_of(&x.url).to_owned()).collect();
        assert_eq!(paths, vec!["/rest/api/2/myself", "/rest/api/2/search"]);
    }

    #[test]
    fn jira_myself_error_opens_the_window() {
        let mock = MockJira::start(|_, _, _| (500, json!({"errorMessages": ["down"]}).to_string()));
        assert_eq!(report::gate(&cfg(480), &Jira::new(&mock.base, "tok"), d(), false, &State::default()), Gate::Ask);
        assert_eq!(mock.requests().len(), 1);
    }

    #[test]
    fn jira_error_on_a_denied_day_still_skips() {
        let mock = mock(&[], 500);
        let s = state(&[(d(), Decision::Denied)]);
        assert_eq!(
            report::gate(&cfg(480), &Jira::new(&mock.base, "tok"), d(), false, &s),
            Gate::Skip("you denied this day; use --force to ask again".into())
        );
    }

    #[test]
    fn force_asks_without_any_jira_request() {
        // (name, state, logged per issue)
        type Row = (&'static str, State, &'static [(&'static str, u32)]);
        let rows: [Row; 3] = [
            ("denied and full", state(&[(d(), Decision::Denied)]), &[("ABC-1", 480)]),
            ("accepted and full", state(&[(d(), Decision::Accepted)]), &[("ABC-1", 600)]),
            ("unanswered and empty", State::default(), &[]),
        ];
        for (name, s, logged) in rows {
            let jira_mock = mock(logged, 200);
            assert_eq!(report::gate(&cfg(480), &Jira::new(&jira_mock.base, "tok"), d(), true, &s), Gate::Ask, "{name}");
            assert_eq!(jira_mock.requests().len(), 0, "{name}: force must not call Jira");
        }
    }
}

mod repo_projects {
    use super::*;
    use std::collections::BTreeMap;

    /// Repos `mapped/svc` and `other/web` (one level under the two roots), each with one commit.
    fn two_roots(tmp: &tempfile::TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
        let setup = local_at(day(2026, 11, 1), 8, 0, 0);
        let (mapped, other) = (tmp.path().join("mapped"), tmp.path().join("other"));
        for repo in [mapped.join("svc"), other.join("web")] {
            init_repo(&repo, &setup);
            commit(&repo, &setup, "init", "");
        }
        (mapped, other)
    }

    fn build(cfg: &Config) -> report::Report {
        let mock = mock_all_known();
        common::empty_git_config();
        report::build(cfg, &Jira::new(&mock.base, "tok"), day(D.0, D.1, D.2)).unwrap()
    }

    fn map(v: &[(&str, &str)]) -> BTreeMap<String, String> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn by_dir_name_conflicting_names_dropped_whatever_the_order() {
        // Paths need not exist. Roots: D:\a -> ABC, D:\b -> XYZ; E:\x has no project.
        let cfg = Config {
            project_keys: vec!["ABC".into(), "XYZ".into()],
            repo_projects: map(&[(r"D:\a", "ABC"), (r"D:\b", "XYZ")]),
            ctx_env_file: None,
            ..Config::default()
        };
        // (name, repos, expected)
        type Row = (&'static str, &'static [&'static str], BTreeMap<String, String>);
        let rows: [Row; 11] = [
            ("distinct names, unmapped absent", &[r"D:\a\svc", r"D:\b\web", r"E:\x\tool"], map(&[("svc", "ABC"), ("web", "XYZ")])),
            ("same name, two projects", &[r"D:\a\backend", r"D:\b\backend", r"D:\a\svc"], map(&[("svc", "ABC")])),
            ("same name, two projects, reversed", &[r"D:\b\backend", r"D:\a\backend", r"D:\a\svc"], map(&[("svc", "ABC")])),
            ("same name, project then none", &[r"D:\a\backend", r"E:\x\backend"], map(&[])),
            ("same name, none then project", &[r"E:\x\backend", r"D:\a\backend"], map(&[])),
            ("same name, same project kept", &[r"D:\a\backend", r"D:\a\sub\backend"], map(&[("backend", "ABC")])),
            ("three-way A, B, A dropped", &[r"D:\a\backend", r"D:\b\backend", r"D:\a\other\backend"], map(&[])),
            ("three-way A, A, B dropped", &[r"D:\a\backend", r"D:\a\other\backend", r"D:\b\backend"], map(&[])),
            ("conflict does not touch other names", &[r"D:\b\web", r"D:\a\backend", r"E:\x\backend", r"D:\a\svc"], map(&[("svc", "ABC"), ("web", "XYZ")])),
            ("path without a dir name skipped", &["D:\\", r"D:\a\svc"], map(&[("svc", "ABC")])),
            ("no repos", &[], map(&[])),
        ];
        for (name, repos, expected) in rows {
            let repos: Vec<std::path::PathBuf> = repos.iter().map(std::path::PathBuf::from).collect();
            assert_eq!(report::repo_projects(&cfg, &repos), expected, "{name}");
        }
    }

    #[test]
    fn repos_under_a_mapped_root_by_dir_name_unmapped_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let (mapped, other) = two_roots(&tmp);
        let cfg = Config {
            repo_roots: vec![mapped.clone(), other],
            author_emails: vec![common::ME.into()],
            project_keys: vec!["AAA".into(), "XY".into()],
            repo_projects: BTreeMap::from([(mapped.to_string_lossy().into_owned(), "AAA".to_string())]),
            ctx_env_file: None,
            jira_url: None,
            ..Config::default()
        };
        assert_eq!(build(&cfg).repo_projects, BTreeMap::from([("svc".to_string(), "AAA".to_string())]));
    }

    #[test]
    fn single_project_covers_every_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let (mapped, other) = two_roots(&tmp);
        let cfg = Config {
            repo_roots: vec![mapped, other],
            author_emails: vec![common::ME.into()],
            project_keys: vec!["ONE".into()],
            repo_projects: BTreeMap::new(),
            ctx_env_file: None,
            jira_url: None,
            ..Config::default()
        };
        assert_eq!(
            build(&cfg).repo_projects,
            BTreeMap::from([("svc".to_string(), "ONE".to_string()), ("web".to_string(), "ONE".to_string())])
        );
    }

    #[test]
    fn repo_paths_lists_every_scanned_repo_by_name_keeping_both_of_a_shared_name() {
        let tmp = tempfile::tempdir().unwrap();
        let (mapped, other) = two_roots(&tmp);
        let setup = local_at(day(2026, 11, 1), 8, 0, 0);
        init_repo(&other.join("svc"), &setup);
        commit(&other.join("svc"), &setup, "init", "");
        let cfg = Config {
            repo_roots: vec![mapped.clone(), other.clone()],
            author_emails: vec![common::ME.into()],
            project_keys: vec!["AAA".into(), "XY".into()],
            repo_projects: BTreeMap::from([(mapped.to_string_lossy().into_owned(), "AAA".to_string())]),
            ctx_env_file: None,
            jira_url: None,
            ..Config::default()
        };
        let r = build(&cfg);
        // discover_repos sorts its result, so mapped\svc comes before other\svc.
        assert_eq!(
            r.repo_paths,
            BTreeMap::from([
                ("svc".to_string(), vec![mapped.join("svc"), other.join("svc")]),
                ("web".to_string(), vec![other.join("web")]),
            ])
        );
        // The name shared by two projects has no project, yet both of its repos are listed above.
        assert_eq!(r.repo_projects, BTreeMap::new());
    }
}

#[test]
fn build_without_repos_warns_and_has_no_activity() {
    let tmp = tempfile::tempdir().unwrap();
    let mock = MockJira::start(|_, url, _| {
        if url.starts_with("/rest/api/2/myself") {
            (200, json!({"name": "jdoe"}).to_string())
        } else {
            (200, json!({"issues": []}).to_string())
        }
    });
    let root = tmp.path().join("empty");
    std::fs::create_dir_all(&root).unwrap();
    let cfg = Config {
        repo_roots: vec![root.clone()],
        author_emails: vec![common::ME.into()],
        ctx_env_file: None,
        ..Config::default()
    };
    let r = report::build(&cfg, &Jira::new(&mock.base, "tok"), day(2026, 11, 4)).unwrap();
    assert_eq!(r.warnings, vec![format!("no git repos found under {:?}", vec![root])]);
    assert!(!r.has_activity());
    assert_eq!(r.budget_minutes, 480);
    assert_eq!(r.candidates, vec![]);
    assert!(r.repo_paths.is_empty(), "{:?}", r.repo_paths);
}
