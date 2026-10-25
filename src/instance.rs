//! Single-instance guard: a named kernel mutex shared by console `run` and the service, so the
//! two can never manage processes at the same time.

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;

/// `Global\` so a service in session 0 and a console in a user session see the same object.
pub const NAME: &str = "Global\\ProcessCorralGovernor";

#[derive(Debug, PartialEq)]
pub enum Error {
    AlreadyRunning,
    Os(u32),
}

/// Held for the life of the process; the mutex disappears when the last handle closes.
pub struct Guard(HANDLE);

impl Drop for Guard {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub fn acquire(name: &str) -> Result<Guard, Error> {
    let w: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    let h = unsafe { CreateMutexW(std::ptr::null(), 0, w.as_ptr()) };
    let err = unsafe { GetLastError() };
    if h.is_null() {
        // An instance owned by another account (e.g. the LocalSystem service seen from a
        // standard user's console) denies us access to its mutex rather than reporting "exists".
        return Err(if err == ERROR_ACCESS_DENIED { Error::AlreadyRunning } else { Error::Os(err) });
    }
    if err == ERROR_ALREADY_EXISTS {
        unsafe { CloseHandle(h) };
        return Err(Error::AlreadyRunning);
    }
    Ok(Guard(h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_is_refused_until_first_is_dropped() {
        let name = format!("Local\\pc-test-{}", std::process::id());
        let first = acquire(&name).expect("first acquire");
        assert_eq!(acquire(&name).err(), Some(Error::AlreadyRunning));
        assert_eq!(acquire(&name).err(), Some(Error::AlreadyRunning), "refusal doesn't release the lock");
        drop(first);
        assert!(acquire(&name).is_ok(), "free again after the holder exits");
    }

    #[test]
    fn different_names_do_not_conflict() {
        let a = acquire(&format!("Local\\pc-test-a-{}", std::process::id())).unwrap();
        let b = acquire(&format!("Local\\pc-test-b-{}", std::process::id()));
        assert!(b.is_ok());
        drop(a);
    }
}
