# Proxies for the preview

**When:** next after the interchange. **In progress:** pull request #26 (needs rework).

For footage the GPU can't decode (10-bit 4:2:2, ProRes, DNxHR, 8K), for slow storage, and for image sequences of big pictures (a 4K 16-bit PNG takes about 100 ms to decode, so such a sequence can't play in real time).

- Resolve generates them from the Media Pool and switches between proxies and originals; export always reads the originals (pp. 207–218).
- Ours keep the source's frame timestamps, so the preview stays frame-exact.

**Before #26 can merge:**
- Generating proxies must run as a background job, with progress and cancel, like an export. Run inside an edit, it freezes the GUI's core thread and, headless, holds the project lock for the whole encode.
- Its spec decision must be **D41**: D40 is now the interchange with Resolve. Renumber all of the pull request's references to D40, and keep master's D40 text when resolving the conflict.
