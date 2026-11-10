//! The "New ticket" window: lists the day's work by row, filtered by project; the user ticks the
//! lines a ticket is about, lets the LLM propose tickets (or writes them), edits and creates them
//! in Jira. Each created ticket goes back to the main window for the rows it was drafted from.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use chrono::NaiveDate;

use eframe::egui;

use crate::config::{Config, JiraCredentials, TicketConfig};
use crate::jira::{Issue, Jira, NewIssue};
use crate::drafting::{self, Stage, WorkItem};
use crate::history::{self, History};
use crate::llm::{self, Llm, MAX_DRAFTS, MAX_TITLE_CHARS, TicketDraft};

/// What the drafting thread reports: each step as it starts, then the result.
enum Progress {
    Stage(Stage),
    /// The drafts, and the items they were drafted from (with the diffs found).
    Done(anyhow::Result<Vec<TicketDraft>>, Vec<WorkItem>),
}

/// Where the code of the ticked lines is: the report's day and its repos by name.
pub struct CodeSource<'a> {
    pub date: NaiveDate,
    pub repo_paths: &'a BTreeMap<String, Vec<PathBuf>>,
}

/// Diff size limits per ticked line and for all of them, in chars (~3.5 per token): a usual commit
/// fits whole, and the total stays near 60k tokens, well inside the 128k+ context of current
/// models, so a big day only costs request time. Only the analyst step gets the diffs.
const CHANGE_CHARS: usize = 20_000;
const CHANGES_TOTAL_CHARS: usize = 200_000;

/// A finished drafting: the drafts and the items they were drafted from.
type Drafted = (anyhow::Result<Vec<TicketDraft>>, Vec<WorkItem>);

/// The pending drafting: the row id of each item sent, the channel, the step running.
struct Generating {
    item_rows: Vec<u64>,
    rx: Receiver<Progress>,
    stage: Option<Stage>,
}

/// Size of commit messages and file names, larger than egui's small body text.
pub const EVIDENCE_TEXT_SIZE: f32 = 15.0;

/// One row of the main window as the New ticket window lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkGroup {
    /// The row's stable id.
    pub row: u64,
    pub title: String,
    /// `None` when the row's repo maps to no project; such a group shows under every project.
    pub project: Option<String>,
    pub repos: Vec<String>,
    pub lines: Vec<String>,
}

impl WorkGroup {
    pub fn visible_in(&self, project: &str) -> bool {
        self.project.as_deref().is_none_or(|p| p.eq_ignore_ascii_case(project))
    }
}

/// The ticked lines of the groups visible in `project`, in group then line order, as
/// (row id, "[repos] line"). Ticks of hidden groups and ticks past a group's lines are left out.
pub fn ticked_items(groups: &[WorkGroup], ticks: &BTreeSet<(u64, usize)>, project: &str) -> Vec<(u64, String)> {
    ticked_lines(groups, ticks, project)
        .into_iter()
        .map(|(row, repos, line)| {
            let text = if repos.is_empty() { line.to_owned() } else { format!("[{}] {line}", repos.join(", ")) };
            (row, text)
        })
        .collect()
}

/// `ticked_items` with the repos and the line apart, as (row id, repos, line).
pub fn ticked_lines<'a>(groups: &'a [WorkGroup], ticks: &BTreeSet<(u64, usize)>, project: &str) -> Vec<(u64, &'a [String], &'a str)> {
    groups
        .iter()
        .filter(|g| g.visible_in(project))
        .flat_map(|g| {
            g.lines.iter().enumerate().filter(|(j, _)| ticks.contains(&(g.row, *j))).map(|(_, line)| (g.row, g.repos.as_slice(), line.as_str()))
        })
        .collect()
}

