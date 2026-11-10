mod common;

use chrono::{DateTime, Local, TimeDelta};
use common::{day, local_at};
use worklog::git::{Event, EventKind};
use worklog::plan::{self, Bucket, Params, Suggestion};

fn t0() -> DateTime<Local> {
    local_at(day(2026, 11, 4), 9, 0, 0)
}

fn ev(repo: &str, offset_secs: i64, kind: EventKind, what: &str, key: Option<&str>) -> Event {
    Event {
        repo: repo.into(),
        time: t0() + TimeDelta::seconds(offset_secs),
        kind,
        what: what.into(),
        key: key.map(str::to_owned),
    }
}

fn commit(repo: &str, minute: i64, subject: &str, key: Option<&str>) -> Event {
    ev(repo, minute * 60, EventKind::Commit, subject, key)
}

const P: Params = Params { budget_minutes: 420, round_minutes: 15, max_gap_minutes: 120, session_start_minutes: 30 };

fn issue(k: &str) -> Bucket {
    Bucket::Issue(k.into())
}
fn unassigned(r: &str) -> Bucket {
    Bucket::Unassigned(r.into())
}

/// (bucket, minutes, raw_minutes) of each suggestion, in output order.
fn shape(s: &[Suggestion]) -> Vec<(Bucket, u32, u32)> {
    s.iter().map(|x| (x.bucket.clone(), x.minutes, x.raw_minutes)).collect()
}

mod allocate {
    use super::*;

