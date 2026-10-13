# Choosing the start timecode

**When:** next.

The start timecode (deviation X7: `00:00:00:00`) can be changed only with `project set --start-tc`. Offer it where a project is made and set:

- **New Project** and **File > Project Settings:** `00:00:00:00` (the default), `01:00:00:00`, `10:00:00:00`, or a custom one; and `vidcut project create --start-tc`.
- **Why these:** `00:00:00:00` is Premiere's and Final Cut's default; `01:00:00:00` Resolve's and Avid's, and where US broadcast programmes start; `10:00:00:00` where UK ones start (DPP, AS-11). Leader comes before the programme start (bars and tone from about `00:58:30:00`, the 2-pop at `00:59:58:00`), not a start of its own. Feature reels start at their number (reel 2 at `02:00:00:00`).
- The choices are labeled by their values alone: vidcut's text names no other program (AGENTS.md rule 2).
- At 29.97 and 59.94 drop-frame, the presets are `01:00:00;00` and so on.
- OTIO carries it both ways (`global_start_time`; Resolve starts its timeline there), and an import into an empty project already takes the file's.
