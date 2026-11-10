use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde_json::json;
use worklog::config::{self, Config, JiraCredentials, TicketConfig};

fn chain(e: &anyhow::Error) -> Vec<String> {
    e.chain().map(|c| c.to_string()).collect()
}

fn json_map(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    v.as_object().expect("a JSON object").clone()
}

/// A file must name at least one project to be valid; tests about other keys start with this.
const KEYS: &str = "project_keys = [\"KEYS1\"]\n";

const NO_PROJECTS: &str = "no Jira projects: set WORKLOG_PROJECT_KEYS in .env (see .env.example) or project_keys in config.toml";

mod tickets {
    use super::*;

    /// The defaults the contract names: no extra fields, no provider, no model. The opencode path
    /// is only compared, never read.
    fn default_tickets() -> TicketConfig {
        TicketConfig {
            issue_type: "Task".into(),
            fields: serde_json::Map::new(),
            language: "Polish".into(),
            opencode_config: Some(dirs::home_dir().unwrap().join(".config").join("opencode").join("opencode.json")),
            llm_provider: String::new(),
            llm_model: String::new(),
            llm_url: None,
            send_code: true,
        }
    }

    #[test]
    fn absent_or_empty_table_gives_contract_defaults() {
        for toml in ["", "[tickets]\n"] {
            assert_eq!(Config::parse(&format!("{KEYS}{toml}")).unwrap().tickets, default_tickets(), "{toml:?}");
        }
        assert_eq!(TicketConfig::default(), default_tickets());
        assert_eq!(Config::default().tickets, default_tickets());
    }

    #[test]
    fn each_key_alone_overrides_only_itself() {
        type Row = (&'static str, fn(&mut TicketConfig));
        let rows: Vec<Row> = vec![
            ("issue_type = \"Story\"", |t| t.issue_type = "Story".into()),
            ("language = \"Deutsch\"", |t| t.language = "Deutsch".into()),
            ("opencode_config = 'E:\\cfg\\oc.json'", |t| t.opencode_config = Some(PathBuf::from(r"E:\cfg\oc.json"))),
            ("llm_provider = \"zeta\"", |t| t.llm_provider = "zeta".into()),
            ("llm_model = \"zeta-mini\"", |t| t.llm_model = "zeta-mini".into()),
            ("llm_url = \"http://127.0.0.1:9/v1\"", |t| t.llm_url = Some("http://127.0.0.1:9/v1".into())),
            ("fields = { labels = [\"x\"] }", |t| t.fields = json_map(json!({"labels": ["x"]}))),
            ("send_code = false", |t| t.send_code = false),
        ];
        for (line, change) in rows {
            let mut expected = default_tickets();
            change(&mut expected);
            let c = Config::parse(&format!("round_minutes = 20\n{KEYS}[tickets]\n{line}\n")).unwrap();
            assert_eq!(c.tickets, expected, "{line}");
            assert_eq!(c.round_minutes, 20, "{line}");
        }
    }

    #[test]
    fn fields_arrive_as_equivalent_json() {
        // (name, toml after `[tickets]`, expected `fields`). A given table replaces the default map.
        let rows = [
            (
                "sub-table with nested table, array, numbers and bool",
                "[tickets.fields]\ncustomfield_1 = { value = \"ops\" }\nlabels = [\"a\", \"ż\"]\ncustomfield_2 = 7\ncustomfield_3 = 2.5\ncustomfield_4 = true\n",
                json!({"customfield_1": {"value": "ops"}, "labels": ["a", "ż"], "customfield_2": 7, "customfield_3": 2.5, "customfield_4": true}),
            ),
            (
                "inline table with array of tables",
                "fields = { components = [{ name = \"api\" }, { name = \"web\" }], priority = { name = \"High\" } }\n",
                json!({"components": [{"name": "api"}, {"name": "web"}], "priority": {"name": "High"}}),
            ),
            (
                "deeply nested dotted table",
                "[tickets.fields.customfield_9.parent]\nid = \"42\"\n",
                json!({"customfield_9": {"parent": {"id": "42"}}}),
            ),
        ];
        for (name, toml, expected) in rows {
            let c = Config::parse(&format!("{KEYS}[tickets]\n{toml}")).unwrap();
            assert_eq!(c.tickets.fields, json_map(expected), "{name}");
            assert_eq!(c.tickets.issue_type, "Task", "{name}");
        }
    }

    #[test]
    fn unknown_key_inside_tickets_is_rejected() {
        let e = Config::parse("[tickets]\nlanguage = \"English\"\nprojekt = \"ABC\"\n").unwrap_err();
        // The wording is toml's; the contract fixes only that the key is named.
        assert!(format!("{e:#}").contains("unknown field `projekt`"), "{e:#}");
    }

    #[test]
    fn wrong_types_inside_tickets_are_rejected() {
        for (toml, field) in [("issue_type = 5", "issue_type"), ("fields = \"x\"", "fields"), ("llm_url = []", "llm_url"), ("send_code = \"no\"", "send_code")] {
            let e = Config::parse(&format!("[tickets]\n{toml}\n")).unwrap_err();
            assert!(format!("{e:#}").contains(field), "{toml}: {e:#}");
        }
    }
}

mod parse {
    use super::*;

