use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use anyhow::Result;
use chrono::NaiveDate;
use eframe::egui;

use crate::config::{Config, JiraCredentials};
use crate::flow::{self, RowInput};
use crate::jira::{self, Jira};
use crate::plan::{self, Bucket, Suggestion};
use crate::report::{self, Report, Row, fmt_minutes};
use crate::state::{Decision, State};
use crate::history::History;
use crate::ui_issues::{BrowserOutcome, IssueBrowser};
use crate::ui_projects::ProjectsEditor;
use crate::ui_settings::{self, WindowSection};
use crate::ui_tickets::{CodeSource, Creator, EVIDENCE_TEXT_SIZE, Outcome, WorkGroup};

#[derive(Debug, Clone, PartialEq)]
enum Posted {
    Ok(String),
    Failed(String),
}

struct RowState {
    /// Stable for the window's life; indices shift when a manual row is removed.
    id: u64,
    checked: bool,
    minutes: u32,
    /// Index into `Report::candidates`, for rows without a ticket of their own.
    picked: Option<usize>,
    /// Project key the ticket picker is narrowed to; `None` shows all projects.
    filter: Option<String>,
    posted: Option<Posted>,
    /// Added with "+", not derived from git activity: no evidence, removable.
    manual: bool,
}

impl RowState {
    fn new(id: u64, checked: bool, minutes: u32, manual: bool) -> Self {
        Self { id, checked, minutes, picked: None, filter: None, posted: None, manual }
    }
}

fn manual_row(minutes: u32) -> Row {
    Row {
        suggestion: Suggestion {
            bucket: Bucket::Unassigned(String::new()),
            minutes,
            raw_minutes: 0,
            repos: Vec::new(),
            evidence: Vec::new(),
            commits: 0,
        },
        issue: None,
        already_logged_minutes: 0,
    }
}

/// (project key, label shown in the picker filter, e.g. "Alpha (ABC)")
pub type Projects = Vec<(String, String)>;

enum Phase {
    Loading(Receiver<Result<(Report, Projects)>>, Receiver<report::Progress>, report::LoadProgress),
    Failed(String),
    Ready,
    Posting(Receiver<(usize, Posted)>, usize),
}

pub struct WorklogApp {
    cfg: Config,
    creds: JiraCredentials,
    state_path: Option<PathBuf>,
    date: NaiveDate,
    phase: Phase,
    report: Option<Report>,
    rows: Vec<RowState>,
    projects: Projects,
    creator: Option<Creator>,
    /// Tickets created here, kept a week for "My tickets"; `Err` says why it is unavailable.
    history: Result<std::sync::Arc<History>, String>,
    /// The "My tickets" window; kept while closed, so unsaved edits survive reopening it.
    browser: Option<IssueBrowser>,
    browser_open: bool,
    /// The "Projects" window; kept while closed, so unsaved edits survive reopening it.
    projects_editor: Option<ProjectsEditor>,
    projects_open: bool,
    // `projects_editor` above is now the Projects section of the Settings window.
    /// The Settings window's Window section: loaded at start, changed by its checkbox.
    window: WindowSection,
    row_ids: flow::RowIds,
}

/// Opens the ticket history and drops records past its week. Another worklog window holding the
/// file (redb locks it) only costs this window the history.
// A held file is now waited for (`History::with_db`); an error here means it stayed held or
// cannot be opened at all.
fn open_history() -> Result<std::sync::Arc<History>, String> {
    let path = crate::history::default_path().ok_or("no config directory for the ticket history")?;
    let h = History::open(&path).map_err(|e| format!("{e:#}"))?;
    let now = chrono::Local::now().timestamp();
    match h.prune(now) {
        Ok(n) if n > 0 => log::info!("ticket history: {n} records older than {} days removed", crate::history::KEEP_DAYS),
        Ok(_) => {}
        Err(e) => log::warn!("ticket history: pruning: {e:#}"),
    }
    match h.recent(now) {
        Ok(r) => log::info!("{}", crate::history::startup_line(r.len(), &path)),
        Err(e) => log::warn!("ticket history: reading {}: {e:#}", path.display()),
    }
    Ok(std::sync::Arc::new(h))
}

