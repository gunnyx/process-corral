//! Service Control Manager client: install / uninstall / start / stop the `ProcessCorral` service.

use crate::config::DEFAULT_INI;
use crate::paths;
use std::ffi::c_void;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows_sys::Win32::System::Diagnostics::Debug::{
    FormatMessageW, FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS,
};
use windows_sys::Win32::System::Services::*;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub const NAME: &str = "ProcessCorral";
const DISPLAY_NAME: &str = "Process Corral";
const DESCRIPTION: &str = "Automatically lowers the priority of runaway processes to keep the system responsive.";
/// Winnt.h `DELETE` standard access right.
const DELETE: u32 = 0x0001_0000;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn err_text(code: u32) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe {
        FormatMessageW(
            FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
            null(),
            code,
            0,
            buf.as_mut_ptr(),
            buf.len() as u32,
            null(),
        )
    };
    if n == 0 {
        format!("Win32 error {code}")
    } else {
        format!("{} (Win32 error {code})", String::from_utf16_lossy(&buf[..n as usize]).trim())
    }
}

pub fn state_name(state: u32) -> &'static str {
    match state {
        SERVICE_STOPPED => "stopped",
        SERVICE_START_PENDING => "start pending",
        SERVICE_STOP_PENDING => "stop pending",
        SERVICE_RUNNING => "running",
        SERVICE_CONTINUE_PENDING => "continue pending",
        SERVICE_PAUSE_PENDING => "pause pending",
        SERVICE_PAUSED => "paused",
        _ => "unknown",
    }
}

/// The command line the SCM will run. Explicit paths are baked in only if the user gave them.
pub fn service_command_line(exe: &Path, config: Option<&str>, log: Option<&str>) -> String {
    let mut s = format!("\"{}\" service", exe.display());
    if let Some(c) = config {
        s.push_str(&format!(" --config \"{c}\""));
    }
    if let Some(l) = log {
        s.push_str(&format!(" --log \"{l}\""));
    }
    s
}

/// Poll `done` until it is true or `timeout` passes.
pub fn wait_until(mut done: impl FnMut() -> bool, timeout: Duration, poll: Duration) -> bool {
    let end = Instant::now() + timeout;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(poll);
    }
}

/// Why a service that stopped by itself did so, from its exit code (see service.rs).
pub fn exit_code_hint(code: u32, config: &Path, log: &Path) -> String {
    match code {
        ERROR_SERVICE_ALREADY_RUNNING => "another Process Corral instance (a console `run`?) is already running".into(),
        ERROR_BAD_CONFIGURATION => format!("the config file is invalid; see {}", log.display()),
        ERROR_OPEN_FAILED => format!("could not open the log file {}", log.display()),
        _ => format!("{} (config {})", err_text(code), config.display()),
    }
}

struct Sc(SC_HANDLE);
impl Drop for Sc {
    fn drop(&mut self) {
        unsafe { CloseServiceHandle(self.0) };
    }
}
fn valid(h: SC_HANDLE) -> bool {
    h as usize != 0
}

fn open_manager(access: u32) -> Result<Sc, u32> {
    let h = unsafe { OpenSCManagerW(null(), null(), access) };
    if valid(h) { Ok(Sc(h)) } else { Err(unsafe { GetLastError() }) }
}

fn open_service(scm: &Sc, access: u32) -> Result<Sc, u32> {
    let name = wide(NAME);
    let h = unsafe { OpenServiceW(scm.0, name.as_ptr(), access) };
    if valid(h) { Ok(Sc(h)) } else { Err(unsafe { GetLastError() }) }
}

fn query(s: &Sc) -> Result<SERVICE_STATUS, u32> {
    let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    if unsafe { QueryServiceStatus(s.0, &mut st) } != 0 { Ok(st) } else { Err(unsafe { GetLastError() }) }
}

fn fail(what: &str, code: u32) -> i32 {
    if code == ERROR_ACCESS_DENIED {
        eprintln!("{what}: access denied. Run this command from an elevated (Administrator) terminal.");
    } else if code == ERROR_SERVICE_DOES_NOT_EXIST {
        eprintln!("{what}: the service '{NAME}' is not installed. Use `process-corral install` first.");
    } else {
        eprintln!("{what}: {}", err_text(code));
    }
    1
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    NotInstalled,
    Stopped,
    StartPending,
    Running,
    StopPending,
    /// Paused/continue-pending etc.
    Other(u32),
    /// Could not ask the SCM (Win32 error code).
    Unknown(u32),
}

