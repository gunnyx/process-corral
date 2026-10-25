//! OS abstraction. The engine only talks to `Os`, so it can be tested with a fake.
//! The Win32 calls used are documented ones: Toolhelp snapshots,
//! GetProcessTimes, Set/GetPriorityClass, Set/GetProcessAffinityMask, TerminateProcess.

use crate::guid::Guid;
use crate::topology::Topology;
use std::sync::Arc;

/// Win32 error codes the default trait methods report.
pub const ERR_ACCESS_DENIED: u32 = 5;
pub const ERR_NOT_IMPLEMENTED: u32 = 120;

/// A process affinity: a mask inside one processor group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Affinity {
    pub group: u16,
    pub mask: u64,
}

#[derive(Clone, Debug)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: u32,
    pub name: String,
}

pub trait Os {
    fn processes(&mut self) -> Vec<ProcInfo>;
    /// Cumulative kernel+user CPU time in 100ns units.
    fn cpu_time(&mut self, pid: u32) -> Option<u64>;
    /// Whole-system busy percentage since the previous call.
    fn system_cpu_pct(&mut self) -> f64;
    fn cpu_count(&self) -> u32;
    fn foreground_pid(&mut self) -> Option<u32>;
    fn is_session0(&mut self, pid: u32) -> bool;
    fn path(&mut self, pid: u32) -> String;
    fn priority(&mut self, pid: u32) -> Option<u32>;
    fn set_priority(&mut self, pid: u32, class: u32) -> bool;
    fn affinity(&mut self, pid: u32) -> Option<u64>;
    fn set_affinity(&mut self, pid: u32, mask: u64) -> bool;
    fn terminate(&mut self, pid: u32) -> bool;

    // ---- profile support. The defaults wrap the older methods so simple fakes keep working; the
    // real implementation reports genuine Win32 error codes and asks for the least access right
    // each call needs.

    fn topology(&mut self) -> Arc<Topology> {
        Arc::new(Topology::simple(self.cpu_count() as usize))
    }
    fn read_priority(&mut self, pid: u32) -> Result<u32, u32> {
        self.priority(pid).ok_or(ERR_ACCESS_DENIED)
    }
    fn apply_priority(&mut self, pid: u32, class: u32) -> Result<(), u32> {
        if self.set_priority(pid, class) { Ok(()) } else { Err(ERR_ACCESS_DENIED) }
    }
    fn read_affinity(&mut self, pid: u32) -> Result<Affinity, u32> {
        self.affinity(pid).map(|mask| Affinity { group: 0, mask }).ok_or(ERR_ACCESS_DENIED)
    }
    fn apply_affinity(&mut self, pid: u32, a: Affinity) -> Result<(), u32> {
        if self.set_affinity(pid, a.mask) { Ok(()) } else { Err(ERR_ACCESS_DENIED) }
    }
    /// Default CPU Set ids of the process; empty means none are set (it may run anywhere).
    fn read_cpu_sets(&mut self, _pid: u32) -> Result<Vec<u32>, u32> {
        Err(ERR_NOT_IMPLEMENTED)
    }
    /// An empty list clears the process's default CPU Sets.
    fn apply_cpu_sets(&mut self, _pid: u32, _ids: &[u32]) -> Result<(), u32> {
        Err(ERR_NOT_IMPLEMENTED)
    }
    /// Is this process running elevated (administrator or SYSTEM)? Decides how an access-denied
    /// refusal is worded: elevated and still refused means the target is protected.
    fn is_elevated(&self) -> bool {
        false
    }
    fn power_plan(&mut self) -> Result<Guid, u32> {
        Err(ERR_NOT_IMPLEMENTED)
    }
    fn set_power_plan(&mut self, _plan: &Guid) -> Result<(), u32> {
        Err(ERR_NOT_IMPLEMENTED)
    }
}

/// Plain-English text for a Win32 error code.
pub fn win32_message(code: u32) -> String {
    #[cfg(windows)]
    {
        crate::scm::err_text(code)
    }
    #[cfg(not(windows))]
    {
        format!("Win32 error {code}")
    }
}

#[cfg(windows)]
pub mod win {
    use super::*;
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
    use windows_sys::Win32::System::Threading::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

    pub struct WinOs {
        ncpu: u32,
        last_sys: Option<(u64, u64)>,
        topo: Option<Arc<Topology>>,
    }

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    fn open_err(pid: u32, access: u32) -> Result<Handle, u32> {
        let h = unsafe { OpenProcess(access, 0, pid) };
        if h.is_null() { Err(unsafe { GetLastError() }) } else { Ok(Handle(h)) }
    }

    fn open(pid: u32, access: u32) -> Option<Handle> {
        let h = unsafe { OpenProcess(access, 0, pid) };
        (!h.is_null()).then_some(Handle(h))
    }

