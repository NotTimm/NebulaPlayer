; Inno Setup script for the Windows installer. Built by
; packaging/build-windows.ps1, which passes:
;   /DAppVersion=0.1.0-alpha.1  the full version (shown to users)
;   /DNumericVersion=0.1.0      the X.Y.Z part (Windows file version)
;   /DStageDir=...              folder with nebula-player.exe and libmpv-2.dll
;   /DOutputDir=...             where the setup .exe goes
;
; Installs per user by default (no admin prompt) into
; %LOCALAPPDATA%\Programs\Nebula Player; the setup can still be run for all
; users from an elevated prompt. Supports silent installs
; (/VERYSILENT /SUPPRESSMSGBOXES /NORESTART, plus /CURRENTUSER or /ALLUSERS).

#define AppName "Nebula Player"
#define AppExe "nebula-player.exe"

[Setup]
; Never change AppId: it's how upgrades find the existing install.
AppId={{C9C94BBB-E2BE-4992-B279-3491E41C2F61}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Tim Singel
AppPublisherURL=https://github.com/NotTimm/NebulaPlayer
AppSupportURL=https://github.com/NotTimm/NebulaPlayer/issues
AppUpdatesURL=https://github.com/NotTimm/NebulaPlayer/releases
VersionInfoVersion={#NumericVersion}
VersionInfoProductVersion={#NumericVersion}
VersionInfoProductTextVersion={#AppVersion}
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=commandline dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\..\LICENSE
SetupIconFile=..\..\res\icons\io.github.NotTimm.NebulaPlayer.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir={#OutputDir}
OutputBaseFilename=NebulaPlayer-{#AppVersion}-windows-x64-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Close a running copy before upgrading files, and reopen it afterwards.
; "force" ends it when it doesn't close on Windows' request (winit apps
; ignore the shutdown message), which a silent install otherwise answers
; by cancelling. Jellyfin progress is reported as you watch, so little is
; lost.
CloseApplications=force
RestartApplications=yes
ChangesAssociations=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#StageDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; Offer Nebula Player under "Open with" for videos without taking over the
; default app for those file types.
Root: HKA; Subkey: "Software\Classes\NebulaPlayer.Video"; ValueType: string; ValueName: ""; ValueData: "Video"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\NebulaPlayer.Video\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: "{app}\{#AppExe},0"
Root: HKA; Subkey: "Software\Classes\NebulaPlayer.Video\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\{#AppExe}"" ""%1"""
Root: HKA; Subkey: "Software\Classes\.mkv\OpenWithProgids"; ValueType: string; ValueName: "NebulaPlayer.Video"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.mp4\OpenWithProgids"; ValueType: string; ValueName: "NebulaPlayer.Video"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.m4v\OpenWithProgids"; ValueType: string; ValueName: "NebulaPlayer.Video"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.webm\OpenWithProgids"; ValueType: string; ValueName: "NebulaPlayer.Video"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.avi\OpenWithProgids"; ValueType: string; ValueName: "NebulaPlayer.Video"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.mov\OpenWithProgids"; ValueType: string; ValueName: "NebulaPlayer.Video"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.wmv\OpenWithProgids"; ValueType: string; ValueName: "NebulaPlayer.Video"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.ts\OpenWithProgids"; ValueType: string; ValueName: "NebulaPlayer.Video"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.m2ts\OpenWithProgids"; ValueType: string; ValueName: "NebulaPlayer.Video"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.mpg\OpenWithProgids"; ValueType: string; ValueName: "NebulaPlayer.Video"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\.flv\OpenWithProgids"; ValueType: string; ValueName: "NebulaPlayer.Video"; ValueData: ""; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent
