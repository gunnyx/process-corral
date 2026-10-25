//! Live config reload: poll the file's (mtime, size), re-read on change, and only ever
//! hand back a fully validated `Config`. A bad or unreadable file never replaces the
//! running settings.

use crate::config::Config;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

pub type Stamp = (Option<SystemTime>, u64);

pub trait Source {
    fn stamp(&mut self) -> io::Result<Stamp>;
    fn read(&mut self) -> io::Result<Vec<u8>>;
    /// Wait briefly before the single retry (editor save in progress, antivirus lock...).
    fn pause(&mut self);
}

pub struct FileSource {
    path: PathBuf,
    delay: Duration,
}

impl FileSource {
    pub fn new(path: PathBuf) -> Self {
        Self { path, delay: Duration::from_millis(250) }
    }
}

impl Source for FileSource {
    fn stamp(&mut self) -> io::Result<Stamp> {
        let m = std::fs::metadata(&self.path)?;
        Ok((m.modified().ok(), m.len()))
    }
    fn read(&mut self) -> io::Result<Vec<u8>> {
        std::fs::read(&self.path)
    }
    fn pause(&mut self) {
        std::thread::sleep(self.delay);
    }
}

#[derive(Debug, PartialEq)]
pub enum Outcome {
    Unchanged,
    /// New settings plus a description of each setting that changed.
    Reloaded(Config, Vec<String>),
    /// The file changed but could not be used; the caller keeps its current settings.
    Failed(String),
}

enum LoadError {
    /// I/O failure: the file may just be locked, so don't mark this version as seen.
    Read(String),
    /// Content is bad: don't re-parse it until the file changes again.
    Invalid(String),
}

impl LoadError {
    fn msg(&self) -> &str {
        match self {
            LoadError::Read(m) | LoadError::Invalid(m) => m,
        }
    }
}

pub struct Reloader<S: Source> {
    src: S,
    last_stamp: Option<Stamp>,
    last_error: Option<String>,
}

fn load(src: &mut impl Source) -> Result<Config, LoadError> {
    let bytes = src.read().map_err(|e| LoadError::Read(format!("cannot read config: {e}")))?;
    Config::from_bytes(&bytes).map_err(LoadError::Invalid)
}

impl<S: Source> Reloader<S> {
    /// Call after the initial successful load so the first poll doesn't see it as a change.
    pub fn new(mut src: S) -> Self {
        let last_stamp = src.stamp().ok();
        Self { src, last_stamp, last_error: None }
    }

