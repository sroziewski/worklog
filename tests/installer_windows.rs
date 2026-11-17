#![cfg(windows)]
//! `install/windows/install.ps1` and `uninstall.ps1`, run for real with every path, the registry
//! key included, pointed into a temp dir or a throwaway `HKCU:\Software\worklog-test-*` key.
//!
//! Never run without `-NoSchedule` against a real worklog.exe: `worklog install` would replace the
//! user's scheduled task, and uninstall.ps1 falls back to deleting it by name. The tests that run
//! a schedule step give the scripts a stand-in exe that cannot schedule anything, and uninstall.ps1
//! a throwaway `-TaskName` (`Layout::uninstall_scheduling` checks both).
//! Tests that run the scripts from the repository seed the config dir with a `.env` first, so the
//! repository's own `.env` is never copied; the `.env` rule is tested on a copy of the scripts in
//! a temp "repository" instead.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};

const SEEDED_ENV: &str = "WORKLOG_TEST_SEEDED=config-dir\n";

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn script(name: &str) -> PathBuf {
    repo().join("install").join("windows").join(name)
}

fn s(p: &Path) -> String {
    p.display().to_string()
}

fn powershell() -> Command {
    let mut c = Command::new("powershell.exe");
    c.args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass"]).stdin(Stdio::null());
    c
}

struct Run {
    code: Option<i32>,
    out: String,
}

impl Run {
    fn has_line(&self, want: &str) -> bool {
        self.out.lines().any(|l| l == want)
    }

    /// An `Ok` line: its check mark reaches a redirected stdout as `?` from Windows PowerShell,
    /// depending on the console code page.
    fn has_ok_line(&self, text: &str) -> bool {
        self.has_line(&format!("    \u{2713} {text}")) || self.has_line(&format!("    ? {text}"))
    }
}

fn run_script(script: &Path, args: &[String]) -> Run {
    run_script_in(script, args, None)
}

fn run_script_in(script: &Path, args: &[String], cwd: Option<&Path>) -> Run {
    let mut c = powershell();
    c.arg("-File").arg(script).args(args);
    if let Some(dir) = cwd {
        c.current_dir(dir);
    }
    let o = c.output().expect("powershell.exe runs");
    Run { code: o.status.code(), out: format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)) }
}

/// A Windows program from System32, to copy in as a stand-in for worklog.exe.
fn system_exe(name: &str) -> PathBuf {
    let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    Path::new(&system).join("System32").join(name)
}

/// sort.exe called with one argument it takes for a missing input file: it writes the error to
/// stderr and exits 1, as worklog does when its settings are broken. Returns that error's last
/// non-blank line, as sort prints it when run from `cwd`.
fn sort_error(sort: &Path, arg: &str, cwd: &Path) -> String {
    let o = Command::new(sort).arg(arg).current_dir(cwd).stdin(Stdio::null()).output().unwrap();
    assert_eq!(o.status.code(), Some(1), "the stand-in must fail");
    assert!(o.stdout.is_empty(), "the stand-in must write only to stderr");
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    err.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).last().expect("an error on stderr").to_owned()
}

/// Runs a PowerShell snippet that must succeed and returns its output lines.
fn ps_lines(snippet: &str) -> Vec<String> {
    let o = powershell().arg("-Command").arg(snippet).output().expect("powershell.exe runs");
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(o.status.success(), "snippet failed: {snippet}\n{out}{}", String::from_utf8_lossy(&o.stderr));
    out.lines().map(str::to_owned).collect()
}

/// A registry key of the test's own, deleted when the test ends however it ends.
struct TestKey(String);

