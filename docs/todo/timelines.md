# Several timelines in one project

**When:** not decided.

**Why:** a project holds many timelines (p. 678). A vidcut project file holds one timeline ([deviation X2](../deviations.md#x2-one-timeline-per-project-file)).

**In the code:**

- `Project` holds the one timeline: `tracks`, `marks`, `markers` and `subtitles`.
- `Settings` holds the frame rate, the size and the start timecode of that timeline (`fps`, `width`, `height`, `start_tc`).
- `Project` also holds the parts that the timelines of a project share: `media`, `bins`, `compounds` and `groups`.

**What the manual tells:**

- The Media Pool stores the timelines. Bins hold them as they hold clips (p. 678).
- The Master bin of the Cut page lists all timelines of the project (p. 655).
- File > New Timeline (Ctrl+N) and Create New Timeline in the Media Pool open a dialog (p. 678). The dialog has these settings:
  - the start timecode, 01:00:00:00 by default;
  - the name;
  - the number of video tracks and audio tracks, and the kind of audio, stereo by default;
  - an empty timeline, or a timeline with all clips of the Media Pool.
- A timeline uses the frame rate, the resolution and the monitoring settings of the project. Use Custom Settings gives a timeline its own (p. 678).
- A double-click on a timeline in the Media Pool opens it (p. 678).
- The Timeline Selection menu at the top of the Cut page viewer changes the open timeline (p. 678).

**To do:**

- Plan the model: the fields of `Project` and `Settings` that move into a timeline.
- The requests that make, open, rename and delete a timeline, in the CLI, IPC and MCP.
- The pool item of a timeline, the New Timeline dialog and the Timeline Selection menu.
- Measure in Resolve what the manual does not tell, with a plan that the owner accepts first.