/// What the window loads before it can show anything: the report, the open tickets for rows
/// without one, and the project names, telling `progress` each step.
pub fn load(
    cfg: &Config,
    jira: &Jira,
    date: NaiveDate,
    today: NaiveDate,
    fallback_email: Option<String>,
    progress: &(dyn Fn(report::Progress) + Sync),
) -> Result<(Report, Projects)> {
    let mut r = report::build_at_with(cfg, jira, date, today, fallback_email, progress)?;
    // The report skips this list when every row has a ticket, but rows added with "+" need it too.
    if r.rows.iter().all(|row| row.issue.is_some()) {
        progress(report::Progress::Step("listing your open tickets".into()));
        match jira.candidates(&cfg.project_keys, 30) {
            Ok(list) => r.candidates = list,
            Err(e) => log::warn!("listing your open issues: {e:#}"),
        }
    }
    progress(report::Progress::Step("reading project names".into()));
    let projects = picker_projects(cfg, jira);
    Ok((r, projects))
}

/// Takes in every progress message waiting on `rx`.
pub fn drain_progress(rx: &Receiver<report::Progress>, into: &mut report::LoadProgress) {
    while let Ok(p) = rx.try_recv() {
        into.apply(p);
    }
}

/// The configured projects in config order, labelled with Jira's project name, all of them
/// (one without open issues still needs its filter to create a first ticket). A failed name
/// lookup only costs the label: the key is shown instead.
pub fn picker_projects(cfg: &Config, jira: &Jira) -> Projects {
    cfg.project_keys
        .iter()
        .map(|k| {
            let label = match jira.project_name(k) {
                Ok(name) => format!("{name} ({k})"),
                Err(e) => {
                    log::warn!("project name of {k}: {e:#}");
                    k.clone()
                }
            };
            (k.clone(), label)
        })
        .collect()
}

/// The worklog icon (`assets/icon/worklog.png`, made by `make_icon.py`) for the title bar and
/// taskbar; `None`, logged, if it cannot be decoded, which only costs the window its icon.
pub fn app_icon() -> Option<egui::IconData> {
    eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon/worklog.png"))
        .inspect_err(|e| log::warn!("window icon: {e}"))
        .ok()
}

/// The main window's options; `settings` decides whether it starts above the other windows.
pub fn native_options(date: NaiveDate, settings: ui_settings::UiSettings) -> eframe::NativeOptions {
    let mut viewport = egui::ViewportBuilder::default();
    if let Some(icon) = app_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    eframe::NativeOptions {
        viewport: viewport
            .with_title(format!("Worklog — {date}"))
            .with_inner_size([860.0, 560.0])
            .with_min_inner_size([560.0, 320.0])
            .with_window_level(ui_settings::window_level(settings.always_on_top)),
        // eframe's default wgpu renderer panicked on this machine ("Failed to create staging
        // buffer for index data"); OpenGL is plenty for a dialog. WORKLOG_RENDERER=wgpu switches back.
        renderer: if std::env::var("WORKLOG_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("wgpu")) {
            eframe::Renderer::Wgpu
        } else {
            eframe::Renderer::Glow
        },
        ..Default::default()
    }
}

pub fn run(cfg: Config, creds: JiraCredentials, date: NaiveDate, state_path: Option<PathBuf>) -> Result<()> {
    let settings = ui_settings::load_or_default(ui_settings::default_path().as_deref());
    log::info!("window settings: {settings:?}");
    let options = native_options(date, settings);
    let app = WorklogApp::new(cfg, creds, date, state_path, settings);
    log::info!("window opened for {date}");
    let res = eframe::run_native("worklog", options, Box::new(|_cc| Ok(Box::new(app)))).map_err(|e| anyhow::anyhow!("GUI: {e}"));
    match &res {
        Ok(()) => log::info!("window closed"),
        Err(e) => log::error!("{e:#}"),
    }
    res
}