impl TestKey {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        TestKey(format!(r"HKCU:\Software\worklog-test-{}-{nanos}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)))
    }

    fn exists(&self) -> bool {
        ps_lines(&format!("Test-Path -LiteralPath '{}'", self.0)) == ["True"]
    }
}

impl Drop for TestKey {
    fn drop(&mut self) {
        assert!(self.0.starts_with(r"HKCU:\Software\worklog-test-"));
        let _ = powershell()
            .arg("-Command")
            .arg(format!("if (Test-Path -LiteralPath '{0}') {{ Remove-Item -LiteralPath '{0}' -Recurse -Force }}", self.0))
            .output();
    }
}

/// A process named `worklog` running from `dir` (a copy of ping.exe waiting out a minute), killed
/// when dropped.
struct RunningCopy(Child);

impl RunningCopy {
    fn start(dir: &Path) -> Self {
        std::fs::create_dir_all(dir).unwrap();
        let exe = dir.join("worklog.exe");
        std::fs::copy(system_exe("PING.EXE"), &exe).unwrap();
        let child = Command::new(&exe).args(["-n", "60", "127.0.0.1"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        RunningCopy(child)
    }
}

impl Drop for RunningCopy {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Every place the scripts write to, inside one temp dir.
struct Layout {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    source: PathBuf,
    install: PathBuf,
    start_menu: PathBuf,
    config: PathBuf,
    key: TestKey,
}

impl Layout {
    /// The source dir holds the built binaries under their release names; the config dir already
    /// has a `.env`, so the repository's is never copied.
    fn new() -> Self {
        let l = Self::empty();
        for (bin, name) in [(env!("CARGO_BIN_EXE_worklog"), "worklog.exe"), (env!("CARGO_BIN_EXE_worklog-popup"), "worklog-popup.exe")] {
            std::fs::copy(bin, l.source.join(name)).unwrap();
        }
        std::fs::write(l.config.join(".env"), SEEDED_ENV).unwrap();
        l
    }

    fn empty() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let l = Layout {
            source: root.join("source"),
            install: root.join("Programs").join("worklog"),
            start_menu: root.join("StartMenu"),
            config: root.join("AppData").join("worklog"),
            root,
            _tmp: tmp,
            key: TestKey::new(),
        };
        std::fs::create_dir_all(&l.source).unwrap();
        std::fs::create_dir_all(&l.config).unwrap();
        l
    }

    fn lnk(&self) -> PathBuf {
        self.start_menu.join("Worklog.lnk")
    }

    fn exe(&self) -> PathBuf {
        self.install.join("worklog.exe")
    }

    fn uninstall_args(&self) -> Vec<String> {
        vec![
            "-NoSchedule".into(),
            "-InstallDir".into(),
            s(&self.install),
            "-StartMenuDir".into(),
            s(&self.start_menu),
            "-ConfigDir".into(),
            s(&self.config),
            "-UninstallKey".into(),
            self.key.0.clone(),
        ]
    }

    fn install_args(&self) -> Vec<String> {
        let mut a = self.uninstall_args();
        a.extend(["-Source".into(), s(&self.source)]);
        a
    }

    fn install(&self, extra: &[&str]) -> Run {
        let mut a = self.install_args();
        a.extend(extra.iter().map(|x| x.to_string()));
        run_script(&script("install.ps1"), &a)
    }

    fn uninstall_with(&self, script: &Path, extra: &[&str]) -> Run {
        let mut a = self.uninstall_args();
        a.extend(extra.iter().map(|x| x.to_string()));
        run_script(script, &a)
    }

    /// The one way a test runs uninstall.ps1's schedule step: with a task name no real task has,
    /// and only after checking that the installed worklog.exe is not the real binary, whose
    /// `uninstall` would remove the user's task.
    fn uninstall_scheduling(&self, task: &str) -> Run {
        assert!(task.starts_with(r"\worklog-test-"), "{task} could be a real task");
        assert!(!same_bytes(&self.exe(), Path::new(env!("CARGO_BIN_EXE_worklog"))), "the installed worklog.exe is the real one");
        let mut a: Vec<String> = self.uninstall_args().into_iter().filter(|x| x != "-NoSchedule").collect();
        a.extend(["-TaskName".to_string(), task.to_string()]);
        run_script_in(&script("uninstall.ps1"), &a, Some(&self.root))
    }
}

/// Puts sort.exe where the installer just wrote worklog.exe. A freshly written exe can be held
/// open for a moment by a virus scanner, so the copy is retried for up to five seconds.
fn replace_with_sort(exe: &Path) {
    let mut last = None;
    for _ in 0..25 {
        match std::fs::copy(system_exe("sort.exe"), exe) {
            Ok(_) => return,
            Err(e) => last = Some(e),
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    panic!("replacing {}: {:?}", exe.display(), last);
}

/// Waits up to ten seconds until `file` can be opened for writing.
fn wait_until_writable(file: &Path) {
    for _ in 0..50 {
        if std::fs::OpenOptions::new().write(true).open(file).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    panic!("{} stayed locked", file.display());
}

/// Opens `file` with no sharing, so nothing else can open it until the handle is dropped. Waits up
/// to ten seconds for a virus scanner that may still hold the freshly written exe.
fn hold_exclusively(file: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    for _ in 0..50 {
        if let Ok(f) = std::fs::OpenOptions::new().read(true).share_mode(0).open(file) {
            return f;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    panic!("cannot open {} exclusively", file.display());
}

/// A scheduled-task path no real task uses.
fn throwaway_task() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    format!(r"\worklog-test-{}-{nanos}\none", std::process::id())
}

fn schtasks(args: &[&str]) -> std::process::Output {
    Command::new("schtasks.exe").args(args).stdin(Stdio::null()).output().expect("schtasks.exe runs")
}

/// A per-user scheduled task of the test's own (in the root folder, so deleting the task leaves
/// nothing behind), deleted when the test ends however it ends.
struct TestTask(String);

impl TestTask {
    fn create() -> Self {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let task = TestTask(format!(r"\worklog-test-{}-{nanos}", std::process::id()));
        let o = schtasks(&["/Create", "/TN", &task.0, "/TR", "cmd.exe", "/SC", "ONCE", "/ST", "00:00", "/F"]);
        assert!(o.status.success(), "creating {}: {}{}", task.0, String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        task
    }

    fn exists(&self) -> bool {
        schtasks(&["/Query", "/TN", &self.0]).status.success()
    }
}

impl Drop for TestTask {
    fn drop(&mut self) {
        assert!(self.0.starts_with(r"\worklog-test-"), "{} could be a real task", self.0);
        if self.exists() {
            let _ = schtasks(&["/Delete", "/F", "/TN", &self.0]);
        }
    }
}

/// Runs the UninstallString registered under the test key, as Windows starts it (the program,
/// then the rest of the command line verbatim), plus -NoSchedule so the real scheduled task is
/// left alone. The string is checked in full first: one missing a location would fall back to
/// the user's real one.
fn run_registered_uninstall(l: &Layout) -> std::process::Output {
    use std::os::windows::process::CommandExt;
    let registered = ps_lines(&format!("(Get-ItemProperty -LiteralPath '{}').UninstallString", l.key.0)).join("\n");
    assert_eq!(registered, expected_uninstall_string(l));
    let rest = registered.strip_prefix("powershell.exe ").unwrap();
    Command::new("powershell.exe").raw_arg(format!("{rest} -NoSchedule")).stdin(Stdio::null()).output().unwrap()
}

/// The Installed apps entry for `l`: exactly these values, with these types.
fn assert_registered(l: &Layout) {
    let install = s(&l.install);
    let exe = s(&l.exe());
    let bytes: u64 = std::fs::read_dir(&l.install).unwrap().map(|e| e.unwrap().metadata().unwrap().len()).sum();
    // PowerShell's [int] cast rounds half to even.
    let size_kb = (bytes as f64 / 1024.0).round_ties_even() as u64;

    let values = registry_values(&l.key.0);
    let names: Vec<&str> = values.iter().map(|v| v.split('|').next().unwrap()).collect();
    assert_eq!(
        names,
        ["DisplayIcon", "DisplayName", "DisplayVersion", "EstimatedSize", "InstallLocation", "NoModify", "NoRepair", "Publisher", "UninstallString", "URLInfoAbout"]
    );
    let get = |name: &str| values.iter().find(|v| v.starts_with(&format!("{name}|"))).unwrap().clone();
    let want = [
        format!("DisplayIcon|String|{exe},0"),
        "DisplayName|String|Worklog".to_string(),
        format!("DisplayVersion|String|{}", env!("CARGO_PKG_VERSION")),
        format!("EstimatedSize|DWord|{size_kb}"),
        format!("InstallLocation|String|{install}"),
        "NoModify|DWord|1".to_string(),
        "NoRepair|DWord|1".to_string(),
        format!("UninstallString|String|{}", expected_uninstall_string(l)),
    ];
    for w in &want {
        let name = w.split('|').next().unwrap();
        assert_eq!(&get(name), w);
    }
    // The contract names these two without fixing their text; the URL is the project page.
    let publisher = get("Publisher");
    assert!(publisher.starts_with("Publisher|String|") && publisher.len() > "Publisher|String|".len(), "{publisher}");
    let url = get("URLInfoAbout");
    assert!(url.starts_with("URLInfoAbout|String|https://github.com/") && url.ends_with("/worklog"), "{url}");
}

fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

fn same_bytes(a: &Path, b: &Path) -> bool {
    std::fs::read(a).unwrap() == std::fs::read(b).unwrap()
}

/// TargetPath, Arguments, WorkingDirectory, WindowStyle and IconLocation of a shortcut.
fn shortcut(lnk: &Path) -> Vec<String> {
    ps_lines(&format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut('{}'); $s.TargetPath; $s.Arguments; $s.WorkingDirectory; $s.WindowStyle; $s.IconLocation",
        lnk.display()
    ))
}

/// `name|kind|value` for every value under the key, sorted by name.
fn registry_values(key: &str) -> Vec<String> {
    ps_lines(&format!(
        "$k = Get-Item -LiteralPath '{key}'; foreach ($n in ($k.GetValueNames() | Sort-Object)) {{ '{{0}}|{{1}}|{{2}}' -f $n, $k.GetValueKind($n), $k.GetValue($n) }}"
    ))
}

/// The command Installed apps runs: every location the install was given, so uninstalling from
/// there cleans those and not the defaults.
fn expected_uninstall_string(l: &Layout) -> String {
    let install = s(&l.install);
    format!(
        r#"powershell.exe -NoProfile -ExecutionPolicy Bypass -File "{install}\uninstall.ps1" -InstallDir "{install}" -StartMenuDir "{}" -ConfigDir "{}" -UninstallKey "{}""#,
        l.start_menu.display(),
        l.config.display(),
        l.key.0
    )
}

mod install {
    use super::*;

    #[test]
    fn copies_both_executables_the_uninstaller_and_the_icon_and_keeps_an_existing_env() {
        let l = Layout::new();
        let run = l.install(&["-NoRegister"]);
        assert_eq!(run.code, Some(0), "{}", run.out);
        assert_eq!(file_names(&l.install), ["uninstall.ps1", "worklog-popup.exe", "worklog.exe", "worklog.ico"]);
        assert!(same_bytes(&l.install.join("worklog.exe"), Path::new(env!("CARGO_BIN_EXE_worklog"))));
        assert!(same_bytes(&l.install.join("worklog-popup.exe"), Path::new(env!("CARGO_BIN_EXE_worklog-popup"))));
        assert!(same_bytes(&l.install.join("uninstall.ps1"), &script("uninstall.ps1")));
        assert!(same_bytes(&l.install.join("worklog.ico"), &repo().join("assets").join("icon").join("worklog.ico")));
        assert_eq!(std::fs::read_to_string(l.config.join(".env")).unwrap(), SEEDED_ENV);
        assert_eq!(file_names(&l.config), [".env"]);
    }

    #[test]
    fn creates_a_start_menu_shortcut_that_opens_the_popup_for_today() {
        let l = Layout::new();
        let run = l.install(&["-NoRegister"]);
        assert_eq!(run.code, Some(0), "{}", run.out);
        let exe = s(&l.exe());
        assert_eq!(shortcut(&l.lnk()), [exe.clone(), "popup --force".into(), s(&l.install), "7".into(), format!("{exe},0")]);
    }

    #[test]
    fn registers_the_installed_apps_entry() {
        let l = Layout::new();
        let run = l.install(&[]);
        assert_eq!(run.code, Some(0), "{}", run.out);
        assert_registered(&l);
    }

    #[test]
    fn a_failing_schedule_that_writes_to_stderr_only_warns_and_the_install_goes_on() {
        // sort.exe as worklog.exe: `install` fails on stderr like worklog with broken settings,
        // and cannot touch the real scheduled task.
        let l = Layout::empty();
        std::fs::write(l.config.join(".env"), SEEDED_ENV).unwrap();
        for name in ["worklog.exe", "worklog-popup.exe"] {
            std::fs::copy(system_exe("sort.exe"), l.source.join(name)).unwrap();
        }
        let error = sort_error(&l.source.join("worklog.exe"), "install", &l.root);
        let args: Vec<String> = l.install_args().into_iter().filter(|a| a != "-NoSchedule").chain(["-NoRegister".to_string()]).collect();

        let run = run_script_in(&script("install.ps1"), &args, Some(&l.root));

        assert_eq!(run.code, Some(0), "{}", run.out);
        let warning = format!("    ! not scheduled: {error}");
        assert!(run.out.lines().any(|x| x.trim_end() == warning), "want {warning:?} in\n{}", run.out);
        assert!(run.has_line(&format!(r#"    ! fix the settings, then run: "{}" install"#, l.exe().display())), "{}", run.out);
        assert!(run.has_line("  Done. Start it from the Start menu: Worklog"), "{}", run.out);
        assert!(l.lnk().is_file(), "the shortcut is still made");
    }

    #[test]
    fn running_again_updates_the_program_and_keeps_the_settings() {
        let l = Layout::new();
        let run = l.install(&[]);
        assert_eq!(run.code, Some(0), "{}", run.out);
        let popup = l.source.join("worklog-popup.exe");
        let mut bytes = std::fs::read(&popup).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x5a;
        std::fs::write(&popup, &bytes).unwrap();
        // The first install ran worklog.exe --version; a virus scanner may still hold the exe it
        // just saw run, which would fail the copy for reasons that are not the script's.
        wait_until_writable(&l.exe());

        let run = l.install(&[]);

        assert_eq!(run.code, Some(0), "{}", run.out);
        // Not assert_eq!: a failure would print the whole executable.
        assert!(std::fs::read(l.install.join("worklog-popup.exe")).unwrap() == bytes, "the installed worklog-popup.exe is not the changed source");
        assert_eq!(std::fs::read_to_string(l.config.join(".env")).unwrap(), SEEDED_ENV);
        let exe = s(&l.exe());
        assert_eq!(shortcut(&l.lnk()), [exe.clone(), "popup --force".into(), s(&l.install), "7".into(), format!("{exe},0")]);
        assert_registered(&l);
    }

    /// Something keeps the installed worklog.exe open the whole time: after its retries the
    /// script stops, naming the file, and leaves the first installation's shortcut and entry.
    #[test]
    fn running_again_gives_up_on_an_exe_something_keeps_open() {
        let l = Layout::new();
        let run = l.install(&[]);
        assert_eq!(run.code, Some(0), "{}", run.out);
        let shortcut_before = shortcut(&l.lnk());
        let registry_before = registry_values(&l.key.0);
        let held = hold_exclusively(&l.exe());

        let run = l.install(&[]);

        drop(held);
        assert_eq!(run.code, Some(1), "{}", run.out);
        // The reason after the prefix is the OS's, in its language.
        let prefix = format!(r"    x cannot replace {}\worklog.exe, something holds it: ", l.install.display());
        assert!(run.out.lines().any(|x| x.starts_with(&prefix)), "want a line starting {prefix:?} in\n{}", run.out);
        assert_eq!(shortcut(&l.lnk()), shortcut_before);
        assert_eq!(registry_values(&l.key.0), registry_before);
    }

    /// The same, but the handle closes after about 1.5 s: a retry then replaces the exe.
    #[test]
    fn running_again_retries_until_the_exe_is_free() {
        let l = Layout::new();
        let run = l.install(&["-NoRegister"]);
        assert_eq!(run.code, Some(0), "{}", run.out);
        let source = l.source.join("worklog.exe");
        let mut bytes = std::fs::read(&source).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x5a;
        // Never run: -NoRegister and -NoSchedule keep the script from calling it.
        std::fs::write(&source, &bytes).unwrap();
        // Opened here, before the install starts; the thread only closes it.
        let held = hold_exclusively(&l.exe());
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            drop(held);
        });

        let run = l.install(&["-NoRegister"]);

        releaser.join().unwrap();
        assert_eq!(run.code, Some(0), "{}", run.out);
        // Not assert_eq!: a failure would print the whole executable.
        assert!(std::fs::read(l.exe()).unwrap() == bytes, "the installed worklog.exe is not the changed source");
    }

    /// Paths end up quoted in the uninstall command: a trailing '\' would escape the closing quote
    /// and a relative one would point elsewhere, so both are made absolute and trimmed.
    #[test]
    fn locations_given_relative_or_with_a_trailing_backslash_are_registered_absolute_and_uninstall() {
        // (case, how a location under the temp dir is spelled, given the temp dir as cwd)
        let rows: [(&str, fn(&Path, &Path) -> String); 2] = [
            ("trailing backslash", |_, p| format!(r"{}\", p.display())),
            ("relative to the working dir", |root, p| p.strip_prefix(root).unwrap().display().to_string()),
        ];
        for (case, spell) in rows {
            let l = Layout::new();
            let args = vec![
                "-NoSchedule".to_string(),
                "-InstallDir".into(),
                spell(&l.root, &l.install),
                "-StartMenuDir".into(),
                spell(&l.root, &l.start_menu),
                "-ConfigDir".into(),
                spell(&l.root, &l.config),
                "-UninstallKey".into(),
                l.key.0.clone(),
                "-Source".into(),
                s(&l.source),
            ];
            let run = run_script_in(&script("install.ps1"), &args, Some(&l.root));
            assert_eq!(run.code, Some(0), "{case}: {}", run.out);
            assert!(l.lnk().is_file(), "{case}: shortcut not in {}", l.start_menu.display());
            assert_eq!(std::fs::read_to_string(l.config.join(".env")).unwrap(), SEEDED_ENV, "{case}");

            let o = run_registered_uninstall(&l);

            assert_eq!(o.status.code(), Some(0), "{case}: {}", String::from_utf8_lossy(&o.stdout));
            assert!(!l.install.exists() && !l.lnk().exists() && !l.key.exists(), "{case}: something left");
        }
    }

    #[test]
    fn a_worklog_running_from_another_folder_does_not_block_it() {
        let l = Layout::new();
        // Same parent and a name the install folder's is a prefix of.
        let _running = RunningCopy::start(&l.root.join("Programs").join("worklog-old"));
        let run = l.install(&["-NoRegister"]);
        assert_eq!(run.code, Some(0), "{}", run.out);
        assert!(l.exe().is_file());
    }

    /// `.env` precedence: the config dir's own, then the repository's, then `.env.example` with a
    /// warning. Runs a copy of install.ps1 in a temp repository so the real `.env` stays out of it.
    #[test]
    fn env_comes_from_the_config_dir_then_the_repository_then_the_example() {
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
            let l = Layout::new();
            if !config_env {
                std::fs::remove_file(l.config.join(".env")).unwrap();
            }
            let fake = l.root.join("repo");
            std::fs::create_dir_all(fake.join("install").join("windows")).unwrap();
            std::fs::create_dir_all(fake.join("assets").join("icon")).unwrap();
            std::fs::copy(script("install.ps1"), fake.join("install").join("windows").join("install.ps1")).unwrap();
            std::fs::copy(script("uninstall.ps1"), fake.join("install").join("windows").join("uninstall.ps1")).unwrap();
            std::fs::copy(repo().join("assets").join("icon").join("worklog.ico"), fake.join("assets").join("icon").join("worklog.ico")).unwrap();
            std::fs::write(fake.join(".env.example"), EXAMPLE).unwrap();
            if repo_env {
                std::fs::write(fake.join(".env"), REPO_ENV).unwrap();
            }
            let mut args = l.install_args();
            args.push("-NoRegister".into());
            let run = run_script(&fake.join("install").join("windows").join("install.ps1"), &args);

            assert_eq!(run.code, Some(0), "{case}: {}", run.out);
            assert_eq!(std::fs::read_to_string(l.config.join(".env")).ok().as_deref(), want_env, "{case}");
            assert_eq!(std::fs::read_to_string(l.config.join(".env.example")).ok().as_deref(), want_example, "{case}");
            let warning = format!(r"    ! no .env yet: copy {}\.env.example to .env there and fill it in (see README)", l.config.display());
            assert_eq!(run.has_line(&warning), warns, "{case}: {}", run.out);
            assert!(!l.key.exists(), "{case}: -NoRegister wrote the uninstall key");
        }
    }

    #[test]
    fn refuses_while_worklog_runs_from_the_install_folder() {
        let l = Layout::new();
        let _running = RunningCopy::start(&l.install);
        let run = l.install(&[]);
        assert_eq!(run.code, Some(1), "{}", run.out);
        assert!(run.has_line("    x worklog is running from the install folder; close its window and run this again"), "{}", run.out);
        assert_eq!(file_names(&l.install), ["worklog.exe"], "nothing copied");
        assert!(!l.lnk().exists());
        assert!(!l.key.exists());
    }
}

mod uninstall {
    use super::*;

    #[test]
    fn removes_shortcut_entry_and_program_but_keeps_settings_until_purge() {
        let l = Layout::new();
        let run = l.install(&[]);
        assert_eq!(run.code, Some(0), "{}", run.out);
        assert!(l.lnk().is_file() && l.key.exists());

        // As the Installed apps entry runs it: the copy inside the install folder.
        let run = l.uninstall_with(&l.install.join("uninstall.ps1"), &[]);
        assert_eq!(run.code, Some(0), "{}", run.out);
        assert!(!l.install.exists(), "install folder left: {:?}", file_names(&l.install));
        assert!(!l.lnk().exists());
        assert!(!l.key.exists());
        assert_eq!(std::fs::read_to_string(l.config.join(".env")).unwrap(), SEEDED_ENV);

        let run = l.uninstall_with(&script("uninstall.ps1"), &["-Purge"]);
        assert_eq!(run.code, Some(0), "{}", run.out);
        assert!(!l.config.exists());
    }

    #[test]
    fn the_registered_uninstall_command_removes_the_installation_it_was_given() {
        let l = Layout::new();
        let run = l.install(&[]);
        assert_eq!(run.code, Some(0), "{}", run.out);

        let o = run_registered_uninstall(&l);

        assert_eq!(o.status.code(), Some(0), "{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        assert!(!l.lnk().exists(), "shortcut left");
        assert!(!l.key.exists(), "key left");
        assert!(!l.install.exists(), "install folder left");
        assert_eq!(std::fs::read_to_string(l.config.join(".env")).unwrap(), SEEDED_ENV);
    }

    /// `worklog uninstall` failing on stderr falls back to schtasks, which finds no task of that
    /// name; neither stops the uninstall.
    #[test]
    fn a_failing_worklog_uninstall_falls_back_to_schtasks_and_goes_on() {
        let l = Layout::new();
        let run = l.install(&[]);
        assert_eq!(run.code, Some(0), "{}", run.out);
        replace_with_sort(&l.exe());
        sort_error(&l.exe(), "uninstall", &l.root);

        let run = l.uninstall_scheduling(&throwaway_task());

        assert_eq!(run.code, Some(0), "{}", run.out);
        assert!(run.has_ok_line("no scheduled task"), "{}", run.out);
        assert!(!l.lnk().exists(), "shortcut left");
        assert!(!l.key.exists(), "key left");
        assert!(!l.install.exists(), "install folder left");
        assert_eq!(std::fs::read_to_string(l.config.join(".env")).unwrap(), SEEDED_ENV);
    }

    /// The fallback deletes the task it is told to, by that name.
    #[test]
    fn a_failing_worklog_uninstall_falls_back_to_schtasks_which_removes_the_named_task() {
        let task = TestTask::create();
        assert!(task.exists());
        let l = Layout::new();
        let run = l.install(&[]);
        assert_eq!(run.code, Some(0), "{}", run.out);
        replace_with_sort(&l.exe());
        sort_error(&l.exe(), "uninstall", &l.root);

        let run = l.uninstall_scheduling(&task.0);

        assert_eq!(run.code, Some(0), "{}", run.out);
        assert!(run.has_ok_line(&format!("removed {}", task.0)), "{}", run.out);
        assert!(!task.exists(), "{} is still scheduled", task.0);
        assert!(!l.install.exists() && !l.lnk().exists() && !l.key.exists(), "something left");
    }

    #[test]
    fn a_worklog_running_from_another_folder_does_not_block_it() {
        let l = Layout::new();
        let run = l.install(&["-NoRegister"]);
        assert_eq!(run.code, Some(0), "{}", run.out);
        // Same parent and a name the install folder's is a prefix of.
        let _running = RunningCopy::start(&l.root.join("Programs").join("worklog-old"));

        let run = l.uninstall_with(&script("uninstall.ps1"), &[]);

        assert_eq!(run.code, Some(0), "{}", run.out);
        assert!(!l.install.exists(), "install folder left");
    }

    /// The guard that keeps it from deleting, say, the repository; nothing at all is removed.
    #[test]
    fn refuses_a_folder_that_is_not_an_installation() {
        // (case, files in the folder)
        let rows: [(&str, &[&str]); 2] =
            [("holds Cargo.toml next to worklog.exe", &["worklog.exe", "Cargo.toml"]), ("lacks worklog.exe", &["worklog-popup.exe", "uninstall.ps1"])];
        for (case, files) in rows {
            let l = Layout::new();
            std::fs::create_dir_all(&l.install).unwrap();
            for f in files {
                std::fs::write(l.install.join(f), "x").unwrap();
            }
            std::fs::create_dir_all(&l.start_menu).unwrap();
            std::fs::write(l.lnk(), "not really a shortcut").unwrap();
            ps_lines(&format!("New-Item -Path '{}' -Force | Out-Null", l.key.0));

            let run = l.uninstall_with(&script("uninstall.ps1"), &["-Purge"]);

            assert_eq!(run.code, Some(1), "{case}: {}", run.out);
            let want = format!("    x {} does not look like a worklog installation; nothing removed", l.install.display());
            assert!(run.has_line(&want), "{case}: {}", run.out);
            let mut expected: Vec<String> = files.iter().map(|f| f.to_string()).collect();
            expected.sort();
            assert_eq!(file_names(&l.install), expected, "{case}");
            assert!(l.lnk().is_file(), "{case}: shortcut removed");
            assert!(l.key.exists(), "{case}: key removed");
            assert!(l.config.join(".env").is_file(), "{case}: settings purged");
        }
    }

    #[test]
    fn refuses_while_worklog_runs_from_the_install_folder() {
        let l = Layout::new();
        let _running = RunningCopy::start(&l.install);
        std::fs::create_dir_all(&l.start_menu).unwrap();
        std::fs::write(l.lnk(), "not really a shortcut").unwrap();

        let run = l.uninstall_with(&script("uninstall.ps1"), &[]);

        assert_eq!(run.code, Some(1), "{}", run.out);
        assert!(run.has_line("    x worklog is running; close its window and run this again"), "{}", run.out);
        assert_eq!(file_names(&l.install), ["worklog.exe"]);
        assert!(l.lnk().is_file(), "shortcut removed");
    }
}
