//! Ticket proposals in four model calls, each with one job, the way a team would write a ticket:
//! an analyst works out from the code changes what the work was for, a ticket writer turns that
//! into a task, a tester adds test scenarios, a reviewer strikes what is vague or unfounded.
//! One call doing all of it guessed the purpose from commit subjects and produced filler.
//!
//! The description is assembled here, not by the model, so its sections and markup are always
//! the same.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::llm::{MAX_DRAFTS, MAX_TITLE_CHARS, TicketDraft, json_answer};

/// One model call: (system, user) in, the assistant's text out.
pub trait Chat {
    fn chat(&self, system: &str, user: &str) -> Result<String>;
}

impl Chat for crate::llm::Llm {
    fn chat(&self, system: &str, user: &str) -> Result<String> {
        crate::llm::Llm::chat(self, system, user)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Analyst,
    Writer,
    Tester,
    Reviewer,
}

impl Stage {
    pub const ALL: [Stage; 4] = [Stage::Analyst, Stage::Writer, Stage::Tester, Stage::Reviewer];

    /// What the step does, for the progress line.
    pub fn label(self) -> &'static str {
        match self {
            Stage::Analyst => "analyst reads the changes",
            Stage::Writer => "writer drafts the tickets",
            Stage::Tester => "tester adds test scenarios",
            Stage::Reviewer => "reviewer checks the tickets",
        }
    }

    /// 1-based position in the pipeline.
    pub fn number(self) -> usize {
        Stage::ALL.iter().position(|&s| s == self).map_or(0, |i| i + 1)
    }
}

/// A ticked line and, when found, what it changed (commit message and diff, or a file's diff).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkItem {
    pub line: String,
    pub change: Option<String>,
}

/// Section headings: the team's Polish names, English for any other language.
pub fn section_names(language: &str) -> [&'static str; 3] {
    if language.to_lowercase().starts_with("pol") {
        ["Opis", "Kryteria akceptacji", "Przypadki testowe"]
    } else {
        ["Description", "Acceptance criteria", "Test cases"]
    }
}

/// Jira wiki markup: the summary, a bullet list of criteria, a numbered list of test cases. Empty
/// sections are left out; list entries lose any bullet or number the model put in front.
pub fn render_description(language: &str, summary: &str, criteria: &[String], tests: &[String]) -> String {
    let [opis, crit, test] = section_names(language);
    let list = |marker: &str, entries: &[String]| -> String {
        entries.iter().map(|e| strip_marker(e)).filter(|e| !e.is_empty()).map(|e| format!("{marker} {e}\n")).collect()
    };
    let mut out = Vec::new();
    if !summary.trim().is_empty() {
        out.push(format!("h3. {opis}\n{}\n", summary.trim()));
    }
    let c = list("*", criteria);
    if !c.is_empty() {
        out.push(format!("h3. {crit}\n{c}"));
    }
    let t = list("#", tests);
    if !t.is_empty() {
        out.push(format!("h3. {test}\n{t}"));
    }
    out.join("\n").trim_end().to_owned()
}

/// A marker counts only when whitespace follows it, so "-1 is rejected", "1.5 h" and Jira bold
/// "*Admin* can" keep their first character.
fn strip_marker(entry: &str) -> String {
    let e = entry.trim();
    let after_bullets = e.trim_start_matches(['*', '#', '-', '•']);
    // An entry of markers alone (the model's empty bullet) is nothing.
    let marker_end = |rest: &str| rest.is_empty() || rest.starts_with(char::is_whitespace);
    let e = if after_bullets.len() < e.len() && marker_end(after_bullets) { after_bullets.trim_start() } else { e };
    // "1. ", "2) "
    let digits = e.chars().take_while(char::is_ascii_digit).count();
    let numbered = digits > 0 && matches!(e[digits..].chars().next(), Some('.' | ')')) && marker_end(&e[digits + 1..]);
    let e = if numbered { e[digits + 1..].trim_start() } else { e };
    e.split_whitespace().collect::<Vec<_>>().join(" ")
}

const NO_FILLER: &str = "Banned filler, because it says nothing a reader can check: \"works correctly\", \"handles \
    properly\", \"is covered by tests\", \"business logic\", \"improves stability\", \"according to the new logic\", \
    and their equivalents in any language. Say instead exactly what happens: which input, which situation, which \
    result (a status code, a message, a value, a state).";

const TASK_NOT_REPORT: &str = "The ticket reads as a task written BEFORE the work: it states the problem or need \
    and the expected outcome. It is never a report of what was done: no past tense like \"fixed\" or \"added\", no \
    commit wording, no lists of changed files, classes or tests; name a component, endpoint or screen only where it \
    defines the scope.";

