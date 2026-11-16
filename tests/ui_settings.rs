//! `ui_settings`: the window's own settings in `ui.toml` - reading, writing it in a temp dir, the
//! window level they map to. `default_path` reads the environment, so it is tested in
//! `tests/env_paths.rs` (one test per binary).

use std::path::Path;

use eframe::egui::WindowLevel;
use worklog::ui_settings::{self, UiSettings, WindowSection};

const ON: UiSettings = UiSettings { always_on_top: true };
const OFF: UiSettings = UiSettings { always_on_top: false };

fn chain(e: &anyhow::Error) -> Vec<String> {
    e.chain().map(|c| c.to_string()).collect()
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    names
}

mod defaults {
    use super::*;

    #[test]
    fn always_on_top_unless_chosen_otherwise() {
        assert_eq!(UiSettings::default(), ON);
    }

    #[test]
    fn the_window_section_starts_with_the_defaults_and_no_error() {
        let section = WindowSection::default();
        assert_eq!(section.settings, ON);
        assert_eq!(section.error, None);
    }

    #[test]
    fn window_level_follows_always_on_top() {
        // (always_on_top, expected)
        let rows = [(true, WindowLevel::AlwaysOnTop), (false, WindowLevel::Normal)];
        for (on, expected) in rows {
            assert_eq!(ui_settings::window_level(on), expected, "always_on_top = {on}");
        }
    }
}

mod load {
    use super::*;

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        // (case, path)
        let rows = [
            ("no file", tmp.path().join("ui.toml")),
            ("no folder either", tmp.path().join("not created").join("ui.toml")),
        ];
        for (case, path) in rows {
            assert_eq!(ui_settings::load(&path).unwrap(), ON, "{case}");
            assert!(!path.exists(), "{case}: load must not create the file");
        }
    }

    #[test]
    fn reads_the_file_and_a_missing_key_means_on() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ui.toml");
        // (case, content, expected)
        let rows = [
            ("off", "always_on_top = false\n", OFF),
            ("on", "always_on_top = true\n", ON),
            ("off with comments and spacing", "# chosen in the Settings window\n  always_on_top   =   false   # really\n", OFF),
            ("empty file", "", ON),
            ("only a comment", "# nothing chosen yet\n", ON),
        ];
        for (case, content, expected) in rows {
            std::fs::write(&path, content).unwrap();
            assert_eq!(ui_settings::load(&path).unwrap(), expected, "{case}");
        }
    }

    #[test]
    fn a_file_that_is_not_window_settings_is_a_parse_error_naming_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ui.toml");
        // (case, content, a part of toml's message, which the contract does not word)
        let rows = [
            ("unknown key", "colour = \"blue\"\n", "unknown field `colour`"),
            ("unknown key next to a valid one", "always_on_top = false\nopacity = 7\n", "unknown field `opacity`"),
            ("key in another case", "Always_On_Top = false\n", "unknown field `Always_On_Top`"),
            ("a string, not a bool", "always_on_top = \"no\"\n", "invalid type"),
            ("a number, not a bool", "always_on_top = 0\n", "invalid type"),
            ("not toml", "always_on_top = \n", "string values must be quoted"),
            ("unclosed table", "[window\nalways_on_top = false\n", "unclosed table"),
        ];
        for (case, content, part) in rows {
            std::fs::write(&path, content).unwrap();
            let c = chain(&ui_settings::load(&path).unwrap_err());
            assert_eq!(c[0], format!("parsing {}", path.display()), "{case}");
            assert!(c.len() == 2 && c[1].contains(part), "{case}: {c:?}");
        }
    }

    #[test]
    fn a_path_that_cannot_be_read_is_a_read_error_naming_it() {
        let tmp = tempfile::tempdir().unwrap();
        // A folder where the file should be: it exists, but reading it as text fails.
        let path = tmp.path().join("ui.toml");
        std::fs::create_dir(&path).unwrap();
        let c = chain(&ui_settings::load(&path).unwrap_err());
        // The rest of the chain is the OS's (and its language's) reason.
        assert_eq!(c[0], format!("reading {}", path.display()));
        assert_eq!(c.len(), 2, "{c:?}");
    }
}

mod load_or_default {
    use super::*;

    #[test]
    fn no_location_gives_the_defaults() {
        assert_eq!(ui_settings::load_or_default(None), ON);
    }

