//! The New ticket window's "Work from" choice: presets, the start check and its description, the
//! groups a span's work is listed in, the git scan behind it, and the creator's range state.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone};
use common::{ME, commit, day, init_repo, local_at, ndt, set_mtime};
use worklog::config::Config;
use worklog::git::{Event, EventKind, Span};
use worklog::plan::Bucket;
use worklog::ticket_range::{self, MainRow, Preset, RANGE_ROW_BASE, Scanned};
use worklog::ui_tickets::{self, Creator, Draft, RangeChoice, WorkGroup};

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn wg(row: u64, title: &str, project: Option<&str>, repos: &[&str], lines: &[&str]) -> WorkGroup {
    WorkGroup { row, title: title.into(), project: project.map(str::to_owned), repos: strings(repos), lines: strings(lines) }
}

fn main_row(bucket: Bucket, row: u64, title: &str) -> MainRow {
    MainRow { bucket, row, title: title.into() }
}

fn issue(k: &str) -> Bucket {
    Bucket::Issue(k.into())
}

fn unassigned(repo: &str) -> Bucket {
    Bucket::Unassigned(repo.into())
}

#[test]
fn presets_in_the_order_the_window_offers_them_report_day_first() {
    assert_eq!(Preset::ALL, [Preset::ReportDay, Preset::SinceYesterday, Preset::Last3Days, Preset::ThisWeek, Preset::Last7Days, Preset::Custom]);
}

#[test]
fn unassigned_work_is_titled_by_its_repo() {
    assert_eq!(ticket_range::unassigned_title("billing-api"), "billing-api — no ticket yet");
}

mod start_of {
    use super::*;

    /// A start the user picked, used by Custom only; far from every preset's answer.
    fn custom() -> NaiveDateTime {
        ndt(day(2026, 10, 3), 7, 45, 0)
    }

    #[test]
    fn each_preset_starts_at_its_midnight_custom_at_the_users_start_report_day_at_none() {
        // Tuesday 2026-11-17 14:35:20. (name, preset, expected start)
        let now = ndt(day(2026, 11, 17), 14, 35, 20);
        let rows = [
            ("report day", Preset::ReportDay, None),
            ("since yesterday", Preset::SinceYesterday, Some(ndt(day(2026, 11, 16), 0, 0, 0))),
            ("last 3 days: today and the two before", Preset::Last3Days, Some(ndt(day(2026, 11, 15), 0, 0, 0))),
            ("this week on a Tuesday: Monday", Preset::ThisWeek, Some(ndt(day(2026, 11, 16), 0, 0, 0))),
            ("last 7 days: today and the six before", Preset::Last7Days, Some(ndt(day(2026, 11, 11), 0, 0, 0))),
            ("custom", Preset::Custom, Some(custom())),
        ];
        for (name, preset, expected) in rows {
            assert_eq!(ticket_range::start_of(preset, now, custom()), expected, "{name}");
        }
    }

    #[test]
    fn this_week_starts_on_monday_whatever_the_day() {
        // (name, now, expected Monday)
        let rows = [
            ("Monday morning: today", ndt(day(2026, 11, 16), 8, 10, 0), day(2026, 11, 16)),
            ("Monday at midnight: today", ndt(day(2026, 11, 16), 0, 0, 0), day(2026, 11, 16)),
            ("Wednesday", ndt(day(2026, 11, 18), 12, 0, 0), day(2026, 11, 16)),
            ("Sunday late: six days back", ndt(day(2026, 11, 22), 23, 59, 59), day(2026, 11, 16)),
            ("next Monday: a new week", ndt(day(2026, 11, 23), 0, 0, 1), day(2026, 11, 23)),
            ("Friday 2027-01-01: Monday of the old year", ndt(day(2027, 1, 1), 0, 30, 0), day(2026, 12, 28)),
        ];
        for (name, now, monday) in rows {
            assert_eq!(ticket_range::start_of(Preset::ThisWeek, now, custom()), Some(ndt(monday, 0, 0, 0)), "{name}");
        }
    }

    #[test]
    fn days_back_cross_month_year_and_leap_day() {
        // (name, now, preset, expected day)
        let rows = [
            ("since yesterday on New Year's Day", ndt(day(2027, 1, 1), 0, 30, 0), Preset::SinceYesterday, day(2026, 12, 31)),
            ("last 3 days on New Year's Day", ndt(day(2027, 1, 1), 0, 30, 0), Preset::Last3Days, day(2026, 12, 30)),
            ("last 7 days on New Year's Day", ndt(day(2027, 1, 1), 0, 30, 0), Preset::Last7Days, day(2026, 12, 26)),
            ("since yesterday on the 1st of a month", ndt(day(2026, 12, 1), 9, 0, 0), Preset::SinceYesterday, day(2026, 11, 30)),
            ("since yesterday after a leap day", ndt(day(2028, 3, 1), 9, 0, 0), Preset::SinceYesterday, day(2028, 2, 29)),
            ("last 3 days over a leap day", ndt(day(2028, 3, 1), 9, 0, 0), Preset::Last3Days, day(2028, 2, 28)),
        ];
        for (name, now, preset, expected) in rows {
            assert_eq!(ticket_range::start_of(preset, now, custom()), Some(ndt(expected, 0, 0, 0)), "{name}");
        }
    }

    #[test]
    fn custom_is_taken_as_given_even_in_the_future() {
        let now = ndt(day(2026, 11, 17), 14, 35, 20);
        let later = ndt(day(2026, 11, 19), 6, 5, 0);
        assert_eq!(ticket_range::start_of(Preset::Custom, now, later), Some(later));
    }
}

mod check_start {
    use super::*;

