# Installing Process Corral

## Requirements

- Windows 10 or later, **64-bit**. The installer refuses to run on 32-bit Windows and on anything older than Windows 10.
- An **administrator** account. Installing writes to `C:\Program Files`, registers a Windows service and creates `C:\ProgramData\ProcessCorral`.
- Nothing else to install. The programs are self-contained.

Do not run Process Corral together with another process manager. See [Another process manager: do not run both](#another-process-manager-do-not-run-both).

## Installing with the installer

### Checking the download

The build script writes a checksum file next to the installer (`ProcessCorral-Setup-<version>.exe.sha256`). Compare it with the file you downloaded:

```powershell
cd $HOME\Downloads
Get-FileHash .\ProcessCorral-Setup-0.1.1.exe -Algorithm SHA256
Get-Content .\ProcessCorral-Setup-0.1.1.exe.sha256
```

The two hashes must match (the `.sha256` file shows the hash in lower case followed by the file name; PowerShell prints it in upper case). If they do not, do not run the file.

### The SmartScreen warning

The installer is **not code-signed**, so Windows shows **"Windows protected your PC"** and lists the publisher as *Unknown publisher*. That is expected for an unsigned program; it does not mean the file is damaged. If the SHA256 matches what you were given:

1. Click **More info**.
2. Click **Run anyway**.
3. Approve the administrator (UAC) prompt.

### The steps

1. Run `ProcessCorral-Setup-<version>.exe`.
2. Choose the install folder. The default is `C:\Program Files\ProcessCorral`. Both `process-corral.exe` and `process-corral-gui.exe` are placed side by side in it, together with `LICENSE` and `THIRD-PARTY-NOTICES.md` (the uninstaller removes them again).
3. If the service of another process manager looks active, the wizard shows a warning page and offers a checkbox to stop and disable it. The checkbox is **off by default**. See [below](#another-process-manager-do-not-run-both).
4. Choose optional tasks. **All are off by default**:
   - *Create a desktop icon* (for all users, opens the GUI).
   - *Start the Process Corral window (tray icon) when I sign in* (an all-users Startup shortcut, started minimised).
   - *Stop and disable the other process manager's service* (only shown when one was detected).
5. Press **Install**.

On a fresh install the installer then runs `process-corral.exe init --system`, `process-corral.exe install` and `process-corral.exe start` for you. The result is:

- the service `ProcessCorral` (automatic start, LocalSystem), running;
- `C:\ProgramData\ProcessCorral\config\corral.ini` (a default config, created only if there was none);
- a Start menu entry **Process Corral** that opens the GUI;
- an entry in **Apps & Features** (publisher "Process Corral").

The data folder `C:\ProgramData\ProcessCorral` is created so that only SYSTEM and Administrators can write to it; ordinary users can read it. If the folder already exists and ordinary users could write to it, the installer tightens it and says so (your files are not changed). If it already exists and is fine, it is left alone.

> **Not exercised here:** the installer's wizard pages were not clicked through by hand when this was written; the steps above follow the installer script and the silent-install tests. If a page looks different from this description, please report it.

### Silent install

Every choice can be made on the command line. These switches are Inno Setup's standard ones plus a few of Process Corral's own:

```powershell
# Quiet install with defaults
.\ProcessCorral-Setup-0.1.1.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART

# Different folder, desktop icon, start at sign-in, and an install log
.\ProcessCorral-Setup-0.1.1.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART `
    /DIR="D:\Tools\ProcessCorral" /TASKS="desktopicon,startgui" /LOG="$env:TEMP\pc-setup.log"
```

| Switch | Meaning |
| --- | --- |
| `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART` | No windows, no message boxes, no restart. |
| `/DIR="path"` | Install to this folder. |
| `/TASKS="desktopicon,startgui,stopother"` | Tick the named optional tasks. With no `/TASKS`, none of the three is ticked. |
| `/NOSERVICE` | Copy files only. Do not touch the Windows service or `C:\ProgramData\ProcessCorral`. |
| `/OTHERSERVICE=<name>` | Treat this service name as the other process manager's service (default `ProcessGovernor`). Mainly for testing. |
| `/LOG="file"` | Write the installer's log; it records every decision, including what it left alone. |

Run the installer from an **elevated** PowerShell (or let the UAC prompt appear) because it needs administrator rights.

## Upgrading

Run the new installer over the old one. It finds the existing install and:

1. asks a running Process Corral window to exit (it runs `process-corral-gui.exe --quit`; Windows' standard "close the application" prompt appears if that does not work, and a silent install stops with an error instead, changing nothing);
2. stops the service if it was running;
3. replaces the files;
4. starts the service again **only if it was running before**.

Your `corral.ini`, logs and status files are kept. The installer only touches a service whose program is inside the folder you are installing to; a service with the same name pointing somewhere else is left alone.

## Uninstalling

Use **Settings > Apps > Installed apps > Process Corral > Uninstall**, or run `unins000.exe` from the install folder.

The uninstaller stops and removes the service (with `process-corral.exe uninstall`), removes the program files and the shortcuts it created, and then asks:

> Also delete your Process Corral settings and logs? `C:\ProgramData\ProcessCorral`

- **No** (the default) keeps the folder, so a later reinstall picks up your settings.
- **Yes** deletes it.

Silent uninstall:

```powershell
# Keep settings and logs
& "C:\Program Files\ProcessCorral\unins000.exe" /VERYSILENT /SUPPRESSMSGBOXES

# Also delete C:\ProgramData\ProcessCorral
& "C:\Program Files\ProcessCorral\unins000.exe" /VERYSILENT /SUPPRESSMSGBOXES /DELETEDATA
```

`/NOSERVICE` on the uninstaller leaves the Windows service alone.

## Manual install from the exes

Use this if you built the programs yourself or do not want the installer. Do it from an **elevated** PowerShell.

1. Copy both programs to a folder that will stay put (not `target\release`, because rebuilding cannot replace a running service's exe):

   ```powershell
   $dest = "C:\Program Files\ProcessCorral"
   New-Item -ItemType Directory -Force $dest | Out-Null
   Copy-Item .\target\release\process-corral.exe, .\target\release\process-corral-gui.exe $dest
   ```

2. Create the data folder and a default config. This also sets the folder permissions (see below):

   ```powershell
   & "$dest\process-corral.exe" init --system
   ```

3. Register the service, then start it:

   ```powershell
   & "$dest\process-corral.exe" install
   & "$dest\process-corral.exe" start
   ```

4. Check it:

   ```powershell
   Get-Service ProcessCorral
   Get-Content "$env:ProgramData\ProcessCorral\logs\corral.log" -Tail 5
   ```

`install` refuses to run if a service called `ProcessCorral` already exists; to replace it run `stop`, `uninstall`, then `install` again. It prints a warning if the exe sits under `target\debug` or `target\release`.

### Folder permissions

`init --system` and `install` create `C:\ProgramData\ProcessCorral` (with `config`, `logs` and `status` inside). **Only when they create the folder** do they set its permissions: SYSTEM and Administrators get full control, Users get read and execute, and the owner becomes Administrators. This matters because the service runs as SYSTEM and reads its settings from there; a folder that ordinary users can write to would let them control a SYSTEM service.

If the folder already existed, its permissions are not changed and the command prints a note. Check it yourself:

```powershell
icacls "$env:ProgramData\ProcessCorral"
```

Look for `BUILTIN\Users:(OI)(CI)(RX)` (read only). If Users have `M`, `W` or `F`, tighten it with the same commands the program uses:

```powershell
icacls "$env:ProgramData\ProcessCorral" /inheritance:r /grant:r "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-544:(OI)(CI)F" "*S-1-5-32-545:(OI)(CI)RX"
icacls "$env:ProgramData\ProcessCorral" /setowner "*S-1-5-32-544"
```

### Console mode (no service)

You can also run the engine in a console without installing anything. It then uses `%APPDATA%\ProcessCorral` instead of ProgramData. See [USAGE](USAGE.md#run).

## Building the installer from source

You need Inno Setup 6. The script looks for `ISCC.exe` on the PATH and in `%LOCALAPPDATA%\Programs\Inno Setup 6`, `%ProgramFiles(x86)%\Inno Setup 6` and `%ProgramFiles%\Inno Setup 6`. If it is missing:

```powershell
winget install --id JRSoftware.InnoSetup -e
```

Then, from the repository root:

```powershell
.\installer\build-installer.ps1
```

The script builds the programs (`cargo build --release --features gui`), reads the version from `Cargo.toml`, compiles `installer\process-corral.iss`, and writes:

- `dist\ProcessCorral-Setup-<version>.exe`
- `dist\ProcessCorral-Setup-<version>.exe.sha256`

Add `-SkipBuild` to reuse the exes already in `target\release`. The `dist` folder is ignored by git; do not commit built files.

You also need the Rust toolchain and the Windows SDK as described in the [README](../README.md#building-from-source).

## Another process manager: do not run both

Process Corral and another process manager both change the priority of busy programs. Running both means two process managers undo each other's changes. Run only one.

### How to check whether another process manager is installed

Look at the running services, the programs that start with Windows, and the tray icons (click the `^` arrow in the taskbar to see hidden ones). From PowerShell:

```powershell
# Running services that do not live under the Windows folder
Get-CimInstance Win32_Service |
    Where-Object { $_.State -eq 'Running' -and $_.PathName -notmatch '\\Windows\\' } |
    Sort-Object DisplayName | Format-Table Name, DisplayName, StartMode, PathName -AutoSize

# Programs that start when you sign in
Get-CimInstance Win32_StartupCommand | Format-Table Name, Command, Location -AutoSize
```

A program whose name mentions priorities, CPU or process management is a candidate. If you find one, stop it (or turn off its automatic restraint) before relying on Process Corral.

### What the installer does

The installer looks for a service named `ProcessGovernor`, the service name used by another process manager. If it is running or set to start automatically, the wizard shows a warning and an **off-by-default** checkbox: *Stop and disable the other process manager's service*. Only if you tick it does the installer run `sc stop` and `sc config ... start= disabled` on that service. It never uninstalls the other program and never touches it without that choice. To bring it back later, set its startup type in the Services app.

Process Corral itself does not look for other process managers at run time. If "nothing is being restrained", see [Troubleshooting](TROUBLESHOOTING.md#nothing-is-being-restrained).
