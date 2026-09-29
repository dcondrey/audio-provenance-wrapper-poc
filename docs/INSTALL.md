# Installing Audio Provenance Capture

Every plug-in format is a separate, optional component. Install only the formats your host
loads. The capture daemon and the login agent are separate components too.

| Format | macOS | Linux | Windows |
| --- | --- | --- | --- |
| VST3 | yes (default) | yes (default) | yes (default) |
| Audio Unit (AU) | yes (default) | no | no |
| CLAP | opt-in build | opt-in build | opt-in build |
| LV2 | opt-in build | built by default | opt-in build |
| VST2 | needs `APW_VST2_SDK_PATH` | needs SDK | needs SDK |
| AUv3 | needs Xcode generator and a container app | no | no |
| AAX | needs `APW_AAX_SDK_PATH` and PACE/iLok signing | no | needs SDK |
| LADSPA, DSSI | no | `-DAPW_BUILD_LADSPA=ON` / `-DAPW_BUILD_DSSI=ON` (targets `apw_ladspa`, `apw_dssi`) | no |

A format is only installable if the build produced it. Build flags: `APW_BUILD_CLAP`,
`APW_BUILD_LV2`, `APW_BUILD_AUV3`, `APW_VST2_SDK_PATH`, `APW_AAX_SDK_PATH` (see `CMakeLists.txt`).

## All platforms: `cmake --install` components

```
cmake --install <build> --config Release --component vst3 --prefix <dir>
cmake --install <build> --component clap --prefix <dir>
```

Components: `vst3 au clap lv2 vst2 auv3 aax ladspa dssi daemon`. A component whose format was
not configured installs nothing. Layout under the prefix: `lib/<format>/` on Linux,
`Library/Audio/Plug-Ins/...` on macOS, `<FORMAT>\` on Windows. `cmake --install` does not sign;
macOS releases go through the pkg below.

## macOS

### Building the pkg (maintainers)

```
APW_INSTALL_FORMATS=vst3,au,clap,lv2 scripts/package_installer.sh   # build, sign, stage
APW_INSTALL_FORMATS=vst3,au,clap,lv2 scripts/package_pkg.sh          # product archive
```

`APW_INSTALL_FORMATS` (default `vst3,au`) controls what is built into the pkg; the user then
chooses among those at install time. Use the same value for both scripts. `vst2` needs
`APW_VST2_SDK_PATH`, `aax` needs `APW_AAX_SDK_PATH`, and `auv3` is only packaged from an already
built container app in `AudioProvenanceCapture_artefacts/<config>/Standalone`.

`package_pkg.sh` builds a `productbuild` archive (`distribution.xml`) with one component package
per format plus `daemon`, `docs` and `loginagent`. It refuses to package any selected component
that is not Developer ID signed, signs the product, and notarizes it. `scripts/notarize.sh`
staples every bundle format before the pkg embeds it (LV2 is a plain directory and is covered
by notarizing the pkg).

For inspecting the layout without a Developer ID: `APW_PKG_DEV_UNSIGNED=1 scripts/package_pkg.sh`
writes `AudioProvenanceCapture-<version>-DEV-UNSIGNED.pkg` to a separate work directory, does
not sign or notarize, and is refused when `APW_RELEASE=1`. Never distribute it.

### Installing

Interactive: open the pkg and press Customize. Formats install to:

| Choice id | Installs | Location | Default |
| --- | --- | --- | --- |
| `vst3` | VST3 | `/Library/Audio/Plug-Ins/VST3` | on |
| `au` | Audio Unit | `/Library/Audio/Plug-Ins/Components` | on |
| `clap` | CLAP | `/Library/Audio/Plug-Ins/CLAP` | off |
| `lv2` | LV2 | `/Library/Audio/Plug-Ins/LV2` | off |
| `vst2` | VST2 | `/Library/Audio/Plug-Ins/VST` | off |
| `auv3` | AUv3 container app | `/Applications` | off |
| `aax` | AAX | `/Library/Application Support/Avid/Audio/Plug-Ins` | off |
| `daemon` | capture daemon and helper scripts | `/Library/Application Support/Audio Provenance Capture` | on |
| `docs` | demo project and QuickStart | same support directory | on |
| `loginagent` | starts the daemon at login | `/Library/LaunchAgents` | off, needs `daemon` |

`loginagent` needs `daemon`: the GUI greys it out without the daemon, and the command-line installer does not enforce that, so its postinstall skips the agent (and removes its plist) when the daemon is absent.

Choices exist only for what the pkg was built with; list them with
`installer -pkg <pkg> -showChoicesXML -target /`.

Non-interactive: give `installer` a choice-changes plist. Generate one that selects exactly the
listed ids and deselects the rest:

```
packaging/macos/make_choices.sh clap,daemon AudioProvenanceCapture-0.9.0.pkg > choices.xml
installer -pkg AudioProvenanceCapture-0.9.0.pkg -showChoicesAfterApplyingChangesXML choices.xml -target /   # preview, no install
sudo installer -pkg AudioProvenanceCapture-0.9.0.pkg -applyChoiceChangesXML choices.xml -target /
```

The optional pkg argument limits the plist to choices that pkg offers. Or write the plist by hand, one dict per choice:

```
<dict><key>choiceIdentifier</key><string>clap</string>
      <key>choiceAttribute</key><string>selected</string>
      <key>attributeSetting</key><integer>1</integer></dict>
