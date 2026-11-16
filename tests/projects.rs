//! `projects`: the Projects window's list - checking it, reading and writing `projects.toml` in a
//! temp dir, and the first list built from `config.toml` / `.env`. `default_path` reads the
//! environment, so it is tested in `tests/env_paths.rs` (one test per binary).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use worklog::config::Config;
use worklog::projects::{self, Project};

fn p(name: &str, git_dir: &str, jira_key: &str, enabled: bool) -> Project {
    Project { name: name.into(), git_dir: git_dir.into(), jira_key: jira_key.into(), enabled }
}

fn chain(e: &anyhow::Error) -> Vec<String> {
    e.chain().map(|c| c.to_string()).collect()
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

mod validate {
    use super::*;

    #[test]
    fn accepts_and_normalises_valid_lists() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("not mounted today").display().to_string();
        // (case, list, expected)
        let rows: Vec<(&str, Vec<Project>, Vec<Project>)> = vec![
            (
                "trims every field, uppercases keys, keeps each enabled flag",
                vec![p("  Alpha  ", "  D:\\dev\\alpha  ", " abc ", true), p("Beta\t", "", "xy_9", false), p(" Meetings", "   ", "Meet2", true)],
                vec![p("Alpha", r"D:\dev\alpha", "ABC", true), p("Beta", "", "XY_9", false), p("Meetings", "", "MEET2", true)],
            ),
            (
                "several projects without a folder, and nested folders",
                vec![p("Ops", "", "OPS", true), p("Support", "", "SUP", false), p("Dev", r"D:\dev", "DEV", true), p("Alpha", r"D:\dev\alpha", "ABC", true), p("Calls", " ", "CAL", true)],
                vec![p("Ops", "", "OPS", true), p("Support", "", "SUP", false), p("Dev", r"D:\dev", "DEV", true), p("Alpha", r"D:\dev\alpha", "ABC", true), p("Calls", "", "CAL", true)],
            ),
            ("a folder that does not exist", vec![p("Gone", &missing, "GON", true)], vec![p("Gone", &missing, "GON", true)]),
            (
                "one key in two projects",
                vec![p("Alpha api", r"D:\dev\alpha", "ABC", true), p("Alpha old", r"E:\old\alpha", "abc", false)],
                vec![p("Alpha api", r"D:\dev\alpha", "ABC", true), p("Alpha old", r"E:\old\alpha", "ABC", false)],
            ),
            (
                "folders that only look alike",
                vec![p("Alpha", r"D:\dev\alpha", "ABC", true), p("Alpha 2", r"D:\dev\alpha2", "XYZ", true), p("Alpha E", r"E:\dev\alpha", "QQQ", true)],
                vec![p("Alpha", r"D:\dev\alpha", "ABC", true), p("Alpha 2", r"D:\dev\alpha2", "XYZ", true), p("Alpha E", r"E:\dev\alpha", "QQQ", true)],
            ),
            ("one-letter key", vec![p("Alpha", "", "A", true)], vec![p("Alpha", "", "A", true)]),
            ("letter and digit", vec![p("Alpha", "", "a1", true)], vec![p("Alpha", "", "A1", true)]),
            ("underscore inside", vec![p("Alpha", "", "AB_2", true)], vec![p("Alpha", "", "AB_2", true)]),
            ("underscore last", vec![p("Alpha", "", "q_", true)], vec![p("Alpha", "", "Q_", true)]),
        ];
        for (case, list, expected) in rows {
            assert_eq!(projects::validate(&list).map_err(|e| e.to_string()), Ok(expected), "{case}");
        }
    }

    #[test]
    fn rejects_with_the_first_problem_naming_the_project() {
        let ok = || p("Alpha", r"D:\dev\alpha", "ABC", true);
        let bad_key = |key: &str| format!("Beta: \"{key}\" is not a Jira project key (A-Z, then A-Z, 0-9 or _)");
        // (case, list, expected message)
        let rows: Vec<(&str, Vec<Project>, String)> = vec![
            ("empty list", vec![], "add at least one project".into()),
            ("empty name, 1-based position", vec![ok(), p("Beta", "", "XYZ", true), p("", "", "QQQ", true)], "project 3: the name is empty".into()),
            ("blank name", vec![ok(), p("   ", "", "XYZ", false)], "project 2: the name is empty".into()),
            ("empty key", vec![ok(), p("  Beta ", "", "", true)], "Beta: the Jira key is empty".into()),
            ("blank key", vec![ok(), p("Beta", "", " \t ", true)], "Beta: the Jira key is empty".into()),
            ("key starts with a digit", vec![ok(), p("Beta", "", "1ab", true)], bad_key("1AB")),
            ("key starts with _", vec![ok(), p("Beta", "", "_AB", true)], bad_key("_AB")),
            ("issue key instead of project key", vec![ok(), p("Beta", "", " xyz-12 ", true)], bad_key("XYZ-12")),
            ("space inside the key", vec![ok(), p("Beta", "", "AB C", true)], bad_key("AB C")),
            ("quote in the key", vec![ok(), p("Beta", "", "A\"B", true)], bad_key("A\"B")),
            // Jira keys are ASCII only: other letters are refused, first or later, and only the
            // ASCII ones are upper-cased, so none is folded into an ASCII key.
            ("non-ASCII first letter", vec![ok(), p("Beta", "", "äbc", true)], bad_key("äBC")),
            ("non-ASCII later letter", vec![ok(), p("Beta", "", "AÇ1", true)], bad_key("AÇ1")),
            ("ligature that upper-cases to FI", vec![ok(), p("Beta", "", "ﬁx", true)], bad_key("ﬁX")),
            ("dotless i that upper-cases to I", vec![ok(), p("Beta", "", "ı", true)], bad_key("ı")),
            ("sharp s that upper-cases to SS", vec![ok(), p("Beta", "", "aß", true)], bad_key("Aß")),
            // Precedence inside one project: name, then key, then folder.
            ("no name and no key", vec![ok(), p("", "", "", true)], "project 2: the name is empty".into()),
            ("no key and a used folder", vec![ok(), p("Beta", r"D:\dev\alpha", "", true)], "Beta: the Jira key is empty".into()),
            ("bad key and a used folder", vec![ok(), p("Beta", r"D:\dev\alpha", "9", true)], bad_key("9")),
            // Across projects: the first one in the list wins.
            ("two bad projects", vec![p("Gamma", "", "", true), p("", "", "ABC", true)], "Gamma: the Jira key is empty".into()),
        ];
        for (case, list, expected) in rows {
            let e = projects::validate(&list).unwrap_err();
            assert_eq!(chain(&e), vec![expected], "{case}");
        }
    }

    #[test]
    fn a_folder_used_twice_is_rejected_whatever_its_spelling() {
        // (case, the second project's folder as typed, as named in the message)
        let rows = [
            ("same text", r"D:\dev\alpha", r"D:\dev\alpha"),
            ("other case", r"d:\DEV\Alpha", r"d:\DEV\Alpha"),
            ("trailing separator", "D:\\dev\\alpha\\", "D:\\dev\\alpha\\"),
            ("forward slashes", "D:/dev/alpha", "D:/dev/alpha"),
            ("surrounding blanks", "  D:\\dev\\alpha ", r"D:\dev\alpha"),
        ];
        for (case, typed, shown) in rows {
            // The clash is with the second entry, not the first, and a disabled project counts too.
            let list = [p("Ops", "", "OPS", true), p("Alpha", r"D:\dev\alpha", "ABC", true), p(" Alpha old ", typed, "XYZ", false)];
            let e = projects::validate(&list).unwrap_err();
            assert_eq!(chain(&e), vec![format!("Alpha old: the folder {shown} is already used by Alpha")], "{case}");
        }
    }
}

