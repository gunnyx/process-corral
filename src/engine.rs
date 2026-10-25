//! Governor engine: Restraint, default priorities/affinities, disallowed processes,
//! instance limits. One `tick` per update interval.
//!
//! The config is passed to every tick, so a reload just means passing the new one. Restraint
//! state remembers what *we* changed (original class, class we set, original affinity), and
//! every release path restores from that record rather than from the current config, so a
//! reload can never strand a process at a lowered priority or restricted affinity.

use crate::config::*;
use crate::log::{id, Event, Sink};
use crate::os::{win32_message, Os, ProcInfo, ERR_ACCESS_DENIED};
use crate::profiles::{Bridge, ProfileManager, ProfileReport};
use std::collections::{HashMap, HashSet};

/// Processes Restraint never touches (a conventional hard-coded list of core Windows processes).
pub(crate) const HARDCODED_EXCLUSIONS: &[&str] = &[
    "csrss.exe", "dwm.exe", "explorer.exe", "lsass.exe", "lsm.exe", "services.exe", "sihost.exe",
    "smss.exe", "svchost.exe", "wininit.exe", "winlogon.exe", "logonui.exe", "wmiprvse.exe",
    "runtimebroker.exe", "ehshell.exe", "taskeng.exe", "wudfhost.exe", "mfpmp.exe",
];

struct Restraint {
    orig_class: u32,
    set_class: u32,
    orig_affinity: Option<u64>,
    since_ms: u64,
    /// The process's CPU% when it was restrained (for display).
    pct: f64,
}

/// A currently restrained process, as reported to the status file.
#[derive(Clone, Debug)]
pub struct RestrainedInfo {
    pub pid: u32,
    pub name: String,
    pub original_class: u32,
    pub set_class: u32,
    /// Engine clock (ms since the governor loop started).
    pub since_ms: u64,
    pub cpu_pct: f64,
}

/// A process Windows refused to let Restraint lower (protected, e.g. by anti-cheat). It is left alone
/// until it exits or the rules that apply to it change.
struct Untouchable {
    name: String,
    reason: String,
    /// The Restraint settings and profile match in force when it was refused; a change retries.
    rules: String,
}

/// An untouchable process, as reported to the status file.
#[derive(Clone, Debug, PartialEq)]
pub struct UntouchableInfo {
    pub pid: u32,
    pub name: String,
    pub reason: String,
}

struct Tracked {
    info: ProcInfo,
    seq: u64,
    last_cpu: Option<u64>,
    pct: f64,
    over_ms: u64,
    restraint: Option<Restraint>,
    /// Why this process is currently exempt from restraint (so we log it once, not every tick).
    exempt: Option<String>,
}

#[derive(Default)]
pub struct Stats {
    pub restraints_total: u64,
    pub sys_cpu: f64,
    pub restrained_now: usize,
}

pub struct Engine {
    tracked: HashMap<u32, Tracked>,
    last_ms: Option<u64>,
    seq: u64,
    own_pid: u32,
    pub stats: Stats,
    profiles: ProfileManager,
    untouchable: HashMap<u32, Untouchable>,
}

/// Lets the profile manager see and adjust the restraints.
struct EngineBridge<'a> {
    tracked: &'a mut HashMap<u32, Tracked>,
}

impl Bridge for EngineBridge<'_> {
    fn restraint_original(&self, pid: u32) -> Option<u32> {
        self.tracked.get(&pid).and_then(|t| t.restraint.as_ref()).map(|r| r.orig_class)
    }
    fn retarget(&mut self, pid: u32, class: u32) -> bool {
        match self.tracked.get_mut(&pid).and_then(|t| t.restraint.as_mut()) {
            Some(r) => {
                r.orig_class = class;
                true
            }
            None => false,
        }
    }
}

/// Put back only what we changed, and only if nobody else changed it since.
fn restore(os: &mut dyn Os, pid: u32, r: &Restraint) {
    if os.priority(pid) == Some(r.set_class) {
        os.set_priority(pid, r.orig_class);
    }
    if let Some(a) = r.orig_affinity {
        os.set_affinity(pid, a);
    }
}

/// `Ok(current class)` if the process may be restrained, `Err(reason)` naming the rule that exempts it.
fn check_exempt(
    os: &mut dyn Os,
    pb: &RestraintSettings,
    info: &ProcInfo,
    fg: Option<u32>,
    names: &HashMap<u32, String>,
    profile_reason: Option<String>,
) -> Result<u32, String> {
    if let Some(r) = profile_reason {
        return Err(r);
    }
    if pb.exclude_foreground && fg == Some(info.pid) {
        return Err("is the foreground window (ExcludeForegroundProcesses2)".into());
    }
    if HARDCODED_EXCLUSIONS.contains(&info.name.to_ascii_lowercase().as_str()) {
        return Err("system process on the built-in exclusion list".into());
    }
    if let Some(g) = pb.exclusions.iter().find(|g| glob(g, &info.name)) {
        return Err(format!("matches OocExclusions pattern '{g}'"));
    }
    if pb.exclude_children_of_foreground {
        if let Some(f) = fg.filter(|f| *f == info.ppid) {
            let who = names.get(&f).map(|n| format!("{n} (pid {f})")).unwrap_or_else(|| format!("pid {f}"));
            return Err(format!("child of foreground app {who} (ExcludeChildrenOfForeground)"));
        }
    }
    if pb.exclude_services && os.is_session0(info.pid) {
        return Err("service running in session 0 (ExcludeServices)".into());
    }
    let Some(class) = os.priority(info.pid) else {
        return Err("priority class could not be read (access denied)".into());
    };
    if pb.tame_only_normal && class != NORMAL {
        return Err(format!("priority class is {}, not normal (TameOnlyNormal)", class_name(class)));
    }
    Ok(class)
}