    #[test]
    fn a_start_before_now_is_accepted_now_or_later_is_rejected_with_the_start_named() {
        let now = ndt(day(2026, 11, 17), 14, 35, 20);
        // (name, start, expected)
        let rows: [(&str, NaiveDateTime, Result<(), String>); 5] = [
            ("days before", ndt(day(2026, 11, 10), 9, 0, 0), Ok(())),
            ("a second before", ndt(day(2026, 11, 17), 14, 35, 19), Ok(())),
            ("now itself", now, Err("Tue 2026-11-17 14:35 is not in the past; pick an earlier start".into())),
            ("a second after", ndt(day(2026, 11, 17), 14, 35, 21), Err("Tue 2026-11-17 14:35 is not in the past; pick an earlier start".into())),
            ("days after", ndt(day(2026, 11, 19), 6, 5, 0), Err("Thu 2026-11-19 06:05 is not in the past; pick an earlier start".into())),
        ];
        for (name, start, expected) in rows {
            assert_eq!(ticket_range::check_start(start, now), expected, "{name}");
        }
    }
}

mod describe_start {
    use super::*;

    #[test]
    fn minutes_under_an_hour_hours_and_minutes_under_a_day_then_days_and_hours() {
        let now = ndt(day(2026, 11, 17), 14, 35, 20);
        // (name, start, now, expected)
        let rows = [
            ("the example", ndt(day(2026, 11, 16), 9, 0, 0), ndt(day(2026, 11, 17), 12, 0, 0), "since Mon 2026-11-16 09:00 (1 d 3 h ago)"),
            ("now itself", now, now, "since Tue 2026-11-17 14:35 (0 min ago)"),
            ("seconds only", ndt(day(2026, 11, 17), 14, 34, 30), now, "since Tue 2026-11-17 14:34 (0 min ago)"),
            ("a future start clamps to nothing ago", ndt(day(2026, 11, 17), 14, 40, 0), now, "since Tue 2026-11-17 14:40 (0 min ago)"),
            ("59 minutes", ndt(day(2026, 11, 17), 13, 36, 20), now, "since Tue 2026-11-17 13:36 (59 min ago)"),
            ("an hour exactly", ndt(day(2026, 11, 17), 13, 35, 20), now, "since Tue 2026-11-17 13:35 (1 h 0 min ago)"),
            ("hours and minutes", ndt(day(2026, 11, 17), 9, 23, 20), now, "since Tue 2026-11-17 09:23 (5 h 12 min ago)"),
            ("a minute short of a day", ndt(day(2026, 11, 16), 14, 36, 20), now, "since Mon 2026-11-16 14:36 (23 h 59 min ago)"),
            ("a day exactly", ndt(day(2026, 11, 16), 14, 35, 20), now, "since Mon 2026-11-16 14:35 (1 d ago)"),
            ("a minute short of a week", ndt(day(2026, 11, 10), 14, 36, 20), now, "since Tue 2026-11-10 14:36 (6 d 23 h ago)"),
            ("over the year", ndt(day(2026, 12, 29), 8, 0, 0), ndt(day(2027, 1, 2), 10, 0, 0), "since Tue 2026-12-29 08:00 (4 d 2 h ago)"),
        ];
        for (name, start, now, expected) in rows {
            assert_eq!(ticket_range::describe_start(start, now), expected, "{name}");
        }
    }
}

mod local {
    use super::*;
    use chrono::TimeDelta;

    #[test]
    fn a_wall_clock_time_that_exists_is_that_local_time() {
        let t = ndt(day(2026, 11, 17), 14, 35, 0);
        assert_eq!(ticket_range::local(t), Local.from_local_datetime(&t).single().unwrap());
    }

    /// The instant the clock jumps forward is where a skipped wall time lands. Read from the UTC
    /// side, so it holds in any zone; a zone without DST has no skipped times.
    #[test]
    fn a_wall_clock_time_a_dst_jump_skipped_moves_to_the_jumps_end() {
        let jumps: Vec<_> = common::offset_changes(2026).into_iter().filter(|(_, old, new)| new > old).collect();
        if jumps.is_empty() {
            eprintln!("the machine's time zone skips no wall time in 2026; nothing to check");
            return;
        }
        for (at_utc, old, new) in jumps {
            let skipped = at_utc.naive_utc() + TimeDelta::seconds(i64::from(old) + i64::from(new - old) / 2);
            assert_eq!(Local.from_local_datetime(&skipped).earliest(), None, "fixture: {skipped} is skipped");
            assert_eq!(ticket_range::local(skipped), Local.from_utc_datetime(&at_utc.naive_utc()), "{skipped}");
        }
    }
}

mod jiff_dates {
    use super::*;
    use chrono::Datelike;

    #[test]
    fn to_jiff_keeps_year_month_and_day_and_from_jiff_brings_it_back() {
        // (name, date)
        let rows = [
            ("ordinary", day(2026, 11, 17)),
            ("day above twelve so month and day cannot swap", day(2026, 3, 29)),
            ("leap day", day(2028, 2, 29)),
            ("last of the year", day(2026, 12, 31)),
            ("first of the year", day(2027, 1, 1)),
        ];
        for (name, d) in rows {
            let j = ui_tickets::to_jiff(d);
            assert_eq!((i32::from(j.year()), i32::from(j.month()), i32::from(j.day())), (d.year(), d.month() as i32, d.day() as i32), "{name}");
            assert_eq!(ui_tickets::from_jiff(j), Some(d), "{name}");
        }
    }
}

mod range_choice {
    use super::*;

    #[test]
    fn new_lists_the_report_day_with_custom_at_midnight_of_today() {
        let r = RangeChoice::new(day(2026, 11, 17));
        assert_eq!((r.preset, r.custom_date, r.custom_hour, r.custom_minute), (Preset::ReportDay, jiff::civil::date(2026, 11, 17), 0, 0));
        assert_eq!((r.is_span(), r.scanning(), r.scanned.is_none(), r.shown_start()), (false, false, true, None));
        assert_eq!(r.custom_start(), ndt(day(2026, 11, 17), 0, 0, 0));
    }

