//! Per-application profiles: the `[Profile.<name>]` INI sections.
//!
//! Keys (all optional except `Match`; unknown keys are tolerated):
//!   Match                       exe name or pattern; wildcards `*` `?`; a pattern containing `\` or `/`
//!                               is matched against the full image path
//!   Enabled                     true/false (default true)
//!   Priority                    idle | belownormal | normal | abovenormal | high   (never realtime)
//!   Affinity                    `0x`hex mask, or a logical-processor list like `0-15,24`
//!   CpuSets                     same syntax; mutually exclusive with Affinity
//!   CpuGroup                    vcache | frequency | <L3 group index>, resolved at apply time
//!   CpuMode                     affinity (default) | cpusets: how CpuGroup / SmtPolicy are applied
//!   SmtPolicy                   all (default) | no-smt  (only the first thread of each core)
//!   GameMode                    true/false
//!   ExcludeFromRestraint        true/false
//!
//! Masks are bit-per-logical-processor: bit N = logical processor N (processor group 0 numbering).
//! Lists use the same global logical processor numbers (`process-corral topology` prints them).

use crate::config::{class_name, parse_class, REALTIME};
use crate::topology::{GroupRef, Topology};

#[derive(Clone, Debug, PartialEq)]
pub enum Select {
    /// Bit N = logical processor N.
    Mask(u64),
    List(Vec<usize>),
    Group(GroupRef),
    /// Every logical processor (only produced internally, e.g. for `SmtPolicy=no-smt` alone).
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mech {
    Affinity,
    CpuSets,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Smt {
    All,
    NoSmt,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CpuRule {
    pub select: Select,
    pub mech: Mech,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub matcher: String,
    pub enabled: bool,
    pub priority: Option<u32>,
    pub cpu: Option<CpuRule>,
    pub smt: Smt,
    pub game_mode: bool,
    pub exclude_restraint: bool,
}

/// The profile key that keeps Restraint away from a program.
pub const EXCLUDE_KEY: &str = "ExcludeFromRestraint";

/// Section name prefix, matched case-insensitively.
pub const PREFIX: &str = "profile.";

pub fn section_name(profile: &str) -> String {
    format!("Profile.{profile}")
}

impl Profile {
    /// A path pattern is matched against the full image path, a plain one against the file name.
    pub fn is_path_pattern(&self) -> bool {
        self.matcher.contains(['\\', '/'])
    }

    pub fn matches(&self, exe_name: &str, full_path: Option<&str>) -> bool {
        if !self.enabled {
            return false;
        }
        if self.is_path_pattern() {
            full_path.is_some_and(|p| crate::config::glob(&self.matcher.replace('/', "\\"), &p.replace('/', "\\")))
        } else {
            crate::config::glob(&self.matcher, exe_name)
        }
    }

    /// The CPU rule that applies: the explicit one, or "first thread of every core" for a profile
    /// that only sets `SmtPolicy=no-smt`.
    pub fn effective_cpu(&self) -> Option<CpuRule> {
        match (&self.cpu, self.smt) {
            (Some(r), _) => Some(r.clone()),
            (None, Smt::NoSmt) => Some(CpuRule { select: Select::All, mech: Mech::Affinity }),
            (None, Smt::All) => None,
        }
    }

    /// Logical processors the profile selects on this machine, or why it can't be expressed.
    pub fn resolve_cpus(&self, topo: &Topology) -> Result<Option<(Mech, Vec<usize>)>, String> {
        let Some(rule) = self.effective_cpu() else { return Ok(None) };
        let n = topo.logical_count();
        let mut cpus: Vec<usize> = match &rule.select {
            Select::Mask(m) => {
                let v: Vec<usize> = (0..64).filter(|b| m >> b & 1 == 1).collect();
                if let Some(bad) = v.iter().find(|b| **b >= n) {
                    return Err(format!(
                        "mask {m:#X} selects processor {bad}, but this machine has only {n} logical processor(s) (0-{})",
                        n.saturating_sub(1)
                    ));
                }
                v
            }
            Select::List(v) => {
                topo.check_indexes(v)?;
                v.clone()
            }
            Select::Group(g) => topo.resolve_group(*g)?,
            Select::All => (0..n).collect(),
        };
        if self.smt == Smt::NoSmt {
            cpus.retain(|i| topo.cpus[*i].smt_rank == 0);
            if cpus.is_empty() {
                return Err("SmtPolicy=no-smt leaves no processors selected".into());
            }
        }
        cpus.sort_unstable();
        cpus.dedup();
        if cpus.is_empty() {
            return Err("the profile selects no processors".into());
        }
        Ok(Some((rule.mech, cpus)))
    }

    /// Everything that can be checked without running anything: used by the GUI and `topology`
    /// style tools to reject rules this machine cannot express.
    pub fn validate_against(&self, topo: &Topology) -> Result<(), String> {
        match self.resolve_cpus(topo)? {
            None => Ok(()),
            Some((Mech::Affinity, cpus)) => topo.affinity_for(&cpus).map(|_| ()),
            Some((Mech::CpuSets, cpus)) => topo.cpu_set_ids_for(&cpus).map(|_| ()),
        }
    }

    pub fn describe(&self) -> String {
        let mut p = vec![format!("match {}", self.matcher)];
        if !self.enabled {
            p.push("disabled".into());
        }
        if let Some(c) = self.priority {
            p.push(format!("priority {}", class_name(c)));
        }
        if let Some(r) = &self.cpu {
            let how = if r.mech == Mech::Affinity { "affinity" } else { "cpusets" };
            p.push(format!("{how} {}", describe_select(&r.select)));
        }
        if self.smt == Smt::NoSmt {
            p.push("no-smt".into());
        }
        if self.game_mode {
            p.push("game mode".into());
        }
        if self.exclude_restraint {
            p.push("no restraint".into());
        }
        p.join(", ")
    }
}

pub fn describe_select(s: &Select) -> String {
    match s {
        Select::Mask(m) => format!("{m:#X}"),
        Select::List(v) => Topology::cpu_list(v),
        Select::Group(g) => format!("group {g}"),
        Select::All => "all".into(),
    }
}

// ---------------------------------------------------------------- parsing

/// Parse the value of `Affinity` / `CpuSets`.
pub fn parse_select(v: &str) -> Result<Select, String> {
    let v = v.trim();
    if let Some(hex) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("'{v}' is not a hexadecimal mask"));
        }
        let trimmed = hex.trim_start_matches('0');
        if trimmed.len() > 16 {
            return Err(format!("mask '{v}' is wider than 64 bits; use a processor list (e.g. 0-79) instead"));
        }
        let m = if trimmed.is_empty() { 0 } else { u64::from_str_radix(trimmed, 16).map_err(|_| format!("'{v}' is not a hexadecimal mask"))? };
        if m == 0 {
            return Err("the mask selects no processors".into());
        }
        return Ok(Select::Mask(m));
    }
    if v.is_empty() {
        return Err("is empty".into());
    }
    if v.bytes().any(|b| b.is_ascii_alphabetic()) {
        return Err(format!("'{v}' is neither a processor list (like 0-15,24) nor a mask; hexadecimal masks need a 0x prefix (like 0xFF)"));
    }
    let mut out: Vec<usize> = vec![];
    for tok in v.split(',') {
        let tok = tok.trim();
        let num = |s: &str| s.trim().parse::<usize>().map_err(|_| format!("'{tok}' is not a processor number or range"));
        match tok.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (num(a)?, num(b)?);
                if a > b {
                    return Err(format!("range '{tok}' runs backwards"));
                }
                if b - a >= 4096 {
                    return Err(format!("range '{tok}' is too large"));
                }
                out.extend(a..=b);
            }
            None => out.push(num(tok)?),
        }
        if out.len() > 4096 {
            return Err("too many processors listed".into());
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(Select::List(out))
}

