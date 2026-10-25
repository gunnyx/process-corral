//! Embeds the application icon and version information into the Windows executables.
//!
//! Resource ids: 1 = the application icon (what Explorer, shortcuts and the taskbar show),
//! 2 = tray icon "engine running", 3 = tray icon "engine not running" (loaded by the GUI at run time
//! with `Icon::from_resource`). Both binaries get the same resource; the tray variants are a few KB.
//!
//! This is a build-time step only: it adds no run-time dependency to either exe.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon/process-corral.ico");
    println!("cargo:rerun-if-changed=assets/icon/tray-running.ico");
    println!("cargo:rerun-if-changed=assets/icon/tray-stopped.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/icon/process-corral.ico");
    res.set_icon_with_id("assets/icon/tray-running.ico", "2");
    res.set_icon_with_id("assets/icon/tray-stopped.ico", "3");
    // FileVersion / ProductVersion come from the package version in Cargo.toml.
    res.set("ProductName", "Process Corral");
    res.set("FileDescription", "Process Corral");
    res.set("CompanyName", "Process Corral");
    res.set("LegalCopyright", "Copyright (c) Process Corral");
    if let Err(e) = res.compile() {
        panic!(
            "could not embed the icon and version info: {e}\n\
             This needs the Windows SDK resource compiler (rc.exe), which ships with Visual Studio Build Tools."
        );
    }
}
