//! Tray icon, its menu, and the "show the window" plumbing.
//!
//! The tray lives on its own thread with its own message loop, and restores the window with plain
//! Win32 calls. That way it keeps working while the egui window is hidden (when eframe runs no UI
//! pass at all) and it can also act on a signal from a second launch of the GUI.

use eframe::egui;
use crate::actions::Runner;
use crate::logic::{circle_icon, encode_service, RUNNING_RGB, STOPPED_RGB};
use process_corral::elevate::Command;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, WAIT_OBJECT_0, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcessId, OpenEventW, SetEvent, EVENT_MODIFY_STATE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, DispatchMessageW, EnumWindows, GetSystemMetrics, GetWindowThreadProcessId,
    IsIconic, LoadImageW, MsgWaitForMultipleObjects, PeekMessageW, SendMessageTimeoutW, SendMessageW, SetForegroundWindow, ShowWindow,
    TranslateMessage, ASFW_ANY, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_SHARED, MSG, PM_REMOVE, QS_ALLINPUT, SM_CXICON,
    SM_CXSMICON, SM_CYICON, SM_CYSMICON, SMTO_ABORTIFHUNG, SW_RESTORE, SW_SHOW, WM_GETTEXT, WM_SETICON,
};

pub const WINDOW_TITLE: &str = "Process Corral";
/// Signalled by a second launch so the running instance shows its window.
pub const SHOW_EVENT: &str = "Local\\ProcessCorralGuiShow";
/// Signalled by `process-corral-gui --quit` so the running instance exits (the installer uses this).
pub const QUIT_EVENT: &str = "Local\\ProcessCorralGuiQuit";

