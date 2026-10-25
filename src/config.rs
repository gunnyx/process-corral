//! INI configuration. Section/key names for the implemented features mirror the
//! layout common to process governors (e.g. `[OutOfControlProcessRestraint]`).

use std::collections::HashMap;
use std::path::Path;

pub const IDLE: u32 = 0x40;
pub const BELOW_NORMAL: u32 = 0x4000;
pub const NORMAL: u32 = 0x20;
pub const ABOVE_NORMAL: u32 = 0x8000;
pub const HIGH: u32 = 0x80;
pub const REALTIME: u32 = 0x100;

pub fn class_name(c: u32) -> &'static str {
    match c {
        IDLE => "idle",
        BELOW_NORMAL => "belownormal",
        NORMAL => "normal",
        ABOVE_NORMAL => "abovenormal",
        HIGH => "high",
        REALTIME => "realtime",
        _ => "unknown",
    }
}

pub fn parse_class(s: &str) -> Option<u32> {
    Some(match s.trim().to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
        "idle" | "low" => IDLE,
        "belownormal" => BELOW_NORMAL,
        "normal" => NORMAL,
        "abovenormal" => ABOVE_NORMAL,
        "high" => HIGH,
        "realtime" => REALTIME,
        _ => return None,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct RestraintSettings {
    pub enabled: bool,
    pub exclude_children_of_foreground: bool,
    pub exclude_foreground: bool,
    pub exclude_services: bool,
    pub tame_only_normal: bool,
    pub lower_to_idle: bool,
    pub total_cpu_before_restraint: f64,
    pub per_process_before_restraint: f64,
    pub time_over_quota_ms: u64,
    pub per_process_for_restore: f64,
    pub min_restraint_ms: u64,
    pub max_restraint_ms: u64,
    pub restrain_by_affinity: bool,
    pub restraint_affinity: u64,
    pub exclusions: Vec<String>,
}

impl Default for RestraintSettings {
    fn default() -> Self {
        // Conventional defaults for this kind of tool.
        Self {
            enabled: true,
            exclude_children_of_foreground: true,
            exclude_foreground: true,
            exclude_services: true,
            tame_only_normal: true,
            lower_to_idle: false,
            total_cpu_before_restraint: 23.0,
            per_process_before_restraint: 15.0,
            time_over_quota_ms: 900,
            per_process_for_restore: 5.0,
            min_restraint_ms: 4200,
            max_restraint_ms: 0,
            restrain_by_affinity: false,
            restraint_affinity: 0,
            exclusions: vec![],
        }
    }
}

/// `[GameMode]`: what the engine does while a GameMode profile has a running process.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameModeCfg {
    /// Lower other (non-excluded) processes to Below Normal while a game runs.
    pub background_priority: bool,
    /// Power scheme GUID to switch to while a game runs.
    pub power_plan: Option<String>,
}

/// `[Logging]` size-based rotation of `corral.log`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LogRotation {
    /// Rotate once the active log reaches this many megabytes (1..=1024).
    pub max_mb: u64,
    /// Rotated files to keep, `corral.1.log` (newest) to `corral.N.log` (0..=50). 0 keeps none.
    pub keep: u32,
}

pub const LOG_MAX_MB_DEFAULT: u64 = 10;
pub const LOG_KEEP_DEFAULT: u32 = 5;
pub const LOG_KEEP_MAX: u32 = 50;

impl Default for LogRotation {
    fn default() -> Self {
        Self { max_mb: LOG_MAX_MB_DEFAULT, keep: LOG_KEEP_DEFAULT }
    }
}

impl LogRotation {
    pub fn max_bytes(&self) -> u64 {
        self.max_mb * 1024 * 1024
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Config {
    pub restraint: RestraintSettings,
    pub profiles: Vec<crate::profile::Profile>,
    pub game_mode: GameModeCfg,
    pub default_priorities: Vec<(String, u32)>,
    pub default_affinities: Vec<(String, u64)>,
    pub disallowed: Vec<String>,
    pub instance_limits: Vec<(String, usize)>,
    pub log_disable: bool,
    pub log_rotation: LogRotation,
    pub update_ms: u64,
}

type Ini = HashMap<(String, String), String>;

fn parse_ini(text: &str) -> Ini {
    let mut out = Ini::new();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = s.to_ascii_lowercase();
        } else if let Some((k, v)) = line.split_once('=') {
            out.insert((section.clone(), k.trim().to_ascii_lowercase()), v.trim().to_string());
        }
    }
    out
}

/// Lenient decode (test helper); production paths use `decode_strict`.
#[cfg(test)]
fn decode(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xff, 0xfe]) {
        let u: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&u)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn list(v: Option<&String>) -> Vec<String> {
    v.map(|s| s.split(',').map(|x| x.trim().to_ascii_lowercase()).filter(|x| !x.is_empty()).collect())
        .unwrap_or_default()
}

/// `name:value;name:value`
fn pairs(v: Option<&String>) -> Vec<(String, String)> {
    v.map(|s| {
        s.split(';')
            .filter_map(|p| p.split_once(':'))
            .map(|(a, b)| (a.trim().to_ascii_lowercase(), b.trim().to_string()))
            .filter(|(a, _)| !a.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

impl Config {
    pub fn parse(text: &str) -> Config {
        let ini = parse_ini(text);
        let g = |s: &str, k: &str| ini.get(&(s.to_ascii_lowercase(), k.to_ascii_lowercase()));
        let b = |s: &str, k: &str, d: bool| g(s, k).map(|v| v.eq_ignore_ascii_case("true") || v == "1").unwrap_or(d);
        let n = |s: &str, k: &str, d: f64| g(s, k).and_then(|v| v.parse().ok()).unwrap_or(d);
        let o = "OutOfControlProcessRestraint";
        let d = RestraintSettings::default();
        let restraint = RestraintSettings {
            enabled: b(o, "OocOn", d.enabled),
            exclude_children_of_foreground: b(o, "ExcludeChildrenOfForeground", d.exclude_children_of_foreground),
            exclude_foreground: b(o, "ExcludeForegroundProcesses2", d.exclude_foreground),
            exclude_services: b(o, "ExcludeServices", d.exclude_services),
            tame_only_normal: b(o, "TameOnlyNormal", d.tame_only_normal),
            lower_to_idle: b(o, "LowerToIdleInsteadOfBelowNormal", d.lower_to_idle),
            total_cpu_before_restraint: n(o, "TotalProcessorUsageBeforeRestraint", d.total_cpu_before_restraint),
            per_process_before_restraint: n(o, "PerProcessUsageBeforeRestraint", d.per_process_before_restraint),
            time_over_quota_ms: n(o, "TimeOverQuotaBeforeRestraint", d.time_over_quota_ms as f64) as u64,
            per_process_for_restore: n(o, "PerProcessUsageForRestore", d.per_process_for_restore),
            min_restraint_ms: n(o, "MinimumTimeOfRestraint", d.min_restraint_ms as f64) as u64,
            max_restraint_ms: n(o, "MaximumTimeOfRestraint", 0.0) as u64,
            restrain_by_affinity: b(o, "RestrainByAffinity", false),
            restraint_affinity: g(o, "RestraintAffinity").and_then(|v| parse_mask(v)).unwrap_or(0),
            exclusions: list(g(o, "OocExclusions")),
        };
        Config {
            restraint,
            profiles: crate::profile::parse_lenient(text),
            game_mode: GameModeCfg {
                background_priority: b("GameMode", "BackgroundPriority", false),
                power_plan: g("GameMode", "PowerPlan").filter(|v| !v.is_empty()).cloned(),
            },
            default_priorities: pairs(g("ProcessDefaults", "DefaultPriorities"))
                .into_iter()
                .filter_map(|(a, b)| Some((a, parse_class(&b)?)))
                .collect(),
            default_affinities: pairs(g("ProcessDefaults", "DefaultAffinities"))
                .into_iter()
                .filter_map(|(a, b)| Some((a, parse_mask(&b)?)))
                .collect(),
            disallowed: list(g("ProcessAllowances", "DisallowedProcesses")),
            instance_limits: pairs(g("ProcessAllowances", "InstanceLimitedProcesses"))
                .into_iter()
                .filter_map(|(a, b)| Some((a, b.parse().ok()?)))
                .collect(),
            log_disable: b("Logging", "LogDisable", false),
            log_rotation: LogRotation {
                // Out-of-range values are rejected by the strict parser; clamp for the lenient one.
                max_mb: (n("Logging", "MaxLogSizeMB", LOG_MAX_MB_DEFAULT as f64) as u64).clamp(1, 1024),
                keep: (n("Logging", "KeepLogFiles", LOG_KEEP_DEFAULT as f64) as u32).min(LOG_KEEP_MAX),
            },
            update_ms: n("Performance", "UpdateSpeedCore", 1000.0) as u64,
        }
    }

    /// Strict parse used for startup and live reload: any malformed line or out-of-range
    /// value is an error, so a bad edit can never silently change behaviour.
    pub fn from_bytes(bytes: &[u8]) -> Result<Config, String> {
        let text = decode_strict(bytes)?;
        validate(&text)?;
        Ok(Config::parse(&text))
    }

    pub fn load_strict(path: &Path) -> Result<Config, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("cannot read config: {e}"))?;
        Config::from_bytes(&bytes)
    }

    /// Human-readable list of settings that differ, using the INI key names.
    pub fn diff(&self, new: &Config) -> Vec<String> {
        let mut out = vec![];
        macro_rules! d {
            ($name:expr, $a:expr, $b:expr) => {
                if $a != $b {
                    out.push(format!("{}: {:?} -> {:?}", $name, $a, $b));
                }
            };
        }
        let (a, b) = (&self.restraint, &new.restraint);
        d!("OocOn", a.enabled, b.enabled);
        d!("ExcludeChildrenOfForeground", a.exclude_children_of_foreground, b.exclude_children_of_foreground);
        d!("ExcludeForegroundProcesses2", a.exclude_foreground, b.exclude_foreground);
        d!("ExcludeServices", a.exclude_services, b.exclude_services);
        d!("TameOnlyNormal", a.tame_only_normal, b.tame_only_normal);
        d!("LowerToIdleInsteadOfBelowNormal", a.lower_to_idle, b.lower_to_idle);
        d!("TotalProcessorUsageBeforeRestraint", a.total_cpu_before_restraint, b.total_cpu_before_restraint);
        d!("PerProcessUsageBeforeRestraint", a.per_process_before_restraint, b.per_process_before_restraint);
        d!("TimeOverQuotaBeforeRestraint", a.time_over_quota_ms, b.time_over_quota_ms);
        d!("PerProcessUsageForRestore", a.per_process_for_restore, b.per_process_for_restore);
        d!("MinimumTimeOfRestraint", a.min_restraint_ms, b.min_restraint_ms);
        d!("MaximumTimeOfRestraint", a.max_restraint_ms, b.max_restraint_ms);
        d!("RestrainByAffinity", a.restrain_by_affinity, b.restrain_by_affinity);
        d!("RestraintAffinity", format!("{:#x}", a.restraint_affinity), format!("{:#x}", b.restraint_affinity));
        d!("OocExclusions", a.exclusions, b.exclusions);
        let prio = |v: &[(String, u32)]| v.iter().map(|(n, c)| format!("{n}:{}", class_name(*c))).collect::<Vec<_>>();
        let mask = |v: &[(String, u64)]| v.iter().map(|(n, m)| format!("{n}:{m:#x}")).collect::<Vec<_>>();
        let cnt = |v: &[(String, usize)]| v.iter().map(|(n, c)| format!("{n}:{c}")).collect::<Vec<_>>();
        d!("DefaultPriorities", prio(&self.default_priorities), prio(&new.default_priorities));
        d!("DefaultAffinities", mask(&self.default_affinities), mask(&new.default_affinities));
        d!("DisallowedProcesses", self.disallowed, new.disallowed);
        d!("InstanceLimitedProcesses", cnt(&self.instance_limits), cnt(&new.instance_limits));
        d!("LogDisable", self.log_disable, new.log_disable);
        d!("MaxLogSizeMB", self.log_rotation.max_mb, new.log_rotation.max_mb);
        d!("KeepLogFiles", self.log_rotation.keep, new.log_rotation.keep);
        d!("UpdateSpeedCore", self.update_ms, new.update_ms);
        d!("[GameMode] BackgroundPriority", self.game_mode.background_priority, new.game_mode.background_priority);
        d!("[GameMode] PowerPlan", self.game_mode.power_plan, new.game_mode.power_plan);
        for p in &self.profiles {
            match new.profiles.iter().find(|n| n.name.eq_ignore_ascii_case(&p.name)) {
                None => out.push(format!("Profile {}: removed", p.name)),
                Some(n) if n != p => out.push(format!("Profile {}: {} -> {}", p.name, p.describe(), n.describe())),
                Some(_) => {}
            }
        }
        for n in &new.profiles {
            if !self.profiles.iter().any(|p| p.name.eq_ignore_ascii_case(&n.name)) {
                out.push(format!("Profile {}: added ({})", n.name, n.describe()));
            }
        }
        out
    }
}

/// UTF-8 or UTF-16LE with BOM; anything else (invalid UTF-8, odd UTF-16 length) is an error,
/// which is what a half-written file often looks like.
fn decode_strict(bytes: &[u8]) -> Result<String, String> {
    if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        if rest.len() % 2 != 0 {
            return Err("file ends in the middle of a UTF-16 character".into());
        }
        let u: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16(&u).map_err(|_| "invalid UTF-16 text".to_string())
    } else {
        String::from_utf8(bytes.to_vec()).map_err(|_| "file is not valid UTF-8".to_string())
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Bool,
    Num(f64, f64),
    Mask,
    /// A whole number in the given inclusive range.
    Int(u32, u32),
    ClassPairs,
    MaskPairs,
    CountPairs,
    /// A power scheme GUID, or empty for none.
    Guid,
}

/// Value types of the keys we actually read; unknown keys are tolerated for forward compatibility.
fn kind(section: &str, key: &str) -> Option<Kind> {
    use Kind::*;
    Some(match (section, key) {
        ("outofcontrolprocessrestraint", k) => match k {
            "oocon" | "excludechildrenofforeground" | "excludeforegroundprocesses2" | "excludeservices"
            | "tameonlynormal" | "lowertoidleinsteadofbelownormal" | "restrainbyaffinity" => Bool,
            "totalprocessorusagebeforerestraint" | "perprocessusagebeforerestraint" | "perprocessusageforrestore" => {
                Num(0.0, 100.0)
            }
            "timeoverquotabeforerestraint" | "minimumtimeofrestraint" | "maximumtimeofrestraint" => {
                Num(0.0, 86_400_000.0)
            }
            "restraintaffinity" => Mask,
            _ => return None,
        },
        ("processdefaults", "defaultpriorities") => ClassPairs,
        ("processdefaults", "defaultaffinities") => MaskPairs,
        ("processallowances", "instancelimitedprocesses") => CountPairs,
        ("performance", "updatespeedcore") => Num(100.0, 3_600_000.0),
        ("logging", "logdisable") => Bool,
        ("logging", "maxlogsizemb") => Int(1, 1024),
        ("logging", "keeplogfiles") => Int(0, LOG_KEEP_MAX),
        ("gamemode", "backgroundpriority") => Bool,
        ("gamemode", "powerplan") => Guid,
        _ => return None,
    })
}

fn nonzero_mask(v: &str) -> Result<(), String> {
    match parse_mask(v) {
        Some(m) if m != 0 => Ok(()),
        _ => Err(format!("'{v}' is not a non-zero hex affinity mask")),
    }
}

fn each_pair(v: &str, check: impl Fn(&str) -> Result<(), String>) -> Result<(), String> {
    for seg in v.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let (name, val) = seg.split_once(':').ok_or_else(|| format!("'{seg}' is not name:value"))?;
        if name.trim().is_empty() {
            return Err(format!("'{seg}' has no process name"));
        }
        check(val.trim())?;
    }
    Ok(())
}

fn check_value(kind: Kind, v: &str) -> Result<(), String> {
    match kind {
        Kind::Bool => match v.to_ascii_lowercase().as_str() {
            "true" | "false" | "1" | "0" => Ok(()),
            _ => Err(format!("'{v}' is not true/false")),
        },
        Kind::Num(lo, hi) => match v.parse::<f64>() {
            Ok(x) if x.is_finite() && (lo..=hi).contains(&x) => Ok(()),
            Ok(_) => Err(format!("{v} is outside the allowed range {lo}..{hi}")),
            Err(_) => Err(format!("'{v}' is not a number")),
        },
        Kind::Int(lo, hi) => match v.parse::<u32>() {
            Ok(x) if (lo..=hi).contains(&x) => Ok(()),
            Ok(_) => Err(format!("{v} is outside the allowed range {lo}..{hi}")),
            Err(_) if v.parse::<i64>().is_ok() => Err(format!("{v} is outside the allowed range {lo}..{hi}")),
            Err(_) => Err(format!("'{v}' is not a whole number")),
        },
        Kind::Mask => if v.is_empty() { Ok(()) } else { nonzero_mask(v) },
        Kind::ClassPairs => each_pair(v, |x| parse_class(x).map(|_| ()).ok_or_else(|| format!("unknown priority class '{x}'"))),
        Kind::MaskPairs => each_pair(v, nonzero_mask),
        Kind::Guid => if v.is_empty() || crate::guid::Guid::parse(v).is_some() { Ok(()) } else { Err(format!("'{v}' is not a GUID like {{381b4222-f694-41f0-9685-ff5bb260df2e}}")) },
        Kind::CountPairs => each_pair(v, |x| x.parse::<usize>().map(|_| ()).map_err(|_| format!("'{x}' is not a count"))),
    }
}

fn validate(text: &str) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("file is empty".into());
    }
    let mut section: Option<String> = None;
    for (i, raw) in text.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            let s = line
                .strip_prefix('[')
                .and_then(|l| l.strip_suffix(']'))
                .ok_or_else(|| format!("line {n}: malformed section header '{line}'"))?;
            section = Some(s.to_ascii_lowercase());
            continue;
        }
        let (k, v) = line.split_once('=').ok_or_else(|| format!("line {n}: expected key=value, got '{line}'"))?;
        let sec = section.as_deref().ok_or_else(|| format!("line {n}: '{}' appears before any [section]", k.trim()))?;
        if let Some(kind) = kind(sec, &k.trim().to_ascii_lowercase()) {
            check_value(kind, v.trim()).map_err(|e| format!("line {n}: {}: {e}", k.trim()))?;
        }
    }
    crate::profile::parse_strict(text)?;
    Ok(())
}

