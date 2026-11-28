# Float pictures

- The render cache in ProRes 4444, ProRes 4444 XQ and DNxHR 444 cuts float pictures to 0..1. Resolve keeps the values outside 0..1 in these formats (pp. 201, 203). Measure how, then fix it.
- The AI upscalers, the macOS scaler and the AI frames of Retime Process and Smooth Cut cut float pictures to 0..1.
