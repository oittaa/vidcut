# More HDR metadata

- Dolby Vision in an export where some pictures have no Dolby Vision.
- Dolby Vision in an export with black before the first picture.
- HDR10+ from the light of HEVC frames after the HDR10+ of the source stops.
- Dolby Vision through NVENC, VAAPI, AMF and QSV.
- Dolby Vision profiles 5 and 7.
- Mastering display metadata through the ffmpeg-sys-next bindings, in place of vidcut's own copy.
