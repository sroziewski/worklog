//! The "Projects" window's editing and saving, without drawing it. `projects.toml` lives in a temp
//! dir.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use worklog::config::Config;
use worklog::projects::{self, Project};
use worklog::ui_projects::ProjectsEditor;

fn p(name: &str, git_dir: &str, jira_key: &str, enabled: bool) -> Project {
    Project { name: name.into(), git_dir: git_dir.into(), jira_key: jira_key.into(), enabled }
}

/// A list as `projects.toml` gave it: two enabled, one disabled.
fn loaded() -> Vec<Project> {
    vec![p("Alpha", r"D:\dev\alpha", "ABC", true), p("Beta", r"E:\src\beta", "XYZ", false), p("Meetings", "", "MEET", true)]
}

fn cfg_with_file_list() -> Config {
    let mut c = Config { ctx_env_file: None, ..Config::default() };
    c.apply_projects(&loaded());
    c
}

fn editor_at(path: &Path) -> ProjectsEditor {
    ProjectsEditor::new(&cfg_with_file_list(), Some(path.to_path_buf()))
}

fn ok(m: &str) -> Option<Result<String, String>> {
    Some(Ok(m.to_owned()))
}

fn err(m: &str) -> Option<Result<String, String>> {
    Some(Err(m.to_owned()))
}

/// No projects file yet: `config.toml` / `.env` settings, with or without a root no mapping
/// covers (which seeds a project without a key, so the list cannot be saved as it is).
fn seeded_cfg(unmapped_root: bool) -> Config {
    Config {
        repo_roots: if unmapped_root { vec![PathBuf::from(r"E:\src\misc")] } else { vec![] },
        project_keys: vec!["ABC".into(), "XYZ".into()],
        repo_projects: BTreeMap::from([(r"D:\dev\alpha".to_string(), "ABC".to_string())]),
        ctx_env_file: None,
        ..Config::default()
    }
}

/// What `seeded_cfg(false)` seeds: the mapped folder and the key no folder uses.
fn seeded() -> Vec<Project> {
    vec![p("alpha", r"D:\dev\alpha", "ABC", true), p("XYZ", "", "XYZ", true)]
}

mod new {
    use super::*;

    #[test]
    fn starts_from_the_file_list_clean_and_without_a_message() {
        let e = ProjectsEditor::new(&cfg_with_file_list(), Some(PathBuf::from(r"C:\nowhere\projects.toml")));
        assert_eq!(e.rows, loaded());
        assert_eq!((e.dirty(), e.edited()), (false, false));
        assert_eq!(e.message, None);
    }

    #[test]
    fn without_a_file_starts_from_the_config_and_env_settings_unsaved_but_unedited() {
        let e = ProjectsEditor::new(&seeded_cfg(true), None);
        // The mapped folder, the unmapped root (two projects: no key), the key no folder uses.
        assert_eq!(e.rows, vec![p("alpha", r"D:\dev\alpha", "ABC", true), p("misc", r"E:\src\misc", "", true), p("XYZ", "", "XYZ", true)]);
        // Not in the file yet, so Save is offered; nothing to revert.
        assert_eq!((e.dirty(), e.edited()), (true, false));
        assert_eq!(e.message, None);
    }
}

/// An editor started from `config.toml` / `.env` because there is no projects file yet.
mod seeded_editor {
    use super::*;

    #[test]
    fn revert_returns_to_the_seeded_list_which_is_still_unsaved() {
        let mut e = ProjectsEditor::new(&seeded_cfg(false), None);
        e.add();
        e.rows[0].enabled = false;
        assert_eq!((e.dirty(), e.edited()), (true, true));
        e.revert();
        assert_eq!(e.rows, seeded());
        assert_eq!((e.dirty(), e.edited()), (true, false));
    }

