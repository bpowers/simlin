# VDF internal-helper / `#`-signature / `.Supplementary` region

This note decodes how Vensim writes the internal stdlib-macro helper variables
(`SMOOTH`/`SMOOTH3`/`SMOOTHI`/`DELAY1`/`DELAY3`/`TREND`/`RAMP FROM TO`/`SSHAPE`/
`SAMPLE UNTIL` ...) and the re-save cruft that accumulates near them, and gives a
deterministic rule that distinguishes:

- (a) real internal-helper variables that own an OT slot and carry a saved series —
  **emit them** (under their `#...#` signature name);
- (b) slot-less placeholder / ghost records — **skip them**;
- (c) re-save cruft (stale view-name records, materialized metadata-tag records) —
  **handled by the same rule as (b)** for the OT-owner path.

It is grounded on `test/bobby/vdf/model_editing/run_{8,9,10}.vdf`,
`test/bobby/vdf/econ/{base,rk,policy,mark2,risk,risk2}.vdf`,
`test/metasd/WRLD3-03/{SCEN01.VDF,experiment.vdf}`, and `test/xmutil_test_models/Ref.vdf`.
Field names are the ones in `docs/design/vdf.md` ("Record fields" table); `f[N]` is
record word `N`. All byte offsets are file offsets.

Companion: `model_editing_diff.md` (the full run_1..run_10 progression).
Do NOT promote anything here into `vdf.md` without re-checking against the corpus.

--------------------------------------------------------------------------------

## 1. The `run_8 → run_9` diff (model: add one `SMOOTH(Time, 1)`)

Model 8: `flow[sub2]=v*sub2`, `stock[sub2]=INTEG(flow[sub2],2*sub2)`, `v=constant*Time`.
Model 9: identical except `v=constant*SMOOTH(Time, 1)`.

### Counts

| quantity | run_8 | run_9 | delta |
|----------|------:|------:|------:|
| records   | 21 | 23 | +2 |
| names     | 22 | 27 | +5 |
| slots     | 22 | 25 | +3 |
| OT entries| 11 | 12 | +1 |
| section-6 OT class codes | `0f 08 08 17 17 11 11 17 17 17 11` | `0f 08 08 08 17 17 11 11 17 17 17 11` | inserts `08` at OT[1] |

Byte offsets: run_8 records `0x22c..0x76c`, slot table `0x774` (22), name table `0x7d0`,
OT `0x1233` (11), class codes `0x11d4`. run_9 records `0x22c..0x7ec`, slot table `0x7f0`
(25), name table `0x858`, OT `0x12c8` (12), class codes `0x1264`.

### The +5 names (run_9 name table indices 22..26)

```
 22  "SMOOTH"             — stdlib-macro function-name token (call-site copy)
 23  "SMOOTH"             — stdlib-macro function-name token (macro-definition copy)
 24  "IN"                 — SMOOTH macro's first parameter name (the input)
 25  "ST"                 — SMOOTH macro's second parameter name (smoothing time)
 26  "#v>SMOOTH#"         — new-style output signature: the SMOOTH state variable
```

**Why two `SMOOTH` names.** Vensim's `SMOOTH(IN, ST)` macro emits its function-name
identifier into the name table twice: once where the *user equation* that calls it is
parsed (`v = constant*SMOOTH(Time, 1)`), and once when the macro module itself is
registered (the macro-definition header). The macro's two formal parameters `IN` and
`ST` are emitted once. The output is the level (SMOOTH1 has a single internal stock and
its output *is* that stock), so there is exactly one helper *variable*, written as the
new-style signature `#v>SMOOTH#` (`#<alias>>SMOOTH#`, alias = the calling variable `v`).

This confirms `#v>SMOOTH#` is at OT[1] with class code `0x08` (stock) and is a real
saved series: OT[1]'s offset-table entry is a data-block pointer (`0x138d`), block is
35/35 dense, `first=0.0 last=16.0` — `SMOOTH(Time, 1)` lagging Time by 1 month.

### The +3 slots