    #[test]
    fn custom_start_joins_the_picked_date_hour_and_minute() {
        let mut r = RangeChoice::new(day(2026, 11, 17));
        r.custom_date = jiff::civil::date(2026, 2, 28);
        r.custom_hour = 7;
        r.custom_minute = 45;
        assert_eq!(r.custom_start(), ndt(day(2026, 2, 28), 7, 45, 0));
    }

    #[test]
    fn every_preset_but_the_report_day_is_a_span() {
        for p in Preset::ALL {
            let mut r = RangeChoice::new(day(2026, 11, 17));
            r.preset = p;
            assert_eq!(r.is_span(), p != Preset::ReportDay, "{p:?}");
        }
    }
}

mod work_groups {
    use super::*;

    fn ev(repo: &str, date: NaiveDate, h: u32, m: u32, kind: EventKind, what: &str, key: Option<&str>) -> Event {
        Event { repo: repo.into(), time: local_at(date, h, m, 0), kind, what: what.into(), key: key.map(str::to_owned) }
    }

    /// Credited with a 60 min gap limit and 30 min per session start, in time order:
    /// ABC-1 30 + 20 (+30 next day) = 80, web unassigned 40, XYZ-7 40, docs unassigned 30 + 10 =
    /// 40, ABC-2 45. Listed out of time order.
    fn events() -> Vec<Event> {
        let (d1, d2) = (day(2026, 11, 16), day(2026, 11, 17));
        vec![
            ev("docs", d1, 13, 10, EventKind::Commit, "typos", None),
            ev("api", d1, 9, 0, EventKind::Commit, "ABC-1 login", Some("ABC-1")),
            ev("web", d2, 8, 45, EventKind::Commit, "ABC-2 header", Some("ABC-2")),
            ev("api", d1, 9, 20, EventKind::Commit, "ABC-1 tests", Some("ABC-1")),
            ev("web", d1, 10, 0, EventKind::Uncommitted, "src/app.ts", None),
            ev("db", d1, 10, 40, EventKind::Checkout, "feature/XYZ-7-x", Some("XYZ-7")),
            ev("docs", d1, 13, 0, EventKind::Commit, "readme", None),
            ev("api", d2, 8, 0, EventKind::Commit, "ABC-1 fix", Some("ABC-1")),
        ]
    }

    /// Only `web` maps to a project; `docs` maps to none.
    fn repo_projects() -> BTreeMap<String, String> {
        BTreeMap::from([("web".to_string(), "WEB".to_string())])
    }

    #[test]
    fn most_credited_first_ties_by_bucket_main_rows_by_bucket_others_synthetic() {
        let main = vec![
            main_row(issue("XYZ-7"), 3, "XYZ-7 Fix the importer"),
            main_row(unassigned("web"), 5, "web (report day)"),
            // Same repo as the ABC-1 group but another bucket: stands for nothing here.
            main_row(unassigned("api"), 9, "api (report day)"),
            main_row(issue("ABC-9"), 11, "ABC-9 not in the span"),
        ];
        assert_eq!(
            ticket_range::work_groups(&events(), &repo_projects(), &main, 60, 30),
            vec![
                wg(RANGE_ROW_BASE, "ABC-1", Some("ABC"), &["api"], &["ABC-1 login", "ABC-1 tests", "ABC-1 fix"]),
                wg(RANGE_ROW_BASE + 1, "ABC-2", Some("ABC"), &["web"], &["ABC-2 header"]),
                wg(3, "XYZ-7 Fix the importer", Some("XYZ"), &["db"], &["checkout feature/XYZ-7-x"]),
                wg(RANGE_ROW_BASE + 3, "docs — no ticket yet", None, &["docs"], &["readme", "typos"]),
                wg(5, "web (report day)", Some("WEB"), &["web"], &["uncommitted src/app.ts"]),
            ]
        );
    }

    /// Swapped limits credit differently (60 min per session start, gaps over 30 min start one):
    /// ABC-1 60 + 20 + 60 = 140, docs 60 + 10 = 70, ABC-2 60, XYZ-7 60, web 60.
    #[test]
    fn the_gap_limit_and_session_credit_decide_the_order() {
        let got: Vec<String> = ticket_range::work_groups(&events(), &repo_projects(), &[], 30, 60).into_iter().map(|g| g.title).collect();
        assert_eq!(got, strings(&["ABC-1", "docs — no ticket yet", "ABC-2", "XYZ-7", "web — no ticket yet"]));
    }

    #[test]
    fn without_main_rows_every_group_gets_a_synthetic_id_in_list_order() {
        let rows: Vec<(u64, String)> = ticket_range::work_groups(&events(), &repo_projects(), &[], 60, 30).into_iter().map(|g| (g.row, g.title)).collect();
        assert_eq!(
            rows,
            vec![
                (RANGE_ROW_BASE, "ABC-1".to_string()),
                (RANGE_ROW_BASE + 1, "ABC-2".to_string()),
                (RANGE_ROW_BASE + 2, "XYZ-7".to_string()),
                (RANGE_ROW_BASE + 3, "docs — no ticket yet".to_string()),
                (RANGE_ROW_BASE + 4, "web — no ticket yet".to_string()),
            ]
        );
    }

    #[test]
    fn no_events_no_groups() {
        assert_eq!(ticket_range::work_groups(&[], &repo_projects(), &[main_row(issue("ABC-1"), 1, "x")], 60, 30), vec![]);
    }

    #[test]
    fn a_single_event_is_one_group() {
        let one = [ev("svc", day(2026, 11, 16), 9, 0, EventKind::Commit, "XYZ-8 one", Some("XYZ-8"))];
        assert_eq!(
            ticket_range::work_groups(&one, &BTreeMap::new(), &[], 60, 30),
            vec![wg(RANGE_ROW_BASE, "XYZ-8", Some("XYZ"), &["svc"], &["XYZ-8 one"])]
        );
    }
}

