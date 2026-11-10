//! git::change_contexts against throw-away repos in temp dirs. git's own layout of `show --stat`
//! (column widths, blob hashes) is not this function's contract, so commit texts are checked for
//! the parts it promises: the message first, the changed lines, and the excluded files missing.

mod common;

use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, NaiveDate};
use common::{day, git_at, init_repo, local_at};
use worklog::git;

const CUT: &str = "\n… (cut)";
const BIG: usize = 100_000;

fn d() -> NaiveDate {
    day(2026, 11, 4)
}
fn at(date: NaiveDate, h: u32, m: u32) -> DateTime<Local> {
    local_at(date, h, m, 0)
}
fn before(n: i64) -> NaiveDate {
    d() - chrono::TimeDelta::days(n)
}

/// Repo with an initial commit of `src/lib.rs` three days before D, outside the searched window.
fn new_repo(parent: &Path, name: &str) -> PathBuf {
    let dir = parent.join(name);
    let t = at(before(3), 8, 0);
    init_repo(&dir, &t);
    write(&dir, "src/lib.rs", "old line\nkept line\n");
    git_at(&dir, &t, &["add", "-A"]);
    git_at(&dir, &t, &["commit", "-q", "-m", "init"]);
    dir
}

fn write(repo: &Path, path: &str, content: &str) {
    let p = repo.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

/// Commits `files` at `when` with a subject and a body.
fn commit_files(repo: &Path, when: &DateTime<Local>, subject: &str, body: &str, files: &[(&str, &str)]) {
    for (path, content) in files {
        write(repo, path, content);
    }
    git_at(repo, when, &["add", "-A"]);
    git_at(repo, when, &["commit", "-q", "-m", subject, "-m", body]);
}

fn contexts(lines: &[(Vec<PathBuf>, &str)], per_item: usize, total: usize) -> Vec<Option<String>> {
    common::empty_git_config();
    let lines: Vec<(Vec<PathBuf>, String)> = lines.iter().map(|(r, l)| (r.clone(), l.to_string())).collect();
    git::change_contexts(&lines, d(), per_item, total)
}

fn one(repos: &[&PathBuf], line: &str) -> Option<String> {
    let repos: Vec<PathBuf> = repos.iter().map(|r| (*r).clone()).collect();
    let mut out = contexts(&[(repos, line)], BIG, BIG);
    assert_eq!(out.len(), 1);
    out.remove(0)
}

mod commits {
    use super::*;

    #[test]
    fn subject_gives_message_then_stat_and_patch() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        commit_files(&r, &at(d(), 10, 0), "ABC-7 add CSV export", "Export rows as CSV for the auditors.", &[
            ("src/export.rs", "pub fn export_csv() -> &'static str { \"a;b\" }\n"),
            ("src/lib.rs", "new line\nkept line\n"),
        ]);

        let text = one(&[&r], "ABC-7 add CSV export").expect("the commit is found");
        assert!(text.starts_with("ABC-7 add CSV export\n\nExport rows as CSV for the auditors.\n"), "{text}");
        for fragment in [
            "src/export.rs",
            "+pub fn export_csv() -> &'static str { \"a;b\" }",
            "-old line",
            "+new line",
            // -U2: the unchanged neighbour is context.
            " kept line",
        ] {
            assert!(text.contains(fragment), "missing {fragment:?} in {text}");
        }
    }

    #[test]
    fn noise_files_are_left_out_of_the_commit_diff() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "web");
        commit_files(&r, &at(d(), 11, 0), "ABC-8 bump deps", "", &[
            ("api/Cargo.lock", "lock-marker-1\n"),
            ("package-lock.json", "lock-marker-2\n"),
            ("ui/vendor.min.js", "min-marker-3\n"),
            ("ui/logo.svg", "svg-marker-4\n"),
            ("ui/__snapshots__/view.snap", "snap-marker-5\n"),
            ("ui/package.json", "real-marker-6\n"),
        ]);

        let text = one(&[&r], "ABC-8 bump deps").expect("the commit is found");
        assert!(text.contains("+real-marker-6"), "{text}");
        assert!(text.contains("ui/package.json"), "{text}");
        for excluded in ["Cargo.lock", "package-lock.json", "vendor.min.js", "logo.svg", "view.snap", "marker-1", "marker-2", "marker-3", "marker-4", "marker-5"] {
            assert!(!text.contains(excluded), "{excluded:?} should be excluded: {text}");
        }
    }

    /// The other four patterns hold in any directory; a front end's lock file usually sits in its
    /// own directory of the repo, not at the root.
    #[test]
    fn package_lock_json_in_a_subdirectory_is_left_out_too() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "web");
        commit_files(&r, &at(d(), 11, 0), "ABC-14 bump ui deps", "", &[
            ("frontend/package-lock.json", "nested-lock-marker\n"),
            ("frontend/package.json", "real-marker\n"),
        ]);

        let text = one(&[&r], "ABC-14 bump ui deps").expect("the commit is found");
        assert!(text.contains("+real-marker"), "{text}");
        assert!(!text.contains("package-lock.json") && !text.contains("nested-lock-marker"), "frontend/package-lock.json should be excluded: {text}");
    }

    #[test]
    fn subject_is_matched_whole_after_trimming_and_case_sensitively() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        commit_files(&r, &at(d(), 10, 0), "ABC-9 fix login", "body nine", &[("src/login.rs", "fn login() {}\n")]);
        // (name, line, found)
        let rows = [
            ("exact", "ABC-9 fix login", true),
            ("surrounding whitespace", "  ABC-9 fix login \t", true),
            ("other case", "abc-9 fix login", false),
            ("prefix of the subject", "ABC-9 fix", false),
            ("subject plus more", "ABC-9 fix login now", false),
            ("unknown subject", "ABC-10 something else", false),
        ];
        for (name, line, found) in rows {
            let got = one(&[&r], line);
            assert_eq!(got.as_deref().map(|t| t.starts_with("ABC-9 fix login\n\nbody nine\n")), found.then_some(true), "{name}: {got:?}");
        }
    }

    #[test]
    fn only_commits_authored_on_the_day_count_from_its_first_to_its_last_second() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        commit_files(&r, &at(before(1), 23, 59), "day before", "b1", &[("b.txt", "b\n")]);
        commit_files(&r, &local_at(d(), 0, 0, 0), "first second", "f0", &[("c.txt", "c\n")]);
        commit_files(&r, &local_at(d(), 23, 59, 59), "last second", "l0", &[("e.txt", "e\n")]);
        commit_files(&r, &local_at(d() + chrono::TimeDelta::days(1), 0, 0, 0), "day after", "a1", &[("f.txt", "f\n")]);
        let got = contexts(
            &[(vec![r.clone()], "day before"), (vec![r.clone()], "first second"), (vec![r.clone()], "last second"), (vec![r.clone()], "day after")],
            BIG,
            BIG,
        );
        assert_eq!(got.len(), 4);
        assert_eq!(got[0], None);
        assert!(got[1].as_deref().is_some_and(|t| t.starts_with("first second\n\nf0\n") && t.contains("+c")), "{:?}", got[1]);
        assert!(got[2].as_deref().is_some_and(|t| t.starts_with("last second\n\nl0\n") && t.contains("+e")), "{:?}", got[2]);
        assert_eq!(got[3], None);
    }

    /// A later or earlier commit with the same subject must not stand in for the day's.
    #[test]
    fn same_subject_on_neighbouring_days_picks_the_one_authored_on_the_day() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        commit_files(&r, &at(before(1), 15, 0), "ABC-40 tune query", "from the day before", &[("q.sql", "before\n")]);
        commit_files(&r, &at(d(), 10, 0), "ABC-40 tune query", "from the day", &[("q.sql", "on-day\n")]);
        commit_files(&r, &at(d() + chrono::TimeDelta::days(1), 9, 0), "ABC-40 tune query", "from the day after", &[("q.sql", "after\n")]);
        let text = one(&[&r], "ABC-40 tune query").expect("the day's commit");
        assert!(text.starts_with("ABC-40 tune query\n\nfrom the day\n") && text.contains("+on-day"), "{text}");
        assert!(!text.contains("after") && !text.contains("+before"), "{text}");
    }

    #[test]
    fn subject_only_on_the_day_before_or_after_gives_none() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        commit_files(&r, &at(before(1), 15, 0), "ABC-41 yesterday only", "y", &[("y.txt", "y\n")]);
        commit_files(&r, &at(d() + chrono::TimeDelta::days(1), 9, 0), "ABC-42 tomorrow only", "t", &[("t.txt", "t\n")]);
        assert_eq!(contexts(&[(vec![r.clone()], "ABC-41 yesterday only"), (vec![r.clone()], "ABC-42 tomorrow only")], BIG, BIG), vec![None, None]);
    }

    #[test]
    fn two_commits_with_the_subject_on_the_day_give_the_earlier() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        commit_files(&r, &at(d(), 9, 0), "ABC-43 wip", "morning", &[("w.txt", "morning-line\n")]);
        commit_files(&r, &at(d(), 16, 0), "ABC-43 wip", "afternoon", &[("w.txt", "afternoon-line\n")]);
        let text = one(&[&r], "ABC-43 wip").expect("found");
        assert!(text.starts_with("ABC-43 wip\n\nmorning\n") && text.contains("+morning-line") && !text.contains("afternoon"), "{text}");
    }

    /// The author date decides, not the committer date (a rebase or amend the next day keeps the
    /// work on the day it was written).
    #[test]
    fn author_date_decides_not_committer_date() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        let author = |t: &DateTime<Local>, committer: &DateTime<Local>, subject: &str, file: &str| {
            write(&r, file, &format!("{subject}\n"));
            common::git_as(&r, t, committer, common::ME, &["add", "-A"]);
            common::git_as(&r, t, committer, common::ME, &["commit", "-q", "-m", subject]);
        };
        // Written on the day, committed the day after.
        author(&at(d(), 11, 0), &at(d() + chrono::TimeDelta::days(1), 8, 0), "ABC-44 rebased", "r.txt");
        // Written the day before, committed on the day.
        author(&at(before(1), 11, 0), &at(d(), 8, 0), "ABC-45 amended", "m.txt");
        let got = contexts(&[(vec![r.clone()], "ABC-44 rebased"), (vec![r.clone()], "ABC-45 amended")], BIG, BIG);
        assert!(got[0].as_deref().is_some_and(|t| t.starts_with("ABC-44 rebased\n") && t.contains("+ABC-44 rebased")), "{:?}", got[0]);
        assert_eq!(got[1], None);
    }

    #[test]
    fn commit_on_another_branch_counts_merge_commits_do_not() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        let t = at(d(), 10, 0);
        git_at(&r, &t, &["checkout", "-q", "-b", "feature/x"]);
        commit_files(&r, &t, "ABC-11 on the branch", "branch body", &[("f.txt", "branch-only\n")]);
        git_at(&r, &t, &["checkout", "-q", "main"]);
        commit_files(&r, &at(d(), 10, 30), "ABC-12 on main", "main body", &[("m.txt", "main-only\n")]);
        // The branch still holds ABC-11 alone; the merge is a second, merge-only commit on main.
        git_at(&r, &at(d(), 11, 0), &["merge", "-q", "--no-ff", "-m", "Merge feature ABC-11", "feature/x"]);
        git_at(&r, &at(d(), 11, 0), &["checkout", "-q", "-b", "feature/y"]);
        commit_files(&r, &at(d(), 12, 0), "ABC-13 unmerged branch", "y body", &[("y.txt", "y-only\n")]);
        git_at(&r, &at(d(), 12, 0), &["checkout", "-q", "main"]);

        assert!(one(&[&r], "ABC-13 unmerged branch").is_some_and(|t| t.starts_with("ABC-13 unmerged branch\n\ny body\n") && t.contains("+y-only")));
        assert_eq!(one(&[&r], "Merge feature ABC-11"), None);
    }

    #[test]
    fn second_repo_is_searched_when_the_first_lacks_the_subject_and_the_first_wins_when_both_have_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (new_repo(&tmp.path().join("one"), "svc"), new_repo(&tmp.path().join("two"), "svc"));
        commit_files(&a, &at(d(), 10, 0), "ABC-20 shared subject", "in a", &[("a.txt", "from-a\n")]);
        commit_files(&b, &at(d(), 10, 5), "ABC-20 shared subject", "in b", &[("b.txt", "from-b\n")]);
        commit_files(&b, &at(d(), 10, 10), "ABC-21 only in b", "b only", &[("b2.txt", "b-only\n")]);

        let only_b = one(&[&a, &b], "ABC-21 only in b").expect("found in the second repo");
        assert!(only_b.starts_with("ABC-21 only in b\n\nb only\n") && only_b.contains("+b-only"), "{only_b}");
        let shared = one(&[&a, &b], "ABC-20 shared subject").expect("found");
        assert!(shared.starts_with("ABC-20 shared subject\n\nin a\n") && shared.contains("+from-a") && !shared.contains("from-b"), "{shared}");
        let shared_b_first = one(&[&b, &a], "ABC-20 shared subject").expect("found");
        assert!(shared_b_first.starts_with("ABC-20 shared subject\n\nin b\n"), "{shared_b_first}");
    }

    #[test]
    fn line_without_repos_or_in_a_non_repo_dir_gives_none() {
        let tmp = tempfile::tempdir().unwrap();
        let plain = tmp.path().join("not-a-repo");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(contexts(&[(vec![], "ABC-1 anything"), (vec![plain], "ABC-1 anything")], BIG, BIG), vec![None, None]);
    }
}

