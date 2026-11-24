# Checks of the Export page

**When:** with the next measurements of the Deliver page.

**Why:** these parts of the Export page are not measured yet. vidcut does a plain thing until a measurement tells what the Deliver page does.

**What to measure:**

- O on the Cut page, with Entire Timeline picked on the page. The measurement shows that I switches Render to In/Out Range. vidcut keeps Entire Timeline for O.
- Alt+X on the Cut page, with In/Out Range picked on the page. Does Render switch to Entire Timeline? vidcut keeps In/Out Range.
- Ctrl+Z after I on the Cut page switched Render. Does Render go back to Entire Timeline? In vidcut it does: the mark and the switch are one undo step.
- With Individual clips, I and O on the page with two clips under the playhead (V1 and V2), and in a gap. vidcut puts them at the playhead.
- With Individual clips, I with the playhead near the end of a clip, for example at frame 90 of a clip from 0 to 95. Does I go to 0, the start of the clip under the playhead, or to 96, the nearest clip start (p. 4129)? vidcut goes to 0.
- The playhead. After Render 1, the playhead was at the In. When does it go there? vidcut does not move it.