mod scan {
    use super::*;

    /// D is a past day; the span runs from D-1 09:00 to D 18:00.
    fn d() -> NaiveDate {
        day(2026, 11, 4)
    }
    fn prev() -> NaiveDate {
        day(2026, 11, 3)
    }
    fn start() -> NaiveDateTime {
        ndt(prev(), 9, 0, 0)
    }
    fn span() -> Span {
        Span { from: local_at(prev(), 9, 0, 0), to: local_at(d(), 18, 0, 0) }
    }

    /// `root/alpha`: ABC-1 twice on D-1 (60 min credited), ABC-2 on D (30), an OLD-3 commit of a
    /// disabled project, one before the span, and an untracked `notes.txt` changed D 10:00.
    /// `root/beta`: two commits without a key on D (30 + 20 = 50) and one after the span.
    fn fixture(tmp: &Path) -> (PathBuf, PathBuf, PathBuf) {
        let root = tmp.join("root");
        let (alpha, beta) = (root.join("alpha"), root.join("beta"));
        let setup = local_at(day(2026, 11, 1), 8, 0, 0);
        for r in [&alpha, &beta] {
            init_repo(r, &setup);
            commit(r, &setup, "init", "");
        }
        commit(&alpha, &local_at(prev(), 8, 0, 0), "ABC-4 before the span", "");
        commit(&alpha, &local_at(prev(), 10, 0, 0), "ABC-1 login form", "");
        commit(&alpha, &local_at(prev(), 10, 30, 0), "ABC-1 login tests", "");
        commit(&alpha, &local_at(d(), 9, 0, 0), "ABC-2 export", "");
        commit(&alpha, &local_at(d(), 9, 30, 0), "OLD-3 legacy fix", "");
        commit(&beta, &local_at(d(), 12, 0, 0), "cleanup scripts", "");
        commit(&beta, &local_at(d(), 12, 20, 0), "more cleanup", "");
        commit(&beta, &local_at(d(), 19, 0, 0), "XYZ-5 after the span", "");
        std::fs::write(alpha.join("notes.txt"), "notes").unwrap();
        set_mtime(&alpha.join("notes.txt"), &local_at(d(), 10, 0, 0));
        (root, alpha, beta)
    }

    fn cfg(root: &Path, beta: &Path) -> Config {
        Config {
            repo_roots: vec![root.to_path_buf()],
            author_emails: vec!["configured@example.test".into()],
            project_keys: vec!["ABC".into(), "XYZ".into()],
            repo_projects: BTreeMap::from([(beta.to_string_lossy().into_owned(), "XYZ".to_string())]),
            inactive_keys: vec!["OLD".into()],
            ctx_env_file: None,
            jira_url: None,
            ..Config::default()
        }
    }

    fn main() -> Vec<MainRow> {
        vec![main_row(issue("ABC-2"), 4, "ABC-2 CSV export")]
    }

    fn me() -> Vec<String> {
        vec![ME.to_string()]
    }

    #[test]
    fn groups_the_spans_commits_of_the_given_emails_like_the_main_window() {
        common::empty_git_config();
        let tmp = tempfile::tempdir().unwrap();
        let (root, alpha, beta) = fixture(tmp.path());
        let got = ticket_range::scan(&cfg(&root, &beta), &me(), start(), span(), false, &main());
        assert_eq!(
            got,
            Scanned {
                span: span(),
                start: start(),
                groups: vec![
                    wg(RANGE_ROW_BASE, "ABC-1", Some("ABC"), &["alpha"], &["ABC-1 login form", "ABC-1 login tests"]),
                    wg(RANGE_ROW_BASE + 1, "beta — no ticket yet", Some("XYZ"), &["beta"], &["cleanup scripts", "more cleanup"]),
                    wg(4, "ABC-2 CSV export", Some("ABC"), &["alpha"], &["ABC-2 export"]),
                ],
                repo_paths: BTreeMap::from([("alpha".to_string(), vec![alpha]), ("beta".to_string(), vec![beta])]),
                warnings: vec![],
                commits: 5,
                repos: 2,
            }
        );
    }

    /// With the untracked file D 10:00: ABC-2 30, alpha's file 60, beta 120 (a gap of exactly
    /// the default 120 min limit) + 20 = 140, ABC-1 60; ABC-1 before alpha's file on the tie.
    #[test]
    fn uncommitted_files_are_listed_when_asked_for() {
        common::empty_git_config();
        let tmp = tempfile::tempdir().unwrap();
        let (root, _, beta) = fixture(tmp.path());
        let got = ticket_range::scan(&cfg(&root, &beta), &me(), start(), span(), true, &main());
        assert_eq!(
            got.groups,
            vec![
                wg(RANGE_ROW_BASE, "beta — no ticket yet", Some("XYZ"), &["beta"], &["cleanup scripts", "more cleanup"]),
                wg(RANGE_ROW_BASE + 1, "ABC-1", Some("ABC"), &["alpha"], &["ABC-1 login form", "ABC-1 login tests"]),
                wg(RANGE_ROW_BASE + 2, "alpha — no ticket yet", None, &["alpha"], &["uncommitted notes.txt"]),
                wg(4, "ABC-2 CSV export", Some("ABC"), &["alpha"], &["ABC-2 export"]),
            ]
        );
        assert_eq!(got.commits, 5);
    }

    #[test]
    fn only_the_given_emails_count_not_the_configured_ones() {
        common::empty_git_config();
        let tmp = tempfile::tempdir().unwrap();
        let (root, _, beta) = fixture(tmp.path());
        let got = ticket_range::scan(&cfg(&root, &beta), &strings(&["someone.else@example.test"]), start(), span(), false, &main());
        assert_eq!((got.groups, got.commits, got.repos, got.warnings), (vec![], 0, 2, vec![]));
    }

