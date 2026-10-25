//! CPU topology: processor groups, physical cores, SMT siblings, efficiency classes and L3 cache
//! groups ("CCDs"), plus the rules for turning a selection of CPUs into an affinity mask or CPU Set
//! ids. Nothing here knows about any particular chip: the "V-Cache" group is simply the L3 group
//! with the strictly largest cache, wherever it sits.
//!
//! Logical processors are numbered globally in (processor group, bit) order. On a machine with at
//! most 64 logical processors that is exactly the usual affinity-mask bit number.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Cpu {
    /// Global logical processor index.
    pub index: usize,
    pub group: u16,
    /// Bit position inside its processor group (0..63).
    pub bit: u8,
    /// Physical core number (global).
    pub core: usize,
    /// 0 for the first hardware thread of a core, 1.. for SMT siblings.
    pub smt_rank: u8,
    pub efficiency_class: u8,
    /// Index into `Topology::l3`, if the cache hierarchy is known.
    pub l3: Option<usize>,
    /// Id for SetProcessDefaultCpuSets, if the OS reports CPU Sets.
    pub cpu_set_id: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct L3Group {
    pub index: usize,
    pub size_bytes: u64,
    /// Global logical processor indices.
    pub cpus: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Topology {
    pub groups: u16,
    pub cpus: Vec<Cpu>,
    pub l3: Vec<L3Group>,
}

/// How an L3 group is named in a profile's `CpuGroup` key.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupRef {
    VCache,
    Frequency,
    Index(usize),
}

impl std::fmt::Display for GroupRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GroupRef::VCache => f.write_str("vcache"),
            GroupRef::Frequency => f.write_str("frequency"),
            GroupRef::Index(i) => write!(f, "{i}"),
        }
    }
}

impl Topology {
    pub fn logical_count(&self) -> usize {
        self.cpus.len()
    }

    pub fn core_count(&self) -> usize {
        self.cpus.iter().map(|c| c.core + 1).max().unwrap_or(0)
    }

    pub fn has_smt(&self) -> bool {
        self.cpus.iter().any(|c| c.smt_rank > 0)
    }

    pub fn size_mb(g: &L3Group) -> f64 {
        g.size_bytes as f64 / (1024.0 * 1024.0)
    }

    /// The L3 group with the strictly largest cache. `None` if there is no cache information, only
    /// one group, or all groups are the same size (nothing is "cache-heavy" then).
    pub fn vcache_group(&self) -> Option<usize> {
        let max = self.l3.iter().map(|g| g.size_bytes).max()?;
        let min = self.l3.iter().map(|g| g.size_bytes).min()?;
        if max == 0 || max == min {
            return None;
        }
        let mut best = self.l3.iter().filter(|g| g.size_bytes == max);
        let first = best.next()?;
        // Several groups sharing the maximum size: still cache-heavy, but ambiguous; take the first.
        Some(first.index)
    }

    /// Every L3 group that is not the cache-heavy one.
    pub fn frequency_groups(&self) -> Vec<usize> {
        match self.vcache_group() {
            Some(v) => self.l3.iter().filter(|g| g.size_bytes < self.l3[v].size_bytes).map(|g| g.index).collect(),
            None => vec![],
        }
    }