```

Uninstall: delete the bundle from its location, and for the login agent run
`launchctl bootout gui/$UID/com.audioprovenance.capture.daemon` then remove
`/Library/LaunchAgents/com.audioprovenance.capture.daemon.plist`.

## Linux

`scripts/install_linux.sh` installs from a payload directory (`lib/<format>/...` plus
`SHA256SUMS`) and never touches a format you did not ask for.

```
cmake --install build --component vst3 --prefix payload     # repeat per format built
cmake --install build --component clap --prefix payload
scripts/install_linux.sh --payload payload --write-checksums

scripts/install_linux.sh --payload payload                                # VST3 to ~/.vst3
scripts/install_linux.sh --payload payload --formats vst3,clap,lv2 --user
sudo scripts/install_linux.sh --payload payload --formats vst3 --system   # /usr/lib/vst3
scripts/install_linux.sh --payload payload --formats clap --prefix /opt/apw
scripts/install_linux.sh --payload payload --formats vst3,clap --dry-run  # print, change nothing
scripts/install_linux.sh --uninstall --formats clap                       # only CLAP
scripts/install_linux.sh --uninstall                                      # everything recorded
```

Formats: `vst3 clap lv2 ladspa dssi` (default `vst3`). Destinations: `--user` (default) uses
`~/.vst3 ~/.clap ~/.lv2 ~/.ladspa ~/.dssi`, `--system` uses `/usr/lib/{vst3,clap,lv2,ladspa,dssi}`,
`--prefix DIR` uses `DIR/lib/<format>`. The script refuses a format the payload lacks, verifies
every file of each selected format against `SHA256SUMS` (and rejects unlisted extra files)
before copying, and records what it installed in a receipt so `--uninstall` removes exactly that.
`--no-verify` is refused unless `APW_ALLOW_UNVERIFIED=1`. The Linux installer does not install the
capture daemon.

## Windows

Build the installer with Inno Setup, see `packaging/windows/README.md`. Components: `vst3 clap
lv2 vst2 aax daemon`.

```
Setup.exe                                                     interactive, pick formats
Setup.exe /VERYSILENT /NORESTART                              default type (VST3 and daemon)
Setup.exe /VERYSILENT /NORESTART /COMPONENTS="vst3,clap,lv2"  chosen formats
Setup.exe /VERYSILENT /NORESTART /CURRENTUSER                 per user, no admin
```

Destinations: `Common Files\VST3`, `Common Files\CLAP`, `Common Files\LV2`,
`Steinberg\VSTPlugins`, `Common Files\Avid\Audio\Plug-Ins`. Uninstall from Apps and Features or
run `unins000.exe /VERYSILENT` in the install directory.

The Inno script has not been compiled or run; see the Windows README.
