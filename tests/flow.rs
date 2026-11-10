mod common;

use chrono::NaiveTime;
use common::{day, ndt};
use worklog::flow::{self, PostJob, RowInput};
use worklog::state::Decision;

fn hm(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).unwrap()
}

mod due_date {
    use super::*;

    #[test]
    fn latest_workday_whose_popup_time_has_passed() {
        // 2026-11-06 Fri, 07 Sat, 08 Sun, 09 Mon, 10 Tue.
        let popup = hm(17, 0);
        let cases = [
            ("weekday after popup time", ndt(day(2026, 11, 10), 17, 30, 0), day(2026, 11, 10)),
            ("weekday one minute before popup time", ndt(day(2026, 11, 10), 16, 59, 0), day(2026, 11, 9)),
            ("weekday exactly at popup time", ndt(day(2026, 11, 10), 17, 0, 0), day(2026, 11, 10)),
            ("Monday morning catches up Friday", ndt(day(2026, 11, 9), 8, 0, 0), day(2026, 11, 6)),
            ("Monday midnight", ndt(day(2026, 11, 9), 0, 0, 0), day(2026, 11, 6)),
            ("Friday evening", ndt(day(2026, 11, 6), 17, 1, 0), day(2026, 11, 6)),
            ("Saturday before popup time", ndt(day(2026, 11, 7), 12, 0, 0), day(2026, 11, 6)),
            ("Saturday after popup time", ndt(day(2026, 11, 7), 18, 0, 0), day(2026, 11, 6)),
            ("Sunday after popup time", ndt(day(2026, 11, 8), 20, 0, 0), day(2026, 11, 6)),
            ("across a month boundary", ndt(day(2026, 12, 1), 9, 0, 0), day(2026, 11, 30)),
        ];
        for (case, now, expected) in cases {
            assert_eq!(flow::due_date(now, popup), expected, "case: {case}");
        }
    }

    #[test]
    fn uses_the_given_popup_time() {
        // 16:30 is after a 16:15 popup but before the 17:00 default.
        assert_eq!(flow::due_date(ndt(day(2026, 11, 10), 16, 30, 0), hm(16, 15)), day(2026, 11, 10));
    }
}

mod prompt_gate {
    use super::*;
    use worklog::flow::Gate;

    #[test]
    fn asks_when_forced_or_the_day_is_not_full_in_jira() {
        // (name, decision, logged minutes, workday minutes, force)
        type Row = (&'static str, Option<Decision>, Option<u32>, u32, bool);
        let rows: [Row; 12] = [
            ("reported bug: accepted 4h of an 8h day", Some(Decision::Accepted), Some(240), 480, false),
            ("accepted, Jira unreachable", Some(Decision::Accepted), None, 480, false),
            ("unanswered, Jira unreachable", None, None, 480, false),
            ("unanswered, nothing logged", None, Some(0), 480, false),
            ("unanswered, one minute short", None, Some(479), 480, false),
            ("accepted, one minute short of a 7h 30m workday", Some(Decision::Accepted), Some(449), 450, false),
            ("forced overrides denied and a full day", Some(Decision::Denied), Some(600), 480, true),
            ("forced overrides denied, Jira unreachable", Some(Decision::Denied), None, 480, true),
            ("forced overrides accepted and a full day", Some(Decision::Accepted), Some(480), 480, true),
            ("forced overrides a full day without a decision", None, Some(480), 480, true),
            ("forced, unanswered, Jira unreachable", None, None, 480, true),
            ("forced, accepted, short day", Some(Decision::Accepted), Some(15), 480, true),
        ];
        for (name, decision, logged, workday, force) in rows {
            assert_eq!(flow::prompt_gate(decision, logged, workday, force), Gate::Ask, "{name}");
        }
    }

    #[test]
    fn skips_a_full_day_naming_logged_and_workday() {
        // Expected text: "<fmt_minutes(logged)> already logged (workday <fmt_minutes(workday)>); use --force to ask anyway".
        // (name, decision, logged minutes, workday minutes, expected reason)
        type Row = (&'static str, Option<Decision>, u32, u32, &'static str);
        let rows: [Row; 5] = [
            ("exactly the workday, unanswered", None, 480, 480, "8h already logged (workday 8h); use --force to ask anyway"),
            ("exactly the workday, accepted", Some(Decision::Accepted), 480, 480, "8h already logged (workday 8h); use --force to ask anyway"),
            ("overtime against a 7h 30m workday", None, 545, 450, "9h 5m already logged (workday 7h 30m); use --force to ask anyway"),
            ("exactly a 7h 30m workday", Some(Decision::Accepted), 450, 450, "7h 30m already logged (workday 7h 30m); use --force to ask anyway"),
            ("zero workday is full with nothing logged", None, 0, 0, "0m already logged (workday 0m); use --force to ask anyway"),
        ];
        for (name, decision, logged, workday, expected) in rows {
            assert_eq!(flow::prompt_gate(decision, Some(logged), workday, false), Gate::Skip(expected.into()), "{name}");
        }
    }

    #[test]
    fn denied_day_is_skipped_whatever_jira_says() {
        // (name, logged minutes); a full day still gives the denied reason, not the logged one.
        let rows: [(&str, Option<u32>); 4] =
            [("Jira unreachable", None), ("nothing logged", Some(0)), ("half a day", Some(240)), ("full day", Some(480))];
        for (name, logged) in rows {
            assert_eq!(
                flow::prompt_gate(Some(Decision::Denied), logged, 480, false),
                Gate::Skip("you denied this day; use --force to ask again".into()),
                "{name}"
            );
        }
    }
}

mod week_of {
    use super::*;

