# Plugins

**When:** not decided.

A plugin is an effect that the code of another maker gives. vidcut loads no plugin.

**Why:**

- The picture effects and the sound tools of vidcut are a fixed set. A user who needs another one cannot add it.
- The Cut page shows the installed plugins beside its own effects.
- Kdenlive lists OpenFX, LV2 and VST on its [roadmap](https://kdenlive.org/roadmap/).

**What the manual says:**

- The Effects tab of the Cut page lists the installed OFX plugins in their own categories (p. 765).
- Its Audio category lists the VST plugins on macOS and Windows, and the Audio Units on macOS (p. 765).
- Resolve supports VST3. VST effects are on macOS and Windows, and not on Linux (p. 4005).
- A search of the manual finds no CLAP and no LV2.
- The Effects panel of the Cut page Inspector has the Fusion FX, the Open FX and the Audio FX of a clip (p. 747).
- The Video Plugins preferences turn each Open FX plugin on or off at startup (p. 103).
  - Resolve skips a plugin that made a crash or a hang at the last start.
- The Audio Plugins preferences list the folders of the VST plugins (p. 104).
  - Resolve turns off a VST plugin that makes it crash at startup. The user can turn it on again.
- The Open FX of the Cut page and the Edit page work before the clip grade ([spec §13](../cut-page-spec.md#13-order-of-operations-pp-287296-3407)).

**In the code:**

- A picture effect is an `EffectKind` with a shader of vidcut ([export](../design/export.md#effects)).
- The sound tools of a clip are fields of `ClipAudio`, in a fixed order ([export](../design/export.md#sound)).
- The design lists Open FX among the stages vidcut does not have ([picture order](../design/README.md#picture-order)).
- A picture stays in GPU memory from the decoder to the encoder, where this is possible ([gpu-frames.md](gpu-frames.md)).

**To do:**

- Measure the lines of the manual above in Resolve. The manual and the application can differ.
- Decide the formats. Resolve has OpenFX, VST3 and Audio Units.
  - CLAP and LV2 are not in Resolve. Record a deviation if vidcut gets one of them.
  - Resolve has no VST effects on Linux (p. 4005). Decide the format for Linux.
- Check the license of each SDK against the license of vidcut (AGENTS.md rule 6).
  - The VST 3 SDK is MIT from version 3.8 ([license](https://steinbergmedia.github.io/vst3_dev_portal/pages/VST+3+Licensing/VST3+License)).
- Decide where a plugin runs. A plugin that crashes must not close vidcut or damage the project.
- Measure the cost of a picture that leaves GPU memory for a plugin and comes back (AGENTS.md rule 9).
- A project names each plugin and stores its values. A project with a missing plugin still opens, and names that plugin.
- The values of a plugin are a list that the requests read and set (AGENTS.md rule 3). The window of the plugin is one more way.
- Measure what an OpenTimelineIO file from Resolve keeps of an Open FX and of an audio plugin.
- Measure where Resolve puts the Audio FX of a clip among its own sound tools.
- The viewer and the export play the same result.
