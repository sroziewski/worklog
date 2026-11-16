//! The "Projects" window: the projects the app works for, each a name, a git folder and a Jira
//! key, enabled or not. Saving writes `projects.toml`; the main window then reloads the day.

use std::path::PathBuf;

use eframe::egui;

use crate::config::Config;
use crate::projects::{self, Project};

pub struct ProjectsEditor {
    /// `projects.toml`; `None` when there is no config directory, which makes Save fail.
    path: Option<PathBuf>,
    /// The list as edited.
    pub rows: Vec<Project>,
    /// The list as last saved (or loaded); what Revert returns to.
    saved: Vec<Project>,
    /// Whether `saved` is in `projects.toml`; until it is, Save is offered even without edits,
    /// so the setup taken over from `config.toml` / `.env` can be kept as it is.
    in_file: bool,
    /// The outcome of the last Save: `Ok` a confirmation, `Err` why nothing was saved.
    pub message: Option<Result<String, String>>,
}

/// Widths of the name, git folder and Jira key fields when the grid has `available` points:
/// what the checkbox, ⚠ and × columns and the spacing leave, mostly to the folder, which holds
/// the longest text. Below `MIN_FIELDS` the fields keep that width and the window scrolls.
pub fn field_widths(available: f32) -> (f32, f32, f32) {
    let rest = (available - FIXED_COLUMNS).max(MIN_FIELDS);
    let name = (rest * 0.2).clamp(120.0, 260.0);
    let key = (rest * 0.1).clamp(70.0, 120.0);
    (name, rest - name - key, key)
}

/// The checkbox, ⚠ and × columns with the grid and row spacing between them.
const FIXED_COLUMNS: f32 = 130.0;
const MIN_FIELDS: f32 = 480.0;

impl ProjectsEditor {
    /// Starts from `cfg`'s projects file list, or, with no file yet, from what `config.toml` and
    /// `.env` configure (`projects::seed`), so the first Save keeps the current setup.
    pub fn new(cfg: &Config, path: Option<PathBuf>) -> Self {
        let in_file = cfg.projects.is_some();
        let rows = cfg.projects.clone().unwrap_or_else(|| projects::seed(cfg));
        Self { path, saved: rows.clone(), rows, message: None, in_file }
    }

    /// Edited since the last save, or never saved to the file yet.
    pub fn dirty(&self) -> bool {
        !self.in_file || self.rows != self.saved
    }

    /// Edited since the last save or load; what Revert would drop.
    pub fn edited(&self) -> bool {
        self.rows != self.saved
    }

    /// An empty, enabled row at the end.
    pub fn add(&mut self) {
        self.rows.push(Project::new("", "", "", true));
        self.message = None;
    }

    pub fn remove(&mut self, i: usize) {
        if i < self.rows.len() {
            self.rows.remove(i);
            self.message = None;
        }
    }

    pub fn revert(&mut self) {
        self.rows = self.saved.clone();
        self.message = None;
    }

    /// Writes the list; on success the rows become the normalised list written (trimmed, keys
    /// upper-cased) and it is returned for the app to apply. On failure nothing is written and
    /// `message` says why.
    pub fn save(&mut self) -> Option<Vec<Project>> {
        let result = match &self.path {
            Some(p) => projects::save(p, &self.rows).map_err(|e| format!("{e:#}")),
            None => Err("no config directory to keep the projects in".to_owned()),
        };
        match result {
            Ok(list) => {
                let on = list.iter().filter(|p| p.enabled).count();
                self.message = Some(Ok(format!("saved: {on} of {} projects enabled", list.len())));
                self.rows = list.clone();
                self.saved = list.clone();
                self.in_file = true;
                Some(list)
            }
            Err(e) => {
                self.message = Some(Err(e));
                None
            }
        }
    }

    /// Draws the window; returns whether it stays open and the list just saved, if any.
    /// `can_save` is false while the main window cannot reload (posting, creating tickets).
    pub fn show(&mut self, ctx: &egui::Context, can_save: bool) -> (bool, Option<Vec<Project>>) {
        let mut open = true;
        let mut saved = None;
        egui::Window::new("Projects").open(&mut open).default_width(720.0).resizable(true).show(ctx, |ui| {
            saved = self.body(ui, can_save);
        });
        (open, saved)
    }

    /// The section itself: the list, "+ Add project", Save / Revert and the last outcome;
    /// returns the list just saved, if any. `show` frames it as a window of its own, the main
    /// window as part of its Settings window.
    pub fn body(&mut self, ui: &mut egui::Ui, can_save: bool) -> Option<Vec<Project>> {
        let mut saved = None;
        ui.label("Suggestions come from the enabled projects only: their git folders are read, and their tickets offered.");
        ui.add_space(6.0);
        let mut remove = None;
        let (name_w, dir_w, key_w) = field_widths(ui.available_width());
        egui::Grid::new("projects").striped(true).num_columns(5).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.strong("On");
            ui.strong("Name");
            ui.strong("Git folder");
            ui.strong("Jira key");
            ui.strong("");
            ui.end_row();
            for (i, p) in self.rows.iter_mut().enumerate() {
                ui.checkbox(&mut p.enabled, "").on_hover_text("off: not read, its tickets not suggested; kept for later");
                ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(name_w).hint_text("name"));
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut p.git_dir).desired_width(dir_w).hint_text("repo, or folder of repos (optional)"));
                    let dir = p.git_dir.trim();
                    if !dir.is_empty() && !std::path::Path::new(dir).is_dir() {
                        ui.colored_label(egui::Color32::from_rgb(190, 130, 20), "⚠").on_hover_text("folder not found");
                    }
                });
                ui.add(egui::TextEdit::singleline(&mut p.jira_key).desired_width(key_w).hint_text("KEY"));
                if ui.small_button("×").on_hover_text("remove this project").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            self.remove(i);
        }
        ui.add_space(4.0);
        if ui.button("+ Add project").clicked() {
            self.add();
        }
        ui.separator();
        ui.horizontal(|ui| {
            let save = ui
                .add_enabled(can_save && self.dirty(), egui::Button::new("💾 Save"))
                .on_hover_text("write the list and reload today's suggestions")
                .on_disabled_hover_text(if can_save { "nothing changed" } else { "wait until worklogs are posted and tickets created" });
            if !self.in_file {
                save.clone().on_hover_text("not saved yet: the list comes from .env / config.toml until it is");
            }
            if save.clicked() {
                saved = self.save();
            }
            if ui.add_enabled(self.edited(), egui::Button::new("↺ Revert")).on_hover_text("drop the unsaved changes").clicked() {
                self.revert();
            }
            if let Some(p) = &self.path {
                ui.weak("projects.toml").on_hover_text(p.display().to_string());
            }
        });
        match &self.message {
            Some(Ok(m)) => {
                ui.colored_label(egui::Color32::from_rgb(40, 150, 70), m);
            }
            Some(Err(e)) => {
                ui.colored_label(egui::Color32::from_rgb(200, 60, 50), e);
            }
            None => {}
        }
        saved
    }
}