mod load {
    use super::*;

    #[test]
    fn a_missing_file_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(projects::load(&tmp.path().join("no such dir").join("projects.toml")).unwrap(), None);
    }

    #[test]
    fn reads_every_project_with_the_defaults_and_validates_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("projects.toml");
        std::fs::write(
            &path,
            "[[project]]\nname = \" Alpha \"\ngit_dir = 'D:\\dev\\alpha'\njira_key = \"abc\"\nenabled = false\n\n\
             [[project]]\nname = \"Meetings\"\njira_key = \"MEET\"\n\n\
             [[project]]\nname = \"Beta\"\ngit_dir = 'E:\\src\\beta'\njira_key = \"XYZ\"\nenabled = true\n",
        )
        .unwrap();
        assert_eq!(
            projects::load(&path).unwrap(),
            Some(vec![p("Alpha", r"D:\dev\alpha", "ABC", false), p("Meetings", "", "MEET", true), p("Beta", r"E:\src\beta", "XYZ", true)])
        );
    }

    #[test]
    fn a_file_that_is_not_a_project_list_is_a_parse_error_naming_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("projects.toml");
        // (case, content, a part of toml's message, which the contract does not word)
        let rows = [
            ("unknown project field", "[[project]]\nname = \"A\"\njira_key = \"ABC\"\ncolour = \"blue\"\n", "unknown field `colour`"),
            ("unknown top-level key", "owner = \"me\"\n", "unknown field `owner`"),
            ("missing key field", "[[project]]\nname = \"A\"\n", "missing field `jira_key`"),
            ("missing name field", "[[project]]\njira_key = \"ABC\"\n", "missing field `name`"),
            ("enabled not a bool", "[[project]]\nname = \"A\"\njira_key = \"ABC\"\nenabled = \"yes\"\n", "invalid type"),
            ("not toml", "[[project]\nname = ", "unclosed array table"),
        ];
        for (case, content, part) in rows {
            std::fs::write(&path, content).unwrap();
            let c = chain(&projects::load(&path).unwrap_err());
            assert_eq!(c[0], format!("parsing {}", path.display()), "{case}");
            assert!(c.len() == 2 && c[1].contains(part), "{case}: {c:?}");
        }
    }

    #[test]
    fn a_list_validate_refuses_is_an_error_naming_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("projects.toml");
        // (case, content, validate's message)
        let rows = [
            ("empty file", "", "add at least one project"),
            ("folder twice", "[[project]]\nname = \"Alpha\"\ngit_dir = 'D:\\a'\njira_key = \"ABC\"\n[[project]]\nname = \"Beta\"\ngit_dir = 'd:\\A\\'\njira_key = \"XYZ\"\n", "Beta: the folder d:\\A\\ is already used by Alpha"),
            ("bad key", "[[project]]\nname = \"Alpha\"\njira_key = \"AB-1\"\n", "Alpha: \"AB-1\" is not a Jira project key (A-Z, then A-Z, 0-9 or _)"),
        ];
        for (case, content, message) in rows {
            std::fs::write(&path, content).unwrap();
            let e = projects::load(&path).unwrap_err();
            assert_eq!(chain(&e), vec![format!("checking {}", path.display()), message.to_string()], "{case}");
        }
    }
}

