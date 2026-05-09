# model_editing/run_1..run_10 — ground-truth diff analysis

Source: `test/bobby/vdf/model_editing/{1_empty..10_reformat}.mdl` + `run_{1..10}.vdf`.
Each `run_N.vdf` is a clean Vensim simulation of model `N` (a "Save As" snapshot of
one evolving model). No participant filtering exists in Vensim, so each VDF contains
*every* variable/dimension/helper that exists in that model version.

All 10 are `exact-by-xray` in the current tool. The point of this analysis: confirm
*which decoded rule* produces the mapping (vs. which "reconstruction" the tool falls
back to), using the progression as ground truth.

## Model inventory per version

| run | model change | OT-bearing variables (besides Time) | OT count | records | names | slots |
|-----|--------------|-------------------------------------|----------|---------|-------|-------|
| 1 | control params only | FINAL TIME, INITIAL TIME, SAVEPER, TIME STEP | 5 | 7 | 9 | 8 |
| 2 | + `sub1:a,b,c`, `sub3:x,y` | (same — dims have no time series) | 5 | 14 | 15 | 15 |
| 3 | + `v = 3.14*Time` | + v | 6 | 15 | 16 | 16 |
| 4 | + `constant=3.1415`; `v=constant*Time` | + constant | 7 | 16 | 17 | 17 |
| 5 | + `flow=v`, `stock=INTEG(flow,2)` | + flow, stock(stock) | 9 | 18 | 19 | 19 |
| 6 | `flow[sub3]=v*sub3`, `stock[sub3]=INTEG(..,2*sub3)` | flow→flow[x],flow[y]; stock→stock[x],stock[y] | 11 | 18 | 19 | 19 |
| 7 | + `sub2:i,j` | (same as 6 — sub2 unused) | 11 | 21 | 22 | 22 |
| 8 | `flow[sub2]`, `stock[sub2]` | (same shape — sub2 size 2 like sub3) | 11 | 21 | 22 | 22 |
| 9 | `v = constant*SMOOTH(Time,1)` | + `#v>SMOOTH#` (the SMOOTH level, a stock) | 12 | 23 | 27 | 25 |
| 10 | sketch reformat only (semantically == model 9) | (same as 9) | 12 | 24 | 28 | 26 |

Key takeaways visible already:
- Adding a *dimension* (run_1→2, run_6→7) adds names+records (dim anchor + element
  records) but **no OT entries**. Section 5 grows by one set per dim.
- Adding a *scalar variable* (run_2→3, run_3→4) adds 1 name, 1 record, 1 OT.
- Adding *stock+flow* (run_4→5) adds 2 names, 2 records, 2 OT.
- *Arraying* an existing variable (run_5→6) changes its record's `f[6]` from `5`
  (scalar) to `32` (arrayed), adds a section-3 shape template, expands its OT block
  from 1 to N slots. **No new records/names** (the dims a,b,c,x,y already existed).
- Adding a SMOOTH call (run_8→9) adds the SMOOTH state variable as a `#alias>FUNC#`
  signature name (`#v>SMOOTH#`), with its own record and OT entry (class 0x08, stock,
  since SMOOTH1's output *is* its level). Plus stdlib helper names. (Details: see
  `helper_signature_region.md`.)
- A pure *sketch reformat* (run_9→10) still adds 1 record + 1 name + 1 slot — re-save
  cruft. Vensim also **compacts** stale section-3 placeholder templates on a fresh
  save (run_8/9 carry 2 sec3 entries, run_10 carries 1). (Details: `helper_signature_region.md`.)

## The decoded extraction chain (confirmed run_1..run_10, subscripts.vdf)

For every OT-bearing variable in these files there is exactly **one** section-1 record,
records are stored in **name-table order** (`f[2]` monotonically increasing), and the
mapping is *direct* — no ordering/alphabetical reconstruction required:

1. **Record → name**: `name = names[ (name_string_start − sec2_data_start)/4 + 7 == f[2] ]`.
   (Already pinned: `build_record_name_key_to_name_index`.)
2. **Record → OT start**: `f[11]` (owner interpretation). For these files `f[11]` is
   always a valid OT index in `[1, OT_count)` for owner records.
3. **Record → shape / OT span length**:
   - `f[6] == 5` → scalar → length 1.
   - `f[6] == 32` → arrayed; there is exactly one *active* section-3 entry
     (`flat_size > 0`) → length = that entry's `flat_size`.
   - (Multi-template files like Ref.vdf use `f[6]` = a section-3 self-positional
     `index_word` rather than the generic 32; not exercised by these fixtures.)
   - OT span = `[f[11], f[11] + length)`; the section-6 class codes over that span
     are homogeneous (all 0x08 / all 0x11 / all 0x17).
