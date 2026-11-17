//! The worklog icon: the PNG the window shows, the ICO and SVG next to it, and (on Windows) the
//! icon resource both executables carry.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use worklog::ui;
use worklog::ui_settings::UiSettings;

const SIDE: u32 = 256;
/// The sizes `make_icon.py` writes into `worklog.ico`.
const ICO_SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

fn asset(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("assets").join("icon").join(name)
}

fn alpha(icon: &eframe::egui::IconData, x: u32, y: u32) -> u8 {
    icon.rgba[((y * icon.width + x) * 4 + 3) as usize]
}

/// One ICONDIRENTRY of an `.ico` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IcoEntry {
    width: u32,
    height: u32,
    bit_count: u16,
    bytes: u32,
    offset: u32,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Width and height bytes of 0 stand for 256 in both ICONDIRENTRY and GRPICONDIRENTRY.
fn dim(byte: u8) -> u32 {
    if byte == 0 { 256 } else { u32::from(byte) }
}

/// ICONDIR: reserved 0, type 1 (icon), count; then `count` 16-byte ICONDIRENTRYs.
fn parse_ico(b: &[u8]) -> Vec<IcoEntry> {
    assert!(b.len() >= 6, "ICO shorter than its header: {} bytes", b.len());
    assert_eq!((u16_at(b, 0), u16_at(b, 2)), (0, 1), "ICONDIR reserved/type");
    let count = usize::from(u16_at(b, 4));
    assert!(b.len() >= 6 + 16 * count, "ICO directory runs past the file end");
    (0..count)
        .map(|i| {
            let e = 6 + 16 * i;
            IcoEntry {
                width: dim(b[e]),
                height: dim(b[e + 1]),
                bit_count: u16_at(b, e + 6),
                bytes: u32_at(b, e + 8),
                offset: u32_at(b, e + 12),
            }
        })
        .collect()
}

mod app_icon {
    use super::*;

    #[test]
    fn decodes_the_embedded_png_as_a_256_px_rgba_square() {
        let icon = ui::app_icon().expect("the embedded PNG decodes");
        assert_eq!((icon.width, icon.height), (SIDE, SIDE));
        assert_eq!(icon.rgba.len(), (SIDE * SIDE * 4) as usize);
    }

    #[test]
    fn is_a_rounded_tile_opaque_in_the_middle_and_transparent_in_every_corner() {
        let icon = ui::app_icon().expect("the embedded PNG decodes");
        assert_eq!(alpha(&icon, SIDE / 2, SIDE / 2), 255, "centre");
        let last = SIDE - 1;
        for (x, y) in [(0, 0), (last, 0), (0, last), (last, last)] {
            assert_eq!(alpha(&icon, x, y), 0, "corner ({x}, {y})");
        }
    }

    #[test]
    fn native_options_give_the_window_the_same_icon() {
        let options = ui::native_options(NaiveDate::from_ymd_opt(2026, 5, 21).unwrap(), UiSettings { always_on_top: false });
        let icon = options.viewport.icon.expect("viewport.icon is set");
        assert_eq!(*icon, ui::app_icon().unwrap());
    }
}

mod files {
    use super::*;

    #[test]
    fn ico_holds_exactly_the_listed_square_sizes_each_inside_the_file() {
        let bytes = std::fs::read(asset("worklog.ico")).unwrap();
        let entries = parse_ico(&bytes);
        let mut sizes: Vec<u32> = entries.iter().map(|e| e.width).collect();
        sizes.sort_unstable();
        assert_eq!(sizes, ICO_SIZES, "entries: {entries:?}");
        for e in &entries {
            assert_eq!(e.height, e.width, "{e:?} is not square");
            assert!(e.bytes > 0 && (e.offset as usize) + (e.bytes as usize) <= bytes.len(), "{e:?} outside the {} byte file", bytes.len());
        }
    }