impl WorklogApp {
    fn new(cfg: Config, creds: JiraCredentials, date: NaiveDate, state_path: Option<PathBuf>, settings: ui_settings::UiSettings) -> Self {
        let phase = start_loading(&cfg, &creds, date);
        Self {
            cfg,
            creds,
            state_path,
            date,
            phase,
            report: None,
            rows: Vec::new(),
            projects: Vec::new(),
            creator: None,
            history: open_history().inspect_err(|e| log::warn!("ticket history: {e}")),
            browser: None,
            browser_open: false,
            projects_editor: None,
            projects_open: false,
            window: WindowSection::new(settings),
            row_ids: flow::RowIds::default(),
        }
    }

    /// Builds the day again under the current `cfg`, after the projects changed. Rows already
    /// logged come back as time already logged; an idle New ticket window goes, as its project
    /// may have been disabled.
    fn reload(&mut self) {
        log::info!("projects changed: reloading {}", self.date);
        self.report = None;
        self.rows.clear();
        self.projects.clear();
        if !self.creator.as_ref().is_some_and(Creator::busy) {
            self.creator = None;
        }
        self.phase = start_loading(&self.cfg, &self.creds, self.date);
    }

    /// Saving projects reloads the day, which would drop rows being posted or a ticket being
    /// created for one.
    fn can_reload(&self) -> bool {
        flow::can_reload(matches!(self.phase, Phase::Posting(..)), self.creator.as_ref().is_some_and(Creator::busy))
    }

    fn projects_ui(&mut self, ctx: &egui::Context) {
        if !self.projects_open {
            return;
        }
        let can_save = self.can_reload();
        let cfg = &self.cfg;
        let editor = self.projects_editor.get_or_insert_with(|| ProjectsEditor::new(cfg, crate::projects::default_path()));
        let window = &mut self.window;
        let mut open = true;
        let mut saved = None;
        let mut toggled = false;
        egui::Window::new("Settings").open(&mut open).default_width(settings_width(ctx.content_rect().width())).resizable(true).show(ctx, |ui| {
            ui.heading("Projects");
            saved = editor.body(ui, can_save);
            ui.add_space(10.0);
            toggled = window.show(ui);
        });
        self.projects_open = open;
        if toggled {
            let level = self.window.apply_toggle(ui_settings::default_path().as_deref());
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
        }
        if let Some(list) = saved {
            self.cfg.apply_projects(&list);
            self.reload();
        }
    }

    fn record(&self, d: Decision) {
        log::info!("{} answered: {d:?}", self.date);
        if let Some(p) = &self.state_path {
            // The prompt still did its job; failing to remember it only risks one more prompt.
            let _ = State::record(p, self.date, d);
        }
    }

    fn target_key(&self, i: usize) -> Option<String> {
        let report = self.report.as_ref()?;
        match &report.rows[i].suggestion.bucket {
            Bucket::Issue(k) => Some(k.clone()),
            Bucket::Unassigned(_) => self.rows[i].picked.map(|c| report.candidates[c].key.clone()),
        }
    }

    fn row_inputs(&self) -> Vec<RowInput> {
        let Some(report) = &self.report else { return Vec::new() };
        (0..self.rows.len())
            .map(|i| RowInput {
                checked: self.rows[i].checked,
                minutes: self.rows[i].minutes,
                target: self.target_key(i),
                posted_ok: matches!(self.rows[i].posted, Some(Posted::Ok(_))),
                comment: if self.rows[i].manual { String::new() } else { plan::comment(&report.rows[i].suggestion) },
            })
            .collect()
    }

    /// Minutes of the workday not yet covered by ticked or already-logged rows; what "+" offers.
    fn unplanned_minutes(&self) -> u32 {
        let already = self.report.as_ref().map_or(0, |r| r.already_logged_minutes);
        let planned: u32 = self.rows.iter().filter(|r| r.checked || matches!(r.posted, Some(Posted::Ok(_)))).map(|r| r.minutes).sum();
        self.cfg.workday_minutes.saturating_sub(already + planned)
    }

    fn add_row(&mut self) {
        let minutes = self.unplanned_minutes();
        if let Some(report) = &mut self.report {
            report.rows.push(manual_row(minutes));
            // Unticked until a ticket is picked, like rows without a key.
            let id = self.row_ids.take();
            self.rows.push(RowState::new(id, false, minutes, true));
        }
    }

