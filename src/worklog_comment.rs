//! The worklog comment of a row, written by the LLM in three calls: an analyst works out from the
//! day's changes what was done for the ticket, a writer turns that into a short comment, a
//! reviewer strikes what the changes do not support. Unlike a ticket, a worklog comment reports
//! work already done, so `drafting`'s prompts (tasks written before the work) do not fit.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

use crate::drafting::{Chat, WorkItem};
use crate::git::Span;
use crate::llm::{Llm, LlmCredentials, json_answer};

/// What the writer is asked to stay under; `clean` cuts only far past it.
pub const TARGET_CHARS: usize = 300;
/// The hard cap on a generated comment, for a model that ignores the target.
pub const MAX_CHARS: usize = 1000;
const CHANGE_CHARS: usize = 8_000;
const CHANGES_TOTAL_CHARS: usize = 40_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Analyst,
    Writer,
    Reviewer,
}

impl Step {
    pub const ALL: [Step; 3] = [Step::Analyst, Step::Writer, Step::Reviewer];

    pub fn label(self) -> &'static str {
        match self {
            Step::Analyst => "analyst reads the changes",
            Step::Writer => "writer drafts the comment",
            Step::Reviewer => "reviewer checks it",
        }
    }

    /// 1-based position in the pipeline.
    pub fn number(self) -> usize {
        Step::ALL.iter().position(|&s| s == self).map_or(0, |i| i + 1)
    }
}

/// The ticket the time is logged on.
#[derive(Debug, Clone, PartialEq)]
pub struct Ticket {
    pub key: String,
    pub summary: String,
}

const NO_FILLER: &str = "No filler that says nothing a reader can check (\"various fixes\", \"improvements\", \
    \"works correctly\", \"code cleanup\" without what was cleaned), in any language.";

fn numbered(items: &[WorkItem], with_changes: bool) -> String {
    let mut out = String::new();
    for (n, it) in items.iter().enumerate() {
        out.push_str(&format!("{}. {}\n", n + 1, it.line));
        if with_changes && let Some(c) = &it.change {
            out.push_str(&format!("```\n{}\n```\n", c.trim_end()));
        }
    }
    out
}

fn ticket_line(t: &Ticket) -> String {
    format!("Jira ticket {}: {}", t.key, t.summary.trim())
}

pub fn analyst_prompt(ticket: &Ticket, items: &[WorkItem]) -> (String, String) {
    let system = "You are an analyst. A developer logs the time of one day on a Jira ticket; you get the ticket \
        and the day's work items (commits, branch checkouts, changed files), most with the code change. Work out \
        what was actually done for the ticket, in terms its readers (team lead, tester) understand: outcomes and \
        behaviour, not file names. Read the diffs; do not paraphrase commit subjects. Branch checkouts say nothing \
        about the work unless they are all there is. Use only what the items show; what cannot be told from them \
        goes in \"unclear\". Write in English. Answer with JSON only: {\"done\": [\"...\"], \"unclear\": \"...\"}"
        .to_owned();
    let user = format!("{}\nWork items:\n{}", ticket_line(ticket), if items.is_empty() { "(none)\n".to_owned() } else { numbered(items, true) });
    (system, user)
}

pub fn writer_prompt(language: &str, ticket: &Ticket, analysis: &Value) -> (String, String) {
    let system = format!(
        "You write the comment of a Jira worklog in {language}: what the logged time was spent on, for the \
         ticket's readers. One to three short sentences, or up to four short lines each starting with \"- \"; \
         at most {TARGET_CHARS} characters. It reports work done, so past tense or noun phrases are right. No \
         ticket key, no commit hashes, no lists of files or classes. {NO_FILLER} Only facts from the analyst's \
         notes. Answer with JSON only: {{\"comment\": \"...\"}}"
    );
    let user = format!("{}\nAnalyst's notes:\n{}", ticket_line(ticket), serde_json::to_string_pretty(analysis).unwrap_or_default());
    (system, user)
}

pub fn reviewer_prompt(language: &str, ticket: &Ticket, items: &[WorkItem], analysis: &Value, comment: &str) -> (String, String) {
    let system = format!(
        "You are a strict reviewer of Jira worklog comments written in {language}. Check the comment against the \
         analyst's notes and the work items and correct it: remove every claim they do not support, replace vague \
         wording with what was done, drop ticket keys, hashes and file lists, keep it at most {TARGET_CHARS} \
         characters and in {language}. {NO_FILLER} Answer with the corrected comment in full, JSON only: \
         {{\"comment\": \"...\"}}"
    );
    let user = format!(
        "{}\nWork items:\n{}\nAnalyst's notes:\n{}\nComment to review:\n{comment}\n",
        ticket_line(ticket),
        if items.is_empty() { "(none)\n".to_owned() } else { numbered(items, false) },
        serde_json::to_string_pretty(analysis).unwrap_or_default()
    );
    (system, user)
}

