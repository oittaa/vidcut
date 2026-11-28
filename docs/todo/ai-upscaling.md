# AI upscaling

- AI Slow at 4K keeps most of its caches in main memory, and waits for their copies.
- The memory plans of AI Slow and AI Fast do not count all the GPU memory that they use.
- AI Fast starts each chunk from zero, with no state or overlap from the chunk before.
- The shaders of AI Slow and AI Fast do not use the tensor cores.
- The shaders of AI Slow and AI Fast do not use 16-bit arithmetic.
- The Winograd shader of AI Fast does few sums in each thread.