    fn remove_row(&mut self, i: usize) {
        if let Some(report) = &mut self.report
            && i < self.rows.len()
            && self.rows[i].manual
        {
            report.rows.remove(i);
            self.rows.remove(i);
        }
    }

    fn postable(&self) -> Vec<usize> {
        flow::postable(&self.row_inputs())
    }

    fn accept(&mut self) {
        let Some(report) = &self.report else { return };
        let Ok(day_start) = self.cfg.day_start_time() else { return };
        let jobs = flow::plan_posts(self.date, day_start, report.already_logged_minutes, &self.row_inputs());
        if jobs.is_empty() {
            return;
        }
        let n = jobs.len();
        let (tx, rx) = channel();
        let creds = self.creds.clone();
        std::thread::spawn(move || {
            let client = Jira::new(&creds.url, &creds.token);
            for job in jobs {
                let res = match client.add_worklog(&job.key, &jira::local(job.started), job.minutes, &job.comment) {
                    Ok(id) => {
                        log::info!("logged {} min on {} from {} (worklog {id})", job.minutes, job.key, job.started);
                        Posted::Ok(id)
                    }
                    Err(e) => {
                        log::error!("logging {} min on {} failed: {e:#}", job.minutes, job.key);
                        Posted::Failed(format!("{e:#}"))
                    }
                };
                if tx.send((job.row, res)).is_err() {
                    return;
                }
            }
        });
        self.phase = Phase::Posting(rx, n);
    }

    fn poll(&mut self) {
        if let Phase::Loading(_, prx, progress) = &mut self.phase {
            drain_progress(prx, progress);
        }
        match &mut self.phase {
            Phase::Loading(rx, ..) => match rx.try_recv() {
                Ok(Ok((report, projects))) => {
                    let ids = &mut self.row_ids;
                    self.rows = report
                        .rows
                        .iter()
                        .map(|r| RowState::new(ids.take(), r.issue.is_some() && r.suggestion.minutes > 0, r.suggestion.minutes, false))
                        .collect();
                    let starter = flow::needs_starter_row(report.rows.len(), report.budget_minutes);
                    self.report = Some(report);
                    self.projects = projects;
                    self.phase = Phase::Ready;
                    // A day without git activity still needs a way to log it: a row to pick a ticket for.
                    if starter {
                        self.add_row();
                    }
                }
                Ok(Err(e)) => self.phase = Phase::Failed(format!("{e:#}")),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(_) => self.phase = Phase::Failed("loading stopped unexpectedly".into()),
            },
            Phase::Posting(rx, remaining) => {
                let mut posted: Vec<Option<Posted>> = self.rows.iter().map(|r| r.posted.clone()).collect();
                let finished = flow::drain(rx, &mut posted, remaining);
                for (row, p) in self.rows.iter_mut().zip(posted) {
                    row.posted = p;
                }
                if finished {
                    self.phase = Phase::Ready;
                    let any_ok = self.rows.iter().any(|r| matches!(r.posted, Some(Posted::Ok(_))));
                    if let Some(d) = flow::decision_after_posting(any_ok) {
                        self.record(d);
                    }
                }
            }
            _ => {}
        }
    }

    fn rows_ui(&mut self, ui: &mut egui::Ui) {
        let editable = matches!(self.phase, Phase::Ready);
        let mut remove = None;
        let mut new_ticket: Option<(String, Option<usize>)> = None;
        self.grid_ui(ui, editable, &mut remove, &mut new_ticket);
        if let Some(i) = remove {
            self.remove_row(i);
        }
        ui.add_space(8.0);
        let busy = self.creator.as_ref().is_some_and(Creator::busy);
        ui.horizontal(|ui| {
            let left = self.unplanned_minutes();
            let add = ui
                .add_enabled(editable && left > 0, egui::Button::new("+ Add row"))
                .on_hover_text(format!("log more work on another ticket ({} of the day not planned yet)", fmt_minutes(left)))
                .on_disabled_hover_text("the ticked rows already fill the workday");
            if add.clicked() {
                self.add_row();
            }
            let project = self.default_project();
            if new_ticket_button(ui, editable && !busy && project.is_some())
                .on_hover_text("create Jira tickets from today's commits and files, proposed by the LLM")
                .clicked()
                && let Some(p) = project
            {
                new_ticket = Some((p, None));
            }
        });
        if let Some((project, row)) = new_ticket {
            self.open_creator(project, row);
        }
    }

