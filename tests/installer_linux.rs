//! `install/linux/install.sh` and `uninstall.sh`, run with bash (Git Bash on Windows) and every
//! location (`HOME`, `XDG_*`, `PREFIX`) inside a temp dir. The binaries are stand-in shell
//! scripts that only record how they were called, so nothing is ever scheduled.
//!
//! Runs from the repository seed the config dir with a `.env` first, so the repository's own
//! `.env` is never copied; the `.env` rule is tested on a copy of the scripts in a temp
//! "repository" instead.

use std::path::{Path, PathBuf};
use std::process::Command;

const SEEDED_ENV: &str = "WORKLOG_TEST_SEEDED=config-dir\n";
const FAKE_BIN: &str = "#!/bin/sh\necho \"$(basename \"$0\") $*\" >> \"$WORKLOG_FAKE_CALLS\"\n";

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn script(name: &str) -> PathBuf {
    repo().join("install").join("linux").join(name)
}

fn icon(name: &str) -> PathBuf {
    repo().join("assets").join("icon").join(name)
}

/// A path as the scripts get it: on Windows with forward slashes, which both bash and Windows
/// understand; elsewhere as it is, since '\' is then an ordinary file name character.
fn fwd(p: &Path) -> String {
    let s = p.display().to_string();
    if cfg!(windows) { s.replace('\\', "/") } else { s }
}

/// `HOME` as bash sees it. Git Bash rewrites it to its own form when it starts (`/c/x`, or
/// `/tmp/x` under the temp dir), while other variables such as `PREFIX` pass through as given.
fn bash_home(bash: &Path, home: &Path) -> String {
    let o = Command::new(bash).args(["-c", r#"printf %s "$HOME""#]).env("HOME", fwd(home)).output().unwrap();
    assert!(o.status.success());
    String::from_utf8(o.stdout).unwrap()
}

/// bash, but on Windows Git Bash: `Command::new("bash")` would pick System32\bash.exe, the WSL
/// launcher, before PATH, and Git's `usr\bin\bash.exe` starts without git on its PATH.
fn bash() -> Option<PathBuf> {
    if cfg!(windows) {
        for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
            let c = dir.join("bash.exe");
            let lower = c.to_string_lossy().to_lowercase();
            if !c.is_file() || lower.contains(r"\system32\") || lower.contains("windowsapps") {
                continue;
            }
            if lower.ends_with(r"\usr\bin\bash.exe") {
                let git_bin = dir.parent()?.parent()?.join("bin").join("bash.exe");
                if git_bin.is_file() {
                    return Some(git_bin);
                }
            }
            return Some(c);
        }
        None
    } else {
        let ok = Command::new("bash").arg("--version").output().is_ok_and(|o| o.status.success());
        ok.then(|| PathBuf::from("bash"))
    }
}

macro_rules! require_bash {
    () => {
        match bash() {
            Some(b) => b,
            None => {
                eprintln!("SKIPPED: no bash (Git Bash on Windows) on PATH");
                return;
            }
        }
    };
}

struct Run {
    code: Option<i32>,
    out: String,
}

impl Run {
    fn has_line(&self, want: &str) -> bool {
        self.out.lines().any(|l| l == want)
    }
}

/// Where an installation lands, worked out from the environment the test hands the scripts.
struct Layout {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    source: PathBuf,
    calls: PathBuf,
    /// `PREFIX` to pass, `None` for the default (`$HOME/.local`).
    prefix: Option<PathBuf>,
    xdg_data: Option<PathBuf>,
    xdg_config: Option<PathBuf>,
    /// The resulting directories, as Rust opens them.
    bin: PathBuf,
    data: PathBuf,
    config: PathBuf,
    /// `<bin>/worklog` as bash spells it in the menu entry.
    exec: String,
}

impl Layout {
    fn new(bash: &Path, custom_prefix: bool, xdg_data: bool, xdg_config: bool) -> Self {
        Self::with_prefix(bash, custom_prefix.then_some("prefix"), xdg_data, xdg_config)
    }

    /// `prefix`: the name of the `PREFIX` folder inside the temp dir, `None` for the default.
    fn with_prefix(bash: &Path, prefix: Option<&str>, xdg_data: bool, xdg_config: bool) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let home = root.join("home");
        let prefix = prefix.map(|name| root.join(name));
        let xdg_data = xdg_data.then(|| root.join("xdg-data"));
        let xdg_config = xdg_config.then(|| root.join("xdg-config"));
        let (bin, exec) = match &prefix {
            Some(p) => (p.join("bin"), format!("{}/bin/worklog", fwd(p))),
            None => (home.join(".local").join("bin"), format!("{}/.local/bin/worklog", bash_home(bash, &home))),
        };
        let data = match (&prefix, &xdg_data) {
            (Some(p), _) => p.join("share"),
            (None, Some(x)) => x.clone(),
            (None, None) => home.join(".local").join("share"),
        };
        let config = xdg_config.clone().unwrap_or_else(|| home.join(".config")).join("worklog");
        let source = root.join("source");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&source).unwrap();
        for b in ["worklog", "worklog-popup"] {
            std::fs::write(source.join(b), FAKE_BIN).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(source.join(b), std::fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join(".env"), SEEDED_ENV).unwrap();
        Layout { calls: root.join("calls.txt"), _tmp: tmp, root, home, source, prefix, xdg_data, xdg_config, bin, data, config, exec }
    }

    fn run(&self, bash: &Path, script: &Path, env: &[(&str, &str)]) -> Run {
        let mut c = Command::new(bash);
        c.arg(fwd(script));
        for v in ["PREFIX", "BUILD", "NO_SCHEDULE", "SOURCE", "PURGE", "XDG_DATA_HOME", "XDG_CONFIG_HOME"] {
            c.env_remove(v);
        }
        c.env("HOME", fwd(&self.home)).env("SOURCE", fwd(&self.source)).env("WORKLOG_FAKE_CALLS", fwd(&self.calls));
        if let Some(p) = &self.prefix {
            c.env("PREFIX", fwd(p));
        }
        if let Some(x) = &self.xdg_data {
            c.env("XDG_DATA_HOME", fwd(x));
        }
        if let Some(x) = &self.xdg_config {
            c.env("XDG_CONFIG_HOME", fwd(x));
        }
        c.envs(env.iter().copied());
        let o = c.output().expect("bash runs");
        Run { code: o.status.code(), out: format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)) }
    }

    fn desktop(&self) -> PathBuf {
        self.data.join("applications").join("worklog.desktop")
    }

    fn svg(&self) -> PathBuf {
        self.data.join("icons").join("hicolor").join("scalable").join("apps").join("worklog.svg")
    }

    fn png(&self) -> PathBuf {
        self.data.join("icons").join("hicolor").join("256x256").join("apps").join("worklog.png")
    }

    fn app_uninstaller(&self) -> PathBuf {
        self.data.join("worklog").join("uninstall.sh")
    }

    fn calls(&self) -> String {
        std::fs::read_to_string(&self.calls).unwrap_or_default()
    }
}