    #[test]
    fn defaults_name_no_repos_projects_or_llm() {
        let c = Config::default();
        assert_eq!(c.repo_roots, Vec::<PathBuf>::new());
        assert_eq!(c.project_keys, Vec::<String>::new());
        assert_eq!(c.repo_projects, BTreeMap::new());
        assert_eq!((c.tickets.fields.len(), c.tickets.llm_provider.as_str(), c.tickets.llm_model.as_str()), (0, "", ""));
    }

    #[test]
    fn file_naming_only_projects_gives_every_other_default() {
        let c = Config::parse("project_keys = [\"QA\"]").unwrap();
        assert_eq!(c.repo_roots, Vec::<PathBuf>::new());
        assert_eq!(c.author_emails, Vec::<String>::new());
        assert_eq!(c.project_keys, vec!["QA".to_string()]);
        assert_eq!(c.repo_projects, BTreeMap::new());
        assert_eq!(c.workday_minutes, 480);
        assert_eq!(c.day_start, "09:00");
        assert_eq!(c.popup_time, "17:00");
        assert_eq!(c.round_minutes, 15);
        assert_eq!(c.max_gap_minutes, 120);
        assert_eq!(c.session_start_minutes, 30);
        assert_eq!(c.jira_url, None);
        // Only the path is compared; the file is never read here.
        let env = c.ctx_env_file.clone().expect("default ctx env file");
        assert!(env.ends_with(Path::new("agents-context").join(".env")), "{env:?}");
        assert_eq!(c.day_start_time().unwrap(), chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap());
        assert_eq!(c.popup_time_parsed().unwrap(), chrono::NaiveTime::from_hms_opt(17, 0, 0).unwrap());
    }

    #[test]
    fn every_field_is_read() {
        let c = Config::parse(
            r#"
            repo_roots = ['C:\work\a', 'C:\work\b']
            author_emails = ["me@example.test", "me2@example.test"]
            project_keys = ["AAA", "XY9"]
            workday_minutes = 465
            day_start = "08:15"
            popup_time = "16:45"
            round_minutes = 30
            max_gap_minutes = 90
            session_start_minutes = 20
            ctx_env_file = 'C:\tmp\ctx.env'
            jira_url = "https://jira.example.test"

            [repo_projects]
            'C:\work\a' = "AAA"
            'C:\work\b\x' = "XY9"

            [tickets]
            issue_type = "Bug"
            language = "English"
            opencode_config = 'C:\tmp\oc.json'
            llm_provider = "acme"
            llm_model = "acme-large-2"
            llm_url = "https://llm.example.test/v1"
            fields = { priority = { name = "Minor" } }
            send_code = false
            "#,
        )
        .unwrap();
        assert_eq!(
            c,
            Config {
                repo_roots: vec![PathBuf::from(r"C:\work\a"), PathBuf::from(r"C:\work\b")],
                author_emails: vec!["me@example.test".into(), "me2@example.test".into()],
                project_keys: vec!["AAA".into(), "XY9".into()],
                repo_projects: BTreeMap::from([(r"C:\work\a".to_string(), "AAA".to_string()), (r"C:\work\b\x".to_string(), "XY9".to_string())]),
                workday_minutes: 465,
                day_start: "08:15".into(),
                popup_time: "16:45".into(),
                round_minutes: 30,
                max_gap_minutes: 90,
                session_start_minutes: 20,
                ctx_env_file: Some(PathBuf::from(r"C:\tmp\ctx.env")),
                jira_url: Some("https://jira.example.test".into()),
                tickets: TicketConfig {
                    issue_type: "Bug".into(),
                    fields: json_map(json!({"priority": {"name": "Minor"}})),
                    language: "English".into(),
                    opencode_config: Some(PathBuf::from(r"C:\tmp\oc.json")),
                    llm_provider: "acme".into(),
                    llm_model: "acme-large-2".into(),
                    llm_url: Some("https://llm.example.test/v1".into()),
                    send_code: false,
                },
            }
        );
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let c = Config::parse("round_minutes = 30\nproject_keys = [\"ABC\", \"XYZ\"]").unwrap();
        assert_eq!(c.round_minutes, 30);
        assert_eq!(c.workday_minutes, 480);
        assert_eq!(c.day_start, "09:00");
        assert_eq!(c.project_keys, vec!["ABC".to_string(), "XYZ".to_string()]);
        assert_eq!(c.repo_projects, BTreeMap::new());
    }

    #[test]
    fn unknown_field_is_rejected() {
        let e = Config::parse("round_minutes = 30\ncolour = \"blue\"").unwrap_err();
        // The wording is toml's; the contract fixes only that the field is named.
        assert!(e.to_string().contains("unknown field `colour`"), "{e:#}");
    }

    #[test]
    fn invalid_times_are_rejected_with_field_and_value() {
        // (toml, field context, value)
        let cases = [
            ("day_start = \"25:00\"", "day_start", "25:00"),
            ("day_start = \"09-00\"", "day_start", "09-00"),
            ("day_start = \"\"", "day_start", ""),
            ("popup_time = \"17:60\"", "popup_time", "17:60"),
            ("popup_time = \"5pm\"", "popup_time", "5pm"),
        ];
        for (toml, field, value) in cases {
            let e = Config::parse(toml).unwrap_err();
            let c = chain(&e);
            assert_eq!(&c[..2], &[field.to_string(), format!("expected HH:MM, got {value:?}")], "case {toml}");
        }
    }

