//! The "New ticket" window: lists the day's work by row, filtered by project; the user ticks the
//! lines a ticket is about, lets the LLM propose tickets (or writes them), edits and creates them
//! in Jira. Each created ticket goes back to the main window for the rows it was drafted from.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, NaiveTime};

use eframe::egui;

use crate::config::{Config, JiraCredentials, TicketConfig};
use crate::jira::{Issue, Jira, NewIssue};
use crate::drafting::{self, Stage, WorkItem};
use crate::history::{self, History};
use crate::llm::{self, Llm, MAX_DRAFTS, MAX_TITLE_CHARS, TicketDraft};
use crate::git::Span;
use crate::ticket_range::{self, MainRow, Preset, Scanned};
use crate::ticket_kind::{self, FieldKind, FieldValue, IssueType, Kind, ProjectMeta, RequiredField};

/// What the drafting thread reports: each step as it starts, then the result.
enum Progress {
    Stage(Stage),
    /// The drafts, and the items they were drafted from (with the diffs found).
    Done(anyhow::Result<Vec<TicketDraft>>, Vec<WorkItem>),
}

/// Where the code of the ticked lines is: the report's day and its repos by name.
pub struct CodeSource<'a> {
    pub span: Span,
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
pub fn change_lookup(ticked: &[(u64, &[String], &str)], code: Option<&CodeSource>, send_code: bool) -> (Vec<(Vec<PathBuf>, String)>, Option<Span>) {
    let code = code.filter(|_| send_code);
    let sources = ticked
        .iter()
        .map(|(_, repos, line)| {
            let paths = code.map(|c| repos.iter().flat_map(|r| c.repo_paths.get(r).cloned().unwrap_or_default()).collect()).unwrap_or_default();
            (paths, (*line).to_owned())
        })
        .collect();
    (sources, code.map(|c| c.span))
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
    /// Issue type, epic, parent and required field values.
    pub kind: Kind,
}

impl Draft {
    pub fn new(project: &str, title: &str, description: &str, rows: Vec<u64>) -> Self {
        Self {
            project: project.to_owned(),
            title: title.to_owned(),
            description: description.to_owned(),
            selected: true,
            created: None,
            rows,
            sources: Vec::new(),
            kind: Kind::default(),
        }
    }

    fn is_created(&self) -> bool {
        matches!(self.created, Some(Ok(_)))
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
    /// Where the listed work comes from: the main window's rows, or a span read from git here.
    pub range: RangeChoice,
    /// What each project offers for a draft's kind, read from Jira once per window.
    pub meta: BTreeMap<String, MetaState>,
    /// The kind last chosen per project; new drafts of that project start with it.
    pub defaults: BTreeMap<String, Kind>,
    /// The search text of each draft's epic list, by draft index.
    epic_filters: BTreeMap<usize, String>,
}

/// A project's create metadata: being read, read, or why it could not be.
pub enum MetaState {
    Loading(Receiver<Result<ProjectMeta, String>>),
    Ready(Arc<ProjectMeta>),
    Failed(String),
}

/// The "Work from" choice and the git read it started.
pub struct RangeChoice {
    pub preset: Preset,
    /// The Custom start, as the date picker and the hour and minute fields hold it.
    pub custom_date: jiff::civil::Date,
    pub custom_hour: u32,
    pub custom_minute: u32,
    /// The start being read, and where its result arrives.
    scanning: Option<(NaiveDateTime, Receiver<Scanned>)>,
    /// Shared so a frame can list it while ticking lines of the window.
    pub scanned: Option<Arc<Scanned>>,
}

impl RangeChoice {
    /// The report's day, with Custom starting at 00:00 of `today`.
    pub fn new(today: NaiveDate) -> Self {
        Self { preset: Preset::ReportDay, custom_date: to_jiff(today), custom_hour: 0, custom_minute: 0, scanning: None, scanned: None }
    }

    pub fn custom_start(&self) -> NaiveDateTime {
        let date = from_jiff(self.custom_date).unwrap_or(NaiveDate::MIN);
        date.and_time(NaiveTime::from_hms_opt(self.custom_hour.min(23), self.custom_minute.min(59), 0).unwrap_or(NaiveTime::MIN))
    }

    /// The listed work comes from a span read here, not from the main window's rows.
    pub fn is_span(&self) -> bool {
        self.preset != Preset::ReportDay
    }

    pub fn scanning(&self) -> bool {
        self.scanning.is_some()
    }

