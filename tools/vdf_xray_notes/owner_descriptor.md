# The owner/descriptor discriminator for record `field[11]`

Task: decode (or rigorously characterize as undecodable) the rule by which a VDF
reader decides whether a section-1 record's `field[11]` is an OT-block start
("owner" record) or a zero-based index into the section-6 lookup-record array
("descriptor" record, i.e. the record for a graphical-function / lookup-table
definition).

Scope of evidence: lookup_ex.vdf (2 lookup records, 1 overlap pair), econ/base
and econ/mark2 and econ/policy (4 lookup records, 3 overlap pairs each),
WRLD3-03/SCEN01.VDF and WRLD3-03/experiment.vdf (55 lookup records, 54 overlap
pairs), Ref.vdf / C-LEARN (165 lookup records, 58 overlapping OT slots / 23
distinct (descriptor,owner) record pairs). MDL ground truth used to label
descriptor records: a section-1 record is a descriptor iff its variable is an MDL
standalone lookup-table definition (`name(...data...)` form) or it is the `#X#`
internal signature record of a `= WITH LOOKUP(...)` variable. Control: the
`model_editing/run_*.vdf` fixtures (0 lookup records, 0 record-span overlaps).

## TL;DR conclusion

**The discriminator is not encoded in any single byte/field of the section-1
record (or any struct directly pointed to by the record).** Across all fixtures,
`field[0]`, `field[1]`, and `field[14]` (the three "type/classification/has-
lookup" words) take *exactly the same* values on descriptor records as on owner
records — there is no value, no bit, and no `(field[0], field[1])` combination
that is unique to descriptors. The section-6 lookup record carries no
back-pointer (its 13 words are fully accounted for: see vdf.md). `field[3]`,
`field[4]`, `field[5]`, `field[7]` do not decode to anything that disambiguates.

**The reframe candidate is correct.** `field[11]` is a genuine union whose tag is
*not stored on disk*; the reader is expected to already know which variables are
graphical-function definitions (it has the compiled model). For those records it
ignores `field[11]` as an OT start — the lookup's data comes entirely from the
section-6 lookup record (x/y arrays via `word[5..6]`, output OT via `word[10]`,
deps via `word[12]`) plus the section-7 packed lookup-point arrays. **When the
descriptor records are excluded, the remaining owner-record spans form a perfectly
clean OT partition with zero overlaps on every fixture with lookups** (verified:
lookup_ex, econ/mark2, WRLD3 SCEN01, WRLD3 experiment, Ref). So xray's
`record-span-overlap` blocker is honest: the overlap is real, not decodable from
the file.

**There is one clean, direct, O(1) FORWARD link** that the reader uses *once it
knows a record is a descriptor*: `field[11]` of a descriptor record == the
zero-based index into the section-6 lookup-record array. Verified by: on every
fixture the descriptor records' `field[11]` values are exactly a permutation of
`[0 .. num_lookup_records - 1]`; on econ/mark2 and lookup_ex they are in
name-table order; on WRLD3 the lookup-record array is in *alphabetical* order of
lookup-def names, so the descriptor `field[11]` values are alphabetically sorted
but not name-table-position-sorted. The reverse direction does not exist.

**Best-available deterministic-ish reconstruction** (for a VDF-only path that has
no model): among the records that share an `field[11]` value `k` with `0 <= k <
num_lookup_records`, treat the one with the *highest* `field[10]` (sort_key) as
the lookup descriptor and the rest as owners. Perfect on lookup_ex (1/1),
econ/base, econ/mark2, econ/policy (3/3 each), Ref.vdf (35/35 — and 29/35 don't
even collide so the rank is trivial there). Fails on 13/55 WRLD3 SCEN01 pairs —
all 13 are cases where the colliding owner is a *real model stock* (e.g. `Land
Fertility`, `Population 0 To 14`, `Persistent Pollution Technology`) whose
view-local `field[10]` happens to exceed the descriptor's. This is a heuristic,
not a format rule (`field[10]` is view-local, so a global comparison is not
well-founded). It is strictly better than the current "non-overlap interval
selection" diagnostic and is worth wiring in *only if labelled as a heuristic*.

---

## 1. Field-by-field comparison: descriptor vs. owner records

### 1.1 lookup_ex.vdf — the single overlap pair (OT[1])

`lookup table 1` is an MDL standalone lookup table (`lookup table 1([(0,0)-
(100,20)],(0,4),(10,5),...)`); it is the descriptor. `stock = INTEG(net change,
0)` owns OT[1] (class code 0x08). Both records carry `field[11] == 1`.

| field | rec[7] `lookup table 1` (DESCRIPTOR) | rec[9] `stock` (OWNER) | model_editing owner `stock` (run_5 rec[16]) |
|------:|--------------------------------------|-------------------------|---------------------------------------------|
| f[0]  | 32 (0x20) | 37 (0x25) | 32 (0x20) |
| f[1]  | 135 (0x87) | 5905 (0x1711) | 5911 (0x1717) |
| f[2]  | 32 (name key → `lookup table 1`) | 43 (→ `stock`) | 54 (→ `stock`) |
| f[3]  | 143 | 85 | 127 |
| f[4]  | 0 | 0 | 0 |
| f[5]  | 0 | 0 | 0 |
| f[6]  | 5 (scalar) | 5 (scalar) | 5 (scalar) |
| f[7]  | 0 | 0 | 0 |
| f[8]  | SENT (0xf6800000) | SENT | SENT |
| f[9]  | SENT | SENT | SENT |
| f[10] | 11 | 0 | 0 |
| f[11] | **1** (= lookup record 1; `lookup[1].word[10] == 5` = OT of `net change`) | **1** (= OT[1], class 0x08) | 1 (= OT[1]) |
| f[12] | 124 | 124 | 124 |
| f[13] | 0 | 0 | 0 |
| f[14] | SENT | 0 | 0 |
| f[15] | 0 | 0 | 0 |

Differences: f[0] (32 vs 37), f[1] (135 vs 5905), f[3] (143 vs 85), f[10] (11 vs
0), f[14] (SENT vs 0). None of these is the discriminator:

- `f[1] == 135` (0x87) is *not* a lookup-def marker — in econ/base 135 appears on
  ordinary constants (`average time to build a house`, `base housing supply`,
  `inventory ratio`) and on unit annotations (`-Month`); in Ref.vdf it is the
  most common owner classification (18 owner records) and only 3 descriptor
  records carry it.
- `f[14] == SENT` is the documented "has-lookup-table" marker; in econ/base 75
  records carry it (all user-facing variables with UI metadata), in Ref.vdf 707
  do, including ordinary auxes. The 2nd lookup descriptor in lookup_ex
  (`#inline lookup table#`, rec[11]) has `f[14] == 0`, and a `WITH LOOKUP`
  variable owner (`inline lookup table`, rec[8]) has `f[14] == SENT`. So f[14]
  flips both ways relative to the owner/descriptor role.
- f[3] decodes to nothing useful — tried "byte offset into section-1 data",
  "word offset into section-2/section-3", "record index": no consistent meaning.
  f[3] for descriptors is in the same numeric ranges as for owners.

The other lookup-related records in lookup_ex for cross-reference:

| rec | name | kind | f[0] | f[1] | f[6] | f[11] | f[14] | interpretation of f[11] |
|----:|------|------|-----:|-----:|-----:|------:|-------|--------------------------|
| 7 | `lookup table 1` | standalone def (DESCRIPTOR) | 32 | 135 (0x87) | 5 | 1 | SENT | lookup record 1 |
| 8 | `inline lookup table` | `= WITH LOOKUP(...)` variable (OWNER) | 32 | 5914 (0x171a) | 5 | 4 | SENT | OT[4] (its own series) |
| 9 | `stock` | `INTEG` stock (OWNER) | 37 | 5905 (0x1711) | 5 | 1 | 0 | OT[1] |
| 10 | `net change` | flow (OWNER) | 40 | 2056 (0x808) | 5 | 5 | 0 | OT[5] |
| 11 | `#inline lookup table#` | the inline lookup's signature (DESCRIPTOR) | 36 | 5905 (0x1711) | 5 | 0 | 0 | lookup record 0; `lookup[0].word[10] == 4` |

Note: `f[1]` low byte 0x1a (= 5914 = 0x171a) is the "lookup-backed variable" code
the doc table mentions — but it is on the *owner* record `inline lookup table`,
not on a descriptor. A standalone lookup def (`lookup table 1`) does not get a
0x1a-low-byte classification; it gets 0x87. So 0x1a is not the discriminator
either; it just marks `WITH LOOKUP` *expression* variables.

### 1.2 econ/base.vdf — 3 overlap pairs

`lk_output_OTs = [34, 45, 49, 62]`; lookup records 0..3 correspond to
`federal funds rate lookup` (name idx 51, f[11]=0), `hud policy lookup` (idx 63,
f[11]=1), `inflation rate lookup` (idx 68, f[11]=2),
`loan standards impact on insolvency table` (idx 78, f[11]=3) — i.e. lookup-def
records in **name-table order**, with `field[11]` == their rank.

| OT | DESCRIPTOR record | f[0] | f[1] | f[6] | f[10] | f[14] | OWNER record (colliding) | f[0] | f[1] | f[6] | f[10] | f[14] |
|---:|--------------------|-----:|-----:|-----:|------:|-------|---------------------------|-----:|-----:|-----:|------:|-------|
| 1 | `hud policy lookup` (rec 62, f11=1) | 36 | 2065 (0x811) | 5 | 95 | SENT | `#LV1<DELAY1(insolvencyrisk,...)#` (rec 87, f11=1) | 13356 | 17 (0x11) | 5 | 8 | 0 |
| 2 | `inflation rate lookup` (rec 67, f11=2) | 44 | 17 (0x11) | 5 | 107 | SENT | `#SMOOTH(indexedHPI,...)#` (rec 89, f11=2) | 13352 | 8 (0x08) | 5 | 10 | 0 |
| 3 | `loan standards impact on insolvency table` (rec 77, f11=3) | 32 | 143 (0x8f) | 5 | 143 | SENT | `#SMOOTH(insolvencyrisk,6)#` (rec 90, f11=3) | 13352 | 8 (0x08) | 5 | 12 | 0 |

The 4th lookup descriptor `federal funds rate lookup` (rec 50, f11=0) does not
collide (OT[0] is Time) — f[11]=0 alone tells the reader it is not an owner OT.

Observations:
- The owners here are internal SMOOTH/DELAY stdlib helpers (`#LV1<...>#`,
  `#SMOOTH(...)#`) — their large `f[0]` values (0x33xx range) are an artifact of
  the helper region, NOT a general "owner" signal. On Ref.vdf the owners are
  ordinary stocks with `f[0]` ∈ {32, 34, 40, 44} — the same set descriptors use.
- The descriptors' `f[1]` here is {2065 (0x811), 17 (0x11), 143 (0x8f)} — three
  different values, two of which (0x11 = "const-/aux"-ish, 0x8f) are heavily used
  by ordinary owners. So no f[1]-based rule.
- `f[10]` (sort_key): descriptor 95/107/143 vs owner 8/10/12. Descriptor strictly
  higher in all 3 — see §3 for why this is suggestive but view-local.

### 1.3 Ref.vdf / C-LEARN — representative overlap pairs

165 lookup records; MDL parser found 50 standalone lookup defs (35 have a matched
section-1 record in this run; the remainder are array-element lookups whose own
record is the dimension-arrayed name and whose 7 elements map to consecutive
lookup records). 23 distinct (descriptor, real-owner) record pairs. Sample:

| conflict OT (class) | DESCRIPTOR record (f[11], f[0], f[1], f[6], f[10]) | OWNER record (f[11], f[0], f[1], f[6], f[10]) |
|---|---|---|
| 113 (0x08 stock) | `RS N2O` (113, 44, 17, 86 [7-elem], 2684) | `C AF Sequestered` (113, 32, 23, 59 [3-elem], 378) |
| 116 (0x08) | `RS N2O` (113, ...) | `C in Atmosphere` (116, 40, 22, 59, 383) |
| 120 (0x08) | `RS PFC` (120, 44, 17, 86, 2694) | `C in Biomass` (119, 44, 17, 59, 389) |
| 127 (0x08) | `RS SF6` (127, 44, 17, 86, 2703) | `C in Deep Ocean` (122, 40, 22, 194, 393) |
| 134 (0x08) | `Solar and albedo forcings` (134, 32, 23, 5, 2951; f[14]=`0x3c23d70a` not SENT!) | `C in Humus` (134, 40, 22, 59, 397) |
| 139 (0x08) | `Specified Global CH4` (139, 32, 26, 5, 2997) | `C in Mixed Layer` (137, 44, 17, 59, 400) |
| 141 (0x08) | `Specified Global N2O` (141, 32, 23, 5, 3001) | `CH4 in Atm` (140, 44, 17, 59, 444) |
| 143 (0x11 dynamic) | `Specified Global SF6` (143, 32, 135, 5, 3007) | `CO2 conc change at impact year` (143, 44, 17, 5, 0) |
| 151 (0x11) | `UN population LOW LOOKUP` (151, 34, 131, 86, 3454) | `Cum CO2 at start` (146, 32, 23, 86, 911) |
| 158 (0x11) | `UN population MED LOOKUP` (158, 32, 26, 86, 3459) | `Cum CO2eq at start` (153, 32, 132, 86, 913) |
| 106 (0x11) | `Annual rate of emissions to target` (106, 40, 22, 86, 317) | `RS HFC4310mee` (106, 34, 131, 86, 2677) [this owner is itself an array of constant data; note it has *higher* f[10] than this particular descriptor — a counterexample to the f[10] rule when the colliding peer is also a data-array variable] |

`Solar and albedo forcings` is the documented oddball: `f[14] == 0x3c23d70a`
(≈ +0.01 as f32) instead of SENT, and `f[8]/f[9] == 0xbf800000/0x3f800000`
(≈ -1.0/+1.0, the lookup's `[(2010,-1)-(2100,0.08)]` y-bounds leaking into the
sentinel slots). So even the `f[8]==f[9]==SENT` sentinel pair is not universal on
descriptors. Still not a discriminator (its co-owner `C in Humus` has the normal
SENT pattern, but plenty of *owners* elsewhere have non-SENT values too).

Aggregate value sets on Ref.vdf:

```
DESCRIPTOR records:  f[0] ∈ {32, 34, 40, 44, 556}        f[1] ∈ {17, 22, 23, 26, 131, 135, 144}
OWNER-shaped records: f[0] ∈ {32, 34, 40, 44, 45, 556, 0, 16416, 552, ...}
                      f[1] ∈ {15, 17, 22, 23, 24, 26, 131, 132, 135, 137, 138, 143, 8, ...}
```

`DESCRIPTOR ⊂ OWNER` for both f[0] and f[1]. No bit of f[0] or f[1] is set on all
descriptors and clear on all owners (or vice versa). No `(f[0], f[1])` pair is
unique to descriptors.

### 1.4 WRLD3-03/SCEN01.VDF — 54 overlap pairs

This is the worst case for any positional/sort heuristic: there are 55 lookup
records (indices 0..54) and OT[0..54] are exactly the 55 stock-coded OT entries
(41 model stocks + ~14 SMOOTH3/DELAY3 internal levels). So the descriptor for
lookup record `k` has `field[11] == k`, AND the owner of OT[k] also has
`field[11] == k`. The collision is numerically exact; the two interpretation
ranges fully overlap and nothing in the record distinguishes them.

Sorted by `field[11]`, the 55 descriptors are alphabetical:
`assimilation half life mult table` (0), `capacity utilization fraction table`
(1), `completed multiplier from perceived lifetime table` (2),
`crowding multiplier from industry table` (3), `development cost per hectare
table` (4), `Education Index LOOKUP` (5), ... — confirming the lookup-record array
is in case-insensitive alphabetical order of the lookup-def names on WRLD3 (vs.
name-table order on econ/lookup_ex; on those fixtures the name table happens to be
near-alphabetical anyway).

Aggregate value sets on WRLD3 SCEN01:

```
DESCRIPTOR records:   f[0] ∈ {0, 32, 36, 44}             f[1] ∈ {17, 23, 26, 138, 4625}
OWNER-shaped records: f[0] ∈ {0, 32, 36, 40, 44, 548, 12324, 12328, 16416, ...}
                      f[1] ∈ {17, 23, 26, 8, 135, 137, 143, 138, 255, 2056, 2065, 4625, ...}
```

Again `DESCRIPTOR ⊂ OWNER`, no discriminating bit.

---

## 2. The reframe candidate, tested rigorously

Claim: the reader simply *ignores* `field[11]` on descriptor records; descriptors
are identified externally (compiled model). Test: if the MDL-identified descriptor
records are removed, do the remaining owner-record spans (under the owner
interpretation of `field[11]`) form a clean partition with zero overlapping OT
slots?

| Fixture | record spans | descriptor records (MDL) | ALL overlapping OT slots | OWNER-only overlapping OT slots after removing descriptors |
|---|---:|---:|---:|---:|
| `lookup_ex.vdf` | 8 | 1 | 1 | **0** |
| `econ/mark2.vdf` | 86 | 4 | 3 | **0** |
| `WRLD3-03/SCEN01.VDF` | 350 | 55 | 54 | **0** |
| `WRLD3-03/experiment.vdf` | 350 | 55 | 54 | **0** |
| `Ref.vdf` | 849 | 35 | 58 | **0** |
| `model_editing/run_1..10` (controls) | 4–9 | 0 | 0 | 0 (trivially) |

Result: **clean owner partition in every case.** This is exactly what the reframe
predicts. (On Ref.vdf, 97 of the OT slots covered by descriptor records' *bogus*
owner spans are not covered by any owner span at all — that is just because
several Ref descriptors have *array* shapes, e.g. `RS N2O` has a 7-element shape,
so `[f[11], f[11]+7)` extends past the real owners' 3-element spans into
unrelated slots. It confirms the bogus span is meaningless, not a problem for the
reframe.)

So: the owner/descriptor union does NOT need an on-disk discriminator for the
owner partition to be unambiguous. The descriptor records just have to be set
aside. The remaining open question — "how does Vensim identify descriptor records
when opening an old VDF without the .mdl?" — has no answer in the file:

- The lookup-def *names* in the name table look like ordinary variable names
  (`RS N2O`, `Solar and albedo forcings`, `assimilation half life mult table`) —
  no marker.
- The section-6 lookup record carries no back-pointer (vdf.md, exhaustively
  verified — all 13 words decoded: graph-axis floats `word[0..4]`, section-7
  x/y offsets `word[5..6]`, xy-pair-count family `word[7..8]`, runtime pointer
  `word[9]`, output OT `word[10]`, output width `word[11]`, optional dep-chain
  root `word[12]`).
- `lookup[k].word[10]` (output OT) is the OT of a *consumer* of the lookup, not of
  the lookup-def name's record — and it can be shared by several lookup records
  (Ref.vdf: 165 lookup records, only 27 distinct `word[10]` values).
- Width agreement (`flat_size(record.shape) == lookup[record.f[11]].word[11]`)
  matches the descriptor in 14/23 Ref pairs and ALSO matches the colliding owner
  in 8/23 (because in Ref the owner is also a small-index record), and matches
  *both* sides in 54/54 WRLD3 pairs — so it cannot separate them.

Therefore the only honest VDF-only statement is: **`field[11]` is a union with no
stored tag; a model-free reader cannot deterministically decide it, and xray's
`record-span-overlap` blocker is correct.**

---

## 3. Best-available deterministic-ish reconstruction (a heuristic, clearly labelled)

Among the section-1 records that share an `field[11]` value `k` with `0 <= k <
num_lookup_records`, classify the one with the **highest `field[10]` (sort_key)**
as the lookup descriptor (its `field[11]` is then the lookup-record index) and the
rest as owners.

Results (per (descriptor,owner) overlap pair, "owner has strictly lower f[10]"):

| Fixture | pairs | rule correct | failures | failure cause |
|---|---:|---:|---:|---|
| `lookup_ex.vdf` | 1 | 1 | 0 | — |
| `econ/base.vdf` | 3 | 3 | 0 | — |
| `econ/mark2.vdf` | 3 | 3 | 0 | — |
| `econ/policy.vdf` | 3 | 3 | 0 | — |
| `Ref.vdf` | 23 | 23 | 0 | — (and 29 of the 35 descriptors don't collide at all) |
| `WRLD3-03/SCEN01.VDF` | 54 | 41 | 13 | colliding owner is a real model stock (`Land Fertility`, `Land Yield Technology`, `Nonrenewable Resources`, `Persistent Pollution`, `Persistent Pollution Technology`, `Population 0/15/45/65 ...`, `Potentially Arable Land`, `Resource Conservation Technology`, `Service Capital`, `Urban and Industrial Land`) whose *view-local* f[10] exceeds the descriptor's |

Why it works as far as it does: lookup-def records sit in normal sketch views and
have moderate-to-high view-local positions; the most common colliding owners
(SMOOTH/DELAY internal `#...#` helpers in the `.Supplementary` view) all have very
low view-local f[10]. It breaks precisely when the colliding owner is a real model
stock in a busy view. Because f[10] restarts per view (vdf.md: "view-local
alphabetical ordering key"), a global comparison is not principled — this is a
heuristic, not a decoded rule, and should be flagged as such if used.

Alternatives ranked (per the WRLD3 SCEN01 stress test, "owner has the smaller
sort key under that order" across the 54 pairs): `f[10] ascending` 41/54;
`f[0] ascending` 19/54 (11 ties); `f[1] ascending` 13/54 (14 ties); file order
8/54; `f[2]` (name key) order 8/54; `f[12]` (view anchor) order 8/54;
`f[11]` order 0/54 (54 ties, by construction). So `f[10]` is by far the strongest
single ordering, and on the four "well-behaved" fixtures it is exact.

Also useful and decoded (not a heuristic): **`field[11]` of a descriptor record
is exactly the index into the section-6 lookup-record array** (verified: on every
fixture the descriptor records' f[11] values are a permutation of
`[0, num_lookup_records)`). So once a record is *known* to be a descriptor, the
binding to its lookup record / x-y arrays / output OT is direct and O(1) — there
is no search there; only the descriptor-vs-owner classification of `field[11]` is
the missing bit.

---

## 4. Things ruled out in this round (in addition to vdf.md's appendix list)

- `(field[0], field[1])` *pair* as a discriminator: refuted. The descriptor set's
  `(f0,f1)` pairs are a subset of the owner set's on every fixture; no pair is
  descriptor-exclusive (lookup_ex, econ/base, WRLD3 SCEN01, Ref.vdf).
- `field[3]` as any kind of pointer/index that disambiguates: refuted. Tried
  byte-offset-into-section-1, word-offset-into-section-2/-3, record-index
  interpretations; descriptor f[3] values are in the same numeric ranges as owner
  f[3] and resolve to nothing meaningful.
- `field[4]`, `field[5]`, `field[7]` as discriminators: refuted. f[5] and f[7] are
  nonzero on most user-facing variables (owners *and* descriptors), zero on the
  `#...#` helper region; not discriminating.
- `flat_size(record.shape) == lookup[record.f[11]].word[11]` ("width match") as a
  discriminator: refuted — matches both sides of 54/54 WRLD3 pairs and 8/23 Ref
  pairs (the colliding owner often also matches its own f[11]'s lookup width
  because in Ref/WRLD3 the owner's f[11] is itself small).
- "Process records in order X, first to claim an OT wins, later ones are
  descriptors" for X ∈ {name-table order, file order, f[10]→f[2], f[12]→f[10]→
  f[2], f[11]→file}: refuted as an exact rule. Name-table/file order even gets
  lookup_ex *backwards* (`lookup table 1` precedes `stock`, so the descriptor
  would claim OT[1] first). `f[10]→f[2]` is the best (perfect on lookup_ex except
  one false positive `#inline lookup table#`, perfect on econ/mark2, 42/55 on
  WRLD3) but is the same view-local-f[10] heuristic as §3, not a format rule.
- `field[14]` (re-confirmed): SENT on both sides of pairs, and on Ref's
  `Solar and albedo forcings` descriptor it is `0x3c23d70a` (a float) — flips
  both ways; "has-lookup-table" marker, not owner/descriptor.

## 5. Recommendation for vdf_xray.py / vdf.rs (not done here — notes only)

1. Treat the `record-span-overlap` blocker as a *correct* statement of an
   undecodable union, not a bug to be closed by more analysis. Document in vdf.md
   that the discriminator is not stored on disk and the reframe is the resolution.
2. Promote to a pinned fact: "a descriptor record's `field[11]` is the zero-based
   index into the section-6 lookup-record array; the lookup-record array order is
   the alphabetical (case-insensitive) order of lookup-definition names" — this is
   the decoded forward link and explains the existing `zip(lookup_names,
   lookup_records)` reconstruction (currently a heuristic).
3. If the model-free path needs *some* answer when overlaps exist, replace the
   "non-overlap interval DP" with the §3 rule ("among records sharing `field[11]
   == k < num_lookups`, highest `field[10]` is the descriptor"), but flag it the
   same way `used_lookup_name_order_pairing` is flagged — it is a heuristic that
   provably fails on ~24% of WRLD3 SCEN01 pairs. With the model in hand
   (`build_section6_guided_ot_map`), use the model's lookup-def set directly and
   skip those records' `field[11]`; that path is exact.
