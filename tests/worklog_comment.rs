//! The worklog comment of a row: the three-call pipeline driven by a scripted chat, its prompts,
//! the cleaning of the model's text, and a row's `Description` fed through real channels. No
//! request leaves the process.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::sync::mpsc::{Sender, channel};

use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use worklog::drafting::{Chat, WorkItem};
use worklog::worklog_comment::{self as wc, Description, Job, MAX_CHARS, Msg, Step, TARGET_CHARS, Ticket};

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
    result: Result<String>,
    calls: Vec<(String, String)>,
    /// Each step reported, with the number of chat calls made before it.
    progress: Vec<(Step, usize)>,
}

fn run_with(language: &str, items: &[WorkItem], answers: Vec<Result<String>>) -> Run {
    let chat = Scripted { answers: RefCell::new(answers.into()), calls: RefCell::new(Vec::new()) };
    let progress = RefCell::new(Vec::new());
    let report = |s: Step| progress.borrow_mut().push((s, chat.calls.borrow().len()));
    let result = wc::describe(&chat, language, &ticket(), items, &report);
    Run { result, calls: chat.calls.into_inner(), progress: progress.into_inner() }
}

fn run(answers: Vec<Result<String>>) -> Run {
    run_with("Polish", &items(), answers)
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

fn ticket() -> Ticket {
    Ticket { key: "ABC-42".into(), summary: "Eksport raportu do CSV".into() }
}

const DIFF_1: &str = "diff --git a/src/export.rs b/src/export.rs\n+pub fn export_csv() {}\n\n";
const DIFF_3: &str = "-let sep = ';';\n+let sep = ',';";

fn items() -> Vec<WorkItem> {
    vec![
        WorkItem { line: "[api] ABC-42 add CSV export".into(), change: Some(DIFF_1.into()) },
        WorkItem { line: "[web] checkout feature/ABC-42".into(), change: None },
        WorkItem { line: "[svc] ABC-42 comma separator".into(), change: Some(DIFF_3.into()) },
    ]
}

const NUMBERED: &str = "1. [api] ABC-42 add CSV export\n2. [web] checkout feature/ABC-42\n3. [svc] ABC-42 comma separator\n";

fn analysis() -> Value {
    json!({"done": ["Raport można pobrać jako CSV", "Separator to przecinek"], "unclear": "czy nagłówki są tłumaczone"})
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap()
}

const WRITTEN: &str = "Dodano pobieranie raportu jako CSV z przecinkiem jako separatorem.";
const REVIEWED: &str = "Raport można pobrać jako plik CSV rozdzielany przecinkami.";

mod step {
    use super::*;

    #[test]
    fn all_in_pipeline_order_numbered_one_to_three() {
        assert_eq!(Step::ALL, [Step::Analyst, Step::Writer, Step::Reviewer]);
        let numbers: Vec<usize> = Step::ALL.iter().map(|s| s.number()).collect();
        assert_eq!(numbers, vec![1, 2, 3]);
    }

    #[test]
    fn labels_are_distinct_and_non_blank() {
        let labels: Vec<&str> = Step::ALL.iter().map(|s| s.label()).collect();
        for (i, l) in labels.iter().enumerate() {
            assert!(!l.trim().is_empty(), "step {i}");
        }
        assert_ne!(labels[0], labels[1]);
        assert_ne!(labels[1], labels[2]);
        assert_ne!(labels[0], labels[2]);
    }
}

mod prompts {
    use super::*;

    fn happy() -> Run {
        run(vec![ok(analysis()), ok(json!({"comment": WRITTEN})), ok(json!({"comment": REVIEWED}))])
    }

    #[test]
    fn analyst_gets_ticket_and_numbered_items_with_changes_unchanged_items_just_listed() {
        let r = happy();
        let expected = "Jira ticket ABC-42: Eksport raportu do CSV\nWork items:\n\
            1. [api] ABC-42 add CSV export\n```\ndiff --git a/src/export.rs b/src/export.rs\n+pub fn export_csv() {}\n```\n\
            2. [web] checkout feature/ABC-42\n\
            3. [svc] ABC-42 comma separator\n```\n-let sep = ';';\n+let sep = ',';\n```\n";
        assert_eq!(r.calls[0].1, expected);
        assert_eq!(r.calls[0], wc::analyst_prompt(&ticket(), &items()));
    }

    #[test]
    fn analyst_reads_none_when_there_are_no_items() {
        assert_eq!(wc::analyst_prompt(&ticket(), &[]).1, "Jira ticket ABC-42: Eksport raportu do CSV\nWork items:\n(none)\n");
    }

    #[test]
    fn writer_gets_ticket_and_analysis_and_never_the_items_or_diffs() {
        let r = happy();
        let expected = format!("Jira ticket ABC-42: Eksport raportu do CSV\nAnalyst's notes:\n{}", pretty(&analysis()));
        assert_eq!(r.calls[1].1, expected);
        assert_eq!(r.calls[1], wc::writer_prompt("Polish", &ticket(), &analysis()));
    }

    #[test]
    fn reviewer_gets_items_without_changes_the_analysis_and_the_writers_cleaned_comment() {
        let r = run(vec![ok(analysis()), ok(json!({"comment": format!("  \"{WRITTEN}\"  ")})), ok(json!({"comment": REVIEWED}))]);
        let expected = format!(
            "Jira ticket ABC-42: Eksport raportu do CSV\nWork items:\n{NUMBERED}\nAnalyst's notes:\n{}\nComment to review:\n{WRITTEN}\n",
            pretty(&analysis())
        );
        assert_eq!(r.calls[2].1, expected);
        assert_eq!(r.calls[2], wc::reviewer_prompt("Polish", &ticket(), &items(), &analysis(), WRITTEN));
    }

    #[test]
    fn writer_and_reviewer_systems_name_the_language_and_the_target_length_analyst_writes_english() {
        let r = run_with("Esperanto", &items(), vec![ok(analysis()), ok(json!({"comment": WRITTEN})), fail("down")]);
        assert_eq!(r.calls.len(), 3);
        let limit = format!("at most {TARGET_CHARS} characters");
        assert_eq!(TARGET_CHARS, 300);
        for (i, (system, _)) in r.calls.iter().enumerate().skip(1) {
            assert!(system.contains("in Esperanto"), "call {i}: {system}");
            assert!(system.contains(&limit), "call {i}: {system}");
            assert!(system.contains("JSON only"), "call {i}: {system}");
        }
        assert!(!r.calls[0].0.contains("Esperanto"), "{}", r.calls[0].0);
        assert!(r.calls[0].0.contains("Write in English"), "{}", r.calls[0].0);
        assert!(r.calls[0].0.contains("JSON only"), "{}", r.calls[0].0);
    }

    #[test]
    fn reviewer_reads_none_when_there_are_no_items() {
        let (_, user) = wc::reviewer_prompt("Polish", &ticket(), &[], &json!({}), "C.");
        assert_eq!(user, "Jira ticket ABC-42: Eksport raportu do CSV\nWork items:\n(none)\n\nAnalyst's notes:\n{}\nComment to review:\nC.\n");
    }

    #[test]
    fn numbering_continues_past_nine_for_the_reviewer() {
        let many: Vec<WorkItem> = (b'a'..=b'k').map(|c| WorkItem { line: format!("item {}", c as char), change: Some(format!("diff {}", c as char)) }).collect();
        let (_, user) = wc::reviewer_prompt("Polish", &ticket(), &many, &json!({}), "C.");
        let listed: String = (b'a'..=b'k').enumerate().map(|(i, c)| format!("{}. item {}\n", i + 1, c as char)).collect();
        assert_eq!(user, format!("Jira ticket ABC-42: Eksport raportu do CSV\nWork items:\n{listed}\nAnalyst's notes:\n{{}}\nComment to review:\nC.\n"));
    }
}

mod pipeline {
    use super::*;

    #[test]
    fn three_calls_progress_before_each_and_the_reviewers_comment_wins() {
        let r = run(vec![ok(analysis()), ok(json!({"comment": WRITTEN})), ok(json!({"comment": REVIEWED}))]);
        assert_eq!(r.progress, vec![(Step::Analyst, 0), (Step::Writer, 1), (Step::Reviewer, 2)]);
        assert_eq!(r.calls.len(), 3);
        assert_eq!(r.result.unwrap(), REVIEWED);
    }

    #[test]
    fn answers_after_thinking_and_in_fences_are_read_and_cleaned() {
        let r = run(vec![
            Ok(format!("<think>{{not json}}</think>```json\n{}\n```", analysis())),
            Ok(format!("Proszę:\n{}\n", json!({"comment": WRITTEN}))),
            Ok(format!("<think>{{x}}</think>```json\n{}\n```", json!({"comment": format!("`{REVIEWED}`  ")}))),
        ]);
        assert_eq!(r.calls[1].1, format!("Jira ticket ABC-42: Eksport raportu do CSV\nAnalyst's notes:\n{}", pretty(&analysis())));
        assert_eq!(r.result.unwrap(), REVIEWED);
    }

    #[test]
    fn reviewer_failure_or_no_comment_keeps_the_writers_comment() {
        let cases: Vec<(&str, Result<String>)> = vec![
            ("chat error", fail("LLM 503: overloaded")),
            ("no JSON", Ok("Looks fine to me.".into())),
            ("no comment key", ok(json!({"verdict": "ok"}))),
            ("comment not a string", ok(json!({"comment": ["a"]}))),
            ("blank comment", ok(json!({"comment": "  \n "}))),
        ];
        for (name, reviewer) in cases {
            let r = run(vec![ok(analysis()), ok(json!({"comment": format!(" {WRITTEN} ")})), reviewer]);
            assert_eq!(r.result.as_deref().ok(), Some(WRITTEN), "{name}");
            assert_eq!(r.calls.len(), 3, "{name}");
            assert_eq!(r.progress, vec![(Step::Analyst, 0), (Step::Writer, 1), (Step::Reviewer, 2)], "{name}");
        }
    }
}

mod analyst_failure {
    use super::*;

    #[test]
    fn chat_error_is_returned_and_nothing_else_is_called() {
        let r = run(vec![fail("LLM 503: overloaded"), ok(json!({"comment": WRITTEN}))]);
        assert_eq!(chain(&r.result.unwrap_err()), vec!["LLM 503: overloaded".to_string()]);
        assert_eq!(r.calls.len(), 1);
        assert_eq!(r.progress, vec![(Step::Analyst, 0)]);
    }

    #[test]
    fn answer_without_json_is_an_analyst_error() {
        let r = run(vec![Ok("I cannot tell.".into()), ok(json!({"comment": WRITTEN}))]);
        assert_eq!(chain(&r.result.unwrap_err()), vec!["analyst".to_string(), "LLM answer holds no JSON: I cannot tell.".to_string()]);
        assert_eq!(r.calls.len(), 1);
        assert_eq!(r.progress, vec![(Step::Analyst, 0)]);
    }
}

mod writer_failure {
    use super::*;

    #[test]
    fn chat_error_is_returned_and_the_reviewer_is_not_called() {
        let r = run(vec![ok(analysis()), fail("LLM 429: rate limited"), ok(json!({"comment": REVIEWED}))]);
        assert_eq!(chain(&r.result.unwrap_err()), vec!["LLM 429: rate limited".to_string()]);
        assert_eq!(r.calls.len(), 2);
        assert_eq!(r.progress, vec![(Step::Analyst, 0), (Step::Writer, 1)]);
    }

    #[test]
    fn answer_without_a_non_empty_comment_is_an_error_and_the_reviewer_is_not_called() {
        let cases: Vec<(&str, Result<String>)> = vec![
            ("no JSON", Ok("Dodano eksport.".into())),
            ("no comment key", ok(json!({"text": WRITTEN}))),
            ("comment not a string", ok(json!({"comment": 7}))),
            ("blank comment", ok(json!({"comment": " \n\t "}))),
            ("only quotes", ok(json!({"comment": "\"``\""}))),
        ];
        for (name, writer) in cases {
            let r = run(vec![ok(analysis()), writer, ok(json!({"comment": REVIEWED}))]);
            assert_eq!(chain(&r.result.unwrap_err()), vec!["writer returned no comment".to_string()], "{name}");
            assert_eq!(r.calls.len(), 2, "{name}");
            assert_eq!(r.progress, vec![(Step::Analyst, 0), (Step::Writer, 1)], "{name}");
        }
    }
}

mod clean {
    use super::*;

    #[test]
    fn strips_whitespace_wrapping_quotes_trailing_spaces_and_blank_lines() {
        let cases: [(&str, &str, &str); 9] = [
            ("surrounding whitespace", "  \n Dodano eksport. \t\n", "Dodano eksport."),
            ("double quotes", "\"Dodano eksport.\"", "Dodano eksport."),
            ("single quotes", "'Dodano eksport.'", "Dodano eksport."),
            ("backticks", "`Dodano eksport.`", "Dodano eksport."),
            ("Polish quotes", "„Dodano eksport.”", "Dodano eksport."),
            ("nested quotes and backticks with spaces", "  \" `Dodano eksport.` \"  ", "Dodano eksport."),
            ("nested without spaces", "\"`x`\"", "x"),
            ("trailing spaces per line, leading kept", "- Eksport CSV   \n  - separator ,\t", "- Eksport CSV\n  - separator ,"),
            ("blank and whitespace lines dropped, CRLF", "Linia 1\r\n\r\n   \r\nLinia 2\n\n", "Linia 1\nLinia 2"),
        ];
        for (name, input, expected) in cases {
            assert_eq!(wc::clean(input), expected, "{name}");
        }
    }

    #[test]
    fn only_a_matching_pair_wrapping_the_whole_text_is_stripped() {
        let cases: [(&str, &str, &str); 9] = [
            ("two quoted words", "\"CSV\" i \"PDF\"", "\"CSV\" i \"PDF\""),
            ("two Polish-quoted words", "„CSV” i „PDF”", "„CSV” i „PDF”"),
            ("two backticked names", "`csv` i `pdf`", "`csv` i `pdf`"),
            ("outer pair around inner single-quoted words", "\"'x' i 'y'\"", "'x' i 'y'"),
            ("double opening, single closing", "\"Dodano eksport.'", "\"Dodano eksport.'"),
            ("single opening, backtick closing", "'Dodano eksport.`", "'Dodano eksport.`"),
            ("Polish opening, straight closing", "„Dodano eksport.\"", "„Dodano eksport.\""),
            ("English curly quotes", "“Dodano eksport.”", "Dodano eksport."),
            ("two closing curly quotes", "”Dodano eksport.”", "Dodano eksport."),
        ];
        for (name, input, expected) in cases {
            assert_eq!(wc::clean(input), expected, "{name}");
        }
    }

    #[test]
    fn quotes_inside_the_text_stay() {
        assert_eq!(wc::clean("Przycisk \"CSV\" pobiera plik"), "Przycisk \"CSV\" pobiera plik");
        assert_eq!(wc::clean("\"x"), "\"x");
    }

    #[test]
    fn exactly_max_chars_of_polish_letters_is_unchanged() {
        let text = "ą".repeat(MAX_CHARS);
        assert_eq!(MAX_CHARS, 1000);
        assert_eq!(wc::clean(&text), text);
    }

    #[test]
    fn one_char_over_max_is_cut_to_max_chars_ending_in_ellipsis() {
        let text = format!("{}ż", "ą".repeat(MAX_CHARS));
        let expected = format!("{}…", "ą".repeat(MAX_CHARS - 1));
        let got = wc::clean(&text);
        assert_eq!(got, expected);
        assert_eq!(got.chars().count(), MAX_CHARS);
    }

    #[test]
    fn cut_counts_newlines_and_drops_the_space_before_the_ellipsis() {
        let text = format!("{}\n{} {}", "a".repeat(500), "b".repeat(497), "c".repeat(50));
        assert_eq!(wc::clean(&text), format!("{}\n{}…", "a".repeat(500), "b".repeat(497)));
    }
}

mod comment_of {
    use super::*;

    #[test]
    fn string_comment_is_read_and_cleaned_also_fenced_or_after_thinking() {
        let cases: [(&str, String); 3] = [
            ("plain", json!({"comment": "  \"Dodano eksport CSV.\"\n\n "}).to_string()),
            ("fenced", format!("```json\n{}\n```", json!({"comment": "Dodano eksport CSV."}))),
            ("after thinking", format!("<think>{{\"comment\": \"draft\"}}</think> {}", json!({"comment": "`Dodano eksport CSV.`"}))),
        ];
        for (name, answer) in cases {
            assert_eq!(wc::comment_of(&answer).as_deref(), Some("Dodano eksport CSV."), "{name}");
        }
    }

    #[test]
    fn missing_non_string_or_blank_comment_is_none() {
        let cases: [(&str, String); 7] = [
            ("no JSON", "Dodano eksport CSV.".into()),
            ("no comment key", json!({"text": "Dodano eksport CSV."}).to_string()),
            ("null", json!({"comment": null}).to_string()),
            ("number", json!({"comment": 3}).to_string()),
            ("object", json!({"comment": {"text": "x"}}).to_string()),
            ("blank", json!({"comment": "   "}).to_string()),
            ("array answer", json!([{"comment": "Dodano eksport CSV."}]).to_string()),
        ];
        for (name, answer) in cases {
            assert_eq!(wc::comment_of(&answer), None, "{name}");
        }
    }
}

mod to_start {
    use super::*;

    #[test]
    fn postable_wanting_rows_first_then_the_rest_in_row_order_up_to_the_room_left() {
        assert_eq!(wc::MAX_PARALLEL, 2);
        let wanting = [true, true, false, true, true];
        let cases: [(&str, &[bool], &[usize], usize, Vec<usize>); 15] = [
            ("room 2, nothing postable: first two wanting", &wanting, &[], 0, vec![0, 1]),
            ("room 1, nothing postable: first wanting", &wanting, &[], 1, vec![0]),
            ("room 0: none", &wanting, &[3], 2, vec![]),
            ("more running than the cap: none", &wanting, &[3], 3, vec![]),
            ("room 1: ticked row 3 wins over unticked row 0", &wanting, &[3], 1, vec![3]),
            ("room 2: ticked row 3, then unticked row 0", &wanting, &[3], 0, vec![3, 0]),
            ("ticked rows before an earlier unticked one", &wanting, &[4, 1], 0, vec![1, 4]),
            ("unsorted ticked rows in row order", &wanting, &[4, 3], 0, vec![3, 4]),
            ("duplicate ticked row taken once", &wanting, &[3, 3], 0, vec![3, 0]),
            ("ticked row that does not want is ignored", &wanting, &[2], 1, vec![0]),
            ("ticked index past the rows is ignored", &wanting, &[9], 1, vec![0]),
            ("no rows", &[], &[0], 0, vec![]),
            ("one wanting row", &[true], &[], 0, vec![0]),
            ("nobody wants", &[false, false, false], &[0, 2], 0, vec![]),
            ("only unticked rows want", &[false, true, false, true], &[0, 2], 0, vec![1, 3]),
        ];
        for (name, wanting, first, running, expected) in cases {
            assert_eq!(wc::to_start(wanting, first, running), expected, "{name}");
        }
    }
}

mod slots_taken {
    use super::*;

    #[test]
    fn running_rows_count_unless_they_want_a_restart() {
        let cases: [(&str, &[bool], &[bool], usize); 7] = [
            ("no rows", &[], &[], 0),
            ("two running, none wanting", &[true, false, true], &[false, false, false], 2),
            ("running row 2 about to restart", &[true, false, true], &[false, false, true], 1),
            ("running row 0 about to restart", &[true, false, true], &[true, false, false], 1),
            ("wanting row that is not running frees nothing", &[true, false, true], &[false, true, false], 2),
            ("all running rows restart", &[true, true], &[true, true], 0),
            ("wanting shorter than running: missing is not wanting", &[true, true, true], &[true], 2),
        ];
        for (name, running, wanting, expected) in cases {
            assert_eq!(wc::slots_taken(running, wanting), expected, "{name}");
        }
    }
}

mod accept_waits {
    use super::*;

    #[test]
    fn waits_only_for_a_postable_row_being_written() {
        let cases: [(&str, &[usize], &[bool], bool); 7] = [
            ("nothing postable", &[], &[true, true], false),
            ("postable row blocking", &[1], &[false, true], true),
            ("blocking row not postable", &[0, 2], &[false, true, false], false),
            ("second postable row blocking", &[0, 2], &[false, false, true], true),
            ("postable listed out of order", &[2, 0], &[true, false, false], true),
            ("postable index past the rows", &[3], &[false, true], false),
            ("none blocking", &[0, 1], &[false, false], false),
        ];
        for (name, postable, blocking, expected) in cases {
            assert_eq!(wc::accept_waits(postable, blocking), expected, "{name}");
        }
    }
}

mod ticket_summary {
    use super::*;
    use worklog::jira::Issue;

    fn issue(key: &str, summary: &str) -> Issue {
        Issue { key: key.into(), summary: summary.into(), status: "In Progress".into() }
    }

    #[test]
    fn own_issue_first_then_the_picked_one_else_empty() {
        let own = issue("ABC-42", "Eksport raportu do CSV");
        let picked = issue("ABC-7", "Import danych z PDF");
        let cases: [(&str, Option<&Issue>, Option<&Issue>, &str); 4] = [
            ("own only", Some(&own), None, "Eksport raportu do CSV"),
            ("picked only", None, Some(&picked), "Import danych z PDF"),
            ("both: own wins", Some(&own), Some(&picked), "Eksport raportu do CSV"),
            ("neither", None, None, ""),
        ];
        for (name, own, picked, expected) in cases {
            assert_eq!(wc::ticket_summary(own, picked), expected, "{name}");
        }
    }
}

mod work_items {
    use super::*;
    use chrono::{Local, TimeZone};
    use std::cell::Cell;
    use std::path::PathBuf;
    use worklog::git::Span;

    fn sources() -> Vec<(Vec<PathBuf>, String)> {
        vec![
            (vec![PathBuf::from("D:/repo/api")], "[api] ABC-42 add CSV export".to_owned()),
            (vec![PathBuf::from("D:/repo/web"), PathBuf::from("D:/repo/web-lib")], "[web] checkout feature/ABC-42".to_owned()),
            (vec![PathBuf::from("D:/repo/svc")], "[svc] ABC-42 comma separator".to_owned()),
        ]
    }

    fn span() -> Span {
        Span { from: Local.with_ymd_and_hms(2026, 11, 17, 8, 30, 0).unwrap(), to: Local.with_ymd_and_hms(2026, 11, 18, 0, 0, 0).unwrap() }
    }

    #[test]
    fn without_a_span_no_code_is_read_and_every_change_is_none() {
        let called = Cell::new(false);
        let got = wc::work_items(sources(), None, |_, _| {
            called.set(true);
            vec![Some("diff".to_owned()); 3]
        });
        assert!(!called.get(), "read was called without a span");
        assert_eq!(
            got,
            vec![
                WorkItem { line: "[api] ABC-42 add CSV export".into(), change: None },
                WorkItem { line: "[web] checkout feature/ABC-42".into(), change: None },
                WorkItem { line: "[svc] ABC-42 comma separator".into(), change: None },
            ]
        );
    }

    #[test]
    fn with_a_span_read_gets_the_sources_and_changes_map_by_position_short_result_is_none() {
        let seen = RefCell::new(None);
        let got = wc::work_items(sources(), Some(span()), |s, sp| {
            *seen.borrow_mut() = Some((s.to_vec(), sp));
            vec![None, Some(DIFF_3.to_owned())]
        });
        assert_eq!(seen.into_inner(), Some((sources(), span())));
        assert_eq!(
            got,
            vec![
                WorkItem { line: "[api] ABC-42 add CSV export".into(), change: None },
                WorkItem { line: "[web] checkout feature/ABC-42".into(), change: Some(DIFF_3.into()) },
                WorkItem { line: "[svc] ABC-42 comma separator".into(), change: None },
            ]
        );
    }

    #[test]
    fn with_a_span_every_result_lands_on_its_own_line() {
        let got = wc::work_items(sources(), Some(span()), |_, _| vec![Some(DIFF_1.to_owned()), None, Some(DIFF_3.to_owned())]);
        let changes: Vec<Option<&str>> = got.iter().map(|w| w.change.as_deref()).collect();
        assert_eq!(changes, vec![Some(DIFF_1), None, Some(DIFF_3)]);
    }
}

mod description {
    use super::*;

    const FALLBACK: &str = "add CSV export; comma separator";

    fn failed(d: &Description) -> Option<&str> {
        match &d.job {
            Job::Failed(e) => Some(e.as_str()),
            _ => None,
        }
    }

    fn idle(d: &Description) -> bool {
        matches!(d.job, Job::Idle)
    }

    fn started(key: &str) -> (Description, Sender<Msg>) {
        let (tx, rx) = channel();
        let mut d = Description::default();
        d.start(key, rx);
        (d, tx)
    }

    mod wants {
        use super::*;

        #[test]
        fn new_row_with_ticket_and_evidence_not_posted_wants_generation() {
            assert!(Description::default().wants(Some("ABC-42"), true, false));
        }

        #[test]
        fn each_blocking_condition_alone_stops_it() {
            let mut edited = Description::default();
            edited.edit();
            let mut same_key = Description::default();
            same_key.for_key = Some("ABC-42".into());
            let fresh = Description::default();
            let cases: [(&str, &Description, Option<&str>, bool, bool); 5] = [
                ("no ticket", &fresh, None, true, false),
                ("no evidence", &fresh, Some("ABC-42"), false, false),
                ("posted", &fresh, Some("ABC-42"), true, true),
                ("edited", &edited, Some("ABC-42"), true, false),
                ("already for this ticket", &same_key, Some("ABC-42"), true, false),
            ];
            for (name, d, key, evidence, posted) in cases {
                assert!(!d.wants(key, evidence, posted), "{name}");
            }
        }

        #[test]
        fn started_for_one_ticket_does_not_restart_it_but_another_ticket_does() {
            let (d, _tx) = started("ABC-42");
            assert!(!d.wants(Some("ABC-42"), true, false));
            assert!(d.wants(Some("ABC-7"), true, false));
        }

        #[test]
        fn written_for_a_ticket_is_not_restarted_but_another_ticket_is() {
            let (mut d, tx) = started("ABC-42");
            tx.send(Msg::Done(Ok(REVIEWED.into()))).unwrap();
            assert!(d.poll(FALLBACK));
            assert!(!d.wants(Some("ABC-42"), true, false));
            assert!(d.wants(Some("ABC-7"), true, false));
        }

        #[test]
        fn failed_for_a_ticket_is_not_restarted_but_another_ticket_is() {
            let mut d = Description::default();
            d.fail("ABC-42", "no LLM credentials".into(), FALLBACK);
            assert!(!d.wants(Some("ABC-42"), true, false));
            assert!(d.wants(Some("ABC-7"), true, false));
        }
    }

    mod start_and_poll {
        use super::*;

        #[test]
        fn start_runs_for_the_key_with_no_step_yet() {
            let (d, _tx) = started("ABC-42");
            assert_eq!(d.for_key.as_deref(), Some("ABC-42"));
            assert!(d.running());
            assert_eq!(d.step(), None);
        }

        #[test]
        fn nothing_sent_changes_nothing() {
            let (mut d, _tx) = started("ABC-42");
            assert!(!d.poll(FALLBACK));
            assert!(d.running());
            assert_eq!(d.step(), None);
            assert_eq!(d.text, "");
        }

        #[test]
        fn each_step_sent_is_the_step_while_running() {
            let (mut d, tx) = started("ABC-42");
            for s in Step::ALL {
                tx.send(Msg::Step(s)).unwrap();
                assert!(d.poll(FALLBACK), "{s:?}");
                assert_eq!(d.step(), Some(s));
                assert!(d.running());
            }
        }

        #[test]
        fn several_steps_in_one_poll_leave_the_last() {
            let (mut d, tx) = started("ABC-42");
            tx.send(Msg::Step(Step::Analyst)).unwrap();
            tx.send(Msg::Step(Step::Writer)).unwrap();
            assert!(d.poll(FALLBACK));
            assert_eq!(d.step(), Some(Step::Writer));
        }

        #[test]
        fn done_replaces_the_text_and_stops_running() {
            let (mut d, tx) = started("ABC-42");
            d.text = "stary tekst".into();
            tx.send(Msg::Step(Step::Reviewer)).unwrap();
            tx.send(Msg::Done(Ok(REVIEWED.into()))).unwrap();
            assert!(d.poll(FALLBACK));
            assert_eq!(d.text, REVIEWED);
            assert!(idle(&d));
            assert!(!d.running());
            assert_eq!(d.step(), None);
        }

        #[test]
        fn done_after_the_user_edited_keeps_the_users_text() {
            let (mut d, tx) = started("ABC-42");
            d.text = "Mój opis".into();
            d.edit();
            tx.send(Msg::Done(Ok(REVIEWED.into()))).unwrap();
            assert!(d.poll(FALLBACK));
            assert_eq!(d.text, "Mój opis");
            assert!(idle(&d));
        }

        #[test]
        fn error_result_falls_back_and_keeps_the_error() {
            let (mut d, tx) = started("ABC-42");
            d.text = "stary tekst".into();
            tx.send(Msg::Done(Err("writer returned no comment".into()))).unwrap();
            assert!(d.poll(FALLBACK));
            assert_eq!(d.text, FALLBACK);
            assert_eq!(failed(&d), Some("writer returned no comment"));
            assert!(!d.running());
        }

        #[test]
        fn error_result_after_the_user_edited_keeps_the_users_text() {
            let (mut d, tx) = started("ABC-42");
            d.text = "Mój opis".into();
            d.edit();
            tx.send(Msg::Done(Err("LLM 503".into()))).unwrap();
            assert!(d.poll(FALLBACK));
            assert_eq!(d.text, "Mój opis");
            assert_eq!(failed(&d), Some("LLM 503"));
        }

        #[test]
        fn thread_gone_without_an_answer_falls_back_with_an_error() {
            let (mut d, tx) = started("ABC-42");
            tx.send(Msg::Step(Step::Writer)).unwrap();
            drop(tx);
            assert!(d.poll(FALLBACK));
            assert_eq!(d.text, FALLBACK);
            assert_eq!(failed(&d), Some("writing the comment stopped unexpectedly"));
            assert_eq!(d.step(), None);
        }

        #[test]
        fn thread_gone_after_the_user_edited_keeps_the_users_text() {
            let (mut d, tx) = started("ABC-42");
            d.text = "Mój opis".into();
            d.edit();
            drop(tx);
            assert!(d.poll(FALLBACK));
            assert_eq!(d.text, "Mój opis");
            assert_eq!(failed(&d), Some("writing the comment stopped unexpectedly"));
        }

        #[test]
        fn poll_when_not_running_changes_nothing() {
            let mut idle_d = Description { text: "Opis".into(), ..Default::default() };
            assert!(!idle_d.poll(FALLBACK));
            assert_eq!(idle_d.text, "Opis");
            let mut failed_d = Description::default();
            failed_d.fail("ABC-42", "LLM 503".into(), "Opis");
            assert!(!failed_d.poll(FALLBACK));
            assert_eq!(failed_d.text, "Opis");
            assert_eq!(failed(&failed_d), Some("LLM 503"));
        }

        #[test]
        fn a_new_start_drops_the_earlier_jobs_answer() {
            let (mut d, old_tx) = started("ABC-42");
            let (tx, rx) = channel();
            d.start("ABC-7", rx);
            assert!(old_tx.send(Msg::Done(Ok("dla starego zgłoszenia".into()))).is_err(), "the earlier receiver is still alive");
            assert!(!d.poll(FALLBACK));
            tx.send(Msg::Done(Ok(REVIEWED.into()))).unwrap();
            assert!(d.poll(FALLBACK));
            assert_eq!(d.text, REVIEWED);
            assert_eq!(d.for_key.as_deref(), Some("ABC-7"));
        }
    }

    mod fail {
        use super::*;

        #[test]
        fn sets_the_key_the_error_and_the_fallback() {
            let mut d = Description { text: "stary tekst".into(), ..Default::default() };
            d.fail("ABC-42", "no LLM credentials".into(), FALLBACK);
            assert_eq!(d.for_key.as_deref(), Some("ABC-42"));
            assert_eq!(failed(&d), Some("no LLM credentials"));
            assert_eq!(d.text, FALLBACK);
            assert!(!d.running());
        }

        #[test]
        fn keeps_the_users_text() {
            let mut d = Description { text: "Mój opis".into(), ..Default::default() };
            d.edit();
            d.fail("ABC-42", "no LLM credentials".into(), FALLBACK);
            assert_eq!(d.text, "Mój opis");
            assert_eq!(failed(&d), Some("no LLM credentials"));
        }
    }

    mod edit_and_regenerate {
        use super::*;

        #[test]
        fn regenerate_after_edit_wants_generation_again_for_the_same_ticket() {
            let (mut d, tx) = started("ABC-42");
            tx.send(Msg::Done(Ok(REVIEWED.into()))).unwrap();
            assert!(d.poll(FALLBACK));
            d.text = "Mój opis".into();
            d.edit();
            assert!(!d.wants(Some("ABC-42"), true, false));
            d.regenerate();
            assert!(!d.edited);
            assert_eq!(d.for_key, None);
            assert!(idle(&d));
            assert!(d.wants(Some("ABC-42"), true, false));
        }

        #[test]
        fn regenerated_text_replaces_the_users_text() {
            let mut d = Description { text: "Mój opis".into(), ..Default::default() };
            d.edit();
            d.regenerate();
            let (tx, rx) = channel();
            d.start("ABC-42", rx);
            tx.send(Msg::Done(Ok(REVIEWED.into()))).unwrap();
            assert!(d.poll(FALLBACK));
            assert_eq!(d.text, REVIEWED);
        }

        #[test]
        fn regenerate_while_running_stops_the_job() {
            let (mut d, tx) = started("ABC-42");
            tx.send(Msg::Step(Step::Writer)).unwrap();
            assert!(d.poll(FALLBACK));
            d.regenerate();
            assert!(!d.running());
            assert_eq!(d.step(), None);
            assert!(d.wants(Some("ABC-42"), true, false));
        }
    }

    mod accept {
        use super::*;

        #[test]
        fn blocks_only_while_running_and_not_edited() {
            let (running, _tx1) = started("ABC-42");
            let (mut running_edited, _tx2) = started("ABC-42");
            running_edited.edit();
            let mut failed_d = Description::default();
            failed_d.fail("ABC-42", "LLM 503".into(), FALLBACK);
            let cases: [(&str, &Description, bool); 4] = [
                ("running", &running, true),
                ("running, edited", &running_edited, false),
                ("idle", &Description::default(), false),
                ("failed", &failed_d, false),
            ];
            for (name, d, expected) in cases {
                assert_eq!(d.blocks_accept(), expected, "{name}");
            }
        }

        #[test]
        fn holds_accept_while_written_unless_edited_or_while_waiting_its_turn() {
            let (running, _tx1) = started("ABC-42");
            let (mut running_edited, _tx2) = started("ABC-42");
            running_edited.edit();
            let idle = Description::default();
            let mut failed_d = Description::default();
            failed_d.fail("ABC-42", "LLM 503".into(), FALLBACK);
            let cases: [(&str, &Description, bool, bool); 6] = [
                ("idle, wanting: queued row would post an empty comment", &idle, true, true),
                ("idle, not wanting", &idle, false, false),
                ("running, not edited, not wanting", &running, false, true),
                ("running, edited, not wanting", &running_edited, false, false),
                ("running, edited, wanting", &running_edited, true, true),
                ("failed, not wanting", &failed_d, false, false),
            ];
            for (name, d, wanting, expected) in cases {
                assert_eq!(d.holds_accept(wanting), expected, "{name}");
            }
        }

        #[test]
        fn comment_is_the_trimmed_text() {
            let d = Description { text: " \n Raport jako CSV.\n- separator ,  \n\t".into(), ..Default::default() };
            assert_eq!(d.comment(), "Raport jako CSV.\n- separator ,");
        }

        #[test]
        fn comment_of_an_empty_row_is_empty() {
            assert_eq!(Description::default().comment(), "");
        }
    }
}