    #[test]
    fn a_valid_file_is_read() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ui.toml");
        std::fs::write(&path, "always_on_top = false\n").unwrap();
        assert_eq!(ui_settings::load_or_default(Some(&path)), OFF);
    }

    #[test]
    fn a_missing_or_broken_file_gives_the_defaults_instead_of_failing() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing.toml");
        // Every broken file also says `false`, so a partial read would show as OFF.
        let unknown_key = tmp.path().join("unknown-key.toml");
        std::fs::write(&unknown_key, "always_on_top = false\nopacity = 7\n").unwrap();
        let not_toml = tmp.path().join("not-toml.toml");
        std::fs::write(&not_toml, "always_on_top = false\n[[[\n").unwrap();
        let folder = tmp.path().join("folder.toml");
        std::fs::create_dir(&folder).unwrap();
        // (case, path)
        let rows = [("missing", missing), ("unknown key", unknown_key), ("not toml", not_toml), ("unreadable", folder)];
        for (case, path) in rows {
            assert_eq!(ui_settings::load_or_default(Some(&path)), ON, "{case}");
        }
    }
}

mod save {
    use super::*;

    #[test]
    fn load_reads_back_either_value_and_no_temp_file_is_left() {
        for s in [OFF, ON] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("ui.toml");
            ui_settings::save(&path, &s).unwrap();
            assert_eq!(ui_settings::load(&path).unwrap(), s);
            assert_eq!(files_in(tmp.path()), vec!["ui.toml".to_string()], "{s:?}");
        }
    }

    #[test]
    fn replaces_an_earlier_value() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ui.toml");
        ui_settings::save(&path, &OFF).unwrap();
        ui_settings::save(&path, &ON).unwrap();
        assert_eq!(ui_settings::load(&path).unwrap(), ON);
        ui_settings::save(&path, &OFF).unwrap();
        assert_eq!(ui_settings::load(&path).unwrap(), OFF);
        assert_eq!(files_in(tmp.path()), vec!["ui.toml".to_string()]);
    }

    #[test]
    fn creates_the_missing_parent_folders() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg").join("worklog").join("ui.toml");
        ui_settings::save(&path, &OFF).unwrap();
        assert_eq!(ui_settings::load(&path).unwrap(), OFF);
        assert_eq!(files_in(path.parent().unwrap()), vec!["ui.toml".to_string()]);
    }

    #[test]
    fn never_creates_or_touches_the_projects_list_next_to_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ui.toml");
        ui_settings::save(&path, &OFF).unwrap();
        assert!(!tmp.path().join("projects.toml").exists());
        assert_eq!(files_in(tmp.path()), vec!["ui.toml".to_string()]);

        let projects = tmp.path().join("projects.toml");
        let list = "[[project]]\nname = \"Alpha\"\njira_key = \"ABC\"\nenabled = false\n";
        std::fs::write(&projects, list).unwrap();
        ui_settings::save(&path, &ON).unwrap();
        assert_eq!(std::fs::read_to_string(&projects).unwrap(), list);
        assert_eq!(files_in(tmp.path()), vec!["projects.toml".to_string(), "ui.toml".to_string()]);
    }

    #[test]
    fn a_failed_replace_removes_the_temp_file_and_names_the_target() {
        let tmp = tempfile::tempdir().unwrap();
        // A folder where the file should go: the temp file is written, the rename over it fails.
        let path = tmp.path().join("ui.toml");
        std::fs::create_dir(&path).unwrap();
        let c = chain(&ui_settings::save(&path, &OFF).unwrap_err());
        // The rest of the chain is the OS's (and its language's) reason.
        assert_eq!(c[0], format!("replacing {}", path.display()));
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!(files_in(tmp.path()), vec!["ui.toml".to_string()]);
        assert!(path.is_dir());
        assert_eq!(files_in(&path), Vec::<String>::new());
    }

    #[test]
    fn a_temp_file_that_cannot_be_written_names_it_and_keeps_the_old_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ui.toml");
        ui_settings::save(&path, &OFF).unwrap();
        // A folder where the temp file should go.
        let tmp_file = tmp.path().join("ui.toml.tmp");
        std::fs::create_dir(&tmp_file).unwrap();
        let c = chain(&ui_settings::save(&path, &ON).unwrap_err());
        assert_eq!(c[0], format!("writing {}", tmp_file.display()));
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!(ui_settings::load(&path).unwrap(), OFF);
    }

    #[test]
    fn a_parent_that_is_a_file_is_an_error_naming_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let blocker = tmp.path().join("worklog");
        std::fs::write(&blocker, "a file, not a folder").unwrap();
        let path = blocker.join("ui.toml");
        let c = chain(&ui_settings::save(&path, &OFF).unwrap_err());
        assert_eq!(c[0], format!("creating {}", blocker.display()));
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!(std::fs::read_to_string(&blocker).unwrap(), "a file, not a folder");
    }
}

mod window_section {
    use super::*;

