use process_corral::{apply, config, engine, log, os, paths, topology};
#[cfg(windows)]
use process_corral::{governor, instance, scm, service};

use std::time::{Duration, Instant};

const USAGE: &str = "\
Process Corral - automatic process priority governor

USAGE:
  corral run  [--config FILE] [--log FILE] [--quiet] [--system]   run in this console (Ctrl+C to stop)
  corral top  [--count N]                                          one-second CPU sample of running processes
  corral init [--config FILE] [--system]                           write a default config file
  corral burn [--threads N] [--secs N]                             consume CPU (for testing Restraint)
  corral topology [--json] [--simulate 9950x3d|9950x3d-ccd1|quad]  CPUs, cores, L3 groups (V-Cache), affinity masks

WINDOWS SERVICE (install/uninstall/start/stop need an elevated terminal):
  corral install [--config FILE] [--log FILE]   register auto-start service 'ProcessCorral' (LocalSystem)
  corral uninstall                              stop and remove the service
  corral start | stop                           start / stop the installed service
  corral service                                entry point used by the Service Control Manager

FOR UIs:
  corral apply-config <file> [--config DEST]    (elevated) validate <file>, then atomically install it as the
                                                service config (%ProgramData% location unless --config).
                                                Exit codes: 0 ok, 2 usage, 3 not elevated, 4 unreadable source,
                                                5 invalid config, 6 write failed. Never writes an invalid config.
  While running, a status snapshot is written about every 2s (see PATHS); `run` also takes --status FILE.

PATHS:
  console (default)  %APPDATA%\\ProcessCorral\\{config\\corral.ini, logs\\corral.log, status\\status.json}
  service / --system %ProgramData%\\ProcessCorral\\{config\\corral.ini, logs\\corral.log, status\\status.json}
";

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

#[cfg(windows)]
unsafe extern "system" fn on_ctrl(_: u32) -> i32 {
    governor::STOP.store(true, std::sync::atomic::Ordering::SeqCst);
    1
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("run") => run(&args),
        Some("top") => top(&args),
        Some("init") => init(&args),
        Some("burn") => burn(&args),
        Some("topology") => topology_cmd(&args),
        #[cfg(windows)]
        Some("install") => scm::install(&args),
        #[cfg(windows)]
        Some("uninstall") => scm::uninstall(),
        #[cfg(windows)]
        Some("start") => scm::start(&args),
        #[cfg(windows)]
        Some("stop") => scm::stop(),
        #[cfg(windows)]
        Some("service") => service::dispatch(),
        #[cfg(windows)]
        Some("apply-config") => apply::run(&args, scm::is_elevated()),
        _ => {
            print!("{USAGE}");
            0
        }
    };
    std::process::exit(code);
}

/// `process-corral topology`: what profiles see. `--simulate` previews a machine you do not have.
fn topology_cmd(args: &[String]) -> i32 {
    let t = match flag(args, "--simulate").as_deref() {
        Some("9950x3d") => Ok(topology::fake_9950x3d(true)),
        Some("9950x3d-ccd1") => Ok(topology::fake_9950x3d(false)),
        Some("quad") => Ok(topology::fake_quad()),
        Some("128") => Ok(topology::fake_two_groups()),
        Some(other) => Err(format!("unknown --simulate value '{other}' (use 9950x3d, 9950x3d-ccd1, quad or 128)")),
        None => topology::detect(),
    };
    match t {
        Ok(t) if has(args, "--json") => {
            println!("{}", serde_json::to_string_pretty(&topology::report(&t)).expect("serializable"));
            0
        }
        Ok(t) => {
            print!("{}", topology::render_text(&t));
            0
        }
        Err(e) => {
            eprintln!("topology: {e}");
            1
        }
    }
}

fn init(args: &[String]) -> i32 {
    let system = has(args, "--system");
    let p = paths::resolve(args, system);
    if p.config.exists() {
        eprintln!("{} already exists; not overwriting", p.config.display());
        return 1;
    }
    #[cfg(windows)]
    if system && flag(args, "--config").is_none() {
        let base = paths::system_base(std::env::var("ProgramData").ok().as_deref());
        if !base.exists() && !scm::is_elevated() {
            eprintln!("init --system: creating {} needs an elevated (Administrator) terminal.", base.display());
            return 1;
        }
        match paths::prepare_system_dirs(&base) {
            Ok(notes) => notes.iter().for_each(|n| eprintln!("note: {n}")),
            Err(e) => {
                eprintln!("{e}");
                return 1;
            }
        }
    }
    std::fs::create_dir_all(p.config.parent().unwrap()).ok();
    match std::fs::write(&p.config, config::DEFAULT_INI) {
        Ok(()) => {
            println!("wrote {}", p.config.display());
            0
        }
        Err(e) => {
            eprintln!("cannot write {}: {e}", p.config.display());
            1
        }
    }
}