    /// The start being read, or the one read last.
    pub fn shown_start(&self) -> Option<NaiveDateTime> {
        self.scanning.as_ref().map(|(s, _)| *s).or_else(|| self.scanned.as_ref().map(|s| s.start))
    }

    /// The Work from controls take input: the window is not busy and no read is running.
    pub fn controls_enabled(&self, busy: bool) -> bool {
        !busy && !self.scanning()
    }

    /// ⟳ reads a preset's span again up to now; Custom has its own button.
    pub fn can_refresh(&self) -> bool {
        self.is_span() && self.preset != Preset::Custom && self.scanned.is_some() && !self.scanning()
    }

    /// Back to the main window's rows; a read still running is dropped.
    pub fn reset(&mut self) {
        self.preset = Preset::ReportDay;
        self.scanning = None;
        self.scanned = None;
    }
}

/// The lines the window lists and where their code is: the main window's rows and `code` for the
/// report's day, the read span's groups once it has arrived, nothing while it has not.
pub fn listed<'a>(range: &RangeChoice, scanned: Option<&'a Scanned>, main: &'a [WorkGroup], code: Option<&CodeSource<'a>>) -> (&'a [WorkGroup], Option<CodeSource<'a>>) {
    match (range.is_span(), scanned) {
        (false, _) => (main, code.map(|c| CodeSource { span: c.span, repo_paths: c.repo_paths })),
        (true, Some(s)) => (&s.groups, Some(CodeSource { span: s.span, repo_paths: &s.repo_paths })),
        (true, None) => (&[], None),
    }
}

/// The title a draft names its row by: from the listed groups, else the main window's (a draft
/// written before the range changed), else the bare id.
pub fn row_title(row: u64, listed: &[WorkGroup], main: &[WorkGroup]) -> String {
    listed.iter().chain(main).find(|g| g.row == row).map_or_else(|| format!("row {row}"), |g| g.title.clone())
}

/// What the work list says in place of (or above) its lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyHint {
    /// A git read is running; shown with a spinner.
    Reading,
    /// A range is chosen but not read yet (Custom before its button).
    PickStart,
    /// The range was read and holds nothing for this project.
    NothingInRange,
    /// The report's day holds nothing for this project.
    NothingToday,
}

impl EmptyHint {
    pub fn text(self) -> &'static str {
        match self {
            EmptyHint::Reading => "reading your work from git…",
            EmptyHint::PickStart => "pick a start above to list your work since then",
            EmptyHint::NothingInRange => "no work of yours found for this project in this range",
            EmptyHint::NothingToday => "no work found for this project today",
        }
    }
}

/// The hint for the work list: a running read always says so; otherwise only an empty list
/// (`shown_empty`) gets one, by where its lines would come from.
pub fn empty_hint(range: &RangeChoice, shown_empty: bool) -> Option<EmptyHint> {
    if range.scanning() {
        return Some(EmptyHint::Reading);
    }
    if !shown_empty {
        return None;
    }
    Some(match (range.is_span(), range.scanned.is_some()) {
        (false, _) => EmptyHint::NothingToday,
        (true, false) => EmptyHint::PickStart,
        (true, true) => EmptyHint::NothingInRange,
    })
}

/// The line under the Work from choice: the report's day, the read running, or what the read
/// found ("since … · 5 commits, 7 lines in 3 groups · 2 repos read"); `None` before anything.
pub fn status_line(range: &RangeChoice, code: Option<&CodeSource>, now: NaiveDateTime) -> Option<String> {
    if !range.is_span() {
        return code.map(|c| format!("the main window's rows: {}", c.span.from.format("%a %Y-%m-%d")));
    }
    if let Some((start, _)) = &range.scanning {
        return Some(format!("reading git {}…", ticket_range::describe_start(*start, now)));
    }
    range.scanned.as_ref().map(|s| {
        let lines: usize = s.groups.iter().map(|g| g.lines.len()).sum();
        format!("{} · {} commits, {lines} lines in {} groups · {} repos read", ticket_range::describe_start(s.start, now), s.commits, s.groups.len(), s.repos)
    })
}

/// What a draft's kind controls offer.
pub struct KindControls<'a> {
    pub types: &'a [IssueType],
    pub default_type: &'a str,
    /// The draft's type is a sub-task as Jira (or the fallback offer) flags it.
    pub subtask: bool,
    pub meta: Option<&'a ProjectMeta>,
    pub required: &'a [RequiredField],
    pub created_parents: &'a [Issue],
}