mod checkouts {
    use super::*;

    #[test]
    fn checkout_line_gives_none_even_when_a_commit_has_that_subject() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        commit_files(&r, &at(d(), 10, 0), "checkout feature/x", "odd subject", &[("x.txt", "x\n")]);
        assert_eq!(one(&[&r], "checkout feature/x"), None);
    }
}

mod uncommitted {
    use super::*;

    #[test]
    fn tracked_modified_file_gives_its_diff_against_head() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        write(&r, "src/lib.rs", "new line\nkept line\n");
        let text = one(&[&r], "uncommitted src/lib.rs").expect("diff");
        assert!(text.starts_with("diff --git a/src/lib.rs b/src/lib.rs\n"), "{text}");
        for fragment in ["-old line\n", "+new line\n", " kept line\n"] {
            assert!(text.contains(fragment), "missing {fragment:?} in {text}");
        }
    }

    #[test]
    fn staged_change_is_part_of_the_diff_against_head() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        write(&r, "src/lib.rs", "staged line\nkept line\n");
        git_at(&r, &at(d(), 9, 0), &["add", "src/lib.rs"]);
        let text = one(&[&r], "uncommitted src/lib.rs").expect("diff");
        assert!(text.contains("-old line\n") && text.contains("+staged line\n"), "{text}");
    }

    #[test]
    fn untracked_file_gives_new_file_and_its_content() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        write(&r, "notes/todo ż.txt", "remember the milk\nand the bread\n");
        assert_eq!(one(&[&r], "uncommitted notes/todo ż.txt"), Some("new file notes/todo ż.txt:\nremember the milk\nand the bread\n".to_string()));
    }

    #[test]
    fn first_repo_holding_the_file_is_used() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (new_repo(&tmp.path().join("one"), "svc"), new_repo(&tmp.path().join("two"), "svc"));
        write(&b, "only-b.txt", "content of b\n");
        assert_eq!(one(&[&a, &b], "uncommitted only-b.txt"), Some("new file only-b.txt:\ncontent of b\n".to_string()));
        write(&a, "both.txt", "from a\n");
        write(&b, "both.txt", "from b\n");
        assert_eq!(one(&[&a, &b], "uncommitted both.txt"), Some("new file both.txt:\nfrom a\n".to_string()));
        assert_eq!(one(&[&b, &a], "uncommitted both.txt"), Some("new file both.txt:\nfrom b\n".to_string()));
    }

    /// Repos of the same name (forks, copies) all hold `src/lib.rs`; only the one that changed it
    /// has something to say about the line.
    #[test]
    fn unchanged_tracked_file_falls_through_to_the_repo_that_changed_it() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b, c) = (new_repo(&tmp.path().join("one"), "svc"), new_repo(&tmp.path().join("two"), "svc"), new_repo(&tmp.path().join("three"), "svc"));
        write(&b, "src/lib.rs", "changed in b\nkept line\n");
        let text = one(&[&a, &b, &c], "uncommitted src/lib.rs").expect("b's diff");
        assert!(text.starts_with("diff --git a/src/lib.rs b/src/lib.rs\n"), "{text}");
        assert!(text.contains("-old line\n") && text.contains("+changed in b\n"), "{text}");
        assert!(!text.starts_with("new file"), "{text}");
    }

    #[test]
    fn unchanged_tracked_file_everywhere_gives_none() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (new_repo(&tmp.path().join("one"), "svc"), new_repo(&tmp.path().join("two"), "svc"));
        assert_eq!(one(&[&a, &b], "uncommitted src/lib.rs"), None);
        assert_eq!(one(&[&a], "uncommitted src/lib.rs"), None);
    }

    #[test]
    fn unchanged_tracked_file_first_then_untracked_copy_gives_the_new_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = (new_repo(&tmp.path().join("one"), "svc"), new_repo(&tmp.path().join("two"), "svc"));
        write(&a, "conf/app.toml", "port = 1\n");
        git_at(&a, &at(before(3), 9, 0), &["add", "-A"]);
        git_at(&a, &at(before(3), 9, 0), &["commit", "-q", "-m", "conf"]);
        write(&b, "conf/app.toml", "port = 2\n");
        assert_eq!(one(&[&a, &b], "uncommitted conf/app.toml"), Some("new file conf/app.toml:\nport = 2\n".to_string()));
    }

    /// The same noise patterns the commit diff leaves out, matched at the end of the path ignoring
    /// case, tracked or not.
    #[test]
    fn noise_files_give_none_tracked_or_untracked() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        commit_files(&r, &at(before(3), 9, 0), "lock files", "", &[("Cargo.lock", "v1\n"), ("web/package-lock.json", "v1\n")]);
        write(&r, "Cargo.lock", "v2\n");
        write(&r, "web/package-lock.json", "v2\n");
        for (path, content) in [("ui/x.min.js", "min\n"), ("ui/LOGO.SVG", "<svg/>\n"), ("ui/__snapshots__/a.snap", "snap\n"), ("deps/Yarn.Lock", "y\n")] {
            write(&r, path, content);
        }
        // Names that only contain a pattern are not noise.
        write(&r, "notes.lock.md", "kept\n");
        write(&r, "ui/x.min.jsx", "kept too\n");
        // (path, expected)
        let rows: [(&str, Option<&str>); 8] = [
            ("Cargo.lock", None),
            ("web/package-lock.json", None),
            ("ui/x.min.js", None),
            ("ui/LOGO.SVG", None),
            ("ui/__snapshots__/a.snap", None),
            ("deps/Yarn.Lock", None),
            ("notes.lock.md", Some("new file notes.lock.md:\nkept\n")),
            ("ui/x.min.jsx", Some("new file ui/x.min.jsx:\nkept too\n")),
        ];
        for (path, expected) in rows {
            assert_eq!(one(&[&r], &format!("uncommitted {path}")).as_deref(), expected, "{path}");
        }
    }

    #[test]
    fn binary_untracked_file_gives_none() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        std::fs::write(r.join("icon.bin"), b"PNG\x00\x01rest").unwrap();
        std::fs::write(r.join("trailing-nul.txt"), b"text then\x00").unwrap();
        assert_eq!(one(&[&r], "uncommitted icon.bin"), None);
        assert_eq!(one(&[&r], "uncommitted trailing-nul.txt"), None);
    }

    /// Only the first 256 KiB of an untracked file are read: a NUL after them is never seen.
    #[test]
    fn untracked_file_is_read_up_to_256_kib() {
        const LIMIT: usize = 256 * 1024;
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        let mut big = "a".repeat(LIMIT - 1).into_bytes();
        big.extend_from_slice(b"b\x00tail");
        std::fs::write(r.join("big.txt"), &big).unwrap();
        let got = contexts(&[(vec![r.clone()], "uncommitted big.txt")], 2 * LIMIT, 2 * LIMIT);
        assert_eq!(got, vec![Some(format!("new file big.txt:\n{}b", "a".repeat(LIMIT - 1)))]);

        // One byte more and the NUL is inside the part read.
        let mut binary = "a".repeat(LIMIT - 1).into_bytes();
        binary.extend_from_slice(b"\x00tail");
        std::fs::write(r.join("big.txt"), &binary).unwrap();
        assert_eq!(contexts(&[(vec![r.clone()], "uncommitted big.txt")], 2 * LIMIT, 2 * LIMIT), vec![None]);
    }

    #[test]
    fn file_in_no_repo_gives_none() {
        let tmp = tempfile::tempdir().unwrap();
        let r = new_repo(tmp.path(), "svc");
        assert_eq!(one(&[&r], "uncommitted gone.txt"), None);
    }
}

