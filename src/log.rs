//! CSV action log. The column layout follows common process-governor log conventions.
//!
//! The active file rotates by size: once `corral.log` reaches the configured limit it becomes
//! `corral.1.log`, older rotated files shift up one number, and the one that would exceed the
//! keep count is deleted. See `CsvLog::rotate`.

use crate::config::{LogRotation, LOG_KEEP_MAX};
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Action IDs 33/43/44 follow common process-governor numbering; the rest are our own.
pub mod id {
    pub const PB_END_TERMINATED: u32 = 33;
    pub const PB_BEGIN: u32 = 43;
    pub const PB_LOWERED: u32 = 44;
    pub const PB_END_RESTORED: u32 = 45;
    pub const PB_END_FOREGROUND: u32 = 46;
    pub const PB_END_TIMEOUT: u32 = 47;
    pub const PB_ERROR: u32 = 48;
    pub const PB_EXEMPT: u32 = 49;
    pub const PB_END_EXCLUDED: u32 = 50;
    /// Follows the common numbering for "Configuration file loaded" (0x3d).
    pub const CONFIG_RELOADED: u32 = 61;
    /// Follows the common numbering for "Process management has begun" (0x3c).
    pub const GOVERNOR_STARTED: u32 = 60;
    pub const GOVERNOR_STOPPED: u32 = 81;
    pub const SERVICE_ERROR: u32 = 82;
    pub const STATUS_ERROR: u32 = 83;
    pub const PROFILE_APPLIED: u32 = 90;
    pub const PROFILE_DIFFERS: u32 = 91;
    pub const PROFILE_REFUSED: u32 = 92;
    pub const PROFILE_RESTORED: u32 = 93;
    pub const PROFILE_OVERRIDES: u32 = 94;
    pub const PROFILE_RETRY: u32 = 95;
    pub const GAME_ENTER: u32 = 96;
    pub const GAME_LEAVE: u32 = 97;
    pub const GAME_INFO: u32 = 98;
    pub const CONFIG_ERROR: u32 = 80;
    pub const DEFAULT_PRIORITY: u32 = 70;
    pub const DEFAULT_AFFINITY: u32 = 71;
    pub const DISALLOWED: u32 = 72;
    pub const INSTANCE_LIMIT: u32 = 73;
    /// A size-based rotation of the log file failed; logging continues in the current file.
    pub const LOG_ROTATE_ERROR: u32 = 84;
}

#[derive(Clone, Debug)]
pub struct Event {
    pub pid: u32,
    pub name: String,
    pub id: u32,
    pub action: String,
    pub info: String,
    pub path: String,
}

impl Event {
    /// An event about the governor itself rather than a managed process.
    pub fn system(id: u32, action: &str, info: &str) -> Event {
        Event { pid: 0, name: "process-corral".into(), id, action: action.into(), info: info.into(), path: String::new() }
    }
}

pub trait Sink {
    fn event(&mut self, e: &Event);
}

pub struct CsvLog {
    path: PathBuf,
    file: Option<File>,
    /// File logging is switched off (`LogDisable`); distinct from `file` being closed mid-rotation.
    disabled: bool,
    machine: String,
    user: String,
    echo: bool,
    /// Last `RECENT_EVENTS` events, kept in memory for the status file (recorded even when
    /// file logging is disabled).
    recent: VecDeque<Recorded>,
    rotation: LogRotation,
    /// Bytes in the active file, header included.
    size: u64,
    /// The active file was moved aside to the staging name but not yet given its `corral.N.log` name.
    staged: bool,
    /// A rotation failed; don't try again before this moment.
    retry_at: Option<Instant>,
    backoff: Duration,
    /// Rotation has been failing and the failure has already been logged (once per episode).
    failing: bool,
    /// Lines that could not be written because the file could not be (re)opened; flushed first
    /// once it can be.
    backlog: VecDeque<String>,
}

pub const RECENT_EVENTS: usize = 20;
/// Wait this long after a failed rotation before trying again.
pub const ROTATE_RETRY: Duration = Duration::from_secs(30);
const BACKLOG_LINES: usize = 10_000;
const HEADER: &str = "\"UTC\",\"UTCText\",\"Machine\",\"User\",\"PID\",\"Basename\",\"ActionId\",\"Action\",\"Info\",\"Path\"\n";

