//! What kind of ticket a draft becomes: its issue type, epic, parent (for a sub-task) and the
//! fields Jira requires for that type, read per project from Jira's create metadata.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::jira::{Issue, Jira};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueType {
    pub id: String,
    pub name: String,
    pub subtask: bool,
}

/// How a required field is filled in the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    /// One of the allowed values, sent as `{"value": v}`.
    Select(Vec<String>),
    /// Any of the allowed values, at least one, sent as `[{"value": v}, ...]`.
    MultiSelect(Vec<String>),
    /// Free text, sent as a string.
    Text,
    /// A field the window cannot edit; its Jira schema type, for the message.
    Unsupported(String),
    /// The Epic Link field: filled by the epic picker, required of anything but a sub-task.
    Epic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredField {
    pub id: String,
    pub name: String,
    pub kind: FieldKind,
}

/// A value the user gave a required field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldValue {
    One(String),
    Many(BTreeSet<String>),
    Text(String),
}

/// A draft's ticket kind. An empty `issue_type` stands for the configured default.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Kind {
    pub issue_type: String,
    pub subtask: bool,
    /// Key of the epic; never sent for a sub-task (Jira gives it the parent's).
    pub epic: Option<String>,
    /// Key of the parent issue, as typed; used for a sub-task only.
    pub parent: String,
    /// Required field values by field id.
    pub values: BTreeMap<String, FieldValue>,
}

impl Kind {
    /// The type sent: the chosen one, or `default` when none was chosen.
    pub fn type_name<'a>(&'a self, default: &'a str) -> &'a str {
        if self.issue_type.trim().is_empty() { default } else { &self.issue_type }
    }
}

/// What the window knows of a project to fill in a draft's kind.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProjectMeta {
    /// The types offered, in the configured order.
    pub types: Vec<IssueType>,
    /// Required fields no setting fills, by type name.
    pub required: BTreeMap<String, Vec<RequiredField>>,
    /// Open epics, most recently updated first.
    pub epics: Vec<Issue>,
    /// Open issues a sub-task can hang under, most recently updated first.
    pub parents: Vec<Issue>,
    /// Id of the Epic Link field; `None` when Jira has none (no epics offered then).
    pub epic_field: Option<String>,
    /// What could not be read; the rest still works.
    pub warnings: Vec<String>,
}

/// The types of `createmeta/{project}/issuetypes` (its `values`).
pub fn parse_issue_types(v: &Value) -> Vec<IssueType> {
    v["values"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some(IssueType { id: id_text(&t["id"])?, name: t["name"].as_str()?.to_owned(), subtask: t["subtask"].as_bool().unwrap_or(false) })
        })
        .collect()
}

fn id_text(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned).or_else(|| v.as_i64().map(|i| i.to_string()))
}

/// Fields the window sets itself, or that a sub-task's parent covers.
const HANDLED: [&str; 7] = ["project", "issuetype", "summary", "description", "assignee", "reporter", "parent"];

/// The required fields of `createmeta/{project}/issuetypes/{id}` (its `values`) the user must
/// fill: not set by the window, not in `provided` (the configured extra fields), without a
/// Jira default.
pub fn parse_required_fields(v: &Value, provided: &Map<String, Value>) -> Vec<RequiredField> {
    v["values"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["required"].as_bool() == Some(true) && f["hasDefaultValue"].as_bool() != Some(true))
        .filter_map(|f| {
            let id = f["fieldId"].as_str()?;
            if HANDLED.contains(&id) || provided.contains_key(id) {
                return None;
            }
            let allowed: Vec<String> = f["allowedValues"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| a["value"].as_str().or_else(|| a["name"].as_str()).map(str::to_owned))
                .collect();
            let schema = &f["schema"];
            let kind = match (schema["type"].as_str(), schema["items"].as_str()) {
                _ if schema["custom"].as_str() == Some(EPIC_LINK_TYPE) => FieldKind::Epic,
                (Some("option"), _) => FieldKind::Select(allowed),
                (Some("array"), Some("option")) => FieldKind::MultiSelect(allowed),
                (Some("string"), _) => FieldKind::Text,
                (t, items) => FieldKind::Unsupported(match (t, items) {
                    (Some(t), Some(i)) => format!("{t} of {i}"),
                    (Some(t), None) => t.to_owned(),
                    _ => "unknown".to_owned(),
                }),
            };
            Some(RequiredField { id: id.to_owned(), name: f["name"].as_str().unwrap_or(id).to_owned(), kind })
        })
        .collect()
}

