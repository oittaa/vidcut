# Inspector

**When:** not decided; after Dynamic Zoom.

A panel to see and type a clip's exact values.

- **Why:** the GUI shows a picture's values nowhere. A value set by a script or an agent (`picture.set`) is seen only by its effect.
- **Resolve:** the Inspector shows Transform and Cropping values as fields (pp. 636, 767). vidcut has no Inspector (X43, X52).
- **vidcut:** a panel in the Media Pool's place, for the selected clip:
  - Position, Zoom, Rotation, Flip, and the four crops, as fields.
  - Reset for each tool.
  - A typed value is one `picture.set`, one undo step.
  - A change from a script shows at once.
- **Open:**
  - Units: pixels, as Resolve shows them, or the requests' fractions.
  - Whether the Title and Subtitle settings move into this panel.
  - How the panel and the Media Pool share the left side.
