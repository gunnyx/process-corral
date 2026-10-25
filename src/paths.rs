//! Where config and logs live.
//!
//! Console mode keeps a per-user location (`%APPDATA%\ProcessCorral`). The Windows
//! service runs as LocalSystem, which has no usable user profile, so it uses the machine-wide
//! `%ProgramData%\ProcessCorral`. `--system` makes `run` and `init` use that same location.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Paths {
    pub config: PathBuf,
    pub log: PathBuf,
    /// `status.json`, in a `status` folder beside `config` and `logs`.
    pub status: PathBuf,
}

/// With an explicit `--log`, keep the status folder beside that log's folder when it is named
/// `logs` (mirroring the default layout), otherwise put `status.json` in the log's folder.
fn status_beside(log: &Path) -> PathBuf {
    let dir = log.parent().unwrap_or_else(|| Path::new(""));
    match (dir.file_name().map(|n| n.to_ascii_lowercase()), dir.parent()) {
        (Some(n), Some(root)) if n == "logs" => root.join("status").join("status.json"),
        _ => dir.join("status.json"),
    }
}

pub fn system_base(program_data: Option<&str>) -> PathBuf {
    PathBuf::from(program_data.filter(|s| !s.is_empty()).unwrap_or(r"C:\ProgramData")).join("ProcessCorral")
}

