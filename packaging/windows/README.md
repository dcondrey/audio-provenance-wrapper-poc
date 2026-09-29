# Windows installer

`installer.iss` builds a single `AudioProvenanceCapture-Setup-<version>.exe` (Inno Setup 6.3 or
newer) in which every plug-in format is its own selectable component.

## Build

1. Build the plug-ins on Windows with the formats you want to ship, for example:

   ```
   cmake -S . -B build -A x64 -DAPW_BUILD_CLAP=ON -DAPW_BUILD_LV2=ON
   cmake --build build --config Release
   ```

2. Stage the payload with the per-format install components (see `cmake/install.cmake`):

   ```
   cmake --install build --config Release --component vst3 --prefix payload
   cmake --install build --config Release --component clap --prefix payload
   cmake --install build --config Release --component lv2  --prefix payload
   ```

   Install only the components you built. On Windows this produces `payload\VST3`,
   `payload\CLAP`, `payload\LV2`, `payload\VST2`, `payload\AAX`. To include the frozen daemon,
   copy it to `payload\daemon` (the PyInstaller output of the daemon build).

3. Compile the installer (a component appears only when its directory exists in the payload):

   ```
   iscc /DPayload=%CD%\payload /DAppVersion=0.9.0 packaging\windows\installer.iss
   ```

   The result is `packaging\windows\Output\AudioProvenanceCapture-Setup-0.9.0.exe`.

4. Sign it. Add `SignTool=<name>` under `[Setup]` and define the tool in the Inno IDE or pass
   `/S<name>=<command>` to `iscc`; an unsigned installer triggers SmartScreen.

## Install and uninstall

| Goal | Command |
| --- | --- |
| Interactive | run the exe, pick the "Choose formats" type |
| Default (VST3) silent | `Setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART` |
| Chosen formats silent | `Setup.exe /VERYSILENT /NORESTART /COMPONENTS="vst3,clap"` |
| Per user, no admin | add `/CURRENTUSER` |
| Uninstall | Apps and Features, or `"%ProgramFiles%\Audio Provenance Capture\unins000.exe" /VERYSILENT` |

Component ids: `vst3`, `clap`, `lv2`, `vst2`, `aax`, `daemon`.
With `/COMPONENTS=` the listed ids are the only ones installed. Ids the installer does not
contain make Inno reject the command line.

Destinations: VST3 `Common Files\VST3`, CLAP `Common Files\CLAP`, LV2 `Common Files\LV2`,
VST2 `Program Files\Steinberg\VSTPlugins`, AAX `Common Files\Avid\Audio\Plug-Ins`, daemon
`{app}\apw-daemon`. A per-user install uses the equivalent folders under the user's profile.

## Not verified

This script has not been compiled: Inno Setup runs only on Windows and no Windows machine was
available. Compile it once and run the silent-install matrix above before shipping.
