//! Read the tail of `corral.log` for UIs: bounded memory, CSV quoting, tolerant of a file that is
//! missing, rotated, truncated or briefly locked.

use crate::log::id;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// "MM-DD-YYYY HH:MM:SS" (UTC), as written.
    pub time: String,
    pub machine: String,
    pub user: String,
    pub pid: u32,
    pub name: String,
    pub action_id: u32,
    pub action: String,
    pub info: String,
    pub path: String,
}

impl Row {
    pub fn matches(&self, needle_lower: &str) -> bool {
        needle_lower.is_empty()
            || [&self.time, &self.user, &self.name, &self.action, &self.info, &self.path]
                .iter()
                .any(|f| f.to_lowercase().contains(needle_lower))
            || self.pid.to_string().contains(needle_lower)
            || self.action_id.to_string() == needle_lower
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Restrain,
    Release,
    Exempt,
    Error,
    Config,
    Lifecycle,
    Rule,
    /// Per-application profile applied / restored.
    Profile,
    Other,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Restrain => "RESTRAIN",
            Kind::Release => "RELEASE",
            Kind::Exempt => "SKIP",
            Kind::Error => "ERROR",
            Kind::Config => "CONFIG",
            Kind::Lifecycle => "ENGINE",
            Kind::Rule => "RULE",
            Kind::Profile => "PROFILE",
            Kind::Other => "INFO",
        }
    }
}

pub fn kind(action_id: u32) -> Kind {
    match action_id {
        id::PB_BEGIN | id::PB_LOWERED => Kind::Restrain,
        id::PB_END_TERMINATED | id::PB_END_RESTORED | id::PB_END_FOREGROUND | id::PB_END_TIMEOUT | id::PB_END_EXCLUDED => {
            Kind::Release
        }
        id::PB_EXEMPT | id::PROFILE_DIFFERS | id::PROFILE_RETRY => Kind::Exempt,
        id::PROFILE_APPLIED | id::PROFILE_RESTORED | id::PROFILE_OVERRIDES => Kind::Profile,
        id::PROFILE_REFUSED => Kind::Error,
        id::GAME_ENTER | id::GAME_LEAVE | id::GAME_INFO => Kind::Lifecycle,
        id::PB_ERROR | id::CONFIG_ERROR | id::SERVICE_ERROR | id::STATUS_ERROR | id::LOG_ROTATE_ERROR => Kind::Error,
        id::CONFIG_RELOADED => Kind::Config,
        id::GOVERNOR_STARTED | id::GOVERNOR_STOPPED => Kind::Lifecycle,
        id::DEFAULT_PRIORITY | id::DEFAULT_AFFINITY | id::DISALLOWED | id::INSTANCE_LIMIT => Kind::Rule,
        _ => Kind::Other,
    }
}

