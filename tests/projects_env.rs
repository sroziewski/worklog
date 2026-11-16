//! `Config::load` with the Projects list named by WORKLOG_PROJECTS. This file holds a single test,
//! so its process has no other thread reading the environment while it changes. WORKLOG_ENV and
//! WORKLOG_PROJECTS always point into a temp dir before `load` runs, so neither the repository's
//! `.env` nor the real config dir is read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use worklog::config::Config;
use worklog::projects::{self, Project};

fn set(name: &str, value: Option<&Path>) {
    // SAFETY: the only test in this binary; no other thread reads or writes the environment.
    unsafe {
        match value {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        }
    }
}

fn chain(e: &anyhow::Error) -> Vec<String> {
    e.chain().map(|c| c.to_string()).collect()
}

fn p(name: &str, git_dir: &str, jira_key: &str, enabled: bool) -> Project {
    Project { name: name.into(), git_dir: git_dir.into(), jira_key: jira_key.into(), enabled }
}

#[test]
fn load_applies_the_list_worklog_projects_names_over_the_file_and_env() {
    let tmp = tempfile::tempdir().unwrap();
    let env_file = tmp.path().join("local.env");
    std::fs::write(&env_file, "WORKLOG_PROJECT_KEYS=ENV\nWORKLOG_REPO_ROOTS='C:\\env\\root'\nJIRA_URL=https://jira.example.org\n").unwrap();
    let cfg_file = tmp.path().join("config.toml");
    std::fs::write(&cfg_file, "round_minutes = 25\nproject_keys = [\"FIL\"]\n").unwrap();
    let list_file = tmp.path().join("lists").join("projects.toml");
    set("WORKLOG_ENV", Some(&env_file));
    set("WORKLOG_PROJECTS", Some(&list_file));

    // No list yet: the file and the .env decide.
    let c = Config::load(Some(&cfg_file)).unwrap();
    assert_eq!((c.project_keys.clone(), c.repo_roots.clone(), c.projects.clone()), (vec!["FIL".to_string()], vec![PathBuf::from(r"C:\env\root")], None));

    // A list saved there replaces the repo and project settings; the rest stays.
    let saved = projects::save(&list_file, &[p(" Alpha ", r"D:\dev\alpha", "abc", true), p("Beta", "", "XYZ", false), p("Meetings", "", "MEET", true)]).unwrap();
    let c = Config::load(Some(&cfg_file)).unwrap();
    assert_eq!(c.project_keys, vec!["ABC".to_string(), "MEET".to_string()]);
    assert_eq!(c.repo_roots, vec![PathBuf::from(r"D:\dev\alpha")]);
    assert_eq!(c.repo_projects, BTreeMap::from([(r"D:\dev\alpha".to_string(), "ABC".to_string())]));
    assert_eq!(c.inactive_keys, vec!["XYZ".to_string()]);
    assert_eq!(c.projects, Some(saved));
    assert_eq!((c.round_minutes, c.jira_url.as_deref()), (25, Some("https://jira.example.org")));

    // Every project disabled still loads.
    projects::save(&list_file, &[p("Alpha", r"D:\dev\alpha", "ABC", false)]).unwrap();
    let c = Config::load(Some(&cfg_file)).unwrap();
    assert_eq!((c.project_keys.len(), c.repo_roots.len()), (0, 0));
    assert_eq!(c.inactive_roots, vec![PathBuf::from(r"D:\dev\alpha")]);

    // A list validate refuses, or one that is not TOML, is an error naming the projects file.
    std::fs::write(&list_file, "[[project]]\nname = \"Alpha\"\njira_key = \"ABC\"\n[[project]]\nname = \"Beta\"\njira_key = \" \"\n").unwrap();
    let e = Config::load(Some(&cfg_file)).unwrap_err();
    assert_eq!(chain(&e), vec![format!("checking {}", list_file.display()), "Beta: the Jira key is empty".to_string()]);
    std::fs::write(&list_file, "[[project]]\nname = \"Alpha\"\njira_key = \"ABC\"\nowner = \"me\"\n").unwrap();
    let e = Config::load(Some(&cfg_file)).unwrap_err();
    assert_eq!(chain(&e)[0], format!("parsing {}", list_file.display()));

    set("WORKLOG_PROJECTS", None);
    set("WORKLOG_ENV", None);
}
