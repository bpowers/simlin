# VDF corpus survey: version markers, `not-proven` re-examination, dataset/0x53 layout

Investigation date: 2026-05-08. Scope: every `.vdf` under `test/` and
`third_party/` (141 files: 130 with magic `7f f7 17 52`, 4 with `7f f7 17 41`
datasets, 5 with `7f f7 17 53` sensitivity runs, plus the duplicated
`praxis/.../Ref.vdf`). Tooling: `tools/vdf_xray.py` + scratch scans in `/tmp`.
Nothing in `vdf_xray.py`, `vdf.rs`, or `vdf.md` was edited.

---

## Part (a): version-marker hunt — verdict: **NO version field exists**

I dumped every documented-"constant" header word and every section-header
`field1..field5` across the entire 2007–2026 corpus and checked whether any
value partitions the corpus by era (2005/2007/2008 vs 2015 vs 2019 vs 2026) or
by any other consistent split. **No word does.** Every "constant" stays
constant across all 19 years; the only differences are container-type splits
(`0x41` dataset vs `0x52`/`0x53` result) and runtime-pointer residue (values
that vary even between two reruns of the same model — these are the doc's
already-documented "arena residue").

### "Constant" header words across all 130 `0x52` files

| Offset | Value (all 130 `0x52` files) | Notes |
|--------|------------------------------|-------|
| `0x50` | `0x012C0065` everywhere (`65 00 2C 01`) | Universal; also identical on `0x41` and `0x53`. Reads `0x012c=300`, `0x65=101`. Not era-sensitive. |
| `0x54` | `0` on 124/130; `1` on 6 (see below) | Doc says "Zero". The 6 exceptions are NOT an era group — all 2007/2008, from just two models (`ZamMod1/2` by Zambaqui, `T21NA` by SimService). Looks like a per-save flag (e.g. "saved with optimization control"), not a version. |
| `0x68` | `0` on 127/130; nonzero only on the 3 `0x53` files | Container/run-mode marker, not era. (Confirmed: paired `0x52` zambaqui runs have `0x68==0`.) |
| `0x80..0x90` | all-zero (five u32) on 127/130 | Three files (`zambaqui/bp-1.vdf`, `bp-2.vdf`, `old runs/new land - 2.vdf`) carry a pointer-shaped u32 at `0x8c` (`0x3531xxxxx`/`0x3833xxxxx` range) — same residue family as `0x94`. Not a version field. |
| `0x94` | small int OR `0x0bXXXXXX`/`0x02b1xxxx`/`0x001fxxxx` arena pointer | Already documented as volatile runtime residue. Varies between reruns of the same model. NOT an era marker (e.g. 2008 `pop/Current.vdf` has `0x1f`; 2026 `model_editing/run_*.vdf` all have `0x0b8857d0`; 2019 `policy.vdf` has `0x001f995c`). |
| `0x98` | `1` on all `0x52`/`0x53`; `0` on `0x41` | Container marker. |
| `0x9c, 0xa0` | `0` everywhere | — |
| `0xa4` | `0x00430000` on all `0x52`/`0x53`; `0x01000000` on `0x41` | Container marker (`0x00430000` as f32 = 128.0; `0x01000000` as u32 = 1). NOT era. |
| `0x6c` | wildly variable; small garbage on tiny files, file-offset-shaped on big ones | Already flagged as "not a reliable version field". On `0x53` files it equals the end of the normal sparse-block run; on a 4307-byte `bact` file it's `0x2d4f` (= 11599, past EOF) ⇒ pure residue. Definitely not a version. |

### Section-header `field3` — a constant per-section "kind" code (parser ignores it)

`field3` is uniform per section index and identical across all years and both
result containers:

| Section | `field3` (all `0x52`/`0x53`) | `field3` (`0x41` dataset) |
|---------|------------------------------|---------------------------|
| 0 (sim cmd) | 500 | 500 |
| 1 (string/meta) | 500 | 500 |
| 2 (name table) | 500 | **135** (name-table section in datasets) |
| 3 (array dir) | **135** | 500 |
| 4 (view) | 500 | 500 |
| 5 (dim sets) | 500 | *(no sec 5)* |
| 6 (OT meta) | **100** | *(no sec 6)* |
| 7 (lookup/OT/data) | 500 | *(no sec 7)* |