impl ServiceState {
    pub fn label(self) -> &'static str {
        match self {
            ServiceState::NotInstalled => "not installed",
            ServiceState::Stopped => "stopped",
            ServiceState::StartPending => "starting",
            ServiceState::Running => "running",
            ServiceState::StopPending => "stopping",
            ServiceState::Other(_) => "other",
            ServiceState::Unknown(_) => "unknown",
        }
    }
}

/// State of the `ProcessCorral` service. Needs no elevation: connecting to the SCM and querying a
/// service's status is allowed for standard users.
pub fn service_state() -> ServiceState {
    service_state_of(NAME)
}

pub fn service_state_of(name: &str) -> ServiceState {
    let scm = match open_manager(SC_MANAGER_CONNECT) {
        Ok(s) => s,
        Err(e) => return ServiceState::Unknown(e),
    };
    let w = wide(name);
    let h = unsafe { OpenServiceW(scm.0, w.as_ptr(), SERVICE_QUERY_STATUS) };
    if !valid(h) {
        let e = unsafe { GetLastError() };
        return if e == ERROR_SERVICE_DOES_NOT_EXIST { ServiceState::NotInstalled } else { ServiceState::Unknown(e) };
    }
    let svc = Sc(h);
    match query(&svc) {
        Ok(st) => match st.dwCurrentState {
            SERVICE_STOPPED => ServiceState::Stopped,
            SERVICE_START_PENDING => ServiceState::StartPending,
            SERVICE_RUNNING => ServiceState::Running,
            SERVICE_STOP_PENDING => ServiceState::StopPending,
            other => ServiceState::Other(other),
        },
        Err(e) => ServiceState::Unknown(e),
    }
}

pub fn is_elevated() -> bool {
    unsafe {
        let mut token = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut ret = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut e as *mut _ as *mut c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret,
        );
        CloseHandle(token);
        ok != 0 && e.TokenIsElevated != 0
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

pub fn install(args: &[String]) -> i32 {
    if !is_elevated() {
        eprintln!("install: must be run from an elevated (Administrator) terminal.");
        return 1;
    }
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("install: cannot determine this program's path: {e}");
            return 1;
        }
    };
    let scm = match open_manager(SC_MANAGER_CONNECT | SC_MANAGER_CREATE_SERVICE) {
        Ok(s) => s,
        Err(e) => return fail("install: cannot open the Service Control Manager", e),
    };

    // Refuse before touching anything else.
    match open_service(&scm, SERVICE_QUERY_STATUS) {
        Ok(existing) => {
            let state = query(&existing).map(|s| state_name(s.dwCurrentState)).unwrap_or("unknown state");
            eprintln!(
                "install: a service named '{NAME}' already exists ({state}). Nothing was changed.\n\
                 To replace it: `process-corral stop`, `process-corral uninstall`, then `process-corral install`."
            );
            return 1;
        }
        Err(ERROR_SERVICE_DOES_NOT_EXIST) => {}
        Err(e) => return fail("install: cannot check for an existing service", e),
    }

    let explicit_cfg = flag(args, "--config");
    let explicit_log = flag(args, "--log");
    let p = paths::resolve(args, true);

    // Data directory + default config (only for the default location).
    if explicit_cfg.is_none() || explicit_log.is_none() {
        let base = paths::system_base(std::env::var("ProgramData").ok().as_deref());
        match paths::prepare_system_dirs(&base) {
            Ok(notes) => notes.iter().for_each(|n| eprintln!("note: {n}")),
            Err(e) => {
                eprintln!("install: {e}");
                return 1;
            }
        }
    }
    if explicit_cfg.is_none() && !p.config.exists() {
        if let Err(e) = std::fs::write(&p.config, DEFAULT_INI) {
            eprintln!("install: cannot write default config {}: {e}", p.config.display());
            return 1;
        }
    }

    let cmdline = service_command_line(&exe, explicit_cfg.as_deref(), explicit_log.as_deref());
    let (name, display, bin) = (wide(NAME), wide(DISPLAY_NAME), wide(&cmdline));
    let h = unsafe {
        CreateServiceW(
            scm.0,
            name.as_ptr(),
            display.as_ptr(),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_AUTO_START,
            SERVICE_ERROR_NORMAL,
            bin.as_ptr(),
            null(),
            null_mut(),
            null(),
            null(), // account: NULL = LocalSystem
            null(),
        )
    };
    if !valid(h) {
        let e = unsafe { GetLastError() };
        if e == ERROR_SERVICE_EXISTS {
            eprintln!("install: a service named '{NAME}' already exists (created just now by someone else). Nothing was changed.");
            return 1;
        }
        return fail("install: CreateService failed", e);
    }
    let svc = Sc(h);
    let mut desc = wide(DESCRIPTION);
    let d = SERVICE_DESCRIPTIONW { lpDescription: desc.as_mut_ptr() };
    unsafe { ChangeServiceConfig2W(svc.0, SERVICE_CONFIG_DESCRIPTION, &d as *const _ as *const c_void) };

    println!("Installed service '{NAME}' (automatic start, account LocalSystem).");
    println!("  command: {cmdline}");
    println!("  config:  {}", p.config.display());
    println!("  log:     {}", p.log.display());
    println!("  status:  {}", p.status.display());
    let lossy = exe.display().to_string().to_ascii_lowercase();
    if lossy.contains("\\target\\debug\\") || lossy.contains("\\target\\release\\") {
        println!(
            "warning: the service runs the binary at {}.\n         Moving or deleting it breaks the service, and `cargo build` cannot replace it while the service is running.\n         Copy the exe to a stable folder and run `install` from there for real use.",
            exe.display()
        );
    }
    println!("It is not started yet: run `process-corral start` (or reboot).");
    0
}

