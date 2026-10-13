# Drop-frame timecode

**When:** next, before the [start timecode choice](start-timecode-choice.md).

At 29.97 and 59.94, timecode is usually counted drop-frame (`;`), and "one hour" is a different frame each way: `01:00:00;00` is frame 107892, `01:00:00:00` frame 108000. vidcut reads drop-frame source timecode (spec D40), but shows and takes record timecode non-drop only: `project set --start-tc 01:00:00;00` is refused, and a start of frame 107892 shows as `00:59:56:12`.

- A project setting, as Resolve has: drop-frame timecode at 29.97 and 59.94, shown and typed with `;`.
- OTIO doesn't say whether a timeline is drop-frame: Resolve's export carries only the frame number, and its import took a drop-frame start as non-drop. An import into an empty project could set it from its media's own timecode (a drop-frame `tmcd` says so), and leave it alone otherwise.
