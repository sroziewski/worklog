//! The `worklog` binary end to end. Every run gets a sandbox: a temp config whose ctx env file
//! and opencode config do not exist, WORKLOG_LOG / WORKLOG_STATE / WORKLOG_ENV inside the temp dir, and
//! JIRA_URL / JIRA_TOKEN pointing at a local mock - all set on the child only.

mod common;

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::{MockJira, local_at};
use serde_json::json;
use worklog::jira;

fn path_of_url(url: &str) -> &str {
    url.split('?').next().unwrap()
}

struct Sandbox {
    _dir: tempfile::TempDir,
    config: String,
    log: PathBuf,
    state: PathBuf,
    /// An empty `.env` as WORKLOG_ENV, so the binary never finds the repository's own `.env`
    /// next to `target/`.
    local_env: PathBuf,
    /// WORKLOG_PROJECTS: no file unless a test writes one, so the binary never reads the real
    /// Projects list.
    projects: PathBuf,
    /// WORKLOG_UI: no file, so a run that opens the window never reads or writes the real
    /// window settings.
    ui: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        let config = p.join("config.toml");
        std::fs::write(
            &config,
            format!(
                "repo_roots = ['{}']\nproject_keys = [\"ABC\", \"XYZ\"]\nworkday_minutes = 480\nctx_env_file = '{}'\n\n[tickets]\nopencode_config = '{}'\n",
                p.display(),
                p.join("missing-ctx.env").display(),
                p.join("missing-opencode.json").display()
            ),
        )
        .unwrap();
        let local_env = p.join("local.env");
        std::fs::write(&local_env, "").unwrap();
        Self {
            config: config.to_str().unwrap().to_owned(),
            log: p.join("logs").join("worklog.log"),
            state: p.join("state.json"),
            local_env,
            projects: p.join("projects.toml"),
            ui: p.join("ui.toml"),
            _dir: dir,
        }
    }

    /// Runs the binary; kills it after 30 s so a window that opened by mistake cannot hang the
    /// suite.
    fn run(&self, args: &[&str], jira_base: &str) -> (Option<i32>, String, String) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_worklog"))
            .args(args)
            .env("JIRA_URL", jira_base)
            .env("JIRA_TOKEN", "tok-cli")
            .env("WORKLOG_LOG", &self.log)
            .env("WORKLOG_STATE", &self.state)
            .env("WORKLOG_ENV", &self.local_env)
            .env("WORKLOG_PROJECTS", &self.projects)
            .env("WORKLOG_UI", &self.ui)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn worklog");
        let start = Instant::now();
        while child.try_wait().unwrap().is_none() {
            if start.elapsed() > Duration::from_secs(30) {
                let _ = child.kill();
                panic!("worklog {args:?} still running after 30 s (window opened?)");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let out = child.wait_with_output().unwrap();
        (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

/// A Jira that must not be called: every request is recorded and answered 500.
fn dead_jira() -> MockJira {
    MockJira::start(|_, _, _| (500, String::new()))
}

/// Jira for the week Mon 2025-03-10 .. Sun 2025-03-16 (long past, so no line is "today").
/// jdoe: ABC-5 Mon 2h, Tue 8h; XYZ-8 Mon 6h, Wed 2h 30m, Sat 30m. A colleague's Wed hour on ABC-5.
fn week_mock() -> MockJira {
    let at = |d: u32, h: u32| jira::format_started(&local_at(common::day(2025, 3, d), h, 0, 0));
    let wl = |who: &str, d: u32, h: u32, secs: u32| json!({"author": {"name": who}, "started": at(d, h), "timeSpentSeconds": secs});
    let abc5 = json!({"worklogs": [wl("jdoe", 10, 9, 7200), wl("jdoe", 11, 9, 28800), wl("colleague", 12, 9, 3600)]});
    let xyz =json!({"worklogs": [wl("jdoe", 10, 11, 21600), wl("jdoe", 15, 10, 1800), wl("jdoe", 12, 14, 9000)]});
    MockJira::start(move |_, url, _| match path_of_url(url) {
        "/rest/api/2/myself" => (200, json!({"name": "jdoe"}).to_string()),
        "/rest/api/2/search" => (200, json!({"issues": [{"key": "ABC-5", "fields": {}}, {"key": "XYZ-8", "fields": {}}]}).to_string()),
        "/rest/api/2/issue/ABC-5/worklog" => (200, abc5.to_string()),
        "/rest/api/2/issue/XYZ-8/worklog" => (200, xyz.to_string()),
        _ => (404, String::new()),
    })
}

mod parse_errors {
    use super::*;

    #[test]
    fn date_without_check_weekly_is_a_usage_error() {
        let sb = Sandbox::new();
        for args in [&["--date", "2026-11-10"][..], &["--date", "2026-11-10", "config"][..], &["--date", "2026-11-10", "popup"][..]] {
            let jira = dead_jira();
            let (code, stdout, stderr) = sb.run(args, &jira.base);
            // clap's usage-error exit code and its "required arguments" message.
            assert_eq!(code, Some(2), "{args:?}: {stderr}");
            assert_eq!(stdout, "", "{args:?}");
            let head: Vec<&str> = stderr.lines().take(2).collect();
            assert_eq!(head, vec!["error: the following required arguments were not provided:", "  --check-weekly"], "{args:?}: {stderr}");
            assert_eq!(jira.requests().len(), 0, "{args:?}");
        }
    }

    #[test]
    fn check_weekly_conflicts_with_every_subcommand() {
        let sb = Sandbox::new();
        // (args, subcommand named in the error). --config keeps a regression away from the real config.
        let rows: [(&[&str], &str); 5] = [
            (&["--check-weekly", "popup"], "popup"),
            (&["--check-weekly", "suggest", "--date", "2026-11-10"], "suggest"),
            (&["--check-weekly", "install"], "install"),
            (&["--check-weekly", "uninstall"], "uninstall"),
            (&["--check-weekly", "--date", "2026-11-10", "config"], "config"),
        ];
        for (args, sub) in rows {
            let jira = dead_jira();
            let args: Vec<&str> = ["--config", sb.config.as_str()].into_iter().chain(args.iter().copied()).collect();
            let (code, stdout, stderr) = sb.run(&args, &jira.base);
            assert_eq!(code, Some(2), "{args:?}: {stderr}");
            assert_eq!(stdout, "", "{args:?}");
            let expected = format!("error: the subcommand '{sub}' cannot be used with '--check-weekly'");
            assert_eq!(stderr.lines().next(), Some(expected.as_str()), "{args:?}: {stderr}");
            assert_eq!(jira.requests().len(), 0, "{args:?}");
        }
    }

    #[test]
    fn check_weekly_with_a_malformed_date_is_rejected() {
        let sb = Sandbox::new();
        let jira = dead_jira();
        let (code, stdout, stderr) = sb.run(&["--check-weekly", "--date", "2026-13-01"], &jira.base);
        assert_eq!(code, Some(2), "{stderr}");
        assert_eq!(stdout, "");
        // The rest of the line is chrono's parse error text.
        assert!(stderr.starts_with("error: invalid value '2026-13-01' for '--date <DATE>'"), "{stderr}");
    }

    #[test]
    fn help_lists_check_weekly_and_date() {
        let sb = Sandbox::new();
        let jira = dead_jira();
        let (code, stdout, stderr) = sb.run(&["--help"], &jira.base);
        assert_eq!(code, Some(0), "{stderr}");
        let flags: Vec<&str> = stdout.lines().map(str::trim_start).filter(|l| l.starts_with("--check-weekly") || l.starts_with("--date")).collect();
        assert_eq!(flags.len(), 2, "{stdout}");
        assert!(flags[0].starts_with("--check-weekly") && flags[1].starts_with("--date <DATE>"), "{stdout}");
    }
}

mod first_log_line {
    use super::*;

    /// `HH:MM:SS.mmm` the console line starts with, then the rest of the line.
    fn console_parts(line: &str) -> (&str, &str) {
        let (time, rest) = line.split_at(line.char_indices().nth(12).map_or(line.len(), |(i, _)| i));
        assert!(chrono::NaiveTime::parse_from_str(time, "%H:%M:%S%.3f").is_ok() && time.len() == 12, "console time in {line:?}");
        (time, rest)
    }

    /// The file line's message after `YYYY-MM-DD HH:MM:SS.mmm <LEVEL padded to 5> [<pid>] `, and
    /// its level part.
    fn file_parts(line: &str) -> (String, String) {
        let (stamp, rest) = line.split_at(line.char_indices().nth(23).map_or(line.len(), |(i, _)| i));
        assert!(chrono::NaiveDateTime::parse_from_str(stamp, "%Y-%m-%d %H:%M:%S%.3f").is_ok(), "file stamp in {line:?}");
        let (level, after) = rest.split_once('[').unwrap_or_else(|| panic!("no [pid] in {line:?}"));
        let (pid, message) = after.split_once("] ").unwrap_or_else(|| panic!("no [pid] in {line:?}"));
        assert!(!pid.is_empty() && pid.chars().all(|c| c.is_ascii_digit()), "pid in {line:?}");
        (level.to_owned(), message.to_owned())
    }

    #[test]
    fn console_and_file_start_with_the_log_file_at_the_cli_s_level() {
        // (flags, the console's first line names the file). -q shows warnings only; the file
        // always gets everything.
        let rows: [(&[&str], bool); 3] = [(&[], true), (&["-v"], true), (&["-q"], false)];
        for (flags, on_console) in rows {
            let sb = Sandbox::new();
            let jira = dead_jira();
            let args: Vec<&str> = flags.iter().copied().chain(["--config", sb.config.as_str(), "config"]).collect();
            let (code, _, stderr) = sb.run(&args, &jira.base);
            assert_eq!(code, Some(0), "{flags:?}: {stderr}");
            let expected = format!(" INFO  log file: {}", sb.log.display());
            if on_console {
                let first = stderr.lines().next().unwrap_or_else(|| panic!("{flags:?}: no console output"));
                assert_eq!(console_parts(first).1, expected, "{flags:?}: {stderr}");
            } else {
                assert!(stderr.lines().all(|l| !l.contains("log file:")), "{flags:?}: {stderr}");
            }
            let log = std::fs::read_to_string(&sb.log).unwrap_or_else(|e| panic!("{}: {e}", sb.log.display()));
            let first = log.lines().next().unwrap_or_else(|| panic!("{flags:?}: empty log"));
            assert_eq!(file_parts(first), (" INFO  ".to_string(), format!("log file: {}", sb.log.display())), "{flags:?}: {log}");
            assert_eq!(jira.requests().len(), 0, "{flags:?}");
        }
    }

    #[test]
    fn a_run_appends_its_log_file_line_after_earlier_runs() {
        let sb = Sandbox::new();
        std::fs::create_dir_all(sb.log.parent().unwrap()).unwrap();
        std::fs::write(&sb.log, "2026-11-09 08:00:00.000 INFO  [7] an earlier run\n").unwrap();
        let jira = dead_jira();
        let (code, _, stderr) = sb.run(&["--config", &sb.config, "config"], &jira.base);
        assert_eq!(code, Some(0), "{stderr}");
        let log = std::fs::read_to_string(&sb.log).unwrap();
        let lines: Vec<&str> = log.lines().collect();
        assert_eq!(lines[0], "2026-11-09 08:00:00.000 INFO  [7] an earlier run", "{log}");
        assert_eq!(file_parts(lines[1]), (" INFO  ".to_string(), format!("log file: {}", sb.log.display())), "{log}");
    }

    #[test]
    fn an_unopenable_log_file_is_named_first_as_a_warning_even_when_quiet() {
        for flags in [&[][..], &["-q"][..]] {
            let mut sb = Sandbox::new();
            // A directory where the file should be: it exists, but cannot be opened as a file.
            sb.log = sb.local_env.parent().unwrap().join("logdir");
            std::fs::create_dir_all(&sb.log).unwrap();
            let jira = dead_jira();
            let args: Vec<&str> = flags.iter().copied().chain(["--config", sb.config.as_str(), "config"]).collect();
            let (code, stdout, stderr) = sb.run(&args, &jira.base);
            assert_eq!(code, Some(0), "{flags:?}: {stderr}");
            assert_eq!(stdout.lines().next(), Some(format!("# {}", sb.config).as_str()), "{flags:?}: the run goes on: {stdout}");
            let first = stderr.lines().next().unwrap_or_else(|| panic!("{flags:?}: no console output"));
            assert_eq!(
                console_parts(first).1,
                format!(" WARN  log file: {} cannot be opened; logging to the console only", sb.log.display()),
                "{flags:?}: {stderr}"
            );
            assert!(sb.log.is_dir(), "{flags:?}");
        }
    }
}

mod runs {
    use super::*;

    #[test]
    fn no_subcommand_and_no_flag_prints_help() {
        let sb = Sandbox::new();
        let jira = dead_jira();
        let (code, stdout, stderr) = sb.run(&["--config", &sb.config], &jira.base);
        assert_eq!(code, Some(0), "{stderr}");
        // The `about` text, then clap's usage section.
        assert!(stdout.starts_with("Suggests Jira worklogs from the day's git activity\n\nUsage: worklog [OPTIONS] [COMMAND]\n"), "{stdout}");
        assert!(stdout.contains("\n      --check-weekly"), "{stdout}");
        assert_eq!(jira.requests().len(), 0);
    }

    #[test]
    fn check_weekly_prints_the_week_from_jira() {
        let sb = Sandbox::new();
        let mock = week_mock();
        let (code, stdout, stderr) = sb.run(&["--config", &sb.config, "--check-weekly", "--date", "2025-03-12"], &mock.base);
        assert_eq!(code, Some(0), "{stderr}");
        // Mon 2h+6h, Tue 8h, Wed 2h 30m (short 5h 30m), Thu/Fri nothing, Sat 30m, Sun hidden.
        // ABC-5 10h, XYZ-8 9h; total 19h; missing 5h 30m + 8h + 8h = 21h 30m.
        let expected = [
            "Week 2025-03-10 – 2025-03-16 for jdoe",
            "",
            "  Mon 2025-03-10       8h / 8h   ok",
            "  Tue 2025-03-11       8h / 8h   ok",
            "  Wed 2025-03-12   2h 30m / 8h   missing 5h 30m",
            "  Thu 2025-03-13       0m / 8h   missing 8h",
            "  Fri 2025-03-14       0m / 8h   missing 8h",
            "  Sat 2025-03-15      30m / 0m   weekend",
            "",
            "By issue:",
            &format!("  ABC-5{}10h", " ".repeat(9 + 1 + 4)),
            &format!("  XYZ-8{}9h", " ".repeat(9 + 1 + 5)),
            "",
            "Total 19h · target so far 40h (missing 21h 30m) · full week 40h",
            "",
        ]
        .join("\n");
        assert_eq!(stdout, expected, "{stderr}");

        let r = mock.requests();
        assert_eq!(
            common::query_param(&r[1].url, "jql").as_deref(),
            Some(r#"worklogAuthor = currentUser() AND worklogDate >= "2025-03-10" AND worklogDate <= "2025-03-16""#)
        );
        assert!(r.iter().all(|x| x.method == "GET" && x.authorization.as_deref() == Some("Bearer tok-cli")), "{r:?}");
    }

    const FULL_DAY: &str = "2025-03-11: not opening — 8h already logged (workday 8h); use --force to ask anyway\n";

    #[test]
    fn popup_on_a_full_day_stays_closed() {
        let sb = Sandbox::new();
        let mock = week_mock();
        // Tue 2025-03-11: ABC-5 8h; the sandbox state file does not exist, so no stored decision.
        let (code, stdout, stderr) = sb.run(&["popup", "--date", "2025-03-11", "--config", &sb.config], &mock.base);
        assert_eq!(code, Some(0), "{stderr}");
        assert_eq!(stdout, FULL_DAY);
        let paths: Vec<String> = mock.requests().iter().map(|x| path_of_url(&x.url).to_owned()).collect();
        assert_eq!(paths, vec!["/rest/api/2/myself", "/rest/api/2/search", "/rest/api/2/issue/ABC-5/worklog", "/rest/api/2/issue/XYZ-8/worklog"]);
        assert!(!sb.state.exists(), "the gate must not write the state file");
    }

    #[test]
    fn global_options_before_the_subcommand_are_accepted() {
        // --config/--verbose/--quiet are declared `global = true`, so they belong before a
        // subcommand as much as after it.
        let sb = Sandbox::new();
        let cfg = sb.config.as_str();
        for args in [&["--config", cfg, "popup", "--date", "2025-03-11"][..], &["-q", "popup", "--date", "2025-03-11", "--config", cfg][..]] {
            let mock = week_mock();
            let (code, stdout, stderr) = sb.run(args, &mock.base);
            assert_eq!((code, stdout.as_str()), (Some(0), FULL_DAY), "{args:?}: {stderr}");
        }
    }

    #[test]
    fn popup_respects_a_denial_in_the_worklog_state_file() {
        let sb = Sandbox::new();
        std::fs::write(&sb.state, r#"{"days": {"2025-03-11": "denied", "2025-03-12": "accepted"}}"#).unwrap();
        // The denied Tuesday has a full 8h in Jira as well: only the state file can turn the
        // reason into the denial, and a broken WORKLOG_STATE still cannot open a window.
        let mock = week_mock();
        let (code, stdout, stderr) = sb.run(&["popup", "--date", "2025-03-11", "--config", &sb.config], &mock.base);
        assert_eq!(code, Some(0), "{stderr}");
        assert_eq!(stdout, "2025-03-11: not opening — you denied this day; use --force to ask again\n");
    }

    #[test]
    fn the_projects_list_worklog_projects_names_replaces_the_configured_keys() {
        let sb = Sandbox::new();
        std::fs::write(&sb.projects, "[[project]]\nname = \"Quality\"\njira_key = \"qqq\"\n[[project]]\nname = \"Alpha\"\njira_key = \"ABC\"\nenabled = false\n").unwrap();
        let jira = dead_jira();
        let (code, stdout, stderr) = sb.run(&["--config", &sb.config, "config"], &jira.base);
        assert_eq!(code, Some(0), "{stderr}");
        // `config` prints the settings with `{:#?}`; the file's ABC and XYZ are gone.
        assert!(stdout.contains("\n    project_keys: [\n        \"QQQ\",\n    ],\n"), "{stdout}");
        assert!(stdout.contains("\n    inactive_keys: [\n        \"ABC\",\n    ],\n"), "{stdout}");
        assert_eq!(jira.requests().len(), 0);
    }

    #[test]
    fn an_invalid_projects_list_stops_the_run_naming_the_file() {
        let sb = Sandbox::new();
        std::fs::write(&sb.projects, "[[project]]\nname = \"  \"\njira_key = \"ABC\"\n").unwrap();
        let jira = dead_jira();
        let (code, stdout, stderr) = sb.run(&["--config", &sb.config, "config"], &jira.base);
        assert_eq!(code, Some(1), "{stderr}");
        assert_eq!(stdout, "");
        let expected = format!("Error: checking {}\n\nCaused by:\n    project 1: the name is empty\n", sb.projects.display());
        assert!(stderr.ends_with(&expected), "{stderr}");
        assert_eq!(jira.requests().len(), 0);
    }

    #[test]
    fn worklog_log_receives_the_run_s_log_lines() {
        let sb = Sandbox::new();
        let mock = week_mock();
        let (code, stdout, stderr) = sb.run(&["popup", "--date", "2025-03-11", "--config", &sb.config], &mock.base);
        assert_eq!((code, stdout.as_str()), (Some(0), FULL_DAY), "{stderr}");
        let log = std::fs::read_to_string(&sb.log).unwrap_or_else(|e| panic!("{}: {e}", sb.log.display()));
        // File line: "<date> <time> <LEVEL padded to 5> [<pid>] <message>"; report::gate logs the total.
        let gate_lines: Vec<&str> = log.lines().filter(|l| l.ends_with("] 2025-03-11: 8h logged in Jira")).collect();
        assert_eq!(gate_lines.len(), 1, "{log}");
        assert!(gate_lines[0].contains(" INFO  ["), "{log}");
    }
}
