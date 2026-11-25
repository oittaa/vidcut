# The clip filter

**When:** after the Resolve runs that the points below need.

The Color page filters its Thumbnail timeline (pp. 3114–3115). vidcut has the filters that it has data for ([spec §10.1](../spec.md#101-export-page-pp-41034132-measured-in-resolve-studio-212)).

**What is left:**

- All Filtered Clips in the Render menu (pp. 4099, 4128).
- The prompts "Unable to Apply Filter" and "Cannot Render Single Clip".
- Unrendered Clips, and the rendered state of each timeline clip.
- Selected Clips, Modified Clips, Tracked and Common Media Pool Source.
- Individual clips renders each clip that overlaps the range. In two runs, Resolve rendered only the clips wholly inside it.

**Why it waits:** the measurements do not show these points:

- What Cancel of "Unable to Apply Filter" keeps. If Clear Filter sets Single clip.
- Which edits make a rendered clip unrendered again.
- If Selected Clips and Common Media Pool Source follow a later selection or playhead.
- Which kinds of tracking count as Tracked, and which edits count as Modified.
- If a filtered strip keeps the numbers of the clips.
- If a pick of a filter is an undo step.
