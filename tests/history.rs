//! The "My tickets" history: an embedded redb file in a temp dir, never the real config dir.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use redb::{Database, TableDefinition};
use serde_json::json;
use worklog::drafting::WorkItem;
use worklog::history::{self, History, KEEP_DAYS, TicketRecord};

const DAY: i64 = 24 * 3600;
/// 2026-11-10 12:00:00 UTC; `now` of every window test.
const NOW: i64 = 1_794_312_000;
/// Oldest creation time still listed: exactly `KEEP_DAYS` days before `NOW`.
const CUTOFF: i64 = NOW - 7 * DAY;

/// The table as `history` stores it, read here without going through `History`.
const TICKETS: TableDefinition<&str, &str> = TableDefinition::new("tickets");

fn path_in(tmp: &tempfile::TempDir) -> PathBuf {
    tmp.path().join("tickets.redb")
}

fn item(line: &str, change: Option<&str>) -> WorkItem {
    WorkItem { line: line.into(), change: change.map(str::to_owned) }
}

/// Every field distinct per key, so a field taken from another record shows.
fn rec(key: &str, created: i64) -> TicketRecord {
    TicketRecord {
        key: key.into(),
        project: key.split('-').next().unwrap().into(),
        created,
        title: format!("title of {key}"),
        description: format!("h3. Opis\ndescription of {key}"),
        language: format!("lang-{key}"),
        sources: vec![item(&format!("[api] {key} commit"), Some(&format!("diff of {key}\n+x"))), item(&format!("[web] uncommitted {key}.ts"), None)],
    }
}

fn keys(v: &[TicketRecord]) -> Vec<&str> {
    v.iter().map(|r| r.key.as_str()).collect()
}

/// Writes a raw value under `key`, bypassing `History` (the file must not be open elsewhere).
fn put_raw(path: &Path, key: &str, value: &str) {
    let db = Database::create(path).unwrap();
    let txn = db.begin_write().unwrap();
    txn.open_table(TICKETS).unwrap().insert(key, value).unwrap();
    txn.commit().unwrap();
}

fn raw(path: &Path, key: &str) -> Option<String> {
    let db = Database::create(path).unwrap();
    let txn = db.begin_read().unwrap();
    let table = txn.open_table(TICKETS).unwrap();
    table.get(key).unwrap().map(|v| v.value().to_owned())
}

#[test]
fn keep_days_is_a_week() {
    assert_eq!(KEEP_DAYS, 7);
}

#[test]
fn startup_line_names_the_count_the_week_and_the_path() {
    let p = Path::new(r"C:\Users\someone\AppData\Roaming\worklog\my tickets.redb");
    for (count, expected) in [
        (0, r"ticket history: 0 tickets from the last 7 days in C:\Users\someone\AppData\Roaming\worklog\my tickets.redb"),
        (12, r"ticket history: 12 tickets from the last 7 days in C:\Users\someone\AppData\Roaming\worklog\my tickets.redb"),
    ] {
        assert_eq!(history::startup_line(count, p), expected);
    }
}

mod open {
    use super::*;

