# Title nodes in compositions

**When:** not decided.

The compositions of vidcut get a Text+ tool and a Merge tool ([spec §8.19](../spec.md#819-compositions-pp-1332-1385-1403-1420-21082111-2472-2480-2483-24902492-2500)).

**Why:**

- Resolve's Text+ title and its Fusion titles are Fusion compositions with text tools (p. 1178). vidcut does not have them ([deviation X53](../deviations.md#x53-titles-are-text-the-three-lower-thirds-and-scroll-styled-by-markup)).
- The compositions of vidcut have no text tool and no tool that joins two pictures.

**What Resolve has:**

- The Text+ node draws text into a 2D picture. Its one input is an effect mask that crops the text (p. 2381).
- The Merge node puts a foreground picture over a background picture by the alpha of the foreground (p. 2211).
  - Its Operation mode sets how the two pictures combine. Its Apply Mode sets the composite mode.
  - It can mix premultiplied and straight pictures.
