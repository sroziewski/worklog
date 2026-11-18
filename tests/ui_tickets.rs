//! The ticket creator's decisions, without drawing a window.

use std::collections::BTreeSet;

use anyhow::anyhow;
use serde_json::json;
use worklog::config::TicketConfig;
use worklog::drafting::WorkItem;
use worklog::jira::{Issue, NewIssue};
use worklog::llm::TicketDraft;
use worklog::ticket_kind::Kind;
use worklog::ui_tickets::{self, Creator, Draft, Outcome, WorkGroup};

/// A draft written for XYZ, the project `creator()` starts in.
fn d(title: &str, description: &str, selected: bool, created: Option<Result<&str, &str>>) -> Draft {
    dr(title, description, selected, created, &[])
}

fn dr(title: &str, description: &str, selected: bool, created: Option<Result<&str, &str>>, rows: &[u64]) -> Draft {
    dp("XYZ", title, description, selected, created, rows)
}

fn dp(project: &str, title: &str, description: &str, selected: bool, created: Option<Result<&str, &str>>, rows: &[u64]) -> Draft {
    Draft {
        project: project.into(),
        title: title.into(),
        description: description.into(),
        selected,
        created: created.map(|r| r.map(str::to_owned).map_err(str::to_owned)),
        rows: rows.to_vec(),
        sources: Vec::new(),
        kind: Kind::default(),
    }
}

type Snapshot = (String, String, bool, Option<Result<String, String>>, Vec<u64>);

fn snapshot(c: &Creator) -> Vec<Snapshot> {
    c.drafts.iter().map(|x| (x.title.clone(), x.description.clone(), x.selected, x.created.clone(), x.rows.clone())).collect()
}

fn snap(title: &str, description: &str, selected: bool, created: Option<Result<&str, &str>>, rows: &[u64]) -> Snapshot {
    (title.into(), description.into(), selected, created.map(|r| r.map(str::to_owned).map_err(str::to_owned)), rows.to_vec())
}

