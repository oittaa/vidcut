# Proxy generation as a background job

**When:** next after the interchange. **In progress:** pull request #26.

Playback proxies are in (decision D41, deviation X18). The preview plays a fresh proxy; export, frames, analysis and OTIO stay on the original.

Generating one still runs inside the edit. That freezes the GUI's core thread and, headless, holds the project lock for the whole encode. It needs to run as a background job, with progress and cancel, like an export.
