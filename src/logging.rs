//! Timestamped log lines to stderr (CLI) and to `<config dir>/worklog/worklog.log` (both
//! binaries; the scheduled popup has no console). The file always gets debug detail.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use log::{Level, LevelFilter, Log, Metadata, Record};

/// Above this the log restarts, so it never grows without bound.
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

struct Logger {
    stderr: Option<LevelFilter>,
    file: Option<Mutex<File>>,
}

/// `WORKLOG_LOG` in the environment overrides the log file location (tests, portable setups).
pub fn default_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("WORKLOG_LOG").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    dirs::config_dir().map(|d| d.join("worklog").join("worklog.log"))
}

/// `stderr`: the level shown on the console, `None` for no console output.
pub fn init(stderr: Option<LevelFilter>) -> Option<PathBuf> {
    let path = default_path();
    let file = path.as_ref().and_then(|p| {
        std::fs::create_dir_all(p.parent()?).ok()?;
        let big = std::fs::metadata(p).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false);
        let f = if big { File::create(p) } else { OpenOptions::new().create(true).append(true).open(p) };
        f.ok().map(Mutex::new)
    });
    let logger = Logger { stderr, file };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(LevelFilter::Debug);
        // The scheduled popup has no console: without this a panic (in the GUI or a worker
        // thread) leaves the log ending mid-run with no reason.
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current();
            log::error!("panic in thread {}: {info}", thread.name().unwrap_or("<unnamed>"));
            log::logger().flush();
            default_hook(info);
        }));
    }
    path
}

impl Log for Logger {
    fn enabled(&self, m: &Metadata) -> bool {
        m.level() <= Level::Debug && m.target().starts_with("worklog")
    }

    fn log(&self, r: &Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let now = chrono::Local::now();
        if self.stderr.is_some_and(|lvl| r.level() <= lvl) {
            eprintln!("{} {:<5} {}", now.format("%H:%M:%S%.3f"), r.level(), r.args());
        }
        if let Some(f) = &self.file
            && let Ok(mut f) = f.lock()
        {
            let _ = writeln!(f, "{} {:<5} [{}] {}", now.format("%Y-%m-%d %H:%M:%S%.3f"), r.level(), std::process::id(), r.args());
        }
    }

    fn flush(&self) {
        if let Some(f) = &self.file
            && let Ok(mut f) = f.lock()
        {
            let _ = f.flush();
        }
    }
}
