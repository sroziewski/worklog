//! WORKLOG_LOG / WORKLOG_STATE / WORKLOG_TICKETS overrides of the default file paths. This file holds a single
//! test, so its process has no other thread reading the environment while it changes.

use std::path::PathBuf;

use worklog::{history, logging, projects, state, ui_settings};

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
    let rows: [Row; 5] = [
        ("WORKLOG_LOG", logging::default_path, "worklog.log", r"C:\sandbox\logs\run-7.log"),
        ("WORKLOG_STATE", state::default_path, "state.json", r"D:\elsewhere\answers-3.json"),
        ("WORKLOG_TICKETS", history::default_path, "tickets.redb", r"E:\portable\my-tickets-9.redb"),
        ("WORKLOG_PROJECTS", projects::default_path, "projects.toml", r"F:\portable\team-projects-4.toml"),
        ("WORKLOG_UI", ui_settings::default_path, "ui.toml", r"G:\portable\window-6.toml"),
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
    assert_eq!(projects::default_path(), Some(config_dir.join("worklog").join("projects.toml")));
    set("WORKLOG_LOG", None);
    set("WORKLOG_TICKETS", Some(r"E:\only\tickets-2.redb"));
    assert_eq!(logging::default_path(), Some(config_dir.join("worklog").join("worklog.log")));
    assert_eq!(state::default_path(), Some(config_dir.join("worklog").join("state.json")));
    assert_eq!(projects::default_path(), Some(config_dir.join("worklog").join("projects.toml")));
    set("WORKLOG_TICKETS", None);
    set("WORKLOG_PROJECTS", Some(r"F:\only\projects-5.toml"));
    assert_eq!(logging::default_path(), Some(config_dir.join("worklog").join("worklog.log")));
    assert_eq!(history::default_path(), Some(config_dir.join("worklog").join("tickets.redb")));
    assert_eq!(ui_settings::default_path(), Some(config_dir.join("worklog").join("ui.toml")));
    set("WORKLOG_PROJECTS", None);
    // The window settings and the projects list are separate files: neither variable moves the other.
    set("WORKLOG_UI", Some(r"G:\only\window-8.toml"));
    assert_eq!(projects::default_path(), Some(config_dir.join("worklog").join("projects.toml")));
    assert_eq!(logging::default_path(), Some(config_dir.join("worklog").join("worklog.log")));
    assert_eq!(state::default_path(), Some(config_dir.join("worklog").join("state.json")));
    assert_eq!(history::default_path(), Some(config_dir.join("worklog").join("tickets.redb")));
    set("WORKLOG_UI", None);
}