    /// Opens the New ticket window, or points the open one at `row` (its project, its lines
    /// ticked). Drafts already written are kept; a busy window is left alone.
    fn open_creator(&mut self, project: String, row: Option<usize>) {
        if self.creator.as_ref().is_some_and(Creator::busy) {
            return;
        }
        let id = row.and_then(|i| self.rows.get(i)).map(|r| r.id);
        let group = id.and_then(|id| self.work_groups().into_iter().find(|g| g.row == id));
        let creator = match &mut self.creator {
            Some(c) => {
                // Opened without a row: keep the project the user is working in.
                if row.is_some() {
                    c.project = project;
                }
                c
            }
            None => self.creator.insert(Creator::new(project, None)),
        };
        creator.origin = id;
        if let Some(g) = &group {
            creator.focus_row(g);
        }
    }

    /// Every row as the New ticket window lists it, with the project its work belongs to.
    fn work_groups(&self) -> Vec<WorkGroup> {
        let Some(report) = &self.report else { return Vec::new() };
        report
            .rows
            .iter()
            .zip(&self.rows)
            .map(|(row, st)| {
                let title = match &row.suggestion.bucket {
                    Bucket::Issue(k) => {
                        let summary = row.issue.as_ref().map(|i| format!(" {}", truncate(&i.summary, 50))).unwrap_or_default();
                        format!("{k}{summary}")
                    }
                    Bucket::Unassigned(_) if st.manual => "row added by hand".to_owned(),
                    Bucket::Unassigned(repo) => format!("{repo} — no ticket yet"),
                };
                let project = flow::group_project(&row.suggestion.bucket, st.manual, &report.repo_projects);
                WorkGroup { row: st.id, title, project, repos: row.suggestion.repos.clone(), lines: row.suggestion.evidence.clone() }
            })
            .collect()
    }

    /// The project a New ticket window opened without a row starts in: the first configured one
    /// (the window lets the user switch).
    // `None` when no project is enabled: there is nothing to create a ticket in.
    fn default_project(&self) -> Option<String> {
        flow::default_project(&self.projects, &self.cfg.project_keys)
    }