/// The type, epic or parent, and required field controls of one draft. Returns (kind changed,
/// "same for all" clicked); `siblings` says whether other drafts could take it.
fn kind_ui(ui: &mut egui::Ui, kind: &mut Kind, c: &KindControls, epic_filter: &mut String, siblings: bool) -> (bool, bool) {
    let before = kind.clone();
    let mut all = false;
    ui.horizontal_wrapped(|ui| {
        ui.label("Type");
        egui::ComboBox::from_id_salt("type").selected_text(kind.type_name(c.default_type)).show_ui(ui, |ui| {
            for t in c.types {
                let on = kind.type_name(c.default_type).eq_ignore_ascii_case(&t.name);
                if ui.selectable_label(on, &t.name).clicked() {
                    kind.issue_type = t.name.clone();
                    kind.subtask = t.subtask;
                }
            }
        });
        // A type picked this frame is flagged by the offer; otherwise as the window resolved it.
        let subtask = if kind.issue_type != before.issue_type { kind.subtask } else { c.subtask };
        let epic_required = c.required.iter().any(|f| f.kind == FieldKind::Epic);
        if subtask {
            parent_ui(ui, kind, c);
        } else if let Some(meta) = c.meta.filter(|m| m.epic_field.is_some() || epic_required) {
            epic_ui(ui, kind, &meta.epics, epic_filter, epic_required);
        }
        for f in c.required.iter().filter(|f| f.kind != FieldKind::Epic) {
            field_ui(ui, kind, f);
        }
        if siblings
            && ui.small_button("⇊ same for all").on_hover_text("give the other drafts of this project this type, epic, parent and fields").clicked()
        {
            all = true;
        }
    });
    (*kind != before, all)
}

fn epic_ui(ui: &mut egui::Ui, kind: &mut Kind, epics: &[Issue], filter: &mut String, required: bool) {
    ui.label(if required { "Epic *" } else { "Epic" });
    let shown = match &kind.epic {
        Some(k) => epics.iter().find(|e| &e.key == k).map_or_else(|| k.clone(), |e| format!("{} {}", e.key, truncate_chars(&e.summary, 40))),
        None => "none".to_owned(),
    };
    egui::ComboBox::from_id_salt("epic").selected_text(shown).width(260.0).show_ui(ui, |ui| {
        ui.add(egui::TextEdit::singleline(filter).hint_text("search epics").desired_width(240.0));
        if ui.selectable_label(kind.epic.is_none(), "— none —").clicked() {
            kind.epic = None;
        }
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            for e in ticket_kind::filter_issues(epics, filter) {
                if ui.selectable_label(kind.epic.as_deref() == Some(&e.key), format!("{} {}", e.key, e.summary)).clicked() {
                    kind.epic = Some(e.key.clone());
                }
            }
        });
        if epics.is_empty() {
            ui.weak("no open epics in this project");
        }
    });
}

fn parent_ui(ui: &mut egui::Ui, kind: &mut Kind, c: &KindControls) {
    ui.label("Parent");
    ui.add(egui::TextEdit::singleline(&mut kind.parent).hint_text("KEY-123").desired_width(110.0));
    let choices = ticket_kind::parent_choices(c.created_parents, c.meta.map(|m| m.parents.as_slice()).unwrap_or_default());
    egui::ComboBox::from_id_salt("parent").selected_text("▾").width(30.0).show_ui(ui, |ui| {
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            for p in &choices {
                if ui.selectable_label(kind.parent.trim() == p.key, format!("{} {}", p.key, truncate_chars(&p.summary, 70))).clicked() {
                    kind.parent = p.key.clone();
                }
            }
            if choices.is_empty() {
                ui.weak("no open issues found; type the key");
            }
        });
    });
}