mod limits {
    use super::*;

    /// Repo with untracked files whose context texts are `new file <name>:\n<content>`.
    fn repo_with(tmp: &tempfile::TempDir, files: &[(&str, &str)]) -> PathBuf {
        let r = new_repo(tmp.path(), "svc");
        for (p, c) in files {
            write(&r, p, c);
        }
        r
    }

    #[test]
    fn each_text_is_cut_to_per_item_chars_with_a_suffix() {
        let tmp = tempfile::tempdir().unwrap();
        // "new file a.txt:\n" is 16 chars; the content adds 10 more: 26 in all.
        let r = repo_with(&tmp, &[("a.txt", "żółć-56789")]);
        let full = "new file a.txt:\nżółć-56789";
        assert_eq!(full.chars().count(), 26);
        // (name, per_item, expected)
        let rows: [(&str, usize, String); 4] = [
            ("longer limit keeps all", 27, full.to_string()),
            ("limit equal to the length keeps all", 26, full.to_string()),
            ("one char less is cut", 25, format!("new file a.txt:\nżółć-5678{CUT}")),
            ("cut inside multibyte chars counts chars", 18, format!("new file a.txt:\nżó{CUT}")),
        ];
        for (name, per_item, expected) in rows {
            let got = contexts(&[(vec![r.clone()], "uncommitted a.txt")], per_item, BIG);
            assert_eq!(got, vec![Some(expected)], "{name}");
        }
    }

