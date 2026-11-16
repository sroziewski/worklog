//! git::Collector and discover_repos against throw-away repos in temp dirs.

mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, NaiveDate};
use common::{ME, commit, day, git_as, git_at, init_repo, local_at, set_mtime};
use worklog::git::{self, Collector, Event, EventKind};
use worklog::keys::KeyMatcher;

/// The day under test; D-1 .. D+1 are its neighbours.
fn d() -> NaiveDate {
    day(2026, 11, 4)
}
fn at(date: NaiveDate, h: u32, m: u32, s: u32) -> DateTime<Local> {
    local_at(date, h, m, s)
}
fn before(n: i64) -> NaiveDate {
    d() - chrono::TimeDelta::days(n)
}
fn after(n: i64) -> NaiveDate {
    d() + chrono::TimeDelta::days(n)
}

fn matcher() -> KeyMatcher {
    KeyMatcher::new(&["ABC".to_string(), "XYZ".to_string()])
}

fn collect_with(repos: &[PathBuf], emails: &[&str], uncommitted: bool) -> (Vec<Event>, Vec<String>) {
    common::empty_git_config();
    let m = matcher();
    let emails: Vec<String> = emails.iter().map(|e| e.to_string()).collect();
    Collector { matcher: &m, author_emails: &emails, date: d(), include_uncommitted: uncommitted }.collect(repos)
}

fn collect(repos: &[PathBuf]) -> Vec<Event> {
    let (events, errors) = collect_with(repos, &[ME], false);
    assert_eq!(errors, Vec::<String>::new());
    events
}

fn event(repo: &str, time: DateTime<Local>, kind: EventKind, what: &str, key: Option<&str>) -> Event {
    Event { repo: repo.into(), time, kind, what: what.into(), key: key.map(str::to_owned) }
}

/// Repo with an initial commit three days before D, so setup history stays outside the day.
fn new_repo(parent: &Path, name: &str) -> PathBuf {
    let dir = parent.join(name);
    let t = at(before(3), 8, 0, 0);
    init_repo(&dir, &t);
    commit(&dir, &t, "init", "");
    dir
}

mod commits {
    use super::*;

    #[test]
    fn only_my_author_emails_count_case_insensitively() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        let (t1, t2, t3, t4) = (at(d(), 10, 0, 0), at(d(), 11, 0, 0), at(d(), 12, 0, 0), at(d(), 13, 0, 0));
        git_as(&r, &t1, &t1, ME, &["commit", "-q", "--allow-empty", "-m", "ABC-1 mine"]);
        git_as(&r, &t2, &t2, "colleague@example.test", &["commit", "-q", "--allow-empty", "-m", "ABC-2 theirs"]);
        git_as(&r, &t3, &t3, "ME@Example.TEST", &["commit", "-q", "--allow-empty", "-m", "ABC-3 mine upper"]);
        git_as(&r, &t4, &t4, "second@example.test", &["commit", "-q", "--allow-empty", "-m", "ABC-4 second address"]);