/// `corral.log` -> `corral.3.log` (same folder, same extension).
pub fn rotated_path(path: &Path, n: u32) -> PathBuf {
    sibling(path, &n.to_string())
}

/// Where the active file waits while the numbered files shift: `corral.rotating.log`.
fn staging_path(path: &Path) -> PathBuf {
    sibling(path, "rotating")
}

fn sibling(path: &Path, tag: &str) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let name = match path.extension() {
        Some(e) => format!("{stem}.{tag}.{}", e.to_string_lossy()),
        None => format!("{stem}.{tag}"),
    };
    path.with_file_name(name)
}

#[derive(Clone, Debug)]
pub struct Recorded {
    pub unix_ms: u64,
    pub event: Event,
}

fn q(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Open for append (creating it), share-all so readers and the rotation rename are never blocked
/// by us. A new or empty file gets the CSV header. Returns the file and its size.
fn open_file(path: &Path) -> io::Result<(File, u64)> {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    let mut f = crate::fsutil::shared_options().create(true).append(true).open(path)?;
    let mut size = f.metadata()?.len();
    if size == 0 {
        f.write_all(HEADER.as_bytes())?;
        size = HEADER.len() as u64;
    }
    Ok((f, size))
}

impl CsvLog {
    pub fn open(path: &Path, echo: bool, disabled: bool) -> io::Result<Self> {
        Self::open_with(path, echo, disabled, LogRotation::default())
    }

    pub fn open_with(path: &Path, echo: bool, disabled: bool, rotation: LogRotation) -> io::Result<Self> {
        let (file, size) = if disabled { (None, 0) } else { open_file(path).map(|(f, s)| (Some(f), s))? };
        let mut log = Self {
            path: path.to_path_buf(),
            file,
            disabled,
            machine: std::env::var("COMPUTERNAME").unwrap_or_default(),
            user: std::env::var("USERNAME").unwrap_or_default(),
            echo,
            recent: VecDeque::new(),
            rotation,
            size,
            staged: !disabled && staging_path(path).exists(), // a crash left a rotation half done
            retry_at: None,
            backoff: ROTATE_RETRY,
            failing: false,
            backlog: VecDeque::new(),
        };
        log.maybe_rotate(); // already over the limit at startup
        Ok(log)
    }

    /// Oldest first.
    pub fn recent(&self) -> impl Iterator<Item = &Recorded> {
        self.recent.iter()
    }

    /// Push everything written so far to disk (called on shutdown).
    pub fn flush(&mut self) {
        if self.file.is_none() && !self.disabled {
            self.reopen();
        }
        if let Some(f) = &mut self.file {
            let _ = f.flush();
            let _ = f.sync_all();
        }
    }

    /// Turn file logging on or off at runtime (config reload). Console echo is unaffected.
    pub fn set_disabled(&mut self, disabled: bool) -> io::Result<()> {
        if disabled {
            self.file = None;
            self.backlog.clear();
        } else if self.disabled {
            let (f, size) = open_file(&self.path)?;
            self.file = Some(f);
            self.size = size;
            self.staged = staging_path(&self.path).exists();
        }
        self.disabled = disabled;
        self.maybe_rotate();
        Ok(())
    }

    /// New rotation limits from a config reload; they apply from the next write.
    pub fn set_rotation(&mut self, rotation: LogRotation) {
        self.rotation = rotation;
        self.retry_at = None;
    }

    /// (Re)open the active file after a rotation or a write error, then write any held-back lines.
    fn reopen(&mut self) {
        match open_file(&self.path) {
            Ok((f, size)) => {
                self.file = Some(f);
                self.size = size;
                while let Some(line) = self.backlog.pop_front() {
                    let ok = self.file.as_mut().is_some_and(|f| f.write_all(line.as_bytes()).is_ok());
                    if ok {
                        self.size += line.len() as u64;
                    } else {
                        self.backlog.push_front(line); // keep order; try again on the next write
                        self.file = None;
                        break;
                    }
                }
            }
            Err(_) => self.file = None,
        }
    }

    /// One `write_all` per record so a reader never sees half a line from us. On failure the line
    /// is kept in memory and the file is reopened on the next write.
    fn append(&mut self, line: &str) {
        let Some(f) = &mut self.file else { return };
        match f.write_all(line.as_bytes()) {
            Ok(()) => self.size += line.len() as u64,
            Err(_) => {
                self.file = None;
                self.hold(line);
            }
        }
    }

    fn hold(&mut self, line: &str) {
        if self.backlog.len() == BACKLOG_LINES {
            self.backlog.pop_front();
        }
        self.backlog.push_back(line.to_string());
    }

    fn write_line(&mut self, line: String) {
        if self.disabled {
            return;
        }
        if self.file.is_none() {
            self.reopen();
        }
        if self.file.is_some() {
            self.append(&line);
        } else {
            self.hold(&line);
        }
    }

    /// Check the size before a write and rotate if the file has reached the limit. A failed
    /// rotation is reported once, then retried after the back-off; the current file keeps growing
    /// meanwhile, so no line is lost.
    fn maybe_rotate(&mut self) {
        if self.disabled || !(self.staged || self.size >= self.rotation.max_bytes()) {
            return;
        }
        if self.retry_at.is_some_and(|t| Instant::now() < t) {
            return;
        }
        match self.rotate() {
            Ok(()) => {
                self.retry_at = None;
                self.failing = false;
            }
            Err(e) => {
                self.retry_at = Some(Instant::now() + self.backoff);
                if !self.failing {
                    self.failing = true;
                    let info = format!("{}: {e}; still logging to the current file, will retry", self.path.display());
                    self.record(&Event::system(id::LOG_ROTATE_ERROR, "Log rotation failed (non-fatal)", &info), false);
                }
            }
        }
    }

    /// `corral.log` -> `corral.1.log`, `corral.1.log` -> `corral.2.log` ... and the file past
    /// `KeepLogFiles` is deleted. With `KeepLogFiles=0` the file is truncated instead.
    ///
    /// Our own handle is closed first (Windows refuses to rename an open file unless every handle
    /// shares delete access), and the active file is renamed *before* anything else is touched, so a
    /// failure there changes nothing. After that step a failure leaves the half-finished state in
    /// place (`staged`) and the next attempt resumes it without losing or double-shifting any file.
    fn rotate(&mut self) -> io::Result<()> {
        self.file = None;
        let result = self.rotate_closed();
        self.reopen(); // success or not, keep logging: a fresh file, or the old one appended to
        result
    }

    fn rotate_closed(&mut self) -> io::Result<()> {
        let keep = self.rotation.keep.min(LOG_KEEP_MAX);
        let numbered = |i: u32| rotated_path(&self.path, i);
        let staging = staging_path(&self.path);
        if keep == 0 {
            let _ = fs::remove_file(&staging);
            let mut f = crate::fsutil::shared_options().write(true).create(true).truncate(true).open(&self.path)?;
            f.write_all(HEADER.as_bytes())?;
            self.staged = false;
            for i in 1..=LOG_KEEP_MAX {
                let _ = fs::remove_file(numbered(i));
            }
            return Ok(());
        }
        if !self.staged && self.path.exists() {
            fs::rename(&self.path, &staging)?;
            self.staged = true;
        }
        // Lowering KeepLogFiles retires the files past the new count.
        for i in keep + 1..=LOG_KEEP_MAX {
            let _ = fs::remove_file(numbered(i));
        }
        // The lowest free slot; if all are taken, the oldest makes room. Shifting only ever moves a
        // file into a free slot, so an interrupted rotation can be resumed from here.
        let gap = match (1..=keep).find(|i| !numbered(*i).exists()) {
            Some(g) => g,
            None => {
                fs::remove_file(numbered(keep))?;
                keep
            }
        };
        for i in (1..gap).rev() {
            fs::rename(numbered(i), numbered(i + 1))?;
        }
        if staging.exists() {
            fs::rename(&staging, numbered(1))?;
        }
        self.staged = false;
        Ok(())
    }

    /// Record an event in memory and the file. The rotation check comes first so the line lands in
    /// the right file; the failure notice itself skips it.
    fn record(&mut self, e: &Event, rotate: bool) {
        if rotate {
            self.maybe_rotate();
        }
        if self.recent.len() == RECENT_EVENTS {
            self.recent.pop_front();
        }
        self.recent.push_back(Recorded { unix_ms: unix_ms_now(), event: e.clone() });
        let (ft, ts) = now_stamp();
        if self.echo {
            println!("[{}] {:>6} {:<24} {} - {}", &ts[11..], e.pid, e.name, e.action, e.info);
        }
        let line = format!(
            "{},{},{},{},{},{},{},{},{},{}\n",
            q(&ft.to_string()), q(&ts), q(&self.machine), q(&self.user), q(&e.pid.to_string()),
            q(&e.name), q(&e.id.to_string()), q(&e.action), q(&e.info), q(&e.path)
        );
        self.write_line(line);
    }
}

/// Days since 1970-01-01 -> (y, m, d), civil-from-days.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + (m <= 2) as i64, m, d)
}