/// Split one CSV record. Handles quoted fields, doubled quotes and commas inside quotes.
/// Unterminated quotes are tolerated (the rest of the line becomes the last field).
pub fn parse_csv_line(line: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut in_q = false;
    let mut chars = line.trim_end_matches(['\r', '\n']).chars().peekable();
    while let Some(c) = chars.next() {
        match (c, in_q) {
            ('"', true) if chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            ('"', _) => in_q = !in_q,
            (',', false) => out.push(std::mem::take(&mut cur)),
            (c, _) => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// `None` for the header, blank lines and anything that isn't a log record.
pub fn parse_row(line: &str) -> Option<Row> {
    if line.trim().is_empty() {
        return None;
    }
    let f = parse_csv_line(line);
    if f.len() < 9 || f[0] == "UTC" {
        return None;
    }
    Some(Row {
        time: f[1].clone(),
        machine: f[2].clone(),
        user: f[3].clone(),
        pid: f[4].parse().ok()?,
        name: f[5].clone(),
        action_id: f[6].parse().ok()?,
        action: f[7].clone(),
        info: f[8].clone(),
        path: f.get(9).cloned().unwrap_or_default(),
    })
}

#[derive(Debug)]
pub struct Tail {
    /// Oldest first, at most `max_rows`.
    pub rows: Vec<Row>,
    pub file_len: u64,
    /// True if the file had more history than we were willing to read.
    pub older_rows_exist: bool,
}

const CHUNK: usize = 64 * 1024;
/// Hard ceiling on bytes read per refresh, whatever the line lengths are.
const MAX_BYTES: usize = 4 * 1024 * 1024;

pub fn tail(path: &Path, max_rows: usize) -> io::Result<Tail> {
    tail_with(path, max_rows, CHUNK, MAX_BYTES)
}

/// Read backwards in `chunk`-sized pieces until `max_rows` records (plus slack for a header and a
/// partial first line) are covered or `max_bytes` is reached. The file is never read whole.
pub fn tail_with(path: &Path, max_rows: usize, chunk: usize, max_bytes: usize) -> io::Result<Tail> {
    // Full sharing incl. delete: never blocks the engine appending to, or rotating, this file.
    let mut f = crate::fsutil::open_shared_read(path)?;
    let len = f.metadata()?.len();
    let mut pos = len;
    let mut pieces: Vec<Vec<u8>> = vec![];
    let (mut total, mut newlines) = (0usize, 0usize);
    while pos > 0 && newlines <= max_rows + 1 && total < max_bytes {
        let n = (chunk as u64).min(pos) as usize;
        pos -= n as u64;
        let mut buf = vec![0u8; n];
        f.seek(SeekFrom::Start(pos))?;
        f.read_exact(&mut buf)?; // a concurrent truncation surfaces as an error; the caller keeps old rows
        newlines += buf.iter().filter(|b| **b == b'\n').count();
        total += n;
        pieces.push(buf);
    }
    let bytes: Vec<u8> = pieces.into_iter().rev().flatten().collect();
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.split('\n');
    if pos > 0 {
        lines.next(); // we started mid-line
    }
    let mut rows: Vec<Row> = lines.filter_map(parse_row).collect();
    let older = pos > 0 || rows.len() > max_rows;
    if rows.len() > max_rows {
        rows.drain(..rows.len() - max_rows);
    }
    Ok(Tail { rows, file_len: len, older_rows_exist: older })
}

/// (modified, length, created) of the log file. A rotation replaces the file, so the creation time
/// changes even if a new file happens to match the old one's size and timestamp.
type Stamp = (Option<std::time::SystemTime>, u64, Option<std::time::SystemTime>);

fn stamp(path: &Path) -> io::Result<Stamp> {
    let m = std::fs::metadata(path)?;
    Ok((m.modified().ok(), m.len(), m.created().ok()))
}

/// What the Log tab shows: the tail of the *current* `corral.log`, re-read whenever the file
/// changes, is replaced by a rotation, or shrinks. Each refresh rebuilds the rows from the file, so
/// a rotation can neither duplicate nor drop lines of the file being shown (older lines live on in
/// `corral.1.log` and friends, which this view does not open).
pub struct Follower {
    path: std::path::PathBuf,
    max_rows: usize,
    stamp: Option<Stamp>,
    rows: std::sync::Arc<Vec<Row>>,
    note: Option<String>,
    /// The last read failed transiently (locked, or the file moved mid-read): try again next poll.
    retry: bool,
    /// Nothing has been read yet, so the first poll always reads (a missing file is then reported).
    fresh: bool,
}

impl Follower {
    pub fn new(path: &Path, max_rows: usize) -> Self {
        Self { path: path.to_path_buf(), max_rows, stamp: None, rows: std::sync::Arc::new(vec![]), note: None, retry: false, fresh: true }
    }

    pub fn rows(&self) -> std::sync::Arc<Vec<Row>> {
        std::sync::Arc::clone(&self.rows)
    }

    pub fn note(&self) -> Option<String> {
        self.note.clone()
    }

    /// Look at the file and reload if needed. Returns true if the rows were replaced.
    pub fn poll(&mut self) -> bool {
        let now = stamp(&self.path);
        let changed = now.as_ref().ok() != self.stamp.as_ref() || (now.is_err() && self.stamp.is_some());
        if !(changed || self.retry || self.fresh) {
            return false;
        }
        self.retry = false;
        self.fresh = false;
        match tail(&self.path, self.max_rows) {
            Ok(t) => {
                self.rows = std::sync::Arc::new(t.rows);
                self.note = t.older_rows_exist.then(|| format!("showing the last {} entries", self.max_rows));
                self.stamp = now.ok();
                true
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                self.rows = std::sync::Arc::new(vec![]);
                self.note = Some("no log file yet".into());
                self.stamp = None;
                true
            }
            // Locked, or rotated mid-read: keep the rows we have.
            Err(e) => {
                self.retry = true;
                self.note = Some(format!("log temporarily unreadable ({e}); showing the last good copy"));
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::Write;

    #[test]
    fn csv_quoting() {
        assert_eq!(parse_csv_line(r#""a","b,c","d ""q"" e",,"""#), vec!["a", "b,c", r#"d "q" e"#, "", ""]);
        assert_eq!(parse_csv_line("plain,line\r\n"), vec!["plain", "line"]);
        assert_eq!(parse_csv_line(r#""unterminated,rest"#), vec!["unterminated,rest"]);
        assert_eq!(parse_csv_line(""), vec![""]);
    }

    const HEADER: &str = "\"UTC\",\"UTCText\",\"Machine\",\"User\",\"PID\",\"Basename\",\"ActionId\",\"Action\",\"Info\",\"Path\"\n";
    fn rec(n: u32, action: &str) -> String {
        format!("\"1\",\"10-24-2026 22:46:{:02}\",\"M\",\"taco\",\"{n}\",\"p{n}.exe\",\"43\",\"{action}\",\"info, with \"\"quotes\"\"\",\"C:\\x\\p{n}.exe\"\n", n % 60)
    }

    #[test]
    fn parses_a_record_and_skips_header_and_junk() {
        let r = parse_row(&rec(7, "Restraint begun")).unwrap();
        assert_eq!((r.pid, r.name.as_str(), r.action_id), (7, "p7.exe", 43));
        assert_eq!(r.info, r#"info, with "quotes""#);
        assert_eq!(r.path, r"C:\x\p7.exe");
        assert_eq!(parse_row(HEADER), None);
        assert_eq!(parse_row("\n"), None);
        assert_eq!(parse_row("only,three,fields"), None);
        assert_eq!(parse_row(r#""1","t","m","u","notanumber","n","43","a","i","p""#), None);
    }

    #[test]
    fn kinds_cover_every_engine_action() {
        assert_eq!(kind(id::PB_BEGIN), Kind::Restrain);
        assert_eq!(kind(id::PB_END_RESTORED), Kind::Release);
        assert_eq!(kind(id::PB_EXEMPT), Kind::Exempt);
        assert_eq!(kind(id::CONFIG_ERROR), Kind::Error);
        assert_eq!(kind(id::CONFIG_RELOADED), Kind::Config);
        assert_eq!(kind(id::GOVERNOR_STOPPED), Kind::Lifecycle);
        assert_eq!(kind(id::DISALLOWED), Kind::Rule);
        assert_eq!(kind(id::PROFILE_APPLIED), Kind::Profile);
        assert_eq!(kind(id::PROFILE_REFUSED), Kind::Error);
        assert_eq!(kind(id::PROFILE_DIFFERS), Kind::Exempt);
        assert_eq!(kind(id::GAME_ENTER), Kind::Lifecycle);
        assert_eq!(kind(9999), Kind::Other);
    }

    fn write_log(name: &str, n: u32) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("pc-logview-{name}-{}.log", std::process::id()));
        let mut f = File::create(&p).unwrap();
        f.write_all(HEADER.as_bytes()).unwrap();
        for i in 1..=n {
            f.write_all(rec(i, "Restraint begun").as_bytes()).unwrap();
        }
        p
    }

    #[test]
    fn tail_returns_the_last_rows_in_order_across_many_chunks() {
        let p = write_log("many", 2000);
        let t = tail_with(&p, 500, 1024, 4 << 20).unwrap(); // tiny chunks force many seeks
        assert_eq!(t.rows.len(), 500);
        assert_eq!(t.rows.first().unwrap().pid, 1501);
        assert_eq!(t.rows.last().unwrap().pid, 2000);
        assert!(t.older_rows_exist);
        assert!(t.rows.iter().all(|r| r.name.starts_with('p')), "no torn first line");
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn short_file_returns_everything_without_the_header() {
        let p = write_log("short", 3);
        let t = tail(&p, 500).unwrap();
        assert_eq!(t.rows.iter().map(|r| r.pid).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert!(!t.older_rows_exist);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn memory_is_bounded_even_for_a_huge_file() {
        let p = write_log("huge", 40_000); // ~4.9 MB
        let len = std::fs::metadata(&p).unwrap().len() as usize;
        assert!(len > 4 << 20, "test file should exceed the byte cap: {len}");
        let t = tail(&p, 500).unwrap();
        assert_eq!(t.rows.len(), 500);
        assert_eq!(t.rows.last().unwrap().pid, 40_000);
        // With a byte cap far below the file size we still get a valid (shorter) tail.
        let small = tail_with(&p, 500, 4096, 20_000).unwrap();
        assert!(!small.rows.is_empty() && small.rows.len() < 500);
        assert_eq!(small.rows.last().unwrap().pid, 40_000);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn missing_empty_and_rotated_files() {
        let p = std::env::temp_dir().join(format!("pc-logview-gone-{}.log", std::process::id()));
        let _ = std::fs::remove_file(&p);
        assert_eq!(tail(&p, 10).unwrap_err().kind(), io::ErrorKind::NotFound);

        std::fs::write(&p, "").unwrap();
        assert!(tail(&p, 10).unwrap().rows.is_empty());

        // Rotation: the file is replaced by a smaller, newer one.
        let big = write_log("rot", 100);
        let before = tail(&big, 10).unwrap();
        assert_eq!(before.rows.last().unwrap().pid, 100);
        std::fs::write(&big, format!("{HEADER}{}", rec(1, "fresh"))).unwrap();
        let after = tail(&big, 10).unwrap();
        assert_eq!(after.rows.len(), 1);
        assert_eq!(after.rows[0].action, "fresh");
        assert!(after.file_len < before.file_len);
        std::fs::remove_file(&big).unwrap();
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn a_partially_written_last_line_does_not_break_parsing() {
        let p = write_log("partial", 5);
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"\"1\",\"10-24-2026 22:46:09\",\"M\",\"ta").unwrap(); // writer mid-line
        let t = tail(&p, 500).unwrap();
        assert_eq!(t.rows.len(), 5);
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn works_while_another_handle_appends() {
        let p = write_log("open", 10);
        let mut writer = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        writer.write_all(rec(11, "later").as_bytes()).unwrap();
        let t = tail(&p, 500).unwrap(); // writer handle still open
        assert_eq!(t.rows.last().unwrap().action, "later");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn filter_matches_any_visible_field_case_insensitively() {
        let r = parse_row(&rec(42, "Restraint begun")).unwrap();
        assert!(r.matches(""));
        assert!(r.matches("restraint"));
        assert!(r.matches("p42.exe"));
        assert!(r.matches("42"));
        assert!(r.matches("taco"));
        assert!(!r.matches("chrome"));
    }

    fn rotating_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("pc-follow-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn pids(f: &Follower) -> Vec<u32> {
        f.rows().iter().map(|r| r.pid).collect()
    }

    #[test]
    fn the_follower_reloads_from_the_new_file_after_a_rotation_without_duplicates_or_gaps() {
        use crate::config::LogRotation;
        use crate::log::{CsvLog, Event, Sink};
        let d = rotating_dir("rot");
        let p = d.join("corral.log");
        let ev = |pid: u32, pad: usize| Event { pid, name: "p.exe".into(), id: 43, action: "Restraint begun".into(), info: "i".repeat(pad), path: String::new() };
        let mut log = CsvLog::open_with(&p, false, false, LogRotation { max_mb: 1, keep: 3 }).unwrap();
        let mut f = Follower::new(&p, 500);
        assert!(f.poll(), "first look loads the (header-only) file");
        assert!(pids(&f).is_empty());
        assert!(!f.poll(), "nothing changed: no reload");

        log.event(&ev(1, 10));
        log.event(&ev(2, 600_000));
        log.event(&ev(3, 600_000)); // now over 1 MB
        assert!(f.poll());
        assert_eq!(pids(&f), vec![1, 2, 3]);

        log.event(&ev(4, 10)); // rotates first, then lands in the new corral.log
        log.event(&ev(5, 10));
        assert!(f.poll(), "the file was replaced by a smaller one");
        assert_eq!(pids(&f), vec![4, 5], "exactly the new file's lines: nothing repeated, nothing from before");
        assert!(f.note().is_none());
        assert!(!f.poll());

        let old = tail(&crate::log::rotated_path(&p, 1), 500).unwrap();
        assert_eq!(old.rows.iter().map(|r| r.pid).collect::<Vec<_>>(), vec![1, 2, 3], "and the lines before are in corral.1.log");

        // A new, smaller file with the same length and mtime would still be noticed through its creation time;
        // here simply check that growth after a rotation keeps flowing.
        log.event(&ev(6, 10));
        assert!(f.poll());
        assert_eq!(pids(&f), vec![4, 5, 6]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn the_follower_survives_a_missing_file_and_a_replaced_file() {
        let d = rotating_dir("replace");
        let p = d.join("corral.log");
        let mut f = Follower::new(&p, 500);
        assert!(f.poll());
        assert_eq!(f.note().as_deref(), Some("no log file yet"));
        std::fs::write(&p, format!("{HEADER}{}{}", rec(1, "a"), rec(2, "b"))).unwrap();
        assert!(f.poll());
        assert_eq!(pids(&f), vec![1, 2]);
        // Replaced by a shorter file (what a rotation looks like from outside).
        std::fs::rename(&p, d.join("corral.1.log")).unwrap();
        assert!(f.poll(), "the file vanished");
        assert!(f.rows().is_empty());
        std::fs::write(&p, format!("{HEADER}{}", rec(3, "c"))).unwrap();
        assert!(f.poll());
        assert_eq!(pids(&f), vec![3]);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
