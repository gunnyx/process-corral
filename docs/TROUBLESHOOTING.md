# Troubleshooting

Most problems are explained by one of three places: the **Log** tab (or `C:\ProgramData\ProcessCorral\logs\corral.log`), the **Status** tab, and the header of the GUI. Look there first.

## The service will not start

```powershell
Get-Service ProcessCorral
sc.exe qc ProcessCorral                      # BINARY_PATH_NAME is the exe Windows will run
Get-Content "$env:ProgramData\ProcessCorral\logs\corral.log" -Tail 20
```

A service that cannot start has no window to complain in, so it writes the reason to the log. Look for lines like:

| Log line | Cause and fix |
| --- | --- |
| `Service refused to start: invalid config` | The config file has a mistake; the line number is in the message. Fix the file (see [Config rejected](#config-rejected)). The engine will not start with a config it cannot trust. |
| `Service refused to start` with "another Process Corral instance (a console `run`?) is already running" | A console `process-corral run` is running. Close it (Ctrl+C), then start the service. See [Already running](#already-running-instance-lock). |
| `Service stopped after an internal error` | A fault inside the engine. Everything it lowered is restored first. Please [report it](#reporting-a-bug). |

If the log has nothing new, the problem is before the engine runs. The usual cause is that the exe the service points at is gone or moved: compare `BINARY_PATH_NAME` with where `process-corral.exe` really is. If it moved, uninstall and install again from the new place (`process-corral stop`, `uninstall`, `install`, `start`, from an elevated PowerShell). Windows also records service problems in the System event log:

```powershell
Get-WinEvent -FilterHashtable @{LogName='System'; ProviderName='Service Control Manager'} -MaxEvents 20 |
    Format-Table TimeCreated, Id, Message -Wrap
```

If the log folder cannot be opened for writing, the service stops without a log line; check the folder permissions with `icacls "$env:ProgramData\ProcessCorral"` (see [INSTALL](INSTALL.md#folder-permissions)).

## The GUI says "Engine not running"

The GUI decides this from `status.json`. The reason is shown next to the red header:

| Reason | What it means | What to do |
| --- | --- | --- |
| no status file yet | Nothing has written a status file in the folder the GUI is reading. | Start the service (header **Start**) or run `process-corral run`. Check the footer: it shows which files the GUI is reading. If you run the engine in a console, the GUI may be looking at the service's folder; start the GUI with `--user` (or `--system`). |
| the engine shut down cleanly | The service or console engine was stopped. | Press **Start**. |
| no update for N s | The file says "running" but has not been updated for more than 10 seconds (or three update intervals). The engine crashed, was killed, or is stuck. | Check `Get-Service ProcessCorral` and the log's last lines. |
| process N is gone | The process id in the file no longer exists. | Same as above; start the engine. |
| status file unreadable | The file is partly written or locked. The GUI keeps showing the last good reading. | Usually clears by itself in a few seconds. |

If the header says the engine is running but **the service** shows "stopped", the engine is a console `run`, not the service.

## Config rejected

When the config has a mistake, Process Corral refuses it as a whole:

- **At startup** the engine does not start (the console prints `invalid config <path>: <reason>`; the service logs "Service refused to start: invalid config").
- **On a live reload** the running engine logs **"Config reload failed; keeping previous settings"** with the reason and keeps working with the old settings. Nothing changes until the file is valid.
- **Saving from the GUI** is blocked, with the problem shown next to the field. If the file is invalid, `apply-config` exits with code 5 and writes nothing.

The message starts with the line number. These are the messages you can get and what they mean:

| Message | Meaning |
| --- | --- |
| `line N: expected key=value, got '...'` | A line that is not blank, a comment, a `[section]` or `key=value`. |
| `line N: malformed section header '...'` | A line starting with `[` that does not end in `]`. |
| `line N: 'Key' appears before any [section]` | A setting above the first `[Section]` line. |
| `line N: Key: 'x' is not true/false` | Booleans are `true`, `false`, `1` or `0`. Often caused by a trailing comment on the line. |
| `line N: Key: 'x' is not a number` / `x is outside the allowed range lo..hi` / `'x' is not a whole number` | Percentages are 0-100, times 0-86,400,000 ms, `UpdateSpeedCore` 100-3,600,000, `MaxLogSizeMB` 1-1024, `KeepLogFiles` 0-50. |
| `line N: Key: 'x' is not a non-zero hex affinity mask` | Hex digits only (optional `0x`), and not zero. |
| `line N: Key: unknown priority class 'x'` | In `DefaultPriorities`. Use idle, belownormal, normal, abovenormal, high. |
| `line N: Key: 'x' is not name:value` / `'x' has no process name` / `'x' is not a count` | A malformed `pattern:value` list entry. |
| `line N: PowerPlan: 'x' is not a GUID like {...}` | See `powercfg /list`. |
| `line N: [Profile.X] OldKey is no longer supported; rename it to NewKey` | The profile uses a key name that was removed. Rename it as the message says (the profile exclusion key is `ExcludeFromRestraint`). |
| `file is empty`, `file is not valid UTF-8`, `invalid UTF-16 text` | The file was saved in an unsupported encoding or is cut off. Save as UTF-8. |
| `line N: [Profile.X] ...` | A problem in a profile: missing `Match=`, an unknown priority, a mask without `0x`, `Affinity` together with `CpuSets`, `CpuGroup` together with either, a `CpuMode` that contradicts the key, `realtime` priority, a duplicate profile name. The text after the profile name says which key. See [PROFILES](PROFILES.md#the-file-format). |

Remember that **misspelled key names are silently ignored** (unknown keys are not errors). If a setting does nothing, compare its spelling with [CONFIG](CONFIG.md).

## Nothing is being restrained

Work through these in order.

1. **Is it running, and is Restraint on?** Status tab: the header must say the engine is running and **Thresholds in effect > Restraint** must be `on` (`OocOn=true`).
2. **Is another process manager running?** Two managers fight over the same programs: another process manager may be restraining the process first (or undoing what Process Corral does). Check the running services, the startup programs and the tray icons, as described in [INSTALL](INSTALL.md#how-to-check-whether-another-process-manager-is-installed), for example:

   ```powershell
   Get-CimInstance Win32_Service |
       Where-Object { $_.State -eq 'Running' -and $_.PathName -notmatch '\\Windows\\' } |
       Format-Table Name, DisplayName, PathName -AutoSize
   ```

   If you find one, run only one of the two. See [INSTALL](INSTALL.md#another-process-manager-do-not-run-both).
3. **Are the thresholds reachable?** The percentages are shares of the **whole machine**. On a CPU with 32 logical processors, a program fully using one core is at about 3% (100 / 32), far below the default `PerProcessUsageBeforeRestraint=15`, and the whole system must also be above `TotalProcessorUsageBeforeRestraint=23`. Restraint is designed for programs that take a large share of the machine. To test, lower both values in a scratch config and make load on purpose:

   ```powershell
   # console 1: try a test config (stop the service first; only one engine may run)
   process-corral run --config .\test.ini --log .\test.log
   # console 2: burn CPU for 30 seconds
   process-corral burn --threads 16 --secs 30
   ```
4. **`TameOnlyNormal=true`** (the default) only lowers programs that are at Normal priority. A program at Above Normal or High is skipped.
5. **Exemptions.** The log records why a busy program was skipped, once per episode: look for **"Restraint skipped process"**. The reason is one of:
   - `is the foreground window (ExcludeForegroundProcesses2)`;
   - `child of foreground app NAME (pid N) (ExcludeChildrenOfForeground)`;
   - `service running in session 0 (ExcludeServices)`;
   - `system process on the built-in exclusion list`;
   - `matches OocExclusions pattern '...'`;
   - `profile 'NAME' is set to never restrain this program` or `game-mode process (profile 'NAME')`;
   - `priority class is X, not normal (TameOnlyNormal)`;
   - `priority class could not be read (access denied)`.
6. **Windows refused.** If a program is listed under **Protected, not managed** on the Status tab, Windows refused access and Process Corral is leaving it alone. See below.

## "Access is denied" on a profile or on Restraint

Some programs, mainly games with anti-cheat, deny other programs the right to change their priority or CPU affinity.

- A **profile** is tried 5 times over about 30 seconds and then shown as refused, on the Status tab and in the Profiles tab, and in the log ("Profile change refused" with "Win32 error 5").
- For **Restraint**, the program is shown under **Protected, not managed** and logged once.

Process Corral does **not** try to get around this, on purpose. Running the engine as the service (administrator rights) is the most it can do; if Windows still says no, the program is protected. See [PROFILES: anti-cheat](PROFILES.md#anti-cheat-and-protected-games) and the next section for V-Cache.

## A game isn't being placed on the V-Cache CCD

On AMD X3D processors with two CCDs, only one CCD has the extra cache. You want a game to run on that one. Normally Windows and AMD's driver decide this, and **that is the preferred route**, especially for games with anti-cheat, because it does not need anyone to change the game's process. **Games with anti-cheat may refuse Process Corral's affinity and priority changes**, so a Process Corral profile may do nothing for them.

> **These steps worked on one machine** (a Ryzen 9 9950X3D) when its games were not landing on the V-Cache CCD. They are **not guaranteed fixes**, and they have **not** been checked on other hardware or Windows versions. Service names, device names and behaviour can change with AMD driver and Windows updates. Check **AMD's current documentation** for your processor and chipset driver.

Work through these in order. Use an **elevated** PowerShell where it says so.

1. **Is the AMD 3D V-Cache driver device present?**

   ```powershell
   Get-PnpDevice | Where-Object { $_.FriendlyName -like '*V-Cache*' } |
       Format-Table Status, Class, FriendlyName, InstanceId -AutoSize
   ```

   You should see a device with `Status` of `OK`. If nothing is listed, the device name may differ on your system; look in Device Manager (System devices) as well. If the device is really missing, install the current AMD chipset driver from AMD, which is what provides it.

2. **Is the `amd3dvcacheSvc` service running?**

   ```powershell
   sc.exe qc amd3dvcacheSvc
   Get-Service amd3dvcacheSvc | Format-List Name, Status, StartType
   ```

   `Status` should be `Running`.

3. **If it was stopped**, start it from an elevated PowerShell and make it start in delayed-automatic mode:

   ```powershell
   Start-Service amd3dvcacheSvc
   sc.exe config amd3dvcacheSvc start= delayed-auto
   ```

   (The space after `start=` is required.)

4. **Check the Windows Game Bar and Game Mode.** Open **Settings > Gaming**, make sure **Xbox Game Bar** (the "Game Bar" setting) is on and **Game Mode** is on. Windows must recognise your game as a game for it to be treated as one. Check AMD's documentation for how current driver versions pick games.

5. **Test it with no Process Corral profile in the way.** Disable or remove any Process Corral profile for that game (set `Enabled=false` in its `[Profile.<name>]` and let the config reload), start the game, and in **Task Manager > Performance > CPU** right-click the graph and choose **Change graph to > Logical processors**. Watch which logical processors carry the load. Find out which numbers are the V-Cache CCD with:

   ```powershell
   process-corral topology
   ```

   The line `L3 group N (96 MB, vcache): CPUs ...` lists them. If the load sits on those, the driver is placing the game correctly and you do not need a profile.

If it still does not work, and the game has no anti-cheat, a Process Corral profile with `CpuGroup=vcache` is an alternative (see [PROFILES](PROFILES.md#pin-a-game-to-the-v-cache-cores)). With anti-cheat it may be refused.

## Already running (instance lock)

```text
Process Corral is already running (another `run` console or the 'ProcessCorral' service), so this instance will not start.
```

Only one engine may manage processes at a time. The console `run` command and the service share a single lock. Stop the other one:

```powershell
# the service (elevated)
process-corral stop
# or a console: press Ctrl+C in that console window
```

If you cannot find the other instance, check `Get-Service ProcessCorral` and `Get-Process process-corral`. The GUI is separate and does not hold the lock.

## Log rotation failed

The log shows **"Log rotation failed (non-fatal)"** (red in the Log tab), with the file and the Windows error. Nothing is lost: the engine keeps writing to the current `corral.log`, which keeps growing past `MaxLogSizeMB` until a rotation works, and retries about every 30 seconds. The message appears once per problem, not once per retry.

Usual causes:

- **Another program has `corral.log` or a `corral.N.log` open** and does not allow renaming or deleting it (a text editor, a log viewer, a backup or antivirus scan). Close it, or open the file in a viewer that allows other programs to move it. The next retry then succeeds.
- **"Access is denied."** Whoever runs the engine cannot rename or delete files in the logs folder: check the folder's permissions (the service runs as SYSTEM and needs write access to `C:\ProgramData\ProcessCorral\logs`), and that no rotated file is read-only.
- **The disk is full or the folder is gone.** Free some space or recreate the folder.

If you would rather not rotate at all, set `MaxLogSizeMB=1024` (the largest allowed) in [`[Logging]`](CONFIG.md#logging).

## An icon that does not refresh

- **The tray icon** follows the engine: a green dot while the engine is running and a grey dot when it is not. It is refreshed about once a second from the status file. If it looks wrong, check the header of the window; the tray icon just mirrors it. If Windows has hidden the icon, look in the tray's overflow (the `^` arrow) and drag it out to pin it.
- **The program or shortcut icon looks old or generic** after an upgrade or install. Windows keeps a cache of file icons. Signing out and back in, or restarting, normally refreshes it. Some people also run `ie4uinit.exe -show` to refresh the cache; this is a general Windows trick, not tested for this program. Deleting and recreating the shortcut also works.

## Reporting a bug

Please include, and remove anything private first (user and computer names appear in the log's `Machine` and `User` columns):

- Process Corral's version and how it was installed (installer or manual):

  ```powershell
  (Get-Item "C:\Program Files\ProcessCorral\process-corral.exe").VersionInfo.FileVersion
  ```

- Your Windows version (`winver`) and CPU model.
- What you expected and what happened, and the exact steps.
- The last part of the log (`corral.log`, or the Log tab) around the time of the problem.
- `C:\ProgramData\ProcessCorral\status\status.json`.
- Your `corral.ini` (or just the relevant section).
- For CPU placement problems, the output of `process-corral topology --json`.
- For service problems, the output of `sc.exe qc ProcessCorral` and `Get-Service ProcessCorral`.
- Whether another process manager is installed or running.