    /// One named row per allocation rule: commits `(repo, minute offset, key)` in, `(bucket, minutes,
    /// raw_minutes)` out, in output order. Buckets are written "ABC-1" (issue) or "~web"
    /// (unassigned repo web).
    #[test]
    fn allocates_per_contract() {
        type Row = (&'static str, Params, &'static [(&'static str, i64, Option<&'static str>)], &'static [(&'static str, u32, u32)]);
        let bucket = |s: &str| match s.strip_prefix('~') {
            Some(repo) => unassigned(repo),
            None => issue(s),
        };
        let rows: Vec<Row> = vec![
            (
                // ABC-1: 30 (session start) + 40 (gap) = 70; web: gap 20.
                "single ticket gets the whole workday, unassigned gets zero",
                P,
                &[("api", 0, Some("ABC-1")), ("api", 40, Some("ABC-1")), ("web", 60, None)],
                &[("ABC-1", 420, 70), ("~web", 0, 20)],
            ),
            (
                // ABC-1: 30 + 30 = 60; ABC-2: gap 90; total 150, 28 steps: 11.2 / 16.8 -> 11 / 16,
                // spare step to the larger remainder (ABC-2) -> 165 / 255. api: after 180 min
                // (> max gap) session start 30, minutes 0.
                "two tickets split by largest remainder",
                P,
                &[("api", 0, Some("ABC-1")), ("api", 90, Some("ABC-2")), ("api", 120, Some("ABC-1")), ("api", 300, None)],
                &[("ABC-2", 255, 90), ("ABC-1", 165, 60), ("~api", 0, 30)],
            ),
            (
                // api: 30 + 60 = 90, web: 30 -> 28 steps: 21 / 7.
                "unassigned rows share when no ticket exists",
                P,
                &[("api", 0, None), ("api", 60, None), ("web", 90, None)],
                &[("~api", 315, 90), ("~web", 105, 30)],
            ),
            ("first event gets session start credit", Params { session_start_minutes: 25, ..P }, &[("api", 0, Some("ABC-1"))], &[("ABC-1", 420, 25)]),
            (
                // 400 / 15 = 26 steps = 390; 60/90 -> 10.4 / 15.6 -> 10 / 15, spare to ABC-2.
                "budget not a multiple of round is rounded down",
                Params { budget_minutes: 400, ..P },
                &[("api", 0, Some("ABC-1")), ("api", 90, Some("ABC-2")), ("api", 120, Some("ABC-1"))],
                &[("ABC-2", 240, 90), ("ABC-1", 150, 60)],
            ),
            (
                // ABC-1: 30 + 119 + 119 = 268, ABC-2: 1; 27.9 / 0.1 -> 27 / 0, spare to ABC-1 -> 28 / 0,
                // one step moves to ABC-2 -> 27 / 1.
                "each ticket gets at least one step",
                P,
                &[("api", 0, Some("ABC-1")), ("api", 119, Some("ABC-1")), ("api", 238, Some("ABC-1")), ("api", 239, Some("ABC-2"))],
                &[("ABC-1", 405, 268), ("ABC-2", 15, 1)],
            ),
            (
                "budget smaller than one step gives zero everywhere",
                Params { budget_minutes: 10, ..P },
                &[("api", 0, Some("ABC-1")), ("web", 5, None)],
                &[("ABC-1", 0, 30), ("~web", 0, 5)],
            ),
            (
                // 30 each (gaps > max gap); 14 steps each. Issue before Unassigned, then by string
                // ("ABC-10" < "ABC-2", "alpha" < "zeta").
                "equal minutes are ordered by bucket",
                P,
                &[("web", 0, Some("ABC-2")), ("web", 151, Some("ABC-10")), ("zeta", 400, None), ("alpha", 700, None)],
                &[("ABC-10", 210, 30), ("ABC-2", 210, 30), ("~alpha", 0, 30), ("~zeta", 0, 30)],
            ),
        ];
        for (case, params, input, expected) in rows {
            let events: Vec<Event> = input.iter().map(|(repo, m, k)| commit(repo, *m, &format!("c{m}"), *k)).collect();
            let expected: Vec<(Bucket, u32, u32)> = expected.iter().map(|(b, m, r)| (bucket(b), *m, *r)).collect();
            assert_eq!(shape(&plan::allocate(&events, params)), expected, "case: {case}");
        }
    }

    #[test]
    fn unsorted_input_is_credited_in_time_order() {
        let mut events = vec![
            commit("api", 0, "ABC-1 a", Some("ABC-1")),
            commit("api", 90, "ABC-2 a", Some("ABC-2")),
            commit("api", 120, "ABC-1 b", Some("ABC-1")),
        ];
        events.reverse();
        assert_eq!(shape(&plan::allocate(&events, P)), vec![(issue("ABC-2"), 255, 90), (issue("ABC-1"), 165, 60)]);
    }

    #[test]
    fn gap_equal_to_max_gap_is_credited_as_gap() {
        let events = vec![commit("api", 0, "ABC-1", Some("ABC-1")), commit("api", 120, "ABC-2", Some("ABC-2"))];
        let raw: Vec<_> = plan::allocate(&events, P).iter().map(|s| (s.bucket.clone(), s.raw_minutes)).collect();
        assert_eq!(raw, vec![(issue("ABC-2"), 120), (issue("ABC-1"), 30)]);
    }

    #[test]
    fn gap_longer_than_max_gap_starts_a_new_session() {
        let p = Params { session_start_minutes: 25, ..P };
        let events = vec![commit("api", 0, "ABC-1", Some("ABC-1")), ev("api", 120 * 60 + 1, EventKind::Commit, "ABC-2", Some("ABC-2"))];
        let mut raw: Vec<_> = plan::allocate(&events, p).iter().map(|s| (s.bucket.clone(), s.raw_minutes)).collect();
        raw.sort();
        assert_eq!(raw, vec![(issue("ABC-1"), 25), (issue("ABC-2"), 25)]);
    }

    #[test]
    fn raw_minutes_round_down_from_seconds() {
        // 30 min + 90 s = 1890 s = 31.5 min -> 31.
        let events = vec![ev("api", 0, EventKind::Commit, "a", Some("ABC-1")), ev("api", 90, EventKind::Commit, "b", Some("ABC-1"))];
        assert_eq!(plan::allocate(&events, P)[0].raw_minutes, 31);
    }

    #[test]
    fn no_events_no_suggestions() {
        assert_eq!(plan::allocate(&[], P), Vec::<Suggestion>::new());
    }

    #[test]
    fn evidence_is_commits_then_checkouts_then_files_deduplicated_and_repos_in_order() {
        let events = vec![
            ev("api", 0, EventKind::Checkout, "feat/abc-1", Some("ABC-1")),
            ev("web", 60, EventKind::Uncommitted, "src/a.rs", Some("ABC-1")),
            ev("api", 120, EventKind::Commit, "fix A", Some("ABC-1")),
            ev("web", 180, EventKind::Commit, "fix B", Some("ABC-1")),
            ev("web", 240, EventKind::Commit, "fix A", Some("ABC-1")),
            ev("api", 300, EventKind::Checkout, "feat/abc-1", Some("ABC-1")),
            ev("api", 360, EventKind::Uncommitted, "src/b.rs", Some("ABC-1")),
        ];
        let s = plan::allocate(&events, P);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].repos, vec!["api".to_string(), "web".to_string()]);
        // "fix A" and "fix B": the duplicate "fix A" is not counted twice.
        assert_eq!(s[0].commits, 2);
        assert_eq!(
            s[0].evidence,
            vec!["fix A", "fix B", "checkout feat/abc-1", "uncommitted src/a.rs", "uncommitted src/b.rs"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn unassigned_buckets_are_per_repo() {
        let events = vec![commit("api", 0, "a", None), commit("web", 10, "b", None), commit("api", 20, "c", None)];
        let s = plan::allocate(&events, P);
        // api: 30 + 10 = 40, web: 10 -> 28 steps: 22.4 / 5.6 -> 22 + 5, spare to web (0.6) -> 22 / 6.
        assert_eq!(shape(&s), vec![(unassigned("api"), 330, 40), (unassigned("web"), 90, 10)]);
        assert_eq!(s[0].evidence, vec!["a".to_string(), "c".to_string()]);
        assert_eq!((s[0].commits, s[1].commits), (2, 1));
    }

    #[test]
    fn shared_minutes_always_sum_to_rounded_budget() {
        // (budget, round, minutes offsets per key) - the invariant floor(budget/round)*round.
        type Case = (u32, u32, &'static [(i64, &'static str)]);
        let cases: &[Case] = &[
            (420, 15, &[(0, "ABC-1"), (7, "ABC-2"), (19, "ABC-3"), (50, "ABC-4")]),
            (419, 15, &[(0, "ABC-1"), (100, "ABC-2"), (101, "ABC-3")]),
            (480, 30, &[(0, "ABC-1"), (1, "ABC-2"), (2, "ABC-3"), (3, "ABC-4"), (4, "ABC-5")]),
            (60, 15, &[(0, "ABC-1"), (13, "ABC-2"), (27, "ABC-3"), (44, "ABC-4"), (59, "ABC-5"), (61, "ABC-6")]),
            (420, 7, &[(0, "ABC-1"), (33, "ABC-2")]),
        ];
        for (budget, round, evs) in cases {
            let p = Params { budget_minutes: *budget, round_minutes: *round, ..P };
            let events: Vec<Event> = evs.iter().map(|(m, k)| commit("api", *m, k, Some(k))).collect();
            let s = plan::allocate(&events, p);
            assert_eq!(s.iter().map(|x| x.minutes).sum::<u32>(), budget / round * round, "budget {budget} round {round}");
            assert!(s.iter().all(|x| x.minutes % round == 0), "multiples of {round}: {s:?}");
        }
    }
}

