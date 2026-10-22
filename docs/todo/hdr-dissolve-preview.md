# HDR dissolves in the viewer as exported

**When:** not decided.

- The viewer and picture renders convert each picture of a dissolve to RGB, and then mix the pictures. For HDR, the conversion tone-maps.
- An HDR export mixes the pictures in their PQ or HLG code values.
- Thus, in the middle of a dissolve, the viewer and the export are different (spec D50).

**To do:** where all pictures of a mix have the same colors, mix their code values before the conversion. Then the viewer and the export agree. The usual HDR project has the same colors in all pictures.
