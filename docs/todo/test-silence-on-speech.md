# Listen to Ripple Delete Silence on speech

Two listening tests on `nasa-speech-1979` (`tests/media-manifest.toml`), 29 minutes of narration. They decide whether the defaults of spec D29 suit speech: threshold −40 dB, Pre Head and Post Tail 0.15 s, Minimum to Trip 0.5 s.

**What the level trace says.** A pause here is 0.5 s or more whose 50 ms level stays 15 dB under the narration around it; the clip has 159.
- In 20 of the 30 minutes the noise between words peaks above −40 dB.
- At −40 dB the analysis finds 40 silences (38 s), and Remove cuts 40 parts (25 s). They cover 38 of the pauses: 33 of 95 in the quiet minutes, 5 of 64 in the noisy ones.
- At −35 dB it finds 98 (82 s), and Remove cuts 98 parts (50 s). They cover 96 of the pauses, 28 of 64 in the noisy minutes, and none lies inside speech.
- A word rises from the noise to the threshold in at most 50 ms, well inside the Pre Head. A word's end takes up to 150 ms to fall back into the noise at −40 dB, and up to 190 ms at −35 dB. Six cuts at −35 dB start 1 to 29 ms before a word's end has fallen (Test 2 goes to them).

**The projects** are in `~/Downloads/vidcut-tmp/silence-speech`: `speech.vidcut` holds the clip alone on V1, and `speech-35.vidcut` holds it already cut at −35 dB with the other settings at their defaults. To make `speech.vidcut` again:

```bash
scripts/fetch-test-media.sh
vidcut project create ~/Downloads/vidcut-tmp/silence-speech/speech.vidcut
vidcut -p ~/Downloads/vidcut-tmp/silence-speech/speech.vidcut media add ~/Downloads/vidcut-tmp/media/nasa-speech-1979/255-hq-296.mp4
vidcut -p ~/Downloads/vidcut-tmp/silence-speech/speech.vidcut edit append m1
```

`speech-35.vidcut` is the same, followed by `edit ripple-delete-silence c1 --threshold-db -35`.

## Test 1: the defaults

Do the cuts sound natural?

1. File > Open Project…, then `~/Downloads/vidcut-tmp/silence-speech/speech.vidcut`
2. Right-click the clip, then Audio Operations > Ripple Delete Silence…
3. Click Remove (the dialog says 40 parts)
4. Press Down (the playhead goes to the next cut)
5. Press Shift + ←
6. Press Space and listen across the cut

Repeat 4–6 at a dozen cuts. Listen for a word's first or last sound missing, a click, and a pause that sounds too short: each cut leaves 0.3 s of the pause.

## Test 2: −35 dB

Are the pauses in the noisy minutes cut as cleanly?

1. File > Open Project…, then `~/Downloads/vidcut-tmp/silence-speech/speech-35.vidcut`
2. Click the viewer's timecode, type 75815 and press Enter (00:07:58:15, a cut)
3. Press Shift + ←
4. Press Space and listen across the cut

Repeat 2–4 with 133018, 193204, 202721, 225805 and 265014: the six cuts closest to a word's end. Then press Home and listen to a few cuts in the first seven minutes, the noisy part, as in Test 1. At the end, the cut takes the last 0.26 s of the closing music's fade.

## What the answers change

- Test 2 as clean as Test 1: −35 dB becomes the default threshold.
- A word's end missing: Post Tail goes up, to 0.2 s.
- A word's start missing: Pre Head goes up.
- A click or a pause too short: note the timecode.