/// Creates each job's ticket in turn and records every created one in `history` with its
/// `sources` (the same position in `sources` as the job in `jobs`) and the time `now` gives once
/// it is created. `on_result` hears each outcome as (draft index, key or error); returning false
/// stops the rest.
pub fn create_tickets(
    jira: &Jira,
    history: Option<&History>,
    jobs: Vec<(usize, NewIssue)>,
    sources: Vec<Vec<WorkItem>>,
    language: &str,
    now: impl Fn() -> i64,
    mut on_result: impl FnMut(usize, Result<String, String>) -> bool,
) {
    let mut sources = sources.into_iter();
    for (i, new) in jobs {
        let item_sources = sources.next().unwrap_or_default();
        let res = jira.create_issue(&new).map_err(|e| {
            log::error!("creating ticket in {}: {e:#}", new.project);
            let msg = format!("{e:#}");
            if msg.contains("imeout") || msg.contains("timed out") {
                format!("{msg} (Jira may still have created it; check before trying again)")
            } else {
                msg
            }
        });
        if let (Ok(key), Some(h)) = (&res, history) {
            if let Err(e) = h.record(&history::ticket_record(key, &new, language, item_sources, now())) {
                log::warn!("ticket history: recording {key}: {e:#}");
            } else {
                log::info!("ticket history: recorded {key}");
            }
        }
        if !on_result(i, res) {
            return;
        }
    }
}

/// Where to look up the change of each ticked line: (its repos' paths, the line) and the day, or
/// no paths and no day when no code is to be sent (`send_code` off or no report).
pub fn change_lookup(ticked: &[(u64, &[String], &str)], code: Option<&CodeSource>, send_code: bool) -> (Vec<(Vec<PathBuf>, String)>, Option<NaiveDate>) {
    let code = code.filter(|_| send_code);
    let sources = ticked
        .iter()
        .map(|(_, repos, line)| {
            let paths = code.map(|c| repos.iter().flat_map(|r| c.repo_paths.get(r).cloned().unwrap_or_default()).collect()).unwrap_or_default();
            (paths, (*line).to_owned())
        })
        .collect();
    (sources, code.map(|c| c.date))
}

fn distinct(rows: impl IntoIterator<Item = u64>) -> Vec<u64> {
    let mut out: Vec<u64> = Vec::new();
    for r in rows {
        if !out.contains(&r) {
            out.push(r);
        }
    }
    out
}

pub struct Draft {
    /// The project the draft was written for; Create sends it there whatever the selector shows now.
    pub project: String,
    pub title: String,
    pub description: String,
    pub selected: bool,
    /// Ok(key) once created; a created draft is never sent again.
    pub created: Option<Result<String, String>>,
    /// Ids of the rows whose work the draft describes; they pick the ticket once it is created.
    pub rows: Vec<u64>,
    /// What the draft was made from, kept with the created ticket for regenerating it later.
    pub sources: Vec<WorkItem>,
}

impl Draft {
    pub fn new(project: &str, title: &str, description: &str, rows: Vec<u64>) -> Self {
        Self { project: project.to_owned(), title: title.to_owned(), description: description.to_owned(), selected: true, created: None, rows, sources: Vec::new() }
    }

    fn is_created(&self) -> bool {
        matches!(self.created, Some(Ok(_)))
    }

    /// What Create sends: selected, titled, and not created already.
    fn is_sendable(&self) -> bool {
        self.selected && !self.is_created() && !self.title.trim().is_empty()
    }
}

pub struct Creator {
    pub project: String,
    /// Row the window was opened from; an empty draft made with nothing ticked is for it.
    pub origin: Option<u64>,
    /// Ticked lines as (row id, line index).
    pub ticks: BTreeSet<(u64, usize)>,
    pub drafts: Vec<Draft>,
    pub error: Option<String>,
    generating: Option<Generating>,
    creating: Option<Receiver<(usize, Result<String, String>)>>,
}

/// What the main window must do after a frame of the creator.
pub enum Outcome {
    /// A ticket was created: add it to the pickers and let the rows it was drafted from pick it.
    Created { issue: Issue, rows: Vec<u64> },
}

impl Creator {
    pub fn new(project: String, origin: Option<u64>) -> Self {
        Self { project, origin, ticks: BTreeSet::new(), drafts: Vec::new(), error: None, generating: None, creating: None }
    }

    pub fn busy(&self) -> bool {
        self.generating.is_some() || self.creating.is_some()
    }

    /// Tickets are being created; one arriving may repick a row.
    pub fn creating(&self) -> bool {
        self.creating.is_some()
    }

    /// Opened for one row: switch to its project and tick exactly its lines.
    pub fn focus_row(&mut self, group: &WorkGroup) {
        if let Some(p) = &group.project {
            self.project = p.clone();
        }
        self.ticks = (0..group.lines.len()).map(|j| (group.row, j)).collect();
        self.origin = Some(group.row);
    }