    pub fn poll(&mut self, current: &Config) -> Outcome {
        let st = self.src.stamp();
        match &st {
            Ok(s) if self.last_stamp.as_ref() == Some(s) => return Outcome::Unchanged,
            // Already reported that the file is missing; wait for it to reappear.
            Err(_) if self.last_stamp.is_none() && self.last_error.is_some() => return Outcome::Unchanged,
            _ => {}
        }

        let mut attempt = load(&mut self.src);
        if attempt.is_err() {
            // Half-written or briefly locked while an editor saves: retry once.
            self.src.pause();
            attempt = load(&mut self.src);
        }

        match attempt {
            Ok(new) => {
                self.last_stamp = st.ok();
                self.last_error = None;
                let changes = current.diff(&new);
                if changes.is_empty() { Outcome::Unchanged } else { Outcome::Reloaded(new, changes) }
            }
            Err(e) => {
                if matches!(e, LoadError::Invalid(_)) {
                    self.last_stamp = st.ok();
                } else if st.is_err() {
                    self.last_stamp = None;
                }
                // Report each distinct problem once, not every poll.
                if self.last_error.as_deref() == Some(e.msg()) {
                    Outcome::Unchanged
                } else {
                    self.last_error = Some(e.msg().to_string());
                    Outcome::Failed(e.msg().to_string())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_INI;
    use std::collections::VecDeque;

    /// Scripted source: `stamp` is a counter we bump to simulate edits; `reads` is consumed per read().
    #[derive(Default)]
    struct Fake {
        version: u64,
        missing: bool,
        reads: VecDeque<io::Result<Vec<u8>>>,
        read_calls: usize,
        pauses: usize,
    }
    impl Source for &mut Fake {
        fn stamp(&mut self) -> io::Result<Stamp> {
            if self.missing { Err(io::Error::new(io::ErrorKind::NotFound, "gone")) } else { Ok((None, self.version)) }
        }
        fn read(&mut self) -> io::Result<Vec<u8>> {
            self.read_calls += 1;
            self.reads.pop_front().unwrap_or_else(|| Err(io::Error::other("script exhausted")))
        }
        fn pause(&mut self) {
            self.pauses += 1;
        }
    }

    fn ok(s: &str) -> io::Result<Vec<u8>> {
        Ok(s.as_bytes().to_vec())
    }
    const NEW: &str = "[OutOfControlProcessRestraint]\nTotalProcessorUsageBeforeRestraint=40\n";

    #[test]
    fn unchanged_stamp_does_not_read() {
        let mut f = Fake::default();
        let cfg = Config::parse(DEFAULT_INI);
        let mut r = Reloader::new(&mut f);
        assert_eq!(r.poll(&cfg), Outcome::Unchanged);
        drop(r);
        assert_eq!(f.read_calls, 0);
    }

    #[test]
    fn valid_edit_reloads_and_reports_changes() {
        let mut f = Fake::default();
        let cfg = Config::parse(DEFAULT_INI);
        let mut r = Reloader::new(&mut f);
        r.src.version = 1;
        r.src.reads.push_back(ok(NEW));
        match r.poll(&cfg) {
            Outcome::Reloaded(new, changes) => {
                assert_eq!(new.restraint.total_cpu_before_restraint, 40.0);
                assert_eq!(changes, vec!["TotalProcessorUsageBeforeRestraint: 23.0 -> 40.0"]);
            }
            o => panic!("{o:?}"),
        }
        assert_eq!(r.poll(&cfg), Outcome::Unchanged, "same stamp is not reloaded twice");
    }

    #[test]
    fn invalid_edit_fails_once_then_recovers() {
        let mut f = Fake::default();
        let cfg = Config::parse(DEFAULT_INI);
        let mut r = Reloader::new(&mut f);
        r.src.version = 1;
        r.src.reads.push_back(ok("[OutOfControlProcessRestraint]\nOocOn=banana\n"));
        r.src.reads.push_back(ok("[OutOfControlProcessRestraint]\nOocOn=banana\n")); // retry sees the same
        let Outcome::Failed(msg) = r.poll(&cfg) else { panic!("expected failure") };
        assert!(msg.contains("line 2") && msg.contains("OocOn"), "{msg}");
        assert_eq!(r.src.pauses, 1);

        // Same bad file: silent. Different bad edit: reported again.
        assert_eq!(r.poll(&cfg), Outcome::Unchanged);
        r.src.version = 2;
        r.src.reads.push_back(ok("garbage"));
        r.src.reads.push_back(ok("garbage"));
        assert!(matches!(r.poll(&cfg), Outcome::Failed(m) if m.contains("key=value")));

        // User fixes it.
        r.src.version = 3;
        r.src.reads.push_back(ok(NEW));
        assert!(matches!(r.poll(&cfg), Outcome::Reloaded(..)));
    }

    #[test]
    fn half_written_file_is_retried_once() {
        let mut f = Fake::default();
        let cfg = Config::parse(DEFAULT_INI);
        let mut r = Reloader::new(&mut f);
        r.src.version = 1;
        r.src.reads.push_back(ok("")); // truncated by the editor
        r.src.reads.push_back(ok(NEW)); // complete after the pause
        assert!(matches!(r.poll(&cfg), Outcome::Reloaded(..)));
        assert_eq!(r.src.pauses, 1);
        assert_eq!(r.src.read_calls, 2);
    }

    #[test]
    fn locked_file_is_retried_once_then_reported() {
        let mut f = Fake::default();
        let cfg = Config::parse(DEFAULT_INI);
        let mut r = Reloader::new(&mut f);
        r.src.version = 1;
        r.src.reads.push_back(Err(io::Error::other("sharing violation")));
        r.src.reads.push_back(ok(NEW));
        assert!(matches!(r.poll(&cfg), Outcome::Reloaded(..)), "lock cleared on retry");

        r.src.version = 2;
        r.src.reads.push_back(Err(io::Error::other("sharing violation")));
        r.src.reads.push_back(Err(io::Error::other("sharing violation")));
        let calls_before = r.src.read_calls;
        assert!(matches!(r.poll(&cfg), Outcome::Failed(m) if m.contains("sharing violation")));
        assert_eq!(r.src.read_calls - calls_before, 2, "exactly one retry");

        // A read failure doesn't mark the version as seen: next poll tries again, and succeeds.
        r.src.reads.push_back(ok("[OutOfControlProcessRestraint]\nTotalProcessorUsageBeforeRestraint=50\n"));
        assert!(matches!(r.poll(&cfg), Outcome::Reloaded(..)));
    }

    #[test]
    fn whitespace_only_edit_is_not_a_reload() {
        let mut f = Fake::default();
        let cfg = Config::parse(DEFAULT_INI);
        let mut r = Reloader::new(&mut f);
        r.src.version = 1;
        r.src.reads.push_back(ok(&format!("{DEFAULT_INI}\n\n; a comment\n")));
        assert_eq!(r.poll(&cfg), Outcome::Unchanged);
    }

    #[test]
    fn deleted_file_keeps_settings_and_recovers_when_restored() {
        let mut f = Fake::default();
        let cfg = Config::parse(DEFAULT_INI);
        let mut r = Reloader::new(&mut f);
        r.src.missing = true;
        r.src.reads.push_back(Err(io::Error::new(io::ErrorKind::NotFound, "not found")));
        r.src.reads.push_back(Err(io::Error::new(io::ErrorKind::NotFound, "not found")));
        assert!(matches!(r.poll(&cfg), Outcome::Failed(_)));
        let calls = r.src.read_calls;
        assert_eq!(r.poll(&cfg), Outcome::Unchanged, "no repeated work or spam while missing");
        assert_eq!(r.src.read_calls, calls);

        r.src.missing = false;
        r.src.version = 5;
        r.src.reads.push_back(ok(NEW));
        assert!(matches!(r.poll(&cfg), Outcome::Reloaded(..)));
    }
}