mod save {
    use super::*;

    /// Text TOML must escape (quotes, backslashes, `#`, `=`), a disabled and a folder-less project.
    fn list() -> Vec<Project> {
        vec![
            p("  Ops \"night\" #1 = x ", "", " ops ", true),
            p("Alpha", "  D:\\dev\\alpha beta\\ ", "abc", false),
            p("Zażółć", r"E:\src\xyz", "XYZ", true),
        ]
    }

    fn written() -> Vec<Project> {
        vec![p("Ops \"night\" #1 = x", "", "OPS", true), p("Alpha", "D:\\dev\\alpha beta\\", "ABC", false), p("Zażółć", r"E:\src\xyz", "XYZ", true)]
    }

    #[test]
    fn writes_the_validated_list_which_load_reads_back_and_leaves_no_temp_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("projects.toml");
        assert_eq!(projects::save(&path, &list()).unwrap(), written());
        assert_eq!(projects::load(&path).unwrap(), Some(written()));
        assert_eq!(files_in(tmp.path()), vec!["projects.toml".to_string()]);
    }

    #[test]
    fn creates_the_missing_parent_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg").join("worklog").join("projects.toml");
        projects::save(&path, &list()).unwrap();
        assert_eq!(projects::load(&path).unwrap(), Some(written()));
        assert_eq!(files_in(path.parent().unwrap()), vec!["projects.toml".to_string()]);
    }

    #[test]
    fn replaces_an_earlier_list() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("projects.toml");
        projects::save(&path, &list()).unwrap();
        let next = vec![p("Beta", r"F:\beta", "BET", false)];
        assert_eq!(projects::save(&path, &next).unwrap(), next);
        assert_eq!(projects::load(&path).unwrap(), Some(next));
        assert_eq!(files_in(tmp.path()), vec!["projects.toml".to_string()]);
    }

    #[test]
    fn an_invalid_list_is_not_written_and_the_file_stays_as_it_was() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("projects.toml");
        projects::save(&path, &list()).unwrap();
        let before = std::fs::read(&path).unwrap();
        let e = projects::save(&path, &[p("Alpha", "", "ABC", true), p("Beta", "", "", true)]).unwrap_err();
        assert_eq!(chain(&e), vec!["Beta: the Jira key is empty".to_string()]);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(files_in(tmp.path()), vec!["projects.toml".to_string()]);
    }

    #[test]
    fn a_failed_replace_removes_the_temp_file_and_names_the_target() {
        let tmp = tempfile::tempdir().unwrap();
        // A folder where the file should go: the temp file is written, the rename over it fails.
        let path = tmp.path().join("projects.toml");
        std::fs::create_dir(&path).unwrap();
        let e = projects::save(&path, &list()).unwrap_err();
        // The rest of the chain is the OS's (and its language's) reason.
        assert_eq!(chain(&e)[0], format!("replacing {}", path.display()));
        assert_eq!(chain(&e).len(), 2, "{:?}", chain(&e));
        assert_eq!(files_in(tmp.path()), vec!["projects.toml".to_string()]);
        assert!(path.is_dir());
        assert_eq!(files_in(&path), Vec::<String>::new());
    }

    #[test]
    fn an_invalid_list_creates_no_file_where_there_was_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sub").join("projects.toml");
        let e = projects::save(&path, &[]).unwrap_err();
        assert_eq!(chain(&e), vec!["add at least one project".to_string()]);
        assert_eq!(files_in(tmp.path()), Vec::<String>::new());
    }
}