So `field3` is a section-type discriminator: name-table section → 135,
array-directory section → 135, OT-metadata section → 100, everything else →
500. (`0x1f4=500`, `0x87=135`, `0x64=100`.) The dataset format keeps the same
codes but the *positions* are shifted: in a dataset, section 2 carries the name
table and gets `135`. This is the cleanest "what kind of section is this" hint
in the file, but it has zero era variation, so it is not a version marker. It's
purely supplementary to position-based section identification — worth a
one-line mention in `vdf.md`'s "Section roles" table as a confirmation, not as
a decoder dependency.

### `field5 >> 16` patterns (also constant, not era-sensitive)

- `sec0.field5 >> 16 == 26 (0x1A)` on every result file (and `sec0.field5 ==
  0x001A0000` exactly). On `0x41` datasets, `sec0.field5 == 0`.
- `sec2.field5 >> 16 == 6` on every result file ("first name length" — `"Time"`
  is 4 chars; the `6` is `4 + 2` (the leading `\x00\x00` padding `"Time\x00\x00"`),
  consistent with the doc's note that `"Time"` has no length prefix and the
  6 comes from the header). Same `6` in datasets' name-table section.
- `sec3.field5 == 1`, `sec3.field4 == 0` (scalar) or `32` (arrayed) — already
  documented.

### The `0x80..0xA7` region (`vdf.md` says it "does not affect decoded output")

Confirmed: bytes `0x80,0x84,0x88,0x8C,0x90` are zero in 127/130 `0x52` files
(three files leak an arena pointer at `0x8c`); `0x94` is the residue word;
`0x98=1`; `0x9C=0`, `0xA0=0`; `0xA4=0x00430000`. There is no version field
hiding here — just zero padding + the volatile `0x94` residue + the `0xA4`
container constant. The doc's framing is accurate.

### Section 0 (sim-command section) — no version field

Contains `[u16 len][sim command string]` + zero padding + a trailing `0x4C`
('L') byte followed by one u32 whose value tracks `sec0.field4` (roughly the
command-string length). E.g. `"sim  Current -I "`, `"sim  base -I -d data"`,
`"sim  opt-1 -I -p ZamMod2.vpd -d Data -w "`. `sec0.field4` varies with command
length; no version stamp.

### Section-1 head words (`vdf.md` "three stable words")

Re-confirmed: `data[0..4] == 124` (or 188 on `SCEN01.VDF`), `data[4..8] ==
ot_count - 1 - max_stock_ot_index`, `data[8..12] == sec6_lookup_record_count`.
On `0x41` datasets the analogous head is `60, N_records, 0, ...` (different
base constant `60` instead of `124`, and `data[4]` = the record count). No
version field.

### Bottom line for (a)

Bob Eberlein never wrote an explicit version word into VDF. The format evolved
*compatibly*: 2008-era Vensim writes small integers into the `block0[0..11]`
header-block region (and `data[4..8]` etc.) where 2019+ Vensim writes
arena-pointer-shaped (still-deterministic) values, but there is no marker
saying which regime applies — a reader must just tolerate whatever is there
(which is exactly what the current parser does, since it never reads those
words for decoding). The only structural fork is the **magic byte** itself
(`0x41` = dataset/reference-mode, `0x52` = run, `0x53` = sensitivity/optimization
run with an extra `0x68`-anchored payload). The `field3` per-section kind codes
and `0xa4`/`0x98` are container discriminators, not version stamps.

---

## Part (b): `not-proven` fixtures — artifact vs real ambiguity

The corpus-precision table currently marks 9 fixtures `not-proven`, **all with
the single blocker `record-span-overlap`**. I drilled into every one. Two
distinct things are conflated under that label:

### B.1 — `record-span-overlap` itself: **a real, possibly-ill-posed format ambiguity** (not a tool artifact)

In *every* `not-proven` fixture the overlapping spans are the `field[11]`
union-field conflict the doc devotes an entire appendix to: a graphical-function /
lookup-table **descriptor record** whose `field[11]` is meant as a section-6
lookup-record index but happens to also be a valid OT start, sitting on the
same OT slot(s) as a real **owner record** (an internal SMOOTH/DELAY state stock
in the scalar cases, a real carbon-cycle array variable in `Ref.vdf`).