    #[test]
    fn saving_it_unchanged_writes_it_and_makes_it_clean() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("projects.toml");
        let mut e = ProjectsEditor::new(&seeded_cfg(false), Some(path.clone()));
        assert_eq!(e.save(), Some(seeded()));
        assert_eq!(e.message, ok("saved: 2 of 2 projects enabled"));
        assert_eq!((e.dirty(), e.edited()), (false, false));
        assert_eq!(projects::load(&path).unwrap(), Some(seeded()));
    }

    #[test]
    fn a_failed_save_leaves_it_unsaved() {
        // (case, cfg, path, message)
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("projects.toml");
        let rows: [(&str, Config, Option<PathBuf>, &str); 2] = [
            ("no config directory", seeded_cfg(false), None, "no config directory to keep the projects in"),
            ("a seeded project without a key", seeded_cfg(true), Some(path.clone()), "misc: the Jira key is empty"),
        ];
        for (case, cfg, at, message) in rows {
            let mut e = ProjectsEditor::new(&cfg, at);
            let before = e.rows.clone();
            assert_eq!(e.save(), None, "{case}");
            assert_eq!(e.message, err(message), "{case}");
            assert_eq!(e.rows, before, "{case}");
            assert_eq!((e.dirty(), e.edited()), (true, false), "{case}");
        }
        assert!(!path.exists());
    }
}

mod edit {
    use super::*;

    #[test]
    fn add_appends_an_empty_enabled_row() {
        let mut e = editor_at(Path::new(r"C:\nowhere\projects.toml"));
        e.add();
        let mut expected = loaded();
        expected.push(p("", "", "", true));
        assert_eq!(e.rows, expected);
        assert_eq!((e.dirty(), e.edited()), (true, true));
    }

    #[test]
    fn remove_drops_the_row_at_the_index() {
        let mut e = editor_at(Path::new(r"C:\nowhere\projects.toml"));
        e.remove(1);
        assert_eq!(e.rows, vec![p("Alpha", r"D:\dev\alpha", "ABC", true), p("Meetings", "", "MEET", true)]);
        assert_eq!((e.dirty(), e.edited()), (true, true));
    }

    #[test]
    fn remove_past_the_end_changes_nothing() {
        let mut e = editor_at(Path::new(r"C:\nowhere\projects.toml"));
        e.remove(3);
        e.remove(usize::MAX);
        assert_eq!(e.rows, loaded());
        assert_eq!((e.dirty(), e.edited()), (false, false));
    }

    #[test]
    fn a_changed_field_makes_it_dirty_and_changing_it_back_does_not() {
        let mut e = editor_at(Path::new(r"C:\nowhere\projects.toml"));
        e.rows[2].enabled = false;
        assert_eq!((e.dirty(), e.edited()), (true, true));
        e.rows[2].enabled = true;
        assert_eq!((e.dirty(), e.edited()), (false, false));
    }

    #[test]
    fn revert_returns_to_the_loaded_list() {
        let mut e = editor_at(Path::new(r"C:\nowhere\projects.toml"));
        e.add();
        e.remove(0);
        e.rows[0].name = "Beta renamed".into();
        e.revert();
        assert_eq!(e.rows, loaded());
        assert_eq!((e.dirty(), e.edited()), (false, false));
    }

    #[test]
    fn add_remove_and_revert_each_clear_the_last_message() {
        type Row = (&'static str, fn(&mut ProjectsEditor));
        let rows: [Row; 3] = [("add", |e| e.add()), ("remove", |e| e.remove(1)), ("revert", |e| e.revert())];
        for (name, op) in rows {
            // A failed save leaves an error to clear.
            let mut e = ProjectsEditor::new(&cfg_with_file_list(), None);
            e.add();
            e.save();
            assert_eq!(e.message, err("no config directory to keep the projects in"), "{name}");
            op(&mut e);
            assert_eq!(e.message, None, "{name}");
        }
    }
}

mod save {
    use super::*;