    /// A blank draft for the rows of the ticked lines, or the origin row when nothing is ticked
    /// and that row is still listed for the current project (a row of another project must not
    /// pick this project's ticket).
    pub fn add_empty_draft(&mut self, groups: &[WorkGroup]) {
        let mut rows = distinct(ticked_items(groups, &self.ticks, &self.project).into_iter().map(|(r, _)| r));
        if rows.is_empty() {
            rows.extend(self.origin.filter(|&o| groups.iter().any(|g| g.row == o && g.visible_in(&self.project))));
        }
        let mut d = Draft::new(&self.project, "", "", rows);
        let mut lines: Vec<String> = ticked_items(groups, &self.ticks, &self.project).into_iter().map(|(_, t)| t).collect();
        if lines.is_empty() {
            lines = groups
                .iter()
                .filter(|g| d.rows.contains(&g.row))
                .flat_map(|g| g.lines.iter().map(|l| if g.repos.is_empty() { l.clone() } else { format!("[{}] {l}", g.repos.join(", ")) }))
                .collect();
        }
        d.sources = lines.into_iter().map(|line| WorkItem { line, change: None }).collect();
        self.drafts.push(d);
    }

    /// Gives the drafts `apply_drafts` just added (those after the first `kept`) their sources:
    /// the items each names, or all of them when it names none.
    pub fn attach_sources(&mut self, kept: usize, item_sets: &[Vec<usize>], items: &[WorkItem]) {
        for (d, set) in self.drafts.iter_mut().skip(kept).zip(item_sets) {
            let named: Vec<WorkItem> = set.iter().filter_map(|&i| items.get(i).cloned()).collect();
            d.sources = if named.is_empty() { items.to_vec() } else { named };
        }
    }

    /// The sources of each job's draft, in job order (the record written once it is created).
    pub fn job_sources(&self, jobs: &[(usize, NewIssue)]) -> Vec<Vec<WorkItem>> {
        jobs.iter().map(|(i, _)| self.drafts.get(*i).map(|d| d.sources.clone()).unwrap_or_default()).collect()
    }

