# VDF reverse-engineering — synthesis plan (Task #6)

Consolidates: `model_editing_diff.md` (keystone), `heuristics_audit.md`,
`corpus_and_versions.md` (corpus-agent), `helper_signature_region.md` (helper-agent),
`owner_descriptor.md` (disc-agent — pending). Plan is ordered by confidence/independence.

## A. High-confidence, independent — do these regardless of disc-agent

### A1. Make `decoded_record_spans` (+ a class-code check) the primary extraction path

`decoded_record_spans ∪ {Time@OT[0]}` is the complete, deterministic, non-overlapping
results map for **all 31 `exact-by-xray` fixtures** (measured). The reconstruction stack
fires only on the 10 with overlapping descriptor spans (and Ref's missing records).

Add to `decoded_record_spans` the helper-agent's class-code guard (step 3 of its rule):
a span is only emitted if `class_codes[f[11]] ∈ {0x08, 0x11, 0x16, 0x17, 0x18}` (i.e. a
real saved-data slot — never `0x0f`/Time, never out of the code set). This is what
distinguishes a real owner from a descriptor whose `f[11]`-as-lookup-index numerically
lands on a real slot only by coincidence. (On the 31 clean fixtures this changes nothing;
it's a precondition that becomes load-bearing on the overlap fixtures.)

`extract_named_results` becomes:
1. `spans = decoded_record_spans(vdf)` (with class-code guard).
2. If `{ot for span in spans for ot in [span.start, span.end)} == set(range(1, OT_count))`
   and no overlaps → emit each span (array labels from the axis-ref→anchor→`f[8]`-catalog
   chain) + `Time@OT[0]`. **Done — zero reconstruction.** This is 31/41 fixtures.
3. Else → current reconstruction machinery, but **only for the residue** (uncovered or
   overlapping OT slots), and set a diagnostic flag for it. The `precision_report` then
   marks `exact-by-xray` iff step 2 fired (or step 3 fired but every residue slot was
   resolved by a *decoded* sub-rule, see B).

### A2. Stop hiding `#alias>FUNC#` / `#FUNC(args)#` internal-helper series

The current `build_owner_record_blocks` `hidden`-rule (a) is derived from the
display-only `preferred_slot_name_alignment` heuristic, so the result count varies with
slot-layout noise (`econ/base` hides 0, identical-model `econ/rk` hides 2 — corpus-agent
B.2), and (b) makes xray disagree with Rust `to_results_via_records` (which keeps these
series). By the decoded record rule, `#v>SMOOTH#` etc. own real OT slots with real data
blocks → they are emittable series. **Delete the `hidden` machinery; emit `#...#`
helpers** under their signature names. Consumers wanting a "user-facing" symbol table can
strip `#`-prefixed names themselves (the doc already says callers may do that). Update
`vdf.md` signal #9/#10 (which currently describe the hiding as deliberate) and the
`run_9`/`run_10` test expectations (results 11 → 12).

### A3. `vdf.md` doc updates (pinned facts)

- **No version marker exists.** Confirm in the doc: every "constant" word is constant
  across 2005–2026; the magic byte (`0x52` run / `0x41` dataset / `0x53` sensitivity) is
  the only structural fork; the 2008-vs-2019+ difference in `block0[0..11]` content is
  written-without-a-marker (a reader just tolerates whatever is there). Replace the
  hedged "0x6c is not a reliable version field" language with a flat "there is no version
  field".
- **Section-header `field3` is a per-section kind code**: 135 for the name-table and
  array-directory sections, 100 for the OT-metadata section, 500 for everything else;
  unchanged across eras and across the `0x52`/`0x53`/`0x41` containers (datasets keep the
  codes but shift positions). Add as a one-line confirmation in "Section roles", noting
  the decoder doesn't depend on it (position-based identification is authoritative).
- **SMOOTH/macro-helper structure**: a `SMOOTH1`/`SMOOTHI` call adds +1 OT (level, class
  0x08, into the contiguous stock block), +2 records (one function-token stub + one
  `#alias>FUNC#` helper record), +5 names (`FUNC` ×2, the two macro params, `#alias>FUNC#`),
  +3 slots. Per-macro helper counts: SMOOTH3 → 4 (LV3=output, LV2, LV1, DL), DELAY1 → 2,
  DELAY3 → 7, RAMP FROM TO → 7, SSHAPE → 2, SAMPLE UNTIL → 1. `#`-signature helper records
  are ordinary scalar records (`f[6]==5`) keyed by `f[2]` to the `#...#` name with `f[11]`
  = a genuine OT start (class 0x08 level / 0x11 rate-aux). `f[0]`/`f[1]` carry opaque high
  bits + re-save-volatile class bytes — use the OT class code.
- **"Zeroed" / ghost records are re-save cruft**: `f[2]==0 ∧ f[6]==0 ∧ f[11]==0` records
  (21 on SCEN01, all in `.Supplementary`, interleaved with live `#`-sig records; 2 on
  bact/euler) are stale `#`-signature records cleared in place on a later re-save —
  `f[0]`/`f[1]` retain old values (the "ghost-range" `f[0]` values `{0x3024,0x3028,0x302c,
  0x3428}` are exactly the `type_flags` of live `#`-sig records). The deterministic skip:
  `f[2]` doesn't decode to a parsed name → not an OT owner. (This subsumes the doc's
  current per-fixture "drop zeroed records that sit in an over-full view block" workaround;
  promote it to "any record whose `f[2]` doesn't resolve is not an owner".)
- **`field[11]==0` shift-by-one over-filter** (the SCEN01 lost-real-variables note):
  reframe — the loss is in the *shift-by-one pair walk*, not in the data; the *direct
  `f[2]`-key path* recovers `unit agricultural input`, `#SMOOTH3(...)#`, etc. correctly.
  Recommend the direct path as primary; keep shift-by-one only as a cross-check.

### A4. Code hygiene
- Remove the stale `/tmp/vdf_audit_phase1.md` references in `vdf_xray.py` comments
  (audit letters B.2.1/B.3.1); inline the reasoning or point at `tools/vdf_xray_notes/`.
- File a tracked issue for: (deferred-if-fixed) the `build_owner_record_blocks` hide-rule
  fragility; (definitely) the `vdf_xray.py` dataset-parsing stub vs the implemented Rust
  `VdfDatasetFile`, including that the Rust dataset path is unverified on arrayed datasets.

## B. Resolves the lookupish-vs-helper overlap (8 of 9 `not-proven`) — promote a fact

The doc already states (for section 7): "Tables appear in the same order as their lookup
definitions in the name table." Promote the dual: **section-6 lookup-mapping records
correspond 1:1, in order, to the lookup-definition name-table entries** (= the `≤ OT_count`
lookupish names; count == `sec1.data[8..12]`). Then for every `record-span-overlap` that
is the helper-vs-lookup flavour (all of them except `Ref.vdf`'s `RS *` cases):
- the descriptor record's name is the k-th lookup-definition name → its `f[11]` is the
  lookup-record index k (not an OT start) → drop it from the owner set; its evaluated
  output OT is `lookup_record[k].word[10]` (already a separate owner record, or a
  to-be-emitted standalone-lookup series at that OT).
- the other record in the overlap (a `#`-signature helper or a regular variable) keeps
  the OT slot.
This replaces both `_select_non_overlapping_owner_blocks` *and* `_heuristic_name_looks_lookupish`
on those 8 fixtures with a decoded rule. The precision report can then drop
`record-span-overlap` for the 8 non-`Ref` fixtures (status → `exact-by-xray`).

**Caveat**: this still doesn't identify a *non-lookupish-named* descriptor (`RS N2O`).
That's `Ref.vdf`'s hard case → see C.

## C. Blocked on disc-agent — `Ref.vdf`'s general owner/descriptor discriminator

`Ref.vdf` has the `RS N2O` (`f[11]=113`, claims 7-elem OT[113..120)) over `C AF
Sequestered`/`C in Atmosphere`/`C in Biomass` flavour: the descriptor's name is *not*
lookupish, so B's rule doesn't catch it. Also: `Ref.vdf` has 429 uncovered OT slots (455
of which the doc says are "no-saved-data" 0x11/raw-0/-1.3e33 entries), 132 records with
unresolvable `f[2]` (possibly an incomplete name-table parse on this older re-saved
build), 205 records with out-of-range `f[11]`, and 62 `#`-signature names with zero
`#`-signature records. Wait for disc-agent's verdict:
- if a deterministic discriminator is found → wire it into step 3 of A1.
- if confirmed ill-posed → keep an honest `record-span-overlap` blocker on `Ref.vdf` only;
  the other 8 are resolved by B; the 31 clean ones by A1.

## D. After A/B: run the test suite, then port to Rust
- `python3 tools/test_vdf_xray.py` (it's a `unittest` module, no pytest), `cargo test -p
  simlin-engine vdf`, `python3 tools/vdf_xray.py --corpus-precision .` (expect the
  `not-proven` count to drop from 9 to 1, i.e. just `Ref.vdf`, after B).
- Port the decoded rules to `src/simlin-engine/src/vdf.rs`: `to_results_via_records`
  should (1) add the class-code guard, (2) keep `#...#` helper series (already does),
  (3) replace `select_non_overlapping_record_candidates` + `is_lookupish_name` with the
  decoded lookup-record↔name-order rule from B, falling back to non-overlap selection only
  for `Ref.vdf`-style non-lookupish-descriptor overlaps.
