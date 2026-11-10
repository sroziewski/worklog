//! Jira client against a local tiny_http mock; no request ever leaves 127.0.0.1.

mod common;

use chrono::{DateTime, FixedOffset, Local, Offset, TimeZone};
use common::{MockJira, day, local_at, ndt, query_param};
use serde_json::json;
use worklog::jira::{self, Issue, Jira, NewIssue};

const TOKEN: &str = "tok-7Hq2-secret";

fn ok(body: serde_json::Value) -> (u16, String) {
    (200, body.to_string())
}

/// `+HHMM` offset of a local time, written out by hand.
fn offset_text(t: &DateTime<Local>) -> String {
    let secs = t.offset().fix().local_minus_utc();
    let sign = if secs < 0 { '-' } else { '+' };
    let a = secs.abs();
    format!("{sign}{:02}{:02}", a / 3600, (a % 3600) / 60)
}

mod pure {
    use super::*;

    #[test]
    fn format_started_has_millis_and_colonless_offset() {
        let t = jira::local(ndt(day(2026, 11, 4), 9, 5, 7));
        assert_eq!(jira::format_started(&t), format!("2026-11-04T09:05:07.000{}", offset_text(&t)));
    }

    #[test]
    fn worklog_body_has_started_seconds_and_comment() {
        let t = local_at(day(2026, 11, 4), 13, 45, 0);
        assert_eq!(
            jira::worklog_body(&t, 90, "- fix A\n- fix B"),
            json!({
                "started": format!("2026-11-04T13:45:00.000{}", offset_text(&t)),
                "timeSpentSeconds": 5400,
                "comment": "- fix A\n- fix B",
            })
        );
    }

    #[test]
    fn worklog_body_omits_empty_or_blank_comment() {
        let t = local_at(day(2026, 11, 5), 10, 15, 0);
        for comment in ["", " ", "\n\t  \r\n"] {
            assert_eq!(
                jira::worklog_body(&t, 30, comment),
                json!({"started": format!("2026-11-05T10:15:00.000{}", offset_text(&t)), "timeSpentSeconds": 1800}),
                "{comment:?}"
            );
        }
    }

    #[test]
    fn worklog_body_keeps_non_blank_comment_verbatim() {
        let t = local_at(day(2026, 11, 5), 10, 15, 0);
        assert_eq!(jira::worklog_body(&t, 30, " x \n")["comment"], json!(" x \n"));
    }

    #[test]
    fn project_of_is_the_part_before_the_number() {
        for (key, expected) in [("XYZ-12", "XYZ"), ("ABC-1", "ABC"), ("ABC", "ABC"), ("", "")] {
            assert_eq!(jira::project_of(key), expected, "{key:?}");
        }
    }

    #[test]
    fn url_encode_keeps_unreserved_and_escapes_the_rest() {
        let cases = [
            ("ABC-1439", "ABC-1439"),
            ("a.b_c~d-e", "a.b_c~d-e"),
            ("a b/c?d=e&f", "a%20b%2Fc%3Fd%3De%26f"),
            ("\"ABC\"", "%22ABC%22"),
            ("ż", "%C5%BC"),
            ("", ""),
        ];
        for (input, expected) in cases {
            assert_eq!(jira::url_encode(input), expected, "{input:?}");
        }
    }
}

mod requests {
    use super::*;