The slot table is 1:1 with the name table (`slot_table[i]` ↔ `names[i]`). The three new
slot entries belong to names 22 (`SMOOTH`), 23 (`SMOOTH`), 24 (`IN`). Names 25 (`ST`) and
26 (`#v>SMOOTH#`) have **no slot entry** — they sit past `slot_count` at the name-table
tail (consistent with `vdf.md`: `#`-signatures and stdlib-helper tail names lack slots).
Note also that the slot *value* for `Time` changed from `0x9c` (run_8) to `0x8` (run_9):
the slot blob region got rearranged on this re-save (slot blobs are runtime-descriptor
residue per `vdf.md`; their *values* are volatile, only the 1:1 pairing with names is
stable). Section-1 `block1[7]` (the "slot count hint" at `sec1.data_offset()+76`) reads
22 in run_8 and 24 in run_9 — i.e. `slot_count - block1[7]` jumps from 0 to 1, the
"+1 delta" already documented in `vdf.md` for `run_9`/`run_10`. The reserved slot is one
of the `SMOOTH`/`IN`/`ST` macro-block names whose slot is allocated without durable
content.

### The +2 records

Only two of the five new names get records:

```
rec[21]: f0=0x0020 f1=0x0083(131) f2=67  -> name 22 "SMOOTH"     f3=90  f6=0  f8=0 f9=0  f10=0  f11=8   f12=124
rec[22]: f0=0x4020   f1=0x0089(137) f2=77  -> name 26 "#v>SMOOTH#" f3=268 f6=5  f8=S f9=S  f10=5  f11=1   f12=412
```

- **rec[21]** is a *stub* record for the call-site `SMOOTH` function-name token:
  `f[6]==0` (no shape), `f[8]==f[9]==0` (no sentinel), and `f[11]==8` is *not* OT[1] —
  it is stale and not an owner OT start. Stub records like this also exist for `step`,
  `if then else`, `MIN`, etc. on other fixtures (`f[1]` low byte `0x89`/`0x8f`/`0x87`
  on builtin-function tokens, `0x83`/`0x84` on macro tokens). They never own a series.
- **rec[22]** is the real helper-stock record for `#v>SMOOTH#`: `f[6]==5` (scalar),
  `f[11]==1` → OT[1], whose class code is `0x08` (stock). The decoded record key
  (`f[2]==77`) maps it directly to `#v>SMOOTH#`. `f[12]==412` puts it in its own
  slot-group separate from the user-view group (`f[12]==124`).

**No records** for `SMOOTH` (name 23, macro-def copy), `IN` (name 24), or `ST` (name 25):
macro parameter / definition-header names are emitted to the name table (and the first
three of them are slotted) but carry no record.

### Determinism

For a model containing exactly one `SMOOTH1` call, the SMOOTH expansion deterministically
contributes: +1 OT entry (the level, class `0x08`, inserted into the contiguous stock
block), +2 records (one function-token stub + one `#alias>SMOOTH#` helper record), +5
names (`SMOOTH` ×2, `IN`, `ST`, `#alias>SMOOTH#`), and +3 slots (the three slotted names
`SMOOTH`, `SMOOTH`, `IN`). The macro-instance count is what scales these — see §3.

--------------------------------------------------------------------------------

## 2. The `run_9 → run_10` diff (cosmetic sketch reformat only)

Model 10 == model 9 semantically; only the sketch section was reordered (connection IDs
renumbered, `|0||` → `|12||` on the Time element, the cloud element gains a `48` ref).
Yet:

| quantity | run_9 | run_10 | delta |
|----------|------:|-------:|------:|
| records   | 23 | 24 | +1 |
| names     | 27 | 28 | +1 |
| slots     | 25 | 26 | +1 |
| OT entries| 12 | 12 |  0 |

The OT, the class codes, and every saved data block are identical between run_9 and
run_10 (final values byte-identical: `17 16 399.007 798.014 3.1415 17 50.264 100.528 0
0.5 0.25 50.264`). So the reformat changed nothing about variables — yet records/names/
slots each grew by one. Resolution: **the diff is a swap plus an addition**, net +1:

- run_9 has the view-group name `.9 smooth time` (name 20, with a record `f[1]==23` and a
  slot). run_10 does **not**.