    fn ft(f: FILETIME) -> u64 {
        ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
    }

    impl WinOs {
        pub fn new() -> Self {
            let mut si: SYSTEM_INFO = unsafe { std::mem::zeroed() };
            unsafe { GetSystemInfo(&mut si) };
            Self { ncpu: si.dwNumberOfProcessors.max(1), last_sys: None, topo: None }
        }
    }

    /// Is there a live process with this pid? Works from a standard user for processes of other
    /// accounts: "access denied" means the process exists. Only "no such process" (or an exited
    /// process whose handle is still held somewhere) counts as gone.
    pub fn pid_alive(pid: u32) -> bool {
        const STILL_ACTIVE: u32 = 259;
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if h.is_null() {
            return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
        }
        let mut code = 0u32;
        let ok = unsafe { GetExitCodeProcess(h, &mut code) };
        unsafe { CloseHandle(h) };
        ok == 0 || code == STILL_ACTIVE
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn own_pid_is_alive_and_an_exited_child_is_not() {
            assert!(pid_alive(std::process::id()));
            let mut child = std::process::Command::new("cmd").args(["/c", "exit", "0"]).spawn().unwrap();
            let pid = child.id();
            child.wait().unwrap();
            assert!(!pid_alive(pid));
            assert!(!pid_alive(0));
        }
    }

    impl Os for WinOs {
        fn processes(&mut self) -> Vec<ProcInfo> {
            let mut out = vec![];
            unsafe {
                let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
                if snap == INVALID_HANDLE_VALUE {
                    return out;
                }
                let _g = Handle(snap);
                let mut e: PROCESSENTRY32W = std::mem::zeroed();
                e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
                let mut ok = Process32FirstW(snap, &mut e);
                while ok != 0 {
                    let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                    out.push(ProcInfo {
                        pid: e.th32ProcessID,
                        ppid: e.th32ParentProcessID,
                        name: String::from_utf16_lossy(&e.szExeFile[..len]),
                    });
                    ok = Process32NextW(snap, &mut e);
                }
            }
            out
        }

        fn cpu_time(&mut self, pid: u32) -> Option<u64> {
            let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
            let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            let (mut c, mut e, mut k, mut u) = (z, z, z, z);
            (unsafe { GetProcessTimes(h.0, &mut c, &mut e, &mut k, &mut u) } != 0).then(|| ft(k) + ft(u))
        }

        fn system_cpu_pct(&mut self) -> f64 {
            let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            let (mut i, mut k, mut u) = (z, z, z);
            if unsafe { GetSystemTimes(&mut i, &mut k, &mut u) } == 0 {
                return 0.0;
            }
            let (idle, total) = (ft(i), ft(k) + ft(u)); // kernel time includes idle
            let pct = match self.last_sys {
                Some((pi, pt)) if total > pt => 100.0 * (1.0 - (idle - pi) as f64 / (total - pt) as f64),
                _ => 0.0,
            };
            self.last_sys = Some((idle, total));
            pct.clamp(0.0, 100.0)
        }

        fn cpu_count(&self) -> u32 {
            self.ncpu
        }

        fn foreground_pid(&mut self) -> Option<u32> {
            unsafe {
                let w = GetForegroundWindow();
                if w.is_null() {
                    return None;
                }
                let mut pid = 0;
                GetWindowThreadProcessId(w, &mut pid);
                (pid != 0).then_some(pid)
            }
        }

        fn is_session0(&mut self, pid: u32) -> bool {
            let mut sid = 1;
            unsafe { ProcessIdToSessionId(pid, &mut sid) != 0 && sid == 0 }
        }

        fn path(&mut self, pid: u32) -> String {
            let Some(h) = open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else { return String::new() };
            let mut buf = [0u16; 1024];
            let mut n = buf.len() as u32;
            if unsafe { QueryFullProcessImageNameW(h.0, 0, buf.as_mut_ptr(), &mut n) } == 0 {
                return String::new();
            }
            String::from_utf16_lossy(&buf[..n as usize])
        }

        fn priority(&mut self, pid: u32) -> Option<u32> {
            let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
            let c = unsafe { GetPriorityClass(h.0) };
            (c != 0).then_some(c)
        }

        fn set_priority(&mut self, pid: u32, class: u32) -> bool {
            open(pid, PROCESS_SET_INFORMATION).is_some_and(|h| unsafe { SetPriorityClass(h.0, class) } != 0)
        }

        fn affinity(&mut self, pid: u32) -> Option<u64> {
            let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
            let (mut p, mut s) = (0usize, 0usize);
            (unsafe { GetProcessAffinityMask(h.0, &mut p, &mut s) } != 0).then_some(p as u64)
        }