    #[test]
    fn myself_returns_name_and_sends_bearer_token() {
        let mock = MockJira::start(|_, _, _| ok(json!({"name": "jdoe", "displayName": "John Doe", "key": "JIRAUSER1"})));
        // Trailing slash on the base must not produce "//rest".
        let me = Jira::new(&format!("{}/", mock.base), TOKEN).myself().unwrap();
        assert_eq!(me, "jdoe");
        let r = mock.requests();
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].method.as_str(), r[0].url.as_str()), ("GET", "/rest/api/2/myself"));
        assert_eq!(r[0].authorization.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
    }

    #[test]
    fn myself_without_name_is_an_error() {
        let mock = MockJira::start(|_, _, _| ok(json!({"displayName": "John Doe"})));
        let e = Jira::new(&mock.base, TOKEN).myself().unwrap_err();
        assert_eq!(e.to_string(), "myself: no name in response");
    }

    #[test]
    fn issue_found_maps_key_summary_status() {
        let mock = MockJira::start(|_, _, _| {
            ok(json!({"key": "ABC-12", "fields": {"summary": "Login fails", "status": {"name": "In Progress"}}}))
        });
        let issue = Jira::new(&mock.base, TOKEN).issue("ABC-12").unwrap();
        assert_eq!(issue, Some(Issue { key: "ABC-12".into(), summary: "Login fails".into(), status: "In Progress".into() }));
        let r = mock.requests();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].url, "/rest/api/2/issue/ABC-12?fields=summary,status");
        assert_eq!(r[0].authorization.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
    }

    #[test]
    fn issue_404_is_none() {
        let mock = MockJira::start(|_, _, _| (404, json!({"errorMessages": ["Issue Does Not Exist"], "errors": {}}).to_string()));
        assert_eq!(Jira::new(&mock.base, TOKEN).issue("ABC-404").unwrap(), None);
    }

    #[test]
    fn issue_key_is_url_encoded_in_path() {
        let mock = MockJira::start(|_, _, _| (404, String::new()));
        Jira::new(&mock.base, TOKEN).issue("ABC 1/x").unwrap();
        assert_eq!(mock.requests()[0].url, "/rest/api/2/issue/ABC%201%2Fx?fields=summary,status");
    }

    #[test]
    fn issue_other_error_is_returned_with_jira_messages() {
        let mock = MockJira::start(|_, _, _| (500, json!({"errorMessages": ["boom"]}).to_string()));
        let e = Jira::new(&mock.base, TOKEN).issue("ABC-12").unwrap_err();
        assert_eq!(e.to_string(), "Jira 500 on /rest/api/2/issue/ABC-12?fields=summary,status: boom");
    }

    #[test]
    fn refused_token_names_jira_token_and_hides_the_token() {
        for status in [401u16, 403] {
            let mock = MockJira::start(move |_, _, _| (status, format!("{{\"message\":\"bad token {TOKEN}\"}}")));
            let e = Jira::new(&mock.base, TOKEN).myself().unwrap_err();
            let full = format!("{e:#}");
            assert_eq!(full, format!("Jira refused the token ({status}) on /rest/api/2/myself; check JIRA_TOKEN"));
            assert!(!full.contains(TOKEN) && !format!("{e:?}").contains(TOKEN), "{full}");
        }
    }

    #[test]
    fn refused_token_on_issue_is_an_error_not_none() {
        let mock = MockJira::start(|_, _, _| (401, String::new()));
        let e = Jira::new(&mock.base, TOKEN).issue("ABC-3").unwrap_err();
        assert_eq!(e.to_string(), "Jira refused the token (401) on /rest/api/2/issue/ABC-3?fields=summary,status; check JIRA_TOKEN");
    }

    #[test]
    fn error_messages_and_field_errors_are_joined() {
        let mock = MockJira::start(|_, _, _| {
            (
                400,
                json!({"errorMessages": ["first", "second"], "errors": {"comment": "too long", "started": "bad date"}}).to_string(),
            )
        });
        let e = Jira::new(&mock.base, TOKEN).myself().unwrap_err();
        assert_eq!(e.to_string(), "Jira 400 on /rest/api/2/myself: first; second; comment: too long; started: bad date");
    }

    #[test]
    fn non_json_error_body_is_quoted_up_to_200_chars() {
        let long = "x".repeat(250);
        let body = long.clone();
        let mock = MockJira::start(move |_, url, _| if url.contains("myself") { (503, body.clone()) } else { (502, "Bad Gateway".into()) });
        let jira = Jira::new(&mock.base, TOKEN);
        assert_eq!(jira.myself().unwrap_err().to_string(), format!("Jira 503 on /rest/api/2/myself: {}", &long[..200]));
        assert_eq!(jira.issue("ABC-1").unwrap_err().to_string(), "Jira 502 on /rest/api/2/issue/ABC-1?fields=summary,status: Bad Gateway");
    }

    #[test]
    fn success_body_that_is_not_json_is_an_error() {
        let mock = MockJira::start(|_, _, _| (200, "<html>login</html>".into()));
        let e = Jira::new(&mock.base, TOKEN).myself().unwrap_err();
        assert_eq!(e.to_string(), "/rest/api/2/myself: response is not JSON");
    }

    #[test]
    fn search_encodes_jql_and_maps_issues() {
        let mock = MockJira::start(|_, _, _| {
            ok(json!({"issues": [
                {"key": "ABC-1", "fields": {"summary": "One", "status": {"name": "Open"}}},
                {"key": "XYZ-2", "fields": {"summary": "Two", "status": {"name": "Done"}}}
            ]}))
        });
        let jql = r#"project = "ABC" AND text ~ "a&b" ORDER BY updated DESC"#;
        let issues = Jira::new(&mock.base, TOKEN).search(jql, 7).unwrap();
        assert_eq!(
            issues,
            vec![
                Issue { key: "ABC-1".into(), summary: "One".into(), status: "Open".into() },
                Issue { key: "XYZ-2".into(), summary: "Two".into(), status: "Done".into() },
            ]
        );
        let url = &mock.requests()[0].url;
        assert!(url.starts_with("/rest/api/2/search?jql="), "{url}");
        assert!(!url.contains(' ') && !url.contains('"'), "jql not encoded: {url}");
        assert_eq!(query_param(url, "jql").as_deref(), Some(jql));
        assert_eq!(query_param(url, "fields").as_deref(), Some("summary,status"));
        assert_eq!(query_param(url, "maxResults").as_deref(), Some("7"));
    }

    #[test]
    fn search_without_issues_is_empty() {
        let mock = MockJira::start(|_, _, _| ok(json!({"total": 0})));
        assert_eq!(Jira::new(&mock.base, TOKEN).search("x", 5).unwrap(), vec![]);
    }

    #[test]
    fn candidates_query_my_open_issues_in_projects() {
        let mock = MockJira::start(|_, _, _| ok(json!({"issues": []})));
        Jira::new(&mock.base, TOKEN).candidates(&["ABC".into(), "XYZ".into()], 30).unwrap();
        let url = &mock.requests()[0].url;
        assert_eq!(
            query_param(url, "jql").as_deref(),
            Some(r#"assignee = currentUser() AND resolution = Unresolved AND project in ("ABC", "XYZ") ORDER BY updated DESC"#)
        );
        assert_eq!(query_param(url, "maxResults").as_deref(), Some("30"));
    }
}

mod logged_on {
    use super::*;

