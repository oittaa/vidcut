# Threads: a cap on job decoders

**When:** if `scripts/threads.sh` shows analyses or light measurements running more CPU decoders at once than the cores serve.

Players bound the interactive decoders and the render queue the jobs', except analyses and light measurements: one for each request. A cap of four job decoders with frame threads, beyond which one opens with a single thread, would bound them. Measure with the census before and after.
