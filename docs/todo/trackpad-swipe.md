# Trackpad swipe moves the playhead

**When:** later.

A two-finger horizontal swipe over the tracks moves the playhead. vidcut's lower timeline takes a horizontal scroll only with Fixed Playhead off, to scroll the view (`timeline/mod.rs`). A pinch zooms the lower timeline as Ctrl+wheel does, as egui gives both as a zoom. Check the manual for what each timeline does with a swipe and with a pinch first.