        let (events, errors) = collect_with(&[r], &[ME, "second@example.test"], false);
        assert_eq!(errors, Vec::<String>::new());
        assert_eq!(
            events,
            vec![
                event("svc", t1, EventKind::Commit, "ABC-1 mine", Some("ABC-1")),
                event("svc", t3, EventKind::Commit, "ABC-3 mine upper", Some("ABC-3")),
                event("svc", t4, EventKind::Commit, "ABC-4 second address", Some("ABC-4")),
            ]
        );
    }

    #[test]
    fn no_author_emails_means_no_commits() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        commit(&r, &at(d(), 10, 0, 0), "ABC-1 mine", "");
        let (events, errors) = collect_with(&[r], &[], false);
        assert_eq!((events, errors), (vec![], vec![]));
    }

    #[test]
    fn the_day_is_decided_by_local_author_date() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        // (subject, author, committer) - committer dates ascend along the chain.
        let chain = [
            ("ABC-101 day before", at(before(1), 23, 59, 59), at(before(1), 23, 59, 59)),
            ("ABC-102 midnight start", at(d(), 0, 0, 0), at(d(), 0, 0, 0)),
            ("ABC-103 old author rebased today", at(before(5), 10, 0, 0), at(d(), 12, 0, 0)),
            ("ABC-104 last second", at(d(), 23, 59, 59), at(d(), 23, 59, 59)),
            ("ABC-105 next day", at(after(1), 0, 0, 0), at(after(1), 0, 0, 0)),
            ("ABC-106 committed next morning", at(d(), 13, 0, 0), at(after(1), 9, 0, 0)),
        ];
        for (subject, author, committer) in &chain {
            git_as(&r, author, committer, ME, &["commit", "-q", "--allow-empty", "-m", subject]);
        }
        assert_eq!(
            collect(&[r]),
            vec![
                event("svc", at(d(), 0, 0, 0), EventKind::Commit, "ABC-102 midnight start", Some("ABC-102")),
                event("svc", at(d(), 13, 0, 0), EventKind::Commit, "ABC-106 committed next morning", Some("ABC-106")),
                event("svc", at(d(), 23, 59, 59), EventKind::Commit, "ABC-104 last second", Some("ABC-104")),
            ]
        );
    }

    // AC 1: the day is the AUTHOR date. A commit written on D and rebased/cherry-picked three days
    // later still belongs to D.
    #[test]
    fn commit_authored_on_the_day_but_rebased_days_later_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        git_as(&r, &at(d(), 15, 0, 0), &at(after(3), 9, 0, 0), ME, &["commit", "-q", "--allow-empty", "-m", "ABC-107 rebased later"]);
        assert_eq!(
            collect(&[r]),
            vec![event("svc", at(d(), 15, 0, 0), EventKind::Commit, "ABC-107 rebased later", Some("ABC-107"))]
        );
    }

    #[test]
    fn merge_commits_are_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        let setup = at(before(3), 9, 0, 0);
        git_at(&r, &setup, &["checkout", "-q", "-b", "feat"]);
        commit(&r, &at(d(), 10, 0, 0), "ABC-21 feature", "");
        git_at(&r, &at(before(3), 9, 30, 0), &["checkout", "-q", "main"]);
        let merge_time = at(d(), 11, 0, 0);
        git_at(&r, &merge_time, &["merge", "-q", "--no-ff", "-m", "Merge ABC-22 branch feat", "feat"]);
        assert_eq!(collect(&[r]), vec![event("svc", at(d(), 10, 0, 0), EventKind::Commit, "ABC-21 feature", Some("ABC-21"))]);
    }

    #[test]
    fn stash_is_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        std::fs::write(r.join("a.txt"), "one").unwrap();
        git_at(&r, &at(before(3), 9, 0, 0), &["add", "a.txt"]);
        commit(&r, &at(d(), 9, 0, 0), "ABC-8 real work", "");
        std::fs::write(r.join("a.txt"), "two").unwrap();
        git_at(&r, &at(d(), 10, 0, 0), &["stash", "push", "-q", "-m", "ABC-9 stashed"]);
        let stash_list = git_at(&r, &at(d(), 10, 0, 0), &["stash", "list"]);
        assert_eq!(stash_list.lines().count(), 1, "fixture must hold a stash");
        assert_eq!(collect(&[r]), vec![event("svc", at(d(), 9, 0, 0), EventKind::Commit, "ABC-8 real work", Some("ABC-8"))]);
    }

    #[test]
    fn remote_and_tag_refs_are_read() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        let setup = at(before(3), 9, 0, 0);
        // A commit reachable only from a tag, another only from a remote-tracking ref.
        git_at(&r, &setup, &["checkout", "-q", "-b", "tmp-tag"]);
        commit(&r, &at(d(), 10, 0, 0), "ABC-31 tagged", "");
        git_at(&r, &at(d(), 10, 0, 0), &["tag", "v1"]);
        git_at(&r, &setup, &["checkout", "-q", "main"]);
        git_at(&r, &setup, &["branch", "-q", "-D", "tmp-tag"]);
        git_at(&r, &setup, &["checkout", "-q", "-b", "tmp-remote"]);
        commit(&r, &at(d(), 11, 0, 0), "ABC-32 on remote", "");
        git_at(&r, &setup, &["update-ref", "refs/remotes/origin/feature", "HEAD"]);
        git_at(&r, &setup, &["checkout", "-q", "main"]);
        git_at(&r, &setup, &["branch", "-q", "-D", "tmp-remote"]);
        assert_eq!(
            collect(&[r]),
            vec![
                event("svc", at(d(), 10, 0, 0), EventKind::Commit, "ABC-31 tagged", Some("ABC-31")),
                event("svc", at(d(), 11, 0, 0), EventKind::Commit, "ABC-32 on remote", Some("ABC-32")),
            ]
        );
    }

    #[test]
    fn key_comes_from_subject_then_body_then_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        git_at(&r, &at(before(2), 9, 0, 0), &["checkout", "-q", "-b", "feat/abc-1439-x"]);
        commit(&r, &at(d(), 10, 0, 0), "XYZ-5 subject wins", "ABC-4 in body");
        commit(&r, &at(d(), 11, 0, 0), "no key in subject", "relates to abc_4");
        commit(&r, &at(d(), 12, 0, 0), "plain subject", "plain body AAA-9");
        git_at(&r, &at(before(2), 9, 30, 0), &["checkout", "-q", "main"]);
        commit(&r, &at(d(), 13, 0, 0), "main without key", "");
        assert_eq!(
            collect(&[r]),
            vec![
                event("svc", at(d(), 10, 0, 0), EventKind::Commit, "XYZ-5 subject wins", Some("XYZ-5")),
                event("svc", at(d(), 11, 0, 0), EventKind::Commit, "no key in subject", Some("ABC-4")),
                event("svc", at(d(), 12, 0, 0), EventKind::Commit, "plain subject", Some("ABC-1439")),
                event("svc", at(d(), 13, 0, 0), EventKind::Commit, "main without key", None),
            ]
        );
    }

    #[test]
    fn a_commit_in_a_clone_counts_once() {
        let tmp = tempfile::tempdir().unwrap();
        let a = new_repo(tmp.path(), "orig");
        commit(&a, &at(d(), 10, 0, 0), "ABC-5 shared", "");
        let b = tmp.path().join("copy");
        git_at(tmp.path(), &at(before(1), 9, 0, 0), &["clone", "-q", a.to_str().unwrap(), b.to_str().unwrap()]);
        commit(&b, &at(d(), 11, 0, 0), "ABC-6 only in clone", "");
        assert_eq!(
            collect(&[a, b]),
            vec![
                event("orig", at(d(), 10, 0, 0), EventKind::Commit, "ABC-5 shared", Some("ABC-5")),
                event("copy", at(d(), 11, 0, 0), EventKind::Commit, "ABC-6 only in clone", Some("ABC-6")),
            ]
        );
    }

    #[test]
    fn seen_set_is_shared_across_collect_repo_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        commit(&r, &at(d(), 10, 0, 0), "ABC-5 once", "");
        let m = matcher();
        let emails = vec![ME.to_string()];
        let c = Collector { matcher: &m, author_emails: &emails, date: d(), include_uncommitted: false };
        let mut seen = HashSet::new();
        assert_eq!(c.collect_repo(&r, &mut seen).unwrap().len(), 1);
        assert_eq!(c.collect_repo(&r, &mut seen).unwrap(), vec![]);
    }

    #[test]
    fn unreadable_repo_is_reported_and_others_still_collected() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("not-a-repo");
        std::fs::create_dir_all(&bad).unwrap();
        let good = new_repo(tmp.path(), "svc");
        commit(&good, &at(d(), 10, 0, 0), "ABC-1 ok", "");
        let (events, errors) = collect_with(&[bad.clone(), good], &[ME], false);
        assert_eq!(events, vec![event("svc", at(d(), 10, 0, 0), EventKind::Commit, "ABC-1 ok", Some("ABC-1"))]);
        assert_eq!(errors.len(), 1, "{errors:?}");
        // The rest of the message is git's stderr.
        assert!(errors[0].starts_with(&format!("{}: ", bad.display())), "{errors:?}");
    }
}