    #[test]
    fn monday_to_sunday_of_the_iso_week() {
        // 2026-11-09 is a Monday; 2027-01-01 is a Friday.
        let week = |y, m, d| -> Vec<chrono::NaiveDate> { (0..7).map(|i| day(y, m, d) + chrono::TimeDelta::days(i)).collect() };
        let rows = [
            ("a Monday", day(2026, 11, 9), week(2026, 11, 9)),
            ("a Wednesday", day(2026, 11, 11), week(2026, 11, 9)),
            ("a Sunday", day(2026, 11, 15), week(2026, 11, 9)),
            ("Saturday", day(2026, 11, 14), week(2026, 11, 9)),
            ("week crossing a month boundary", day(2026, 12, 2), week(2026, 11, 30)),
            ("week crossing a year boundary", day(2027, 1, 1), week(2026, 12, 28)),
            ("Sunday of a year-crossing week", day(2027, 1, 3), week(2026, 12, 28)),
        ];
        for (name, date, expected) in rows {
            assert_eq!(flow::week_of(date), expected, "{name}");
        }
    }

    #[test]
    fn spelled_out_year_boundary_week() {
        assert_eq!(
            flow::week_of(day(2026, 12, 31)),
            vec![day(2026, 12, 28), day(2026, 12, 29), day(2026, 12, 30), day(2026, 12, 31), day(2027, 1, 1), day(2027, 1, 2), day(2027, 1, 3)]
        );
    }
}

mod week_summary {
    use super::*;
    use chrono::NaiveDate;
    use worklog::flow::{DayLine, WeekSummary};

    /// Mon 2026-11-09 .. Sun 2026-11-15.
    fn week() -> Vec<NaiveDate> {
        (9..=15).map(|d| day(2026, 11, d)).collect()
    }

    fn e(d: u32, key: &str, minutes: u32) -> (NaiveDate, String, u32) {
        (day(2026, 11, d), key.to_owned(), minutes)
    }

    fn line(d: u32, logged: u32, target: u32, future: bool) -> DayLine {
        DayLine { date: day(2026, 11, d), logged, target, future }
    }

    fn issues(v: &[(&str, u32)]) -> Vec<(String, u32)> {
        v.iter().map(|(k, m)| (k.to_string(), *m)).collect()
    }

