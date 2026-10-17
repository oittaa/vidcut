# HDR dissolves in the viewer as exported

**When:** not decided.

The viewer and picture renders mix a dissolve's pictures after converting each to RGB (tone-mapped for HDR), while an HDR export mixes them in its PQ or HLG code values, so mid-dissolve they differ (spec D50). Where every picture of a mix has the same colors (the usual HDR project), mixing their code values before the conversion would make them match.