        fn set_affinity(&mut self, pid: u32, mask: u64) -> bool {
            open(pid, PROCESS_SET_INFORMATION)
                .is_some_and(|h| unsafe { SetProcessAffinityMask(h.0, mask as usize) } != 0)
        }

        fn terminate(&mut self, pid: u32) -> bool {
            open(pid, PROCESS_TERMINATE).is_some_and(|h| unsafe { TerminateProcess(h.0, 1) } != 0)
        }

        fn is_elevated(&self) -> bool {
            crate::scm::is_elevated()
        }

        fn topology(&mut self) -> Arc<Topology> {
            let n = self.ncpu as usize;
            self.topo.get_or_insert_with(|| Arc::new(crate::topology::detect().unwrap_or_else(|_| Topology::simple(n)))).clone()
        }

        fn read_priority(&mut self, pid: u32) -> Result<u32, u32> {
            let h = open_err(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
            let c = unsafe { GetPriorityClass(h.0) };
            if c == 0 { Err(unsafe { GetLastError() }) } else { Ok(c) }
        }

        fn apply_priority(&mut self, pid: u32, class: u32) -> Result<(), u32> {
            let h = open_err(pid, PROCESS_SET_INFORMATION)?;
            if unsafe { SetPriorityClass(h.0, class) } == 0 { Err(unsafe { GetLastError() }) } else { Ok(()) }
        }

        fn read_affinity(&mut self, pid: u32) -> Result<Affinity, u32> {
            let h = open_err(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
            let (mut p, mut s) = (0usize, 0usize);
            if unsafe { GetProcessAffinityMask(h.0, &mut p, &mut s) } == 0 {
                return Err(unsafe { GetLastError() });
            }
            // The processor group is best effort: reading it may need more rights than we ask for.
            let (mut count, mut groups) = (4u16, [0u16; 4]);
            let group = if unsafe { GetProcessGroupAffinity(h.0, &mut count, groups.as_mut_ptr()) } != 0 && count == 1 { groups[0] } else { 0 };
            Ok(Affinity { group, mask: p as u64 })
        }

        fn apply_affinity(&mut self, pid: u32, a: Affinity) -> Result<(), u32> {
            let h = open_err(pid, PROCESS_SET_INFORMATION)?;
            if unsafe { SetProcessAffinityMask(h.0, a.mask as usize) } == 0 { Err(unsafe { GetLastError() }) } else { Ok(()) }
        }

        fn read_cpu_sets(&mut self, pid: u32) -> Result<Vec<u32>, u32> {
            let h = open_err(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
            let mut required = 0u32;
            if unsafe { GetProcessDefaultCpuSets(h.0, std::ptr::null_mut(), 0, &mut required) } != 0 {
                return Ok(vec![]); // succeeded with room for zero ids: none are set
            }
            let e = unsafe { GetLastError() };
            if e != ERROR_INSUFFICIENT_BUFFER {
                return Err(e);
            }
            let mut ids = vec![0u32; required as usize];
            let mut got = 0u32;
            if unsafe { GetProcessDefaultCpuSets(h.0, ids.as_mut_ptr(), required, &mut got) } == 0 {
                return Err(unsafe { GetLastError() });
            }
            ids.truncate(got as usize);
            ids.sort_unstable();
            Ok(ids)
        }

        fn apply_cpu_sets(&mut self, pid: u32, ids: &[u32]) -> Result<(), u32> {
            let h = open_err(pid, PROCESS_SET_LIMITED_INFORMATION)?;
            let ptr = if ids.is_empty() { std::ptr::null() } else { ids.as_ptr() };
            if unsafe { SetProcessDefaultCpuSets(h.0, ptr, ids.len() as u32) } == 0 { Err(unsafe { GetLastError() }) } else { Ok(()) }
        }

        fn power_plan(&mut self) -> Result<Guid, u32> {
            use windows_sys::Win32::System::Power::PowerGetActiveScheme;
            let mut p: *mut windows_sys::core::GUID = std::ptr::null_mut();
            let r = unsafe { PowerGetActiveScheme(std::ptr::null_mut(), &mut p) };
            if r != 0 || p.is_null() {
                return Err(if r != 0 { r } else { ERROR_INVALID_DATA });
            }
            let g = unsafe { *p };
            unsafe { LocalFree(p as *mut _) };
            Ok(Guid { data1: g.data1, data2: g.data2, data3: g.data3, data4: g.data4 })
        }

        fn set_power_plan(&mut self, plan: &Guid) -> Result<(), u32> {
            use windows_sys::Win32::System::Power::PowerSetActiveScheme;
            let g = windows_sys::core::GUID { data1: plan.data1, data2: plan.data2, data3: plan.data3, data4: plan.data4 };
            match unsafe { PowerSetActiveScheme(std::ptr::null_mut(), &g) } {
                0 => Ok(()),
                e => Err(e),
            }
        }
    }
}