fn same_bytes(a: &Path, b: &Path) -> bool {
    std::fs::read(a).unwrap() == std::fs::read(b).unwrap()
}

#[test]
fn both_scripts_parse() {
    let bash = require_bash!();
    for name in ["install.sh", "uninstall.sh"] {
        let o = Command::new(&bash).arg("-n").arg(fwd(&script(name))).output().unwrap();
        assert!(o.status.success(), "{name}: {}", String::from_utf8_lossy(&o.stderr));
    }
}

#[test]
fn scripts_have_lf_line_endings_and_git_keeps_them_so() {
    for name in ["install.sh", "uninstall.sh"] {
        let bytes = std::fs::read(script(name)).unwrap();
        assert!(!bytes.contains(&b'\r'), "{name} has CR bytes");
    }
    let attrs = std::fs::read_to_string(repo().join(".gitattributes")).unwrap();
    assert!(attrs.lines().any(|l| l.split_whitespace().collect::<Vec<_>>() == ["*.sh", "text", "eol=lf"]), "{attrs}");
}

#[test]
fn git_records_both_scripts_as_executable() {
    let o = Command::new("git")
        .args(["ls-files", "-s", "--", "install/linux/install.sh", "install/linux/uninstall.sh"])
        .current_dir(repo())
        .output()
        .expect("git runs");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = String::from_utf8(o.stdout).unwrap();
    // "<mode> <object> <stage>\t<path>"
    let modes: Vec<(&str, &str)> = out.lines().map(|l| (l.split(' ').next().unwrap(), l.split('\t').nth(1).unwrap())).collect();
    assert_eq!(modes, [("100755", "install/linux/install.sh"), ("100755", "install/linux/uninstall.sh")]);
}

