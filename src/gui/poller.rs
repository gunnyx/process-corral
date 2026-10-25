//! Background reader: once a second it looks at status.json, the service state, the config file and
//! the log tail, and sends a `Snapshot` to the UI. All file I/O happens here, never on the UI thread.

use eframe::egui;
use crate::logic::encode_service;
use crate::tray::TrayShared;
use process_corral::log::unix_ms_now;
use process_corral::logview::{self, Row};
use process_corral::os::win::pid_alive;
use process_corral::paths::Paths;
use process_corral::scm::{self, ServiceState};
use process_corral::status::Status;
use process_corral::statusview::{self, EngineView, NotRunning};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

pub const LOG_ROWS: usize = 500;
const CONFIG_MAX_BYTES: u64 = 1 << 20;

pub struct Snapshot {
    pub engine: EngineView,
    /// `engine.status` came from an earlier read because the file is unreadable right now.
    pub status_is_cached: bool,
    pub service: ServiceState,
    /// Bumped whenever the config file's content changes.
    pub config_version: u64,
    /// `None` = no config file.
    pub config: Option<Arc<Vec<u8>>>,
    pub config_error: Option<String>,
    pub log_rows: Arc<Vec<Row>>,
    pub log_note: Option<String>,
}

fn read_capped(path: &Path) -> std::io::Result<Vec<u8>> {
    let f = process_corral::fsutil::open_shared_read(path)?;
    let mut buf = vec![];
    f.take(CONFIG_MAX_BYTES + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > CONFIG_MAX_BYTES {
        return Err(std::io::Error::other("config file is larger than 1 MiB"));
    }
    Ok(buf)
}

type Stamp = (Option<SystemTime>, u64);
fn stamp(path: &Path) -> std::io::Result<Stamp> {
    let m = std::fs::metadata(path)?;
    Ok((m.modified().ok(), m.len()))
}

pub fn spawn(paths: Paths, ctx: egui::Context, tx: Sender<Snapshot>, stop: Arc<AtomicBool>, shared: Arc<TrayShared>) {
    std::thread::Builder::new()
        .name("poller".into())
        .spawn(move || {
            let mut last_good: Option<Status> = None;
            let (mut cfg_stamp, mut cfg_version, mut cfg_bytes, mut cfg_err): (Option<Stamp>, u64, Option<Arc<Vec<u8>>>, Option<String>) =
                (None, 0, None, None);
            let mut cfg_seen = false;
            let mut log = logview::Follower::new(&paths.log, LOG_ROWS);

            while !stop.load(Ordering::SeqCst) {
                // --- engine state
                let mut engine = statusview::evaluate(process_corral::fsutil::read_shared(&paths.status), unix_ms_now(), &pid_alive);
                let mut cached = false;
                match &engine.status {
                    Some(s) => last_good = Some(s.clone()),
                    None if matches!(engine.not_running, Some(NotRunning::Unreadable(_))) && last_good.is_some() => {
                        // Partial/locked/corrupt right now: keep showing the last good data, greyed.
                        engine.status = last_good.clone();
                        cached = true;
                    }
                    None => {}
                }

                // --- service
                let service = scm::service_state();

                // --- config (re-read only when it changes)
                let now_stamp = stamp(&paths.config);
                let changed = match (&now_stamp, &cfg_stamp) {
                    (Ok(a), Some(b)) => a != b,
                    (Ok(_), None) => true,
                    (Err(_), _) => !cfg_seen || cfg_stamp.is_some(),
                };
                if changed {
                    match read_capped(&paths.config) {
                        Ok(b) => {
                            cfg_bytes = Some(Arc::new(b));
                            cfg_err = None;
                            cfg_stamp = now_stamp.ok();
                            cfg_version += 1;
                            cfg_seen = true;
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            cfg_bytes = None;
                            cfg_err = None;
                            cfg_stamp = None;
                            cfg_version += 1;
                            cfg_seen = true;
                        }
                        // Locked or half-written: keep what we had and try again next second.
                        Err(e) => cfg_err = Some(format!("config file unreadable right now ({e})")),
                    }
                }

                // --- log tail (re-read when the file changes, shrinks or is replaced by a rotation)
                log.poll();

                shared.engine_running.store(engine.running(), Ordering::SeqCst);
                shared.service.store(encode_service(service), Ordering::SeqCst);
                let snap = Snapshot {
                    engine,
                    status_is_cached: cached,
                    service,
                    config_version: cfg_version,
                    config: cfg_bytes.clone(),
                    config_error: cfg_err.clone(),
                    log_rows: log.rows(),
                    log_note: log.note(),
                };
                if tx.send(snap).is_err() {
                    break;
                }
                ctx.request_repaint();

                // Sleep ~1s in short slices so shutdown is prompt.
                let until = Instant::now() + Duration::from_millis(1000);
                while Instant::now() < until && !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        })
        .expect("spawn poller");
}
