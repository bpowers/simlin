# External-tool probe models

Questions this branch could not settle from documentation or from the
checked-in ground truth. Each model is small, self-contained, and uses values
chosen so that **every candidate rule produces a different output**.

Nothing here is committed to `test/`. Each `.mdl` carries a generated sketch
(`cargo run -p simlin-engine --example layout_probe_models`) so it opens with a
visible diagram; that harness splices only the sketch block and leaves the
equation text byte-identical.

| model | tool | status |
|---|---|---|
| `elm_map_computed_source.mdl` | Vensim DSS | **answered 2026-08-04** |
| `repeated_dimension.mdl` | Vensim DSS | **answered 2026-08-04** |
| `elm_map_variable_sources.mdl` | Vensim DSS | awaiting a run |
| `stella_repeated_dimension.stmx` | Stella | awaiting a run |

---

## 1. `elm_map_computed_source.mdl` — ANSWERED 2026-08-04

**Question:** is an inline expression legal as argument 1 of `VECTOR ELM MAP`,
and if so what storage does the mapping range over?

**Result: Rule R.** Vensim refuses to simulate the model:

> `Argument 1 to function VECTOR ELM MAP must be a normal variable`

(raised for `probe elem expr`). Inline expressions are rejected outright. The
model aborted before producing values, so the `probe helper *` rows went
unanswered — model 3 re-asks them.

| variable | equation | **R: rejected** (measured) | V: transparent, full storage | T: confined to the temp (Simlin) |
|---|---|---|---|---|
| `ctl slice` | `VECTOR ELM MAP(d[DimA,B1], off[DimA])` | — (aborted) | `1,1,5,5,6,6` | `1,1,5,5,6,6` |
| `probe slice expr` | `VECTOR ELM MAP(d[DimA,B1] * 1, off[DimA])` | **error** ✅ | `1,1,5,5,6,6` | `1,1,2,2,2,2` |
| `ctl elem` | `VECTOR ELM MAP(x[three], DimA - 1)` | — (aborted) | `3,4,5` | `3,4,5` |
| `probe elem expr` | `VECTOR ELM MAP(x[three] * 1, DimA - 1)` | **error** ✅ | `3,4,5` | `3,:NA:,:NA:` |

**What this settles.** There is no Vensim behaviour to match, because Vensim has
none: the shape is a syntax error. Simlin accepting it is an **extension**, and
the extension is *defined* by helper-equivalence — an inline expression means
exactly what the same values pre-assigned to a named helper variable mean, which
is the spelling that IS legal Vensim. The temp-confined semantics implement that
definition and are no longer provisional.

## 2. `repeated_dimension.mdl` — ANSWERED 2026-08-04

**Question:** does Vensim accept a variable declared over the same subscript
range twice (`sq[DimA,DimA]`), and if so what does reading it mean?

**Result: Rule A.** Vensim refuses to simulate the model:

> `DimA appears more than once on LHS`

(raised for `probe copy`). The declaration is illegal Vensim.

| variable | equation | **A: declaration rejected** (measured) | D: diagonal (Simlin) | F: true 2-D |
|---|---|---|---|---|
| `probe copy` | `sq[DimA,DimA]` | **error** ✅ | `11,11,11,22,22,22,33,33,33` | `11,12,13,21,22,23,31,32,33` |
| `probe sort` | `VECTOR SORT ORDER(sq[DimA,DimA], 1)` | — (aborted) | `0,0,0,1,1,1,2,2,2` | `0,1,2,0,1,2,0,1,2` |
| `probe sum` | `SUM(sq[DimA!,DimA!])` | — (aborted) | `66` | `198` |

**What this settles, and what it does not.** The shape is **unreachable from MDL
import**: no Vensim model can declare it, so no imported model can contain it.
That bounds Simlin's repeated-dimension residual family to hand-authored
XMILE / JSON / protobuf.

It does **not** make the shape illegitimate. The XMILE v1.0 spec exemplifies the
declaration directly — `docs/reference/xmile-v1.0.html` shows "A 2D
non-apply-to-all array with dimensions X by X, where X is size 2" with
`<dim name="X"/><dim name="X"/>` (verified in-repo). So a conformant XMILE file
may contain it and Simlin must keep reading it. Note the spec exemplifies only
the **declaration**, with per-element equations; it says nothing about what a
*reference* such as `sq[X,X]` on a right-hand side means, which is exactly the
part Simlin gets wrong. Model 4 asks Stella.

---

## 3. `elm_map_variable_sources.mdl` — AWAITING A VENSIM RUN

