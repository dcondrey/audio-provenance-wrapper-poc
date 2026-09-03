# Installer demo assets

| Path | What it is |
| --- | --- |
| `quickstart.pdf` | One-page quickstart. Regenerate with `scripts/make_quickstart_pdf.py`. |
| `demo-root-ca.pem` | Trust anchor for the demo identity, byte-identical to `demo-provenance-store/ca/root_cert.pem`. Load it as a verifier trust anchor or a good signature reads as untrusted. |
| `demo-provenance-store/` | The key-custody store the capture daemon signs with (`--provenance-store`). Holds the demo root key and certificate, the issued leaf, and the soft-binding mark key. |
| `demo-project/apw-demo Project/` | Ableton project for the demo clips. |

`scripts/package_installer.sh` creates the store if it is absent, rewrites `demo-root-ca.pem`
from that store's own root, and refuses to build a DMG unless the two fingerprints match. The
anchor and the material the daemon signs with therefore cannot drift apart.

The demo identity is **self-issued and not a verified identity**. A trusted result proves only
that the bytes match a signature made by this demo key. It attests to no person or organisation.
The root private key is shipped in the clear on purpose so the demo can sign; never reuse it.

## Starting the daemon

The plug-in only emits UDP datagrams; it never launches anything. `Start Capture Daemon.command`
at the root of the disk image runs the frozen daemon from inside the installed plug-in bundle
(`Contents/Resources/apw-daemon/`), copies this store to `~/.apw/demo-provenance-store` on first
run so it is writable, and points the daemon at it with `--provenance-store`. With no daemon
running, every observation the plug-in sends is dropped and no manifest is ever produced.

Both `.command` files are shell scripts, and a shell script can carry no notarization
ticket. On a machine that downloaded this image they are quarantined, so the first
double-click is refused. Right-click the file and choose **Open**, then confirm; macOS
remembers the choice. The plug-ins themselves are notarized and need no such step, and
`Install Plug-Ins.command` clears their quarantine flag as it copies them.

## Demo project

Two 8.000 s 16-bit PCM 44.1 kHz stereo clips on the arrangement track, referenced
project-relative from `Samples/Imported/`. Their PCM `data` chunks are byte-identical
(SHA-256 `5f2545393ffb5603...`); the only difference is the appended manifest.

- `apw-demo-signed.wav` carries an embedded C2PA manifest in a top-level `C2PA` RIFF
  chunk appended at the original file length, with a `c2pa.hash.data` exclusion covering
  exactly that appended region. `package_installer.sh` re-signs it from the shipped store
  on every build and fails if the result does not verify against the shipped anchor.
- `apw-demo-unsigned.wav` carries none. That is an absence of evidence and never evidence
  of synthetic origin.

The set is **sample-only by design**. It saves no plug-in device, and one must not be added.
Coverage grading requires the plug-in's cumulative `windows_hashed` to equal the daemon's
received chain length, so the plug-in has to be inserted by hand *after* the capture session
starts; a device restored at set-open time is already ahead and grades every later session
`partial_observed_path`. A saved device also renders as a greyed-out placeholder on any
machine where the plug-in is absent or rebuilt under a different UID.

`OriginalFileSize` in the `.als` tracks the signed clip's current byte length (1424870).
Live only consults it for name-plus-size search when path resolution fails, and both the
absolute `Path` and the `RelativePath` resolve, so a mismatch is not fatal. It does go stale
whenever `package_installer.sh` re-signs the clip to a different length. Re-save the set from
Live, or patch the two `OriginalFileSize` values in the gzipped XML, if it drifts.

The set was patched on 2026-08-31 and re-gzipped with `gzip -9 -n`. The gunzip round-trip is
byte-identical, the XML parses, and all four sample references resolve, but *that Live 12.4.5
opens the re-gzipped file has not been verified*. `apw-demo.als.orig` beside it is the exact
pre-patch bytes (SHA-256 `6622fb39b62d9b41...`); restore with
`cat "apw-demo.als.orig" > "apw-demo.als"` if Live refuses the set.

Running **Collect All and Save** in Live rewrites the samples into the project and the signed
clip silently loses its manifest. Do not run it on this project before a demo.

## Trust anchor scope

Three different roots exist and using the wrong one reads as a failure that is not one.

| Asset | Anchor |
| --- | --- |
| The two shipped clips above | `demo-root-ca.pem` (this folder) |
| Anything a repo session produces via `scripts/demo.sh` | `~/.apw/provenance/ca/root_cert.pem` |
| Anything the installed DMG daemon produces | `~/.apw/demo-provenance-store/ca/root_cert.pem` |

## Verified 2026-08-31

Observed with `c2patool 0.26.68` and c2pa-rs `0.90.15`:

```
apw-demo-signed.wav       verified                       / Trusted / []
apw-demo-unsigned.wav     nothing_found                  / None    / []
signed, foreign anchor    mark_found_claim_not_trusted   / Valid   / ['signingCredential.untrusted']
signed, one PCM byte flipped   registered_but_changed    / Invalid / ['assertion.dataHash.mismatch']
```

`c2patool ... trust --trust_anchors demo-root-ca.pem` on the signed clip reports
`"validation_state": "Trusted"` with zero failures, ES256, common name
`APW Demo Signer (not a verified identity)`. The `.als` decompresses to XML that parses,
every sample reference is project-relative and resolves inside the project folder, and no
plug-in device node is present, as intended. `docs/DEMO_RUNBOOK.md` carries the exact
commands.
