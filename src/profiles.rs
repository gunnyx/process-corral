//! Per-application profiles at run time: find matching processes, apply priority / affinity /
//! CPU Sets, verify by reading the value back, retry a refused change a bounded number of times,
//! restore originals when a profile stops applying, and run game mode.
//!
//! Principles:
//! * Only what a profile asks for is touched (a profile that sets only Affinity never calls the
//!   priority API).
//! * Every change we make is recorded with the original value and undone when the profile is
//!   disabled, removed or edited away, when the process stops matching, and on shutdown. Processes
//!   that have exited are simply forgotten.
//! * If Windows refuses a change, that is recorded and reported (with the Win32 error in plain
//!   English). We never try to work around a refusal: protected games and anti-cheat software are
//!   allowed to say no.

use crate::config::{class_name, glob, Config, BELOW_NORMAL, NORMAL};
use crate::engine::HARDCODED_EXCLUSIONS;
use crate::guid::Guid;
use crate::log::{id, iso_utc, unix_ms_now, Event, Sink};
use crate::os::{win32_message, Affinity, Os, ProcInfo, ERR_NOT_IMPLEMENTED};
use crate::profile::{Mech, Profile};
use crate::status::{AppliedProfile, ChangeStatus, GameModeStatus, GameTrigger, ProfileSummary, RefusedChange};
use crate::topology::Topology;
use std::collections::{HashMap, HashSet};

/// A refused change is tried this many times in total, then recorded as refused.
pub const MAX_ATTEMPTS: u32 = 5;
/// Offsets from the first attempt: 5 attempts spread over 30 seconds.
pub const RETRY_OFFSETS_MS: [u64; MAX_ATTEMPTS as usize] = [0, 2_000, 6_000, 14_000, 30_000];

