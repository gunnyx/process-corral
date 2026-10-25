//! A scriptable fake operating system for tests (and for exercising the engine without touching
//! real processes). It models per-process priority, affinity and CPU Sets, refusals that can clear
//! after N attempts, values the "OS" changes behind our back, a power plan, and records every call.

use crate::config::NORMAL;
use crate::guid::Guid;
use crate::os::{Affinity, Os, ProcInfo};
use crate::topology::Topology;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    Priority,
    Affinity,
    CpuSets,
}

#[derive(Clone, Copy, Debug)]
pub struct Refusal {
    pub code: u32,
    /// `Some(n)`: refuse the next n attempts, then allow. `None`: refuse forever.
    pub remaining: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SimVal {
    Prio(u32),
    Aff(Affinity),
    Sets(Vec<u32>),
}

pub struct SimProc {
    pub info: ProcInfo,
    pub priority: u32,
    pub affinity: Affinity,
    pub cpu_sets: Vec<u32>,
    /// CPU time added each time the engine samples it (100 ns units).
    pub cpu_per_sample: u64,
    cpu_total: u64,
    pub session0: bool,
    pub path: String,
}

impl SimProc {
    pub fn ppid_of(&mut self, ppid: u32) -> &mut Self {
        self.info.ppid = ppid;
        self
    }
}

pub struct SimOs {
    pub topo: Arc<Topology>,
    pub procs: BTreeMap<u32, SimProc>,
    pub refuse: HashMap<(u32, Op), Refusal>,
    /// How many times each change was attempted (successful or not).
    pub tries: HashMap<(u32, Op), u32>,
    /// After a successful apply, the "OS" immediately rewrites the value (read-back mismatch).
    pub rewrite: HashMap<(u32, Op), SimVal>,
    pub sys_pct: f64,
    pub fg: Option<u32>,
    pub power: Guid,
    pub power_set_error: Option<u32>,
    pub power_get_error: Option<u32>,
    /// Every Os call that changes something: "prio 12 32", "aff 12 0xF", ...
    pub calls: Vec<String>,
    pub killed: Vec<u32>,
}

impl SimOs {
    pub fn new(topo: Topology) -> SimOs {
        SimOs {
            topo: Arc::new(topo),
            procs: BTreeMap::new(),
            refuse: HashMap::new(),
            tries: HashMap::new(),
            rewrite: HashMap::new(),
            sys_pct: 0.0,
            fg: None,
            power: Guid::parse("{381b4222-f694-41f0-9685-ff5bb260df2e}").unwrap(), // Balanced
            power_set_error: None,
            power_get_error: None,
            calls: vec![],
            killed: vec![],
        }
    }

    pub fn add(&mut self, pid: u32, name: &str) -> &mut SimProc {
        let all: u64 = if self.topo.logical_count() >= 64 { u64::MAX } else { (1u64 << self.topo.logical_count()) - 1 };
        self.procs.entry(pid).or_insert(SimProc {
            info: ProcInfo { pid, ppid: 1, name: name.into() },
            priority: NORMAL,
            affinity: Affinity { group: 0, mask: all },
            cpu_sets: vec![],
            cpu_per_sample: 0,
            cpu_total: 0,
            session0: false,
            path: format!(r"C:\Apps\{name}"),
        })
    }

    pub fn remove(&mut self, pid: u32) {
        self.procs.remove(&pid);
    }

    pub fn all_mask(&self) -> u64 {
        if self.topo.logical_count() >= 64 { u64::MAX } else { (1u64 << self.topo.logical_count()) - 1 }
    }

    pub fn count(&self, prefix: &str) -> usize {
        self.calls.iter().filter(|c| c.starts_with(prefix)).count()
    }

    fn gate(&mut self, pid: u32, op: Op) -> Result<(), u32> {
        if !self.procs.contains_key(&pid) {
            return Err(87); // ERROR_INVALID_PARAMETER: no such process
        }
        *self.tries.entry((pid, op)).or_insert(0) += 1;
        if let Some(r) = self.refuse.get_mut(&(pid, op)) {
            match &mut r.remaining {
                None => return Err(r.code),
                Some(0) => {}
                Some(n) => {
                    *n -= 1;
                    return Err(r.code);
                }
            }
        }
        Ok(())
    }