    #[test]
    fn no_emails_and_no_repos_are_both_warned_about() {
        common::empty_git_config();
        let tmp = tempfile::tempdir().unwrap();
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let cfg = Config { repo_roots: vec![empty.clone()], ctx_env_file: None, jira_url: None, ..Config::default() };
        let got = ticket_range::scan(&cfg, &[], start(), span(), false, &[]);
        assert_eq!(
            got,
            Scanned {
                span: span(),
                start: start(),
                groups: vec![],
                repo_paths: BTreeMap::new(),
                warnings: vec![
                    "no author email configured and git user.email is unset; no commits can be matched".to_string(),
                    format!("no git repos found under {:?}", vec![empty]),
                ],
                commits: 0,
                repos: 0,
            }
        );
    }

    /// A repo whose `.git/HEAD` is garbage: git refuses it, the other repos are still read. The
    /// warning is "<repo>: " and then the git command with git's own stderr, whose wording is
    /// git's, not this function's contract.
    #[test]
    fn a_repo_git_cannot_read_is_a_warning_and_the_others_are_still_read() {
        common::empty_git_config();
        let tmp = tempfile::tempdir().unwrap();
        let (root, alpha, beta) = fixture(tmp.path());
        let broken = root.join("broken");
        init_repo(&broken, &local_at(day(2026, 11, 1), 8, 0, 0));
        std::fs::write(broken.join(".git").join("HEAD"), "not a ref\n").unwrap();
        let got = ticket_range::scan(&cfg(&root, &beta), &me(), start(), span(), false, &main());
        assert_eq!(got.warnings.len(), 1, "{:?}", got.warnings);
        assert!(got.warnings[0].starts_with(&format!("{}: ", broken.display())), "{:?}", got.warnings);
        assert_eq!(
            (got.groups.iter().map(|g| g.row).collect::<Vec<_>>(), got.commits, got.repos),
            (vec![RANGE_ROW_BASE, RANGE_ROW_BASE + 1, 4], 5, 3)
        );
        assert_eq!(
            got.repo_paths,
            BTreeMap::from([("alpha".to_string(), vec![alpha]), ("beta".to_string(), vec![beta]), ("broken".to_string(), vec![broken])])
        );
    }

    #[test]
    fn no_emails_with_repos_warns_once_and_finds_no_commits() {
        common::empty_git_config();
        let tmp = tempfile::tempdir().unwrap();
        let (root, _, beta) = fixture(tmp.path());
        let got = ticket_range::scan(&cfg(&root, &beta), &[], start(), span(), false, &main());
        assert_eq!(
            (got.groups, got.commits, got.repos, got.warnings),
            (vec![], 0, 2, strings(&["no author email configured and git user.email is unset; no commits can be matched"]))
        );
    }
}

mod creator {
    use super::*;
    use eframe::egui;
    use std::time::{Duration, Instant};
    use worklog::config::JiraCredentials;

    fn ticks(v: &[(u64, usize)]) -> BTreeSet<(u64, usize)> {
        v.iter().copied().collect()
    }

    /// No repos to read: a scan started here finishes at once and touches nothing.
    fn cfg() -> Config {
        Config { author_emails: vec![ME.into()], ctx_env_file: None, jira_url: None, ..Config::default() }
    }

    fn today() -> NaiveDate {
        Local::now().date_naive()
    }

    fn creator() -> Creator {
        common::empty_git_config();
        let mut c = Creator::new("ABC".into(), Some(5));
        c.ticks = ticks(&[(10, 0), (20, 3)]);
        c.error = Some("an earlier failure".into());
        c
    }

    #[test]
    fn a_past_preset_starts_reading_from_its_start_and_drops_the_ticks() {
        let mut c = creator();
        let before = today();
        c.choose_range(Preset::SinceYesterday, &cfg(), &[]);
        let yesterday = before.pred_opt().unwrap();
        assert_eq!(today(), before, "fixture: the test ran across midnight");
        assert_eq!(
            (c.range.preset, c.range.is_span(), c.range.scanning(), c.range.shown_start(), c.ticks.clone(), c.error.clone()),
            (Preset::SinceYesterday, true, true, Some(ndt(yesterday, 0, 0, 0)), BTreeSet::new(), None)
        );
    }

    #[test]
    fn a_custom_start_in_the_past_is_read_from_that_start() {
        let mut c = creator();
        c.range.custom_date = jiff::civil::date(2026, 11, 2);
        c.range.custom_hour = 8;
        c.range.custom_minute = 30;
        c.choose_range(Preset::Custom, &cfg(), &[]);
        assert_eq!(
            (c.range.preset, c.range.scanning(), c.range.shown_start(), c.ticks.clone(), c.error.clone()),
            (Preset::Custom, true, Some(ndt(day(2026, 11, 2), 8, 30, 0)), BTreeSet::new(), None)
        );
    }

    #[test]
    fn a_custom_start_in_the_future_is_refused_with_a_message_and_nothing_is_read() {
        let mut c = creator();
        c.range.custom_date = jiff::civil::date(2099, 3, 2);
        c.range.custom_hour = 7;
        c.range.custom_minute = 5;
        c.choose_range(Preset::Custom, &cfg(), &[]);
        assert_eq!(
            (c.range.preset, c.range.scanning(), c.range.scanned.is_none(), c.range.shown_start(), c.ticks.clone()),
            (Preset::Custom, false, true, None, BTreeSet::new())
        );
        assert_eq!(c.error.as_deref(), Some("Mon 2099-03-02 07:05 is not in the past; pick an earlier start"));
    }