/// The installed menu entry's Exec line for a bin dir with characters the Desktop Entry spec
/// reserves, and the files really landing in that dir.
fn install_into_prefix(bash: &Path, prefix_name: &str) -> (Layout, String) {
    let l = Layout::with_prefix(bash, Some(prefix_name), false, false);
    let run = l.run(bash, &script("install.sh"), &[("NO_SCHEDULE", "1")]);
    assert_eq!(run.code, Some(0), "{}", run.out);
    assert_eq!(std::fs::read_to_string(l.bin.join("worklog")).ok().as_deref(), Some(FAKE_BIN));
    let desktop = std::fs::read_to_string(l.desktop()).unwrap();
    let exec: Vec<&str> = desktop.lines().filter(|x| x.starts_with("Exec=")).collect();
    assert_eq!(exec.len(), 1, "{desktop}");
    let line = exec[0].to_owned();
    (l, line)
}

/// Desktop Entry spec, "The Exec key": a quoted argument may hold a space as it is, and `%` is
/// written `%%` so it is not taken for a field code.
#[test]
fn exec_quotes_a_bin_dir_with_a_space_and_doubles_its_percent_sign() {
    let bash = require_bash!();
    let (l, exec) = install_into_prefix(&bash, "a b%c");
    let path = format!("{}/bin/worklog", fwd(l.prefix.as_ref().unwrap())).replace('%', "%%");
    assert_eq!(exec, format!(r#"Exec="{path}" popup --force"#));
}

/// Desktop Entry spec, "The Exec key": inside quotes `$` and `` ` `` take a backslash, and the
/// string escape rule is applied before that one, so the file holds `\\$` and ``\\` `` ("a
/// literal dollar sign in a quoted argument ... is unambiguously represented with \\$").
#[test]
fn exec_escapes_a_dollar_sign_and_a_backtick_in_the_bin_dir() {
    let bash = require_bash!();
    let (l, exec) = install_into_prefix(&bash, "d$e`f");
    let path = format!("{}/bin/worklog", fwd(l.prefix.as_ref().unwrap())).replace('$', r"\\$").replace('`', r"\\`");
    assert_eq!(exec, format!(r#"Exec="{path}" popup --force"#));
}

/// Desktop Entry spec, "The Exec key", with the string escape rule applied first: a literal '\'
/// in a quoted argument is written as four backslashes, '"' as `\\"`. Windows folder names cannot
/// hold either character.
#[cfg(unix)]
#[test]
fn exec_escapes_a_backslash_and_a_double_quote_in_the_bin_dir() {
    let bash = require_bash!();
    let (l, exec) = install_into_prefix(&bash, r#"a\b"c"#);
    let path = format!("{}/bin/worklog", fwd(l.prefix.as_ref().unwrap())).replace('\\', r"\\\\").replace('"', r#"\\""#);
    assert_eq!(exec, format!(r#"Exec="{path}" popup --force"#));
}

/// The same rule for every reserved character at once, on install.sh's own `exec_path=` line
/// run in bash with a `bin_dir` no Windows folder could have.
#[test]
fn the_exec_path_line_escapes_backslash_quote_dollar_backtick_and_percent() {
    let bash = require_bash!();
    let text = std::fs::read_to_string(script("install.sh")).unwrap();
    let lines: Vec<&str> = text.lines().filter(|x| x.starts_with("exec_path=")).collect();
    assert_eq!(lines.len(), 1, "one exec_path= line in install.sh");
    // Through a file: Git Bash's command-line parsing would mangle the quotes of a `-c` program.
    let tmp = tempfile::tempdir().unwrap();
    let program = tmp.path().join("exec_path.sh");
    std::fs::write(&program, format!("bin_dir=\"$TEST_BIN_DIR\"\n{}\nprintf '%s' \"$exec_path\"\n", lines[0])).unwrap();
    let o = Command::new(&bash).arg(fwd(&program)).env("TEST_BIN_DIR", r#"/x/a\b"c$d`e%f"#).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // '\' -> \\\\, '"' -> \\", '$' -> \\$, '`' -> \\`, '%' -> %%
    assert_eq!(String::from_utf8(o.stdout).unwrap(), r#"/x/a\\\\b\\"c\\$d\\`e%%f/worklog"#);
}

/// When `worklog uninstall` fails and there is no systemctl (Git Bash), it says so and goes on.
#[cfg(windows)]
#[test]
fn uninstall_goes_on_when_worklog_uninstall_fails_without_systemd() {
    let bash = require_bash!();
    let has_systemctl = Command::new(&bash).args(["-c", "command -v systemctl"]).output().unwrap().status.success();
    assert!(!has_systemctl, "this case needs a bash without systemctl");
    let l = Layout::new(&bash, true, false, false);
    let run = l.run(&bash, &script("install.sh"), &[("NO_SCHEDULE", "1")]);
    assert_eq!(run.code, Some(0), "{}", run.out);
    std::fs::write(l.bin.join("worklog"), format!("{FAKE_BIN}exit 1\n")).unwrap();

    let run = l.run(&bash, &l.app_uninstaller(), &[]);

    assert_eq!(run.code, Some(0), "{}", run.out);
    assert_eq!(l.calls(), "worklog uninstall\n");
    assert!(run.has_line("    \u{2713} no systemd here"), "{}", run.out);
    for gone in [l.bin.join("worklog"), l.bin.join("worklog-popup"), l.svg(), l.png(), l.desktop(), l.data.join("worklog")] {
        assert!(!gone.exists(), "{} left", gone.display());
    }
}

/// Where things go: `PREFIX` (when not the default) beats `XDG_DATA_HOME`, which beats
/// `~/.local/share`; the config dir is `XDG_CONFIG_HOME` or `~/.config`, plus `worklog`.
#[test]
fn install_then_uninstall_in_every_location_layout() {
    let bash = require_bash!();
    // (case, custom PREFIX, XDG_DATA_HOME set, XDG_CONFIG_HOME set)
    let rows = [
        ("defaults", false, false, false),
        ("XDG dirs set", false, true, true),
        ("custom PREFIX beats XDG_DATA_HOME", true, true, false),
    ];
    for (case, prefix, xdg_data, xdg_config) in rows {
        let l = Layout::new(&bash, prefix, xdg_data, xdg_config);

        let run = l.run(&bash, &script("install.sh"), &[("NO_SCHEDULE", "1")]);
        assert_eq!(run.code, Some(0), "{case}: {}", run.out);
        for b in ["worklog", "worklog-popup"] {
            assert_eq!(std::fs::read_to_string(l.bin.join(b)).ok().as_deref(), Some(FAKE_BIN), "{case}: {b}");
        }
        assert!(same_bytes(&l.svg(), &icon("worklog.svg")), "{case}: svg");
        assert!(same_bytes(&l.png(), &icon("worklog.png")), "{case}: png");
        assert!(same_bytes(&l.app_uninstaller(), &script("uninstall.sh")), "{case}: uninstall.sh");
        let desktop = std::fs::read_to_string(l.desktop()).unwrap();
        assert!(!desktop.contains('\r'), "{case}: CR in the menu entry");
        let lines: Vec<&str> = desktop.lines().collect();
        assert_eq!(lines.first(), Some(&"[Desktop Entry]"), "{case}: {desktop}");
        for want in [format!(r#"Exec="{}" popup --force"#, l.exec), "Icon=worklog".into(), "StartupWMClass=worklog".into(), "Type=Application".into(), "Name=Worklog".into()] {
            assert_eq!(lines.iter().filter(|x| **x == want).count(), 1, "{case}: {want:?} in\n{desktop}");
        }
        assert_eq!(std::fs::read_to_string(l.config.join(".env")).unwrap(), SEEDED_ENV, "{case}");
        assert_eq!(l.calls(), "", "{case}: NO_SCHEDULE=1 still ran a binary");

        let run = l.run(&bash, &l.app_uninstaller(), &[("NO_SCHEDULE", "1")]);
        assert_eq!(run.code, Some(0), "{case}: {}", run.out);
        for gone in [l.bin.join("worklog"), l.bin.join("worklog-popup"), l.svg(), l.png(), l.desktop(), l.data.join("worklog")] {
            assert!(!gone.exists(), "{case}: {} left", gone.display());
        }
        assert_eq!(std::fs::read_to_string(l.config.join(".env")).unwrap(), SEEDED_ENV, "{case}: settings kept");

        let run = l.run(&bash, &script("uninstall.sh"), &[("NO_SCHEDULE", "1"), ("PURGE", "1")]);
        assert_eq!(run.code, Some(0), "{case}: {}", run.out);
        assert!(!l.config.exists(), "{case}: PURGE=1 left {}", l.config.display());
        assert_eq!(l.calls(), "", "{case}");
    }
}

/// Without `NO_SCHEDULE` and without systemd (Git Bash has no systemctl) it warns and goes on.
#[cfg(windows)]
#[test]
fn install_without_a_systemd_user_session_warns_and_schedules_nothing() {
    let bash = require_bash!();
    let l = Layout::new(&bash, true, false, false);
    let run = l.run(&bash, &script("install.sh"), &[]);
    assert_eq!(run.code, Some(0), "{}", run.out);
    let bin = fwd(l.prefix.as_ref().unwrap()) + "/bin";
    assert!(run.has_line(&format!("    ! no systemd user session here; start worklog from the menu, or schedule '{bin}/worklog-popup' yourself")), "{}", run.out);
    assert_eq!(l.calls(), "");
    assert!(l.desktop().is_file());
}

#[test]
fn uninstall_without_no_schedule_asks_worklog_to_remove_its_timer() {
    let bash = require_bash!();
    let l = Layout::new(&bash, true, false, false);
    let run = l.run(&bash, &script("install.sh"), &[("NO_SCHEDULE", "1")]);
    assert_eq!(run.code, Some(0), "{}", run.out);

    let run = l.run(&bash, &l.app_uninstaller(), &[]);

    assert_eq!(run.code, Some(0), "{}", run.out);
    assert_eq!(l.calls(), "worklog uninstall\n");
    assert!(run.has_line("    \u{2713} removed worklog.timer"), "{}", run.out);
    assert!(!l.bin.join("worklog").exists());
}

/// `.env` precedence: the config dir's own, then the repository's, then `.env.example` with a
/// warning. Runs a copy of install.sh in a temp repository so the real `.env` stays out of it.
#[test]
fn env_comes_from_the_config_dir_then_the_repository_then_the_example() {
    let bash = require_bash!();
    const REPO_ENV: &str = "WORKLOG_TEST_FROM=repository\n";
    const EXAMPLE: &str = "# WORKLOG_TEST_FROM=example\n";
    // (case, config has .env, repo has .env, expected config/.env, expected config/.env.example, warns)
    let rows: [(&str, bool, bool, Option<&str>, Option<&str>, bool); 4] = [
        ("config .env wins over the repository's", true, true, Some(SEEDED_ENV), None, false),
        ("config .env kept without a repository one", true, false, Some(SEEDED_ENV), None, false),
        ("repository .env copied", false, true, Some(REPO_ENV), None, false),
        ("example copied with a warning", false, false, None, Some(EXAMPLE), true),
    ];
    for (case, config_env, repo_env, want_env, want_example, warns) in rows {
        let l = Layout::new(&bash, false, true, true);
        if !config_env {
            std::fs::remove_file(l.config.join(".env")).unwrap();
        }
        let fake = l.root.join("repo");
        std::fs::create_dir_all(fake.join("install").join("linux")).unwrap();
        std::fs::create_dir_all(fake.join("assets").join("icon")).unwrap();
        for name in ["install.sh", "uninstall.sh"] {
            std::fs::copy(script(name), fake.join("install").join("linux").join(name)).unwrap();
        }
        for name in ["worklog.svg", "worklog.png"] {
            std::fs::copy(icon(name), fake.join("assets").join("icon").join(name)).unwrap();
        }
        std::fs::write(fake.join(".env.example"), EXAMPLE).unwrap();
        if repo_env {
            std::fs::write(fake.join(".env"), REPO_ENV).unwrap();
        }

        let run = l.run(&bash, &fake.join("install").join("linux").join("install.sh"), &[("NO_SCHEDULE", "1")]);

        assert_eq!(run.code, Some(0), "{case}: {}", run.out);
        assert_eq!(std::fs::read_to_string(l.config.join(".env")).ok().as_deref(), want_env, "{case}");
        assert_eq!(std::fs::read_to_string(l.config.join(".env.example")).ok().as_deref(), want_example, "{case}");
        let config = format!("{}/worklog", fwd(l.xdg_config.as_ref().unwrap()));
        let warning = format!("    ! no .env yet: copy {config}/.env.example to .env there and fill it in (see README)");
        assert_eq!(run.has_line(&warning), warns, "{case}: {}", run.out);
    }
}