pub fn parse_group(v: &str) -> Result<GroupRef, String> {
    match v.trim().to_ascii_lowercase().as_str() {
        "vcache" => Ok(GroupRef::VCache),
        "frequency" => Ok(GroupRef::Frequency),
        s => s.parse::<usize>().map(GroupRef::Index).map_err(|_| format!("'{v}' must be vcache, frequency or an L3 group number")),
    }
}

fn parse_bool(v: &str) -> Result<bool, String> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Ok(true),
        "false" | "0" | "no" | "off" => Ok(false),
        _ => Err(format!("'{v}' is not true/false")),
    }
}

/// One section's raw lines: (lower-cased key, trimmed value, 1-based line number).
pub struct RawSection {
    pub name: String,
    pub line: usize,
    pub entries: Vec<(String, String, usize)>,
}

pub fn raw_sections(text: &str) -> Vec<RawSection> {
    let mut out: Vec<RawSection> = vec![];
    let mut cur: Option<RawSection> = None;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(h) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            if let Some(s) = cur.take() {
                out.push(s);
            }
            if h.len() >= PREFIX.len() && h[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
                cur = Some(RawSection { name: h[PREFIX.len()..].trim().to_string(), line: i + 1, entries: vec![] });
            }
        } else if let (Some(s), Some((k, v))) = (cur.as_mut(), line.split_once('=')) {
            s.entries.push((k.trim().to_ascii_lowercase(), v.trim().to_string(), i + 1));
        }
    }
    if let Some(s) = cur {
        out.push(s);
    }
    out
}

