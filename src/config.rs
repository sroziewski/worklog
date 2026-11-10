use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::NaiveTime;
use serde::Deserialize;

/// User settings, read from `<config dir>/worklog/config.toml` over the local `.env` (see
/// `local_env_path`); every field has a default so both are optional. The defaults name no repos,
/// projects or LLM provider: those are the user's own and live outside the repository.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Directories that are git repos themselves or hold git repos one or two levels down.
    pub repo_roots: Vec<PathBuf>,
    /// Commit author emails that count as "me". Empty: taken from `git config --global user.email`.
    pub author_emails: Vec<String>,
    /// Jira project keys a ticket reference may belong to.
    pub project_keys: Vec<String>,
    /// Project the work in each repo root belongs to (root path → project key); lets the New
    /// ticket window filter work without a ticket key by project.
    pub repo_projects: BTreeMap<String, String>,
    pub workday_minutes: u32,
    /// Local time the first suggested worklog starts at, `HH:MM`.
    pub day_start: String,
    /// Local time `install` schedules the popup for, `HH:MM`.
    pub popup_time: String,
    pub round_minutes: u32,
    /// A gap between two activity events longer than this starts a new work session.
    pub max_gap_minutes: u32,
    /// Credit given to the first event of a session, which has no predecessor to measure from.
    pub session_start_minutes: u32,
    /// ctx's env file, the fallback source of JIRA_URL / JIRA_TOKEN.
    pub ctx_env_file: Option<PathBuf>,
    pub jira_url: Option<String>,
    /// Creating tickets from the app (`[tickets]` in the file).
    pub tickets: TicketConfig,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TicketConfig {
    /// Issue type name new tickets get.
    pub issue_type: String,
    /// Extra Jira fields sent with every new ticket, as Jira's REST JSON (e.g. a select field the
    /// project requires for a Task).
    pub fields: serde_json::Map<String, serde_json::Value>,
    /// Language the model writes ticket titles and descriptions in.
    pub language: String,
    /// opencode's config, the source of the LLM endpoint and key.
    pub opencode_config: Option<PathBuf>,
    /// Provider entry in opencode's config to borrow (`providers.<name>.settings`).
    pub llm_provider: String,
    pub llm_model: String,
    /// Overrides the provider's baseURL.
    pub llm_url: Option<String>,
    /// Send the diffs of the ticked commits and files with them, so the model sees what the work
    /// did rather than guessing from commit subjects; false sends the lines alone.
    pub send_code: bool,
}

impl Default for TicketConfig {
    fn default() -> Self {
        Self {
            issue_type: "Task".into(),
            fields: serde_json::Map::new(),
            language: "Polish".into(),
            opencode_config: dirs::home_dir().map(|h| h.join(".config").join("opencode").join("opencode.json")),
            llm_provider: String::new(),
            llm_model: String::new(),
            llm_url: None,
            send_code: true,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            repo_roots: Vec::new(),
            author_emails: Vec::new(),
            project_keys: Vec::new(),
            repo_projects: BTreeMap::new(),
            workday_minutes: 8 * 60,
            day_start: "09:00".into(),
            popup_time: "17:00".into(),
            round_minutes: 15,
            max_gap_minutes: 120,
            session_start_minutes: 30,
            ctx_env_file: dirs::config_dir().map(|d| d.join("agents-context").join(".env")),
            jira_url: None,
            tickets: TicketConfig::default(),
        }
    }
}