    #[test]
    fn creates_missing_parent_dirs_and_an_empty_table() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a").join("b c").join("tickets.redb");
        let h = History::open(&p).unwrap();
        assert!(p.is_file());
        assert_eq!(h.recent(NOW).unwrap(), vec![]);
        assert_eq!(h.get("ABC-1").unwrap(), None);
        assert_eq!(h.prune(NOW).unwrap(), 0);
    }

    #[test]
    fn two_handles_on_one_file_both_read_and_write_what_the_other_wrote() {
        let tmp = tempfile::tempdir().unwrap();
        let p = path_in(&tmp);
        let first = History::open(&p).unwrap();
        let second = History::open(&p).unwrap();
        first.record(&rec("ABC-5", NOW - DAY)).unwrap();
        second.record(&rec("XYZ-6", NOW)).unwrap();
        assert_eq!(second.get("ABC-5").unwrap(), Some(rec("ABC-5", NOW - DAY)));
        assert_eq!(first.get("XYZ-6").unwrap(), Some(rec("XYZ-6", NOW)));
        assert!(second.update_text("ABC-5", "edited in the second", "body 2").unwrap());
        let edited = TicketRecord { title: "edited in the second".into(), description: "body 2".into(), ..rec("ABC-5", NOW - DAY) };
        assert_eq!(first.recent(NOW).unwrap(), vec![rec("XYZ-6", NOW), edited.clone()]);
        assert_eq!(second.recent(NOW).unwrap(), vec![rec("XYZ-6", NOW), edited]);
    }

    #[test]
    fn the_file_is_not_held_between_operations() {
        let tmp = tempfile::tempdir().unwrap();
        let p = path_in(&tmp);
        let h = History::open(&p).unwrap();
        h.record(&rec("ABC-3", NOW)).unwrap();
        // `raw` opens the file itself, which redb refuses while any handle holds it.
        let stored: TicketRecord = serde_json::from_str(&raw(&p, "ABC-3").expect("stored under the key")).unwrap();
        assert_eq!(stored, rec("ABC-3", NOW));
        assert_eq!(h.get("ABC-3").unwrap(), Some(rec("ABC-3", NOW)));
    }

    #[test]
    fn path_is_the_one_given() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("sub dir").join("my tickets.redb");
        assert_eq!(History::open(&p).unwrap().path(), p.as_path());
    }

    #[test]
    fn a_path_that_cannot_be_a_file_fails_at_once_naming_it() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("tickets.redb");
        std::fs::create_dir(&p).unwrap();
        let started = Instant::now();
        let e = History::open(&p).err().expect("a directory is not a database file");
        assert_eq!(e.to_string(), format!("opening {}", p.display()));
        assert!(started.elapsed() < Duration::from_secs(2), "not retried as a held file: {:?}", started.elapsed());
    }

    #[test]
    fn parent_that_is_a_file_is_an_error_naming_the_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, "not a dir").unwrap();
        let p = blocker.join("tickets.redb");
        let e = History::open(&p).err().expect("cannot create a dir over a file");
        assert_eq!(e.to_string(), format!("creating {}", blocker.display()));
    }
}

/// Another process (or a raw `Database` in this one) holding the file.
mod held_elsewhere {
    use super::*;

    type Op = fn(&History, &Path) -> anyhow::Result<String>;

    fn summary(r: Option<TicketRecord>) -> String {
        r.map_or_else(|| "none".into(), |r| r.title)
    }

    /// History with ABC-1 created at `NOW`.
    fn with_abc1(p: &Path) -> History {
        let h = History::open(p).unwrap();
        h.record(&rec("ABC-1", NOW)).unwrap();
        h
    }

    #[test]
    fn every_operation_waits_and_succeeds_once_the_file_is_released() {
        // (name, operation, its result summary)
        let rows: [(&str, Op, &str); 6] = [
            ("record", |h, _| {
                h.record(&rec("XYZ-2", NOW - DAY))?;
                Ok(h.get("XYZ-2")?.map_or_else(|| "lost".into(), |r| format!("{} {}", r.key, r.created)))
            }, "XYZ-2 1794225600"),
            ("get", |h, _| Ok(summary(h.get("ABC-1")?)), "title of ABC-1"),
            ("recent", |h, _| Ok(keys(&h.recent(NOW)?).join(",")), "ABC-1"),
            ("update_text", |h, _| Ok(h.update_text("ABC-1", "T2", "D2")?.to_string()), "true"),
            ("prune", |h, _| Ok(h.prune(NOW + 30 * DAY)?.to_string()), "1"),
            ("open", |_, p| Ok(summary(History::open(p)?.get("ABC-1")?)), "title of ABC-1"),
        ];
        for (name, op, expected) in rows {
            let tmp = tempfile::tempdir().unwrap();
            let p = path_in(&tmp);
            let h = with_abc1(&p);
            let holder = Database::create(&p).unwrap();
            let (calling_tx, calling) = std::sync::mpsc::channel();
            let (result, finished, released, took) = std::thread::scope(|s| {
                let worker = s.spawn(|| {
                    calling_tx.send(()).unwrap();
                    let result = op(&h, &p).map_err(|e| format!("{e:#}"));
                    (result, Instant::now())
                });
                calling.recv().unwrap();
                let started = Instant::now();
                std::thread::sleep(Duration::from_millis(300));
                let released = Instant::now();
                drop(holder);
                let (result, finished) = worker.join().unwrap();
                (result, finished, released, started.elapsed())
            });
            assert_eq!(result.as_deref(), Ok(expected), "{name}");
            // The op cannot have finished while the file was still held.
            assert!(finished >= released, "{name}: finished {:?} before the release", released - finished);
            assert!(took >= Duration::from_millis(300), "{name}: result after only {took:?}");
        }
    }

