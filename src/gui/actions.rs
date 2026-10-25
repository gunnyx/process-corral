//! Elevated actions (save settings, install/uninstall/start/stop the service), run one at a time
//! on a worker thread so the window never freezes while the UAC prompt is open.

use eframe::egui;
use crate::runas;
use process_corral::elevate::{build_params, interpret, Command, ExecOutcome, Message};
use process_corral::logview;
use process_corral::scm::{self, ServiceState};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default, Clone)]
pub struct ActionState {
    /// Description of the running action, e.g. "Saving settings".
    pub busy: Option<String>,
    pub last: Option<Message>,
    pub last_command: Option<Command>,
    /// Incremented when an action finishes, so the UI can react once.
    pub finished: u64,
}

pub struct Runner {
    helper: Option<PathBuf>,
    log_path: PathBuf,
    ctx: egui::Context,
    state: Mutex<ActionState>,
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh, uniquely named path in the user's temp folder.
fn unique_temp_path() -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!(
        "process-corral-settings-{}-{nanos}-{}.ini",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
    ))
}

/// Write `bytes` to a new unique temp file (never reusing or following an existing one).
pub fn write_temp(bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;
    let path = unique_temp_path();
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&path)?;
    f.write_all(bytes)?;
    f.flush()?;
    Ok(path)
}

impl Runner {
    pub fn new(helper: Option<PathBuf>, log_path: PathBuf, ctx: egui::Context) -> Arc<Self> {
        Arc::new(Self { helper, log_path, ctx, state: Mutex::new(ActionState::default()) })
    }

    pub fn helper_present(&self) -> bool {
        self.helper.as_ref().is_some_and(|p| p.is_file())
    }

    pub fn helper_path(&self) -> Option<&Path> {
        self.helper.as_deref()
    }

    pub fn snapshot(&self) -> ActionState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn dismiss(&self) {
        if let Ok(mut s) = self.state.lock() {
            s.last = None;
        }
    }

    /// Start/Stop/Install/Uninstall. Returns false if nothing was started.
    pub fn service(self: &Arc<Self>, cmd: Command) -> bool {
        debug_assert!(cmd != Command::ApplyConfig);
        self.launch(cmd, vec![cmd.subcommand().to_string()], None)
    }

    /// Write `bytes` to a temp file and have `process-corral apply-config` install it elevated.
    /// `dest` is passed as `--config` only when the GUI is not editing the default service config.
    pub fn save(self: &Arc<Self>, bytes: &[u8], dest: Option<&Path>) -> bool {
        if self.snapshot().busy.is_some() {
            return false;
        }
        let temp = match write_temp(bytes) {
            Ok(p) => p,
            Err(e) => {
                self.finish(Command::ApplyConfig, Message { ok: false, text: format!("Save failed: could not create the temporary file ({e}). Nothing was changed.") });
                return false;
            }
        };
        let mut args = vec!["apply-config".to_string(), temp.display().to_string()];
        if let Some(d) = dest {
            args.push("--config".into());
            args.push(d.display().to_string());
        }
        if self.launch(Command::ApplyConfig, args, Some(temp.clone())) {
            true
        } else {
            let _ = std::fs::remove_file(temp);
            false
        }
    }

    fn finish(&self, cmd: Command, msg: Message) {
        if let Ok(mut s) = self.state.lock() {
            s.busy = None;
            s.last = Some(msg);
            s.last_command = Some(cmd);
            s.finished += 1;
        }
        self.ctx.request_repaint();
    }

    fn launch(self: &Arc<Self>, cmd: Command, args: Vec<String>, temp: Option<PathBuf>) -> bool {
        let exe = {
            let Ok(mut s) = self.state.lock() else { return false };
            if s.busy.is_some() {
                return false;
            }
            match self.helper.clone().filter(|p| p.is_file()) {
                Some(p) => {
                    s.busy = Some(cmd.verb().to_string());
                    s.last = None;
                    p
                }
                None => {
                    s.last = Some(Message {
                        ok: false,
                        text: format!(
                            "process-corral.exe was not found{}. Keep process-corral.exe and process-corral-gui.exe in the same folder.",
                            self.helper.as_ref().map(|p| format!(" at {}", p.display())).unwrap_or_default()
                        ),
                    });
                    return false;
                }
            }
        };
        self.ctx.request_repaint();
        let me = Arc::clone(self);
        std::thread::spawn(move || {
            let outcome = runas::run_elevated(&exe, &build_params(&args), Duration::from_secs(120));
            if let Some(t) = temp {
                let _ = std::fs::remove_file(t);
            }
            let mut msg = interpret(cmd, &outcome);
            if matches!(outcome, ExecOutcome::Exited(0)) && matches!(cmd, Command::Start | Command::Stop | Command::Install | Command::Uninstall) {
                msg = me.verify_service_result(cmd, msg);
            }
            me.finish(cmd, msg);
        });
        true
    }