mod checkouts {
    use super::*;

    #[test]
    fn checkouts_on_the_day_with_key_from_target_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        git_at(&r, &at(before(1), 18, 0, 0), &["checkout", "-q", "-b", "feat/old-day"]);
        git_at(&r, &at(d(), 8, 30, 0), &["checkout", "-q", "-b", "feat/abc-77-login"]);
        git_at(&r, &at(d(), 9, 10, 0), &["checkout", "-q", "main"]);
        git_at(&r, &at(d(), 9, 20, 0), &["checkout", "-q", "-b", "x"]);
        git_at(&r, &at(after(1), 0, 0, 0), &["checkout", "-q", "main"]);
        assert_eq!(
            collect(&[r]),
            vec![
                event("svc", at(d(), 8, 30, 0), EventKind::Checkout, "feat/abc-77-login", Some("ABC-77")),
                event("svc", at(d(), 9, 10, 0), EventKind::Checkout, "main", None),
                event("svc", at(d(), 9, 20, 0), EventKind::Checkout, "x", None),
            ]
        );
    }

    #[test]
    fn checkout_of_branch_with_key_in_underscore_form() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        git_at(&r, &at(d(), 14, 0, 0), &["checkout", "-q", "-b", "fix/xyz_12-upload"]);
        assert_eq!(
            collect(&[r]),
            vec![event("svc", at(d(), 14, 0, 0), EventKind::Checkout, "fix/xyz_12-upload", Some("XYZ-12"))]
        );
    }
}

mod uncommitted {
    use super::*;

