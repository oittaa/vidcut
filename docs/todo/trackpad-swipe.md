# Trackpad swipe moves the playhead

**When:** later.

On a MacBook (M4), a two-finger left or right swipe over the Cut page's tracks moves the playhead in DaVinci Resolve. vidcut's lower timeline takes a horizontal scroll (`smooth_scroll_delta.x`, `timeline/mod.rs`) only with Fixed Playhead off, where it scrolls the view; with Fixed Playhead on, the default, a swipe does nothing, and the upper timeline ignores it.

What to settle first: what the manual says a horizontal scroll does on each timeline, with Fixed Playhead on and off, and how far a swipe moves (pixels or frames). Then the same on every system: a touchpad's horizontal scroll, and Shift+wheel, arrive the same way on Linux and Windows. The spec gets the rule; a deviation only if vidcut does otherwise.