    #[test]
    fn zero_round_minutes_is_rejected() {
        assert_eq!(Config::parse("round_minutes = 0").unwrap_err().to_string(), "round_minutes must be at least 1");
    }

    #[test]
    fn no_projects_is_rejected_pointing_at_the_env_file() {
        // (name, file)
        for (name, toml) in [("empty file", ""), ("empty list", "project_keys = []"), ("other keys only", "round_minutes = 30\n[tickets]\nlanguage = \"English\"")] {
            let e = Config::parse(toml).unwrap_err();
            assert_eq!(chain(&e), vec![NO_PROJECTS.to_string()], "{name}");
        }
    }

    #[test]
    fn wrong_types_are_rejected() {
        // (toml, offending field). The rest of the message is toml's wording; the contract fixes
        // only that the caller learns which field is wrong.
        for (toml, field) in [
            ("round_minutes = 30\nworkday_minutes = -1", "workday_minutes"),
            ("workday_minutes = \"seven\"", "workday_minutes"),
            ("project_keys = \"ABC\"", "project_keys"),
            ("repo_projects = \"ABC\"", "repo_projects"),
            ("[repo_projects]\n'D:\\dev\\x' = 5", "'D:\\dev\\x' = 5"),
        ] {
            let e = Config::parse(toml).unwrap_err();
            assert!(format!("{e:#}").contains(field), "{toml}: {e:#}");
        }
    }
}

mod project_of_repo {
    use super::*;

    fn cfg(roots: &[(&str, &str)], project_keys: &[&str]) -> Config {
        Config {
            repo_projects: roots.iter().map(|(r, k)| (r.to_string(), k.to_string())).collect(),
            project_keys: project_keys.iter().map(|k| k.to_string()).collect(),
            ctx_env_file: None,
            ..Config::default()
        }
    }

    #[test]
    fn deepest_root_holding_the_repo_component_wise_and_case_insensitive() {
        let c = cfg(
            &[
                (r"D:\dev\alpha", "ABC"),
                (r"D:\dev\xyz\main", "XYZ"),
                (r"D:\dev\help", "HELP"),
                (r"D:\dev\alpha\legacy", "LEG"),
                (r"D:\dev\Żółw", "ZW"),
            ],
            &["ABC", "XYZ"],
        );
        // (name, repo path, expected project)
        let rows: [(&str, &str, Option<&str>); 15] = [
            ("the root itself", r"D:\dev\alpha", Some("ABC")),
            ("child of a root", r"D:\dev\alpha\api", Some("ABC")),
            ("grandchild of a root", r"D:\dev\xyz\main\core\web", Some("XYZ")),
            ("deeper root wins over its parent", r"D:\dev\alpha\legacy\old-api", Some("LEG")),
            ("sibling of the deeper root stays with the parent", r"D:\dev\alpha\legacy2", Some("ABC")),
            ("case differs in every component", r"d:\DEV\ALPHA\API", Some("ABC")),
            ("non-ASCII case differs", r"d:\dev\żÓŁW\x", Some("ZW")),
            ("trailing separator", "D:\\dev\\alpha\\api\\", Some("ABC")),
            ("forward slashes", "D:/dev/alpha/api", Some("ABC")),
            ("root is a partial first component", r"D:\dev\alphaX\api", None),
            ("short root is a partial component of a longer dir", r"D:\dev\helper", None),
            ("short root's own child", r"D:\dev\help\x", Some("HELP")),
            ("parent of a root", r"D:\dev", None),
            ("other drive, same rest", r"E:\dev\alpha\api", None),
            ("unrelated", r"C:\work\svc", None),
        ];
        for (name, repo, expected) in rows {
            assert_eq!(c.project_of_repo(Path::new(repo)).as_deref(), expected, "{name}");
        }
    }

    #[test]
    fn fallback_to_the_only_project_empty_root_and_key_spelling() {
        // Built without `parse`, so validation does not run: a root's value outside project_keys
        // ("ABC" with keys ["AAA"], "EMPTY") tells a root match apart from the only-project fallback.
        // (name, roots, project keys, repo, expected)
        type Row = (&'static str, &'static [(&'static str, &'static str)], &'static [&'static str], &'static str, Option<&'static str>);
        let rows: [Row; 9] = [
            ("only project when no root holds the repo", &[(r"D:\dev\alpha", "ABC")], &["AAA"], r"E:\elsewhere\svc", Some("AAA")),
            ("a root wins over the only project", &[(r"D:\dev\alpha", "ABC")], &["AAA"], r"D:\dev\alpha\api", Some("ABC")),
            ("no root and several projects", &[], &["ABC", "XYZ"], r"D:\dev\alpha\api", None),
            ("no root and one project", &[], &["XY"], r"D:\dev\alpha\api", Some("XY")),
            ("empty root never matches", &[("", "EMPTY")], &["ABC", "XYZ"], r"D:\dev\x", None),
            ("empty root, the only project still applies", &[("", "EMPTY")], &["ONE"], r"D:\dev\x", Some("ONE")),
            ("key returned as spelled in project_keys", &[(r"D:\dev\alpha", "abc")], &["XYZ", "ABC"], r"D:\dev\alpha\api", Some("ABC")),
            ("mixed-case key respelled", &[(r"D:\dev\xyz", "Xyz")], &["ABC", "XYZ"], r"D:\dev\xyz\core", Some("XYZ")),
            ("value outside project_keys returned as written", &[(r"D:\dev\x", "Zz")], &["ABC", "XYZ"], r"D:\dev\x\y", Some("Zz")),
        ];
        for (name, roots, keys, repo, expected) in rows {
            assert_eq!(cfg(roots, keys).project_of_repo(Path::new(repo)).as_deref(), expected, "{name}");
        }
    }

