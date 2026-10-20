# Paid speech recognition, and speakers

**When:** next. It does not wait on Qwen3-ASR's aligner ([subtitles](subtitles.md)). Local models stay the default.

xAI and Google, behind the same transcribe requests as the local models, and speakers for every model. A key comes from the environment, a command or the system's keychain, never a file vidcut writes. Audio leaves the computer only after a cost question, unless the user has turned it off for that service.

## Services

Two models, each with word start and end times, so a transcript kept with the file, the Transcription window and cutting by words can use them. Parakeet Ultra stays the default. Neither service is chosen on its own.

- **xAI.** `grok-voice-transcribe-2.0` at `https://api.x.ai/v1/stt`, Bearer key. One multipart upload for everything a run hears, the options first and the file last: the heard files joined end to end, or the mix. No time limit, only xAI's 500 MB: vidcut encodes the audio before asking, and the question shows its size; over 500 MB it refuses with the size and sends nothing. About 1 MB a minute of FLAC, so hours fit. Words come back with start, end, a confidence and, with `diarize`, a speaker. $0.10 per hour of audio (October 2026). 38 languages. vidcut uploads the bytes: no `url` for the service to fetch.
- **Google.** `gemini-3.5-transcribe` through the Interactions API (`POST https://generativelanguage.googleapis.com/v1beta/interactions`), an API key (`x-goog-api-key`), the audio uploaded through the Files API and deleted after. Not Cloud Speech-to-Text: Chirp wants a project, a location and a recognizer, which is not a key. Word times and speakers each cap a request at 30 minutes (an hour without them), so up to 30 minutes of heard audio is one request, joined end to end like xAI's, and more is cut at pauses into pieces under 30 minutes, each word shifted back by its piece's start. Google says word times can worsen the transcript. About $0.30 per hour (October 2026), from $2 per million audio tokens and $12 per million text tokens at 25 audio tokens a second. A free-tier key may be used to improve Google's models and a paid-tier key is not; vidcut can't tell them apart, so the question says so. 85 languages and more.

## Options

Every option of both services that keeps word times is in vidcut. On the transcribe requests (`media.transcribe`, `subtitles.transcribe`, `analyze.words` hearing the timeline) each service's options are one object, `xai` or `google`, with the service's own field names, so a new field is one more field there. In the GUI, Speakers is beside Language and the rest is under Advanced.

| xAI field | vidcut |
|---|---|
| `language` | the request's `language`, the Language row |
| `diarize` | the request's `speakers`, the Speakers checkbox |
| `keyterm` | Key Terms: names and words to bias toward, 100 at most, 50 characters each |
| `format` | Numbers as Digits ("$100" for "one hundred dollars"); needs a language, and works in 11; off by default |
| `filler_words` | Keep Filler Words, on by default; off, Delete Filler Words has nothing to find |
| `vad_threshold` | Voice Threshold, 0 to 1; 0 when the speech is found by sound level, so xAI's own gate doesn't drop the singing again, else xAI's 0.5 |
| `multichannel` | Each Channel Separately: sends the file's channels instead of one, and keeps each channel's words |
| `model` | `grok-voice-transcribe-2.0` unless set |

vidcut sets `file`, `audio_format`, `sample_rate` and `channels` itself, and never `url`. Streaming (`wss://api.x.ai/v1/stt`) is for live audio, not files.

| Google field (`transcription_config`) | vidcut |
|---|---|
| `language_codes` | the request's `language`, and under Advanced more than one, for speech that switches languages |
| `mode.diarization_mode: "speaker"` | the request's `speakers`, the Speakers checkbox |
| `mode.timestamp_granularities: ["word"]` | always: vidcut keeps words with their times |
| `mode` verbatim | always: it keeps the fillers vidcut marks |
| `model` | `gemini-3.5-transcribe` unless set |

`custom_vocabulary` and `smart` mode are left out: Google refuses `custom_vocabulary` with word times or speakers, and `smart` gives no word times.

## Speakers

`speakers: true` on a transcribe request, and a Speakers checkbox in Transcribe Audio and Create Subtitles from Audio, gives each word a speaker. Speakers are numbered by first appearance within what one request heard, and a word takes the speaker heard over most of it.

- **This computer, per file.** With any local model, transcribe.cpp's diarization runs on each heard file: NVIDIA's Sortformer (`diar_streaming_sortformer_4spk-v2.1`, Q8_0, 139 MB, downloaded like the other models, NVIDIA Open Model License). Free. Up to 4 speakers in a file; more share those 4 numbers. It tells voices apart, not words, so it serves every language, but it is published as English and unverified: try it on Finnish before it is offered for other languages.
- **xAI, the whole run.** `diarize` on the one request that holds every heard file, so a speaker has one number in all of them, up to the 500 MB.
- **Google, up to 30 minutes.** `diarization_mode` on one request when the heard audio is 30 minutes or less, so a speaker has one number in all the files. Up to 8 speakers; Google calls 3 or more experimental. Over 30 minutes the pieces would number their speakers apart, so the words come from Google and the speakers from Sortformer, per file.