    #[test]
    fn report_day_drops_the_ticks_the_error_and_a_running_read() {
        let mut c = creator();
        c.choose_range(Preset::Last3Days, &cfg(), &[]);
        assert!(c.range.scanning(), "fixture: a read is running");
        c.ticks = ticks(&[(30, 1)]);
        c.error = Some("another failure".into());
        c.choose_range(Preset::ReportDay, &cfg(), &[]);
        assert_eq!(
            (c.range.preset, c.range.is_span(), c.range.scanning(), c.range.scanned.is_none(), c.range.shown_start(), c.ticks.clone(), c.error.clone()),
            (Preset::ReportDay, false, false, true, None, BTreeSet::new(), None)
        );
    }

    #[test]
    fn opening_from_a_main_row_goes_back_to_the_report_day() {
        let mut c = creator();
        c.choose_range(Preset::Last7Days, &cfg(), &[]);
        assert!(c.range.scanning(), "fixture: a read is running");
        c.focus_row(&wg(30, "row 30", Some("XYZ"), &["svc"], &["XYZ-4 export"]));
        assert_eq!(
            (c.range.preset, c.range.scanning(), c.range.scanned.is_none(), c.range.shown_start(), c.ticks.clone(), c.project.as_str()),
            (Preset::ReportDay, false, true, None, ticks(&[(30, 0)]), "XYZ")
        );
    }

    /// Frames of the window, headless, until the read started by `choose_range` has arrived.
    fn frames_until_read(c: &mut Creator, cfg: &Config) {
        let ctx = egui::Context::default();
        let creds = JiraCredentials { url: "https://jira.example.test".into(), token: "tok".into() };
        let deadline = Instant::now() + Duration::from_secs(60);
        while c.range.scanning() && Instant::now() < deadline {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                let _ = c.show(ui.ctx(), cfg, &creds, "jdoe", &[], None, None, true, &[], &[]);
            });
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A commit of mine yesterday 10:00 in a repo under the configured root, read through the
    /// window: the read arrives as one group, and the ticks made while it ran go.
    #[test]
    fn the_read_arrives_in_the_window_as_groups_and_clears_the_ticks() {
        common::empty_git_config();
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("root").join("svc");
        let yesterday = today().pred_opt().unwrap();
        init_repo(&repo, &local_at(yesterday - chrono::TimeDelta::days(3), 8, 0, 0));
        commit(&repo, &local_at(yesterday - chrono::TimeDelta::days(3), 8, 0, 0), "init", "");
        commit(&repo, &local_at(yesterday, 10, 0, 0), "ABC-77 range work", "");
        let cfg = Config { repo_roots: vec![tmp.path().join("root")], project_keys: vec!["ABC".into(), "XYZ".into()], ..cfg() };
        let mut c = creator();
        c.choose_range(Preset::SinceYesterday, &cfg, &[main_row(issue("ABC-77"), 8, "ABC-77 Range work")]);
        c.ticks = ticks(&[(8, 0)]);

        frames_until_read(&mut c, &cfg);

        let scanned = c.range.scanned.clone().expect("the read arrived");
        assert_eq!(
            (scanned.start, scanned.span.from, scanned.groups.clone(), scanned.commits, scanned.repos, scanned.warnings.clone()),
            (
                ndt(yesterday, 0, 0, 0),
                ticket_range::local(ndt(yesterday, 0, 0, 0)),
                vec![wg(8, "ABC-77 Range work", Some("ABC"), &["svc"], &["ABC-77 range work"])],
                1,
                1,
                vec![]
            )
        );
        assert!(scanned.span.to > Local::now() - chrono::TimeDelta::minutes(1) && scanned.span.to <= Local::now() + chrono::TimeDelta::seconds(1), "{:?}", scanned.span);
        assert_eq!((c.range.scanning(), c.range.shown_start(), c.ticks.clone(), c.error.clone()), (false, Some(ndt(yesterday, 0, 0, 0)), BTreeSet::new(), None));
    }

    type DraftView = (String, String, String, Vec<u64>);

    fn drafts(c: &Creator) -> Vec<DraftView> {
        c.drafts.iter().map(|d| (d.project.clone(), d.title.clone(), d.description.clone(), d.rows.clone())).collect()
    }

    #[test]
    fn drafts_survive_every_range_change_and_the_reads_arrival() {
        let cfg = cfg();
        let mut c = creator();
        c.drafts = vec![Draft::new("ABC", "Fix the login form", "SSO broke", vec![10, 20]), Draft::new("XYZ", "Export CSV", "for auditors", vec![RANGE_ROW_BASE + 2])];
        let before = drafts(&c);

        c.choose_range(Preset::SinceYesterday, &cfg, &[]);
        assert_eq!(drafts(&c), before, "after choosing a span");
        frames_until_read(&mut c, &cfg);
        assert!(c.range.scanned.is_some(), "fixture: the read arrived");
        assert_eq!(drafts(&c), before, "after the read arrived");
        c.choose_range(Preset::ReportDay, &cfg, &[]);
        assert_eq!(drafts(&c), before, "after going back to the report day");
        assert_eq!(c.click_preset(Preset::Custom), None);
        assert_eq!(drafts(&c), before, "after picking Custom");
    }

    /// A finished read: its own span and repos, two groups.
    fn scanned() -> Scanned {
        let at = |d: u32, h: u32| local_at(day(2026, 11, d), h, 0, 0);
        Scanned {
            span: Span { from: at(12, 9), to: at(17, 15) },
            start: ndt(day(2026, 11, 12), 9, 0, 0),
            groups: vec![wg(RANGE_ROW_BASE, "ABC-1", Some("ABC"), &["api"], &["ABC-1 login"]), wg(7, "XYZ-7 Importer", Some("XYZ"), &["db"], &["XYZ-7 fix"])],
            repo_paths: BTreeMap::from([("api".to_string(), vec![PathBuf::from(r"C:\scan\api")])]),
            warnings: vec![],
            commits: 2,
            repos: 2,
        }
    }