    fn after_apply(&mut self, pid: u32, op: Op) {
        match (self.rewrite.get(&(pid, op)).cloned(), self.procs.get_mut(&pid)) {
            (Some(SimVal::Prio(c)), Some(p)) => p.priority = c,
            (Some(SimVal::Aff(a)), Some(p)) => p.affinity = a,
            (Some(SimVal::Sets(s)), Some(p)) => p.cpu_sets = s,
            _ => {}
        }
    }
}

impl Os for SimOs {
    fn processes(&mut self) -> Vec<ProcInfo> {
        self.procs.values().map(|p| p.info.clone()).collect()
    }
    fn cpu_time(&mut self, pid: u32) -> Option<u64> {
        let p = self.procs.get_mut(&pid)?;
        p.cpu_total += p.cpu_per_sample;
        Some(p.cpu_total)
    }
    fn system_cpu_pct(&mut self) -> f64 {
        self.sys_pct
    }
    fn cpu_count(&self) -> u32 {
        self.topo.logical_count() as u32
    }
    fn foreground_pid(&mut self) -> Option<u32> {
        self.fg
    }
    fn is_session0(&mut self, pid: u32) -> bool {
        self.procs.get(&pid).is_some_and(|p| p.session0)
    }
    fn path(&mut self, pid: u32) -> String {
        self.procs.get(&pid).map(|p| p.path.clone()).unwrap_or_default()
    }
    fn priority(&mut self, pid: u32) -> Option<u32> {
        self.procs.get(&pid).map(|p| p.priority)
    }
    fn set_priority(&mut self, pid: u32, class: u32) -> bool {
        self.apply_priority(pid, class).is_ok()
    }
    fn affinity(&mut self, pid: u32) -> Option<u64> {
        self.procs.get(&pid).map(|p| p.affinity.mask)
    }
    fn set_affinity(&mut self, pid: u32, mask: u64) -> bool {
        let group = self.procs.get(&pid).map_or(0, |p| p.affinity.group);
        self.apply_affinity(pid, Affinity { group, mask }).is_ok()
    }
    fn terminate(&mut self, pid: u32) -> bool {
        self.killed.push(pid);
        self.procs.remove(&pid).is_some()
    }

    fn topology(&mut self) -> Arc<Topology> {
        self.topo.clone()
    }
    fn read_priority(&mut self, pid: u32) -> Result<u32, u32> {
        self.procs.get(&pid).map(|p| p.priority).ok_or(87)
    }
    fn apply_priority(&mut self, pid: u32, class: u32) -> Result<(), u32> {
        self.gate(pid, Op::Priority)?;
        self.procs.get_mut(&pid).unwrap().priority = class;
        self.calls.push(format!("prio {pid} {class:#x}"));
        self.after_apply(pid, Op::Priority);
        Ok(())
    }
    fn read_affinity(&mut self, pid: u32) -> Result<Affinity, u32> {
        self.procs.get(&pid).map(|p| p.affinity).ok_or(87)
    }
    fn apply_affinity(&mut self, pid: u32, a: Affinity) -> Result<(), u32> {
        self.gate(pid, Op::Affinity)?;
        self.procs.get_mut(&pid).unwrap().affinity = a;
        self.calls.push(format!("aff {pid} {:#x}", a.mask));
        self.after_apply(pid, Op::Affinity);
        Ok(())
    }
    fn read_cpu_sets(&mut self, pid: u32) -> Result<Vec<u32>, u32> {
        self.procs.get(&pid).map(|p| p.cpu_sets.clone()).ok_or(87)
    }
    fn apply_cpu_sets(&mut self, pid: u32, ids: &[u32]) -> Result<(), u32> {
        self.gate(pid, Op::CpuSets)?;
        let mut v = ids.to_vec();
        v.sort_unstable();
        self.procs.get_mut(&pid).unwrap().cpu_sets = v;
        self.calls.push(format!("sets {pid} {}", ids.len()));
        self.after_apply(pid, Op::CpuSets);
        Ok(())
    }
    fn power_plan(&mut self) -> Result<Guid, u32> {
        match self.power_get_error {
            Some(e) => Err(e),
            None => Ok(self.power),
        }
    }
    fn set_power_plan(&mut self, plan: &Guid) -> Result<(), u32> {
        if let Some(e) = self.power_set_error {
            return Err(e);
        }
        self.calls.push(format!("power {plan}"));
        self.power = *plan;
        Ok(())
    }
}