    /// Branch `feature/XYZ-12_upload` with committed a/b/c/old files (all three days before D),
    /// then: a modified (D 14:00), new untracked file in a subdir (D 15:00), c deleted, old renamed
    /// (D 16:00), b modified but last touched D-1.
    fn fixture(tmp: &Path) -> PathBuf {
        let r = new_repo(tmp, "svc");
        let setup = at(before(3), 9, 0, 0);
        git_at(&r, &setup, &["checkout", "-q", "-b", "feature/XYZ-12_upload"]);
        for f in ["a.txt", "b.txt", "c.txt", "old.txt"] {
            std::fs::write(r.join(f), format!("{f} v1")).unwrap();
        }
        git_at(&r, &setup, &["add", "."]);
        commit(&r, &setup, "files", "");

        std::fs::write(r.join("a.txt"), "a v2").unwrap();
        set_mtime(&r.join("a.txt"), &at(d(), 14, 0, 0));
        std::fs::create_dir_all(r.join("sub dir")).unwrap();
        let new_file = r.join("sub dir").join("new ż.txt");
        std::fs::write(&new_file, "new").unwrap();
        set_mtime(&new_file, &at(d(), 15, 0, 0));
        std::fs::remove_file(r.join("c.txt")).unwrap();
        git_at(&r, &setup, &["mv", "old.txt", "renamed.txt"]);
        set_mtime(&r.join("renamed.txt"), &at(d(), 16, 0, 0));
        std::fs::write(r.join("b.txt"), "b v2").unwrap();
        set_mtime(&r.join("b.txt"), &at(before(1), 12, 0, 0));
        r
    }

    #[test]
    fn changed_and_untracked_files_of_the_day_with_branch_key() {
        let tmp = tempfile::tempdir().unwrap();
        let r = fixture(tmp.path());
        let (events, errors) = collect_with(&[r], &[ME], true);
        assert_eq!(errors, Vec::<String>::new());
        let k = Some("XYZ-12");
        assert_eq!(
            events,
            vec![
                event("svc", at(d(), 14, 0, 0), EventKind::Uncommitted, "a.txt", k),
                event("svc", at(d(), 15, 0, 0), EventKind::Uncommitted, "sub dir/new ż.txt", k),
                event("svc", at(d(), 16, 0, 0), EventKind::Uncommitted, "renamed.txt", k),
            ]
        );
    }

    #[test]
    fn staged_deletion_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let r = fixture(tmp.path());
        git_at(&r, &at(d(), 9, 0, 0), &["rm", "-q", "--cached", "a.txt"]);
        // a.txt is now "D " in the index and "??" in the worktree: only the untracked entry counts,
        // so it appears once.
        let (events, errors) = collect_with(&[r], &[ME], true);
        assert_eq!(errors, Vec::<String>::new());
        let k = Some("XYZ-12");
        assert_eq!(
            events,
            vec![
                event("svc", at(d(), 14, 0, 0), EventKind::Uncommitted, "a.txt", k),
                event("svc", at(d(), 15, 0, 0), EventKind::Uncommitted, "sub dir/new ż.txt", k),
                event("svc", at(d(), 16, 0, 0), EventKind::Uncommitted, "renamed.txt", k),
            ]
        );
    }

    // With -z the rename source "a.c" is a field of its own; it must not swallow the entry after
    // it (" M x.txt").
    #[test]
    fn rename_with_short_source_path_keeps_the_following_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        let setup = at(before(3), 9, 0, 0);
        std::fs::write(r.join("a.c"), "int a;").unwrap();
        std::fs::write(r.join("x.txt"), "x v1").unwrap();
        git_at(&r, &setup, &["add", "."]);
        commit(&r, &setup, "files", "");
        git_at(&r, &setup, &["mv", "a.c", "b.c"]);
        set_mtime(&r.join("b.c"), &at(d(), 10, 0, 0));
        std::fs::write(r.join("x.txt"), "x v2").unwrap();
        set_mtime(&r.join("x.txt"), &at(d(), 11, 0, 0));
        let status = git_at(&r, &setup, &["status", "--porcelain=v1"]);
        assert_eq!(status, "R  a.c -> b.c\n M x.txt\n", "fixture");

        let (events, errors) = collect_with(&[r], &[ME], true);
        assert_eq!(errors, Vec::<String>::new());
        assert_eq!(
            events,
            vec![
                event("svc", at(d(), 10, 0, 0), EventKind::Uncommitted, "b.c", None),
                event("svc", at(d(), 11, 0, 0), EventKind::Uncommitted, "x.txt", None),
            ]
        );
    }

    // A worktree-only rename (intent-to-add) is reported in the Y column (" R"). Its source field
    // "zz_b.c" must be skipped: read as an entry it would yield the path "b.c" a second time.
    #[test]
    fn worktree_rename_in_y_column_skips_its_source_field() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        let setup = at(before(3), 9, 0, 0);
        std::fs::write(r.join("zz_b.c"), "int a;\nint b;\nint c;\n").unwrap();
        std::fs::write(r.join("x.txt"), "x v1").unwrap();
        git_at(&r, &setup, &["add", "."]);
        commit(&r, &setup, "files", "");
        std::fs::rename(r.join("zz_b.c"), r.join("b.c")).unwrap();
        git_at(&r, &setup, &["add", "-N", "b.c"]);
        set_mtime(&r.join("b.c"), &at(d(), 10, 0, 0));
        std::fs::write(r.join("x.txt"), "x v2").unwrap();
        set_mtime(&r.join("x.txt"), &at(d(), 11, 0, 0));
        let status = git_at(&r, &setup, &["status", "--porcelain=v1", "-z", "--untracked-files=all"]);
        assert_eq!(status, " R b.c\0zz_b.c\0 M x.txt\0", "fixture");

        let (events, errors) = collect_with(&[r], &[ME], true);
        assert_eq!(errors, Vec::<String>::new());
        assert_eq!(
            events,
            vec![
                event("svc", at(d(), 10, 0, 0), EventKind::Uncommitted, "b.c", None),
                event("svc", at(d(), 11, 0, 0), EventKind::Uncommitted, "x.txt", None),
            ]
        );
    }

    #[test]
    fn not_collected_when_include_uncommitted_is_false() {
        let tmp = tempfile::tempdir().unwrap();
        let r = fixture(tmp.path());
        assert_eq!(collect_with(&[r], &[ME], false), (vec![], vec![]));
    }

    #[test]
    fn detached_head_gives_no_key() {
        let tmp = tempfile::tempdir().unwrap();
        let r = fixture(tmp.path());
        git_at(&r, &at(before(3), 9, 0, 0), &["checkout", "-q", "--detach"]);
        let (events, errors) = collect_with(&[r], &[ME], true);
        assert_eq!(errors, Vec::<String>::new());
        assert_eq!(
            events,
            vec![
                event("svc", at(d(), 14, 0, 0), EventKind::Uncommitted, "a.txt", None),
                event("svc", at(d(), 15, 0, 0), EventKind::Uncommitted, "sub dir/new ż.txt", None),
                event("svc", at(d(), 16, 0, 0), EventKind::Uncommitted, "renamed.txt", None),
            ]
        );
    }
}