    /// "vcache", "frequency" or "" for display.
    pub fn label(&self, l3_index: usize) -> &'static str {
        if self.vcache_group() == Some(l3_index) {
            "vcache"
        } else if self.frequency_groups().contains(&l3_index) {
            "frequency"
        } else {
            ""
        }
    }

    /// Logical processors of a named group, or a clear error if this machine doesn't have it.
    pub fn resolve_group(&self, g: GroupRef) -> Result<Vec<usize>, String> {
        let idx: Vec<usize> = match g {
            GroupRef::VCache => vec![self.vcache_group().ok_or_else(|| {
                "CpuGroup=vcache: this machine has no cache-heavy L3 group (its L3 groups are all the same size, or the cache layout is unknown)".to_string()
            })?],
            GroupRef::Frequency => {
                let f = self.frequency_groups();
                if f.is_empty() {
                    return Err("CpuGroup=frequency: this machine has no distinct frequency L3 group (no L3 group is smaller than the cache-heavy one)".into());
                }
                f
            }
            GroupRef::Index(i) => {
                if i >= self.l3.len() {
                    return Err(format!(
                        "CpuGroup={i}: this machine has {} L3 group(s) (valid indexes 0-{})",
                        self.l3.len(),
                        self.l3.len().saturating_sub(1)
                    ));
                }
                vec![i]
            }
        };
        let mut cpus: Vec<usize> = idx.iter().flat_map(|i| self.l3[*i].cpus.iter().copied()).collect();
        cpus.sort_unstable();
        Ok(cpus)
    }

    /// Check that every index exists on this machine.
    pub fn check_indexes(&self, cpus: &[usize]) -> Result<(), String> {
        let n = self.logical_count();
        if let Some(bad) = cpus.iter().find(|c| **c >= n) {
            return Err(format!(
                "processor {bad} does not exist: this machine has {n} logical processor(s) (0-{})",
                n.saturating_sub(1)
            ));
        }
        Ok(())
    }

    /// Per-processor-group masks for a set of logical processors.
    pub fn group_masks(&self, cpus: &[usize]) -> BTreeMap<u16, u64> {
        let mut m = BTreeMap::new();
        for c in cpus.iter().filter_map(|i| self.cpus.get(*i)) {
            *m.entry(c.group).or_insert(0u64) |= 1u64 << c.bit;
        }
        m
    }

    /// An affinity mask can only address one processor group. Reject (never truncate) otherwise.
    pub fn affinity_for(&self, cpus: &[usize]) -> Result<(u16, u64), String> {
        self.check_indexes(cpus)?;
        let masks = self.group_masks(cpus);
        match masks.len() {
            0 => Err("no processors selected".into()),
            1 => Ok(masks.into_iter().next().unwrap()),
            _ => Err(format!(
                "the selected processors span processor groups {}; an affinity mask can only address one group, so use CpuSets for this",
                masks.keys().map(|g| g.to_string()).collect::<Vec<_>>().join(" and ")
            )),
        }
    }

    pub fn cpu_set_ids_for(&self, cpus: &[usize]) -> Result<Vec<u32>, String> {
        self.check_indexes(cpus)?;
        let mut ids = Vec::with_capacity(cpus.len());
        for i in cpus {
            ids.push(self.cpus[*i].cpu_set_id.ok_or_else(|| {
                "CPU Sets are not available on this system (Windows did not report CPU Set ids)".to_string()
            })?);
        }
        ids.sort_unstable();
        ids.dedup();
        Ok(ids)
    }

    /// Logical processor indexes for CPU Set ids (unknown ids are ignored).
    pub fn cpus_for_ids(&self, ids: &[u32]) -> Vec<usize> {
        self.cpus.iter().filter(|c| c.cpu_set_id.is_some_and(|id| ids.contains(&id))).map(|c| c.index).collect()
    }

    /// Compact list form: `0-15,24,28-31`.
    pub fn cpu_list(cpus: &[usize]) -> String {
        let mut v: Vec<usize> = cpus.to_vec();
        v.sort_unstable();
        v.dedup();
        let mut out: Vec<String> = vec![];
        let mut i = 0;
        while i < v.len() {
            let mut j = i;
            while j + 1 < v.len() && v[j + 1] == v[j] + 1 {
                j += 1;
            }
            out.push(if j == i { v[i].to_string() } else { format!("{}-{}", v[i], v[j]) });
            i = j + 1;
        }
        out.join(",")
    }

    // ------------------------------------------------------------ builders

    /// `n` cores, no SMT, one L3 group of unknown size. Used when detection is unavailable.
    pub fn simple(n: usize) -> Topology {
        synthetic(&[Ccd { cores: n.max(1), l3_mb: 0 }], false)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Ccd {
    pub cores: usize,
    pub l3_mb: u64,
}

/// Build a topology from CCD descriptions, filling processor groups of 64 logical processors.
/// SMT siblings are adjacent (core 0 = cpus 0,1), like Windows numbers them.
pub fn synthetic(ccds: &[Ccd], smt: bool) -> Topology {
    let per = if smt { 2 } else { 1 };
    let mut cpus = vec![];
    let mut l3 = vec![];
    let mut core = 0usize;
    for (gi, ccd) in ccds.iter().enumerate() {
        let mut members = vec![];
        for _ in 0..ccd.cores {
            for rank in 0..per {
                let index = cpus.len();
                cpus.push(Cpu {
                    index,
                    group: (index / 64) as u16,
                    bit: (index % 64) as u8,
                    core,
                    smt_rank: rank as u8,
                    efficiency_class: 0,
                    l3: Some(gi),
                    cpu_set_id: Some(0x100 + index as u32),
                });
                members.push(index);
            }
            core += 1;
        }
        l3.push(L3Group { index: gi, size_bytes: ccd.l3_mb * 1024 * 1024, cpus: members });
    }
    let groups = cpus.len().div_ceil(64).max(1) as u16;
    Topology { groups, cpus, l3 }
}

/// Ryzen 9 9950X3D-like: 2 CCDs x 8 cores x 2 threads = 32 logical processors. `vcache_first`
/// chooses which CCD carries the 96 MB cache (it is not always CCD0).
pub fn fake_9950x3d(vcache_first: bool) -> Topology {
    let (a, b) = if vcache_first { (96, 32) } else { (32, 96) };
    synthetic(&[Ccd { cores: 8, l3_mb: a }, Ccd { cores: 8, l3_mb: b }], true)
}

/// A plain 4-core, no-SMT machine with one 8 MB L3.
pub fn fake_quad() -> Topology {
    synthetic(&[Ccd { cores: 4, l3_mb: 8 }], false)
}

/// 128 logical processors in two processor groups (4 CCDs of 16 cores x 2 threads).
pub fn fake_two_groups() -> Topology {
    synthetic(&[Ccd { cores: 16, l3_mb: 64 }; 4], true)
}

// ---------------------------------------------------------------- raw OS records

#[derive(Clone, Debug, PartialEq)]
pub struct CoreRec {
    pub efficiency: u8,
    pub masks: Vec<(u16, u64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CacheRec {
    pub level: u8,
    pub size: u64,
    /// Unified or data cache (instruction-only caches are ignored).
    pub data: bool,
    pub masks: Vec<(u16, u64)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CpuSetRec {
    pub id: u32,
    pub group: u16,
    pub logical_processor: u8,
}

fn bits(mask: u64) -> impl Iterator<Item = u8> {
    (0..64u8).filter(move |b| mask >> b & 1 == 1)
}

/// Combine the OS records into a `Topology`.
pub fn from_records(cores: &[CoreRec], caches: &[CacheRec], sets: &[CpuSetRec]) -> Topology {
    // Cores in (group, first bit) order.
    let mut cores: Vec<&CoreRec> = cores.iter().collect();
    let first = |c: &CoreRec| c.masks.iter().flat_map(|(g, m)| bits(*m).map(move |b| (*g, b))).min();
    cores.sort_by_key(|c| first(c));

    let mut cpus: Vec<Cpu> = vec![];
    for (core_idx, c) in cores.iter().enumerate() {
        let mut members: Vec<(u16, u8)> = c.masks.iter().flat_map(|(g, m)| bits(*m).map(move |b| (*g, b))).collect();
        members.sort_unstable();
        for (rank, (g, b)) in members.into_iter().enumerate() {
            cpus.push(Cpu {
                index: 0,
                group: g,
                bit: b,
                core: core_idx,
                smt_rank: rank.min(255) as u8,
                efficiency_class: c.efficiency,
                l3: None,
                cpu_set_id: None,
            });
        }
    }
    cpus.sort_by_key(|c| (c.group, c.bit));
    for (i, c) in cpus.iter_mut().enumerate() {
        c.index = i;
    }
    let pos: BTreeMap<(u16, u8), usize> = cpus.iter().map(|c| ((c.group, c.bit), c.index)).collect();

    for s in sets {
        if let Some(i) = pos.get(&(s.group, s.logical_processor)) {
            cpus[*i].cpu_set_id = Some(s.id);
        }
    }

    // L3 groups in order of their first processor.
    let mut l3: Vec<L3Group> = vec![];
    let mut l3_recs: Vec<&CacheRec> = caches.iter().filter(|c| c.level == 3 && c.data).collect();
    l3_recs.sort_by_key(|c| c.masks.iter().flat_map(|(g, m)| bits(*m).map(move |b| (*g, b))).min());
    for rec in l3_recs {
        let mut members: Vec<usize> =
            rec.masks.iter().flat_map(|(g, m)| bits(*m).filter_map(|b| pos.get(&(*g, b)).copied()).collect::<Vec<_>>()).collect();
        members.sort_unstable();
        if members.is_empty() {
            continue;
        }
        let index = l3.len();
        for m in &members {
            cpus[*m].l3 = Some(index);
        }
        l3.push(L3Group { index, size_bytes: rec.size, cpus: members });
    }
    let groups = cpus.iter().map(|c| c.group).max().map_or(1, |g| g + 1);
    Topology { groups, cpus, l3 }
}

// ---- GetLogicalProcessorInformationEx buffer (x64 layout) -----------------------------------

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

/// `count` GROUP_AFFINITY structs (16 bytes: mask, group, 3 reserved words) starting at `o`.
fn group_affinities(b: &[u8], o: usize, count: u16) -> Option<Vec<(u16, u64)>> {
    (0..count as usize).map(|i| Some((u16_at(b, o + i * 16 + 8)?, u64_at(b, o + i * 16)?))).collect()
}

/// Parse the records returned by `GetLogicalProcessorInformationEx(RelationAll)`. Unknown or
/// truncated records are skipped rather than trusted.
pub fn parse_glpi_ex(buf: &[u8]) -> (Vec<CoreRec>, Vec<CacheRec>) {
    let (mut cores, mut caches) = (vec![], vec![]);
    let mut off = 0usize;
    while off + 8 <= buf.len() {
        let (Some(rel), Some(size)) = (u32_at(buf, off), u32_at(buf, off + 4)) else { break };
        let size = size as usize;
        if size < 8 || off + size > buf.len() {
            break;
        }
        let rec = &buf[off..off + size];
        match rel {
            // RelationProcessorCore: Flags(1) Efficiency(1) Reserved(20) GroupCount(2) GroupMask[]
            0 => {
                if let (Some(&eff), Some(gc)) = (rec.get(9), u16_at(rec, 8 + 22)) {
                    if let Some(masks) = group_affinities(rec, 8 + 24, gc) {
                        cores.push(CoreRec { efficiency: eff, masks });
                    }
                }
            }
            // RelationCache: Level(1) Assoc(1) LineSize(2) CacheSize(4) Type(4) Reserved(18) GroupCount(2) GroupMasks[]
            2 => {
                if let (Some(&level), Some(sz), Some(ty), Some(gc)) = (rec.get(8), u32_at(rec, 8 + 4), u32_at(rec, 8 + 8), u16_at(rec, 8 + 30)) {
                    if let Some(masks) = group_affinities(rec, 8 + 32, gc.max(1)) {
                        // Type: 0 unified, 1 instruction, 2 data, 3 trace
                        caches.push(CacheRec { level, size: sz as u64, data: ty == 0 || ty == 2, masks });
                    }
                }
            }
            _ => {}
        }
        off += size;
    }
    (cores, caches)
}

/// Parse `GetSystemCpuSetInformation` records (type 0 = CpuSetInformation).
pub fn parse_cpu_sets(buf: &[u8]) -> Vec<CpuSetRec> {
    let mut out = vec![];
    let mut off = 0usize;
    while off + 8 <= buf.len() {
        let (Some(size), Some(ty)) = (u32_at(buf, off), u32_at(buf, off + 4)) else { break };
        let size = size as usize;
        if size < 8 || off + size > buf.len() {
            break;
        }
        if ty == 0 && size >= 24 {
            if let (Some(id), Some(group), Some(&lp)) = (u32_at(buf, off + 8), u16_at(buf, off + 12), buf.get(off + 14)) {
                out.push(CpuSetRec { id, group, logical_processor: lp });
            }
        }
        off += size;
    }
    out
}

#[cfg(windows)]
pub fn detect() -> Result<Topology, String> {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_INSUFFICIENT_BUFFER};
    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformationEx, GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    const RELATION_ALL: i32 = 0xffff;
    let mut len = 0u32;
    unsafe { GetLogicalProcessorInformationEx(RELATION_ALL, std::ptr::null_mut(), &mut len) };
    if len == 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
        return Err(format!("GetLogicalProcessorInformationEx failed (Win32 error {})", unsafe { GetLastError() }));
    }
    let mut buf = vec![0u8; len as usize];
    if unsafe { GetLogicalProcessorInformationEx(RELATION_ALL, buf.as_mut_ptr() as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, &mut len) } == 0 {
        return Err(format!("GetLogicalProcessorInformationEx failed (Win32 error {})", unsafe { GetLastError() }));
    }
    buf.truncate(len as usize);
    let (cores, caches) = parse_glpi_ex(&buf);
    if cores.is_empty() {
        return Err("Windows reported no processor cores".into());
    }

    // CPU Sets (Windows 10+). Optional: without them only affinity rules work.
    let mut sets = vec![];
    let mut slen = 0u32;
    let process = unsafe { GetCurrentProcess() };
    unsafe { GetSystemCpuSetInformation(std::ptr::null_mut(), 0, &mut slen, process, 0) };
    if slen > 0 {
        let mut sbuf = vec![0u8; slen as usize];
        if unsafe { GetSystemCpuSetInformation(sbuf.as_mut_ptr() as *mut SYSTEM_CPU_SET_INFORMATION, slen, &mut slen, process, 0) } != 0 {
            sbuf.truncate(slen as usize);
            sets = parse_cpu_sets(&sbuf);
        }
    }
    Ok(from_records(&cores, &caches, &sets))
}

#[cfg(not(windows))]
pub fn detect() -> Result<Topology, String> {
    Err("topology detection is only implemented for Windows".into())
}

// ---------------------------------------------------------------- reporting

#[derive(Serialize)]
pub struct GroupReport {
    pub index: usize,
    pub size_bytes: u64,
    pub size_mb: f64,
    /// "vcache", "frequency" or "".
    pub label: &'static str,
    pub cpus: Vec<usize>,
    pub cpu_list: String,
    /// One entry per processor group the L3 group touches: `0xFFFF` style, per group.
    pub masks: Vec<MaskReport>,
}

#[derive(Serialize)]
pub struct MaskReport {
    pub processor_group: u16,
    pub mask: String,
}

#[derive(Serialize)]
pub struct Report<'a> {
    pub logical_processors: usize,
    pub cores: usize,
    pub smt: bool,
    pub processor_groups: u16,
    pub vcache_group: Option<usize>,
    pub frequency_groups: Vec<usize>,
    pub l3_groups: Vec<GroupReport>,
    pub cpus: &'a [Cpu],
}

pub fn report(t: &Topology) -> Report<'_> {
    Report {
        logical_processors: t.logical_count(),
        cores: t.core_count(),
        smt: t.has_smt(),
        processor_groups: t.groups,
        vcache_group: t.vcache_group(),
        frequency_groups: t.frequency_groups(),
        l3_groups: t
            .l3
            .iter()
            .map(|g| GroupReport {
                index: g.index,
                size_bytes: g.size_bytes,
                size_mb: Topology::size_mb(g),
                label: t.label(g.index),
                cpus: g.cpus.clone(),
                cpu_list: Topology::cpu_list(&g.cpus),
                masks: t.group_masks(&g.cpus).into_iter().map(|(pg, m)| MaskReport { processor_group: pg, mask: format!("{m:#018X}") }).collect(),
            })
            .collect(),
        cpus: &t.cpus,
    }
}

