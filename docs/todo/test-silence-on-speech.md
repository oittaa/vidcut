# Try Ripple Delete Silence on speech

Try it in the GUI on `nasa-speech-1979` from `tests/media-manifest.toml`, with the default settings.

- After Remove, does the speech still sound natural at the cuts: no clipped word starts or ends, no clicks?
- Do the defaults (Pre Head and Post Tail 0.15 s, Minimum to Trip 0.5 s) suit speech, or should they change?
- In about half of that clip, hiss keeps the pauses above −40 dB, so none are found there. Measuring only the speech frequencies might find them.