/// Parse one profile section. The error carries the line the problem is on.
pub fn parse_section(sec: &RawSection) -> Result<Profile, (usize, String)> {
    if sec.name.is_empty() {
        return Err((sec.line, "a profile section needs a name, like [Profile.MyGame]".into()));
    }
    let err = |line: usize, key: &str, e: String| (line, format!("{key}: {e}"));
    // The last occurrence of a key wins, as everywhere else in the config.
    let get = |k: &str| sec.entries.iter().rev().find(|e| e.0 == k);
    let nonempty = |k: &str| get(k).filter(|e| !e.1.is_empty());

    let matcher = match get("match") {
        Some(e) if !e.1.is_empty() => e.1.clone(),
        Some(e) => return Err(err(e.2, "Match", "must not be empty".into())),
        None => return Err((sec.line, format!("[Profile.{}] has no Match= line", sec.name))),
    };
    let enabled = match get("enabled") {
        Some(e) => parse_bool(&e.1).map_err(|m| err(e.2, "Enabled", m))?,
        None => true,
    };
    let priority = match nonempty("priority") {
        Some(e) => {
            let c = parse_class(&e.1).ok_or_else(|| err(e.2, "Priority", format!("unknown priority '{}' (use idle, belownormal, normal, abovenormal or high)", e.1)))?;
            if c == REALTIME {
                return Err(err(e.2, "Priority", "realtime priority is not allowed in profiles".into()));
            }
            Some(c)
        }
        None => None,
    };
    let affinity = nonempty("affinity");
    let cpusets = nonempty("cpusets");
    let group = nonempty("cpugroup");
    let mode = match nonempty("cpumode") {
        Some(e) => Some((
            match e.1.to_ascii_lowercase().as_str() {
                "affinity" => Mech::Affinity,
                "cpusets" | "cpuset" | "cpu-sets" => Mech::CpuSets,
                _ => return Err(err(e.2, "CpuMode", format!("'{}' must be affinity or cpusets", e.1))),
            },
            e.2,
        )),
        None => None,
    };
    if let (Some(a), Some(c)) = (affinity, cpusets) {
        return Err((a.2.max(c.2), "Affinity and CpuSets are mutually exclusive; set only one".into()));
    }
    if let Some(g) = group {
        if let Some(x) = affinity.or(cpusets) {
            return Err((x.2.max(g.2), "CpuGroup cannot be combined with Affinity or CpuSets; use one of them".into()));
        }
    }

    let cpu = if let Some(e) = affinity {
        if let Some((Mech::CpuSets, l)) = mode {
            return Err((l, "CpuMode=cpusets contradicts the Affinity key".into()));
        }
        Some(CpuRule { select: parse_select(&e.1).map_err(|m| err(e.2, "Affinity", m))?, mech: Mech::Affinity })
    } else if let Some(e) = cpusets {
        if let Some((Mech::Affinity, l)) = mode {
            return Err((l, "CpuMode=affinity contradicts the CpuSets key".into()));
        }
        Some(CpuRule { select: parse_select(&e.1).map_err(|m| err(e.2, "CpuSets", m))?, mech: Mech::CpuSets })
    } else if let Some(e) = group {
        Some(CpuRule { select: Select::Group(parse_group(&e.1).map_err(|m| err(e.2, "CpuGroup", m))?), mech: mode.map_or(Mech::Affinity, |m| m.0) })
    } else {
        None
    };
    let smt = match nonempty("smtpolicy") {
        Some(e) => match e.1.to_ascii_lowercase().as_str() {
            "all" => Smt::All,
            "no-smt" | "nosmt" => Smt::NoSmt,
            _ => return Err(err(e.2, "SmtPolicy", format!("'{}' must be all or no-smt", e.1))),
        },
        None => Smt::All,
    };
    let game_mode = match get("gamemode") {
        Some(e) => parse_bool(&e.1).map_err(|m| err(e.2, "GameMode", m))?,
        None => false,
    };
    let exclude_restraint = match get(&EXCLUDE_KEY.to_ascii_lowercase()) {
        Some(e) => parse_bool(&e.1).map_err(|m| err(e.2, EXCLUDE_KEY, m))?,
        None => false,
    };
    Ok(Profile { name: sec.name.clone(), matcher, enabled, priority, cpu, smt, game_mode, exclude_restraint })
}

