//! `process-corral apply-config <path>`: install a config file into the real location.
//!
//! Meant to be run elevated by a UI (which stages the file somewhere it can write). The source is
//! read into memory exactly once; that buffer is validated with the strict parser and, only if it
//! is valid, those same bytes are written atomically to the destination. The source path is never
//! opened again, so it cannot be swapped between validation and use, and an invalid config can
//! never reach the destination.
//!
//! Exit codes: 0 ok, 2 usage, 3 not elevated, 4 source unreadable/too large, 5 invalid config,
//! 6 could not write the destination.

use crate::config::Config;
use crate::{fsutil, paths};
use std::io::Read;
use std::path::Path;

/// Real configs are a few KB; refuse anything absurd rather than read it into memory.
pub const MAX_BYTES: u64 = 1 << 20;

#[derive(Debug, PartialEq)]
pub enum ApplyError {
    Usage(String),
    NotElevated,
    Read(String),
    Invalid(String),
    Write(String),
}

impl ApplyError {
    pub fn exit_code(&self) -> i32 {
        match self {
            ApplyError::Usage(_) => 2,
            ApplyError::NotElevated => 3,
            ApplyError::Read(_) => 4,
            ApplyError::Invalid(_) => 5,
            ApplyError::Write(_) => 6,
        }
    }

    pub fn message(&self) -> String {
        match self {
            ApplyError::Usage(m) => m.clone(),
            ApplyError::NotElevated => {
                "apply-config must be run from an elevated (Administrator) terminal; nothing was read or written.".into()
            }
            ApplyError::Read(m) => format!("cannot read the source config: {m}. Nothing was written."),
            ApplyError::Invalid(m) => format!("the config is invalid: {m}. Nothing was written; the current config is unchanged."),
            ApplyError::Write(m) => format!("cannot write the destination config: {m}. The current config is unchanged."),
        }
    }
}

/// One bounded read of the whole source file.
pub fn read_source(path: &Path) -> Result<Vec<u8>, ApplyError> {
    let f = std::fs::File::open(path).map_err(|e| ApplyError::Read(format!("{}: {e}", path.display())))?;
    let mut buf = Vec::new();
    f.take(MAX_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| ApplyError::Read(format!("{}: {e}", path.display())))?;
    if buf.len() as u64 > MAX_BYTES {
        return Err(ApplyError::Read(format!("{} is larger than {} bytes", path.display(), MAX_BYTES)));
    }
    Ok(buf)
}

/// Read -> validate -> `prepare` (e.g. create the data directory) -> atomic write of the validated
/// bytes. `read` is called exactly once. Returns the number of bytes written.
pub fn apply_with(
    read: impl FnOnce() -> Result<Vec<u8>, ApplyError>,
    dest: &Path,
    prepare: impl FnOnce() -> Result<(), ApplyError>,
) -> Result<usize, ApplyError> {
    let bytes = read()?;
    Config::from_bytes(&bytes).map_err(ApplyError::Invalid)?;
    prepare()?;
    fsutil::atomic_write(dest, &bytes, true).map_err(|e| ApplyError::Write(format!("{}: {e}", dest.display())))?;
    Ok(bytes.len())
}

fn run_inner(args: &[String], elevated: bool) -> Result<String, ApplyError> {
    if !elevated {
        return Err(ApplyError::NotElevated);
    }
    let src = args
        .get(1)
        .filter(|a| !a.starts_with("--"))
        .ok_or_else(|| ApplyError::Usage("usage: process-corral apply-config <path-to-config.ini> [--config <destination>]".into()))?;
    let explicit_dest = args.iter().any(|a| a == "--config");
    let dest = paths::resolve(args, true).config;

    let n = apply_with(
        || read_source(Path::new(src)),
        &dest,
        || {
            // Default destination: make sure the (hardened) system data directory exists.
            if explicit_dest {
                return Ok(());
            }
            let base = paths::system_base(std::env::var("ProgramData").ok().as_deref());
            let notes = paths::prepare_system_dirs(&base).map_err(ApplyError::Write)?;
            notes.iter().for_each(|n| eprintln!("note: {n}"));
            Ok(())
        },
    )?;
    Ok(format!(
        "Applied {src} -> {} ({n} bytes). A running console or service instance reloads it within about 2 seconds.",
        dest.display()
    ))
}