const EPIC_LINK_TYPE: &str = "com.pyxis.greenhopper.jira:gh-epic-link";

/// The Epic Link field of `/rest/api/2/field`, found by its type, not its name.
pub fn epic_link_field(v: &Value) -> Option<String> {
    v.as_array()?
        .iter()
        .find(|f| f["schema"]["custom"].as_str() == Some(EPIC_LINK_TYPE))
        .and_then(|f| f["id"].as_str().map(str::to_owned))
}

/// The types of `all` named in `wanted` (case-insensitive), in `wanted` order, `default` first
/// when the project has it; an epic is never offered (it is not work to log).
pub fn offered_types(all: &[IssueType], wanted: &[String], default: &str) -> Vec<IssueType> {
    let mut out: Vec<IssueType> = Vec::new();
    for name in std::iter::once(default).chain(wanted.iter().map(String::as_str)).map(str::trim) {
        if name.eq_ignore_ascii_case("epic") {
            continue;
        }
        if let Some(t) = all.iter().find(|t| t.name.eq_ignore_ascii_case(name.trim()))
            && !out.iter().any(|o| o.id == t.id)
        {
            out.push(t.clone());
        }
    }
    out
}

/// The offer when Jira's types cannot be read: the configured names, a name with "sub" in it
/// taken for a sub-task.
pub fn fallback_types(wanted: &[String], default: &str) -> Vec<IssueType> {
    let mut out: Vec<IssueType> = Vec::new();
    for name in std::iter::once(default).chain(wanted.iter().map(String::as_str)).map(str::trim) {
        if name.is_empty() || name.eq_ignore_ascii_case("epic") || out.iter().any(|t| t.name.eq_ignore_ascii_case(name)) {
            continue;
        }
        out.push(IssueType { id: String::new(), name: name.to_owned(), subtask: name.to_lowercase().contains("sub") });
    }
    out
}

/// Whether `key` is an issue key of `project` (`HD-12` of HD).
pub fn key_in_project(key: &str, project: &str) -> bool {
    match key.trim().rsplit_once('-') {
        Some((p, n)) => p.eq_ignore_ascii_case(project) && !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()),
        None => false,
    }
}

/// What keeps a draft of `project` with `kind` from being created, as the window says it; empty
/// when nothing does. `required` are the fields its type needs.
pub fn blockers(kind: &Kind, project: &str, required: &[RequiredField]) -> Vec<String> {
    let mut out = Vec::new();
    if kind.subtask {
        let parent = kind.parent.trim();
        if parent.is_empty() {
            out.push("pick a parent".to_owned());
        } else if !key_in_project(parent, project) {
            out.push(format!("the parent must be an issue of {project}"));
        }
    }
    for f in required {
        let filled = match (kind.values.get(&f.id), &f.kind) {
            (_, FieldKind::Unsupported(t)) => {
                out.push(format!("{} ({t}) cannot be set here; add it to WORKLOG_TICKET_FIELDS", f.name));
                continue;
            }
            // A sub-task takes its parent's epic.
            (_, FieldKind::Epic) if kind.subtask => continue,
            (_, FieldKind::Epic) => {
                if kind.epic.is_none() {
                    out.push("pick an epic".to_owned());
                }
                continue;
            }
            (Some(FieldValue::One(v)), FieldKind::Select(allowed)) => allowed.contains(v),
            (Some(FieldValue::Many(vs)), FieldKind::MultiSelect(allowed)) => !vs.is_empty() && vs.iter().all(|v| allowed.contains(v)),
            (Some(FieldValue::Text(t)), FieldKind::Text) => !t.trim().is_empty(),
            _ => false,
        };
        if !filled {
            out.push(f.name.clone());
        }
    }
    out
}