The follow-up model 1 could not answer, with **no expression arguments
anywhere**, so it simulates. Every argument is a variable reference.

**The one live question.** For the whole-variable spelling
`VECTOR ELM MAP(helper[DimA], off[DimA])`, Simlin collapses the base to 0 —
making it identical to `VECTOR ELM MAP(helper[A1], off[DimA])`. Taking the base
from the reference instead (what the documented rule says, and what `vector.dat`
shows for the *strict slice* `d[DimA,B1]`) predicts something different,
including two `:NA:`s. `vector.dat` covers the strict-slice spelling but **not**
this one.

Fixture: `d` flat storage `[1,4,2,5,3,6]`; `off = [0,1,1]`; `off2 = [0,1,2]`;
`helper[DimA] = d[DimA,B1]` = `[1,2,3]`; `x = [1,2,3,4,5]`, `three` its third
element. The Simlin column is **measured**, not predicted.

| variable | equation | Simlin (measured) | base-from-reference predicts | R2: Vensim rejects the spelling |
|---|---|---|---|---|
| `ctl slice` | `VECTOR ELM MAP(d[DimA,B1], off[DimA])` | `1,1,5,5,6,6` ✅ matches `vector.dat` | `1,1,5,5,6,6` | — |
| `ctl elem` | `VECTOR ELM MAP(x[three], (DimA - 1))` | **fails to compile** (MDL import only — see below) | `3,4,5` | — |
| `ctl elem off` | `VECTOR ELM MAP(x[three], off2[DimA])` | `3,4,5` ✅ | `3,4,5` | — |
| `probe helper elem` | `VECTOR ELM MAP(helper[A1], off[DimA])` | `1,1,2,2,2,2` | `1,1,2,2,2,2` (base is 0 here by construction) | — |
| **`probe helper slice`** | **`VECTOR ELM MAP(helper[DimA], off[DimA])`** | **`1,1,2,2,2,2`** | **`1,1,3,3,:NA:,:NA:`** | error |

**How to read the result.**

- Vensim prints `1,1,3,3,:NA:,:NA:` → Simlin has a real **base bug** for the
  whole-variable source spelling: it should take the base from the reference (as
  it already does for a strict slice) and does not.
- Vensim prints `1,1,2,2,2,2` → Simlin matches; the two helper spellings are
  genuinely one thing.
- Vensim errors on `helper[DimA]` as argument 1 → Rule R2, also an answer: the
  legal spelling is element-pinned only, and Simlin accepting the whole-array
  spelling is another extension to define rather than match.

**Note on `ctl elem`.** It is spelled byte-identically to `vector.mdl`'s `y` and
should print `3,4,5` in Vensim. Simlin fails it **through MDL import only**: the
importer turns `y[DimA] = ...` into per-element `Arrayed` slots, where `DimA - 1`
has no active apply-to-all dimension to resolve against. The same equation via
XMILE gives `3,4,5` — which is why the corpus, which runs `vector.xmile`, never
caught it. Separate importer defect, recorded rather than fixed here;
`ctl elem off` is the control to read.

## 4. `stella_repeated_dimension.stmx` — AWAITING A STELLA RUN

The tiebreaker for model 2: the XMILE spec exemplifies an `X by X` declaration,
Vensim rejects the equivalent, so Stella decides whether any shipping tool reads
the shape and how.

`sq[i,j] = 10*i + j`, so every cell is distinct. Simlin column **measured**:

| variable | equation | Simlin (measured) | true 2-D | diagonal |
|---|---|---|---|---|
| `probe_copy` | `sq[X, X]` | **`11,11,11, 22,22,22, 33,33,33`** | `11,12,13, 21,22,23, 31,32,33` | same as Simlin |
| `probe_row` | `SUM(sq[X, *])` | `36, 66, 96` ✅ | `36, 66, 96` | `11, 22, 33` |
| `probe_sum` | `SUM(sq[*, *])` | `198` ✅ | `198` | `66` |

This sharpens what Simlin's defect actually is. The **storage is a correct 2-D
array** — the reducers read all nine distinct cells and agree with the true 2-D
column. Only the subscripted **reference** `sq[X,X]` collapses, because both
subscripts resolve to the first axis. So the residual is confined to
reference resolution, not to array construction or reduction, which is a much
smaller thing to fix than the earlier framing suggested.

If Stella prints the true 2-D row for `probe_copy`, Simlin has a plain bug with
an external referent. If Stella rejects the declaration, the spec example is a
dead letter in both major tools and the shape is Simlin-and-spec-only.