    fn wl(author: &str, started: String, secs: i64) -> serde_json::Value {
        json!({"author": {"name": author, "displayName": author}, "started": started, "timeSpentSeconds": secs})
    }

    fn started(t: &DateTime<Local>) -> String {
        jira::format_started(t)
    }

    #[test]
    fn sums_my_worklogs_on_the_local_date_per_issue() {
        let d = day(2026, 11, 4);
        // 00:30 local on the date, written in UTC: the text may show the previous day, the local
        // date is still `d`.
        let utc_text = local_at(d, 0, 30, 0).with_timezone(&FixedOffset::east_opt(0).unwrap()).format(jira::STARTED_FORMAT).to_string();
        let worklogs_1 = json!({"worklogs": [
            wl("jdoe", started(&local_at(d, 9, 0, 0)), 3600),
            wl("colleague", started(&local_at(d, 10, 0, 0)), 1800),
            wl("jdoe", started(&local_at(day(2026, 11, 3), 23, 30, 0)), 900),
            wl("jdoe", started(&local_at(day(2026, 11, 5), 0, 0, 0)), 2700),
            wl("jdoe", utc_text, 1200),
            wl("jdoe", "2026-11-04 09:00".into(), 600),
        ]});
        let worklogs_2 = json!({"worklogs": [wl("colleague", started(&local_at(d, 11, 0, 0)), 7200)]});
        let worklogs_3 = json!({"worklogs": [wl("jdoe", started(&local_at(d, 23, 59, 59)), 90)]});
        let mock = MockJira::start(move |_, url, _| {
            let path = url.split('?').next().unwrap();
            match path {
                "/rest/api/2/search" => ok(json!({"issues": [
                    {"key": "ABC-1", "fields": {"summary": "a", "status": {"name": "Open"}}},
                    {"key": "ABC-2", "fields": {"summary": "b", "status": {"name": "Open"}}},
                    {"key": "XYZ-3", "fields": {"summary": "c", "status": {"name": "Open"}}}
                ]})),
                "/rest/api/2/issue/ABC-1/worklog" => ok(worklogs_1.clone()),
                "/rest/api/2/issue/ABC-2/worklog" => ok(worklogs_2.clone()),
                "/rest/api/2/issue/XYZ-3/worklog" => ok(worklogs_3.clone()),
                _ => (404, String::new()),
            }
        });
        let logged = Jira::new(&mock.base, TOKEN).logged_on("jdoe", d).unwrap();
        // ABC-1: 3600 + 1200 = 80 min; ABC-2: nothing of mine; XYZ-3: 90 s -> 1 min.
        assert_eq!(logged, vec![("ABC-1".to_string(), 80), ("XYZ-3".to_string(), 1)]);

        let r = mock.requests();
        assert_eq!(r.len(), 4);
        assert_eq!(
            query_param(&r[0].url, "jql").as_deref(),
            Some(r#"worklogAuthor = currentUser() AND worklogDate = "2026-11-04""#)
        );
        assert_eq!(query_param(&r[0].url, "maxResults").as_deref(), Some("100"));
        let paths: Vec<&str> = r[1..].iter().map(|x| x.url.split('?').next().unwrap()).collect();
        assert_eq!(paths, vec!["/rest/api/2/issue/ABC-1/worklog", "/rest/api/2/issue/ABC-2/worklog", "/rest/api/2/issue/XYZ-3/worklog"]);
        assert!(r.iter().all(|x| x.authorization.as_deref() == Some(&format!("Bearer {TOKEN}") as &str)));
    }

    #[test]
    fn nothing_logged_is_empty() {
        let mock = MockJira::start(|_, _, _| ok(json!({"issues": []})));
        assert_eq!(Jira::new(&mock.base, TOKEN).logged_on("jdoe", day(2026, 11, 4)).unwrap(), vec![]);
        assert_eq!(mock.requests().len(), 1);
    }

    #[test]
    fn worklog_fetch_error_fails_the_call() {
        let mock = MockJira::start(|_, url, _| {
            if url.starts_with("/rest/api/2/search") {
                ok(json!({"issues": [{"key": "ABC-1", "fields": {}}]}))
            } else {
                (500, json!({"errorMessages": ["db down"]}).to_string())
            }
        });
        let e = Jira::new(&mock.base, TOKEN).logged_on("jdoe", day(2026, 11, 4)).unwrap_err();
        assert_eq!(e.to_string(), "Jira 500 on /rest/api/2/issue/ABC-1/worklog?maxResults=5000: db down");
    }
}

mod logged_between {
    use super::*;
    use chrono::NaiveDate;

    fn wl(author: &str, started: String, secs: i64) -> serde_json::Value {
        json!({"author": {"name": author, "displayName": author}, "started": started, "timeSpentSeconds": secs})
    }

    fn at(d: u32, h: u32, m: u32, s: u32) -> String {
        jira::format_started(&local_at(day(2026, 11, d), h, m, s))
    }

