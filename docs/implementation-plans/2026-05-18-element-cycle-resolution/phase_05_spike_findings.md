# Phase 5 Spike: VECTOR ELM MAP base + full-source-stride resolution

**Status:** Mostly clean and implementable for the f/g cross-dimension case
(AC6.1, AC6.2, AC6.4) with a small, well-scoped VM change. **One genuine
obstacle blocks AC6.3** as the phase file frames it: `vector.xmile`'s
`y[DimA] = VECTOR ELM MAP(x[three], (DimA - 1))` does not compile *at all*
today — a compiler-side failure that is not the VM base+stride bug and is
larger than Task 2's "VM change, optionally a view-construction tweak"
anticipates. Detail and options in section 6.

**Verified:** 2026-05-18, branch `clearn-hero-model`, by reading the
compilation pipeline and by running the real `vector_simple.mdl` /
`vector.mdl` through the engine with a temporary read-only diagnostic
(reverted; no engine code changed by this spike).

---

## 1. Current line numbers (phase file references had drifted)

| Structure | Phase file said | Actual (verified) |
|---|---|---|
| `Opcode::VectorElmMap` handler | `vm.rs:2301-2356` | `vm.rs:2304-2356` (comment `2301-2303`) |
| `read_view_element` | `vm.rs:2664-2684` | `vm.rs:2684-2697` |
| `flat_offset` | `bytecode.rs:281-309` | `bytecode.rs:283-309` |
| `RuntimeView` struct | `bytecode.rs:160-181` | `bytecode.rs:164-181` |
| `apply_single_subscript` | `bytecode.rs:311-345` | `bytecode.rs:312-345` |

Supporting (not in the phase file, needed by Task 2):

- ELM MAP source/offset views are pushed in `codegen.rs:1086-1095`
  (`AssignTemp` → `walk_expr_as_view(source)`, `walk_expr_as_view(offset)`,
  `Opcode::VectorElmMap`).
- `walk_expr_as_view`: `codegen.rs:283-361`.
- `StaticArrayView` → `RuntimeView`: `bytecode.rs:1060-1073`
  (`to_runtime_view` copies `base_off`, `offset`, `dims`, `strides`,
  `sparse` verbatim — **the slice's full addressing survives onto the view
  stack**).
- The lowering that shapes the source-arg view:
  `context.rs:1369-1418` (`build_view_from_ops` at `subscript.rs:306-486`).
- Row-major declared-order strides: `context.rs:1315-1318`
  ("Calculate original strides (row-major)").

---

## 2. Ground truth from the real fixtures (empirical, not assumed)

Read with `cat -A` (`.dat` is tab-separated ASCII; `Read` misfires).

`vector_simple.mdl` / `vector.mdl` share these definitions:

```
DimA: A1, A2, A3      DimB: B1, B2
a[DimA] = 0, 1, 1
b[DimB] = 1, 2
c[DimA] = 10 + VECTOR ELM MAP(b[B1], a[DimA])
d[A1,B1]=1 d[A2,B1]=2 d[A3,B1]=3 d[A1,B2]=4 d[A2,B2]=5 d[A3,B2]=6
e[A1,B1]=0 e[A2,B1]=1 e[A3,B1]=0 e[A1,B2]=1 e[A2,B2]=0 e[A3,B2]=1
f[DimA,DimB] = VECTOR ELM MAP(d[DimA,B1], a[DimA])
g[DimA,DimB] = VECTOR ELM MAP(d[DimA,B1], e[DimA,DimB])
```

`vector.mdl` additionally has `DimX: one,two,three,four,five`,
`x[DimX]=1,2,3,4,5`, and `y[DimA] = VECTOR ELM MAP(x[three], (DimA - 1))`.

Genuine-Vensim ground truth (`vector_simple.dat`, `vector.dat`):

| var | genuine value |
|---|---|
| `c` | `c[A1]=11, c[A2]=12, c[A3]=12` |
| `f` | `f[A1,*]=1, f[A2,*]=5, f[A3,*]=6` (constant across DimB) |
| `g` | `g[A1,B1]=1, g[A1,B2]=4, g[A2,B1]=5, g[A2,B2]=2, g[A3,B1]=3, g[A3,B2]=6` |
| `y` | `y[A1]=3, y[A2]=4, y[A3]=5` |

`vector_simple.dat` has no `c` column (it is MDL-only here; `c` ground
truth comes from `vector.dat`, identical model rows).

### Current engine output (real MDL run, before Task 2)

`vector_simple.mdl` and `vector.mdl` (with the failing `y` line removed)
both produce, identically:

```
c[a1]=11  c[a2]=12  c[a3]=12          <- already matches genuine
f[a1,*]=1 f[a2,*]=2 f[a3,*]=2          <- WRONG (genuine 1,5,6)
g[a1,b1]=1 g[a1,b2]=2 g[a2,b1]=2
g[a2,b2]=1 g[a3,b1]=1 g[a3,b2]=2       <- WRONG (genuine 1,4,5,2,3,6)
```

`vector.mdl` *with* `y` fails to compile entirely:
`NotSimulatable: "failed to compile fragments for variables: y"`.
A 4-line model containing only `x` and that one `y` equation reproduces
the same failure, with **no model-level diagnostic emitted** (silent until
the assembly stage at `db.rs:4979`).

So there are two independent problems:

- **Problem A — the VM base+stride bug.** `c` (1-D source) is already
  correct; `f`/`g` (cross-dimension source `d[DimA,B1]`) are wrong. This is
  exactly the AC6.1/AC6.2/AC6.4 target and is cleanly fixable (sections 3–5).
- **Problem B — `y` does not compile.** A scalar source (`x[three]`) with
  an arithmetic offset (`(DimA - 1)`) never reaches the VM. This blocks
  AC6.3's "un-exclude `vector.xmile`" as written (section 6).

---

## 3. How the source-arg view reaches the opcode

`VECTOR ELM MAP(src, offs)` is hoisted into `AssignTemp(temp, App(VEM,
src, offs), out_view)`. `find_expr_array_view` (`mod.rs:872-879`) takes the
**offset** argument's view as the result shape — result element count and
iteration come from `offs`, not `src`.

`codegen.rs:1086` emits, in order: a static view for `src`, a static view
for `offs`, then `Opcode::VectorElmMap`. The opcode reads
`offset_view = view_stack[len-1]`, `source_view = view_stack[len-2]`
(`vm.rs:2305-2306`).

The source arg is lowered under `with_vector_builtin_wildcards`
(`context.rs:2239-2245`: `promote_active_dim_ref = true`,
`preserve_wildcards_for_iteration = true`). Three shapes occur in the
fixtures:

**`b[B1]` (1-D, fully collapsed to scalar).** `view.dims.is_empty()` is
true and `promote_active_dim_ref` is set, so `context.rs:1396-1417`
promotes the `Single(B1)` op to `Wildcard` and rebuilds the **full `b`
array** view: `base_off=b`, `offset=0`, `dims=[2]`, `strides=[1]`. The base
established by the (now-promoted-away) element reference is `0`. This is
why `c` is already correct: `result[i] = b[round(offs[i])]` over the full
2-element `b`, `offs = a = [0,1,1]` ⇒ `[1,2,2]`, `+10` ⇒ `[11,12,12]`. ✓

**`d[DimA,B1]` (2-D, DimA is the A2A active dim of `f`/`g`, B1 a named
element).** This hits the `preserve_for_iteration` branch
(`context.rs:1369-1387`): `ActiveDimRef(DimA)` → `Wildcard`, but the
`Single(B1)` is **kept**. `build_view_from_ops` over `orig_dims=[3,2]`,
`orig_strides=[2,1]` (row-major, declared order DimA-outer, DimB-inner):

- DimA → `Wildcard`: keeps `new_dims=[3]`, `new_strides=[2]`.
- DimB → `Single(0)` (B1 is element index 0): dimension removed,
  `offset_adjustment += 0 * 1 = 0`.

Result `ArrayView`: `base_off=d`, `offset=0`, `dims=[3]`, `strides=[2]`,
`dim_names=["DimA"]`. Carried verbatim onto the view stack by
`to_runtime_view`.

