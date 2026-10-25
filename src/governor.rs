//! The run loop shared by console `run` and the Windows service: config + log setup, the tick
//! loop, live reload, and a shutdown path that always restores every restrained process and
//! flushes the log.

use crate::config::{Config, DEFAULT_INI};
use crate::engine::Engine;
use crate::log::{id, unix_ms_now, iso_utc, CsvLog, Event, Sink};
use crate::os::Os;
use crate::reload::{FileSource, Outcome, Reloader, Source};
use crate::status::{self, CpuInfo, RecentEvent, RestrainedProc, Status, StatusWriter, Thresholds, TopProc, TopologySummary};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Set by Ctrl+C (console) or an SCM stop/shutdown request (service).
pub static STOP: AtomicBool = AtomicBool::new(false);

/// How often the config file is checked for edits.
pub const RELOAD_POLL: Duration = Duration::from_secs(2);

/// How often `status.json` is rewritten.
pub const STATUS_INTERVAL: Duration = Duration::from_secs(2);

struct StatusState {
    writer: StatusWriter,
    mode: &'static str,
    interval: Duration,
    last: Option<Instant>,
}

impl StatusState {
    fn due(&self) -> bool {
        self.last.is_none_or(|t| t.elapsed() >= self.interval)
    }
}

/// Identity and clocks of one `run` call.
struct Session {
    pid: u32,
    start: Instant,
    started_unix_ms: u64,
}

#[derive(Debug)]
pub enum OpenError {
    BadConfig(String),
    Log(String),
}

pub struct Governor<S: Source> {
    cfg_path: PathBuf,
    pub cfg: Config,
    /// False when the config file didn't exist and defaults are in use.
    pub config_found: bool,
    sink: CsvLog,
    reloader: Reloader<S>,
    status: Option<StatusState>,
}

impl Governor<FileSource> {
    /// A config that exists but is invalid is an error: there are no old settings to fall back to.
    pub fn open(cfg_path: &Path, log_path: &Path, echo: bool) -> Result<Self, OpenError> {
        let config_found = cfg_path.exists();
        let cfg = if config_found {
            Config::load_strict(cfg_path).map_err(OpenError::BadConfig)?
        } else {
            Config::parse(DEFAULT_INI)
        };
        let sink = CsvLog::open_with(log_path, echo, cfg.log_disable, cfg.log_rotation)
            .map_err(|e| OpenError::Log(format!("cannot open log {}: {e}", log_path.display())))?;
        Ok(Self {
            cfg_path: cfg_path.to_path_buf(),
            cfg,
            config_found,
            sink,
            reloader: Reloader::new(FileSource::new(cfg_path.to_path_buf())),
            status: None,
        })
    }
}

impl<S: Source> Governor<S> {
    /// Publish `status.json` at `path` while running. Call after the single-instance lock is held.
    pub fn with_status(mut self, path: PathBuf, mode: &'static str) -> Self {
        self.status = Some(StatusState { writer: StatusWriter::new(path), mode, interval: STATUS_INTERVAL, last: None });
        self
    }

    #[cfg(test)]
    fn with_status_interval(mut self, interval: Duration) -> Self {
        if let Some(s) = &mut self.status {
            s.interval = interval;
        }
        self
    }

    /// Run until `stop` is set. On the way out — normal stop, SCM shutdown, or a panic — every
    /// restrained process gets its priority back and the log is flushed. Returns the number of
    /// restraints applied this session.
    pub fn run(&mut self, os: &mut dyn Os, stop: &AtomicBool, own_pid: u32, announce: bool) -> Result<u64, String> {
        let mut eng = Engine::new(own_pid);
        if announce {
            self.sink.event(&Event::system(
                id::GOVERNOR_STARTED,
                "Process management has begun",
                &format!("config {}", self.cfg_path.display()),
            ));
        }
        let session = Session { pid: own_pid, start: Instant::now(), started_unix_ms: unix_ms_now() };
        let looped = catch_unwind(AssertUnwindSafe(|| self.tick_loop(&mut eng, os, stop, &session)));
        let released = catch_unwind(AssertUnwindSafe(|| eng.release_all(os, &mut self.sink)));
        if announce {
            self.sink.event(&Event::system(id::GOVERNOR_STOPPED, "Process management has ended", "stopped on request"));
        }
        // Final snapshot (after the restore above) marks the file as stopped.
        let _ = catch_unwind(AssertUnwindSafe(|| self.write_status(true, &mut eng, os, &session)));
        self.sink.flush();
        if looped.is_err() || released.is_err() {
            return Err("internal error while managing processes; restrained priorities were restored where possible".into());
        }
        Ok(eng.stats.restraints_total)
    }

