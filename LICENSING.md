# Licensing

This repository is **not** under a single licence, and there is deliberately no
LICENSE file at its root. Different parts of the tree have different authorship
and different third-party constraints, and a root licence would assert rights
over code this repository cannot grant.

Read this file before copying, publishing, or distributing anything here.

## Summary

| Path | Licence | Why |
|---|---|---|
| `rust/` | **Apache-2.0** (`rust/LICENSE`) | Sole authorship. No copyleft dependency. Does not link JUCE. |
| `sdk/` | **Apache-2.0** (`sdk/LICENSE`) | Sole authorship. No copyleft dependency. Does not link JUCE. |
| `src/` | **All rights reserved.** Not licensed. | Jointly authored, and links JUCE, which is AGPLv3-or-commercial. |
| `daemon/` | **All rights reserved.** Not licensed. | Jointly authored. |
| `src/ladspa/ladspa.h`, `src/dssi/dssi.h`, `src/dssi/compat/` | **LGPL-2.1-or-later** (`src/ladspa/LICENSE.LGPL-2.1`) | Vendored unmodified from the LADSPA 1.17 and DSSI 1.1.1 SDKs; declarations only. Sources and checksums in `docs/LADSPA_DSSI.md`. |
| `tests/fixtures/parity/ots/` (except `SOURCES.json`) | **LGPL-3.0-or-later** (`LICENSE.third-party` there) | Unmodified OpenTimestamps client example proofs used as test inputs; not linked into any binary. |
| `tests/fixtures/projects/milkytracker/test.{xm,mod}` | **BSD-3-Clause** (`LICENSE.third-party` there) | Unmodified test modules from the OpenMPT project (`test/`, commit `f83cedb`); test inputs only. |
| `requirements/`, `docs/`, `tests/`, `CMakeLists.txt`, `AGENTS.md`, `README.md` | **All rights reserved.** Not licensed. | Jointly authored. |

## The Apache-2.0 parts

`rust/` and `sdk/` are the Rust workspaces: the provenance core, the C2PA and
manifest engines, the watermark and trace crates, the registry and trust layers,
the CLI, and the WASM/JS verification SDK. Every line in both trees was written
by David Condrey. Neither links JUCE.

Apache-2.0 was chosen over MIT deliberately, for its express patent grant in
section 3. A provenance and watermarking implementation is not something a
standards body or a prospective adopter's counsel will build on without one.

Third-party dependency obligations are recorded in `sdk/NOTICE` and
`rust/NOTICE`. The one that matters: `sdk/` depends on **symphonia**, which is
**MPL-2.0** and is in the default build graph, not behind an optional feature.
MPL-2.0 is file-level copyleft, compatible with Apache-2.0 in a combined work,
and imposes no licence obligation on this repository's own files — but it does
require that recipients of a distributed binary be told where to obtain the
source of the MPL-licensed files. `sdk/NOTICE` does that.

## The parts that are not licensed

`src/`, `daemon/`, and the requirements and documentation trees are **jointly
authored** by David Condrey and Jeremy Uzan. Neither this repository nor the
upstream it was forked from has ever carried a licence, so the default applies:
all rights reserved, held jointly. Neither author can license the whole of it
alone.

As measured on 2026-09-09, roughly 1,547 lines across 23 files remain
attributable to Jeremy Uzan, concentrated in `src/PluginProcessor.{cpp,h}`,
`src/PluginEditor.{cpp,h}`, `CMakeLists.txt`, `requirements/*.md` and several
files under `docs/`. `daemon/sample_watcher/` was independently reimplemented on
2026-09-09; what remains attributed there is interface — schema keys, function
signatures, constants — that cannot change without breaking callers and
orphaning evidence records already written to disk.

Licensing these parts requires a written grant from Jeremy Uzan naming a
specific licence. Until then they may not be distributed under any open licence.

## JUCE

The VST3/AU capture plugin at `src/` links JUCE, pinned at 8.0.15 and fetched at
build time by `CMakeLists.txt`. **JUCE is dual-licensed AGPLv3 or commercial. It
is not permissively licensed.** Any distributed binary that links it must either
carry AGPLv3 terms or be covered by a commercial JUCE licence.

The coupling is currently confined. `CMakeLists.txt` links only JUCE modules
into the plugin target, and nothing links the Rust engine into it — `apw-ffi`
builds a `staticlib` intended for exactly that, but no build wires it up today.
The moment it is wired, the Rust code becomes part of a work AGPLv3 would govern
on distribution. Decide the JUCE licence before that happens, not after.

JUCE's own bundled tree also carries the Avid AAX SDK and the Steinberg ASIO
SDK, both proprietary-or-GPLv3. Neither is enabled by this build.

## What would change this

- **A contributor licence grant from Jeremy Uzan.** Unblocks `src/`, `daemon/`,
  `requirements/` and `docs/`, subject to JUCE for the plugin specifically.
- **A commercial JUCE licence**, or a decision to ship the plugin under AGPLv3.
  Unblocks distribution of the plugin binary.

Neither is required for `rust/` or `sdk/`, which stand on their own.

## Publication status

Every crate in both workspaces sets `publish = false`, and the npm package sets
`"private": true`. Nothing here is published to crates.io or npm. Those flags are
deliberate and should not be removed without a decision to publish.