    /// Search returns XYZ-7, ABC-2, ABC-10 (unsorted); worklogs around Mon 09 .. Sun 15 Nov 2026.
    fn mock() -> MockJira {
        // 00:30 local on Thu 12th, written in UTC so the text may show the 11th.
        let utc_text = local_at(day(2026, 11, 12), 0, 30, 0).with_timezone(&FixedOffset::east_opt(0).unwrap()).format(jira::STARTED_FORMAT).to_string();
        let abc2 = json!({"worklogs": [
            wl("jdoe", at(10, 10, 0, 0), 3600),
            wl("jdoe", at(9, 0, 0, 0), 1800),
            wl("colleague", at(11, 9, 0, 0), 7200),
            wl("jdoe", at(8, 23, 59, 59), 600),
            wl("jdoe", at(15, 23, 59, 59), 2700),
            wl("jdoe", at(16, 0, 0, 0), 900),
            wl("jdoe", "2026-11-10 10:00".into(), 1500),
            json!({"author": {"name": "jdoe"}, "started": 20261110, "timeSpentSeconds": 1500}),
            wl("jdoe", at(11, 8, 0, 0), 0),
            wl("jdoe", at(11, 8, 30, 0), 59),
            json!({"author": {"name": "jdoe"}, "started": at(11, 9, 0, 0)}),
            json!({"started": at(11, 10, 0, 0), "timeSpentSeconds": 3000}),
        ]});
        let abc10 = json!({"worklogs": [
            wl("jdoe", at(10, 15, 0, 0), 1500),
            wl("jdoe", at(10, 9, 0, 0), 7200),
            wl("jdoe", utc_text, 1200),
            wl("JDOE", at(13, 9, 0, 0), 3000),
        ]});
        let xyz = json!({"worklogs": [wl("jdoe", at(9, 12, 0, 0), 919)]});
        MockJira::start(move |_, url, _| match url.split('?').next().unwrap() {
            "/rest/api/2/search" => ok(json!({"issues": [
                {"key": "XYZ-7", "fields": {"summary": "a", "status": {"name": "Open"}}},
                {"key": "ABC-2", "fields": {"summary": "b", "status": {"name": "Open"}}},
                {"key": "ABC-10", "fields": {"summary": "c", "status": {"name": "Done"}}}
            ]})),
            "/rest/api/2/issue/ABC-2/worklog" => ok(abc2.clone()),
            "/rest/api/2/issue/ABC-10/worklog" => ok(abc10.clone()),
            "/rest/api/2/issue/XYZ-7/worklog" => ok(xyz.clone()),
            _ => (404, String::new()),
        })
    }

    fn entry(d: u32, key: &str, minutes: u32) -> (NaiveDate, String, u32) {
        (day(2026, 11, d), key.to_owned(), minutes)
    }

    #[test]
    fn my_worklogs_in_range_one_entry_each_sorted_by_date_key_minutes() {
        let mock = mock();
        let got = Jira::new(&mock.base, TOKEN).logged_between("jdoe", day(2026, 11, 9), day(2026, 11, 15)).unwrap();
        // Dropped: colleague, Sun 8th and Mon 16th (outside), two malformed `started`, 0 s and 59 s
        // (0 min), no timeSpentSeconds, no author, "JDOE" (other name). 919 s -> 15 min.
        assert_eq!(
            got,
            vec![
                entry(9, "ABC-2", 30),
                entry(9, "XYZ-7", 15),
                entry(10, "ABC-10", 25),
                entry(10, "ABC-10", 120),
                entry(10, "ABC-2", 60),
                entry(12, "ABC-10", 20),
                entry(15, "ABC-2", 45),
            ]
        );

        let r = mock.requests();
        assert_eq!(r.len(), 4);
        assert_eq!(r[0].method, "GET");
        assert_eq!(
            query_param(&r[0].url, "jql").as_deref(),
            Some(r#"worklogAuthor = currentUser() AND worklogDate >= "2026-11-09" AND worklogDate <= "2026-11-15""#)
        );
        assert_eq!(query_param(&r[0].url, "maxResults").as_deref(), Some("200"));
        let urls: Vec<&str> = r[1..].iter().map(|x| x.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "/rest/api/2/issue/XYZ-7/worklog?maxResults=5000",
                "/rest/api/2/issue/ABC-2/worklog?maxResults=5000",
                "/rest/api/2/issue/ABC-10/worklog?maxResults=5000",
            ]
        );
        assert!(r.iter().all(|x| x.method == "GET" && x.authorization.as_deref() == Some(&format!("Bearer {TOKEN}") as &str)));
    }