    #[test]
    fn whole_summary_depending_on_where_today_falls() {
        // (name, entries, today, expected)
        type Row = (&'static str, Vec<(NaiveDate, String, u32)>, NaiveDate, WeekSummary);
        let rows: Vec<Row> = vec![
            (
                "mid-week: only past shortfalls count, overtime does not offset",
                vec![
                    e(9, "ABC-1", 300),
                    e(9, "XYZ-2", 240),
                    e(10, "ABC-1", 200),
                    e(12, "ABC-3", 480),
                    e(14, "ABC-1", 60),
                    // Outside the week: the Sunday before and the Monday after.
                    e(8, "ABC-9", 999),
                    e(16, "ABC-9", 77),
                ],
                day(2026, 11, 12),
                WeekSummary {
                    // Mon 540 (overtime), Tue 200, Wed 0, Thu 480 (today), Fri upcoming, Sat logged so shown, Sun empty so hidden.
                    days: vec![
                        line(9, 540, 480, false),
                        line(10, 200, 480, false),
                        line(11, 0, 480, false),
                        line(12, 480, 480, false),
                        line(13, 0, 480, true),
                        line(14, 60, 0, true),
                    ],
                    // ABC-1 300+200+60, ABC-3 480, XYZ-2 240.
                    per_issue: issues(&[("ABC-1", 560), ("ABC-3", 480), ("XYZ-2", 240)]),
                    total: 1280,
                    target_so_far: 4 * 480,
                    target_week: 5 * 480,
                    // Tue 280 + Wed 480; Monday's 60 overtime does not cover them.
                    missing_so_far: 760,
                },
            ),
            (
                "today before the week: every day future, nothing due",
                vec![e(10, "ABC-7", 60)],
                day(2026, 11, 2),
                WeekSummary {
                    days: vec![line(9, 0, 480, true), line(10, 60, 480, true), line(11, 0, 480, true), line(12, 0, 480, true), line(13, 0, 480, true)],
                    per_issue: issues(&[("ABC-7", 60)]),
                    total: 60,
                    target_so_far: 0,
                    target_week: 2400,
                    missing_so_far: 0,
                },
            ),
            (
                "today after the week: every workday counted",
                vec![e(9, "ABC-7", 480), e(11, "ABC-8", 100), e(13, "ABC-7", 500)],
                day(2026, 11, 20),
                WeekSummary {
                    days: vec![line(9, 480, 480, false), line(10, 0, 480, false), line(11, 100, 480, false), line(12, 0, 480, false), line(13, 500, 480, false)],
                    per_issue: issues(&[("ABC-7", 980), ("ABC-8", 100)]),
                    total: 1080,
                    target_so_far: 2400,
                    target_week: 2400,
                    // Tue 480 + Wed 380 + Thu 480.
                    missing_so_far: 1340,
                },
            ),
        ];
        for (name, entries, today, expected) in rows {
            assert_eq!(flow::week_summary(&entries, &week(), today, 480), expected, "{name}");
        }
    }

    #[test]
    fn weekend_logging_is_shown_and_totalled_but_does_not_reduce_missing() {
        // Today is the Sunday: the whole week is past. Only Sunday has weekend work.
        let entries = vec![e(9, "ABC-4", 420), e(10, "ABC-4", 450), e(11, "ABC-5", 450), e(12, "ABC-5", 450), e(13, "ABC-6", 300), e(15, "ABC-6", 180)];
        let s = flow::week_summary(&entries, &week(), day(2026, 11, 15), 450);
        assert_eq!(
            s.days,
            vec![
                line(9, 420, 450, false),
                line(10, 450, 450, false),
                line(11, 450, 450, false),
                line(12, 450, 450, false),
                line(13, 300, 450, false),
                line(15, 180, 0, false),
            ]
        );
        assert_eq!(s.total, 420 + 450 * 3 + 300 + 180);
        assert_eq!((s.target_so_far, s.target_week), (5 * 450, 5 * 450));
        // Mon 30 + Fri 150; Sunday's 180 doesn't count against them.
        assert_eq!(s.missing_so_far, 180);
        assert_eq!(s.per_issue, issues(&[("ABC-5", 900), ("ABC-4", 870), ("ABC-6", 480)]));
    }

    #[test]
    fn per_issue_sums_repeated_keys_and_breaks_ties_by_key() {
        // String order: "AB-5" < "ABC-10" < "ABC-2" < "XYZ-1".
        let entries = vec![
            e(10, "XYZ-1", 90),
            e(11, "ABC-2", 50),
            e(9, "AB-5", 30),
            e(12, "ABC-10", 90),
            e(11, "ABC-2", 40),
            e(13, "ZZ-1", 120),
        ];
        let s = flow::week_summary(&entries, &week(), day(2026, 11, 13), 480);
        assert_eq!(s.per_issue, issues(&[("ZZ-1", 120), ("ABC-10", 90), ("ABC-2", 90), ("XYZ-1", 90), ("AB-5", 30)]));
        assert_eq!(s.days[2], line(11, 90, 480, false));
        assert_eq!(s.total, 420);
    }

    #[test]
    fn monday_is_today_and_counted_tuesday_is_future() {
        let s = flow::week_summary(&[e(9, "ABC-1", 100)], &week(), day(2026, 11, 9), 480);
        assert_eq!(
            s.days,
            vec![line(9, 100, 480, false), line(10, 0, 480, true), line(11, 0, 480, true), line(12, 0, 480, true), line(13, 0, 480, true)]
        );
        assert_eq!((s.target_so_far, s.missing_so_far), (480, 380));
    }

    #[test]
    fn saturday_shown_and_sunday_hidden_when_only_saturday_logged() {
        let s = flow::week_summary(&[e(14, "ABC-1", 45)], &week(), day(2026, 11, 15), 480);
        let dates: Vec<NaiveDate> = s.days.iter().map(|d| d.date).collect();
        assert_eq!(dates, (9..=14).map(|d| day(2026, 11, d)).collect::<Vec<_>>());
        assert_eq!(s.days[5], line(14, 45, 0, false));
        assert_eq!(s.missing_so_far, 2400);
    }

    #[test]
    fn nothing_logged_and_only_out_of_week_entries_give_an_empty_week() {
        for (name, entries) in [("no entries", vec![]), ("only outside the week", vec![e(8, "ABC-1", 60), e(16, "ABC-2", 70)])] {
            let s = flow::week_summary(&entries, &week(), day(2026, 11, 11), 480);
            assert_eq!(
                s,
                WeekSummary {
                    days: vec![line(9, 0, 480, false), line(10, 0, 480, false), line(11, 0, 480, false), line(12, 0, 480, true), line(13, 0, 480, true)],
                    per_issue: vec![],
                    total: 0,
                    target_so_far: 1440,
                    target_week: 2400,
                    missing_so_far: 1440,
                },
                "{name}"
            );
        }
    }
}

mod render_week {
    use super::*;
    use chrono::NaiveDate;
    use worklog::flow::{DayLine, WeekSummary};