    /// The helper's exit code only says the command ran; check what the service actually did.
    fn verify_service_result(&self, cmd: Command, ok: Message) -> Message {
        let settled = |s: ServiceState| !matches!(s, ServiceState::StartPending | ServiceState::StopPending);
        let mut state = scm::service_state();
        for _ in 0..30 {
            if settled(state) {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
            state = scm::service_state();
        }
        match (cmd, state) {
            (Command::Start, ServiceState::Running) | (Command::Stop, ServiceState::Stopped) | (Command::Uninstall, ServiceState::NotInstalled) => {
                Message { ok: true, text: format!("{} The service is now {}.", ok.text, state.label()) }
            }
            (Command::Install, s) if s != ServiceState::NotInstalled => {
                Message { ok: true, text: format!("{} The service is installed ({}); use Start to run it.", ok.text, s.label()) }
            }
            (Command::Start, ServiceState::Stopped) => Message {
                ok: false,
                text: format!("The service started and then stopped again.{}", self.last_service_error()),
            },
            (_, s) => Message { ok: false, text: format!("{} But the service is {} (expected a different state); check the Log tab.", ok.text, s.label()) },
        }
    }

    /// The service logs why it refused to start; surface the newest such line.
    fn last_service_error(&self) -> String {
        logview::tail(&self.log_path, 200)
            .ok()
            .and_then(|t| t.rows.into_iter().rev().find(|r| logview::kind(r.action_id) == logview::Kind::Error))
            .map(|r| format!(" Last error in the log ({}): {} - {}", r.time, r.action, r.info))
            .unwrap_or_else(|| " See the Log tab.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These tests all look at (or create) files with this process's temp-file prefix, so they must not
    /// overlap: one test's short-lived temp files would otherwise show up in another test's count.
    static TEMP_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn leftover_temps() -> usize {
        let prefix = format!("process-corral-settings-{}-", std::process::id());
        std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().starts_with(&prefix))
            .count()
    }

    #[test]
    fn missing_helper_gives_a_clear_message_launches_nothing_and_leaves_no_temp_file() {
        let _serial = TEMP_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let before = leftover_temps();
        let r = Runner::new(Some(PathBuf::from(r"C:\definitely\missing\process-corral.exe")), PathBuf::from("none.log"), egui::Context::default());
        assert!(!r.helper_present());
        assert!(!r.service(Command::Start));
        let m = r.snapshot().last.expect("an explanation is shown");
        assert!(!m.ok && m.text.contains("not found") && m.text.contains("missing"), "{}", m.text);
        assert!(r.snapshot().busy.is_none(), "nothing is left 'in progress'");

        r.dismiss();
        assert!(!r.save(b"[OutOfControlProcessRestraint]\nOocOn=true\n", None));
        assert!(r.snapshot().last.is_some_and(|m| !m.ok && m.text.contains("not found")));
        assert_eq!(leftover_temps(), before, "the staged settings file is removed when nothing could run");

        let none = Runner::new(None, PathBuf::from("none.log"), egui::Context::default());
        assert!(!none.service(Command::Stop));
        assert!(none.snapshot().last.unwrap().text.contains("same folder"));
    }

    /// End to end through ShellExecute "runas" and the real `process-corral.exe apply-config`.
    /// Run with `cargo test --features gui -- --ignored elevated`. From a non-elevated shell this
    /// shows a real UAC prompt; from an elevated one Windows runs the helper without prompting.
    #[test]
    #[ignore = "runs process-corral.exe elevated"]
    fn elevated_apply_config_round_trip() {
        let _serial = TEMP_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let exe = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("process-corral.exe");
        assert!(exe.is_file(), "build the engine first: cargo build --features gui ({})", exe.display());
        let dir = std::env::temp_dir().join(format!("pc-gui-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("corral.ini");
        let original = "[OutOfControlProcessRestraint]\r\n; keep\r\nOocOn=true\r\n";
        std::fs::write(&dest, original).unwrap();

        let r = Runner::new(Some(exe), dir.join("none.log"), egui::Context::default());
        let wait = |r: &Arc<Runner>, n: u64| {
            let end = std::time::Instant::now() + Duration::from_secs(60);
            while r.snapshot().finished < n {
                assert!(std::time::Instant::now() < end, "helper did not finish");
                std::thread::sleep(Duration::from_millis(100));
            }
        };

        let good = "[OutOfControlProcessRestraint]\r\n; keep\r\nOocOn=false\r\n";
        assert!(r.save(good.as_bytes(), Some(&dest)));
        assert!(r.snapshot().busy.is_some() || r.snapshot().finished > 0);
        wait(&r, 1);
        let m = r.snapshot().last.unwrap();
        assert!(m.ok, "{}", m.text);
        assert!(m.text.contains("within about 2 seconds"));
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), good, "valid settings were applied exactly");

        assert!(r.save(b"[OutOfControlProcessRestraint]\nOocOn=banana\n", Some(&dest)));
        wait(&r, 2);
        let m = r.snapshot().last.unwrap();
        assert!(!m.ok && m.text.contains("exit 5"), "{}", m.text);
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), good, "an invalid config never reaches the destination");

        assert_eq!(leftover_temps(), 0, "temp files are deleted after the helper finishes");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn temp_files_are_unique_created_once_and_contain_the_bytes() {
        let _serial = TEMP_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        let a = write_temp(b"one").unwrap();
        let b = write_temp(b"two").unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read(&a).unwrap(), b"one");
        assert_eq!(std::fs::read(&b).unwrap(), b"two");
        assert!(a.starts_with(std::env::temp_dir()));
        assert!(a.file_name().unwrap().to_string_lossy().starts_with("process-corral-settings-"));
        std::fs::remove_file(a).unwrap();
        std::fs::remove_file(b).unwrap();
    }
}