    #[test]
    fn single_day_range_keeps_only_that_day() {
        let mock = mock();
        let got = Jira::new(&mock.base, TOKEN).logged_between("jdoe", day(2026, 11, 10), day(2026, 11, 10)).unwrap();
        assert_eq!(got, vec![entry(10, "ABC-10", 25), entry(10, "ABC-10", 120), entry(10, "ABC-2", 60)]);
        assert_eq!(
            query_param(&mock.requests()[0].url, "jql").as_deref(),
            Some(r#"worklogAuthor = currentUser() AND worklogDate >= "2026-11-10" AND worklogDate <= "2026-11-10""#)
        );
    }

    #[test]
    fn inverted_range_matches_no_worklog() {
        let mock = mock();
        let got = Jira::new(&mock.base, TOKEN).logged_between("jdoe", day(2026, 11, 15), day(2026, 11, 9)).unwrap();
        assert_eq!(got, vec![]);
    }

    #[test]
    fn other_author_gets_only_their_worklogs() {
        let mock = mock();
        let got = Jira::new(&mock.base, TOKEN).logged_between("colleague", day(2026, 11, 9), day(2026, 11, 15)).unwrap();
        assert_eq!(got, vec![entry(11, "ABC-2", 120)]);
    }

    #[test]
    fn nothing_logged_is_empty_after_one_search() {
        let mock = MockJira::start(|_, _, _| ok(json!({"issues": []})));
        assert_eq!(Jira::new(&mock.base, TOKEN).logged_between("jdoe", day(2026, 11, 9), day(2026, 11, 15)).unwrap(), vec![]);
        assert_eq!(mock.requests().len(), 1);
    }

    #[test]
    fn search_error_fails_the_call() {
        let mock = MockJira::start(|_, _, _| (500, json!({"errorMessages": ["search down"]}).to_string()));
        let e = Jira::new(&mock.base, TOKEN).logged_between("jdoe", day(2026, 11, 9), day(2026, 11, 15)).unwrap_err();
        let jql = r#"worklogAuthor = currentUser() AND worklogDate >= "2026-11-09" AND worklogDate <= "2026-11-15""#;
        assert_eq!(
            e.to_string(),
            format!("Jira 500 on /rest/api/2/search?jql={}&fields=summary,status&maxResults=200: search down", jira::url_encode(jql))
        );
    }

    #[test]
    fn worklog_fetch_error_fails_the_call() {
        let mock = MockJira::start(|_, url, _| {
            if url.starts_with("/rest/api/2/search") {
                ok(json!({"issues": [{"key": "ABC-4", "fields": {}}]}))
            } else {
                (500, json!({"errorMessages": ["db down"]}).to_string())
            }
        });
        let e = Jira::new(&mock.base, TOKEN).logged_between("jdoe", day(2026, 11, 9), day(2026, 11, 15)).unwrap_err();
        assert_eq!(e.to_string(), "Jira 500 on /rest/api/2/issue/ABC-4/worklog?maxResults=5000: db down");
    }
}

mod add_worklog {
    use super::*;

    #[test]
    fn posts_started_seconds_and_comment_and_returns_string_id() {
        let mock = MockJira::start(|_, _, _| (201, json!({"id": "10101", "timeSpentSeconds": 2700}).to_string()));
        let t = local_at(day(2026, 11, 4), 9, 45, 0);
        let id = Jira::new(&mock.base, TOKEN).add_worklog("ABC-77", &t, 45, "- fix \"A\"\n- ż").unwrap();
        assert_eq!(id, "10101");
        let r = mock.requests();
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].method.as_str(), r[0].url.as_str()), ("POST", "/rest/api/2/issue/ABC-77/worklog"));
        assert_eq!(r[0].authorization.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
        assert!(r[0].content_type.as_deref().is_some_and(|c| c.starts_with("application/json")), "{:?}", r[0].content_type);
        let body: serde_json::Value = serde_json::from_str(&r[0].body).unwrap();
        assert_eq!(
            body,
            json!({"started": format!("2026-11-04T09:45:00.000{}", offset_text(&t)), "timeSpentSeconds": 2700, "comment": "- fix \"A\"\n- ż"})
        );
    }

    #[test]
    fn numeric_id_is_returned_as_text() {
        let mock = MockJira::start(|_, _, _| (201, json!({"id": 10102}).to_string()));
        let id = Jira::new(&mock.base, TOKEN).add_worklog("ABC-77", &local_at(day(2026, 11, 4), 9, 0, 0), 15, "x").unwrap();
        assert_eq!(id, "10102");
    }

    #[test]
    fn key_is_url_encoded_in_path() {
        let mock = MockJira::start(|_, _, _| (201, json!({"id": "1"}).to_string()));
        Jira::new(&mock.base, TOKEN).add_worklog("ABC 1/x", &local_at(day(2026, 11, 4), 9, 0, 0), 15, "x").unwrap();
        assert_eq!(mock.requests()[0].url, "/rest/api/2/issue/ABC%201%2Fx/worklog");
    }

    #[test]
    fn zero_minutes_is_refused_without_a_request() {
        let mock = MockJira::start(|_, _, _| (201, json!({"id": "1"}).to_string()));
        let e = Jira::new(&mock.base, TOKEN).add_worklog("ABC-5", &local_at(day(2026, 11, 4), 9, 0, 0), 0, "x").unwrap_err();
        assert_eq!(e.to_string(), "ABC-5: refusing to log 0 minutes");
        assert_eq!(mock.requests().len(), 0);
    }

    #[test]
    fn rejected_worklog_reports_field_errors() {
        let mock = MockJira::start(|_, _, _| (400, json!({"errorMessages": [], "errors": {"timeLogged": "You must indicate the time spent working."}}).to_string()));
        let e = Jira::new(&mock.base, TOKEN).add_worklog("ABC-5", &local_at(day(2026, 11, 4), 9, 0, 0), 15, "x").unwrap_err();
        assert_eq!(e.to_string(), "Jira 400 on /rest/api/2/issue/ABC-5/worklog: timeLogged: You must indicate the time spent working.");
    }
}

fn fields(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    v.as_object().expect("a JSON object").clone()
}

fn new_issue() -> NewIssue {
    NewIssue {
        project: "XYZ".into(),
        issue_type: "Task".into(),
        summary: "Popraw logowanie \"SSO\"".into(),
        description: "Naprawiono SSO.\n- api\n- web".into(),
        assignee: Some("jdoe".into()),
        extra_fields: fields(json!({"customfield_10000": {"value": "development"}, "labels": ["worklog", "ż"]})),
    }
}

mod create_body {
    use super::*;

    #[test]
    fn puts_named_and_extra_fields_under_fields() {
        assert_eq!(
            jira::create_body(&new_issue()),
            json!({"fields": {
                "customfield_10000": {"value": "development"},
                "labels": ["worklog", "ż"],
                "project": {"key": "XYZ"},
                "issuetype": {"name": "Task"},
                "summary": "Popraw logowanie \"SSO\"",
                "description": "Naprawiono SSO.\n- api\n- web",
                "assignee": {"name": "jdoe"}
            }})
        );
    }

    #[test]
    fn named_fields_override_same_named_extra_fields() {
        let n = NewIssue {
            extra_fields: fields(json!({
                "project": {"key": "EVIL"},
                "issuetype": {"name": "Epic"},
                "summary": "from extras",
                "description": "from extras",
                "assignee": {"name": "someone-else"},
                "priority": {"name": "Minor"}
            })),
            ..new_issue()
        };
        assert_eq!(
            jira::create_body(&n),
            json!({"fields": {
                "priority": {"name": "Minor"},
                "project": {"key": "XYZ"},
                "issuetype": {"name": "Task"},
                "summary": "Popraw logowanie \"SSO\"",
                "description": "Naprawiono SSO.\n- api\n- web",
                "assignee": {"name": "jdoe"}
            }})
        );
    }

    #[test]
    fn blank_description_and_no_assignee_are_omitted() {
        for description in ["", "  ", "\n\t"] {
            let n = NewIssue { description: description.into(), assignee: None, extra_fields: serde_json::Map::new(), ..new_issue() };
            assert_eq!(
                jira::create_body(&n),
                json!({"fields": {"project": {"key": "XYZ"}, "issuetype": {"name": "Task"}, "summary": "Popraw logowanie \"SSO\""}}),
                "{description:?}"
            );
        }
    }

    #[test]
    fn description_and_assignee_are_independent() {
        let n = NewIssue { assignee: None, extra_fields: serde_json::Map::new(), ..new_issue() };
        let f = &jira::create_body(&n)["fields"];
        assert_eq!((f.get("description"), f.get("assignee")), (Some(&json!("Naprawiono SSO.\n- api\n- web")), None));
        let n = NewIssue { description: " ".into(), extra_fields: serde_json::Map::new(), ..new_issue() };
        let f = &jira::create_body(&n)["fields"];
        assert_eq!((f.get("description"), f.get("assignee")), (None, Some(&json!({"name": "jdoe"}))));
    }
}

mod create_issue {
    use super::*;

    #[test]
    fn posts_create_body_and_returns_the_key() {
        let mock = MockJira::start(|_, _, _| {
            (201, json!({"id": "10442", "key": "XYZ-77", "self": "http://127.0.0.1/rest/api/2/issue/10442"}).to_string())
        });
        let key = Jira::new(&mock.base, TOKEN).create_issue(&new_issue()).unwrap();
        assert_eq!(key, "XYZ-77");
        let r = mock.requests();
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].method.as_str(), r[0].url.as_str()), ("POST", "/rest/api/2/issue"));
        assert_eq!(r[0].authorization.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
        assert!(r[0].content_type.as_deref().is_some_and(|c| c.starts_with("application/json")), "{:?}", r[0].content_type);
        let body: serde_json::Value = serde_json::from_str(&r[0].body).unwrap();
        assert_eq!(
            body,
            json!({"fields": {
                "customfield_10000": {"value": "development"},
                "labels": ["worklog", "ż"],
                "project": {"key": "XYZ"},
                "issuetype": {"name": "Task"},
                "summary": "Popraw logowanie \"SSO\"",
                "description": "Naprawiono SSO.\n- api\n- web",
                "assignee": {"name": "jdoe"}
            }})
        );
    }

    #[test]
    fn response_without_usable_key_is_an_error() {
        for resp in [json!({"id": "10442"}), json!({"id": "10442", "key": ""}), json!({"key": 77}), json!({"key": null})] {
            let body = resp.to_string();
            let mock = MockJira::start(move |_, _, _| (201, body.clone()));
            let e = Jira::new(&mock.base, TOKEN).create_issue(&new_issue()).unwrap_err();
            assert_eq!(e.to_string(), "Jira created the issue but returned no key", "{resp}");
        }
    }

    #[test]
    fn rejected_issue_surfaces_jira_field_errors() {
        let mock = MockJira::start(|_, _, _| {
            (400, json!({"errorMessages": [], "errors": {"customfield_10000": "Element is required"}}).to_string())
        });
        let e = Jira::new(&mock.base, TOKEN).create_issue(&new_issue()).unwrap_err();
        assert_eq!(e.to_string(), "Jira 400 on /rest/api/2/issue: customfield_10000: Element is required");
    }

    #[test]
    fn several_field_errors_all_surface() {
        let mock = MockJira::start(|_, _, _| {
            (
                400,
                json!({"errorMessages": ["Bad request"], "errors": {"summary": "Summary is too long", "customfield_10000": "Element is required"}})
                    .to_string(),
            )
        });
        let e = Jira::new(&mock.base, TOKEN).create_issue(&new_issue()).unwrap_err();
        let text = e.to_string();
        let detail = text.strip_prefix("Jira 400 on /rest/api/2/issue: ").unwrap_or_else(|| panic!("{text}"));
        // errorMessages first; the order of the `errors` map is Jira's/serde's, so compared as a set.
        let parts: Vec<&str> = detail.split("; ").collect();
        assert_eq!(parts[0], "Bad request", "{text}");
        let mut field_errors = parts[1..].to_vec();
        field_errors.sort();
        assert_eq!(field_errors, vec!["customfield_10000: Element is required", "summary: Summary is too long"], "{text}");
    }

    #[test]
    fn refused_token_is_reported() {
        let mock = MockJira::start(|_, _, _| (401, String::new()));
        let e = Jira::new(&mock.base, TOKEN).create_issue(&new_issue()).unwrap_err();
        assert_eq!(e.to_string(), "Jira refused the token (401) on /rest/api/2/issue; check JIRA_TOKEN");
    }
}

