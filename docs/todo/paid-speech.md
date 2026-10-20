# Paid speech recognition, and speakers

**When:** next. It does not wait on Qwen3-ASR's aligner ([subtitles](subtitles.md)). Local models stay the default.

xAI and Google, behind the same transcribe requests as the local models, and speakers for every model. A key comes from the environment, a command or the system's keychain, never a file vidcut writes. Audio leaves the computer only after a cost question, unless the user has turned it off for that service.

## Services

Two models, each with word start and end times, so a transcript kept with the file, the Transcription window and cutting by words can use them. Parakeet Ultra stays the default. Neither service is chosen on its own.

- **xAI.** `grok-voice-transcribe-2.0` at `https://api.x.ai/v1/stt`, Bearer key. One multipart upload per heard file, or one for the mix, the options first and the file last. No time limit, only xAI's 500 MB: vidcut encodes the audio before asking, and the question shows its size; over 500 MB it refuses with the size and sends nothing. About 1 MB a minute of FLAC, so hours fit. One request per file keeps the speaker numbers the same across the whole file. Words come back with start, end, a confidence and, with `diarize`, a speaker. $0.10 per hour of audio (October 2026). 38 languages. vidcut uploads the bytes: no `url` for the service to fetch.
- **Google.** `gemini-3.5-transcribe` on the Gemini API, an API key (`x-goog-api-key`). Not Cloud Speech-to-Text: Chirp wants a project, a location and a recognizer, which is not a key. Upload through the Files API, ask for a verbatim transcript with word timestamps, then delete the file. Word timestamps cap a request at 30 minutes, so Gemini alone gets pieces, cut at a pause before 30 minutes, each word shifted back by its piece's start; Google says word timestamps can worsen the transcript. About $0.30 per hour (October 2026), from $2 per million audio tokens and $12 per million text tokens at 25 audio tokens a second. A free-tier key may be used to improve Google's models and a paid-tier key is not; vidcut can't tell them apart, so the question says so. Speakers come from this computer.

## xAI's options

Every option of `/v1/stt` is in vidcut. On the transcribe requests (`media.transcribe`, `subtitles.transcribe`, `analyze.words` hearing the timeline) they are one `xai` object with xAI's own field names, so a new xAI field is one more field there. In the GUI, Speakers is beside Language and the rest is under Advanced.

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

## Speakers

`speakers: true` on a transcribe request, and a Speakers checkbox in Transcribe Audio and Create Subtitles from Audio, gives each word a speaker. With xAI it is `diarize`. With any other model, transcribe.cpp's diarization runs on the same heard audio: NVIDIA's Sortformer (`diar_streaming_sortformer_4spk-v2.1`, Q8_0, 139 MB, downloaded like the other models, NVIDIA Open Model License). It tells voices apart, not words, so it serves every language, but it is published as English and unverified: try it on Finnish before it is offered for other languages. Up to 4 speakers, numbered by first appearance; a word takes the speaker heard over most of it.

- **Kept with the file.** A kept transcript's words carry their speaker. `Asked::made` reuses a transcript with speakers for either; one without them isn't used when speakers are asked for.
- **Numbers and names.** Speakers are numbered per file (`hear: media`) or per run over the mix. The project names them: (media, number) to a name, renamed in the Transcription window; giving two files' speakers the same name makes them one person. Assign To moves the selected words to another speaker. Resolve remembers voices across a project's clips instead (Speaker Detection, pp. 997–1001); neither source gives a voice signature to match across files, so this is a deviation.
- **Transcription window.** A paragraph starts at each speaker change, with the speaker's name; Display Speakers hides the names.
- **Subtitles.** A cue ends where the speaker changes.
- **Scripts.** `analyze.words` gives each word's `speaker` and name; `speakers.name` and `speakers.assign` rename and reassign.

## Keys

No key is stored in plain text: not in `preferences.toml`, its backups, the project, the transcript cache, a job's details, a log line or an error. A request looks for the key in this order:

1. The environment: `XAI_API_KEY` (xAI's own name) and `GEMINI_API_KEY`.
2. A key command, `speech.xai_key_command` or `speech.google_key_command` in the preferences: run when a key is needed, its output the key, such as `pass show xai`, `op read op://Private/xAI/credential` or `secret-tool lookup service xai`. The GUI keeps its answer in memory until vidcut quits.
3. The system's keychain, through the `keyring` crate (4.2): macOS Keychain, Windows Credential Manager, and on Linux the Secret Service (GNOME Keyring, KWallet, KeePassXC), in Rust with no new system library. Service `vidcut`, account `xai` or `google`.

Preferences gets a Services page: for each service a hidden key field with Save (to the keychain) and Remove, where the key comes from now ("XAI_API_KEY", "the key command", "the keychain", or none), the key command under Advanced, and the question's "Don't ask again". Where no keychain answers, as on Linux without a Secret Service, Save says so and names the environment variable and the key command; nothing falls back to a file. On macOS the keychain asks once to let vidcut read the key, and asks again after each new build of vidcut from source.

`preferences.get` reports only where each key comes from. `keys.set` and `keys.remove` write the keychain from scripts, IPC and MCP; `vidcut keys set xai` reads the key from standard input, never from its arguments, which shell history and process lists keep. A model whose key isn't found is an error that names the three places and uploads nothing; its row in the GUI stays and says the key is missing. Tests use `keyring`'s mock store.

## The question

Before the first byte goes out, the GUI asks. The question names the company and the model, what would be uploaded (the mix, or which files), how long that audio is and its size, and the estimate: "about $0.04, at $0.10 an hour." It says the figure is the published rate times that duration, that a request the service accepted and vidcut then lost can be charged again, and that the audio leaves the computer. For Google it adds the free-tier line. Send and Cancel. Esc cancels.

A checkbox, "Don't ask again for xAI" or "for Google", is off unless the user turns it on. It is stored per service (`ask_xai`, `ask_google`, both asking by default). The model row still says the audio is sent to that company when the question is skipped.

Scripts, IPC and MCP don't read the checkbox. They upload only when the request passes `send: true`, the same shape as `download`. Without it the call fails with the dependency error, the message gives the duration, the size and the estimate, and nothing is uploaded. The GUI sets `send` after Send, or straight away when that service's question is off.

The estimate is the published rate, a constant next to the model, not a billing quote. Audio whose words are kept is left out: a transcript is reused on the same rule as a local one (`Asked::made`: the model, the language asked, the way the speech was found, the speakers), and costs nothing the second time.

## What is uploaded

The same heard audio as a local model: one channel at 16 kHz (each channel with Each Channel Separately), from each clip's file or from the mix, the stretches `find` picks (voice, or sound level for singing), encoded to FLAC. The picture stays.

One job, with progress, cancellable while encoding, connecting and uploading. A failure before the service accepts the body may be tried once; a request whose body was sent isn't retried.

The answer's `model` is the service's model id. Word times are real times, so `hear: media` is the default and the transcript is kept with the file under that model name. A later local run doesn't reuse it, and a later paid run doesn't upload it again.

Re-time to Speech and Synchronize Word Timings stay on Parakeet Ultra for the words they match. They don't call a paid service.

## Out

Cloud Speech-to-Text and Chirp. Streaming. A vote between a paid model and a local one. Making either service the default. Matching speakers across files by voice, until transcribe.cpp gives speaker embeddings.