    #[test]
    fn default_config_maps_no_repo() {
        let c = Config::default();
        for repo in [r"D:\dev\alpha", r"D:\dev\xyz\main\frontend", r"D:\dev\my\worklog"] {
            assert_eq!(c.project_of_repo(Path::new(repo)), None, "{repo}");
        }
    }

    #[test]
    fn roots_come_from_the_file_alone() {
        let c = Config::parse("project_keys = [\"SHOP\", \"ABC\"]\n[repo_projects]\n'D:\\work\\shop' = \"SHOP\"\n").unwrap();
        assert_eq!(c.repo_projects, BTreeMap::from([(r"D:\work\shop".to_string(), "SHOP".to_string())]));
        assert_eq!(c.project_of_repo(Path::new(r"D:\work\shop\api")).as_deref(), Some("SHOP"));
        assert_eq!(c.project_of_repo(Path::new(r"D:\dev\alpha\api")), None);
    }
}

mod repo_projects_validation {
    use super::*;

    fn file(keys: &str, entries: &[(&str, &str)]) -> String {
        let mut s = format!("project_keys = {keys}\n[repo_projects]\n");
        for (root, key) in entries {
            s.push_str(&format!("'{root}' = \"{key}\"\n"));
        }
        s
    }

    #[test]
    fn rejects_unknown_projects_and_roots_listed_twice() {
        // Messages filled in by hand: the root as written in the file between single quotes, the
        // key in double quotes, the keys joined by ", " in parentheses. BTreeMap order decides
        // which of two equal roots is reported: the later one ('D' < 'd', '/' < '\', "x" < "x\").
        let rows: [(&str, String, &str); 5] = [
            (
                "project not among the keys",
                file("[\"ABC\", \"XYZ\"]", &[(r"D:\dev\alpha", "ABC"), (r"D:\dev\x", "XY")]),
                r#"repo_projects: 'D:\dev\x' maps to "XY", which is not in project_keys (ABC, XYZ)"#,
            ),
            (
                "roots differing only in case",
                file("[\"ABC\"]", &[(r"D:\Dev\X", "ABC"), (r"d:\dev\x", "ABC")]),
                r"repo_projects: 'd:\dev\x' is listed twice (paths are compared ignoring case)",
            ),
            (
                "roots differing only in a trailing separator",
                file("[\"ABC\"]", &[(r"D:\dev\x", "ABC"), ("D:\\dev\\x\\", "ABC")]),
                r"repo_projects: 'D:\dev\x\' is listed twice (paths are compared ignoring case)",
            ),
            (
                "roots differing only in separators",
                file("[\"ABC\", \"XYZ\"]", &[("D:/dev/x", "ABC"), (r"D:\dev\x", "XYZ")]),
                r"repo_projects: 'D:\dev\x' is listed twice (paths are compared ignoring case)",
            ),
            (
                "same root twice with different projects",
                file("[\"ABC\", \"XYZ\"]", &[(r"D:\dev\X", "ABC"), (r"D:\dev\x", "XYZ")]),
                r"repo_projects: 'D:\dev\x' is listed twice (paths are compared ignoring case)",
            ),
        ];
        for (name, toml, expected) in rows {
            let e = Config::parse(&toml).unwrap_err();
            assert_eq!(chain(&e), vec![expected.to_string()], "{name}");
        }
    }

    #[test]
    fn accepts_keys_in_any_case_and_nested_distinct_roots() {
        let c = Config::parse(&file("[\"ABC\", \"XYZ\"]", &[(r"D:\dev\x", "abc"), (r"D:\dev\x\y", "Xyz")])).unwrap();
        assert_eq!(
            c.repo_projects,
            BTreeMap::from([(r"D:\dev\x".to_string(), "abc".to_string()), (r"D:\dev\x\y".to_string(), "Xyz".to_string())])
        );
        assert_eq!(c.project_of_repo(Path::new(r"D:\dev\x\api")).as_deref(), Some("ABC"));
        assert_eq!(c.project_of_repo(Path::new(r"D:\dev\x\y\api")).as_deref(), Some("XYZ"));
    }

    #[test]
    fn project_keys_alone_give_no_roots() {
        // (project_keys toml, expected project_keys)
        let rows: [(&str, Vec<&str>); 3] = [("[\"AAA\"]", vec!["AAA"]), ("[\"abc\"]", vec!["abc"]), ("[\"XYZ\", \"XY\"]", vec!["XYZ", "XY"])];
        for (keys, expected_keys) in rows {
            let got = Config::parse(&format!("project_keys = {keys}\n")).map(|c| (c.project_keys, c.repo_projects)).map_err(|e| format!("{e:#}"));
            let expected_keys: Vec<String> = expected_keys.into_iter().map(str::to_owned).collect();
            assert_eq!(got, Ok((expected_keys, BTreeMap::new())), "{keys}");
        }
    }
}

