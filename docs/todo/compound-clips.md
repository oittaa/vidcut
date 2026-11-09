# Compound clips: the rest

**When:** not decided.

`edit.compound` makes a compound clip from timeline clips ([spec §8.18](../cut-page-spec.md#818-compound-clips-pp-10521054-1126-measured-in-resolve-studio-2120), [deviation X163](../deviations.md#x163-a-compound-clip-takes-its-clips-whole)). These parts are still open.

**Why:**

- vidcut refuses a smooth cut at an edge of a compound clip. Its interpolation needs each side as one flat picture.

**To do:**

- Let an edge of a compound clip take a smooth cut.