/// Never touched in any way: the OS depends on these.
const CRITICAL_NAMES: &[&str] = &[
    "system", "registry", "secure system", "memory compression", "smss.exe", "csrss.exe", "wininit.exe", "services.exe", "lsass.exe",
    "lsm.exe", "winlogon.exe",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protection {
    None,
    /// Restraint's built-in exclusion list: priority only, never affinity or CPU Sets.
    ExclusionList,
    /// Critical system processes (and this program): nothing at all.
    Critical,
}

pub fn protection(name: &str, pid: u32, own_pid: u32) -> Protection {
    let lower = name.to_ascii_lowercase();
    if pid <= 4 || pid == own_pid || CRITICAL_NAMES.contains(&lower.as_str()) {
        Protection::Critical
    } else if HARDCODED_EXCLUSIONS.contains(&lower.as_str()) {
        Protection::ExclusionList
    } else {
        Protection::None
    }
}

/// How profiles reach into Restraint's bookkeeping.
pub trait Bridge {
    /// If Restraint currently has this process restrained: the priority it will restore.
    fn restraint_original(&self, pid: u32) -> Option<u32>;
    /// Change the priority Restraint will restore. Returns false if the process isn't restrained.
    fn retarget(&mut self, pid: u32, class: u32) -> bool;
}

pub struct NoBridge;
impl Bridge for NoBridge {
    fn restraint_original(&self, _: u32) -> Option<u32> {
        None
    }
    fn retarget(&mut self, _: u32, _: u32) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Aspect {
    Priority,
    Affinity,
    CpuSets,
}

impl Aspect {
    const ALL: [Aspect; 3] = [Aspect::Priority, Aspect::Affinity, Aspect::CpuSets];
    fn key(self) -> &'static str {
        match self {
            Aspect::Priority => "priority",
            Aspect::Affinity => "affinity",
            Aspect::CpuSets => "cpu_sets",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Aspect::Priority => "priority class",
            Aspect::Affinity => "CPU affinity",
            Aspect::CpuSets => "CPU Sets",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Val {
    Prio(u32),
    Aff(Affinity),
    Sets(Vec<u32>),
}

fn show(v: &Val, topo: &Topology) -> String {
    match v {
        Val::Prio(c) => class_name(*c).to_string(),
        Val::Aff(a) if a.group == 0 => format!("{:#X}", a.mask),
        Val::Aff(a) => format!("group {} {:#X}", a.group, a.mask),
        Val::Sets(ids) if ids.is_empty() => "none (any processor)".into(),
        Val::Sets(ids) => format!("{} set(s), CPUs {}", ids.len(), Topology::cpu_list(&topo.cpus_for_ids(ids))),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Pending,
    Retrying,
    Applied,
    Differs,
    Refused,
}

struct AspectState {
    aspect: Aspect,
    /// What we want, or why it cannot be done on this machine / for this process.
    desire: Result<Val, String>,
    original: Option<Val>,
    /// We successfully changed the value, so it must be undone later.
    changed: bool,
    attempts: u32,
    first_try_ms: Option<u64>,
    next_ms: u64,
    phase: Phase,
    current: Option<Val>,
    /// The value as of our last read-back; if it still equals this, nobody else touched it since.
    last_seen: Option<Val>,
    win32: Option<u32>,
    message: String,
    time_unix_ms: u64,
}

impl AspectState {
    fn new(aspect: Aspect, desire: Result<Val, String>) -> Self {
        AspectState {
            aspect,
            desire,
            original: None,
            changed: false,
            attempts: 0,
            first_try_ms: None,
            next_ms: 0,
            phase: Phase::Pending,
            current: None,
            last_seen: None,
            win32: None,
            message: String::new(),
            time_unix_ms: 0,
        }
    }

    /// The profile now wants something different: start over, but keep what we know of the original.
    fn reset(&mut self, desire: Result<Val, String>) {
        self.desire = desire;
        self.attempts = 0;
        self.first_try_ms = None;
        self.next_ms = 0;
        self.phase = Phase::Pending;
        self.win32 = None;
        self.message.clear();
    }
}

struct Entry {
    name: String,
    profile: String,
    aspects: Vec<AspectState>,
    since_unix_ms: u64,
}

/// The profile a process matches, whether or not anything has been applied yet.
#[derive(Clone, Debug)]
pub struct Matched {
    pub name: String,
    pub profile: String,
    pub exclude_restraint: bool,
    pub game_mode: bool,
}

struct Lowered {
    pid: u32,
    orig: u32,
}

#[derive(Default)]
struct Game {
    active: bool,
    since_unix_ms: u64,
    trigger: Option<(u32, String, String)>,
    lowered: Vec<Lowered>,
    seen: HashSet<u32>,
    power_prev: Option<Guid>,
    power_set: Option<Guid>,
    power_failed: Option<Guid>,
}

#[derive(Default)]
pub struct ProfileManager {
    entries: HashMap<u32, Entry>,
    matched: HashMap<u32, Matched>,
    paths: HashMap<u32, String>,
    game: Game,
}

struct Cx<'a> {
    os: &'a mut dyn Os,
    log: &'a mut dyn Sink,
    bridge: &'a mut dyn Bridge,
    topo: &'a Topology,
    now_ms: u64,
    unix_ms: u64,
}

fn emit(log: &mut dyn Sink, pid: u32, name: &str, id: u32, action: &str, info: &str) {
    log.event(&Event { pid, name: name.into(), id, action: action.into(), info: info.into(), path: String::new() });
}

fn read(cx: &mut Cx, pid: u32, a: Aspect) -> Result<Val, u32> {
    Ok(match a {
        Aspect::Priority => Val::Prio(cx.os.read_priority(pid)?),
        Aspect::Affinity => Val::Aff(cx.os.read_affinity(pid)?),
        Aspect::CpuSets => Val::Sets(cx.os.read_cpu_sets(pid)?),
    })
}

fn write(cx: &mut Cx, pid: u32, v: &Val) -> Result<(), u32> {
    match v {
        Val::Prio(c) => cx.os.apply_priority(pid, *c),
        Val::Aff(a) => cx.os.apply_affinity(pid, *a),
        Val::Sets(ids) => cx.os.apply_cpu_sets(pid, ids),
    }
}

/// Plain-English refusal text. Access denied gets an extra explanation because it is by far the most
/// common refusal and the user can't tell protected software from a bug otherwise.
pub fn refusal_text(aspect: Aspect, code: u32) -> String {
    let mut s = format!("Windows refused to change the {}: {}", aspect.label(), win32_message(code));
    if code == 5 {
        s.push_str(". Access was denied: protected games and anti-cheat software commonly block this, and Process Corral does not try to get around it");
    }
    // A complete sentence: it is followed by more text when logged ("... Retrying up to ...").
    s.push('.');
    s
}

/// The aspect states a profile asks for on this machine, in `Aspect::ALL` order.
fn desired(p: &Profile, topo: &Topology, prot: Protection) -> [Option<Result<Val, String>>; 3] {
    let mut out: [Option<Result<Val, String>>; 3] = [None, None, None];
    if let Some(c) = p.priority {
        out[0] = Some(if prot == Protection::Critical { Err("this is a critical system process; Process Corral never changes it".into()) } else { Ok(Val::Prio(c)) });
    }
    let mech = p.effective_cpu().map(|r| r.mech);
    let slot = if mech == Some(Mech::CpuSets) { 2 } else { 1 };
    match p.resolve_cpus(topo) {
        Err(e) => out[slot] = Some(Err(e)),
        Ok(None) => {}
        Ok(Some((m, cpus))) => {
            out[slot] = Some(if prot != Protection::None {
                Err(format!(
                    "{} processes are never given a CPU affinity or CPU Sets",
                    if prot == Protection::Critical { "critical system" } else { "built-in exclusion list" }
                ))
            } else {
                match m {
                    Mech::Affinity => topo.affinity_for(&cpus).map(|(group, mask)| Val::Aff(Affinity { group, mask })),
                    Mech::CpuSets => topo.cpu_set_ids_for(&cpus).map(Val::Sets),
                }
            });
        }
    }
    out
}

fn same(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Sets(x), Val::Sets(y)) => {
            let (mut x, mut y) = (x.clone(), y.clone());
            x.sort_unstable();
            y.sort_unstable();
            x == y
        }
        _ => a == b,
    }
}

fn fail(cx: &mut Cx, pid: u32, name: &str, profile: &str, st: &mut AspectState, code: u32) {
    st.attempts += 1;
    st.win32 = Some(code);
    st.message = refusal_text(st.aspect, code);
    st.time_unix_ms = cx.unix_ms;
    let first = st.first_try_ms.unwrap_or(cx.now_ms);
    if code == ERR_NOT_IMPLEMENTED || st.attempts >= MAX_ATTEMPTS {
        st.phase = Phase::Refused;
        emit(
            cx.log,
            pid,
            name,
            id::PROFILE_REFUSED,
            "Profile change refused",
            &format!("{profile}: {} Gave up after {} attempt(s); this change was NOT applied.", st.message, st.attempts),
        );
    } else {
        st.phase = Phase::Retrying;
        st.next_ms = first + RETRY_OFFSETS_MS[st.attempts as usize];
        if st.attempts == 1 {
            emit(
                cx.log,
                pid,
                name,
                id::PROFILE_RETRY,
                "Profile change refused; will retry",
                &format!("{profile}: {} Retrying up to {} times over about 30 seconds.", st.message, MAX_ATTEMPTS),
            );
        }
    }
}

fn attempt(cx: &mut Cx, pid: u32, name: &str, profile: &str, st: &mut AspectState) {
    let wanted = match &st.desire {
        Ok(v) => v.clone(),
        Err(why) => {
            st.phase = Phase::Refused;
            st.message = why.clone();
            st.time_unix_ms = cx.unix_ms;
            emit(cx.log, pid, name, id::PROFILE_REFUSED, "Profile change not applied", &format!("{profile}: {} - {why}", st.aspect.label()));
            return;
        }
    };
    st.first_try_ms.get_or_insert(cx.now_ms);

    if st.original.is_none() {
        match read(cx, pid, st.aspect) {
            Ok(mut v) => {
                if st.aspect == Aspect::Priority {
                    // A process Restraint is restraining shows its lowered priority right now; the
                    // real original is what Restraint will put back.
                    if let Some(c) = cx.bridge.restraint_original(pid) {
                        v = Val::Prio(c);
                    }
                }
                st.original = Some(v);
            }
            Err(e) => return fail(cx, pid, name, profile, st, e),
        }
    }
    if let (Val::Aff(w), Some(Val::Aff(o))) = (&wanted, &st.original) {
        if w.group != o.group {
            st.phase = Phase::Refused;
            st.message = format!("the process runs in processor group {} but the profile selects processors in group {}; an affinity cannot move it between groups (use CpuSets)", o.group, w.group);
            st.time_unix_ms = cx.unix_ms;
            emit(cx.log, pid, name, id::PROFILE_REFUSED, "Profile change not applied", &format!("{profile}: {}", st.message));
            return;
        }
    }

    // A restrained process keeps its Restraint priority until released; the profile priority is
    // what it will be released *to*.
    if let (Val::Prio(c), true) = (&wanted, st.aspect == Aspect::Priority && cx.bridge.restraint_original(pid).is_some()) {
        cx.bridge.retarget(pid, *c);
        st.changed = true;
        st.phase = Phase::Applied;
        st.current = Some(wanted.clone());
        st.last_seen = None;
        st.message = "the process is currently restrained; the profile priority is restored when the restraint ends".into();
        st.time_unix_ms = cx.unix_ms;
        emit(cx.log, pid, name, id::PROFILE_APPLIED, "Profile applied", &format!("{profile}: priority class -> {} (when the restraint is released)", show(&wanted, cx.topo)));
        return;
    }

    if let Err(e) = write(cx, pid, &wanted) {
        return fail(cx, pid, name, profile, st, e);
    }
    st.changed = true;
    st.attempts += 1;
    st.time_unix_ms = cx.unix_ms;
    st.win32 = None;
    let orig = st.original.as_ref().map(|v| show(v, cx.topo)).unwrap_or_default();
    match read(cx, pid, st.aspect) {
        Ok(cur) => {
            st.current = Some(cur.clone());
            st.last_seen = Some(cur.clone());
            if same(&cur, &wanted) {
                st.phase = Phase::Applied;
                st.message.clear();
                emit(cx.log, pid, name, id::PROFILE_APPLIED, "Profile applied", &format!("{profile}: {} {orig} -> {}", st.aspect.label(), show(&cur, cx.topo)));
            } else {
                st.phase = Phase::Differs;
                st.message = format!("Windows or the process changed it: wanted {}, now {}", show(&wanted, cx.topo), show(&cur, cx.topo));
                emit(cx.log, pid, name, id::PROFILE_DIFFERS, "Profile applied, but the value differs", &format!("{profile}: {}: {}", st.aspect.label(), st.message));
            }
        }
        Err(e) => {
            st.phase = Phase::Applied;
            st.current = None;
            st.message = format!("applied, but reading it back failed ({})", win32_message(e));
            emit(cx.log, pid, name, id::PROFILE_APPLIED, "Profile applied (not verified)", &format!("{profile}: {} {orig} -> {}; {}", st.aspect.label(), show(&wanted, cx.topo), st.message));
        }
    }
}

fn step(cx: &mut Cx, pid: u32, name: &str, profile: &str, st: &mut AspectState) {
    if matches!(st.phase, Phase::Pending | Phase::Retrying) && cx.now_ms >= st.next_ms {
        attempt(cx, pid, name, profile, st);
    }
}

/// Put one aspect back to its original, unless something else has changed it since.
fn restore_aspect(cx: &mut Cx, pid: u32, name: &str, profile: &str, st: &mut AspectState, why: &str) {
    if !st.changed {
        return;
    }
    let Some(orig) = st.original.clone() else { return };
    let label = st.aspect.label();
    if let (Val::Prio(c), true) = (&orig, st.aspect == Aspect::Priority && cx.bridge.restraint_original(pid).is_some()) {
        cx.bridge.retarget(pid, *c);
        st.changed = false;
        emit(cx.log, pid, name, id::PROFILE_RESTORED, "Profile removed: original restored", &format!("{profile}: {label} -> {} once the restraint is released ({why})", show(&orig, cx.topo)));
        return;
    }
    match read(cx, pid, st.aspect) {
        Ok(cur) => {
            if same(&cur, &orig) {
                st.changed = false;
                return;
            }
            let ours = st.desire.as_ref().ok().is_some_and(|w| same(&cur, w)) || st.last_seen.as_ref().is_some_and(|l| same(&cur, l));
            if !ours {
                emit(
                    cx.log,
                    pid,
                    name,
                    id::PROFILE_RESTORED,
                    "Profile removed: original NOT restored",
                    &format!("{profile}: the {label} was changed by something else (now {}); left alone", show(&cur, cx.topo)),
                );
                st.changed = false;
                return;
            }
        }
        Err(e) => {
            emit(cx.log, pid, name, id::PROFILE_REFUSED, "Profile restore failed", &format!("{profile}: could not read the {label}: {}", win32_message(e)));
            return;
        }
    }
    match write(cx, pid, &orig) {
        Ok(()) => {
            st.changed = false;
            emit(cx.log, pid, name, id::PROFILE_RESTORED, "Profile removed: original restored", &format!("{profile}: {label} -> {} ({why})", show(&orig, cx.topo)));
        }
        Err(e) => emit(cx.log, pid, name, id::PROFILE_REFUSED, "Profile restore refused", &format!("{profile}: {} ({why})", refusal_text(st.aspect, e))),
    }
}

fn restore_entry(cx: &mut Cx, pid: u32, mut e: Entry, why: &str) {
    for st in e.aspects.iter_mut() {
        restore_aspect(cx, pid, &e.name.clone(), &e.profile.clone(), st, why);
    }
}

impl ProfileManager {
    pub fn matched(&self, pid: u32) -> Option<&Matched> {
        self.matched.get(&pid)
    }

    pub fn game_active(&self) -> bool {
        self.game.active
    }

    fn path_of(&mut self, os: &mut dyn Os, pid: u32) -> String {
        self.paths.entry(pid).or_insert_with(|| os.path(pid)).clone()
    }

    /// The first enabled profile matching this process (used for new-process decisions).
    pub fn find_match<'a>(&mut self, os: &mut dyn Os, cfg: &'a Config, p: &ProcInfo) -> Option<&'a Profile> {
        let need_path = cfg.profiles.iter().any(|x| x.enabled && x.is_path_pattern());
        let path = need_path.then(|| self.path_of(os, p.pid));
        cfg.profiles.iter().find(|x| x.matches(&p.name, path.as_deref()))
    }

    /// One engine tick: match, reconcile every matching process with its profile, run game mode.
    pub fn tick(&mut self, os: &mut dyn Os, cfg: &Config, log: &mut dyn Sink, procs: &[ProcInfo], own_pid: u32, now_ms: u64, bridge: &mut dyn Bridge) {
        let unix_ms = unix_ms_now();
        let topo = os.topology();
        let alive: HashMap<u32, &ProcInfo> = procs.iter().map(|p| (p.pid, p)).collect();
        // Exited processes (or a recycled pid) are forgotten, never "restored".
        self.entries.retain(|pid, e| alive.get(pid).is_some_and(|p| p.name.eq_ignore_ascii_case(&e.name)));
        self.paths.retain(|pid, _| alive.contains_key(pid));
        self.matched.clear();

        let mut want: Vec<(&ProcInfo, &Profile)> = vec![];
        if !cfg.profiles.is_empty() {
            for p in procs {
                if p.pid == 0 || p.pid == 4 || p.pid == own_pid {
                    continue;
                }
                if let Some(pr) = self.find_match(os, cfg, p) {
                    self.matched.insert(
                        p.pid,
                        Matched { name: p.name.clone(), profile: pr.name.clone(), exclude_restraint: pr.exclude_restraint, game_mode: pr.game_mode },
                    );
                    want.push((p, pr));
                }
            }
        }

        let mut cx = Cx { os, log, bridge, topo: &topo, now_ms, unix_ms };

        // Profiles that no longer apply to a process (removed, disabled, edited, different match).
        let stale: Vec<u32> =
            self.entries.iter().filter(|(pid, e)| !want.iter().any(|(p, pr)| p.pid == **pid && pr.name == e.profile)).map(|(pid, _)| *pid).collect();
        for pid in stale {
            if let Some(e) = self.entries.remove(&pid) {
                restore_entry(&mut cx, pid, e, "the profile no longer applies to this process");
            }
        }

        for (p, pr) in &want {
            let prot = protection(&p.name, p.pid, own_pid);
            let want_states = desired(pr, &topo, prot);
            let entry = self.entries.entry(p.pid).or_insert_with(|| Entry { name: p.name.clone(), profile: pr.name.clone(), aspects: vec![], since_unix_ms: unix_ms });
            for (i, aspect) in Aspect::ALL.iter().enumerate() {
                let pos = entry.aspects.iter().position(|a| a.aspect == *aspect);
                match (want_states[i].clone(), pos) {
                    (None, None) => {}
                    (None, Some(k)) => {
                        let mut st = entry.aspects.remove(k);
                        restore_aspect(&mut cx, p.pid, &entry.name, &entry.profile, &mut st, "the profile no longer sets this");
                    }
                    (Some(w), None) => entry.aspects.push(AspectState::new(*aspect, w)),
                    (Some(w), Some(k)) => {
                        if entry.aspects[k].desire != w {
                            entry.aspects[k].reset(w);
                        }
                    }
                }
            }
            let (name, profile) = (entry.name.clone(), entry.profile.clone());
            for st in entry.aspects.iter_mut() {
                step(&mut cx, p.pid, &name, &profile, st);
            }
        }

        self.update_game(&mut cx, cfg, procs, own_pid);
    }

    // ------------------------------------------------------------ game mode

    fn update_game(&mut self, cx: &mut Cx, cfg: &Config, procs: &[ProcInfo], own_pid: u32) {
        let mut games: Vec<(u32, String, String)> =
            self.matched.iter().filter(|(_, m)| m.game_mode).map(|(pid, m)| (*pid, m.name.clone(), m.profile.clone())).collect();
        games.sort();
        if games.is_empty() {
            if self.game.active {
                game_leave(cx, &mut self.game, "the last game-mode process exited");
            }
            return;
        }
        if !self.game.active {
            let (pid, name, profile) = games[0].clone();
            self.game.active = true;
            self.game.since_unix_ms = cx.unix_ms;
            self.game.trigger = Some((pid, name.clone(), profile.clone()));
            self.game.seen.clear();
            emit(cx.log, pid, &name, id::GAME_ENTER, "Game mode entered", &format!("triggered by {name} (pid {pid}), profile '{profile}'"));
        }

        // Background priority: lower everything that isn't the game or excluded, once per process.
        if cfg.game_mode.background_priority {
            let game_pids: HashSet<u32> = games.iter().map(|g| g.0).collect();
            let mut lowered_now = 0;
            for p in procs {
                if self.game.seen.contains(&p.pid) {
                    continue;
                }
                if game_pids.contains(&p.pid) || game_pids.contains(&p.ppid) || self.matched.contains_key(&p.pid) {
                    self.game.seen.insert(p.pid);
                    continue;
                }
                self.game.seen.insert(p.pid);
                let user_excluded = cfg.restraint.exclusions.iter().any(|g| glob(g, &p.name));
                if p.pid == own_pid || protection(&p.name, p.pid, own_pid) != Protection::None || user_excluded || cx.os.is_session0(p.pid) {
                    continue;
                }
                if cx.bridge.restraint_original(p.pid).is_some() {
                    continue;
                }
                if cx.os.read_priority(p.pid) == Ok(NORMAL) && cx.os.apply_priority(p.pid, BELOW_NORMAL).is_ok() {
                    self.game.lowered.push(Lowered { pid: p.pid, orig: NORMAL });
                    lowered_now += 1;
                }
            }
            if lowered_now > 0 {
                emit(cx.log, 0, "process-corral", id::GAME_INFO, "Game mode: background processes lowered", &format!("{lowered_now} process(es) set to Below Normal ({} in total)", self.game.lowered.len()));
            }
        } else if !self.game.lowered.is_empty() {
            undo_background(cx, &mut self.game, "BackgroundPriority was turned off");
        }

        // Power plan.
        match cfg.game_mode.power_plan.as_deref().and_then(Guid::parse) {
            Some(want) => {
                if self.game.power_set != Some(want) && self.game.power_failed != Some(want) {
                    if self.game.power_prev.is_none() {
                        match cx.os.power_plan() {
                            Ok(cur) => self.game.power_prev = Some(cur),
                            Err(e) => {
                                self.game.power_failed = Some(want);
                                emit(cx.log, 0, "process-corral", id::GAME_INFO, "Game mode: power plan not changed", &format!("could not read the current plan: {}", win32_message(e)));
                                return;
                            }
                        }
                    }
                    match cx.os.set_power_plan(&want) {
                        Ok(()) => {
                            self.game.power_set = Some(want);
                            emit(cx.log, 0, "process-corral", id::GAME_INFO, "Game mode: power plan switched", &format!("{want} (previous {})", self.game.power_prev.map_or("?".into(), |g| g.to_string())));
                        }
                        Err(e) => {
                            self.game.power_failed = Some(want);
                            emit(cx.log, 0, "process-corral", id::GAME_INFO, "Game mode: power plan not changed", &format!("Windows refused: {}", win32_message(e)));
                        }
                    }
                }
            }
            None => {
                if self.game.power_set.is_some() {
                    undo_power(cx, &mut self.game, "PowerPlan was removed from [GameMode]");
                }
                self.game.power_failed = None;
            }
        }
    }

    /// Undo everything: every profile change and all of game mode. The shutdown path.
    pub fn restore_all(&mut self, os: &mut dyn Os, log: &mut dyn Sink, bridge: &mut dyn Bridge, why: &str) {
        let topo = os.topology();
        let mut cx = Cx { os, log, bridge, topo: &topo, now_ms: 0, unix_ms: unix_ms_now() };
        if self.game.active {
            game_leave(&mut cx, &mut self.game, why);
        }
        let pids: Vec<u32> = self.entries.keys().copied().collect();
        for pid in pids {
            if let Some(e) = self.entries.remove(&pid) {
                restore_entry(&mut cx, pid, e, why);
            }
        }
        self.matched.clear();
    }

    // ------------------------------------------------------------ reporting

    pub fn report(&mut self, os: &mut dyn Os, cfg: &Config) -> ProfileReport {
        let topo = os.topology();
        let mut applied = vec![];
        let mut refused = vec![];
        let mut pids: Vec<u32> = self.entries.keys().copied().collect();
        pids.sort_unstable();
        for pid in pids {
            // Re-read live values so "current" reflects what the process has now.
            let live: Vec<(Aspect, Option<Val>)> = self.entries[&pid]
                .aspects
                .iter()
                .map(|a| {
                    let cur = if matches!(a.phase, Phase::Applied | Phase::Differs) && a.current.is_some() {
                        match a.aspect {
                            Aspect::Priority => os.read_priority(pid).ok().map(Val::Prio),
                            Aspect::Affinity => os.read_affinity(pid).ok().map(Val::Aff),
                            Aspect::CpuSets => os.read_cpu_sets(pid).ok().map(Val::Sets),
                        }
                    } else {
                        None
                    };
                    (a.aspect, cur)
                })
                .collect();
            let e = &self.entries[&pid];
            let mut changes = vec![];
            for (st, (_, live_cur)) in e.aspects.iter().zip(live) {
                let wanted = st.desire.as_ref().map(|v| show(v, &topo)).unwrap_or_else(|why| format!("not possible: {why}"));
                let shown = |v: &Option<Val>| v.as_ref().map(|v| show(v, &topo)).unwrap_or_default();
                let cur = live_cur.or_else(|| st.current.clone());
                let result = match st.phase {
                    Phase::Pending => "pending",
                    Phase::Retrying => "retrying",
                    Phase::Applied => "applied",
                    Phase::Differs => "applied_but_different",
                    Phase::Refused => "refused",
                };
                changes.push(ChangeStatus {
                    aspect: st.aspect.key().into(),
                    wanted,
                    original: shown(&st.original),
                    current: shown(&cur),
                    result: result.into(),
                    win32_error: st.win32,
                    message: st.message.clone(),
                    attempts: st.attempts,
                });
                if st.phase == Phase::Refused {
                    refused.push(RefusedChange {
                        pid,
                        process: e.name.clone(),
                        profile: e.profile.clone(),
                        aspect: st.aspect.key().into(),
                        win32_error: st.win32,
                        message: st.message.clone(),
                        attempts: st.attempts,
                        time: iso_utc(st.time_unix_ms.max(e.since_unix_ms)),
                    });
                }
            }
            applied.push(AppliedProfile { pid, process: e.name.clone(), profile: e.profile.clone(), time: iso_utc(e.since_unix_ms), changes });
        }
        let g = &self.game;
        ProfileReport {
            profiles: cfg.profiles.iter().map(|p| ProfileSummary { name: p.name.clone(), enabled: p.enabled, matcher: p.matcher.clone() }).collect(),
            applied,
            refused,
            game: GameModeStatus {
                active: g.active,
                since: g.active.then(|| iso_utc(g.since_unix_ms)),
                triggered_by: g.trigger.clone().filter(|_| g.active).map(|(pid, process, profile)| GameTrigger { pid, process, profile }),
                background_lowered: g.lowered.len() as u32,
                power_plan_switched: g.power_set.is_some(),
            },
        }
    }
}