mod collect_with {
    use super::*;
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;
    use worklog::git::{PARALLEL_REPOS, ScanEvent};

    /// Long enough for any git call here; a wait that runs out lets the scan go on, so a wrong
    /// implementation fails an assertion instead of hanging.
    const WAIT: Duration = Duration::from_secs(20);

    #[derive(Default)]
    struct Seen {
        /// Every event with `secs` zeroed, in the order the callback heard them.
        log: Vec<ScanEvent>,
        running: usize,
        peak: usize,
        started: usize,
    }

    /// Records what `collect_with` reports. `gate(repo)` names what a repo's `Started` waits for
    /// before the read goes on: a number of repos started, or another repo finished.
    struct Probe {
        seen: Mutex<Seen>,
        cv: Condvar,
        gate: Box<dyn Fn(&Path) -> Gate + Sync>,
    }

    enum Gate {
        Open,
        StartedAtLeast(usize),
        FinishedFirst(PathBuf),
    }

    impl Probe {
        fn new(gate: impl Fn(&Path) -> Gate + Sync + 'static) -> Self {
            Self { seen: Mutex::new(Seen::default()), cv: Condvar::new(), gate: Box::new(gate) }
        }

        fn on_event(&self, e: ScanEvent) {
            let mut s = self.seen.lock().unwrap();
            match e {
                ScanEvent::Started(repo) => {
                    s.started += 1;
                    s.running += 1;
                    s.peak = s.peak.max(s.running);
                    s.log.push(ScanEvent::Started(repo.clone()));
                    self.cv.notify_all();
                    let ready = |s: &mut Seen| match (self.gate)(&repo) {
                        Gate::Open => true,
                        Gate::StartedAtLeast(n) => s.started >= n,
                        Gate::FinishedFirst(other) => s.log.iter().any(|x| matches!(x, ScanEvent::Finished { repo, .. } if *repo == other)),
                    };
                    let _ = self.cv.wait_timeout_while(s, WAIT, |s| !ready(s)).unwrap();
                }
                ScanEvent::Finished { repo, events, .. } => {
                    s.running -= 1;
                    s.log.push(ScanEvent::Finished { repo, events, secs: 0.0 });
                    self.cv.notify_all();
                }
            }
        }

