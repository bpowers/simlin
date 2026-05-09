# vdf_xray.py heuristics audit (preliminary — pre-agent-findings)

Goal: enumerate every place `vdf_xray.py` uses scoring / scanning / interval-DP /
lexical "lookupish" tests / alphabetical reconstruction, identify when each fires, and
decide whether it can be replaced by a decoded rule. (The user's standing instruction:
heuristics signal incomplete understanding; they are bugs to fix where possible.)

## Category 0 — Display-only (do NOT affect extraction; low priority)

| Heuristic | Where | Fires | Verdict |
|-----------|-------|-------|---------|
| `_slot_name_alignment_class_score`, `score_slot_name_alignment`, `best_slot_name_alignment`, `preferred_slot_name_alignment`, `build_display_slot_to_names` | L1175-1311 | `--slots` display; also `build_owner_record_blocks` uses `preferred_slot_name_alignment` to get `hidden_slots` | Display alignment is fine to keep as a labelled exploratory aid. BUT `build_owner_record_blocks` consuming it for `hidden_slots` is a coupling we should sever (see C-1). The "skip N leading slots" is purely cosmetic; the doc already says so. **Action: keep as display, decouple from extraction.** |
| `find_slot_table` backward scan | L4176 | every file (to locate the slot table) | The doc says section-1 header `field1` *should* point at the slot/ref area but "edited files can retain leading or adjacent stale/helper entries". `risk2.vdf` is the documented case where the scan beats `field1`. **Likely-decodable**: `field1` + a deterministic rule for trimming stale leading entries (the stale ones are runtime-descriptor cells whose `slot_count - block1[7]` delta is ≥1; see vdf.md "Section-1 block-1 invariants"). Worth a focused attempt but low-stakes (the slot table doesn't drive name→OT). |

## Category A — Genuinely-needed reconstruction *today* because a format rule is missing

These fire only when `decoded_record_spans` (the fact-only path) is incomplete or
overlapping. On the model_editing corpus + subscripts.vdf they are all no-ops.

| Heuristic | Where | Fires when | What it papers over | Replaceable? |
|-----------|-------|------------|---------------------|--------------|
| `_select_non_overlapping_owner_blocks` (interval DP, weighted by OT-class) | L2045-2087 | record-derived owner spans overlap (Ref.vdf, WRLD3 SCEN01/experiment, econ) | the **owner/descriptor discriminator** — which of two records with conflicting `f[11]` is the emitted owner | **Pending disc-agent.** If the discriminator is decoded → delete the DP entirely. If proven ill-posed → keep but rename honestly. |
| `_heuristic_name_looks_lookupish` + `_heuristic_name_allowed_for_block` | L2608-2659 | a record-key name competes for a stock-coded block; lexical "lookup/table/graphical function" test | same descriptor/owner gap | **Pending disc-agent.** Lexical name tests are exactly the wrong shape for a C format. Should be replaceable by "is there a section-6 lookup record for this OT?" once the descriptor↔lookup-record binding is decoded. |
| `_lookup_record_names` + the `zip(lookup_names, lookup_records)` order-pairing in `extract_named_results_with_diagnostics` (L3647-3666) | L2783-2806, L3647 | a standalone lookup-definition name is otherwise unmapped and `len(lookupish names) == len(lookup records)` | the descriptor↔lookup-record-by-order binding (the doc says lookup tables appear in name-table order, so this is *probably* a decoded rule, just not framed as one) | **Likely decodable** — promote "lookup definitions appear in the name table in the same order as section-6 lookup records / section-7 packed lookup data" to a pinned fact (the doc already states it for section 7). Then the pairing is a fact, not a heuristic. |
| `_assign_group_positions` + `_nonstock_assignment_items` ("stocks-first-alphabetical" / Vensim-sort placement) | L2668-2782 | `decoded_record_spans` doesn't cover all OT slots → fall back to placing names alphabetically into class-coded OT positions | missing records for some OT-bearing variables (the `.Supplementary` `#`-name region; `f[11]==0` sentinel-loss; "zeroed" placeholder records) | **Pending helper-agent.** The model_editing fixtures prove every OT-bearing variable normally HAS a record; the question is why WRLD3's tail doesn't. If those records turn out to exist (just past `slot_count`, with `f[6]==0` or `f[2]==0`) and can be decoded → delete this fallback. If some OT slots genuinely have no record → keep but only as a last resort and flag it (it already flags via `used_system_variable_fallback`, but only for system names — model names placed this way are NOT flagged; that's a gap). |
| the `system_positions` alphabetical-zip fallback (L3679-3717) | L3679 | a system variable (INITIAL/FINAL/SAVEPER/TIME STEP) has no record (only on malformed/partial files) | malformed files | Low priority — system records exist on every clean fixture. Keep as last resort; already flagged. |
| `_recover_sec5_dimension_sets` (sec5 payload-subsequence element recovery) | L2491-2552 | a dimension has no complete `f[8]` element record catalog (Ref.vdf subranges) | nothing decodable is missing here — this IS a decoded rule (signal #14: subrange payload is an in-order subsequence of its root's payload). The "fallback" framing is misleading; it's a genuine decoded path for subrange dims. **Action: re-label, not remove.** |

## Category B — Things that look decoded but the doc still calls "reconstruction"

- **`decoded_record_spans` is the deterministic core** (L3062). It is *not* a heuristic:
  it uses the f[2] key (fact), in-range `f[11]` (fact), decoded `f[6]` shape (fact). On
  every clean fixture it produces a complete, non-overlapping, all-OT-slots map (modulo
  OT[0]=Time which is implicit). **`extract_named_results` should be restructured to use
  this directly as the primary path**, with Category A only for the residue. Today it
  instead goes through `build_owner_record_blocks` → `_select_non_overlapping_owner_blocks`
  → `_mapping_from_record_name_keys`, which for clean fixtures reproduces the same answer
  but via the heavier scaffolding. (`map_names_to_owner_blocks` even reruns the non-overlap
  filter twice.) This is the single biggest cleanup: a clean fixture's extraction should
  not touch a single line of reconstruction code.

- **`record_shape_length`'s `f[6]==32 → single active sec3 size` rule** is decoded
  (the model_editing fixtures + subscripts.vdf confirm: one active 1-D template, picked
  by `flat_size > 0`). Multi-active-template files would need `f[6]` = the sec3 index_word
  (the Ref-style path, also handled). Not a heuristic.

- **The axis-ref → dim-anchor binding** is fully decoded: `axis_ref == 60 + 16*k` where
  `k` is the anchor's record index (equivalently `sec1.data_offset + 4*axis_ref ==
  anchor.file_offset + 36`). `section3_axis_ref_to_dimension_anchor` already does this.
  Not a heuristic. (`_array_element_labels_from_sort_anchor` at L2974 is the *fallback*
  for files lacking this binding — it labels by cardinality-uniqueness, which IS a guess;
  the model_editing fixtures don't need it.)

## Category C — Couplings / cleanups (no decode needed, just code hygiene)

- **C-1**: `build_owner_record_blocks` consuming `preferred_slot_name_alignment().hidden_slots`
  to decide `block.hidden`. The "hidden" concept (a one-element helper block adjacent to a
  variable, whose owner record keys to a `#alias>FUNC#` signature) should be decided from
  the *signature/alias name relation* (which the doc says is the actual guard), not from
  slot-alignment scoring. Pending helper-agent's `#`-signature findings.
- **C-2**: Stale `/tmp/vdf_audit_phase1.md` references in comments (L3889, and the audit
  letters "B.2.1", "B.3.1" sprinkled around). That file no longer exists. Fix during
  integration: either restore the relevant notes here under `tools/vdf_xray_notes/` and
  re-point, or inline the reasoning.
- **C-3**: `extract_named_results_with_diagnostics` only sets `used_system_variable_fallback`
  for *system* names placed by the alphabetical-zip fallback. If a *model* name is ever
  placed by `_assign_group_positions`/the zip fallback path it should be flagged too (it
  currently can't happen because `_mapping_from_record_name_keys` doesn't use that path —
  but if Category A is invoked for the residue, it must flag).

## Corpus-wide `decoded_record_spans` coverage (measured)

Ran `decoded_record_spans` (the fact-only path) over all 41 tracked fixtures and
checked: does it cover every non-Time OT slot, and with zero overlaps?

| Group | Coverage of OT[1..N) | Overlapping span-claims | Missing OT slots |
|-------|----------------------|-------------------------|------------------|
| **All 31 `exact-by-xray`** (bact×8, consts×2, level_vs_aux×2, model_editing×10, pop×2, sd202_a2, water×4, subscripts, econ/risk) | **complete** | **0** | **0** |
| econ/base, econ/rk | complete | 3 | 0 |
| econ/mark2, econ/policy | complete | 3 | 0 |
| econ/risk2 | complete | 1 | 0 |
| lookup_ex | complete | 1 | 0 |
| WRLD3 SCEN01.VDF, WRLD3 experiment.vdf | complete | 54 | 0 |
| Ref.vdf | partial (3484/3913) | 58 | 429 |

So: **for every `exact-by-xray` fixture, `decoded_record_spans` ∪ {Time@OT[0]} is the
complete, deterministic results map** — the entire reconstruction stack (`build_owner_record_blocks`,
`_select_non_overlapping_owner_blocks`, `_mapping_from_record_name_keys`,
`_assign_group_positions`, the alphabetical-zip fallbacks, `_heuristic_name_allowed_for_block`,
`_lookup_record_names` pairing) is dead weight on those files. The 9 `not-proven`
fixtures all share the same one issue — *overlapping* record spans (the owner/descriptor
conflict) — and `Ref.vdf` additionally has 429 OT slots with no covering record (132
records have an `f[2]` that doesn't resolve to a parsed name; 132 resolved records have
`f[6]==0`; 205 records have `f[11]` outside `[1, OT_count)` — almost certainly C-LEARN
module-nesting quirks and/or an incomplete name-table parse — needs disc-agent / synthesis).

**Concrete refactor (Task #6):** `extract_named_results` should:
1. compute `decoded_record_spans`;
2. if it covers every OT slot in `[1, OT_count)` with no overlaps → emit it directly
   (with array-element labels from the decoded axis-ref → anchor → `f[8]`-catalog chain)
   + `Time@OT[0]`. *This is the entire path for 31/41 fixtures and touches zero
   reconstruction code.*
3. otherwise → keep the current reconstruction path **only for the residue** (the
   overlapping/uncovered OT slots), and flag it.

This is the single change that most directly serves "eliminate heuristics": it makes the
heuristic code unreachable except on the fixtures where the format genuinely isn't yet
decoded, and makes the `not-proven` set exactly "the fixtures with unresolved overlaps".

## Open items blocked on agents

1. Owner/descriptor discriminator (disc-agent → updates Category A row 1-3).
2. `.Supplementary` / `#`-signature / "zeroed"-record handling (helper-agent → updates
   Category A row 4, Category C-1).
3. Version-marker / `not-proven`-fixture re-examination (corpus-agent → may reveal that
   some `not-proven` blockers are tool artifacts, narrowing what Category A must handle).