pub struct ProfileReport {
    pub profiles: Vec<ProfileSummary>,
    pub applied: Vec<AppliedProfile>,
    pub refused: Vec<RefusedChange>,
    pub game: GameModeStatus,
}

fn undo_background(cx: &mut Cx, g: &mut Game, why: &str) {
    let mut restored = 0;
    for l in g.lowered.drain(..) {
        // Only if it is still the priority we set (the user may have changed it since).
        if cx.os.read_priority(l.pid) == Ok(BELOW_NORMAL) && cx.os.apply_priority(l.pid, l.orig).is_ok() {
            restored += 1;
        }
    }
    emit(cx.log, 0, "process-corral", id::GAME_INFO, "Game mode: background priorities restored", &format!("{restored} process(es) back to their original priority ({why})"));
}

fn undo_power(cx: &mut Cx, g: &mut Game, why: &str) {
    let (Some(set), prev) = (g.power_set.take(), g.power_prev.take()) else { return };
    // Leave the plan alone if the user switched it themselves while the game ran.
    if cx.os.power_plan().ok() != Some(set) {
        emit(cx.log, 0, "process-corral", id::GAME_INFO, "Game mode: power plan left as is", "it was changed by someone else during game mode");
        return;
    }
    if let Some(prev) = prev {
        match cx.os.set_power_plan(&prev) {
            Ok(()) => emit(cx.log, 0, "process-corral", id::GAME_INFO, "Game mode: power plan restored", &format!("{prev} ({why})")),
            Err(e) => emit(cx.log, 0, "process-corral", id::GAME_INFO, "Game mode: power plan NOT restored", &format!("Windows refused: {}", win32_message(e))),
        }
    }
}