pub fn uninstall() -> i32 {
    let scm = match open_manager(SC_MANAGER_CONNECT) {
        Ok(s) => s,
        Err(e) => return fail("uninstall: cannot open the Service Control Manager", e),
    };
    let svc = match open_service(&scm, DELETE | SERVICE_STOP | SERVICE_QUERY_STATUS) {
        Ok(s) => s,
        Err(ERROR_SERVICE_DOES_NOT_EXIST) => {
            println!("Service '{NAME}' is not installed; nothing to remove.");
            return 0;
        }
        Err(e) => return fail("uninstall", e),
    };
    let mut note = "";
    if query(&svc).map(|s| s.dwCurrentState != SERVICE_STOPPED).unwrap_or(false) {
        println!("Stopping service first...");
        let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
        unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut st) };
        let stopped = wait_until(
            || query(&svc).map(|s| s.dwCurrentState == SERVICE_STOPPED).unwrap_or(true),
            Duration::from_secs(30),
            Duration::from_millis(250),
        );
        if !stopped {
            note = " It did not stop within 30s; it will be removed when it does (or at reboot).";
        }
    }
    if unsafe { DeleteService(svc.0) } == 0 {
        let e = unsafe { GetLastError() };
        if e == ERROR_SERVICE_MARKED_FOR_DELETE {
            println!("Service '{NAME}' is already marked for deletion; it disappears once it stops and handles close.");
            return 0;
        }
        return fail("uninstall: DeleteService failed", e);
    }
    println!("Removed service '{NAME}'.{note} Config and logs were left in place.");
    0
}

pub fn start(args: &[String]) -> i32 {
    let scm = match open_manager(SC_MANAGER_CONNECT) {
        Ok(s) => s,
        Err(e) => return fail("start: cannot open the Service Control Manager", e),
    };
    let svc = match open_service(&scm, SERVICE_START | SERVICE_QUERY_STATUS) {
        Ok(s) => s,
        Err(e) => return fail("start", e),
    };
    if unsafe { StartServiceW(svc.0, 0, null()) } == 0 {
        let e = unsafe { GetLastError() };
        if e == ERROR_SERVICE_ALREADY_RUNNING {
            println!("Service '{NAME}' is already running.");
            return 0;
        }
        return fail("start: StartService failed", e);
    }
    let up = wait_until(
        || query(&svc).map(|s| s.dwCurrentState != SERVICE_START_PENDING).unwrap_or(true),
        Duration::from_secs(20),
        Duration::from_millis(250),
    );
    match query(&svc) {
        Ok(s) if s.dwCurrentState == SERVICE_RUNNING => {
            println!("Service '{NAME}' is running.");
            0
        }
        Ok(s) if s.dwCurrentState == SERVICE_STOPPED => {
            let p = paths::resolve(args, true);
            eprintln!("start: the service started and then stopped: {}", exit_code_hint(s.dwWin32ExitCode, &p.config, &p.log));
            1
        }
        Ok(s) => {
            eprintln!("start: service is {}{}", state_name(s.dwCurrentState), if up { "" } else { " (timed out waiting)" });
            1
        }
        Err(e) => fail("start: cannot query status", e),
    }
}