        fn scan(&self, repos: &[PathBuf]) -> (Vec<Event>, Vec<String>) {
            common::empty_git_config();
            let m = matcher();
            let emails = vec![ME.to_string()];
            Collector { matcher: &m, author_emails: &emails, date: d(), include_uncommitted: false }.collect_with(repos, &|e| self.on_event(e))
        }

        fn log(&self) -> Vec<ScanEvent> {
            self.seen.lock().unwrap().log.clone()
        }

        /// The repos in the order they finished.
        fn finish_order(&self) -> Vec<PathBuf> {
            self.log().into_iter().filter_map(|e| if let ScanEvent::Finished { repo, .. } = e { Some(repo) } else { None }).collect()
        }
    }

    fn finished(repo: &Path, events: Option<usize>) -> ScanEvent {
        ScanEvent::Finished { repo: repo.to_path_buf(), events, secs: 0.0 }
    }

    /// Seven repos `r0`..`r6`; repo `ri` has i commits on D (r0 none), so each has its own count.
    fn seven_repos(tmp: &Path) -> Vec<PathBuf> {
        (0..7)
            .map(|i| {
                let r = new_repo(tmp, &format!("r{i}"));
                for j in 0..i {
                    commit(&r, &at(d(), 9 + i as u32, j as u32, 0), &format!("ABC-{} r{i} work {j}", 100 + i), "");
                }
                r
            })
            .collect()
    }

    #[test]
    fn reads_parallel_repos_at_a_time_and_reports_each_repo_once() {
        let tmp = tempfile::tempdir().unwrap();
        let repos = seven_repos(tmp.path());
        // Every Started waits until PARALLEL_REPOS repos have started: the peak reaches exactly
        // PARALLEL_REPOS when that many are read at once, and goes past it when more are.
        let probe = Probe::new(|_| Gate::StartedAtLeast(PARALLEL_REPOS));
        let (events, errors) = probe.scan(&repos);

        assert_eq!(errors, Vec::<String>::new());
        assert_eq!(events.len(), 21, "0 + 1 + .. + 6 commits");
        let s = probe.seen.lock().unwrap();
        assert_eq!((s.peak, s.running, s.started), (PARALLEL_REPOS, 0, 7));
        assert_eq!(s.log.len(), 14, "{:?}", s.log);
        for (i, repo) in repos.iter().enumerate() {
            let started = s.log.iter().position(|e| *e == ScanEvent::Started(repo.clone()));
            let done = s.log.iter().position(|e| *e == finished(repo, Some(i)));
            assert!(matches!((started, done), (Some(a), Some(b)) if a < b), "r{i}: {started:?} {done:?} in {:?}", s.log);
        }
    }

    #[test]
    fn parallel_repos_is_five() {
        assert_eq!(PARALLEL_REPOS, 5);
    }

    #[test]
    fn same_result_as_collect() {
        let tmp = tempfile::tempdir().unwrap();
        let repos = seven_repos(tmp.path());
        let probe = Probe::new(|_| Gate::Open);
        assert_eq!(probe.scan(&repos), collect_with(&repos, &[ME], false));
    }

    #[test]
    fn a_shared_commit_goes_to_the_first_repo_in_the_list_even_when_it_finishes_last() {
        let tmp = tempfile::tempdir().unwrap();
        let orig = new_repo(tmp.path(), "orig");
        commit(&orig, &at(d(), 10, 0, 0), "ABC-5 shared", "");
        let copy = tmp.path().join("copy");
        git_at(tmp.path(), &at(before(1), 9, 0, 0), &["clone", "-q", orig.to_str().unwrap(), copy.to_str().unwrap()]);
        commit(&copy, &at(d(), 11, 0, 0), "ABC-6 only in copy", "");
        commit(&orig, &at(d(), 12, 0, 0), "ABC-7 only in orig", "");

        // `copy` is first in the list but waits for `orig` to finish before it is read.
        let orig_done = orig.clone();
        let copy_slow = copy.clone();
        let probe = Probe::new(move |r| if r == copy_slow { Gate::FinishedFirst(orig_done.clone()) } else { Gate::Open });
        let (events, errors) = probe.scan(&[copy.clone(), orig.clone()]);

        assert_eq!(probe.finish_order(), vec![orig.clone(), copy.clone()], "fixture: the first repo finishes last");
        assert_eq!(errors, Vec::<String>::new());
        assert_eq!(
            events,
            vec![
                event("copy", at(d(), 10, 0, 0), EventKind::Commit, "ABC-5 shared", Some("ABC-5")),
                event("copy", at(d(), 11, 0, 0), EventKind::Commit, "ABC-6 only in copy", Some("ABC-6")),
                event("orig", at(d(), 12, 0, 0), EventKind::Commit, "ABC-7 only in orig", Some("ABC-7")),
            ]
        );
    }