fn numbered(items: &[WorkItem], with_changes: bool) -> String {
    let mut out = String::new();
    for (n, it) in items.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", n + 1, it.line));
        if with_changes {
            match &it.change {
                Some(c) => out.push_str(&format!("```\n{}\n```\n", c.trim_end())),
                None => out.push_str("(no code change available)\n"),
            }
        }
    }
    out
}

/// A ticket that exists in Jira and is being rewritten.
#[derive(Debug, Clone, PartialEq)]
pub struct CurrentTicket {
    pub key: String,
    pub title: String,
    pub description: String,
}

fn current_text(c: &CurrentTicket) -> String {
    let d = if c.description.trim().is_empty() { "(empty)" } else { c.description.trim() };
    format!("Existing ticket {}:\nTitle: {}\nDescription:\n{d}\n", c.key, c.title.trim())
}

pub fn analyst_prompt(project: &str, items: &[WorkItem]) -> (String, String) {
    analyst_prompt_for(project, items, None)
}

/// `analyst_prompt` for rewriting `current`: one task, the ticket's own statements count as known.
pub fn analyst_prompt_for(project: &str, items: &[WorkItem], current: Option<&CurrentTicket>) -> (String, String) {
    let mut system = format!(
        "You are a business and systems analyst. A developer did some work; you get its items (commits, branches, \
         changed files), most with the code change. Work out from the code what requirement the work fulfils: \
         the situation before (what went wrong or was missing, for whom, when), the behaviour required now \
         (concrete: inputs, outputs, status codes, messages, rules, data), and what must keep working as before. \
         Read the diffs; do not paraphrase commit subjects. Group items serving one purpose into one task: \
         refactoring, formatting and tests belong to the task they support, not a task of their own. At most \
         {MAX_DRAFTS} tasks; separate tasks only for clearly unrelated purposes. Use only what the changes show; \
         when the purpose cannot be told from them, say so in \"unclear\" instead of guessing. Write in English. \
         Answer with JSON only: {{\"tasks\": [{{\"items\": [1, 2], \"area\": \"feature or component as its users \
         name it\", \"problem\": \"...\", \"required_behaviour\": \"...\", \"keep_working\": \"...\", \
         \"testable\": true, \"unclear\": \"...\"}}]}}"
    );
    let mut user = format!("Jira project: {project}\nWork items:\n{}", if items.is_empty() { "(none)\n".to_owned() } else { numbered(items, true) });
    if let Some(c) = current {
        system.push_str(&format!(
            " All of this belongs to the existing ticket {}, given below: produce exactly one task, covering the \
             ticket's scope. What the ticket states counts as known; the work items show what it required in fact. \
             There may be no work items yet; then work from the ticket alone.",
            c.key
        ));
        user.push_str(&format!("\n{}", current_text(c)));
    }
    (system, user)
}

pub fn writer_prompt(project: &str, language: &str, items: &[WorkItem], analysis: &Value) -> (String, String) {
    writer_prompt_for(project, language, items, analysis, None)
}

/// `writer_prompt` for rewriting `current`: exactly one ticket, a better version of it.
pub fn writer_prompt_for(project: &str, language: &str, items: &[WorkItem], analysis: &Value, current: Option<&CurrentTicket>) -> (String, String) {
    let mut system = format!(
        "You are a product owner writing Jira tickets in {language} from an analyst's notes, one ticket per task, \
         in the same order. {TASK_NOT_REPORT} The title is one short line naming the problem or the required \
         behaviour, without a ticket key or a prefix like \"feat:\". The summary is 2–4 sentences: the context, \
         the problem or need in concrete terms, and why it matters. The criteria are 2–5 verifiable outcomes, \
         each observable from outside (what a user, a client or the API sees), never about how the work is done \
         or tested. {NO_FILLER} Do not add facts the notes do not hold; where the notes say something is unclear, \
         leave it out rather than invent it. Answer with JSON only: {{\"tickets\": [{{\"task\": 1, \
         \"title\": \"...\", \"summary\": \"...\", \"criteria\": [\"...\"]}}]}}"
    );
    let mut user = format!(
        "Jira project: {project}\nWork items:\n{}\nAnalyst's notes:\n{}",
        numbered(items, false),
        serde_json::to_string_pretty(analysis).unwrap_or_default()
    );
    if let Some(c) = current {
        system.push_str(&format!(
            " Write exactly one ticket: a better version of the existing ticket {}, given below, keeping its scope \
             and any fact it states that the notes do not contradict.",
            c.key
        ));
        user.push_str(&format!("\n{}", current_text(c)));
    }
    (system, user)
}

