# OTIO checks in CI

**When:** next.

`tests/cli.rs` checks vidcut's files against OpenTimelineIO's own Python library, but only in the full gate (`scripts/check.sh full`) with the library installed (`pip install opentimelineio`; `VIDCUT_OTIO_PYTHON` names the Python), and CI runs the fast gate. It had never run on Windows before 2026-10-13, where it found the library taking a drive letter for a URL scheme. A CI step could install the library and run just these tests (`cargo test -p vidcut --test cli -- otio stills_and_sequences`) on Linux and Windows.