pub fn stop() -> i32 {
    let scm = match open_manager(SC_MANAGER_CONNECT) {
        Ok(s) => s,
        Err(e) => return fail("stop: cannot open the Service Control Manager", e),
    };
    let svc = match open_service(&scm, SERVICE_STOP | SERVICE_QUERY_STATUS) {
        Ok(s) => s,
        Err(e) => return fail("stop", e),
    };
    let mut st: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    if unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut st) } == 0 {
        let e = unsafe { GetLastError() };
        if e == ERROR_SERVICE_NOT_ACTIVE {
            println!("Service '{NAME}' is not running.");
            return 0;
        }
        return fail("stop: ControlService failed", e);
    }
    let stopped = wait_until(
        || query(&svc).map(|s| s.dwCurrentState == SERVICE_STOPPED).unwrap_or(true),
        Duration::from_secs(30),
        Duration::from_millis(250),
    );
    if stopped {
        println!("Service '{NAME}' stopped.");
        0
    } else {
        eprintln!("stop: the service did not stop within 30s.");
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn command_line_quotes_exe_and_only_bakes_explicit_paths() {
        let exe = Path::new(r"C:\Program Files\Corral\process-corral.exe");
        assert_eq!(service_command_line(exe, None, None), r#""C:\Program Files\Corral\process-corral.exe" service"#);
        assert_eq!(
            service_command_line(exe, Some(r"D:\my cfg\c.ini"), Some(r"D:\l.log")),
            r#""C:\Program Files\Corral\process-corral.exe" service --config "D:\my cfg\c.ini" --log "D:\l.log""#
        );
    }

    #[test]
    fn unknown_service_is_reported_as_not_installed_without_elevation() {
        assert_eq!(service_state_of("ProcessCorralNoSuchService_f3a9"), ServiceState::NotInstalled);
        // The real one: whatever it is, the query itself must succeed.
        assert!(!matches!(service_state(), ServiceState::Unknown(_)), "{:?}", service_state());
        assert_eq!(ServiceState::Running.label(), "running");
    }

    #[test]
    fn state_names() {
        assert_eq!(state_name(SERVICE_RUNNING), "running");
        assert_eq!(state_name(SERVICE_STOPPED), "stopped");
        assert_eq!(state_name(999), "unknown");
    }

    #[test]
    fn wait_until_returns_when_condition_met_or_times_out() {
        let n = Cell::new(0);
        assert!(wait_until(|| { n.set(n.get() + 1); n.get() >= 3 }, Duration::from_secs(5), Duration::from_millis(1)));
        assert_eq!(n.get(), 3);
        let t = Instant::now();
        assert!(!wait_until(|| false, Duration::from_millis(40), Duration::from_millis(5)));
        assert!(t.elapsed() >= Duration::from_millis(40));
    }

    #[test]
    fn exit_code_hints_are_actionable() {
        let (c, l) = (Path::new(r"C:\c.ini"), Path::new(r"C:\l.log"));
        assert!(exit_code_hint(ERROR_SERVICE_ALREADY_RUNNING, c, l).contains("already running"));
        assert!(exit_code_hint(ERROR_BAD_CONFIGURATION, c, l).contains(r"C:\l.log"));
    }

    #[test]
    fn err_text_is_readable() {
        assert!(err_text(ERROR_ACCESS_DENIED).contains("5"));
        assert!(err_text(ERROR_SERVICE_DOES_NOT_EXIST).to_lowercase().contains("service"));
    }
}
