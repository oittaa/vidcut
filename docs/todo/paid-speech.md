# Paid speech recognition

**When:** next. It does not wait on Qwen3-ASR's aligner ([subtitles](subtitles.md)). Local models stay the default.

xAI and Google, behind the same transcribe requests as the local models. The user puts an API key in Preferences. Audio leaves the computer only after a cost estimate, unless they have turned that question off for that service.

## Services

Two models, each with word start and end times, so a transcript kept with the file, the Transcription window and cutting by words can use them. Parakeet Ultra stays the default. Neither service is chosen on its own.

- **xAI.** `grok-voice-transcribe-2.0` at `https://api.x.ai/v1/stt`, Bearer key. One multipart upload, the file last, 500 MB at most. Words come back with start, end and a confidence. Published rate: $0.10 per hour of audio uploaded (October 2026). The cluster is us-east-1. 38 languages; a language code biases it. No `url`: vidcut uploads the bytes and does not hand the service an address to fetch.
- **Google.** `gemini-3.5-transcribe` on the Gemini API, an API key (`x-goog-api-key`). Not Cloud Speech-to-Text: Chirp wants a project, a location and a recognizer, which is not a key dropped into Preferences. Upload through the Files API, ask for a verbatim transcript with word timestamps, then delete the file. Word timestamps cap a request at 30 minutes, and Google says they can worsen the transcript. Published blend: about $0.005 per minute, $0.30 per hour (October 2026), from $2 per million audio tokens and $12 per million text tokens at 25 audio tokens a second. A free-tier key may be used to improve Google's models; a paid-tier key is not. vidcut cannot tell the tier from the key, so the question says so.

Leave diarization, keyterm lists and inverse text normalization out of the first version. xAI's formatting and Gemini's smart mode glue a spoken number into one token and drop fillers. Ask for the words that were said (`filler_words` on xAI, verbatim on Gemini). vidcut's own filler list still marks them.

## Keys

A `speech` section in the preferences, beside the filler lists: `xai_key` and `google_key`. The Preferences window has two fields. The window says they are stored in the clear in `preferences.toml`.

`preferences.get` reports only whether each key is set. `preferences.set` writes a new key, or `null` to clear it. A patch that leaves the field out keeps the key, so saving another preference does not wipe it. The key never goes into the project, the transcript cache, a job's details, a log line or an error. `preferences.reset` already copies the whole file to a backup; that backup holds the key too.

A model whose key is missing is an error that names the preference and uploads nothing. In the Create Subtitles dialog the row stays visible and says the key is missing.

## The question

Before the first byte goes out, the GUI asks. The question names the company and the model, what would be uploaded (the mix, or which files), how long that audio is, and the estimate: "about $0.04, at $0.10 an hour." It says the figure is the published rate times that duration, that a request the service accepted and vidcut then lost can be charged again, and that the audio leaves the computer. For Google it adds the free-tier line. Send and Cancel. Esc cancels.

A checkbox, "Don't ask again for xAI" or "for Google", is off unless the user turns it on. It is stored per service (`ask_xai`, `ask_google`, both defaulting to asking). Turning it off for one service does not cover the other. The model row still says the audio is sent to that company when the question is skipped.

Scripts, IPC and MCP do not read the checkbox. They upload only when the request passes `send: true`, the same shape as `download`. Without it the call fails with the dependency error, the message gives the duration and the estimate, and nothing is uploaded. The GUI sets `send` after Send, or straight away when that service's question is off.

The estimate is a constant next to the model, the published rate above. It is not a billing-API quote. Already-kept audio is left out of the duration. A file's transcript is reused on the same rule as a local one (`Asked::made`): this model, the language asked, and the same way of finding the speech, voice or sound level. What is reused costs nothing the second time, and the question says so.

## What is uploaded

The same heard audio as a local model: one channel at 16 kHz, from each clip's file or from the mix, the stretches `find` already picks (voice, or sound level for singing). Encoded to FLAC. The picture stays. Pieces are contiguous ranges, ending at a pause and at 25 minutes, the same split the local models use, so a Gemini request stays under 30 minutes and a cancel has somewhere to land. Word times shift back by the piece's start.

One job, with progress, cancellable while connecting and while uploading. A failure before the service accepts the body may be tried once. A request whose body was sent is not retried.

The answer's `model` is the service's model id. Word times are real times, so `hear: media` is the default and the transcript is kept with the file under that model name. A later local run does not reuse it, and a later paid run does not upload it again.

Re-time to Speech and Synchronize Word Timings stay on Parakeet Ultra for the words they match. They do not call a paid service.

## Out

Cloud Speech-to-Text and Chirp. Streaming. A vote between a paid model and a local one. Making either service the default.
