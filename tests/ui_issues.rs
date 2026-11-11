//! The "My tickets" window's decisions, without drawing it, Jira or the LLM. The history lives in
//! a temp dir.

mod common;

use std::sync::Arc;

use anyhow::anyhow;
use common::{day, local_at};
use worklog::drafting::WorkItem;
use worklog::history::{History, TicketRecord};
use worklog::jira::{Issue, IssueDetails};
use worklog::llm::TicketDraft;
use worklog::ui_issues::{self, Editor, IssueBrowser};

fn issue(key: &str, summary: &str, status: &str) -> Issue {
    Issue { key: key.into(), summary: summary.into(), status: status.into() }
}

fn details(key: &str, summary: &str, description: &str) -> IssueDetails {
    IssueDetails { key: key.into(), summary: summary.into(), description: description.into(), status: "In Progress".into(), updated: "2026-11-09T10:00:00.000+0100".into() }
}

fn sources() -> Vec<WorkItem> {
    vec![WorkItem { line: "[api] ABC-7 add export".into(), change: Some("+fn export()".into()) }, WorkItem { line: "[web] uncommitted a.ts".into(), change: None }]
}

fn editor() -> Editor {
    Editor::new(details("ABC-7", "Export rows", "h3. Opis\nRows.\n"), sources(), "Polish".into())
}

fn rec(key: &str, created: i64) -> TicketRecord {
    TicketRecord {
        key: key.into(),
        project: key.split('-').next().unwrap().into(),
        created,
        title: format!("title {key}"),
        description: format!("description {key}"),
        language: "English".into(),
        sources: vec![WorkItem { line: format!("line {key}"), change: None }],
    }
}

mod list_entry {
    use super::*;

    #[test]
    fn key_title_and_local_creation_time_as_status() {
        let created = local_at(day(2026, 11, 4), 9, 5, 0).timestamp();
        let r = TicketRecord { title: "Eksport CSV".into(), ..rec("XYZ-12", created) };
        assert_eq!(ui_issues::list_entry(&r), issue("XYZ-12", "Eksport CSV", "created 2026-11-04 09:05"));
    }

    #[test]
    fn seconds_are_dropped_not_rounded() {
        let created = local_at(day(2026, 1, 31), 23, 59, 59).timestamp();
        assert_eq!(ui_issues::list_entry(&rec("ABC-1", created)).status, "created 2026-01-31 23:59");
    }
}

mod filter_issues {
    use super::*;

    fn list() -> Vec<Issue> {
        vec![
            issue("ABC-12", "Fix Login page", "s1"),
            issue("XYZ-4", "Export CSV for auditors", "s2"),
            issue("ABC-7", "export report", "s3"),
            issue("ABCX-1", "Login audit", "s4"),
        ]
    }

