; Audio Provenance Capture - Windows installer (Inno Setup 6.3+).
;
; Build:   iscc /DPayload=C:\path\to\payload /DAppVersion=0.9.0 installer.iss
; Payload: the tree `cmake --install <build> --prefix <payload>` writes on Windows:
;            <payload>\VST3\Audio Provenance Capture.vst3\...
;            <payload>\CLAP\Audio Provenance Capture.clap
;            <payload>\LV2\Audio Provenance Capture.lv2\...
;            <payload>\VST2\...   <payload>\AAX\...            (opt-in builds only)
;            <payload>\daemon\...                               (frozen daemon, optional)
; A component is compiled in only when its directory exists in the payload, so the installer
; never offers a format the build did not produce.
;
; Silent install, choose formats:
;   AudioProvenanceCapture-Setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /COMPONENTS="vst3,clap"
; Component ids: vst3 clap lv2 vst2 aax daemon
; Per-user (no admin):  add /CURRENTUSER    All users (admin): /ALLUSERS

#ifndef Payload
  #error Pass /DPayload=<dir> (the cmake --install prefix for Windows)
#endif
#ifndef AppVersion
  #define AppVersion "0.9.0"
#endif

#define HasVST3   DirExists(AddBackslash(Payload) + "VST3")
#define HasCLAP   DirExists(AddBackslash(Payload) + "CLAP")
#define HasLV2    DirExists(AddBackslash(Payload) + "LV2")
#define HasVST2   DirExists(AddBackslash(Payload) + "VST2")
#define HasAAX    DirExists(AddBackslash(Payload) + "AAX")
#define HasDaemon DirExists(AddBackslash(Payload) + "daemon")

[Setup]
AppId={{6B1F3C52-8E3E-4B49-9A57-2C7D0AB5C0DE}
AppName=Audio Provenance Capture
AppVersion={#AppVersion}
AppPublisher=Audio Provenance
DefaultDirName={autopf}\Audio Provenance Capture
DisableDirPage=yes
DefaultGroupName=Audio Provenance Capture
DisableProgramGroupPage=yes
OutputBaseFilename=AudioProvenanceCapture-Setup-{#AppVersion}
OutputDir=Output
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=dialog commandline
UninstallDisplayName=Audio Provenance Capture
; A component selection is remembered, so an upgrade keeps the formats already chosen.
UsePreviousSetupType=yes
; Sign the installer and uninstaller in the release pipeline: SignTool=<name> (see README).

[Types]
Name: "default"; Description: "Recommended (VST3)"
Name: "full"; Description: "Every format in this installer"
Name: "custom"; Description: "Choose formats"; Flags: iscustom

[Components]
#if HasVST3
Name: "vst3"; Description: "VST3 plug-in (Common Files\VST3)"; Types: default full custom
#endif
#if HasCLAP
Name: "clap"; Description: "CLAP plug-in (Common Files\CLAP)"; Types: full custom
#endif
#if HasLV2
Name: "lv2"; Description: "LV2 plug-in (Common Files\LV2)"; Types: full custom
#endif
#if HasVST2
Name: "vst2"; Description: "VST2 plug-in, legacy (Steinberg\VSTPlugins)"; Types: full custom
#endif
#if HasAAX
Name: "aax"; Description: "AAX plug-in for Pro Tools (Common Files\Avid\Audio\Plug-Ins)"; Types: full custom
#endif
#if HasDaemon
Name: "daemon"; Description: "Capture daemon (required for the plug-ins to record)"; Types: default full custom
#endif

[Files]
; {autocf64}/{autopf64} resolve to Program Files\Common Files for an all-users install and to
; the user's Programs\Common tree for a per-user install.
#if HasVST3
Source: "{#Payload}\VST3\*"; DestDir: "{autocf64}\VST3"; Components: vst3; Flags: recursesubdirs createallsubdirs ignoreversion
#endif
#if HasCLAP
Source: "{#Payload}\CLAP\*"; DestDir: "{autocf64}\CLAP"; Components: clap; Flags: recursesubdirs createallsubdirs ignoreversion
#endif
#if HasLV2
Source: "{#Payload}\LV2\*"; DestDir: "{autocf64}\LV2"; Components: lv2; Flags: recursesubdirs createallsubdirs ignoreversion
#endif
#if HasVST2
Source: "{#Payload}\VST2\*"; DestDir: "{autopf64}\Steinberg\VSTPlugins"; Components: vst2; Flags: recursesubdirs createallsubdirs ignoreversion
#endif
#if HasAAX
Source: "{#Payload}\AAX\*"; DestDir: "{autocf64}\Avid\Audio\Plug-Ins"; Components: aax; Flags: recursesubdirs createallsubdirs ignoreversion
#endif
#if HasDaemon
Source: "{#Payload}\daemon\*"; DestDir: "{app}\apw-daemon"; Components: daemon; Flags: recursesubdirs createallsubdirs ignoreversion
#endif