mod project_name {
    use super::*;

    #[test]
    fn gets_project_and_returns_its_name() {
        let mock = MockJira::start(|_, _, _| ok(json!({"key": "ABC", "id": "10100", "name": "Alpha"})));
        assert_eq!(Jira::new(&mock.base, TOKEN).project_name("ABC").unwrap(), "Alpha");
        let r = mock.requests();
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].method.as_str(), r[0].url.as_str()), ("GET", "/rest/api/2/project/ABC"));
        assert_eq!(r[0].authorization.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
    }

    #[test]
    fn missing_name_is_an_error_naming_the_project() {
        let mock = MockJira::start(|_, _, _| ok(json!({"key": "XYZ", "id": "10200"})));
        let e = Jira::new(&mock.base, TOKEN).project_name("XYZ").unwrap_err();
        assert_eq!(e.to_string(), "project XYZ: no name in response");
    }

    #[test]
    fn unknown_project_is_an_error() {
        let mock = MockJira::start(|_, _, _| (404, json!({"errorMessages": ["No project could be found with key 'XX'."], "errors": {}}).to_string()));
        let e = Jira::new(&mock.base, TOKEN).project_name("XX").unwrap_err();
        assert_eq!(e.to_string(), "Jira 404 on /rest/api/2/project/XX: No project could be found with key 'XX'.");
    }
}

mod update_body {
    use super::*;

    #[test]
    fn only_the_given_fields_blank_description_is_null() {
        // (name, summary, description, expected body)
        type Row = (&'static str, Option<&'static str>, Option<&'static str>, Option<serde_json::Value>);
        let rows: [Row; 8] = [
            ("nothing to change", None, None, None),
            ("summary only", Some("Eksport \"CSV\" ż"), None, Some(json!({"fields": {"summary": "Eksport \"CSV\" ż"}}))),
            ("description only", None, Some("h3. Opis\n* a"), Some(json!({"fields": {"description": "h3. Opis\n* a"}}))),
            ("both", Some("T2"), Some("D2"), Some(json!({"fields": {"summary": "T2", "description": "D2"}}))),
            ("empty description clears it", None, Some(""), Some(json!({"fields": {"description": null}}))),
            ("blank description clears it", None, Some(" \n\t"), Some(json!({"fields": {"description": null}}))),
            ("summary with a cleared description", Some("T3"), Some(""), Some(json!({"fields": {"summary": "T3", "description": null}}))),
            ("non-blank description sent verbatim", None, Some(" D4\n"), Some(json!({"fields": {"description": " D4\n"}}))),
        ];
        for (name, summary, description, expected) in rows {
            assert_eq!(jira::update_body(summary, description), expected, "{name}");
        }
    }
}

mod update_issue {
    use super::*;

    #[test]
    fn puts_the_changed_fields_to_the_issue() {
        let mock = MockJira::start(|_, _, _| (204, String::new()));
        Jira::new(&mock.base, TOKEN).update_issue("XYZ-77", Some("Nowy tytuł"), Some("h3. Opis\nNowy opis.")).unwrap();
        let r = mock.requests();
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].method.as_str(), r[0].url.as_str()), ("PUT", "/rest/api/2/issue/XYZ-77"));
        assert_eq!(r[0].authorization.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
        assert!(r[0].content_type.as_deref().is_some_and(|c| c.starts_with("application/json")), "{:?}", r[0].content_type);
        let body: serde_json::Value = serde_json::from_str(&r[0].body).unwrap();
        assert_eq!(body, json!({"fields": {"summary": "Nowy tytuł", "description": "h3. Opis\nNowy opis."}}));
    }

    #[test]
    fn body_per_changed_field() {
        // (name, summary, description, expected body)
        type Row = (&'static str, Option<&'static str>, Option<&'static str>, serde_json::Value);
        let rows: [Row; 3] = [
            ("title only", Some("Only title"), None, json!({"fields": {"summary": "Only title"}})),
            ("description only", None, Some("Only body"), json!({"fields": {"description": "Only body"}})),
            ("description cleared", None, Some("  "), json!({"fields": {"description": null}})),
        ];
        for (name, summary, description, expected) in rows {
            let mock = MockJira::start(|_, _, _| (204, String::new()));
            Jira::new(&mock.base, TOKEN).update_issue("ABC-12", summary, description).unwrap();
            let r = mock.requests();
            assert_eq!(r.len(), 1, "{name}");
            assert_eq!((r[0].method.as_str(), r[0].url.as_str()), ("PUT", "/rest/api/2/issue/ABC-12"), "{name}");
            assert_eq!(serde_json::from_str::<serde_json::Value>(&r[0].body).unwrap(), expected, "{name}");
        }
    }

    #[test]
    fn nothing_changed_sends_no_request() {
        let mock = MockJira::start(|_, _, _| (204, String::new()));
        Jira::new(&mock.base, TOKEN).update_issue("ABC-12", None, None).unwrap();
        assert_eq!(mock.requests().len(), 0);
    }

    #[test]
    fn key_is_url_encoded_in_path() {
        let mock = MockJira::start(|_, _, _| (204, String::new()));
        Jira::new(&mock.base, TOKEN).update_issue("ABC 1/x", Some("t"), None).unwrap();
        assert_eq!(mock.requests()[0].url, "/rest/api/2/issue/ABC%201%2Fx");
    }

    #[test]
    fn rejected_update_surfaces_jira_field_errors() {
        let mock = MockJira::start(|_, _, _| (400, json!({"errorMessages": [], "errors": {"summary": "Field 'summary' cannot be set."}}).to_string()));
        let e = Jira::new(&mock.base, TOKEN).update_issue("ABC-12", Some("t"), None).unwrap_err();
        assert_eq!(e.to_string(), "Jira 400 on /rest/api/2/issue/ABC-12: summary: Field 'summary' cannot be set.");
    }

    #[test]
    fn refused_token_is_reported() {
        let mock = MockJira::start(|_, _, _| (403, String::new()));
        let e = Jira::new(&mock.base, TOKEN).update_issue("ABC-12", None, Some("d")).unwrap_err();
        assert_eq!(e.to_string(), "Jira refused the token (403) on /rest/api/2/issue/ABC-12; check JIRA_TOKEN");
    }
}

