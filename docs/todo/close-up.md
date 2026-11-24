# Close Up: the zoom and the faces

**When:** next. The measurement run needs the owner's OK.

The Close Up edit is in [spec §7.2](../spec.md#72-edit-functions). Its range, its track, the marks and the playhead come from 11 measured cases. Its zoom and its framing do not.

**Why:**

- The manual says that Close Up adds the clip at about 150% (pp. 698–699). A face detection puts a face that it finds in the frame.
- One measured clip gave Zoom 2.0 and Position 0, and a person off the middle stayed off the middle. That clip was 1280×720.
- One clip cannot tell a face that the detection did not find from no detection. It also cannot show whether the zoom depends on the sizes of the media and the timeline.
- The manual names media that is larger than the timeline as the use of this edit. No run had such media.

**In the code:**

- `ops::close_up` gives each copy Zoom 2.0 and the usual picture until this measurement.

**To do:**

- Measure Close Up in Resolve with several clips: one face at the left, in the middle and at the right, two faces, and no face.
- Use media that is larger than the timeline, of the same size, and smaller: for example 3840×2160 media in a 1920×1080 timeline.
- Read the Zoom, the Position and any keyframes of each copy.
- Then match the zoom and the framing, and remove this file.