    fn grid_ui(&mut self, ui: &mut egui::Ui, editable: bool, remove: &mut Option<usize>, new_ticket: &mut Option<(String, Option<usize>)>) {
        let busy = self.creator.as_ref().is_some_and(Creator::busy);
        let default_project = self.default_project();
        let Some(report) = &self.report else { return };
        egui::Grid::new("rows").striped(true).num_columns(5).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.strong("");
            ui.strong("Ticket");
            ui.strong("Summary / evidence");
            ui.strong("Hours");
            ui.strong("");
            ui.end_row();
            for (i, row) in report.rows.iter().enumerate() {
                let st = &mut self.rows[i];
                let done = matches!(st.posted, Some(Posted::Ok(_)));
                ui.add_enabled(editable && !done, egui::Checkbox::without_text(&mut st.checked));
                match &row.suggestion.bucket {
                    Bucket::Issue(k) => {
                        ui.hyperlink_to(k, format!("{}/browse/{k}", self.creds.url));
                    }
                    Bucket::Unassigned(repo) => {
                        let label = st.picked.map(|c| report.candidates[c].key.clone()).unwrap_or_else(|| "pick ticket…".into());
                        ui.vertical(|ui| {
                        ui.add_enabled_ui(editable && !done, |ui| {
                            egui::ComboBox::from_id_salt(("pick", i))
                                .selected_text(label)
                                .width(130.0)
                                // The default closes on any click, so a filter click would shut it.
                                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                                .show_ui(ui, |ui| {
                                if self.projects.len() > 1 {
                                    ui.horizontal(|ui| {
                                        ui.selectable_value(&mut st.filter, None, "All");
                                        for (key, label) in &self.projects {
                                            ui.selectable_value(&mut st.filter, Some(key.clone()), label);
                                        }
                                    });
                                    ui.separator();
                                }
                                let shown = report.candidates.iter().enumerate().filter(|(_, cand)| {
                                    st.filter.as_ref().is_none_or(|f| jira::project_of(&cand.key).eq_ignore_ascii_case(f))
                                });
                                for (c, cand) in shown {
                                    let text = format!("{} {}", cand.key, truncate(&cand.summary, 60));
                                    if ui.selectable_value(&mut st.picked, Some(c), text).clicked() {
                                        st.checked = true;
                                        ui.close();
                                    }
                                }
                            });
                        })
                        .response
                        .on_hover_text(if st.manual { "added by hand".to_owned() } else { format!("activity in {repo} names no ticket") });
                        let project = st.filter.clone().or_else(|| default_project.clone());
                        if new_ticket_button(ui, editable && !done && !busy && project.is_some())
                            .on_hover_text("no fitting ticket? draft one from this row's commits and files and create it in Jira")
                            .clicked()
                            && let Some(p) = project
                        {
                            *new_ticket = Some((p, Some(i)));
                        }
                        });
                    }
                }
                ui.vertical(|ui| {
                    ui.set_max_width(460.0);
                    let picked = st.picked.map(|c| &report.candidates[c]);
                    let title = match (&row.issue, &row.suggestion.bucket, picked) {
                        (Some(issue), _, _) | (None, Bucket::Unassigned(_), Some(issue)) => format!("{} [{}]", issue.summary, issue.status),
                        (None, Bucket::Issue(_), _) => "(not found in Jira)".into(),
                        (None, Bucket::Unassigned(_), None) if st.manual => "pick a ticket for this time".into(),
                        (None, Bucket::Unassigned(repo), None) => format!("no ticket key — {repo}"),
                    };
                    ui.label(title);
                    if row.suggestion.evidence.is_empty() {
                        return;
                    }
                    let extra = if row.already_logged_minutes > 0 {
                        format!(" · already logged {} today", fmt_minutes(row.already_logged_minutes))
                    } else {
                        String::new()
                    };
                    egui::CollapsingHeader::new(
                        egui::RichText::new(format!("{} items · {}{extra}", row.suggestion.evidence.len(), row.suggestion.repos.join(", ")))
                            .size(EVIDENCE_TEXT_SIZE),
                    )
                        .id_salt(("ev", i))
                        .show(ui, |ui| {
                            for e in &row.suggestion.evidence {
                                ui.label(egui::RichText::new(e).size(EVIDENCE_TEXT_SIZE));
                            }
                        });
                });
                let mut hours = f64::from(st.minutes) / 60.0;
                let step = f64::from(self.cfg.round_minutes) / 60.0;
                let resp = ui.add_enabled(
                    editable && !done,
                    egui::DragValue::new(&mut hours).range(0.0..=24.0).speed(step).fixed_decimals(2).suffix(" h"),
                );
                if resp.changed() {
                    let q = f64::from(self.cfg.round_minutes);
                    st.minutes = ((hours * 60.0 / q).round() * q) as u32;
                }
                match &st.posted {
                    Some(Posted::Ok(_)) => ui.colored_label(egui::Color32::from_rgb(40, 150, 70), "logged"),
                    Some(Posted::Failed(e)) => ui.colored_label(egui::Color32::from_rgb(200, 60, 50), "failed").on_hover_text(e),
                    None if st.manual => {
                        let r = ui.add_enabled(editable, egui::Button::new("×").small()).on_hover_text("remove this row");
                        if r.clicked() {
                            *remove = Some(i);
                        }
                        r
                    }
                    None => ui.label(""),
                };
                ui.end_row();
            }
        });
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_owned() } else { format!("{}…", s.chars().take(n).collect::<String>()) }
}