pub fn unix_ms_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// ISO 8601 UTC with milliseconds, e.g. `2023-11-14T22:13:20.000Z`.
pub fn iso_utc(unix_ms: u64) -> String {
    let secs = (unix_ms / 1000) as i64;
    let (y, m, d) = civil(secs.div_euclid(86400));
    let t = secs.rem_euclid(86400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z", t / 3600, t % 3600 / 60, t % 60, unix_ms % 1000)
}

/// Inverse of `civil`: days since 1970-01-01 for a proleptic Gregorian date.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Parse `YYYY-MM-DDTHH:MM:SS[.fff]Z` (as written by `iso_utc`) back to unix milliseconds.
pub fn parse_iso_utc(s: &str) -> Option<u64> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut dp = date.split('-');
    let (y, m, d) = (dp.next()?.parse::<i64>().ok()?, dp.next()?.parse::<u32>().ok()?, dp.next()?.parse::<u32>().ok()?);
    if dp.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut tp = hms.split(':');
    let (hh, mm, ss) = (tp.next()?.parse::<u64>().ok()?, tp.next()?.parse::<u64>().ok()?, tp.next()?.parse::<u64>().ok()?);
    if tp.next().is_some() || hh > 23 || mm > 59 || ss > 60 || frac.is_empty() || frac.len() > 3 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let ms = format!("{frac:0<3}").parse::<u64>().ok()?;
    let secs = days_from_civil(y, m, d) * 86400 + (hh * 3600 + mm * 60 + ss) as i64;
    u64::try_from(secs).ok().map(|s| s * 1000 + ms)
}