4. **Non-owner record kinds** (these have `f[11]` meaning something else, or 0):
   - `f[1] == 138` → view-header / unit-annotation record; `f[6]==0`, `f[11]==0`. NOT an owner.
   - `f[1] == 23 (0x17) AND f[6]==0 AND f[11]==0` → the `.N <viewname>` group record. NOT an owner.
   - dimension **anchor**: `f[6]==0`, `f[8]` = a small positive non-sentinel group id,
     `f[14] == SENT` (0xf6800000), `f[11]` = compact dim id, `f[2]` → the dim's name.
   - dimension **element**: `f[6]==0`, `f[8]` = the same group id, `f[14] == 0`,
     `f[11]` = the 0-based element index, `f[2]` → the element's name. (Element idx 0
     records carry `f[1]==33924 (0x8484)`; later elements `f[1]==131 (0x83)`. Anchors
     carry `f[1] ∈ {131, 135, 5905}` — `f[1]` is *not* a stable anchor marker; `f[14]`
     is.)
   - system records use `f[1] ∈ {15 (0x0f, INITIAL TIME), 23 (0x17, FINAL/SAVEPER/TIME STEP)}`
     and ARE owners (`f[6]==5`, `f[11]` = their OT). (`INITIAL TIME` uniquely gets `f[1]==15`;
     the other three are `f[1]==23`.)

### Array element labels (confirmed run_6/7/8/10, subscripts.vdf)

Given an arrayed owner record (`f[6] != 5`):
- The section-3 entry it uses (the single active one for `f[6]==32`) has `axis_refs`.
- Each `axis_ref` is **`60 + 16*k`** where `k` is the *record index* of the dimension
  anchor for that axis. (Derivation: `sec1.data_offset + 4*axis_ref == anchor.file_offset + 36`
  and `anchor.file_offset == sec1.data_offset + 204 + 64*k`, so `axis_ref == 60 + 16k`.)
  Reader inverts: `k = (axis_ref − 60) / 16`.
- That anchor's `f[8]` group id → its element records → their `f[2]`-names ordered by
  `f[11]` (0-based element index). For multi-axis shapes, axes are read in `axis_refs`
  order and elements enumerated row-major.
- The owner's OT slots get labels `name[elem...]` (1-D) / `name[e0,e1,...]` (n-D) in
  subscript order.

**Stale section-3 templates are skipped by `flat_size == 0`.** run_8 carries two
sec3 entries: entry 0 (`idx_word=59`, `flat=2`, axis→sub2 anchor) is active; entry 1
(`idx_word=0`, `flat=0`, axis→sub3 anchor) is a placeholder left over from run_7's
`flow[sub3]`. The owner records' `f[6]==32` resolves to the single active entry, so
`flow[sub2]`/`stock[sub2]` get `[i]`/`[j]` (sub2 elements), not `[x]`/`[y]`. No
cardinality-guessing needed even though sub2 and sub3 are both size 2.

### Section 5 (confirmed run_2..run_10, subscripts.vdf)

One section-5 set per declared dimension. Set file order == dim-anchor order sorted by
`f[8]` ascending (signal #13). `set.n` == the dim's cardinality (number of elements).
The set's payload refs are "axis-participation tokens" (slot offsets of variables that
use that dim, or — for the last/empty set — TIME STEP/SAVEPER fillers). The trailing
ref usually equals a section-3 axis ref or 0 (last set). Section 5 is *not* needed for
the array chain above (record `f[8]` groups carry the element catalog directly); it's
the fallback path + the source of subrange-element recovery on Ref.vdf.

## What is *not* needed for these fixtures (i.e. heuristics that don't fire here)

- "stocks-first-alphabetical" OT ordering — a *consequence* of Vensim's compiler
  allocation, not a read mechanism. The records give the map directly.
- `_select_non_overlapping_owner_blocks` (interval DP) — no record-span overlaps exist.
- slot-table "skip N leading slots" alignment scoring — display-only; doesn't affect
  extraction (`run_9`/`run_10` show "skip 2"; harmless).
- lookupish-name filtering — these files have no lookups (`header[0x70] == 0`).
- `_array_element_labels_from_sort_anchor` / unique-cardinality guessing — the
  section-3 axis-ref → anchor binding is exact.

Conclusion: the entire model_editing corpus + subscripts.vdf is decodable by the
direct record chain. The "reconstruction" machinery in xray exists for fixtures where
records overlap (owner vs graphical-function descriptor — see `owner_descriptor.md`)
or are missing/zeroed (the `.Supplementary` tail in WRLD3 — see
`helper_signature_region.md`). Cleaning up xray means: make `extract_named_results`
prefer the direct `decoded_record_spans` map when it is complete + non-overlapping +
covers all non-system OT slots, and only invoke reconstruction for the residue.