/// The fields `kind` adds to the configured extra fields: the parent of a sub-task, the epic of
/// anything else (when Jira has an Epic Link field) and the required field values.
pub fn kind_fields(kind: &Kind, epic_field: Option<&str>, required: &[RequiredField]) -> Map<String, Value> {
    let mut out = Map::new();
    if kind.subtask {
        out.insert("parent".into(), json!({ "key": kind.parent.trim() }));
    } else if let (Some(field), Some(epic)) = (epic_field, &kind.epic) {
        out.insert(field.to_owned(), json!(epic));
    }
    for f in required {
        if f.kind == FieldKind::Epic {
            if let (false, Some(epic)) = (kind.subtask, &kind.epic) {
                out.insert(f.id.clone(), json!(epic));
            }
            continue;
        }
        let value = match kind.values.get(&f.id) {
            Some(FieldValue::One(v)) => json!({ "value": v }),
            Some(FieldValue::Many(vs)) => Value::Array(vs.iter().map(|v| json!({ "value": v })).collect()),
            Some(FieldValue::Text(t)) => json!(t.trim()),
            None => continue,
        };
        out.insert(f.id.clone(), value);
    }
    out
}

/// The issues whose key or summary contains `query` (case-insensitive); all of them for a blank one.
pub fn filter_issues<'a>(issues: &'a [Issue], query: &str) -> Vec<&'a Issue> {
    let q = query.trim().to_lowercase();
    issues.iter().filter(|i| q.is_empty() || i.key.to_lowercase().contains(&q) || i.summary.to_lowercase().contains(&q)).collect()
}

/// The parents offered: tickets created in the window first, then Jira's, each key once.
pub fn parent_choices(created: &[Issue], jira: &[Issue]) -> Vec<Issue> {
    let mut out: Vec<Issue> = Vec::new();
    for i in created.iter().chain(jira) {
        if !out.iter().any(|o| o.key == i.key) {
            out.push(i.clone());
        }
    }
    out
}

/// "2 Task, 1 Bug": how many drafts of each type, in order of first appearance.
pub fn type_counts<'a>(types: impl IntoIterator<Item = &'a str>) -> String {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for t in types {
        match counts.iter_mut().find(|(n, _)| *n == t) {
            Some((_, c)) => *c += 1,
            None => counts.push((t, 1)),
        }
    }
    counts.iter().map(|(t, c)| format!("{c} {t}")).collect::<Vec<_>>().join(", ")
}

/// Reads what the window needs of `project`: the offered types and their required fields, the
/// open epics and the issues a sub-task can go under. Only the types failing is an error; any
/// other part that fails is a warning and is left empty.
pub fn load(jira: &Jira, project: &str, wanted: &[String], default: &str, provided: &Map<String, Value>) -> Result<ProjectMeta> {
    let all = jira.issue_types(project)?;
    let types = offered_types(&all, wanted, default);
    let mut meta = ProjectMeta { types, ..ProjectMeta::default() };
    for t in &meta.types {
        match jira.create_fields(project, &t.id) {
            Ok(v) => {
                meta.required.insert(t.name.clone(), parse_required_fields(&v, provided));
            }
            Err(e) => meta.warnings.push(format!("fields of {}: {e:#}", t.name)),
        }
    }
    match jira.fields() {
        Ok(v) => meta.epic_field = epic_link_field(&v),
        Err(e) => meta.warnings.push(format!("the Epic Link field: {e:#}")),
    }
    if meta.epic_field.is_some() {
        match jira.epics(project, 100) {
            Ok(e) => meta.epics = e,
            Err(e) => meta.warnings.push(format!("epics: {e:#}")),
        }
    }
    if meta.types.iter().any(|t| t.subtask) {
        match jira.parent_candidates(project, 50) {
            Ok(p) => meta.parents = p,
            Err(e) => meta.warnings.push(format!("parent issues: {e:#}")),
        }
    }
    Ok(meta)
}