fn game_leave(cx: &mut Cx, g: &mut Game, why: &str) {
    if !g.lowered.is_empty() {
        undo_background(cx, g, why);
    }
    if g.power_set.is_some() {
        undo_power(cx, g, why);
    }
    let trig = g.trigger.take();
    g.active = false;
    g.seen.clear();
    g.power_prev = None;
    g.power_failed = None;
    let (pid, name) = trig.map_or((0, "process-corral".to_string()), |t| (t.0, t.1));
    emit(cx.log, pid, &name, id::GAME_LEAVE, "Game mode left", why);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ABOVE_NORMAL, HIGH, IDLE};
    use crate::simos::{Op, Refusal, SimOs, SimVal};
    use crate::topology::{fake_9950x3d, fake_quad};

    #[derive(Default)]
    struct Rec(Vec<Event>);
    impl Sink for Rec {
        fn event(&mut self, e: &Event) {
            self.0.push(e.clone());
        }
    }
    impl Rec {
        fn ids(&self, i: u32) -> Vec<&Event> {
            self.0.iter().filter(|e| e.id == i).collect()
        }
    }

    fn cfg(text: &str) -> Config {
        Config::from_bytes(text.as_bytes()).unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    struct Rig {
        os: SimOs,
        m: ProfileManager,
        log: Rec,
        t: u64,
    }
    impl Rig {
        fn new(topo: Topology) -> Rig {
            Rig { os: SimOs::new(topo), m: ProfileManager::default(), log: Rec::default(), t: 0 }
        }
        fn tick(&mut self, c: &Config) {
            self.t += 1000;
            let procs = self.os.processes();
            self.m.tick(&mut self.os, c, &mut self.log, &procs, 1, self.t, &mut NoBridge);
        }
        fn ticks(&mut self, c: &Config, n: u32) {
            for _ in 0..n {
                self.tick(c);
            }
        }
        fn stop(&mut self) {
            self.m.restore_all(&mut self.os, &mut self.log, &mut NoBridge, "engine stopping");
        }
    }

    const GAME: &str = "[Profile.Game]\nMatch=game.exe\nPriority=high\nCpuGroup=vcache\n";

    // ---- applying -----------------------------------------------------------------------

    #[test]
    fn applies_priority_and_vcache_affinity_and_reads_them_back() {
        let mut r = Rig::new(fake_9950x3d(true));
        r.os.add(10, "game.exe");
        r.os.add(11, "other.exe");
        let c = cfg(GAME);
        r.tick(&c);
        let g = &r.os.procs[&10];
        assert_eq!((g.priority, g.affinity.mask), (HIGH, 0xFFFF));
        assert_eq!(r.os.procs[&11].priority, NORMAL, "non-matching process untouched");
        assert_eq!(r.os.procs[&11].affinity.mask, 0xFFFF_FFFF);
        let applied = r.log.ids(id::PROFILE_APPLIED);
        assert_eq!(applied.len(), 2);
        assert!(applied.iter().any(|e| e.info.contains("priority class normal -> high")), "{applied:?}");
        assert!(applied.iter().any(|e| e.info.contains("CPU affinity 0xFFFFFFFF -> 0xFFFF")));
        let rep = r.m.report(&mut r.os, &c);
        let ch = &rep.applied[0].changes;
        assert!(ch.iter().all(|c| c.result == "applied"));
        let aff = ch.iter().find(|c| c.aspect == "affinity").unwrap();
        assert_eq!((aff.original.as_str(), aff.wanted.as_str(), aff.current.as_str()), ("0xFFFFFFFF", "0xFFFF", "0xFFFF"));
    }

    #[test]
    fn vcache_resolves_to_the_96mb_group_even_when_it_is_the_second_ccd() {
        let mut r = Rig::new(fake_9950x3d(false));
        r.os.add(10, "game.exe");
        r.tick(&cfg(GAME));
        assert_eq!(r.os.procs[&10].affinity.mask, 0xFFFF_0000);
    }

    #[test]
    fn a_profile_that_sets_only_affinity_never_touches_priority() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe").priority = ABOVE_NORMAL;
        r.tick(&cfg("[Profile.G]\nMatch=game.exe\nAffinity=0-1\n"));
        assert_eq!(r.os.procs[&10].affinity.mask, 0b11);
        assert_eq!(r.os.procs[&10].priority, ABOVE_NORMAL);
        assert_eq!(r.os.count("prio"), 0, "{:?}", r.os.calls);
    }

    #[test]
    fn a_profile_that_sets_only_priority_leaves_affinity_alone() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe");
        r.tick(&cfg("[Profile.G]\nMatch=game.exe\nPriority=idle\n"));
        assert_eq!(r.os.procs[&10].priority, IDLE);
        assert_eq!((r.os.count("aff"), r.os.count("sets")), (0, 0));
    }

    #[test]
    fn already_running_matching_processes_are_applied_on_first_tick_and_never_reapplied() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe");
        let c = cfg("[Profile.G]\nMatch=game.exe\nPriority=high\n");
        r.ticks(&c, 10);
        assert_eq!(r.os.count("prio"), 1, "applied exactly once");
        r.os.add(11, "game.exe");
        r.ticks(&c, 3);
        assert_eq!(r.os.count("prio"), 2, "a new matching pid is picked up");
    }

    #[test]
    fn cpu_sets_apply_read_back_and_restore_to_none() {
        let mut r = Rig::new(fake_9950x3d(true));
        r.os.add(10, "game.exe");
        let c = cfg("[Profile.G]\nMatch=game.exe\nCpuSets=0-3\n");
        r.tick(&c);
        assert_eq!(r.os.procs[&10].cpu_sets, vec![0x100, 0x101, 0x102, 0x103]);
        assert_eq!(r.os.count("aff"), 0, "CpuSets does not use the affinity API");
        r.tick(&cfg("[Other]\nx=1\n"));
        assert!(r.os.procs[&10].cpu_sets.is_empty(), "restored to 'no CPU Sets'");
    }

    #[test]
    fn cpu_sets_unsupported_is_refused_once_without_retries() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe");
        r.os.refuse.insert((10, Op::CpuSets), Refusal { code: ERR_NOT_IMPLEMENTED, remaining: None });
        r.ticks(&cfg("[Profile.G]\nMatch=game.exe\nCpuSets=0-1\n"), 40);
        assert_eq!(r.os.tries.get(&(10, Op::CpuSets)), Some(&1));
        assert_eq!(r.log.ids(id::PROFILE_REFUSED).len(), 1);
    }

    // ---- reload: change / disable / remove ------------------------------------------------

    #[test]
    fn reload_adds_a_profile_to_a_running_process_changes_it_and_removes_it() {
        let mut r = Rig::new(fake_9950x3d(true));
        r.os.add(10, "game.exe");
        let none = cfg("[Other]\nx=1\n");
        r.ticks(&none, 3);
        assert_eq!(r.os.calls.len(), 0, "old configs without profiles do nothing");

        let v1 = cfg("[Profile.G]\nMatch=game.exe\nPriority=high\nAffinity=0-7\n");
        r.tick(&v1);
        assert_eq!((r.os.procs[&10].priority, r.os.procs[&10].affinity.mask), (HIGH, 0xFF));

        let v2 = cfg("[Profile.G]\nMatch=game.exe\nPriority=abovenormal\nAffinity=0-3\n");
        r.tick(&v2);
        assert_eq!((r.os.procs[&10].priority, r.os.procs[&10].affinity.mask), (ABOVE_NORMAL, 0xF), "edited values are applied");

        r.tick(&none);
        assert_eq!((r.os.procs[&10].priority, r.os.procs[&10].affinity.mask), (NORMAL, 0xFFFF_FFFF), "removed -> the true originals, not the intermediate values");
        let restored = r.log.ids(id::PROFILE_RESTORED);
        assert_eq!(restored.len(), 2, "{restored:?}");
        let before = r.os.calls.len();
        r.ticks(&none, 5);
        assert_eq!(r.os.calls.len(), before, "nothing more happens");
    }

    #[test]
    fn disabling_a_profile_restores_and_enabling_reapplies() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe");
        let on = cfg("[Profile.G]\nMatch=game.exe\nPriority=high\n");
        let off = cfg("[Profile.G]\nMatch=game.exe\nPriority=high\nEnabled=false\n");
        r.tick(&on);
        assert_eq!(r.os.procs[&10].priority, HIGH);
        r.tick(&off);
        assert_eq!(r.os.procs[&10].priority, NORMAL);
        r.tick(&on);
        assert_eq!(r.os.procs[&10].priority, HIGH);
    }

    #[test]
    fn dropping_one_setting_from_a_profile_restores_just_that_setting() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe");
        r.tick(&cfg("[Profile.G]\nMatch=game.exe\nPriority=high\nAffinity=0-1\n"));
        r.tick(&cfg("[Profile.G]\nMatch=game.exe\nPriority=high\n"));
        assert_eq!((r.os.procs[&10].priority, r.os.procs[&10].affinity.mask), (HIGH, 0xF));
    }

    #[test]
    fn a_process_that_stops_matching_is_restored() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe");
        r.tick(&cfg("[Profile.G]\nMatch=game.exe\nPriority=high\n"));
        r.tick(&cfg("[Profile.G]\nMatch=other.exe\nPriority=high\n"));
        assert_eq!(r.os.procs[&10].priority, NORMAL);
    }

    #[test]
    fn exited_processes_are_forgotten_and_never_restored() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe");
        let c = cfg("[Profile.G]\nMatch=game.exe\nPriority=high\n");
        r.tick(&c);
        r.os.remove(10);
        let calls = r.os.calls.len();
        r.ticks(&c, 3);
        r.stop();
        assert_eq!(r.os.calls.len(), calls, "no restore attempted for a dead process");
        assert!(r.m.report(&mut r.os, &c).applied.is_empty());
        // A new process reusing the pid under another name is not treated as the old one.
        r.os.add(10, "unrelated.exe");
        r.ticks(&c, 2);
        assert_eq!(r.os.procs[&10].priority, NORMAL);
    }

    #[test]
    fn clean_stop_restores_everything() {
        let mut r = Rig::new(fake_9950x3d(true));
        r.os.add(10, "game.exe");
        r.tick(&cfg(GAME));
        assert_eq!(r.os.procs[&10].priority, HIGH);
        r.stop();
        assert_eq!((r.os.procs[&10].priority, r.os.procs[&10].affinity.mask), (NORMAL, 0xFFFF_FFFF));
        assert!(r.log.ids(id::PROFILE_RESTORED).iter().all(|e| e.info.contains("engine stopping")));
    }

    #[test]
    fn restore_leaves_alone_a_value_someone_else_changed() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe");
        let c = cfg("[Profile.G]\nMatch=game.exe\nPriority=high\n");
        r.tick(&c);
        r.os.procs.get_mut(&10).unwrap().priority = IDLE; // the user changed it in Task Manager
        r.stop();
        assert_eq!(r.os.procs[&10].priority, IDLE);
        assert!(r.log.ids(id::PROFILE_RESTORED)[0].action.contains("NOT restored"));
    }

    // ---- refusal, retry, give-up ------------------------------------------------------------

    #[test]
    fn a_refused_change_is_retried_five_times_over_30_seconds_then_given_up() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe");
        r.os.refuse.insert((10, Op::Affinity), Refusal { code: 5, remaining: None });
        let c = cfg("[Profile.G]\nMatch=game.exe\nAffinity=0-1\n");
        let mut tries_at = vec![];
        for _ in 0..40 {
            r.tick(&c);
            tries_at.push(*r.os.tries.get(&(10, Op::Affinity)).unwrap_or(&0));
        }
        // first attempt at t=1s; then +2s, +6s, +14s, +30s after it.
        assert_eq!(tries_at[0], 1, "attempt 1 immediately");
        assert_eq!(tries_at[2], 2, "attempt 2 at +2s");
        assert_eq!(tries_at[6], 3, "attempt 3 at +6s");
        assert_eq!(tries_at[14], 4, "attempt 4 at +14s");
        assert_eq!(tries_at[29], 4, "still waiting just before +30s");
        assert_eq!(tries_at[30], 5, "attempt 5 at +30s");
        assert_eq!(tries_at[39], 5, "then it stops: never retried forever");
        let rep = r.m.report(&mut r.os, &c);
        let ch = &rep.applied[0].changes[0];
        assert_eq!((ch.result.as_str(), ch.win32_error, ch.attempts), ("refused", Some(5), 5));
        assert_eq!(rep.refused.len(), 1);
        assert_eq!(r.log.ids(id::PROFILE_RETRY).len(), 1, "one retry notice, not one per attempt");
        // Two sentences, each properly ended: the refusal text, then what happens next.
        let retry = r.log.ids(id::PROFILE_RETRY);
        assert!(retry[0].info.contains("does not try to get around it. Retrying up to 5 times"), "{}", retry[0].info);
        let refused = r.log.ids(id::PROFILE_REFUSED);
        assert!(refused[0].info.contains("does not try to get around it. Gave up after 5 attempt(s)"), "{}", refused[0].info);
        assert_eq!(refused.len(), 1);
        assert!(refused[0].info.contains("NOT applied") && refused[0].info.contains("5 attempt"), "{}", refused[0].info);
        assert_eq!(r.os.procs[&10].affinity.mask, 0xF, "nothing was changed");
    }

    #[test]
    fn access_denied_is_explained_in_plain_english() {
        let t = refusal_text(Aspect::Affinity, 5);
        assert!(t.contains("Windows refused to change the CPU affinity"), "{t}");
        assert!(t.contains("Win32 error 5"), "{t}");
        assert!(t.contains("anti-cheat") && t.contains("does not try to get around it"), "{t}");
        assert!(t.ends_with("get around it."), "complete sentence: {t}");
        let other = refusal_text(Aspect::Priority, 87);
        assert!(other.contains("Win32 error 87") && !other.contains("anti-cheat"), "{other}");
        assert!(other.ends_with("(Win32 error 87)."), "complete sentence: {other}");
    }

    #[test]
    fn a_refusal_that_clears_during_the_startup_race_is_applied() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe");
        r.os.refuse.insert((10, Op::Priority), Refusal { code: 5, remaining: Some(2) });
        let c = cfg("[Profile.G]\nMatch=game.exe\nPriority=high\n");
        r.ticks(&c, 4);
        assert_eq!(r.os.procs[&10].priority, NORMAL, "still refused at +3s");
        r.ticks(&c, 8);
        assert_eq!(r.os.procs[&10].priority, HIGH, "third attempt (+6s) succeeds");
        assert_eq!(r.os.tries[&(10, Op::Priority)], 3);
        assert_eq!(r.m.report(&mut r.os, &c).applied[0].changes[0].result, "applied");
        assert!(r.m.report(&mut r.os, &c).refused.is_empty());
    }

    #[test]
    fn one_refused_aspect_does_not_block_the_others() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe");
        r.os.refuse.insert((10, Op::Affinity), Refusal { code: 5, remaining: None });
        r.ticks(&cfg("[Profile.G]\nMatch=game.exe\nPriority=high\nAffinity=0-1\n"), 3);
        assert_eq!(r.os.procs[&10].priority, HIGH);
        assert_eq!(r.os.procs[&10].affinity.mask, 0xF);
    }

    #[test]
    fn changing_the_profile_resets_the_retry_budget() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe");
        r.os.refuse.insert((10, Op::Affinity), Refusal { code: 5, remaining: None });
        r.ticks(&cfg("[Profile.G]\nMatch=game.exe\nAffinity=0-1\n"), 40);
        assert_eq!(r.os.tries[&(10, Op::Affinity)], 5);
        r.os.refuse.clear();
        r.ticks(&cfg("[Profile.G]\nMatch=game.exe\nAffinity=0-2\n"), 2);
        assert_eq!(r.os.procs[&10].affinity.mask, 0b111, "a new request gets a fresh attempt");
    }

    #[test]
    fn read_back_mismatch_is_reported_as_applied_but_different_and_not_fought() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe");
        r.os.rewrite.insert((10, Op::Affinity), SimVal::Aff(Affinity { group: 0, mask: 0b1100 }));
        let c = cfg("[Profile.G]\nMatch=game.exe\nAffinity=0-1\n");
        r.ticks(&c, 20);
        assert_eq!(r.os.tries[&(10, Op::Affinity)], 1, "no retry loop against a process that rewrites it");
        let d = r.log.ids(id::PROFILE_DIFFERS);
        assert_eq!(d.len(), 1);
        assert!(d[0].info.contains("wanted 0x3, now 0xC"), "{}", d[0].info);
        let ch = &r.m.report(&mut r.os, &c).applied[0].changes[0];
        assert_eq!((ch.result.as_str(), ch.wanted.as_str(), ch.current.as_str()), ("applied_but_different", "0x3", "0xC"));
        // We did change it, so we still undo our change on stop.
        r.os.rewrite.clear();
        r.stop();
        assert_eq!(r.os.procs[&10].affinity.mask, 0xF);
    }

    // ---- inexpressible rules ----------------------------------------------------------------

    #[test]
    fn rules_the_machine_cannot_express_are_refused_with_a_clear_reason_and_no_os_call() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "a.exe");
        r.os.add(11, "b.exe");
        r.os.add(12, "c.exe");
        let c = cfg("[Profile.A]\nMatch=a.exe\nAffinity=0x1FFFF\n[Profile.B]\nMatch=b.exe\nCpuGroup=vcache\n[Profile.C]\nMatch=c.exe\nAffinity=0-9\n");
        r.ticks(&c, 3);
        assert_eq!(r.os.calls.len(), 0);
        let rep = r.m.report(&mut r.os, &c);
        assert_eq!(rep.refused.len(), 3);
        let msgs: Vec<&str> = rep.refused.iter().map(|x| x.message.as_str()).collect();
        assert!(msgs[0].contains("selects processor 4"), "{msgs:?}");
        assert!(msgs[1].contains("no cache-heavy"), "{msgs:?}");
        assert!(msgs[2].contains("does not exist"), "{msgs:?}");
        assert!(rep.refused.iter().all(|x| x.win32_error.is_none() && x.attempts == 0));
    }

    #[test]
    fn affinity_spanning_processor_groups_is_rejected_but_cpu_sets_work() {
        let mut r = Rig::new(crate::topology::fake_two_groups());
        r.os.add(10, "a.exe");
        r.os.add(11, "b.exe");
        let c = cfg("[Profile.A]\nMatch=a.exe\nAffinity=60-70\n[Profile.B]\nMatch=b.exe\nCpuSets=60-70\n");
        r.tick(&c);
        assert_eq!(r.os.count("aff"), 0);
        assert!(r.m.report(&mut r.os, &c).refused[0].message.contains("span processor groups"));
        assert_eq!(r.os.procs[&11].cpu_sets.len(), 11);
    }

    #[test]
    fn protected_processes_get_no_affinity_or_cpu_sets_and_critical_ones_nothing() {
        let mut r = Rig::new(fake_quad());
        r.os.add(100, "explorer.exe");
        r.os.add(101, "lsass.exe");
        let c = cfg("[Profile.E]\nMatch=explorer.exe\nPriority=abovenormal\nAffinity=0-1\n[Profile.L]\nMatch=lsass.exe\nPriority=high\nCpuSets=0-1\n");
        r.ticks(&c, 3);
        assert_eq!(r.os.procs[&100].priority, ABOVE_NORMAL, "exclusion-list processes may still get a priority");
        assert_eq!(r.os.procs[&100].affinity.mask, 0xF, "...but never an affinity");
        assert_eq!((r.os.procs[&101].priority, r.os.procs[&101].affinity.mask), (NORMAL, 0xF), "critical: nothing at all");
        assert!(r.os.procs[&101].cpu_sets.is_empty());
        assert_eq!(r.os.count("sets"), 0);
        let rep = r.m.report(&mut r.os, &c);
        let msgs: Vec<_> = rep.refused.iter().map(|x| (x.process.as_str(), x.aspect.as_str(), x.message.as_str())).collect();
        assert!(msgs.iter().any(|m| m.0 == "explorer.exe" && m.1 == "affinity" && m.2.contains("built-in exclusion list")), "{msgs:?}");
        assert!(msgs.iter().any(|m| m.0 == "lsass.exe" && m.1 == "priority" && m.2.contains("critical")), "{msgs:?}");
        assert_eq!(protection("svchost.exe", 5, 1), Protection::ExclusionList);
        assert_eq!(protection("System", 4, 1), Protection::Critical);
        assert_eq!(protection("anything.exe", 77, 77), Protection::Critical, "ourselves");
        assert_eq!(protection("game.exe", 77, 1), Protection::None);
    }

    #[test]
    fn matching_by_full_path_uses_the_process_path() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe").path = r"C:\Games\Steam\game.exe".into();
        r.os.add(11, "game.exe").path = r"D:\Other\game.exe".into();
        r.tick(&cfg("[Profile.G]\nMatch=C:\\Games\\*\\game.exe\nPriority=high\n"));
        assert_eq!((r.os.procs[&10].priority, r.os.procs[&11].priority), (HIGH, NORMAL));
    }

    #[test]
    fn first_matching_enabled_profile_wins() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe");
        r.tick(&cfg("[Profile.Off]\nMatch=game.exe\nPriority=idle\nEnabled=false\n[Profile.A]\nMatch=game*\nPriority=high\n[Profile.B]\nMatch=game.exe\nPriority=abovenormal\n"));
        assert_eq!(r.os.procs[&10].priority, HIGH);
    }

    // ---- game mode ------------------------------------------------------------------------

    const GM: &str = "[Profile.Game]\nMatch=game.exe\nGameMode=true\n[GameMode]\nBackgroundPriority=true\nPowerPlan={8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c}\n";

    fn game_rig() -> Rig {
        let mut r = Rig::new(fake_quad());
        r.os.add(20, "chrome.exe");
        r.os.add(21, "code.exe");
        r.os.add(22, "svchost.exe");
        r.os.add(23, "service.exe").session0 = true;
        r.os.add(24, "busy.exe").priority = HIGH;
        r
    }

    #[test]
    fn game_mode_enters_logs_the_trigger_and_leaves_with_full_undo() {
        let mut r = game_rig();
        let c = cfg(GM);
        let balanced = r.os.power;
        r.tick(&c);
        assert!(!r.m.game_active());
        r.os.add(10, "game.exe");
        r.os.add(11, "chrome_child.exe").ppid_of(10);
        r.tick(&c);
        assert!(r.m.game_active());
        let enter = r.log.ids(id::GAME_ENTER);
        assert_eq!(enter.len(), 1);
        assert!(enter[0].info.contains("game.exe (pid 10)") && enter[0].info.contains("'Game'"), "{}", enter[0].info);
        // background priority: only ordinary, non-excluded, non-service, Normal-priority processes
        assert_eq!(r.os.procs[&20].priority, BELOW_NORMAL);
        assert_eq!(r.os.procs[&21].priority, BELOW_NORMAL);
        assert_eq!(r.os.procs[&22].priority, NORMAL, "built-in exclusion list untouched");
        assert_eq!(r.os.procs[&23].priority, NORMAL, "services untouched");
        assert_eq!(r.os.procs[&24].priority, HIGH, "only Normal-priority processes are lowered");
        assert_eq!(r.os.procs[&10].priority, NORMAL, "the game itself is not lowered");
        assert_eq!(r.os.procs[&11].priority, NORMAL, "the game's own child is not lowered");
        assert_eq!(r.os.power.to_string(), "{8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c}");
        let st = r.m.report(&mut r.os, &c).game;
        assert!(st.active && st.power_plan_switched && st.background_lowered == 2);
        assert_eq!(st.triggered_by.unwrap().pid, 10);
        // A process that starts during game mode is lowered too.
        r.os.add(25, "late.exe");
        r.tick(&c);
        assert_eq!(r.os.procs[&25].priority, BELOW_NORMAL);
        // The game exits: everything is undone.
        r.os.remove(10);
        r.tick(&c);
        assert!(!r.m.game_active());
        for pid in [20, 21, 25] {
            assert_eq!(r.os.procs[&pid].priority, NORMAL, "pid {pid}");
        }
        assert_eq!(r.os.power, balanced);
        let leave = r.log.ids(id::GAME_LEAVE);
        assert_eq!(leave.len(), 1);
        assert!(leave[0].info.contains("last game-mode process exited") && leave[0].name == "game.exe");
    }

    #[test]
    fn game_mode_off_by_default_changes_nothing_but_tracking() {
        let mut r = game_rig();
        let c = cfg("[Profile.Game]\nMatch=game.exe\nGameMode=true\n");
        r.os.add(10, "game.exe");
        r.ticks(&c, 3);
        assert!(r.m.game_active());
        assert_eq!(r.os.calls.len(), 0, "BackgroundPriority and PowerPlan both default to off");
        r.os.remove(10);
        r.tick(&c);
        assert!(!r.m.game_active());
    }

    #[test]
    fn game_mode_is_undone_on_reload_that_disables_it() {
        for off in ["[Profile.Game]\nMatch=game.exe\nGameMode=false\n", "[Profile.Game]\nMatch=game.exe\nGameMode=true\nEnabled=false\n", "[Other]\nx=1\n"] {
            let mut r = game_rig();
            let before = r.os.power;
            r.os.add(10, "game.exe");
            r.ticks(&cfg(GM), 2);
            assert_eq!(r.os.procs[&20].priority, BELOW_NORMAL);
            r.tick(&cfg(off));
            assert!(!r.m.game_active(), "{off}");
            assert_eq!((r.os.procs[&20].priority, r.os.power), (NORMAL, before), "{off}");
        }
    }

    #[test]
    fn turning_off_background_priority_or_power_plan_while_active_undoes_just_that() {
        let mut r = game_rig();
        let before = r.os.power;
        r.os.add(10, "game.exe");
        r.ticks(&cfg(GM), 2);
        r.tick(&cfg("[Profile.Game]\nMatch=game.exe\nGameMode=true\n[GameMode]\nPowerPlan={8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c}\n"));
        assert_eq!(r.os.procs[&20].priority, NORMAL);
        assert_ne!(r.os.power, before, "power plan still switched");
        r.tick(&cfg("[Profile.Game]\nMatch=game.exe\nGameMode=true\n"));
        assert_eq!(r.os.power, before);
        assert!(r.m.game_active());
    }

    #[test]
    fn game_mode_is_undone_on_clean_stop() {
        let mut r = game_rig();
        let before = r.os.power;
        r.os.add(10, "game.exe");
        r.ticks(&cfg(GM), 2);
        r.stop();
        assert!(!r.m.game_active());
        assert_eq!((r.os.procs[&20].priority, r.os.power), (NORMAL, before));
        assert_eq!(r.log.ids(id::GAME_LEAVE)[0].info, "engine stopping");
    }

    #[test]
    fn game_mode_does_not_fight_the_user_or_fail_loudly() {
        let mut r = game_rig();
        r.os.add(10, "game.exe");
        r.os.power_set_error = Some(5);
        r.ticks(&cfg(GM), 3);
        assert_eq!(r.os.count("power"), 0);
        assert!(r.log.0.iter().any(|e| e.id == id::GAME_INFO && e.action.contains("power plan not changed")));
        assert_eq!(r.log.0.iter().filter(|e| e.action.contains("power plan not changed")).count(), 1, "reported once, not every tick");
        // The user changes a lowered process's priority; leaving game mode must not overwrite it.
        r.os.procs.get_mut(&21).unwrap().priority = IDLE;
        r.os.remove(10);
        r.tick(&cfg(GM));
        assert_eq!(r.os.procs[&21].priority, IDLE);
        assert_eq!(r.os.procs[&20].priority, NORMAL);
    }

    #[test]
    fn user_changing_the_power_plan_during_game_mode_is_respected() {
        let mut r = game_rig();
        r.os.add(10, "game.exe");
        r.ticks(&cfg(GM), 2);
        let user_plan = Guid::parse("{a1841308-3541-4fab-bc81-f71556f20b4a}").unwrap();
        r.os.power = user_plan;
        r.os.remove(10);
        r.tick(&cfg(GM));
        assert_eq!(r.os.power, user_plan);
    }

    #[test]
    fn game_mode_user_exclusions_are_honoured() {
        let mut r = game_rig();
        r.os.add(10, "game.exe");
        r.ticks(&cfg(&format!("{GM}[OutOfControlProcessRestraint]\nOocExclusions=chrome.exe\n")), 2);
        assert_eq!(r.os.procs[&20].priority, NORMAL);
        assert_eq!(r.os.procs[&21].priority, BELOW_NORMAL);
    }

    // ---- Restraint bridge ----------------------------------------------------------------

    struct FakeBridge {
        restrained: HashMap<u32, u32>,
    }
    impl Bridge for FakeBridge {
        fn restraint_original(&self, pid: u32) -> Option<u32> {
            self.restrained.get(&pid).copied()
        }
        fn retarget(&mut self, pid: u32, class: u32) -> bool {
            self.restrained.get_mut(&pid).map(|c| *c = class).is_some()
        }
    }

    #[test]
    fn a_profile_applied_to_a_restrained_process_becomes_what_restraint_restores() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe").priority = BELOW_NORMAL; // Restraint lowered it from Normal
        let mut b = FakeBridge { restrained: HashMap::from([(10, NORMAL)]) };
        let c = cfg("[Profile.G]\nMatch=game.exe\nPriority=high\n");
        r.t += 1000;
        let procs = r.os.processes();
        r.m.tick(&mut r.os, &c, &mut r.log, &procs, 1, r.t, &mut b);
        assert_eq!(b.restrained[&10], HIGH, "Restraint will now release it to the profile priority");
        assert_eq!(r.os.procs[&10].priority, BELOW_NORMAL, "the restraint itself is left in place");
        assert_eq!(r.os.count("prio"), 0);
        // Removing the profile while restrained sends it back to the true original.
        let none = cfg("[Other]\nx=1\n");
        r.t += 1000;
        r.m.tick(&mut r.os, &none, &mut r.log, &procs, 1, r.t, &mut b);
        assert_eq!(b.restrained[&10], NORMAL);
    }

    #[test]
    fn report_is_empty_for_configs_without_profiles() {
        let mut r = Rig::new(fake_quad());
        r.os.add(10, "game.exe");
        let c = cfg("[OutOfControlProcessRestraint]\nOocOn=true\n");
        r.ticks(&c, 3);
        let rep = r.m.report(&mut r.os, &c);
        assert!(rep.profiles.is_empty() && rep.applied.is_empty() && rep.refused.is_empty() && !rep.game.active);
        assert_eq!(r.os.calls.len(), 0);
    }
}