/// The model's text made fit for a worklog: trimmed, without wrapping quotes or backticks,
/// trailing spaces off each line, blank lines dropped, and cut to `MAX_CHARS` characters
/// (ending in "…").
pub fn clean(text: &str) -> String {
    let mut t = text.trim();
    // Only a pair wrapping the whole text: `"CSV" i "PDF"` starts and ends with quotes of two words.
    const PAIRS: [(char, char); 6] = [('"', '"'), ('\'', '\''), ('`', '`'), ('„', '”'), ('“', '”'), ('”', '”')];
    while let Some(inner) = PAIRS
        .iter()
        .find_map(|&(open, close)| t.strip_prefix(open).and_then(|r| r.strip_suffix(close)).filter(|inner| !inner.contains(close)))
    {
        t = inner.trim();
    }
    let joined = t.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n");
    if joined.chars().count() <= MAX_CHARS {
        return joined;
    }
    let cut: String = joined.chars().take(MAX_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// The `comment` of a writer's or reviewer's answer, cleaned; `None` when it holds none.
pub fn comment_of(answer: &str) -> Option<String> {
    let v = json_answer(answer).ok()?;
    let c = clean(v["comment"].as_str()?);
    (!c.is_empty()).then_some(c)
}

/// Runs the three steps; `progress` hears each before it starts. The analyst and the writer are
/// required; a failed or empty review keeps the writer's comment (logged).
pub fn describe(llm: &dyn Chat, language: &str, ticket: &Ticket, items: &[WorkItem], progress: &dyn Fn(Step)) -> Result<String> {
    progress(Step::Analyst);
    let (s, u) = analyst_prompt(ticket, items);
    let analysis = json_answer(&llm.chat(&s, &u)?).context("analyst")?;
    log::debug!("worklog analyst for {}: {analysis}", ticket.key);

    progress(Step::Writer);
    let (s, u) = writer_prompt(language, ticket, &analysis);
    let written = comment_of(&llm.chat(&s, &u)?).ok_or_else(|| anyhow!("writer returned no comment"))?;

    progress(Step::Reviewer);
    let (s, u) = reviewer_prompt(language, ticket, items, &analysis, &written);
    match llm.chat(&s, &u) {
        Ok(a) => match comment_of(&a) {
            Some(c) => Ok(c),
            None => {
                log::warn!("worklog reviewer for {} returned no comment; keeping the writer's", ticket.key);
                Ok(written)
            }
        },
        Err(e) => {
            log::warn!("worklog reviewer for {} failed, keeping the writer's comment: {e:#}", ticket.key);
            Ok(written)
        }
    }
}

/// How many rows are written at once: the endpoint may rate-limit many parallel calls, and each
/// row also reads its diffs from git.
pub const MAX_PARALLEL: usize = 2;

/// The rows to start now: those wanting a comment, the ones Accept would post (`first`) before
/// the rest, each in row order, as many as `MAX_PARALLEL` leaves room for beside `running`.
pub fn to_start(wanting: &[bool], first: &[usize], running: usize) -> Vec<usize> {
    let room = MAX_PARALLEL.saturating_sub(running);
    let mut firsts: Vec<usize> = first.iter().copied().filter(|&i| wanting.get(i).copied().unwrap_or(false)).collect();
    firsts.sort_unstable();
    firsts.dedup();
    let rest = (0..wanting.len()).filter(|&i| wanting[i] && !firsts.contains(&i));
    firsts.iter().copied().chain(rest).take(room).collect()
}

/// The slots the jobs take: a row running for a ticket it no longer has (`wanting` again) is
/// about to be restarted and takes none.
pub fn slots_taken(running: &[bool], wanting: &[bool]) -> usize {
    running.iter().enumerate().filter(|&(i, &r)| r && !wanting.get(i).copied().unwrap_or(false)).count()
}

/// Accept waits while one of the rows it would post (`postable`) is still being written; an
/// unticked or ticketless row being written does not hold it up.
pub fn accept_waits(postable: &[usize], blocking: &[bool]) -> bool {
    postable.iter().any(|&i| blocking.get(i).copied().unwrap_or(false))
}

/// The summary of the ticket the time goes on: the row's own issue, else the one picked for it.
pub fn ticket_summary(own: Option<&crate::jira::Issue>, picked: Option<&crate::jira::Issue>) -> String {
    own.or(picked).map(|i| i.summary.clone()).unwrap_or_default()
}

/// The lines with what they changed, read by `read` only when there is a span; without one
/// (`send_code` off) no code goes to the model.
pub fn work_items(
    sources: Vec<(Vec<PathBuf>, String)>,
    span: Option<Span>,
    read: impl FnOnce(&[(Vec<PathBuf>, String)], Span) -> Vec<Option<String>>,
) -> Vec<WorkItem> {
    let changes = match span {
        Some(s) => read(&sources, s),
        None => Vec::new(),
    };
    sources
        .into_iter()
        .enumerate()
        .map(|(i, (_, line))| WorkItem { line, change: changes.get(i).cloned().flatten() })
        .collect()
}

pub enum Msg {
    Step(Step),
    Done(Result<String, String>),
}

/// Reads the changes of `lines` (each with its repos' paths; no span means no code is sent) and
/// runs `describe` on a thread.
pub fn spawn(creds: LlmCredentials, language: String, ticket: Ticket, sources: Vec<(Vec<PathBuf>, String)>, span: Option<Span>) -> Receiver<Msg> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let items = work_items(sources, span, |s, span| crate::git::change_contexts(s, span, CHANGE_CHARS, CHANGES_TOTAL_CHARS));
        let progress_tx = tx.clone();
        let key = ticket.key.clone();
        let progress = move |s: Step| {
            log::info!("worklog comment for {key}: step {}/3: {}", s.number(), s.label());
            let _ = progress_tx.send(Msg::Step(s));
        };
        let res = describe(&Llm::new(creds), &language, &ticket, &items, &progress).map_err(|e| format!("{e:#}"));
        if let Err(e) = &res {
            log::error!("worklog comment for {}: {e}", ticket.key);
        }
        let _ = tx.send(Msg::Done(res));
    });
    rx
}

