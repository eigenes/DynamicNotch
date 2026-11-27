; Inno Setup 6.3+ script for Dynamic Notch.
;
; One setup.exe for both architectures: it installs the ARM64 build on Windows
; on ARM and the x64 build everywhere else. Installs per user into
; %LOCALAPPDATA%\Programs\Dynamic Notch, so no admin rights are needed.
;
; The release workflow builds it with:
;   iscc /DAppVersion=0.2.0 /DX64Exe=<path> /DArm64Exe=<path> /DOutputDir=<dir> installer\dynamic-notch.iss
; Locally, after building both targets, plain `iscc installer\dynamic-notch.iss` works.

#ifndef AppVersion
  #define AppVersion "0.0.0-dev"
#endif
#ifndef X64Exe
  #define X64Exe "..\target\x86_64-pc-windows-msvc\release\dynamic-notch.exe"
#endif
#ifndef Arm64Exe
  #define Arm64Exe "..\target\aarch64-pc-windows-msvc\release\dynamic-notch.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\dist"
#endif

#define AppName "Dynamic Notch"
#define AppExe "dynamic-notch.exe"
#define AppIcon "dynamic-notch.ico"
; Same file build.rs embeds in the exe
#define AppIconSrc "..\assets\" + AppIcon
#define RepoUrl "https://github.com/eigenes/DynamicNotch"

[Setup]
; Never change AppId: it is how Windows recognises an existing install to update.
AppId={{015FB9B0-067B-4DE0-B6DC-68FDA44B92B6}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=eigenes
AppPublisherURL={#RepoUrl}
AppSupportURL={#RepoUrl}/issues
AppUpdatesURL={#RepoUrl}/releases
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline
ArchitecturesAllowed=x64compatible or arm64
ArchitecturesInstallIn64BitMode=x64compatible or arm64
; Windows 10 1903, the same minimum as the app
MinVersion=10.0.18362
OutputDir={#OutputDir}
OutputBaseFilename=dynamic-notch-v{#AppVersion}-windows-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Icon of setup.exe itself; the same file is installed for shortcuts and Apps & features.
SetupIconFile={#AppIconSrc}
UninstallDisplayName={#AppName}
UninstallDisplayIcon={app}\{#AppIcon}
; The running notch is closed in [Code] below; this catches anything else.
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "german"; MessagesFile: "compiler:Languages\German.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#X64Exe}"; DestDir: "{app}"; DestName: "{#AppExe}"; Check: not IsArm64; Flags: ignoreversion
Source: "{#Arm64Exe}"; DestDir: "{app}"; DestName: "{#AppExe}"; Check: IsArm64; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#AppIconSrc}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE";DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\.env.example"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\{#AppIcon}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\{#AppIcon}"; Tasks: desktopicon

[Run]
; Interactive install: offer to start it. Silent update (e.g. winget): start it
; again only if it was running before.
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
Filename: "{app}\{#AppExe}"; Flags: nowait skipifnotsilent; Check: WasRunning

[Code]
const
  NotchClass = 'DynamicNotch.Window';
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  RunValue = 'DynamicNotch';
  WM_CLOSE = $0010;

var
  NotchWasRunning: Boolean;

function WasRunning: Boolean;
begin
  Result := NotchWasRunning;
end;

{ Ask a running notch to quit (WM_CLOSE runs its normal shutdown) and wait
  up to five seconds for it to go. Returns True if one was running. }
function CloseRunningNotch: Boolean;
var
  Wnd: HWND;
  I: Integer;
begin
  Wnd := FindWindowByClassName(NotchClass);
  Result := Wnd <> 0;
  if not Result then
    Exit;
  PostMessage(Wnd, WM_CLOSE, 0, 0);
  for I := 1 to 50 do
  begin
    Sleep(100);
    if FindWindowByClassName(NotchClass) = 0 then
      Exit;
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  NotchWasRunning := CloseRunningNotch;
  Result := '';
end;

function InitializeUninstall: Boolean;
begin
  CloseRunningNotch;
  Result := True;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Cmd: String;
begin
  { Drop "Start with Windows" only if it points at this install, not at a
    portable copy elsewhere. Settings in %APPDATA%\DynamicNotch are kept. }
  if CurUninstallStep = usPostUninstall then
    if RegQueryStringValue(HKCU, RunKey, RunValue, Cmd) then
      if Pos(Lowercase(ExpandConstant('{app}')), Lowercase(Cmd)) > 0 then
        RegDeleteValue(HKCU, RunKey, RunValue);
end;
