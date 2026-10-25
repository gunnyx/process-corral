; Process Corral installer (Inno Setup 6).
;
; Build with installer\build-installer.ps1 (it passes /DAppVersion=<Cargo.toml version>).
; Run by hand:  ISCC.exe /DAppVersion=0.1.1 installer\process-corral.iss
;
; SCRIPTING (all work with /VERYSILENT /SUPPRESSMSGBOXES /NORESTART):
;   /DIR="C:\Some\Folder"        install somewhere else
;   /TASKS="desktopicon,startgui,stopother"   tick optional tasks (all are OFF by default)
;   /NOSERVICE                   files only: never touch the Windows service or the ProgramData folder
;   /OTHERSERVICE=<name>         service name to treat as another process manager (default ProcessGovernor)
;   /LOG="file.log"              write Inno's log (the installer logs every decision it makes)
; Uninstaller (unins000.exe):
;   /VERYSILENT /SUPPRESSMSGBOXES   uninstall silently; keeps C:\ProgramData\ProcessCorral
;   /DELETEDATA                     ...and also delete that folder (config and logs)
;   /NOSERVICE                      do not touch the Windows service
;
; SAFETY RULE: the service is only stopped, restarted or removed when its executable is inside the
; folder being installed to / uninstalled from. A "ProcessCorral" service that points somewhere else
; is left completely alone.
;
; /DTestBuild=1 compiles a throw-away variant (different AppId, service name "PCTestService", data
; folder "ProcessCorralTest") so installer logic can be exercised without touching a real install.

#ifndef AppVersion
  #error Pass /DAppVersion=x.y.z (installer\build-installer.ps1 does this).
#endif
#ifndef SourceDir
  #define SourceDir "..\target\release"
#endif
#ifndef OutDir
  #define OutDir "..\dist"
#endif

#ifdef TestBuild
  #define AppGuid "{{47699C42-604F-4D5F-AD53-990C1DF8FD26}"
  #define AppTitle "Process Corral (test build)"
  #define ServiceName "PCTestService"
  #define DataDirName "ProcessCorralTest"
  #define OutName "ProcessCorral-TEST-Setup-" + AppVersion
#else
  #define AppGuid "{{864F53BD-597E-45A1-9497-2729281A53EC}"
  #define AppTitle "Process Corral"
  #define ServiceName "ProcessCorral"
  #define DataDirName "ProcessCorral"
  #define OutName "ProcessCorral-Setup-" + AppVersion
#endif

