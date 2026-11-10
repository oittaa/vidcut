# Plugins

**When:** not decided.

A plugin is an effect that the code of another maker gives. vidcut loads no plugin.

**Why:**

- The picture effects and the sound tools of vidcut are a fixed set. A user who needs another one cannot add it.
- Editors and audio programs share plugins. A user who has them expects them in each editor.
- The order of the picture has a place for the Open FX of the Cut page and the Edit page ([spec §13](../cut-page-spec.md#13-order-of-operations-pp-287296-3407)).
- Kdenlive lists OpenFX, LV2 and VST on its [roadmap](https://kdenlive.org/roadmap/).

**In the code:**

- A picture effect is an `EffectKind` with a shader of vidcut ([export](../design/export.md#effects)).
- The sound tools of a clip are fields of `ClipAudio`, in a fixed order ([export](../design/export.md#sound)).
- The design lists Open FX among the stages vidcut does not have ([picture order](../design/README.md#picture-order)).
- A picture stays in GPU memory from the decoder to the encoder, where this is possible ([gpu-frames.md](gpu-frames.md)).

**To do:**

- Decide the formats. The candidates are OpenFX for pictures, and VST 3, CLAP and LV2 for sound. macOS also has Audio Units.
- Check the license of each SDK against the license of vidcut (AGENTS.md rule 6).
  - The VST 3 SDK is MIT from version 3.8 ([license](https://steinbergmedia.github.io/vst3_dev_portal/pages/VST+3+Licensing/VST3+License)).
- Decide where a plugin runs. A plugin that crashes must not close vidcut or damage the project.
- Measure the cost of a picture that leaves GPU memory for a plugin and comes back (AGENTS.md rule 9).
- A project names each plugin and stores its values. A project with a missing plugin still opens, and names that plugin.
- The values of a plugin are a list that the requests read and set (AGENTS.md rule 3). The window of the plugin is one more way.
- Measure what an OpenTimelineIO file from Resolve keeps of an Open FX and of an audio plugin.
- Measure where Resolve puts the audio plugins of a clip among its own sound tools.
- The viewer and the export play the same result.