/// Entry point. `elevated` is injected so the refusal path is testable.
pub fn run(args: &[String], elevated: bool) -> i32 {
    match run_inner(args, elevated) {
        Ok(msg) => {
            println!("{msg}");
            0
        }
        Err(e) => {
            eprintln!("apply-config: {}", e.message());
            e.exit_code()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_INI;
    use std::cell::Cell;
    use std::path::PathBuf;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pc-apply-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn files(d: &Path) -> Vec<String> {
        let mut v: Vec<_> = std::fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }
    fn args(src: &Path, dest: &Path) -> Vec<String> {
        ["apply-config", src.to_str().unwrap(), "--config", dest.to_str().unwrap()].map(String::from).to_vec()
    }
    const GOOD: &str = "[OutOfControlProcessRestraint]\nTotalProcessorUsageBeforeRestraint=41\n";

    #[test]
    fn valid_config_is_written_exactly_and_is_loadable() {
        let d = dir("ok");
        let (src, dest) = (d.join("staged.ini"), d.join("corral.ini"));
        std::fs::write(&src, GOOD).unwrap();
        std::fs::write(&dest, DEFAULT_INI).unwrap();
        assert_eq!(run(&args(&src, &dest), true), 0);
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), GOOD);
        assert_eq!(Config::load_strict(&dest).unwrap().restraint.total_cpu_before_restraint, 41.0);
        assert_eq!(files(&d), vec!["corral.ini", "staged.ini"], "no temp files left");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn utf16_config_is_preserved_byte_for_byte() {
        let d = dir("u16");
        let (src, dest) = (d.join("s.ini"), d.join("c.ini"));
        let mut b = vec![0xff, 0xfe];
        for u in GOOD.encode_utf16() {
            b.extend(u.to_le_bytes());
        }
        std::fs::write(&src, &b).unwrap();
        assert_eq!(run(&args(&src, &dest), true), 0);
        assert_eq!(std::fs::read(&dest).unwrap(), b);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn invalid_configs_are_rejected_and_the_destination_is_untouched() {
        let d = dir("bad");
        let dest = d.join("corral.ini");
        std::fs::write(&dest, DEFAULT_INI).unwrap();
        let cases: &[(&str, &[u8])] = &[
            ("bad value", b"[OutOfControlProcessRestraint]\nOocOn=banana\n"),
            ("garbage", b"this is not an ini file"),
            ("out of range", b"[OutOfControlProcessRestraint]\nPerProcessUsageBeforeRestraint=500\n"),
            ("empty", b""),
            ("not utf-8", &[b'[', b'a', b']', 0xff, 0xfe, 0xfd]),
        ];
        for (name, content) in cases {
            let src = d.join("staged.ini");
            std::fs::write(&src, content).unwrap();
            assert_eq!(run(&args(&src, &dest), true), 5, "{name}");
            assert_eq!(std::fs::read_to_string(&dest).unwrap(), DEFAULT_INI, "{name}: destination unchanged");
            assert_eq!(files(&d), vec!["corral.ini", "staged.ini"], "{name}: nothing else created");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn invalid_config_does_not_create_a_missing_destination() {
        let d = dir("nodest");
        let (src, dest) = (d.join("s.ini"), d.join("corral.ini"));
        std::fs::write(&src, "OocOn=true").unwrap(); // setting before any [section]
        assert_eq!(run(&args(&src, &dest), true), 5);
        assert!(!dest.exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn refuses_without_elevation_before_reading_anything() {
        let d = dir("elev");
        let dest = d.join("corral.ini");
        std::fs::write(&dest, DEFAULT_INI).unwrap();
        // The source doesn't even exist: exit 3 (not 4) proves elevation is checked first.
        assert_eq!(run(&args(&d.join("missing.ini"), &dest), false), 3);
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), DEFAULT_INI);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn usage_and_unreadable_source_have_distinct_exit_codes() {
        let d = dir("codes");
        let dest = d.join("c.ini");
        assert_eq!(run(&["apply-config".to_string()], true), 2);
        assert_eq!(run(&["apply-config".to_string(), "--config".to_string(), "x".to_string()], true), 2);
        assert_eq!(run(&args(&d.join("missing.ini"), &dest), true), 4);
        assert!(!dest.exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn oversized_source_is_refused_without_being_loaded() {
        let d = dir("big");
        let (src, dest) = (d.join("s.ini"), d.join("c.ini"));
        std::fs::write(&src, vec![b'#'; MAX_BYTES as usize + 10]).unwrap();
        assert_eq!(run(&args(&src, &dest), true), 4);
        assert!(!dest.exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn source_is_read_once_and_the_validated_bytes_are_what_get_written() {
        let d = dir("once");
        let (src, dest) = (d.join("s.ini"), d.join("c.ini"));
        std::fs::write(&src, GOOD).unwrap();
        let reads = Cell::new(0);
        let n = apply_with(
            || {
                reads.set(reads.get() + 1);
                let bytes = read_source(&src)?;
                // An attacker/editor swaps the source right after we read it.
                std::fs::write(&src, "[OutOfControlProcessRestraint]\nOocOn=banana\n").unwrap();
                Ok(bytes)
            },
            &dest,
            || Ok(()),
        )
        .unwrap();
        assert_eq!(reads.get(), 1);
        assert_eq!(n, GOOD.len());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), GOOD, "wrote what was validated, not the swapped file");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn prepare_failure_and_write_failure_leave_everything_unchanged() {
        let d = dir("fail");
        let dest = d.join("c.ini");
        std::fs::write(&dest, DEFAULT_INI).unwrap();
        let e = apply_with(|| Ok(GOOD.as_bytes().to_vec()), &dest, || Err(ApplyError::Write("no dir".into()))).unwrap_err();
        assert_eq!(e.exit_code(), 6);
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), DEFAULT_INI);

        // Destination is a directory: the final rename fails; temp file is cleaned up.
        let blocked = d.join("blocked");
        std::fs::create_dir_all(&blocked).unwrap();
        let e = apply_with(|| Ok(GOOD.as_bytes().to_vec()), &blocked, || Ok(())).unwrap_err();
        assert!(matches!(e, ApplyError::Write(_)));
        assert_eq!(files(&d), vec!["blocked", "c.ini"]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_running_governor_style_reader_sees_only_complete_configs() {
        use crate::reload::{FileSource, Outcome, Reloader};
        let d = dir("live");
        let (src, dest) = (d.join("s.ini"), d.join("c.ini"));
        std::fs::write(&dest, DEFAULT_INI).unwrap();
        let current = Config::load_strict(&dest).unwrap();
        let mut reloader = Reloader::new(FileSource::new(dest.clone()));
        std::fs::write(&src, GOOD).unwrap();
        assert_eq!(run(&args(&src, &dest), true), 0);
        match reloader.poll(&current) {
            Outcome::Reloaded(new, changes) => {
                assert_eq!(new.restraint.total_cpu_before_restraint, 41.0);
                assert_eq!(changes, vec!["TotalProcessorUsageBeforeRestraint: 23.0 -> 41.0"]);
            }
            o => panic!("live reload should pick up apply-config: {o:?}"),
        }
        std::fs::remove_dir_all(&d).unwrap();
    }
}
