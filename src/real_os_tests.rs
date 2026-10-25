//! Opt-in checks against the real operating system: `cargo test --lib -- --ignored real_`.
//!
//! They launch a real `notepad.exe` that *we* started, run the real engine with the real Win32
//! layer over it, and verify every change independently through PowerShell's `Get-Process`.
//! The engine is wrapped in `OnlyThese`, which hides every process except the ones we launched, so
//! a profile matching `notepad.exe` can never touch a Notepad the user has open.

use crate::config::{Config, ABOVE_NORMAL, BELOW_NORMAL, NORMAL};
use crate::engine::Engine;
use crate::guid::Guid;
use crate::log::{id, Event, Sink};
use crate::os::win::WinOs;
use crate::os::{Affinity, Os, ProcInfo};
use crate::topology::Topology;
use std::collections::HashSet;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Delegates to the real OS but only ever shows (and so only ever lets the engine touch) `allowed`.
struct OnlyThese {
    inner: WinOs,
    allowed: HashSet<u32>,
    /// System CPU % reported to the engine. 0 keeps Restraint out of the profile test; the Restraint
    /// test reports a fixed high value so it acts whatever the machine is really doing.
    fake_sys: f64,
}

impl Os for OnlyThese {
    fn processes(&mut self) -> Vec<ProcInfo> {
        self.inner.processes().into_iter().filter(|p| self.allowed.contains(&p.pid)).collect()
    }
    fn cpu_time(&mut self, pid: u32) -> Option<u64> {
        self.inner.cpu_time(pid)
    }
    fn system_cpu_pct(&mut self) -> f64 {
        self.fake_sys
    }
    fn is_elevated(&self) -> bool {
        self.inner.is_elevated()
    }
    fn cpu_count(&self) -> u32 {
        self.inner.cpu_count()
    }
    fn foreground_pid(&mut self) -> Option<u32> {
        None
    }
    fn is_session0(&mut self, pid: u32) -> bool {
        self.inner.is_session0(pid)
    }
    fn path(&mut self, pid: u32) -> String {
        self.inner.path(pid)
    }
    fn priority(&mut self, pid: u32) -> Option<u32> {
        self.inner.priority(pid)
    }
    fn set_priority(&mut self, pid: u32, c: u32) -> bool {
        self.inner.set_priority(pid, c)
    }
    fn affinity(&mut self, pid: u32) -> Option<u64> {
        self.inner.affinity(pid)
    }
    fn set_affinity(&mut self, pid: u32, m: u64) -> bool {
        self.inner.set_affinity(pid, m)
    }
    fn terminate(&mut self, _pid: u32) -> bool {
        false
    }
    fn topology(&mut self) -> Arc<Topology> {
        self.inner.topology()
    }
    fn read_priority(&mut self, pid: u32) -> Result<u32, u32> {
        self.inner.read_priority(pid)
    }
    fn apply_priority(&mut self, pid: u32, c: u32) -> Result<(), u32> {
        self.inner.apply_priority(pid, c)
    }
    fn read_affinity(&mut self, pid: u32) -> Result<Affinity, u32> {
        self.inner.read_affinity(pid)
    }
    fn apply_affinity(&mut self, pid: u32, a: Affinity) -> Result<(), u32> {
        self.inner.apply_affinity(pid, a)
    }
    fn read_cpu_sets(&mut self, pid: u32) -> Result<Vec<u32>, u32> {
        self.inner.read_cpu_sets(pid)
    }
    fn apply_cpu_sets(&mut self, pid: u32, ids: &[u32]) -> Result<(), u32> {
        self.inner.apply_cpu_sets(pid, ids)
    }
    fn power_plan(&mut self) -> Result<Guid, u32> {
        self.inner.power_plan()
    }
    fn set_power_plan(&mut self, p: &Guid) -> Result<(), u32> {
        self.inner.set_power_plan(p)
    }
}

#[derive(Default)]
struct Rec(Vec<Event>);
impl Sink for Rec {
    fn event(&mut self, e: &Event) {
        println!("  log: [{}] {} (pid {}) - {}", e.id, e.action, e.pid, e.info);
        self.0.push(e.clone());
    }
}

fn ps(expr: &str) -> String {
    let out = Command::new("powershell").args(["-NoProfile", "-Command", expr]).output().expect("powershell");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn notepad_pids() -> HashSet<u32> {
    ps("(Get-Process -Name notepad -ErrorAction SilentlyContinue).Id -join ' '").split_whitespace().filter_map(|s| s.parse().ok()).collect()
}

/// Independent read-back, not through our own Win32 layer.
fn os_priority(pid: u32) -> String {
    ps(&format!("(Get-Process -Id {pid}).PriorityClass"))
}
fn os_affinity(pid: u32) -> u64 {
    ps(&format!("(Get-Process -Id {pid}).ProcessorAffinity.ToInt64()")).parse().unwrap_or(0)
}

struct Cleanup(Vec<u32>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for pid in &self.0 {
            // Only the processes this test launched.
            let _ = ps(&format!("Stop-Process -Id {pid} -Force -ErrorAction SilentlyContinue"));
        }
    }
}

