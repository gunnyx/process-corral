//! `status.json`: a snapshot of the running governor for UIs to poll.
//!
//! Written atomically (see fsutil) about every 2 seconds. On a clean shutdown a final snapshot
//! with `"state": "stopped"` is written. A reader should treat a `"running"` file whose
//! `updated_at` is older than a few `update_interval_ms`, or whose `pid` is gone, as stale (the
//! governor crashed or was killed).
//!
//! The file lives in `...\ProcessCorral\status\`, which inherits the data directory's ACL: in
//! service mode that means SYSTEM/Administrators write, standard users read only.

use crate::config::{class_name, RestraintSettings};
use crate::engine::RestrainedInfo;
use crate::fsutil;
use crate::log::{iso_utc, Recorded};
use crate::topology::Topology;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Schema 2 adds topology, profiles, applied/refused profile changes and game mode. Every schema-1
/// field is unchanged, so a reader that ignores unknown fields still understands the rest.
pub const SCHEMA: u32 = 2;
/// Oldest schema a reader of this version still accepts (new fields are optional).
pub const MIN_SCHEMA: u32 = 1;
pub const TOP_PROCESSES: usize = 10;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Status {
    pub schema: u32,
    /// `"running"` or `"stopped"`.
    pub state: String,
    pub engine_version: String,
    /// `"console"` or `"service"`.
    pub mode: String,
    pub pid: u32,
    pub started_at: String,
    pub updated_at: String,
    pub stopped_at: Option<String>,
    pub update_interval_ms: u64,
    pub config_path: String,
    pub thresholds: Thresholds,
    pub cpu: CpuInfo,
    pub restraints_this_session: u64,
    pub restrained: Vec<RestrainedProc>,
    pub top_processes: Vec<TopProc>,
    pub recent_events: Vec<RecentEvent>,

    // ---- schema 2 (all optional when reading an older file)
    #[serde(default)]
    pub topology: Option<Topology>,
    #[serde(default)]
    pub topology_summary: Option<TopologySummary>,
    #[serde(default)]
    pub profiles: Vec<ProfileSummary>,
    #[serde(default)]
    pub applied_profiles: Vec<AppliedProfile>,
    #[serde(default)]
    pub refused: Vec<RefusedChange>,
    #[serde(default)]
    pub game_mode: GameModeStatus,
    /// Processes Windows refused Restraint access to; left alone until they exit or the rules change.
    /// Added after schema 2 shipped: optional, and readers that do not know it ignore it.
    #[serde(default)]
    pub untouchable: Vec<UntouchableProc>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct UntouchableProc {
    pub name: String,
    pub pid: u32,
    pub reason: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TopologySummary {
    pub logical_processors: usize,
    pub cores: usize,
    pub smt: bool,
    pub processor_groups: u16,
    pub vcache_group: Option<usize>,
    pub l3_groups: Vec<L3Summary>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct L3Summary {
    pub index: usize,
    pub size_mb: f64,
    /// "vcache", "frequency" or "".
    pub label: String,
    pub cpus: usize,
    pub cpu_list: String,
}

impl TopologySummary {
    pub fn of(t: &Topology) -> Self {
        TopologySummary {
            logical_processors: t.logical_count(),
            cores: t.core_count(),
            smt: t.has_smt(),
            processor_groups: t.groups,
            vcache_group: t.vcache_group(),
            l3_groups: t
                .l3
                .iter()
                .map(|g| L3Summary { index: g.index, size_mb: Topology::size_mb(g), label: t.label(g.index).to_string(), cpus: g.cpus.len(), cpu_list: Topology::cpu_list(&g.cpus) })
                .collect(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ProfileSummary {
    pub name: String,
    pub enabled: bool,
    #[serde(rename = "match")]
    pub matcher: String,
}

/// One change the engine made (or tried to make) to one process.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ChangeStatus {
    /// "priority", "affinity" or "cpu_sets".
    pub aspect: String,
    pub wanted: String,
    pub original: String,
    pub current: String,
    /// "applied", "applied_but_different", "retrying", "pending" or "refused".
    pub result: String,
    pub win32_error: Option<u32>,
    pub message: String,
    pub attempts: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AppliedProfile {
    pub pid: u32,
    pub process: String,
    pub profile: String,
    pub time: String,
    pub changes: Vec<ChangeStatus>,
}

/// A change that was finally refused (after the bounded retries) or never attempted.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct RefusedChange {
    pub pid: u32,
    pub process: String,
    pub profile: String,
    pub aspect: String,
    pub win32_error: Option<u32>,
    pub message: String,
    pub attempts: u32,
    pub time: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct GameModeStatus {
    pub active: bool,
    pub since: Option<String>,
    pub triggered_by: Option<GameTrigger>,
    pub background_lowered: u32,
    pub power_plan_switched: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct GameTrigger {
    pub pid: u32,
    pub process: String,
    pub profile: String,
}

/// The thresholds in effect. A file without `restraint_enabled` (one written by an older build) is
/// rejected with a message saying so, and readers show "Engine not running" with that reason.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(try_from = "ThresholdsRaw")]
pub struct Thresholds {
    pub restraint_enabled: bool,
    pub total_cpu_percent: f64,
    pub per_process_percent: f64,
    pub time_over_quota_ms: u64,
    pub restore_below_percent: f64,
    pub min_restraint_ms: u64,
    pub max_restraint_ms: u64,
    pub tame_only_normal: bool,
    pub lower_to_idle: bool,
    pub restrain_by_affinity: bool,
}

#[derive(Deserialize)]
struct ThresholdsRaw {
    #[serde(default)]
    restraint_enabled: Option<bool>,
    total_cpu_percent: f64,
    per_process_percent: f64,
    time_over_quota_ms: u64,
    restore_below_percent: f64,
    min_restraint_ms: u64,
    max_restraint_ms: u64,
    tame_only_normal: bool,
    lower_to_idle: bool,
    restrain_by_affinity: bool,
}

impl TryFrom<ThresholdsRaw> for Thresholds {
    type Error = String;
    fn try_from(r: ThresholdsRaw) -> Result<Self, String> {
        let on = r.restraint_enabled.ok_or(
            "missing field `restraint_enabled` in thresholds (the status file was written by an older Process Corral; restart the engine to rewrite it)",
        )?;
        Ok(Thresholds {
            restraint_enabled: on,
            total_cpu_percent: r.total_cpu_percent,
            per_process_percent: r.per_process_percent,
            time_over_quota_ms: r.time_over_quota_ms,
            restore_below_percent: r.restore_below_percent,
            min_restraint_ms: r.min_restraint_ms,
            max_restraint_ms: r.max_restraint_ms,
            tame_only_normal: r.tame_only_normal,
            lower_to_idle: r.lower_to_idle,
            restrain_by_affinity: r.restrain_by_affinity,
        })
    }
}

impl Thresholds {
    pub fn from_config(pb: &RestraintSettings) -> Self {
        Self {
            restraint_enabled: pb.enabled,
            total_cpu_percent: pb.total_cpu_before_restraint,
            per_process_percent: pb.per_process_before_restraint,
            time_over_quota_ms: pb.time_over_quota_ms,
            restore_below_percent: pb.per_process_for_restore,
            min_restraint_ms: pb.min_restraint_ms,
            max_restraint_ms: pb.max_restraint_ms,
            tame_only_normal: pb.tame_only_normal,
            lower_to_idle: pb.lower_to_idle,
            restrain_by_affinity: pb.restrain_by_affinity,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CpuInfo {
    pub total_percent: f64,
    pub cores: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RestrainedProc {
    pub pid: u32,
    pub name: String,
    pub original_priority: String,
    /// Read from the OS at snapshot time; falls back to the class we set if unreadable.
    pub current_priority: String,
    pub restrained_since: String,
    pub restrained_for_ms: u64,
    pub cpu_percent_at_restraint: f64,
}

impl RestrainedProc {
    /// `engine_start_unix_ms` converts the engine's monotonic clock to wall time.
    pub fn new(r: &RestrainedInfo, current_class: Option<u32>, engine_start_unix_ms: u64, engine_now_ms: u64) -> Self {
        Self {
            pid: r.pid,
            name: r.name.clone(),
            original_priority: class_name(r.original_class).to_string(),
            current_priority: class_name(current_class.unwrap_or(r.set_class)).to_string(),
            restrained_since: iso_utc(engine_start_unix_ms + r.since_ms),
            restrained_for_ms: engine_now_ms.saturating_sub(r.since_ms),
            cpu_percent_at_restraint: round1(r.cpu_pct),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TopProc {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f64,
    pub restrained: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RecentEvent {
    pub time: String,
    pub pid: u32,
    pub name: String,
    pub action_id: u32,
    pub action: String,
    pub info: String,
    pub path: String,
}

impl From<&Recorded> for RecentEvent {
    fn from(r: &Recorded) -> Self {
        Self {
            time: iso_utc(r.unix_ms),
            pid: r.event.pid,
            name: r.event.name.clone(),
            action_id: r.event.id,
            action: r.event.action.clone(),
            info: r.event.info.clone(),
            path: r.event.path.clone(),
        }
    }
}

/// One decimal place; non-finite values (which JSON cannot represent) become 0.
pub fn round1(x: f64) -> f64 {
    if x.is_finite() { (x * 10.0).round() / 10.0 } else { 0.0 }
}

pub struct StatusWriter {
    path: PathBuf,
    failing: bool,
}

impl StatusWriter {
    /// Only call while holding the single-instance lock (it deletes leftover temp files).
    pub fn new(path: PathBuf) -> Self {
        fsutil::remove_stale_temps(&path);
        Self { path, failing: false }
    }

    /// Write a snapshot. Never fails the caller: returns `Some(message)` only for the first
    /// failure of an episode (so it is logged once), and `None` for success or a repeat failure.
    /// After a success, a later failure is reported again.
    pub fn write(&mut self, status: &Status) -> Option<String> {
        match self.try_write(status) {
            Ok(()) => {
                self.failing = false;
                None
            }
            Err(e) => {
                let first = !self.failing;
                self.failing = true;
                first.then_some(e)
            }
        }
    }

    fn try_write(&self, status: &Status) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(status).map_err(|e| format!("cannot serialize status: {e}"))?;
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        fsutil::atomic_write(&self.path, &bytes, false).map_err(|e| format!("cannot write {}: {e}", self.path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BELOW_NORMAL, NORMAL};
    use crate::log::Event;
    use serde_json::Value;

    pub(crate) fn sample() -> Status {
        let r = RestrainedInfo { pid: 42, name: "hog.exe".into(), original_class: NORMAL, set_class: BELOW_NORMAL, since_ms: 5_000, cpu_pct: 48.349 };
        Status {
            schema: SCHEMA,
            state: "running".into(),
            engine_version: "9.9.9".into(),
            mode: "service".into(),
            pid: 777,
            started_at: iso_utc(1_700_000_000_000),
            updated_at: iso_utc(1_700_000_010_000),
            stopped_at: None,
            update_interval_ms: 2000,
            config_path: r"C:\ProgramData\ProcessCorral\config\corral.ini".into(),
            thresholds: Thresholds::from_config(&RestraintSettings::default()),
            cpu: CpuInfo { total_percent: 37.5, cores: 16 },
            restraints_this_session: 3,
            restrained: vec![RestrainedProc::new(&r, Some(BELOW_NORMAL), 1_700_000_000_000, 8_000)],
            top_processes: vec![TopProc { pid: 42, name: "hog.exe".into(), cpu_percent: 48.3, restrained: true }],
            recent_events: vec![RecentEvent::from(&Recorded {
                unix_ms: 1_700_000_005_000,
                event: Event { pid: 42, name: "hog.exe".into(), id: 43, action: "Restraint begun".into(), info: "48.3% CPU".into(), path: String::new() },
            })],
            topology: None,
            topology_summary: None,
            profiles: vec![],
            applied_profiles: vec![],
            refused: vec![],
            game_mode: Default::default(),
            untouchable: vec![],
        }
    }

    fn json(s: &Status) -> Value {
        serde_json::from_slice(&serde_json::to_vec_pretty(s).unwrap()).unwrap()
    }

    #[test]
    fn serializes_every_documented_field() {
        let v = json(&sample());
        assert_eq!(v["schema"], 2);
        assert_eq!(v["state"], "running");
        assert_eq!(v["engine_version"], "9.9.9");
        assert_eq!(v["mode"], "service");
        assert_eq!(v["pid"], 777);
        assert_eq!(v["started_at"], "2023-11-14T22:13:20.000Z");
        assert_eq!(v["updated_at"], "2023-11-14T22:13:30.000Z");
        assert!(v["stopped_at"].is_null());
        assert_eq!(v["update_interval_ms"], 2000);
        assert_eq!(v["config_path"], r"C:\ProgramData\ProcessCorral\config\corral.ini");
        assert_eq!(v["thresholds"]["total_cpu_percent"], 23.0);
        assert_eq!(v["thresholds"]["per_process_percent"], 15.0);
        assert_eq!(v["thresholds"]["time_over_quota_ms"], 900);
        assert_eq!(v["thresholds"]["min_restraint_ms"], 4200);
        assert_eq!(v["thresholds"]["restraint_enabled"], true);
        assert_eq!(v["cpu"]["total_percent"], 37.5);
        assert_eq!(v["cpu"]["cores"], 16);
        assert_eq!(v["restraints_this_session"], 3);

        let r = &v["restrained"][0];
        assert_eq!((r["pid"].as_u64(), r["name"].as_str()), (Some(42), Some("hog.exe")));
        assert_eq!(r["original_priority"], "normal");
        assert_eq!(r["current_priority"], "belownormal");
        assert_eq!(r["restrained_since"], "2023-11-14T22:13:25.000Z");
        assert_eq!(r["restrained_for_ms"], 3000);
        assert_eq!(r["cpu_percent_at_restraint"], 48.3);

        assert_eq!(v["top_processes"][0]["restrained"], true);
        let e = &v["recent_events"][0];
        assert_eq!((e["action_id"].as_u64(), e["action"].as_str()), (Some(43), Some("Restraint begun")));
        assert_eq!(e["time"], "2023-11-14T22:13:25.000Z");
    }

    #[test]
    fn schema_2_round_trips_and_keeps_every_schema_1_field() {
        use crate::topology::fake_9950x3d;
        let mut s = sample();
        let topo = fake_9950x3d(true);
        s.topology_summary = Some(TopologySummary::of(&topo));
        s.topology = Some(topo);
        s.profiles = vec![ProfileSummary { name: "Game".into(), enabled: true, matcher: "game.exe".into() }];
        s.applied_profiles = vec![AppliedProfile {
            pid: 42,
            process: "game.exe".into(),
            profile: "Game".into(),
            time: iso_utc(1_700_000_000_000),
            changes: vec![ChangeStatus {
                aspect: "affinity".into(),
                wanted: "0xFFFF".into(),
                original: "0xFFFFFFFF".into(),
                current: "0xFFFF".into(),
                result: "applied".into(),
                win32_error: None,
                message: String::new(),
                attempts: 1,
            }],
        }];
        s.refused = vec![RefusedChange {
            pid: 43,
            process: "locked.exe".into(),
            profile: "Game".into(),
            aspect: "priority".into(),
            win32_error: Some(5),
            message: "Access is denied".into(),
            attempts: 5,
            time: iso_utc(1_700_000_001_000),
        }];
        s.game_mode = GameModeStatus {
            active: true,
            since: Some(iso_utc(1_700_000_002_000)),
            triggered_by: Some(GameTrigger { pid: 42, process: "game.exe".into(), profile: "Game".into() }),
            background_lowered: 7,
            power_plan_switched: true,
        };
        let bytes = serde_json::to_vec_pretty(&s).unwrap();
        let back: Status = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), serde_json::to_value(&s).unwrap(), "lossless round trip");
        assert_eq!(back.schema, 2);
        assert_eq!(back.topology.as_ref().unwrap().vcache_group(), Some(0));
        assert_eq!(back.topology_summary.as_ref().unwrap().l3_groups[0].label, "vcache");
        assert_eq!(back.refused[0].win32_error, Some(5));
        assert!(back.game_mode.active && back.game_mode.triggered_by.unwrap().pid == 42);

        // Every schema-1 field is still there under the same name and shape...
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        for k in ["schema", "state", "engine_version", "mode", "pid", "started_at", "updated_at", "stopped_at", "update_interval_ms", "config_path", "thresholds", "cpu", "restraints_this_session", "restrained", "top_processes", "recent_events"] {
            assert!(v.get(k).is_some(), "schema-1 field {k} missing");
        }
        // ...so a reader that only knows schema 1 can still parse the file (it just ignores the rest).
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct OldStatus {
            schema: u32,
            state: String,
            pid: u32,
            updated_at: String,
            thresholds: Thresholds,
            cpu: CpuInfo,
            restrained: Vec<RestrainedProc>,
            top_processes: Vec<TopProc>,
            recent_events: Vec<RecentEvent>,
        }
        let old: OldStatus = serde_json::from_slice(&bytes).unwrap();
        assert_eq!((old.state.as_str(), old.pid), ("running", 777));
    }

    #[test]
    fn a_schema_1_file_still_reads_with_empty_new_fields() {
        let mut v = serde_json::to_value(sample()).unwrap();
        let obj = v.as_object_mut().unwrap();
        for k in ["topology", "topology_summary", "profiles", "applied_profiles", "refused", "game_mode", "untouchable"] {
            obj.remove(k);
        }
        obj.insert("schema".into(), 1.into());
        let s: Status = serde_json::from_value(v).unwrap();
        assert_eq!(s.schema, 1);
        assert!(s.topology.is_none() && s.profiles.is_empty() && s.applied_profiles.is_empty() && s.refused.is_empty() && s.untouchable.is_empty());
        assert_eq!(s.game_mode, GameModeStatus::default());
    }

    #[test]
    fn current_priority_reflects_a_change_made_behind_our_back() {
        let r = RestrainedInfo { pid: 1, name: "a".into(), original_class: NORMAL, set_class: BELOW_NORMAL, since_ms: 0, cpu_pct: 1.0 };
        assert_eq!(RestrainedProc::new(&r, Some(crate::config::HIGH), 0, 0).current_priority, "high");
        assert_eq!(RestrainedProc::new(&r, None, 0, 0).current_priority, "belownormal", "unreadable -> class we set");
    }

    #[test]
    fn stopped_state_and_hostile_strings_round_trip() {
        let mut s = sample();
        s.state = "stopped".into();
        s.stopped_at = Some(iso_utc(1_700_000_020_000));
        s.restrained.clear();
        let nasty = "we\"ird\\name\n\u{1}\t\u{e9}\u{1F600}.exe";
        s.top_processes[0].name = nasty.into();
        s.config_path = r#"C:\a "b"\c.ini"#.into();
        let v = json(&s);
        assert_eq!(v["state"], "stopped");
        assert_eq!(v["stopped_at"], "2023-11-14T22:13:40.000Z");
        assert_eq!(v["restrained"].as_array().unwrap().len(), 0);
        assert_eq!(v["top_processes"][0]["name"], nasty);
        assert_eq!(v["config_path"], r#"C:\a "b"\c.ini"#);
    }

    #[test]
    fn non_finite_numbers_never_reach_the_file() {
        assert_eq!(round1(f64::NAN), 0.0);
        assert_eq!(round1(f64::INFINITY), 0.0);
        assert_eq!(round1(12.349), 12.3);
        assert_eq!(round1(12.35), 12.4);
    }

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pc-status-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn writer_creates_the_directory_and_replaces_atomically() {
        let d = dir("write");
        let path = d.join("status").join("status.json");
        let mut w = StatusWriter::new(path.clone());
        assert_eq!(w.write(&sample()), None);
        let first: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(first["pid"], 777);
        let mut s = sample();
        s.state = "stopped".into();
        assert_eq!(w.write(&s), None);
        let second: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(second["state"], "stopped");
        let left: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 1, "no temp files: {left:?}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn write_failure_is_reported_once_per_episode_and_never_panics() {
        let d = dir("fail");
        let blocker = d.join("status"); // a *file* where the directory should be
        std::fs::write(&blocker, "x").unwrap();
        let mut w = StatusWriter::new(blocker.join("status.json"));

        let msg = w.write(&sample()).expect("first failure is reported");
        assert!(msg.starts_with("cannot "), "{msg}");
        for _ in 0..5 {
            assert_eq!(w.write(&sample()), None, "repeats are silent");
        }

        std::fs::remove_file(&blocker).unwrap(); // fixed -> success clears the episode
        assert_eq!(w.write(&sample()), None);
        assert!(blocker.join("status.json").is_file());

        std::fs::remove_dir_all(&blocker).unwrap();
        std::fs::write(&blocker, "x").unwrap(); // broken again -> a new episode is reported
        assert!(w.write(&sample()).is_some());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn new_writer_clears_stale_temp_files_from_a_crash() {
        let d = dir("stale");
        let sd = d.join("status");
        std::fs::create_dir_all(&sd).unwrap();
        std::fs::write(sd.join("status.json.99.0.tmp"), "junk").unwrap();
        let _w = StatusWriter::new(sd.join("status.json"));
        assert_eq!(std::fs::read_dir(&sd).unwrap().count(), 0);
        std::fs::remove_dir_all(&d).unwrap();
    }

    fn thresholds_json(extra: serde_json::Value) -> serde_json::Value {
        let mut v = serde_json::json!({
            "total_cpu_percent": 23.0, "per_process_percent": 15.0, "time_over_quota_ms": 900, "restore_below_percent": 5.0,
            "min_restraint_ms": 4200, "max_restraint_ms": 0, "tame_only_normal": true, "lower_to_idle": false, "restrain_by_affinity": false
        });
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        v
    }

    #[test]
    fn the_enabled_flag_is_written_as_restraint_enabled() {
        for on in [true, false] {
            let mut s = sample();
            s.thresholds.restraint_enabled = on;
            let v = json(&s);
            assert_eq!(v["thresholds"]["restraint_enabled"], on);
        }
        let mut cfg = RestraintSettings::default();
        cfg.enabled = false;
        assert!(!Thresholds::from_config(&cfg).restraint_enabled);
    }

    #[test]
    fn thresholds_without_restraint_enabled_are_rejected_clearly() {
        let read = |extra: serde_json::Value| serde_json::from_value::<Thresholds>(thresholds_json(extra));
        assert!(!read(serde_json::json!({ "restraint_enabled": false })).unwrap().restraint_enabled);
        assert!(read(serde_json::json!({ "restraint_enabled": true })).unwrap().restraint_enabled);
        let e = read(serde_json::json!({})).unwrap_err().to_string();
        assert!(e.contains("missing field `restraint_enabled`") && e.contains("older Process Corral"), "{e}");
    }

    #[test]
    fn a_whole_status_file_without_restraint_enabled_fails_to_parse() {
        let mut v = serde_json::to_value(sample()).unwrap();
        let th = v["thresholds"].as_object_mut().unwrap();
        th.remove("restraint_enabled");
        let e = serde_json::from_value::<Status>(v).unwrap_err().to_string();
        assert!(e.contains("restraint_enabled"), "{e}");
    }

    #[test]
    fn untouchable_round_trips_and_a_file_without_it_still_reads() {
        let mut s = sample();
        s.untouchable = vec![UntouchableProc { name: "bf6.exe".into(), pid: 4242, reason: "access denied: protected".into() }];
        let bytes = serde_json::to_vec_pretty(&s).unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["untouchable"][0]["name"], "bf6.exe");
        assert_eq!(v["untouchable"][0]["pid"], 4242);
        assert_eq!(v["untouchable"][0]["reason"], "access denied: protected");
        assert_eq!(v["schema"], 2, "additive: the schema number is unchanged");
        let back: Status = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.untouchable, s.untouchable);

        // Without the field (written by an older engine, or nothing is protected): empty list.
        let mut old = v.clone();
        old.as_object_mut().unwrap().remove("untouchable");
        let none: Status = serde_json::from_value(old).unwrap();
        assert!(none.untouchable.is_empty());
        assert_eq!(json(&sample())["untouchable"], serde_json::json!([]), "an empty list is written explicitly");
    }

    /// The Status struct exactly as the previous GUI build declares it (no `untouchable`): it must
    /// still read a file that has the new field.
    #[test]
    fn the_previous_reader_still_parses_a_file_with_untouchable() {
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct PreviousStatus {
            schema: u32,
            state: String,
            engine_version: String,
            mode: String,
            pid: u32,
            started_at: String,
            updated_at: String,
            stopped_at: Option<String>,
            update_interval_ms: u64,
            config_path: String,
            thresholds: Thresholds,
            cpu: CpuInfo,
            restraints_this_session: u64,
            restrained: Vec<RestrainedProc>,
            top_processes: Vec<TopProc>,
            recent_events: Vec<RecentEvent>,
            #[serde(default)]
            topology: Option<Topology>,
            #[serde(default)]
            topology_summary: Option<TopologySummary>,
            #[serde(default)]
            profiles: Vec<ProfileSummary>,
            #[serde(default)]
            applied_profiles: Vec<AppliedProfile>,
            #[serde(default)]
            refused: Vec<RefusedChange>,
            #[serde(default)]
            game_mode: GameModeStatus,
        }
        let mut s = sample();
        s.untouchable = vec![UntouchableProc { name: "bf6.exe".into(), pid: 1, reason: "r".into() }];
        let prev: PreviousStatus = serde_json::from_slice(&serde_json::to_vec_pretty(&s).unwrap()).unwrap();
        assert_eq!((prev.schema, prev.pid, prev.restrained.len()), (2, 777, 1));
    }
}
