# Config file reference (`corral.ini`)

This page lists every setting the engine reads. It was written from the parser in `src/config.rs` (and `src/profile.rs` for profiles); the defaults below are the ones in the source.

The file is an ordinary INI file. For the service it is `C:\ProgramData\ProcessCorral\config\corral.ini`. `process-corral init` writes a starting copy.

## Reading rules

- Sections are `[Name]`; settings are `Key=Value`. Section and key names are **not** case sensitive. Spaces around `=` are ignored.
- Lines starting with `;` or `#` are comments, and so are blank lines. There are no end-of-line comments: `OocOn=true ; yes` makes the value `true ; yes`, which is invalid.
- If a key appears twice in a section, the **last** one wins.
- The file may be UTF-8 (with or without a byte-order mark) or UTF-16 little-endian with a byte-order mark. Anything else is rejected.
- A **missing file** means "use the defaults". A **missing key** means "use that key's default".
- **Unknown sections and unknown keys are ignored and not checked.** That keeps old configs working, but it also means a misspelled key name is silently ignored. If a setting seems to have no effect, check the spelling against this page.
- At startup, on live reload, and in `apply-config`, the file is checked strictly: a line that is not `key=value` or `[section]`, a key before any section, a value of the wrong type, or a number out of range makes the **whole file invalid**, with a message that starts `line N:`. See [Troubleshooting](TROUBLESHOOTING.md#config-rejected). An invalid file at startup stops the engine from starting; an invalid file on reload is ignored and the previous settings stay.

Value types used below:

- **bool**: `true`, `false`, `1` or `0` (capitals do not matter).
- **number**: a plain number, optionally with decimals. Percentages must be between 0 and 100, times in milliseconds between 0 and 86,400,000 (one day), unless stated.
- **hex mask**: hexadecimal digits with an optional `0x` prefix, for example `0xFF`. Must not be zero.
- **list**: names separated by commas, for example `a.exe, b*.exe`. Names are matched without regard to capitals, and `*` (any text) and `?` (one character) work.
- **pairs**: entries separated by `;`, each `pattern:value`, for example `a.exe:high;b*.exe:idle`.

## `[OutOfControlProcessRestraint]`

This section is Restraint. Every "percent" here is a share of the **whole machine's** CPU capacity, all logical processors together. On a 32-thread CPU, a program that completely fills one core uses only about 3% by this measure. See [Troubleshooting](TROUBLESHOOTING.md#nothing-is-being-restrained).

A process is lowered when, for at least `TimeOverQuotaBeforeRestraint` milliseconds in a row, the **whole system** is above `TotalProcessorUsageBeforeRestraint` **and** that process is above `PerProcessUsageBeforeRestraint`. It is restored when it falls below `PerProcessUsageForRestore` after at least `MinimumTimeOfRestraint` ms, or when `MaximumTimeOfRestraint` is reached. Only what Process Corral changed is put back: if you or the program changed the priority meanwhile, it is left as it is.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `OocOn` | bool | `true` | Turn Restraint on or off. Turning it off (also by live reload) immediately restores anything currently lowered. |
| `TotalProcessorUsageBeforeRestraint` | number 0-100 | `23` | Whole-system CPU use (%) that must be exceeded before any process is lowered. |
| `PerProcessUsageBeforeRestraint` | number 0-100 | `15` | A process must use more than this (% of the whole machine) to be lowered. |
| `TimeOverQuotaBeforeRestraint` | ms | `900` | How long both limits must stay exceeded. |
| `PerProcessUsageForRestore` | number 0-100 | `5` | A lowered process is restored when it uses less than this. |
| `MinimumTimeOfRestraint` | ms | `4200` | A process stays lowered at least this long. |
| `MaximumTimeOfRestraint` | ms | `0` | Restore a process after this long even if it is still busy. `0` means no limit. |
| `LowerToIdleInsteadOfBelowNormal` | bool | `false` | Lower to Idle instead of Below Normal. |
| `TameOnlyNormal` | bool | `true` | Only lower processes currently at Normal priority. |
| `RestrainByAffinity` | bool | `false` | Also restrict a lowered process to the CPUs in `RestraintAffinity` (the original affinity is restored with the priority). Does nothing while `RestraintAffinity` is empty. |
| `RestraintAffinity` | hex mask, or empty | empty | Affinity mask used by `RestrainByAffinity`. Must not be zero if given. |
| `ExcludeForegroundProcesses2` | bool | `true` | Never lower the program that owns the foreground window; if it is lowered when it comes to the front, it is restored. |
| `ExcludeChildrenOfForeground` | bool | `true` | Never lower programs started by the foreground program. |
| `ExcludeServices` | bool | `true` | Never lower services (processes in Windows session 0). |
| `OocExclusions` | list | empty | Programs Restraint never lowers, for example `backup*.exe, game.exe`. |

Always excluded, whatever you set: Process Corral itself, and a built-in list of system processes (`csrss.exe`, `dwm.exe`, `explorer.exe`, `lsass.exe`, `lsm.exe`, `services.exe`, `sihost.exe`, `smss.exe`, `svchost.exe`, `wininit.exe`, `winlogon.exe`, `logonui.exe`, `wmiprvse.exe`, `runtimebroker.exe`, `ehshell.exe`, `taskeng.exe`, `wudfhost.exe`, `mfpmp.exe`). Programs a profile excludes (`ExcludeFromRestraint`) or marks as game mode during game mode are also skipped. Processes that cannot be changed (Windows says "Access is denied") are noted as protected and left alone.

The key names in this section follow common process-governor conventions, which is why they look the way they do (for example the `2` in `ExcludeForegroundProcesses2`).

## `[ProcessDefaults]`

Priorities and affinities applied **once** to every matching program when Process Corral first sees it: programs started later, and every program already running when the engine starts. A program that was already being watched is **not** re-checked when you change these lists; they take effect for new programs (or after the engine restarts). The values are not undone afterwards. A program that matches a [profile](PROFILES.md) is skipped here.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `DefaultPriorities` | pairs `pattern:class` | empty | Priority to set. The first matching pattern wins. Classes: `idle` (also `low`), `belownormal`, `normal`, `abovenormal`, `high`, `realtime`. Spaces, `_` and `-` in the class are ignored. Using `realtime` is accepted here but is a bad idea; profiles refuse it. Example: `game.exe:high;render*.exe:belownormal` |
| `DefaultAffinities` | pairs `pattern:hex mask` | empty | Affinity mask to set. The first matching pattern wins. Example: `encoder.exe:0xFF00` |

## `[ProcessAllowances]`

> **Careful:** these settings end programs.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `DisallowedProcesses` | list | empty | Programs that may not run. A matching program is **terminated** when Process Corral first sees it (at startup, every program already running that matches is ended). Skipped for programs a profile matches. Logged as "Disallowed process terminated". |
| `InstanceLimitedProcesses` | pairs `pattern:count` | empty | At most `count` copies of a program may run. The oldest copies are kept; copies beyond the limit are **terminated**. Example: `helper.exe:1` |

## `[Performance]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `UpdateSpeedCore` | ms, 100-3,600,000 | `1000` | How often the engine looks at processes. Lower is more responsive and uses a little more CPU. The status file is rewritten about every 2 seconds regardless. |

## `[Logging]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `LogDisable` | bool | `false` | `true` stops writing the log file (the GUI's Log tab is then empty). The last 20 events are still kept in memory for the status file and the Status tab. It can be switched at run time. |
| `MaxLogSizeMB` | whole number, 1-1024 | `10` | Rotate the log when `corral.log` reaches this many megabytes. The size is checked before each write, so a rotated file can end a few hundred bytes over. |
| `KeepLogFiles` | whole number, 0-50 | `5` | How many rotated files to keep: `corral.1.log` (newest) to `corral.N.log` (oldest). The oldest is deleted when a rotation would exceed this number. `0` keeps none: the log is emptied (a fresh header row only) when it reaches the limit. |

Both rotation settings are validated like every other value (a bad one rejects the whole file, with the line number) and take effect on a live reload, with no restart. Rotation also happens when the engine starts and the file is already over the limit. It works the same in console and service mode and with `--log`, and it is skipped while `LogDisable=true`. See [Log files and rotation](USAGE.md#log-files-and-rotation).

## `[GameMode]`

What game mode does while a profile with `GameMode=true` has a running program. Details and caveats are in [PROFILES.md](PROFILES.md#game-mode).

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `BackgroundPriority` | bool | `false` | Lower other Normal-priority programs to Below Normal while a game runs; restored when it ends. |
| `PowerPlan` | GUID, or empty | empty | Power scheme to switch to while a game runs, for example `{8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c}` (braces optional). Empty leaves the power plan alone. Must be a valid GUID. |

## `[Profile.<name>]`

One section per program profile. All keys (`Match`, `Enabled`, `Priority`, `Affinity`, `CpuSets`, `CpuGroup`, `CpuMode`, `SmtPolicy`, `GameMode`, `ExcludeFromRestraint`) are described in [PROFILES.md](PROFILES.md#the-file-format).

## The default file

`process-corral init` (and the installer) write this:

```ini
; Process Corral configuration
[OutOfControlProcessRestraint]
OocOn=true
ExcludeChildrenOfForeground=true
ExcludeForegroundProcesses2=true
ExcludeServices=true
TameOnlyNormal=true
LowerToIdleInsteadOfBelowNormal=false
TotalProcessorUsageBeforeRestraint=23
PerProcessUsageBeforeRestraint=15
TimeOverQuotaBeforeRestraint=900
PerProcessUsageForRestore=5
MinimumTimeOfRestraint=4200
MaximumTimeOfRestraint=0
RestrainByAffinity=false
RestraintAffinity=
OocExclusions=

[ProcessDefaults]
; name:class;name:class   (idle, belownormal, normal, abovenormal, high)
DefaultPriorities=
; name:hexmask;name:hexmask
DefaultAffinities=

[ProcessAllowances]
; comma-separated names, wildcards allowed
DisallowedProcesses=
; name:max;name:max
InstanceLimitedProcesses=

[Performance]
UpdateSpeedCore=1000

[Logging]
LogDisable=false
MaxLogSizeMB=10
KeepLogFiles=5
```

The `[GameMode]` section and profiles are not in the default file; add them when you want them.
