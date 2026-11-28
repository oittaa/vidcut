# Export page: File tab

- The frame count in the names of an interlaced source. On a 25 fps timeline, clips from source frames 0 and 5 give 1 and 7 in Resolve. vidcut gives 0 and 5.
- An interlaced clip that is trimmed, as EXR. Resolve writes more files than the clip has fields: 25 and 29 files for 22 fields. vidcut writes 22 files.
- The EXR names of a source whose name ends in a digit. vidcut puts no `_` before the count. Resolve is not measured.
- vidcut refuses an EXR sequence with Source name, File suffix, Use unique filenames or no frame count.
- Render timeline effects and Render at source resolution for Individual clips.
- The Preserved Path row.
- Use commercial workflow.
- The Single clip rows Filename uses Timeline name, File suffix, File subfolder, Render without timecode and Start timeline timecode at.