pub fn tester_prompt(language: &str, analysis: &Value, tickets: &Value) -> (String, String) {
    let system = format!(
        "You are a QA engineer. For each ticket write test scenarios in {language} that prove its acceptance \
         criteria: each one line with the situation (concrete data), the action and the expected result. Cover \
         the main case, the edge case the work is about, and the behaviour that must keep working. 2–5 \
         scenarios per ticket, only when the task has behaviour that can be tested (the analyst's \"testable\"); \
         otherwise an empty list. {NO_FILLER} Answer with JSON only, one entry per ticket in order: \
         {{\"tests\": [{{\"ticket\": 1, \"cases\": [\"...\"]}}]}}"
    );
    let user = format!(
        "Analyst's notes:\n{}\nTickets:\n{}",
        serde_json::to_string_pretty(analysis).unwrap_or_default(),
        serde_json::to_string_pretty(tickets).unwrap_or_default()
    );
    (system, user)
}

pub fn reviewer_prompt(language: &str, items: &[WorkItem], analysis: &Value, tickets: &Value) -> (String, String) {
    let system = format!(
        "You are a strict reviewer of Jira tickets written in {language}. Check each ticket against the analyst's \
         notes and the work items, and fix it: remove every claim the notes do not support, replace vague wording \
         with the concrete behaviour from the notes, make every criterion verifiable, make every test case \
         concrete (situation, action, expected result), drop test cases that only repeat another. \
         {TASK_NOT_REPORT} {NO_FILLER} The title must tell a reader who knows nothing of the work what the task \
         is about. Keep the tickets in the same order and number. Answer with the corrected tickets in full, JSON \
         only: {{\"tickets\": [{{\"title\": \"...\", \"summary\": \"...\", \"criteria\": [\"...\"], \
         \"tests\": [\"...\"]}}]}}"
    );
    let user = format!(
        "Work items:\n{}\nAnalyst's notes:\n{}\nTickets to review:\n{}",
        numbered(items, false),
        serde_json::to_string_pretty(analysis).unwrap_or_default(),
        serde_json::to_string_pretty(tickets).unwrap_or_default()
    );
    (system, user)
}

/// Ticket parts as the writer and reviewer return them.
#[derive(Debug, Clone, PartialEq, Default)]
struct Parts {
    title: String,
    summary: String,
    criteria: Vec<String>,
    tests: Option<Vec<String>>,
}

fn strings(v: &Value) -> Option<Vec<String>> {
    v.as_array().map(|a| a.iter().filter_map(Value::as_str).map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect())
}

fn parts(v: &Value) -> Parts {
    let title: String = v["title"].as_str().unwrap_or_default().trim().lines().next().unwrap_or_default().trim().chars().take(MAX_TITLE_CHARS).collect();
    Parts {
        title,
        summary: v["summary"].as_str().unwrap_or_default().trim().to_owned(),
        criteria: strings(&v["criteria"]).unwrap_or_default(),
        tests: strings(&v["tests"]),
    }
}

fn list<'a>(v: &'a Value, key: &str) -> Vec<&'a Value> {
    match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) => o.get(key).and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// 0-based, sorted, distinct item indices from a list of 1-based numbers.
fn item_indices(v: &Value) -> Vec<usize> {
    let mut out: Vec<usize> = v
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_u64).filter(|&n| n >= 1).filter_map(|n| usize::try_from(n - 1).ok()).collect())
        .unwrap_or_default();
    out.sort_unstable();
    out.dedup();
    out
}

/// Runs the four steps; `progress` hears each step before it starts. The analyst and the writer
/// are required; a failed tester leaves the tickets without test cases and a failed reviewer
/// leaves them as the writer and tester made them (both logged).
pub fn draft_tickets(llm: &dyn Chat, project: &str, language: &str, items: &[WorkItem], progress: &dyn Fn(Stage)) -> Result<Vec<TicketDraft>> {
    run(llm, project, language, items, None, progress)
}

/// The same four steps rewriting the existing ticket `current` from its own text and `items` (the
/// work done for it so far, possibly none): one draft, its `items` as the analyst named them.
pub fn redraft_ticket(
    llm: &dyn Chat,
    project: &str,
    language: &str,
    current: &CurrentTicket,
    items: &[WorkItem],
    progress: &dyn Fn(Stage),
) -> Result<TicketDraft> {
    let mut drafts = run(llm, project, language, items, Some(current), progress)?;
    Ok(drafts.swap_remove(0))
}