    mod click_preset {
        use super::*;

        /// A creator reading Last 3 days, with ticks.
        fn reading() -> Creator {
            let mut c = creator();
            c.choose_range(Preset::Last3Days, &cfg(), &[]);
            c.ticks = ticks(&[(10, 0), (20, 3)]);
            c
        }

        fn state(c: &Creator) -> (Preset, bool, bool, BTreeSet<(u64, usize)>) {
            (c.range.preset, c.range.scanning(), c.range.scanned.is_some(), c.ticks.clone())
        }

        #[test]
        fn the_preset_already_chosen_changes_nothing() {
            let mut c = reading();
            assert_eq!(c.click_preset(Preset::Last3Days), None);
            assert_eq!(state(&c), (Preset::Last3Days, true, false, ticks(&[(10, 0), (20, 3)])));
        }

        #[test]
        fn another_preset_is_handed_back_to_be_read_and_nothing_changes_yet() {
            let mut c = reading();
            assert_eq!(c.click_preset(Preset::ThisWeek), Some(Preset::ThisWeek));
            assert_eq!(state(&c), (Preset::Last3Days, true, false, ticks(&[(10, 0), (20, 3)])));
        }

        #[test]
        fn report_day_from_a_span_is_handed_back_too() {
            let mut c = reading();
            assert_eq!(c.click_preset(Preset::ReportDay), Some(Preset::ReportDay));
            assert_eq!(state(&c), (Preset::Last3Days, true, false, ticks(&[(10, 0), (20, 3)])));
        }

        #[test]
        fn custom_opens_its_fields_drops_the_read_and_the_ticks_and_reads_nothing() {
            let mut c = reading();
            c.range.scanned = Some(std::sync::Arc::new(scanned()));
            assert_eq!(c.click_preset(Preset::Custom), None);
            assert_eq!(state(&c), (Preset::Custom, false, false, BTreeSet::new()));
            assert_eq!(c.range.shown_start(), None);
        }
    }

    mod controls {
        use super::*;

        #[test]
        fn enabled_only_when_neither_busy_nor_reading() {
            let idle = RangeChoice::new(day(2026, 11, 17));
            let mut c = creator();
            c.choose_range(Preset::SinceYesterday, &cfg(), &[]);
            assert!(c.range.scanning(), "fixture: a read is running");
            // (name, range, busy, expected)
            let rows = [("idle", &idle, false, true), ("busy", &idle, true, false), ("reading", &c.range, false, false), ("both", &c.range, true, false)];
            for (name, range, busy, expected) in rows {
                assert_eq!(range.controls_enabled(busy), expected, "{name}");
            }
        }

        #[test]
        fn refresh_only_for_a_read_preset_span_that_is_not_custom_and_not_reading() {
            let with = |preset: Preset, read: bool| {
                let mut r = RangeChoice::new(day(2026, 11, 17));
                r.preset = preset;
                r.scanned = read.then(|| std::sync::Arc::new(scanned()));
                r
            };
            let mut c = creator();
            c.choose_range(Preset::SinceYesterday, &cfg(), &[]);
            c.range.scanned = Some(std::sync::Arc::new(scanned()));
            assert!(c.range.scanning(), "fixture: a read is running");
            // (name, range, expected) - every false row breaks exactly one condition.
            let rows = [
                ("read preset span", with(Preset::Last7Days, true), true),
                ("report day", with(Preset::ReportDay, true), false),
                ("custom", with(Preset::Custom, true), false),
                ("nothing read yet", with(Preset::Last7Days, false), false),
            ];
            for (name, r, expected) in rows {
                assert_eq!(r.can_refresh(), expected, "{name}");
            }
            assert!(!c.range.can_refresh(), "reading again while a read runs");
        }
    }

    mod listed {
        use super::*;
        use worklog::ui_tickets::CodeSource;

        fn main_groups() -> Vec<WorkGroup> {
            vec![wg(1, "ABC-1 Login", Some("ABC"), &["api"], &["ABC-1 main line"]), wg(2, "web — no ticket yet", None, &["web"], &["uncommitted a.ts"])]
        }

        fn report_paths() -> BTreeMap<String, Vec<PathBuf>> {
            BTreeMap::from([("web".to_string(), vec![PathBuf::from(r"C:\report\web")])])
        }

        fn report_span() -> Span {
            Span::day(day(2026, 11, 16))
        }

        fn range(preset: Preset) -> RangeChoice {
            let mut r = RangeChoice::new(day(2026, 11, 17));
            r.preset = preset;
            r
        }

        type Got = (Vec<WorkGroup>, Option<(Span, BTreeMap<String, Vec<PathBuf>>)>);

        fn run(preset: Preset, read: Option<&Scanned>, with_code: bool) -> Got {
            let (main, paths) = (main_groups(), report_paths());
            let code = CodeSource { span: report_span(), repo_paths: &paths };
            let (groups, code) = ui_tickets::listed(&range(preset), read, &main, with_code.then_some(&code));
            (groups.to_vec(), code.map(|c| (c.span, c.repo_paths.clone())))
        }

        #[test]
        fn report_day_lists_the_main_rows_and_the_reports_code_even_with_a_read_at_hand() {
            let s = scanned();
            assert_eq!(run(Preset::ReportDay, Some(&s), true), (main_groups(), Some((report_span(), report_paths()))));
            assert_eq!(run(Preset::ReportDay, None, false), (main_groups(), None));
        }

        #[test]
        fn a_read_span_lists_its_groups_and_its_own_code() {
            let s = scanned();
            assert_eq!(run(Preset::Last3Days, Some(&s), true), (s.groups.clone(), Some((s.span, s.repo_paths.clone()))));
        }