    /// Mon 2026-11-09 .. Sun 2026-11-15.
    fn week() -> Vec<NaiveDate> {
        (9..=15).map(|d| day(2026, 11, d)).collect()
    }

    fn line(d: u32, logged: u32, target: u32, future: bool) -> DayLine {
        DayLine { date: day(2026, 11, d), logged, target, future }
    }

    // Day line: "  <Www> <date>  <logged right-aligned in 7> / <target left-aligned in 4> <note><marker>".
    // Issue line: "  <key left-aligned in 14> <minutes right-aligned in 7>".

    #[test]
    fn every_note_marker_issue_block_and_totals() {
        let s = WeekSummary {
            days: vec![
                line(9, 540, 480, false),
                line(10, 200, 480, false),
                line(11, 480, 480, false),
                line(12, 479, 480, false),
                line(13, 30, 480, true),
                line(14, 75, 0, true),
            ],
            per_issue: vec![("ABC-1".into(), 600), ("XYZ-12".into(), 45)],
            total: 1404,
            target_so_far: 1920,
            target_week: 2400,
            missing_so_far: 281,
        };
        let expected = [
            "Week 2026-11-09 – 2026-11-15 for jdoe",
            "",
            // overtime -> ok
            "  Mon 2026-11-09       9h / 8h   ok",
            // 480 - 200 = 280 = 4h 40m
            "  Tue 2026-11-10   3h 20m / 8h   missing 4h 40m",
            // exactly the target -> ok
            "  Wed 2026-11-11       8h / 8h   ok",
            // one minute short, and today
            "  Thu 2026-11-12   7h 59m / 8h   missing 1m ← today",
            "  Fri 2026-11-13      30m / 8h   upcoming",
            // weekend wins over future
            "  Sat 2026-11-14   1h 15m / 0m   weekend",
            "",
            "By issue:",
            &format!("  ABC-1{}10h", " ".repeat(9 + 1 + 4)),
            &format!("  XYZ-12{}45m", " ".repeat(8 + 1 + 4)),
            "",
            "Total 23h 24m · target so far 32h (missing 4h 41m) · full week 40h",
            "",
        ]
        .join("\n");
        assert_eq!(flow::render_week(&s, "jdoe", &week(), day(2026, 11, 12)), expected);
    }

    #[test]
    fn no_issue_block_when_nothing_logged_per_issue_and_weekend_by_weekday() {
        // Monday with a 0 target is a workday ("ok"), not "weekend"; the past Sunday is "weekend".
        let s = WeekSummary {
            days: vec![line(9, 0, 0, false), line(15, 60, 0, false)],
            per_issue: vec![],
            total: 60,
            target_so_far: 0,
            target_week: 0,
            missing_so_far: 0,
        };
        let expected = [
            "Week 2026-11-09 – 2026-11-15 for anna.k",
            "",
            "  Mon 2026-11-09       0m / 0m   ok",
            "  Sun 2026-11-15       1h / 0m   weekend ← today",
            "",
            "Total 1h · target so far 0m (missing 0m) · full week 0m",
            "",
        ]
        .join("\n");
        assert_eq!(flow::render_week(&s, "anna.k", &week(), day(2026, 11, 15)), expected);
    }

