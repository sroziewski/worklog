use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Local, NaiveDate, TimeZone};

use crate::keys::KeyMatcher;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Commit,
    Checkout,
    Uncommitted,
}

/// One timestamped trace of work in a repo, already tied to a Jira key when one could be found.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub repo: String,
    pub time: DateTime<Local>,
    pub kind: EventKind,
    /// Commit subject, checked-out branch, or changed file path.
    pub what: String,
    pub key: Option<String>,
}

/// `git` without a console window popping up when called from the GUI binary on Windows.
pub fn git(repo: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn run(mut cmd: Command) -> Result<String> {
    let t = std::time::Instant::now();
    let out = cmd.output().context("running git (is it on PATH?)")?;
    let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    log::debug!("git {} -> {} in {:.2}s, {} bytes", args.join(" "), out.status, t.elapsed().as_secs_f64(), out.stdout.len());
    if !out.status.success() {
        bail!("{:?} failed: {}", cmd, String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn global_user_email() -> Option<String> {
    let mut cmd = git(Path::new("."));
    cmd.args(["config", "--global", "user.email"]);
    run(cmd).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Repos under each root: the root itself when it is a repo, otherwise children and
/// grandchildren holding a `.git` entry (directory, or file for worktrees). Sorted, deduplicated.
pub fn discover_repos(roots: &[PathBuf]) -> Vec<PathBuf> {
    fn is_repo(p: &Path) -> bool {
        p.join(".git").exists()
    }
    fn subdirs(p: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(p)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect())
            .unwrap_or_default()
    }
    let mut out = Vec::new();
    for root in roots {
        if is_repo(root) {
            out.push(root.clone());
            continue;
        }
        for child in subdirs(root) {
            if is_repo(&child) {
                out.push(child);
            } else {
                out.extend(subdirs(&child).into_iter().filter(|g| is_repo(g)));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

pub struct Collector<'a> {
    pub matcher: &'a KeyMatcher,
    pub author_emails: &'a [String],
    pub date: NaiveDate,
    /// Uncommitted files are a snapshot of now, so they only make sense when `date` is today.
    pub include_uncommitted: bool,
}

const FS: char = '\u{1f}';
const RS: char = '\u{1e}';

impl Collector<'_> {
    /// Events of all repos. Commits reachable from several repos (forks, copies) count once.
    pub fn collect(&self, repos: &[PathBuf]) -> (Vec<Event>, Vec<String>) {
        // Repos are read in parallel (each costs a few git processes, ~0.4 s on Windows); the
        // cross-repo dedup then runs in repo order, so the first repo still wins a shared commit.
        let raw: Vec<Result<RawRepo>> = std::thread::scope(|s| {
            let handles: Vec<_> = repos
                .iter()
                .map(|repo| {
                    s.spawn(move || {
                        let t = std::time::Instant::now();
                        let r = self.collect_raw(repo);
                        if let Ok((c, o)) = &r {
                            log::info!("  {}: {} events ({:.2}s)", repo.display(), c.len() + o.len(), t.elapsed().as_secs_f64());
                        }
                        r
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap_or_else(|_| Err(anyhow::anyhow!("reader thread panicked")))).collect()
        });

        let mut events = Vec::new();
        let mut errors = Vec::new();
        let mut seen = HashSet::new();
        for (repo, r) in repos.iter().zip(raw) {
            match r {
                Ok(raw) => events.extend(dedup(raw, &mut seen)),
                Err(e) => errors.push(format!("{}: {e:#}", repo.display())),
            }
        }
        events.sort_by_key(|e| e.time);
        (events, errors)
    }

    fn is_mine(&self, email: &str) -> bool {
        self.author_emails.iter().any(|m| m.eq_ignore_ascii_case(email.trim()))
    }

    fn on_date(&self, t: &DateTime<Local>) -> bool {
        t.date_naive() == self.date
    }

    /// One repo's events, skipping commits whose hash is already in `seen` (and adding its own).
    pub fn collect_repo(&self, repo: &Path, seen: &mut HashSet<String>) -> Result<Vec<Event>> {
        Ok(dedup(self.collect_raw(repo)?, seen))
    }

    fn collect_raw(&self, repo: &Path) -> Result<RawRepo> {
        let name = repo.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let commits = self.commits(repo, &name)?;
        let mut other = self.checkouts(repo, &name)?;
        if self.include_uncommitted {
            other.extend(self.uncommitted(repo, &name)?);
        }
        Ok((commits, other))
    }

    /// Lower bound for git's committer-date filter. The day itself is decided on author date
    /// below; a commit is committed no earlier than authored, so only the lower bound is safe to
    /// pass to git (rebases and cherry-picks push the committer date arbitrarily later). One day
    /// of slack covers clock skew between machines.
    fn committed_since(&self) -> String {
        format!("{} 00:00", self.date.pred_opt().unwrap_or(self.date))
    }

    /// My commits authored on the day, with their hashes for the cross-repo dedup.
    fn commits(&self, repo: &Path, name: &str) -> Result<Vec<(String, Event)>> {
        let since = self.committed_since();
        let mut cmd = git(repo);
        // --branches --remotes --tags rather than --all: refs/stash would turn every stash into
        // a "commit" of work.
        cmd.args(["log", "--branches", "--remotes", "--tags", "--source", "--no-merges"])
            .arg(format!("--since={since}"))
            .arg(format!("--format=%H{FS}%ae{FS}%at{FS}%S{FS}%s{FS}%b{RS}"));
        let out = run(cmd)?;
        let mut events = Vec::new();
        for rec in out.split(RS) {
            let f: Vec<&str> = rec.trim_start_matches(['\n', '\r']).splitn(6, FS).collect();
            if f.len() < 6 || !self.is_mine(f[1]) {
                continue;
            }
            let Some(time) = f[2].trim().parse::<i64>().ok().and_then(|s| Local.timestamp_opt(s, 0).single()) else {
                continue;
            };
            if !self.on_date(&time) {
                continue;
            }
            let (subject, body, source) = (f[4], f[5], f[3]);
            let key = self
                .matcher
                .first(subject)
                .or_else(|| self.matcher.first(body))
                .or_else(|| self.matcher.first(source));
            events.push((
                f[0].to_owned(),
                Event { repo: name.to_owned(), time, kind: EventKind::Commit, what: subject.trim().to_owned(), key },
            ));
        }
        Ok(events)
    }

    fn checkouts(&self, repo: &Path, name: &str) -> Result<Vec<Event>> {
        let mut cmd = git(repo);
        cmd.args(["reflog", "show", "--date=unix", &format!("--format=%gd{FS}%gs"), "HEAD"]);
        // A fresh repo without a reflog makes git fail; that is "no checkouts", not an error.
        let Ok(out) = run(cmd) else { return Ok(Vec::new()) };
        let mut events = Vec::new();
        for line in out.lines() {
            let Some((selector, msg)) = line.split_once(FS) else { continue };
            let Some(target) = msg.strip_prefix("checkout: moving from ").and_then(|m| m.split_once(" to ")).map(|(_, to)| to)
            else {
                continue;
            };
            let Some(time) = selector
                .rsplit_once('{')
                .and_then(|(_, t)| t.trim_end_matches('}').parse::<i64>().ok())
                .and_then(|s| Local.timestamp_opt(s, 0).single())
            else {
                continue;
            };
            if !self.on_date(&time) {
                continue;
            }
            events.push(Event {
                repo: name.to_owned(),
                time,
                kind: EventKind::Checkout,
                what: target.trim().to_owned(),
                key: self.matcher.first(target),
            });
        }
        Ok(events)
    }

    fn uncommitted(&self, repo: &Path, name: &str) -> Result<Vec<Event>> {
        let mut cmd = git(repo);
        cmd.args(["status", "--porcelain=v1", "-z", "--untracked-files=all"]);
        let status = run(cmd)?;
        let mut branch_cmd = git(repo);
        branch_cmd.args(["branch", "--show-current"]);
        let branch = run(branch_cmd).unwrap_or_default().trim().to_owned();
        let key = self.matcher.first(&branch);

        let mut events = Vec::new();
        let mut entries = status.split('\0').filter(|s| !s.is_empty());
        while let Some(entry) = entries.next() {
            if xy_is_copy_or_rename(entry) {
                // -z puts the source path in the next field, which has no status prefix.
                entries.next();
            }
            if entry.len() <= 3 {
                continue;
            }
            let (xy, path) = entry.split_at(3);
            if xy.trim() == "D" {
                continue;
            }
            let Ok(modified) = std::fs::metadata(repo.join(path)).and_then(|m| m.modified()) else { continue };
            let time: DateTime<Local> = modified.into();
            if self.on_date(&time) {
                events.push(Event { repo: name.to_owned(), time, kind: EventKind::Uncommitted, what: path.to_owned(), key: key.clone() });
            }
        }
        Ok(events)
    }
}

/// Either status column may carry the rename/copy (`R `, ` R` after `git add -N`, `RM`, ...).
fn xy_is_copy_or_rename(entry: &str) -> bool {
    entry.chars().take(2).any(|c| c == 'R' || c == 'C')
}

/// A repo's commits (with hashes) and its other events, before the cross-repo dedup.
type RawRepo = (Vec<(String, Event)>, Vec<Event>);

fn dedup((commits, other): RawRepo, seen: &mut HashSet<String>) -> Vec<Event> {
    commits.into_iter().filter(|(hash, _)| seen.insert(hash.clone())).map(|(_, e)| e).chain(other).collect()
}

/// Lock files and the like: large, and say nothing about what the work was for.
const NOISE_PATHSPECS: [&str; 5] = [":(exclude)*.lock", ":(exclude)*package-lock.json", ":(exclude)*.min.js", ":(exclude)*.svg", ":(exclude)*.snap"];

/// `NOISE_PATHSPECS` for a single path (uncommitted files are looked up one by one).
fn is_noise(path: &str) -> bool {
    let p = path.to_lowercase();
    [".lock", "package-lock.json", ".min.js", ".svg", ".snap"].iter().any(|s| p.ends_with(s))
}

/// How much of an untracked file is read: enough for any `per_item` cut.
const UNTRACKED_READ_BYTES: u64 = 256 * 1024;

/// What each work line changed, for the LLM to understand the work: a commit's message, file list
/// and diff, or an uncommitted file's diff (its start when untracked). `lines` pairs the repos a
/// line may come from with the line as the main window shows it (`checkout x`, `uncommitted x`, or
/// a commit subject, matched against the repos' commits authored on `date`; with several, the
/// day's earliest).
/// Checkouts and lines nothing is found for give `None`. Each text is cut to `per_item` chars and
/// all of them together to `total` (later lines get what is left).
pub fn change_contexts(lines: &[(Vec<PathBuf>, String)], date: NaiveDate, per_item: usize, total: usize) -> Vec<Option<String>> {
    let mut subjects: std::collections::HashMap<PathBuf, Vec<(String, String)>> = std::collections::HashMap::new();
    let mut left = total;
    lines
        .iter()
        .map(|(repos, line)| {
            if left == 0 || line.starts_with("checkout ") {
                return None;
            }
            let text = match line.strip_prefix("uncommitted ") {
                Some(path) => repos.iter().find_map(|r| uncommitted_change(r, path)),
                None => repos.iter().find_map(|r| {
                    let list = subjects.entry(r.clone()).or_insert_with(|| day_subjects(r, date));
                    let hash = list.iter().find(|(_, s)| s == line.trim()).map(|(h, _)| h.clone())?;
                    commit_change(r, &hash)
                }),
            }?;
            let cut = cut_chars(&text, per_item.min(left));
            left = left.saturating_sub(cut.chars().count());
            Some(cut)
        })
        .collect()
}

/// (hash, subject) of the commits authored on `date` (local time), oldest first. The committer
/// date filter only narrows git's work (as in `Collector::commits`); a later commit with the same
/// subject must not stand in for the day's.
fn day_subjects(repo: &Path, date: NaiveDate) -> Vec<(String, String)> {
    let since = format!("{} 00:00", date.pred_opt().unwrap_or(date));
    let mut cmd = git(repo);
    cmd.args(["log", "--branches", "--remotes", "--tags", "--no-merges", "--reverse"])
        .arg(format!("--since={since}"))
        .arg(format!("--format=%H{FS}%at{FS}%s"));
    let Ok(out) = run(cmd) else { return Vec::new() };
    out.lines()
        .filter_map(|l| {
            let mut f = l.splitn(3, FS);
            let (hash, at, subject) = (f.next()?, f.next()?, f.next()?);
            let t = Local.timestamp_opt(at.trim().parse().ok()?, 0).single()?;
            (t.date_naive() == date).then(|| (hash.to_owned(), subject.trim().to_owned()))
        })
        .collect()
}

fn commit_change(repo: &Path, hash: &str) -> Option<String> {
    let mut cmd = git(repo);
    cmd.args(["show", "--no-color", "--stat", "--patch", "-U2", "--format=%B", hash, "--", "."]).args(NOISE_PATHSPECS);
    run(cmd).ok().filter(|s| !s.trim().is_empty())
}

fn uncommitted_change(repo: &Path, path: &str) -> Option<String> {
    if is_noise(path) || !repo.join(path).exists() {
        return None;
    }
    let mut cmd = git(repo);
    cmd.args(["diff", "--no-color", "-U2", "HEAD", "--", path]);
    if let Some(diff) = run(cmd).ok().filter(|s| !s.trim().is_empty()) {
        return Some(diff);
    }
    // No diff: untracked (its content shows what it is), or unchanged here, when the next repo of
    // the same name may hold the change.
    let mut status = git(repo);
    status.args(["status", "--porcelain=v1", "--untracked-files=all", "--", path]);
    if !run(status).ok()?.starts_with("??") {
        return None;
    }
    use std::io::Read;
    let mut head = Vec::new();
    std::fs::File::open(repo.join(path)).ok()?.take(UNTRACKED_READ_BYTES).read_to_end(&mut head).ok()?;
    // Binary content would reach the model as replacement characters.
    if head.contains(&0) {
        return None;
    }
    Some(format!("new file {path}:\n{}", String::from_utf8_lossy(&head)))
}

fn cut_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}\n… (cut)", &s[..i]),
        None => s.to_owned(),
    }
}
