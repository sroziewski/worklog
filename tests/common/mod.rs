#![allow(dead_code)]
//! Shared fixtures: a local Jira mock (tiny_http on 127.0.0.1, ephemeral port) and temp git repos
//! built without the machine's global or system git config.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, TimeZone};

pub const ME: &str = "me@example.test";

// ---------------------------------------------------------------------------------------------
// Jira mock
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub url: String,
    pub authorization: Option<String>,
    pub content_type: Option<String>,
    pub body: String,
}

pub struct MockJira {
    pub base: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
    server: Arc<tiny_http::Server>,
    handle: Option<JoinHandle<()>>,
}

impl MockJira {
    /// `handler(method, url, body) -> (status, response body)`.
    pub fn start<F>(handler: F) -> Self
    where
        F: Fn(&str, &str, &str) -> (u16, String) + Send + 'static,
    {
        let server = Arc::new(tiny_http::Server::http("127.0.0.1:0").expect("bind mock"));
        let port = server.server_addr().to_ip().expect("ip listener").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (srv, reqs) = (Arc::clone(&server), Arc::clone(&requests));
        let handle = std::thread::spawn(move || {
            for mut req in srv.incoming_requests() {
                let header = |name: &str| {
                    req.headers()
                        .iter()
                        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
                        .map(|h| h.value.as_str().to_owned())
                };
                let (authorization, content_type) = (header("Authorization"), header("Content-Type"));
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                let method = req.method().as_str().to_owned();
                let url = req.url().to_owned();
                let (status, resp) = handler(&method, &url, &body);
                reqs.lock().unwrap().push(Recorded { method, url, authorization, content_type, body });
                let response = tiny_http::Response::from_string(resp)
                    .with_status_code(status)
                    .with_header("Content-Type: application/json".parse::<tiny_http::Header>().unwrap());
                let _ = req.respond(response);
            }
        });
        Self { base: format!("http://127.0.0.1:{port}"), requests, server, handle: Some(handle) }
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for MockJira {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Independent percent-decoder, so assertions on encoded URLs do not reuse `jira::url_encode`.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

/// Value of a query parameter, percent-decoded.
pub fn query_param(url: &str, name: &str) -> Option<String> {
    let q = url.split_once('?')?.1;
    q.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == name).map(|(_, v)| percent_decode(v)))
}

// ---------------------------------------------------------------------------------------------
// Time
// ---------------------------------------------------------------------------------------------

pub fn ndt(date: NaiveDate, h: u32, m: u32, s: u32) -> NaiveDateTime {
    date.and_hms_opt(h, m, s).unwrap()
}

pub fn local_at(date: NaiveDate, h: u32, m: u32, s: u32) -> DateTime<Local> {
    Local.from_local_datetime(&ndt(date, h, m, s)).single().expect("unambiguous local time")
}

pub fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

// ---------------------------------------------------------------------------------------------
// Git fixtures
// ---------------------------------------------------------------------------------------------

static EMPTY_CONFIG: OnceLock<PathBuf> = OnceLock::new();

/// An empty file used as GIT_CONFIG_GLOBAL, also set on this test process so the production
/// `git` calls do not read the machine's global/system config either.
///
/// Every helper that leads to a `git` process (fixtures, `collect_with`, `report::build` tests)
/// calls this first. `OnceLock` runs the `set_var` exactly once and makes every other caller wait
/// until it is done, so no thread of a test binary spawns git while the variables change.
pub fn empty_git_config() -> PathBuf {
    EMPTY_CONFIG
        .get_or_init(|| {
            let f = tempfile::NamedTempFile::new().unwrap();
            let (_, path) = f.keep().unwrap();
            // SAFETY: runs once; on Windows env access is serialized by the OS and std's env lock.
            unsafe {
                std::env::set_var("GIT_CONFIG_GLOBAL", &path);
                std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
            }
            path
        })
        .clone()
}

/// `git` date with an explicit local offset, so the fixture does not depend on how git reads a
/// zone-less date.
pub fn git_date(t: &DateTime<Local>) -> String {
    t.format("%Y-%m-%dT%H:%M:%S%z").to_string()
}

/// Runs git in `dir` as `email`, with author date `author` and committer (and reflog) date
/// `committer`.
pub fn git_as(dir: &Path, author: &DateTime<Local>, committer: &DateTime<Local>, email: &str, args: &[&str]) -> String {
    let cfg = empty_git_config();
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "init.defaultBranch=main", "-c", "user.name=Fixture User", "-c"])
        .arg(format!("user.email={email}"))
        .args(["-c", "commit.gpgsign=false", "-c", "core.autocrlf=false", "-c", "core.quotepath=false"])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", &cfg)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Fixture User")
        .env("GIT_COMMITTER_NAME", "Fixture User")
        .env("GIT_AUTHOR_EMAIL", email)
        .env("GIT_COMMITTER_EMAIL", email)
        .env("GIT_AUTHOR_DATE", git_date(author))
        .env("GIT_COMMITTER_DATE", git_date(committer))
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Same date for author and committer, as me.
pub fn git_at(dir: &Path, when: &DateTime<Local>, args: &[&str]) -> String {
    git_as(dir, when, when, ME, args)
}

pub fn init_repo(dir: &Path, when: &DateTime<Local>) {
    std::fs::create_dir_all(dir).unwrap();
    git_at(dir, when, &["init", "-q"]);
}

pub fn commit(dir: &Path, when: &DateTime<Local>, subject: &str, body: &str) {
    git_as(dir, when, when, ME, &["commit", "-q", "--allow-empty", "-m", subject, "-m", body]);
}

pub fn set_mtime(path: &Path, t: &DateTime<Local>) {
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::from(*t)).unwrap();
}
