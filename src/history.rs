//! Tickets this app created, kept for a week with what they were drafted from (the ticked lines
//! and their diffs), so "My tickets" can regenerate them later from the same material. Stored in
//! an embedded redb file, one JSON record per issue key.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::drafting::WorkItem;
use crate::jira::NewIssue;

/// How long a created ticket stays regenerable.
pub const KEEP_DAYS: i64 = 7;

const TICKETS: TableDefinition<&str, &str> = TableDefinition::new("tickets");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TicketRecord {
    pub key: String,
    pub project: String,
    /// Unix seconds of creation.
    pub created: i64,
    /// The ticket's text as created, then as last saved from "My tickets".
    pub title: String,
    pub description: String,
    pub language: String,
    /// What it was drafted from: the ticked lines, with their diffs when they were sent.
    pub sources: Vec<WorkItem>,
}

/// The record of a ticket just created as `key` from `new`, drafted from `sources`.
pub fn ticket_record(key: &str, new: &NewIssue, language: &str, sources: Vec<WorkItem>, now: i64) -> TicketRecord {
    TicketRecord {
        key: key.to_owned(),
        project: new.project.clone(),
        created: now,
        title: new.summary.clone(),
        description: new.description.clone(),
        language: language.to_owned(),
        sources,
    }
}

pub struct History {
    db: Database,
}

/// `WORKLOG_TICKETS` in the environment overrides the file location (tests, portable setups).
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("WORKLOG_TICKETS").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    dirs::config_dir().map(|d| d.join("worklog").join("tickets.redb"))
}

impl History {
    /// Opens or creates the file. Fails while another process has it open: redb locks it.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let db = Database::create(path).with_context(|| format!("opening {}", path.display()))?;
        // The table exists from here on, so readers never meet a missing one.
        let txn = db.begin_write()?;
        txn.open_table(TICKETS)?;
        txn.commit()?;
        Ok(Self { db })
    }

    /// Adds or replaces the record of `r.key`.
    pub fn record(&self, r: &TicketRecord) -> Result<()> {
        let json = serde_json::to_string(r)?;
        let txn = self.db.begin_write()?;
        txn.open_table(TICKETS)?.insert(r.key.as_str(), json.as_str())?;
        txn.commit()?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> Result<Option<TicketRecord>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(TICKETS)?;
        let Some(v) = table.get(key)? else { return Ok(None) };
        Ok(Some(serde_json::from_str(v.value()).with_context(|| format!("record of {key}"))?))
    }

    /// The text a save to Jira left the ticket with; false when there is no record of it.
    pub fn update_text(&self, key: &str, title: &str, description: &str) -> Result<bool> {
        let Some(mut r) = self.get(key)? else { return Ok(false) };
        r.title = title.to_owned();
        r.description = description.to_owned();
        self.record(&r)?;
        Ok(true)
    }

    /// Records created in the last `KEEP_DAYS` days before `now` (unix seconds), newest first.
    /// A record that cannot be read is skipped (logged), not an error for the rest.
    pub fn recent(&self, now: i64) -> Result<Vec<TicketRecord>> {
        let cutoff = now - KEEP_DAYS * 24 * 3600;
        let txn = self.db.begin_read()?;
        let table = txn.open_table(TICKETS)?;
        let mut out = Vec::new();
        for entry in table.iter()? {
            let (k, v) = entry?;
            match serde_json::from_str::<TicketRecord>(v.value()) {
                Ok(r) if r.created >= cutoff => out.push(r),
                Ok(_) => {}
                Err(e) => log::warn!("ticket history: unreadable record {}: {e}", k.value()),
            }
        }
        // Stable, over redb's key order: equal times stay ordered by key.
        out.sort_by(|a, b| b.created.cmp(&a.created));
        Ok(out)
    }

    /// Deletes records older than `KEEP_DAYS` days before `now`, and unreadable ones; returns how
    /// many went.
    pub fn prune(&self, now: i64) -> Result<usize> {
        let cutoff = now - KEEP_DAYS * 24 * 3600;
        let txn = self.db.begin_write()?;
        let removed = {
            let mut table = txn.open_table(TICKETS)?;
            let old: Vec<String> = table
                .iter()?
                .filter_map(|e| e.ok())
                .filter(|(_, v)| serde_json::from_str::<TicketRecord>(v.value()).map_or(true, |r| r.created < cutoff))
                .map(|(k, _)| k.value().to_owned())
                .collect();
            for k in &old {
                table.remove(k.as_str())?;
            }
            old.len()
        };
        txn.commit()?;
        Ok(removed)
    }
}
