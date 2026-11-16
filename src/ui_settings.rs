//! Settings of the window itself, kept in `ui.toml` next to `config.toml`: apart from the
//! projects, whose file would replace the `.env` project settings the moment it exists.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use eframe::egui;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiSettings {
    /// The window stays above other windows, so the 17:00 prompt is not lost behind them.
    pub always_on_top: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self { always_on_top: true }
    }
}

/// `WORKLOG_UI` in the environment overrides the location (tests, portable setups).
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("WORKLOG_UI").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    dirs::config_dir().map(|d| d.join("worklog").join("ui.toml"))
}

/// The settings in `path`; the defaults when there is no file.
pub fn load(path: &Path) -> Result<UiSettings> {
    if !path.exists() {
        return Ok(UiSettings::default());
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// `load` that never fails: a window setting must not keep the prompt from opening, so a
/// missing location or a broken file gives the defaults, logged.
pub fn load_or_default(path: Option<&Path>) -> UiSettings {
    let Some(path) = path else {
        log::warn!("window settings: no config directory; using the defaults");
        return UiSettings::default();
    };
    load(path).unwrap_or_else(|e| {
        log::warn!("window settings: {e:#}; using the defaults");
        UiSettings::default()
    })
}

/// Writes `s` through a temporary file renamed over the old one, removed again if that fails.
pub fn save(path: &Path, s: &UiSettings) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let text = toml::to_string(s).context("formatting the window settings")?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("replacing {}", path.display()));
    }
    Ok(())
}

/// `save` to an optional location: `None` (no config directory) is an error to show, since the
/// choice would silently be lost at the next start.
pub fn save_at(path: Option<&Path>, s: &UiSettings) -> Result<(), String> {
    match path {
        Some(p) => save(p, s).map_err(|e| format!("{e:#}")),
        None => Err("no config directory to keep the window settings in".to_owned()),
    }
}

pub fn window_level(always_on_top: bool) -> egui::WindowLevel {
    if always_on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal }
}

/// The Window section of the Settings window, as the main window holds it.
#[derive(Debug, Clone, Default)]
pub struct WindowSection {
    pub settings: UiSettings,
    /// Why the last change could not be saved; it still applies to this run.
    pub error: Option<String>,
}

impl WindowSection {
    pub fn new(settings: UiSettings) -> Self {
        Self { settings, error: None }
    }

    /// Draws the section; true when "Always on top" was just toggled.
    pub fn show(&mut self, ui: &mut egui::Ui) -> bool {
        ui.heading("Window");
        let toggled = ui
            .checkbox(&mut self.settings.always_on_top, "Always on top")
            .on_hover_text("keep this window above the others; applies at once and is remembered")
            .changed();
        if let Some(e) = self.error_line() {
            ui.colored_label(egui::Color32::from_rgb(200, 60, 50), e);
        }
        toggled
    }

    /// After the checkbox changed: remembers the settings in `path` (`error` says why not, and
    /// is cleared by a later success) and returns the level the window must switch to, which
    /// follows the checkbox even when remembering failed.
    pub fn apply_toggle(&mut self, path: Option<&Path>) -> egui::WindowLevel {
        log::info!("always on top: {}", self.settings.always_on_top);
        self.error = save_at(path, &self.settings).err();
        window_level(self.settings.always_on_top)
    }

    /// The line shown under the checkbox when the last change could not be remembered.
    pub fn error_line(&self) -> Option<String> {
        self.error.as_ref().map(|e| format!("not remembered for next time: {e}"))
    }
}
