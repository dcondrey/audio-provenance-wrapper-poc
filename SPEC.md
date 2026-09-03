# Audio Provenance - Build Spec (superseded)

**This spec is superseded and kept only as a pointer.** It described the build-out
from the original scaffolded state; everything it listed under "What Needs to Be
Built" now exists, including the unified daemon (`python3 -m daemon`), export
watching, session management, manifest generation on export, the software key
provider, and end-to-end integration tests.

For the current state, read:

- [docs/ROADMAP.md](docs/ROADMAP.md) - project goal, architecture, and what remains
- [docs/VALIDATION.md](docs/VALIDATION.md) - milestone validation status

Test coverage is whatever the live suite under `tests/` reports
(`python -m pytest tests/`); any count written into a document goes stale.

The original spec text is in git history
(`git log --follow -p -- SPEC.md`).