impl Engine {
    pub fn new(own_pid: u32) -> Self {
        Self { tracked: HashMap::new(), last_ms: None, seq: 0, own_pid, stats: Stats::default(), profiles: ProfileManager::default(), untouchable: HashMap::new() }
    }

    pub fn tick(&mut self, os: &mut dyn Os, cfg: &Config, log: &mut dyn Sink, now_ms: u64) {
        let dt_ms = self.last_ms.map(|l| now_ms.saturating_sub(l)).unwrap_or(0);
        self.last_ms = Some(now_ms);
        let ncpu = os.cpu_count() as f64;
        let sys = os.system_cpu_pct();
        let fg = os.foreground_pid();
        let procs = os.processes();
        let alive: HashSet<u32> = procs.iter().map(|p| p.pid).collect();
        let names: HashMap<u32, String> = procs.iter().map(|p| (p.pid, p.name.clone())).collect();

        // Processes that exited: close out any open restraint.
        let gone: Vec<u32> = self.tracked.keys().filter(|p| !alive.contains(p)).copied().collect();
        for pid in gone {
            self.untouchable.remove(&pid);
            let t = self.tracked.remove(&pid).unwrap();
            if t.restraint.is_some() {
                emit(log, &t.info, "", id::PB_END_TERMINATED, "Restraint ended due to termination.", "This process exited while restrained.");
            }
        }

        for p in &procs {
            if p.pid == 0 || p.pid == 4 || p.pid == self.own_pid {
                continue;
            }
            let is_new = !self.tracked.contains_key(&p.pid);
            if is_new {
                self.seq += 1;
                self.tracked.insert(
                    p.pid,
                    Tracked { info: p.clone(), seq: self.seq, last_cpu: None, pct: 0.0, over_ms: 0, restraint: None, exempt: None },
                );
                if self.on_new_process(os, cfg, log, p) {
                    self.tracked.remove(&p.pid);
                    continue;
                }
            }
            let cpu = os.cpu_time(p.pid);
            let t = self.tracked.get_mut(&p.pid).unwrap();
            if let (Some(prev), Some(now), true) = (t.last_cpu, cpu, dt_ms > 0) {
                t.pct = 100.0 * now.saturating_sub(prev) as f64 / (dt_ms as f64 * 10_000.0 * ncpu);
            }
            t.last_cpu = cpu.or(t.last_cpu);
        }

        // Per-application profiles run before Restraint so its exemptions see the current matches.
        {
            let mut bridge = EngineBridge { tracked: &mut self.tracked };
            self.profiles.tick(os, cfg, log, &procs, self.own_pid, now_ms, &mut bridge);
        }

        if cfg.restraint.enabled {
            self.restrain_pass(os, cfg, log, now_ms, dt_ms, sys, fg, &names);
        } else {
            // Turned off (possibly by a live reload): nothing may stay restrained.
            self.release_restrained(os, log, "restraint is disabled in the config");
        }
        self.enforce_instance_limits(os, cfg, log);

        self.stats.sys_cpu = sys;
        self.stats.restrained_now = self.tracked.values().filter(|t| t.restraint.is_some()).count();
    }

    /// Returns true when the process was terminated.
    fn on_new_process(&mut self, os: &mut dyn Os, cfg: &Config, log: &mut dyn Sink, p: &ProcInfo) -> bool {
        // A matching profile wins over the older lists. Say so once (this runs once per new pid).
        if let Some(pr) = self.profiles.find_match(os, cfg, p) {
            let mut lists = vec![];
            if cfg.default_priorities.iter().any(|(g, _)| glob(g, &p.name)) {
                lists.push("DefaultPriorities");
            }
            if cfg.default_affinities.iter().any(|(g, _)| glob(g, &p.name)) {
                lists.push("DefaultAffinities");
            }
            if cfg.disallowed.iter().any(|g| glob(g, &p.name)) {
                lists.push("DisallowedProcesses");
            }
            if !lists.is_empty() {
                emit(
                    log,
                    p,
                    "",
                    id::PROFILE_OVERRIDES,
                    "Profile takes precedence over the older lists",
                    &format!("profile '{}' wins over {} for this process", pr.name, lists.join(", ")),
                );
            }
            return false;
        }
        if cfg.disallowed.iter().any(|g| glob(g, &p.name)) {
            let ok = os.terminate(p.pid);
            let path = os.path(p.pid);
            emit(log, p, &path, id::DISALLOWED,
                if ok { "Disallowed process terminated" } else { "Failed to terminate disallowed process" },
                "Matched DisallowedProcesses.");
            return ok;
        }
        if let Some((_, class)) = cfg.default_priorities.iter().find(|(g, _)| glob(g, &p.name)) {
            if os.set_priority(p.pid, *class) {
                let path = os.path(p.pid);
                emit(log, p, &path, id::DEFAULT_PRIORITY, "Default priority class applied", class_name(*class));
            }
        }
        if let Some((_, mask)) = cfg.default_affinities.iter().find(|(g, _)| glob(g, &p.name)) {
            if os.set_affinity(p.pid, *mask) {
                let path = os.path(p.pid);
                emit(log, p, &path, id::DEFAULT_AFFINITY, "Default CPU affinity applied", &format!("0x{mask:x}"));
            }
        }
        false
    }

