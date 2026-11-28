# The clip filter

**When:** now. The F4 runs measured the points below.

The Color page filters its Thumbnail timeline (pp. 3114–3115). vidcut has the filters that it has data for. [Spec §10.1](../spec.md#101-export-page-pp-41034132-measured-in-resolve-studio-212) gives the measured rules.

**What is left:**

- All Filtered Clips in the Render menu of the Export page (pp. 4099, 4128).
- The prompts "Unable to Apply Filter" and "Cannot Render Single Clip".
- The strip of the Export page shows the filter only with Individual clips. A filter that leaves no clip greys Add to Render Queue.
- Unrendered Clips. It needs the rendered state of each timeline clip.
- Selected Clips: the clips selected in the strip at the pick.
- Modified Clips: the clips with a grade change in the last 1, 2, 3, 6 or 24 hours. It needs the time of the last grade change of each clip.
- Tracked: the clips with a window track or a stabilization.
- Common Media Pool Source: the clips of the media of the current clip.

**Why:** each item needs a control or data that vidcut does not have yet.
