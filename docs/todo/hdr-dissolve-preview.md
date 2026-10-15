# HDR dissolves in the viewer as exported

The viewer and picture renders mix a dissolve's pictures after converting each to RGB (tone-mapped for an HDR picture), while an HDR export mixes them in its PQ or HLG code values, so mid-dissolve they show otherwise (spec D50). Where every picture of a mix has the same colors (the usual HDR project), mixing their code values before the conversion would make them match.