    fn enforce_instance_limits(&mut self, os: &mut dyn Os, cfg: &Config, log: &mut dyn Sink) {
        for (pattern, max) in &cfg.instance_limits {
            let mut group: Vec<(u64, u32)> = self
                .tracked
                .iter()
                .filter(|(_, t)| glob(pattern, &t.info.name))
                .map(|(pid, t)| (t.seq, *pid))
                .collect();
            group.sort();
            for (_, pid) in group.into_iter().skip(*max) {
                let info = self.tracked[&pid].info.clone();
                if os.terminate(pid) {
                    emit(log, &info, &os.path(pid), id::INSTANCE_LIMIT, "Instance limit exceeded; process terminated",
                        &format!("limit {max}"));
                    self.tracked.remove(&pid);
                }
            }
        }
    }

    /// End one restraint, restoring from the record of what we changed.
    fn end_restraint(&mut self, os: &mut dyn Os, log: &mut dyn Sink, pid: u32, aid: u32, msg: &str) {
        let Some(t) = self.tracked.get_mut(&pid) else { return };
        let Some(r) = t.restraint.take() else { return };
        t.over_ms = 0;
        t.exempt = None;
        restore(os, pid, &r);
        let info = t.info.clone();
        emit(log, &info, &os.path(pid), aid, msg, &format!("restored to {}", class_name(r.orig_class)));
    }

    #[allow(clippy::too_many_arguments)]
    fn restrain_pass(
        &mut self,
        os: &mut dyn Os,
        cfg: &Config,
        log: &mut dyn Sink,
        now_ms: u64,
        dt_ms: u64,
        sys: f64,
        fg: Option<u32>,
        names: &HashMap<u32, String>,
    ) {
        let pb = &cfg.restraint;
        let pids: Vec<u32> = self.tracked.keys().copied().collect();
        for pid in pids {
            // Refused earlier as protected: leave it alone unless it is a different process now or the
            // rules that apply to it changed (then it gets one fresh attempt).
            if let Some(u) = self.untouchable.get(&pid) {
                if names.get(&pid) == Some(&u.name) && u.rules == self.rules_for(cfg, pid) {
                    continue;
                }
                self.untouchable.remove(&pid);
            }
            let (info, pct, since, over_ms) = {
                let t = &self.tracked[&pid];
                (t.info.clone(), t.pct, t.restraint.as_ref().map(|r| r.since_ms), t.over_ms)
            };

            if let Some(since) = since {
                let elapsed = now_ms.saturating_sub(since);
                let profile_ex = self.profile_exemption(pid);
                let ending: Option<(u32, String)> = if let Some(r) = profile_ex {
                    Some((id::PB_END_EXCLUDED, format!("Restraint ended: {r}")))
                } else if let Some(g) = pb.exclusions.iter().find(|g| glob(g, &info.name)) {
                    Some((id::PB_END_EXCLUDED, format!("Restraint ended: process now matches OocExclusions pattern '{g}'")))
                } else if fg == Some(pid) {
                    Some((id::PB_END_FOREGROUND, "Restraint ended: process is now the foreground window".into()))
                } else if pct < pb.per_process_for_restore && elapsed >= pb.min_restraint_ms {
                    Some((id::PB_END_RESTORED, "Restraint ended: process usage dropped; priority restored".into()))
                } else if pb.max_restraint_ms > 0 && elapsed >= pb.max_restraint_ms {
                    Some((id::PB_END_TIMEOUT, "Restraint ended: maximum restraint time reached".into()))
                } else {
                    None
                };
                if let Some((aid, msg)) = ending {
                    self.end_restraint(os, log, pid, aid, &msg);
                }
                continue;
            }

            let over = sys > pb.total_cpu_before_restraint && pct > pb.per_process_before_restraint;
            if !over {
                let t = self.tracked.get_mut(&pid).unwrap();
                t.over_ms = 0;
                t.exempt = None;
                continue;
            }
            let over_ms = over_ms + dt_ms;
            self.tracked.get_mut(&pid).unwrap().over_ms = over_ms;
            if over_ms < pb.time_over_quota_ms {
                continue;
            }

            let profile_ex = self.profile_exemption(pid);
            let orig_class = match check_exempt(os, pb, &info, fg, names, profile_ex) {
                Ok(c) => c,
                Err(reason) => {
                    let t = self.tracked.get_mut(&pid).unwrap();
                    if t.exempt.as_deref() != Some(reason.as_str()) {
                        emit(log, &info, &os.path(pid), id::PB_EXEMPT, "Restraint skipped process",
                            &format!("{pct:.1}% CPU but exempt: {reason}"));
                        t.exempt = Some(reason);
                    }
                    continue;
                }
            };
            self.tracked.get_mut(&pid).unwrap().exempt = None;

            let set_class = if pb.lower_to_idle { IDLE } else { BELOW_NORMAL };
            let path = os.path(pid);
            let detail = format!("{pct:.1}% CPU while system at {sys:.0}%");
            if let Err(code) = os.apply_priority(pid, set_class) {
                self.tracked.get_mut(&pid).unwrap().over_ms = 0;
                if code == ERR_ACCESS_DENIED {
                    // Say it once, then stop trying for as long as this process runs.
                    let reason = denied_reason(os.is_elevated());
                    emit(log, &info, &path, id::PB_ERROR, "Restraint could not lower priority", &reason);
                    let rules = self.rules_for(cfg, pid);
                    self.untouchable.insert(pid, Untouchable { name: info.name.clone(), reason, rules });
                } else {
                    emit(log, &info, &path, id::PB_ERROR, "Restraint could not lower priority", &win32_message(code));
                }
                continue;
            }
            let orig_affinity = if pb.restrain_by_affinity && pb.restraint_affinity != 0 {
                let a = os.affinity(pid);
                os.set_affinity(pid, pb.restraint_affinity);
                a
            } else {
                None
            };
            emit(log, &info, &path, id::PB_BEGIN, "Restraint begun", &detail);
            emit(log, &info, &path, id::PB_LOWERED, "Process priority temporarily lowered by Restraint",
                &format!("{} -> {}", class_name(orig_class), class_name(set_class)));
            self.stats.restraints_total += 1;
            self.tracked.get_mut(&pid).unwrap().restraint =
                Some(Restraint { orig_class, set_class, orig_affinity, since_ms: now_ms, pct });
        }
    }