fn field_ui(ui: &mut egui::Ui, kind: &mut Kind, f: &RequiredField) {
    ui.label(format!("{} *", f.name));
    match &f.kind {
        FieldKind::Select(allowed) => {
            let current = match kind.values.get(&f.id) {
                Some(FieldValue::One(v)) => v.clone(),
                _ => "pick…".to_owned(),
            };
            egui::ComboBox::from_id_salt(("field", &f.id)).selected_text(current).show_ui(ui, |ui| {
                for v in allowed {
                    let on = matches!(kind.values.get(&f.id), Some(FieldValue::One(x)) if x == v);
                    if ui.selectable_label(on, v).clicked() {
                        kind.values.insert(f.id.clone(), FieldValue::One(v.clone()));
                    }
                }
            });
        }
        FieldKind::MultiSelect(allowed) => {
            let mut set = match kind.values.get(&f.id) {
                Some(FieldValue::Many(vs)) => vs.clone(),
                _ => BTreeSet::new(),
            };
            let shown = if set.is_empty() { "pick…".to_owned() } else { set.iter().cloned().collect::<Vec<_>>().join(", ") };
            egui::ComboBox::from_id_salt(("field", &f.id))
                .selected_text(truncate_chars(&shown, 40))
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show_ui(ui, |ui| {
                    for v in allowed {
                        let mut on = set.contains(v);
                        if ui.checkbox(&mut on, v).changed() {
                            if on {
                                set.insert(v.clone());
                            } else {
                                set.remove(v);
                            }
                        }
                    }
                });
            if set.is_empty() {
                kind.values.remove(&f.id);
            } else {
                kind.values.insert(f.id.clone(), FieldValue::Many(set));
            }
        }
        FieldKind::Text => {
            let mut text = match kind.values.get(&f.id) {
                Some(FieldValue::Text(t)) => t.clone(),
                _ => String::new(),
            };
            if ui.add(egui::TextEdit::singleline(&mut text).desired_width(160.0)).changed() {
                kind.values.insert(f.id.clone(), FieldValue::Text(text));
            }
        }
        FieldKind::Unsupported(t) => {
            ui.weak(format!("({t}: set it in WORKLOG_TICKET_FIELDS)"));
        }
        FieldKind::Epic => {}
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_owned() } else { format!("{}…", s.chars().take(max).collect::<String>()) }
}

pub fn to_jiff(d: NaiveDate) -> jiff::civil::Date {
    let (y, m, day) = (i16::try_from(d.year()).unwrap_or(2000), d.month() as i8, d.day() as i8);
    jiff::civil::Date::new(y, m, day).unwrap_or(jiff::civil::date(2000, 1, 1))
}

pub fn from_jiff(d: jiff::civil::Date) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(i32::from(d.year()), u32::try_from(d.month()).ok()?, u32::try_from(d.day()).ok()?)
}

/// What the main window must do after a frame of the creator.
pub enum Outcome {
    /// A ticket was created: add it to the pickers and let the rows it was drafted from pick it.
    Created { issue: Issue, rows: Vec<u64> },
}

impl Creator {
    pub fn new(project: String, origin: Option<u64>) -> Self {
        Self {
            project,
            origin,
            ticks: BTreeSet::new(),
            drafts: Vec::new(),
            error: None,
            generating: None,
            creating: None,
            range: RangeChoice::new(Local::now().date_naive()),
            meta: BTreeMap::new(),
            defaults: BTreeMap::new(),
            epic_filters: BTreeMap::new(),
        }
    }

    /// What `project` offers, once read.
    pub fn meta_of(&self, project: &str) -> Option<Arc<ProjectMeta>> {
        match self.meta.get(project) {
            Some(MetaState::Ready(m)) => Some(m.clone()),
            _ => None,
        }
    }

    pub fn set_meta(&mut self, project: &str, meta: ProjectMeta) {
        self.meta.insert(project.to_owned(), MetaState::Ready(Arc::new(meta)));
    }

    /// Starts reading `project`'s create metadata unless it is read, being read, or failed (a
    /// failure is read again only on ⟳, which drops it).
    pub fn ensure_meta(&mut self, cfg: &TicketConfig, creds: &JiraCredentials, project: &str) {
        if self.meta.contains_key(project) {
            return;
        }
        let (tx, rx) = channel();
        let (creds, project_key) = (creds.clone(), project.to_owned());
        let (wanted, default, provided) = (cfg.issue_types.clone(), cfg.issue_type.clone(), cfg.fields.clone());
        std::thread::spawn(move || {
            let jira = Jira::new(&creds.url, &creds.token);
            let res = ticket_kind::load(&jira, &project_key, &wanted, &default, &provided).map_err(|e| format!("{e:#}"));
            match &res {
                Ok(m) => log::info!(
                    "{project_key}: {} issue types, {} epics, {} parent candidates, epic field {:?}, {} warnings",
                    m.types.len(),
                    m.epics.len(),
                    m.parents.len(),
                    m.epic_field,
                    m.warnings.len()
                ),
                Err(e) => log::warn!("{project_key}: reading issue types: {e}"),
            }
            let _ = tx.send(res);
        });
        self.meta.insert(project.to_owned(), MetaState::Loading(rx));
    }