- run_10 has the view-group name `.mdl` (name 5, record `f[1]==23`, slot) — this is a
  **corrupted/truncated re-save of the view title** "`.10 reformat.mdl`". (`run_8` had
  `.8 change subscript`, `run_9` had `.9 smooth time`; the re-parse mangled `.10
  reformat.mdl` down to `.mdl`. The sketch's `*View 1` line was also touched in model 10.)
- run_10 *additionally* has `:SUPPLEMENTARY` (name 21, record rec[20] = `f0=0x4020
  f1=0x0089 f2=59 f6=0 f8=S f9=S f10=0 f11=0 f12=124`, slot) — this is genuine re-save
  cruft (`f[6]==0`, `f[11]==0` ⇒ a slot-less stub, skipped by §4.3). The model has `v ~ ~ ~
  :SUPPLEMENTARY |` in models 8, 9, *and* 10, but only run_10 materializes the
  `:SUPPLEMENTARY` metadata keyword as a name-table entry + record + slot. Earlier saves
  did not. (`:`-prefix names are metadata tags, never OT owners — `vdf.md`.)

`-1` (`.9 smooth time` gone) `+1` (`.mdl`) `+1` (`:SUPPLEMENTARY`) `= +1`. Same arithmetic
for names, records, and slots.

(Note `f0=0x4020 f1=0x0089` on the `:SUPPLEMENTARY` stub is exactly the type/class pair
`#v>SMOOTH#` had in run_9 — these "stub" `f[0]`/`f[1]` patterns are recycled / opaque, not
meaningful types.)

Also visible on this re-save: the name table got fully re-sequenced into Vensim
compilation order (`Time, sysvars, .mdl, constant, flow, sub2, v, stock, sub1, a, b, c, i,
j, sub3, x, y, SMOOTH, :SUPPLEMENTARY, .Control, -Month, SMOOTH, IN, ST, #v>SMOOTH#`),
whereas run_7/8/9 carried an edit-accumulated ordering. The record `f[1]` values for many
variables also got "demoted" to stub-like values on run_10 (e.g. `constant`'s record:
`(f0=548, f1=5905)` in run_9 → `(f0=0, f1=138)` in run_10 — `(0, 138)` is the same pair
used for view-header / DELAY1-output records). The `f[2]` key and `f[6]`/`f[11]` are still
correct, so `to_results_via_records` is unaffected. Lesson for the larger corpus: **on a
re-saved file the only structurally trustworthy fields on a record are `f[2]` (name key),
`f[6]` (shape), `f[11]` (OT-or-lookup union), `f[12]` (slot group); `f[0]`/`f[1]` are
re-save-volatile and the `f[1]==138` "view header" signal can leak onto ordinary records.**

This run_9→run_10 transition is the smallest known case of "a re-save adds an entry that
does not correspond to any variable" — exactly the mechanism behind the record/name count
mismatches in larger fixtures (SCEN01: 419 records / 421 names; experiment: see §3).

--------------------------------------------------------------------------------

## 3. Decoded structure of `#`-signature names and their records

### 3.1 Name forms

Vensim writes stdlib-macro helpers in two coexisting encodings (already in `vdf.md`'s
"Two stdlib signature encodings" table; restated with the helper inventory):

| | output signature | internal-stock / internal-rate / internal-aux signatures |
|---|---|---|
| Old style | `#FUNC(args)#` | `#LV1<FUNC(args)#`, `#LV2<FUNC(args)#`, `#LV3<FUNC(args)#`, `#ST<FUNC(args)#`, `#DL<FUNC(args)#`, `#RT1<FUNC(args)#`, `#RT2<FUNC(args)#` |
| New style | `#alias>FUNC#` | `#alias>FUNC>LV1#`, `>LV2#`, `>LV3#`, `>ST#`, `>DL#`, `>RT1#`, `>RT2#`, and for multi-output macros `>linear#`/`>linear ramp#`/`>exp ramp#`/`>slope#`/`>rate#`/`>interval#`/`>input#` |

(The classifier in `VdfFile::output_signatures` / `new_style_alias_signatures` already
matches these — `>FUNC#` with exactly one `>` and not ending in an internal suffix is an
output; `#FUNC(...)#` with no `<` is an output; everything else is internal. Names like
`#BAU atm conc CO2#` in `Ref.vdf` have no `<`/`>`/`(` and are user *display* names, not
stdlib signatures — they are filtered out by requiring `(`/`>`.)

Per-macro helper inventory (matches Vensim's macro definitions and the WRLD3/econ
fixtures):

| macro call | helper variables emitted (output first) | OT slots |
|---|---|---|
| `SMOOTH(in,t)` (= SMOOTH1) | output == the level | 1 |
| `SMOOTHI(in,t,init)` | output == the level | 1 |
| `SMOOTH3(in,t)` | output == LV3; plus `LV2`, `LV1`, `DL` | 4 |
| `DELAY1(in,t)` | output (outflow rate); plus `LV1` | 2 |
| `DELAY3(in,t)` | output == RT3 (outflow rate); plus `LV3`, `LV2`, `LV1`, `RT2`, `RT1`, `DL` | 7 |
| `TREND(in,t,init)` | output; plus `AV` (averaged level) etc. | (not in current fixtures with records) |
| `RAMP FROM TO(...)` | output; plus `linear`, `linear ramp`, `exp ramp`, `slope`, `rate`, `interval` | 7 (`Ref.vdf`) |
| `SSHAPE(...)` | output; plus `input` | 2 (`Ref.vdf`) |
| `SAMPLE UNTIL(...)` | output | 1 (`Ref.vdf`) |

So a model with `K` macro instances emits `K` `#alias>FUNC#` (or `#FUNC(args)#`) output
names plus the per-macro internal-helper names; the OT count grows by `sum(helper-slot
counts)`. (On `econ/base.vdf` the four `#SMOOTH(...)#` outputs and one `#LV1<DELAY1(...)#`
plus one `#DELAY1(...)#` account exactly for 5 SMOOTH1 levels + 1 DELAY1 level + 1 DELAY1
output rate = the 7 macro-related OT entries beyond the user variables.)

### 3.2 Record layout for `#`-signature helpers

Every `#`-signature helper variable (output or internal) that owns an OT slot **has its
own record** with the same shape as any scalar variable: `f[6]==5`, `f[2]` keying the
`#...#` name via the standard formula `(name_string_start - sec2_data_start)/4 + 7`, and
`f[11]` = a *genuine* OT block start in `[1, ot_count)` whose class code is `0x08`
(internal stock) or `0x11` (internal rate/aux data block) — never `0x17`/`0x0f`. The
function-token stub and macro-parameter names get no record (or a `f[6]==0`, no-OT stub).

Examples (record index, raw fields, decoded name, OT class):

```
econ/policy.vdf:
  r92  f0=0x00000 f1=0x008a f2=648 f6=5 f10=5  f11=13(c=0x11)  #defaults>DELAY1#               (DELAY1 output, a rate)
  r93  f0=0x0302c f1=0x0011 f2=654 f6=5 f10=8  f11=1 (c=0x08)  #defaults>DELAY1>LV1#           (DELAY1 internal level)
  r94  f0=0x03028 f1=0x0008 f2=661 f6=5 f10=12 f11=3 (c=0x08)  #perceived inflation rate>SMOOTH#  (SMOOTH1 level)
  r95..r97  similar SMOOTH1 levels at OT[2], OT[5], OT[4].

WRLD3-03/experiment.vdf (new-style; one SMOOTH3 instance "Land Yield Factor 2"):
  r359 f0=0x03428 f1=0x0008 f2=3176 f6=5 f10=33 f11=11(c=0x08)  #Land Yield Factor 2>SMOOTH3#       (= LV3, the output)
  r360 f0=0x03428 f1=0x0008 f2=3185 f6=5 f10=41 f11=13(c=0x08)  #Land Yield Factor 2>SMOOTH3>LV2#
  r361 f0=0x03028 f1=0x0008 f2=3195 f6=5 f10=35 f11=44(c=0x11)  #Land Yield Factor 2>SMOOTH3>DL#    (delay/3 aux)
  r362 f0=0x03024 f1=0x0011 f2=3204 f6=5 f10=39 f11=12(c=0x08)  #Land Yield Factor 2>SMOOTH3>LV1#

WRLD3-03/experiment.vdf (one DELAY3 instance "persistent pollution appearance rate"):
  r369 #...>DELAY3#       f0=0x03028 f1=0x0008 f11=46(c=0x11)   (= RT3, the outflow rate)
  r370 #...>DELAY3>LV3#   f0=0x03424 f1=0x0011 f11=20(c=0x08)
  r371 #...>DELAY3>DL#    f0=0x03028 f1=0x0008 f11=47(c=0x11)
  r372 #...>DELAY3>RT2#   f0=0x0302c f1=0x0011 f11=49(c=0x11)
  r373 #...>DELAY3>LV2#   f0=0x03024 f1=0x0011 f11=19(c=0x08)
  r374 #...>DELAY3>RT1#   f0=0x03028 f1=0x0008 f11=48(c=0x11)
  r375 #...>DELAY3>LV1#   f0=0x03024 f1=0x0011 f11=18(c=0x08)
```

Note the LV slots straddle the contiguous stock-coded OT block (`0x08`) while DL/RT slots
land in the `0x11` data-block range; SCEN01/experiment keep the LV* of all macro instances
clustered at OT[1..N] (a contiguous stock sub-block) and the DL/RT* at OT[42..51].

### 3.3 `f[0]` / `f[1]` on `#`-signature records — partially decoded, NOT a reliable type

Across the corpus the `#`-signature records take these `f[0]`/`f[1]` pairs:

| pair `(f0, f1)` | seen on | reading |
|---|---|---|
| `(0x0000, 0x008a)` | `#alias>DELAY1#` outputs (econ base/rk/policy/mark2) | `0x8a == 138` — same value as a view-header marker; a "stripped" stub-style pair |
| `(0x3028, 0x0008)` | `#alias>SMOOTH#` levels, `#...>SMOOTH3>DL#`, `#...>SMOOTH3>LV1#` (var.), `#...>DELAY3>RT1#`, `#...>DELAY3>DL#`, `#...>DELAY3>LV1/2#` | low byte `0x28` = "flow / initial-stock" type; high bits `0x3000`; class `0x08` |
| `(0x3428, 0x0008)` | `#...>SMOOTH3#` (=LV3), `#...>SMOOTH3>LV2#`, `#...>SMOOTHI#`, `#...>DELAY3>DL#` (var.) | high bits `0x3400`; class `0x08` |
| `(0x3024, 0x0011)` | `#...>SMOOTH3>LV1#`, `#...>DELAY3>LV1/2#` | low byte `0x24` = "aux" type; class `0x11` (despite owning a stock-coded OT — the OT class code is authoritative) |
| `(0x302c, 0x0011)` | `#alias>DELAY1>LV1#`, `#...>DELAY3>RT2#`, `#LV1<SMOOTH3(...)#` (var.) | low byte `0x2c` = "const" type; class `0x11` |
| `(0x3424, 0x0011)` / `(0x3424, ...)` | `#...>DELAY3>LV3#` | high bits `0x3400`; class `0x11` |
| `(0x4020, 0x0089)` | `#v>SMOOTH#` in run_9; `#RT2<DELAY3(...)#` in SCEN01 | high bit `0x4000`; class-byte `0x89` |
| `(0x0020, 0x0087)` | `#v>SMOOTH#` in run_10 | plain type `0x20`; class-byte `0x87` |
| `(0x0020, 0x00ff)`, `(0x0020, 0x0017)`, `(0x0020, 0x1717)`, `(0x0020, 0x1717)` | many `#SMOOTH(...)#` / `#SMOOTH3(...)#` / `#DL<...#` records in **SCEN01** | plain type `0x20`, miscellaneous class bytes (`0xff`, `0x17`, `0x17_17`) — older Vensim / heavily-re-saved values |

Takeaways:
- `f[0]` of a `#`-signature record is `(opaque high bits) | (a normal `type_flags` low
  byte ∈ {`0x20`,`0x24`,`0x28`,`0x2c`})`. High bits `0x3000` / `0x3400` / `0x4000` appear
  only on `#`-signature / internal records; **they do NOT reliably encode whether the
  helper is a level/rate/aux** — the same macro's `LV1` is `0x3024` while its `LV2` is
  `0x3428` and its `DL` is `0x3028`. The only consistent macro-internal correlation is the
  *low byte* (`0x24` for first-level/aux-like internals, `0x28` for later levels & rates,
  `0x2c` for some) — useful as a tie-breaker, not as ground truth.
- `f[1]` on `#`-signature records is `0x08` (stock-associated) or `0x11` (dynamic) when the
  file is freshly compiled, but is whatever stale value survived on re-saved files
  (SCEN01: `0x00ff`, `0xff`, etc.). **Use the OT class code at `f[11]`, not `f[1]`.**
- The "ghost-range" `f[0]` values `{12324, 12328, 12332, 13352}` cited in `vdf.md` are
  exactly `{0x3024, 0x3028, 0x302c, 0x3428}` — i.e. `f[0]` values of `#`-signature helper
  records. They are **not RAM addresses or free-list markers**; they are the residual
  `type_flags` of `#`-signature records that were *cleared* on a later re-save (see §4.2).

### 3.4 `Ref.vdf` exception

`Ref.vdf` (C-LEARN, re-saved from an older build) has 62 `#`-signature names in the name
table (34 new-style internal helpers, 20 new-style outputs, 8 `#display name#` user names)
but **zero `#`-signature records** keyed via `f[2]`. The 116 `f[2]==0` records in `Ref.vdf`
are dimension-element records (huge `f[6]` = section-3 self-positional index words, `f[12]`
= group ids / pointers, `f[14]` = element index), not `#`-signature records. So on `Ref.vdf`
the `#`-signature helpers have no OT-owner records and cannot be recovered through the
record path; their OT slots are filled by the alphabetical/non-overlap reconstruction.
This is a known gap, not addressable from the `#`-signature region.

--------------------------------------------------------------------------------

## 4. Deterministic rule for the `.Supplementary` / ghost-record region

### 4.1 What `.Supplementary` looks like

On WRLD3 SCEN01 the final view block starts at record `r349` (`f[1]==138`-shaped view
header keyed to `.Supplementary`). It contains, in file order: a run of ordinary scalar
auxiliaries (`consumed industrial output`, ... `unit agricultural input`) and then an
*interleaved* run of `#`-signature helper records and "zeroed" records:

```
r358  unit agricultural input            f6=5  f11=290(c0x17)            (real scalar; f10=931)
r359  #LV2<SMOOTH3(lifeexpectancy,...)#  f6=5  f11=12 (c0x08)  f0=0x20  f1=0x17     (real helper stock)
r360  <f2=0>                              f6=0  f11=0          f0=0x3028 f1=0x08     ZEROED CRUFT
r361  #SMOOTH(currentagriculturalinputs,...)#  f6=5  f11=16 (c0x08)  f0=0x20 f1=0xff (real helper stock)
r362  #DL<SMOOTH3(LandYieldTechnology,...)#    f6=5  f11=46 (c0x11)  f0=0x3428 f1=0x08
r363  #LV1<SMOOTH3(fertilitycontrol...)#       f6=5  f11=2  (c0x08)  f0=0x3024 f1=0x11
r364  <f2=0>                              f6=0  f11=0          f0=0x3028 f1=0x08     ZEROED CRUFT
r365  #SMOOTH3(fertilitycontrol...)#      f6=5  f11=20 (c0x08)  f0=0x20  f1=0xff
r366..r369  <f2=0> ×4                     f6=0  f11=0          f0∈{0x3428,0x20}      ZEROED CRUFT
...
r418  #LV1<SMOOTH3(industrialoutput...)#  f6=5  f11=3  (c0x08)  f0=0x3028 f1=0x08    (real helper stock, last record)
```

SCEN01 has **21 zeroed records** (`f[2]==0 ∧ f[6]==0 ∧ f[10]==0 ∧ f[11]==0 ∧ f[12]==0 ∧
f[8]==0 ∧ f[9]==0`), all inside `.Supplementary` (record indices 360, 364, 366, 367, 368,
369, 372, 373, 378, 380, 382, 386, 390, 391, 393, 394, 398, 400, 402, 404, 407). Their
*only* nonzero fields are `f[0]` ∈ `{0x3024, 0x3028, 0x3428, 0x20}` and `f[1]` ∈
`{0x08, 0x11, 0xff}` — the residual `type_flags`/`classification` of stale `#`-signature
records (§3.3). `bact/euler.vdf` has 2 analogous zeroed records (r10, r11), mid-view, with
`f[0]==0x20`, `f[1]∈{0x8f, 0xff}` — same shape, no useful content. `Ref.vdf` also has many
`f[2]==0` records but they are dim-element records (nonzero `f[6]`/`f[11]`/`f[12]`).

### 4.2 The mechanism (hypothesis with evidence)

**Hypothesis H1: zeroed `.Supplementary` records are stale `#`-signature records left over
from earlier compilations of a re-saved file.** When Vensim re-saves a model after the set
of macro instances changed (or after a sketch edit triggered re-numbering), it rewrites the
`#`-signature region with the *current* macro helpers but does **not** physically remove the
slots of the *previous* `#`-signature records — it instead **clears** `f[2]` (name key),
`f[6]` (shape), `f[10]` (sort), `f[11]` (OT), `f[12]` (slot ref), `f[8]`/`f[9]` (sentinel)
to zero while leaving `f[0]`/`f[1]` at their old values. The interleave of real `#`-sig
records and zeroed records in SCEN01 is exactly the footprint of "old region partly
overwritten in place by the new region".

Evidence:
- the `f[0]` values on the zeroed records (`0x3024`/`0x3028`/`0x3428`/`0x20`) are precisely
  the `f[0]` values of *live* `#`-signature records in the same file (§3.3 / the SCEN01
  dump);
- the zeroed records appear *only* in the `.Supplementary` view block, interspersed with
  the live `#`-signature records, never in earlier view blocks;
- the run_9→run_10 transition (§2) is the minimal version of "re-save adds a stub record
  that names no variable" (`:SUPPLEMENTARY`); a multi-cycle re-save accumulates many such;
- the `bact/euler.vdf` zeroed records (which `vdf.md` flags as "must NOT be filtered" for
  the shift-by-one path) are also content-free `f[2]==0` records — they are cruft too; the
  warning is about the *shift-by-one pair walk* shifting when records are dropped, not about
  these records carrying any series.

(`vdf.md`'s `field[11]==0` shift-by-one sentinel "over-filters" `unit agricultural input`,
`#SMOOTH3(...)#` etc. on SCEN01 for a different reason: those names appear *after* a
`f[11]==0`-bearing predecessor in the file-order pair stream, so the shift-by-one rule never
assigns them an OT even though they have a perfectly good *direct* record. The fix for that
is to use the direct `f[2]`-key path, which §4.3 does, not to relax the `f[11]==0` sentinel.)

### 4.3 The rule

Use the **direct record key**, not the shift-by-one pair walk. For each record:

1. **Skip if `f[2]` does not decode to a real name-table entry.** A real name key is
   `≥ 7` and resolves through `(name_string_start - sec2_data_start)/4 + 7`. `f[2]==0`
   (and any value not landing on a parsed name string) ⇒ the record is a ghost / stub /
   cruft / dim-element record ⇒ **skip from the OT-owner path**. This single test removes
   all 21 SCEN01 zeroed records, both `bact/euler` zeroed records, the run_10
   `:SUPPLEMENTARY` stub, the `#desired stock#`/`#inline lookup table#` descriptor stubs
   (they actually have `f[11]==0`, see below), and the `Ref.vdf` `f[2]==0` element records.

2. **Skip if `f[11]` (owner interpretation) is not in `[1, ot_count)`.** `f[11]==0` is the
   "Time / no-owner" sentinel; out-of-range values are stale. This catches `rk4.vdf`
   `#desired stock#` and `lookup_ex.vdf` `#inline lookup table#` (descriptor-style records
   with `f[11]==0`), and SMOOTH/builtin function-token stubs whose `f[11]` is stale.

3. **Skip if the OT class code at `f[11]` is not a "real saved data" code.** Real codes:
   `0x08` (stock), `0x11` (dynamic data block / inline), `0x16`/`0x18` (Ref.vdf inline),
   `0x17` (constant). Time (`0x0f`) is OT[0] only (already excluded by step 2). This is the
   step that disambiguates the lookup-vs-helper overlap (§4.4): an internal-helper stock
   record's `f[11]` points at a `0x08` slot; a lookup-table descriptor record's `f[11]` is a
   *lookup-record index* (so following it to a `0x17`/`0x08` slot is a coincidence — see
   §4.4 for the real resolution).

4. **Resolve the shape from `f[6]`.** `f[6]==5` ⇒ scalar (1 OT slot); a nonzero section-3
   key (including `32` in single-shape files) ⇒ arrayed, span = section-3 `flat_size`;
   `f[6]==0` ⇒ stub ⇒ skip (already excluded for `#`-sig records, which all have `f[6]==5`).

A record that passes 1–4 contributes a series under its decoded name — **including
`#`-signature internal-helper names**. On the current corpus this is `118/120`
`#`-signature records (`rk4`/`lookup_ex` are the two with `f[11]==0`; both are user
display names / lookup descriptors, not stdlib helpers) and `350/419` records on SCEN01
(the other 69 are: 21 zeroed cruft, the `.Supplementary` view header, builtin-token /
dim-anchor / dim-element / module-IO stub records, and `f[11]==0` sentinel records).

### 4.4 Residual overlap: `#`-signature helper vs lookup-table descriptor

On SCEN01 the rule above leaves 54 OT slots claimed by *two* records each — e.g. OT[1] is
claimed by `rec[182]` "`capacity utilization fraction table`" (a lookup definition) **and**
`rec[389]` "`#LV1<DELAY3(...)#`" (an internal helper stock). All 54 such OT slots have
class code `0x08` (stock); a lookup-table definition's own value is class `0x17` (inline
constant), so OT[1]'s *data block* is the helper stock, not the lookup table. Deterministic
resolution:

- The lookup descriptor record's name is one of the `≤ ot_count` lookupish name-table
  entries that pair 1:1 with the section-6 lookup-mapping records; its `f[11]` is a
  *zero-based index into that lookup-record array*, not an OT start (this is the documented
  `f[11]` union — `vdf.md` §"Lookup mapping records"). Following lookup-record `f[11]`'s
  `word[10]` gives the lookup's *evaluated-output OT*, which is a different slot.
- The `#`-signature record's name matches the stdlib internal pattern (`#LVk<FUNC(...)#` /
  `#alias>FUNC>HELPER#`), `HELPER ∈ {LV1,LV2,LV3,ST,DL,RT1,RT2,linear,...}`; its `f[11]`
  is a *genuine* OT start (class `0x08`/`0x11`).

So for the **helper-vs-lookup** flavour of `record-span-overlap`, the discriminator is
deterministic: *the record whose name is a stdlib `#`-signature wins the OT slot; the
record whose name is a lookupish definition has a lookup-record index in `f[11]`.* (The
**general** owner/descriptor overlap — non-lookup variable vs non-lookup descriptor, e.g.
`RS N2O` over `C AF Sequestered` on `Ref.vdf` — is still unsolved per `vdf.md`'s appendix
and is *not* in scope here.)

### 4.5 What this rule does NOT solve