fn creator(drafts: Vec<Draft>) -> Creator {
    let mut c = Creator::new("XYZ".into(), Some(42));
    c.drafts = drafts;
    c
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn group(row: u64, project: Option<&str>, repos: &[&str], lines: &[&str]) -> WorkGroup {
    WorkGroup { row, title: format!("row {row} title"), project: project.map(str::to_owned), repos: strings(repos), lines: strings(lines) }
}

/// Listed out of row-id order so group order and tick (row id) order differ: row 50 (ABC, lower
/// case key), row 10 (ABC), row 20 (no project), row 30 (XYZ).
fn groups() -> Vec<WorkGroup> {
    vec![
        group(50, Some("abc"), &["svc"], &["Cargo.toml", "src/main.rs"]),
        group(10, Some("ABC"), &["api", "web"], &["ABC-1 fix login", "branch feature/sso", "src/a.rs"]),
        group(20, None, &[], &["notes.md", "typo fix"]),
        group(30, Some("XYZ"), &["xyz"], &["pom.xml", "XYZ-4 export"]),
    ]
}

fn ticks(v: &[(u64, usize)]) -> BTreeSet<(u64, usize)> {
    v.iter().copied().collect()
}

fn items(v: &[(u64, &str)]) -> Vec<(u64, String)> {
    v.iter().map(|(r, s)| (*r, s.to_string())).collect()
}

#[test]
fn fresh_creator_is_idle_and_empty() {
    let c = Creator::new("ABC".into(), None);
    assert!(!c.busy());
    assert!(!c.creating());
    assert_eq!((c.project.as_str(), c.origin, c.drafts.len(), c.error.as_deref()), ("ABC", None, 0, None));
    assert_eq!(c.ticks, BTreeSet::new());
    assert_eq!(c.create_jobs(&TicketConfig { opencode_config: None, ..TicketConfig::default() }, "jdoe"), vec![]);
}

#[test]
fn new_draft_is_selected_uncreated_with_its_project_and_rows() {
    let x = Draft::new("XY", "Title A", "Body B", vec![7, 3]);
    assert_eq!(
        (x.project.as_str(), x.title.as_str(), x.description.as_str(), x.selected, x.created, x.rows, x.sources, x.kind),
        ("XY", "Title A", "Body B", true, None, vec![7, 3], vec![], Kind::default())
    );
}

#[test]
fn evidence_text_is_size_15() {
    assert_eq!(ui_tickets::EVIDENCE_TEXT_SIZE, 15.0);
}

mod visible_in {
    use super::*;

    #[test]
    fn own_project_case_insensitively_or_every_project_without_one() {
        // (name, group project, selected project, visible)
        let rows: [(&str, Option<&str>, &str, bool); 9] = [
            ("same project", Some("ABC"), "ABC", true),
            ("group key lower case", Some("abc"), "ABC", true),
            ("selected key lower case", Some("XYZ"), "xyz", true),
            ("other project", Some("XYZ"), "ABC", false),
            ("selected is a prefix of the group's", Some("ABCX"), "ABC", false),
            ("group's is a prefix of the selected", Some("ABC"), "ABCX", false),
            ("no selected project", Some("ABC"), "", false),
            ("group without project, any selection", None, "XYZ", true),
            ("group without project, empty selection", None, "", true),
        ];
        for (name, project, selected, expected) in rows {
            assert_eq!(group(1, project, &[], &["x"]).visible_in(selected), expected, "{name}");
        }
    }
}

mod ticked_items {
    use super::*;

    #[test]
    fn ticked_lines_of_visible_groups_in_group_then_line_order() {
        // Ticks: of hidden row 30, past the lines of row 10 (index 7) and row 20 (index 5), and of
        // the unknown row 99 are all left out.
        let t = ticks(&[(30, 0), (10, 2), (20, 1), (50, 1), (10, 0), (10, 7), (99, 0), (20, 5), (50, 0)]);
        // (name, selected project, expected)
        type Row = (&'static str, &'static str, Vec<(u64, String)>);
        let rows: [Row; 4] = [
            (
                "ABC",
                "ABC",
                items(&[
                    (50, "[svc] Cargo.toml"),
                    (50, "[svc] src/main.rs"),
                    (10, "[api, web] ABC-1 fix login"),
                    (10, "[api, web] src/a.rs"),
                    (20, "typo fix"),
                ]),
            ),
            ("XYZ", "XYZ", items(&[(20, "typo fix"), (30, "[xyz] pom.xml")])),
            ("selected in lower case", "xyz", items(&[(20, "typo fix"), (30, "[xyz] pom.xml")])),
            ("project with no groups of its own", "XY", items(&[(20, "typo fix")])),
        ];
        for (name, project, expected) in rows {
            assert_eq!(ui_tickets::ticked_items(&groups(), &t, project), expected, "{name}");
        }
    }

    #[test]
    fn nothing_ticked_or_no_groups_gives_nothing() {
        assert_eq!(ui_tickets::ticked_items(&groups(), &BTreeSet::new(), "ABC"), vec![]);
        assert_eq!(ui_tickets::ticked_items(&[], &ticks(&[(10, 0)]), "ABC"), vec![]);
    }

    #[test]
    fn group_without_lines_gives_nothing_for_its_ticks() {
        let g = vec![group(60, Some("ABC"), &["api"], &[]), group(61, Some("ABC"), &["web"], &["index.ts"])];
        assert_eq!(ui_tickets::ticked_items(&g, &ticks(&[(60, 0), (61, 0)]), "ABC"), items(&[(61, "[web] index.ts")]));
    }
}

mod ticked_lines {
    use super::*;

    type Line = (u64, Vec<String>, String);

    fn owned(v: Vec<(u64, &[String], &str)>) -> Vec<Line> {
        v.into_iter().map(|(r, repos, line)| (r, repos.to_vec(), line.to_owned())).collect()
    }

    fn line(row: u64, repos: &[&str], text: &str) -> Line {
        (row, strings(repos), text.to_owned())
    }

    #[test]
    fn repos_and_raw_line_apart_same_order_and_filter_as_ticked_items() {
        let t = ticks(&[(30, 0), (10, 2), (20, 1), (50, 1), (10, 0), (10, 7), (99, 0), (20, 5), (50, 0)]);
        // (name, selected project, expected)
        type Row = (&'static str, &'static str, Vec<Line>);
        let rows: [Row; 3] = [
            (
                "ABC",
                "ABC",
                vec![
                    line(50, &["svc"], "Cargo.toml"),
                    line(50, &["svc"], "src/main.rs"),
                    line(10, &["api", "web"], "ABC-1 fix login"),
                    line(10, &["api", "web"], "src/a.rs"),
                    line(20, &[], "typo fix"),
                ],
            ),
            ("XYZ", "XYZ", vec![line(20, &[], "typo fix"), line(30, &["xyz"], "pom.xml")]),
            ("project with no groups of its own", "XY", vec![line(20, &[], "typo fix")]),
        ];
        for (name, project, expected) in rows {
            let g = groups();
            let got = owned(ui_tickets::ticked_lines(&g, &t, project));
            assert_eq!(got, expected, "{name}");
            // Same rows in the same order as the joined form the prompt numbers.
            let joined: Vec<u64> = ui_tickets::ticked_items(&g, &t, project).into_iter().map(|(r, _)| r).collect();
            assert_eq!(got.iter().map(|(r, _, _)| *r).collect::<Vec<_>>(), joined, "{name}");
        }
    }

    #[test]
    fn nothing_ticked_or_no_groups_gives_nothing() {
        assert_eq!(owned(ui_tickets::ticked_lines(&groups(), &BTreeSet::new(), "ABC")), vec![]);
        assert_eq!(owned(ui_tickets::ticked_lines(&[], &ticks(&[(10, 0)]), "ABC")), vec![]);
    }
}

mod focus_row {
    use super::*;

    #[test]
    fn takes_the_rows_project_if_it_has_one_ticks_exactly_its_lines_and_becomes_the_origin() {
        // (name, row, group project, lines, expected project, expected ticks); the creator starts
        // in ABC with origin 5 and ticks of other rows and of row 30 itself.
        type Row = (&'static str, u64, Option<&'static str>, &'static [&'static str], &'static str, Vec<(u64, usize)>);
        let rows: [Row; 3] = [
            ("row of another project", 30, Some("XYZ"), &["pom.xml", "XYZ-4 export", "README.md"], "XYZ", vec![(30, 0), (30, 1), (30, 2)]),
            ("row without project keeps the selected one", 20, None, &["notes.md", "typo fix"], "ABC", vec![(20, 0), (20, 1)]),
            ("row without lines clears the ticks", 40, Some("XY"), &[], "XY", vec![]),
        ];
        for (name, row, project, lines, expected_project, expected_ticks) in rows {
            let mut c = Creator::new("ABC".into(), Some(5));
            c.ticks = ticks(&[(10, 0), (77, 1), (30, 9)]);
            c.focus_row(&group(row, project, &["repo"], lines));
            assert_eq!((c.project.as_str(), c.ticks.clone(), c.origin), (expected_project, ticks(&expected_ticks), Some(row)), "{name}");
        }
    }
}

mod add_empty_draft {
    use super::*;

    fn run(project: &str, origin: Option<u64>, t: &[(u64, usize)]) -> Vec<Snapshot> {
        let mut c = Creator::new(project.into(), origin);
        c.drafts = vec![d("Existing", "e", false, Some(Ok("ABC-3")))];
        c.ticks = ticks(t);
        c.add_empty_draft(&groups());
        assert_eq!(c.drafts.iter().map(|x| x.project.as_str()).collect::<Vec<_>>(), vec!["XYZ", project]);
        snapshot(&c)
    }

    #[test]
    fn rows_of_visible_ticks_else_origin_else_none() {
        // (name, project, origin, ticks, rows of the new draft)
        type Row = (&'static str, &'static str, Option<u64>, Vec<(u64, usize)>, Vec<u64>);
        // Origins: 10 and 50 are ABC rows (50 spelled "abc"), 30 a XYZ row, 20 a row without
        // project (visible everywhere), 99 no row at all.
        let rows: [Row; 10] = [
            ("distinct ticked rows in group order, origin ignored", "ABC", Some(30), vec![(10, 2), (50, 1), (10, 0), (20, 0)], vec![50, 10, 20]),
            ("one ticked row", "ABC", None, vec![(10, 1)], vec![10]),
            ("nothing ticked, origin visible in the project", "ABC", Some(10), vec![], vec![10]),
            ("nothing ticked, origin's project in other case", "ABC", Some(50), vec![], vec![50]),
            ("nothing ticked, origin of another project", "ABC", Some(30), vec![], vec![]),
            ("nothing ticked, origin without project", "XYZ", Some(20), vec![], vec![20]),
            ("only hidden ticks, origin without project", "ABC", Some(20), vec![(30, 0), (30, 1)], vec![20]),
            ("ticks past the lines, origin visible", "XYZ", Some(30), vec![(30, 4)], vec![30]),
            ("nothing ticked, origin missing from the groups", "ABC", Some(99), vec![], vec![]),
            ("nothing ticked and no origin", "ABC", None, vec![(99, 0)], vec![]),
        ];
        for (name, project, origin, t, expected) in rows {
            assert_eq!(
                run(project, origin, &t),
                vec![snap("Existing", "e", false, Some(Ok("ABC-3")), &[]), snap("", "", true, None, &expected)],
                "{name}"
            );
        }
    }

    fn sources(project: &str, origin: Option<u64>, t: &[(u64, usize)]) -> Vec<WorkItem> {
        let mut c = Creator::new(project.into(), origin);
        c.ticks = ticks(t);
        c.add_empty_draft(&groups());
        assert_eq!(c.drafts.len(), 1);
        c.drafts.remove(0).sources
    }

    fn lines(v: &[&str]) -> Vec<WorkItem> {
        v.iter().map(|l| WorkItem { line: l.to_string(), change: None }).collect()
    }

    #[test]
    fn sources_are_the_visible_ticked_items_else_the_lines_of_the_drafts_rows() {
        // (name, project, origin, ticks, expected source lines)
        type Row = (&'static str, &'static str, Option<u64>, Vec<(u64, usize)>, Vec<WorkItem>);
        let rows: [Row; 6] = [
            (
                "ticked items as listed, in group order, hidden ticks left out",
                "ABC",
                Some(30),
                vec![(10, 2), (50, 1), (10, 0), (20, 1), (30, 0)],
                lines(&["[svc] src/main.rs", "[api, web] ABC-1 fix login", "[api, web] src/a.rs", "typo fix"]),
            ),
            ("one ticked line", "XYZ", None, vec![(30, 1)], lines(&["[xyz] XYZ-4 export"])),
            (
                "nothing ticked: every line of the origin row, with its repos like a ticked line",
                "ABC",
                Some(10),
                vec![],
                lines(&["[api, web] ABC-1 fix login", "[api, web] branch feature/sso", "[api, web] src/a.rs"]),
            ),
            ("nothing ticked, origin row without repos: bare lines", "XYZ", Some(20), vec![], lines(&["notes.md", "typo fix"])),
            ("nothing ticked, origin of another project: none", "ABC", Some(30), vec![], vec![]),
            ("nothing ticked and no origin: none", "ABC", None, vec![], vec![]),
        ];
        for (name, project, origin, t, expected) in rows {
            assert_eq!(sources(project, origin, &t), expected, "{name}");
        }
    }
}

mod draft_project {
    use super::*;

    fn projects(c: &Creator) -> Vec<String> {
        c.drafts.iter().map(|x| x.project.clone()).collect()
    }

    fn job_targets(c: &Creator) -> Vec<(usize, String, String)> {
        let cfg = TicketConfig { opencode_config: None, ..TicketConfig::default() };
        c.create_jobs(&cfg, "jdoe").into_iter().map(|(i, n)| (i, n.project, n.summary)).collect()
    }

    fn proposed(titles: &[&str]) -> Vec<TicketDraft> {
        titles.iter().map(|t| TicketDraft { title: t.to_string(), description: String::new(), items: vec![] }).collect()
    }

    #[test]
    fn drafts_stay_in_the_project_they_were_written_for() {
        let mut c = Creator::new("ABC".into(), None);
        c.apply_drafts(Ok(proposed(&["Fix login", "Add export"])), &[]);
        c.project = "XYZ".into();
        assert_eq!(job_targets(&c), vec![(0, "ABC".into(), "Fix login".into()), (1, "ABC".into(), "Add export".into())]);

        c.add_empty_draft(&[]);
        c.drafts[2].title = "Written by hand".into();
        assert_eq!(projects(&c), vec!["ABC".to_string(), "ABC".to_string(), "XYZ".to_string()]);
        assert_eq!(
            job_targets(&c),
            vec![(0, "ABC".into(), "Fix login".into()), (1, "ABC".into(), "Add export".into()), (2, "XYZ".into(), "Written by hand".into())]
        );
    }

    #[test]
    fn new_proposals_take_the_current_project_created_drafts_keep_theirs() {
        let mut c = Creator::new("XY".into(), None);
        c.drafts = vec![dp("ABC", "Done", "", true, Some(Ok("ABC-4")), &[]), dp("ABC", "Unsent", "", true, None, &[])];
        c.apply_drafts(Ok(proposed(&["Proposed"])), &[]);
        assert_eq!(projects(&c), vec!["ABC".to_string(), "XY".to_string()]);
        assert_eq!(job_targets(&c), vec![(1, "XY".into(), "Proposed".into())]);
    }
}

mod create_jobs {
    use super::*;

    fn cfg() -> TicketConfig {
        TicketConfig {
            issue_type: "Bug".into(),
            fields: json!({"customfield_10000": {"value": "maintenance"}, "labels": ["wl"]}).as_object().unwrap().clone(),
            opencode_config: None,
            ..TicketConfig::default()
        }
    }

    fn job(summary: &str, description: &str) -> NewIssue {
        NewIssue {
            project: "XYZ".into(),
            issue_type: "Bug".into(),
            summary: summary.into(),
            description: description.into(),
            assignee: Some("jdoe".into()),
            extra_fields: cfg().fields,
        }
    }

    #[test]
    fn selected_titled_uncreated_drafts_by_index() {
        let c = creator(vec![
            d("Not selected", "x", false, None),
            d("  Fix login  ", "\n Naprawiono SSO. \n", true, None),
            d("Already created", "y", true, Some(Ok("XYZ-5"))),
            d(" \t ", "blank title", true, None),
            d("Retry me", "", true, Some(Err("Jira 500"))),
        ]);
        assert_eq!(c.create_jobs(&cfg(), "jdoe"), vec![(1, job("Fix login", "Naprawiono SSO.")), (4, job("Retry me", ""))]);
    }

    #[test]
    fn each_condition_alone_excludes_the_draft() {
        // (name, the single draft at index 0) - every row breaks exactly one condition.
        let rows = [
            ("not selected", d("T", "x", false, None)),
            ("created", d("T", "x", true, Some(Ok("ABC-1")))),
            ("empty title", d("", "x", true, None)),
            ("blank title", d("  \n", "x", true, None)),
        ];
        for (name, draft) in rows {
            let c = creator(vec![d("Eligible", "e", true, None), draft]);
            assert_eq!(c.create_jobs(&cfg(), "jdoe"), vec![(0, job("Eligible", "e"))], "{name}");
        }
    }

    #[test]
    fn project_from_the_draft_type_and_fields_from_config_assignee_is_me() {
        let mut c = Creator::new("XYZ".into(), None);
        c.drafts = vec![dp("ABC", "Title", "Body", true, None, &[])];
        let cfg = TicketConfig { issue_type: "Story".into(), fields: serde_json::Map::new(), ..cfg() };
        assert_eq!(
            c.create_jobs(&cfg, "anowak"),
            vec![(
                0,
                NewIssue {
                    project: "ABC".into(),
                    issue_type: "Story".into(),
                    summary: "Title".into(),
                    description: "Body".into(),
                    assignee: Some("anowak".into()),
                    extra_fields: serde_json::Map::new(),
                }
            )]
        );
    }
}

mod apply_drafts {
    use super::*;

    fn new_drafts(n: usize) -> Vec<TicketDraft> {
        (1..=n).map(|i| TicketDraft { title: format!("New {i}"), description: format!("nd{i}"), items: vec![] }).collect()
    }

    fn with_items(lists: &[&[usize]]) -> Vec<TicketDraft> {
        lists
            .iter()
            .enumerate()
            .map(|(i, l)| TicketDraft { title: format!("T{i}"), description: format!("D{i}"), items: l.to_vec() })
            .collect()
    }

    #[test]
    fn keeps_created_replaces_the_rest_up_to_three() {
        // No items sent, so no new draft gets rows. (name, drafts before, new drafts offered,
        // expected drafts after)
        let rows: Vec<(&str, Vec<Draft>, usize, Vec<Snapshot>)> = vec![
            (
                "no drafts yet",
                vec![],
                3,
                vec![snap("New 1", "nd1", true, None, &[]), snap("New 2", "nd2", true, None, &[]), snap("New 3", "nd3", true, None, &[])],
            ),
            (
                "one created kept first with its rows, uncreated and failed dropped, cap 3",
                vec![
                    dr("Unsent", "u", false, None, &[1]),
                    dr("Done", "c", false, Some(Ok("XYZ-1")), &[8, 4]),
                    dr("Failed", "f", true, Some(Err("Jira 400")), &[2]),
                ],
                3,
                vec![
                    snap("Done", "c", false, Some(Ok("XYZ-1")), &[8, 4]),
                    snap("New 1", "nd1", true, None, &[]),
                    snap("New 2", "nd2", true, None, &[]),
                ],
            ),
            (
                "two created leave room for one",
                vec![d("Done A", "a", true, Some(Ok("XYZ-1"))), d("Unsent", "u", true, None), d("Done B", "b", true, Some(Ok("XYZ-2")))],
                2,
                vec![
                    snap("Done A", "a", true, Some(Ok("XYZ-1")), &[]),
                    snap("Done B", "b", true, Some(Ok("XYZ-2")), &[]),
                    snap("New 1", "nd1", true, None, &[]),
                ],
            ),
            (
                "three created leave no room",
                vec![d("A", "a", true, Some(Ok("ABC-1"))), d("B", "b", true, Some(Ok("ABC-2"))), d("C", "c", true, Some(Ok("ABC-3")))],
                1,
                vec![
                    snap("A", "a", true, Some(Ok("ABC-1")), &[]),
                    snap("B", "b", true, Some(Ok("ABC-2")), &[]),
                    snap("C", "c", true, Some(Ok("ABC-3")), &[]),
                ],
            ),
            ("fewer new drafts than room", vec![d("Unsent", "u", true, None)], 1, vec![snap("New 1", "nd1", true, None, &[])]),
        ];
        for (name, before, n, expected) in rows {
            let mut c = creator(before);
            c.error = Some("previous failure".into());
            c.apply_drafts(Ok(new_drafts(n)), &[]);
            assert_eq!(snapshot(&c), expected, "{name}");
            assert_eq!(c.error, None, "{name}");
        }
    }

    #[test]
    fn each_draft_gets_the_distinct_rows_of_its_items() {
        // Items sent: 0 -> row 10, 1 -> row 20, 2 -> row 10, 3 -> row 30, 4 -> row 50.
        let item_rows = [10, 20, 10, 30, 50];
        // (name, items per draft, expected rows per draft)
        type Row = (&'static str, Vec<&'static [usize]>, Vec<Vec<u64>>);
        let rows: [Row; 9] = [
            ("duplicates collapse", vec![&[0, 2, 3]], vec![vec![10, 30]]),
            ("first-seen order, not row order", vec![&[3, 1, 0]], vec![vec![30, 20, 10]]),
            ("several drafts", vec![&[0, 2], &[1, 3], &[4]], vec![vec![10], vec![20, 30], vec![50]]),
            ("out-of-range items ignored", vec![&[1, 9], &[5, 4]], vec![vec![20], vec![50]]),
            ("lone draft naming some items gets only those", vec![&[4]], vec![vec![50]]),
            ("lone draft naming no item gets every row", vec![&[]], vec![vec![10, 20, 30, 50]]),
            ("lone draft naming only unknown items gets every row", vec![&[5, 8]], vec![vec![10, 20, 30, 50]]),
            ("item-less draft among several gets no rows", vec![&[], &[4]], vec![vec![], vec![50]]),
            ("unknown-only draft among several gets no rows", vec![&[1], &[7]], vec![vec![20], vec![]]),
        ];
        for (name, lists, expected) in rows {
            let mut c = creator(vec![]);
            c.apply_drafts(Ok(with_items(&lists)), &item_rows);
            let got: Vec<Vec<u64>> = c.drafts.iter().map(|x| x.rows.clone()).collect();
            assert_eq!(got, expected, "{name}");
            let titles: Vec<String> = c.drafts.iter().map(|x| x.title.clone()).collect();
            let want: Vec<String> = (0..lists.len()).map(|i| format!("T{i}")).collect();
            assert_eq!(titles, want, "{name}");
        }
    }

    #[test]
    fn lone_draft_without_items_and_no_items_sent_gets_no_rows() {
        let mut c = creator(vec![]);
        c.apply_drafts(Ok(with_items(&[&[]])), &[]);
        assert_eq!(snapshot(&c), vec![snap("T0", "D0", true, None, &[])]);
    }

    #[test]
    fn lone_means_one_draft_in_the_answer_not_one_that_fits() {
        let created = || vec![d("A", "a", true, Some(Ok("ABC-1"))), d("B", "b", true, Some(Ok("ABC-2")))];
        let item_rows = [10, 20];

        // Two drafts offered, one fits: the item-less first is not lone, so it gets no rows.
        let mut c = creator(created());
        c.apply_drafts(Ok(with_items(&[&[], &[1]])), &item_rows);
        assert_eq!(snapshot(&c)[2], snap("T0", "D0", true, None, &[]));
        assert_eq!(c.drafts.len(), 3);

        // One draft offered next to created ones: it is lone and gets every row.
        let mut c = creator(created());
        c.apply_drafts(Ok(with_items(&[&[]])), &item_rows);
        assert_eq!(snapshot(&c)[2], snap("T0", "D0", true, None, &[10, 20]));
    }

    #[test]
    fn error_is_kept_with_its_chain_and_drafts_are_untouched() {
        let mut c = creator(vec![dr("Unsent", "u", true, None, &[3]), dr("Done", "c", false, Some(Ok("XYZ-1")), &[4, 6])]);
        c.apply_drafts(Err(anyhow!("LLM 503: overloaded").context("drafting tickets")), &[10, 20]);
        assert_eq!(c.error.as_deref(), Some("drafting tickets: LLM 503: overloaded"));
        assert_eq!(snapshot(&c), vec![snap("Unsent", "u", true, None, &[3]), snap("Done", "c", false, Some(Ok("XYZ-1")), &[4, 6])]);
    }
}

mod apply_create_result {
    use super::*;

    fn created(o: Option<Outcome>) -> Option<(Issue, Vec<u64>)> {
        o.map(|Outcome::Created { issue, rows }| (issue, rows))
    }

    fn new_issue(key: &str, summary: &str) -> Issue {
        Issue { key: key.into(), summary: summary.into(), status: "new".into() }
    }

    #[test]
    fn success_yields_the_ticket_and_the_drafts_rows_failure_is_recorded() {
        let mut c = creator(vec![
            dr("  Zero  ", "", true, None, &[7]),
            dr("One", "", true, None, &[3, 9]),
            dr("Two", "", true, None, &[12, 5]),
        ]);

        assert!(created(c.apply_create_result(1, Err("Jira 500: boom".into()))).is_none());
        assert_eq!(created(c.apply_create_result(2, Ok("XYZ-9".into()))), Some((new_issue("XYZ-9", "Two"), vec![12, 5])));
        assert_eq!(created(c.apply_create_result(0, Ok("XYZ-10".into()))), Some((new_issue("XYZ-10", "Zero"), vec![7])));

        assert_eq!(
            snapshot(&c),
            vec![
                snap("  Zero  ", "", true, Some(Ok("XYZ-10")), &[7]),
                snap("One", "", true, Some(Err("Jira 500: boom")), &[3, 9]),
                snap("Two", "", true, Some(Ok("XYZ-9")), &[12, 5]),
            ]
        );
    }

    #[test]
    fn retried_draft_replaces_its_failure() {
        let mut c = creator(vec![d("Zero", "", true, None), dr("One", "", true, Some(Err("Jira 500")), &[4])]);
        assert_eq!(created(c.apply_create_result(1, Ok("ABC-77".into()))), Some((new_issue("ABC-77", "One"), vec![4])));
        assert_eq!(snapshot(&c)[1], snap("One", "", true, Some(Ok("ABC-77")), &[4]));
    }

    #[test]
    fn index_out_of_range_changes_nothing() {
        let mut c = creator(vec![dr("Zero", "", true, None, &[2])]);
        assert!(created(c.apply_create_result(1, Ok("ABC-5".into()))).is_none());
        assert_eq!(snapshot(&c), vec![snap("Zero", "", true, None, &[2])]);
    }

    #[test]
    fn draft_without_rows_yields_no_rows() {
        let mut c = Creator::new("ABC".into(), None);
        c.drafts = vec![d("Zero", "", true, None)];
        assert_eq!(created(c.apply_create_result(0, Ok("ABC-6".into()))), Some((new_issue("ABC-6", "Zero"), vec![])));
    }
}

fn wi(line: &str, change: Option<&str>) -> WorkItem {
    WorkItem { line: line.into(), change: change.map(str::to_owned) }
}

fn with_sources(mut d: Draft, sources: Vec<WorkItem>) -> Draft {
    d.sources = sources;
    d
}

mod attach_sources {
    use super::*;

    fn items() -> Vec<WorkItem> {
        vec![wi("[api] ABC-1 fix login", Some("+login")), wi("typo fix", None), wi("[web] src/a.rs", Some("+a"))]
    }

    fn sources(c: &Creator) -> Vec<Vec<WorkItem>> {
        c.drafts.iter().map(|x| x.sources.clone()).collect()
    }

    #[test]
    fn drafts_after_the_kept_ones_get_the_items_they_name_else_all() {
        let kept_sources = vec![wi("kept line", Some("kept diff"))];
        let mut c = creator(vec![
            with_sources(d("Done", "c", true, Some(Ok("XYZ-1"))), kept_sources.clone()),
            d("Names one", "", true, None),
            d("Names none", "", true, None),
            d("Names only unknown", "", true, None),
            d("Names two and an unknown", "", true, None),
        ]);
        c.attach_sources(1, &[vec![2], vec![], vec![7, 9], vec![2, 0, 5]], &items());
        let all = items();
        assert_eq!(
            sources(&c),
            vec![kept_sources, vec![all[2].clone()], all.clone(), all.clone(), vec![all[2].clone(), all[0].clone()]]
        );
    }

    #[test]
    fn nothing_kept_starts_at_the_first_draft() {
        let mut c = creator(vec![d("A", "", true, None), d("B", "", true, None)]);
        c.attach_sources(0, &[vec![1], vec![0]], &items());
        assert_eq!(sources(&c), vec![vec![items()[1].clone()], vec![items()[0].clone()]]);
    }

    #[test]
    fn drafts_without_an_item_set_keep_their_sources() {
        let old = vec![wi("old", None)];
        let mut c = creator(vec![with_sources(d("A", "", true, None), old.clone()), with_sources(d("B", "", true, None), old.clone())]);
        c.attach_sources(0, &[vec![2]], &items());
        assert_eq!(sources(&c), vec![vec![items()[2].clone()], old]);
    }

    #[test]
    fn no_items_sent_gives_empty_sources() {
        let mut c = creator(vec![with_sources(d("A", "", true, None), vec![wi("old", None)])]);
        c.attach_sources(0, &[vec![0]], &[]);
        assert_eq!(sources(&c), vec![Vec::<WorkItem>::new()]);
    }
}

mod job_sources {
    use super::*;

    fn cfg() -> TicketConfig {
        TicketConfig { opencode_config: None, ..TicketConfig::default() }
    }

    #[test]
    fn sources_of_each_jobs_draft_in_job_order_skipping_unsendable_drafts() {
        let s = |n: &str| vec![wi(&format!("line {n}"), Some(&format!("diff {n}")))];
        // Draft 0 is not selected and draft 2 already created: the jobs are drafts 1 and 3.
        let c = creator(vec![
            with_sources(d("Not selected", "", false, None), s("zero")),
            with_sources(d("First job", "", true, None), s("one")),
            with_sources(d("Created", "", true, Some(Ok("XYZ-2"))), s("two")),
            with_sources(d("Second job", "", true, None), s("three")),
        ]);
        let jobs = c.create_jobs(&cfg(), "jdoe");
        assert_eq!(jobs.iter().map(|(i, _)| *i).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(c.job_sources(&jobs), vec![s("one"), s("three")]);
    }

    #[test]
    fn follows_the_jobs_order_and_an_unknown_index_gives_no_sources() {
        let c = creator(vec![with_sources(d("A", "", true, None), vec![wi("a", None)]), with_sources(d("B", "", true, None), vec![wi("b", Some("+b"))])]);
        let job = |i: usize| (i, c.create_jobs(&cfg(), "jdoe")[0].1.clone());
        assert_eq!(c.job_sources(&[job(1), job(9), job(0)]), vec![vec![wi("b", Some("+b"))], vec![], vec![wi("a", None)]]);
        assert_eq!(c.job_sources(&[]), Vec::<Vec<WorkItem>>::new());
    }
}

mod change_lookup {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use chrono::{Local, NaiveDate, TimeZone};
    use worklog::git::Span;
    use worklog::ui_tickets::CodeSource;

    /// Two days and a few hours, so neither end sits on a midnight a day-only lookup would assume.
    fn span() -> Span {
        let at = |d: u32, h: u32, m: u32| Local.from_local_datetime(&NaiveDate::from_ymd_opt(2026, 11, d).unwrap().and_hms_opt(h, m, 0).unwrap()).single().unwrap();
        Span { from: at(2, 9, 15), to: at(4, 17, 40) }
    }

    fn repo_paths() -> BTreeMap<String, Vec<PathBuf>> {
        BTreeMap::from([
            ("api".to_string(), vec![PathBuf::from(r"C:\r\api"), PathBuf::from(r"D:\fork\api")]),
            ("web".to_string(), vec![PathBuf::from(r"C:\r\web")]),
        ])
    }

    type Lookup = (Vec<(Vec<PathBuf>, String)>, Option<Span>);

    fn lookup(code: bool, send_code: bool) -> Lookup {
        let (both, none, unknown_first) = (strings(&["web", "api"]), strings(&[]), strings(&["gone", "web"]));
        let ticked: Vec<(u64, &[String], &str)> =
            vec![(10, both.as_slice(), "ABC-1 fix login"), (20, none.as_slice(), "typo fix"), (30, unknown_first.as_slice(), "uncommitted pom.xml")];
        let paths = repo_paths();
        let source = CodeSource { span: span(), repo_paths: &paths };
        ui_tickets::change_lookup(&ticked, code.then_some(&source), send_code)
    }

    fn p(v: &[&str]) -> Vec<PathBuf> {
        v.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn with_code_each_lines_repo_paths_in_repo_order_and_the_day() {
        assert_eq!(
            lookup(true, true),
            (
                vec![
                    (p(&[r"C:\r\web", r"C:\r\api", r"D:\fork\api"]), "ABC-1 fix login".to_string()),
                    (vec![], "typo fix".to_string()),
                    (p(&[r"C:\r\web"]), "uncommitted pom.xml".to_string()),
                ],
                Some(span())
            )
        );
    }

    #[test]
    fn send_code_off_or_no_report_gives_no_paths_and_no_day() {
        let bare: Lookup =
            (vec![(vec![], "ABC-1 fix login".to_string()), (vec![], "typo fix".to_string()), (vec![], "uncommitted pom.xml".to_string())], None);
        // (name, report present, send_code)
        for (name, code, send_code) in [("send_code off", true, false), ("no report", false, true), ("neither", false, false)] {
            assert_eq!(lookup(code, send_code), bare, "{name}");
        }
    }

    #[test]
    fn nothing_ticked_still_reports_the_day_when_code_is_sent() {
        let paths = repo_paths();
        let source = CodeSource { span: span(), repo_paths: &paths };
        assert_eq!(ui_tickets::change_lookup(&[], Some(&source), true), (vec![], Some(span())));
        assert_eq!(ui_tickets::change_lookup(&[], Some(&source), false), (vec![], None));
    }
}
