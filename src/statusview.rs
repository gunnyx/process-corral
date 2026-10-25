//! Decide whether the engine is really running from `status.json`, for UIs.
//!
//! The file alone cannot be trusted: a crashed or killed engine leaves `"state": "running"`
//! behind. So "running" requires all of: the file reads and parses, `state` is not `stopped`,
//! `updated_at` is recent, and the recorded `pid` still exists.

use crate::log::parse_iso_utc;
use crate::status::{Status, MIN_SCHEMA, SCHEMA};
use std::io;

/// `updated_at` older than this (or 3 update intervals, if larger) means the engine is gone.
pub const STALE_AFTER_MS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq)]
pub enum NotRunning {
    /// No status file: the engine never ran here, or was cleaned up.
    Missing,
    /// Unreadable, partial or corrupt file.
    Unreadable(String),
    /// The engine shut down cleanly.
    Stopped,
    /// `updated_at` is too old; the engine stopped writing (crash, kill, hang).
    Stale { age_ms: u64 },
    /// The recorded pid no longer exists.
    PidGone(u32),
}

impl NotRunning {
    pub fn describe(&self) -> String {
        match self {
            NotRunning::Missing => "no status file yet".into(),
            NotRunning::Unreadable(why) => format!("status file unreadable ({why})"),
            NotRunning::Stopped => "the engine shut down cleanly".into(),
            NotRunning::Stale { age_ms } => format!("no update for {}s", age_ms / 1000),
            NotRunning::PidGone(pid) => format!("process {pid} is gone"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct EngineView {
    /// The parsed file when it could be read, even if the engine is not running (last session).
    pub status: Option<Status>,
    /// `None` means the engine is running.
    pub not_running: Option<NotRunning>,
}

impl EngineView {
    pub fn running(&self) -> bool {
        self.not_running.is_none()
    }
}

/// `raw` is the result of reading the status file. Pure apart from `pid_alive`, so every case is
/// testable.
pub fn evaluate(raw: io::Result<Vec<u8>>, now_unix_ms: u64, pid_alive: &dyn Fn(u32) -> bool) -> EngineView {
    let bytes = match raw {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return EngineView { status: None, not_running: Some(NotRunning::Missing) };
        }
        Err(e) => return EngineView { status: None, not_running: Some(NotRunning::Unreadable(e.to_string())) },
    };
    let bad = |why: String| EngineView { status: None, not_running: Some(NotRunning::Unreadable(why)) };
    // Tolerate a UTF-8 byte-order mark (some Windows tools add one when rewriting a file).
    let body = bytes.strip_prefix(&[0xef, 0xbb, 0xbf][..]).unwrap_or(&bytes);
    let status: Status = match serde_json::from_slice(body) {
        Ok(s) => s,
        Err(e) => return bad(if bytes.is_empty() { "empty file".into() } else { e.to_string() }),
    };
    if !(MIN_SCHEMA..=SCHEMA).contains(&status.schema) {
        let why = format!("unsupported schema {} (this version reads {MIN_SCHEMA}-{SCHEMA})", status.schema);
        return EngineView { status: None, not_running: Some(NotRunning::Unreadable(why)) };
    }
    let verdict = judge(&status, now_unix_ms, pid_alive);
    EngineView { status: Some(status), not_running: verdict }
}

fn judge(s: &Status, now_unix_ms: u64, pid_alive: &dyn Fn(u32) -> bool) -> Option<NotRunning> {
    if s.state == "stopped" {
        return Some(NotRunning::Stopped);
    }
    let Some(updated) = parse_iso_utc(&s.updated_at) else {
        return Some(NotRunning::Unreadable(format!("bad updated_at {:?}", s.updated_at)));
    };
    let limit = STALE_AFTER_MS.max(3 * s.update_interval_ms);
    let age_ms = now_unix_ms.saturating_sub(updated); // a clock a little behind ours is not "stale"
    if age_ms > limit {
        return Some(NotRunning::Stale { age_ms });
    }
    if !pid_alive(s.pid) {
        return Some(NotRunning::PidGone(s.pid));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::iso_utc;
    use serde_json::{json, Value};

    const NOW: u64 = 1_700_000_100_000;

    fn doc(state: &str, updated_ms: u64, pid: u32) -> Value {
        json!({
            "schema": 1, "state": state, "engine_version": "0.1.0", "mode": "service", "pid": pid,
            "started_at": iso_utc(1_700_000_000_000), "updated_at": iso_utc(updated_ms),
            "stopped_at": null, "update_interval_ms": 2000, "config_path": "C:\\c.ini",
            "thresholds": { "restraint_enabled": true, "total_cpu_percent": 23.0, "per_process_percent": 15.0,
                "time_over_quota_ms": 900, "restore_below_percent": 5.0, "min_restraint_ms": 4200,
                "max_restraint_ms": 0, "tame_only_normal": true, "lower_to_idle": false, "restrain_by_affinity": false },
            "cpu": { "total_percent": 12.5, "cores": 8 }, "restraints_this_session": 2,
            "restrained": [ { "pid": 9, "name": "hog.exe", "original_priority": "normal", "current_priority": "belownormal",
                "restrained_since": iso_utc(NOW - 5000), "restrained_for_ms": 5000, "cpu_percent_at_restraint": 50.0 } ],
            "top_processes": [ { "pid": 9, "name": "hog.exe", "cpu_percent": 50.0, "restrained": true } ],
            "recent_events": [ { "time": iso_utc(NOW - 5000), "pid": 9, "name": "hog.exe", "action_id": 43,
                "action": "Restraint begun", "info": "", "path": "" } ]
        })
    }
    fn eval(v: &Value, alive: bool) -> EngineView {
        evaluate(Ok(serde_json::to_vec(v).unwrap()), NOW, &|_| alive)
    }

    #[test]
    fn a_status_file_without_restraint_enabled_is_not_running_with_a_clear_reason() {
        let mut v = doc("running", NOW - 1500, 1234);
        let th = v["thresholds"].as_object_mut().unwrap();
        th.remove("restraint_enabled");
        let view = eval(&v, true);
        assert!(!view.running() && view.status.is_none());
        let why = view.not_running.unwrap();
        assert!(matches!(why, NotRunning::Unreadable(_)), "{why:?}");
        let text = why.describe(); // what the GUI header shows under "Engine not running"
        assert!(text.starts_with("status file unreadable (") && text.contains("restraint_enabled") && text.contains("older Process Corral"), "{text}");
    }

    #[test]
    fn healthy_engine_is_running() {
        let v = eval(&doc("running", NOW - 1500, 1234), true);
        assert!(v.running(), "{:?}", v.not_running);
        let s = v.status.unwrap();
        assert_eq!((s.pid, s.mode.as_str(), s.restrained.len()), (1234, "service", 1));
        assert_eq!(s.restrained[0].current_priority, "belownormal");
    }

    #[test]
    fn a_utf8_bom_is_tolerated() {
        let mut bytes = vec![0xef, 0xbb, 0xbf];
        bytes.extend(serde_json::to_vec(&doc("running", NOW - 500, 7)).unwrap());
        let v = evaluate(Ok(bytes), NOW, &|_| true);
        assert!(v.running(), "{:?}", v.not_running);
    }

    #[test]
    fn missing_file() {
        let v = evaluate(Err(io::Error::new(io::ErrorKind::NotFound, "nope")), NOW, &|_| true);
        assert_eq!(v.not_running, Some(NotRunning::Missing));
        assert!(v.status.is_none());
    }

    #[test]
    fn locked_or_unreadable_file_is_not_a_crash() {
        let v = evaluate(Err(io::Error::new(io::ErrorKind::PermissionDenied, "locked")), NOW, &|_| true);
        assert!(matches!(v.not_running, Some(NotRunning::Unreadable(ref m)) if m.contains("locked")));
    }

    #[test]
    fn corrupt_partial_or_wrong_shape_files() {
        let full = serde_json::to_vec_pretty(&doc("running", NOW, 1)).unwrap();
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", vec![]),
            ("garbage", b"\x00\x01 not json".to_vec()),
            ("truncated", full[..full.len() / 2].to_vec()),
            ("not an object", b"[1,2,3]".to_vec()),
            ("wrong types", br#"{"schema":1,"state":5}"#.to_vec()),
            ("missing fields", br#"{"schema":1,"state":"running"}"#.to_vec()),
            ("invalid utf8", vec![b'{', 0xff, 0xfe, b'}']),
        ];
        for (name, bytes) in cases {
            let v = evaluate(Ok(bytes), NOW, &|_| true);
            assert!(matches!(v.not_running, Some(NotRunning::Unreadable(_))), "{name}: {:?}", v.not_running);
            assert!(v.status.is_none(), "{name}");
        }
        // Newer/older schema is refused rather than misread.
        let mut d = doc("running", NOW, 1);
        d["schema"] = json!(3);
        let v = eval(&d, true);
        assert!(matches!(&v.not_running, Some(NotRunning::Unreadable(m)) if m.contains("schema 3")), "{:?}", v.not_running);
    }

    #[test]
    fn clean_shutdown_keeps_the_last_session_data() {
        let v = eval(&doc("stopped", NOW - 100, 1234), true);
        assert_eq!(v.not_running, Some(NotRunning::Stopped), "stopped wins even if the pid happens to be reused");
        assert_eq!(v.status.unwrap().restraints_this_session, 2);
    }

    #[test]
    fn stale_update_means_not_running_with_data_kept() {
        let v = eval(&doc("running", NOW - 12_000, 1234), true);
        assert_eq!(v.not_running, Some(NotRunning::Stale { age_ms: 12_000 }));
        assert!(v.status.is_some());
        assert!(v.not_running.unwrap().describe().contains("12s"));
    }

    #[test]
    fn staleness_boundary_and_slow_update_intervals() {
        assert!(eval(&doc("running", NOW - 9_900, 1), true).running());
        assert!(!eval(&doc("running", NOW - 10_100, 1), true).running());
        // An engine configured to write every 10s is allowed 3 intervals before it counts as stale.
        let mut d = doc("running", NOW - 25_000, 1);
        d["update_interval_ms"] = json!(10_000);
        assert!(eval(&d, true).running());
        d["updated_at"] = json!(iso_utc(NOW - 31_000));
        assert!(!eval(&d, true).running());
    }

    #[test]
    fn clock_skew_into_the_future_is_tolerated() {
        assert!(eval(&doc("running", NOW + 3_000, 1), true).running());
    }

    #[test]
    fn dead_pid_means_not_running_even_with_a_fresh_timestamp() {
        let v = eval(&doc("running", NOW - 500, 4321), false);
        assert_eq!(v.not_running, Some(NotRunning::PidGone(4321)));
        assert!(v.status.is_some());
    }

    #[test]
    fn unparseable_timestamp_is_unreadable() {
        let mut d = doc("running", NOW, 1);
        d["updated_at"] = json!("yesterday");
        assert!(matches!(eval(&d, true).not_running, Some(NotRunning::Unreadable(_))));
    }

    #[test]
    fn reads_what_the_engine_actually_writes() {
        // Round-trip through the engine's own writer-side types.
        use crate::status::{CpuInfo, Status, Thresholds};
        let s = Status {
            schema: SCHEMA,
            state: "running".into(),
            engine_version: "0.1.0".into(),
            mode: "console".into(),
            pid: 77,
            started_at: iso_utc(NOW - 60_000),
            updated_at: iso_utc(NOW - 1_000),
            stopped_at: None,
            update_interval_ms: 2000,
            config_path: "c.ini".into(),
            thresholds: Thresholds::from_config(&crate::config::RestraintSettings::default()),
            cpu: CpuInfo { total_percent: 1.0, cores: 4 },
            restraints_this_session: 0,
            restrained: vec![],
            top_processes: vec![],
            recent_events: vec![],
            topology: None,
            topology_summary: None,
            profiles: vec![],
            applied_profiles: vec![],
            refused: vec![],
            game_mode: Default::default(),
            untouchable: vec![],
        };
        let v = evaluate(Ok(serde_json::to_vec_pretty(&s).unwrap()), NOW, &|pid| pid == 77);
        assert!(v.running());
    }
}