    /// The fields draft `i`'s type requires, as far as its project's metadata is read.
    pub fn required_for(&self, i: usize, default_type: &str) -> Vec<RequiredField> {
        let Some(d) = self.drafts.get(i) else { return Vec::new() };
        let kind = self.effective_kind(i, default_type);
        self.meta_of(&d.project).and_then(|m| m.required.get(kind.type_name(default_type)).cloned()).unwrap_or_default()
    }

    /// Draft `i`'s kind with its type (the chosen one or `default_type`) spelled and flagged as
    /// its project's Jira type of that name, any case, once the project is read; as it is before.
    pub fn effective_kind(&self, i: usize, default_type: &str) -> Kind {
        let Some(d) = self.drafts.get(i) else { return Kind::default() };
        let mut kind = d.kind.clone();
        let name = kind.type_name(default_type).trim().to_owned();
        match self.meta_of(&d.project) {
            Some(m) => {
                if let Some(t) = m.types.iter().find(|t| t.name.eq_ignore_ascii_case(&name)) {
                    kind.issue_type = t.name.clone();
                    kind.subtask = t.subtask;
                }
            }
            // Unread (or unreadable) project: the default is flagged as the fallback offer flags it.
            None if d.kind.issue_type.trim().is_empty() => kind.subtask = name.to_lowercase().contains("sub"),
            None => {}
        }
        kind
    }

    /// What keeps draft `i` from being created, as the window says it; empty when nothing does.
    pub fn draft_blockers(&self, i: usize, default_type: &str) -> Vec<String> {
        let Some(d) = self.drafts.get(i) else { return Vec::new() };
        let mut out = Vec::new();
        if d.title.trim().is_empty() {
            out.push("a title".to_owned());
        }
        out.extend(ticket_kind::blockers(&self.effective_kind(i, default_type), &d.project, &self.required_for(i, default_type)));
        out
    }

    /// What Create sends: selected, not created already, and nothing missing.
    pub fn sendable(&self, i: usize, default_type: &str) -> bool {
        self.drafts.get(i).is_some_and(|d| d.selected && !d.is_created()) && self.draft_blockers(i, default_type).is_empty()
    }

    /// Gives draft `i` `kind`, which new drafts of its project then start with.
    pub fn set_kind(&mut self, i: usize, kind: Kind) {
        let Some(d) = self.drafts.get_mut(i) else { return };
        self.defaults.insert(d.project.clone(), kind.clone());
        d.kind = kind;
    }

    /// Gives draft `i`'s kind to every other draft of its project not created yet.
    pub fn same_for_all(&mut self, i: usize) {
        let Some(src) = self.drafts.get(i) else { return };
        let (project, kind) = (src.project.clone(), src.kind.clone());
        for (j, d) in self.drafts.iter_mut().enumerate() {
            if j != i && d.project == project && !d.is_created() {
                d.kind = kind.clone();
            }
        }
    }

    /// The kind a new draft of `project` starts with.
    fn default_kind(&self, project: &str) -> Kind {
        self.defaults.get(project).cloned().unwrap_or_default()
    }

    /// "Create 3 (2 Task, 1 Bug) in HD as jdoe" for the drafts Create would send.
    pub fn create_label(&self, cfg: &TicketConfig, me: &str) -> String {
        let sendable_ids: Vec<usize> = (0..self.drafts.len()).filter(|&i| self.sendable(i, &cfg.issue_type)).collect();
        let kinds: Vec<Kind> = sendable_ids.iter().map(|&i| self.effective_kind(i, &cfg.issue_type)).collect();
        let sendable: Vec<&Draft> = sendable_ids.iter().map(|&i| &self.drafts[i]).collect();
        let mut targets: Vec<&str> = Vec::new();
        for d in &sendable {
            if !targets.contains(&d.project.as_str()) {
                targets.push(&d.project);
            }
        }
        let place = if targets.is_empty() { "Jira".to_owned() } else { targets.join(" + ") };
        let counts = ticket_kind::type_counts(kinds.iter().map(|k| k.type_name(&cfg.issue_type)));
        let counts = if counts.is_empty() { String::new() } else { format!(" ({counts})") };
        format!("Create {}{counts} in {place} as {me}", sendable.len())
    }

    /// Tickets created in this window that a sub-task of `project` can go under, newest first.
    pub fn created_parents(&self, project: &str, default_type: &str) -> Vec<Issue> {
        (0..self.drafts.len())
            .rev()
            .filter(|&i| self.drafts[i].project == project && !self.effective_kind(i, default_type).subtask)
            .map(|i| &self.drafts[i])
            .filter_map(|d| match &d.created {
                Some(Ok(key)) => Some(Issue { key: key.clone(), summary: d.title.trim().to_owned(), status: "new".into() }),
                _ => None,
            })
            .collect()
    }

