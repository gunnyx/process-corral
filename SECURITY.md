# Security policy

## Why this matters

Process Corral's engine normally runs as a Windows service under the **LocalSystem** account. It reads a configuration file, changes the priority and CPU placement of other programs, and (only if you list programs in `DisallowedProcesses` or `InstanceLimitedProcesses`) ends processes. A flaw in how it handles its files, its configuration or its elevation could let a lower-privileged user gain more than they should have, so security reports are taken seriously and are welcome.

## Reporting a vulnerability

Please report privately, not in a public issue.

Contact: use GitHub's private reporting. Open the **Security** tab of this repository and click **Report a vulnerability**.

Please include:

- the Process Corral version (shown in the GUI, or in the exe's Properties under Details) and how it was installed (installer or built from source);
- your Windows version (`winver`) and whether you ran the service or a console `run`;
- the steps to reproduce the problem, as exact as you can make them;
- the impact: what an attacker needs to start with (for example "a standard user account on the machine") and what they gain.

Please **do not post exploit details in public issues**, pull requests or discussions until a fix is available. A public issue that says only "I found a security problem, how do I contact you?" is fine.

## Supported versions

Only the latest released version is supported. This is early software (0.1.x); fixes are made on the main branch and released as a new version.

## Security design in brief

This summarises [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#apply-config-and-the-elevation-model), which has the details.

- **Admin-only config folder.** `C:\ProgramData\ProcessCorral` is created with inheritance removed: SYSTEM and Administrators have full control, ordinary users can only read, and Administrators own it. The permissions use well-known security identifiers, so they do not depend on the Windows display language. They are applied when the program creates the folder; an existing folder is left alone and reported, and the installer tightens one that ordinary users can write to. Log files, including rotated ones, and the status file live inside it and inherit its permissions.
- **Config validated in memory before writing.** Settings changes go through `apply-config`, which reads the file once, validates exactly those bytes in memory, and only then installs them with an atomic replace. A different file swapped in afterwards is never installed, and an invalid file is never written. The running engine applies the same strict check on a live reload and keeps its previous settings if the file is bad.
- **Elevation model.** The GUI runs as the ordinary signed-in user and never holds administrator rights. Anything that needs them (start, stop, install, uninstall, `apply-config`) runs `process-corral.exe` as a separate, short-lived elevated process through the Windows "runas" mechanism, which shows the UAC prompt. The GUI cannot write the service's config itself; it can only ask the elevated helper to.
- **No attempt to defeat protection.** Where Windows or a program's own protection refuses a change, Process Corral records the refusal and moves on.

## Out of scope

Behaviour that requires administrator rights to set up (for example an administrator adding a harmful rule to the config) is by design: administrators control the engine. Reports about the unsigned installer showing an "unknown publisher" warning are known; see the [README](README.md#use-at-your-own-risk).