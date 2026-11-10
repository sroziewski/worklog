use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Accepted,
    Denied,
}

/// How the prompt was answered per day. Only `Denied` keeps the window closed (see
/// `flow::prompt_gate`); after an Accept, Jira's logged total decides, so a partly logged day
/// is asked about again. `Accepted` is kept as a record in the file and the log.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub days: BTreeMap<NaiveDate, Decision>,
}

/// `WORKLOG_STATE` in the environment overrides the state file location (tests, portable setups).
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("WORKLOG_STATE").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    dirs::config_dir().map(|d| d.join("worklog").join("state.json"))
}

impl State {
    pub fn load(path: &Path) -> Self {
        // A missing or corrupt file only means "nothing answered yet".
        std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
    }

    pub fn record(path: &Path, date: NaiveDate, d: Decision) -> Result<()> {
        let mut s = Self::load(path);
        s.days.insert(date, d);
        // Keep a quarter of history; older days are never checked again.
        while s.days.len() > 92 {
            let first = *s.days.keys().next().unwrap();
            s.days.remove(&first);
        }
        s.save(path)
    }
}