#[cfg(windows)]
fn run(args: &[String]) -> i32 {
    use governor::{Governor, OpenError};
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    let p = paths::resolve(args, has(args, "--system"));

    // One governor at a time: console `run` and the service share this lock.
    let _instance = match instance::acquire(instance::NAME) {
        Ok(g) => g,
        Err(instance::Error::AlreadyRunning) => {
            eprintln!(
                "Process Corral is already running (another `run` console or the '{}' service), so this instance will not start.\n\
                 Only one instance may manage processes at a time. Stop the other one first:\n\
                 Ctrl+C in its console, or `process-corral stop` (elevated) for the service.",
                scm::NAME
            );
            return 1;
        }
        Err(instance::Error::Os(e)) => {
            eprintln!("cannot create the single-instance lock: {}", scm::err_text(e));
            return 1;
        }
    };

    let mut gov = match Governor::open(&p.config, &p.log, !has(args, "--quiet")) {
        Ok(g) => g.with_status(p.status.clone(), "console"),
        Err(OpenError::BadConfig(e)) => {
            // Nothing to fall back to at startup; refuse rather than run with unintended settings.
            eprintln!("invalid config {}: {e}", p.config.display());
            return 1;
        }
        Err(OpenError::Log(e)) => {
            eprintln!("{e}");
            return 1;
        }
    };
    if gov.config_found {
        println!("config: {}", p.config.display());
    } else {
        println!("config: {} not found, using defaults (run `corral init` to create it); it will be picked up if created", p.config.display());
    }
    unsafe { SetConsoleCtrlHandler(Some(on_ctrl), 1) };

    let pb = &gov.cfg.restraint;
    println!(
        "Restraint {}: restrain when system > {}% and process > {}% for {}ms; log: {}",
        if pb.enabled { "on" } else { "off" }, pb.total_cpu_before_restraint, pb.per_process_before_restraint,
        pb.time_over_quota_ms, p.log.display()
    );
    let mut os = os::win::WinOs::new();
    match gov.run(&mut os, &governor::STOP, std::process::id(), false) {
        Ok(n) => {
            println!("stopped; {n} restraint(s) this session");
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

#[cfg(windows)]
fn top(args: &[String]) -> i32 {
    let n: usize = flag(args, "--count").and_then(|v| v.parse().ok()).unwrap_or(15);
    let mut os = os::win::WinOs::new();
    let cfg = config::Config::parse(config::DEFAULT_INI);
    let mut eng = engine::Engine::new(std::process::id());
    let mut sink = log::CsvLog::open(std::path::Path::new(""), false, true).unwrap();
    eng.tick(&mut os, &cfg, &mut sink, 0);
    std::thread::sleep(Duration::from_secs(1));
    let mut cfg = cfg;
    cfg.restraint.enabled = false; // observe only
    eng.tick(&mut os, &cfg, &mut sink, 1000);
    println!("system CPU: {:.1}%   (cores: {})", eng.stats.sys_cpu, os::Os::cpu_count(&os));
    println!("{:>7}  {:>6}  {}", "PID", "CPU%", "NAME");
    for (pid, name, pct, _) in eng.snapshot().into_iter().take(n) {
        println!("{pid:>7}  {pct:>6.1}  {name}");
    }
    0
}

fn burn(args: &[String]) -> i32 {
    let threads: usize = flag(args, "--threads").and_then(|v| v.parse().ok()).unwrap_or(2);
    let secs: u64 = flag(args, "--secs").and_then(|v| v.parse().ok()).unwrap_or(20);
    println!("burning {threads} thread(s) for {secs}s (pid {})", std::process::id());
    let end = Instant::now() + Duration::from_secs(secs);
    let hs: Vec<_> = (0..threads)
        .map(|_| {
            std::thread::spawn(move || {
                let mut x = 1u64;
                while Instant::now() < end {
                    for _ in 0..100_000 {
                        x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
                    }
                }
                x
            })
        })
        .collect();
    for h in hs {
        h.join().ok();
    }
    0
}

#[cfg(not(windows))]
fn run(_: &[String]) -> i32 { eprintln!("Windows only"); 1 }
#[cfg(not(windows))]
fn top(_: &[String]) -> i32 { eprintln!("Windows only"); 1 }
