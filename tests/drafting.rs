//! The four-step ticket drafting (analyst, writer, tester, reviewer) driven by a scripted chat;
//! no request leaves the process.

use std::cell::RefCell;
use std::collections::VecDeque;

use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use worklog::drafting::{self, Chat, CurrentTicket, Stage, WorkItem};
use worklog::llm::TicketDraft;

/// Answers in order; records the (system, user) of every call. Runs out with an error, so a call
/// the test did not expect fails the pipeline instead of hanging.
struct Scripted {
    answers: RefCell<VecDeque<Result<String>>>,
    calls: RefCell<Vec<(String, String)>>,
}

impl Chat for Scripted {
    fn chat(&self, system: &str, user: &str) -> Result<String> {
        self.calls.borrow_mut().push((system.to_owned(), user.to_owned()));
        self.answers.borrow_mut().pop_front().unwrap_or_else(|| Err(anyhow!("unscripted call")))
    }
}

struct Run {
    result: Result<Vec<TicketDraft>>,
    calls: Vec<(String, String)>,
    /// Each stage reported, with the number of chat calls made before it.
    progress: Vec<(Stage, usize)>,
}

fn run_with(project: &str, language: &str, items: &[WorkItem], answers: Vec<Result<String>>) -> Run {
    let chat = Scripted { answers: RefCell::new(answers.into()), calls: RefCell::new(Vec::new()) };
    let progress = RefCell::new(Vec::new());
    let report = |s: Stage| progress.borrow_mut().push((s, chat.calls.borrow().len()));
    let result = drafting::draft_tickets(&chat, project, language, items, &report);
    Run { result, calls: chat.calls.into_inner(), progress: progress.into_inner() }
}

fn run(answers: Vec<Result<String>>) -> Run {
    run_with("Alpha", "Polish", &items(), answers)
}

fn ok(v: Value) -> Result<String> {
    Ok(v.to_string())
}

fn fail(msg: &str) -> Result<String> {
    Err(anyhow!(msg.to_owned()))
}

fn chain(e: &anyhow::Error) -> Vec<String> {
    e.chain().map(|c| c.to_string()).collect()
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn draft(title: &str, description: &str, items: &[usize]) -> TicketDraft {
    TicketDraft { title: title.into(), description: description.into(), items: items.to_vec() }
}

const DIFF_1: &str = "diff --git a/src/export.rs b/src/export.rs\n+pub fn export_csv() {}\n\n";
const DIFF_3: &str = "ABC-8 fix timezone\n\n-let tz = utc;\n+let tz = local;";

fn items() -> Vec<WorkItem> {
    vec![
        WorkItem { line: "[api] ABC-7 add CSV export".into(), change: Some(DIFF_1.into()) },
        WorkItem { line: "[web] uncommitted src/view.ts".into(), change: None },
        WorkItem { line: "[svc] ABC-8 fix timezone".into(), change: Some(DIFF_3.into()) },
    ]
}

/// Two tasks: the first covers items 3 and 1 (named twice), the second item 2.
fn analysis() -> Value {
    json!({"tasks": [
        {"items": [3, 1, 1], "area": "CSV export", "problem": "Auditors cannot get the rows.", "required_behaviour": "A CSV download.", "keep_working": "The table view.", "testable": true, "unclear": ""},
        {"items": [2], "area": "Report view", "problem": "Dates shift.", "required_behaviour": "Local dates.", "keep_working": "Sorting.", "testable": false, "unclear": "why"}
    ]})
}

fn writer() -> Value {
    json!({"tickets": [
        {"task": 1, "title": " Eksport wierszy do CSV ", "summary": " Audytorzy nie mogą pobrać wierszy. ", "criteria": [" Przycisk pobiera plik CSV ", "", 5, "Plik ma nagłówek"]},
        {"task": 2, "title": "Daty w raporcie w czasie lokalnym", "summary": "Daty przesuwają się o godzinę.", "criteria": ["Data 2026-03-29 02:30 pokazuje się bez przesunięcia"]}
    ]})
}

fn tester() -> Value {
    json!({"tests": [
        {"ticket": 1, "cases": ["Tabela z 2 wierszami -> plik z 3 liniami", "  ", "Pusta tabela -> sam nagłówek"]},
        {"ticket": 2, "cases": ["Strefa Europe/Warsaw, 29.03 -> 02:30 bez zmiany"]}
    ]})
}

/// The writer's tickets as the tester and reviewer see them: trimmed, blank or non-text criteria
/// dropped.
fn written_tickets() -> Value {
    json!({"tickets": [
        {"title": "Eksport wierszy do CSV", "summary": "Audytorzy nie mogą pobrać wierszy.", "criteria": ["Przycisk pobiera plik CSV", "Plik ma nagłówek"]},
        {"title": "Daty w raporcie w czasie lokalnym", "summary": "Daty przesuwają się o godzinę.", "criteria": ["Data 2026-03-29 02:30 pokazuje się bez przesunięcia"]}
    ]})
}

fn tested_tickets() -> Value {
    json!({"tickets": [
        {"title": "Eksport wierszy do CSV", "summary": "Audytorzy nie mogą pobrać wierszy.", "criteria": ["Przycisk pobiera plik CSV", "Plik ma nagłówek"], "tests": ["Tabela z 2 wierszami -> plik z 3 liniami", "Pusta tabela -> sam nagłówek"]},
        {"title": "Daty w raporcie w czasie lokalnym", "summary": "Daty przesuwają się o godzinę.", "criteria": ["Data 2026-03-29 02:30 pokazuje się bez przesunięcia"], "tests": ["Strefa Europe/Warsaw, 29.03 -> 02:30 bez zmiany"]}
    ]})
}

/// Descriptions of the two written tickets, filled in by hand from the markup rules.
const DESC_1_WITH_TESTS: &str = "h3. Opis\nAudytorzy nie mogą pobrać wierszy.\n\nh3. Kryteria akceptacji\n* Przycisk pobiera plik CSV\n* Plik ma nagłówek\n\nh3. Przypadki testowe\n# Tabela z 2 wierszami -> plik z 3 liniami\n# Pusta tabela -> sam nagłówek";
const DESC_2_WITH_TESTS: &str = "h3. Opis\nDaty przesuwają się o godzinę.\n\nh3. Kryteria akceptacji\n* Data 2026-03-29 02:30 pokazuje się bez przesunięcia\n\nh3. Przypadki testowe\n# Strefa Europe/Warsaw, 29.03 -> 02:30 bez zmiany";
const DESC_1_NO_TESTS: &str = "h3. Opis\nAudytorzy nie mogą pobrać wierszy.\n\nh3. Kryteria akceptacji\n* Przycisk pobiera plik CSV\n* Plik ma nagłówek";
const DESC_2_NO_TESTS: &str = "h3. Opis\nDaty przesuwają się o godzinę.\n\nh3. Kryteria akceptacji\n* Data 2026-03-29 02:30 pokazuje się bez przesunięcia";

const NUMBERED: &str = "1. [api] ABC-7 add CSV export\n2. [web] uncommitted src/view.ts\n3. [svc] ABC-8 fix timezone\n";

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap()
}