    #[test]
    fn failed_repos_finish_without_a_count_and_their_errors_keep_the_list_order() {
        let tmp = tempfile::tempdir().unwrap();
        let good_a = new_repo(tmp.path(), "alpha");
        commit(&good_a, &at(d(), 10, 0, 0), "ABC-1 alpha", "");
        let bad_1 = tmp.path().join("bad-one");
        let good_b = new_repo(tmp.path(), "beta");
        commit(&good_b, &at(d(), 11, 0, 0), "XYZ-2 beta", "");
        commit(&good_b, &at(d(), 11, 30, 0), "XYZ-3 beta", "");
        let bad_2 = tmp.path().join("bad-two");
        for bad in [&bad_1, &bad_2] {
            std::fs::create_dir_all(bad).unwrap();
        }
        let repos = vec![good_a.clone(), bad_1.clone(), good_b.clone(), bad_2.clone()];

        // The first failing repo is read only after the second one failed.
        let (first, second) = (bad_1.clone(), bad_2.clone());
        let probe = Probe::new(move |r| if r == first { Gate::FinishedFirst(second.clone()) } else { Gate::Open });
        let (events, errors) = probe.scan(&repos);

        let order = probe.finish_order();
        let pos = |r: &PathBuf| order.iter().position(|x| x == r).unwrap();
        assert!(pos(&bad_2) < pos(&bad_1), "fixture: bad-two fails first: {order:?}");
        assert_eq!(
            events,
            vec![
                event("alpha", at(d(), 10, 0, 0), EventKind::Commit, "ABC-1 alpha", Some("ABC-1")),
                event("beta", at(d(), 11, 0, 0), EventKind::Commit, "XYZ-2 beta", Some("XYZ-2")),
                event("beta", at(d(), 11, 30, 0), EventKind::Commit, "XYZ-3 beta", Some("XYZ-3")),
            ]
        );
        assert_eq!(errors.len(), 2, "{errors:?}");
        // After "<repo>: " comes the git command and git's own stderr.
        assert!(errors[0].starts_with(&format!("{}: ", bad_1.display())), "{errors:?}");
        assert!(errors[1].starts_with(&format!("{}: ", bad_2.display())), "{errors:?}");
        // Finish order is up to the threads; compare in list order.
        let log = probe.log();
        let finish_of = |r: &PathBuf| log.iter().filter(|e| matches!(e, ScanEvent::Finished { repo, .. } if repo == r)).cloned().collect::<Vec<_>>();
        assert_eq!(
            repos.iter().flat_map(finish_of).collect::<Vec<_>>(),
            vec![finished(&good_a, Some(1)), finished(&bad_1, None), finished(&good_b, Some(2)), finished(&bad_2, None)]
        );
        assert_eq!(log.len(), 8, "{log:?}");
    }

    /// Directory name whose reading panics: the logger below panics on any record naming it, and
    /// every git call `collect_raw` makes logs its `-C <repo>` at debug level.
    const PANICS: &str = "panic-on-read";

    struct PanickingLogger;