impl eframe::App for WorklogApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll();
        let ctx = ui.ctx().clone();
        if matches!(self.phase, Phase::Loading(..) | Phase::Posting(..)) {
            ctx.request_repaint_after(Duration::from_millis(100));
        }

        egui::Panel::bottom("footer").show(ui, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                match &self.phase {
                    Phase::Ready => {
                        let selected: u32 = self.postable().iter().map(|&i| self.rows[i].minutes).sum();
                        let posted: u32 = self.rows.iter().filter(|r| matches!(r.posted, Some(Posted::Ok(_)))).map(|r| r.minutes).sum();
                        let already = self.report.as_ref().map_or(0, |r| r.already_logged_minutes);
                        ui.label(format!(
                            "To log: {} · logged today: {} · day total: {} / {}",
                            fmt_minutes(selected),
                            fmt_minutes(already + posted),
                            fmt_minutes(already + posted + selected),
                            fmt_minutes(self.cfg.workday_minutes)
                        ));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let any_posted = posted > 0;
                            if ui.button(if any_posted { "Close" } else { "Deny" }).clicked() {
                                if !any_posted {
                                    self.record(Decision::Denied);
                                }
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            let failed = self.rows.iter().any(|r| matches!(r.posted, Some(Posted::Failed(_))));
                            let label = if failed { "Retry" } else { "Accept" };
                            // A ticket being created may still repick a row; let it land first.
                            let creating = self.creator.as_ref().is_some_and(Creator::creating);
                            let accept = ui
                                .add_enabled(selected > 0 && !creating, egui::Button::new(label))
                                .on_disabled_hover_text(if creating { "wait until the new tickets are created" } else { "nothing ticked to log" });
                            if accept.clicked() {
                                self.accept();
                            }
                        });
                    }
                    Phase::Loading(_, _, progress) => {
                        ui.spinner();
                        ui.label(progress.label());
                        // The header with the button only shows once the day is loaded.
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if projects_button(ui).clicked() {
                                self.projects_open = true;
                            }
                        });
                    }
                    Phase::Posting(_, n) => {
                        ui.spinner();
                        ui.label(format!("Logging work… {n} left"));
                    }
                    Phase::Failed(_) => {
                        if ui.button("Close").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                }
            });
            ui.add_space(6.0);
        });

        egui::CentralPanel::default().show(ui, |ui| {
            if let Phase::Failed(e) = &self.phase {
                ui.heading("Could not prepare suggestions");
                ui.label(e);
                ui.add_space(8.0);
                if projects_button(ui).clicked() {
                    self.projects_open = true;
                }
                return;
            }
            let Some(report) = &self.report else { return };
            let mut open_new = false;
            let has_project = self.default_project().is_some();
            ui.horizontal(|ui| {
                ui.heading(format!("Work log suggestions for {}", report.date));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if projects_button(ui).clicked() {
                        self.projects_open = true;
                    }
                    let busy = self.creator.as_ref().is_some_and(Creator::busy);
                    open_new = new_ticket_button(ui, matches!(self.phase, Phase::Ready) && !busy && has_project)
                        .on_hover_text("create Jira tickets from today's commits and files, proposed by the LLM")
                        .clicked();
                    let mine = ui
                        .button(egui::RichText::new("🗂 My tickets").strong())
                        .on_hover_text("tickets created here in the last week: edit or regenerate their title and description, then save to Jira");
                    if mine.clicked() {
                        if let Some(b) = &mut self.browser {
                            b.refresh();
                        }
                        self.browser_open = true;
                    }
                });
            });
            if open_new && let Some(project) = self.default_project() {
                self.open_creator(project, None);
            }
            let Some(report) = &self.report else { return };
            ui.label(format!(
                "Jira user {} · already logged {} · {} left to fill",
                report.me,
                fmt_minutes(report.already_logged_minutes),
                fmt_minutes(report.budget_minutes)
            ));
            for w in &report.warnings {
                ui.colored_label(egui::Color32::from_rgb(190, 130, 20), format!("⚠ {w}"));
            }
            if let Some(w) = flow::no_project_warning(&self.cfg.project_keys) {
                ui.colored_label(egui::Color32::from_rgb(190, 130, 20), format!("⚠ {w}"));
            }
            ui.separator();
            let activity_rows = self.rows.iter().filter(|r| !r.manual).count();
            match flow::day_view(activity_rows, self.rows.len(), self.unplanned_minutes()) {
                flow::DayView::Full => {
                    ui.label("No git activity found for this day, and the day is already full.");
                    return;
                }
                flow::DayView::NoActivity => {
                    ui.label("No git activity found for this day: pick the tickets you worked on from the list, set the hours, and Accept. \"+ Add row\" adds another.");
                    ui.add_space(4.0);
                }
                flow::DayView::Activity => {}
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.rows_ui(ui));
        });

        self.creator_ui(&ctx);
        self.browser_ui(&ctx);
        self.projects_ui(&ctx);
    }
}