fn run(llm: &dyn Chat, project: &str, language: &str, items: &[WorkItem], current: Option<&CurrentTicket>, progress: &dyn Fn(Stage)) -> Result<Vec<TicketDraft>> {
    let max = if current.is_some() { 1 } else { MAX_DRAFTS };
    progress(Stage::Analyst);
    let (s, u) = analyst_prompt_for(project, items, current);
    let analysis = json_answer(&llm.chat(&s, &u)?).context("analyst")?;
    let tasks = list(&analysis, "tasks");
    if tasks.is_empty() {
        bail!("analyst found no task in the ticked work");
    }
    log::debug!("analyst: {analysis}");

    progress(Stage::Writer);
    let (s, u) = writer_prompt_for(project, language, items, &analysis, current);
    let written_v = json_answer(&llm.chat(&s, &u)?).context("ticket writer")?;
    let written: Vec<(Parts, Vec<usize>)> = list(&written_v, "tickets")
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            // The writer names its task; without that, tickets follow the tasks in order.
            let task = t["task"].as_u64().and_then(|n| usize::try_from(n).ok()).filter(|&n| n >= 1 && n <= tasks.len()).map_or(i, |n| n - 1);
            (parts(t), tasks.get(task).map(|t| item_indices(&t["items"])).unwrap_or_default())
        })
        .filter(|(p, _)| !p.title.is_empty())
        .take(max)
        .collect();
    if written.is_empty() {
        bail!("ticket writer returned no ticket with a title");
    }
    let tickets_json = json!({ "tickets": written.iter().map(|(p, _)| json!({ "title": p.title, "summary": p.summary, "criteria": p.criteria })).collect::<Vec<_>>() });

    progress(Stage::Tester);
    let (s, u) = tester_prompt(language, &analysis, &tickets_json);
    let tests: Vec<Vec<String>> = match llm.chat(&s, &u).and_then(|a| json_answer(&a)) {
        Ok(v) => {
            let entries = list(&v, "tests");
            // Once the tester numbers its entries, a ticket it gave no number gets no cases rather
            // than another ticket's by position.
            let numbered = entries.iter().any(|e| e["ticket"].as_u64().is_some());
            (0..written.len())
                .map(|i| {
                    let entry = if numbered { entries.iter().find(|e| e["ticket"].as_u64() == Some(i as u64 + 1)) } else { entries.get(i) };
                    entry.and_then(|e| strings(&e["cases"])).unwrap_or_default()
                })
                .collect()
        }
        Err(e) => {
            log::warn!("tester step failed, tickets go without test cases: {e:#}");
            vec![Vec::new(); written.len()]
        }
    };
    let mut drafted: Vec<(Parts, Vec<usize>)> =
        written.into_iter().zip(tests).map(|((mut p, items), t)| {
            p.tests = Some(t);
            (p, items)
        }).collect();

    progress(Stage::Reviewer);
    let review_input = json!({ "tickets": drafted.iter().map(|(p, _)| json!({
        "title": p.title, "summary": p.summary, "criteria": p.criteria, "tests": p.tests.clone().unwrap_or_default()
    })).collect::<Vec<_>>() });
    let (s, u) = reviewer_prompt(language, items, &analysis, &review_input);
    match llm.chat(&s, &u).and_then(|a| json_answer(&a)) {
        Ok(v) => {
            let reviewed = list(&v, "tickets");
            if reviewed.len() == drafted.len() {
                for ((p, _), r) in drafted.iter_mut().zip(reviewed) {
                    let r = parts(r);
                    if r.title.is_empty() {
                        continue;
                    }
                    p.title = r.title;
                    if !r.summary.is_empty() {
                        p.summary = r.summary;
                    }
                    if !r.criteria.is_empty() {
                        p.criteria = r.criteria;
                    }
                    // An empty list is the reviewer's verdict that the task has nothing to test.
                    if let Some(t) = r.tests {
                        p.tests = Some(t);
                    }
                }
            } else {
                log::warn!("reviewer returned {} tickets for {}; keeping the unreviewed ones", reviewed.len(), drafted.len());
            }
        }
        Err(e) => log::warn!("reviewer step failed, keeping the unreviewed tickets: {e:#}"),
    }

    Ok(drafted
        .into_iter()
        .map(|(p, items)| TicketDraft {
            description: render_description(language, &p.summary, &p.criteria, p.tests.as_deref().unwrap_or_default()),
            title: p.title,
            items,
        })
        .collect())
}