/// Strict: every profile must be valid; duplicate names are errors. Messages start with `line N:`.
pub fn parse_strict(text: &str) -> Result<Vec<Profile>, String> {
    let mut out: Vec<Profile> = vec![];
    for sec in raw_sections(text) {
        let p = parse_section(&sec).map_err(|(l, m)| format!("line {l}: [Profile.{}] {m}", sec.name))?;
        if out.iter().any(|o| o.name.eq_ignore_ascii_case(&p.name)) {
            return Err(format!("line {}: duplicate profile name '{}'", sec.line, p.name));
        }
        out.push(p);
    }
    Ok(out)
}

/// Lenient (the running engine after validation): invalid or duplicate profiles are skipped.
pub fn parse_lenient(text: &str) -> Vec<Profile> {
    let mut out: Vec<Profile> = vec![];
    for sec in raw_sections(text) {
        if let Ok(p) = parse_section(&sec) {
            if !out.iter().any(|o| o.name.eq_ignore_ascii_case(&p.name)) {
                out.push(p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::{fake_9950x3d, fake_quad, fake_two_groups};

    fn one(body: &str) -> Result<Profile, String> {
        let mut v = parse_strict(&format!("[Profile.Game]\n{body}\n"))?;
        Ok(v.remove(0))
    }

    #[test]
    fn masks_and_lists_parse() {
        assert_eq!(parse_select("0xFF").unwrap(), Select::Mask(0xFF));
        assert_eq!(parse_select("0XffFF_".trim_end_matches('_')).unwrap(), Select::Mask(0xFFFF));
        assert_eq!(parse_select("0x0000000000000003").unwrap(), Select::Mask(3));
        assert_eq!(parse_select("0-3,8, 10-11").unwrap(), Select::List(vec![0, 1, 2, 3, 8, 10, 11]));
        assert_eq!(parse_select("5").unwrap(), Select::List(vec![5]));
        assert_eq!(parse_select("3,1,1").unwrap(), Select::List(vec![1, 3]));
    }

    #[test]
    fn malformed_selections_are_rejected_with_a_reason() {
        for (bad, needle) in [
            ("", "empty"),
            ("0x", "hexadecimal"),
            ("0x0", "no processors"),
            ("0xZZ", "hexadecimal"),
            ("FF", "0x prefix"),
            ("0x1FFFFFFFFFFFFFFFF", "wider than 64 bits"),
            ("5-2", "backwards"),
            ("1,,2", "not a processor"),
            ("a-b", "0x prefix"),
            ("0-99999", "too large"),
            ("-3", "not a processor"),
        ] {
            let e = parse_select(bad).unwrap_err();
            assert!(e.contains(needle), "{bad:?}: {e}");
        }
    }

    #[test]
    fn a_full_profile_round_trips_every_key() {
        let p = one("Match=Game.exe\nEnabled=true\nPriority=high\nCpuGroup=vcache\nCpuMode=cpusets\nSmtPolicy=no-smt\nGameMode=true\nExcludeFromRestraint=true").unwrap();
        assert_eq!(p.name, "Game");
        assert_eq!(p.matcher, "Game.exe");
        assert_eq!(p.priority, Some(crate::config::HIGH));
        assert_eq!(p.cpu, Some(CpuRule { select: Select::Group(GroupRef::VCache), mech: Mech::CpuSets }));
        assert_eq!((p.smt, p.game_mode, p.exclude_restraint, p.enabled), (Smt::NoSmt, true, true, true));
        let d = one("Match=x.exe").unwrap();
        assert_eq!((d.priority, d.cpu.clone(), d.smt, d.game_mode, d.exclude_restraint, d.enabled), (None, None, Smt::All, false, false, true));
    }

    #[test]
    fn the_exclude_key_true_false_missing_and_bad_values() {
        let ex = |body: &str| one(&format!("Match=g.exe\n{body}")).map(|p| p.exclude_restraint);
        assert_eq!(ex("ExcludeFromRestraint=true"), Ok(true));
        assert_eq!(ex("ExcludeFromRestraint=false"), Ok(false));
        assert_eq!(ex("excludefromrestraint = yes"), Ok(true), "case-insensitive, same yes/no words");
        assert_eq!(ex("Enabled=true"), Ok(false));
        assert!(ex("ExcludeFromRestraint=maybe").unwrap_err().contains("ExcludeFromRestraint: 'maybe' is not true/false"));
    }

    #[test]
    fn explicit_affinity_and_cpusets_pick_their_mechanism() {
        assert_eq!(one("Match=a\nAffinity=0-15").unwrap().cpu.unwrap().mech, Mech::Affinity);
        assert_eq!(one("Match=a\nCpuSets=0xFF").unwrap().cpu.unwrap().mech, Mech::CpuSets);
        assert_eq!(one("Match=a\nCpuGroup=1").unwrap().cpu.unwrap().select, Select::Group(GroupRef::Index(1)));
        assert_eq!(one("Match=a\nCpuGroup=frequency").unwrap().cpu.unwrap().mech, Mech::Affinity);
    }

    #[test]
    fn conflicts_and_bad_values_report_the_offending_line() {
        let src = "[Profile.G]\nMatch=a.exe\nAffinity=0-3\nCpuSets=0-3\n";
        let e = parse_strict(src).unwrap_err();
        assert!(e.starts_with("line 4:") && e.contains("mutually exclusive"), "{e}");
        let e = parse_strict("[Profile.G]\nMatch=a\nCpuGroup=vcache\nAffinity=0xF").unwrap_err();
        assert!(e.starts_with("line 4:") && e.contains("CpuGroup cannot be combined"), "{e}");
        let e = parse_strict("[Profile.G]\nMatch=a\nPriority=realtime").unwrap_err();
        assert!(e.starts_with("line 3:") && e.contains("realtime"), "{e}");
        let e = parse_strict("[Profile.G]\nMatch=a\nPriority=turbo").unwrap_err();
        assert!(e.contains("unknown priority"), "{e}");
        let e = parse_strict("[Profile.G]\nPriority=high").unwrap_err();
        assert!(e.starts_with("line 1:") && e.contains("no Match"), "{e}");
        let e = parse_strict("[Profile.G]\nMatch=").unwrap_err();
        assert!(e.contains("Match: must not be empty"), "{e}");
        let e = parse_strict("[Profile.]\nMatch=a").unwrap_err();
        assert!(e.contains("needs a name"), "{e}");
        let e = parse_strict("[Profile.G]\nMatch=a\nSmtPolicy=sometimes").unwrap_err();
        assert!(e.contains("all or no-smt"), "{e}");
        let e = parse_strict("[Profile.G]\nMatch=a\nGameMode=maybe").unwrap_err();
        assert!(e.contains("GameMode") && e.contains("true/false"), "{e}");
        let e = parse_strict("[Profile.G]\nMatch=a\nCpuGroup=fast").unwrap_err();
        assert!(e.contains("vcache, frequency"), "{e}");
        let e = parse_strict("[Profile.G]\nMatch=a\nAffinity=0xF\nCpuMode=cpusets").unwrap_err();
        assert!(e.contains("contradicts"), "{e}");
        let e = parse_strict("[Profile.G]\nMatch=a\n[profile.g]\nMatch=b").unwrap_err();
        assert!(e.starts_with("line 3:") && e.contains("duplicate"), "{e}");
    }

    #[test]
    fn unknown_keys_and_other_sections_are_tolerated() {
        let src = "[Other]\nx=1\n[Profile.G]\n; note\nMatch=a.exe\nFutureKey=7\n[Z]\nMatch=ignored\n";
        let v = parse_strict(src).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].name, "G");
    }

    #[test]
    fn lenient_parsing_skips_bad_profiles_but_keeps_good_ones() {
        let src = "[Profile.Bad]\nMatch=a\nPriority=realtime\n[Profile.Good]\nMatch=b.exe\n[Profile.good]\nMatch=c.exe\n";
        let v = parse_lenient(src);
        assert_eq!(v.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["Good"]);
    }

    #[test]
    fn names_with_spaces_and_dots_work() {
        let v = parse_strict("[Profile.My Game v2.1]\nMatch=a").unwrap();
        assert_eq!(v[0].name, "My Game v2.1");
        assert_eq!(section_name(&v[0].name), "Profile.My Game v2.1");
    }

    #[test]
    fn matching_by_name_and_by_path() {
        let by_name = one("Match=Game*.EXE").unwrap();
        assert!(by_name.matches("game2.exe", None));
        assert!(!by_name.matches("other.exe", None));
        assert!(!by_name.is_path_pattern());
        let by_path = one(r"Match=C:\Games\*\game.exe").unwrap();
        assert!(by_path.is_path_pattern());
        assert!(by_path.matches("game.exe", Some(r"c:\games\steam\game.exe")));
        assert!(!by_path.matches("game.exe", Some(r"c:\other\game.exe")));
        assert!(!by_path.matches("game.exe", None), "no path available -> no match");
        let fwd = one("Match=C:/Games/*/game.exe").unwrap();
        assert!(fwd.matches("game.exe", Some(r"C:\Games\x\game.exe")));
        let off = one("Match=a.exe\nEnabled=false").unwrap();
        assert!(!off.matches("a.exe", None), "disabled profiles never match");
    }

    // ---- resolution against fake topologies -------------------------------------------------

    #[test]
    fn cpu_groups_resolve_per_machine_not_per_chip() {
        let p = one("Match=a\nCpuGroup=vcache").unwrap();
        let (m, cpus) = p.resolve_cpus(&fake_9950x3d(true)).unwrap().unwrap();
        assert_eq!((m, cpus), (Mech::Affinity, (0..16).collect::<Vec<_>>()));
        let (_, cpus) = p.resolve_cpus(&fake_9950x3d(false)).unwrap().unwrap();
        assert_eq!(cpus, (16..32).collect::<Vec<_>>(), "follows the 96 MB group, not CCD0");
        assert!(p.resolve_cpus(&fake_quad()).unwrap_err().contains("no cache-heavy"));
        let f = one("Match=a\nCpuGroup=frequency\nCpuMode=cpusets").unwrap();
        assert_eq!(f.resolve_cpus(&fake_9950x3d(true)).unwrap().unwrap(), (Mech::CpuSets, (16..32).collect::<Vec<_>>()));
    }

    #[test]
    fn masks_beyond_the_available_processors_are_rejected_not_truncated() {
        let p = one("Match=a\nAffinity=0x1FFFF").unwrap();
        assert!(p.resolve_cpus(&fake_9950x3d(true)).is_ok(), "17 bits fit in 32 processors");
        let e = p.resolve_cpus(&fake_quad()).unwrap_err();
        assert!(e.contains("selects processor 4") && e.contains("only 4 logical"), "{e}");
        let l = one("Match=a\nAffinity=0-40").unwrap();
        assert!(l.resolve_cpus(&fake_9950x3d(true)).unwrap_err().contains("processor 32 does not exist"));
    }

    #[test]
    fn smt_policy_keeps_only_first_threads() {
        let p = one("Match=a\nAffinity=0-7\nSmtPolicy=no-smt").unwrap();
        let (_, cpus) = p.resolve_cpus(&fake_9950x3d(true)).unwrap().unwrap();
        assert_eq!(cpus, vec![0, 2, 4, 6]);
        let all = one("Match=a\nSmtPolicy=no-smt").unwrap();
        let (m, cpus) = all.resolve_cpus(&fake_9950x3d(true)).unwrap().unwrap();
        assert_eq!((m, cpus.len()), (Mech::Affinity, 16));
        assert!(cpus.iter().all(|c| c % 2 == 0));
        assert_eq!(all.resolve_cpus(&fake_quad()).unwrap().unwrap().1, vec![0, 1, 2, 3], "no SMT: nothing removed");
        let g = one("Match=a\nCpuGroup=vcache\nSmtPolicy=no-smt").unwrap();
        assert_eq!(g.resolve_cpus(&fake_9950x3d(true)).unwrap().unwrap().1, (0..16).step_by(2).collect::<Vec<_>>());
    }

    #[test]
    fn validation_against_a_machine_covers_groups_and_cpu_sets() {
        let t2 = fake_two_groups();
        let span = one("Match=a\nAffinity=60-70").unwrap();
        assert!(span.validate_against(&t2).unwrap_err().contains("span processor groups"));
        let ok = one("Match=a\nCpuSets=60-70").unwrap();
        assert!(ok.validate_against(&t2).is_ok(), "CPU Sets can span groups");
        let inside = one("Match=a\nAffinity=64-70").unwrap();
        assert!(inside.validate_against(&t2).is_ok());
        let none = one("Match=a\nPriority=high").unwrap();
        assert!(none.validate_against(&fake_quad()).is_ok());
    }

    #[test]
    fn describe_mentions_each_setting() {
        let d = one("Match=g.exe\nPriority=high\nCpuGroup=vcache\nGameMode=true\nExcludeFromRestraint=true\nEnabled=false").unwrap().describe();
        for n in ["match g.exe", "disabled", "priority high", "affinity group vcache", "game mode", "no restraint"] {
            assert!(d.contains(n), "{d}");
        }
    }
}