/// Returns (FILETIME ticks, "MM-DD-YYYY HH:MM:SS" in UTC).
fn now_stamp() -> (u64, String) {
    let d = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    let secs = d.as_secs() as i64;
    let ft = (secs as u64 + 11_644_473_600) * 10_000_000 + d.subsec_nanos() as u64 / 100;
    let (y, m, dd) = civil(secs.div_euclid(86400));
    let t = secs.rem_euclid(86400);
    (ft, format!("{m:02}-{dd:02}-{y} {:02}:{:02}:{:02}", t / 3600, t % 3600 / 60, t % 60))
}

impl Sink for CsvLog {
    fn event(&mut self, e: &Event) {
        self.record(e, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_timestamps() {
        assert_eq!(super::iso_utc(1_700_000_000_000), "2023-11-14T22:13:20.000Z");
        assert_eq!(super::iso_utc(1_700_000_000_123), "2023-11-14T22:13:20.123Z");
        assert_eq!(super::iso_utc(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn iso_parse_round_trips_and_rejects_garbage() {
        for ms in [0u64, 1, 999, 1_700_000_000_123, 951_782_400_000 /* 2000-02-29 */, 4_102_444_800_000] {
            assert_eq!(parse_iso_utc(&iso_utc(ms)), Some(ms), "{ms}");
        }
        assert_eq!(parse_iso_utc("2023-11-14T22:13:20Z"), Some(1_700_000_000_000));
        assert_eq!(parse_iso_utc("2023-11-14T22:13:20.5Z"), Some(1_700_000_000_500));
        for bad in ["", "garbage", "2023-11-14 22:13:20Z", "2023-11-14T22:13:20", "2023-13-14T22:13:20Z",
                    "2023-11-14T25:13:20Z", "2023-11-14T22:13:20.1234Z", "2023-11-14T22:13Z", "1969-01-01T00:00:00Z"] {
            assert_eq!(parse_iso_utc(bad), None, "{bad}");
        }
    }

    #[test]
    fn keeps_only_the_last_20_events_even_with_file_logging_off() {
        let mut l = CsvLog::open(Path::new(""), false, true).unwrap();
        for i in 0..30 {
            l.event(&Event::system(1, &format!("event {i}"), ""));
        }
        let got: Vec<_> = l.recent().map(|r| r.event.action.clone()).collect();
        assert_eq!(got.len(), RECENT_EVENTS);
        assert_eq!(got.first().unwrap(), "event 10");
        assert_eq!(got.last().unwrap(), "event 29");
    }

    #[test]
    fn civil_dates() {
        assert_eq!(super::civil(0), (1970, 1, 1));
        assert_eq!(super::civil(20_000), (2024, 10, 4));
    }

    // ---- rotation ------------------------------------------------------------------------

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pc-logrot-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// An event whose log line is a little over `n` bytes.
    fn big(tag: &str, n: usize) -> Event {
        Event::system(1, tag, &"y".repeat(n))
    }

    fn text(p: &Path) -> String {
        String::from_utf8(crate::fsutil::read_shared(p).unwrap()).unwrap()
    }

    fn names(d: &Path) -> Vec<String> {
        let mut v: Vec<_> = fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    /// Action names (tags) of every record in a file, in order.
    fn tags(p: &Path) -> Vec<String> {
        text(p).lines().skip(1).map(|l| crate::logview::parse_csv_line(l)[7].clone()).collect()
    }

    fn policy(max_mb: u64, keep: u32) -> LogRotation {
        LogRotation { max_mb, keep }
    }

    /// Two ~600 KB events: the file is over 1 MB afterwards, so the next write rotates it.
    fn fill(l: &mut CsvLog, round: u32) {
        l.event(&big(&format!("g{round}a"), 600_000));
        l.event(&big(&format!("g{round}b"), 600_000));
    }

    const HEADER_LINE: &str = "\"UTC\",\"UTCText\",\"Machine\",\"User\",\"PID\",\"Basename\",\"ActionId\",\"Action\",\"Info\",\"Path\"";
    const NOTICE: &str = "Log rotation failed (non-fatal)";

    #[test]
    fn rotates_at_the_threshold_and_the_new_file_starts_with_the_header() {
        let d = dir("thresh");
        let p = d.join("corral.log");
        let mut l = CsvLog::open_with(&p, false, false, policy(1, 5)).unwrap();
        l.event(&big("small", 100));
        l.event(&big("under", 900_000));
        assert_eq!(names(&d), vec!["corral.log"], "just under 1 MB: no rotation yet");
        l.event(&big("reaches", 200_000)); // checked before the write, so this one still lands in the file
        assert_eq!(names(&d), vec!["corral.log"]);
        l.event(&big("after", 10)); // the file has reached 1 MB: rotate first
        assert_eq!(names(&d), vec!["corral.1.log", "corral.log"]);
        assert_eq!(tags(&d.join("corral.1.log")), vec!["small", "under", "reaches"]);
        assert!(fs::metadata(d.join("corral.1.log")).unwrap().len() >= 1_048_576);
        assert_eq!(text(&p).lines().next().unwrap(), HEADER_LINE);
        assert_eq!(tags(&p), vec!["after"]);
        assert_eq!(l.recent().count(), 4, "the in-memory list is not affected by rotation");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn names_shift_up_and_the_oldest_is_deleted() {
        let d = dir("shift");
        let p = d.join("corral.log");
        let mut l = CsvLog::open_with(&p, false, false, policy(1, 2)).unwrap();
        for g in 0..5 {
            fill(&mut l, g);
        }
        l.event(&big("last", 10));
        assert_eq!(names(&d), vec!["corral.1.log", "corral.2.log", "corral.log"], "never more than KeepLogFiles rotated files");
        assert_eq!(tags(&d.join("corral.2.log")), vec!["g3a", "g3b"], "oldest kept; g0..g2 were deleted");
        assert_eq!(tags(&d.join("corral.1.log")), vec!["g4a", "g4b"], "newest rotated file");
        assert_eq!(tags(&p), vec!["last"]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn keep_zero_truncates_instead_of_keeping_files() {
        let d = dir("keep0");
        let p = d.join("corral.log");
        fs::write(d.join("corral.1.log"), "stale rotated file from an earlier setting").unwrap();
        let mut l = CsvLog::open_with(&p, false, false, policy(1, 0)).unwrap();
        fill(&mut l, 0);
        l.event(&big("after", 10));
        assert_eq!(names(&d), vec!["corral.log"], "no rotated files, and leftovers are retired");
        assert_eq!(text(&p).lines().next().unwrap(), HEADER_LINE);
        assert_eq!(tags(&p), vec!["after"]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_file_already_over_the_limit_rotates_at_startup() {
        let d = dir("startup");
        let p = d.join("corral.log");
        {
            let mut l = CsvLog::open_with(&p, false, false, policy(10, 5)).unwrap();
            fill(&mut l, 0);
        }
        assert!(fs::metadata(&p).unwrap().len() > 1_048_576);
        let l = CsvLog::open_with(&p, false, false, policy(1, 5)).unwrap();
        assert_eq!(names(&d), vec!["corral.1.log", "corral.log"]);
        assert_eq!(tags(&d.join("corral.1.log")), vec!["g0a", "g0b"]);
        assert_eq!(text(&p), format!("{HEADER_LINE}\n"));
        assert_eq!(l.recent().count(), 0);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn reloaded_limits_apply_without_a_restart() {
        let d = dir("reload");
        let p = d.join("corral.log");
        let mut l = CsvLog::open_with(&p, false, false, policy(10, 5)).unwrap();
        fill(&mut l, 0);
        l.event(&big("still-one-file", 10));
        assert_eq!(names(&d), vec!["corral.log"], "10 MB limit: 1.2 MB is fine");
        l.set_rotation(policy(1, 5));
        l.event(&big("rotates-now", 10));
        assert_eq!(names(&d), vec!["corral.1.log", "corral.log"]);
        assert_eq!(tags(&p), vec!["rotates-now"]);
        fill(&mut l, 1);
        l.event(&big("x", 10));
        fill(&mut l, 2);
        l.event(&big("y", 10));
        assert_eq!(names(&d), vec!["corral.1.log", "corral.2.log", "corral.3.log", "corral.log"]);
        // Fewer files to keep: the surplus goes at the next rotation.
        l.set_rotation(policy(1, 1));
        fill(&mut l, 3);
        l.event(&big("z", 10));
        assert_eq!(names(&d), vec!["corral.1.log", "corral.log"]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn disabled_logging_never_touches_files() {
        let d = dir("disabled");
        let p = d.join("corral.log");
        let mut l = CsvLog::open_with(&p, false, true, policy(1, 5)).unwrap();
        fill(&mut l, 0);
        l.event(&big("x", 10));
        assert!(names(&d).is_empty());
        assert_eq!(l.recent().count(), 3, "events are still kept in memory");
        l.set_disabled(false).unwrap();
        l.event(&big("back", 10));
        assert_eq!(tags(&p), vec!["back"]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn custom_log_names_rotate_next_to_themselves() {
        assert_eq!(rotated_path(Path::new(r"D:\pc\corral.log"), 2), PathBuf::from(r"D:\pc\corral.2.log"));
        assert_eq!(rotated_path(Path::new(r"D:\pc\my.txt"), 1), PathBuf::from(r"D:\pc\my.1.txt"));
        assert_eq!(rotated_path(Path::new(r"D:\pc\noext"), 1), PathBuf::from(r"D:\pc\noext.1"));
    }

    /// Hold `p` open the way a careless viewer does: reads and writes allowed, delete/rename not.
    #[cfg(windows)]
    fn hold_without_delete_sharing(p: &Path) -> File {
        use std::os::windows::fs::OpenOptionsExt;
        fs::OpenOptions::new().read(true).share_mode(0x1 | 0x2).open(p).unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn a_held_open_file_makes_rotation_fail_without_losing_lines_and_it_retries_later() {
        let d = dir("held");
        let p = d.join("corral.log");
        fs::write(d.join("corral.1.log"), "history that must survive failed attempts").unwrap();
        let mut l = CsvLog::open_with(&p, false, false, policy(1, 3)).unwrap();
        l.backoff = Duration::from_millis(80);
        fill(&mut l, 0);
        let holder = hold_without_delete_sharing(&p);

        l.event(&big("while-held-1", 10)); // rotation attempted here, fails
        l.event(&big("while-held-2", 10)); // inside the back-off: not even attempted
        l.event(&big("while-held-3", 10));
        assert_eq!(names(&d), vec!["corral.1.log", "corral.log"], "nothing was shifted or created");
        assert_eq!(fs::read_to_string(d.join("corral.1.log")).unwrap(), "history that must survive failed attempts");
        let failures = |l: &CsvLog| l.recent().filter(|r| r.event.id == id::LOG_ROTATE_ERROR).count();
        assert_eq!(failures(&l), 1, "reported once per episode");

        std::thread::sleep(Duration::from_millis(120));
        l.event(&big("retry-while-still-held", 10)); // back-off over, still held: fails again, silently
        assert_eq!(failures(&l), 1, "still the same episode");
        assert_eq!(names(&d), vec!["corral.1.log", "corral.log"]);

        drop(holder);
        std::thread::sleep(Duration::from_millis(120));
        l.event(&big("after-release", 10));
        assert_eq!(names(&d), vec!["corral.1.log", "corral.2.log", "corral.log"]);
        assert_eq!(fs::read_to_string(d.join("corral.2.log")).unwrap(), "history that must survive failed attempts");
        // Every line is in exactly one place: the rotated file has all of them but the last.
        assert_eq!(
            tags(&d.join("corral.1.log")),
            vec!["g0a", "g0b", NOTICE, "while-held-1", "while-held-2", "while-held-3", "retry-while-still-held"]
        );
        assert_eq!(tags(&p), vec!["after-release"]);

        // A later failure is a new episode and is reported again.
        fill(&mut l, 1);
        let holder = hold_without_delete_sharing(&p);
        l.event(&big("held-again", 10));
        assert_eq!(failures(&l), 2);
        drop(holder);
        fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_failure_after_the_active_file_moved_is_resumed_without_double_shifting() {
        let d = dir("resume");
        let p = d.join("corral.log");
        for i in 1..=2 {
            fs::write(d.join(format!("corral.{i}.log")), format!("old{i}")).unwrap();
        }
        let mut l = CsvLog::open_with(&p, false, false, policy(1, 2)).unwrap();
        l.backoff = Duration::from_millis(60);
        fill(&mut l, 0);
        // The oldest file cannot be deleted right now, so the rotation stops after moving the active file aside.
        let holder = hold_without_delete_sharing(&d.join("corral.2.log"));
        l.event(&big("during", 10));
        assert!(l.staged);
        assert!(names(&d).contains(&"corral.rotating.log".to_string()));
        assert_eq!(fs::read_to_string(d.join("corral.1.log")).unwrap(), "old1", "no half-shift");
        l.event(&big("still-logging", 10)); // goes to the fresh corral.log, header and all
        assert_eq!(text(&p).lines().next().unwrap(), HEADER_LINE);
        drop(holder);
        std::thread::sleep(Duration::from_millis(100));
        l.event(&big("resumed", 10));
        assert!(!l.staged);
        assert_eq!(names(&d), vec!["corral.1.log", "corral.2.log", "corral.log"]);
        assert_eq!(fs::read_to_string(d.join("corral.2.log")).unwrap(), "old1", "old2 aged out exactly once");
        assert_eq!(tags(&d.join("corral.1.log")), vec!["g0a", "g0b"]);
        assert_eq!(tags(&p), vec![NOTICE, "during", "still-logging", "resumed"]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_leftover_staged_file_from_a_crash_is_finished_at_startup() {
        let d = dir("crash");
        let p = d.join("corral.log");
        fs::write(d.join("corral.rotating.log"), format!("{HEADER_LINE}\ncrashed-mid-rotation\n")).unwrap();
        let _l = CsvLog::open_with(&p, false, false, policy(10, 5)).unwrap();
        assert_eq!(names(&d), vec!["corral.1.log", "corral.log"]);
        assert!(text(&d.join("corral.1.log")).contains("crashed-mid-rotation"));
        fs::remove_dir_all(&d).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn readers_that_share_delete_do_not_block_rotation() {
        let d = dir("reader");
        let p = d.join("corral.log");
        let mut l = CsvLog::open_with(&p, false, false, policy(1, 5)).unwrap();
        fill(&mut l, 0);
        let _reader = crate::fsutil::open_shared_read(&p).unwrap(); // held open for the whole test
        l.event(&big("after", 10));
        assert_eq!(names(&d), vec!["corral.1.log", "corral.log"], "rotation succeeded despite the open reader");
        fs::remove_dir_all(&d).unwrap();
    }
}