        #[test]
        fn a_span_not_read_yet_lists_nothing_and_no_code() {
            assert_eq!(run(Preset::Custom, None, true), (vec![], None));
        }
    }

    mod row_title {
        use super::*;

        #[test]
        fn listed_groups_first_then_the_main_rows_else_the_bare_id() {
            let listed = vec![wg(RANGE_ROW_BASE, "ABC-1", None, &[], &[]), wg(7, "XYZ-7 from the span", None, &[], &[])];
            let main = vec![wg(7, "XYZ-7 from the main window", None, &[], &[]), wg(3, "ABC-3 main only", None, &[], &[])];
            // (name, row, expected)
            let rows = [
                ("only listed", RANGE_ROW_BASE, "ABC-1"),
                ("in both: listed wins", 7, "XYZ-7 from the span"),
                ("only in the main window", 3, "ABC-3 main only"),
                ("nowhere", 42, "row 42"),
            ];
            for (name, row, expected) in rows {
                assert_eq!(ui_tickets::row_title(row, &listed, &main), expected, "{name}");
            }
        }
    }

    /// A range with `preset`, and a finished read when `read`.
    fn range_with(preset: Preset, read: bool) -> RangeChoice {
        let mut r = RangeChoice::new(day(2026, 11, 17));
        r.preset = preset;
        r.scanned = read.then(|| std::sync::Arc::new(scanned()));
        r
    }

    /// A creator reading from Custom Mon 2026-11-02 08:30.
    fn reading_custom() -> Creator {
        let mut c = creator();
        c.range.custom_date = jiff::civil::date(2026, 11, 2);
        c.range.custom_hour = 8;
        c.range.custom_minute = 30;
        c.choose_range(Preset::Custom, &cfg(), &[]);
        assert!(c.range.scanning(), "fixture: a read is running");
        c
    }

    mod empty_hint {
        use super::*;
        use worklog::ui_tickets::EmptyHint;

        #[test]
        fn each_hint_reads_as_the_window_shows_it() {
            // (hint, text)
            let rows = [
                (EmptyHint::Reading, "reading your work from git…"),
                (EmptyHint::PickStart, "pick a start above to list your work since then"),
                (EmptyHint::NothingInRange, "no work of yours found for this project in this range"),
                (EmptyHint::NothingToday, "no work found for this project today"),
            ];
            for (hint, text) in rows {
                assert_eq!(hint.text(), text, "{hint:?}");
            }
        }

        #[test]
        fn a_running_read_always_says_so_otherwise_only_an_empty_list_gets_a_hint_by_its_source() {
            let reading = reading_custom();
            // (name, range, list empty, expected)
            let rows = [
                ("reading, empty list", &reading.range, true, Some(EmptyHint::Reading)),
                ("reading, lines listed", &reading.range, false, Some(EmptyHint::Reading)),
                ("report day with lines", &range_with(Preset::ReportDay, false), false, None),
                ("read span with lines", &range_with(Preset::Last3Days, true), false, None),
                ("report day empty", &range_with(Preset::ReportDay, false), true, Some(EmptyHint::NothingToday)),
                ("report day empty with a stale read", &range_with(Preset::ReportDay, true), true, Some(EmptyHint::NothingToday)),
                ("span not read yet", &range_with(Preset::Custom, false), true, Some(EmptyHint::PickStart)),
                ("read span empty", &range_with(Preset::Last3Days, true), true, Some(EmptyHint::NothingInRange)),
            ];
            for (name, range, empty, expected) in rows {
                assert_eq!(ui_tickets::empty_hint(range, empty), expected, "{name}");
            }
        }
    }

    mod status_line {
        use super::*;
        use worklog::ui_tickets::CodeSource;

        /// Tue 2026-11-17 14:35:20.
        fn now() -> NaiveDateTime {
            ndt(day(2026, 11, 17), 14, 35, 20)
        }

        /// A read since Thu 2026-11-12 09:00: 5 commits, 7 lines in 3 groups, 2 repos.
        fn read() -> RangeChoice {
            let mut s = scanned();
            s.groups = vec![
                wg(RANGE_ROW_BASE, "ABC-1", None, &["api"], &["a1", "a2", "a3"]),
                wg(7, "XYZ-7 Importer", None, &["db"], &["x1"]),
                wg(RANGE_ROW_BASE + 2, "web — no ticket yet", None, &["web"], &["w1", "w2", "w3"]),
            ];
            s.commits = 5;
            s.repos = 2;
            let mut r = range_with(Preset::Last7Days, false);
            r.scanned = Some(std::sync::Arc::new(s));
            r
        }

        #[test]
        fn says_the_report_day_the_running_read_or_what_the_read_found() {
            let paths = BTreeMap::new();
            let code = CodeSource { span: Span::day(day(2026, 11, 16)), repo_paths: &paths };
            let reading = reading_custom();
            // (name, range, code, expected)
            let rows: [(&str, &RangeChoice, Option<&CodeSource>, Option<&str>); 6] = [
                ("report day", &range_with(Preset::ReportDay, false), Some(&code), Some("the main window's rows: Mon 2026-11-16")),
                ("report day without a report", &range_with(Preset::ReportDay, false), None, None),
                ("reading", &reading.range, Some(&code), Some("reading git since Mon 2026-11-02 08:30 (15 d 6 h ago)…")),
                (
                    "read",
                    &read(),
                    Some(&code),
                    Some("since Thu 2026-11-12 09:00 (5 d 5 h ago) · 5 commits, 7 lines in 3 groups · 2 repos read"),
                ),
                ("span not read yet", &range_with(Preset::Custom, false), Some(&code), None),
                ("span not read yet without code", &range_with(Preset::Last3Days, false), None, None),
            ];
            for (name, range, code, expected) in rows {
                assert_eq!(ui_tickets::status_line(range, code, now()).as_deref(), expected, "{name}");
            }
        }
    }
}