    #[test]
    fn total_budget_is_shared_and_later_lines_get_none_once_used_up() {
        let tmp = tempfile::tempdir().unwrap();
        // Each text is "new file x.txt:\n" (16) + 4 chars = 20 chars.
        let r = repo_with(&tmp, &[("a.txt", "AAAA"), ("b.txt", "BBBB"), ("c.txt", "CCCC"), ("d.txt", "DDDD")]);
        let lines = [
            (vec![r.clone()], "uncommitted a.txt"),
            // Lines without a context use none of the budget.
            (vec![r.clone()], "checkout main"),
            (vec![r.clone()], "uncommitted missing.txt"),
            (vec![r.clone()], "uncommitted b.txt"),
            (vec![r.clone()], "uncommitted c.txt"),
            (vec![r.clone()], "uncommitted d.txt"),
        ];
        // Budget 50: a takes 20, b takes 20, c gets the 10 left, d nothing.
        assert_eq!(
            contexts(&lines, 30, 50),
            vec![
                Some("new file a.txt:\nAAAA".to_string()),
                None,
                None,
                Some("new file b.txt:\nBBBB".to_string()),
                Some(format!("new file c{CUT}")),
                None,
            ]
        );
    }

    #[test]
    fn budget_used_exactly_leaves_none_for_the_rest() {
        let tmp = tempfile::tempdir().unwrap();
        let r = repo_with(&tmp, &[("a.txt", "AAAA"), ("b.txt", "BBBB")]);
        let lines = [(vec![r.clone()], "uncommitted a.txt"), (vec![r.clone()], "uncommitted b.txt")];
        assert_eq!(contexts(&lines, 30, 20), vec![Some("new file a.txt:\nAAAA".to_string()), None]);
    }

    #[test]
    fn zero_budget_and_no_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let r = repo_with(&tmp, &[("a.txt", "AAAA")]);
        assert_eq!(contexts(&[(vec![r], "uncommitted a.txt")], 30, 0), vec![None]);
        assert_eq!(contexts(&[], 30, 50), Vec::<Option<String>>::new());
    }
}

#[test]
fn commit_and_untracked_lines_keep_their_order() {
    let tmp = tempfile::tempdir().unwrap();
    let r = new_repo(tmp.path(), "svc");
    commit_files(&r, &at(d(), 10, 0), "ABC-30 first", "first body", &[("one.txt", "one\n")]);
    write(&r, "two.txt", "two\n");
    let got = contexts(&[(vec![r.clone()], "uncommitted two.txt"), (vec![r.clone()], "ABC-30 first")], BIG, BIG);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].as_deref(), Some("new file two.txt:\ntwo\n"));
    assert!(got[1].as_deref().is_some_and(|t| t.starts_with("ABC-30 first\n\nfirst body\n") && t.contains("+one")), "{:?}", got[1]);
}