mod distribute {
    use super::*;

    #[test]
    fn splits_per_contract() {
        // (case, weights, units, expected) - derivations in the case names / comments.
        type Case = (&'static str, &'static [u64], u32, &'static [u32]);
        let cases: &[Case] = &[
            // 60/90 of 28 -> 11.2 / 16.8 -> 11 / 17.
            ("proportional, largest remainder", &[60, 90], 28, &[11, 17]),
            // 1000/1 of 28 -> 27.97 / 0.03 -> 28 / 0 -> one unit moves -> 27 / 1.
            ("minimum one unit taken from the largest share", &[1000, 1], 28, &[27, 1]),
            ("zero-weight item gets a unit when units suffice", &[0, 10], 4, &[1, 3]),
            // 5/3/1 of 2: 1.11 / 0.67 / 0.22 -> 1 / 0 / 0, spare to the largest remainder.
            ("no minimum when fewer units than items", &[5, 3, 1], 2, &[1, 1, 0]),
            ("all-zero weights split evenly, spare to the earliest", &[0, 0, 0], 7, &[3, 2, 2]),
            // 0.5 / 1.5 -> 0 / 1, equal remainders -> heavier -> 0 / 2, minimum -> 1 / 1.
            ("equal remainders to heavier, then minimum", &[1, 3], 2, &[1, 1]),
            // 1.5 / 4.5 -> 1 / 4, equal remainders -> heavier -> 1 / 5.
            ("equal remainders to heavier", &[1, 3], 6, &[1, 5]),
            ("no items", &[], 5, &[]),
            ("zero units", &[4, 2], 0, &[0, 0]),
            ("single item gets everything", &[3], 28, &[28]),
        ];
        for (case, w, u, expected) in cases {
            assert_eq!(plan::distribute(w, *u), expected.to_vec(), "case: {case}");
        }
    }

    #[test]
    fn sum_equals_units_and_minimum_holds() {
        let cases: &[(&[u64], u32)] = &[
            (&[1, 1, 1], 28),
            (&[7, 13, 29, 31], 28),
            (&[1000, 1, 1, 1], 5),
            (&[0, 0, 5, 0], 4),
            (&[3, 0, 0, 0, 0], 4),
            (&[u64::from(u32::MAX), 1], 28),
        ];
        for (w, u) in cases {
            let out = plan::distribute(w, *u);
            assert_eq!(out.len(), w.len(), "{w:?}");
            assert_eq!(out.iter().sum::<u32>(), *u, "{w:?} -> {out:?}");
            if *u as usize >= w.len() {
                assert!(out.iter().all(|&x| x >= 1), "{w:?} -> {out:?}");
            }
        }
    }
}

mod start_times {
    use super::*;
    use common::ndt;

