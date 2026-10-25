# Profiles, game mode and V-Cache

A **profile** tells Process Corral what to do with one program: give it a priority, pin it to some CPU cores, keep Restraint away from it, and optionally treat it as a game. Profiles live in the config file and are edited in the GUI's **Profiles** tab or by hand.

![The Profiles tab](images/gui-profiles.png)

## The file format

Each profile is a section named `[Profile.<name>]`. The name is yours to choose (spaces are fine) and must be unique, ignoring capitals. Only `Match` is required.

```ini
[Profile.Game]
Match=game.exe
Priority=high
CpuGroup=vcache
GameMode=true
```

Section names and keys are not case sensitive. If a key appears twice in a section, the **last** one wins. Lines starting with `;` or `#` are comments. Unknown keys are ignored.

| Key | Values | Meaning |
| --- | --- | --- |
| `Match` | exe name or pattern | **Required.** Which programs the profile applies to. `*` and `?` work, capitals do not matter. A plain name (`game.exe`) is compared with the exe's file name. A pattern containing `\` or `/` (for example `C:\Games\*\game.exe`) is compared with the program's full path. |
| `Enabled` | `true` / `false` | Default `true`. A disabled profile does nothing, and anything it changed is put back. |
| `Priority` | `idle`, `belownormal`, `normal`, `abovenormal`, `high` | The priority class to set. `low` is accepted as another name for `idle`. `realtime` is **not allowed** in a profile and makes the file invalid. Leave the key out to leave priority alone. |
| `Affinity` | `0x` hex mask, or a list | Restrict the program to these logical processors with the Windows **affinity** mask. A mask must start with `0x` (`0xFFFF`); a list looks like `0-15,24`. See [Numbering](#numbering-the-cpus). |
| `CpuSets` | `0x` hex mask, or a list | Same syntax as `Affinity`, but applied as **CPU Sets**. You cannot use `Affinity` and `CpuSets` in the same profile. |
| `CpuGroup` | `vcache`, `frequency`, or an L3 group number | Choose a whole L3 cache group by name; it is worked out on each machine when the profile is applied. Cannot be combined with `Affinity` or `CpuSets`. |
| `CpuMode` | `affinity` (default), `cpusets` | How `CpuGroup` (and `SmtPolicy`) are applied. If you also give `Affinity` or `CpuSets`, `CpuMode` must agree with it. |
| `SmtPolicy` | `all` (default), `no-smt` | `no-smt` keeps only the first hardware thread of each core in the chosen set. Used alone, it means "the first thread of every core". |
| `GameMode` | `true` / `false` | Default `false`. While this program runs, game mode is on. See [Game mode](#game-mode). |
| `ExcludeFromRestraint` | `true` / `false` | Default `false`. Restraint never lowers this program. |

Yes/no values accept `true`, `1`, `yes`, `on` and `false`, `0`, `no`, `off`. An empty `Priority`, `Affinity`, `CpuSets`, `CpuGroup`, `CpuMode` or `SmtPolicy` counts as "not set"; an empty `Match` or yes/no value is an error.

If several enabled profiles match the same program, the **first one in the file** is used.

A profile with a mistake makes the whole file invalid when saved or reloaded, and the error says which line (`line 14: [Profile.Game] Priority: unknown priority 'turbo' ...`). See [Troubleshooting](TROUBLESHOOTING.md#config-rejected).

### Numbering the CPUs

Lists and masks use **logical processor numbers**: number 0 is the first hardware thread the system reports, and so on. Which numbers belong to which core and which cache group depends on your CPU. Print the layout:

```powershell
process-corral topology
```

For example, for a (simulated) Ryzen 9 9950X3D where the first cache group is the V-Cache one:

```text
Logical processors: 32   Physical cores: 16   SMT: yes   Processor groups: 1
L3 cache groups ("CCDs"): 2   cache-heavy (V-Cache): group 0 (96 MB)   frequency: group 1
...
L3 group 0 (96 MB, vcache): CPUs 0-15
    affinity mask in processor group 0: 0x000000000000FFFF
L3 group 1 (32 MB, frequency): CPUs 16-31
    affinity mask in processor group 0: 0x00000000FFFF0000
```

On a real chip the V-Cache group may be the second one (CPUs 16-31). This is why `CpuGroup=vcache` is better than typing a mask: Process Corral picks the L3 group with the **strictly largest** cache. If all groups are the same size (a plain 9950X, or a single-CCD chip) there is no V-Cache group and a `CpuGroup=vcache` profile is reported as "not possible" instead of guessing. You can preview other layouts with `process-corral topology --simulate 9950x3d-ccd1`.

## Examples

### Pin a game to the V-Cache cores

```ini
[Profile.Game]
Match=game.exe
Priority=high
CpuGroup=vcache
```

By hand with the same effect on the simulated chip above: `Affinity=0-15`. The named form keeps working if the V-Cache CCD is numbered differently on another machine.

### Keep a background job on the other cores

```ini
[Profile.Encoder]
Match=encoder.exe
Priority=belownormal
CpuGroup=frequency
ExcludeFromRestraint=true
```

### One thread per core, using CPU Sets

```ini
[Profile.Sim]
Match=sim.exe
CpuGroup=vcache
CpuMode=cpusets
SmtPolicy=no-smt
```

### Match by path

```ini
[Profile.Launcher]
Match=C:\Games\*\launcher.exe
Priority=idle
```

## CPU Sets versus Affinity

Both choose which logical processors a program may use. They are different Windows features:

- **Affinity** is a hard limit: the program's threads are only allowed on the chosen processors. Some programs set their own affinity or fight a change; some anti-cheat refuses it.
- **CPU Sets** are what the GUI calls "a softer preference". The program is steered to the chosen processors but, per Microsoft's documentation, Windows treats them as a hint rather than a strict prohibition. They also work on machines with more than 64 logical processors, where an affinity mask cannot move a program between processor groups.

Which one is better for a given game is something to try; neither is a guaranteed win. Check Microsoft's documentation for "CPU sets" for the exact scheduling rules. Process Corral applies exactly what you ask for, reads the value back to check it, and says in the log if it differs.

## What the engine does with a profile

- It only touches what the profile sets: a profile with only `Affinity` never changes priority.
- It applies the change once per matching process, including programs already running when the engine starts or the config is reloaded.
- It reads the value back. If Windows or the program changed it, the Status tab shows "applied, but differs".
- If Windows **refuses**, it tries again: 5 attempts in all, at 0, 2, 6, 14 and 30 seconds, and then records the change as refused (log lines "Profile change refused; will retry" and "Profile change refused"). It does not keep trying forever, and it never tries to get around the refusal.
- When the profile is removed, disabled, edited, or the program stops matching, it puts the original value back, unless something else changed the value in the meantime (then it leaves it alone and says so). When the engine stops, everything is put back.
- Programs that have exited are forgotten, never "restored".
- Critical Windows processes (such as `smss.exe`, `csrss.exe`, `lsass.exe`, `winlogon.exe`, `services.exe`, `wininit.exe`) and Process Corral itself are never touched. A few more system processes on Restraint's built-in list (`explorer.exe`, `dwm.exe`, `svchost.exe` and others) can get a priority from a profile but never an affinity or CPU Sets.

## How profiles fit with Restraint and the older lists

- **Restraint.** `ExcludeFromRestraint=true` keeps Restraint away from the program. While **game mode** is active, programs of profiles with `GameMode=true` are also never restrained. If Restraint happens to have a program lowered when its profile is applied, the profile's priority becomes the one Restraint restores when the restraint ends.
- **The older lists** (`DefaultPriorities`, `DefaultAffinities`, `DisallowedProcesses` in [CONFIG.md](CONFIG.md#processdefaults)). A profile **wins**: for a program that a profile matches, those lists are skipped entirely, including `DisallowedProcesses`. The log says "Profile takes precedence over the older lists" once per process. Programs no profile matches are handled by the lists as before.

## Game mode

Mark a profile with `GameMode=true` (the **Game mode** checkbox in the GUI). While at least one running program matches such a profile, game mode is **on**: the log says "Game mode entered" naming the program, and "Game mode left" when the last one exits.

What game mode does is set in the `[GameMode]` section of the config file. It is not edited in the GUI.

```ini
[GameMode]
BackgroundPriority=true
PowerPlan={8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c}
```

| Key | Default | Meaning |
| --- | --- | --- |
| `BackgroundPriority` | `false` | When game mode starts, lower other programs that are at **Normal** priority to **Below Normal** (each program once). Skipped: the game itself and its child processes, programs matched by any profile, Windows system processes, services, anything in `OocExclusions`, and programs Restraint already restrained. |
| `PowerPlan` | empty (do not change) | A Windows power scheme GUID, with or without braces. Game mode switches to it and switches back afterwards. |

Find the power scheme GUIDs on your PC:

```powershell
powercfg /list
```

On leaving game mode, processes are restored only if they are still at Below Normal (someone may have changed them), and the power plan is only switched back if it is still the one game mode set. If Windows refuses the power plan change the log says so ("Game mode: power plan not changed").

## Finding a program's exe name

- In the GUI Profiles tab: **Pick running process** (a list of what is running right now, with a filter) or **Browse...** (choose the `.exe`; only its file name is used).
- From the command line: `process-corral top --count 30` lists the busiest processes with their exe names.
- From PowerShell:

  ```powershell
  Get-Process | Sort-Object CPU -Descending | Select-Object -First 15 Name, Id, Path
  ```

  `Name` has no `.exe`; the `Match` value is the exe file name, so add it (`game` becomes `game.exe`), or copy it from the end of `Path`.
- Task Manager: **Details** tab, the **Name** column.

Launchers often start the real game as a second program, and the game's exe is the one to match.

## Anti-cheat and protected games

Some games, especially online games with anti-cheat, **refuse** changes to their priority and CPU affinity from other programs. Windows then answers "Access is denied" (Win32 error 5). On the author's machine **Battlefield 6** does this.

What Process Corral does when that happens:

- it records the refusal: in the log, on the Status tab (refused list and, for Restraint, the **Protected, not managed** list), and in the profile's row;
- it retries a few times for a profile (see above) and then stops;
- for Restraint, it remembers the process as protected, logs it **once**, and leaves it alone until it exits or the rules change;
- it **does not try to get around** the refusal. It never tries to take over the program's handle, change its permissions, or hide from the anti-cheat. Running Process Corral elevated or as the service gives it the normal administrator rights; if Windows still says no, the program is deliberately protected.

So for such a game a profile may simply not take effect. That is the game's choice, not a fault in your config.

**You may not need a profile for V-Cache anyway.** On X3D processors Windows and AMD's chipset/V-Cache driver may already decide which CCD a game runs on. If that works for your game, an extra Process Corral profile is unnecessary, and with anti-cheat games the driver is the better route because it does not need to change the game's process. If the game is not landing on the V-Cache cores, see [A game isn't being placed on the V-Cache CCD](TROUBLESHOOTING.md#a-game-isnt-being-placed-on-the-v-cache-ccd).
