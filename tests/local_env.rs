//! WORKLOG_ENV and `Config::load` over the local `.env`. This file holds a single test, so its
//! process has no other thread reading the environment while it changes. WORKLOG_ENV always points
//! into a temp dir before `load` runs, so neither the repository's `.env` nor the real config dir
//! is read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use worklog::config::{self, Config};

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

const NO_PROJECTS: &str = "no Jira projects: set WORKLOG_PROJECT_KEYS in .env (see .env.example) or project_keys in config.toml";

#[test]
fn worklog_env_names_the_local_settings_that_load_merges_under_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    // `load` also reads the Projects list; a missing one keeps the real one out and the settings
    // below in charge (`tests/projects_env.rs` covers a list that is there).
    set("WORKLOG_PROJECTS", Some(&tmp.path().join("no-projects.toml")));
    let env_file = tmp.path().join("my settings.env");
    std::fs::write(
        &env_file,
        "WORKLOG_PROJECT_KEYS=ABC,XYZ\n\
         WORKLOG_REPO_PROJECTS='D:\\dev\\alpha=ABC'\n\
         JIRA_URL=https://jira.example.org\n\
         WORKLOG_LLM_MODEL=my-model\n",
    )
    .unwrap();

    // WORKLOG_ENV wins, existing or not; empty counts as unset.
    set("WORKLOG_ENV", Some(&env_file));
    assert_eq!(config::local_env_path(), Some(env_file.clone()));
    let missing = tmp.path().join("missing.env");
    set("WORKLOG_ENV", Some(&missing));
    assert_eq!(config::local_env_path(), Some(missing.clone()));
    set("WORKLOG_ENV", Some(Path::new("")));
    // The fallback depends on the machine (config dir, the executable's folders); only that the
    // empty value is not taken as a path, and that a hit is an existing `.env`.
    let fallback = config::local_env_path();
    assert_ne!(fallback, Some(PathBuf::new()));
    assert!(fallback.as_deref().is_none_or(|p| p.is_file() && p.file_name() == Some(".env".as_ref())), "{fallback:?}");

    let cfg_file = tmp.path().join("config.toml");

    // The .env fills what the file leaves unset; the file's own keys win.
    set("WORKLOG_ENV", Some(&env_file));
    std::fs::write(&cfg_file, "round_minutes = 5\n[tickets]\nllm_model = \"file-model\"\n").unwrap();
    let c = Config::load(Some(&cfg_file)).unwrap();
    assert_eq!(c.round_minutes, 5);
    assert_eq!(c.project_keys, vec!["ABC".to_string(), "XYZ".to_string()]);
    assert_eq!(c.repo_projects, BTreeMap::from([(r"D:\dev\alpha".to_string(), "ABC".to_string())]));
    assert_eq!(c.jira_url.as_deref(), Some("https://jira.example.org"));
    assert_eq!(c.tickets.llm_model, "file-model");

    // Errors after merging name the config file.
    std::fs::write(&cfg_file, "project_keys = [\"QA\"]\n").unwrap();
    let e = Config::load(Some(&cfg_file)).unwrap_err();
    assert_eq!(
        chain(&e),
        vec![format!("parsing {}", cfg_file.display()), r#"repo_projects: 'D:\dev\alpha' maps to "ABC", which is not in project_keys (QA)"#.to_string()]
    );
    std::fs::write(&cfg_file, "round_minutes = 0\n").unwrap();
    let e = Config::load(Some(&cfg_file)).unwrap_err();
    assert_eq!(chain(&e), vec![format!("parsing {}", cfg_file.display()), "round_minutes must be at least 1".to_string()]);

    // An explicit config path must exist.
    let nope = tmp.path().join("nope.toml");
    let e = Config::load(Some(&nope)).unwrap_err();
    assert_eq!(chain(&e), vec![format!("config file {} does not exist", nope.display())]);

    // Without project keys in either place the config is invalid.
    let empty_env = tmp.path().join("empty.env");
    std::fs::write(&empty_env, "").unwrap();
    set("WORKLOG_ENV", Some(&empty_env));
    std::fs::write(&cfg_file, "workday_minutes = 450\n").unwrap();
    let e = Config::load(Some(&cfg_file)).unwrap_err();
    assert_eq!(chain(&e), vec![format!("parsing {}", cfg_file.display()), NO_PROJECTS.to_string()]);

    // A WORKLOG_ENV that names no file is an error naming it, not an empty .env.
    set("WORKLOG_ENV", Some(&missing));
    std::fs::write(&cfg_file, "project_keys = [\"QA\"]\n").unwrap();
    let e = Config::load(Some(&cfg_file)).unwrap_err();
    assert_eq!(chain(&e)[0], format!("reading {}", missing.display()));

    set("WORKLOG_ENV", None);
    set("WORKLOG_PROJECTS", None);
}
