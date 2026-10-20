# Voice signatures

**When:** once transcribe.cpp gives speaker embeddings.

Speakers found on this computer are numbered per file, and the user links them across files by hand ([deviation X61](../deviations.md#x61-speakers-are-numbered-per-file-or-per-run-and-joined-by-hand)). With a voice signature for each speaker, vidcut would link them by itself, as Resolve's Speaker Detection remembers voices across a project's clips (pp. 997–1001).

- **The model.** transcribe.cpp already runs ECAPA-TDNN for language identification; SpeechBrain's speaker model (`speechbrain/spkrec-ecapa-voxceleb`) is the same network trained on voices, and gives 192 numbers for a stretch of speech. transcribe.cpp would need a role that returns them.
- **Kept with the file.** Each speaker's signature, from its longest turns, goes into the kept transcript, so it serves every project that uses the file.
- **In a project.** Speakers whose signatures are close enough become one person by themselves; the user's links and Separate still win.
- **Across projects.** A person named in one project could be offered in another when a voice matches: a list of known voices in the config folder, each with a name and its signatures. Asked before a name carries over.
- **The threshold.** Measured on recordings with a lav and a camera microphone, where the same person sounds most different.