pub fn parse_mask(s: &str) -> Option<u64> {
    let s = s.trim();
    u64::from_str_radix(s.strip_prefix("0x").unwrap_or(s), 16).ok()
}

/// Case-insensitive glob supporting `*` and `?`.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

pub const DEFAULT_INI: &str = "\
; Process Corral configuration
[OutOfControlProcessRestraint]
OocOn=true
ExcludeChildrenOfForeground=true
ExcludeForegroundProcesses2=true
ExcludeServices=true
TameOnlyNormal=true
LowerToIdleInsteadOfBelowNormal=false
TotalProcessorUsageBeforeRestraint=23
PerProcessUsageBeforeRestraint=15
TimeOverQuotaBeforeRestraint=900
PerProcessUsageForRestore=5
MinimumTimeOfRestraint=4200
MaximumTimeOfRestraint=0
RestrainByAffinity=false
RestraintAffinity=
OocExclusions=

[ProcessDefaults]
; name:class;name:class   (idle, belownormal, normal, abovenormal, high)
DefaultPriorities=
; name:hexmask;name:hexmask
DefaultAffinities=

[ProcessAllowances]
; comma-separated names, wildcards allowed
DisallowedProcesses=
; name:max;name:max
InstanceLimitedProcesses=

[Performance]
UpdateSpeedCore=1000