    /// What decides whether Restraint may touch this process: the Restraint settings and any
    /// profile that matches it. Compared against the value stored when it was refused.
    fn rules_for(&self, cfg: &Config, pid: u32) -> String {
        let m = self.profiles.matched(pid).map(|m| format!("{}/{}/{}", m.profile, m.exclude_restraint, m.game_mode));
        format!("{:?}|{:?}", cfg.restraint, m)
    }

    /// Processes Windows refused to let Restraint lower, for the status file, by pid.
    pub fn untouchable(&self) -> Vec<UntouchableInfo> {
        let mut v: Vec<_> =
            self.untouchable.iter().map(|(pid, u)| UntouchableInfo { pid: *pid, name: u.name.clone(), reason: u.reason.clone() }).collect();
        v.sort_by_key(|u| u.pid);
        v
    }

    /// Restore every restrained process (Restraint turned off, or shutdown).
    fn release_restrained(&mut self, os: &mut dyn Os, log: &mut dyn Sink, why: &str) {
        let pids: Vec<u32> = self.tracked.iter().filter(|(_, t)| t.restraint.is_some()).map(|(p, _)| *p).collect();
        for pid in pids {
            self.end_restraint(os, log, pid, id::PB_END_RESTORED, &format!("Restraint ended: {why}"));
        }
    }

    /// Restore everything we changed (called on shutdown).
    pub fn release_all(&mut self, os: &mut dyn Os, log: &mut dyn Sink) {
        self.release_restrained(os, log, "governor stopping");
        // Then everything profiles and game mode changed (restraints are gone, so
        // profile priorities restore straight to the originals).
        let mut bridge = EngineBridge { tracked: &mut self.tracked };
        self.profiles.restore_all(os, log, &mut bridge, "engine stopping");
    }

    /// The profile that exempts this process from Restraint, if any, phrased for the log.
    fn profile_exemption(&self, pid: u32) -> Option<String> {
        let m = self.profiles.matched(pid)?;
        if m.exclude_restraint {
            Some(format!("profile '{}' is set to never restrain this program", m.profile))
        } else if self.profiles.game_active() && m.game_mode {
            Some(format!("game-mode process (profile '{}')", m.profile))
        } else {
            None
        }
    }

    pub fn profile_report(&mut self, os: &mut dyn Os, cfg: &Config) -> ProfileReport {
        self.profiles.report(os, cfg)
    }

    /// Currently restrained processes, oldest restraint first.
    pub fn restrained(&self) -> Vec<RestrainedInfo> {
        let mut v: Vec<_> = self
            .tracked
            .iter()
            .filter_map(|(pid, t)| {
                t.restraint.as_ref().map(|r| RestrainedInfo {
                    pid: *pid,
                    name: t.info.name.clone(),
                    original_class: r.orig_class,
                    set_class: r.set_class,
                    since_ms: r.since_ms,
                    cpu_pct: r.pct,
                })
            })
            .collect();
        v.sort_by_key(|r| (r.since_ms, r.pid));
        v
    }

    /// Snapshot for display: (pid, name, cpu%, restrained).
    pub fn snapshot(&self) -> Vec<(u32, String, f64, bool)> {
        let mut v: Vec<_> =
            self.tracked.iter().map(|(p, t)| (*p, t.info.name.clone(), t.pct, t.restraint.is_some())).collect();
        v.sort_by(|a, b| b.2.total_cmp(&a.2));
        v
    }
}

/// Why Restraint gave up on a process Windows refused access to.
pub(crate) fn denied_reason(elevated: bool) -> String {
    if elevated {
        "access denied even though Process Corral is running elevated: this process is protected (for example by anti-cheat software). Process Corral will not try to get around that and will leave it alone while it runs.".into()
    } else {
        "access denied: Process Corral is not running elevated. Run it as administrator (or as the Windows service) to manage this process; it will not try again while this process runs.".into()
    }
}