    #[test]
    fn today_outside_the_week_marks_no_line() {
        let s = WeekSummary {
            days: vec![line(9, 480, 480, false), line(13, 120, 480, false)],
            per_issue: vec![("ABC-3".into(), 600)],
            total: 600,
            target_so_far: 2400,
            target_week: 2400,
            missing_so_far: 360,
        };
        let expected = [
            "Week 2026-11-09 – 2026-11-15 for jdoe",
            "",
            "  Mon 2026-11-09       8h / 8h   ok",
            "  Fri 2026-11-13       2h / 8h   missing 6h",
            "",
            "By issue:",
            &format!("  ABC-3{}10h", " ".repeat(9 + 1 + 4)),
            "",
            "Total 10h · target so far 40h (missing 6h) · full week 40h",
            "",
        ]
        .join("\n");
        assert_eq!(flow::render_week(&s, "jdoe", &week(), day(2026, 11, 20)), expected);
    }
}

fn row(checked: bool, minutes: u32, target: Option<&str>, posted_ok: bool, comment: &str) -> RowInput {
    RowInput { checked, minutes, target: target.map(str::to_owned), posted_ok, comment: comment.into() }
}

mod postable {
    use super::*;

    #[test]
    fn each_condition_alone_excludes_a_row() {
        // Rows 1 and 5 meet all four conditions; each of rows 0, 2, 3, 4 fails exactly one.
        let rows = vec![
            row(false, 30, Some("ABC-1"), false, "unchecked"),
            row(true, 45, Some("ABC-2"), false, "ok"),
            row(true, 0, Some("ABC-3"), false, "zero minutes"),
            row(true, 15, Some("ABC-4"), true, "already posted"),
            row(true, 60, None, false, "no target"),
            row(true, 90, Some("XYZ-6"), false, "ok too"),
        ];
        assert_eq!(flow::postable(&rows), vec![1, 5]);
    }

    #[test]
    fn nothing_postable() {
        assert_eq!(flow::postable(&[]), Vec::<usize>::new());
        assert_eq!(flow::postable(&[row(false, 30, Some("ABC-1"), false, "")]), Vec::<usize>::new());
    }
}

mod plan_posts {
    use super::*;

    #[test]
    fn back_to_back_after_logged_and_already_posted_minutes() {
        let d = day(2026, 11, 4);
        let rows = vec![
            row(true, 60, Some("ABC-1"), true, "posted earlier"),
            row(true, 30, Some("ABC-2"), false, "- fix B"),
            row(false, 15, Some("ABC-3"), false, "unchecked"),
            row(true, 90, Some("XYZ-4"), false, "- picked for unassigned"),
            row(true, 20, None, false, "no target"),
        ];
        // 09:00 + 45 already logged + 60 posted by an earlier Accept = 10:45.
        assert_eq!(
            flow::plan_posts(d, hm(9, 0), 45, &rows),
            vec![
                PostJob { row: 1, key: "ABC-2".into(), started: ndt(d, 10, 45, 0), minutes: 30, comment: "- fix B".into() },
                PostJob { row: 3, key: "XYZ-4".into(), started: ndt(d, 11, 15, 0), minutes: 90, comment: "- picked for unassigned".into() },
            ]
        );
    }

    #[test]
    fn retry_skips_rows_posted_ok() {
        let d = day(2026, 11, 4);
        let rows = vec![
            row(true, 60, Some("ABC-1"), true, "a"),
            row(true, 30, Some("ABC-2"), false, "b"),
            row(true, 45, Some("ABC-3"), true, "c"),
        ];
        // 08:30 + 0 logged + 60 + 45 posted = 10:15.
        assert_eq!(
            flow::plan_posts(d, hm(8, 30), 0, &rows),
            vec![PostJob { row: 1, key: "ABC-2".into(), started: ndt(d, 10, 15, 0), minutes: 30, comment: "b".into() }]
        );
    }

    #[test]
    fn everything_posted_plans_nothing() {
        let rows = vec![row(true, 60, Some("ABC-1"), true, "a")];
        assert_eq!(flow::plan_posts(day(2026, 11, 4), hm(9, 0), 0, &rows), vec![]);
    }
}

mod drain {
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn applies_all_results_and_finishes_when_sender_is_gone() {
        let (tx, rx) = mpsc::channel::<(usize, &str)>();
        tx.send((2, "posted 101")).unwrap();
        tx.send((0, "failed: 400")).unwrap();
        drop(tx);
        let mut results = vec![None, None, None];
        let mut remaining = 3;
        assert!(flow::drain(&rx, &mut results, &mut remaining));
        assert_eq!(results, vec![Some("failed: 400"), None, Some("posted 101")]);
        assert_eq!(remaining, 1);
    }