/// State shared between the poller, the UI and the tray thread.
#[derive(Default)]
pub struct TrayShared {
    pub engine_running: AtomicBool,
    /// See `logic::encode_service`.
    pub service: AtomicU8,
    /// Set by the tray when it has shown the window; the UI then also asks eframe to show/focus it.
    pub show_requested: AtomicBool,
    /// Set by the UI's File > Exit; the tray thread removes the icon and ends the process.
    pub quit_requested: AtomicBool,
    /// True once the tray icon exists. If it could not be created, closing the window must really
    /// quit rather than hide to a tray that is not there.
    pub tray_ok: AtomicBool,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Called by a second instance: ask the first one to come forward. Returns false if it can't be reached.
pub fn signal_existing() -> bool {
    signal_named(SHOW_EVENT)
}

/// `process-corral-gui --quit` (used by the installer): ask a running GUI to exit. The service is not touched.
pub fn signal_quit() -> bool {
    signal_named(QUIT_EVENT)
}

fn signal_named(event: &str) -> bool {
    let name = wide(event);
    unsafe {
        let h = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
        if h.is_null() {
            return false;
        }
        AllowSetForegroundWindow(ASFW_ANY); // let the first instance take the foreground
        let ok = SetEvent(h) != 0;
        CloseHandle(h);
        ok
    }
}

struct Find {
    pid: u32,
    found: HWND,
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> i32 {
    let f = unsafe { &mut *(lparam as *mut Find) };
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid != f.pid {
        return 1;
    }
    // Ask for the title with a timeout instead of GetWindowTextW: that call waits for the window's own
    // thread to answer, and one of this process's windows may not be pumping messages at that moment.
    let mut buf = [0u16; 128];
    let mut n = 0usize;
    let answered = unsafe {
        SendMessageTimeoutW(hwnd, WM_GETTEXT, buf.len() as WPARAM, buf.as_mut_ptr() as LPARAM, SMTO_ABORTIFHUNG, 100, &mut n)
    };
    if answered == 0 {
        return 1;
    }
    if String::from_utf16_lossy(&buf[..n.min(buf.len())]) == WINDOW_TITLE {
        f.found = hwnd;
        return 0; // stop enumerating
    }
    1
}

/// Our own top-level window (visible or hidden), found by title among this process's windows.
pub fn find_main_window() -> Option<HWND> {
    let mut f = Find { pid: unsafe { GetCurrentProcessId() }, found: std::ptr::null_mut() };
    unsafe { EnumWindows(Some(enum_cb), &mut f as *mut Find as LPARAM) };
    (!f.found.is_null()).then_some(f.found)
}

/// Give our window the icon embedded by build.rs (resource id 1) for the title bar (small) and the
/// taskbar / Alt+Tab (large), picking the size Windows asks for at the current DPI.
///
/// Runs on its own short-lived thread: sending WM_SETICON to our window from inside winit's own
/// callback re-enters winit and hangs the UI thread, whereas from another thread it is just a message.
pub fn set_window_icons() {
    std::thread::spawn(|| {
        for _ in 0..100 {
            if let Some(hwnd) = find_main_window() {
                apply_window_icons(hwnd);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
}

fn apply_window_icons(hwnd: HWND) {
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        for (which, cx, cy) in [(ICON_SMALL, SM_CXSMICON, SM_CYSMICON), (ICON_BIG, SM_CXICON, SM_CYICON)] {
            let icon = LoadImageW(hinst, 1 as *const u16, IMAGE_ICON, GetSystemMetrics(cx), GetSystemMetrics(cy), LR_SHARED);
            if !icon.is_null() {
                SendMessageW(hwnd, WM_SETICON, which as WPARAM, icon as LPARAM);
            }
        }
    }
}

/// Bring the window back from the tray / minimised state and to the foreground.
pub fn show_main_window(ctx: &egui::Context, shared: &TrayShared) {
    if let Some(hwnd) = find_main_window() {
        unsafe {
            ShowWindow(hwnd, if IsIconic(hwnd) != 0 { SW_RESTORE } else { SW_SHOW });
            SetForegroundWindow(hwnd);
        }
    }
    shared.show_requested.store(true, Ordering::SeqCst);
    ctx.request_repaint();
}

/// Tray icon resource ids embedded by build.rs: 2 = engine running, 3 = engine not running.
fn icon(running: bool) -> Icon {
    Icon::from_resource(if running { 2 } else { 3 }, None).unwrap_or_else(|_| {
        // Resource missing (should not happen in a normal build): fall back to a plain coloured dot.
        let rgb = if running { RUNNING_RGB } else { STOPPED_RGB };
        Icon::from_rgba(circle_icon(32, rgb), 32, 32).expect("valid 32x32 RGBA icon")
    })
}

fn tooltip(running: bool, service: u8) -> String {
    let svc = match service {
        1 => "service not installed",
        2 => "service stopped",
        3 => "service starting",
        4 => "service running",
        5 => "service stopping",
        _ => "service state unknown",
    };
    format!("Process Corral - engine {} ({svc})", if running { "running" } else { "not running" })
}

pub fn spawn(shared: Arc<TrayShared>, runner: Arc<Runner>, ctx: egui::Context) {
    std::thread::Builder::new()
        .name("tray".into())
        .spawn(move || run(shared, runner, ctx))
        .expect("spawn tray thread");
}

fn run(shared: Arc<TrayShared>, runner: Arc<Runner>, ctx: egui::Context) {
    let name = wide(SHOW_EVENT);
    let show_event = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) }; // auto-reset
    let quit_name = wide(QUIT_EVENT);
    let quit_event = unsafe { CreateEventW(std::ptr::null(), 0, 0, quit_name.as_ptr()) };
    let events = [show_event, quit_event];

    let menu = Menu::new();
    let open = MenuItem::new("Open", true, None);
    let start = MenuItem::new("Start service", false, None);
    let stop = MenuItem::new("Stop service", false, None);
    let exit = MenuItem::new("Exit (service keeps running)", true, None);
    let _ = menu.append(&open);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&start);
    let _ = menu.append(&stop);
    let _ = menu.append(&PredefinedMenuItem::separator());
    let _ = menu.append(&exit);

    let tray: TrayIcon = match TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_icon(icon(false))
        .with_tooltip(tooltip(false, 0))
        .build()
    {
        Ok(t) => t,
        Err(e) => {
            // No tray (e.g. restricted shell): the window still works and closing it will quit.
            eprintln!("tray icon unavailable: {e}");
            return;
        }
    };
    shared.tray_ok.store(true, Ordering::SeqCst);

    let (mut shown_running, mut shown_service) = (false, 0u8);
    loop {
        // Wake on the "show" / "quit" events, on window messages, or every 250 ms to poll channels.
        let (n, handles) = if events.iter().any(|h| h.is_null()) { (0, std::ptr::null()) } else { (2, events.as_ptr()) };
        let waited = unsafe { MsgWaitForMultipleObjects(n, handles, 0, 250, QS_ALLINPUT) };
        if n == 2 && waited == WAIT_OBJECT_0 {
            show_main_window(&ctx, &shared);
        } else if n == 2 && waited == WAIT_OBJECT_0 + 1 {
            shared.quit_requested.store(true, Ordering::SeqCst);
        }
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if ev.id == *open.id() {
                show_main_window(&ctx, &shared);
            } else if ev.id == *start.id() {
                runner.service(Command::Start);
            } else if ev.id == *stop.id() {
                runner.service(Command::Stop);
            } else if ev.id == *exit.id() {
                shared.quit_requested.store(true, Ordering::SeqCst);
            }
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            match ev {
                TrayIconEvent::DoubleClick { button: MouseButton::Left, .. }
                | TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => {
                    show_main_window(&ctx, &shared)
                }
                _ => {}
            }
        }

        // Reflect state: icon colour, tooltip, which service items make sense.
        let running = shared.engine_running.load(Ordering::SeqCst);
        let service = shared.service.load(Ordering::SeqCst);
        if running != shown_running || service != shown_service {
            let _ = tray.set_icon(Some(icon(running)));
            let _ = tray.set_tooltip(Some(tooltip(running, service)));
            let can = runner.helper_present() && runner.snapshot().busy.is_none();
            start.set_enabled(can && service == encode_service(process_corral::scm::ServiceState::Stopped));
            stop.set_enabled(can && matches!(service, 3 | 4));
            (shown_running, shown_service) = (running, service);
        }

        if shared.quit_requested.load(Ordering::SeqCst) {
            // Exiting the GUI never touches the service. Drop the icon first so no ghost is left.
            drop(tray);
            std::process::exit(0);
        }
    }
}
