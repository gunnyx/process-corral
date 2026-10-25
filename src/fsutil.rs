//! Atomic file replacement shared by the status file and `apply-config`.
//!
//! Data goes to a uniquely named temp file in the destination's own directory (same volume, so the
//! rename can be atomic), then replaces the destination in one step. Readers see the old file or
//! the new one, never a partial write. A failure leaves the destination untouched and removes the
//! temp file.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn split(dest: &Path) -> io::Result<(PathBuf, String)> {
    let name = dest
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination has no file name"))?
        .to_string_lossy()
        .into_owned();
    let dir = dest.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new(".")).to_path_buf();
    Ok((dir, name))
}

/// FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE.
///
/// Rust's std already requests all three on Windows; spelling it out keeps log rotation safe even
/// if that default ever changes. A reader that omits FILE_SHARE_DELETE makes the engine's rename of
/// `corral.log` fail with a sharing violation, so every reader of the log opens through here.
#[cfg(windows)]
pub const SHARE_ALL: u32 = 0x1 | 0x2 | 0x4;

/// Open options with full sharing (`read` access unless the caller changes it).
pub fn shared_options() -> OpenOptions {
    let mut o = OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(SHARE_ALL);
    }
    o
}

/// Open for reading without blocking the writer's rename, delete or append.
pub fn open_shared_read(path: &Path) -> io::Result<fs::File> {
    shared_options().read(true).open(path)
}

/// Read a whole file through `open_shared_read`.
pub fn read_shared(path: &Path) -> io::Result<Vec<u8>> {
    use io::Read;
    let mut buf = vec![];
    open_shared_read(path)?.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Sharing/lock violations are transient: another process (an editor, a viewer, antivirus) has
/// the destination open without delete-sharing. Access denied is retried too, since that is what
/// replacing a briefly-open file reports.
fn is_transient(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(32) | Some(33))
}

fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if is_transient(&e) && attempt < 5 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(20 * attempt));
            }
            Err(e) => return Err(e),
        }
    }
}

/// `durable` also fsyncs the data before the rename (use it for configs; skip it for the
/// frequently rewritten status file).
pub fn atomic_write(dest: &Path, data: &[u8], durable: bool) -> io::Result<()> {
    let (dir, name) = split(dest)?;
    let tmp = dir.join(format!("{name}.{}.{}.tmp", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst)));
    let result = (|| {
        // create_new: never write through a pre-existing file or link at the temp name.
        let mut f = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(data)?;
        f.flush()?;
        if durable {
            f.sync_all()?;
        }
        drop(f);
        rename_with_retry(&tmp, dest)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Remove leftovers of `atomic_write(dest)` from a crashed earlier run. Only call while holding
/// the single-instance lock for a file that has exactly one writer.
pub fn remove_stale_temps(dest: &Path) {
    let Ok((dir, name)) = split(dest) else { return };
    let prefix = format!("{name}.");
    if let Ok(entries) = fs::read_dir(&dir) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.starts_with(&prefix) && n.ends_with(".tmp") {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pc-fs-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }
    fn names(d: &Path) -> Vec<String> {
        let mut v: Vec<_> = fs::read_dir(d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn creates_and_replaces_without_leaving_temp_files() {
        let d = dir("basic");
        let f = d.join("a.json");
        atomic_write(&f, b"one", true).unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"one");
        atomic_write(&f, b"two-longer", false).unwrap();
        atomic_write(&f, b"3", false).unwrap();
        assert_eq!(fs::read(&f).unwrap(), b"3", "shorter content fully replaces longer");
        assert_eq!(names(&d), vec!["a.json"]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn failure_leaves_existing_destination_intact_and_cleans_up() {
        let d = dir("fail");
        // Destination is a directory: the rename must fail.
        let blocked = d.join("blocked");
        fs::create_dir_all(blocked.join("child")).unwrap();
        assert!(atomic_write(&blocked, b"x", false).is_err());
        assert!(blocked.join("child").is_dir(), "destination untouched");
        assert_eq!(names(&d), vec!["blocked"], "temp file removed");

        // Missing parent directory.
        assert!(atomic_write(&d.join("nope").join("f"), b"x", false).is_err());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn readers_never_observe_a_partial_file() {
        let d = dir("race");
        let f = d.join("s.json");
        let (a, b) = (vec![b'A'; 200_000], vec![b'B'; 150_000]);
        atomic_write(&f, &a, false).unwrap();
        let stop = std::sync::atomic::AtomicBool::new(false);
        std::thread::scope(|s| {
            let reader = s.spawn(|| {
                let mut reads = 0;
                while !stop.load(Ordering::SeqCst) {
                    let got = fs::read(&f).expect("file always exists");
                    assert!(got == a || got == b, "torn read of {} bytes", got.len());
                    reads += 1;
                }
                reads
            });
            for i in 0..200 {
                atomic_write(&f, if i % 2 == 0 { &b } else { &a }, false).unwrap();
            }
            stop.store(true, Ordering::SeqCst);
            assert!(reader.join().unwrap() > 0);
        });
        assert_eq!(names(&d), vec!["s.json"]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn concurrent_writers_do_not_share_a_temp_file() {
        let d = dir("conc");
        let f = d.join("c.ini");
        std::thread::scope(|s| {
            for t in 0..4u8 {
                let f = &f;
                s.spawn(move || {
                    for _ in 0..50 {
                        atomic_write(f, &vec![b'0' + t; 50_000], false).unwrap();
                    }
                });
            }
        });
        let got = fs::read(&f).unwrap();
        assert_eq!(got.len(), 50_000);
        assert!(got.iter().all(|b| *b == got[0]), "one writer's content, never a blend");
        assert_eq!(names(&d), vec!["c.ini"]);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn stale_temps_from_a_crash_are_removed_but_other_files_are_not() {
        let d = dir("stale");
        let f = d.join("status.json");
        fs::write(&f, "live").unwrap();
        fs::write(d.join("status.json.123.0.tmp"), "junk").unwrap();
        fs::write(d.join("other.json.123.0.tmp"), "keep").unwrap();
        remove_stale_temps(&f);
        assert_eq!(names(&d), vec!["other.json.123.0.tmp", "status.json"]);
        fs::remove_dir_all(&d).unwrap();
    }
}
