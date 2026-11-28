# The audio page

**When:** the steps come in the order below. Sound on audio tracks comes first, before the Page step.

The audio page is a page for sound, beside the Cut, Effects, Color and Export pages. It shows the tracks with sound as tall rows, the Mixer and the meters together. vidcut names its pages in plain words, so the page is "Audio".

**Why:**

- The sound tools of vidcut are in the Mixer, the Inspector and their windows. No page shows the rows, the strips and the meters together.
- Use case 2 of the [roadmap](../roadmap.md) needs fades at the ends, even levels and a loudness target. The page puts these in one place.
- Use case 1 sends the clip volume, the pan, the clip equalizer and the cross fades to Resolve in OpenTimelineIO. The page edits the same values.

## What the manual has, and what vidcut has

The page numbers are those of the 21.1 manual. The Fairlight chapters are pp. 3746–4087. "Part" tells what vidcut has.

### The page and its timeline (Ch. 170, 172, 175)

| Part | Pages | vidcut |
|---|---|---|
| Panels of the toolbar: Media Pool, Effects, Index, Groups, Sound Library, ADR, Mixer, Meters, Metadata, Inspector | 3748–3749 | Part: the Media tab, the Effects tab, the Mixer and the Inspector |
| A lane for each channel of a track; track formats | 3749, 3801, 3804–3805 | Missing: a track has no format, and a clip shows one waveform |
| Track header: color, number, name, format, fader value, clip count, lock, arm, solo, mute, meter | 3752–3753 | Part: name, lock, mute and solo |
| Waveform zoom, Smart Zoom, Lock Track Height To | 3753–3754 | Missing |
| Overwrite and Layered Audio Editing | 3755–3757, 3854, 3888–3889 | Part: overwrite only |
| Pointer, Range and Focus modes | 3757–3758, 3859–3869 | Part: the tools of the Cut page |
| Razor, snapping, markers, flags | 3758–3759, 3829–3830 | Part: split, snapping and markers. No flags |
| Transient detection, tempo grid, scrollers, zoom presets | 3759–3767 | Missing |
| Fade handles and fade curves of a clip | 3875–3876, 3891 | Missing. Cross fades at edits are there ([§8.11](../spec.md#811-dissolves-pp-645-747-758761-895-1114-1159-1173)) |
| Batch Fades, Fade In and Fade Out to Playhead | 3892–3894 | Missing |
| Cross Fade with a Fade In and a Fade Out level | 3890–3891, 3915 | Part: one level for both sides |
| Clip gain line and volume keyframes | 3923–3924 | There ([§9](../spec.md#9-audio-pp-642-649650-743744-771-773-778779), D81) |
| Trim Start and Trim End | 3876–3877 | There ([§8.6](../spec.md#86-trimming-pp-693-716-718-726732)) |
| Trim to Selection, nudge, subframe nudge, Duplicate Selection | 3877–3880 | Missing |
| Cut, copy and paste, also of a part of a clip | 3881–3884 | Part: whole clips ([§8.3](../spec.md#83-copy-cut-paste-p-723)) |
| Paste and Remove Attributes | 3884–3885, 3926 | Part: clips. No track attributes |
| EQ Matcher, Level Matcher, AI Dialogue Matcher | 3886–3888 | Missing |
| Remove Silence | 3873 | There: Ripple Delete Silence ([§9](../spec.md#9-audio-pp-642-649650-743744-771-773-778779)) |
| Checkerboard to New Tracks | 3874 | Missing |
| Compound clips | 3890 | There ([§8.18](../spec.md#818-compound-clips-pp-10521054-1126-measured-in-resolve-studio-2120)) |
| Folder tracks, Link Groups | 3807–3809, 3870–3872 | Missing |
| Sample editing, zero crossing line | 3899–3900 | Missing |
| Export Audio Files, External Audio Process | 3897–3898 | Missing. The Export page renders the timeline ([§10.1](../spec.md#101-export-page-pp-41034132-measured-in-resolve-studio-212)) |
| Voice Convert, Speech Generator | 3900–3906 | Missing |
| Elastic Wave | 4001–4004 | Part: speed keyframes keep the pitch (D48) |

### The Inspector (Ch. 176)

| Part | Pages | vidcut |
|---|---|---|
| Clip mode and Track mode | 3908 | Part: the clip sections. The track settings are in the Mixer |
| Volume, Pan, Voice Isolation, Dialogue Leveler, equalizer of a clip | 3909–3913 | There ([§9](../spec.md#9-audio-pp-642-649650-743744-771-773-778779)) |
| Pitch of a clip | 3911 | Missing |
| Audio Configuration: Active, Mute, Level and Source of each channel | 3916–3920 | Part: a clip takes one sound stream |
| Several clips at once, keyframes | 3921–3922 | There |

### The Mixer (Ch. 171, 177, 179)

| Part | Pages | vidcut |
|---|---|---|
| A channel strip for each track: name, meter, fader, mute, solo | 3929, 3965–3966 | There ([§9](../spec.md#9-audio-pp-642-649650-743744-771-773-778779)) |
| Arm, Solo Safe, Exclusive Solo | 3965 | Missing |
| Input menu, Path Settings: Trim Level, Polarity, Direct Output | 3938–3940 | Missing |
| Track FX: Voice Isolation, Dialogue Leveler, Ducker | 4014–4019 | Part: Voice Isolation and the Ducker on tracks. The Dialogue Leveler is on clips only |
| Track FX: Dialogue Separator, Music Remixer | 4016, 4018 | Missing |
| Six insert slots, Effects Order, Effects In | 3938, 3941–3943 | Missing |
| Dynamics of a track | 3944–3948 | Part: no presets, no Sidechain ([below](#track-dynamics)) |
| Equalizer of a track | 3949–3952 | Part: no Gain fader, presets or range buttons ([below](#track-equalizer)) |
| Pan of a track: 2D and 3D panners | 3954–3960 | Missing. A clip has a Pan |
| Buses, Bus Format, Bus Outputs, Bus Sends | 3802–3804, 3810–3814, 3952–3953, 3960 | Missing: one main mix |
| Patch Input/Output | 3814–3817 | Missing |
| Nested timelines | 3960–3962 | Part: compound clips |
| VCAs, track groups | 3963–3964, 3995–3999 | Missing |
| Bounce Selected Tracks, Bounce Mix to Track | 3968–3969 | Missing |
| AI Audio Assistant | 3969–3970 | Missing ([gaps](gaps.md)) |
| Presets Library | 3820–3821 | Missing |

### Automation (Ch. 178)

| Part | Pages | vidcut |
|---|---|---|
| Automation of the controls of a strip and of plugins | 3972–3979, 3988–3992 | Missing. A clip has volume and pan keyframes |
| Automation toolbar: Write, Trim, Touch, On Stop, enables | 3983–3986 | Missing |
| Preview mode, Active Range, Mix List | 3986–3987, 3993–3994 | Missing |
| Audio Panning to Video | 3980–3983 | Missing |

### Effects (Ch. 180, 181)

| Part | Pages | vidcut |
|---|---|---|
| Common controls of an FX: presets, A/B, Reset, Bypass | 4013 | Missing |
| Noise Reduction | 4041–4042 | Part: on a clip ([X157](../deviations.md#x157-noise-reduction-is-our-own-spectral-gate)) |
| Limiter, Gain, De-Esser, De-Hummer, Multiband Compressor, Vocal Channel, Dialogue Processor | 4023–4027, 4036–4041, 4048 | Missing |
| Chorus, Delay, Distortion, Echo, Flanger, Modulation, Pitch, Reverb, Soft Clipper, Stereo Fixer, Stereo Width | 4021–4046 | Missing |
| Chain FX, Fairlight EQ insert | 4019–4021, 4029–4031 | Missing |
| Meters as FX: Meter, Phase Meter, Frequency Analyzer, Surround Analyzer | 4035–4038, 4042, 4047 | Missing |
| VST and AU plugins, Sidechain, Cache Audio Effects | 4004–4011 | Missing ([plugins](plugins.md)) |

### Meters and monitoring (Ch. 170, 182)

| Part | Pages | vidcut |
|---|---|---|
| Track meters and the main meter | 4050–4051 | There: the Mixer |
| Bus meters, Control Room meter with true peak | 4051, 4054 | Missing |
| Loudness meter: M, Short, Short Max, Range, Integrated | 4054–4057 | Missing. `analyze.loudness` measures the mix offline |
| Target Loudness Level, loudness scale and standards | 4055, 4060 | Part: the export's loudness setting and Normalize Audio Levels (D80) |
| Offline Loudness Analyzer | 4057 | Part: `analyze.loudness` |
| Loudness History | 4058 | Missing |
| Meter Type, Pre fader metering | 4059–4060 | Missing |
| Output mute, output level, DIM | 3751 | Part: the speaker button of the viewer meter |

### Other parts (Ch. 170, 172–174, 184)

| Part | Pages | vidcut |
|---|---|---|
| Recording, input monitoring, VSTi | 3831–3840 | Missing |
| ADR | 3841–3850 | Missing |
| Sound Library, Foley Sampler | 3781–3786, 4032–4035 | Missing |
| Test tones, timecode chase | 3793–3795 | Missing |
| Loop Jog | 3824–3825 | Missing. Audio scrubbing is there |
| AAF import | 3796 | Missing ([§1](../spec.md#1-scope)) |
| Control surfaces | 3971 | Missing |
| Immersive formats: Dolby Atmos, Ambisonics, MPEG-H | 4064–4087 | Missing |
| Undo | 3798–3799 | There ([§8.10](../spec.md#810-undo-pp-651652)) |

## The rows of the page

vidcut keeps the sound of a clip on a video track in that clip ([spec §6](../spec.md#6-track-model-pp-647-649-716-721-727)). Resolve keeps that sound on an audio track of its own. Its Fairlight page shows the audio tracks, with a lane for each channel (p. 3801). Short video tracks can show above them for reference (p. 3765).

The owner chose to keep sound as Resolve does: on audio tracks, on all pages. The step [Sound on audio tracks](#sound-on-audio-tracks) makes this change before the Page step. Then the rows of the page are the audio tracks.

The strips, the track equalizer, the Dynamics, the buses and the automation are also for each audio track.

## Rules for each step

- Each step is one pull request, but Sound on audio tracks has four. The steps come in the order below, by their value for the use cases.
- Each step adds its requests to the CLI, IPC and MCP. Each request that changes the project is one undo step.
- A step does its Resolve run first, then the code. The run measures the defaults, the ranges and the sound.
- A step that adds a setting tells what an OpenTimelineIO export writes for it, as the run shows.

## Steps

### Sound on audio tracks

The sound of a clip on a video track moves to a clip of its own on an audio track. The two clips are linked, as in Resolve (pp. 646–647, 913–916). The Cut page still shows a linked pair as one item (p. 646).

**The model:**

- A clip on a video track has picture only. `Clip::audio` is set only on the clips of audio tracks.
- `ClipAudio` loses `head` and `tail`. A J or L cut is a sound clip that starts or ends at a different time than its picture.
- `Clip` gets `link`, a link group number. The clips of one group act as one clip in an edit (pp. 916, 3875). A group can have any number of clips.
- A video track loses `muted`, `solo`, `volume_db`, `voice_isolation`, `duck`, `equalizer` and `dynamics`. An audio track keeps them.
- The sound transitions of a video track move to the `transitions` of the audio track.
- `validate` refuses sound on a clip of a video track, and a sound setting on a video track.
- A project, a history or a saved version in the old form does not load (rule 13). There is no migration.

**Where the sound goes:**

- An edit of a clip with picture and sound makes a link group. The picture goes on the video track, and the sound goes on an audio track.
- The sound of a picture on Vn goes on An, and stream k goes on A(n+k). The Link run measured this on V1 with 7 files (p. 646).
- Open: in Resolve, a stream of more than 2 channels gives one sound clip for each channel. vidcut gives one sound clip for each stream. The Link run measured this with 7 files.
- Open: the Link run added clips with the script call that appends a clip at a record frame. In 3 cases on one layout, a used audio track cut the new sound to its free part.
  - vidcut writes the new sound over the clip there, as Overwrite writes over the target track (spec §7.2).
  - Batch 2 measures Overwrite, Place On Top and Append over a used audio track. The rule changes only if the edit functions also cut.
- An edit of sound alone goes to an audio track (p. 648). The Link run measures which one. D26 changes to that rule.
- Video Only makes the picture clip alone. Audio Only makes the sound clip alone (p. 694).
- `close_up` and `compound::with_sound` add the audio tracks that the sound needs.

**Edits of linked clips:**

- `Project::tied` finds the parts of an edit by `link` alone. Until then, the parts must also have one place.
- A split gives the right halves one new link group. Paste, spread and import already give copies new groups.
- Select, move, trim, roll, slip, slide, split, join, delete, speed and paste act on each clip of a group (p. 3875).
- Each part moves and trims by the same amount as the lead. Today `move_clips` and `trim` give each part the place of the lead, and that erases a J or L cut.
- A J or L cut (spec §8.7) is a roll of the edit between two sound clips on the audio track. The picture clips do not change.
- `fix_audio`, `audio_roll`, `audio_roll_room` and the J/L rules of `split_at` and `joined` work on the audio track. The overlap rule of `validate` for sound goes, since clips on a track never overlap.
- `follow` moves a clip that is linked to a Track 1 clip with its group, not by its In point. `settle` does not move such a clip to another track. D21 applies to the other clips.
- A speed change acts on each clip of the group. The sound clip keeps the speed of its own span, as `compound::sound_part` does now.
- The sound effects of a clip stay on the sound clip: volume, pan, equalizer, Dialogue Leveler, compressor, noise reduction, Voice Isolation and audio effects.

**The Cut page (pp. 646–648, 679):**

- A main track shows the clips of its video track and the sound clips linked to them as one item. The sound shows as the band along the bottom (p. 647).
- An audio track that holds only linked sound has no row. An audio track with other clips has a row, and an empty audio track too.
  - Open: batch 1 measured one timeline. An empty A2 and A4 had no row. A sound-only clip on A1 showed as a thin strip in the row of V1. Batch 2 measures more timelines.
- The mute of a main track header mutes each audio track that holds the linked sound of its clips (p. 647).
- A J/L drag on the band rolls the edit of the linked sound clips.
- The Audio part of the Inspector edits the linked sound clips of the selected item.
- A clip out of sync shows its offset, as on the Edit page (pp. 915, 3878). An OpenTimelineIO import can bring such a clip.

**Compound clips:**

- `compound::with_sound` and `spread_blocks` give the picture block and the sound block one link group.
- The tracks inside a compound clip use the same model. `picture_tracks`, `sounds`, `fit_sound`, `spread` and `render::expanded` read sound only from audio tracks.

**Playback and export:**

- `render::planned_audio` reads sound only from audio tracks. Each audio track has one `Bus`, as now.
- The Mixer, the track equalizer and the track Dynamics have a strip for each audio track. `Track::has_sound` is true only for audio tracks.
- The sources of ducking are audio tracks. `duck::default_sources` lists the audio tracks.
- Transcription, silence detection, Normalize and Voice Isolation read the sound clips.

**OpenTimelineIO:**

- Each track of vidcut is one track of the file, in its order.
- The clips of a link group get one `Link Group ID`, as Resolve writes it.
- The import keeps each `Link Group ID` as a link group. It no longer merges an audio clip into the sound of a video clip.
- A file without link groups links a sound clip to the picture clip of the same media at the same place.
- A file with no audio tracks gives each picture clip a linked sound clip, as it gives the clip its sound now.

**Requests and the schema:**

- The schema loses `head`, `tail` and the sound settings of video tracks. It gets `link`.
- `clip.set`, `clip.normalize`, `clip.paste_attributes`, `clip.remove_attributes`, the `keyframes` requests with Volume or Pan, `analyze.words` and `analyze.silence` on a picture clip act on its linked sound clips. The Inspector does the same. With no linked sound clip, they give an error.
- `track.set` with a sound setting on a video track gives an error.
- The clips in the `timeline` output show `link`. Their `audio` loses `head` and `tail`.
- `clip.link` and `clip.unlink` link and unlink clips (pp. 916, 3875). A setting turns Linked Selection on and off (pp. 913–914).
- Each of these edits is one undo step.

**The pull requests.** Each leaves vidcut working:

1. **Link groups** (#1411). `Clip::link` holds the groups of compound clips and clips with no media. The parts still need one place. Each part moves by the change of the lead. The behavior does not change.
2. **Sound clips** (#1420). The sound moves to the audio tracks, in each part above. The parts of a group no longer need one place. The Cut page draws a linked pair as one item, with the band, the header mute and the J/L drag. The spec, the deviations and the design docs change.
3. **Cut page rows.** The rows of the audio tracks as batch 2 measures them.
4. **Link and unlink** (#1427). `clip.link`, `clip.unlink`, Linked Selection (pp. 913–914), the sync offset and its four fixes (p. 915), as `edit.resync`.
   - Open: Resync Clip on the Cut page (p. 646). Batch 1 saw no change in one case. Batch 2 measures what it changes.
   - Open: the manual does not settle these points. The link run of batch 2 measures them.
     - The text of the offset indicator, and whether a slip shows it.
     - The start, the end and the source In after each of the four fixes, from the picture and from the sound.
     - The indicators of a clip with two sound clips when one sound clip moves.
     - An offset between linked clips of other media.
     - Link of clips that are in other groups, and Unlink of one sound clip of two.
     - A split with Linked Selection off.
     - Whether the project keeps the Linked Selection switch, and whether Undo changes it.
     - The groups of what is left after an overwrite over a linked pair.

Each pull request changes the existing tests to the new model. It adds no tests.

### Page

- `Page` gets Audio. The status bar gets its button. `gui.page` with `audio` opens it.
- The layout comes from the Layout run: the meters and the viewer at the top, the rows, and the Mixer at the right.
- A row shows the waveform of each clip at its full height, the clip name, the volume line and its keyframes.
- The header of a row has the name, mute, solo, lock, the fader value and a meter.
- The edits of the Cut page act on the rows: select, move, trim, split, delete and the volume line.
- `gui.show` and `gui.close` show and hide the Mixer, the meters, the Media tab and the Inspector on the page.
- The pull request adds the page to [spec §1](../spec.md#1-scope) and names its module in the design docs.

### Loudness meter

- The meter panel gets a loudness meter of the mix: M, Short, Short Max, Range, Integrated and the true peak (pp. 4054–4057).
- The project gets the Target Loudness Level, −23 LUFS by default, and the loudness scale (p. 4055).
- The meter shows LU from the target, or LUFS. It has Start, Pause and Reset.
- The live meter and `analyze.loudness` use the same measurement of ITU-R BS.1770.
- A request reads the values of the meter, and a request resets it.

### Clip fades

- Each clip with sound gets a fade in and a fade out: a length and a curve with X-Level and X-Point (pp. 3875–3876).
- The page shows the handles at the top corners of a clip. The Cut page shows them too, if the Fades run finds them on the Cut page.
- Fade In to Playhead and Fade Out to Playhead (p. 3894), and Batch Fades (pp. 3892–3894).
- The player and the export play the same curve.
- A fade is not a transition with one side ([§8.11](../spec.md#811-dissolves-pp-645-747-758761-895-1114-1159-1173)). The Fades run tells how Resolve keeps the two apart.
- `clip.set` sets the fades. Requests do Batch Fades and the fades to the playhead.

### Track pan

- Each track gets a Pan with a switch: the indicator on its strip, and the Pan window (pp. 3954–3955).
- Left/Right is the control of a mix of two channels. The surround controls come with the track formats.
- The pan comes after the equalizer and before the fader (p. 3938). The Pan run gives the law.
- `track.set` sets `pan` and `pan_on`.

### Main strip and inserts

- The main strip of the Mixer gets the Dynamics, the equalizer and inserts, as the strip of a track has.
- Each track and each clip gets insert slots, after the Track FX and before the Dynamics (p. 3938).
- The Effects Order (p. 3942) and Effects In (p. 3943) come with the slots.
- The first insert is the Limiter, a true peak limiter with lookahead (pp. 4036–4038).
- `track.set` and `clip.set` set `inserts`. A request sets the main strip.

### Insert effects

- One pull request for each FX, in this order: Gain, De-Esser, De-Hummer, Noise Reduction on tracks, Vocal Channel, Dialogue Processor, Multiband Compressor.
- Then Soft Clipper, Stereo Width, Stereo Fixer, Pitch, Reverb, Delay, Echo and the other FX of Ch. 181.
- Each FX has the common controls: presets, A/B, Reset and Bypass (p. 4013).
- Each FX needs its own run of about 20 min: its controls, their defaults and ranges, and renders at about 6 settings.
- Where the renders cannot give the processor, vidcut uses a well-known method. The owner decides on that deviation first.

### Track automation

- Each row gets lanes for the fader, the mute and the pan of its track (p. 3975).
- A lane holds keyframes at sample positions, with straight ramps between them (p. 3976).
- Draw, add, move and delete keyframes. Automation Follows Edit is on by default (p. 3989).
- Requests list, add, set, move and remove the keyframes of a lane, as the `keyframes` requests do for a clip.
- The record modes of the Automation toolbar come after the drawn lanes (pp. 3983–3986).
- Then lanes for the equalizer, the Dynamics, the sends and the inserts.

### Track equalizer

vidcut has the track equalizer with the filters that the renders measured ([spec §9](../spec.md#9-audio-pp-642-649650-743744-771-773-778779), Track equalizer). Some cases use the nearest measured rule, and some controls are not there.

To measure:

- The range of the Gain fader.
- A Low Pass of Fire at the slopes 6 and 18, in a render of white noise.
- Whether the window shows the slope of a pass filter with Earth, Air and Ice. Their sound does not use it.
- Whether the first click of a double click on the small graph also bypasses the equalizer. In vidcut it does.
- A second sample rate, if a project can have one. The scripting API keeps the timeline at 48 kHz.

To build:

- The Gain fader after the bands, once its range is measured.
- The presets of the window.
- Copy and Paste of the "..." menu, also to and from a clip equalizer (p. 745).
- The L, ML, MH and H buttons.

### Track Dynamics

vidcut has the track Dynamics with the processor that the renders measured ([spec §9](../spec.md#9-audio-pp-642-649650-743744-771-773-778779), Track Dynamics). Some cases use the nearest measured rule, and some controls are not there.

To measure:

- Mix at 0, 25, 50, 75 and 100, with two settings and two signals: run R17 (`scripts/fl_mix_cases.py` of the fixtures).
  - The renders of Mix 50 and 100 give the input the gain (Mix/100)². The manual says that Mix goes from compressed to not compressed.
  - If R17 gives the same rule, vidcut takes it (`dynamics::mixed`).
- A second sample rate, if a project can have one. The detector of vidcut runs once in each 3 samples at all rates.
  - Resolve can also run its detector at a fixed 16 kHz. The renders at 48 kHz cannot show which rule it uses.
- The rule from the position of a control to its value. The renders give the value only at some positions.
- The Knee at thresholds other than −30 dB, and at ratios far from 4.
- The renders of the Compressor at the ratios 1.2 and 1.6 are 0.1 dB lower than the processor. The cause is not known.
- A render that starts after the start of the timeline. The phase of the detector runs is known only from the start.
- The Expander at the Attack 0.50 ms, and signals that clip inside the modules.
- The preset values `DYN_TYPE`, `DYN_HPF_FREQ` and `DYN_LINK_IN`. The window shows no control for them.
- What the graph of the window shows, and its moving balls.
- Whether the first click of a double click on the small graph also bypasses the Dynamics. In vidcut it does.

To build:

- The presets of the window.
- The Sidechain of the Compressor, with On, Listen and Source.
- The moving level on the graph of the window.

### Buses and sends

- The Bus Format window adds, renames and removes buses (p. 3810). Mono and stereo buses come first.
- Each track gets Bus Outputs (p. 3960) and Bus Sends with a level, a pan, and pre or post fader (pp. 3952–3953).
- A bus has a strip with the Dynamics, the equalizer, inserts and a fader. The meter panel shows bus meters (p. 4051).
- The mix plan has a `Bus` for each track ([export](../design/export.md#sound)). A bus of the page is one more `Bus` in it.
- Requests add, set and remove buses. `track.set` sets the outputs and the sends.

### Track commands

- Track color, from 16 colors (p. 3752).
- Duplicate Track, Move Track Up and Down, Delete Empty Tracks and Disable Track (pp. 3805–3807).
- Lock Track Height To (p. 3753).
- Audio track formats: mono, stereo, 5.1, 7.1 and adaptive, and Change Track Type (pp. 3804–3805).
- `track.add` and `track.set` take the new fields.

### Range and Focus modes

- Range mode and Focus mode, beside the tools of the Cut page (pp. 3859–3869).
- Cut, copy and paste of a range on several tracks (pp. 3881–3884).
- Trim to Selection, Duplicate Selection and the nudges (pp. 3877–3880).
- The edit requests take a range of tracks and times.

### Layers

- Layered Audio Editing, Show Audio Track Layers and Flatten Audio Track Layers (pp. 3755–3757, 3888–3889).
- A layer on top mutes what it covers on the layers below.
- This changes `Track`: its clips can then overlap on different layers.

### No step yet

The owner decides when these come:

- Recording, input monitoring and VSTi (pp. 3831–3840). ADR (pp. 3841–3850).
- The Sound Library and the Foley Sampler (pp. 3781–3786, 4032–4035).
- VST and AU plugins ([plugins](plugins.md)).
- VCAs and track groups (pp. 3963–3964, 3995–3999). Link Groups and folder tracks (pp. 3807–3809, 3870–3872).
- Patch Input/Output, Path Settings and test tones (pp. 3793, 3814–3820). Timecode chase (pp. 3794–3795).
- EQ Matcher, Level Matcher and AI Dialogue Matcher (pp. 3886–3888). Sample editing (p. 3899).
- Voice Convert, Speech Generator and Checkerboard to New Tracks (pp. 3874, 3900–3906).
- Dialogue Separator and Music Remixer (pp. 4016, 4018). Elastic Wave (pp. 4001–4004).
- Transient detection, the tempo grid and the scrollers (pp. 3759–3767).
- Mix List, Bounce and the AI Audio Assistant (pp. 3968–3970, 3993–3994). Audio Panning to Video (pp. 3980–3983).
- Loudness History (p. 4058). Immersive formats (pp. 4064–4087). Control surfaces (p. 3971). AAF import (p. 3796).

## Resolve runs

All runs are in the project "vidcut measure", in Resolve Studio 21.2.

- The scripting API cannot set the Mixer, the pan, buses, automation, fade curves or meters. Thus each run needs computer use on the Fairlight page.
- Scripts set up the timelines and tracks (`AddTrack` with a format), set fade lengths (`SetFades`), render and export OpenTimelineIO.
- The renders use the scripts of `fixtures/program/scripts`: one clip on, the Audio Only preset, 48 kHz, 32-bit integer.
- An equalizer or Dynamics preset file loads at once (run R16). Thus their cases are preset loads, not mouse steps.

| Run | Step | Time |
|---|---|---|
| Link | Sound on audio tracks | 40 min |
| Layout | Page | 30 min |
| Loudness | Loudness meter | 45 min |
| Fades | Clip fades | 45 min |
| Pan | Track pan | 35 min |
| Main bus | Main strip and inserts | 40 min |
| Automation | Track automation | 45 min |
| Equalizer | Track equalizer | 30 min |
| Dynamics | Track Dynamics | 60 min |
| Bus | Buses and sends | 45 min |
| Track menu | Track commands | 25 min |
| Range | Range and Focus modes | 40 min |
| Layers | Layers | 25 min |

The runs go in three batches. Each batch measures before the code of its steps:

- Link, Layout, Loudness, Fades and Pan: about 3 h 15 min.
- Main bus, Automation, Equalizer and Dynamics: about 2 h 55 min.
- Bus, Track menu, Range and Layers: about 2 h 15 min.
- An insert FX run takes about 20 min. It goes in the batch before its pull request.

The steps of a batch go in this order:

1. The script setup that the window steps need, and no more.
2. The window steps of all runs, one after the other, while the owner is at the computer. Window steps do not work with the screen locked.
3. The script steps: script runs, renders, fits and notes. Scripts work with the screen locked.

### Link run

Window steps, about 25 min:

- On the Cut page, edit a stereo video clip, a clip with two sound streams and a clip with mono channels. On the Edit page, write the track of each sound clip.
- On the Cut page, edit a music file with no picture, with a sound clip on A1 and with none. Write its track.
- Put music on A1 where V1 has no clip, and on A3. Write which audio tracks the Cut page shows apart.
- On the Edit page, move the sound of a V1 clip to A2. Write what the Cut page shows.
- On the Edit page, Option-drag a sound clip out of sync. Write what the Cut page shows, and the result of Resync Clip.
- Unlink a pair on the Edit page. Write what the Cut page shows.
- Mute a main track on the Cut page. Write which tracks the Edit page shows muted.
- Make a J cut on the Cut page. Write the ends of each clip on the Edit page.
- Trim the In of a linked pair with a J cut on the Edit page. Write the change of each clip.
- Select a clip with two sound clips on the Cut page. Write what the Inspector shows.

Script steps, about 15 min:

- Read `GetLinkedItems` for each case above.
- Export OpenTimelineIO for each case. Write the `Link Group ID` and the source ranges of each clip.
- Import each export into a new timeline. Write the links that it gets.

### Layout run

- A timeline with a clip of a stereo video file on V1, and a mono music file on A2.
- Screenshot the page at its defaults. Click each toolbar button, and write what it shows or hides.
- Write the items of a track header at each track height, and the default height.
- Write the parts of the meter panel, and the default width of the Mixer.
- Write the menus of a track header, of a clip, of the ruler, and the Fairlight menu.
- Write what the Inspector shows in Clip mode and in Track mode.
- Write the keys of Pointer, Range and Focus mode, and which keys of the Cut page work on the page.

### Loudness run

- The signals of EBU Tech 3341 and 3342, made before the run. Their loudness is known.
- Play each signal on the main bus. Stop at fixed frames, and write M, Short, Short Max, Range, Integrated and the true peak.
- The manual gives the Short window as 30 s (p. 4056) and as 3 s (p. 4058). A step in level shows which one is correct.
- Write the Target Loudness Level, the scales, the standards, Relative and Absolute, Reset, Pause and Lock Metering to Transport.
- Write the Meter Type choices, Pre fader metering, and the levels where a track meter turns yellow and red.
- Write what the output mute, the output level and DIM do. Render with DIM on.
- Run the Offline Loudness Analyzer on the same signals, by script (`AnalyzeAudioLevels`).

### Fades run

- Set fades of 1 s by script, and render. The renders give the default curve.
- Drag the curve handle to 6 positions. Write X-Level and X-Point from the tooltip, and render each.
- Open Batch Fades. Write the 6 shapes, the defaults, Equal Power, Equal Gain and Unlinked. Apply each shape, and render.
- Write what Fade In to Playhead and Fade Out to Playhead change.
- Render a Cross Fade with a Fade In level that is different from its Fade Out level.
- Write whether the fade handles show on the Cut page and on the Edit page.
- Export OpenTimelineIO with fades. Compare it with an export without them, and import it again.

### Pan run

- A stereo track and a mono track, each with noise.
- Open the Pan window of each. Write each control, its default and its range, and what a typed value outside the range does.
- Set Left/Right to −100, −50, −25, 25, 50 and 100 on each track. Render each.
- Render a clip Pan together with a track pan. The render gives their order.
- Click the pan indicator once to bypass the pan, and render.
- Export OpenTimelineIO with a track pan. Compare it with an export before.

### Main bus run

- Write the strip of Bus 1: its sections, its Dynamics, its equalizer and its fader.
- Save a Limiter preset in the Presets Library. Find its file and read its form. If it is XML, the Limiter cases are preset loads.
- Write each control of the Limiter, its default and its range. Render noise and tone bursts at 6 settings.
- Write the insert slots of a track: their number, the "+" menu and the Effects Order choices.
- Render with Effects In off. Export OpenTimelineIO with an insert on a track and on a clip.

### Automation run

- Turn on Toggle Automation. Write the lanes of the dropdown of a track.
- Add fader keyframes of 0 dB and −20 dB, 1 s apart. Render. The render gives the shape of the ramp.
- Render one case of mute automation and one of pan automation.
- Move the clip with Automation Follows Edit on, then off. Write where the keyframes go.
- Write the Automation toolbar: the modes, Touch, On Stop and the enables.
- Write the undo steps of a drawn line. Export OpenTimelineIO with automation, and compare it.

### Equalizer run

- The cases to measure of the [Track equalizer](#track-equalizer) step.

### Dynamics run

- The cases to measure of the [Track Dynamics](#track-dynamics) step, and renders of the Sidechain with On and a source track.

### Bus run

- Open the Bus Format window. Write the default buses, the formats and Add Bus.
- Add a stereo Bus 2. Send a track to Bus 2, and Bus 2 to Bus 1. Render.
- Render a mono track on a stereo bus, and a stereo track on a mono bus.
- Render a bus send pre fader and post fader, with a level and a pan.
- Write what the Deliver page renders when the timeline has two main buses.
- Export OpenTimelineIO with buses. Compare it with an export before.

### Track menu run

- Write the items of the track header menu, and the columns of the Index.
- Write the formats of Add Track and Add Tracks, and the name that Duplicate Track gives.
- Do Move Track Up and Down, Delete Empty Tracks and Disable Track. Write the result of each.
- Write the 16 colors of Change Track Color, and the heights of Lock Track Height To.
- Export OpenTimelineIO with a track color, a disabled track and a mono track. Compare it with an export before.

### Range run

- Range mode: write what a click, a drag, I and O select. Cut, copy and paste a range on two tracks.
- Focus mode: write the tools of the two halves of a clip.
- Do Trim to Selection, Duplicate Selection and the nudge keys. Write the nudge step in subframes and in ms.
- Write the undo steps of each.

### Layers run

- Turn on Layered Audio Editing. Drop a clip over a part of another clip, and render.
- Show Audio Track Layers. Delete the top clip, and write what plays. Flatten the layers, and write the result.
- Export OpenTimelineIO and AAF with layers. Compare them with a flat timeline.