    impl log::Log for PanickingLogger {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, r: &log::Record) {
            if r.args().to_string().contains(PANICS) {
                panic!("logger refuses {PANICS}");
            }
        }
        fn flush(&self) {}
    }

    /// Installs `PanickingLogger` for this test binary; harmless to every other test here.
    fn panicking_logger() {
        static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        ONCE.get_or_init(|| {
            log::set_boxed_logger(Box::new(PanickingLogger)).expect("no other logger in this test binary");
            log::set_max_level(log::LevelFilter::Debug);
        });
    }

    #[test]
    fn a_panic_while_reading_one_repo_costs_that_repo_only() {
        panicking_logger();
        let tmp = tempfile::tempdir().unwrap();
        let alpha = new_repo(tmp.path(), "alpha");
        commit(&alpha, &at(d(), 10, 0, 0), "ABC-1 alpha", "");
        let panicky = new_repo(tmp.path(), PANICS);
        commit(&panicky, &at(d(), 10, 30, 0), "ABC-2 lost with the panic", "");
        let beta = new_repo(tmp.path(), "beta");
        commit(&beta, &at(d(), 11, 0, 0), "XYZ-3 beta", "");
        let not_repo = tmp.path().join("not-a-repo");
        std::fs::create_dir_all(&not_repo).unwrap();
        let repos = vec![alpha.clone(), panicky.clone(), beta.clone(), not_repo.clone()];

        let probe = Probe::new(|_| Gate::Open);
        let (events, errors) = probe.scan(&repos);

        assert_eq!(
            events,
            vec![
                event("alpha", at(d(), 10, 0, 0), EventKind::Commit, "ABC-1 alpha", Some("ABC-1")),
                event("beta", at(d(), 11, 0, 0), EventKind::Commit, "XYZ-3 beta", Some("XYZ-3")),
            ]
        );
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert_eq!(errors[0], format!("{}: reader thread panicked", panicky.display()));
        // The rest of this one is the git command and git's own stderr.
        assert!(errors[1].starts_with(&format!("{}: ", not_repo.display())), "{errors:?}");
        let log = probe.log();
        let finish_of = |r: &PathBuf| log.iter().filter(|e| matches!(e, ScanEvent::Finished { repo, .. } if repo == r)).cloned().collect::<Vec<_>>();
        assert_eq!(
            repos.iter().flat_map(finish_of).collect::<Vec<_>>(),
            vec![finished(&alpha, Some(1)), finished(&panicky, None), finished(&beta, Some(1)), finished(&not_repo, None)]
        );
        assert_eq!(log.len(), 8, "{log:?}");
    }

    #[test]
    fn no_repos_no_events_and_no_callbacks() {
        let probe = Probe::new(|_| Gate::Open);
        assert_eq!(probe.scan(&[]), (vec![], vec![]));
        assert_eq!(probe.log(), vec![]);
    }

    #[test]
    fn a_single_repo_is_reported_once() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "solo");
        commit(&r, &at(d(), 10, 0, 0), "ABC-9 solo", "");
        let probe = Probe::new(|_| Gate::Open);
        let (events, errors) = probe.scan(std::slice::from_ref(&r));
        assert_eq!((events.len(), errors), (1, vec![]));
        assert_eq!(probe.log(), vec![ScanEvent::Started(r.clone()), finished(&r, Some(1))]);
    }
}

mod discover {
    use super::*;

    #[test]
    fn finds_root_children_and_grandchildren_sorted_and_deduplicated() {
        let tmp = tempfile::tempdir().unwrap();
        let t = at(before(3), 8, 0, 0);
        let roots = tmp.path().join("roots");
        init_repo(&roots.join("a"), &t);
        init_repo(&roots.join("group").join("b"), &t);
        init_repo(&roots.join("group").join("c").join("too-deep"), &t);
        std::fs::create_dir_all(roots.join("plain").join("dir")).unwrap();
        // A worktree has a `.git` file instead of a directory.
        std::fs::create_dir_all(roots.join("wt")).unwrap();
        std::fs::write(roots.join("wt").join(".git"), "gitdir: elsewhere").unwrap();
        let solo = tmp.path().join("solo");
        init_repo(&solo, &t);
        init_repo(&solo.join("nested"), &t);

        let found = git::discover_repos(&[solo.clone(), roots.clone(), tmp.path().join("missing"), roots.clone()]);
        assert_eq!(found, vec![roots.join("a"), roots.join("group").join("b"), roots.join("wt"), solo]);
    }

    /// Paths compared as Windows does: component by component, ignoring case.
    fn folded(p: &Path) -> Vec<String> {
        p.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect()
    }

    #[test]
    fn one_repo_reached_through_differently_spelled_roots_is_listed_once() {
        let tmp = tempfile::tempdir().unwrap();
        let t = at(before(3), 8, 0, 0);
        let root = tmp.path().join("root");
        init_repo(&root.join("alpha"), &t);
        init_repo(&root.join("beta"), &t);
        // The folder of both repos in another case, one repo directly, the other with a
        // trailing separator and in upper case (the file system ignores case).
        let roots = [tmp.path().join("Root"), root.join("alpha"), tmp.path().join("ROOT").join("beta").join("")];

        let found = git::discover_repos(&roots);

        // Which spelling survives is not part of the contract; each repo is there once.
        let found: Vec<Vec<String>> = found.iter().map(|p| folded(p)).collect();
        assert_eq!(found, vec![folded(&root.join("alpha")), folded(&root.join("beta"))]);
    }

    #[test]
    fn repos_are_sorted_ignoring_case() {
        let tmp = tempfile::tempdir().unwrap();
        let t = at(before(3), 8, 0, 0);
        let root = tmp.path().join("root");
        for name in ["Bravo", "alpha", "charlie"] {
            init_repo(&root.join(name), &t);
        }
        assert_eq!(git::discover_repos(&[root.clone()]), vec![root.join("alpha"), root.join("Bravo"), root.join("charlie")]);
    }

    #[test]
    fn no_roots_no_repos() {
        assert_eq!(git::discover_repos(&[]), Vec::<PathBuf>::new());
    }
}
