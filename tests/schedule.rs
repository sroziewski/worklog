//! Pure text generation only; `install` would register a real scheduled task and is never called.

use std::path::Path;

use chrono::NaiveTime;
use worklog::schedule;

fn time(h: u32, m: u32, s: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, s).unwrap()
}

fn trimmed_lines(s: &str) -> Vec<&str> {
    s.lines().map(str::trim).collect()
}

mod task_xml {
    use super::*;

    // The XML carries boilerplate the contract does not fix; each contracted element is asserted
    // as a whole line.
    #[test]
    fn weekly_monday_to_friday_trigger_at_the_given_time() {
        let xml = schedule::task_xml(Path::new(r"C:\Tools\worklog-popup.exe"), time(17, 30, 0));
        let lines = trimmed_lines(&xml);
        assert_eq!(xml.matches("<CalendarTrigger>").count(), 1);
        // 2020-01-06 is a Monday.
        assert!(lines.contains(&"<StartBoundary>2020-01-06T17:30:00</StartBoundary>"), "{xml}");
        assert!(lines.contains(&"<DaysOfWeek><Monday/><Tuesday/><Wednesday/><Thursday/><Friday/></DaysOfWeek>"), "{xml}");
        assert!(lines.contains(&"<WeeksInterval>1</WeeksInterval>"), "{xml}");
        assert!(!xml.contains("Saturday") && !xml.contains("Sunday"), "{xml}");
        assert!(lines.contains(&"<StartWhenAvailable>true</StartWhenAvailable>"), "{xml}");
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"), "{xml}");
        assert!(xml.contains("<Exec><Command>C:\\Tools\\worklog-popup.exe</Command></Exec>"), "{xml}");
    }

    #[test]
    fn time_keeps_minutes_and_seconds() {
        let xml = schedule::task_xml(Path::new(r"C:\w.exe"), time(7, 5, 9));
        assert!(trimmed_lines(&xml).contains(&"<StartBoundary>2020-01-06T07:05:09</StartBoundary>"), "{xml}");
    }

    #[test]
    fn exe_path_is_xml_escaped() {
        let xml = schedule::task_xml(Path::new(r#"C:\A&B\<w>"it's".exe"#), time(17, 0, 0));
        assert!(
            xml.contains(r"<Command>C:\A&amp;B\&lt;w&gt;&quot;it&apos;s&quot;.exe</Command>"),
            "{xml}"
        );
    }
}

mod systemd_units {
    use super::*;

    #[test]
    fn timer_runs_monday_to_friday_persistently() {
        let (_, timer) = schedule::systemd_units(Path::new("/opt/worklog/worklog-popup"), time(17, 30, 0));
        let lines = trimmed_lines(&timer);
        assert_eq!(lines.iter().filter(|l| l.starts_with("OnCalendar=")).count(), 1, "{timer}");
        assert!(lines.contains(&"OnCalendar=Mon..Fri *-*-* 17:30:00"), "{timer}");
        assert!(lines.contains(&"Persistent=true"), "{timer}");
        assert!(lines.contains(&"[Timer]"), "{timer}");
    }

    #[test]
    fn timer_seconds_are_always_zero() {
        let (_, timer) = schedule::systemd_units(Path::new("/opt/w"), time(7, 5, 9));
        assert!(trimmed_lines(&timer).contains(&"OnCalendar=Mon..Fri *-*-* 07:05:00"), "{timer}");
    }

    #[test]
    fn service_executes_the_quoted_exe() {
        let (service, _) = schedule::systemd_units(Path::new("/opt/my apps/worklog-popup"), time(17, 0, 0));
        assert!(trimmed_lines(&service).contains(&"ExecStart=\"/opt/my apps/worklog-popup\""), "{service}");
        assert!(trimmed_lines(&service).contains(&"Type=oneshot"), "{service}");
        assert!(trimmed_lines(&service).contains(&"PartOf=graphical-session.target"), "{service}");
    }

    #[test]
    fn service_escapes_percent_and_quote_in_exe() {
        let (service, _) = schedule::systemd_units(Path::new(r#"/opt/100% "w"/popup"#), time(17, 0, 0));
        let exec: Vec<&str> = trimmed_lines(&service).into_iter().filter(|l| l.starts_with("ExecStart=")).collect();
        assert_eq!(exec, vec![r#"ExecStart="/opt/100%% \"w\"/popup""#]);
    }
}