    #[test]
    fn keeps_waiting_while_results_are_missing() {
        let (tx, rx) = mpsc::channel::<(usize, &str)>();
        tx.send((1, "posted 7")).unwrap();
        let mut results = vec![None, None];
        let mut remaining = 2;
        assert!(!flow::drain(&rx, &mut results, &mut remaining));
        assert_eq!(results, vec![None, Some("posted 7")]);
        assert_eq!(remaining, 1);
        drop(tx);
    }

    #[test]
    fn finishes_when_all_expected_arrived_while_sender_alive() {
        let (tx, rx) = mpsc::channel::<(usize, &str)>();
        tx.send((1, "b")).unwrap();
        tx.send((0, "a")).unwrap();
        let mut results = vec![None, None];
        let mut remaining = 2;
        assert!(flow::drain(&rx, &mut results, &mut remaining));
        assert_eq!(results, vec![Some("a"), Some("b")]);
        assert_eq!(remaining, 0);
        drop(tx);
    }

    #[test]
    fn nothing_sent_leaves_state_untouched() {
        let (tx, rx) = mpsc::channel::<(usize, &str)>();
        let mut results: Vec<Option<&str>> = vec![None, None];
        let mut remaining = 2;
        assert!(!flow::drain(&rx, &mut results, &mut remaining));
        assert_eq!(results, vec![None, None]);
        assert_eq!(remaining, 2);
        drop(tx);
    }
}

mod apply_created {
    use super::*;
    use worklog::flow::Pick;
    use worklog::jira::Issue;

    fn issue(key: &str, summary: &str) -> Issue {
        Issue { key: key.into(), summary: summary.into(), status: "Open".into() }
    }

    fn pick(picked: Option<usize>, checked: bool, posted_ok: bool, own_key: bool) -> Pick {
        Pick { picked, checked, posted_ok, own_key }
    }

    /// One blocker per row where possible, so each condition is tested alone: row 1 and row 6 are
    /// free (no pick, not logged, no own key; 6 is already ticked), row 3 only has a pick (a failed
    /// post), row 4 is only logged, row 5 only has its own key. Rows 0 and 2 have picks too.
    fn picks() -> Vec<Pick> {
        vec![
            pick(Some(1), true, false, false),
            pick(None, false, false, false),
            pick(Some(0), true, true, false),
            pick(Some(1), false, false, false),
            pick(None, false, true, false),
            pick(None, false, false, true),
            pick(None, true, false, false),
        ]
    }

    /// The same picks after every index moved one down.
    fn shifted() -> Vec<Pick> {
        vec![
            pick(Some(2), true, false, false),
            pick(None, false, false, false),
            pick(Some(1), true, true, false),
            pick(Some(2), false, false, false),
            pick(None, false, true, false),
            pick(None, false, false, true),
            pick(None, true, false, false),
        ]
    }

    fn shifted_with_new_ticket_on(rows: &[usize]) -> Vec<Pick> {
        let mut p = shifted();
        for &r in rows {
            p[r].picked = Some(0);
            p[r].checked = true;
        }
        p
    }

    #[test]
    fn inserts_at_top_shifts_every_pick_and_picks_it_for_free_target_rows() {
        // (name, targets, expected picks)
        let rows: Vec<(&str, Vec<usize>, Vec<Pick>)> = vec![
            ("no targets", vec![], shifted()),
            ("targets past the rows", vec![7, 100], shifted()),
            ("free row is picked and ticked", vec![1], shifted_with_new_ticket_on(&[1])),
            ("free row already ticked is picked", vec![6], shifted_with_new_ticket_on(&[6])),
            ("several free rows", vec![6, 1], shifted_with_new_ticket_on(&[1, 6])),
            ("row whose post failed keeps its pick", vec![3], shifted()),
            ("row with a pick keeps it", vec![0], shifted()),
            ("logged row with a pick keeps it", vec![2], shifted()),
            ("logged row without a pick stays unpicked", vec![4], shifted()),
            ("row with its own key stays unpicked", vec![5], shifted()),
            ("repeated target", vec![1, 1], shifted_with_new_ticket_on(&[1])),
            ("mixed: only the free rows", vec![5, 1, 4, 9, 6, 3, 0, 2], shifted_with_new_ticket_on(&[1, 6])),
        ];
        for (name, targets, expected) in rows {
            let mut candidates = vec![issue("ABC-1", "one"), issue("ABC-2", "two")];
            let mut p = picks();
            flow::apply_created(&mut candidates, &mut p, issue("XYZ-9", "new one"), &targets);
            assert_eq!(candidates, vec![issue("XYZ-9", "new one"), issue("ABC-1", "one"), issue("ABC-2", "two")], "{name}");
            assert_eq!(p, expected, "{name}");
        }
    }

