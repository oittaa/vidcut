# Voice signatures

**When:** once transcribe.cpp gives speaker embeddings.

- vidcut numbers the speakers that it finds on this computer for each file.
- The user links them across files by hand ([deviation X61](../deviations.md#x61-speakers-are-numbered-per-file-or-per-run-and-joined-by-hand)).
- Resolve's Speaker Detection remembers voices across the clips of a project (pp. 997–1001).

**The plan:** a voice signature for each speaker, so that vidcut links the speakers by itself.

- **The model.**
  - transcribe.cpp already runs ECAPA-TDNN for language identification.
  - SpeechBrain's speaker model (`speechbrain/spkrec-ecapa-voxceleb`) is the same network, trained on voices.
  - It gives 192 numbers for a stretch of speech.
  - transcribe.cpp needs a role that returns these numbers.
- **Kept with the file.**
  - The kept transcript holds the signature of each speaker, made from the speaker's longest turns.
  - Thus the signature serves every project that uses the file.
- **In a project.**
  - Speakers with signatures that are close enough become one person by themselves.
  - The user's links and Separate still win.
- **Across projects.**
  - When a voice matches, vidcut can offer a person named in one project in another project.
  - For this, the config folder holds a list of known voices, each with a name and its signatures.
  - vidcut asks the user before a name carries over.
- **The threshold.**
  - Measure it on recordings with a lav and a camera microphone. There, the same person sounds most different.
