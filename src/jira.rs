//! Jira Data Center REST v2 client, Bearer Personal Access Token.

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone};
use serde_json::{Value, json};

pub struct Jira {
    base: String,
    token: String,
    agent: ureq::Agent,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub key: String,
    pub summary: String,
    pub status: String,
}

/// Jira's `started` format: millis and a numeric offset without a colon (`+0100`).
pub const STARTED_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.3f%z";

pub fn format_started(t: &DateTime<Local>) -> String {
    t.format(STARTED_FORMAT).to_string()
}

pub fn local(t: NaiveDateTime) -> DateTime<Local> {
    // A wall time inside a DST gap does not exist; the same time an hour later does.
    Local
        .from_local_datetime(&t)
        .earliest()
        .or_else(|| Local.from_local_datetime(&(t + chrono::TimeDelta::hours(1))).earliest())
        .unwrap_or_else(|| Local.from_utc_datetime(&t))
}

pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl Jira {
    pub fn new(base: &str, token: &str) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .build()
            .into();
        Self { base: base.trim_end_matches('/').to_owned(), token: token.to_owned(), agent }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn check(path: &str, mut resp: ureq::http::Response<ureq::Body>) -> Result<Value> {
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        match status {
            200..=299 => {
                if text.trim().is_empty() {
                    Ok(Value::Null)
                } else {
                    serde_json::from_str(&text).with_context(|| format!("{path}: response is not JSON"))
                }
            }
            401 | 403 => bail!("Jira refused the token ({status}) on {path}; check JIRA_TOKEN"),
            _ => {
                let detail = serde_json::from_str::<Value>(&text)
                    .ok()
                    .map(|v| {
                        let mut msgs: Vec<String> =
                            v["errorMessages"].as_array().into_iter().flatten().filter_map(|m| m.as_str().map(str::to_owned)).collect();
                        if let Some(errs) = v["errors"].as_object() {
                            msgs.extend(errs.iter().map(|(k, m)| format!("{k}: {}", m.as_str().unwrap_or_default())));
                        }
                        msgs.join("; ")
                    })
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| text.chars().take(200).collect());
                bail!("Jira {status} on {path}: {detail}")
            }
        }
    }

    fn get(&self, path: &str) -> Result<Value> {
        let t = std::time::Instant::now();
        let resp = self
            .agent
            .get(&self.url(path))
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Accept", "application/json")
            .call()
            .map_err(|e| anyhow!("GET {path}: {e}"));
        Self::logged("GET", path, t, resp)
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let t = std::time::Instant::now();
        let resp = self
            .agent
            .post(&self.url(path))
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Accept", "application/json")
            .send_json(body)
            .map_err(|e| anyhow!("POST {path}: {e}"));
        Self::logged("POST", path, t, resp)
    }

    fn put(&self, path: &str, body: &Value) -> Result<Value> {
        let t = std::time::Instant::now();
        let resp = self
            .agent
            .put(&self.url(path))
            .header("Authorization", &format!("Bearer {}", self.token))
            .header("Accept", "application/json")
            .send_json(body)
            .map_err(|e| anyhow!("PUT {path}: {e}"));
        Self::logged("PUT", path, t, resp)
    }

    fn logged(method: &str, path: &str, t: std::time::Instant, resp: Result<ureq::http::Response<ureq::Body>>) -> Result<Value> {
        let resp = resp.inspect_err(|e| log::warn!("{method} {path} failed after {:.2}s: {e:#}", t.elapsed().as_secs_f64()))?;
        let status = resp.status().as_u16();
        let res = Self::check(path, resp);
        log::debug!("{method} {path} -> {status} in {:.2}s", t.elapsed().as_secs_f64());
        if let Err(e) = &res {
            log::warn!("{e:#}");
        }
        res
    }

    /// The user name the token belongs to (`name`, which worklog authors are matched on).
    pub fn myself(&self) -> Result<String> {
        let v = self.get("/rest/api/2/myself")?;
        v["name"].as_str().map(str::to_owned).context("myself: no name in response")
    }

    /// `None` when the issue does not exist or is not visible to the token.
    pub fn issue(&self, key: &str) -> Result<Option<Issue>> {
        let path = format!("/rest/api/2/issue/{}?fields=summary,status", url_encode(key));
        match self.get(&path) {
            Ok(v) => Ok(Some(parse_issue(&v))),
            Err(e) if e.to_string().contains("Jira 404") => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn search(&self, jql: &str, max_results: u32) -> Result<Vec<Issue>> {
        let path = format!("/rest/api/2/search?jql={}&fields=summary,status&maxResults={max_results}", url_encode(jql));
        let v = self.get(&path)?;
        Ok(v["issues"].as_array().into_iter().flatten().map(parse_issue).collect())
    }

    /// The project's display name, e.g. "Alpha" for ABC.
    pub fn project_name(&self, key: &str) -> Result<String> {
        let v = self.get(&format!("/rest/api/2/project/{}", url_encode(key)))?;
        v["name"].as_str().map(str::to_owned).with_context(|| format!("project {key}: no name in response"))
    }

    /// Open issues assigned to me in the given projects, most recently updated first: the
    /// choices offered for activity that names no ticket.
    pub fn candidates(&self, project_keys: &[String], max_results: u32) -> Result<Vec<Issue>> {
        // `project in ()` is a JQL error; no projects simply means no tickets.
        if project_keys.is_empty() {
            return Ok(Vec::new());
        }
        let projects = project_keys.iter().map(|k| format!("\"{k}\"")).collect::<Vec<_>>().join(", ");
        self.search(
            &format!("assignee = currentUser() AND resolution = Unresolved AND project in ({projects}) ORDER BY updated DESC"),
            max_results,
        )
    }

    /// Minutes `me` has already logged on `date` (local), per issue key.
    pub fn logged_on(&self, me: &str, date: NaiveDate) -> Result<Vec<(String, u32)>> {
        let jql = format!("worklogAuthor = currentUser() AND worklogDate = \"{date}\"");
        let mut out = Vec::new();
        for issue in self.search(&jql, 100)? {
            let v = self.get(&format!("/rest/api/2/issue/{}/worklog?maxResults=5000", url_encode(&issue.key)))?;
            let secs: i64 = v["worklogs"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|w| w["author"]["name"].as_str() == Some(me))
                .filter(|w| {
                    w["started"]
                        .as_str()
                        .and_then(|s| DateTime::parse_from_str(s, STARTED_FORMAT).ok())
                        .is_some_and(|t| t.with_timezone(&Local).date_naive() == date)
                })
                .map(|w| w["timeSpentSeconds"].as_i64().unwrap_or(0))
                .sum();
            if secs > 0 {
                out.push((issue.key, u32::try_from(secs / 60).unwrap_or(u32::MAX)));
            }
        }
        Ok(out)
    }

    /// Every worklog of `me` started in `from..=to` (local dates), as (date, issue key, minutes),
    /// one entry per worklog, sorted by date then key.
    pub fn logged_between(&self, me: &str, from: NaiveDate, to: NaiveDate) -> Result<Vec<(NaiveDate, String, u32)>> {
        let jql = format!("worklogAuthor = currentUser() AND worklogDate >= \"{from}\" AND worklogDate <= \"{to}\"");
        let mut out = Vec::new();
        for issue in self.search(&jql, 200)? {
            let v = self.get(&format!("/rest/api/2/issue/{}/worklog?maxResults=5000", url_encode(&issue.key)))?;
            for w in v["worklogs"].as_array().into_iter().flatten() {
                if w["author"]["name"].as_str() != Some(me) {
                    continue;
                }
                let Some(day) = w["started"]
                    .as_str()
                    .and_then(|s| DateTime::parse_from_str(s, STARTED_FORMAT).ok())
                    .map(|t| t.with_timezone(&Local).date_naive())
                else {
                    continue;
                };
                let minutes = u32::try_from(w["timeSpentSeconds"].as_i64().unwrap_or(0) / 60).unwrap_or(0);
                if (from..=to).contains(&day) && minutes > 0 {
                    out.push((day, issue.key.clone(), minutes));
                }
            }
        }
        out.sort();
        Ok(out)
    }

    /// Logs work; returns the new worklog's id.
    pub fn add_worklog(&self, key: &str, started: &DateTime<Local>, minutes: u32, comment: &str) -> Result<String> {
        if minutes == 0 {
            bail!("{key}: refusing to log 0 minutes");
        }
        let body = worklog_body(started, minutes, comment);
        let v = self.post(&format!("/rest/api/2/issue/{}/worklog", url_encode(key)), &body)?;
        Ok(v["id"].as_str().map(str::to_owned).or_else(|| v["id"].as_i64().map(|i| i.to_string())).unwrap_or_default())
    }

    /// Summary, description, status and last update of one issue.
    pub fn issue_details(&self, key: &str) -> Result<IssueDetails> {
        let v = self.get(&format!("/rest/api/2/issue/{}?fields=summary,description,status,updated", url_encode(key)))?;
        Ok(parse_details(&v))
    }

    /// Sets the given fields; `None` leaves a field as it is. Does nothing when both are `None`.
    pub fn update_issue(&self, key: &str, summary: Option<&str>, description: Option<&str>) -> Result<()> {
        let Some(body) = update_body(summary, description) else { return Ok(()) };
        self.put(&format!("/rest/api/2/issue/{}", url_encode(key)), &body)?;
        log::info!("updated {key}: summary {}, description {}", summary.is_some(), description.is_some());
        Ok(())
    }

    /// Creates an issue; returns its key.
    pub fn create_issue(&self, new: &NewIssue) -> Result<String> {
        let v = self.post("/rest/api/2/issue", &create_body(new))?;
        let key = v["key"].as_str().filter(|k| !k.is_empty()).context("Jira created the issue but returned no key")?;
        log::info!("created {key}: {}", new.summary);
        Ok(key.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct NewIssue {
    pub project: String,
    pub issue_type: String,
    pub summary: String,
    pub description: String,
    /// Jira user name; `None` leaves the project's default assignee.
    pub assignee: Option<String>,
    /// Further fields in Jira's REST shape. A field set above wins over the same key here; an
    /// extra `description` or `assignee` only fills in when the one above is blank or `None`.
    pub extra_fields: serde_json::Map<String, Value>,
}

pub fn create_body(n: &NewIssue) -> Value {
    let mut fields = n.extra_fields.clone();
    fields.insert("project".into(), json!({ "key": n.project }));
    fields.insert("issuetype".into(), json!({ "name": n.issue_type }));
    fields.insert("summary".into(), json!(n.summary));
    if !n.description.trim().is_empty() {
        fields.insert("description".into(), json!(n.description));
    }
    if let Some(a) = &n.assignee {
        fields.insert("assignee".into(), json!({ "name": a }));
    }
    json!({ "fields": fields })
}

/// An empty comment is left out rather than sent as `""` (rows added by hand have none).
pub fn worklog_body(started: &DateTime<Local>, minutes: u32, comment: &str) -> Value {
    let mut body = json!({
        "started": format_started(started),
        "timeSpentSeconds": u64::from(minutes) * 60,
    });
    if !comment.trim().is_empty() {
        body["comment"] = Value::from(comment);
    }
    body
}

/// An issue as the ticket editor loads it. `updated` is Jira's timestamp text, compared as is to
/// notice a change made elsewhere since loading.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct IssueDetails {
    pub key: String,
    pub summary: String,
    /// Jira wiki markup; empty when the issue has none.
    pub description: String,
    pub status: String,
    pub updated: String,
}

pub fn parse_details(v: &Value) -> IssueDetails {
    let s = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or_default().to_owned();
    IssueDetails { key: s("/key"), summary: s("/fields/summary"), description: s("/fields/description"), status: s("/fields/status/name"), updated: s("/fields/updated") }
}

/// `{"fields": {...}}` with the fields to change; an empty description is sent as `null`, which
/// clears it (Jira rejects `""` on some setups). `None` when nothing is to change.
pub fn update_body(summary: Option<&str>, description: Option<&str>) -> Option<Value> {
    let mut fields = serde_json::Map::new();
    if let Some(s) = summary {
        fields.insert("summary".into(), json!(s));
    }
    if let Some(d) = description {
        fields.insert("description".into(), if d.trim().is_empty() { Value::Null } else { json!(d) });
    }
    (!fields.is_empty()).then(|| json!({ "fields": fields }))
}

fn parse_issue(v: &Value) -> Issue {
    let s = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or_default().to_owned();
    Issue { key: s("/key"), summary: s("/fields/summary"), status: s("/fields/status/name") }
}

/// The project part of an issue key: `XYZ26-12` → `XYZ26`.
pub fn project_of(key: &str) -> &str {
    key.rsplit_once('-').map_or(key, |(p, _)| p)
}
