# Float pictures

- The render cache cuts float pictures to 0..1. Thus the viewer can show other values than the export.
- The upscaler of macOS and the frame rate conversion of macOS cut float pictures to 0..1. Give them the rest of `held::Rest`, and check it on a Mac.
