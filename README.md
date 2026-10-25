# Process Corral

Process Corral is a small Windows program that keeps one runaway program from making the whole PC feel stuck. When a process hogs the CPU while the machine is busy, Process Corral lowers its priority for a while and puts it back afterwards. You can also give chosen programs (for example a game) a fixed priority and a fixed set of CPU cores, such as the V-Cache cores of an AMD X3D processor.

It runs as a normal Windows service, has an optional window with a tray icon, and is configured with a plain text file.

> **Status: early (version 0.1.1).** It works on the author's machines, but it has not had wide testing. Read [Status and limitations](#status-and-limitations) before relying on it.

![The Status tab](docs/images/gui-status.png)

*Screenshots in this repository use made-up demo data, not a real machine.*

## What it does

- **Restraint**: when total CPU use is high and one process uses a lot of it for a while, that process is lowered to Below Normal (or Idle) priority, then restored once it calms down. Foreground programs, services and a built-in list of system processes are left alone.
- **Live config reload**: edit the config file and the running engine picks the change up in about 2 seconds. A file with a mistake is rejected and the previous settings stay in force.
- **Windows service**: installs as `ProcessCorral`, starts automatically with Windows, runs as LocalSystem.
- **Status file**: the engine writes `status.json` every 2 seconds so other tools can see what it is doing.
- **GUI with tray icon**: a window with Status, Settings, Profiles and Log tabs. The tray icon shows whether the engine is running. Closing the window hides it to the tray.
- **Per-application profiles**: for a program you name, set its priority class and its CPU affinity or CPU Sets, by core list, by mask, or by name (`vcache` / `frequency`).
- **Game mode**: while a game you marked is running, optionally lower everything else to Below Normal and switch the Windows power plan, then undo it afterwards.
- **CPU topology and V-Cache detection**: finds cores, hardware threads and L3 cache groups, and treats the L3 group with the largest cache as the "V-Cache" group.
- **Installer**: an Inno Setup installer that installs the service, creates shortcuts, and upgrades or removes it cleanly.

![The Profiles tab](docs/images/gui-profiles.png)

## Quick start

1. Get `ProcessCorral-Setup-<version>.exe`, or [build it](docs/INSTALL.md#building-the-installer-from-source). Check its SHA256 (see [INSTALL](docs/INSTALL.md#checking-the-download)).
2. Run it and approve the administrator prompt. Windows will show a "protected your PC" warning because the installer is not code-signed; see [INSTALL](docs/INSTALL.md#the-smartscreen-warning).
3. Finish the wizard. The service is installed and started for you.
4. Open **Process Corral** from the Start menu. The header should say **Engine running (service mode)**.
5. Optional: open the **Profiles** tab, press **Add**, pick your game with **Pick running process**, choose **V-Cache group** if you have an X3D processor, and press **Save**. See [PROFILES](docs/PROFILES.md).

If you use another process manager, do not run both programs at the same time. See [INSTALL](docs/INSTALL.md#another-process-manager-do-not-run-both).

## Documentation

| Document | What is in it |
| --- | --- |
| [docs/INSTALL.md](docs/INSTALL.md) | Requirements, installer, silent install, upgrade, uninstall, manual install, building the installer |
| [docs/USAGE.md](docs/USAGE.md) | The GUI tab by tab, the tray icon, every command line subcommand, file locations |
| [docs/PROFILES.md](docs/PROFILES.md) | Per-application profiles, V-Cache, CPU Sets versus Affinity, game mode, anti-cheat |
| [docs/CONFIG.md](docs/CONFIG.md) | Every `corral.ini` key with its default, type and meaning |
| [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md) | Service will not start, nothing is restrained, access denied, V-Cache placement, bug reports |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | How the pieces fit together, and the security design |

## Repository layout

```text
Cargo.toml, Cargo.lock   Rust package (library + two programs)
build.rs                 embeds the icon and version info into the exes (build time only)
src/                     the engine library and the engine program (src/main.rs)
src/gui/                 the optional GUI program (src/gui/main.rs), built with --features gui
assets/icon/             icon source (generate.js, SVG) and the generated .ico/.png files
installer/               Inno Setup script and the PowerShell script that builds the installer
docs/                    this documentation; docs/images/ holds the screenshots
```

Built files (`target/`, `dist/`, installers, checksums) are not stored in the repository.

## Building from source

You need Windows 10 or later, a recent stable Rust toolchain (the package uses Rust edition 2024; it was built and tested with Rust 1.98.1), and the Visual Studio Build Tools with the Windows SDK, because the build embeds the icon with the SDK's `rc.exe`.

```powershell
git clone <this repository>
cd Process-Corral-v2

# Engine only (no GUI libraries are compiled)
cargo build --release

# Engine + GUI
cargo build --release --features gui
```

The programs are written to `target\release\process-corral.exe` (engine and service) and `target\release\process-corral-gui.exe` (GUI). Keep them in the same folder: the GUI uses the engine exe to start, stop and install the service and to save settings.

Run the tests:

```powershell
cargo test --all-targets --features gui
```

A few extra tests use the real operating system and are skipped by default; they are listed in `src/real_os_tests.rs` and run with `cargo test --lib -- --ignored`. They launch only programs they start themselves.

To build the installer, see [docs/INSTALL.md](docs/INSTALL.md#building-the-installer-from-source).

## Status and limitations

- **Early software.** Version 0.1.1. Developed and tested mainly on Windows 11. Hardware-specific behaviour (V-Cache, several processor groups) is covered by simulated machines in the tests; real V-Cache placement has only been tried on one machine, an AMD Ryzen 9 9950X3D.
- **The installer is not code-signed.** Windows SmartScreen will warn about it.
- **Protected programs.** Some games and anti-cheat systems refuse changes to their priority or CPU affinity ("Access is denied"). Process Corral records the refusal and does not try to get around it. See [PROFILES](docs/PROFILES.md#anti-cheat-and-protected-games).
- **One instance at a time.** The console `run` command and the service cannot manage processes at the same time.
- **The GUI does not edit everything.** The Settings tab covers the Restraint settings and the Profiles tab covers profiles. The `[GameMode]` section, the per-application lists, `LogDisable` and `UpdateSpeedCore` are edited in the config file by hand.
- **`DisallowedProcesses` and `InstanceLimitedProcesses` end processes.** Matching programs are terminated. Use them carefully.
- **Log rotation is by size only.** `corral.log` rotates at 10 MB and keeps 5 old files by default (`MaxLogSizeMB`, `KeepLogFiles`). The GUI Log tab shows only the current file; open `corral.1.log` and the others in a text editor. See [USAGE](docs/USAGE.md#log-files-and-rotation).
- **Windows only.** Windows 10 or later, 64-bit, for the installer.

## Independent project

Process Corral is an independent project and contains no third-party process-manager code, branding or icons.

## Use at your own risk

Process Corral is provided "as is", under the MIT license, without warranty of any kind. It changes the priority and CPU placement of other programs, and its optional lists can end them, so check your settings before relying on it.

- **Games and anti-cheat.** Some games and anti-cheat systems may refuse, or object to, changes to a game's process. Process Corral records a refusal and does not try to get around it. Check a game's rules before using Process Corral with it online.
- **Unsigned installer.** The installer is not code-signed, so Windows shows an "unknown publisher" warning (and SmartScreen may warn too).

To report a security problem privately, see [SECURITY.md](SECURITY.md).

## License

Process Corral is released under the [MIT License](LICENSE). The open-source libraries it is built with, and their licenses, are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
