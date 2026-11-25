# Resolve behavior that vidcut does not have

What [the spec](../spec.md) describes and vidcut does not do.

- Date Imported clusters by day: vidcut keeps no date of import ([§3](../spec.md#3-media-pool-ch-34-pp-653675))
- Zoom of the waveform in the Source Clip scroll area, once measured ([§4.1](../spec.md#41-modes))
- Audio Assistant, Voice Convert, Set Color and Add Marker, and Add Transition to All Edits in Timeline Actions ([§2](../spec.md#2-layout-pp-636637))
- The switch of Dynamic Zoom, Composite, Speed and the other Audio groups, which turns a group off (p. 734; [§8.15](../spec.md#815-transform-and-crop-pp-766768))
- Most of the System and User preferences (p. 94)
- The destination controls of the Edit page, one for each source track (pp. 896–898, 1055; [§8.18](../spec.md#818-compound-clips-pp-10521054-1126-measured-in-resolve-studio-2120))
- Deinterlace Quality for each clip in Retime and Scaling: Normal, High and DaVinci Neural Engine (pp. 739, 741)
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
- The Sizing mode, the Auto-Keyframe buttons and the Lock buttons of the Keyframe Editor of the Color page (pp. 3454–3455; [§8.22](../spec.md#822-color-page-keyframes-pp-34533465))
- The keyframe tracks inside a corrector track, and the keyframe badge of a node (pp. 3458–3459)
- Previous and Next Keyframe on the Color page, and a box that selects keyframes there (p. 3462)
- Add EDL Marks on Tracks, and the keyframes of a grade that a copy or a still takes (pp. 3464–3465)
- On the Export page: Estimated File Size, additional outputs, and the import and export of a render preset (pp. 4112, 4127; [§10.1](../spec.md#101-export-page-pp-41034132-measured-in-resolve-studio-212))
- On the Export page: the clip filter, and All Filtered Clips in the Render menu (pp. 3114–3115, 4099, 4128)
- In the Thumbnail timeline: the grade in the pictures, a frame picked by a drag, and the versions (pp. 3109–3111, 4129)
- In the render queue: Show All Projects (p. 4132) and Review Before Upload (p. 4108)
- In the render queue: Reveal in Media Storage in the right-click menu, and Refresh in the "..." menu (pp. 4131–4132)
- Don't show until next copy in Paste Attributes ([§8.26](../spec.md#826-paste-attributes-pp-11551156-measured-in-resolve-studio-212))
- Paste Attributes of a copied speed that has keyframes ([§8.26](../spec.md#826-paste-attributes-pp-11551156-measured-in-resolve-studio-212))
- The Stabilization tool of the viewer Tools, and the Stabilizer mode and the graph of the Tracker palette (pp. 769, 3502; [§8.27](../spec.md#827-stabilization-pp-295-737738-769-923924-12091210-35023503-measured-in-resolve-studio-212))