    #[test]
    fn writes_the_normalised_list_returns_it_and_makes_it_the_baseline() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("projects.toml");
        let mut e = editor_at(&path);
        e.add();
        e.rows[3] = p("  Gamma ", "  F:\\gamma ", " gam ", false);
        let normalised = vec![
            p("Alpha", r"D:\dev\alpha", "ABC", true),
            p("Beta", r"E:\src\beta", "XYZ", false),
            p("Meetings", "", "MEET", true),
            p("Gamma", r"F:\gamma", "GAM", false),
        ];

        assert_eq!(e.save(), Some(normalised.clone()));
        assert_eq!(e.message, ok("saved: 2 of 4 projects enabled"));
        assert_eq!(e.rows, normalised);
        assert_eq!((e.dirty(), e.edited()), (false, false));
        assert_eq!(projects::load(&path).unwrap(), Some(normalised.clone()));

        // Revert now returns to what was saved, not to the list the editor started from.
        e.remove(0);
        e.revert();
        assert_eq!(e.rows, normalised);
    }

    #[test]
    fn an_invalid_list_writes_nothing_and_keeps_the_edits() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("projects.toml");
        let mut e = editor_at(&path);
        e.rows[1].name = "  ".into();
        let edited = e.rows.clone();

        assert_eq!(e.save(), None);
        assert_eq!(e.message, err("project 2: the name is empty"));
        assert!(!path.exists());
        assert_eq!(e.rows, edited);
        assert_eq!((e.dirty(), e.edited()), (true, true));
        // The baseline is still the loaded list.
        e.revert();
        assert_eq!(e.rows, loaded());
    }

    #[test]
    fn an_invalid_list_leaves_an_existing_file_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("projects.toml");
        projects::save(&path, &loaded()).unwrap();
        let before = std::fs::read(&path).unwrap();
        let mut e = editor_at(&path);
        e.rows[2].git_dir = r"d:\DEV\alpha\".into();

        assert_eq!(e.save(), None);
        assert_eq!(e.message, err(r"Meetings: the folder d:\DEV\alpha\ is already used by Alpha"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn without_a_config_directory_nothing_is_saved() {
        let mut e = ProjectsEditor::new(&cfg_with_file_list(), None);
        e.rows[0].enabled = false;
        let edited = e.rows.clone();
        assert_eq!(e.save(), None);
        assert_eq!(e.message, err("no config directory to keep the projects in"));
        assert_eq!(e.rows, edited);
        assert_eq!((e.dirty(), e.edited()), (true, true));
    }

    #[test]
    fn a_folder_that_cannot_be_created_is_reported_as_the_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let blocker = tmp.path().join("a file");
        std::fs::write(&blocker, "not a folder").unwrap();
        let mut e = editor_at(&blocker.join("projects.toml"));
        e.rows[0].enabled = false;
        assert_eq!(e.save(), None);
        // The rest of the message is the OS's (and its language's) reason.
        let m = e.message.clone().unwrap().unwrap_err();
        assert!(m.starts_with(&format!("creating {}: ", blocker.display())), "{m}");
        assert_eq!((e.dirty(), e.edited()), (true, true));
    }

    #[test]
    fn saving_with_every_project_disabled_is_allowed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("projects.toml");
        let mut e = editor_at(&path);
        for r in &mut e.rows {
            r.enabled = false;
        }
        let saved = e.save().unwrap();
        assert_eq!(saved.iter().filter(|x| x.enabled).count(), 0);
        assert_eq!(e.message, ok("saved: 0 of 3 projects enabled"));
    }
}

mod field_widths {
    use worklog::ui_projects::field_widths;

    /// f32 products such as 1270 * 0.2 are not exact; a thousandth of a pixel is far below what
    /// the layout can show.
    const EPS: f32 = 1e-3;