- `Ref.vdf` has `#`-signature *names* but no `#`-signature *records*; their OT slots remain
  un-record-mapped (§3.4).
- Whether a `#alias>FUNC#` *output* helper should be presented to the user under the alias
  name vs the `#...#` name is a presentation choice, not a decoding question. Decoding-wise,
  the `#...#`-named record owns its OT slot.
- The current `vdf_xray.py` `build_owner_record_blocks` *hides* the one-element
  `#alias>SMOOTH#` block when (i) it is length 1, all `0x08`-coded, (ii) `#alias>SMOOTH#`'s
  alias matches a visible block, and (iii) an adjacent longer all-`0x08` block exists with
  no direct sort keys (the user stock array). On run_9/run_10 this folds `#v>SMOOTH#`
  (OT[1], real data `first=0.0 last=16.0`) into the `stock[i]/stock[j]` block, so
  `--extract` returns 11 results instead of 12. By the rule above `#v>SMOOTH#` *is* an
  emittable internal-helper series (it has its own record `rec[22]`/`rec[23]`, its own data
  block, and a `0x08` OT slot); the current hiding is a conservative presentation choice the
  task framing argues against, but changing it is out of scope for this note (no edits to
  `vdf_xray.py`).

--------------------------------------------------------------------------------

## 5. Summary of pinnable facts

