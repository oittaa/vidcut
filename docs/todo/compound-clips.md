# Compound clips: the Media Pool

**When:** not decided.

`edit.compound` makes a compound clip from timeline clips ([spec §8.18](../cut-page-spec.md#818-compound-clips-pp-10521054-1126-measured-in-resolve-studio-2120), [deviation X163](../deviations.md#x163-a-compound-clip-takes-its-clips-whole)). These parts are still open.

**Why:**

- A compound clip in the Media Pool can go on the timeline many times.
- A dissolve inside a compound clip should play at each length and speed.

**To do:**

- Build Create Compound Clip, as spec §8.18 tells. It needs a Media Pool item that holds a compound clip. The pool holds only files now.
- Make a dissolve play where an end of the compound clip cuts through it, and at speed sections.