    /// rest = max(available - 130, 480); name = clamp(rest * 0.2, 120, 260);
    /// key = clamp(rest * 0.1, 70, 120); dir = rest - name - key.
    #[test]
    fn split_the_rest_of_the_row_between_name_folder_and_key() {
        // (case, available, rest, name, dir, key), worked out by hand from the rule above
        let rows: [(&str, f32, f32, f32, f32, f32); 13] = [
            ("zero width", 0.0, 480.0, 120.0, 290.0, 70.0),
            ("negative width", -50.0, 480.0, 120.0, 290.0, 70.0),
            ("narrow: both minimums", 600.0, 480.0, 120.0, 290.0, 70.0),
            ("just under the floor", 609.0, 480.0, 120.0, 290.0, 70.0),
            ("at the floor", 610.0, 480.0, 120.0, 290.0, 70.0),
            ("just over the floor", 611.0, 481.0, 120.0, 291.0, 70.0),
            ("name leaves its minimum", 730.0, 600.0, 120.0, 410.0, 70.0),
            ("key leaves its minimum", 830.0, 700.0, 140.0, 490.0, 70.0),
            ("default window", 1100.0, 970.0, 194.0, 679.0, 97.0),
            ("key reaches its maximum", 1330.0, 1200.0, 240.0, 840.0, 120.0),
            ("wide: key capped, name not yet", 1400.0, 1270.0, 254.0, 896.0, 120.0),
            ("name reaches its maximum", 1430.0, 1300.0, 260.0, 920.0, 120.0),
            ("very wide: both maximums", 2000.0, 1870.0, 260.0, 1490.0, 120.0),
        ];
        for (case, available, rest, name, dir, key) in rows {
            let (n, d, k) = field_widths(available);
            let close = |a: f32, b: f32| (a - b).abs() < EPS;
            assert!(close(n, name) && close(d, dir) && close(k, key), "{case} ({available}): got (name, dir, key) = ({n}, {d}, {k}), want ({name}, {dir}, {key})");
            assert!(close(n + d + k, rest), "{case} ({available}): {n} + {d} + {k} != {rest}");
            assert!(d > n && d > k, "{case} ({available}): the folder ({d}) is not the widest field");
        }
    }
}

mod layout {
    use super::*;
    use worklog::ui_projects::field_widths;

    fn rect_widths(shape: &eframe::egui::Shape, out: &mut Vec<f32>) {
        match shape {
            eframe::egui::Shape::Rect(r) => out.push(r.rect.width()),
            eframe::egui::Shape::Vec(v) => v.iter().for_each(|s| rect_widths(s, out)),
            _ => {}
        }
    }

    /// egui's grid hands a cell only its column's width of the frame before, so a text field
    /// asking for more never got it: the fields stayed a few characters wide. Every field must
    /// be painted at the width `field_widths` gives for the space the section has.
    #[test]
    fn each_field_is_painted_as_wide_as_field_widths_says() {
        let tmp = tempfile::tempdir().unwrap();
        let mut editor = editor_at(&tmp.path().join("projects.toml"));
        let rows = editor.rows.len();
        assert!(rows >= 2, "the fixture needs several rows");
        let ctx = eframe::egui::Context::default();
        let input = || eframe::egui::RawInput {
            screen_rect: Some(eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(1200.0, 800.0))),
            ..Default::default()
        };
        let mut available = 0.0;
        let mut widths = Vec::new();
        for _ in 0..3 {
            let out = ctx.run_ui(input(), |ui| {
                available = ui.available_width();
                let _ = editor.body(ui, true);
            });
            widths.clear();
            for s in &out.shapes {
                rect_widths(&s.shape, &mut widths);
            }
        }
        let (name, dir, key) = field_widths(available);
        assert!(dir > 600.0, "the window is wide enough for a wide folder field ({available} -> {dir})");
        for (field, want) in [("name", name), ("git folder", dir), ("Jira key", key)] {
            let n = widths.iter().filter(|w| (**w - want).abs() < 0.5).count();
            assert_eq!(n, rows, "{field}: {rows} fields {want} wide expected, painted rect widths: {widths:?}");
        }
    }
}
