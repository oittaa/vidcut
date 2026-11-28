# Grade versions

**When:** not decided. The Gallery steps of [color-gallery.md](color-gallery.md) do not wait for it.

Each grade of a clip is a version. A clip can have many versions, and one of them applies at a time (p. 3351).

vidcut has one grade for each clip. Thus the ID of a still uses version 1 ([color-gallery.md](color-gallery.md)).

**What the manual says** (DaVinci Resolve 21.1):

- The first grade of each clip is a local version named "Version 1" (p. 3351).
- The menu of a thumbnail lists the versions under Local Versions and Remote Versions (p. 3351).
- A local version belongs to its clip on its timeline. A change has no effect on other clips (pp. 3351–3352).
- The clips with remote versions that come from the same media share their grades, in all timelines (pp. 3351–3353).
- The project setting "Use local version for new clips in timeline" is on by default. Off, new clips get remote versions (p. 3352).
- The timeline filter Common Media Pool Clips shows the clips that are linked to the current clip (p. 3353).
- Color > Grade Version > Add (Command-Y) adds a version of the same kind, named "Version x". It copies the keyframes (p. 3357).
- Create New Version asks for a name and for "Copy all marks" or "Copy the first mark only" (p. 3357).
- The Versions section of the Project Settings has ten preset names (p. 3358).
- The menu of a version has Load, Rename, Delete and Copy to Local. The current version cannot be deleted (pp. 3356, 3358).
- Delete All Versions and Delete Unused Versions keep only the current version (pp. 3358, 3360).
- Use Local Grades, Use Remote Grades, Copy Remote Grades to Local and Copy Local Grades to Remote act on all clips of the timeline (pp. 3356–3357).
  - The manual says that Use Local Grades and Copy Remote Grades to Local have no undo (p. 3356).
- Color > Grade Version > Default (Command-U) loads Version 1 (p. 3359).
- A switch of a timeline between local and remote versions loads Version 1 of each clip (p. 3359).
- The Deliver page renders the current version of each clip (p. 3360).
  - Render Disabled and Enable Flat Pass act in the Commercial Workflow output.
- A group can have versions of its Pre-Clip and Post-Clip grades. All clips of the group use the same group version (p. 3382).
- The split screens Versions and Versions and Original show up to 16 versions of the current clip (p. 3090).

**To do:**

- Plan the build steps and the Resolve runs, as [color-gallery.md](color-gallery.md) does.
