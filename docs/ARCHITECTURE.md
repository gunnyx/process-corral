# Architecture

A short tour of how the pieces fit. For what each setting does, see [CONFIG.md](CONFIG.md).

## The pieces

```text
                 +----------------------------+
 corral.ini ---> |  engine (process-corral)   | ---> corral.log   (CSV, rotated by size)
 (config)        |  runs as the Windows       | ---> status.json  (JSON, every ~2 s)
                 |  service, or in a console  |
                 +-------------^--------------+
                               | runs elevated ("runas") for:
                               | start, stop, install, uninstall, apply-config
                 +-------------+--------------+
 you ----------> |  GUI (process-corral-gui)  | <--- reads status.json and corral.log
                 |  window + tray icon        |
                 +----------------------------+
```

- **The library** (`process_corral`, `src/lib.rs`) holds all the logic. It depends on nothing graphical.
- **The engine program** (`process-corral.exe`, `src/main.rs`) is a thin command line over the library: `run`, `install`, `service` and so on. Built without the `gui` feature, **no GUI crate is compiled at all**; the engine's only run-time dependencies are `serde`, `serde_json` and `windows-sys`. (`winresource` is used at build time to embed the icon and version info; it is not linked into the programs.)
- **The GUI program** (`process-corral-gui.exe`, `src/gui/`) is built with `--features gui` and adds `eframe`/`egui` (window) and `tray-icon`. It never changes processes. It reads files the engine writes and asks the engine to do privileged things.
- **The installer** (`installer/`) is an Inno Setup script that copies the two exes and calls the engine's own `init`, `install`, `start`, `stop` and `uninstall` subcommands.

## Inside the engine

| Module | Job |
| --- | --- |
| `os.rs` | The `Os` trait: every Windows call the engine makes (list processes, read CPU time, get/set priority, affinity, CPU Sets, power plan, terminate) goes through it. `WinOs` is the real implementation using documented Win32 calls. The tests use a fake (`simos.rs`), so the logic is tested without touching real processes. |
| `config.rs`, `profile.rs` | Parsing and strict validation of `corral.ini` and `[Profile.*]` sections. |
| `engine.rs` | One `tick` per update interval: new processes (default priorities and affinities, disallowed programs), profiles, Restraint, instance limits. Remembers exactly what it changed so every release path restores from that record, not from the current config. |
| `profiles.rs` | Per-application profiles and game mode: apply, read back, retry refusals, restore. |
| `topology.rs` | CPUs, cores, SMT siblings, L3 cache groups, and turning a CPU selection into an affinity mask or CPU Set ids. The V-Cache group is the L3 group with the strictly largest cache. |
| `governor.rs`, `reload.rs` | The run loop: poll the config file every 2 seconds, hand the engine only fully validated configs, write the status file, restore everything on exit (even after a panic). |
| `service.rs`, `scm.rs`, `instance.rs` | The Windows service entry point, install/start/stop/uninstall, and the single-instance lock (a named mutex shared by the console `run` and the service). |
| `status.rs`, `statusview.rs` | The status file format, and how a reader decides whether the engine is running. |
| `apply.rs`, `fsutil.rs`, `paths.rs`, `elevate.rs` | Config installation, atomic file writes, locations and permissions, and the shared description of elevated commands and their exit codes. |
| `log.rs`, `logview.rs` | The CSV log writer and the tolerant reader the GUI uses. |
| `settings.rs`, `profile_edit.rs` | The GUI's editing model: change only the keys the user edited and keep everything else in the file. |

## The status file

`status.json` is the engine's public face. It is written about every 2 seconds, atomically (temp file then rename), and a final copy with `"state": "stopped"` is written on a clean exit. It contains the state, engine version, mode (`console` or `service`), process id, timestamps, the thresholds in effect, total CPU, the restrained processes, the top processes, the last events, the CPU topology, the profiles with what was applied or refused, game mode, and the list of processes Restraint found protected.