mod seed {
    use super::*;

    fn cfg(roots: &[&str], keys: &[&str], repo_projects: &[(&str, &str)]) -> Config {
        Config {
            repo_roots: roots.iter().map(PathBuf::from).collect(),
            project_keys: keys.iter().map(|k| k.to_string()).collect(),
            repo_projects: repo_projects.iter().map(|(r, k)| (r.to_string(), k.to_string())).collect::<BTreeMap<_, _>>(),
            ctx_env_file: None,
            ..Config::default()
        }
    }

    #[test]
    fn one_enabled_project_per_mapped_folder_then_other_root_then_unused_key() {
        // (case, cfg, expected)
        let rows: Vec<(&str, Config, Vec<Project>)> = vec![
            (
                "every kind, in that order",
                // alpha is mapped (the root spelled differently), tools sits under it, misc has no mapping.
                cfg(&[r"D:\Dev\Alpha", r"D:\dev\alpha\tools", r"E:\src\misc"], &["ABC", "XYZ", "MEET", "QQQ"], &[(r"D:\dev\alpha", "abc"), (r"F:\work\beta", "XYZ")]),
                vec![
                    // repo_projects in its (sorted) order, the key as project_keys spells it.
                    p("alpha", r"D:\dev\alpha", "ABC", true),
                    p("beta", r"F:\work\beta", "XYZ", true),
                    // Roots no mapping names: the key of the deepest mapped root above, else empty
                    // when several projects are configured.
                    p("tools", r"D:\dev\alpha\tools", "ABC", true),
                    p("misc", r"E:\src\misc", "", true),
                    // Keys no project above uses, named after the key.
                    p("MEET", "", "MEET", true),
                    p("QQQ", "", "QQQ", true),
                ],
            ),
            (
                "roots with the only configured key",
                cfg(&[r"E:\src\misc", r"E:\src\other"], &["ONE"], &[]),
                vec![p("misc", r"E:\src\misc", "ONE", true), p("other", r"E:\src\other", "ONE", true)],
            ),
            (
                "a key a folder uses in another case gets no folder-less project",
                cfg(&[], &["abc", "XYZ"], &[(r"D:\dev\alpha", "ABC")]),
                vec![p("alpha", r"D:\dev\alpha", "abc", true), p("XYZ", "", "XYZ", true)],
            ),
            ("only keys", cfg(&[], &["ABC", "XYZ"], &[]), vec![p("ABC", "", "ABC", true), p("XYZ", "", "XYZ", true)]),
            ("nothing configured", cfg(&[], &[], &[]), vec![]),
        ];
        for (case, c, expected) in rows {
            assert_eq!(projects::seed(&c), expected, "{case}");
        }
    }
}
