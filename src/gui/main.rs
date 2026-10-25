//! process-corral-gui: a window and tray icon on top of the engine's status.json, config file, log
//! and the `process-corral.exe` helper subcommands. Runs as a normal user; elevation happens only
//! in the short-lived helper, per action, via UAC.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod actions;
mod app;
mod logic;
mod picker;
mod poller;
mod runas;
mod tray;

use eframe::egui;
use process_corral::instance;
use process_corral::paths;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::sync::Arc;
use tray::TrayShared;
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK};

const GUI_MUTEX: &str = "Local\\ProcessCorralGui";

fn message_box(text: &str, error: bool) {
    let t: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let c: Vec<u16> = tray::WINDOW_TITLE.encode_utf16().chain(Some(0)).collect();
    unsafe { MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | if error { MB_ICONERROR } else { MB_ICONINFORMATION }) };
}

/// `process-corral.exe` in the same folder as this exe.
fn helper_path() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(|d| d.join("process-corral.exe"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        message_box(
            "process-corral-gui [--system | --user] [--config FILE] [--log FILE] [--status FILE] [--tab status|settings|log] [--quit]\n\n\
             --quit asks a running window to exit (the service is not touched).\n\n\
             Default: the service files under C:\\ProgramData\\ProcessCorral, or the per-user files in\n\
             %APPDATA%\\ProcessCorral if the service location has none.",
            false,
        );
        return;
    }

    if args.iter().any(|a| a == "--quit") {
        // Used by the installer before it replaces the exe. Nothing running is not an error.
        tray::signal_quit();
        return;
    }

    // One GUI at a time: a second launch asks the first to come forward and exits.
    let _single = match instance::acquire(GUI_MUTEX) {
        Ok(g) => g,
        Err(instance::Error::AlreadyRunning) => {
            if !tray::signal_existing() {
                message_box("Process Corral is already running (it may still be starting). Look for its icon in the system tray.", false);
            }
            return;
        }
        Err(instance::Error::Os(e)) => {
            message_box(&format!("Could not create the single-instance lock (Windows error {e})."), true);
            return;
        }
    };

    let (paths, scope) = paths::resolve_gui(&args);
    let start_tab = args.iter().position(|a| a == "--tab").and_then(|i| args.get(i + 1)).cloned().unwrap_or_default();
    let service_config = paths::resolve_with(&[], true, &|k| std::env::var(k).ok()).config;
    let shared = Arc::new(TrayShared::default());
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(tray::WINDOW_TITLE)
            .with_inner_size([1000.0, 720.0])
            .with_min_inner_size([760.0, 480.0])
            // Empty on purpose: tells eframe not to apply its own default icon; tray::set_window_icons
            // sets ours from the exe resources instead.
            .with_icon(egui::IconData::default()),
        ..Default::default()
    };

    let result = eframe::run_native(
        tray::WINDOW_TITLE,
        options,
        Box::new({
            let (paths, shared, stop) = (paths.clone(), Arc::clone(&shared), Arc::clone(&stop));
            move |cc| {
                tray::set_window_icons();
                let ctx = cc.egui_ctx.clone();
                let runner = actions::Runner::new(helper_path(), paths.log.clone(), ctx.clone());
                tray::spawn(Arc::clone(&shared), Arc::clone(&runner), ctx.clone());
                poller::spawn(paths.clone(), ctx, tx, stop, Arc::clone(&shared));
                Ok(Box::new(app::App::new(paths, scope, runner, shared, rx, service_config, &start_tab)))
            }
        }),
    );
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Err(e) = result {
        message_box(&format!("Process Corral could not open its window:\n{e}"), true);
    }
}