1. A `SMOOTH1` call adds `+1` OT (the level, class `0x08`, inserted into the contiguous
   stock block), `+2` records (function-token stub + `#alias>SMOOTH#` helper record), `+5`
   names (`SMOOTH` ×2 — call-site & macro-def — `IN`, `ST`, `#alias>SMOOTH#`), `+3` slots
   (the three slotted names). Generalises: `K` macro instances ⇒ `K` output signatures + the
   per-macro internal helper names/records, OT growth = Σ helper-slot counts (1 SMOOTH1/
   SMOOTHI, 4 SMOOTH3, 2 DELAY1, 7 DELAY3, 7 RAMP FROM TO, 2 SSHAPE, 1 SAMPLE UNTIL).
2. `#`-signature helper records are ordinary scalar records (`f[6]==5`) whose `f[2]` keys
   the `#...#` name and whose `f[11]` is a genuine OT block start with class `0x08` (level)
   or `0x11` (rate/aux). `f[0]`/`f[1]` carry opaque high bits / re-save-volatile class bytes
   and are NOT a reliable type signal — use the OT class code.
3. The "ghost-range" `f[0]` values `{12324, 12328, 12332, 13352}` = `{0x3024, 0x3028,
   0x302c, 0x3428}` are the residual `type_flags` of `#`-signature records that were cleared
   in place on a re-save. Such "zeroed" records (`f[2]==0 ∧ f[6]==0 ∧ f[11]==0`) are cruft;
   on SCEN01 there are 21 of them, all in `.Supplementary`, interleaved with the live
   `#`-signature records. `bact/euler.vdf`'s 2 zeroed records are the same kind of cruft.
4. A cosmetic re-save (run_9→run_10) can rename a view (`.9 smooth time` → corrupted
   `.mdl`) AND materialize a metadata-tag stub (`:SUPPLEMENTARY`), netting `+1` record /
   name / slot with `0` OT change — the minimal example of re-save cruft accumulation.
5. Deterministic OT-owner extraction rule (works for `#`-signature helpers and skips all
   cruft): emit a record iff `f[2]` decodes to a real name **and** `f[11] ∈ [1, ot_count)`
   **and** the OT class code at `f[11]` ∈ `{0x08, 0x11, 0x16, 0x17, 0x18}` **and** `f[6]`
   resolves to a shape (`5` or a section-3 key). For the helper-vs-lookup overlap, the
   `#`-signature-named record wins the OT slot; the lookupish-named record's `f[11]` is a
   lookup-record index (its true output OT is `lookup_record[f[11]].word[10]`).