/// `Config::parse_with`: the `.env` pairs fill what the file leaves unset. `Config::load` and
/// `local_env_path` read the environment, so they are tested in `tests/local_env.rs` (one test
/// per binary).
mod parse_with {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn parse(toml: &str, pairs: &[(&str, &str)]) -> anyhow::Result<Config> {
        Config::parse_with(toml, &env(pairs))
    }

    /// Every `.env` key at once, each with a value no default has.
    const ALL: &[(&str, &str)] = &[
        ("WORKLOG_REPO_ROOTS", r" D:\dev\alpha ; ;E:\src\xyz;"),
        ("WORKLOG_PROJECT_KEYS", " ABC, ,XYZ ,"),
        ("WORKLOG_REPO_PROJECTS", r" D:\dev\alpha = ABC ;E:\src\xyz=XYZ"),
        ("JIRA_URL", " https://jira.example.org "),
        ("WORKLOG_LLM_PROVIDER", " myllm "),
        ("WORKLOG_LLM_MODEL", " my-model "),
        ("WORKLOG_TICKET_FIELDS", r#"{"customfield_10000": {"value": "development"}, "labels": ["wl"]}"#),
    ];

    #[test]
    fn every_key_fills_an_empty_file_trimmed_and_split() {
        let c = parse("", ALL).unwrap();
        assert_eq!(c.repo_roots, vec![PathBuf::from(r"D:\dev\alpha"), PathBuf::from(r"E:\src\xyz")]);
        assert_eq!(c.project_keys, vec!["ABC".to_string(), "XYZ".to_string()]);
        assert_eq!(
            c.repo_projects,
            BTreeMap::from([(r"D:\dev\alpha".to_string(), "ABC".to_string()), (r"E:\src\xyz".to_string(), "XYZ".to_string())])
        );
        assert_eq!(c.jira_url.as_deref(), Some("https://jira.example.org"));
        assert_eq!((c.tickets.llm_provider.as_str(), c.tickets.llm_model.as_str()), ("myllm", "my-model"));
        assert_eq!(c.tickets.fields, json_map(json!({"customfield_10000": {"value": "development"}, "labels": ["wl"]})));
        // Keys the .env has no say in keep their defaults.
        assert_eq!((c.workday_minutes, c.tickets.language.as_str(), c.tickets.issue_type.as_str()), (480, "Polish", "Task"));
    }

    #[test]
    fn a_key_set_in_the_file_wins_over_the_env() {
        // (name, file, check that the file's value is kept). `ALL` sets every .env key.
        type Row = (&'static str, &'static str, fn(&Config) -> bool);
        let rows: [Row; 7] = [
            ("repo_roots", "repo_roots = ['F:\\own']\n", |c| c.repo_roots == vec![PathBuf::from(r"F:\own")]),
            ("project_keys", "project_keys = [\"ABC\", \"XYZ\", \"OWN\"]\n", |c| c.project_keys == ["ABC", "XYZ", "OWN"]),
            ("repo_projects", "[repo_projects]\n'F:\\own' = \"XYZ\"\n", |c| c.repo_projects == BTreeMap::from([(r"F:\own".to_string(), "XYZ".to_string())])),
            ("jira_url", "jira_url = \"https://own.example.test\"\n", |c| c.jira_url.as_deref() == Some("https://own.example.test")),
            ("llm_provider", "[tickets]\nllm_provider = \"own-provider\"\n", |c| c.tickets.llm_provider == "own-provider"),
            ("llm_model", "[tickets]\nllm_model = \"own-model\"\n", |c| c.tickets.llm_model == "own-model"),
            ("fields", "[tickets.fields]\nown = 1\n", |c| c.tickets.fields == json_map(json!({"own": 1}))),
        ];
        let from_env = parse("", ALL).unwrap();
        for (name, toml, kept) in rows {
            let c = parse(toml, ALL).unwrap();
            assert!(kept(&c), "{name}: {c:?}");
            // Only that key: the rest still comes from the .env.
            let mut expected = c.clone();
            match name {
                "repo_roots" => expected.repo_roots = from_env.repo_roots.clone(),
                "project_keys" => expected.project_keys = from_env.project_keys.clone(),
                "repo_projects" => expected.repo_projects = from_env.repo_projects.clone(),
                "jira_url" => expected.jira_url = from_env.jira_url.clone(),
                "llm_provider" => expected.tickets.llm_provider = from_env.tickets.llm_provider.clone(),
                "llm_model" => expected.tickets.llm_model = from_env.tickets.llm_model.clone(),
                _ => expected.tickets.fields = from_env.tickets.fields.clone(),
            }
            assert_eq!(expected, from_env, "{name}");
        }
    }

    #[test]
    fn a_ticket_key_at_the_top_level_does_not_count_as_set() {
        // `llm_model` belongs under [tickets]; at the top level it is an unknown key, not a value
        // that would keep the .env's model out.
        let e = parse("llm_model = \"own\"\n", ALL).unwrap_err();
        assert!(format!("{e:#}").contains("unknown field `llm_model`"), "{e:#}");
        let c = parse("[tickets]\nlanguage = \"English\"\n", ALL).unwrap();
        assert_eq!((c.tickets.language.as_str(), c.tickets.llm_model.as_str(), c.tickets.llm_provider.as_str()), ("English", "my-model", "myllm"));
    }

    #[test]
    fn blank_values_count_as_unset() {
        let blanks = [
            ("WORKLOG_REPO_ROOTS", "  "),
            ("WORKLOG_REPO_PROJECTS", ""),
            ("JIRA_URL", " \t"),
            ("WORKLOG_LLM_PROVIDER", ""),
            ("WORKLOG_LLM_MODEL", "   "),
            ("WORKLOG_TICKET_FIELDS", " "),
        ];
        let c = parse(KEYS, &blanks).unwrap();
        let defaults = Config { project_keys: vec!["KEYS1".into()], ..Config::default() };
        assert_eq!(c, defaults);
        let e = parse("", &[("WORKLOG_PROJECT_KEYS", "  ")]).unwrap_err();
        assert_eq!(chain(&e), vec![NO_PROJECTS.to_string()]);
    }

    #[test]
    fn separators_and_empty_parts() {
        // (name, WORKLOG_PROJECT_KEYS, expected keys)
        let rows: [(&str, &str, Vec<&str>); 4] = [
            ("one key", "ABC", vec!["ABC"]),
            ("spaces around, empty parts dropped", " ,ABC ,, XYZ, ", vec!["ABC", "XYZ"]),
            ("';' is not a separator here", "ABC;XYZ", vec!["ABC;XYZ"]),
            ("order kept, duplicates kept", "XYZ,ABC,XYZ", vec!["XYZ", "ABC", "XYZ"]),
        ];
        for (name, keys, expected) in rows {
            let c = parse("", &[("WORKLOG_PROJECT_KEYS", keys)]).unwrap();
            assert_eq!(c.project_keys, expected, "{name}");
        }
        // Roots split on ';' only: a ',' stays in the path.
        let c = parse(KEYS, &[("WORKLOG_REPO_ROOTS", r"D:\a,b;;D:\c ")]).unwrap();
        assert_eq!(c.repo_roots, vec![PathBuf::from(r"D:\a,b"), PathBuf::from(r"D:\c")]);
    }

    #[test]
    fn repo_projects_split_on_the_last_equals_sign() {
        let c = parse("", &[("WORKLOG_PROJECT_KEYS", "ABC,XYZ"), ("WORKLOG_REPO_PROJECTS", r"D:\dev\a=b = XYZ ;; D:\dev\c=ABC")]).unwrap();
        assert_eq!(c.repo_projects, BTreeMap::from([(r"D:\dev\a=b".to_string(), "XYZ".to_string()), (r"D:\dev\c".to_string(), "ABC".to_string())]));
    }

    #[test]
    fn malformed_repo_projects_entry_is_an_error_quoting_it() {
        // (name, entry, as quoted in the message: Rust's {:?} of the trimmed entry)
        let rows: [(&str, &str, &str); 5] = [
            ("no equals sign", r"D:\dev\alpha", r#""D:\\dev\\alpha""#),
            ("empty key", r"D:\dev\alpha=", r#""D:\\dev\\alpha=""#),
            ("blank key", r"D:\dev\alpha=  ", r#""D:\\dev\\alpha=""#),
            ("empty root", "=ABC", r#""=ABC""#),
            ("blank root before the last equals sign", " =ABC", r#""=ABC""#),
        ];
        for (name, entry, quoted) in rows {
            let pairs = [("WORKLOG_PROJECT_KEYS", "ABC"), ("WORKLOG_REPO_PROJECTS", entry)];
            let e = parse("", &pairs).unwrap_err();
            assert_eq!(chain(&e), vec![format!("WORKLOG_REPO_PROJECTS: expected root=KEY, got {quoted}")], "{name}");
        }
        // A bad entry after a good one still fails.
        let e = parse("", &[("WORKLOG_PROJECT_KEYS", "ABC"), ("WORKLOG_REPO_PROJECTS", r"D:\a=ABC;D:\b")]).unwrap_err();
        assert_eq!(chain(&e), vec![r#"WORKLOG_REPO_PROJECTS: expected root=KEY, got "D:\\b""#.to_string()]);
    }

    #[test]
    fn ticket_fields_must_be_a_json_object() {
        for (name, value) in [("array", "[1, 2]"), ("string", "\"x\""), ("not JSON", "{customfield_10000: 1}"), ("number", "7")] {
            let e = parse(KEYS, &[("WORKLOG_TICKET_FIELDS", value)]).unwrap_err();
            let c = chain(&e);
            // The cause after the context is serde_json's wording.
            assert_eq!(c[0], "WORKLOG_TICKET_FIELDS must be a JSON object", "{name}: {c:?}");
            assert_eq!(c.len(), 2, "{name}: {c:?}");
        }
        let c = parse(KEYS, &[("WORKLOG_TICKET_FIELDS", "{}")]).unwrap();
        assert_eq!(c.tickets.fields, serde_json::Map::new());
    }

    #[test]
    fn last_occurrence_of_a_repeated_key_wins() {
        let c = parse("", &[("WORKLOG_PROJECT_KEYS", "OLD"), ("JIRA_URL", "https://old.example.test"), ("WORKLOG_PROJECT_KEYS", "NEW1,NEW2")]).unwrap();
        assert_eq!((c.project_keys, c.jira_url.as_deref()), (vec!["NEW1".to_string(), "NEW2".to_string()], Some("https://old.example.test")));
        // A blank last occurrence does not fall back to an earlier one: the key is unset.
        let e = parse("", &[("WORKLOG_PROJECT_KEYS", "OLD"), ("WORKLOG_PROJECT_KEYS", " ")]).unwrap_err();
        assert_eq!(chain(&e), vec![NO_PROJECTS.to_string()]);
    }

    #[test]
    fn unknown_and_lowercase_names_are_ignored() {
        let c = parse(KEYS, &[("worklog_llm_model", "lower"), ("WORKLOG_UNKNOWN", "x"), ("JIRA_TOKEN", "t")]).unwrap();
        assert_eq!(c, Config { project_keys: vec!["KEYS1".into()], ..Config::default() });
    }

    #[test]
    fn validation_runs_after_merging() {
        // .env roots naming a project outside the keys, whichever side the keys came from.
        let pairs = [("WORKLOG_PROJECT_KEYS", "ABC"), ("WORKLOG_REPO_PROJECTS", r"D:\dev\xyz=XYZ")];
        let e = parse("", &pairs).unwrap_err();
        assert_eq!(chain(&e), vec![r#"repo_projects: 'D:\dev\xyz' maps to "XYZ", which is not in project_keys (ABC)"#.to_string()]);
        let e = parse("project_keys = [\"QA\"]\n", &[("WORKLOG_REPO_PROJECTS", r"D:\dev\xyz=XYZ")]).unwrap_err();
        assert_eq!(chain(&e), vec![r#"repo_projects: 'D:\dev\xyz' maps to "XYZ", which is not in project_keys (QA)"#.to_string()]);
        // The file's roots checked against the .env's keys.
        let c = parse("[repo_projects]\n'D:\\dev\\xyz' = \"xyz\"\n", &[("WORKLOG_PROJECT_KEYS", "XYZ")]).unwrap();
        assert_eq!(c.project_of_repo(Path::new(r"D:\dev\xyz\core")).as_deref(), Some("XYZ"));
    }

    #[test]
    fn parse_is_parse_with_no_env() {
        let toml = "project_keys = [\"ABC\"]\nround_minutes = 10\n";
        assert_eq!(Config::parse(toml).unwrap(), parse(toml, &[]).unwrap());
        assert_eq!(chain(&Config::parse("").unwrap_err()), chain(&parse("", &[]).unwrap_err()));
    }
}

mod credentials {
    use super::*;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k: &str| m.get(k).cloned()
    }

    /// Config whose ctx env file is a temp file with `content` (never the real one).
    fn cfg_with_file(dir: &tempfile::TempDir, content: &str, jira_url: Option<&str>) -> Config {
        let p = dir.path().join("ctx.env");
        std::fs::write(&p, content).unwrap();
        Config { ctx_env_file: Some(p), jira_url: jira_url.map(str::to_owned), ..Config::default() }
    }

    const FILE: &str = "JIRA_URL=https://file.example.test\nJIRA_TOKEN=file-token\n";

    fn resolve(cfg: &Config, env: &[(&str, &str)]) -> (String, String) {
        let c = config::jira_credentials(cfg, env_of(env)).unwrap();
        (c.url, c.token)
    }

    #[test]
    fn environment_beats_config_and_file() {
        let d = tempfile::tempdir().unwrap();
        let cfg = cfg_with_file(&d, FILE, Some("https://cfg.example.test"));
        assert_eq!(
            resolve(&cfg, &[("JIRA_URL", "https://env.example.test"), ("JIRA_TOKEN", "env-token")]),
            ("https://env.example.test".into(), "env-token".into())
        );
    }

    #[test]
    fn config_url_beats_file_url_token_from_file() {
        let d = tempfile::tempdir().unwrap();
        let cfg = cfg_with_file(&d, FILE, Some("https://cfg.example.test"));
        assert_eq!(resolve(&cfg, &[]), ("https://cfg.example.test".into(), "file-token".into()));
    }

    #[test]
    fn file_alone_is_enough() {
        let d = tempfile::tempdir().unwrap();
        let cfg = cfg_with_file(&d, FILE, None);
        assert_eq!(resolve(&cfg, &[]), ("https://file.example.test".into(), "file-token".into()));
    }

    #[test]
    fn empty_and_blank_values_fall_through() {
        let d = tempfile::tempdir().unwrap();
        // (env url, cfg url) blank variants
        for (env_url, cfg_url) in [("", ""), ("   ", " \t "), ("", "  ")] {
            let cfg = cfg_with_file(&d, FILE, Some(cfg_url));
            assert_eq!(
                resolve(&cfg, &[("JIRA_URL", env_url), ("JIRA_TOKEN", "  ")]),
                ("https://file.example.test".into(), "file-token".into()),
                "env {env_url:?} cfg {cfg_url:?}"
            );
        }
    }

    #[test]
    fn blank_file_values_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        let cfg = cfg_with_file(&d, "JIRA_URL=\nJIRA_TOKEN=\"  \"\n", None);
        let e = config::jira_credentials(&cfg, env_of(&[])).unwrap_err();
        assert_eq!(e.to_string(), "JIRA_URL is not set (environment, config jira_url, or ctx .env)");
    }

    #[test]
    fn trailing_slashes_are_trimmed_from_every_source() {
        let d = tempfile::tempdir().unwrap();
        let cfg = cfg_with_file(&d, "JIRA_URL=https://file.example.test/\nJIRA_TOKEN=t1\n", Some("https://cfg.example.test//"));
        assert_eq!(resolve(&cfg, &[("JIRA_URL", "https://env.example.test/")]).0, "https://env.example.test");
        assert_eq!(resolve(&cfg, &[]).0, "https://cfg.example.test");
        let cfg = cfg_with_file(&d, "JIRA_URL=https://file.example.test/\nJIRA_TOKEN=t1\n", None);
        assert_eq!(resolve(&cfg, &[]).0, "https://file.example.test");
    }

    #[test]
    fn missing_url_is_an_error() {
        let cfg = Config { ctx_env_file: None, jira_url: None, ..Config::default() };
        let e = config::jira_credentials(&cfg, env_of(&[("JIRA_TOKEN", "t")])).unwrap_err();
        assert_eq!(e.to_string(), "JIRA_URL is not set (environment, config jira_url, or ctx .env)");
    }

    #[test]
    fn missing_token_is_an_error_and_config_has_no_token_source() {
        let d = tempfile::tempdir().unwrap();
        let cfg = cfg_with_file(&d, "JIRA_URL=https://file.example.test\n", Some("https://cfg.example.test"));
        let e = config::jira_credentials(&cfg, env_of(&[])).unwrap_err();
        assert_eq!(e.to_string(), "JIRA_TOKEN is not set (environment or ctx .env)");
    }

    #[test]
    fn nonexistent_env_file_is_treated_as_empty() {
        let d = tempfile::tempdir().unwrap();
        let cfg = Config { ctx_env_file: Some(d.path().join("missing.env")), jira_url: None, ..Config::default() };
        assert_eq!(
            resolve(&cfg, &[("JIRA_URL", "https://env.example.test"), ("JIRA_TOKEN", "env-token")]),
            ("https://env.example.test".into(), "env-token".into())
        );
    }

    #[test]
    fn debug_never_shows_the_token() {
        let c = JiraCredentials { url: "https://jira.example.test".into(), token: "s3cr3t-tok".into() };
        assert_eq!(format!("{c:?}"), r#"JiraCredentials { url: "https://jira.example.test", token: "<redacted>" }"#);
        assert!(!format!("{c:#?}").contains("s3cr3t-tok"));
    }
}

mod env_file {
    use super::*;

    #[test]
    fn malformed_lines_are_skipped_and_later_lines_kept() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".env");
        std::fs::write(
            &p,
            "# comment\n\
             JIRA_URL=https://jira.example.test\n\
             this line is malformed\n\
             JIRA_TOKEN=\"tok en\"\n\
             BROKEN=\"never closed\n\
             SINGLE='single q'\n\
             export EXPORTED=yes\n\
             \n\
             EMPTY=\n\
             LAST=last\n",
        )
        .unwrap();
        let pairs = |v: &[(&str, &str)]| v.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        assert_eq!(
            config::read_env_file(&p).unwrap(),
            pairs(&[
                ("JIRA_URL", "https://jira.example.test"),
                ("JIRA_TOKEN", "tok en"),
                ("SINGLE", "single q"),
                ("EXPORTED", "yes"),
                ("EMPTY", ""),
                ("LAST", "last"),
            ])
        );
    }

    #[test]
    fn crlf_line_endings() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".env");
        std::fs::write(&p, "JIRA_URL=https://jira.example.test\r\nJIRA_TOKEN=abc\r\n").unwrap();
        assert_eq!(
            config::read_env_file(&p).unwrap(),
            vec![("JIRA_URL".to_string(), "https://jira.example.test".to_string()), ("JIRA_TOKEN".to_string(), "abc".to_string())]
        );
    }

    /// How the `.env` must be written: single quotes keep Windows paths and JSON as they are, so
    /// the values reach `parse_with` unchanged.
    #[test]
    fn single_quoted_values_are_kept_literally() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".env");
        std::fs::write(
            &p,
            "WORKLOG_REPO_ROOTS='D:\\dev\\alpha;E:\\new\\tools'\n\
             WORKLOG_REPO_PROJECTS='D:\\dev\\alpha=ABC'\n\
             WORKLOG_TICKET_FIELDS='{\"customfield_10000\":{\"value\":\"development\"}}'\n",
        )
        .unwrap();
        let pairs = config::read_env_file(&p).unwrap();
        assert_eq!(
            pairs,
            vec![
                ("WORKLOG_REPO_ROOTS".to_string(), r"D:\dev\alpha;E:\new\tools".to_string()),
                ("WORKLOG_REPO_PROJECTS".to_string(), r"D:\dev\alpha=ABC".to_string()),
                ("WORKLOG_TICKET_FIELDS".to_string(), r#"{"customfield_10000":{"value":"development"}}"#.to_string()),
            ]
        );
        let c = Config::parse_with("project_keys = [\"ABC\"]\n", &pairs).unwrap();
        assert_eq!(c.repo_roots, vec![PathBuf::from(r"D:\dev\alpha"), PathBuf::from(r"E:\new\tools")]);
        assert_eq!(c.repo_projects, BTreeMap::from([(r"D:\dev\alpha".to_string(), "ABC".to_string())]));
        assert_eq!(c.tickets.fields, json_map(json!({"customfield_10000": {"value": "development"}})));
    }

    #[test]
    fn missing_file_is_an_error_naming_the_path() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("nope.env");
        let e = config::read_env_file(&p).unwrap_err();
        assert_eq!(e.to_string(), format!("reading {}", p.display()));
    }
}
