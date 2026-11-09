# The mode of a crossing at a compound clip that ends inside a transition

**When:** not decided.

**Why:**

- A transition at an edge of a compound clip takes the mode of the picture that the outgoing side shows ([design/export.md](../design/export.md#placed-pictures), `mode_at`).
- `mode_at` reads that mode at the last frame of the compound clip.
- `pictures_at` takes one clip from each inner track at that frame, with `clip_covering`.
- A frame inside an inner transition has two clips on its track. After the cut, `clip_covering` gives the incoming clip.
- The picture of that track on that frame is the inner crossing. `crossing` gives it the mode of the inner outgoing clip.
- Thus the outer crossing and the picture that it crosses can have different modes. The case needs these things together:
  - A compound clip holds two clips with a dissolve between them.
  - The two inner clips have different composite modes.
  - The compound clip ends inside that dissolve, after its cut.
  - The compound clip has a transition to the next clip.
- The plan of that case, with Multiply on one inner clip:

  | Multiply is on | The mode of the inner crossing | The mode of the outer crossing |
  |---|---|---|
  | No inner clip | Normal | Normal |
  | The inner outgoing clip | Multiply | Normal |
  | The inner incoming clip | Normal | Multiply |

- In the second row, the compound clip multiplies with the tracks below up to the outer transition. Inside the outer transition it does not.
- In the third row, the compound clip does not multiply up to the outer transition. Inside the outer transition, both sides multiply with the tracks below.
- A crossing has one mode for all its frames. The pictures of a compound clip can change inside the transition at its edge.

**How:**

- Resolve's frames for this case are not measured.
- `mode_at` can read an inner track as the plan does. On a frame inside an inner transition, the mode is then the mode of the inner crossing.

**To do:**

- Measure the three rows in Resolve.
- Measure a compound clip that has one picture, and then two pictures, inside the transition at its edge.
- Then change `mode_at` to agree with the measures.