    #[test]
    fn a_write_while_held_past_the_5_s_wait_fails_naming_the_file_and_loses_nothing_earlier() {
        // Slow by design: the full lock wait of 5 s.
        let tmp = tempfile::tempdir().unwrap();
        let p = path_in(&tmp);
        let h = with_abc1(&p);
        let holder = Database::create(&p).unwrap();
        let started = Instant::now();
        let e = h.record(&rec("XYZ-9", NOW)).expect_err("the file stays held");
        let took = started.elapsed();
        drop(holder);
        assert_eq!(e.to_string(), format!("opening {}", p.display()));
        assert!(
            matches!(e.root_cause().downcast_ref::<redb::DatabaseError>(), Some(redb::DatabaseError::DatabaseAlreadyOpen)),
            "{e:#}"
        );
        assert!(took >= Duration::from_secs(5), "gave up after {took:?}");
        assert_eq!(h.recent(NOW).unwrap(), vec![rec("ABC-1", NOW)]);
        assert_eq!(h.get("XYZ-9").unwrap(), None);
    }

    #[test]
    fn concurrent_records_through_two_handles_are_all_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let p = path_in(&tmp);
        let handles = [History::open(&p).unwrap(), History::open(&p).unwrap()];
        std::thread::scope(|s| {
            for t in 0..8_i64 {
                let h = &handles[(t % 2) as usize];
                s.spawn(move || {
                    for n in 0..5_i64 {
                        h.record(&rec(&format!("ABC-{}", t * 10 + n), NOW - (t * 10 + n))).unwrap();
                    }
                });
            }
        });
        let expected: Vec<String> = (0..8).flat_map(|t| (0..5).map(move |n| format!("ABC-{}", t * 10 + n))).collect();
        for h in &handles {
            // Created NOW - number, so newest first is ascending number.
            assert_eq!(keys(&h.recent(NOW).unwrap()), expected.iter().map(String::as_str).collect::<Vec<_>>());
        }
    }

    #[test]
    fn records_from_two_handles_survive_dropping_both_and_reopening() {
        let tmp = tempfile::tempdir().unwrap();
        let p = path_in(&tmp);
        {
            let first = History::open(&p).unwrap();
            let second = History::open(&p).unwrap();
            first.record(&rec("ABC-1", NOW - DAY)).unwrap();
            second.record(&rec("XYZ-2", NOW - 2 * DAY)).unwrap();
        }
        assert_eq!(History::open(&p).unwrap().recent(NOW).unwrap(), vec![rec("ABC-1", NOW - DAY), rec("XYZ-2", NOW - 2 * DAY)]);
    }
}

mod record_and_get {
    use super::*;

    #[test]
    fn roundtrip_keeps_every_field_and_sources_with_and_without_change() {
        let tmp = tempfile::tempdir().unwrap();
        let h = History::open(&path_in(&tmp)).unwrap();
        let r = TicketRecord {
            key: "XYZ-77".into(),
            project: "XYZ".into(),
            created: NOW - 3 * DAY,
            title: "Eksport \"CSV\" dla audytorów ż".into(),
            description: "h3. Opis\nLinia 1\n\nh3. Kryteria akceptacji\n* a\\b".into(),
            language: "Polish".into(),
            sources: vec![item("[api, web] ABC-7 add CSV export", Some("diff --git a/x b/x\n+żółć\n")), item("[svc] uncommitted notes.md", None)],
        };
        h.record(&r).unwrap();
        h.record(&rec("ABC-1", NOW)).unwrap();
        assert_eq!(h.get("XYZ-77").unwrap(), Some(r));
        assert_eq!(h.get("ABC-1").unwrap(), Some(rec("ABC-1", NOW)));
    }

    #[test]
    fn missing_key_is_none_and_keys_are_case_sensitive() {
        let tmp = tempfile::tempdir().unwrap();
        let h = History::open(&path_in(&tmp)).unwrap();
        h.record(&rec("ABC-1", NOW)).unwrap();
        assert_eq!(h.get("ABC-2").unwrap(), None);
        assert_eq!(h.get("abc-1").unwrap(), None);
        assert_eq!(h.get("").unwrap(), None);
    }

    #[test]
    fn same_key_replaces_the_record() {
        let tmp = tempfile::tempdir().unwrap();
        let h = History::open(&path_in(&tmp)).unwrap();
        h.record(&rec("ABC-1", NOW - DAY)).unwrap();
        let replacement = TicketRecord { title: "second".into(), sources: vec![], ..rec("ABC-1", NOW - 2 * DAY) };
        h.record(&replacement).unwrap();
        assert_eq!(h.get("ABC-1").unwrap(), Some(replacement.clone()));
        assert_eq!(h.recent(NOW).unwrap(), vec![replacement]);
    }