    #[test]
    fn project_and_every_query_word_in_key_or_summary_ignoring_case_in_list_order() {
        // (name, project, query, expected keys)
        type Row = (&'static str, Option<&'static str>, &'static str, &'static [&'static str]);
        let rows: [Row; 14] = [
            ("no filter", None, "", &["ABC-12", "XYZ-4", "ABC-7", "ABCX-1"]),
            ("blank query is no filter", None, " \t ", &["ABC-12", "XYZ-4", "ABC-7", "ABCX-1"]),
            ("project only, not its prefix twin", Some("ABC"), "", &["ABC-12", "ABC-7"]),
            ("project in lower case", Some("abc"), "", &["ABC-12", "ABC-7"]),
            ("other project", Some("XYZ"), "", &["XYZ-4"]),
            ("unknown project", Some("XY"), "", &[]),
            ("word in summary, any case", None, "EXPORT", &["XYZ-4", "ABC-7"]),
            ("every word must occur", None, "export csv", &["XYZ-4"]),
            ("words matching different issues match none", None, "csv report", &[]),
            ("word in the key", None, "abcx", &["ABCX-1"]),
            ("key prefix", None, "abc-1", &["ABC-12"]),
            ("word across key and summary", None, "abc-7 report", &["ABC-7"]),
            ("project and query together", Some("ABC"), "login", &["ABC-12"]),
            ("surrounding and repeated spaces", None, "  login   audit ", &["ABCX-1"]),
        ];
        let list = list();
        for (name, project, query, expected) in rows {
            let got: Vec<&str> = ui_issues::filter_issues(&list, project, query).into_iter().map(|i| i.key.as_str()).collect();
            assert_eq!(got, expected.to_vec(), "{name}");
        }
    }

    #[test]
    fn returns_the_listed_issues_themselves() {
        let list = list();
        let got = ui_issues::filter_issues(&list, Some("XYZ"), "csv");
        assert_eq!(got, vec![&issue("XYZ-4", "Export CSV for auditors", "s2")]);
        assert!(std::ptr::eq(got[0], &list[1]));
    }

    #[test]
    fn empty_list() {
        assert_eq!(ui_issues::filter_issues(&[], None, "x"), Vec::<&Issue>::new());
    }
}

mod editor {
    use super::*;

    #[test]
    fn starts_with_jiras_fields_clean_and_idle() {
        let e = editor();
        assert_eq!((e.title.as_str(), e.description.as_str()), ("Export rows", "h3. Opis\nRows.\n"));
        assert_eq!(e.loaded, details("ABC-7", "Export rows", "h3. Opis\nRows.\n"));
        assert_eq!((e.sources.clone(), e.language.as_str()), (sources(), "Polish"));
        assert_eq!((e.error.as_deref(), e.notice.as_deref()), (None, None));
        assert_eq!(e.changes(), (None, None));
        assert!(!e.is_dirty());
        assert!(!e.can_save());
        assert!(!e.busy());
    }

    #[test]
    fn changes_report_only_changed_fields_trimmed() {
        // (name, title, description, expected changes); Jira has "Export rows" and
        // "h3. Opis\nRows.\n".
        type Row = (&'static str, &'static str, &'static str, (Option<&'static str>, Option<&'static str>));
        let rows: [Row; 8] = [
            ("nothing changed", "Export rows", "h3. Opis\nRows.\n", (None, None)),
            ("only surrounding whitespace changed", "  Export rows \t", "\nh3. Opis\nRows.", (None, None)),
            ("title changed", "  Export all rows ", "h3. Opis\nRows.\n", (Some("Export all rows"), None)),
            ("description changed", "Export rows", " h3. Opis\nAll rows.\n\n", (None, Some("h3. Opis\nAll rows."))),
            ("both changed", "New title", "New body", (Some("New title"), Some("New body"))),
            ("inner whitespace counts", "Export  rows", "h3. Opis\n\nRows.", (Some("Export  rows"), Some("h3. Opis\n\nRows."))),
            ("description cleared", "Export rows", "  \n ", (None, Some(""))),
            ("case counts", "export rows", "h3. Opis\nRows.\n", (Some("export rows"), None)),
        ];
        for (name, title, description, (t, d)) in rows {
            let mut e = editor();
            e.title = title.into();
            e.description = description.into();
            assert_eq!(e.changes(), (t.map(str::to_owned), d.map(str::to_owned)), "{name}");
            assert_eq!(e.is_dirty(), t.is_some() || d.is_some(), "{name}");
        }
    }

    #[test]
    fn jiras_own_surrounding_whitespace_is_not_a_change() {
        let mut e = Editor::new(details("ABC-8", "  Padded title ", "\nBody\n\n"), vec![], String::new());
        e.title = "Padded title".into();
        e.description = "Body".into();
        assert_eq!(e.changes(), (None, None));
    }

    #[test]
    fn can_save_needs_a_change_and_a_non_blank_title() {
        // (name, title, description, can save)
        let rows: [(&str, &str, &str, bool); 6] = [
            ("nothing changed", "Export rows", "h3. Opis\nRows.\n", false),
            ("whitespace only", " Export rows ", "h3. Opis\nRows.", false),
            ("title changed", "Export all rows", "h3. Opis\nRows.\n", true),
            ("only the description changed", "Export rows", "Other", true),
            ("title blanked", "  ", "h3. Opis\nRows.\n", false),
            ("title emptied while the description changed", "", "Other", false),
        ];
        for (name, title, description, expected) in rows {
            let mut e = editor();
            e.title = title.into();
            e.description = description.into();
            assert_eq!(e.can_save(), expected, "{name}");
        }
    }

    #[test]
    fn revert_restores_jiras_fields_and_clears_messages() {
        let mut e = editor();
        e.title = "Typed".into();
        e.description = "Typed body".into();
        e.error = Some("old error".into());
        e.notice = Some("old notice".into());
        e.revert();
        assert_eq!((e.title.as_str(), e.description.as_str()), ("Export rows", "h3. Opis\nRows.\n"));
        assert_eq!((e.error.as_deref(), e.notice.as_deref()), (None, None));
        assert!(!e.is_dirty());
    }

    #[test]
    fn revert_goes_back_to_the_last_saved_version() {
        let mut e = editor();
        e.title = "Saved title".into();
        e.apply_saved(Ok(details("ABC-7", "Saved title", "h3. Opis\nRows.\n")));
        e.title = "Typed after save".into();
        e.revert();
        assert_eq!(e.title, "Saved title");
    }
}

mod regen_request {
    use super::*;
    use worklog::drafting::CurrentTicket;

    fn item(line: &str, change: Option<&str>) -> WorkItem {
        WorkItem { line: line.into(), change: change.map(str::to_owned) }
    }

    fn two_diffs() -> Vec<WorkItem> {
        vec![item("[api] ABC-7 add export", Some("+fn export()")), item("[web] uncommitted a.ts", None), item("[svc] ABC-7 csv header", Some("+header"))]
    }

    #[test]
    fn carries_the_fields_as_edited_the_stored_language_the_sources_and_the_instructions_as_typed() {
        let mut e = Editor::new(details("ABC-7", "Export rows", "h3. Opis\nRows.\n"), two_diffs(), "Polish".into());
        e.title = "Edited title".into();
        e.description = "Edited body".into();
        e.instructions = "  add a test case for an expired token\n".into();
        assert_eq!(
            e.regen_request("English", true),
            (
                CurrentTicket { key: "ABC-7".into(), title: "Edited title".into(), description: "Edited body".into() },
                "Polish".to_owned(),
                two_diffs(),
                "  add a test case for an expired token\n".to_owned(),
            )
        );
    }

    #[test]
    fn send_code_off_drops_every_diff_and_keeps_the_lines() {
        let e = Editor::new(details("ABC-7", "Export rows", "Body"), two_diffs(), "Polish".into());
        let (_, _, items, _) = e.regen_request("English", false);
        assert_eq!(items, vec![item("[api] ABC-7 add export", None), item("[web] uncommitted a.ts", None), item("[svc] ABC-7 csv header", None)]);
    }

    #[test]
    fn blank_stored_language_falls_back_to_the_default() {
        // (name, stored language, expected)
        let rows: [(&str, &str, &str); 4] = [
            ("stored", "Polish", "Polish"),
            ("stored, kept as stored", " German ", " German "),
            ("empty", "", "Esperanto"),
            ("whitespace", " \t", "Esperanto"),
        ];
        for (name, stored, expected) in rows {
            let e = Editor::new(details("XYZ-3", "T3", "D3"), vec![], stored.into());
            let (current, language, items, instructions) = e.regen_request("Esperanto", true);
            assert_eq!(language, expected, "{name}");
            assert_eq!(current, CurrentTicket { key: "XYZ-3".into(), title: "T3".into(), description: "D3".into() }, "{name}");
            assert_eq!((items, instructions), (vec![], String::new()), "{name}");
        }
    }
}

mod instructions {
    use super::*;

    const TYPED: &str = "  add a test case for an expired token\nshorter ";

    #[test]
    fn start_empty_and_are_not_a_change_to_save() {
        let mut e = editor();
        assert_eq!(e.instructions, "");
        e.instructions = TYPED.into();
        assert_eq!(e.changes(), (None, None));
        assert!(!e.is_dirty());
        assert!(!e.can_save());
    }

    #[test]
    fn are_kept_as_typed_by_revert_redraft_and_save_results() {
        type Step = fn(&mut Editor);
        // (name, step, expected title after it): the title shows the step really ran.
        let rows: [(&str, Step, &str); 5] = [
            ("revert", |e| e.revert(), "Export rows"),
            ("redraft ok", |e| e.apply_redraft(Ok(TicketDraft { title: "Redrafted".into(), description: "D".into(), items: vec![1] })), "Redrafted"),
            ("redraft error", |e| e.apply_redraft(Err(anyhow!("LLM 503: overloaded"))), "Typed title"),
            ("saved", |e| drop(e.apply_saved(Ok(details("ABC-7", "Typed title", "h3. Opis\nRows.\n")))), "Typed title"),
            ("save error", |e| drop(e.apply_saved(Err(anyhow!("ABC-7 changed in Jira since you opened it")))), "Typed title"),
        ];
        for (name, step, title) in rows {
            let mut e = editor();
            e.title = "Typed title".into();
            e.instructions = TYPED.into();
            step(&mut e);
            assert_eq!(e.title, title, "{name}");
            assert_eq!(e.instructions, TYPED, "{name}");
        }
    }
}

mod apply_redraft {
    use super::*;

    #[test]
    fn draft_fills_the_fields_unsaved_and_clears_the_error() {
        let mut e = editor();
        e.error = Some("previous failure".into());
        e.apply_redraft(Ok(TicketDraft { title: "Pobieranie wierszy jako CSV".into(), description: "h3. Opis\nNowy opis.".into(), items: vec![1] }));
        assert_eq!((e.title.as_str(), e.description.as_str()), ("Pobieranie wierszy jako CSV", "h3. Opis\nNowy opis."));
        assert_eq!(e.error, None);
        assert_eq!(e.notice.as_deref(), Some("regenerated — check it, edit it if needed, then Save changes"));
        assert_eq!(e.loaded, details("ABC-7", "Export rows", "h3. Opis\nRows.\n"));
        assert_eq!(e.changes(), (Some("Pobieranie wierszy jako CSV".into()), Some("h3. Opis\nNowy opis.".into())));
        assert!(e.can_save());
    }

    #[test]
    fn error_keeps_the_typed_fields_and_shows_the_whole_chain() {
        let mut e = editor();
        e.title = "Typed title".into();
        e.description = "Typed body".into();
        e.notice = Some("saved to Jira".into());
        e.apply_redraft(Err(anyhow!("LLM 503: overloaded").context("analyst")));
        assert_eq!((e.title.as_str(), e.description.as_str()), ("Typed title", "Typed body"));
        assert_eq!(e.error.as_deref(), Some("analyst: LLM 503: overloaded"));
        assert_eq!(e.notice, None);
        assert_eq!(e.loaded, details("ABC-7", "Export rows", "h3. Opis\nRows.\n"));
    }
}

mod apply_saved {
    use super::*;

    #[test]
    fn jiras_state_becomes_the_baseline_and_the_issue_is_returned() {
        let mut e = editor();
        e.title = "Export all rows".into();
        e.error = Some("old".into());
        let saved = IssueDetails { status: "Review".into(), updated: "2026-11-10T08:00:00.000+0100".into(), ..details("ABC-7", "Export all rows", "h3. Opis\nRows.\n") };
        let got = e.apply_saved(Ok(saved.clone()));
        assert_eq!(got, Some(issue("ABC-7", "Export all rows", "Review")));
        assert_eq!(e.loaded, saved);
        assert_eq!((e.error.as_deref(), e.notice.as_deref()), (None, Some("saved to Jira")));
        assert_eq!(e.changes(), (None, None));
        assert!(!e.can_save());
    }

    #[test]
    fn text_typed_during_the_save_is_kept_and_stays_unsaved() {
        let mut e = editor();
        e.title = "Typed while saving".into();
        e.description = "Body typed while saving".into();
        e.apply_saved(Ok(details("ABC-7", "Export all rows", "h3. Opis\nRows.\n")));
        assert_eq!((e.title.as_str(), e.description.as_str()), ("Typed while saving", "Body typed while saving"));
        assert_eq!(e.changes(), (Some("Typed while saving".into()), Some("Body typed while saving".into())));
    }

    #[test]
    fn error_keeps_the_baseline_and_the_fields() {
        let mut e = editor();
        e.title = "Export all rows".into();
        e.notice = Some("regenerated".into());
        let got = e.apply_saved(Err(anyhow!("ABC-7 changed in Jira since you opened it")));
        assert_eq!(got, None);
        assert_eq!(e.error.as_deref(), Some("ABC-7 changed in Jira since you opened it"));
        assert_eq!(e.notice, None);
        assert_eq!(e.loaded, details("ABC-7", "Export rows", "h3. Opis\nRows.\n"));
        assert_eq!(e.title, "Export all rows");
        assert!(e.can_save());
    }
}

mod regen_items {
    use super::*;

    fn three() -> Vec<WorkItem> {
        vec![
            WorkItem { line: "[api] ABC-7 add export".into(), change: Some("+fn export()".into()) },
            WorkItem { line: "[web] uncommitted a.ts".into(), change: None },
            WorkItem { line: "[web] ABC-8 style".into(), change: Some("+.x {}".into()) },
        ]
    }

    #[test]
    fn with_code_the_sources_go_as_stored() {
        assert_eq!(ui_issues::regen_items(&three(), true), three());
    }

    #[test]
    fn without_code_every_diff_is_dropped_and_every_line_kept_in_order() {
        let lines: Vec<WorkItem> = ["[api] ABC-7 add export", "[web] uncommitted a.ts", "[web] ABC-8 style"]
            .iter()
            .map(|l| WorkItem { line: l.to_string(), change: None })
            .collect();
        assert_eq!(ui_issues::regen_items(&three(), false), lines);
    }

    #[test]
    fn no_sources_give_no_items() {
        assert_eq!(ui_issues::regen_items(&[], true), vec![]);
        assert_eq!(ui_issues::regen_items(&[], false), vec![]);
    }
}

/// `save_issue` against the local Jira mock: the first GET checks `updated`, the PUT writes, the
/// second GET is the new baseline.
mod save_issue {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use common::MockJira;
    use serde_json::json;
    use worklog::jira::Jira;

    const LOADED_AT: &str = "2026-11-09T10:00:00.000+0100";
    const DETAILS_URL: &str = "/rest/api/2/issue/ABC-7?fields=summary,description,status,updated";

    fn body(summary: &str, description: &str, status: &str, updated: &str) -> String {
        json!({"key": "ABC-7", "fields": {"summary": summary, "description": description, "status": {"name": status}, "updated": updated}}).to_string()
    }

    /// Jira whose GETs answer `gets` in turn and whose PUT answers `put`.
    fn jira(gets: Vec<(u16, String)>, put: (u16, String)) -> MockJira {
        let n = AtomicUsize::new(0);
        MockJira::start(move |method, _, _| match method {
            "GET" => gets[n.fetch_add(1, Ordering::SeqCst).min(gets.len() - 1)].clone(),
            _ => put.clone(),
        })
    }

    fn calls(mock: &MockJira) -> Vec<(String, String, String)> {
        mock.requests().into_iter().map(|r| (r.method, r.url, r.body)).collect()
    }

    fn loaded() -> IssueDetails {
        details("ABC-7", "Export rows", "h3. Opis\nRows.\n")
    }

    #[test]
    fn unchanged_in_jira_puts_the_fields_and_returns_jiras_new_state() {
        let mock = jira(
            vec![
                (200, body("Export rows", "h3. Opis\nRows.\n", "In Progress", LOADED_AT)),
                (200, body("Export all rows", "h3. Opis\nAll rows.", "Review", "2026-11-10T08:15:00.000+0100")),
            ],
            (204, String::new()),
        );
        let got = ui_issues::save_issue(&Jira::new(&mock.base, "tok-save"), &loaded(), Some("Export all rows"), Some("h3. Opis\nAll rows.")).unwrap();
        assert_eq!(
            got,
            IssueDetails {
                key: "ABC-7".into(),
                summary: "Export all rows".into(),
                description: "h3. Opis\nAll rows.".into(),
                status: "Review".into(),
                updated: "2026-11-10T08:15:00.000+0100".into(),
            }
        );
        let c = calls(&mock);
        assert_eq!(c.iter().map(|(m, u, _)| (m.as_str(), u.as_str())).collect::<Vec<_>>(), vec![
            ("GET", DETAILS_URL),
            ("PUT", "/rest/api/2/issue/ABC-7"),
            ("GET", DETAILS_URL),
        ]);
        let put: serde_json::Value = serde_json::from_str(&c[1].2).unwrap();
        assert_eq!(put, json!({"fields": {"summary": "Export all rows", "description": "h3. Opis\nAll rows."}}));
    }

    #[test]
    fn only_the_given_field_is_put() {
        let same = body("Export rows", "h3. Opis\nRows.\n", "In Progress", LOADED_AT);
        let mock = jira(vec![(200, same.clone()), (200, same)], (204, String::new()));
        ui_issues::save_issue(&Jira::new(&mock.base, "tok-save"), &loaded(), None, Some("New body")).unwrap();
        let c = calls(&mock);
        assert_eq!(c.len(), 3);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&c[1].2).unwrap(), json!({"fields": {"description": "New body"}}));
    }

    #[test]
    fn changed_in_jira_since_loading_is_refused_without_a_put() {
        // Another edit moved `updated` on; the fields themselves need not differ.
        let mock = jira(vec![(200, body("Export rows", "h3. Opis\nRows.\n", "In Progress", "2026-11-09T11:30:00.000+0100"))], (204, String::new()));
        let e = ui_issues::save_issue(&Jira::new(&mock.base, "tok-save"), &loaded(), Some("Export all rows"), None).unwrap_err();
        assert_eq!(
            e.to_string(),
            "ABC-7 changed in Jira since you opened it; ⟳ Reload loads the new version (your unsaved text is discarded)"
        );
        assert_eq!(calls(&mock), vec![("GET".to_string(), DETAILS_URL.to_string(), String::new())]);
    }

    #[test]
    fn failing_check_is_the_error_and_nothing_is_put() {
        let mock = jira(vec![(404, json!({"errorMessages": ["Issue Does Not Exist"], "errors": {}}).to_string())], (204, String::new()));
        let e = ui_issues::save_issue(&Jira::new(&mock.base, "tok-save"), &loaded(), Some("T"), None).unwrap_err();
        assert_eq!(e.to_string(), format!("Jira 404 on {DETAILS_URL}: Issue Does Not Exist"));
        assert_eq!(mock.requests().iter().map(|r| r.method.as_str()).collect::<Vec<_>>(), vec!["GET"]);
    }

    #[test]
    fn rejected_put_is_the_error_and_jira_is_not_read_again() {
        let mock = jira(
            vec![(200, body("Export rows", "h3. Opis\nRows.\n", "In Progress", LOADED_AT))],
            (400, json!({"errorMessages": [], "errors": {"summary": "Field 'summary' cannot be set."}}).to_string()),
        );
        let e = ui_issues::save_issue(&Jira::new(&mock.base, "tok-save"), &loaded(), Some("T"), None).unwrap_err();
        assert_eq!(e.to_string(), "Jira 400 on /rest/api/2/issue/ABC-7: summary: Field 'summary' cannot be set.");
        assert_eq!(mock.requests().iter().map(|r| r.method.as_str()).collect::<Vec<_>>(), vec!["GET", "PUT"]);
    }
}

mod browser {
    use super::*;

