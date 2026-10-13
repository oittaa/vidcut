# Track faders into Resolve

**When:** later.

Resolve's OTIO has no place for a track's fader (deviation X9), so a timeline mixed with faders sounds different in Resolve. An export option could fold each track's fader into its clips' volume, which Resolve reads. Resolve drops vidcut's metadata, so a timeline coming back can't tell the folded part from the clip's own gain: it stays an option, off by default, and the result says it was used.