fn emit(log: &mut dyn Sink, p: &ProcInfo, path: &str, id: u32, action: &str, info: &str) {
    log.event(&Event { pid: p.pid, name: p.name.clone(), id, action: action.into(), info: info.into(), path: path.into() });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        procs: Vec<ProcInfo>,
        cpu: HashMap<u32, u64>,
        class: HashMap<u32, u32>,
        aff: HashMap<u32, u64>,
        services: Vec<u32>,
        sys: f64,
        fg: Option<u32>,
        killed: Vec<u32>,
        /// Pids Windows refuses to change (access denied), and how often a change was attempted.
        deny: HashSet<u32>,
        tries: HashMap<u32, u32>,
        elevated: bool,
    }
    impl Os for Fake {
        fn processes(&mut self) -> Vec<ProcInfo> { self.procs.clone() }
        fn cpu_time(&mut self, pid: u32) -> Option<u64> { self.cpu.get(&pid).copied() }
        fn system_cpu_pct(&mut self) -> f64 { self.sys }
        fn cpu_count(&self) -> u32 { 4 }
        fn foreground_pid(&mut self) -> Option<u32> { self.fg }
        fn is_session0(&mut self, pid: u32) -> bool { self.services.contains(&pid) }
        fn path(&mut self, _: u32) -> String { String::new() }
        fn priority(&mut self, pid: u32) -> Option<u32> { self.class.get(&pid).copied() }
        fn set_priority(&mut self, pid: u32, c: u32) -> bool { self.class.insert(pid, c); true }
        fn affinity(&mut self, pid: u32) -> Option<u64> { Some(self.aff.get(&pid).copied().unwrap_or(0xf)) }
        fn set_affinity(&mut self, pid: u32, m: u64) -> bool { self.aff.insert(pid, m); true }
        fn terminate(&mut self, pid: u32) -> bool { self.killed.push(pid); self.procs.retain(|p| p.pid != pid); true }
        fn apply_priority(&mut self, pid: u32, c: u32) -> Result<(), u32> {
            *self.tries.entry(pid).or_default() += 1;
            if self.deny.contains(&pid) { Err(ERR_ACCESS_DENIED) } else { self.set_priority(pid, c); Ok(()) }
        }
        fn is_elevated(&self) -> bool { self.elevated }
    }
    #[derive(Default)]
    struct Vlog(Vec<Event>);
    impl Sink for Vlog {
        fn event(&mut self, e: &Event) { self.0.push(e.clone()); }
    }
    impl Vlog {
        fn exempt(&self) -> Vec<&Event> { self.0.iter().filter(|e| e.id == id::PB_EXEMPT).collect() }
    }

    fn proc(pid: u32, name: &str) -> ProcInfo { ProcInfo { pid, ppid: 1, name: name.into() } }

    /// Advance 1s; `busy` CPU-seconds consumed by pid 100 (of 4 cores => pct = busy/4*100).
    fn step(e: &mut Engine, os: &mut Fake, cfg: &Config, log: &mut Vlog, t: &mut u64, busy: f64) {
        *t += 1000;
        *os.cpu.entry(100).or_insert(0) += (busy * 10_000_000.0) as u64;
        e.tick(os, cfg, log, *t);
    }

    fn setup() -> (Engine, Fake, Config, Vlog) {
        let mut os = Fake { sys: 80.0, ..Default::default() };
        os.procs = vec![proc(100, "hog.exe"), proc(200, "idle.exe")];
        os.class.insert(100, NORMAL);
        os.class.insert(200, NORMAL);
        os.cpu.insert(200, 0);
        (Engine::new(1), os, Config::parse(DEFAULT_INI), Vlog::default())
    }

    /// Baseline tick plus one hot tick: hog.exe (pid 100) ends up restrained under `cfg`.
    fn restrained(cfg: &Config) -> (Engine, Fake, Vlog, u64) {
        let (mut e, mut os, _, mut log) = setup();
        let mut t = 0;
        step(&mut e, &mut os, cfg, &mut log, &mut t, 0.0);
        step(&mut e, &mut os, cfg, &mut log, &mut t, 2.0);
        assert_eq!(os.class[&100], BELOW_NORMAL);
        (e, os, log, t)
    }

    #[test]
    fn restrains_then_restores() {
        let (mut e, mut os, cfg, mut log) = setup();
        let mut t = 0;
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 0.0); // baseline
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); // 50% of 4 cores, 1s > 900ms quota
        assert_eq!(os.class[&100], BELOW_NORMAL);
        assert_eq!(os.class[&200], NORMAL);
        assert!(log.0.iter().any(|e| e.id == id::PB_BEGIN));

        // Calms down but min restraint (4.2s) not yet elapsed.
        for _ in 0..3 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 0.0); }
        assert_eq!(os.class[&100], BELOW_NORMAL);
        for _ in 0..3 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 0.0); }
        assert_eq!(os.class[&100], NORMAL);
        assert!(log.0.iter().any(|e| e.id == id::PB_END_RESTORED));
    }

    #[test]
    fn no_restraint_when_system_is_calm() {
        let (mut e, mut os, cfg, mut log) = setup();
        os.sys = 10.0;
        let mut t = 0;
        for _ in 0..5 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
        assert_eq!(os.class[&100], NORMAL);
    }

    #[test]
    fn foreground_and_non_normal_are_exempt() {
        let (mut e, mut os, cfg, mut log) = setup();
        os.fg = Some(100);
        let mut t = 0;
        for _ in 0..4 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
        assert_eq!(os.class[&100], NORMAL);

        let (mut e, mut os, cfg, mut log) = setup();
        os.class.insert(100, HIGH);
        let mut t = 0;
        for _ in 0..4 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
        assert_eq!(os.class[&100], HIGH);
    }

    #[test]
    fn foreground_ends_restraint() {
        let (mut e, mut os, cfg, mut log) = setup();
        let mut t = 0;
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 0.0);
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0);
        assert_eq!(os.class[&100], BELOW_NORMAL);
        os.fg = Some(100);
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0);
        assert_eq!(os.class[&100], NORMAL);
        assert!(log.0.iter().any(|e| e.id == id::PB_END_FOREGROUND));
    }

    #[test]
    fn user_changed_priority_is_not_overwritten() {
        let (mut e, mut os, cfg, mut log) = setup();
        let mut t = 0;
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 0.0);
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0);
        os.class.insert(100, HIGH); // user raised it meanwhile
        os.fg = Some(100);
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0);
        assert_eq!(os.class[&100], HIGH);
    }

    #[test]
    fn exit_closes_restraint_and_release_all_restores() {
        let (mut e, mut os, cfg, mut log) = setup();
        let mut t = 0;
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 0.0);
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0);
        e.release_all(&mut os, &mut log);
        assert_eq!(os.class[&100], NORMAL);

        let (mut e, mut os, cfg, mut log) = setup();
        let mut t = 0;
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 0.0);
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0);
        os.procs.retain(|p| p.pid != 100);
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 0.0);
        assert!(log.0.iter().any(|e| e.id == id::PB_END_TERMINATED));
    }

    #[test]
    fn defaults_disallowed_and_instance_limits() {
        let mut os = Fake::default();
        os.procs = vec![proc(10, "a.exe"), proc(11, "bad.exe"), proc(12, "n.exe"), proc(13, "n.exe"), proc(14, "n.exe")];
        let cfg = Config::parse(
            "[ProcessDefaults]\nDefaultPriorities=a.exe:high\n[ProcessAllowances]\n\
             DisallowedProcesses=bad.exe\nInstanceLimitedProcesses=n.exe:1\n",
        );
        let (mut e, mut log) = (Engine::new(1), Vlog::default());
        e.tick(&mut os, &cfg, &mut log, 1000);
        assert_eq!(os.class[&10], HIGH);
        assert!(os.killed.contains(&11));
        assert_eq!(os.killed.iter().filter(|p| (12..=14).contains(*p)).count(), 2);
        assert!(!os.killed.contains(&12), "oldest instance survives");
    }

    // ---- exemption messages ----

    #[test]
    fn foreground_exemption_names_the_rule_and_is_logged_once() {
        let (mut e, mut os, cfg, mut log) = setup();
        os.fg = Some(100);
        let mut t = 0;
        for _ in 0..6 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
        let ex = log.exempt();
        assert_eq!(ex.len(), 1, "once per episode, not per tick");
        assert!(ex[0].info.contains("is the foreground window"), "{}", ex[0].info);
        assert_eq!(ex[0].name, "hog.exe");
    }

    #[test]
    fn child_of_foreground_names_the_parent_app() {
        let (mut e, mut os, cfg, mut log) = setup();
        os.procs.push(proc(300, "editor.exe"));
        os.class.insert(300, NORMAL);
        os.procs[0].ppid = 300;
        os.fg = Some(300);
        let mut t = 0;
        for _ in 0..3 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
        assert_eq!(os.class[&100], NORMAL);
        let ex = log.exempt();
        assert_eq!(ex.len(), 1);
        assert!(ex[0].info.contains("child of foreground app editor.exe (pid 300)"), "{}", ex[0].info);
        assert!(!ex[0].info.contains("is the foreground window"));
    }

    #[test]
    fn other_exemptions_say_which_rule() {
        let reason = |cfg_text: &str, setup_os: &dyn Fn(&mut Fake)| -> String {
            let (mut e, mut os, _, mut log) = setup();
            setup_os(&mut os);
            let cfg = Config::parse(cfg_text);
            let mut t = 0;
            for _ in 0..3 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
            let ex = log.exempt();
            assert_eq!(ex.len(), 1);
            ex[0].info.clone()
        };
        let r = reason("[OutOfControlProcessRestraint]\nOocExclusions=ho*.exe\n", &|_| {});
        assert!(r.contains("OocExclusions pattern 'ho*.exe'"), "{r}");
        let r = reason("", &|os| {
            os.class.insert(100, HIGH);
        });
        assert!(r.contains("priority class is high, not normal (TameOnlyNormal)"), "{r}");
        let r = reason("", &|os| os.services.push(100));
        assert!(r.contains("session 0 (ExcludeServices)"), "{r}");
        let r = reason("", &|os| os.procs[0].name = "svchost.exe".into());
        assert!(r.contains("built-in exclusion list"), "{r}");
    }

    #[test]
    fn exemption_is_logged_again_after_a_new_episode() {
        let (mut e, mut os, cfg, mut log) = setup();
        os.fg = Some(100);
        let mut t = 0;
        for _ in 0..3 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
        for _ in 0..2 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 0.0); } // calms down
        for _ in 0..3 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
        assert_eq!(log.exempt().len(), 2);
    }

    // ---- live reload safety ----

    #[test]
    fn disabling_restraint_restores_restrained_processes() {
        let cfg = Config::parse(DEFAULT_INI);
        let (mut e, mut os, mut log, mut t) = restrained(&cfg);
        let off = Config::parse("[OutOfControlProcessRestraint]\nOocOn=false\n");
        step(&mut e, &mut os, &off, &mut log, &mut t, 2.0); // still hot, but Restraint is off
        assert_eq!(os.class[&100], NORMAL);
        assert!(log.0.iter().any(|e| e.id == id::PB_END_RESTORED && e.action.contains("disabled")));
        for _ in 0..5 { step(&mut e, &mut os, &off, &mut log, &mut t, 2.0); }
        assert_eq!(os.class[&100], NORMAL, "and it is not restrained again");
        assert_eq!(e.stats.restrained_now, 0);
    }

    #[test]
    fn affinity_is_restored_even_if_the_setting_was_turned_off() {
        let on = Config::parse("[OutOfControlProcessRestraint]\nRestrainByAffinity=true\nRestraintAffinity=0x1\n");
        let (mut e, mut os, mut log, mut t) = restrained(&on);
        assert_eq!(os.aff[&100], 0x1);
        let off = Config::parse(DEFAULT_INI); // RestrainByAffinity=false
        for _ in 0..5 { step(&mut e, &mut os, &off, &mut log, &mut t, 0.0); }
        assert_eq!(os.class[&100], NORMAL);
        assert_eq!(os.aff[&100], 0xf, "original affinity put back");
    }

    #[test]
    fn new_exclusion_releases_a_restrained_process() {
        let cfg = Config::parse(DEFAULT_INI);
        let (mut e, mut os, mut log, mut t) = restrained(&cfg);
        let excl = Config::parse("[OutOfControlProcessRestraint]\nOocExclusions=hog.exe\n");
        step(&mut e, &mut os, &excl, &mut log, &mut t, 2.0);
        assert_eq!(os.class[&100], NORMAL);
        let end = log.0.iter().find(|e| e.id == id::PB_END_EXCLUDED).expect("logged");
        assert!(end.action.contains("OocExclusions pattern 'hog.exe'"), "{}", end.action);
    }

    #[test]
    fn shorter_min_restraint_after_reload_releases_sooner() {
        let cfg = Config::parse(DEFAULT_INI); // 4200ms minimum
        let (mut e, mut os, mut log, mut t) = restrained(&cfg);
        for _ in 0..2 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 0.0); }
        assert_eq!(os.class[&100], BELOW_NORMAL, "2s < 4.2s minimum");
        let quick = Config::parse("[OutOfControlProcessRestraint]\nMinimumTimeOfRestraint=1000\n");
        step(&mut e, &mut os, &quick, &mut log, &mut t, 0.0);
        assert_eq!(os.class[&100], NORMAL);
    }

    #[test]
    fn longer_min_restraint_after_reload_keeps_it_restrained_then_restores() {
        let cfg = Config::parse(DEFAULT_INI);
        let (mut e, mut os, mut log, mut t) = restrained(&cfg);
        let slow = Config::parse("[OutOfControlProcessRestraint]\nMinimumTimeOfRestraint=8000\n");
        for _ in 0..6 { step(&mut e, &mut os, &slow, &mut log, &mut t, 0.0); }
        assert_eq!(os.class[&100], BELOW_NORMAL, "6s < 8s");
        for _ in 0..3 { step(&mut e, &mut os, &slow, &mut log, &mut t, 0.0); }
        assert_eq!(os.class[&100], NORMAL, "never stuck");
    }

    #[test]
    fn raising_thresholds_does_not_strand_a_hot_restrained_process() {
        let cfg = Config::parse(DEFAULT_INI);
        let (mut e, mut os, mut log, mut t) = restrained(&cfg);
        // Reload sets a 60% trigger; the process stays at 50% so it would no longer qualify,
        // but it was already restrained, so it follows the restore rules: calm -> restored.
        let hi = Config::parse("[OutOfControlProcessRestraint]\nPerProcessUsageBeforeRestraint=60\n");
        for _ in 0..6 { step(&mut e, &mut os, &hi, &mut log, &mut t, 0.0); }
        assert_eq!(os.class[&100], NORMAL);
    }

    // ---- access denied (protected processes) ----

    fn denied(log: &Vlog) -> Vec<&Event> { log.0.iter().filter(|e| e.id == id::PB_ERROR).collect() }

    #[test]
    fn access_denied_is_logged_once_per_pid_and_never_retried() {
        let (mut e, mut os, cfg, mut log) = setup();
        os.deny.insert(100);
        let mut t = 0;
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 0.0);
        for _ in 0..60 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
        let d = denied(&log);
        assert_eq!(d.len(), 1, "one line for the whole minute, not one per tick");
        assert_eq!(os.tries[&100], 1, "no further attempts on a protected process");
        assert_eq!(os.class[&100], NORMAL);
        assert_eq!(e.stats.restrained_now, 0);
        let u = e.untouchable();
        assert_eq!(u.len(), 1);
        assert_eq!((u[0].pid, u[0].name.as_str()), (100, "hog.exe"));
        assert_eq!(u[0].reason, d[0].info, "the status file says the same thing as the log");
    }

    #[test]
    fn a_second_protected_pid_gets_its_own_single_line() {
        let (mut e, mut os, cfg, mut log) = setup();
        os.procs.push(proc(300, "other.exe"));
        os.class.insert(300, NORMAL);
        os.deny.extend([100, 300]);
        let mut t = 0;
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 0.0);
        for _ in 0..10 {
            *os.cpu.entry(300).or_insert(0) += 20_000_000;
            step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0);
        }
        let d = denied(&log);
        assert_eq!(d.len(), 2);
        assert_ne!(d[0].pid, d[1].pid);
        assert_eq!(e.untouchable().len(), 2);
    }

    #[test]
    fn wording_depends_on_whether_the_engine_is_elevated() {
        let run = |elevated: bool| -> String {
            let (mut e, mut os, cfg, mut log) = setup();
            os.deny.insert(100);
            os.elevated = elevated;
            let mut t = 0;
            for _ in 0..4 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
            let d = denied(&log);
            assert_eq!(d.len(), 1);
            assert_eq!(d[0].action, "Restraint could not lower priority");
            d[0].info.clone()
        };
        let elevated = run(true);
        assert!(elevated.contains("protected") && elevated.contains("anti-cheat"), "{elevated}");
        assert!(elevated.contains("will not try to get around"), "{elevated}");
        assert!(!elevated.to_lowercase().contains("run it as administrator"), "no pointless advice to elevate: {elevated}");
        let plain = run(false);
        assert!(plain.contains("not running elevated") && plain.contains("administrator"), "{plain}");
        assert!(!plain.contains("anti-cheat"), "do not blame anti-cheat when we simply lack rights: {plain}");
    }

    #[test]
    fn a_protected_pid_is_forgotten_when_the_process_exits() {
        let (mut e, mut os, cfg, mut log) = setup();
        os.deny.insert(100);
        let mut t = 0;
        for _ in 0..4 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
        assert_eq!(e.untouchable().len(), 1);
        os.procs.retain(|p| p.pid != 100);
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 0.0);
        assert!(e.untouchable().is_empty(), "list is cleaned when the process is gone");
        // The same pid number reused by a new, unprotected process is managed again.
        os.deny.clear();
        os.procs.push(proc(100, "new.exe"));
        os.class.insert(100, NORMAL);
        for _ in 0..4 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
        assert_eq!(os.class[&100], BELOW_NORMAL, "a fresh process under a reused pid is restrained as normal");
        assert!(e.untouchable().is_empty());
    }

    #[test]
    fn a_reused_pid_with_a_different_name_is_not_treated_as_protected() {
        let (mut e, mut os, cfg, mut log) = setup();
        os.deny.insert(100);
        let mut t = 0;
        for _ in 0..4 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
        os.deny.clear();
        os.procs[0].name = "other.exe".into(); // same pid, new image, between two ticks
        for _ in 0..3 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
        assert_eq!(os.class[&100], BELOW_NORMAL);
    }

    #[test]
    fn a_rule_change_on_reload_gives_a_protected_process_one_fresh_attempt() {
        let (mut e, mut os, cfg, mut log) = setup();
        os.deny.insert(100);
        let mut t = 0;
        for _ in 0..4 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
        assert_eq!((denied(&log).len(), os.tries[&100]), (1, 1));

        // A reload that does not touch Restraint (or this process) changes nothing.
        let unrelated = Config::parse("[ProcessDefaults]\nDefaultPriorities=other.exe:high\n");
        for _ in 0..4 { step(&mut e, &mut os, &unrelated, &mut log, &mut t, 2.0); }
        assert_eq!((denied(&log).len(), os.tries[&100]), (1, 1), "unrelated reload: still left alone");

        // A reload that changes the Restraint rules retries once, and logs once more if refused again.
        let changed = Config::parse("[OutOfControlProcessRestraint]\nPerProcessUsageBeforeRestraint=16\n");
        for _ in 0..6 { step(&mut e, &mut os, &changed, &mut log, &mut t, 2.0); }
        assert_eq!((denied(&log).len(), os.tries[&100]), (2, 2), "one fresh attempt after the rules changed");

        // And once it is no longer refused, it is managed like any other process.
        os.deny.clear();
        let changed_again = Config::parse("[OutOfControlProcessRestraint]\nPerProcessUsageBeforeRestraint=17\n");
        for _ in 0..3 { step(&mut e, &mut os, &changed_again, &mut log, &mut t, 2.0); }
        assert_eq!(os.class[&100], BELOW_NORMAL);
        assert!(e.untouchable().is_empty());
    }

    #[test]
    fn a_profile_that_starts_matching_counts_as_a_rule_change() {
        let (mut e, mut os, cfg, mut log) = setup();
        os.deny.insert(100);
        let mut t = 0;
        for _ in 0..4 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0); }
        assert_eq!(os.tries[&100], 1);
        let with_profile = Config::parse("[Profile.Hog]\nMatch=hog.exe\nPriority=normal\n");
        for _ in 0..4 { step(&mut e, &mut os, &with_profile, &mut log, &mut t, 2.0); }
        assert!(os.tries[&100] >= 2, "a profile now applies to this process: it gets another attempt");
    }

    #[test]
    fn processes_that_can_be_restrained_behave_exactly_as_before() {
        let (mut e, mut os, cfg, mut log) = setup();
        os.procs.push(proc(300, "protected.exe"));
        os.class.insert(300, NORMAL);
        os.deny.insert(300);
        let mut t = 0;
        step(&mut e, &mut os, &cfg, &mut log, &mut t, 0.0);
        for _ in 0..2 {
            *os.cpu.entry(300).or_insert(0) += 20_000_000;
            step(&mut e, &mut os, &cfg, &mut log, &mut t, 2.0);
        }
        assert_eq!(os.class[&100], BELOW_NORMAL, "the normal hog is still restrained");
        assert!(log.0.iter().any(|x| x.pid == 100 && x.id == id::PB_BEGIN));
        assert_eq!(e.stats.restrained_now, 1);
        assert_eq!(denied(&log).iter().map(|x| x.pid).collect::<Vec<_>>(), vec![300]);
        // ...and it is released and restored on the usual schedule.
        for _ in 0..8 { step(&mut e, &mut os, &cfg, &mut log, &mut t, 0.0); }
        assert_eq!(os.class[&100], NORMAL);
        assert!(log.0.iter().any(|x| x.pid == 100 && x.id == id::PB_END_RESTORED));
    }
}