impl Config {
    pub fn default_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("worklog").join("config.toml"))
    }

    /// An explicitly given file must exist (a typo must not silently fall back to defaults);
    /// the default file is optional. The local `.env` fills what the file leaves unset.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let local = match local_env_path() {
            Some(p) => {
                log::debug!("local settings from {}", p.display());
                read_env_file(&p)?
            }
            None => Vec::new(),
        };
        let path = match path {
            Some(p) if !p.exists() => bail!("config file {} does not exist", p.display()),
            Some(p) => Some(p.to_path_buf()),
            None => Self::default_path().filter(|p| p.exists()),
        };
        let text = match &path {
            Some(p) => std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?,
            None => String::new(),
        };
        let what = path.as_ref().map_or_else(|| "the settings".to_owned(), |p| p.display().to_string());
        Self::parse_with(&text, &local).with_context(|| format!("parsing {what}"))
    }

    pub fn parse(text: &str) -> Result<Self> {
        Self::parse_with(text, &[])
    }

    /// `parse` with the local settings `local` (`.env` pairs) filling every key the file does not
    /// set: `WORKLOG_REPO_ROOTS` (';'-separated), `WORKLOG_PROJECT_KEYS` (','-separated),
    /// `WORKLOG_REPO_PROJECTS` ("root=KEY;root=KEY"), `JIRA_URL`, `WORKLOG_LLM_PROVIDER`,
    /// `WORKLOG_LLM_MODEL`, `WORKLOG_TICKET_FIELDS` (a JSON object). Blank values count as unset.
    pub fn parse_with(text: &str, local: &[(String, String)]) -> Result<Self> {
        let mut cfg: Self = toml::from_str(text)?;
        let file = toml::from_str::<toml::Table>(text)?;
        let tickets = file.get("tickets").and_then(toml::Value::as_table);
        let get = |k: &str| local.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.trim()).filter(|v| !v.is_empty());
        let list = |v: &str, sep: char| -> Vec<String> { v.split(sep).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect() };

        if let Some(v) = get("WORKLOG_REPO_ROOTS").filter(|_| !file.contains_key("repo_roots")) {
            cfg.repo_roots = list(v, ';').into_iter().map(PathBuf::from).collect();
        }
        if let Some(v) = get("WORKLOG_PROJECT_KEYS").filter(|_| !file.contains_key("project_keys")) {
            cfg.project_keys = list(v, ',');
        }
        if let Some(v) = get("WORKLOG_REPO_PROJECTS").filter(|_| !file.contains_key("repo_projects")) {
            cfg.repo_projects = list(v, ';')
                .iter()
                .map(|e| match e.rsplit_once('=') {
                    Some((root, key)) if !root.trim().is_empty() && !key.trim().is_empty() => Ok((root.trim().to_owned(), key.trim().to_owned())),
                    _ => bail!("WORKLOG_REPO_PROJECTS: expected root=KEY, got {e:?}"),
                })
                .collect::<Result<_>>()?;
        }
        if let Some(v) = get("JIRA_URL").filter(|_| !file.contains_key("jira_url")) {
            cfg.jira_url = Some(v.to_owned());
        }
        let in_tickets = |k: &str| tickets.is_some_and(|t| t.contains_key(k));
        if let Some(v) = get("WORKLOG_LLM_PROVIDER").filter(|_| !in_tickets("llm_provider")) {
            cfg.tickets.llm_provider = v.to_owned();
        }
        if let Some(v) = get("WORKLOG_LLM_MODEL").filter(|_| !in_tickets("llm_model")) {
            cfg.tickets.llm_model = v.to_owned();
        }
        if let Some(v) = get("WORKLOG_TICKET_FIELDS").filter(|_| !in_tickets("fields")) {
            cfg.tickets.fields = serde_json::from_str(v).context("WORKLOG_TICKET_FIELDS must be a JSON object")?;
        }
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<()> {
        self.day_start_time()?;
        self.popup_time_parsed()?;
        if self.round_minutes == 0 {
            bail!("round_minutes must be at least 1");
        }
        if self.project_keys.is_empty() {
            bail!("no Jira projects: set WORKLOG_PROJECT_KEYS in .env (see .env.example) or project_keys in config.toml");
        }
        let mut roots: Vec<Vec<String>> = Vec::new();
        for (root, key) in &self.repo_projects {
            if !self.project_keys.iter().any(|k| k.eq_ignore_ascii_case(key)) {
                bail!("repo_projects: '{root}' maps to \"{key}\", which is not in project_keys ({})", self.project_keys.join(", "));
            }
            let folded = path_key(Path::new(root));
            if roots.contains(&folded) {
                bail!("repo_projects: '{root}' is listed twice (paths are compared ignoring case)");
            }
            roots.push(folded);
        }
        Ok(())
    }

    /// The project of a repo: the entry of the deepest `repo_projects` root holding it, else the
    /// only configured project. Path components are compared case-insensitively (Windows paths);
    /// the key is returned as spelled in `project_keys`.
    pub fn project_of_repo(&self, repo: &Path) -> Option<String> {
        let repo = path_key(repo);
        self.repo_projects
            .iter()
            .map(|(root, key)| (path_key(Path::new(root)), key))
            .filter(|(root, _)| !root.is_empty() && repo.starts_with(root))
            .max_by_key(|(root, _)| root.len())
            .map(|(_, key)| self.project_keys.iter().find(|k| k.eq_ignore_ascii_case(key)).unwrap_or(key).clone())
            .or_else(|| match self.project_keys.as_slice() {
                [only] => Some(only.clone()),
                _ => None,
            })
    }

    pub fn day_start_time(&self) -> Result<NaiveTime> {
        parse_hhmm(&self.day_start).context("day_start")
    }

    pub fn popup_time_parsed(&self) -> Result<NaiveTime> {
        parse_hhmm(&self.popup_time).context("popup_time")
    }
}