    #[test]
    fn second_ticket_for_the_same_row_does_not_override_the_first() {
        let mut candidates = vec![issue("ABC-1", "one")];
        let mut p = vec![pick(None, false, false, false), pick(None, false, false, false)];
        flow::apply_created(&mut candidates, &mut p, issue("ABC-7", "first new"), &[1]);
        flow::apply_created(&mut candidates, &mut p, issue("ABC-8", "second new"), &[1, 0]);
        assert_eq!(candidates, vec![issue("ABC-8", "second new"), issue("ABC-7", "first new"), issue("ABC-1", "one")]);
        // Row 1 still points at ABC-7 (now index 1); row 0 was free and takes ABC-8.
        assert_eq!(p, vec![pick(Some(0), true, false, false), pick(Some(1), true, false, false)]);
    }

    #[test]
    fn first_ticket_into_empty_lists() {
        let mut candidates = vec![];
        let mut p: Vec<Pick> = vec![];
        flow::apply_created(&mut candidates, &mut p, issue("ABC-3", "first"), &[0]);
        assert_eq!(candidates, vec![issue("ABC-3", "first")]);
        assert_eq!(p, vec![]);

        let mut candidates = vec![];
        let mut p = vec![pick(None, false, false, false)];
        flow::apply_created(&mut candidates, &mut p, issue("ABC-3", "first"), &[0]);
        assert_eq!(candidates, vec![issue("ABC-3", "first")]);
        assert_eq!(p, vec![pick(Some(0), true, false, false)]);
    }
}

mod apply_updated {
    use super::*;
    use std::collections::BTreeMap;
    use worklog::jira::Issue;
    use worklog::plan::{Bucket, Suggestion};
    use worklog::report::{Report, Row};

    fn issue(key: &str, summary: &str, status: &str) -> Issue {
        Issue { key: key.into(), summary: summary.into(), status: status.into() }
    }

    fn row(bucket: Bucket, issue: Option<Issue>) -> Row {
        let suggestion = Suggestion { bucket, minutes: 60, raw_minutes: 45, repos: vec!["svc".into()], evidence: vec!["e".into()], commits: 1 };
        Row { suggestion, issue, already_logged_minutes: 15 }
    }

    /// ABC-1 is a candidate and the issue of rows 0 and 3; ABC-10 and abc-1 only look alike.
    fn report() -> Report {
        Report {
            date: day(2026, 11, 4),
            me: "jdoe".into(),
            rows: vec![
                row(Bucket::Issue("ABC-1".into()), Some(issue("ABC-1", "Old one", "Open"))),
                row(Bucket::Issue("ABC-10".into()), Some(issue("ABC-10", "Ten", "Done"))),
                row(Bucket::Unassigned("svc".into()), None),
                row(Bucket::Issue("ABC-1".into()), Some(issue("ABC-1", "Old one", "Open"))),
                row(Bucket::Issue("abc-1".into()), Some(issue("abc-1", "Lower", "Open"))),
            ],
            already_logged_minutes: 30,
            budget_minutes: 480,
            candidates: vec![issue("ABC-2", "Two", "Open"), issue("ABC-1", "Old one", "Open"), issue("ABC-10", "Ten", "Done")],
            warnings: vec![],
            repo_projects: BTreeMap::new(),
            repo_paths: BTreeMap::new(),
        }
    }

    fn row_issues(r: &Report) -> Vec<Option<Issue>> {
        r.rows.iter().map(|x| x.issue.clone()).collect()
    }

    #[test]
    fn summary_and_status_of_every_candidate_and_row_with_that_key() {
        let mut r = report();
        flow::apply_updated(&mut r, &issue("ABC-1", "Saved title", "In Review"));
        assert_eq!(r.candidates, vec![issue("ABC-2", "Two", "Open"), issue("ABC-1", "Saved title", "In Review"), issue("ABC-10", "Ten", "Done")]);
        assert_eq!(
            row_issues(&r),
            vec![
                Some(issue("ABC-1", "Saved title", "In Review")),
                Some(issue("ABC-10", "Ten", "Done")),
                None,
                Some(issue("ABC-1", "Saved title", "In Review")),
                Some(issue("abc-1", "Lower", "Open")),
            ]
        );
    }

    #[test]
    fn key_nowhere_in_the_report_changes_nothing() {
        let mut r = report();
        flow::apply_updated(&mut r, &issue("XY-1", "Other", "Done"));
        assert_eq!(r.candidates, report().candidates);
        assert_eq!(row_issues(&r), row_issues(&report()));
    }
}

mod row_ids {
    use super::*;
    use worklog::flow::RowIds;