**`x[three]` (1-D, fully collapsed to scalar — `y`'s source).** Same shape
as `b[B1]`: promoted to full `x` (`dims=[5]`, `strides=[1]`, base 0). The
source view is fine; the failure is elsewhere (section 6).

### The actual bug in the opcode

`vm.rs:2311-2329` flattens `source_view` into a `source_values` vec by
iterating exactly `source_view.size()` elements. For `d[DimA,B1]` that is 3
elements at `flat_offset` `{0, 2, 4}` (offset 0, stride 2) ⇒
`source_values = [d[A1,B1], d[A2,B1], d[A3,B1]] = [1, 2, 3]`.

Then `vm.rs:2337-2353` computes, for each result element `i`,
`source_values[round(offset[i])]` with out-of-range ⇒ NaN. There is **no
per-element base** and the materialized vec is only the 3-element B1
column — it cannot address the B2 column at all.

`f` (offset `a = [0,1,1]`): `source_values[0]=1`, `[1]=2`, `[1]=2` ⇒
`f=[1,2,2]`. Confirmed by the real run. Genuine wants `[1,5,6]`.

The OOB→NaN guard at `vm.rs:2348-2352` is genuine Vensim and stays.

---

## 4. The chosen base + stride computation (Problem A)

Genuine Vensim, per the user-approved correction: result element `i` =
`source[base_i + offset[i]]` over the **full source array**, last subscript
fastest, out-of-range ⇒ `:NA:`/NaN, **no modulo**.

Two quantities are needed at the opcode and are both already encoded in
`source_view` (a `RuntimeView`):

**(a) Per-result-element base `base_i`.** This is the flat position the
first-argument element reference established, expressed in the source
variable's storage. The view's `offset` field already holds the static
part of it (the slice start). For the cross-dimension case the *remaining*
free axis of the source view is the dimension the offset walks; the base
for result element `i` is the source view's flat offset at the source's
position-`i` along that axis with the offset contribution set to zero.

Concretely: after the source arg is lowered, `source_view` has exactly one
remaining free dimension (the wildcard-promoted axis — `DimA` for
`d[DimA,B1]`, the whole dim for the promoted 1-D `b`/`x` cases) plus
`offset` carrying the collapsed-subscript contribution. The per-element
base is

```
base_i = source_view.offset + i * source_view.strides[free_axis_for_i]
```

where `i` is the result element's index *projected onto the source's free
axis* (for `f`/`g`, result iterates `DimA × DimB`; the source free axis is
`DimA`; the projection is `result_index / DimB_size`, i.e. broadcast `DimB`).
For the promoted 1-D source (`c`), the source has no collapsed offset
(`offset = 0`), the free axis stride is `1`, and `base_i` reduces to `0`
when the offset reference is element-position-independent — which is why
`c` already works and must keep working.

**(b) Full-array innermost stride for the offset step.** The offset
indexes *into the full source array*, innermost-fastest. The innermost
stride of the full source array is the stride of the source variable's
last declared dimension. For a `RuntimeView` that was sliced from a
contiguous variable, the smallest positive stride present (or `1` for a
contiguous full array) is that innermost stride. For the fixture sources it
is `1` in every case (`d` is row-major contiguous; `strides=[2,1]`, the
last/innermost dim stride is `1`; the promoted 1-D `b`/`x` have stride
`1`). So:

```
flat_i  = base_i + round(offset[i]) * innermost_source_stride
value_i = curr[source_view.base_off + flat_i]      // via read_view_element
```

with `value_i = NaN` when `offset[i]` is NaN **or** `flat_i` falls outside
`[source_view_full_lo, source_view_full_hi)` — the full storage extent of
the source variable, *not* the 3-element slice. The full extent is
`[min over the source's full dims of base, that + product(full_dims))`;
in practice, since the source variable is contiguous, it is
`[var_base, var_base + full_len)` where `full_len` is the product of the
source variable's *original* dimension sizes.

### Exact change to `vm.rs:2311-2353`

Replace the "materialize a flat `source_values` vec then index it" block
with a direct addressed read against the full source array:

1. Delete the `source_values` materialization loop (`vm.rs:2311-2329`).
2. Compute, once: `inner_stride` = the innermost (smallest positive)
   stride of `source_view` — for the contiguous fixtures this is `1`;
   compute it as `*source_view.strides.iter().filter(|s| **s > 0).min()`
   defaulting to `1`. Compute the full source extent
   `[lo, hi)` from `source_view` (`lo = source_view.offset` rebased to the
   variable; `hi = lo + full_source_len`). The full length is recoverable
   from the source variable's declared dims; the simplest robust route is
   to push the **full source-array view** for ELM MAP rather than the
   sliced one (see "compiler-side option" below), making `lo = 0`,
   `hi = source_view.size()`, and the base derivation trivial.
3. In the existing per-offset-element loop (`vm.rs:2337-2353`), for result
   element `i` compute `base_i` per (a) and
   `flat_i = base_i + round(offset_val) * inner_stride`; write
   `temp_storage[temp_off + i] = if offset_val.is_nan() || flat_i out of
   [lo,hi) { NaN } else { read_view_element(source_full, flat_i, ...) }`.
4. **No `rem_euclid`, no modulo.** `vm.rs:102`'s `Op2::Mod` is unrelated.

**Compiler-side option (recommended, smaller VM diff).** The cleanest
shape is for `walk_expr_as_view(source)` to push the **full source-array
view** (all original dims, `offset = 0`) for ELM MAP, plus enough metadata
to recover `base_i`. The lowering at `context.rs:1396-1417` already does
exactly this promotion for the fully-collapsed 1-D case (`b[B1]` → full
`b`). Extending the same "promote `Single` → `Wildcard`" treatment to the
`preserve_for_iteration` branch (`context.rs:1369-1387`) for ELM MAP's
source argument — promoting the kept `Single(B1)` to `Wildcard` and
folding its offset (`B1`'s element index along DimB, here `0`) into a
per-result-element base — turns `d[DimA,B1]` into the full 2-D `d` view
(`dims=[3,2]`, `strides=[2,1]`, `offset=0`). Then the opcode needs only:

```
base_i = (i / DimB_size) * d.strides[DimA] + B1_index * d.strides[DimB]
flat_i = base_i + round(offset[i]) * d.strides[innermost]   // *1
```

The B1-index term is the "base from the first-arg element reference" the
phase file describes. For the fixtures `B1_index = 0`, so `base_i` is just
the `DimA`-row start, exactly reproducing genuine Vensim (verified in
section 5). This keeps the OOB→NaN test against the *full* `d` length (6),
not the slice (3).

Either route is correct; the recommended one (push full view + derive base
from the collapsed-subscript offset) localizes the change and makes the
base term explicit. Task 2 should pick the compiler-side promotion: it
reuses the existing, tested 1-D promotion code path and keeps the VM
opcode a straightforward "full-array indexed read with a per-element base".

---

## 5. Hand-derivation vs genuine `.dat` (Problem A)

Source `d` full storage, row-major declared order `d[DimA,DimB]`,
`strides=[2,1]`, flat layout:

```
idx: 0      1      2      3      4      5
val: d11=1  d12=4  d21=2  d22=5  d31=3  d32=6
```

(`d[Ai,Bj]` flat index `= (i-1)*2 + (j-1)`.)

### `f[DimA,DimB] = VECTOR ELM MAP(d[DimA,B1], a[DimA])`

Result iterates `DimA × DimB`. Source free axis = `DimA`; `B1_index = 0`.
`base` for `DimA = Ak` is `(k-1)*2 + 0*1 = (k-1)*2`. Offset broadcasts
`a[DimA]` across `DimB`: `a = [0,1,1]`.

| elem | base | offset | flat = base + offset*1 | d[flat] | genuine |
|---|---|---|---|---|---|
| A1,* | 0 | 0 | 0 | 1 | 1 ✓ |
| A2,* | 2 | 1 | 3 | 5 | 5 ✓ |
| A3,* | 4 | 1 | 5 | 6 | 6 ✓ |

Reproduces `f = [1,5,6]` exactly.

### `g[DimA,DimB] = VECTOR ELM MAP(d[DimA,B1], e[DimA,DimB])`

Same base as `f`. Offset is the full 2-D `e`:
`e[A1,B1]=0,e[A1,B2]=1,e[A2,B1]=1,e[A2,B2]=0,e[A3,B1]=0,e[A3,B2]=1`.

| elem | base | offset | flat | d[flat] | genuine |
|---|---|---|---|---|---|
| A1,B1 | 0 | 0 | 0 | 1 | 1 ✓ |
| A1,B2 | 0 | 1 | 1 | 4 | 4 ✓ |
| A2,B1 | 2 | 1 | 3 | 5 | 5 ✓ |
| A2,B2 | 2 | 0 | 2 | 2 | 2 ✓ |
| A3,B1 | 4 | 0 | 4 | 3 | 3 ✓ |
| A3,B2 | 4 | 1 | 5 | 6 | 6 ✓ |

Reproduces `g = [1,4,5,2,3,6]` exactly.

### `c[DimA] = 10 + VECTOR ELM MAP(b[B1], a[DimA])`

Source promoted to full `b` (`dims=[2]`, `strides=[1]`, `offset=0`);
`base = 0` for all elements (the `B1` collapsed-subscript offset is `0` and
there is no free source axis the result projects onto — the source is fully
broadcast). `flat_i = 0 + round(a[i])*1`. `b` flat = `[1,2]`.

| elem | offset (a) | flat | b[flat] | +10 | genuine |
|---|---|---|---|---|---|
| A1 | 0 | 0 | 1 | 11 | 11 ✓ |
| A2 | 1 | 1 | 2 | 12 | 12 ✓ |
| A3 | 1 | 1 | 2 | 12 | 12 ✓ |

Reproduces `c = [11,12,12]` — and matches the current engine output, so
the fix must leave this case unchanged. The chosen formula does (base 0,
full 2-element source).

### `vector_simple` / `vector.xmile` OOB cases

No fixture offset lands out of range under the genuine rule (all `base_i +
offset[i]` stay inside the full source). So the numeric gates pass
identically whether OOB→NaN or a hypothetical wrap were used — consistent
with the phase file's note that no real offset wraps. OOB→NaN is kept
because it is genuine Vensim (`:NA:`), pinned by the existing
`out_of_bounds_*` / `negative_offset_*` unit tests, which stay (their NaN
premise is correct genuine Vensim; only their *base* changes, and for those
1-D `source[*]` fixtures `base = 0`, so they are already correct and need
no edit — confirmed against Task 3's scope).

**Conclusion for Problem A:** the base+stride approach is concrete,
localized, and reproduces every genuine `vector.dat` ELM MAP value
(`c`, `f`, `g`) exactly. AC6.1, AC6.2, AC6.4 are cleanly implementable.

---

## 6. Obstacle: `y = VECTOR ELM MAP(x[three], (DimA - 1))` does not compile

This blocks AC6.3 ("un-exclude `vector.xmile`", which contains `y`) and
AC6.2/Task 6's cited `y[A1]=3,y[A2]=4,y[A3]=5`. **It is not the VM
base+stride bug** — `y` never produces bytecode that runs.

**Reproduction:** a 4-variable model (`DimA`, `DimX`, `x[DimX]=1..5`,
`y[DimA] = VECTOR ELM MAP(x[three], (DimA - 1))`) fails with
`NotSimulatable: "failed to compile fragments for variables: y"` and emits
**no model-level diagnostic** (silent until the assembly stage,
`db.rs:4979-4983`).

**Root cause (from the pipeline):** ELM MAP takes its result shape from the
*offset* argument's view (`find_expr_array_view`, `mod.rs:872-879`:
`VectorElmMap(_, offset) -> find_expr_array_view(offset)`). That function
only returns a view for `Expr::StaticSubscript` / `Expr::TempArray`
(`mod.rs:874`). Here:

- The source `x[three]` is a **scalar** (a single element, fully
  collapsed), not an array view.
- The offset `(DimA - 1)` is an **arithmetic expression** (`Op2(Sub, …)`)
  over the iteration dimension's position, not a `StaticSubscript`.
  `find_expr_array_view` returns `None` for it.

So the hoister cannot derive the result array shape for `y`, the
`AssignTemp` fragment for `y` is never built, and assembly later reports
the missing fragment. `f`/`g`/`c` all have an *array-valued* offset arg
(`a[DimA]`, `e[DimA,DimB]`) that is a `StaticSubscript`, so they hoist
fine; `y` is the only fixture whose offset is a derived expression and
whose source is a scalar.

This is the same pattern the now-stale `simulate.rs:651-657` exclusion
comment already flagged ("`y[DimA] = VECTOR ELM MAP(x[three], (DimA-1))`
fails in VM incremental path") — except it fails at *compile/assembly*, not
in the VM.

### Why this is larger than Task 2 anticipates

Task 2 scopes itself to `vm.rs:2311-2353` plus "if the spike requires, the
compiler-side view construction that pushes the ELM MAP source argument".
Fixing `y` is a *different* compiler change: ELM MAP must derive its result
shape (and per-element offset values) when the offset is an arbitrary
scalar-valued expression evaluated per result element, and the source is a
scalar broadcast. That touches `find_expr_array_view` / the
`expand_arrayed_with_hoisting` shape inference (`mod.rs:1598-1650`), not
just the view push. It also needs the per-element-evaluated offset path
(the offset is `position(DimA) - 1`, recomputed for each `DimA` element),
which the current ELM MAP hoist (single whole-array offset view) does not
model.

### Recommended decision for the orchestrator (before Task 2)

Three viable paths, in order of preference:

1. **Scope AC6.3 to the ELM MAP variables `vector.xmile` can actually
   exercise, and narrow the `y` part to a tracked follow-up.** The phase
   file's Task 6 already contemplates narrowing: "narrow the gate to the
   ELM-MAP variables rather than weakening the whole comparison … only
   narrow with a tracked issue". Land Problem A (the `f`/`g`/`c` fix +
   `vector_simple.dat` + unit tests), include `vector.xmile` but exclude
   the `y` variable from the comparison, and file a `track-issue` for
   "ELM MAP with scalar source + per-element expression offset does not
   compile". This keeps Phase 5 green and delivers AC6.1/AC6.2/AC6.4 plus
   most of AC6.3 without scope creep.

2. **Expand Task 2 to also fix the scalar-source / expression-offset
   compile path.** Correct, but it is a separate, non-trivial compiler
   change (shape inference + per-element offset evaluation) that the phase
   plan did not budget. Only do this if AC6.3 must be fully met in Phase 5.

3. **Defer all of `vector.xmile` (keep it excluded), land Problem A via
   `vector_simple.dat` + unit tests only.** Weakest: loses the
   genuine-Vensim regression gate AC6.3 explicitly wants.

Option 1 is the best fit for the phase file's own "prefer full inclusion;
narrow with a tracked issue if a variable genuinely resists a general fix"
guidance. The `y` failure is a genuine, pre-existing, separately-scoped
compiler gap, not effort-avoidance.

---

## 7. Summary for Task 2's implementor

- **Do** implement the base+stride read for Problem A. Push the **full
  source-array view** for ELM MAP's source (extend the existing
  `context.rs:1396-1417` `Single → Wildcard` promotion to the
  `preserve_for_iteration` branch for the ELM MAP source arg), fold the
  collapsed-subscript element index into a per-result-element base, and in
  `vm.rs:2311-2353` compute `flat_i = base_i + round(offset[i]) *
  innermost_source_stride`, reading via `read_view_element` against the
  full source, OOB (vs the **full** source extent) ⇒ NaN.
- **Do not** add `rem_euclid`/modulo. **Do not** change the OOB→NaN
  semantics. **Do not** edit the 1-D `source[*]` OOB/negative unit tests'
  expectations (their base is 0; already genuine).
- The hand-derivation in section 5 reproduces genuine `vector.dat`
  `c=[11,12,12]`, `f=[1,5,6]`, `g=[1,4,5,2,3,6]` **exactly**.
- **Flag to orchestrator:** `y` (scalar source + expression offset) does
  not compile today; it is out of Task 2's scope. Prefer option 1 in
  section 6 (narrow AC6.3's `vector.xmile` gate to the ELM MAP array-offset
  variables + `track-issue` for `y`).