    #[test]
    fn stored_value_is_the_record_as_json() {
        let tmp = tempfile::tempdir().unwrap();
        let p = path_in(&tmp);
        {
            let h = History::open(&p).unwrap();
            h.record(&TicketRecord {
                key: "ABC-9".into(),
                project: "ABC".into(),
                created: 1_794_000_000,
                title: "T9".into(),
                description: "D9".into(),
                language: "English".into(),
                sources: vec![item("L1", Some("C1")), item("L2", None)],
            })
            .unwrap();
        }
        let v: serde_json::Value = serde_json::from_str(&raw(&p, "ABC-9").expect("stored under the key")).unwrap();
        assert_eq!(
            v,
            json!({
                "key": "ABC-9", "project": "ABC", "created": 1_794_000_000, "title": "T9", "description": "D9",
                "language": "English", "sources": [{"line": "L1", "change": "C1"}, {"line": "L2", "change": null}]
            })
        );
    }

    #[test]
    fn records_survive_closing_and_reopening_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = path_in(&tmp);
        {
            let h = History::open(&p).unwrap();
            h.record(&rec("ABC-1", NOW - DAY)).unwrap();
            h.record(&rec("XYZ-2", NOW - 2 * DAY)).unwrap();
            h.update_text("ABC-1", "saved title", "saved description").unwrap();
        }
        let h = History::open(&p).unwrap();
        let abc1 = TicketRecord { title: "saved title".into(), description: "saved description".into(), ..rec("ABC-1", NOW - DAY) };
        assert_eq!(h.recent(NOW).unwrap(), vec![abc1, rec("XYZ-2", NOW - 2 * DAY)]);
    }
}

mod update_text {
    use super::*;

    #[test]
    fn existing_record_gets_title_and_description_rest_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let h = History::open(&path_in(&tmp)).unwrap();
        h.record(&rec("ABC-1", NOW - DAY)).unwrap();
        h.record(&rec("ABC-2", NOW - 2 * DAY)).unwrap();
        assert!(h.update_text("ABC-2", "New title 2", "New description 2").unwrap());
        assert_eq!(
            h.get("ABC-2").unwrap(),
            Some(TicketRecord { title: "New title 2".into(), description: "New description 2".into(), ..rec("ABC-2", NOW - 2 * DAY) })
        );
        assert_eq!(h.get("ABC-1").unwrap(), Some(rec("ABC-1", NOW - DAY)));
    }

    #[test]
    fn missing_record_is_false_and_nothing_is_created() {
        let tmp = tempfile::tempdir().unwrap();
        let h = History::open(&path_in(&tmp)).unwrap();
        h.record(&rec("ABC-1", NOW)).unwrap();
        assert!(!h.update_text("ABC-404", "t", "d").unwrap());
        assert_eq!(h.get("ABC-404").unwrap(), None);
        assert_eq!(h.recent(NOW).unwrap(), vec![rec("ABC-1", NOW)]);
    }
}

mod recent {
    use super::*;

    #[test]
    fn last_week_newest_first_cutoff_inclusive() {
        let tmp = tempfile::tempdir().unwrap();
        let h = History::open(&path_in(&tmp)).unwrap();
        // Recorded in an order matching neither creation time nor key.
        for r in [
            rec("ABC-3", CUTOFF),
            rec("ABC-1", CUTOFF - 1),
            rec("XYZ-4", NOW),
            rec("AB-9", NOW - DAY),
            rec("ZZ-1", CUTOFF + 1),
            rec("CD-2", NOW - 30 * DAY),
        ] {
            h.record(&r).unwrap();
        }
        let got = h.recent(NOW).unwrap();
        assert_eq!(got, vec![rec("XYZ-4", NOW), rec("AB-9", NOW - DAY), rec("ZZ-1", CUTOFF + 1), rec("ABC-3", CUTOFF)]);
    }

    #[test]
    fn same_creation_time_is_ordered_by_key() {
        let tmp = tempfile::tempdir().unwrap();
        let h = History::open(&path_in(&tmp)).unwrap();
        for k in ["ABC-9", "AB-3", "ABC-10", "XY-1"] {
            h.record(&rec(k, NOW - 60)).unwrap();
        }
        h.record(&rec("ZZ-5", NOW - 30)).unwrap();
        assert_eq!(keys(&h.recent(NOW).unwrap()), vec!["ZZ-5", "AB-3", "ABC-10", "ABC-9", "XY-1"]);
    }