    #[test]
    fn svg_draws_on_a_1024_unit_canvas() {
        let svg = std::fs::read_to_string(asset("worklog.svg")).unwrap();
        let root = svg.lines().find(|l| l.trim_start().starts_with("<svg")).expect("an <svg> element");
        assert!(root.contains(r#"viewBox="0 0 1024 1024""#), "root element: {root}");
    }
}

/// Reads RT_GROUP_ICON resources out of an executable without running it.
#[cfg(windows)]
mod pe {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    type Handle = *mut c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LoadLibraryExW(name: *const u16, file: Handle, flags: u32) -> Handle;
        fn FindResourceW(module: Handle, name: *const u16, kind: *const u16) -> Handle;
        fn LoadResource(module: Handle, res: Handle) -> Handle;
        fn LockResource(data: Handle) -> *const u8;
        fn SizeofResource(module: Handle, res: Handle) -> u32;
        fn FreeLibrary(module: Handle) -> i32;
    }

    const LOAD_LIBRARY_AS_DATAFILE: u32 = 0x2;
    const LOAD_LIBRARY_AS_IMAGE_RESOURCE: u32 = 0x20;
    const RT_GROUP_ICON: usize = 14;

    /// One GRPICONDIRENTRY: like an ICONDIRENTRY, with a resource id in place of the file offset.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct GroupEntry {
        pub width: u32,
        pub height: u32,
        pub bit_count: u16,
        pub bytes: u32,
    }

    /// The entries of icon group `id` in `exe`, or `None` when the exe has no such group.
    pub fn group_icon(exe: &Path, id: u16) -> Option<Vec<GroupEntry>> {
        let wide: Vec<u16> = exe.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: plain Win32 calls on a module loaded as data; the resource bytes are copied out
        // before the module is freed.
        unsafe {
            let module = LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_AS_DATAFILE | LOAD_LIBRARY_AS_IMAGE_RESOURCE);
            assert!(!module.is_null(), "cannot load {} as data", exe.display());
            let res = FindResourceW(module, usize::from(id) as *const u16, RT_GROUP_ICON as *const u16);
            let data = if res.is_null() {
                None
            } else {
                let size = SizeofResource(module, res) as usize;
                let ptr = LockResource(LoadResource(module, res));
                Some(std::slice::from_raw_parts(ptr, size).to_vec())
            };
            FreeLibrary(module);
            data.map(|b| {
                let count = usize::from(super::u16_at(&b, 4));
                (0..count)
                    .map(|i| {
                        let e = 6 + 14 * i;
                        GroupEntry {
                            width: super::dim(b[e]),
                            height: super::dim(b[e + 1]),
                            bit_count: super::u16_at(&b, e + 6),
                            bytes: super::u32_at(&b, e + 8),
                        }
                    })
                    .collect()
            })
        }
    }
}

#[cfg(windows)]
mod executables {
    use super::*;
    use pe::{GroupEntry, group_icon};

    /// What `1 ICON "worklog.ico"` compiles to: one group entry per ICO image, in the file's order.
    fn expected_group() -> Vec<GroupEntry> {
        parse_ico(&std::fs::read(asset("worklog.ico")).unwrap())
            .into_iter()
            .map(|e| GroupEntry { width: e.width, height: e.height, bit_count: e.bit_count, bytes: e.bytes })
            .collect()
    }

    #[test]
    fn both_executables_carry_worklog_ico_as_icon_group_1() {
        let expected = expected_group();
        for exe in [env!("CARGO_BIN_EXE_worklog"), env!("CARGO_BIN_EXE_worklog-popup")] {
            assert_eq!(group_icon(Path::new(exe), 1), Some(expected.clone()), "{exe}");
        }
    }

    #[test]
    fn the_resource_check_sees_no_icon_in_an_exe_built_without_one() {
        // The test harness is not one of the bins build.rs gives the icon to.
        let me = std::env::current_exe().unwrap();
        assert_eq!(group_icon(&me, 1), None, "{}", me.display());
    }
}