    fn now() -> i64 {
        chrono::Local::now().timestamp()
    }

    struct Fixture {
        history: Arc<History>,
        browser: IssueBrowser,
        // Dropped last, after the history has closed the file.
        _tmp: tempfile::TempDir,
    }

    /// ABC-2 (a minute old), XYZ-1 (an hour), ABC-3 (two days), ABC-1 (eight days: not listed).
    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let history = Arc::new(History::open(&tmp.path().join("tickets.redb")).unwrap());
        let n = now();
        for r in [rec("ABC-1", n - 8 * 86400), rec("ABC-3", n - 2 * 86400), rec("XYZ-1", n - 3600), rec("ABC-2", n - 60)] {
            history.record(&r).unwrap();
        }
        let browser = IssueBrowser::new(Ok(Arc::clone(&history)));
        Fixture { history, browser, _tmp: tmp }
    }

    fn list_keys(b: &IssueBrowser) -> Vec<&str> {
        b.list.iter().map(|i| i.key.as_str()).collect()
    }

    #[test]
    fn without_history_shows_why_and_an_empty_list() {
        let b = IssueBrowser::new(Err("tickets.redb is locked by another worklog".into()));
        assert_eq!(b.list_error.as_deref(), Some("ticket history unavailable: tickets.redb is locked by another worklog"));
        assert_eq!((b.list.len(), b.records.len()), (0, 0));
        assert_eq!((b.project.clone(), b.query.as_str(), b.pending_open.clone()), (None, "", None));
        assert!(b.editor.is_none());
        assert!(!b.busy());
    }