    #[test]
    fn window_moves_with_now() {
        let tmp = tempfile::tempdir().unwrap();
        let h = History::open(&path_in(&tmp)).unwrap();
        h.record(&rec("ABC-1", NOW - 3 * DAY)).unwrap();
        h.record(&rec("ABC-2", NOW - 6 * DAY)).unwrap();
        // Two days later ABC-2 is 8 days old; recent does not delete it.
        assert_eq!(keys(&h.recent(NOW + 2 * DAY).unwrap()), vec!["ABC-1"]);
        assert_eq!(keys(&h.recent(NOW).unwrap()), vec!["ABC-1", "ABC-2"]);
    }

    #[test]
    fn unreadable_record_is_skipped_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let p = path_in(&tmp);
        {
            let h = History::open(&p).unwrap();
            h.record(&rec("ABC-1", NOW - DAY)).unwrap();
            h.record(&rec("ABC-3", NOW - 2 * DAY)).unwrap();
        }
        put_raw(&p, "ABC-2", "{not json");
        put_raw(&p, "ABC-0", r#"{"key": "ABC-0"}"#);
        let h = History::open(&p).unwrap();
        assert_eq!(h.recent(NOW).unwrap(), vec![rec("ABC-1", NOW - DAY), rec("ABC-3", NOW - 2 * DAY)]);
    }
}

mod ticket_record {
    use super::*;
    use worklog::history;
    use worklog::jira::NewIssue;

    #[test]
    fn project_and_text_from_the_new_issue_key_language_sources_and_time_as_given() {
        let new = NewIssue {
            project: "XYZ".into(),
            issue_type: "Bug".into(),
            summary: "Eksport CSV".into(),
            description: "h3. Opis\nAudytorzy.".into(),
            assignee: Some("jdoe".into()),
            extra_fields: json!({"labels": ["wl"]}).as_object().unwrap().clone(),
        };
        let sources = vec![item("[api] commit one", Some("+a")), item("[web] uncommitted b.ts", None)];
        assert_eq!(
            history::ticket_record("XYZ-31", &new, "English", sources.clone(), NOW - 5),
            TicketRecord {
                key: "XYZ-31".into(),
                project: "XYZ".into(),
                created: NOW - 5,
                title: "Eksport CSV".into(),
                description: "h3. Opis\nAudytorzy.".into(),
                language: "English".into(),
                sources,
            }
        );
    }

    #[test]
    fn project_is_the_issues_not_the_keys_prefix() {
        let new = NewIssue {
            project: "ABC".into(),
            issue_type: "Task".into(),
            summary: "S".into(),
            description: String::new(),
            assignee: None,
            extra_fields: serde_json::Map::new(),
        };
        let r = history::ticket_record("QA-1", &new, "", vec![], 0);
        assert_eq!((r.key.as_str(), r.project.as_str(), r.title.as_str(), r.description.as_str(), r.language.as_str(), r.sources, r.created), ("QA-1", "ABC", "S", "", "", vec![], 0));
    }
}

mod prune {
    use super::*;

    #[test]
    fn deletes_older_and_unreadable_records_and_counts_them() {
        let tmp = tempfile::tempdir().unwrap();
        let p = path_in(&tmp);
        {
            let h = History::open(&p).unwrap();
            for r in [rec("ABC-1", NOW), rec("ABC-2", CUTOFF), rec("ABC-3", CUTOFF - 1), rec("ABC-4", NOW - 400 * DAY), rec("ABC-5", CUTOFF + 1)] {
                h.record(&r).unwrap();
            }
        }
        put_raw(&p, "ABC-6", "garbage");
        let h = History::open(&p).unwrap();
        assert_eq!(h.prune(NOW).unwrap(), 3);
        for (key, kept) in [("ABC-1", true), ("ABC-2", true), ("ABC-3", false), ("ABC-4", false), ("ABC-5", true)] {
            assert_eq!(h.get(key).unwrap().is_some(), kept, "{key}");
        }
        drop(h);
        assert_eq!(raw(&p, "ABC-6"), None);
        let h = History::open(&p).unwrap();
        assert_eq!(keys(&h.recent(NOW).unwrap()), vec!["ABC-1", "ABC-5", "ABC-2"]);
        assert_eq!(h.prune(NOW).unwrap(), 0);
    }

    #[test]
    fn nothing_old_deletes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let h = History::open(&path_in(&tmp)).unwrap();
        h.record(&rec("ABC-1", NOW - DAY)).unwrap();
        h.record(&rec("ABC-2", CUTOFF)).unwrap();
        assert_eq!(h.prune(NOW).unwrap(), 0);
        assert_eq!(keys(&h.recent(NOW).unwrap()), vec!["ABC-1", "ABC-2"]);
    }
}