[Setup]
AppId={#AppGuid}
AppName={#AppTitle}
AppVersion={#AppVersion}
AppVerName={#AppTitle} {#AppVersion}
AppPublisher=Process Corral
AppComments=Automatically keeps runaway processes from making Windows unresponsive.
VersionInfoVersion={#AppVersion}
VersionInfoCompany=Process Corral
VersionInfoProductName={#AppTitle}
VersionInfoDescription={#AppTitle} Setup
DefaultDirName={autopf}\ProcessCorral
UsePreviousAppDir=yes
DisableProgramGroupPage=yes
; Needs administrator: it installs a service and writes to Program Files and ProgramData.
PrivilegesRequired=admin
; 64-bit Windows 10 or newer only. Setup refuses to start anywhere else.
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
SetupIconFile=..\assets\icon\process-corral.ico
UninstallDisplayIcon={app}\process-corral-gui.exe
UninstallDisplayName={#AppTitle}
OutputDir={#OutDir}
OutputBaseFilename={#OutName}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Ask (Restart Manager) to close the GUI if it is running; never kill it. The engine exe is not in
; this filter: the service is stopped properly by the installer's own code instead.
CloseApplications=yes
CloseApplicationsFilter=process-corral-gui.exe
RestartApplications=no
ChangesEnvironment=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop icon"; GroupDescription: "Shortcuts:"; Flags: unchecked
Name: "startgui"; Description: "Start the Process Corral &window (tray icon) when I sign in"; GroupDescription: "Startup:"; Flags: unchecked
Name: "stopother"; Description: "Stop and &disable the other process manager's service (recommended, so two process managers do not fight)"; GroupDescription: "Another process manager was detected:"; Flags: unchecked; Check: OtherManagerDetected

[Files]
Source: "{#SourceDir}\process-corral.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\process-corral-gui.exe"; DestDir: "{app}"; Flags: ignoreversion
; Licence and third-party notices sit next to the exes; the uninstaller removes them with the rest.
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\THIRD-PARTY-NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
; Start Menu (always) and desktop (task, all-users desktop because this is an all-users install).
; The shortcuts open the GUI, never the console engine exe.
Name: "{autoprograms}\{#AppTitle}"; Filename: "{app}\process-corral-gui.exe"; IconFilename: "{app}\process-corral-gui.exe"; WorkingDir: "{app}"; Comment: "Process Corral"
Name: "{autodesktop}\{#AppTitle}"; Filename: "{app}\process-corral-gui.exe"; IconFilename: "{app}\process-corral-gui.exe"; WorkingDir: "{app}"; Comment: "Process Corral"; Tasks: desktopicon
; Login start: an all-users Startup shortcut, minimised so it comes up in the tray without a window in your face.
Name: "{commonstartup}\{#AppTitle}"; Filename: "{app}\process-corral-gui.exe"; IconFilename: "{app}\process-corral-gui.exe"; WorkingDir: "{app}"; Flags: runminimized; Tasks: startgui

[Code]
const
  ServiceName = '{#ServiceName}';
  DataDirName = '{#DataDirName}';

var
  OtherPage: TOutputMsgWizardPage;
  ServiceWasRunning: Boolean;

// ---------------------------------------------------------------- helpers (shared with the uninstaller)

function CmdParam(const Name: string): string;
var
  i: Integer;
  p, prefix: string;
begin
  Result := '';
  prefix := '/' + Uppercase(Name) + '=';
  for i := 1 to ParamCount do
  begin
    p := ParamStr(i);
    if Pos(prefix, Uppercase(p)) = 1 then
    begin
      Result := Copy(p, Length(prefix) + 1, MaxInt);
      Exit;
    end;
  end;
end;

function CmdFlag(const Name: string): Boolean;
var
  i: Integer;
  p: string;
begin
  Result := CmdParam(Name) <> '';
  if Result then Exit;
  for i := 1 to ParamCount do
  begin
    p := Uppercase(ParamStr(i));
    if p = '/' + Uppercase(Name) then
    begin
      Result := True;
      Exit;
    end;
  end;
end;

function NoService: Boolean;
begin
  Result := CmdFlag('NOSERVICE');
end;

function OtherServiceName: string;
begin
  Result := CmdParam('OTHERSERVICE');
  if Result = '' then Result := 'ProcessGovernor';
end;

// Runs `cmd /C ""exe" args > tmp 2>&1"` and returns the exit code and what was printed.
function RunCapture(const Exe, Args: string; var Output: string): Integer;
var
  tmp: string;
  raw: AnsiString;
  rc: Integer;
begin
  tmp := ExpandConstant('{tmp}\pc-capture.txt');
  DeleteFile(tmp);
  Output := '';
  if Exec(ExpandConstant('{cmd}'), '/C ""' + Exe + '" ' + Args + ' > "' + tmp + '" 2>&1"', '', SW_HIDE, ewWaitUntilTerminated, rc) then
  begin
    Result := rc;
    if LoadStringFromFile(tmp, raw) then Output := raw;
  end
  else
    Result := -1;
end;

function ScQuery(const Svc: string; var Output: string): Integer;
begin
  Result := RunCapture(ExpandConstant('{sys}\sc.exe'), 'query "' + Svc + '"', Output);
end;

function ScConfig(const Svc: string; var Output: string): Integer;
begin
  Result := RunCapture(ExpandConstant('{sys}\sc.exe'), 'qc "' + Svc + '"', Output);
end;

function ServiceExists(const Svc: string): Boolean;
var
  o: string;
begin
  Result := (ScQuery(Svc, o) = 0);
end;

function ServiceRunning(const Svc: string): Boolean;
var
  o: string;
begin
  Result := (ScQuery(Svc, o) = 0) and (Pos('RUNNING', o) > 0);
end;

// True when the service's executable lives inside `Dir`. This is the rule that keeps us away from
// a service that belongs to a different installation.
function ServiceIsInDir(const Svc, Dir: string): Boolean;
var
  o: string;
begin
  Result := (ScConfig(Svc, o) = 0) and (Pos(Lowercase(RemoveBackslash(Dir)) + '\', Lowercase(o)) > 0);
end;

function OtherManagerRunningOrAuto: Boolean;
var
  o: string;
begin
  Result := False;
  if ScQuery(OtherServiceName, o) <> 0 then Exit;
  if Pos('RUNNING', o) > 0 then
  begin
    Result := True;
    Exit;
  end;
  if (ScConfig(OtherServiceName, o) = 0) and (Pos('AUTO_START', o) > 0) then Result := True;
end;

function OtherManagerDetected: Boolean;
begin
  Result := OtherManagerRunningOrAuto;
end;

// Asks a running Process Corral window to exit. The window hides to the tray when closed, so the
// normal "close the application" request cannot end it; "--quit" can. The service is not affected.
// If this cannot reach it (for example it runs in another session), Inno's Restart Manager prompt
// is still the fallback during install.
procedure QuitGui(const Dir: string);
var
  exe: string;
  rc: Integer;
begin
  exe := AddBackslash(Dir) + 'process-corral-gui.exe';
  if not FileExists(exe) then Exit;
  if Exec(exe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, rc) then
  begin
    Log('process-corral-gui --quit -> exit ' + IntToStr(rc));
    Sleep(1500);
  end;
end;

// ---------------------------------------------------------------- data folder permissions

function DataDir: string;
begin
  Result := ExpandConstant('{commonappdata}\' + DataDirName);
end;

// Does an ordinary-user group (Users, Authenticated Users, Everyone) have an Allow entry that
// permits writing, creating, deleting or changing permissions on this folder? Looked up by SID so it
// works on any display language.
function UsersCanWrite(const Dir: string): Boolean;
var
  script, o: string;
  rc: Integer;
begin
  script := ExpandConstant('{tmp}\pc-acl.ps1');
  SaveStringToFile(script,
    '$bad = $false' + #13#10 +
    'foreach ($a in (Get-Acl -LiteralPath $args[0]).Access) {' + #13#10 +
    '  if ($a.AccessControlType -ne "Allow") { continue }' + #13#10 +
    '  $sid = $a.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value' + #13#10 +
    '  if ($sid -in "S-1-5-32-545","S-1-5-11","S-1-1-0") {' + #13#10 +
    '    if ([int]$a.FileSystemRights -band 0xD0156) { $bad = $true }' + #13#10 +
    '  }' + #13#10 +
    '}' + #13#10 +
    'if ($bad) { "WRITABLE" } else { "OK" }' + #13#10, False);
  rc := RunCapture(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoProfile -ExecutionPolicy Bypass -File "' + script + '" "' + Dir + '"', o);
  Result := (rc = 0) and (Pos('WRITABLE', o) > 0);
end;

// The same ACL the engine applies when it creates the folder: SYSTEM and Administrators full
// control, Users read-only.
procedure LockDownDataDir(const Dir: string);
var
  o: string;
begin
  RunCapture(ExpandConstant('{sys}\icacls.exe'),
    '"' + Dir + '" /inheritance:r /grant:r *S-1-5-18:(OI)(CI)F *S-1-5-32-544:(OI)(CI)F *S-1-5-32-545:(OI)(CI)RX', o);
  Log('icacls: ' + o);
  RunCapture(ExpandConstant('{sys}\icacls.exe'), '"' + Dir + '" /setowner *S-1-5-32-544', o);
end;

procedure PrepareDataDir;
var
  existed: Boolean;
  o: string;
  rc: Integer;
begin
  existed := DirExists(DataDir);
  Log('Data folder ' + DataDir + (' already existed: ' + IntToStr(Ord(existed))));
  // The engine creates the folders (hardened when it creates them) and writes a default corral.ini
  // only if there is none. It exits 1 with "already exists" when the config is there: that is fine.
  rc := RunCapture(ExpandConstant('{app}\process-corral.exe'), 'init --system', o);
  Log('process-corral init --system -> exit ' + IntToStr(rc) + ': ' + o);
  if DirExists(DataDir) and UsersCanWrite(DataDir) then
  begin
    LockDownDataDir(DataDir);
    if UsersCanWrite(DataDir) then
      Log('WARNING: could not remove write access for ordinary users on ' + DataDir)
    else
    begin
      Log('Fixed permissions on ' + DataDir + ': ordinary users had write access; now read-only.');
      if existed then
        SuppressibleMsgBox('The existing folder ' + DataDir + ' allowed ordinary users to write to it.' + #13#10 +
          'Because the Process Corral service runs as SYSTEM and reads its settings from there, Setup changed it so that only SYSTEM and Administrators can write (users can still read). Your files were not changed.',
          mbInformation, MB_OK, IDOK);
    end;
  end
  else if existed then
    Log('Existing data folder permissions are fine (ordinary users cannot write); left unchanged.');
end;

// ---------------------------------------------------------------- wizard

procedure InitializeWizard;
begin
  OtherPage := CreateOutputMsgPage(wpSelectDir,
    'Another process manager is running',
    'Another process manager is installed and active on this computer.',
    'Its service (' + OtherServiceName + ') is running or set to start automatically.' + #13#10#13#10 +
    'Process Corral and the other process manager both change the priority of busy programs. Running both means two' + #13#10 +
    'process managers will fight over the same processes, undoing each other''s changes.' + #13#10#13#10 +
    'On the "Select Additional Tasks" page you can choose to stop and disable that service.' + #13#10 +
    'It is off by default: Setup will not touch it unless you tick that box, and it never' + #13#10 +
    'uninstalls it. You can re-enable it later in the Services app.');
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := False;
  if (OtherPage <> nil) and (PageID = OtherPage.ID) then
    Result := not OtherManagerDetected;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  o: string;
  rc: Integer;
  app: string;
begin
  Result := '';
  ServiceWasRunning := False;
  app := ExpandConstant('{app}');
  QuitGui(app);
  if NoService then
  begin
    Log('/NOSERVICE: leaving the Windows service alone.');
    Exit;
  end;
  if not ServiceExists(ServiceName) then
  begin
    Log('Service ' + ServiceName + ' does not exist: this is a fresh service install.');
    Exit;
  end;
  if not ServiceIsInDir(ServiceName, app) then
  begin
    Log('Service ' + ServiceName + ' exists but its executable is NOT in ' + app + '. Leaving it completely alone.');
    Exit;
  end;
  ServiceWasRunning := ServiceRunning(ServiceName);
  Log('Existing service found in ' + app + '; running before upgrade: ' + IntToStr(Ord(ServiceWasRunning)));
  if ServiceWasRunning then
  begin
    // The engine's own stop restores every restrained process priority before it exits.
    rc := RunCapture(app + '\process-corral.exe', 'stop', o);
    Log('process-corral stop -> exit ' + IntToStr(rc) + ': ' + o);
    if ServiceRunning(ServiceName) then
      Result := 'The Process Corral service could not be stopped, so its files cannot be replaced.' + #13#10 +
        'Stop it from the Services app (or run "process-corral stop" as administrator) and run Setup again.';
  end;
end;

procedure StopAndDisableOther;
var
  o: string;
begin
  Log('Task selected: stopping and disabling the other process manager''s service ' + OtherServiceName);
  RunCapture(ExpandConstant('{sys}\sc.exe'), 'stop "' + OtherServiceName + '"', o);
  Log(o);
  RunCapture(ExpandConstant('{sys}\sc.exe'), 'config "' + OtherServiceName + '" start= disabled', o);
  Log(o);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  o, app: string;
  rc: Integer;
begin
  if CurStep <> ssPostInstall then Exit;
  app := ExpandConstant('{app}');

  if WizardIsTaskSelected('stopother') and OtherManagerDetected then StopAndDisableOther
  else if OtherManagerDetected then
    Log('The other process manager''s service ' + OtherServiceName + ' is active; NOT touching it (the task was not selected).');

  if NoService then
  begin
    Log('/NOSERVICE: skipping the ProgramData folder and the Windows service.');
    Exit;
  end;

  PrepareDataDir;

  if not ServiceExists(ServiceName) then
  begin
    rc := RunCapture(app + '\process-corral.exe', 'install', o);
    Log('process-corral install -> exit ' + IntToStr(rc) + ': ' + o);
    if rc = 0 then
    begin
      rc := RunCapture(app + '\process-corral.exe', 'start', o);
      Log('process-corral start -> exit ' + IntToStr(rc) + ': ' + o);
      if rc <> 0 then
        SuppressibleMsgBox('Process Corral was installed, but its service did not start.' + #13#10 + Trim(o) + #13#10#13#10 +
          'You can start it later from the Process Corral window or the Services app.', mbError, MB_OK, IDOK);
    end
    else
      SuppressibleMsgBox('Process Corral was installed, but the Windows service could not be registered.' + #13#10 + Trim(o),
        mbError, MB_OK, IDOK);
  end
  else if ServiceIsInDir(ServiceName, app) then
  begin
    if ServiceWasRunning then
    begin
      rc := RunCapture(app + '\process-corral.exe', 'start', o);
      Log('process-corral start (it was running before the upgrade) -> exit ' + IntToStr(rc) + ': ' + o);
      if rc <> 0 then
        SuppressibleMsgBox('Process Corral was updated, but its service did not start again.' + #13#10 + Trim(o), mbError, MB_OK, IDOK);
    end
    else
      Log('Service was not running before the upgrade: leaving it stopped.');
  end
  else
    Log('Service ' + ServiceName + ' belongs to another location; not touched.');
end;

// ---------------------------------------------------------------- uninstall

var
  DeleteData: Boolean;

function InitializeUninstall: Boolean;
begin
  Result := True;
  DeleteData := CmdFlag('DELETEDATA');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  o, app: string;
  rc: Integer;
begin
  app := ExpandConstant('{app}');
  if CurUninstallStep = usUninstall then
  begin
    QuitGui(app);
    if NoService then
      Log('/NOSERVICE: leaving the Windows service alone.')
    else if not ServiceExists(ServiceName) then
      Log('No service to remove.')
    else if not ServiceIsInDir(ServiceName, app) then
      Log('Service ' + ServiceName + ' is not in ' + app + '; leaving it alone.')
    else
    begin
      // The engine's own subcommand stops the service (restoring priorities) and removes it.
      rc := RunCapture(app + '\process-corral.exe', 'uninstall', o);
      Log('process-corral uninstall -> exit ' + IntToStr(rc) + ': ' + o);
    end;
  end
  else if CurUninstallStep = usPostUninstall then
  begin
    if (not DeleteData) and DirExists(DataDir) then
      DeleteData := (SuppressibleMsgBox('Also delete your Process Corral settings and logs?' + #13#10#13#10 + DataDir + #13#10#13#10 +
        'Choose No to keep them (for example to reinstall later).', mbConfirmation, MB_YESNO or MB_DEFBUTTON2, IDNO) = IDYES);
    if DeleteData and DirExists(DataDir) then
    begin
      Log('Deleting ' + DataDir);
      // Log rotation leaves corral.1.log .. corral.N.log (and corral.rotating.log after an interrupted
      // rotation) beside corral.log. Removing them by name first is belt and braces: the DelTree
      // below takes the whole folder anyway. Only reached with /DELETEDATA or a Yes answer, so
      // keeping the data keeps the rotated files too.
      DelTree(DataDir + '\logs\corral.*.log', False, True, False);
      DelTree(DataDir, True, True, True);
    end
    else
      Log('Keeping ' + DataDir);
  end;
end;
