//! WORKLOG_LOG / WORKLOG_STATE / WORKLOG_TICKETS overrides of the default file paths. This file holds a single
//! test, so its process has no other thread reading the environment while it changes.

use std::path::PathBuf;

use worklog::{history, logging, state};

fn set(name: &str, value: Option<&str>) {
    // SAFETY: the only test in this binary; no other thread reads or writes the environment.
    unsafe {
        match value {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }
}

#[test]
fn env_var_overrides_the_config_dir_default_unless_unset_or_empty() {
    let config_dir = dirs::config_dir().expect("config dir");
    // (variable, function, default file name, override)
    type Row = (&'static str, fn() -> Option<PathBuf>, &'static str, &'static str);
    let rows: [Row; 3] = [
        ("WORKLOG_LOG", logging::default_path, "worklog.log", r"C:\sandbox\logs\run-7.log"),
        ("WORKLOG_STATE", state::default_path, "state.json", r"D:\elsewhere\answers-3.json"),
        ("WORKLOG_TICKETS", history::default_path, "tickets.redb", r"E:\portable\my-tickets-9.redb"),
    ];
    for (var, default_path, file, custom) in rows {
        let fallback = Some(config_dir.join("worklog").join(file));
        set(var, Some(custom));
        assert_eq!(default_path(), Some(PathBuf::from(custom)), "{var} set");
        set(var, None);
        assert_eq!(default_path(), fallback, "{var} unset");
        set(var, Some(""));
        assert_eq!(default_path(), fallback, "{var} empty");
    }
    // Each variable steers only its own path.
    set("WORKLOG_LOG", Some(r"C:\only\the.log"));
    assert_eq!(state::default_path(), Some(config_dir.join("worklog").join("state.json")));
    assert_eq!(history::default_path(), Some(config_dir.join("worklog").join("tickets.redb")));
    set("WORKLOG_LOG", None);
    set("WORKLOG_TICKETS", Some(r"E:\only\tickets-2.redb"));
    assert_eq!(logging::default_path(), Some(config_dir.join("worklog").join("worklog.log")));
    assert_eq!(state::default_path(), Some(config_dir.join("worklog").join("state.json")));
    set("WORKLOG_TICKETS", None);
}