mod stage {
    use super::*;

    #[test]
    fn numbers_and_labels_in_pipeline_order() {
        assert_eq!(Stage::ALL, [Stage::Analyst, Stage::Writer, Stage::Tester, Stage::Reviewer]);
        let got: Vec<(usize, &str)> = Stage::ALL.iter().map(|s| (s.number(), s.label())).collect();
        assert_eq!(
            got,
            vec![
                (1, "analyst reads the changes"),
                (2, "writer drafts the tickets"),
                (3, "tester adds test scenarios"),
                (4, "reviewer checks the tickets"),
            ]
        );
    }
}

mod render_description {
    use super::*;

    #[test]
    fn section_names_polish_for_pol_prefix_any_case_english_otherwise() {
        let pl = ["Opis", "Kryteria akceptacji", "Przypadki testowe"];
        let en = ["Description", "Acceptance criteria", "Test cases"];
        // (language, expected)
        let rows = [
            ("Polish", pl),
            ("polski", pl),
            ("POLSKI", pl),
            ("pol", pl),
            ("English", en),
            ("German", en),
            ("", en),
            ("po", en),
            (" Polish", en),
        ];
        for (language, expected) in rows {
            assert_eq!(drafting::section_names(language), expected, "{language:?}");
        }
    }

    #[test]
    fn sections_markup_and_omissions() {
        // (name, language, summary, criteria, tests, expected)
        type Row = (&'static str, &'static str, &'static str, &'static [&'static str], &'static [&'static str], &'static str);
        let rows: [Row; 9] = [
            ("all three, Polish", "Polish", "Sum A.", &["c1", "c2"], &["t1", "t2"], "h3. Opis\nSum A.\n\nh3. Kryteria akceptacji\n* c1\n* c2\n\nh3. Przypadki testowe\n# t1\n# t2"),
            ("all three, English", "English", "Sum B.", &["c3"], &["t3"], "h3. Description\nSum B.\n\nh3. Acceptance criteria\n* c3\n\nh3. Test cases\n# t3"),
            ("summary trimmed, inner lines kept", "English", "  line 1\nline 2 \n", &[], &[], "h3. Description\nline 1\nline 2"),
            ("blank summary left out", "English", " \n ", &["c4"], &["t4"], "h3. Acceptance criteria\n* c4\n\nh3. Test cases\n# t4"),
            ("no criteria left out", "English", "Sum C.", &[], &["t5"], "h3. Description\nSum C.\n\nh3. Test cases\n# t5"),
            ("no tests left out", "Polish", "Sum D.", &["c6"], &[], "h3. Opis\nSum D.\n\nh3. Kryteria akceptacji\n* c6"),
            ("only tests", "Polish", "", &[], &["t7"], "h3. Przypadki testowe\n# t7"),
            ("nothing at all", "Polish", "", &[], &[], ""),
            ("entries that are only markers or blank count as none", "English", "Sum E.", &["", "  ", "*", "- ", "1.", "**", " -- "], &["#", "•", "2)", "## ", "10.\t"], "h3. Description\nSum E."),
        ];
        for (name, language, summary, criteria, tests, expected) in rows {
            assert_eq!(drafting::render_description(language, summary, &strings(criteria), &strings(tests)), expected, "{name}");
        }
    }

    #[test]
    fn list_entries_lose_markers_and_inner_newlines() {
        // (entry, expected list text)
        let rows = [
            ("* star", "star"),
            ("- dash", "dash"),
            ("• bullet", "bullet"),
            ("# hash", "hash"),
            ("** double star", "double star"),
            ("1. numbered", "numbered"),
            ("2) paren", "paren"),
            ("10. two digits", "two digits"),
            ("  - indented dash  ", "indented dash"),
            ("first line\nsecond line", "first line second line"),
            ("Kod 404 dla ż", "Kod 404 dla ż"),
            ("404 returned", "404 returned"),
            ("3 rows -> 4 lines", "3 rows -> 4 lines"),
            // A marker counts only when whitespace or the end follows it.
            ("-1 is rejected", "-1 is rejected"),
            ("1.5 h are logged", "1.5 h are logged"),
            ("*Admin* can export", "*Admin* can export"),
            ("#3 is first", "#3 is first"),
            ("2)x stays", "2)x stays"),
            ("--force is refused", "--force is refused"),
            ("-\tTab after dash", "Tab after dash"),
            ("3.\tTab after number", "Tab after number"),
            ("* -1 after a bullet", "-1 after a bullet"),
            ("12) twelve", "twelve"),
            // Every whitespace run inside becomes one space.
            ("a\r\nb\t\tc   d", "a b c d"),
            ("x - y", "x - y"),
        ];
        for (entry, expected) in rows {
            let entries = strings(&[entry]);
            assert_eq!(drafting::render_description("English", "", &entries, &[]), format!("h3. Acceptance criteria\n* {expected}"), "criterion {entry:?}");
            assert_eq!(drafting::render_description("English", "", &[], &entries), format!("h3. Test cases\n# {expected}"), "test {entry:?}");
        }
    }
}

mod prompts {
    use super::*;

    fn happy() -> Run {
        run(vec![ok(analysis()), ok(writer()), ok(tester()), ok(json!({"tickets": [{"title": ""}, {"title": ""}]}))])
    }

    #[test]
    fn analyst_gets_project_and_numbered_items_each_with_its_change_or_a_note() {
        let r = happy();
        let expected = "Jira project: Alpha\nWork items:\n\
            1. [api] ABC-7 add CSV export\n```\ndiff --git a/src/export.rs b/src/export.rs\n+pub fn export_csv() {}\n```\n\
            2. [web] uncommitted src/view.ts\n(no code change available)\n\
            3. [svc] ABC-8 fix timezone\n```\nABC-8 fix timezone\n\n-let tz = utc;\n+let tz = local;\n```\n";
        assert_eq!(r.calls[0].1, expected);
        assert_eq!(r.calls[0], drafting::analyst_prompt("Alpha", &items()));
    }

    #[test]
    fn writer_tester_reviewer_get_the_analysis_and_never_the_diffs() {
        let r = happy();
        assert_eq!(r.calls.len(), 4);
        let notes = pretty(&analysis());
        assert_eq!(r.calls[1].1, format!("Jira project: Alpha\nWork items:\n{NUMBERED}\nAnalyst's notes:\n{notes}"));
        assert_eq!(r.calls[2].1, format!("Analyst's notes:\n{notes}\nTickets:\n{}", pretty(&written_tickets())));
        assert_eq!(r.calls[3].1, format!("Work items:\n{NUMBERED}\nAnalyst's notes:\n{notes}\nTickets to review:\n{}", pretty(&tested_tickets())));
        for (i, (system, user)) in r.calls.iter().enumerate().skip(1) {
            for diff_text in ["export_csv", "let tz", "```", "no code change available"] {
                assert!(!user.contains(diff_text) && !system.contains(diff_text), "call {i} holds {diff_text:?}");
            }
        }
    }

    #[test]
    fn systems_name_the_language_where_they_write_and_all_ask_for_json() {
        let r = run_with("XYZ", "Esperanto", &items(), vec![ok(analysis()), ok(writer()), ok(tester()), fail("down")]);
        assert_eq!(r.calls.len(), 4);
        let systems: Vec<&str> = r.calls.iter().map(|(s, _)| s.as_str()).collect();
        for (i, s) in systems.iter().enumerate() {
            assert!(s.contains("JSON only"), "call {i}: {s}");
        }
        assert!(!systems[0].contains("Esperanto"), "the analyst writes notes in English: {}", systems[0]);
        for (i, s) in systems.iter().enumerate().skip(1) {
            assert!(s.contains("Esperanto"), "call {i}: {s}");
        }
        assert_eq!(r.calls[1].0, drafting::writer_prompt("XYZ", "Esperanto", &items(), &analysis()).0);
        assert_eq!(r.calls[2].0, drafting::tester_prompt("Esperanto", &analysis(), &written_tickets()).0);
        assert_eq!(r.calls[3].0, drafting::reviewer_prompt("Esperanto", &items(), &analysis(), &tested_tickets()).0);
        assert!(r.calls[1].1.starts_with("Jira project: XYZ\n"), "{}", r.calls[1].1);
    }

    #[test]
    fn numbering_continues_past_nine() {
        let many: Vec<WorkItem> = (b'a'..=b'k').map(|c| WorkItem { line: format!("item {}", c as char), change: None }).collect();
        let (_, user) = drafting::writer_prompt("ABC", "Polish", &many, &json!({}));
        assert!(user.contains("\n9. item i\n10. item j\n11. item k\n\nAnalyst's notes:"), "{user}");
    }
}

mod pipeline {
    use super::*;

    #[test]
    fn happy_path_four_calls_progress_before_each_and_reviewed_drafts() {
        let reviewed = json!({"tickets": [
            {"title": "Pobieranie wierszy tabeli jako CSV", "summary": "Audytor potrzebuje wierszy w CSV.", "criteria": ["Przycisk 'CSV' pobiera plik raport.csv"], "tests": ["2 wiersze -> 3 linie"]},
            {"title": "Daty raportu bez przesunięcia strefy"}
        ]});
        let r = run(vec![ok(analysis()), ok(writer()), ok(tester()), ok(reviewed)]);
        assert_eq!(r.progress, vec![(Stage::Analyst, 0), (Stage::Writer, 1), (Stage::Tester, 2), (Stage::Reviewer, 3)]);
        assert_eq!(r.calls.len(), 4);
        assert_eq!(
            r.result.unwrap(),
            vec![
                draft(
                    "Pobieranie wierszy tabeli jako CSV",
                    "h3. Opis\nAudytor potrzebuje wierszy w CSV.\n\nh3. Kryteria akceptacji\n* Przycisk 'CSV' pobiera plik raport.csv\n\nh3. Przypadki testowe\n# 2 wiersze -> 3 linie",
                    &[0, 2],
                ),
                draft("Daty raportu bez przesunięcia strefy", DESC_2_WITH_TESTS, &[1]),
            ]
        );
    }

    #[test]
    fn answers_wrapped_in_thinking_fences_and_bare_arrays_are_read() {
        let r = run(vec![
            Ok(format!("<think>{{maybe}}</think>```json\n{}\n```", json!({"tasks": [{"items": [2]}]}))),
            Ok(format!("Here you go:\n{}\nDone.", json!({"tickets": [{"title": "Bare array ticket", "summary": "S.", "criteria": ["C."]}]}))),
            ok(json!([{"cases": ["Case."]}])),
            ok(json!([{"title": "Reviewed bare"}])),
        ]);
        assert_eq!(r.result.unwrap(), vec![draft("Reviewed bare", "h3. Opis\nS.\n\nh3. Kryteria akceptacji\n* C.\n\nh3. Przypadki testowe\n# Case.", &[1])]);
    }

    #[test]
    fn english_language_gives_english_headings() {
        let r = run_with("ABC", "English", &items(), vec![
            ok(json!({"tasks": [{"items": [1]}]})),
            ok(json!({"tickets": [{"title": "Export rows", "summary": "Rows.", "criteria": ["CSV file"]}]})),
            ok(json!({"tests": [{"ticket": 1, "cases": ["2 rows -> 3 lines"]}]})),
            fail("reviewer down"),
        ]);
        assert_eq!(
            r.result.unwrap(),
            vec![draft("Export rows", "h3. Description\nRows.\n\nh3. Acceptance criteria\n* CSV file\n\nh3. Test cases\n# 2 rows -> 3 lines", &[0])]
        );
    }
}

mod analyst_failure {
    use super::*;

    #[test]
    fn chat_error_is_returned_and_nothing_else_is_called() {
        let r = run(vec![fail("LLM 503: overloaded"), ok(writer())]);
        assert_eq!(chain(&r.result.unwrap_err()), vec!["LLM 503: overloaded".to_string()]);
        assert_eq!(r.calls.len(), 1);
        assert_eq!(r.progress, vec![(Stage::Analyst, 0)]);
    }

    #[test]
    fn answer_without_json_is_an_analyst_error() {
        let r = run(vec![Ok("I cannot tell.".into()), ok(writer())]);
        assert_eq!(chain(&r.result.unwrap_err()), vec!["analyst".to_string(), "LLM answer holds no JSON: I cannot tell.".to_string()]);
        assert_eq!(r.calls.len(), 1);
        assert_eq!(r.progress, vec![(Stage::Analyst, 0)]);
    }

    #[test]
    fn no_task_is_an_error() {
        // (name, analyst answer)
        let rows = [
            ("empty task list", json!({"tasks": []})),
            ("no tasks key", json!({"notes": "nothing"})),
            ("tasks not a list", json!({"tasks": {"items": [1]}})),
            ("empty bare array", json!([])),
            ("scalar", json!(42)),
        ];
        for (name, answer) in rows {
            let r = run(vec![ok(answer), ok(writer())]);
            assert_eq!(chain(&r.result.unwrap_err()), vec!["analyst found no task in the ticked work".to_string()], "{name}");
            assert_eq!(r.calls.len(), 1, "{name}");
        }
    }
}

mod writer_failure {
    use super::*;

    #[test]
    fn chat_error_or_no_json_is_returned_and_the_tester_is_not_called() {
        // (name, writer answer, expected error chain)
        let rows: Vec<(&str, Result<String>, Vec<String>)> = vec![
            ("chat error", fail("LLM 500: boom"), strings(&["LLM 500: boom"])),
            ("no json", Ok("No tickets today.".into()), strings(&["ticket writer", "LLM answer holds no JSON: No tickets today."])),
        ];
        for (name, answer, expected) in rows {
            let r = run(vec![ok(analysis()), answer, ok(tester())]);
            assert_eq!(chain(&r.result.unwrap_err()), expected, "{name}");
            assert_eq!(r.calls.len(), 2, "{name}");
            assert_eq!(r.progress, vec![(Stage::Analyst, 0), (Stage::Writer, 1)], "{name}");
        }
    }

    #[test]
    fn no_titled_ticket_is_an_error() {
        // (name, writer answer)
        let rows = [
            ("empty list", json!({"tickets": []})),
            ("only blank titles", json!({"tickets": [{"title": " \n "}, {"title": ""}]})),
            ("no title keys", json!({"tickets": [{"summary": "s", "criteria": ["c"]}]})),
            ("non-text title", json!({"tickets": [{"title": 7}]})),
            ("no tickets key", json!({"title": "Lone object"})),
        ];
        for (name, answer) in rows {
            let r = run(vec![ok(analysis()), ok(answer), ok(tester())]);
            assert_eq!(chain(&r.result.unwrap_err()), vec!["ticket writer returned no ticket with a title".to_string()], "{name}");
            assert_eq!(r.calls.len(), 2, "{name}");
        }
    }
}

mod items_and_titles {
    use super::*;

    /// Three tasks: T1 items [0, 2], T2 [1], T3 only item 4 valid -> [3].
    fn three_tasks() -> Value {
        json!({"tasks": [
            {"items": [3, 1, 1]},
            {"items": [2]},
            {"items": [0, -1, 1.5, "2", null, 4, 4]}
        ]})
    }

    /// Runs with tester and reviewer failing, so the drafts are the writer's.
    fn drafts_for(analysis: Value, writer: Value) -> Vec<TicketDraft> {
        let r = run(vec![ok(analysis), ok(writer), fail("tester down"), fail("reviewer down")]);
        assert_eq!(r.calls.len(), 4);
        r.result.unwrap()
    }

    #[test]
    fn items_come_from_the_named_task_else_the_task_at_the_same_position() {
        // (name, writer tickets, expected items per draft)
        type Row = (&'static str, Value, Vec<Vec<usize>>);
        let rows: Vec<Row> = vec![
            ("task names the third task", json!([{"task": 3, "title": "X"}]), vec![vec![3]]),
            ("no task: first position", json!([{"title": "X"}]), vec![vec![0, 2]]),
            ("task 0 out of range", json!([{"task": 0, "title": "X"}]), vec![vec![0, 2]]),
            ("task past the last", json!([{"task": 4, "title": "X"}]), vec![vec![0, 2]]),
            ("negative task", json!([{"task": -2, "title": "X"}]), vec![vec![0, 2]]),
            ("task as text", json!([{"task": "3", "title": "X"}]), vec![vec![0, 2]]),
            ("task as float", json!([{"task": 2.0, "title": "X"}]), vec![vec![0, 2]]),
            ("each its own, mixed", json!([{"task": 2, "title": "A"}, {"title": "B"}, {"task": 1, "title": "C"}]), vec![vec![1], vec![1], vec![0, 2]]),
            ("an untitled ticket keeps its position", json!([{"title": ""}, {"title": "B"}, {"title": "C"}]), vec![vec![1], vec![3]]),
        ];
        for (name, writer, expected) in rows {
            let got: Vec<Vec<usize>> = drafts_for(three_tasks(), writer).into_iter().map(|d| d.items).collect();
            assert_eq!(got, expected, "{name}");
        }
    }

    #[test]
    fn task_without_a_usable_item_list_or_ticket_past_the_tasks_gives_no_items() {
        let analysis = json!({"tasks": [{"area": "no items"}, {"items": "1, 2"}]});
        let writer = json!([{"title": "A"}, {"title": "B"}, {"title": "C"}]);
        let got: Vec<Vec<usize>> = drafts_for(analysis, writer).into_iter().map(|d| d.items).collect();
        assert_eq!(got, vec![Vec::<usize>::new(), vec![], vec![]]);
    }

    #[test]
    fn at_most_three_titled_drafts_in_order() {
        // (name, titles, expected titles)
        let rows: [(&str, &[&str], &[&str]); 4] = [
            ("exactly three", &["T1", "T2", "T3"], &["T1", "T2", "T3"]),
            ("five cut to three", &["T1", "T2", "T3", "T4", "T5"], &["T1", "T2", "T3"]),
            ("blank one does not use a slot", &["T1", " ", "T3", "T4", "T5"], &["T1", "T3", "T4"]),
            ("one", &["T1"], &["T1"]),
        ];
        for (name, titles, expected) in rows {
            let writer: Vec<Value> = titles.iter().map(|t| json!({"title": t})).collect();
            let got: Vec<String> = drafts_for(three_tasks(), json!({ "tickets": writer })).into_iter().map(|d| d.title).collect();
            assert_eq!(got, strings(expected), "{name}");
        }
    }

    #[test]
    fn untitled_writer_tickets_are_dropped() {
        let writer = json!({"tickets": [
            {"title": "", "summary": "empty"},
            {"summary": "missing"},
            {"title": null, "summary": "null"},
            {"title": "Kept", "summary": "kept summary"},
            {"title": "  \n\t ", "summary": "blank"},
            {"title": 42, "summary": "number"}
        ]});
        // Kept is the fourth ticket, so its items are those of the task at position 4: none.
        assert_eq!(drafts_for(three_tasks(), writer), vec![draft("Kept", "h3. Opis\nkept summary", &[])]);
    }

    #[test]
    fn title_is_the_first_line_trimmed_and_cut_to_255_chars() {
        let s = |n: usize| "żółć".chars().cycle().take(n).collect::<String>();
        // (name, title, expected)
        let rows: Vec<(&str, String, String)> = vec![
            ("surrounding spaces", "  Fix login  ".into(), "Fix login".into()),
            ("first line only", "Fix login\nsecond line".into(), "Fix login".into()),
            ("leading blank lines", "\n\n  Fix login\nmore".into(), "Fix login".into()),
            ("crlf", "Fix login \r\nmore".into(), "Fix login".into()),
            ("254 kept", s(254), s(254)),
            ("255 kept", s(255), s(255)),
            ("256 cut to 255", s(256), s(255)),
            ("600 cut to 255", s(600), s(255)),
        ];
        for (name, title, expected) in rows {
            let got = drafts_for(three_tasks(), json!([{ "title": title }]));
            assert_eq!(got.iter().map(|d| d.title.clone()).collect::<Vec<_>>(), vec![expected], "{name}");
        }
    }
}

mod tester_step {
    use super::*;

    fn with_tester(answer: Result<String>) -> Run {
        run(vec![ok(analysis()), ok(writer()), answer, fail("reviewer down")])
    }

    fn descriptions(r: Run) -> Vec<String> {
        assert_eq!(r.calls.len(), 4);
        r.result.unwrap().into_iter().map(|d| d.description).collect()
    }

    #[test]
    fn cases_matched_by_ticket_number_else_by_position() {
        let desc1 = |tests: &str| format!("{DESC_1_NO_TESTS}\n\nh3. Przypadki testowe\n{tests}");
        let desc2 = |tests: &str| format!("{DESC_2_NO_TESTS}\n\nh3. Przypadki testowe\n{tests}");
        // (name, tester answer, expected descriptions)
        let rows: Vec<(&str, Value, Vec<String>)> = vec![
            ("by number, listed in reverse", json!({"tests": [{"ticket": 2, "cases": ["B1"]}, {"ticket": 1, "cases": ["A1", "A2"]}]}), vec![desc1("# A1\n# A2"), desc2("# B1")]),
            ("by position", json!({"tests": [{"cases": ["P1"]}, {"cases": ["P2"]}]}), vec![desc1("# P1"), desc2("# P2")]),
            ("bare array", json!([{"cases": ["Q1"]}, {"cases": ["Q2"]}]), vec![desc1("# Q1"), desc2("# Q2")]),
            ("fewer entries than tickets", json!({"tests": [{"ticket": 1, "cases": ["A1"]}]}), vec![desc1("# A1"), DESC_2_NO_TESTS.into()]),
            ("empty and non-text cases dropped", json!({"tests": [{"cases": ["", 3, " A3 "]}, {"cases": []}]}), vec![desc1("# A3"), DESC_2_NO_TESTS.into()]),
            ("cases not a list", json!({"tests": [{"cases": "A1"}, {"cases": null}]}), vec![DESC_1_NO_TESTS.into(), DESC_2_NO_TESTS.into()]),
            ("tests key not a list", json!({"tests": "none"}), vec![DESC_1_NO_TESTS.into(), DESC_2_NO_TESTS.into()]),
            ("once one is numbered, an unnumbered entry goes nowhere", json!({"tests": [{"cases": ["U1"]}, {"ticket": 2, "cases": ["B1"]}]}), vec![DESC_1_NO_TESTS.into(), desc2("# B1")]),
            ("numbered first, unnumbered second", json!({"tests": [{"ticket": 1, "cases": ["A1"]}, {"cases": ["U2"]}]}), vec![desc1("# A1"), DESC_2_NO_TESTS.into()]),
            ("number past the tickets matches none", json!({"tests": [{"ticket": 3, "cases": ["X3"]}, {"cases": ["U2"]}]}), vec![DESC_1_NO_TESTS.into(), DESC_2_NO_TESTS.into()]),
            ("text numbers are no numbers: by position", json!({"tests": [{"ticket": "2", "cases": ["S1"]}, {"ticket": "1", "cases": ["S2"]}]}), vec![desc1("# S1"), desc2("# S2")]),
        ];
        for (name, answer, expected) in rows {
            assert_eq!(descriptions(with_tester(ok(answer))), expected, "{name}");
        }
    }

    #[test]
    fn failure_leaves_drafts_without_tests_and_the_reviewer_still_runs() {
        // (name, tester answer)
        let rows: Vec<(&str, Result<String>)> = vec![("chat error", fail("LLM 502: gateway")), ("no json", Ok("Nothing to test.".into()))];
        for (name, answer) in rows {
            let r = with_tester(answer);
            assert_eq!(r.progress, vec![(Stage::Analyst, 0), (Stage::Writer, 1), (Stage::Tester, 2), (Stage::Reviewer, 3)], "{name}");
            assert!(r.calls[3].1.ends_with(&format!("Tickets to review:\n{}", pretty(&json!({"tickets": [
                {"title": "Eksport wierszy do CSV", "summary": "Audytorzy nie mogą pobrać wierszy.", "criteria": ["Przycisk pobiera plik CSV", "Plik ma nagłówek"], "tests": []},
                {"title": "Daty w raporcie w czasie lokalnym", "summary": "Daty przesuwają się o godzinę.", "criteria": ["Data 2026-03-29 02:30 pokazuje się bez przesunięcia"], "tests": []}
            ]})))), "{name}: {}", r.calls[3].1);
            assert_eq!(
                r.result.unwrap(),
                vec![draft("Eksport wierszy do CSV", DESC_1_NO_TESTS, &[0, 2]), draft("Daty w raporcie w czasie lokalnym", DESC_2_NO_TESTS, &[1])],
                "{name}"
            );
        }
    }
}

mod reviewer_step {
    use super::*;

    fn with_reviewer(answer: Result<String>) -> Vec<TicketDraft> {
        let r = run(vec![ok(analysis()), ok(writer()), ok(tester()), answer]);
        assert_eq!(r.calls.len(), 4);
        r.result.unwrap()
    }

    fn unreviewed() -> Vec<TicketDraft> {
        vec![draft("Eksport wierszy do CSV", DESC_1_WITH_TESTS, &[0, 2]), draft("Daty w raporcie w czasie lokalnym", DESC_2_WITH_TESTS, &[1])]
    }

    /// The second ticket left to the writer and tester: an untitled review entry.
    fn second_kept() -> Value {
        json!({"title": "", "summary": "ignored", "criteria": ["ignored"], "tests": ["ignored"]})
    }

    #[test]
    fn titled_entry_replaces_title_and_nonempty_parts_tests_whenever_a_list() {
        let pl = |s: &str, c: &[&str], t: &[&str]| drafting::render_description("Polish", s, &strings(c), &strings(t));
        let w_crit = ["Przycisk pobiera plik CSV", "Plik ma nagłówek"];
        let t_cases = ["Tabela z 2 wierszami -> plik z 3 liniami", "Pusta tabela -> sam nagłówek"];
        let w_sum = "Audytorzy nie mogą pobrać wierszy.";
        // (name, review of the first ticket, expected title, expected description)
        let rows: Vec<(&str, Value, &str, String)> = vec![
            ("everything replaced", json!({"title": "R title", "summary": "R sum.", "criteria": ["R c"], "tests": ["R t"]}), "R title", pl("R sum.", &["R c"], &["R t"])),
            ("title only keeps the rest", json!({"title": "R title"}), "R title", pl(w_sum, &w_crit, &t_cases)),
            ("blank summary and empty criteria kept, empty tests remove the section", json!({"title": "R title", "summary": "  ", "criteria": [], "tests": []}), "R title", pl(w_sum, &w_crit, &[])),
            ("tests of blank entries remove the section", json!({"title": "R title", "tests": ["  ", 4]}), "R title", pl(w_sum, &w_crit, &[])),
            ("tests not a list keep the tester's", json!({"title": "R title", "tests": "none"}), "R title", pl(w_sum, &w_crit, &t_cases)),
            ("criteria of blank entries keep the writer's", json!({"title": "R title", "criteria": [" ", ""]}), "R title", pl(w_sum, &w_crit, &t_cases)),
            ("title first line, trimmed", json!({"title": "  R title\nmore", "summary": "R sum."}), "R title", pl("R sum.", &w_crit, &t_cases)),
            ("untitled entry keeps everything", json!({"title": " ", "summary": "R sum.", "criteria": ["R c"], "tests": []}), "Eksport wierszy do CSV", DESC_1_WITH_TESTS.into()),
        ];
        for (name, review, title, description) in rows {
            let got = with_reviewer(ok(json!({"tickets": [review, second_kept()]})));
            let mut expected = unreviewed();
            expected[0] = draft(title, &description, &[0, 2]);
            assert_eq!(got, expected, "{name}");
        }
    }

    #[test]
    fn entries_apply_by_position_second_ticket_reviewed_first_kept() {
        let got = with_reviewer(ok(json!({"tickets": [second_kept(), {"title": "Daty bez przesunięcia", "tests": ["Nowy przypadek"]}]})));
        let mut expected = unreviewed();
        expected[1] = draft(
            "Daty bez przesunięcia",
            "h3. Opis\nDaty przesuwają się o godzinę.\n\nh3. Kryteria akceptacji\n* Data 2026-03-29 02:30 pokazuje się bez przesunięcia\n\nh3. Przypadki testowe\n# Nowy przypadek",
            &[1],
        );
        assert_eq!(got, expected);
    }

    #[test]
    fn wrong_count_or_failure_keeps_the_unreviewed_drafts() {
        let one = json!({"title": "R one"});
        // (name, reviewer answer)
        let rows: Vec<(&str, Result<String>)> = vec![
            ("fewer tickets", ok(json!({"tickets": [one.clone()]}))),
            ("more tickets", ok(json!({"tickets": [one.clone(), one.clone(), one.clone()]}))),
            ("no tickets", ok(json!({"tickets": []}))),
            ("no tickets key", ok(json!({"verdict": "fine"}))),
            ("chat error", fail("LLM 429: slow down")),
            ("no json", Ok("Looks good to me.".into())),
        ];
        for (name, answer) in rows {
            assert_eq!(with_reviewer(answer), unreviewed(), "{name}");
        }
    }
}

fn current() -> CurrentTicket {
    CurrentTicket { key: "ABC-42".into(), title: "  Eksport do CSV ".into(), description: "\nh3. Opis\nStary opis.\n\n".into() }
}

/// The block both prompts end with for `current()`: title and description trimmed.
const CURRENT_BLOCK: &str = "Existing ticket ABC-42:\nTitle: Eksport do CSV\nDescription:\nh3. Opis\nStary opis.\n";

mod prompts_for_a_current_ticket {
    use super::*;

    #[test]
    fn without_a_current_ticket_they_are_the_plain_prompts() {
        assert_eq!(drafting::analyst_prompt_for("Alpha", &items(), None), drafting::analyst_prompt("Alpha", &items()));
        assert_eq!(
            drafting::writer_prompt_for("Alpha", "Polish", &items(), &analysis(), None),
            drafting::writer_prompt("Alpha", "Polish", &items(), &analysis())
        );
        assert!(!drafting::analyst_prompt("Alpha", &items()).1.contains("Existing ticket"));
    }

    /// The added sentence's wording is the prompt author's; its contract is that it follows the
    /// plain system prompt, names the key and asks for exactly one task / ticket.
    #[test]
    fn analyst_gets_one_task_sentence_and_the_ticket_after_the_items() {
        let (plain_system, plain_user) = drafting::analyst_prompt("Alpha", &items());
        let (system, user) = drafting::analyst_prompt_for("Alpha", &items(), Some(&current()));
        let added = system.strip_prefix(&plain_system).unwrap_or_else(|| panic!("plain system prompt first: {system}"));
        assert!(added.contains("existing ticket ABC-42") && added.contains("exactly one task"), "{added}");
        assert_eq!(user, format!("{plain_user}\n{CURRENT_BLOCK}"));
    }

    #[test]
    fn writer_gets_one_ticket_sentence_and_the_ticket_after_the_notes() {
        let (plain_system, plain_user) = drafting::writer_prompt("Alpha", "Polish", &items(), &analysis());
        let (system, user) = drafting::writer_prompt_for("Alpha", "Polish", &items(), &analysis(), Some(&current()));
        let added = system.strip_prefix(&plain_system).unwrap_or_else(|| panic!("plain system prompt first: {system}"));
        assert!(added.contains("existing ticket ABC-42") && added.contains("exactly one ticket"), "{added}");
        assert_eq!(user, format!("{plain_user}\n{CURRENT_BLOCK}"));
    }

    #[test]
    fn blank_description_reads_empty() {
        for description in ["", "  \n\t"] {
            let c = CurrentTicket { key: "ABC-5".into(), title: "T5".into(), description: description.into() };
            let block = "Existing ticket ABC-5:\nTitle: T5\nDescription:\n(empty)\n";
            let (_, a) = drafting::analyst_prompt_for("ABC", &items(), Some(&c));
            let (_, w) = drafting::writer_prompt_for("ABC", "Polish", &items(), &json!({}), Some(&c));
            assert!(a.ends_with(&format!("\n{block}")), "{description:?}: {a}");
            assert!(w.ends_with(&format!("\n{block}")), "{description:?}: {w}");
        }
    }

    #[test]
    fn no_items_read_none_for_the_analyst() {
        assert_eq!(drafting::analyst_prompt("ABC", &[]).1, "Jira project: ABC\nWork items:\n(none)\n");
        assert_eq!(drafting::analyst_prompt_for("ABC", &[], Some(&current())).1, format!("Jira project: ABC\nWork items:\n(none)\n\n{CURRENT_BLOCK}"));
    }
}

mod redraft {
    use super::*;

    struct Redraft {
        result: Result<TicketDraft>,
        calls: Vec<(String, String)>,
        progress: Vec<(Stage, usize)>,
    }

    fn redraft_with(items: &[WorkItem], answers: Vec<Result<String>>) -> Redraft {
        let chat = Scripted { answers: RefCell::new(answers.into()), calls: RefCell::new(Vec::new()) };
        let progress = RefCell::new(Vec::new());
        let report = |s: Stage| progress.borrow_mut().push((s, chat.calls.borrow().len()));
        let result = drafting::redraft_ticket(&chat, "Alpha", "Polish", &current(), items, &report);
        Redraft { result, calls: chat.calls.into_inner(), progress: progress.into_inner() }
    }

    /// Two tasks although one was asked for, so keeping one is the pipeline's doing.
    fn two_tasks() -> Value {
        json!({"tasks": [{"items": [3, 2]}, {"items": [1]}]})
    }

    /// An untitled ticket, then two titled ones; the first titled names task 2.
    fn three_written() -> Value {
        json!({"tickets": [
            {"task": 1, "title": " ", "summary": "untitled"},
            {"task": 2, "title": "Eksport wierszy do CSV", "summary": "Audytorzy nie mogą pobrać wierszy.", "criteria": ["Przycisk pobiera plik CSV"]},
            {"task": 1, "title": "Drugi tytuł", "summary": "S2.", "criteria": ["C2"]}
        ]})
    }

    fn one_ticket() -> Value {
        json!({"tickets": [{"title": "Eksport wierszy do CSV", "summary": "Audytorzy nie mogą pobrać wierszy.", "criteria": ["Przycisk pobiera plik CSV"]}]})
    }

    #[test]
    fn four_calls_with_the_ticket_in_the_prompts_and_one_reviewed_draft() {
        let tester = json!({"tests": [{"ticket": 1, "cases": ["Tabela z 2 wierszami -> 3 linie"]}, {"ticket": 2, "cases": ["never used"]}]});
        let reviewed = json!({"tickets": [{"title": "Pobieranie wierszy jako CSV"}]});
        let r = redraft_with(&items(), vec![ok(two_tasks()), ok(three_written()), ok(tester), ok(reviewed)]);

        assert_eq!(r.progress, vec![(Stage::Analyst, 0), (Stage::Writer, 1), (Stage::Tester, 2), (Stage::Reviewer, 3)]);
        assert_eq!(r.calls.len(), 4);
        assert_eq!(r.calls[0], drafting::analyst_prompt_for("Alpha", &items(), Some(&current())));
        assert_eq!(r.calls[1], drafting::writer_prompt_for("Alpha", "Polish", &items(), &two_tasks(), Some(&current())));
        assert_eq!(r.calls[2], drafting::tester_prompt("Polish", &two_tasks(), &one_ticket()));
        let tested = json!({"tickets": [{"title": "Eksport wierszy do CSV", "summary": "Audytorzy nie mogą pobrać wierszy.", "criteria": ["Przycisk pobiera plik CSV"], "tests": ["Tabela z 2 wierszami -> 3 linie"]}]});
        assert_eq!(r.calls[3], drafting::reviewer_prompt("Polish", &items(), &two_tasks(), &tested));
        for i in [0, 1] {
            assert!(r.calls[i].1.ends_with(&format!("\n{CURRENT_BLOCK}")), "call {i}: {}", r.calls[i].1);
        }

        assert_eq!(
            r.result.unwrap(),
            TicketDraft {
                title: "Pobieranie wierszy jako CSV".into(),
                description: "h3. Opis\nAudytorzy nie mogą pobrać wierszy.\n\nh3. Kryteria akceptacji\n* Przycisk pobiera plik CSV\n\nh3. Przypadki testowe\n# Tabela z 2 wierszami -> 3 linie".into(),
                items: vec![0],
            }
        );
    }

    #[test]
    fn reviewer_answering_for_more_tickets_keeps_the_unreviewed_draft() {
        let r = redraft_with(&items(), vec![
            ok(two_tasks()),
            ok(three_written()),
            fail("tester down"),
            ok(json!({"tickets": [{"title": "R1"}, {"title": "R2"}]})),
        ]);
        assert_eq!(
            r.result.unwrap(),
            TicketDraft {
                title: "Eksport wierszy do CSV".into(),
                description: "h3. Opis\nAudytorzy nie mogą pobrać wierszy.\n\nh3. Kryteria akceptacji\n* Przycisk pobiera plik CSV".into(),
                items: vec![0],
            }
        );
    }

    #[test]
    fn no_items_work_from_the_ticket_alone() {
        let r = redraft_with(&[], vec![
            ok(json!({"tasks": [{"items": [], "area": "CSV"}]})),
            ok(json!({"tickets": [{"title": "Eksport CSV", "summary": "Opis.", "criteria": ["Plik CSV"]}]})),
            ok(json!({"tests": [{"ticket": 1, "cases": []}]})),
            ok(json!({"tickets": [{"title": "Eksport CSV dla audytu", "tests": []}]})),
        ]);
        assert_eq!(r.calls.len(), 4);
        assert_eq!(r.calls[0].1, format!("Jira project: Alpha\nWork items:\n(none)\n\n{CURRENT_BLOCK}"));
        assert_eq!(
            r.result.unwrap(),
            TicketDraft { title: "Eksport CSV dla audytu".into(), description: "h3. Opis\nOpis.\n\nh3. Kryteria akceptacji\n* Plik CSV".into(), items: vec![] }
        );
    }

    #[test]
    fn analyst_or_writer_failure_is_the_error() {
        // (name, answers, expected chain, calls made)
        type Row = (&'static str, Vec<Result<String>>, Vec<String>, usize);
        let rows: Vec<Row> = vec![
            ("analyst chat error", vec![fail("LLM 503: overloaded")], strings(&["LLM 503: overloaded"]), 1),
            ("analyst without tasks", vec![ok(json!({"tasks": []}))], strings(&["analyst found no task in the ticked work"]), 1),
            ("writer without a titled ticket", vec![ok(two_tasks()), ok(json!({"tickets": [{"title": ""}]}))], strings(&["ticket writer returned no ticket with a title"]), 2),
            ("writer without JSON", vec![ok(two_tasks()), Ok("No.".into())], strings(&["ticket writer", "LLM answer holds no JSON: No."]), 2),
        ];
        for (name, answers, expected, calls) in rows {
            let r = redraft_with(&items(), answers);
            assert_eq!(chain(&r.result.unwrap_err()), expected, "{name}");
            assert_eq!(r.calls.len(), calls, "{name}");
        }
    }
}

/// The sentence `steer` appends to every system prompt, copied from the contract.
const STEER_SYSTEM: &str = " The ticket's author added instructions for this rewrite, given at the end: follow them, also over the style rules above, but never invent facts for them.";

mod steer {
    use super::*;

    #[test]
    fn blank_instructions_leave_both_prompts_unchanged() {
        for blank in ["", " ", "  \n\t \r\n"] {
            assert_eq!(drafting::steer("SYS-a".into(), "USER-b\n".into(), blank), ("SYS-a".to_owned(), "USER-b\n".to_owned()), "{blank:?}");
        }
    }

    #[test]
    fn instructions_rank_in_the_system_prompt_and_end_the_user_message_trimmed() {
        // (name, instructions, expected text after "Author's instructions:\n")
        let rows: [(&str, &str, &str); 4] = [
            ("plain", "shorter", "shorter"),
            ("surrounding whitespace trimmed", " \t add a test case for an expired token \n\n", "add a test case for an expired token"),
            ("multi-line keeps inner lines and their indent", "\n- shorter\n  - mention the admin panel\n\n", "- shorter\n  - mention the admin panel"),
            ("format characters and diacritics kept as typed", "dodaj „wygasły token” {0} %s \\n", "dodaj „wygasły token” {0} %s \\n"),
        ];
        for (name, instructions, kept) in rows {
            let (system, user) = drafting::steer("System prompt X.".into(), "Jira project: XYZ\nWork items:\n(none)\n".into(), instructions);
            assert_eq!(system, format!("System prompt X.{STEER_SYSTEM}"), "{name}");
            assert_eq!(user, format!("Jira project: XYZ\nWork items:\n(none)\n\nAuthor's instructions:\n{kept}\n"), "{name}");
        }
    }
}

mod redraft_with_instructions {
    use super::*;

    struct Redraft {
        result: Result<TicketDraft>,
        calls: Vec<(String, String)>,
        progress: Vec<(Stage, usize)>,
    }

    fn redraft(instructions: Option<&str>) -> Redraft {
        let answers = vec![
            ok(json!({"tasks": [{"items": [3, 2]}, {"items": [1]}]})),
            ok(json!({"tickets": [
                {"task": 1, "title": "Eksport wierszy do CSV", "summary": "Audytorzy nie mogą pobrać wierszy.", "criteria": ["Przycisk pobiera plik CSV"]},
                {"task": 2, "title": "Drugi tytuł", "summary": "S2.", "criteria": ["C2"]}
            ]})),
            ok(json!({"tests": [{"ticket": 1, "cases": ["Tabela z 2 wierszami -> 3 linie"]}]})),
            ok(json!({"tickets": [{"title": "Pobieranie wierszy jako CSV"}]})),
        ];
        let chat = Scripted { answers: RefCell::new(answers.into()), calls: RefCell::new(Vec::new()) };
        let progress = RefCell::new(Vec::new());
        let report = |s: Stage| progress.borrow_mut().push((s, chat.calls.borrow().len()));
        let result = match instructions {
            Some(i) => drafting::redraft_ticket_with(&chat, "Alpha", "Polish", &current(), &items(), i, &report),
            None => drafting::redraft_ticket(&chat, "Alpha", "Polish", &current(), &items(), &report),
        };
        Redraft { result, calls: chat.calls.into_inner(), progress: progress.into_inner() }
    }

    /// The four prompts of `redraft` without instructions, built from the public prompt builders.
    fn plain_calls() -> Vec<(String, String)> {
        let analysis = json!({"tasks": [{"items": [3, 2]}, {"items": [1]}]});
        let one = json!({"tickets": [{"title": "Eksport wierszy do CSV", "summary": "Audytorzy nie mogą pobrać wierszy.", "criteria": ["Przycisk pobiera plik CSV"]}]});
        let tested = json!({"tickets": [{"title": "Eksport wierszy do CSV", "summary": "Audytorzy nie mogą pobrać wierszy.", "criteria": ["Przycisk pobiera plik CSV"], "tests": ["Tabela z 2 wierszami -> 3 linie"]}]});
        vec![
            drafting::analyst_prompt_for("Alpha", &items(), Some(&current())),
            drafting::writer_prompt_for("Alpha", "Polish", &items(), &analysis, Some(&current())),
            drafting::tester_prompt("Polish", &analysis, &one),
            drafting::reviewer_prompt("Polish", &items(), &analysis, &tested),
        ]
    }

    fn reviewed_draft() -> TicketDraft {
        draft(
            "Pobieranie wierszy jako CSV",
            "h3. Opis\nAudytorzy nie mogą pobrać wierszy.\n\nh3. Kryteria akceptacji\n* Przycisk pobiera plik CSV\n\nh3. Przypadki testowe\n# Tabela z 2 wierszami -> 3 linie",
            // Task 1 names items 3 and 2: zero-based, in item order.
            &[1, 2],
        )
    }

    #[test]
    fn every_one_of_the_four_calls_carries_the_trimmed_instructions() {
        let r = redraft(Some("  add a test case for an expired token\nshorter description \n"));
        let expected: Vec<(String, String)> = plain_calls()
            .into_iter()
            .map(|(s, u)| {
                (format!("{s}{STEER_SYSTEM}"), format!("{u}\nAuthor's instructions:\nadd a test case for an expired token\nshorter description\n"))
            })
            .collect();
        assert_eq!(r.calls.len(), 4);
        for (i, (got, want)) in r.calls.iter().zip(&expected).enumerate() {
            assert_eq!(got, want, "call {i}");
        }
        assert_eq!(r.progress, vec![(Stage::Analyst, 0), (Stage::Writer, 1), (Stage::Tester, 2), (Stage::Reviewer, 3)]);
        assert_eq!(r.result.unwrap(), reviewed_draft());
    }

    #[test]
    fn blank_instructions_give_exactly_the_plain_redraft() {
        let plain = redraft(None);
        assert_eq!(plain.calls, plain_calls(), "fixture: redraft_ticket sends the plain prompts");
        for blank in ["", " \n\t "] {
            let r = redraft(Some(blank));
            assert_eq!(r.calls, plain_calls(), "{blank:?}");
            assert_eq!(r.progress, plain.progress, "{blank:?}");
            assert_eq!(r.result.unwrap(), reviewed_draft(), "{blank:?}");
        }
        assert_eq!(plain.result.unwrap(), reviewed_draft());
    }

    #[test]
    fn draft_tickets_never_carries_instructions() {
        let r = run(vec![ok(analysis()), ok(writer()), ok(tester()), ok(json!({"tickets": [{"title": ""}, {"title": ""}]}))]);
        assert_eq!(r.calls.len(), 4);
        for (i, (system, user)) in r.calls.iter().enumerate() {
            assert!(!user.contains("Author's instructions"), "call {i}: {user}");
            assert!(!system.contains("added instructions"), "call {i}: {system}");
        }
    }
}
