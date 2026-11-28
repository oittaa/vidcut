# AI upscaling

- At 4K, AI Slow keeps most of its caches in main memory and waits for the copies.
- The memory plans of AI Slow and AI Fast do not count all the GPU memory that they use.
- AI Fast starts each chunk from zero, with no state from the chunk before.
- The shaders of AI Slow and AI Fast do not use the tensor cores or 16-bit arithmetic.
