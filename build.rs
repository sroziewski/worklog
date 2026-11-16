// Gives both executables the worklog icon on Windows (Explorer, taskbar, shortcuts). Needs the
// Windows SDK's rc.exe, which the MSVC toolchain installs; other targets skip it.
fn main() {
    println!("cargo:rerun-if-changed=assets/windows/worklog.rc");
    println!("cargo:rerun-if-changed=assets/icon/worklog.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/windows/worklog.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("compiling the Windows icon resource (is the Windows SDK installed?)");
    }
}
