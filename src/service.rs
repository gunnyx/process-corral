//! Service-side entry point: what the SCM runs (`process-corral service`).
//!
//! Lifecycle: START_PENDING -> take the single-instance lock -> open config/log -> RUNNING ->
//! (governor loop) -> on STOP/SHUTDOWN the handler sets the stop flag and reports STOP_PENDING;
//! the loop exits, restrained priorities are restored and the log flushed, the instance lock is
//! released, and only then is STOPPED reported.

use crate::governor::{self, Governor, OpenError};
use crate::instance;
use crate::log::{id, CsvLog, Event, Sink};
use crate::os::win::WinOs;
use crate::paths;
use crate::scm::{err_text, wide, NAME};
use std::ffi::c_void;
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::Services::*;

static STATUS_HANDLE: AtomicUsize = AtomicUsize::new(0);
static STATE: AtomicU32 = AtomicU32::new(SERVICE_START_PENDING);
static CHECKPOINT: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, PartialEq)]
pub enum Control {
    Stop,
    Interrogate,
    Unsupported,
}

/// SERVICE_CONTROL_SHUTDOWN is treated exactly like STOP: same cleanup path.
pub fn classify(code: u32) -> Control {
    match code {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => Control::Stop,
        SERVICE_CONTROL_INTERROGATE => Control::Interrogate,
        _ => Control::Unsupported,
    }
}

fn is_pending(state: u32) -> bool {
    matches!(state, SERVICE_START_PENDING | SERVICE_STOP_PENDING)
}

/// Build the status block. Stop/shutdown are only accepted while RUNNING; checkpoints and wait
/// hints only make sense for pending states.
pub fn status(state: u32, exit_code: u32, checkpoint: u32, wait_hint: u32) -> SERVICE_STATUS {
    SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: if state == SERVICE_RUNNING { SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN } else { 0 },
        dwWin32ExitCode: exit_code,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: if is_pending(state) { checkpoint } else { 0 },
        dwWaitHint: if is_pending(state) { wait_hint } else { 0 },
    }
}

fn report(state: u32, exit_code: u32, wait_hint: u32) {
    let checkpoint = if is_pending(state) { CHECKPOINT.fetch_add(1, Ordering::SeqCst) + 1 } else { 0 };
    STATE.store(state, Ordering::SeqCst);
    let h = STATUS_HANDLE.load(Ordering::SeqCst);
    if h != 0 {
        let st = status(state, exit_code, checkpoint, wait_hint);
        unsafe { SetServiceStatus(h as SERVICE_STATUS_HANDLE, &st) };
    }
}

unsafe extern "system" fn handler(control: u32, _event_type: u32, _data: *mut c_void, _ctx: *mut c_void) -> u32 {
    match classify(control) {
        Control::Stop => {
            governor::STOP.store(true, Ordering::SeqCst);
            // Cleanup (restore priorities, flush log) happens on the service thread; give it time.
            report(SERVICE_STOP_PENDING, 0, 15_000);
            NO_ERROR
        }
        Control::Interrogate => {
            report(STATE.load(Ordering::SeqCst), 0, 5_000);
            NO_ERROR
        }
        Control::Unsupported => ERROR_CALL_NOT_IMPLEMENTED,
    }
}

/// Best effort: a failed start has no console, so the log is the only place to say why.
fn log_failure(log_path: &Path, action: &str, info: &str) {
    if let Ok(mut l) = CsvLog::open(log_path, false, false) {
        l.event(&Event::system(id::SERVICE_ERROR, action, info));
        l.flush();
    }
}

