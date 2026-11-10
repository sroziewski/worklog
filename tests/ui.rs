//! Pure helpers of the main window against a local tiny_http Jira mock; nothing is drawn.

mod common;

use common::MockJira;
use serde_json::json;
use worklog::config::Config;
use worklog::jira::Jira;
use worklog::ui;

#[test]
fn picker_projects_lists_every_configured_project_in_order_with_name_or_key() {
    let mock = MockJira::start(|_, url, _| match url {
        "/rest/api/2/project/XY" => (200, json!({"key": "XY", "name": "Empty Project"}).to_string()),
        "/rest/api/2/project/ABC" => (200, json!({"key": "ABC", "name": "Alpha"}).to_string()),
        "/rest/api/2/project/XYZ" => (404, json!({"errorMessages": ["No project could be found with key 'XYZ'."]}).to_string()),
        "/rest/api/2/project/AAA" => (500, "boom".into()),
        "/rest/api/2/project/NN" => (200, json!({"key": "NN"}).to_string()),
        _ => (418, String::new()),
    });
    let cfg = Config {
        project_keys: vec!["XY".into(), "ABC".into(), "XYZ".into(), "AAA".into(), "NN".into()],
        ctx_env_file: None,
        jira_url: None,
        ..Config::default()
    };
    let projects = ui::picker_projects(&cfg, &Jira::new(&mock.base, "tok-ui"));
    let pairs = |v: &[(&str, &str)]| v.iter().map(|(k, l)| (k.to_string(), l.to_string())).collect::<Vec<_>>();
    assert_eq!(
        projects,
        pairs(&[("XY", "Empty Project (XY)"), ("ABC", "Alpha (ABC)"), ("XYZ", "XYZ"), ("AAA", "AAA"), ("NN", "NN")])
    );
    let urls: Vec<String> = mock.requests().iter().map(|r| format!("{} {}", r.method, r.url)).collect();
    assert_eq!(
        urls,
        vec![
            "GET /rest/api/2/project/XY",
            "GET /rest/api/2/project/ABC",
            "GET /rest/api/2/project/XYZ",
            "GET /rest/api/2/project/AAA",
            "GET /rest/api/2/project/NN",
        ]
    );
}

#[test]
fn picker_projects_with_one_project() {
    let mock = MockJira::start(|_, _, _| (200, json!({"name": "Alpha"}).to_string()));
    let cfg = Config { project_keys: vec!["ABC".into()], ctx_env_file: None, jira_url: None, ..Config::default() };
    assert_eq!(ui::picker_projects(&cfg, &Jira::new(&mock.base, "tok-ui")), vec![("ABC".to_string(), "Alpha (ABC)".to_string())]);
}