impl WorklogApp {
    fn creator_ui(&mut self, ctx: &egui::Context) {
        if self.creator.is_none() {
            return;
        }
        let groups = self.work_groups();
        let (Some(creator), Some(report)) = (&mut self.creator, &mut self.report) else { return };
        let me = report.me.clone();
        let can_create = matches!(self.phase, Phase::Ready);
        let code = CodeSource { date: report.date, repo_paths: &report.repo_paths };
        let history = self.history.as_ref().ok();
        let (open, outcomes) = creator.show(ctx, &self.cfg, &self.creds, &me, &groups, Some(&code), history, can_create, &self.projects);
        for Outcome::Created { issue, rows } in outcomes {
            let mut picks: Vec<flow::Pick> = self
                .rows
                .iter()
                .zip(&report.rows)
                .map(|(r, row)| flow::Pick {
                    picked: r.picked,
                    checked: r.checked,
                    posted_ok: matches!(r.posted, Some(Posted::Ok(_))),
                    own_key: matches!(row.suggestion.bucket, Bucket::Issue(_)),
                })
                .collect();
            let ids: Vec<u64> = self.rows.iter().map(|r| r.id).collect();
            let targets = flow::created_targets(&ids, &rows);
            log::info!("created {}; rows picking it: {targets:?}", issue.key);
            flow::apply_created(&mut report.candidates, &mut picks, issue, &targets);
            for (st, p) in self.rows.iter_mut().zip(picks) {
                st.picked = p.picked;
                st.checked = p.checked;
            }
        }
        if !open {
            self.creator = None;
        }
    }

    fn browser_ui(&mut self, ctx: &egui::Context) {
        if !self.browser_open {
            return;
        }
        let history = self.history.clone();
        let browser = self.browser.get_or_insert_with(|| IssueBrowser::new(history));
        let (open, outcomes) = browser.show(ctx, &self.cfg, &self.creds, &self.projects);
        self.browser_open = open;
        for BrowserOutcome::Updated(issue) in outcomes {
            if let Some(report) = &mut self.report {
                flow::apply_updated(report, &issue);
            }
        }
    }
}

fn new_ticket_button(ui: &mut egui::Ui, enabled: bool) -> egui::Response {
    ui.add_enabled(enabled, egui::Button::new(egui::RichText::new("➕ New ticket…").strong()))
        .on_disabled_hover_text("not now: data is loading, worklogs are posting, the New ticket window is busy, or no project is enabled")
}

fn projects_button(ui: &mut egui::Ui) -> egui::Response {
    ui.button(egui::RichText::new("⚙ Settings").strong())
        .on_hover_text("the projects suggestions come from (name, git folder, Jira key; add, remove, disable) and the window (always on top)")
}

/// The Settings window's opening width in a main window `content` points wide: 80 narrower,
/// at least 640 and at most 1280, since the git folder paths are long and the fields grow with
/// the window (egui keeps a window wider than the screen on it).
pub fn settings_width(content: f32) -> f32 {
    (content - 80.0).clamp(640.0, 1280.0)
}

/// Starts building the day's report on a thread.
fn start_loading(cfg: &Config, creds: &JiraCredentials, date: NaiveDate) -> Phase {
    let (tx, rx) = channel();
    let (ptx, prx) = channel();
    let (c, cr) = (cfg.clone(), creds.clone());
    std::thread::spawn(move || {
        let jira = Jira::new(&cr.url, &cr.token);
        let progress = |p: report::Progress| {
            let _ = ptx.send(p);
        };
        let today = chrono::Local::now().date_naive();
        let _ = tx.send(load(&c, &jira, date, today, crate::git::global_user_email(), &progress));
    });
    Phase::Loading(rx, prx, report::LoadProgress::default())
}
