# A render queue

**When:** later.

Export jobs run side by side, so several big exports at once compete for the encoder. Past NVENC's few sessions, `auto` exports move on to VAAPI or the CPU ([export.md §5](../decisions/export.md)), and ones asking for `nvenc` fail. Resolve's Deliver page queues renders instead.