pub fn user_base(appdata: Option<&str>) -> PathBuf {
    PathBuf::from(appdata.filter(|s| !s.is_empty()).unwrap_or(".")).join("ProcessCorral")
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// Explicit `--config` / `--log` win; otherwise the system or per-user default.
pub fn resolve_with(args: &[String], system: bool, env: &dyn Fn(&str) -> Option<String>) -> Paths {
    let base = if system {
        system_base(env("ProgramData").as_deref())
    } else {
        user_base(env("APPDATA").as_deref())
    };
    let explicit_log = arg(args, "--log").map(PathBuf::from);
    let status = arg(args, "--status").map(PathBuf::from).unwrap_or_else(|| match &explicit_log {
        Some(l) => status_beside(l),
        None => base.join("status").join("status.json"),
    });
    Paths {
        config: arg(args, "--config").map(PathBuf::from).unwrap_or_else(|| base.join("config").join("corral.ini")),
        log: explicit_log.unwrap_or_else(|| base.join("logs").join("corral.log")),
        status,
    }
}

pub fn resolve(args: &[String], system: bool) -> Paths {
    resolve_with(args, system, &|k| std::env::var(k).ok())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// `%ProgramData%\ProcessCorral` (the service).
    System,
    /// `%APPDATA%\ProcessCorral` (console mode).
    User,
}

fn has_files(p: &Paths) -> bool {
    p.config.is_file() || p.log.is_file() || p.status.is_file()
}

/// Paths for the GUI. `--system` / `--user` force a location; otherwise the service location is
/// used unless it holds none of our files while the per-user one does. `--config`, `--log` and
/// `--status` then override individual paths exactly like the engine's flags.
pub fn resolve_gui_with(
    args: &[String],
    env: &dyn Fn(&str) -> Option<String>,
    exists: &dyn Fn(&Paths) -> bool,
) -> (Paths, Scope) {
    let scope = if args.iter().any(|a| a == "--user") {
        Scope::User
    } else if args.iter().any(|a| a == "--system") {
        Scope::System
    } else if !exists(&resolve_with(&[], true, env)) && exists(&resolve_with(&[], false, env)) {
        Scope::User
    } else {
        Scope::System
    };
    (resolve_with(args, scope == Scope::System, env), scope)
}

pub fn resolve_gui(args: &[String]) -> (Paths, Scope) {
    resolve_gui_with(args, &|k| std::env::var(k).ok(), &has_files)
}

/// icacls invocations that make the system data directory writable only by SYSTEM and
/// Administrators (well-known SIDs, so this works on any display language). Without this a
/// standard user could pre-create the directory and then control the LocalSystem service's config.
pub fn icacls_commands(dir: &Path) -> Vec<Vec<String>> {
    let d = dir.display().to_string();
    vec![
        vec![
            d.clone(),
            "/inheritance:r".into(),
            "/grant:r".into(),
            "*S-1-5-18:(OI)(CI)F".into(),
            "*S-1-5-32-544:(OI)(CI)F".into(),
            "*S-1-5-32-545:(OI)(CI)RX".into(),
        ],
        vec![d, "/setowner".into(), "*S-1-5-32-544".into()],
    ]
}

/// Create the system data directories. Permissions are only tightened on a directory *we* create;
/// a pre-existing one is left alone and reported. Returns human-readable notes/warnings.
pub fn prepare_system_dirs(base: &Path) -> Result<Vec<String>, String> {
    let mut notes = vec![];
    let existed = base.exists();
    for sub in ["config", "logs", "status"] {
        std::fs::create_dir_all(base.join(sub)).map_err(|e| format!("cannot create {}: {e}", base.join(sub).display()))?;
    }
    if existed {
        notes.push(format!("{} already existed; its permissions were not changed. Make sure only Administrators can write to it.", base.display()));
    } else {
        for args in icacls_commands(base) {
            match std::process::Command::new("icacls").args(&args).output() {
                Ok(o) if o.status.success() => {}
                Ok(o) => notes.push(format!("could not tighten permissions on {}: {}", base.display(), String::from_utf8_lossy(&o.stdout).trim())),
                Err(e) => notes.push(format!("could not run icacls to tighten permissions on {}: {e}", base.display())),
            }
        }
    }
    Ok(notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn console_default_is_per_user_and_unchanged() {
        let p = resolve_with(&[], false, &env(&[("APPDATA", r"C:\Users\x\AppData\Roaming"), ("ProgramData", r"D:\PD")]));
        assert_eq!(p.config, PathBuf::from(r"C:\Users\x\AppData\Roaming\ProcessCorral\config\corral.ini"));
        assert_eq!(p.log, PathBuf::from(r"C:\Users\x\AppData\Roaming\ProcessCorral\logs\corral.log"));
    }

    #[test]
    fn service_default_is_programdata() {
        let p = resolve_with(&[], true, &env(&[("APPDATA", r"C:\Users\x"), ("ProgramData", r"D:\PD")]));
        assert_eq!(p.config, PathBuf::from(r"D:\PD\ProcessCorral\config\corral.ini"));
        assert_eq!(p.log, PathBuf::from(r"D:\PD\ProcessCorral\logs\corral.log"));
    }

    #[test]
    fn falls_back_when_environment_is_missing() {
        assert_eq!(system_base(None), PathBuf::from(r"C:\ProgramData\ProcessCorral"));
        assert_eq!(system_base(Some("")), PathBuf::from(r"C:\ProgramData\ProcessCorral"));
        assert_eq!(user_base(None), PathBuf::from(r".\ProcessCorral"));
    }

    #[test]
    fn explicit_flags_override_either_mode() {
        let a: Vec<String> = ["service", "--config", r"E:\c.ini", "--log", r"E:\l.log"].map(String::from).to_vec();
        for system in [false, true] {
            let p = resolve_with(&a, system, &env(&[]));
            assert_eq!(p, Paths { config: r"E:\c.ini".into(), log: r"E:\l.log".into(), status: r"E:\status.json".into() });
        }
        let only_cfg: Vec<String> = ["--config", r"E:\c.ini"].map(String::from).to_vec();
        let p = resolve_with(&only_cfg, true, &env(&[("ProgramData", r"D:\PD")]));
        assert_eq!(p.config, PathBuf::from(r"E:\c.ini"));
        assert_eq!(p.log, PathBuf::from(r"D:\PD\ProcessCorral\logs\corral.log"));
    }

    #[test]
    fn status_file_sits_beside_the_logs_folder() {
        let p = resolve_with(&[], true, &env(&[("ProgramData", r"D:\PD")]));
        assert_eq!(p.status, PathBuf::from(r"D:\PD\ProcessCorral\status\status.json"));
        let p = resolve_with(&[], false, &env(&[("APPDATA", r"C:\U\AppData\Roaming")]));
        assert_eq!(p.status, PathBuf::from(r"C:\U\AppData\Roaming\ProcessCorral\status\status.json"));
    }

    #[test]
    fn explicit_log_or_status_flags_move_the_status_file() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // log inside a folder named "logs": mirror the default layout next to it.
        let p = resolve_with(&a(&["--log", r"E:\data\Logs\x.log"]), true, &env(&[]));
        assert_eq!(p.status, PathBuf::from(r"E:\data\status\status.json"));
        // any other folder: status.json goes in the log's folder.
        let p = resolve_with(&a(&["--log", r"E:\misc\x.log"]), true, &env(&[]));
        assert_eq!(p.status, PathBuf::from(r"E:\misc\status.json"));
        // --status wins over everything.
        let p = resolve_with(&a(&["--log", r"E:\misc\x.log", "--status", r"F:\s.json"]), true, &env(&[]));
        assert_eq!(p.status, PathBuf::from(r"F:\s.json"));
        // a bare relative log name doesn't panic.
        assert_eq!(resolve_with(&a(&["--log", "x.log"]), true, &env(&[])).status, PathBuf::from("status.json"));
    }

    #[test]
    fn gui_prefers_service_paths_and_falls_back_to_user_paths() {
        let e = env(&[("APPDATA", r"C:\U\Roaming"), ("ProgramData", r"D:\PD")]);
        let sys_only = |p: &Paths| p.config.starts_with(r"D:\PD");
        let user_only = |p: &Paths| p.config.starts_with(r"C:\U");
        let both = |_: &Paths| true;
        let neither = |_: &Paths| false;
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

        let (p, s) = resolve_gui_with(&[], &e, &sys_only);
        assert_eq!((s, p.config), (Scope::System, PathBuf::from(r"D:\PD\ProcessCorral\config\corral.ini")));
        let (p, s) = resolve_gui_with(&[], &e, &user_only);
        assert_eq!((s, p.config), (Scope::User, PathBuf::from(r"C:\U\Roaming\ProcessCorral\config\corral.ini")));
        assert_eq!(resolve_gui_with(&[], &e, &both).1, Scope::System, "service wins when both exist");
        assert_eq!(resolve_gui_with(&[], &e, &neither).1, Scope::System, "fresh machine: service location");

        // Explicit choice beats detection.
        assert_eq!(resolve_gui_with(&a(&["--user"]), &e, &sys_only).1, Scope::User);
        assert_eq!(resolve_gui_with(&a(&["--system"]), &e, &user_only).1, Scope::System);
        // Per-path overrides apply on top of the chosen scope.
        let (p, s) = resolve_gui_with(&a(&["--config", r"E:\c.ini", "--status", r"E:\s.json"]), &e, &user_only);
        assert_eq!(s, Scope::User);
        assert_eq!((p.config, p.status), (PathBuf::from(r"E:\c.ini"), PathBuf::from(r"E:\s.json")));
        assert_eq!(p.log, PathBuf::from(r"C:\U\Roaming\ProcessCorral\logs\corral.log"));
    }

    #[test]
    fn acl_commands_only_trust_system_and_admins_for_write() {
        let cmds = icacls_commands(Path::new(r"D:\PD\ProcessCorral"));
        let all = cmds.concat().join(" ");
        assert!(all.contains("/inheritance:r"));
        assert!(all.contains("*S-1-5-18:(OI)(CI)F") && all.contains("*S-1-5-32-544:(OI)(CI)F"));
        assert!(all.contains("*S-1-5-32-545:(OI)(CI)RX"), "Users get read-only");
        assert!(!all.contains("S-1-5-32-545:(OI)(CI)F") && !all.contains("(M)") && !all.contains("(W)"));
        assert!(all.contains("/setowner *S-1-5-32-544"));
        assert!(cmds.iter().all(|c| c[0] == r"D:\PD\ProcessCorral"));
    }

    #[test]
    fn prepare_creates_subdirs_and_notes_preexisting_dir() {
        let base = std::env::temp_dir().join(format!("pc-paths-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap(); // pre-existing: no ACL changes attempted
        let notes = prepare_system_dirs(&base).unwrap();
        assert!(base.join("config").is_dir() && base.join("logs").is_dir() && base.join("status").is_dir());
        assert!(notes.iter().any(|n| n.contains("already existed")), "{notes:?}");
        std::fs::remove_dir_all(&base).unwrap();
    }
}
