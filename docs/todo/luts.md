# LUTs

**When:** not decided.

vidcut has an input LUT of a media item and of the project, and an output LUT. It also has a Video Monitor LUT and a Color Viewer LUT ([design](../design/README.md#lut-files)).

**Why:**

- The measured facts are in [spec §13](../spec.md#the-lut-tables).

**In the code:**

- The scopes read the canvas before the display LUT (`Screen::set_display_lut`).

**To do:**

- The Scopes LUT of the project (p. 152). It makes the scopes use the Video Monitor LUT or another LUT.
  - Why: the scopes then show the picture as the monitor shows it.
  - First, measure its dropdown in Resolve: the entries and the default.
- A LUT on the node of a title, when titles take a grade.

**Later:**

- File types other than `.cube`, when a real file needs one.
- OpenColorIO, after the extra color spaces exist ([timeline-color-spaces-more.md](timeline-color-spaces-more.md)).
