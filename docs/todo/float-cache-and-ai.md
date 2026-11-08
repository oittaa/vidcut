# Float pictures in the render cache and the AI models

**When:** not decided.

**Why:** two paths still cut a float picture to the values from 0 to 1.

- The render cache:
  - `cache_encoder` (`crates/vidcut-av/src/proxy.rs`) writes YUV integers: 8-bit or 10-bit YUV, or the format of the `CacheCodec`.
  - Thus a cached frame of a float picture loses its values above 1 and below 0.
  - Exports do not read the cache. Thus the viewer can show other values than the export.
  - The difference shows where a later step uses these values, for example the output LUT.
- Super Scale and Retime Process:
  - The AI upscalers take 16-bit integer RGB pictures (`RGB48`). The scaler of macOS takes P416.
  - The AI frames of Retime Process and Smooth Cut also take 16-bit integer RGB pictures.
  - Thus these steps cut a float picture to 0..1, in the viewer and in the export.

**How:**

- The render cache: keep a float picture out of the jobs of the clip level, or add a cache format that holds floats.
- The AI models: find what each model does with values above 1. Then give it half-float pictures, or map the values into 0..1 and back.
- Check an export to a float OpenEXR: a value of 4.0 in an enlarged or retimed clip must stay 4.0.
