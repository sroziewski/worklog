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

    #[test]
    fn no_roots_no_repos() {
        assert_eq!(git::discover_repos(&[]), Vec::<PathBuf>::new());
    }
}