The `schema` field is `2`. Newer fields are optional when reading and were added without changing the number: a reader that ignores fields it does not know keeps working, and this GUI reads schema 1 and 2; a file with any other schema number is shown as unreadable rather than misread. The GUI treats the engine as not running when the state is `stopped`, the file is older than 10 seconds (or three update intervals, if longer), or the recorded process id is gone.

## `apply-config` and the elevation model

The GUI runs as the ordinary signed-in user and never holds administrator rights. Anything that needs them runs `process-corral.exe` as a separate, short-lived elevated process through the Windows "runas" mechanism (which shows the UAC prompt), waits for it (up to 120 seconds, and never kills it), and turns its exit code into a message. After a start, stop, install or uninstall it also checks what state the service really ended up in.

Saving settings works like this:

1. The GUI builds the new config text from your edits, **re-reading the file on disk just before** and refusing to save if it changed underneath you.
2. It writes the text to a uniquely named temporary file in your temp folder.
3. It runs `process-corral.exe apply-config <temp file>` elevated.
4. That process reads the file **once**, into memory, with a size limit of 1 MiB.
5. It validates **those bytes** with the same strict parser the engine uses.
6. Only if they are valid, it writes **those same bytes** to the service's config with an atomic replace (temp file in the same folder, flushed to disk, then renamed over the old file).
7. The GUI deletes the temporary file. The running engine sees the change within about 2 seconds.

Exit codes: `0` ok, `2` usage, `3` not elevated, `4` source unreadable or too large, `5` invalid config, `6` could not write.

## Security design

The service runs as **LocalSystem**, and its config can make it end programs (`DisallowedProcesses`, `InstanceLimitedProcesses`) and change priorities of almost any process. So **whoever can write the config controls a SYSTEM service**. The design follows from that:

- **Admin-only config folder.** `C:\ProgramData\ProcessCorral` is created with inheritance removed, **SYSTEM and Administrators with full control, Users read-and-execute only**, and Administrators as owner. The permissions are given with well-known security identifiers, so they work on any display language. They are set when the program creates the folder; an existing folder is left alone and reported (the installer additionally tightens one that ordinary users can write to). Without this, a standard user could create the folder first and control the service.
- **Validate in memory, then write.** `apply-config` reads its source exactly once and never opens the source path again, so the file cannot be swapped between the check and the write. An invalid config never reaches the destination.
- **Atomic replacement.** The service never sees a half-written config; a failed write leaves the old file untouched.
- **Strict at the door, safe at run time.** The engine only ever runs with a config that passed strict validation. If a reload fails, the previous settings stay. A misspelled key is ignored rather than guessed.
- **Least rights per call.** Each Windows call asks for the least access it needs (for example, reading a priority asks only for limited query access).
- **Protected software is respected.** If Windows refuses a change, it is recorded and left; Process Corral does not use other means to change a protected program. Profiles cannot set `realtime` priority, and critical Windows processes and Process Corral itself are never touched.
- **One instance.** A named mutex (`Global\ProcessCorralGovernor`) keeps a console engine and the service from managing processes at the same time.
- **The GUI cannot write the service config itself.** It can only ask the elevated helper to.

Things to know, so you are not surprised:

- `status.json` and the config are **readable by all users** on the machine. The status file lists process names and ids, and the log contains the computer and user name.
- The temporary settings file in your temp folder is briefly readable by you and by administrators. It is not trusted: only the bytes validated in memory are installed.
- The installer is not code-signed.

## Testing

Unit tests run the engine against the simulated `Os` and simulated CPU layouts (including a 9950X3D-like machine, with the V-Cache CCD first or second, and a machine with two processor groups). A few opt-in tests (`cargo test --lib -- --ignored`) use the real operating system, launching only programs they start themselves. Behaviour that depends on specific hardware and software (a real V-Cache driver, a real anti-cheat) is not covered by automated tests.