| Fixture | overlap slots | overlap kind | extraction result count vs OT | verdict |
|---------|---------------|--------------|-------------------------------|---------|
| `lookups/lookup_ex.vdf` | 1 (OT[1]) | `"lookup table 1"` (standalone-lookup descr, `f11=1`) vs `"stock"` (real owner, `f11=1`) | 8 = 8 ✓ | Real ambiguity; **extraction is in fact correct** (the lookup descr is correctly not emitted because OT[1] is otherwise used). |
| `econ/base.vdf` | 3 (OT[1..3]) | `"hud policy lookup"` / `"inflation rate lookup"` / `"loan standards impact on insolvency table"` (lookup descrs, `f11=1/2/3`) vs `#LV1<DELAY1(...)#` / `#SMOOTH(...)#` (internal stocks, same `f11`) | 78 = 78 ✓ | Same; extraction correct. The 1:1 lookup-record-count-to-lookupish-name match resolves it in practice. |
| `econ/mark2.vdf`, `econ/policy.vdf` | 3 | same pattern, new-style names (`#perceived HPI>SMOOTH#` etc.) | 82 of 84 (see B.2) | Real overlap; the 82≠84 gap is a *separate* issue (B.2). |
| `econ/risk2.vdf` | 1 | `"loan standards impact on insolvency table"` vs `#SMOOTH(indexedHPI,...)#` | 91 of 93 (see B.2) | same |
| `econ/rk.vdf` | 3 | same pattern as base | 76 of 78 (see B.2) | same |
| `WRLD3-03/experiment.vdf`, `WRLD3-03/SCEN01.VDF` | 54 each | `"...table"`/`"...LOOKUP"` descriptors (one per OT slot in [1..54]) vs `#...>SMOOTH...#` / `#LV1<...>#` / `#LV2<...>#` internal stocks occupying OT[1..54] | 297 = 297 ✓ | Real overlap, extremely structured (54-vs-54 1:1). Extraction correct. Name-category filtering *would* resolve it here, but the doc rightly notes that doesn't generalize (`Ref.vdf`). |
| `xmutil_test_models/Ref.vdf` | 58 | the harder array-range-straddle form: `RS N2O` (`f11=113`, claims 7-elem OT[113..120)) crosses `C AF Sequestered` / `C in Atmosphere` / `C in Biomass`; `"Annual rate of emissions to target"` (`f11=106`) exactly overlaps `RS HFC4310mee` (`f11=106`) | 3219 of 3914 (this fixture also has the well-known 455 no-data OT entries + the unsolved C-LEARN view-grouping) | Real ambiguity, the *worst* case; the descriptor names (`RS *`) don't look like lookups, so name filtering fails (the doc's stated counterexample). |

So `record-span-overlap` is **not** the kind of "blocker that turns out to be
an artifact" the doc's history is full of (risk2 name-table parse, the
record-finder fix). It is the genuine `field[11]`-union discriminator gap. The
doc already argues this may be **formally ill-posed from the reader's
perspective**: section-6 lookup records carry everything Vensim needs to
evaluate a lookup (x/y arrays = `word[5..6]`, output OT = `word[10]`, input
deps = `word[12]` chains), so the reader may simply *ignore* `field[11]` on
descriptor records, leaving the OT-vs-lookup-index union undisambiguated
on disk. If that's true, `record-span-overlap` is an honest "the overlap is
real, not decodable" flag, and none of these 9 can become `exact-by-xray`
without observing Vensim's reader behavior or finding a discriminator byte
that ~200 candidate tests (`/tmp/vdf_discriminator_hunt.md`, etc.) have not
found. **My assessment: `lookup_ex`, `base`, `WRLD3 experiment`, `SCEN01` are
fixtures where the extracted name→data mapping is already 100% correct and the
blocker is a conservative-honesty flag; `mark2`, `policy`, `risk2`, `rk` are
the same plus the B.2 bug; `Ref.vdf` is the one with genuinely unresolved
structural ambiguity that affects output.**

### B.2 — A real, fixable tool fragility on edited/re-saved econ + model_editing files: `build_owner_record_blocks`'s `hidden`-rule

`build_owner_record_blocks` (`vdf_xray.py` ≈ L1867-1871) marks an owner block
`hidden` when *all* its slot refs are in `preferred_slot_name_alignment(...).
hidden_slots` and each slot ref is unshared. `hidden_slots` comes from the
exploratory display-only slot-name alignment heuristic — which `vdf.md` itself
warns "must not be used as evidence for on-disk refs". The fact-only span
report (`decoded_record_spans`) is *fine* — it covers all OT slots — but the
hide-rule throws away 1–3 owner blocks on these fixtures:

| Fixture | OT | xray results | hidden blocks (all verified to point at real data blocks) |
|---------|----|--------------|-----------------------------------------------------------|
| `econ/base.vdf` | 78 | 78 | (none) — slot layout happens not to trigger the rule |
| `econ/rk.vdf` | 78 | 76 | OT[4],OT[5] = `#SMOOTH(interest...)#`, `#SMOOTH(realinflationrate,3)#` |
| `econ/risk.vdf` | 87 | 84 | OT[3],OT[4],OT[5] = three `#SMOOTH(...)#` states — **and yet `exact-by-xray`** |
| `econ/risk2.vdf` | 93 | 91 | OT[3],OT[4] = `#SMOOTH(averageriskofderivatives,6)#`, `#SMOOTH(indexedHPI,...)#` |
| `econ/mark2.vdf` | 84 | 82 | OT[4],OT[5] = `#perceived mortgage balance>SMOOTH#`, `#perceived risk of insolvency>SMOOTH#` |
| `econ/policy.vdf` | 84 | 82 | same as mark2 |
| `model_editing/run_9.vdf`, `run_10.vdf` | 12 | 11 | OT[1] = `#v>SMOOTH#` — **and `exact-by-xray`** (the doc endorses this hiding, signal #9/#10) |

Note `base.vdf` and `rk.vdf` are the same model (identical file size), yet
base hides 0 and rk hides 2 — purely because their slot-table layouts differ
slightly and the `hidden_slots` heuristic reacts to that. **That run-to-run
inconsistency is the bug**: whether `#SMOOTH(realinflationrate,3)#` is treated
as a result shouldn't depend on which run you opened. The *intent* (hide
internal SMOOTH/DELAY state stocks because they aren't user-facing series) is
the documented, deliberate behavior for run_9/run_10/risk; the *mechanism*
(deriving the hide set from `preferred_slot_name_alignment`) is fragile and
should instead key off the decoded `#SMOOTH(...)#` / `#alias>FUNC>LV1#` name
patterns (which are stable and already classified — see `vdf.md` "Two stdlib
signature encodings"). Also note: the Rust `to_results_via_records` path
*keeps* `#SMOOTH(...)#` series (it only drops `f[6]==0` records), so xray and
Rust diverge here. Either decision can be defensible, but they should agree,
and the result count must not be a function of slot-layout noise. → I'm filing
this as a tracked issue.

Crucially: B.2 is *independent* of `not-proven` status. `risk.vdf` and
`run_9`/`run_10` have the B.2 hiding and are still `exact-by-xray`; the four
econ files (`mark2`, `policy`, `risk2`, `rk`) are `not-proven` only because
they *also* have the B.1 lookup-descriptor overlap.

### B.3 — Could any `not-proven` fixture become `exact-by-xray` with a better-decoded rule?

- **No** for all 9 via a *fully-decoded* rule, unless the `field[11]` owner/
  descriptor discriminator gets decoded (the doc credibly argues it may be
  ill-posed). The extraction is already correct on `lookup_ex`, `base`, `WRLD3
  experiment`, `SCEN01` — only the conservative fact-only overlap check flags
  them.
- A pragmatic path: if `vdf.md` promotes "section-6 lookup records correspond
  1:1 to lookup-definition names in name-table / section-7-packed-data order"
  to a *pinned fact* (it's already stated for section 7), then on every fixture
  except `Ref.vdf` the lookup-descriptor side of each overlap is identifiable
  ("this record's `field[11]` indexes lookup record k; ignore it as an owner
  span"), which would let the precision report drop `record-span-overlap` for
  the 8 non-`Ref` fixtures. `Ref.vdf` would still need the array-straddle
  discriminator. (This is a *reconstruction* still — but a much more principled
  one than the current "lexical lookupish name test" — and it's what
  `heuristics_audit.md` Category A already flags as "likely decodable".)
- `Ref.vdf` is the genuinely ill-posed one and is the right place to keep the
  honest `record-span-overlap` flag.

---

## Part (c): dataset (`0x41`) and `0x53`-family layout notes

### C.1 — Dataset / reference-mode VDF (`7f f7 17 41`): confirms + extends `vdf.md`

Inspected `test/bobby/vdf/econ/data.vdf` (13537 B, 11 series), `third_party/
uib_sd/{spring_2008,fall_2008/econ,}zambaqui/Data.vdf` (~14952 B, 80+ series,
the zambaqui ones identical), `third_party/uib_sd/fall_2008/econ/data.vdf`
(identical to `test/bobby/vdf/econ/data.vdf`).

Confirmed from `vdf.md`:
- magic `7f f7 17 41`; **5 sections** (0..4).
- section 0 carries the string/record area; section 1 carries the printable
  name table; section 4 holds a u32 block-offset list terminated by `0`, then
  reuses the same sparse-block encoding as result VDFs.
- 64-byte record layout (12-byte preamble + 3×64 header blocks, then records)
  is identical, just relocated to section 0.

New / extended details:
- **Header constants that flip for datasets**: `0xa4 = 0x01000000` (vs
  `0x00430000` for results), `0x98 = 0` (vs `1`). `0x78 = 0` (no
  `saved_time_point_count`); the time-point count lives at `0x7c` (= `0x74`,
  both = 225 for the econ rates dataset, 26 for the zambaqui dataset). The Rust
  `dataset_header_time_point_count` already handles this by trying
  `[0x7c, 0x78, 0x74]` in order.
- **Section-1 head constant**: `sec0.data[0..4] == 60` (`0x3C`), *not* 124.
  `sec0.data[4] ==` the record count (11 for econ rates, 80 for zambaqui).
- **Origin string** at `0x04` is descriptive prose, not a `(timestamp) From X.mdl`
  string: `"rates_vensim.xls converted to dataset on Tue Nov 04 13:08:42 2008"`,
  `"Data.xls converted to dataset on Wed Apr 16 09:10:35 2008"`.
- **Record `field[11]` = dataset block index** (not OT index): econ rates
  records have `f[11]` ∈ {0,1,2,3,5,6,7,8,9,10} matching the 1+9 block offsets
  in section 4 (`[first_data_block, 4331, 5258, 6189, 7120, 8051, 8982, 9909,
  10792, 11723]`). Record `field[12] == 60` (one slot group). `field[1] ∈
  {15 (system `Date`/time-related), 138 (the `.<dataset name>` view header), 11
  (dynamic non-stock)}`. Record→name link is "sort records by `(f[2],
  file_offset)`, pair 1:1 with name-table-order non-`Time`/non-`.` names" —
  matches the Rust `series_bindings` impl and the doc's MEMORY note about
  `build_deterministic_ot_map`.
- **Section 4 header `field4/field5`** carry *data*, not the usual small ints:
  econ rates has `sec4.field4=12606 field5=3404` (looks like the first two
  block-offset-list entries or packed counts); contrast result VDFs where
  `sec4.field4` is ~8/14/152. The zambaqui dataset (`f4=1340 f5=2`) is an
  **arrayed** dataset (its record 11 has `f[6]==32`, and section 2 carries a
  real array-shape entry `index_word=86`) — its section-4 block list parsed as
  empty in my naïve scan, meaning the first word at `sec4.data_offset()` is `0`;
  arrayed datasets evidently put the block list slightly differently. (The Rust
  parser's `data.vdf` path handles `econ/data.vdf` but I did not verify it on
  the arrayed zambaqui `Data.vdf` — worth a follow-up if datasets ever become a
  priority.)
- The `--corpus-precision` table only tracks `test/bobby/vdf/econ/data.vdf` as
  `dataset/not-implemented` (xray.py prints "Dataset parsing not yet implemented
  in this tool" and emits a stub row); the Rust `VdfDatasetFile` *is*
  implemented and extracts series for `econ/data.vdf`.

### C.2 — `0x53` sensitivity/optimization runs (`7f f7 17 53`)

Inspected all 5 (`spring_2008/zambaqui/opt-1.vdf` + `sens-train_cost.vdf`,
`zambaqui/{opt-1,sens-train_cost}.vdf` [dup of spring_2008], `zambaqui/old runs/
sensi-1.vdf`). All from 2008 (`ZamMod1/2/3.mdl`).

Confirmed from `vdf.md`:
- 8-section layout; header offsets `0x58/0x5c/0x60`, section-6 class/final/lookup
  tail, offset table, and the normal sparse-block run all parse with the `0x52`
  rules. (xray.py already accepts `0x53` as a result-family container.)
- `header[0x68]` is **nonzero** (`0x5a2b6` / `0x5bde8` / `0x5bdb0`) and points
  past the normal sparse-block run. In the paired `0x52` zambaqui runs (`baserun.vdf`,
  `test-1.vdf`, …) `0x68 == 0`.
- `header[0x6c]` is also nonzero here (`0x5a1d2` / `0x5bcde` / `0x5bca6`) and
  marks ~the end of the normal sparse-block run: the max OT-referenced block
  offset is `0x5a164` / `0x5bc70` / `0x5bc38` (within a block's-worth of `0x6c`).
  So `0x6c ≈ end of base-run blocks`, `0x68 ≈ start of the extra payload`, with
  a ~250–270-byte "gap" between them.

What's in the gap `[0x6c .. 0x68]` (≈266 bytes on opt-1): a stream of
fixed-width records, ~20 u32 per record, containing `0x3f800000` (=1.0f) and
nearby-1.0 floats (`0x3f7f3756`≈0.997, `0x3f5a8003`≈0.854, `0x3fa3e002`≈1.28),
packed `(1, k)` pairs (`0x10001`, `0x10087`, `0x10091`, …), and several
file-offset-shaped u32 values (`0x150b3`, `0x13f5f`, `0x1422b`, …) plus
sentinel-range words (`0xf6c75023` ≈ -2e33, `0xe9ab0000`). Best guess:
**per-sampled-parameter descriptors** (a multiplier near 1.0 + the slot/range
of the parameter being perturbed) — i.e. the sensitivity-design table.

What's in the big payload `[0x68 .. EOF]` (114 KB on sensi-1, 273 KB on opt-1,
455 KB on sens-train_cost — *bigger* than the base run's 260-ish KB of blocks):
it starts with a ~15-word header `[0xa0/0xc8, 0x47, <ptr=0x68+0x3c>, 0x47,
0xc00fcc, 0xc00fd0, 0x10001, 0x6, 0x2df, 0x4, 0xc, 0x121, 0x471, 0x484, 0x509]`
— the `0x47 = 71` is the `0x78`/`0x7c` time-point count, repeated; `0xc00fcc,
0xc00fd0` are consecutive (a range?); `0x10001` is the `(1,0)` packed pair;
the trailing `0x471, 0x484, 0x509` look like OT-range or record indices —
immediately followed by a long run of monotonically-increasing f32 values
(`90.8165, 2000000.0, 2075676.125, 2147199.75, …` on opt-1 — plausibly a
Zambaqui `total population`-type series starting ~2M). So the `0x68` payload is
**a second simulation-output region with the same x/y-array + sparse-block
structure as the base run** — i.e. the sensitivity / optimization **ensemble of
alternate runs**, each storing the same set of saved variables. The base run in
sections 0–7 is "run 0"; the `0x68` block holds runs 1..N (and the gap holds
the per-run parameter samples). Whether each ensemble member is a full
`offset_table_count`-wide block or a compact subset, and how the `0xc00fcc`-style
counts decode, I did not pursue (rabbit hole per the mandate). The doc's
existing characterization ("treat data past the normal sparse-block run as
unknown; do not assume sensitivity/optimization semantics are decoded") is
correct; what I'd add to it is the structural sketch above: gap = parameter-
sample records, `0x68` payload = ensemble of additional simulation-output runs
sharing the base run's block layout.

`0x53` does **not** have its own header constant beyond `0x68 != 0`: `0xa4 =
0x00430000`, `0x98 = 1` (same as `0x52`), `0x50 = 0x012C0065`, `field3` per-
section codes identical. So `0x53` is best read as "`0x52` plus the `0x68`
ensemble payload"; a reader that wants only the base-run series can treat a
`0x53` file exactly like a `0x52` file and ignore everything past `0x6c`.

---

## Tracked issues filed during this investigation

1. `build_owner_record_blocks`'s `hidden`-rule derives its hide-set from the
   exploratory `preferred_slot_name_alignment(...).hidden_slots` heuristic,
   making the extracted result count depend on slot-table layout noise (e.g.
   `econ/base.vdf` hides 0 internal-SMOOTH blocks, the identical-model
   `econ/rk.vdf` hides 2; both should behave the same). The intent — hiding
   internal SMOOTH/DELAY state stocks because they aren't user-facing series —
   is documented and deliberate (signal #9/#10), but it should key off the
   decoded `#SMOOTH(...)#` / `#alias>FUNC>LV1#` name patterns instead, and
   xray should agree with the Rust `to_results_via_records` decision (which
   currently *keeps* those series). See B.2.
2. `vdf_xray.py` only stubs dataset (`0x41`) parsing ("Dataset parsing not yet
   implemented in this tool") even though `src/simlin-engine/src/vdf.rs`
   `VdfDatasetFile` *is* implemented; the corpus-precision table can't audit
   dataset extraction. (Lower priority.) Also: the Rust dataset path is not
   verified on *arrayed* datasets (the zambaqui `Data.vdf` has an arrayed
   series and a different section-4 layout). See C.1.