fn cfg(text: &str) -> Config {
    Config::from_bytes(text.as_bytes()).unwrap_or_else(|e| panic!("{e}\n{text}"))
}

#[test]
#[ignore = "launches notepad.exe and changes its priority/affinity"]
fn real_notepad_profile_apply_reload_remove_restore() {
    let before = notepad_pids();
    let mut child = Command::new("notepad.exe").spawn().expect("launch notepad");
    let deadline = Instant::now() + Duration::from_secs(15);
    let mine: Vec<u32> = loop {
        let new: Vec<u32> = notepad_pids().difference(&before).copied().collect();
        if !new.is_empty() || Instant::now() > deadline {
            break new;
        }
        std::thread::sleep(Duration::from_millis(300));
    };
    let _cleanup = Cleanup(mine.clone());
    let _ = child.try_wait();
    assert!(!mine.is_empty(), "notepad did not start");
    let pid = mine[0];
    println!("launched notepad.exe pid(s) {mine:?}; the engine will only see {mine:?}");

    let mut os = OnlyThese { inner: WinOs::new(), allowed: mine.iter().copied().collect(), fake_sys: 0.0 };
    let topo = os.topology();
    println!("topology: {} logical processors, {} cores", topo.logical_count(), topo.core_count());
    let n = topo.logical_count();
    assert!(n >= 2, "need at least 2 logical processors for this test");
    let all: u64 = if n >= 64 { u64::MAX } else { (1u64 << n) - 1 };

    let (mut eng, mut log) = (Engine::new(std::process::id()), Rec::default());
    let start = Instant::now();
    let tick = |eng: &mut Engine, os: &mut OnlyThese, log: &mut Rec, c: &Config| {
        for _ in 0..3 {
            std::thread::sleep(Duration::from_millis(250));
            eng.tick(os, c, log, start.elapsed().as_millis() as u64);
        }
    };

    // Baseline as the OS reports it.
    let (p0, a0) = (os_priority(pid), os_affinity(pid));
    println!("baseline: priority {p0}, affinity {a0:#x}");
    assert_eq!(p0, "Normal");
    assert_eq!(a0, all);

    // 1) A profile for notepad.exe: small affinity mask and a lower priority.
    println!("--- apply");
    let c1 = cfg("[Profile.Notepad]\nMatch=notepad.exe\nPriority=belownormal\nAffinity=0x3\n");
    tick(&mut eng, &mut os, &mut log, &c1);
    assert_eq!(os_priority(pid), "BelowNormal", "priority as the OS sees it");
    assert_eq!(os_affinity(pid), 0x3, "affinity as the OS sees it");
    assert_eq!(os.inner.read_priority(pid), Ok(BELOW_NORMAL));
    assert_eq!(os.inner.read_affinity(pid).unwrap().mask, 0x3);
    let rep = eng.profile_report(&mut os, &c1);
    let ch = &rep.applied[0].changes;
    assert!(ch.iter().all(|c| c.result == "applied"), "{ch:?}");
    let aff = ch.iter().find(|c| c.aspect == "affinity").unwrap();
    assert_eq!((aff.original.as_str(), aff.current.as_str()), (format!("{all:#X}").as_str(), "0x3"));
    assert!(log.0.iter().any(|e| e.id == id::PROFILE_APPLIED));

    // 2) Reload with different values.
    println!("--- change on reload");
    let c2 = cfg("[Profile.Notepad]\nMatch=notepad.exe\nPriority=abovenormal\nAffinity=0x1\n");
    tick(&mut eng, &mut os, &mut log, &c2);
    assert_eq!(os_priority(pid), "AboveNormal");
    assert_eq!(os_affinity(pid), 0x1);
    assert_eq!(os.inner.read_priority(pid), Ok(ABOVE_NORMAL));

    // 3) CPU Sets through the real API: apply, read back, and clear again.
    println!("--- CPU sets");
    let c3 = cfg("[Profile.Notepad]\nMatch=notepad.exe\nCpuSets=0-1\n");
    tick(&mut eng, &mut os, &mut log, &c3);
    let ids = os.inner.read_cpu_sets(pid).expect("read CPU sets");
    println!("  CPU Set ids now: {ids:?}");
    assert_eq!(ids.len(), 2);
    assert_eq!(os_priority(pid), "Normal", "the earlier profile values were restored when they were dropped");
    assert_eq!(os_affinity(pid), all);

    // 4) Remove the profile: everything goes back.
    println!("--- remove profile");
    let none = cfg("[Other]\nx=1\n");
    tick(&mut eng, &mut os, &mut log, &none);
    assert_eq!(os_priority(pid), "Normal");
    assert_eq!(os_affinity(pid), all);
    assert!(os.inner.read_cpu_sets(pid).unwrap().is_empty(), "CPU Sets cleared");
    assert!(log.0.iter().any(|e| e.id == id::PROFILE_RESTORED));

    // 5) Re-apply, then a clean engine stop restores it as well.
    println!("--- engine stop");
    tick(&mut eng, &mut os, &mut log, &c1);
    assert_eq!(os_affinity(pid), 0x3);
    eng.release_all(&mut os, &mut log);
    assert_eq!(os_priority(pid), "Normal");
    assert_eq!(os_affinity(pid), all);
    assert_eq!(NORMAL, os.inner.read_priority(pid).unwrap());
}
/// Needs two scratch programs (built outside the repo): `pc_protected` burns a core after adding a deny
/// ACE for PROCESS_SET_INFORMATION to its own process object, so priority changes from outside get
/// "access denied" like a protected game; `pc_hog` just burns a core. Point `PC_PROTECTED_EXE` and
/// `PC_HOG_EXE` at them. The test launches both itself and the engine can only see those two pids.
#[test]
#[ignore = "launches two scratch busy-loop programs (PC_PROTECTED_EXE, PC_HOG_EXE) and lowers one's priority"]
fn real_restraint_access_denied_is_logged_once_and_a_normal_hog_is_still_restrained() {
    let protected_exe = std::env::var("PC_PROTECTED_EXE").expect("set PC_PROTECTED_EXE");
    let hog_exe = std::env::var("PC_HOG_EXE").expect("set PC_HOG_EXE");
    let mut p = Command::new(&protected_exe).arg("protect").stdout(std::process::Stdio::null()).spawn().expect("launch protected");
    let mut h = Command::new(&hog_exe).stdout(std::process::Stdio::null()).spawn().expect("launch hog");
    let (ppid, hpid) = (p.id(), h.id());
    // Kill through the handles we got from launching them (a protected process refuses Stop-Process).
    struct KillOnDrop(Vec<std::process::Child>);
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            for c in &mut self.0 {
                let _ = c.kill();
            }
        }
    }
    let mut guard = KillOnDrop(vec![]);
    println!("launched protected pid {ppid}, hog pid {hpid}; the engine will only see those two");
    std::thread::sleep(Duration::from_millis(1500));
    assert!(p.try_wait().unwrap().is_none() && h.try_wait().unwrap().is_none(), "scratch programs are running");
    guard.0.extend([p, h]);

    // Windows really does refuse, independent of the engine.
    let mut os = OnlyThese { inner: WinOs::new(), allowed: [ppid, hpid].into_iter().collect(), fake_sys: 80.0 };
    assert_eq!(os.inner.apply_priority(ppid, BELOW_NORMAL), Err(5), "the protected scratch process refuses a priority change");
    assert_eq!(os_priority(ppid), "Normal");
    println!("elevated engine: {}", os.is_elevated());

    let c = cfg("[OutOfControlProcessRestraint]\nOocOn=true\nExcludeChildrenOfForeground=false\nExcludeForegroundProcesses2=false\nExcludeServices=false\n\
                 TotalProcessorUsageBeforeRestraint=5\nPerProcessUsageBeforeRestraint=5\nTimeOverQuotaBeforeRestraint=900\nMinimumTimeOfRestraint=4200\n");
    let (mut eng, mut log) = (Engine::new(std::process::id()), Rec::default());
    let start = Instant::now();
    for _ in 0..25 {
        std::thread::sleep(Duration::from_millis(1000));
        eng.tick(&mut os, &c, &mut log, start.elapsed().as_millis() as u64);
    }

    let denied: Vec<_> = log.0.iter().filter(|e| e.id == id::PB_ERROR).collect();
    println!("access-denied log lines over 25 ticks: {}", denied.len());
    assert_eq!(denied.len(), 1, "exactly one line for the protected process, not one per tick");
    assert_eq!(denied[0].pid, ppid);
    assert_eq!(os_priority(ppid), "Normal", "never lowered");
    let u = eng.untouchable();
    assert_eq!(u.len(), 1);
    assert_eq!((u[0].pid, u[0].name.to_lowercase()), (ppid, std::path::Path::new(&protected_exe).file_name().unwrap().to_string_lossy().to_lowercase()));
    assert!(u[0].reason.contains(if os.is_elevated() { "protected" } else { "not running elevated" }), "{}", u[0].reason);

    // The ordinary hog was restrained as usual, and verified from outside.
    assert!(log.0.iter().any(|e| e.pid == hpid && e.id == id::PB_BEGIN), "hog was restrained");
    let hog_class_during = os_priority(hpid);
    println!("hog priority as the OS sees it right now: {hog_class_during}");
    eng.release_all(&mut os, &mut log);
    assert_eq!(os_priority(hpid), "Normal", "restored on stop");
    assert_eq!(os_priority(ppid), "Normal");
}