    #[test]
    fn new_keeps_the_given_settings_without_an_error() {
        for s in [OFF, ON] {
            let section = WindowSection::new(s);
            assert_eq!(section.settings, s);
            assert_eq!(section.error, None, "{s:?}");
        }
    }

    #[test]
    fn a_toggle_is_remembered_in_ui_toml_alone_and_returns_the_level() {
        // (settings after the toggle, expected level)
        let rows = [(OFF, WindowLevel::Normal), (ON, WindowLevel::AlwaysOnTop)];
        for (s, level) in rows {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("ui.toml");
            let mut section = WindowSection::new(s);
            assert_eq!(section.apply_toggle(Some(&path)), level, "{s:?}");
            assert_eq!(section.error, None, "{s:?}");
            assert_eq!(section.settings, s);
            assert_eq!(ui_settings::load(&path).unwrap(), s);
            assert_eq!(files_in(tmp.path()), vec!["ui.toml".to_string()], "{s:?}");
        }
    }

    #[test]
    fn a_failed_save_is_the_error_and_the_level_still_follows_the_checkbox() {
        // (settings after the toggle, expected level)
        let rows = [(OFF, WindowLevel::Normal), (ON, WindowLevel::AlwaysOnTop)];
        for (s, level) in rows {
            let tmp = tempfile::tempdir().unwrap();
            // A folder where the file should go, so the rename over it fails.
            let path = tmp.path().join("ui.toml");
            std::fs::create_dir(&path).unwrap();
            let mut section = WindowSection::new(s);
            assert_eq!(section.apply_toggle(Some(&path)), level, "{s:?}");
            assert_eq!(section.settings, s);
            let e = section.error.clone().expect("an error");
            // The OS's reason after the head is not ours to word.
            let head = format!("replacing {}: ", path.display());
            assert!(e.starts_with(&head) && e.len() > head.len(), "{s:?}: {e}");
            assert_eq!(section.error_line(), Some(format!("not remembered for next time: {e}")));
        }
    }

    #[test]
    fn no_location_is_the_error_and_the_level_still_follows_the_checkbox() {
        // (settings after the toggle, expected level)
        let rows = [(OFF, WindowLevel::Normal), (ON, WindowLevel::AlwaysOnTop)];
        for (s, level) in rows {
            let mut section = WindowSection::new(s);
            assert_eq!(section.apply_toggle(None), level, "{s:?}");
            assert_eq!(section.settings, s);
            assert_eq!(section.error.as_deref(), Some("no config directory to keep the window settings in"), "{s:?}");
        }
    }

    #[test]
    fn a_later_successful_save_clears_the_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ui.toml");
        std::fs::create_dir(&path).unwrap();
        let mut section = WindowSection::new(OFF);
        section.apply_toggle(Some(&path));
        assert!(section.error.is_some(), "the first save must fail for this test to mean anything");

        std::fs::remove_dir(&path).unwrap();
        section.settings = ON;
        assert_eq!(section.apply_toggle(Some(&path)), WindowLevel::AlwaysOnTop);
        assert_eq!(section.error, None);
        assert_eq!(section.error_line(), None);
        assert_eq!(ui_settings::load(&path).unwrap(), ON);
    }

    #[test]
    fn the_error_line_names_why_the_change_was_not_remembered() {
        // (error, expected line)
        let rows = [
            (None, None),
            (Some("disk F: is read-only"), Some("not remembered for next time: disk F: is read-only")),
            (Some("no config directory to keep the window settings in"), Some("not remembered for next time: no config directory to keep the window settings in")),
        ];
        for (error, expected) in rows {
            let section = WindowSection { settings: OFF, error: error.map(str::to_string) };
            assert_eq!(section.error_line().as_deref(), expected, "{error:?}");
        }
    }
}

mod save_at {
    use super::*;

    #[test]
    fn no_location_is_an_error_to_show() {
        assert_eq!(ui_settings::save_at(None, &OFF), Err("no config directory to keep the window settings in".to_string()));
    }

    #[test]
    fn a_location_is_saved_to() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg").join("ui.toml");
        for s in [OFF, ON] {
            assert_eq!(ui_settings::save_at(Some(&path), &s), Ok(()), "{s:?}");
            assert_eq!(ui_settings::load(&path).unwrap(), s);
        }
    }

    #[test]
    fn a_failed_save_is_the_whole_chain_on_one_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ui.toml");
        std::fs::create_dir(&path).unwrap();
        let e = ui_settings::save_at(Some(&path), &OFF).unwrap_err();
        // `{e:#}` joins the chain with ": "; the OS's reason after it is not ours to word.
        let head = format!("replacing {}: ", path.display());
        assert!(e.starts_with(&head) && e.len() > head.len(), "{e}");
        assert!(!e.contains('\n'), "{e}");
    }
}