    /// Starts drafting from the ticked lines of `groups`; with `code` (and `send_code` on) each
    /// line goes with what it changed.
    fn generate(&mut self, cfg: &Config, groups: &[WorkGroup], code: Option<&CodeSource>) {
        self.error = None;
        let creds = match llm::credentials(&cfg.tickets, |k| std::env::var(k).ok()) {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                return;
            }
        };
        let ticked = ticked_lines(groups, &self.ticks, &self.project);
        let item_rows: Vec<u64> = ticked.iter().map(|(r, _, _)| *r).collect();
        let texts: Vec<String> = ticked_items(groups, &self.ticks, &self.project).into_iter().map(|(_, t)| t).collect();
        let (sources, date) = change_lookup(&ticked, code, cfg.tickets.send_code);
        log::info!("asking {} for ticket drafts in {} from {} items (with code: {})", creds.model, self.project, texts.len(), date.is_some());
        let (tx, rx) = channel();
        let (project, language) = (self.project.clone(), cfg.tickets.language.clone());
        std::thread::spawn(move || {
            let changes = match date {
                Some(d) => crate::git::change_contexts(&sources, d, CHANGE_CHARS, CHANGES_TOTAL_CHARS),
                None => vec![None; sources.len()],
            };
            log::info!("code changes found for {} of {} items", changes.iter().filter(|c| c.is_some()).count(), changes.len());
            let items: Vec<WorkItem> = texts.into_iter().zip(changes).map(|(line, change)| WorkItem { line, change }).collect();
            let progress_tx = tx.clone();
            let progress = move |s: Stage| {
                log::info!("drafting step {}/4: {}", s.number(), s.label());
                let _ = progress_tx.send(Progress::Stage(s));
            };
            let res = drafting::draft_tickets(&Llm::new(creds), &project, &language, &items, &progress);
            if let Err(e) = &res {
                log::error!("drafting tickets: {e:#}");
            }
            let _ = tx.send(Progress::Done(res, items));
        });
        self.generating = Some(Generating { item_rows, rx, stage: None });
    }

    /// The drafts Create sends, by draft index: selected, titled, and not created already.
    pub fn create_jobs(&self, cfg: &TicketConfig, me: &str) -> Vec<(usize, NewIssue)> {
        self.drafts
            .iter()
            .enumerate()
            .filter(|(_, d)| d.is_sendable())
            .map(|(i, d)| {
                let new = NewIssue {
                    project: d.project.clone(),
                    issue_type: cfg.issue_type.clone(),
                    summary: d.title.trim().to_owned(),
                    description: d.description.trim().to_owned(),
                    assignee: Some(me.to_owned()),
                    extra_fields: cfg.fields.clone(),
                };
                (i, new)
            })
            .collect()
    }

    /// Takes in an LLM answer: created drafts stay, the others are replaced by the new drafts
    /// (never more than `MAX_DRAFTS` in all) and written for the current project; an error is
    /// kept for display. `item_rows[i]` is the
    /// row of item `i` sent to the model: a draft is for the rows of the items it names, and a
    /// lone draft naming none (or only unknown items) is for every row sent.
    pub fn apply_drafts(&mut self, res: anyhow::Result<Vec<TicketDraft>>, item_rows: &[u64]) {
        match res {
            Ok(drafts) => {
                self.drafts.retain(Draft::is_created);
                let room = MAX_DRAFTS.saturating_sub(self.drafts.len());
                let lone = drafts.len() == 1;
                for d in drafts.into_iter().take(room) {
                    let mut rows = distinct(d.items.iter().filter_map(|&i| item_rows.get(i).copied()));
                    if rows.is_empty() && lone {
                        rows = distinct(item_rows.iter().copied());
                    }
                    self.drafts.push(Draft::new(&self.project, &d.title, &d.description, rows));
                }
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    /// Records the result of creating draft `i`; a success yields the ticket and the draft's rows.
    pub fn apply_create_result(&mut self, i: usize, res: Result<String, String>) -> Option<Outcome> {
        let d = self.drafts.get_mut(i)?;
        let out = res.as_ref().ok().map(|key| Outcome::Created {
            issue: Issue { key: key.clone(), summary: d.title.trim().to_owned(), status: "new".into() },
            rows: d.rows.clone(),
        });
        d.created = Some(res);
        out
    }

    /// Creates the sendable drafts; each created one is recorded in `history` with its sources.
    fn create(&mut self, cfg: &Config, creds: &JiraCredentials, me: &str, history: Option<Arc<History>>) {
        let jobs = self.create_jobs(&cfg.tickets, me);
        if jobs.is_empty() {
            return;
        }
        let sources = self.job_sources(&jobs);
        let language = cfg.tickets.language.clone();
        let (tx, rx) = channel();
        let creds = creds.clone();
        std::thread::spawn(move || {
            let jira = Jira::new(&creds.url, &creds.token);
            create_tickets(&jira, history.as_deref(), jobs, sources, &language, || chrono::Local::now().timestamp(), |i, res| tx.send((i, res)).is_ok());
        });
        self.creating = Some(rx);
    }

    fn poll(&mut self) -> Vec<Outcome> {
        let mut out = Vec::new();
        if let Some(g) = &mut self.generating {
            // None: still running; Some(None): the thread died without an answer.
            let mut done: Option<Option<Drafted>> = None;
            loop {
                match g.rx.try_recv() {
                    Ok(Progress::Stage(s)) => g.stage = Some(s),
                    Ok(Progress::Done(res, items)) => {
                        done = Some(Some((res, items)));
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        done = Some(None);
                        break;
                    }
                }
            }
            match done {
                Some(Some((res, items))) => {
                    let item_rows = std::mem::take(&mut g.item_rows);
                    self.generating = None;
                    let item_sets: Vec<Vec<usize>> = res.as_ref().map(|ds| ds.iter().map(|d| d.items.clone()).collect()).unwrap_or_default();
                    let kept = self.drafts.iter().filter(|d| d.is_created()).count();
                    self.apply_drafts(res, &item_rows);
                    self.attach_sources(kept, &item_sets, &items);
                }
                Some(None) => {
                    self.error = Some("the LLM request stopped unexpectedly".into());
                    self.generating = None;
                }
                None => {}
            }
        }
        let mut results = Vec::new();
        if let Some(rx) = &self.creating {
            loop {
                match rx.try_recv() {
                    Ok(r) => results.push(r),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.creating = None;
                        break;
                    }
                }
            }
        }
        for (i, res) in results {
            out.extend(self.apply_create_result(i, res));
        }
        out
    }

    /// Draws the window over the main window's `groups`. Returns false once the user closed it.
    /// `can_create` is false while the main window posts worklogs: a ticket arriving then would
    /// repick a row whose worklog is already on its way to another ticket.
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        cfg: &Config,
        creds: &JiraCredentials,
        me: &str,
        groups: &[WorkGroup],
        code: Option<&CodeSource>,
        history: Option<&Arc<History>>,
        can_create: bool,
        projects: &[(String, String)],
    ) -> (bool, Vec<Outcome>) {
        let outcomes = self.poll();
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let mut open = true;
        egui::Window::new("New Jira ticket")
            .id(egui::Id::new("new-ticket"))
            .open(&mut open)
            .default_size([720.0, 600.0])
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().id_salt("new-ticket-body").show(ui, |ui| {
                    self.body(ui, cfg, creds, me, groups, code, history, can_create, projects);
                });
            });
        // Closing mid-request would drop results the user still expects; keep it open until done.
        (open || self.busy(), outcomes)
    }

    #[allow(clippy::too_many_arguments)]
    fn body(
        &mut self,
        ui: &mut egui::Ui,
        cfg: &Config,
        creds: &JiraCredentials,
        me: &str,
        groups: &[WorkGroup],
        code: Option<&CodeSource>,
        history: Option<&Arc<History>>,
        can_create: bool,
        projects: &[(String, String)],
    ) {
        let editable = !self.busy();
        ui.horizontal(|ui| {
            ui.strong("1. Project:");
            ui.add_enabled_ui(editable, |ui| {
                for (k, l) in projects {
                    ui.selectable_value(&mut self.project, k.clone(), egui::RichText::new(l).size(EVIDENCE_TEXT_SIZE));
                }
            });
        });
        ui.add_space(4.0);

        let items = ticked_items(groups, &self.ticks, &self.project);
        ui.strong(format!("2. Tick the commits and files the ticket is about ({} ticked):", items.len()));
        let shown: Vec<&WorkGroup> = groups.iter().filter(|g| g.visible_in(&self.project) && !g.lines.is_empty()).collect();
        let hidden = groups.iter().filter(|g| !g.visible_in(&self.project) && !g.lines.is_empty()).count();
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("work").max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
                if shown.is_empty() {
                    ui.weak("no work found for this project today");
                }
                for g in shown {
                    self.group_ui(ui, g, editable);
                }
            });
        });
        if hidden > 0 {
            ui.weak(format!("{hidden} rows of other projects hidden; switch the project to see them"));
        }
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            let can_generate = !items.is_empty() && editable && self.drafts.iter().filter(|d| d.is_created()).count() < MAX_DRAFTS;
            if ui
                .add_enabled(can_generate, egui::Button::new(egui::RichText::new("3. ✨ Propose tickets").strong()))
                .on_hover_text(format!(
                    "{} drafts up to {MAX_DRAFTS} tickets in {} from the ticked lines{}, in four steps: an analyst works \
                     out what the work was for, a writer drafts the tickets, a tester adds test scenarios, a reviewer \
                     removes what is vague or unfounded",
                    cfg.tickets.llm_model,
                    cfg.tickets.language,
                    if cfg.tickets.send_code { " and their code changes" } else { "" }
                ))
                .on_disabled_hover_text("tick some commits or files first")
                .clicked()
            {
                self.generate(cfg, groups, code);
            }
            if ui.add_enabled(editable && self.drafts.len() < MAX_DRAFTS, egui::Button::new("+ Write one myself")).clicked() {
                self.add_empty_draft(groups);
            }
            if let Some(g) = &self.generating {
                ui.spinner();
                ui.label(match g.stage {
                    Some(s) => format!("{}/4 {}…", s.number(), s.label()),
                    None => "collecting the code changes…".to_owned(),
                });
            }
        });
        if let Some(e) = &self.error {
            ui.colored_label(egui::Color32::from_rgb(200, 60, 50), e);
        }
        if self.drafts.is_empty() {
            return;
        }
        ui.separator();
        ui.strong("4. Check and edit the proposals, then create them:");

        let title_of = |row: u64| groups.iter().find(|g| g.row == row).map_or_else(|| format!("row {row}"), |g| g.title.clone());
        for (i, d) in self.drafts.iter_mut().enumerate() {
            ui.push_id(("draft", i), |ui| {
                let done = d.is_created();
                ui.horizontal(|ui| {
                    ui.add_enabled(editable && !done, egui::Checkbox::without_text(&mut d.selected));
                    ui.add_enabled(
                        editable && !done,
                        egui::TextEdit::singleline(&mut d.title).hint_text("title").char_limit(MAX_TITLE_CHARS).desired_width(520.0),
                    );
                    match &d.created {
                        Some(Ok(key)) => {
                            ui.hyperlink_to(key, format!("{}/browse/{key}", creds.url));
                        }
                        Some(Err(e)) => {
                            ui.colored_label(egui::Color32::from_rgb(200, 60, 50), "failed").on_hover_text(e);
                        }
                        None => {}
                    }
                });
                ui.add_enabled(
                    editable && !done,
                    egui::TextEdit::multiline(&mut d.description).hint_text("description").desired_rows(4).desired_width(f32::INFINITY),
                );
                let rows: Vec<String> = d.rows.iter().map(|&r| title_of(r)).collect();
                let from = if rows.is_empty() { String::new() } else { format!(" · drafted from: {}", rows.join("; ")) };
                let note = if done {
                    format!("created in {}{from} · rows without a ticket picked it; it is also in every \"pick ticket…\" list", d.project)
                } else {
                    format!("for {}{from}", d.project)
                };
                ui.weak(note);
                ui.add_space(6.0);
            });
        }

        let todo = self.drafts.iter().filter(|d| d.is_sendable()).count();
        ui.horizontal(|ui| {
            let mut targets: Vec<&str> = Vec::new();
            for d in self.drafts.iter().filter(|d| d.is_sendable()) {
                if !targets.contains(&d.project.as_str()) {
                    targets.push(&d.project);
                }
            }
            let place = if targets.is_empty() { "Jira".to_owned() } else { targets.join(" + ") };
            let label = format!("Create {todo} in {place} ({} as {})", cfg.tickets.issue_type, me);
            let button = ui
                .add_enabled(todo > 0 && editable && can_create, egui::Button::new(egui::RichText::new(label).strong()))
                .on_disabled_hover_text(if can_create { "tick a titled draft" } else { "wait until the worklogs are posted" });
            if button.clicked() {
                self.create(cfg, creds, me, history.cloned());
            }
            if self.creating.is_some() {
                ui.spinner();
            }
        });
    }

    fn group_ui(&mut self, ui: &mut egui::Ui, g: &WorkGroup, editable: bool) {
        ui.push_id(("group", g.row), |ui| {
            let ticked = (0..g.lines.len()).filter(|&j| self.ticks.contains(&(g.row, j))).count();
            let mut all = ticked == g.lines.len();
            let partly = ticked > 0 && !all;
            ui.horizontal(|ui| {
                let r = ui
                    .add_enabled(editable, egui::Checkbox::new(&mut all, egui::RichText::new(&g.title).strong().size(EVIDENCE_TEXT_SIZE)).indeterminate(partly))
                    .on_hover_text("tick or untick every line of this row");
                if r.changed() {
                    for j in 0..g.lines.len() {
                        if all {
                            self.ticks.insert((g.row, j));
                        } else {
                            self.ticks.remove(&(g.row, j));
                        }
                    }
                }
            });
            ui.indent("lines", |ui| {
                for (j, line) in g.lines.iter().enumerate() {
                    let mut on = self.ticks.contains(&(g.row, j));
                    if ui.add_enabled(editable, egui::Checkbox::new(&mut on, egui::RichText::new(line).size(EVIDENCE_TEXT_SIZE))).changed() {
                        if on {
                            self.ticks.insert((g.row, j));
                        } else {
                            self.ticks.remove(&(g.row, j));
                        }
                    }
                }
            });
            ui.add_space(4.0);
        });
    }
}
