//! The projects the app works for, edited in the "Projects" window and kept in `projects.toml`
//! next to `config.toml`. While the file exists it replaces `repo_roots`, `project_keys` and
//! `repo_projects` from `config.toml` / `.env`; disabled projects are kept but not scanned, and
//! their tickets are not suggested.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub name: String,
    /// A git repo, or a folder holding repos one or two levels down. Empty: a Jira-only project
    /// (meetings, support), offered in the ticket lists but with nothing to scan.
    #[serde(default)]
    pub git_dir: String,
    pub jira_key: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

impl Project {
    pub fn new(name: &str, git_dir: &str, jira_key: &str, enabled: bool) -> Self {
        Self { name: name.to_owned(), git_dir: git_dir.to_owned(), jira_key: jira_key.to_owned(), enabled }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    project: Vec<Project>,
}

/// `WORKLOG_PROJECTS` in the environment overrides the location (tests, portable setups).
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("WORKLOG_PROJECTS").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    dirs::config_dir().map(|d| d.join("worklog").join("projects.toml"))
}

/// The list in the file, checked as `validate` does; `None` when there is no file.
pub fn load(path: &Path) -> Result<Option<Vec<Project>>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let file: File = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let list = validate(&file.project).with_context(|| format!("checking {}", path.display()))?;
    Ok(Some(list))
}

/// Writes `list` after `validate`, returning what was written. Through a temporary file renamed
/// over the old one, so a failed write never leaves half a list (and the app unable to start).
pub fn save(path: &Path, list: &[Project]) -> Result<Vec<Project>> {
    let list = validate(list)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let text = toml::to_string(&File { project: list.clone() }).context("formatting the projects")?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("replacing {}", path.display()));
    }
    Ok(list)
}

/// The list trimmed, with keys upper-cased; or the first problem, naming the project. At least
/// one project, each with a name and a Jira key (A-Z, then A-Z, 0-9 or `_`, as Jira allows); a git
/// folder may be empty but not listed twice (compared ignoring case). Whether the folder exists
/// is not checked: a drive that is not mounted today must not make the list unsavable.
pub fn validate(list: &[Project]) -> Result<Vec<Project>> {
    if list.is_empty() {
        bail!("add at least one project");
    }
    let mut out: Vec<Project> = Vec::with_capacity(list.len());
    for (i, p) in list.iter().enumerate() {
        let name = p.name.trim();
        if name.is_empty() {
            bail!("project {}: the name is empty", i + 1);
        }
        // ASCII only: Unicode upper-casing turns "ﬁx" into "FIX", a key the user never typed.
        let key = p.jira_key.trim().to_ascii_uppercase();
        if key.is_empty() {
            bail!("{name}: the Jira key is empty");
        }
        if !is_project_key(&key) {
            bail!("{name}: \"{key}\" is not a Jira project key (A-Z, then A-Z, 0-9 or _)");
        }
        let dir = p.git_dir.trim();
        if !dir.is_empty()
            && let Some(other) = out.iter().find(|o| !o.git_dir.is_empty() && same_dir(&o.git_dir, dir))
        {
            bail!("{name}: the folder {dir} is already used by {}", other.name);
        }
        out.push(Project { name: name.to_owned(), git_dir: dir.to_owned(), jira_key: key, enabled: p.enabled });
    }
    Ok(out)
}

fn is_project_key(k: &str) -> bool {
    let mut chars = k.chars();
    chars.next().is_some_and(|c| c.is_ascii_uppercase()) && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn same_dir(a: &str, b: &str) -> bool {
    crate::config::path_key(Path::new(a)) == crate::config::path_key(Path::new(b))
}

/// The list the Projects window starts from when there is no file yet: one project per
/// `repo_projects` entry, one per other `repo_roots` folder (its key from `project_of_repo`,
/// empty when that is ambiguous, for the user to fill), and one without a folder per key no
/// folder uses. Each is named after its folder, or its key.
pub fn seed(cfg: &Config) -> Vec<Project> {
    let dir_name = |d: &str| Path::new(d).file_name().map_or_else(|| d.to_owned(), |n| n.to_string_lossy().into_owned());
    let mut out: Vec<Project> = Vec::new();
    for (root, key) in &cfg.repo_projects {
        let key = cfg.project_keys.iter().find(|k| k.eq_ignore_ascii_case(key)).unwrap_or(key);
        out.push(Project::new(&dir_name(root), root, key, true));
    }
    for root in &cfg.repo_roots {
        let root_s = root.display().to_string();
        if out.iter().any(|p| same_dir(&p.git_dir, &root_s)) {
            continue;
        }
        let key = cfg.project_of_repo(root).unwrap_or_default();
        out.push(Project::new(&dir_name(&root_s), &root_s, &key, true));
    }
    for key in &cfg.project_keys {
        if !out.iter().any(|p| p.jira_key.eq_ignore_ascii_case(key)) {
            out.push(Project::new(key, "", key, true));
        }
    }
    out
}
