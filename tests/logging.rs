//! `logging::first_line`: what every run logs first about where its log goes.

use std::path::Path;

use log::Level;
use worklog::logging;

#[test]
fn first_line_names_the_file_or_says_why_there_is_none() {
    let p = Path::new(r"C:\Users\someone\AppData\Roaming\worklog dir\zażółć.log");
    // (name, path, opened, expected level and text)
    type Row<'a> = (&'a str, Option<&'a Path>, bool, (Level, &'a str));
    let rows: [Row; 4] = [
        ("opened", Some(p), true, (Level::Info, r"log file: C:\Users\someone\AppData\Roaming\worklog dir\zażółć.log")),
        (
            "cannot be opened",
            Some(p),
            false,
            (Level::Warn, r"log file: C:\Users\someone\AppData\Roaming\worklog dir\zażółć.log cannot be opened; logging to the console only"),
        ),
        ("no path", None, false, (Level::Warn, "log file: none (no config directory); logging to the console only")),
        ("no path, opened ignored", None, true, (Level::Warn, "log file: none (no config directory); logging to the console only")),
    ];
    for (name, path, opened, (level, text)) in rows {
        assert_eq!(logging::first_line(path, opened), (level, text.to_string()), "{name}");
    }
}