#[derive(Default)]
pub enum Job {
    #[default]
    Idle,
    Running {
        rx: Receiver<Msg>,
        step: Option<Step>,
    },
    Failed(String),
}

/// One row's Description: the text Accept posts and how it is being written.
#[derive(Default)]
pub struct Description {
    pub text: String,
    /// The user typed in the field: a generated text never replaces it until ⟳ is pressed.
    pub edited: bool,
    /// The ticket the text was last written (or is being written) for; another ticket starts over.
    pub for_key: Option<String>,
    pub job: Job,
}

impl Description {
    /// Whether a comment should be written now: the row has a ticket and work to describe, is not
    /// posted, the user has not written their own, and none was written or started for this ticket.
    pub fn wants(&self, key: Option<&str>, has_evidence: bool, posted: bool) -> bool {
        match key {
            Some(k) => has_evidence && !posted && !self.edited && self.for_key.as_deref() != Some(k),
            None => false,
        }
    }

    /// Writing for `key` has started; any earlier job's answer is dropped with its receiver.
    pub fn start(&mut self, key: &str, rx: Receiver<Msg>) {
        self.for_key = Some(key.to_owned());
        self.job = Job::Running { rx, step: None };
    }

    /// Writing for `key` could not start or failed: the row keeps the user's text, or gets
    /// `fallback` (the commit subjects) when it has none.
    pub fn fail(&mut self, key: &str, error: String, fallback: &str) {
        self.for_key = Some(key.to_owned());
        self.job = Job::Failed(error);
        if !self.edited {
            self.text = fallback.to_owned();
        }
    }

    /// Takes in what the running job sent. A finished comment replaces the text unless the user
    /// typed their own; a failure falls back like `fail`. Returns whether anything changed.
    pub fn poll(&mut self, fallback: &str) -> bool {
        let Job::Running { rx, step } = &mut self.job else { return false };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(Msg::Step(s)) => {
                    *step = Some(s);
                    changed = true;
                }
                Ok(Msg::Done(Ok(text))) => {
                    if !self.edited {
                        self.text = text;
                    }
                    self.job = Job::Idle;
                    return true;
                }
                Ok(Msg::Done(Err(e))) => {
                    self.settle_failed(e, fallback);
                    return true;
                }
                Err(TryRecvError::Empty) => return changed,
                Err(TryRecvError::Disconnected) => {
                    self.settle_failed("writing the comment stopped unexpectedly".into(), fallback);
                    return true;
                }
            }
        }
    }

    fn settle_failed(&mut self, e: String, fallback: &str) {
        self.job = Job::Failed(e);
        if !self.edited {
            self.text = fallback.to_owned();
        }
    }

    /// The user changed the text.
    pub fn edit(&mut self) {
        self.edited = true;
    }

    /// ⟳: write the comment again, over the user's text.
    pub fn regenerate(&mut self) {
        self.edited = false;
        self.for_key = None;
        self.job = Job::Idle;
    }

    pub fn running(&self) -> bool {
        matches!(self.job, Job::Running { .. })
    }

    /// The step the running job is at, if it said.
    pub fn step(&self) -> Option<Step> {
        match &self.job {
            Job::Running { step, .. } => *step,
            _ => None,
        }
    }

    /// Accept waits for a comment being written, unless the user already wrote their own.
    pub fn blocks_accept(&self) -> bool {
        self.running() && !self.edited
    }

    /// Accept waits for this row (when it would post it): its comment is being written, or still
    /// waits its turn (`wanting`), which would otherwise post an empty comment.
    pub fn holds_accept(&self, wanting: bool) -> bool {
        self.blocks_accept() || wanting
    }

    /// What Accept posts.
    pub fn comment(&self) -> String {
        self.text.trim().to_owned()
    }
}