    fn tick_loop(&mut self, eng: &mut Engine, os: &mut dyn Os, stop: &AtomicBool, session: &Session) {
        let start = session.start;
        let mut last_poll = Instant::now();
        while !stop.load(Ordering::SeqCst) {
            if last_poll.elapsed() >= RELOAD_POLL {
                last_poll = Instant::now();
                let outcome = self.reloader.poll(&self.cfg);
                self.apply_reload(outcome);
            }
            let interval = Duration::from_millis(self.cfg.update_ms.max(100));
            let tick_start = Instant::now();
            eng.tick(os, &self.cfg, &mut self.sink, start.elapsed().as_millis() as u64);
            if self.status.as_ref().is_some_and(StatusState::due) {
                self.write_status(false, eng, os, session);
            }
            if let Some(rest) = interval.checked_sub(tick_start.elapsed()) {
                // Sleep in short slices so a stop request is honoured promptly.
                let end = Instant::now() + rest;
                while Instant::now() < end && !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    /// Build and publish a snapshot. A failed write is non-fatal and logged once per failure episode.
    fn write_status(&mut self, stopped: bool, eng: &mut Engine, os: &mut dyn Os, s: &Session) {
        let Some(mut st) = self.status.take() else { return };
        let now_ms = s.start.elapsed().as_millis() as u64;
        let now_unix = unix_ms_now();
        let topo = os.topology();
        let report = eng.profile_report(os, &self.cfg);
        let snapshot = Status {
            schema: status::SCHEMA,
            state: if stopped { "stopped" } else { "running" }.to_string(),
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            mode: st.mode.to_string(),
            pid: s.pid,
            started_at: iso_utc(s.started_unix_ms),
            updated_at: iso_utc(now_unix),
            stopped_at: stopped.then(|| iso_utc(now_unix)),
            update_interval_ms: st.interval.as_millis() as u64,
            config_path: self.cfg_path.display().to_string(),
            thresholds: Thresholds::from_config(&self.cfg.restraint),
            cpu: CpuInfo { total_percent: status::round1(eng.stats.sys_cpu), cores: os.cpu_count() },
            restraints_this_session: eng.stats.restraints_total,
            restrained: eng
                .restrained()
                .iter()
                .map(|r| RestrainedProc::new(r, os.priority(r.pid), s.started_unix_ms, now_ms))
                .collect(),
            top_processes: eng
                .snapshot()
                .into_iter()
                .take(status::TOP_PROCESSES)
                .map(|(pid, name, pct, restrained)| TopProc { pid, name, cpu_percent: status::round1(pct), restrained })
                .collect(),
            recent_events: self.sink.recent().map(RecentEvent::from).collect(),
            topology_summary: Some(TopologySummary::of(&topo)),
            topology: Some((*topo).clone()),
            profiles: report.profiles,
            applied_profiles: report.applied,
            refused: report.refused,
            game_mode: report.game,
            untouchable: eng
                .untouchable()
                .into_iter()
                .map(|u| status::UntouchableProc { name: u.name, pid: u.pid, reason: u.reason })
                .collect(),
        };
        if let Some(msg) = st.writer.write(&snapshot) {
            self.sink.event(&Event::system(id::STATUS_ERROR, "Status file write failed (non-fatal)", &msg));
        }
        st.last = Some(Instant::now());
        self.status = Some(st);
    }

    /// Swap in reloaded settings (or report why the old ones were kept). The engine takes the
    /// config on every tick and restraint bookkeeping is independent of it, so nothing else resets.
    fn apply_reload(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Unchanged => {}
            Outcome::Failed(msg) => self.sink.event(&Event::system(
                id::CONFIG_ERROR,
                "Config reload failed; keeping previous settings",
                &format!("{}: {msg}", self.cfg_path.display()),
            )),
            Outcome::Reloaded(new, changes) => {
                // New rotation limits apply from the next write, including the reload event below.
                self.sink.set_rotation(new.log_rotation);
                // Enable file logging before the event so it is recorded; disable after.
                if !new.log_disable {
                    if let Err(e) = self.sink.set_disabled(false) {
                        eprintln!("cannot enable log file: {e}");
                    }
                }
                self.sink.event(&Event::system(id::CONFIG_RELOADED, "Config reloaded", &changes.join("; ")));
                if new.log_disable {
                    let _ = self.sink.set_disabled(true);
                }
                self.cfg = new;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NORMAL;
    use crate::os::ProcInfo;

    /// One process that burns far more CPU than any threshold on every sample.
    struct HogOs {
        cpu: u64,
        class: u32,
    }
    impl Os for HogOs {
        fn processes(&mut self) -> Vec<ProcInfo> {
            vec![ProcInfo { pid: 100, ppid: 1, name: "hog.exe".into() }]
        }
        fn cpu_time(&mut self, _: u32) -> Option<u64> {
            self.cpu += 5_000_000_000;
            Some(self.cpu)
        }
        fn system_cpu_pct(&mut self) -> f64 { 100.0 }
        fn cpu_count(&self) -> u32 { 4 }
        fn foreground_pid(&mut self) -> Option<u32> { None }
        fn is_session0(&mut self, _: u32) -> bool { false }
        fn path(&mut self, _: u32) -> String { String::new() }
        fn priority(&mut self, _: u32) -> Option<u32> { Some(self.class) }
        fn set_priority(&mut self, _: u32, c: u32) -> bool { self.class = c; true }
        fn affinity(&mut self, _: u32) -> Option<u64> { Some(0xf) }
        fn set_affinity(&mut self, _: u32, _: u64) -> bool { true }
        fn terminate(&mut self, _: u32) -> bool { false }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pc-gov-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    const HOT_CFG: &str = "[OutOfControlProcessRestraint]\nTimeOverQuotaBeforeRestraint=0\n\
                           MinimumTimeOfRestraint=600000\n[Performance]\nUpdateSpeedCore=100\n";

    #[test]
    fn stop_restores_restrained_priorities_and_flushes_the_log() {
        let dir = temp_dir("stop");
        let (cfg, log) = (dir.join("c.ini"), dir.join("l.log"));
        std::fs::write(&cfg, HOT_CFG).unwrap();
        let mut gov = Governor::open(&cfg, &log, false).unwrap();
        let mut os = HogOs { cpu: 0, class: NORMAL };
        let stop = AtomicBool::new(false);

        let result = std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(800));
                stop.store(true, Ordering::SeqCst); // what the SCM stop handler does
            });
            gov.run(&mut os, &stop, 1, true)
        });

        assert!(result.unwrap() >= 1, "the hog was restrained while running");
        assert_eq!(os.class, NORMAL, "priority restored on stop");
        let text = String::from_utf8(crate::fsutil::read_shared(&log).unwrap()).unwrap();
        assert!(text.contains("Restraint begun"), "{text}");
        assert!(text.contains("governor stopping"), "restore is logged");
        assert!(text.contains("Process management has begun") && text.contains("Process management has ended"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stop_already_requested_exits_immediately_and_cleanly() {
        let dir = temp_dir("early");
        let mut gov = Governor::open(&dir.join("missing.ini"), &dir.join("l.log"), false).unwrap();
        assert!(!gov.config_found, "defaults used when the file is absent");
        let mut os = HogOs { cpu: 0, class: NORMAL };
        let stop = AtomicBool::new(true); // stop arrived during startup
        let t = Instant::now();
        assert_eq!(gov.run(&mut os, &stop, 1, true).unwrap(), 0);
        assert!(t.elapsed() < Duration::from_secs(1));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_config_is_refused_at_open_not_silently_defaulted() {
        let dir = temp_dir("bad");
        let cfg = dir.join("c.ini");
        std::fs::write(&cfg, "[OutOfControlProcessRestraint]\nOocOn=banana\n").unwrap();
        match Governor::open(&cfg, &dir.join("l.log"), false) {
            Err(OpenError::BadConfig(m)) => assert!(m.contains("OocOn"), "{m}"),
            other => panic!("expected BadConfig, got {:?}", other.err()),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn read_json(p: &Path) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
    }

    #[test]
    fn status_file_is_published_while_running_and_marked_stopped_after() {
        let dir = temp_dir("status");
        let (cfg, log, sp) = (dir.join("c.ini"), dir.join("l.log"), dir.join("status").join("status.json"));
        std::fs::write(&cfg, HOT_CFG).unwrap();
        let mut gov = Governor::open(&cfg, &log, false)
            .unwrap()
            .with_status(sp.clone(), "console")
            .with_status_interval(Duration::from_millis(200));
        let mut os = HogOs { cpu: 0, class: NORMAL };
        let stop = AtomicBool::new(false);
        let mut mid = serde_json::Value::Null;

        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(1200));
                mid = read_json(&sp);
                stop.store(true, Ordering::SeqCst);
            });
            gov.run(&mut os, &stop, 4242, false).unwrap();
        });

        assert_eq!(mid["state"], "running");
        assert_eq!(mid["mode"], "console");
        assert_eq!(mid["pid"], 4242);
        assert_eq!(mid["engine_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(mid["config_path"], cfg.display().to_string());
        assert!(mid["started_at"].as_str().unwrap().ends_with('Z'));
        assert!(mid["stopped_at"].is_null());
        assert_eq!(mid["update_interval_ms"], 200);
        assert_eq!(mid["thresholds"]["time_over_quota_ms"], 0);
        assert_eq!(mid["cpu"]["total_percent"], 100.0);
        assert!(mid["restraints_this_session"].as_u64().unwrap() >= 1);
        let r = &mid["restrained"][0];
        assert_eq!((r["name"].as_str(), r["pid"].as_u64()), (Some("hog.exe"), Some(100)));
        assert_eq!((r["original_priority"].as_str(), r["current_priority"].as_str()), (Some("normal"), Some("belownormal")));
        assert!(r["cpu_percent_at_restraint"].as_f64().unwrap() > 15.0);
        assert!(mid["top_processes"].as_array().unwrap().len() <= 10);
        assert_eq!(mid["top_processes"][0]["restrained"], true);
        let events: Vec<_> = mid["recent_events"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap()).collect();
        assert!(events.contains(&"Restraint begun"), "{events:?}");

        let fin = read_json(&sp);
        assert_eq!(fin["state"], "stopped");
        assert!(fin["stopped_at"].as_str().unwrap().ends_with('Z'));
        assert_eq!(fin["restrained"].as_array().unwrap().len(), 0, "restored before the final snapshot");
        let events: Vec<_> = fin["recent_events"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap().to_string()).collect();
        assert!(events.iter().any(|a| a.contains("governor stopping")), "{events:?}");
        assert_eq!(os.class, NORMAL);
        assert_eq!(std::fs::read_dir(sp.parent().unwrap()).unwrap().count(), 1, "no temp files left behind");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unwritable_status_path_is_non_fatal_and_logged_once() {
        let dir = temp_dir("statusfail");
        let (cfg, log) = (dir.join("c.ini"), dir.join("l.log"));
        std::fs::write(&cfg, HOT_CFG).unwrap();
        let blocker = dir.join("blocker");
        std::fs::write(&blocker, "a file, not a directory").unwrap();
        let mut gov = Governor::open(&cfg, &log, false)
            .unwrap()
            .with_status(blocker.join("status.json"), "service")
            .with_status_interval(Duration::from_millis(100));
        let mut os = HogOs { cpu: 0, class: NORMAL };
        let stop = AtomicBool::new(false);
        let result = std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(900));
                stop.store(true, Ordering::SeqCst);
            });
            gov.run(&mut os, &stop, 1, false)
        });
        assert!(result.is_ok(), "governor keeps running and stops cleanly");
        assert_eq!(os.class, NORMAL, "management itself was unaffected");
        let text = String::from_utf8(crate::fsutil::read_shared(&log).unwrap()).unwrap();
        assert_eq!(text.matches("Status file write failed").count(), 1, "{text}");
        assert!(text.contains("Restraint begun"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn status_reports_thresholds_from_a_live_reload() {
        let dir = temp_dir("statusreload");
        let (cfg, log, sp) = (dir.join("c.ini"), dir.join("l.log"), dir.join("status").join("status.json"));
        std::fs::write(&cfg, HOT_CFG).unwrap();
        let mut gov = Governor::open(&cfg, &log, false)
            .unwrap()
            .with_status(sp.clone(), "console")
            .with_status_interval(Duration::from_millis(200));
        let mut os = HogOs { cpu: 0, class: NORMAL };
        let stop = AtomicBool::new(false);
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(500));
                std::fs::write(
                    &cfg,
                    "[OutOfControlProcessRestraint]\nTimeOverQuotaBeforeRestraint=0\nMinimumTimeOfRestraint=600000\n\
                     TotalProcessorUsageBeforeRestraint=55\n[Performance]\nUpdateSpeedCore=100\n",
                )
                .unwrap();
                std::thread::sleep(Duration::from_millis(RELOAD_POLL.as_millis() as u64 + 1500));
                stop.store(true, Ordering::SeqCst);
            });
            gov.run(&mut os, &stop, 1, false).unwrap();
        });
        assert_eq!(read_json(&sp)["thresholds"]["total_cpu_percent"], 55.0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn live_reload_still_applies_while_running() {
        let dir = temp_dir("reload");
        let (cfg, log) = (dir.join("c.ini"), dir.join("l.log"));
        std::fs::write(&cfg, "[OutOfControlProcessRestraint]\nOocOn=false\n[Performance]\nUpdateSpeedCore=100\n").unwrap();
        let mut gov = Governor::open(&cfg, &log, false).unwrap();
        let mut os = HogOs { cpu: 0, class: NORMAL };
        let stop = AtomicBool::new(false);
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(500));
                std::fs::write(&cfg, HOT_CFG).unwrap(); // enable Restraint while running
                std::thread::sleep(Duration::from_millis(RELOAD_POLL.as_millis() as u64 + 1500));
                stop.store(true, Ordering::SeqCst);
            });
            gov.run(&mut os, &stop, 1, false).unwrap();
        });
        let text = String::from_utf8(crate::fsutil::read_shared(&log).unwrap()).unwrap();
        assert!(text.contains("Config reloaded") && text.contains("OocOn"), "{text}");
        assert!(text.contains("Restraint begun"), "new settings took effect");
        assert_eq!(os.class, NORMAL);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn live_reload_changes_the_log_rotation_limits_without_a_restart() {
        let dir = temp_dir("rotreload");
        let (cfg, log) = (dir.join("c.ini"), dir.join("l.log"));
        std::fs::write(&cfg, "[Performance]\nUpdateSpeedCore=100\n").unwrap(); // rotation settings absent: defaults
        let mut gov = Governor::open(&cfg, &log, false).unwrap();
        assert_eq!(gov.cfg.log_rotation, crate::config::LogRotation { max_mb: 10, keep: 5 });
        for i in 0..2 {
            gov.sink.event(&Event::system(1, &format!("bulk{i}"), &"z".repeat(600_000)));
        }
        let mut os = HogOs { cpu: 0, class: NORMAL };
        let stop = AtomicBool::new(false);
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(300));
                std::fs::write(&cfg, "[Logging]\nMaxLogSizeMB=1\nKeepLogFiles=2\n[Performance]\nUpdateSpeedCore=100\n").unwrap();
                std::thread::sleep(Duration::from_millis(RELOAD_POLL.as_millis() as u64 + 1500));
                stop.store(true, Ordering::SeqCst);
            });
            gov.run(&mut os, &stop, 1, false).unwrap();
        });
        assert_eq!(gov.cfg.log_rotation, crate::config::LogRotation { max_mb: 1, keep: 2 });
        let rotated = crate::log::rotated_path(&log, 1);
        assert!(rotated.exists(), "the 1.2 MB file rotated as soon as the lower limit was loaded");
        let old = String::from_utf8(crate::fsutil::read_shared(&rotated).unwrap()).unwrap();
        assert!(old.contains("bulk0") && old.contains("bulk1"));
        let now = String::from_utf8(crate::fsutil::read_shared(&log).unwrap()).unwrap();
        assert!(now.starts_with("\"UTC\","), "new file starts with the header: {now}");
        assert!(now.contains("Config reloaded"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