/// Runs the whole service lifetime; returns the Win32 exit code to report.
fn run_service() -> u32 {
    let args: Vec<String> = std::env::args().collect();
    let p = paths::resolve(&args, true);
    report(SERVICE_START_PENDING, 0, 15_000);

    let _lock = match instance::acquire(instance::NAME) {
        Ok(g) => g,
        Err(e) => {
            let why = match e {
                instance::Error::AlreadyRunning => "another Process Corral instance (a console `run`?) is already running".to_string(),
                instance::Error::Os(c) => format!("cannot create the instance lock: {}", err_text(c)),
            };
            log_failure(&p.log, "Service refused to start", &why);
            return ERROR_SERVICE_ALREADY_RUNNING;
        }
    };
    let mut gov = match Governor::open(&p.config, &p.log, false) {
        Ok(g) => g.with_status(p.status.clone(), "service"),
        Err(OpenError::BadConfig(m)) => {
            log_failure(&p.log, "Service refused to start: invalid config", &format!("{}: {m}", p.config.display()));
            return ERROR_BAD_CONFIGURATION;
        }
        Err(OpenError::Log(_)) => return ERROR_OPEN_FAILED,
    };

    // A stop that arrived during startup must not be overwritten by RUNNING.
    if !governor::STOP.load(Ordering::SeqCst) {
        report(SERVICE_RUNNING, 0, 0);
    }
    let mut os = WinOs::new();
    match gov.run(&mut os, &governor::STOP, std::process::id(), true) {
        Ok(_) => NO_ERROR,
        Err(m) => {
            log_failure(&p.log, "Service stopped after an internal error", &m);
            ERROR_INTERNAL_ERROR
        }
    }
    // `gov` and `_lock` drop here: log closed, instance lock released, before STOPPED is reported.
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
    let name = wide(NAME);
    let h = unsafe { RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(handler), null()) };
    if h as usize == 0 {
        return;
    }
    STATUS_HANDLE.store(h as usize, Ordering::SeqCst);
    let code = std::panic::catch_unwind(run_service).unwrap_or(ERROR_INTERNAL_ERROR);
    report(SERVICE_STOPPED, code, 0);
}

/// Connects to the SCM and blocks until the service stops. Only works when launched by the SCM.
pub fn dispatch() -> i32 {
    let name = wide(NAME);
    let table = [
        SERVICE_TABLE_ENTRYW { lpServiceName: name.as_ptr() as *mut u16, lpServiceProc: Some(service_main) },
        SERVICE_TABLE_ENTRYW { lpServiceName: null_mut(), lpServiceProc: None },
    ];
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        let e = unsafe { GetLastError() };
        if e == ERROR_FAILED_SERVICE_CONTROLLER_CONNECT {
            eprintln!(
                "`service` is the entry point for the Service Control Manager and cannot be run from a console.\n\
                 For console mode use `process-corral run`; to run as a service use `install` then `start`."
            );
        } else {
            eprintln!("service: {}", err_text(e));
        }
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_and_shutdown_take_the_same_path() {
        assert_eq!(classify(SERVICE_CONTROL_STOP), Control::Stop);
        assert_eq!(classify(SERVICE_CONTROL_SHUTDOWN), Control::Stop);
        assert_eq!(classify(SERVICE_CONTROL_INTERROGATE), Control::Interrogate);
        assert_eq!(classify(SERVICE_CONTROL_PAUSE), Control::Unsupported);
    }

    #[test]
    fn only_running_accepts_stop_and_shutdown() {
        let running = status(SERVICE_RUNNING, 0, 0, 0);
        assert_eq!(running.dwControlsAccepted, SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN);
        for s in [SERVICE_START_PENDING, SERVICE_STOP_PENDING, SERVICE_STOPPED] {
            assert_eq!(status(s, 0, 1, 1000).dwControlsAccepted, 0, "state {s}");
        }
    }

    #[test]
    fn checkpoints_and_hints_only_for_pending_states() {
        let p = status(SERVICE_STOP_PENDING, 0, 3, 15_000);
        assert_eq!((p.dwCheckPoint, p.dwWaitHint), (3, 15_000));
        let r = status(SERVICE_RUNNING, 0, 3, 15_000);
        assert_eq!((r.dwCheckPoint, r.dwWaitHint), (0, 0));
        let s = status(SERVICE_STOPPED, ERROR_BAD_CONFIGURATION, 3, 15_000);
        assert_eq!((s.dwCheckPoint, s.dwWaitHint, s.dwWin32ExitCode), (0, 0, ERROR_BAD_CONFIGURATION));
        assert_eq!(s.dwServiceType, SERVICE_WIN32_OWN_PROCESS);
    }
}