[Logging]
LogDisable=false
MaxLogSizeMB=10
KeepLogFiles=5
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_works() {
        assert!(glob("chr*.exe", "Chrome.EXE"));
        assert!(glob("a?c", "abc"));
        assert!(!glob("a?c", "ac"));
        assert!(glob("*", ""));
    }

    #[test]
    fn parses_defaults_and_rules() {
        let c = Config::parse(DEFAULT_INI);
        assert_eq!(c.restraint.total_cpu_before_restraint, 23.0);
        assert_eq!(c.restraint.min_restraint_ms, 4200);
        let c = Config::parse(
            "[ProcessDefaults]\nDefaultPriorities=a.exe:high;b.exe:below normal\n\
             DefaultAffinities=a.exe:0x3\n[ProcessAllowances]\nDisallowedProcesses=Bad.exe, x*.exe\n\
             InstanceLimitedProcesses=n.exe:2\n",
        );
        assert_eq!(c.default_priorities, vec![("a.exe".into(), HIGH), ("b.exe".into(), BELOW_NORMAL)]);
        assert_eq!(c.default_affinities, vec![("a.exe".into(), 3)]);
        assert_eq!(c.disallowed, vec!["bad.exe", "x*.exe"]);
        assert_eq!(c.instance_limits, vec![("n.exe".into(), 2)]);
    }

    #[test]
    fn strict_accepts_default_config() {
        assert_eq!(Config::from_bytes(DEFAULT_INI.as_bytes()).unwrap(), Config::parse(DEFAULT_INI));
    }

    #[test]
    fn strict_rejects_bad_edits() {
        let bad = |s: &str| Config::from_bytes(s.as_bytes()).unwrap_err();
        assert!(bad("").contains("empty"));
        assert!(bad("  \n\n").contains("empty"));
        assert!(bad("[OutOfControlProcessRestraint]\nOocOn=maybe\n").contains("line 2"));
        assert!(bad("[OutOfControlProcessRestraint]\nPerProcessUsageBeforeRestraint=abc\n").contains("not a number"));
        assert!(bad("[OutOfControlProcessRestraint]\nTotalProcessorUsageBeforeRestraint=250\n").contains("range"));
        assert!(bad("[OutOfControlProcessRestraint]\nthis is not a setting\n").contains("key=value"));
        assert!(bad("[OutOfControlProcessRestraint\nOocOn=true\n").contains("section header"));
        assert!(bad("OocOn=true\n").contains("before any [section]"));
        assert!(bad("[ProcessDefaults]\nDefaultPriorities=a.exe:turbo\n").contains("turbo"));
        assert!(bad("[ProcessDefaults]\nDefaultAffinities=a.exe:0\n").contains("non-zero"));
        assert!(bad("[ProcessAllowances]\nInstanceLimitedProcesses=a.exe\n").contains("name:value"));
        assert!(bad("[Performance]\nUpdateSpeedCore=5\n").contains("range"));
        assert!(Config::from_bytes(&[b'[', b'a', b']', b'\n', 0xff, 0xfe]).unwrap_err().contains("UTF-8"));
        assert!(Config::from_bytes(&[0xff, 0xfe, b'[']).unwrap_err().contains("UTF-16"));
    }

    #[test]
    fn strict_tolerates_unknown_keys_and_utf16() {
        assert!(Config::from_bytes(b"[Whatever]\nFutureSetting=7\n").is_ok());
        let mut b = vec![0xff, 0xfe];
        for u in "[Logging]\nLogDisable=true\n".encode_utf16() {
            b.extend(u.to_le_bytes());
        }
        assert!(Config::from_bytes(&b).unwrap().log_disable);
    }

    #[test]
    fn diff_names_changed_settings_only() {
        let old = Config::parse(DEFAULT_INI);
        assert!(old.diff(&old.clone()).is_empty());
        let new = Config::parse(
            "[OutOfControlProcessRestraint]\nTotalProcessorUsageBeforeRestraint=40\nOocExclusions=x.exe\n\
             [ProcessDefaults]\nDefaultPriorities=a.exe:high\n",
        );
        let d = old.diff(&new);
        assert_eq!(d.len(), 3, "{d:?}");
        assert_eq!(d[0], "TotalProcessorUsageBeforeRestraint: 23.0 -> 40.0");
        assert!(d.iter().any(|l| l.starts_with("OocExclusions") && l.contains("x.exe")));
        assert!(d.iter().any(|l| l.contains("DefaultPriorities") && l.contains("a.exe:high")));
    }

    #[test]
    fn strict_parser_validates_profiles_and_game_mode_with_line_numbers() {
        let bad = |s: &str| Config::from_bytes(s.as_bytes()).unwrap_err();
        let e = bad("[OutOfControlProcessRestraint]\nOocOn=true\n\n[Profile.G]\nMatch=a.exe\nAffinity=0-3\nCpuSets=0-3\n");
        assert!(e.starts_with("line 7:") && e.contains("mutually exclusive"), "{e}");
        let e = bad("[Profile.G]\nMatch=a.exe\nPriority=realtime\n");
        assert!(e.starts_with("line 3:") && e.contains("realtime"), "{e}");
        let e = bad("[Profile.G]\nMatch=a.exe\nAffinity=FF\n");
        assert!(e.starts_with("line 3:") && e.contains("0x prefix"), "{e}");
        let e = bad("[GameMode]\nBackgroundPriority=maybe\n");
        assert!(e.starts_with("line 2:") && e.contains("true/false"), "{e}");
        let e = bad("[GameMode]\nPowerPlan=not-a-guid\n");
        assert!(e.starts_with("line 2:") && e.contains("GUID"), "{e}");
        let e = bad("[Profile.A]\nMatch=a\n[Profile.a]\nMatch=b\n");
        assert!(e.contains("duplicate profile name"), "{e}");
        let ok = Config::from_bytes(b"[GameMode]\nBackgroundPriority=true\nPowerPlan={8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c}\n[Profile.G]\nMatch=g.exe\nFutureKey=1\n").unwrap();
        assert!(ok.game_mode.background_priority);
        assert_eq!(ok.game_mode.power_plan.as_deref(), Some("{8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c}"));
        assert_eq!(ok.profiles.len(), 1);
    }

    #[test]
    fn configs_without_profiles_parse_exactly_as_before() {
        let c = Config::from_bytes(DEFAULT_INI.as_bytes()).unwrap();
        assert!(c.profiles.is_empty());
        assert_eq!(c.game_mode, GameModeCfg::default());
        assert!(!c.game_mode.background_priority && c.game_mode.power_plan.is_none(), "game mode options default to off");
        // [GameMode] keys written by other tools are not ours: unknown keys stay tolerated.
        assert!(Config::from_bytes(b"[GameMode]\nGamingModeEnabled=false\nTargetPowerPlan=Highest Performance\n").is_ok());
    }

    #[test]
    fn reload_diff_names_profile_and_game_mode_changes() {
        let old = Config::parse("[Profile.A]\nMatch=a.exe\nPriority=high\n[Profile.B]\nMatch=b.exe\n");
        let new = Config::parse("[Profile.A]\nMatch=a.exe\nPriority=idle\n[Profile.C]\nMatch=c.exe\n[GameMode]\nBackgroundPriority=true\n");
        let d = old.diff(&new);
        assert!(d.iter().any(|l| l.starts_with("Profile A:") && l.contains("priority high") && l.contains("priority idle")), "{d:?}");
        assert!(d.iter().any(|l| l == "Profile B: removed"), "{d:?}");
        assert!(d.iter().any(|l| l.starts_with("Profile C: added")), "{d:?}");
        assert!(d.iter().any(|l| l.contains("BackgroundPriority")), "{d:?}");
        assert!(old.diff(&old.clone()).is_empty());
    }

    #[test]
    fn log_rotation_settings_default_validate_and_diff() {
        let c = Config::from_bytes(DEFAULT_INI.as_bytes()).unwrap();
        assert_eq!(c.log_rotation, LogRotation { max_mb: 10, keep: 5 });
        // Existing configs without the new keys behave as before, with rotation on by default.
        let old = Config::from_bytes(b"[Logging]\nLogDisable=false\n").unwrap();
        assert_eq!(old.log_rotation, LogRotation::default());
        assert_eq!(Config::default().log_rotation, LogRotation::default());
        let c = Config::from_bytes(b"[Logging]\nMaxLogSizeMB=1024\nKeepLogFiles=0\n").unwrap();
        assert_eq!(c.log_rotation, LogRotation { max_mb: 1024, keep: 0 });
        let c = Config::from_bytes(b"[Logging]\nMaxLogSizeMB=1\nKeepLogFiles=50\n").unwrap();
        assert_eq!(c.log_rotation, LogRotation { max_mb: 1, keep: 50 });
        let bad = |s: &str| Config::from_bytes(s.as_bytes()).unwrap_err();
        assert!(bad("[Logging]\nMaxLogSizeMB=0\n").starts_with("line 2:"));
        assert!(bad("[Logging]\nMaxLogSizeMB=1025\n").contains("range"));
        assert!(bad("[Logging]\nLogDisable=false\nMaxLogSizeMB=ten\n").starts_with("line 3:"));
        assert!(bad("[Logging]\nMaxLogSizeMB=1.5\n").contains("whole number"));
        assert!(bad("[Logging]\nKeepLogFiles=51\n").contains("range"));
        assert!(bad("[Logging]\nKeepLogFiles=-1\n").contains("range"));
        assert!(Config::from_bytes(b"[Logging]\nFutureLogKey=7\n").is_ok(), "unknown keys stay tolerated");
        let d = Config::parse(DEFAULT_INI).diff(&Config::parse("[Logging]\nMaxLogSizeMB=2\nKeepLogFiles=0\n"));
        assert_eq!(d, vec!["MaxLogSizeMB: 10 -> 2", "KeepLogFiles: 5 -> 0"]);
    }

    #[test]
    fn decodes_utf16_bom() {
        let mut b = vec![0xff, 0xfe];
        for u in "[A]\nk=v\n".encode_utf16() {
            b.extend(u.to_le_bytes());
        }
        assert_eq!(parse_ini(&decode(&b)).get(&("a".into(), "k".into())).unwrap(), "v");
    }

    /// Every ```ini block in the docs, with the file it came from. Line endings normalised
    /// (a checkout with autocrlf turns the docs into CRLF).
    fn doc_ini_blocks() -> Vec<(&'static str, String)> {
        let docs = [
            ("README.md", include_str!("../README.md")),
            ("docs/CONFIG.md", include_str!("../docs/CONFIG.md")),
            ("docs/INSTALL.md", include_str!("../docs/INSTALL.md")),
            ("docs/PROFILES.md", include_str!("../docs/PROFILES.md")),
            ("docs/TROUBLESHOOTING.md", include_str!("../docs/TROUBLESHOOTING.md")),
            ("docs/USAGE.md", include_str!("../docs/USAGE.md")),
            ("docs/ARCHITECTURE.md", include_str!("../docs/ARCHITECTURE.md")),
        ];
        let mut out = vec![];
        for (name, text) in docs {
            let text = text.replace("\r\n", "\n");
            let mut rest = text.as_str();
            while let Some(start) = rest.find("```ini\n") {
                let body = &rest[start + 7..];
                let end = body.find("```").expect("unterminated ini block");
                out.push((name, body[..end].to_string()));
                rest = &body[end + 3..];
            }
        }
        out
    }

    #[test]
    fn the_default_config_block_in_the_docs_is_byte_identical_to_default_ini() {
        let blocks = doc_ini_blocks();
        let default = blocks.iter().find(|(f, b)| *f == "docs/CONFIG.md" && b.starts_with("; Process Corral configuration"));
        assert_eq!(default.expect("default block in docs/CONFIG.md").1, DEFAULT_INI);
    }

    #[test]
    fn every_ini_example_in_the_docs_passes_the_strict_parser() {
        let blocks = doc_ini_blocks();
        assert!(blocks.len() >= 7, "found {} blocks", blocks.len());
        for (file, body) in blocks {
            if let Err(e) = Config::from_bytes(body.as_bytes()) {
                panic!("{file}: {e}\n---\n{body}");
            }
        }
    }
}