    /// Lists the work of `preset` from now on: the main window's rows for the report's day,
    /// otherwise a fresh git read from its start. The ticks go: they name lines of the old list.
    pub fn choose_range(&mut self, preset: Preset, cfg: &Config, main: &[MainRow]) {
        self.range.preset = preset;
        self.ticks.clear();
        self.error = None;
        self.range.scanning = None;
        self.range.scanned = None;
        let now = Local::now().naive_local();
        let Some(start) = ticket_range::start_of(preset, now, self.range.custom_start()) else { return };
        if let Err(e) = ticket_range::check_start(start, now) {
            self.error = Some(e);
            return;
        }
        let (tx, rx) = channel();
        let (cfg, main) = (cfg.clone(), main.to_vec());
        std::thread::spawn(move || {
            let emails = crate::report::author_emails(&cfg, crate::git::global_user_email());
            let span = Span::since(ticket_range::local(start), Local::now());
            let _ = tx.send(ticket_range::scan(&cfg, &emails, start, span, true, &main));
        });
        self.range.scanning = Some((start, rx));
    }

    /// A click on a Work from preset: the preset to read, if any. The current one does nothing;
    /// Custom only opens its fields (read once the user has set the start), dropping the old list.
    pub fn click_preset(&mut self, p: Preset) -> Option<Preset> {
        if p == self.range.preset {
            return None;
        }
        if p != Preset::Custom {
            return Some(p);
        }
        self.range.preset = p;
        self.range.scanning = None;
        self.range.scanned = None;
        self.ticks.clear();
        None
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
        self.range.reset();
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
        d.kind = self.default_kind(&self.project);
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
        (0..self.drafts.len())
            .filter(|&i| self.sendable(i, &cfg.issue_type))
            .map(|i| {
                let d = &self.drafts[i];
                let epic_field = self.meta_of(&d.project).and_then(|m| m.epic_field.clone());
                let mut extra_fields = cfg.fields.clone();
                let kind = self.effective_kind(i, &cfg.issue_type);
                extra_fields.extend(ticket_kind::kind_fields(&kind, epic_field.as_deref(), &self.required_for(i, &cfg.issue_type)));
                let new = NewIssue {
                    project: d.project.clone(),
                    issue_type: kind.type_name(&cfg.issue_type).to_owned(),
                    summary: d.title.trim().to_owned(),
                    description: d.description.trim().to_owned(),
                    assignee: Some(me.to_owned()),
                    extra_fields,
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
                    let mut draft = Draft::new(&self.project, &d.title, &d.description, rows);
                    draft.kind = self.default_kind(&self.project);
                    self.drafts.push(draft);
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

    /// Takes in the project metadata reads that have finished: read, failed, or their thread
    /// gone without an answer.
    pub fn poll_meta(&mut self) {
        for state in self.meta.values_mut() {
            if let MetaState::Loading(rx) = state {
                match rx.try_recv() {
                    Ok(Ok(m)) => *state = MetaState::Ready(Arc::new(m)),
                    Ok(Err(e)) => *state = MetaState::Failed(e),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => *state = MetaState::Failed("reading Jira stopped unexpectedly".into()),
                }
            }
        }
    }

    fn poll(&mut self) -> Vec<Outcome> {
        let mut out = Vec::new();
        self.poll_meta();
        if let Some((_, rx)) = &self.range.scanning {
            match rx.try_recv() {
                Ok(scanned) => {
                    self.range.scanned = Some(Arc::new(scanned));
                    self.range.scanning = None;
                    self.ticks.clear();
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.error = Some("reading git stopped unexpectedly".into());
                    self.range.scanning = None;
                }
            }
        }
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
        main: &[MainRow],
    ) -> (bool, Vec<Outcome>) {
        let outcomes = self.poll();
        if self.busy() || self.range.scanning() || self.meta.values().any(|m| matches!(m, MetaState::Loading(_))) {
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
                    self.body(ui, cfg, creds, me, groups, code, history, can_create, projects, main);
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
        main: &[MainRow],
    ) {
        let editable = !self.busy();
        let main_groups = groups;
        let scanned = self.range.scanned.clone();
        let (groups, code) = listed(&self.range, scanned.as_deref(), main_groups, code);
        let code = code.as_ref();
        ui.horizontal(|ui| {
            ui.strong("1. Project:");
            ui.add_enabled_ui(editable, |ui| {
                for (k, l) in projects {
                    ui.selectable_value(&mut self.project, k.clone(), egui::RichText::new(l).size(EVIDENCE_TEXT_SIZE));
                }
            });
        });
        ui.add_space(4.0);
        self.range_ui(ui, cfg, main, code, editable);
        ui.add_space(4.0);

        let items = ticked_items(groups, &self.ticks, &self.project);
        ui.strong(format!("3. Tick the commits and files the ticket is about ({} ticked):", items.len()));
        let shown: Vec<&WorkGroup> = groups.iter().filter(|g| g.visible_in(&self.project) && !g.lines.is_empty()).collect();
        let hidden = groups.iter().filter(|g| !g.visible_in(&self.project) && !g.lines.is_empty()).count();
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("work").max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
                match empty_hint(&self.range, shown.is_empty()) {
                    Some(EmptyHint::Reading) => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.weak(EmptyHint::Reading.text());
                        });
                    }
                    Some(h) => {
                        ui.weak(h.text());
                    }
                    None => {}
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
                .add_enabled(can_generate, egui::Button::new(egui::RichText::new("4. ✨ Propose tickets").strong()))
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
        ui.strong("5. Check and edit the proposals, then create them:");
        let draft_projects: BTreeSet<String> = self.drafts.iter().map(|d| d.project.clone()).collect();
        for p in &draft_projects {
            self.ensure_meta(&cfg.tickets, creds, p);
        }
        self.meta_status_ui(ui, &draft_projects);

        let default_type = cfg.tickets.issue_type.clone();
        let fallback = ticket_kind::fallback_types(&cfg.tickets.issue_types, &default_type);
        let mut kind_changes: Vec<(usize, Kind)> = Vec::new();
        let mut same_all: Option<usize> = None;
        let metas: Vec<Option<Arc<ProjectMeta>>> = self.drafts.iter().map(|d| self.meta_of(&d.project)).collect();
        let requireds: Vec<Vec<RequiredField>> = (0..self.drafts.len()).map(|i| self.required_for(i, &default_type)).collect();
        let blockers: Vec<Vec<String>> = (0..self.drafts.len()).map(|i| self.draft_blockers(i, &default_type)).collect();
        let created_parents: BTreeMap<String, Vec<Issue>> = draft_projects.iter().map(|p| (p.clone(), self.created_parents(p, &default_type))).collect();
        let subtasks: Vec<bool> = (0..self.drafts.len()).map(|i| self.effective_kind(i, &default_type).subtask).collect();
        let siblings: Vec<bool> = self
            .drafts
            .iter()
            .enumerate()
            .map(|(i, d)| self.drafts.iter().enumerate().any(|(j, o)| j != i && o.project == d.project && !o.is_created()))
            .collect();

        let title_of = |row: u64| row_title(row, groups, main_groups);
        let epic_filters = &mut self.epic_filters;
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
                let types = metas[i].as_ref().map_or(&fallback, |m| if m.types.is_empty() { &fallback } else { &m.types });
                let parents = created_parents.get(&d.project).map(Vec::as_slice).unwrap_or_default();
                let mut kind = d.kind.clone();
                let ctl = KindControls {
                    types,
                    default_type: &default_type,
                    subtask: subtasks[i],
                    meta: metas[i].as_deref(),
                    required: &requireds[i],
                    created_parents: parents,
                };
                let epic_filter = epic_filters.entry(i).or_default();
                let (changed, all) = ui.add_enabled_ui(editable && !done, |ui| kind_ui(ui, &mut kind, &ctl, epic_filter, siblings[i])).inner;
                if changed {
                    kind_changes.push((i, kind));
                }
                if all {
                    same_all = Some(i);
                }
                ui.add_enabled(
                    editable && !done,
                    egui::TextEdit::multiline(&mut d.description).hint_text("description").desired_rows(4).desired_width(f32::INFINITY),
                );
                if !done && d.selected && !blockers[i].is_empty() {
                    ui.colored_label(egui::Color32::from_rgb(210, 140, 30), format!("missing: {}", blockers[i].join(", ")));
                }
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

        for (i, kind) in kind_changes {
            self.set_kind(i, kind);
        }
        if let Some(i) = same_all {
            self.same_for_all(i);
        }

        let todo = (0..self.drafts.len()).filter(|&i| self.sendable(i, &default_type)).count();
        ui.horizontal(|ui| {
            let label = self.create_label(&cfg.tickets, me);
            let button = ui
                .add_enabled(todo > 0 && editable && can_create, egui::Button::new(egui::RichText::new(label).strong()))
                .on_disabled_hover_text(if can_create { "tick a draft with nothing missing" } else { "wait until the worklogs are posted" });
            if button.clicked() {
                self.create(cfg, creds, me, history.cloned());
            }
            if self.creating.is_some() {
                ui.spinner();
            }
        });
    }

    /// One line per project of the drafts whose metadata is being read or failed; ⟳ reads a
    /// failed one again.
    fn meta_status_ui(&mut self, ui: &mut egui::Ui, projects: &BTreeSet<String>) {
        let mut retry = None;
        for p in projects {
            match self.meta.get(p) {
                Some(MetaState::Loading(_)) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.weak(format!("reading {p}'s issue types, epics and required fields…"));
                    });
                }
                Some(MetaState::Failed(e)) => {
                    ui.horizontal(|ui| {
                        ui.colored_label(egui::Color32::from_rgb(210, 140, 30), format!("⚠ {p}: issue types not read; offering the configured ones"))
                            .on_hover_text(e);
                        if ui.small_button("⟳").on_hover_text("read again").clicked() {
                            retry = Some(p.clone());
                        }
                    });
                }
                Some(MetaState::Ready(m)) if !m.warnings.is_empty() => {
                    ui.weak(format!("⚠ {p}: {} parts not read", m.warnings.len())).on_hover_text(m.warnings.join("\n"));
                }
                _ => {}
            }
        }
        if let Some(p) = retry {
            self.meta.remove(&p);
        }
    }