**People.** The project keeps who is who: each file's speaker numbers belong to people, "Speaker 1" until named. A paid request over several files puts each of its numbers into one person across all of them. A local run starts each file's speakers as people of their own, and the user links them by hand: renaming a file's speaker to one of the project's people joins them, and Separate splits one back out. Assign To moves the selected words to another person, kept in the project. Kept transcripts hold only the numbers, so every project links its own.

- **Kept with the file.** A kept transcript's words carry their speaker number. `Asked::made` reuses a transcript with speakers for either; one without them isn't used when speakers are asked for.
- **Transcription window.** A paragraph starts at each change of person, with the name; Display Speakers hides the names.
- **Subtitles.** A cue ends where the person changes.
- **Scripts.** `analyze.words` gives each word's speaker number and person; `speakers.list`, `speakers.name`, `speakers.link` and `speakers.assign` do the rest.
- **The spec.** Resolve's Speaker Detection remembers voices across a project's clips (pp. 997–1001); vidcut links speakers through a paid request or by hand until it has voice signatures ([voice-signatures.md](voice-signatures.md)). A deviation.

## Keys

No key is stored in plain text: not in `preferences.toml`, its backups, the project, the transcript cache, a job's details, a log line or an error. A request looks for the key in this order:

1. The environment: `XAI_API_KEY` (xAI's own name) and `GEMINI_API_KEY`.
2. A key command, `speech.xai_key_command` or `speech.google_key_command` in the preferences: run when a key is needed, its output the key, such as `pass show xai`, `op read op://Private/xAI/credential` or `secret-tool lookup service xai`. The GUI keeps its answer in memory until vidcut quits.
3. The system's keychain, through the `keyring` crate (4.2): macOS Keychain, Windows Credential Manager, and on Linux the Secret Service (GNOME Keyring, KWallet, KeePassXC), in Rust with no new system library. Service `vidcut`, account `xai` or `google`.

Preferences gets a Services page: for each service a hidden key field with Save (to the keychain) and Remove, where the key comes from now ("XAI_API_KEY", "the key command", "the keychain", or none), the key command under Advanced, and the question's "Don't ask again". Where no keychain answers, as on Linux without a Secret Service, Save says so and names the environment variable and the key command; nothing falls back to a file. On macOS the keychain asks once to let vidcut read the key, and asks again after each new build of vidcut from source.

`preferences.get` reports only where each key comes from. `keys.set` and `keys.remove` write the keychain from scripts, IPC and MCP; `vidcut keys set xai` reads the key from standard input, never from its arguments, which shell history and process lists keep. A model whose key isn't found is an error that names the three places and uploads nothing; its row in the GUI stays and says the key is missing. Tests use `keyring`'s mock store.

## The question

Before the first byte goes out, the GUI asks. The question names the company and the model, what would be uploaded (the mix, or which files), how long that audio is and its size, and the estimate: "about $0.04, at $0.10 an hour." It says the figure is the published rate times that duration, that a request the service accepted and vidcut then lost can be charged again, and that the audio leaves the computer. For Google it adds the free-tier line, and over 30 minutes with speakers, that the speakers come from this computer. Send and Cancel. Esc cancels.

A checkbox, "Don't ask again for xAI" or "for Google", is off unless the user turns it on. It is stored per service (`ask_xai`, `ask_google`, both asking by default). The model row still says the audio is sent to that company when the question is skipped.

Scripts, IPC and MCP don't read the checkbox. They upload only when the request passes `send: true`, the same shape as `download`. Without it the call fails with the dependency error, the message gives the duration, the size and the estimate, and nothing is uploaded. The GUI sets `send` after Send, or straight away when that service's question is off.

The estimate is the published rate, a constant next to the model, not a billing quote. Audio whose words are kept is left out: a transcript is reused on the same rule as a local one (`Asked::made`: the model, the language asked, the way the speech was found, the speakers), and costs nothing the second time. With speakers over several files, a file whose words are kept is sent again only when the others are, so they share numbers; the question counts it.

## What is uploaded

The same heard audio as a local model: one channel at 16 kHz (each channel with Each Channel Separately), from each clip's file or from the mix, the stretches `find` picks (voice, or sound level for singing), encoded to FLAC. The picture stays.

One job, with progress, cancellable while encoding, connecting and uploading. A failure before the service accepts the body may be tried once; a request whose body was sent isn't retried.

The answer's `model` is the service's model id. Word times are real times, so `hear: media` is the default and the transcript is kept with the file under that model name. A later local run doesn't reuse it, and a later paid run doesn't upload it again.

Re-time to Speech and Synchronize Word Timings stay on Parakeet Ultra for the words they match. They don't call a paid service.

## Out

Cloud Speech-to-Text and Chirp. Streaming. A vote between a paid model and a local one. Making either service the default. Matching speakers by voice: [voice-signatures.md](voice-signatures.md).