/// The local settings file: `WORKLOG_ENV` when set, else the first `.env` in
/// `<config dir>/worklog/`, then next to the executable or up to three folders above it (the
/// repository's own `.env` for `target/release/worklog.exe`). `None` when there is none.
pub fn local_env_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("WORKLOG_ENV").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let mut candidates: Vec<PathBuf> = dirs::config_dir().map(|d| d.join("worklog").join(".env")).into_iter().collect();
    if let Ok(exe) = std::env::current_exe() {
        candidates.extend(exe.ancestors().skip(1).take(4).map(|d| d.join(".env")));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn path_key(p: &Path) -> Vec<String> {
    p.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect()
}

fn parse_hhmm(s: &str) -> Result<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M").with_context(|| format!("expected HH:MM, got {s:?}"))
}

#[derive(Clone)]
pub struct JiraCredentials {
    pub url: String,
    pub token: String,
}

impl std::fmt::Debug for JiraCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JiraCredentials").field("url", &self.url).field("token", &"<redacted>").finish()
    }
}

/// Resolves Jira credentials: process environment first, then config `jira_url`, then the ctx
/// env file. `env` is injected so tests do not depend on the real environment.
pub fn jira_credentials(cfg: &Config, env: impl Fn(&str) -> Option<String>) -> Result<JiraCredentials> {
    let file = match &cfg.ctx_env_file {
        Some(p) if p.exists() => read_env_file(p)?,
        _ => Vec::new(),
    };
    let from_file = |k: &str| file.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let nonempty = |v: Option<String>| v.filter(|s| !s.trim().is_empty());

    let url = nonempty(env("JIRA_URL"))
        .or_else(|| nonempty(cfg.jira_url.clone()))
        .or_else(|| nonempty(from_file("JIRA_URL")))
        .context("JIRA_URL is not set (environment, config jira_url, or ctx .env)")?;
    let token = nonempty(env("JIRA_TOKEN"))
        .or_else(|| nonempty(from_file("JIRA_TOKEN")))
        .context("JIRA_TOKEN is not set (environment or ctx .env)")?;
    Ok(JiraCredentials { url: url.trim_end_matches('/').to_owned(), token })
}

/// Reads a dotenv file without touching the process environment. Unlike `dotenvy::from_path`,
/// a malformed line is skipped instead of silently dropping every line after it.
pub fn read_env_file(path: &Path) -> Result<Vec<(String, String)>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(text
        .lines()
        .filter_map(|line| dotenvy::from_read_iter(line.as_bytes()).next()?.ok())
        .collect())
}
