# Vensim probe models

Two questions this branch could not settle from documentation or from the
checked-in ground truth. Each `.mdl` is small, self-contained, and uses values
chosen so that **every candidate rule produces a different output**.

Please open each in Vensim DSS, run it, and report the values of the `probe *`
variables (and any error Vensim raises — an error IS an answer here). The
`ctl *` variables are controls that reproduce output we already have from
`test/sdeverywhere/models/vector/vector.dat`; if a control disagrees, the sheet
is mis-set-up and nothing else on it can be read.

Nothing here is committed to `test/` until we have real output.

---

## 1. `elm_map_computed_source.mdl`

**What is already settled.** The Vensim reference page for `VECTOR ELM MAP`
(retrieved 2026-08-02) says the function "returns the value of **the variable**
that is offset from vec by the specified amount", and that an offset "outside
**the range of the variable**" yields `:NA:`. Its multi-subscript example makes
the addressing explicitly flat over the whole variable:

```
v2[sub,tub,gub] = VECTOR ELM MAP(v[s1,t1,g1],
   (sub-1)*ELMCOUNT(tub)*ELMCOUNT(gub) + (tub-1)*ELMCOUNT(gub) + (gub-1))
```

Our ground truth confirms it: in `vector.mdl`,
`f[DimA,DimB] = VECTOR ELM MAP(d[DimA,B1], a[DimA])` prints `1,1,5,5,6,6`, and
`f[A2,B1] = 5 = d[A2,B2]` — the mapping read **past its own `B1` slice** into the
next row of `d`'s storage. Simlin implements exactly this.

**What is NOT settled.** Every example in the documentation, and every case in
the corpus, spells argument 1 as a **variable reference pinned to an element**.
The page never shows an inline expression there and never says whether one is
legal. An expression has no "variable" and no "range of the variable", so the
documented rule does not reach it. Simlin currently materializes such an operand
into a temp and confines the mapping to it; that choice is unverified.

Fixture: `d` is `[DimA,DimB]` with flat storage `[1,4,2,5,3,6]`;
`off = [0,1,1]`; `helper[DimA] = d[DimA,B1]` = `[1,2,3]`;
`x = [1,2,3,4,5]` and `three` is its third element.

| variable | equation | Rule **R**: Vensim rejects an expression | Rule **V**: expression transparent, still addresses `d`'s full storage | Rule **T**: expression materialized, mapping confined to it (Simlin today) |
|---|---|---|---|---|
| `ctl slice` | `VECTOR ELM MAP(d[DimA,B1], off[DimA])` | `1,1,5,5,6,6` | `1,1,5,5,6,6` | `1,1,5,5,6,6` |
| `probe slice expr` | `VECTOR ELM MAP(d[DimA,B1] * 1, off[DimA])` | error | `1,1,5,5,6,6` | `1,1,2,2,2,2` |
| `ctl elem` | `VECTOR ELM MAP(x[three], DimA - 1)` | `3,4,5` | `3,4,5` | `3,4,5` |
| `probe elem expr` | `VECTOR ELM MAP(x[three] * 1, DimA - 1)` | error | `3,4,5` | refuses to compile |

Note the last cell: `x[three]` collapses to a SINGLE element, so the computed
operand carries no array shape and Simlin declines to materialize it -- the
equation does not compile (measured: "Cannot push view for expression type ...
expected array expression"). Rule T's own arithmetic would give `3,:NA:,:NA:`
if it did materialize, but it does not, so a Vensim error on this row is
AGREEMENT with Simlin rather than a change to make.

Two further rows test the **equivalence claim** Simlin's choice rests on — that a
computed source behaves like the same values assigned to a variable first:

| variable | equation | hand-derived from the documented full-storage rule | Simlin today (measured) |
|---|---|---|---|
| `probe helper elem` | `VECTOR ELM MAP(helper[A1], off[DimA])` | `1,1,2,2,2,2` | `1,1,2,2,2,2` |
| `probe helper slice` | `VECTOR ELM MAP(helper[DimA], off[DimA])` | `1,1,3,3,:NA:,:NA:` | `1,1,2,2,2,2` |

`probe helper elem` is what Simlin's Rule T claims to be equivalent to. If
Vensim accepts `probe slice expr` and prints the same values as
`probe helper elem`, the claim holds and Rule T is right. If it prints
`1,1,5,5,6,6` instead, Rule V is right and Simlin is wrong. If it errors, Rule R
is right and Simlin should refuse the shape rather than define it.

The two columns DISAGREE on `probe helper slice`, and that disagreement is a
third question rather than a typo. Hand-deriving from the documented rule, the
base comes from the reference, so `helper[A2]` bases at flat 1 and offset 1
reads `helper[2] = 3`. Simlin instead treats `helper[DimA]` inside a vector
builtin as the WHOLE variable, which its `source_is_full_array` test bases at 0
-- giving the same answer as `helper[A1]`. Whichever Vensim prints tells us
whether the base is taken from the reference's spelling or from the variable's
extent, and Simlin has to move if it is the former.

## 2. `repeated_dimension.mdl`

**Question:** does Vensim accept a variable declared over the same subscript
range twice (`sq[DimA,DimA]`), and if so what does reading it mean?

Simlin accepts it and reads a **diagonal**: every projection between an array and
a temp matches by dimension name and takes the first hit, so `out[i,j]` reads
`sq[i,i]`. That behaviour predates this branch and is pinned as a disclosed
residual, not endorsed. If Vensim rejects the declaration, the shape is
unreachable from imported models and the residual is bounded to hand-authored
XMILE; if Vensim reads all nine cells, Simlin has a plain bug to fix.

Fixture: `sq[A_i,A_j]` = `10*i + j` (so `11,12,13,21,22,23,31,32,33`).

| variable | equation | Rule **A**: Vensim rejects the declaration | Rule **D**: diagonal / first-axis-wins (Simlin today) | Rule **F**: a true 2-D array |
|---|---|---|---|---|
| `probe copy` | `sq[DimA,DimA]` | error | `11,11,11,22,22,22,33,33,33` | `11,12,13,21,22,23,31,32,33` |
| `probe sort` | `VECTOR SORT ORDER(sq[DimA,DimA], 1)` | error | `0,0,0,1,1,1,2,2,2` | `0,1,2,0,1,2,0,1,2` |
| `probe sum` | `SUM(sq[DimA!,DimA!])` | error | `66` (diagonal only) | `198` (all nine) |

An error on the **declaration** answers all three rows at once; if only some
rows error, please report which.

Caveat on `probe sum`, which unlike the two rows above does NOT cleanly separate
D from F: repeating one `!` range in a single reference is its own Vensim
question, and a genuinely 2-D `sq` could still sum the diagonal to `66` if
Vensim binds both occurrences of `DimA!` to one index. So `198` is decisive for
F, but `66` is not decisive for D. `probe copy` and `probe sort` are the rows to
read; if you can, also try `SUM(sq[DimA!,DimA2!])` with a second range mapped to
`DimA`, which asks the all-nine question without the repeated-index confound.