    #[test]
    fn takes_are_distinct_and_increasing_from_zero() {
        let mut ids = RowIds::default();
        let taken: Vec<u64> = (0..4).map(|_| ids.take()).collect();
        assert_eq!(taken, vec![0, 1, 2, 3]);
    }

    #[test]
    fn removed_row_id_is_never_reused_and_no_longer_found() {
        let mut id_source =RowIds::default();
        // Five git rows at load, then "+ Add row".
        let mut rows: Vec<u64> = (0..5).map(|_| id_source.take()).collect();
        let manual = id_source.take();
        rows.push(manual);
        assert_eq!((manual, flow::row_index(&rows, manual)), (5, Some(5)));

        // A ticket window opened from the manual row, then the row is removed and another added.
        let origin = manual;
        rows.retain(|&i| i != manual);
        let added = id_source.take();
        rows.push(added);
        assert_eq!(added, 6);
        assert_eq!(rows, vec![0, 1, 2, 3, 4, 6]);
        assert_eq!(flow::row_index(&rows, origin), None);
        assert_eq!(flow::row_index(&rows, added), Some(5));
    }

    #[test]
    fn surviving_row_is_found_at_its_shifted_index() {
        let mut id_source =RowIds::default();
        let mut rows: Vec<u64> = (0..5).map(|_| id_source.take()).collect();
        let origin = rows[3];
        rows.remove(1);
        assert_eq!(flow::row_index(&rows, origin), Some(2));
        assert_eq!(flow::row_index(&rows, 1), None);
        assert_eq!(flow::row_index(&[], 0), None);
    }
}

mod group_project {
    use super::*;
    use std::collections::BTreeMap;
    use worklog::plan::Bucket;

    #[test]
    fn ticket_project_else_none_for_manual_rows_else_the_repos_project() {
        let repo_projects = BTreeMap::from([("svc".to_string(), "ABC".to_string()), ("web2".to_string(), "XYZ".to_string())]);
        // (name, bucket, manual, expected)
        let rows: [(&str, Bucket, bool, Option<&str>); 7] = [
            ("ticket row", Bucket::Issue("ABC-12".into()), false, Some("ABC")),
            ("manual ticket row still has its ticket's project", Bucket::Issue("XYZ-4".into()), true, Some("XYZ")),
            ("ticket project absent from the repo map",Bucket::Issue("XY-3".into()), false, Some("XY")),
            ("unassigned repo with a project", Bucket::Unassigned("svc".into()), false, Some("ABC")),
            ("second mapped repo", Bucket::Unassigned("web2".into()), false, Some("XYZ")),
            ("unassigned repo without a project", Bucket::Unassigned("tool".into()), false, None),
            ("manual unassigned row ignores the repo map", Bucket::Unassigned("svc".into()), true, None),
        ];
        for (name, bucket, manual, expected) in rows {
            assert_eq!(flow::group_project(&bucket, manual, &repo_projects).as_deref(), expected, "{name}");
        }
    }

    #[test]
    fn empty_map_leaves_unassigned_rows_without_project() {
        assert_eq!(flow::group_project(&Bucket::Unassigned("svc".into()), false, &BTreeMap::new()), None);
    }
}

mod created_targets {
    use super::*;

    #[test]
    fn current_index_of_each_wanted_row_in_wanted_order_missing_dropped() {
        let ids = [7, 3, 9, 12];
        // (name, wanted ids, expected indices)
        let rows: [(&str, &[u64], Vec<usize>); 7] = [
            ("wanted order, not row order", &[9, 7], vec![2, 0]),
            ("removed row dropped", &[12, 5, 3], vec![3, 1]),
            ("duplicates give duplicate indices", &[3, 3], vec![1, 1]),
            ("last row", &[12], vec![3]),
            ("all removed", &[1, 2], vec![]),
            ("nothing wanted", &[], vec![]),
            ("every row", &[7, 3, 9, 12], vec![0, 1, 2, 3]),
        ];
        for (name, wanted, expected) in rows {
            assert_eq!(flow::created_targets(&ids, wanted), expected, "{name}");
        }
        assert_eq!(flow::created_targets(&[], &[3]), Vec::<usize>::new());
    }
}

#[test]
fn decision_after_posting_accepts_only_when_something_was_posted() {
    assert_eq!(flow::decision_after_posting(true), Some(Decision::Accepted));
    assert_eq!(flow::decision_after_posting(false), None);
}