pub fn render_text(t: &Topology) -> String {
    let r = report(t);
    let mut s = String::new();
    s.push_str(&format!(
        "Logical processors: {}   Physical cores: {}   SMT: {}   Processor groups: {}\n",
        r.logical_processors,
        r.cores,
        if r.smt { "yes" } else { "no" },
        r.processor_groups
    ));
    if t.l3.is_empty() {
        s.push_str("L3 cache groups: none reported\n");
    } else {
        s.push_str(&format!("L3 cache groups (\"CCDs\"): {}", t.l3.len()));
        match r.vcache_group {
            Some(v) => s.push_str(&format!("   cache-heavy (V-Cache): group {v} ({:.0} MB)", Topology::size_mb(&t.l3[v]))),
            None => s.push_str("   cache-heavy (V-Cache): none (all groups the same size)"),
        }
        if !r.frequency_groups.is_empty() {
            s.push_str(&format!("   frequency: group {}", r.frequency_groups.iter().map(|g| g.to_string()).collect::<Vec<_>>().join(",")));
        }
        s.push('\n');
    }
    s.push_str("\n  CPU  grp.bit  core  smt  L3grp   L3 size  eff  cpuset-id\n");
    for c in &t.cpus {
        let (l3, size) = match c.l3 {
            Some(i) => (i.to_string(), format!("{:.0} MB", Topology::size_mb(&t.l3[i]))),
            None => ("-".into(), "-".into()),
        };
        s.push_str(&format!(
            "  {:>3}  {:>3}.{:<3}  {:>4}  {:>3}  {:>5}  {:>8}  {:>3}  {}\n",
            c.index,
            c.group,
            c.bit,
            c.core,
            c.smt_rank,
            l3,
            size,
            c.efficiency_class,
            c.cpu_set_id.map_or("-".to_string(), |i| i.to_string())
        ));
    }
    s.push('\n');
    for g in &r.l3_groups {
        let label = if g.label.is_empty() { String::new() } else { format!(", {}", g.label) };
        s.push_str(&format!("L3 group {} ({:.0} MB{label}): CPUs {}\n", g.index, g.size_mb, g.cpu_list));
        for m in &g.masks {
            s.push_str(&format!("    affinity mask in processor group {}: {}\n", m.processor_group, m.mask));
        }
    }
    if t.groups > 1 {
        s.push_str("\nNote: this machine has more than one processor group. Affinity masks can only address one group at a time; use CpuSets for rules that span groups.\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulated_9950x3d_has_the_expected_shape() {
        let t = fake_9950x3d(true);
        assert_eq!((t.logical_count(), t.core_count(), t.groups), (32, 16, 1));
        assert!(t.has_smt());
        assert_eq!(t.l3.len(), 2);
        assert_eq!(t.l3[0].cpus.len(), 16);
        assert_eq!(t.l3[0].size_bytes, 96 << 20);
        assert_eq!(t.l3[1].size_bytes, 32 << 20);
    }

    #[test]
    fn vcache_is_the_largest_l3_wherever_it_sits() {
        let a = fake_9950x3d(true);
        assert_eq!((a.vcache_group(), a.frequency_groups()), (Some(0), vec![1]));
        let b = fake_9950x3d(false);
        assert_eq!((b.vcache_group(), b.frequency_groups()), (Some(1), vec![0]), "CCD0 is not assumed to be the V-Cache one");
        assert_eq!(b.label(1), "vcache");
        assert_eq!(b.label(0), "frequency");
        assert_eq!(b.resolve_group(GroupRef::VCache).unwrap(), (16..32).collect::<Vec<_>>());
        assert_eq!(b.resolve_group(GroupRef::Frequency).unwrap(), (0..16).collect::<Vec<_>>());
    }

    #[test]
    fn machines_without_a_cache_heavy_group_say_so() {
        let q = fake_quad();
        assert_eq!(q.vcache_group(), None);
        assert!(q.resolve_group(GroupRef::VCache).unwrap_err().contains("no cache-heavy"));
        assert!(q.resolve_group(GroupRef::Frequency).unwrap_err().contains("no distinct frequency"));
        assert_eq!(q.resolve_group(GroupRef::Index(0)).unwrap(), vec![0, 1, 2, 3]);
        assert!(q.resolve_group(GroupRef::Index(1)).unwrap_err().contains("0-0"));
        let equal = synthetic(&[Ccd { cores: 8, l3_mb: 32 }, Ccd { cores: 8, l3_mb: 32 }], true);
        assert_eq!(equal.vcache_group(), None, "two equal CCDs (a plain 9950X) have no V-Cache group");
    }

    #[test]
    fn quad_core_shape() {
        let q = fake_quad();
        assert_eq!((q.logical_count(), q.core_count(), q.has_smt(), q.l3.len()), (4, 4, false, 1));
        assert_eq!(q.l3[0].size_bytes, 8 << 20);
    }

    #[test]
    fn masks_and_cpu_set_ids() {
        let t = fake_9950x3d(true);
        assert_eq!(t.affinity_for(&(0..16).collect::<Vec<_>>()).unwrap(), (0, 0xFFFF));
        assert_eq!(t.affinity_for(&(16..32).collect::<Vec<_>>()).unwrap(), (0, 0xFFFF_0000));
        assert_eq!(t.cpu_set_ids_for(&[3, 1, 1]).unwrap(), vec![0x101, 0x103]);
        assert!(t.affinity_for(&[32]).unwrap_err().contains("processor 32 does not exist"));
        assert!(t.affinity_for(&[]).is_err());
        assert_eq!(t.cpus_for_ids(&[0x101, 0x103]), vec![1, 3]);
    }

    #[test]
    fn processors_beyond_64_use_groups_and_affinity_rejects_spanning() {
        let t = fake_two_groups();
        assert_eq!((t.logical_count(), t.groups), (128, 2));
        assert_eq!((t.cpus[63].group, t.cpus[63].bit, t.cpus[64].group, t.cpus[64].bit), (0, 63, 1, 0));
        // Entirely inside group 1: expressible.
        assert_eq!(t.affinity_for(&[64, 65]).unwrap(), (1, 0b11));
        // Spanning both groups: refused with a clear message, never truncated.
        let e = t.affinity_for(&[0, 100]).unwrap_err();
        assert!(e.contains("span processor groups 0 and 1") && e.contains("CpuSets"), "{e}");
        // CPU Sets can express it.
        assert_eq!(t.cpu_set_ids_for(&[0, 100]).unwrap().len(), 2);
    }

    #[test]
    fn missing_cpu_set_ids_are_reported() {
        let mut t = fake_quad();
        t.cpus[0].cpu_set_id = None;
        assert!(t.cpu_set_ids_for(&[0]).unwrap_err().contains("CPU Sets are not available"));
        assert!(t.cpu_set_ids_for(&[1]).is_ok());
    }

    #[test]
    fn cpu_list_formatting() {
        assert_eq!(Topology::cpu_list(&[0, 1, 2, 3, 8, 10, 11]), "0-3,8,10-11");
        assert_eq!(Topology::cpu_list(&[5]), "5");
        assert_eq!(Topology::cpu_list(&[]), "");
        assert_eq!(Topology::cpu_list(&[3, 1, 2, 2]), "1-3");
    }

    // ---- OS record parsing ---------------------------------------------------------------

    fn ga(mask: u64, group: u16) -> Vec<u8> {
        let mut v = mask.to_le_bytes().to_vec();
        v.extend(group.to_le_bytes());
        v.extend([0u8; 6]);
        v
    }
    fn core_rec(eff: u8, mask: u64, group: u16) -> Vec<u8> {
        let mut body = vec![1u8, eff]; // flags (SMT), efficiency
        body.extend([0u8; 20]);
        body.extend(1u16.to_le_bytes());
        body.extend(ga(mask, group));
        let mut r = 0u32.to_le_bytes().to_vec();
        r.extend((8 + body.len() as u32).to_le_bytes());
        r.extend(body);
        r
    }
    fn cache_rec(level: u8, size: u32, ty: u32, mask: u64, group: u16) -> Vec<u8> {
        let mut body = vec![level, 16];
        body.extend(64u16.to_le_bytes());
        body.extend(size.to_le_bytes());
        body.extend(ty.to_le_bytes());
        body.extend([0u8; 18]);
        body.extend(1u16.to_le_bytes());
        body.extend(ga(mask, group));
        let mut r = 2u32.to_le_bytes().to_vec();
        r.extend((8 + body.len() as u32).to_le_bytes());
        r.extend(body);
        r
    }
    fn set_rec(id: u32, group: u16, lp: u8) -> Vec<u8> {
        let mut r = 32u32.to_le_bytes().to_vec();
        r.extend(0u32.to_le_bytes());
        r.extend(id.to_le_bytes());
        r.extend(group.to_le_bytes());
        r.push(lp);
        r.extend([0u8; 32 - 15]);
        r
    }

    #[test]
    fn parses_windows_records_into_a_topology() {
        // Two cores with SMT (cpus 0-1, 2-3), one 96 MB L3 over cpus 0-1, one 32 MB L3 over cpus 2-3,
        // plus L1/L2 and an instruction cache that must be ignored.
        let mut buf = vec![];
        buf.extend(core_rec(0, 0b0011, 0));
        buf.extend(core_rec(0, 0b1100, 0));
        buf.extend(cache_rec(1, 32768, 1, 0b0011, 0));
        buf.extend(cache_rec(2, 1 << 20, 0, 0b0011, 0));
        buf.extend(cache_rec(3, 96 << 20, 0, 0b0011, 0));
        buf.extend(cache_rec(3, 32 << 20, 0, 0b1100, 0));
        let (cores, caches) = parse_glpi_ex(&buf);
        assert_eq!((cores.len(), caches.len()), (2, 4));
        let mut sets = vec![];
        for i in 0..4u8 {
            sets.extend(set_rec(0x100 + i as u32, 0, i));
        }
        let t = from_records(&cores, &caches, &parse_cpu_sets(&sets));
        assert_eq!((t.logical_count(), t.core_count(), t.l3.len()), (4, 2, 2));
        assert_eq!(t.vcache_group(), Some(0));
        assert_eq!((t.cpus[1].core, t.cpus[1].smt_rank, t.cpus[3].l3), (0, 1, Some(1)));
        assert_eq!(t.cpus[2].cpu_set_id, Some(0x102));
        assert_eq!(t.l3[0].size_bytes, 96 << 20);
    }

    #[test]
    fn truncated_or_garbage_buffers_do_not_panic_or_invent_data() {
        let mut full = core_rec(0, 0b1, 0);
        full.extend(cache_rec(3, 1 << 20, 0, 0b1, 0));
        for cut in 0..full.len() {
            let _ = parse_glpi_ex(&full[..cut]);
            let _ = parse_cpu_sets(&full[..cut]);
        }
        assert_eq!(parse_glpi_ex(&[0xff; 64]), (vec![], vec![]));
        assert!(parse_cpu_sets(&[]).is_empty());
        let (c, _) = parse_glpi_ex(&full[..full.len() - 1]);
        assert_eq!(c.len(), 1, "complete records before the truncation are still used");
    }

    #[test]
    fn two_processor_groups_from_records() {
        let mut buf = vec![];
        buf.extend(core_rec(0, 0b1, 0));
        buf.extend(core_rec(0, 0b1, 1));
        let (cores, caches) = parse_glpi_ex(&buf);
        let t = from_records(&cores, &caches, &[]);
        assert_eq!((t.groups, t.cpus[1].group, t.cpus[1].bit), (2, 1, 0));
        assert!(t.l3.is_empty() && t.vcache_group().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn real_detection_is_self_consistent_on_this_machine() {
        let t = detect().expect("detect");
        assert!(t.logical_count() >= 1);
        assert!(t.cpus.iter().enumerate().all(|(i, c)| c.index == i));
        assert!(t.core_count() <= t.logical_count());
        for g in &t.l3 {
            assert!(g.cpus.iter().all(|i| t.cpus[*i].l3 == Some(g.index)));
        }
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        assert_eq!(t.logical_count(), n, "matches what Rust sees");
    }

    #[test]
    fn text_and_json_reports() {
        let t = fake_9950x3d(false);
        let text = render_text(&t);
        assert!(text.contains("cache-heavy (V-Cache): group 1 (96 MB)"), "{text}");
        assert!(text.contains("frequency: group 0"));
        assert!(text.contains("affinity mask in processor group 0: 0x00000000FFFF0000"), "{text}");
        assert!(text.lines().filter(|l| l.trim_start().starts_with(|c: char| c.is_ascii_digit())).count() >= 32);
        let json = serde_json::to_value(report(&t)).unwrap();
        assert_eq!(json["vcache_group"], 1);
        assert_eq!(json["l3_groups"][1]["label"], "vcache");
        assert_eq!(json["l3_groups"][1]["masks"][0]["mask"], "0x00000000FFFF0000");
        assert_eq!(json["cpus"].as_array().unwrap().len(), 32);
        assert!(render_text(&fake_two_groups()).contains("more than one processor group"));
    }
}