mod issue_details {
    use super::*;
    use worklog::jira::IssueDetails;

    fn full() -> serde_json::Value {
        json!({"id": "10442", "key": "XYZ-77", "fields": {
            "summary": "Eksport CSV",
            "description": "h3. Opis\nAudytorzy.",
            "status": {"name": "In Review", "id": "3"},
            "updated": "2026-11-09T14:03:12.000+0100"
        }})
    }

    fn xyz77() -> IssueDetails {
        IssueDetails {
            key: "XYZ-77".into(),
            summary: "Eksport CSV".into(),
            description: "h3. Opis\nAudytorzy.".into(),
            status: "In Review".into(),
            updated: "2026-11-09T14:03:12.000+0100".into(),
        }
    }

    #[test]
    fn parse_details_maps_every_field() {
        assert_eq!(jira::parse_details(&full()), xyz77());
    }

    #[test]
    fn parse_details_missing_or_non_text_fields_are_empty() {
        // (name, response, expected)
        let rows: Vec<(&str, serde_json::Value, IssueDetails)> = vec![
            ("description null", json!({"key": "ABC-1", "fields": {"summary": "S", "description": null, "status": {"name": "Open"}, "updated": "U"}}),
                IssueDetails { key: "ABC-1".into(), summary: "S".into(), description: String::new(), status: "Open".into(), updated: "U".into() }),
            ("description missing", json!({"key": "ABC-2", "fields": {"summary": "S2", "status": {"name": "Done"}, "updated": "U2"}}),
                IssueDetails { key: "ABC-2".into(), summary: "S2".into(), description: String::new(), status: "Done".into(), updated: "U2".into() }),
            ("status without name", json!({"key": "ABC-3", "fields": {"summary": "S3", "description": "D3", "status": {}, "updated": "U3"}}),
                IssueDetails { key: "ABC-3".into(), summary: "S3".into(), description: "D3".into(), status: String::new(), updated: "U3".into() }),
            ("no fields", json!({"key": "ABC-4"}), IssueDetails { key: "ABC-4".into(), ..IssueDetails::default() }),
            ("empty object", json!({}), IssueDetails::default()),
            ("not an object", json!([1, 2]), IssueDetails::default()),
        ];
        for (name, v, expected) in rows {
            assert_eq!(jira::parse_details(&v), expected, "{name}");
        }
    }

    #[test]
    fn gets_the_four_fields_and_parses_them() {
        let body = full();
        let mock = MockJira::start(move |_, _, _| ok(body.clone()));
        assert_eq!(Jira::new(&mock.base, TOKEN).issue_details("XYZ-77").unwrap(), xyz77());
        let r = mock.requests();
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].method.as_str(), r[0].url.as_str()), ("GET", "/rest/api/2/issue/XYZ-77?fields=summary,description,status,updated"));
        assert_eq!(r[0].authorization.as_deref(), Some(format!("Bearer {TOKEN}").as_str()));
    }

    #[test]
    fn key_is_url_encoded_in_path() {
        let mock = MockJira::start(|_, _, _| ok(json!({"key": "x"})));
        Jira::new(&mock.base, TOKEN).issue_details("ABC 1/x").unwrap();
        assert_eq!(mock.requests()[0].url, "/rest/api/2/issue/ABC%201%2Fx?fields=summary,description,status,updated");
    }

    #[test]
    fn missing_issue_is_an_error() {
        let mock = MockJira::start(|_, _, _| (404, json!({"errorMessages": ["Issue Does Not Exist"], "errors": {}}).to_string()));
        let e = Jira::new(&mock.base, TOKEN).issue_details("ABC-404").unwrap_err();
        assert_eq!(e.to_string(), "Jira 404 on /rest/api/2/issue/ABC-404?fields=summary,description,status,updated: Issue Does Not Exist");
    }
}

#[test]
fn local_resolves_wall_time_in_local_zone() {
    let n = ndt(day(2026, 11, 4), 12, 0, 0);
    assert_eq!(jira::local(n), Local.from_local_datetime(&n).single().unwrap());
}