    #[test]
    fn lists_the_last_week_newest_first_right_away() {
        let f = fixture();
        let b = &f.browser;
        assert_eq!(b.list_error, None);
        assert_eq!(b.records.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), vec!["ABC-2", "XYZ-1", "ABC-3"]);
        assert_eq!(b.list, b.records.iter().map(ui_issues::list_entry).collect::<Vec<_>>());
        assert_eq!(b.list[1].summary, "title XYZ-1");
        assert!(b.editor.is_none());
    }

    #[test]
    fn refresh_picks_up_tickets_created_since() {
        let mut f = fixture();
        f.history.record(&rec("XY-9", now())).unwrap();
        assert_eq!(list_keys(&f.browser), vec!["ABC-2", "XYZ-1", "ABC-3"]);
        f.browser.refresh();
        assert_eq!(list_keys(&f.browser), vec!["XY-9", "ABC-2", "XYZ-1", "ABC-3"]);
        assert_eq!(f.browser.records[0], rec("XY-9", f.browser.records[0].created));
    }

    #[test]
    fn empty_history_is_an_empty_list_without_error() {
        let tmp = tempfile::tempdir().unwrap();
        let b = IssueBrowser::new(Ok(Arc::new(History::open(&tmp.path().join("t.redb")).unwrap())));
        assert_eq!((b.list.len(), b.records.len(), b.list_error.clone()), (0, 0, None));
    }

    #[test]
    fn request_open_asks_before_dropping_unsaved_changes_to_another_ticket() {
        // (name, editor: None or Some(dirty), clicked key, pending before, may load now, pending after).
        // The editor holds ABC-7; clicking it again (or ⟳ Reload) loads it anew.
        type Row = (&'static str, Option<bool>, &'static str, Option<&'static str>, bool, Option<&'static str>);
        let rows: [Row; 7] = [
            ("no editor", None, "ABC-2", None, true, None),
            ("no editor clears an old question", None, "ABC-2", Some("ABC-3"), true, None),
            ("clean editor, other ticket", Some(false), "ABC-2", Some("ABC-3"), true, None),
            ("clean editor, same ticket reloads", Some(false), "ABC-7", None, true, None),
            ("unsaved changes, other ticket asks", Some(true), "ABC-2", None, false, Some("ABC-2")),
            ("unsaved changes, reload of the same ticket asks too", Some(true), "ABC-7", None, false, Some("ABC-7")),
            ("unsaved changes, reload replaces an earlier question", Some(true), "ABC-7", Some("ABC-2"), false, Some("ABC-7")),
        ];
        for (name, dirty, key, pending_before, expected, pending_after) in rows {
            let mut b = IssueBrowser::new(Err("none".into()));
            b.editor = dirty.map(|d| {
                let mut e = editor();
                if d {
                    e.description = "changed".into();
                }
                e
            });
            b.pending_open = pending_before.map(str::to_owned);
            assert_eq!(b.request_open(key), expected, "{name}");
            assert_eq!(b.pending_open.as_deref(), pending_after, "{name}");
        }
    }

    #[test]
    fn second_click_elsewhere_replaces_the_pending_ticket() {
        let mut b = IssueBrowser::new(Err("none".into()));
        let mut e = editor();
        e.title = "changed".into();
        b.editor = Some(e);
        assert!(!b.request_open("ABC-2"));
        assert!(!b.request_open("XYZ-1"));
        assert_eq!(b.pending_open.as_deref(), Some("XYZ-1"));
    }

    #[test]
    fn apply_updated_changes_the_list_the_record_and_the_file_of_that_key_only() {
        let mut f = fixture();
        let before_list = f.browser.list.clone();
        f.browser.apply_updated(&issue("XYZ-1", "Saved title", "Review"), "Saved description");

        let mut expected_list = before_list.clone();
        expected_list[1].summary = "Saved title".into();
        assert_eq!(f.browser.list, expected_list, "status keeps the creation time");

        let xyz = f.browser.records.iter().find(|r| r.key == "XYZ-1").unwrap().clone();
        let expected_record = TicketRecord { title: "Saved title".into(), description: "Saved description".into(), ..xyz.clone() };
        assert_eq!(xyz, expected_record);
        assert_eq!(f.history.get("XYZ-1").unwrap(), Some(expected_record));

        for k in ["ABC-2", "ABC-3", "ABC-1"] {
            let r = f.history.get(k).unwrap().unwrap();
            assert_eq!((r.title, r.description), (format!("title {k}"), format!("description {k}")), "{k}");
        }
        for r in f.browser.records.iter().filter(|r| r.key != "XYZ-1") {
            assert_eq!((r.title.clone(), r.description.clone()), (format!("title {}", r.key), format!("description {}", r.key)));
        }
    }

    #[test]
    fn apply_updated_of_an_unknown_key_changes_nothing_and_records_nothing() {
        let mut f = fixture();
        let (list, records) = (f.browser.list.clone(), f.browser.records.clone());
        f.browser.apply_updated(&issue("ABC-99", "Other", "Open"), "Other description");
        assert_eq!((f.browser.list.clone(), f.browser.records.clone()), (list, records));
        assert_eq!(f.history.get("ABC-99").unwrap(), None);
    }

    #[test]
    fn apply_updated_without_history_still_updates_the_list() {
        let mut b = IssueBrowser::new(Err("none".into()));
        b.list = vec![issue("ABC-1", "old", "created 2026-11-01 10:00"), issue("ABC-2", "two", "created 2026-11-02 11:00")];
        b.apply_updated(&issue("ABC-1", "new", "Done"), "d");
        assert_eq!(b.list, vec![issue("ABC-1", "new", "created 2026-11-01 10:00"), issue("ABC-2", "two", "created 2026-11-02 11:00")]);
    }
}
