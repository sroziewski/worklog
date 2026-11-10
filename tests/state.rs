use std::collections::BTreeMap;

use chrono::{NaiveDate, TimeDelta};
use worklog::state::{Decision, State};

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

#[test]
fn load_of_missing_file_is_empty() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(State::load(&d.path().join("state.json")).days, BTreeMap::new());
}

#[test]
fn load_of_corrupt_file_is_empty() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    std::fs::write(&p, "{not json").unwrap();
    assert_eq!(State::load(&p).days, BTreeMap::new());
}

#[test]
fn record_stores_per_date_and_overwrites_same_date() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("nested").join("dir").join("state.json");
    State::record(&p, day(2026, 11, 4), Decision::Accepted).unwrap();
    State::record(&p, day(2026, 11, 5), Decision::Accepted).unwrap();
    State::record(&p, day(2026, 11, 4), Decision::Denied).unwrap();
    assert_eq!(
        State::load(&p).days,
        BTreeMap::from([(day(2026, 11, 4), Decision::Denied), (day(2026, 11, 5), Decision::Accepted)])
    );
}

#[test]
fn file_format_is_dates_to_lowercase_decisions() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    State::record(&p, day(2026, 11, 4), Decision::Accepted).unwrap();
    State::record(&p, day(2026, 11, 6), Decision::Denied).unwrap();
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(v, serde_json::json!({"days": {"2026-11-04": "accepted", "2026-11-06": "denied"}}));
}

#[test]
fn record_over_corrupt_file_starts_fresh() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    std::fs::write(&p, "garbage").unwrap();
    State::record(&p, day(2026, 11, 4), Decision::Denied).unwrap();
    assert_eq!(State::load(&p).days, BTreeMap::from([(day(2026, 11, 4), Decision::Denied)]));
}

fn record_days(p: &std::path::Path, first: NaiveDate, n: i64) {
    for i in 0..n {
        State::record(p, first + TimeDelta::days(i), Decision::Accepted).unwrap();
    }
}

#[test]
fn exactly_92_days_are_all_kept() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    record_days(&p, day(2026, 8, 1), 92);
    let days = State::load(&p).days;
    assert_eq!(days.len(), 92);
    assert_eq!(days.keys().next(), Some(&day(2026, 8, 1)));
    assert_eq!(days.keys().last(), Some(&(day(2026, 8, 1) + TimeDelta::days(91))));
}

#[test]
fn the_93rd_day_drops_the_oldest() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    record_days(&p, day(2026, 8, 1), 93);
    let days = State::load(&p).days;
    assert_eq!(days.len(), 92);
    assert!(!days.contains_key(&day(2026, 8, 1)));
    assert_eq!(days.keys().next(), Some(&day(2026, 8, 2)));
    assert_eq!(days.keys().last(), Some(&(day(2026, 8, 1) + TimeDelta::days(92))));
}

#[test]
fn recording_an_older_date_into_a_full_state_drops_that_date() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("state.json");
    record_days(&p, day(2026, 8, 1), 92);
    State::record(&p, day(2026, 7, 1), Decision::Denied).unwrap();
    let days = State::load(&p).days;
    assert_eq!(days.len(), 92);
    assert!(!days.contains_key(&day(2026, 7, 1)));
    assert_eq!(days.keys().next(), Some(&day(2026, 8, 1)));
}
