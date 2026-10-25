//! Run `process-corral.exe <args>` elevated via ShellExecute's `runas` verb and wait for it.

use process_corral::elevate::ExecOutcome;
use process_corral::scm::err_text;
use std::path::Path;
use std::time::Duration;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_CANCELLED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Shows the UAC prompt, waits up to `timeout` for the helper, and returns its exit code. The
/// helper is never killed on timeout. `params` must already be quoted (see `elevate::build_params`).
pub fn run_elevated(exe: &Path, params: &str, timeout: Duration) -> ExecOutcome {
    let verb = wide("runas");
    let file = wide(&exe.display().to_string());
    let params_w = wide(params);
    let dir = exe.parent().map(|d| wide(&d.display().to_string()));

    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params_w.as_ptr();
    info.lpDirectory = dir.as_ref().map_or(std::ptr::null(), |d| d.as_ptr());
    info.nShow = SW_HIDE;

    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let e = unsafe { GetLastError() };
        return if e == ERROR_CANCELLED { ExecOutcome::Cancelled } else { ExecOutcome::LaunchFailed(err_text(e)) };
    }
    let h = info.hProcess;
    if h.is_null() {
        return ExecOutcome::LaunchFailed("Windows did not return a process handle".into());
    }
    let ms = timeout.as_millis().min(u32::MAX as u128 - 1) as u32;
    let waited = unsafe { WaitForSingleObject(h, ms) };
    let outcome = match waited {
        WAIT_OBJECT_0 => {
            let mut code = 0u32;
            if unsafe { GetExitCodeProcess(h, &mut code) } != 0 {
                ExecOutcome::Exited(code as i32)
            } else {
                ExecOutcome::LaunchFailed(err_text(unsafe { GetLastError() }))
            }
        }
        WAIT_TIMEOUT => ExecOutcome::TimedOut(timeout.as_secs()),
        _ => ExecOutcome::LaunchFailed(err_text(unsafe { GetLastError() })),
    };
    unsafe { CloseHandle(h) };
    outcome
}