    #[test]
    fn back_to_back_after_already_logged() {
        let d = day(2026, 11, 4);
        let at = |h, m| ndt(d, h, m, 0);
        assert_eq!(
            plan::start_times(d, chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap(), 45, &[90, 15, 0, 30]),
            vec![at(9, 45), at(11, 15), at(11, 30), at(11, 30)]
        );
    }

    #[test]
    fn crosses_midnight() {
        let d = day(2026, 11, 4);
        assert_eq!(
            plan::start_times(d, chrono::NaiveTime::from_hms_opt(23, 0, 0).unwrap(), 30, &[60, 10]),
            vec![ndt(d, 23, 30, 0), ndt(day(2026, 11, 5), 0, 30, 0)]
        );
    }

    #[test]
    fn empty_durations() {
        assert_eq!(plan::start_times(day(2026, 11, 4), chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap(), 0, &[]), vec![]);
    }
}

mod comment {
    use super::*;

    fn sugg(evidence: Vec<String>, commits: usize, repos: &[&str]) -> Suggestion {
        Suggestion {
            bucket: issue("ABC-1"),
            minutes: 60,
            raw_minutes: 30,
            repos: repos.iter().map(|r| r.to_string()).collect(),
            evidence,
            commits,
        }
    }

    fn lines(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("line {i}")).collect()
    }

    fn dashed(n: usize) -> String {
        (1..=n).map(|i| format!("- line {i}")).collect::<Vec<_>>().join("\n")
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn lists_only_commit_subjects_when_there_are_commits() {
        let s = sugg(strings(&["fix A", "fix B", "checkout feat/abc-1", "uncommitted a.rs"]), 2, &["api"]);
        assert_eq!(plan::comment(&s), "- fix A\n- fix B");
    }

    #[test]
    fn falls_back_to_checkouts_and_files_without_commits() {
        let s = sugg(strings(&["checkout feat/abc-1", "uncommitted a.rs"]), 0, &["api"]);
        assert_eq!(plan::comment(&s), "- checkout feat/abc-1\n- uncommitted a.rs");
    }

    #[test]
    fn fallback_from_allocate_output_without_commits() {
        let events = vec![
            ev("api", 0, EventKind::Checkout, "feat/abc-1", Some("ABC-1")),
            ev("api", 60, EventKind::Uncommitted, "src/a.rs", Some("ABC-1")),
        ];
        let s = plan::allocate(&events, P);
        assert_eq!(s[0].commits, 0);
        assert_eq!(plan::comment(&s[0]), "- checkout feat/abc-1\n- uncommitted src/a.rs");
    }

    #[test]
    fn exactly_ten_commits_without_more_line() {
        let mut ev = lines(10);
        ev.push("checkout x".into());
        assert_eq!(plan::comment(&sugg(ev, 10, &["api"])), dashed(10));
    }

    #[test]
    fn more_than_ten_commits_are_summarized_counting_commits_only() {
        let mut ev = lines(12);
        ev.extend(strings(&["checkout x", "uncommitted y", "uncommitted z"]));
        assert_eq!(plan::comment(&sugg(ev, 12, &["api"])), dashed(10) + "\n… and 2 more");
    }

    #[test]
    fn more_than_ten_fallback_items_are_summarized() {
        assert_eq!(plan::comment(&sugg(lines(13), 0, &["api"])), dashed(10) + "\n… and 3 more");
    }

    #[test]
    fn empty_evidence_names_the_repos() {
        assert_eq!(plan::comment(&sugg(vec![], 0, &["api", "web"])), "Work in api, web");
    }
}