    /// "2. Work from": the report's day or a start to read git from, with what was read.
    fn range_ui(&mut self, ui: &mut egui::Ui, cfg: &Config, main: &[MainRow], code: Option<&CodeSource>, editable: bool) {
        let enabled = self.range.controls_enabled(!editable);
        let mut chosen = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong("2. Work from:");
            ui.add_enabled_ui(enabled, |ui| {
                for p in Preset::ALL {
                    let r = ui.selectable_label(self.range.preset == p, egui::RichText::new(p.label()).size(EVIDENCE_TEXT_SIZE)).on_hover_text(p.hint());
                    if r.clicked() {
                        chosen = chosen.or(self.click_preset(p));
                    }
                }
            });
        });
        let now = Local::now().naive_local();
        if self.range.preset == Preset::Custom {
            ui.horizontal(|ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    ui.label("since");
                    ui.add(
                        egui_extras::DatePickerButton::new(&mut self.range.custom_date)
                            .id_salt("work-since-date")
                            .calendar_week(true)
                            .highlight_weekends(true)
                            .format("%a %Y-%m-%d"),
                    );
                    let two = |n: f64, _: std::ops::RangeInclusive<usize>| format!("{:02}", n as u32);
                    ui.add(egui::DragValue::new(&mut self.range.custom_hour).range(0..=23).custom_formatter(two)).on_hover_text("hour");
                    ui.label(":");
                    ui.add(egui::DragValue::new(&mut self.range.custom_minute).range(0..=59).speed(0.5).custom_formatter(two)).on_hover_text("minute");
                    let start = self.range.custom_start();
                    let fresh = self.range.shown_start() != Some(start);
                    let future = ticket_range::check_start(start, now).err();
                    let label = if fresh { "📥 List work" } else { "⟳ Read again" };
                    let button = egui::Button::new(egui::RichText::new(label).strong());
                    let r = ui
                        .add_enabled(future.is_none(), button)
                        .on_hover_text(format!("read your commits, checkouts and changed files {} until now", ticket_range::describe_start(start, now)))
                        .on_disabled_hover_text(future.unwrap_or_default());
                    if r.clicked() {
                        chosen = Some(Preset::Custom);
                    }
                });
            });
        }
        if let Some(p) = chosen {
            self.choose_range(p, cfg, main);
        }

        let status = status_line(&self.range, code, now);
        let refresh = self.range.can_refresh();
        ui.horizontal(|ui| {
            if let Some(st) = status {
                ui.weak(st);
            }
            if refresh && ui.add_enabled(enabled, egui::Button::new("⟳").small()).on_hover_text("read again up to now").clicked() {
                self.choose_range(self.range.preset, cfg, main);
            }
        });
        if let Some(s) = self.range.scanned.clone().filter(|s| !s.warnings.is_empty()) {
            egui::CollapsingHeader::new(format!("⚠ {} warnings while reading git", s.warnings.len())).id_salt("range-warnings").show(ui, |ui| {
                for w in &s.warnings {
                    ui.weak(w);
                }
            });
        }
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
