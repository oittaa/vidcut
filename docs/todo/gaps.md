# Resolve behavior that vidcut does not have

What [the spec](../spec.md) describes and vidcut does not do.

- Date Imported clusters by day: vidcut keeps no date of import ([§3](../spec.md#3-media-pool-ch-34-pp-653675))
- Zoom of the waveform in the Source Clip scroll area, once measured ([§4.1](../spec.md#41-modes))
- Audio Assistant, Voice Convert, Set Color and Add Marker, and Add Transition to All Edits in Timeline Actions ([§2](../spec.md#2-layout-pp-636637))
- The switch of Dynamic Zoom, Composite, Speed and the other Audio groups, which turns a group off (p. 734; [§8.15](../spec.md#815-transform-and-crop-pp-766768))
- Most of the System and User preferences (p. 94)
- Deinterlace Quality for each clip in Retime and Scaling: Normal, High and DaVinci Neural Engine (pp. 739, 741)
- The Use proxy media setting of an export (p. 214)
- CEA-608 and EAS708 captions, and MXF files (pp. 1247–1248)
- Export and import of a subtitle preset as a `.preset` file (pp. 1239–1240)
- KeyKode in the timecode menu of the viewer (pp. 713–714)
- The Solo icon of a track header, between Lock and Mute ([§5](../spec.md#5-dual-timeline-pp-66-643646-689692-716718))
- The Histogram and the other scopes (pp. 3093, 3097)
- The Y, CbCr and RGB overlays of the Waveform (p. 3097), and RGB, YRGB and YCbCr in the Parade (p. 3102)
- The skin tone line of the Vectorscope (p. 3098), and Video Level Scopes in the scale menu
- The unlabeled dashed lines of the HDR scale
- Copy and paste of the Equalizer settings of a clip (p. 745; [§9](../spec.md#9-audio-pp-642-649650-743744-771-773-778779))
- Keyframes of the Angle and the Border of an Edge Wipe. Resolve can keyframe them. vidcut keeps one value of each ([§8.11](../spec.md#811-dissolves-pp-645-747-758761-895-1114-1159-1173))
- The Reset of the Dialogue Leveler section, once its values are measured (p. 734; [§9](../spec.md#9-audio-pp-642-649650-743744-771-773-778779))
