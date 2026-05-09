# LTM Cross-Element Aggregate Scoring — Phase 6: Cleanup, docs, and tracking

**Goal:** Remove the now-obviated `⁚wildcard` / `⁚dynamic` per-shape link-score path (made dead by Phase 5's aggregate-node reroute), update the LTM documentation to describe the aggregate-node treatment and element-level cross-element loop scoring, and update issue tracking (close GH #503, tick the corresponding box in epic #488). Verify no regression on scalar / pure-A2A models and that the test-suite stays within the 3-minute wall-clock cap.

**Architecture:** With reducers routed through `$⁚ltm⁚agg⁚{n}` nodes (Phase 5), `model_ltm_variables` no longer emits `$⁚ltm⁚link_score⁚…→to⁚wildcard` / `…→to⁚dynamic` variables. So: `link_score_var_name`'s `⁚wildcard`/`⁚dynamic` suffix arms, `parse_link_offsets`'s suffix-stripping (`strip_to_shape_suffix_with_rank`, `ShapeRank::Wildcard`/`::DynamicIndex`), the `LINK_SCORE_SHAPE_SUFFIXES` check in `assemble_module`'s LTM pass, `resolve_link_score_name_for_loop`'s Wildcard/DynamicIndex lookups, and `shape_aware_source_ref`'s Wildcard/DynamicIndex TODO comment all become dead. `ShapeRank` shrinks to `Bare`/`FixedIndex` (still needed for the Bare-A2A-vs-FixedIndex-A2A dedup tie-break). The `RefShape::Wildcard`/`RefShape::DynamicIndex` *enum variants survive* — they remain the reference-site-walker's markers for "this is a reducer/dynamic reference, route through an agg node." Tests pinned to the old behavior are removed; docs are updated; GH #503 is closed (manually — the repo forbids "fixes"/"resolves" auto-close keywords) and epic #488's `#503` checklist box is ticked.

**Tech Stack:** Rust; `simlin-engine` (`ltm_augment.rs`, `ltm_finding.rs`, `db.rs` assemble path); Markdown docs (`docs/design/ltm--loops-that-matter.md`, `src/simlin-engine/CLAUDE.md` + `AGENTS.md`, `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md`, `docs/README.md`); `gh` CLI for issue tracking.

**Scope:** Phase 6 of 6 from `docs/design-plans/2026-05-09-ltm-503-cross-element-agg.md`. **Codebase state caveat:** the codebase-investigator ran before Phases 1-5 landed — all locations below are pre-Phase-1 state; reconcile against whatever Phases 1-5 actually produced (e.g. `LtmSyntheticVar.equation` is `datamodel::Equation` after Phase 1; `resolve_link_score_name_for_loop` has a `target_element` param after Phase 2; `link_score_var_name` may already have lost the `⁚wildcard`/`⁚dynamic` *emission* in Phase 5 even if the arms still exist).

**Codebase verified:** 2026-05-09 (codebase-investigator).

---

## Acceptance Criteria Coverage

This phase implements and tests:

### ltm-503-cross-element-agg.AC5: Wildcard/Dynamic link-score path retired
- **ltm-503-cross-element-agg.AC5.1 Success:** No `$⁚ltm⁚link_score⁚…⁚wildcard` or `…⁚dynamic` variables are emitted for any model; `link_score_var_name` no longer appends those suffixes; `parse_link_offsets` no longer strips them; `resolve_link_score_name_for_loop` has no `Wildcard`/`DynamicIndex` cases.
- **ltm-503-cross-element-agg.AC5.2 Success:** `shape_aware_source_ref`'s Wildcard/DynamicIndex TODO branch is removed (the function's only remaining special case, if any, is `FixedIndex`).

### ltm-503-cross-element-agg.AC6: No regression on scalar / pure-A2A models; perf budget
- **ltm-503-cross-element-agg.AC6.1 Success:** Golden-data integration tests for scalar and pure-A2A models (e.g. `simulates_population_ltm` vs `test/logistic_growth_ltm/ltm_results.tsv`, the WRLD3 LTM smoke) pass with unchanged expected values.
- **ltm-503-cross-element-agg.AC6.2 Success:** `cargo test --workspace` completes within the 3-minute wall-clock cap; the pre-commit hook passes.

### ltm-503-cross-element-agg.AC7: Docs and tracking updated
- **ltm-503-cross-element-agg.AC7.1 Success:** `docs/design/ltm--loops-that-matter.md` describes the aggregate-node treatment and element-level cross-element loop scoring; `src/simlin-engine/CLAUDE.md` reflects the new `$⁚ltm⁚agg⁚{n}` synthetic family, the retired Wildcard path, and the `LtmSyntheticVar.equation` type change; the `2026-04-25-ltm-per-ref-elem-graph.md` measurement postscript notes the SCC/loop-count shift on reducer-bearing fixtures.
- **ltm-503-cross-element-agg.AC7.2 Success:** GH #503 is marked resolved referencing the implementing commit(s); epic #488's checklist is ticked.

> Phase 6 depends on Phase 5.

---

## Context for the implementer (read before starting)

### What's dead after Phase 5, and what survives

| Dead (remove in Phase 6) | Survives |
|---|---|
| `link_score_var_name`'s `RefShape::Wildcard => …⁚wildcard` and `RefShape::DynamicIndex => …⁚dynamic` arms (`ltm_augment.rs:462-466`) | `link_score_var_name`'s `RefShape::FixedIndex => {from}[{elems}]→{to}` and `Bare => {from}→{to}` arms; the Phase-3-added scalar→arrayed `{from}→{to}[{elem}]` emission |
| `LINK_SCORE_WILDCARD_SUFFIX` / `LINK_SCORE_DYNAMIC_SUFFIX` / `LINK_SCORE_SHAPE_SUFFIXES` constants (`ltm_augment.rs:446,450,455`) — once all consumers are gone | `LINK_SCORE_PREFIX` (`db.rs:4985`, `ltm_finding.rs:44`) |
| `strip_to_shape_suffix_with_rank` (`ltm_finding.rs:467-475`) and its callsite in `parse_link_offsets` (`ltm_finding.rs:355-362`) | `parse_link_offsets`'s four-way dispatch (minus the suffix-rank input) |
| `ShapeRank::Wildcard` / `ShapeRank::DynamicIndex` (`ltm_finding.rs:310-316`) — enum shrinks to `Bare = 0, FixedIndex = 1` (or replace with a `bool is_fixed_index`) | `ShapeRank`/the rank concept (still needed: `test_parse_link_offsets_dedupes_a2a_bare_over_fixed_index` at `ltm_finding.rs:1856` pins Bare(0) < FixedIndex(1)) |
| `resolve_link_score_name_for_loop`'s `wildcard`/`dynamic` lookup blocks (`ltm_augment.rs:914-921`) and the doc-comment lines describing them | `resolve_link_score_name_for_loop`'s Bare check, `find_fixed_index_emitted_name` (`ltm_augment.rs:930-945`), the Bare fallback; the Phase-2 `target_element` param and the Phase-3 `{from}→{to}[{elem}]` resolution |
| `db.rs:5002`'s `let has_shape_suffix = LINK_SCORE_SHAPE_SUFFIXES.iter().any(|s| suffix.ends_with(s))` in `assemble_module` LTM pass 3 (full block `db.rs:4985-5036`) — becomes an `[elem]`-only check; doc comment `db.rs:4993-4997` loses case "(c) Per-shape Wildcard/DynamicIndex" | the `[`/`]` element-subscript detection and the salsa-cached vs direct-compile branching for the surviving link-score-var forms |
| `shape_aware_source_ref`'s doc-comment TODO (`ltm_augment.rs:717-738`, the aggregate-denominator follow-up — quoted verbatim in GH #503) — **note: there is NO Wildcard/DynamicIndex *code* branch; the code already is `match { FixedIndex(non-empty) => from[elems], _ => from }`**. Phase 6's work here is: delete the TODO comment; the function body needs no change (or trivial — once no Wildcard/DynamicIndex shape reaches it post-Phase-5). | `shape_aware_source_ref`'s `FixedIndex` special case |
| `RefShape::Wildcard`/`DynamicIndex` *as link-score-generation markers* | `RefShape::Wildcard`/`DynamicIndex` *as reference-site-walker markers* — `classify_subscript_shape` (`db_analysis.rs:459`), `emit_edges_for_reference` (`db_analysis.rs:520`, the arm Phase 5 rerouted through agg), `classify_cycle` (`db_analysis.rs:1396-1450`), `model_edge_shapes` — **keep the enum variants** |
| `classify_expr0_subscript_shape` / `wrap_non_matching_in_previous` / `build_partial_equation_shaped`'s shaped-partial machinery — **AUDIT**: if Phase 5 made the agg's `source[d]→agg` link score use the reducer machinery (`generate_element_to_scalar_equation`/`generate_element_to_reduced_equation`) and the `agg→target` link score a Bare partial, then no caller passes a `Wildcard`/`DynamicIndex` `RefShape` to `build_partial_equation_shaped` anymore — but `FixedIndex` and `Bare` still do (Phase 1's arrayed-target partials, the FixedIndex link scores). So `build_partial_equation_shaped` survives; `classify_expr0_subscript_shape`'s `Wildcard`/`DynamicIndex` arms become dead-code-but-harmless (or remove them if `Expr0` subscripts can never be wildcards in the surviving call paths — verify). | `build_partial_equation_shaped` / `wrap_non_matching_in_previous` (used for Bare + FixedIndex partials) |
| `link_score_equation_text_shaped` / `enumerate_shapes` / `emit_per_shape_link_scores` — **AUDIT**: post-Phase-5 these only ever see `Bare` and `FixedIndex` shapes; simplify (drop the Wildcard/Dynamic branches) or leave them (harmless). The design's Phase-6 prose doesn't name these; do whatever keeps the code clean and the tests green. | the Bare/FixedIndex emission path |

### Tests removed / rewritten

| Test | Location | Action |
|---|---|---|
| `test_parse_link_offsets_wildcard_suffix_scalar` | `src/simlin-engine/src/ltm_finding.rs:1512` | **REMOVE** (the `⁚wildcard` suffix no longer exists) |
| `test_parse_link_offsets_wildcard_suffix_a2a_expansion` | `src/simlin-engine/src/ltm_finding.rs:1550` | **REMOVE** |
| `test_parse_link_offsets_dedupes_bare_and_wildcard_for_same_edge` | `src/simlin-engine/src/ltm_finding.rs:1729` | **REMOVE** |
| `test_parse_link_offsets_dedupes_a2a_bare_and_wildcard` | `src/simlin-engine/src/ltm_finding.rs:1784` | **REMOVE** |
| `test_parse_link_offsets_dedupes_a2a_bare_over_fixed_index` | `src/simlin-engine/src/ltm_finding.rs:1856` | **KEEP** — pins the Bare(0) < FixedIndex(1) dedup; if `ShapeRank` becomes a `bool`, adapt the assertion but keep the test |
| `test_parse_link_offsets_fixed_index_from_a2a_expansion` / `..._from_scalar` | `src/simlin-engine/src/ltm_finding.rs:1616 / 1681` | **KEEP** (their `LtmSyntheticVar` literals already migrated to `datamodel::Equation` in Phase 1) |
| `link_score_name_wildcard_always_suffixed` | `src/simlin-engine/src/ltm_augment.rs:2161` | **REMOVE** |
| `link_score_name_dynamic_index_always_suffixed` | `src/simlin-engine/src/ltm_augment.rs:2177` | **REMOVE** |
| `resolver_prefers_bare_over_other_shapes` | `src/simlin-engine/src/ltm_augment.rs:2276` | **REWRITE** (drop the `pop→share⁚wildcard` variant; keep the Bare-vs-FixedIndex preference; rename `resolver_prefers_bare_over_fixed_index`) |
| `loop_score_equation_falls_back_to_wildcard_when_bare_not_emitted` | `src/simlin-engine/src/ltm_augment.rs:2312` | **REMOVE** (or rewrite so the loop goes through `$⁚ltm⁚agg⁚0`, if it isn't already covered by a Phase-5 test) |
| `partial_equation_dynamic_index_wraps_inner_deps` | `src/simlin-engine/src/ltm_augment.rs:2354` | **AUDIT** — fate depends on whether the shaped-partial `DynamicIndex` path survives; remove if dead |
| `test_partial_equation_share_wildcard_shape` | `src/simlin-engine/src/ltm_augment.rs:2034` | **REMOVE** (Phase 5 already handled this, but if Phase 5 left it, remove it here) |
| `per_shape_link_scores_for_share_with_sum`, `loop_score_picks_emitted_shape_when_only_wildcard_exists` | `src/simlin-engine/src/db_ltm_unified_tests.rs:728 / 1392` | **already rewritten in Phase 5** to the agg-node path — confirm; if not, do it here |
| `cross_element_wildcard_in_a2a`, `element_graph_wildcard_reducer_plus_bare_truthful` | `src/simlin-engine/src/db_element_graph_tests.rs:214 / 719` | **already rewritten in Phase 5** to the agg topology — confirm |
| Helper `subscript_wildcard` (`ltm_finding.rs`), comments referencing `RefShape` Wildcard/Dynamic (`db_ltm_unified_tests.rs:721,921`, `db_element_graph_tests.rs:662-667`) | various | delete orphaned helper; update stale comment prose |

### Docs to update

| File | What | Notes |
|---|---|---|
| `docs/design/ltm--loops-that-matter.md` (~827 lines) | (a) "Naming Convention" table (lines 216-225): add `$⁚ltm⁚agg⁚{n}` (synthetic aggregate node = hoisted maximal inlined reducer subexpression; an aux whose equation is the canonical reducer subexpr, inserted between the reducer's array-element sources and the consumers that referenced it inline; whole-RHS-scalar reducers are *not* synthesized — the variable itself is the agg). (b) "Array Support → Element-Level Causal Graph" table (lines 540-549): change the `Wildcard`/`DynamicIndex` rows from "full cross-product" to "rerouted through `$⁚ltm⁚agg⁚{n}` — `from[d] → agg` + `agg → to[e]`, O(N+M)"; rewrite the prose at lines 559-573, 581-592. (c) "Array Support → Link Score Classification" (lines 596-617): note reducers are hoisted into agg auxes (`pop[d]→agg` = the reducer's own equation; `agg→target` = a plain Bare scalar→arrayed/A2A link); mention Phase-4 arrayed-result reducer support. (d) "Array Support → Loop Scores" (lines 619-654), esp. the Mixed-loops paragraph (651-654): cross-element loops are now element-subscripted with subscripted link-score refs (Phase 2); update the `classify_cycle` description. (e) "Two Modes / Discovery" (lines 117-132, 656-672): scalar→arrayed link scores now named `$⁚ltm⁚link_score⁚{from}→{to}[{elem}]` per target element (Phase 3). (f) ADD a subsection on aggregate nodes (rationale: keeps the element graph O(N+M) per reducer, the link-score denominator is naturally Δ(aggregate) — resolves GH #503; canonical-subexpr keying via `enumerate_agg_nodes`; the loop-reporting trim — agg nodes don't appear in the user-facing loop list, like DELAY3/SMOOTH internal nodes in the papers; the whole-RHS special case). Also retire/rewrite whatever the doc says about the per-shape Wildcard link score (tech-debt #26's mechanism). | Editing this existing file does NOT require a `docs/README.md` change (already indexed at `docs/README.md:7`). |
| `src/simlin-engine/CLAUDE.md` **and** `src/simlin-engine/AGENTS.md` (byte-identical — edit both, or check if one is a symlink) | (a) `ltm_augment.rs` bullet: delete the `link_score_var_name` "Wildcard always suffixes `\u{205A}wildcard`, DynamicIndex always suffixes `\u{205A}dynamic`" clause; add the scalar→arrayed `{from}→{to}[{elem}]` convention (Phase 3); add `enumerate_agg_nodes`; note `generate_link_score_equation_for_link` no longer takes a per-shape `RefShape` for the Wildcard/Dynamic cases; if `build_partial_equation_shaped`/`classify_expr0_subscript_shape` lost their Wildcard/Dynamic relevance, reword. (b) `db_analysis.rs` bullet: keep `Bare`/`FixedIndex`/`Wildcard`/`DynamicIndex` but add "Wildcard/DynamicIndex references are rerouted through a synthetic `$⁚ltm⁚agg⁚{n}` aggregate node rather than emitting a full cross-product"; same for the `classify_cycle` clause. (c) `db_ltm.rs` bullet: add "reducer subexpressions are hoisted into `$⁚ltm⁚agg⁚{n}` auxiliaries (`enumerate_agg_nodes`); `model_ltm_variables` emits those auxes plus their link scores; loop reporting trims agg nodes"; update "mixed scalar loops" → "element-subscripted cross-element loops" (Phase 2); add that `LtmSyntheticVar.equation` is now `datamodel::Equation` (Phase 1). (d) `ltm/types.rs` bullet: note Wildcard/Dynamic are now agg-routing markers. (e) **Decide on the freshness-date convention**: the file currently has a "Maintenance note: Keep this file up to date when adding, removing, or reorganizing modules." but **no date**. The `writing-claude-md-files` skill mandates a freshness date — Phase 6 would be *introducing* one (e.g. add `**Last updated: 2026-05-09**` near the maintenance note). If you're unsure whether to introduce it, ask the user; otherwise add it. | These are *the* engine module-map docs; keep them accurate. |
| `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md` (~638 lines) | Add a new subsection to the existing "Measurement Postscript" (heading at line 573; the existing table at ~573-639 records 2026-04-25 baselines: `cross_element_ltm` 8/20/10 → 18/10, `arrayed_population_ltm` 6/18/3 → 18/3, `hero_culture_ltm` 41/41/15, WRLD3-03 483/483/166 auto-flip yes) recording the post-Phases-1-5 numbers: re-run the `ltm_full_bench` example (or `measure_tiered` in `simulate_ltm.rs`) on `cross_element_ltm`, `arrayed_population_ltm`, `hero_culture_ltm`, WRLD3-03, **plus** the new reducer-bearing fixtures Phase 5 added (`share[r]=pop[r]/SUM(pop[*])`-with-feedback etc. — for those, element-edge count drops because NxM cross-products → N+M through agg; that's the measurable win). Note: `cross_element_ltm`'s `total_population=SUM(...)` is whole-RHS so it gains no synthetic agg → its element-edge count may be a wash there, but its *loop scores* go from garbage (diagonal A2A) to correct (element-level). Update the threshold note if any fixture's SCC moved. Cross-link to `2026-05-09-ltm-503-cross-element-agg.md`. | Editing this existing file does NOT require a `docs/README.md` change. The two `measurement_postscript_*` tests in `simulate_ltm.rs` (`:3982`, `:4014`) have rustdocs pinning the old narrative — update those rustdocs to match the re-measured numbers (the tests' *asserts* are loose — `m.fast_path >= 1`, `m.slow_path_scc <= m.elem_scc`, `m.slow_path == 0` for pure-A2A — so they likely still pass, but the rustdoc narrative must be accurate). |
| `docs/README.md` | **No change required** for the planned edits (all to existing files; `2026-05-09-ltm-503-cross-element-agg.md` is already indexed at line 27). *Optional:* add the missing `2026-04-25-ltm-per-ref-elem-graph.md` line to the `design-plans/` nested list (a pre-existing omission). | per `docs/CLAUDE.md` ("update README.md when adding/moving/renaming files under docs/") — none of Phase 6's edits add/move/rename a file. |
| `docs/tech-debt.md` | Update the **RESOLVED-note prose** of: #20 (`:181`, "FixedIndex N² edges" — add: the same per-reference approach now also handles Wildcard/Dynamic via agg-node hoisting, see `2026-05-09-ltm-503-cross-element-agg.md`); #26 (`:235`, "A2A partial wrong with mixed refs" — add a "superseded-by" line: the per-shape Wildcard link score described here was retired in `<commit>`; reducer references are now hoisted into `$⁚ltm⁚agg⁚{n}` auxes; the Bare-vs-FixedIndex per-shape split survives); #34 (`:305`, "A2A loop-score slot-0 broadcast" — add: the relaxed `test_cross_element_ltm_exhaustive` assertions it mentions were tightened in `<commit>` once cross-element loops became correct). Add the implementing commit hash(es) to each. **No new tech-debt row** is created (#503 has no dedicated row; it's closed as a GH issue). | If Phases 1-5 surfaced a *new* limitation (e.g. the multidim partial-fixed `source[NYC,*]` case still over-approximating, or STDDEV/RANK delta-ratio being more visible now), file it via the `track-issue` agent — do NOT silently drop it. |

### GitHub tracking (read the issues with `gh issue view 503` / `gh issue view 488` first; repo is `bpowers/simlin`)

- **GH #503** (`ltm: cross-element loops should normalize by Δ-aggregate, not diagonal A2A link score`, OPEN): close it with `gh issue close 503 --comment "..."` where the comment references the implementing commit(s) and notes that the fix took the **aggregate-node** approach (hoist the reducer into `$⁚ltm⁚agg⁚{n}`, score `pop[d]→agg→target` by the chain rule, retire the `⁚wildcard` suffix machinery) rather than the threading-`SUM(from[*])`-through-`shape_aware_source_ref` approach the issue originally sketched. Do NOT use a "fixes #503" / "resolves #503" keyword in a commit (the repo `CLAUDE.md` forbids auto-close keywords) — close it manually.
- **GH #488** (`epic: Loops that Matter (LTM)`, OPEN): edit the epic body — under the "Augmentation -- `src/simlin-engine/src/ltm_augment.rs`" checklist, change `- [ ] #503 -- cross-element loops should normalize by Δ-aggregate...` to `- [x] #503 -- ...` (done in `<commit>`). Use `gh issue edit 488 --body-file -` (or `gh api`) — fetch the current body, make the one-character edit, write it back.
- #487, #483, #309 are **out of scope** — do not touch.

### Conventions

TDD where there's behavior to test (AC5.1's "no `⁚wildcard`/`⁚dynamic` vars emitted" is a positive assertion you can write a test for: build a few reducer-bearing models, fetch `model_ltm_variables`, assert no var name contains `⁚wildcard` or `⁚dynamic`). Removals: delete the dead code and the dead tests in the same commit; let `cargo build`/`cargo clippy --all-targets` flag any orphans. Docs/tracking are not TDD'd but must be accurate. Each new test under ~2s; `cargo test --workspace` under the 3-minute cap (AC6.2 — Phase 6 is also where you confirm the *whole* suite, after all 6 phases' changes, fits). Commits: `engine: ...` for code, `doc: ...` for the docs-only commit, no emoji, no `Co-Authored-By`, never `--no-verify`. Lean on the pre-commit hook.

---

## Tasks

<!-- START_TASK_1 -->
### Task 1: Remove the dead `⁚wildcard` / `⁚dynamic` link-score path; shrink `ShapeRank`

**Verifies:** ltm-503-cross-element-agg.AC5.1, ltm-503-cross-element-agg.AC5.2

**Files:**
- Modify: `src/simlin-engine/src/ltm_augment.rs` — drop the `RefShape::Wildcard => …⁚wildcard` / `RefShape::DynamicIndex => …⁚dynamic` arms of `link_score_var_name` (`:462-466`) so `to_part = to.to_string()` always; delete `LINK_SCORE_WILDCARD_SUFFIX` / `LINK_SCORE_DYNAMIC_SUFFIX` / `LINK_SCORE_SHAPE_SUFFIXES` constants (`:446,450,455`) once no consumer remains; drop the `wildcard`/`dynamic` lookup blocks from `resolve_link_score_name_for_loop` (`:914-921`) and the corresponding doc-comment lines; delete the `shape_aware_source_ref` doc-comment TODO (`:717-738`) — the function body needs no change; audit `classify_expr0_subscript_shape` / `link_score_equation_text_shaped` / `enumerate_shapes` / `emit_per_shape_link_scores` and simplify (drop dead Wildcard/Dynamic branches) or leave them, whichever keeps the code clean.
- Modify: `src/simlin-engine/src/ltm_finding.rs` — delete `strip_to_shape_suffix_with_rank` (`:467-475`) and its callsite in `parse_link_offsets` (`:355-362`); shrink `ShapeRank` (`:310-316`) to `Bare = 0, FixedIndex = 1` (or replace it entirely with a `bool is_fixed_index` threaded into the dedup tie-break key at ~`:430-441` — your call; keep the Bare-beats-FixedIndex semantics).
- Modify: `src/simlin-engine/src/db.rs` — `assemble_module` LTM pass 3 (`:4985-5036`): drop the `let has_shape_suffix = LINK_SCORE_SHAPE_SUFFIXES.iter().any(|s| suffix.ends_with(s))` term at `:5002`; the branch that previously routed `…⁚wildcard`/`…⁚dynamic` vars to `compile_ltm_equation_fragment` directly is no longer reachable — keep the `[`/`]` element-subscript detection and the salsa-cached-vs-direct branching for the surviving forms; update the doc comment at `:4993-4997` to drop case "(c) Per-shape Wildcard/DynamicIndex".
- Decide: keep `RefShape::Wildcard` / `RefShape::DynamicIndex` enum variants (`db_analysis.rs:77-94`) — **yes, keep them** (the reference-site walker still constructs/matches them as "route through agg" markers); add a doc comment to the enum explaining their post-Phase-5 meaning.
- Remove dead tests (see the "Tests removed / rewritten" table above): `test_parse_link_offsets_wildcard_suffix_scalar` (`ltm_finding.rs:1512`), `..._wildcard_suffix_a2a_expansion` (`:1550`), `..._dedupes_bare_and_wildcard_for_same_edge` (`:1729`), `..._dedupes_a2a_bare_and_wildcard` (`:1784`), `link_score_name_wildcard_always_suffixed` (`ltm_augment.rs:2161`), `link_score_name_dynamic_index_always_suffixed` (`:2177`), `loop_score_equation_falls_back_to_wildcard_when_bare_not_emitted` (`:2312`), `test_partial_equation_share_wildcard_shape` (`:2034` if Phase 5 left it), `partial_equation_dynamic_index_wraps_inner_deps` (`:2354` if its path is dead); rewrite `resolver_prefers_bare_over_other_shapes` (`:2276`) → `resolver_prefers_bare_over_fixed_index`; delete orphaned helper `subscript_wildcard` in `ltm_finding.rs` if no longer referenced; update stale `RefShape`-mentioning comments.
- Add: a test (in `db_ltm_unified_tests.rs` or `simulate_ltm.rs`) asserting AC5.1 positively — build 2-3 reducer-bearing models (`share[r]=pop[r]/SUM(pop[*])`-with-feedback, `total_pop=SUM(pop[*])`, a `MEAN`-reducer model), fetch `model_ltm_variables` for each, assert no `v.name` contains `"\u{205A}wildcard"` or `"\u{205A}dynamic"`.

**Implementation contract:**
- No `$⁚ltm⁚link_score⁚…⁚wildcard` or `…⁚dynamic` variable is emitted by `model_ltm_variables` for any model (this should already be true after Phase 5's reroute; Phase 6 removes the now-dead *machinery* and asserts the property).
- `link_score_var_name`, `parse_link_offsets`, `resolve_link_score_name_for_loop`, `shape_aware_source_ref` no longer reference the suffixes; `ShapeRank` (or its replacement) no longer has Wildcard/DynamicIndex.
- `RefShape::Wildcard`/`DynamicIndex` enum variants remain (walker markers).
- `cargo clippy --workspace --all-targets -- -D warnings` is clean (no dead-code warnings, no unused imports/constants left behind).

**Testing:** TDD for the positive AC5.1 assertion (write the test, confirm it fails if you stub out the Phase-5 reroute — actually it already passes post-Phase-5, so just add it as a guard). For the removals, the test is "everything still compiles and all surviving tests pass."

**Verification:**
- Run: `cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings` — clean.
- Run: `cargo test -p simlin-engine && cargo test -p simlin-engine --features file_io --test simulate_ltm` — green; the AC5.1 guard test passes; no test references the deleted symbols.

**Commit:** `engine: retire the wildcard/dynamic per-shape link-score path`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Update LTM documentation (design doc, engine CLAUDE.md/AGENTS.md, measurement postscript)

**Verifies:** ltm-503-cross-element-agg.AC7.1

**Files:**
- Modify: `docs/design/ltm--loops-that-matter.md` — per the "Docs to update" table above: add `$⁚ltm⁚agg⁚{n}` to the naming-convention table; rewrite the array-support truth table's Wildcard/DynamicIndex rows and the surrounding prose for the agg-node reroute; update the Link Score Classification, Loop Scores, and Discovery sections for the agg-node link scores, element-level cross-element loop scoring, and the Phase-3 scalar→arrayed naming; add an aggregate-node subsection; retire/rewrite the per-shape-Wildcard description.
- Modify: `src/simlin-engine/CLAUDE.md` **and** `src/simlin-engine/AGENTS.md` (byte-identical — change both; if one is a symlink, one edit suffices) — per the table: `ltm_augment.rs` / `db_analysis.rs` / `db_ltm.rs` / `ltm/types.rs` bullets updated for `enumerate_agg_nodes`, the retired Wildcard path, the `$⁚ltm⁚agg⁚{n}` family, `LtmSyntheticVar.equation: datamodel::Equation`, element-subscripted cross-element loops; **add a freshness date** (`**Last updated: 2026-05-09**` near the Maintenance note) — or ask the user if unsure.
- Modify: `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md` — add a new subsection to the "Measurement Postscript" with the re-measured post-Phases-1-5 numbers (run `cargo run --release --example ltm_full_bench -- <fixture>` for each of `cross_element_ltm`, `arrayed_population_ltm`, `hero_culture_ltm`, WRLD3-03, plus the new reducer fixtures); cross-link to `2026-05-09-ltm-503-cross-element-agg.md`.
- Modify: `src/simlin-engine/tests/simulate_ltm.rs` — update the rustdocs of `measurement_postscript_cross_element_ltm` (`:3982`) and `measurement_postscript_arrayed_population_ltm` (`:4014`) to reflect the re-measured numbers (the asserts are loose; the rustdoc narrative must be accurate).
- Optional: `docs/README.md` — add the missing `2026-04-25-ltm-per-ref-elem-graph.md` line to the `design-plans/` nested list.

**Implementation contract:** Documentation matches the post-Phases-1-5 reality: the aggregate-node treatment, element-level cross-element loop scoring, the retired Wildcard path, the `LtmSyntheticVar.equation` type change, the new `$⁚ltm⁚agg⁚{n}` synthetic family, the Phase-3 scalar→arrayed naming. The measurement postscript records the SCC/loop-count shift. No stale references to `⁚wildcard`/`⁚dynamic` link scores or "full cross-product for Wildcard" remain in the docs.

**Testing:** N/A (docs). Verify by re-reading the changed sections against the implemented code; run `python3 scripts/check-docs.py` (the docs-links checker, part of the pre-commit hook) to catch broken links.

**Verification:**
- Run: `python3 scripts/check-docs.py` — passes.
- Run: `cargo test -p simlin-engine --features file_io --test simulate_ltm measurement_postscript` — the two postscript tests still pass (their asserts are loose); their rustdocs now match the eprintln'd numbers.
- Manual: re-read `docs/design/ltm--loops-that-matter.md`'s Array Support / Naming Convention / Link Score Classification sections — they describe agg nodes, not the old Wildcard cross-product.

**Commit:** `doc: document LTM aggregate nodes and cross-element loop scoring`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Verify no scalar / pure-A2A regression and the 3-minute budget; close GH #503; tick epic #488

**Verifies:** ltm-503-cross-element-agg.AC6.1, ltm-503-cross-element-agg.AC6.2, ltm-503-cross-element-agg.AC7.2

**Files:**
- (no source changes expected) — this task is verification + tracking.
- Possibly modify: `test/logistic_growth_ltm/ltm_results.tsv` — **only if** `simulates_population_ltm` fails and investigation confirms the new expected values are *correct* (a scalar model should be untouched by Phases 1-6 — if `simulates_population_ltm` changes, that's a bug, not a golden update; investigate first). Same caution for any other golden TSV.

**Implementation contract:**
- `simulates_population_ltm` (`simulate_ltm.rs:287`, vs `test/logistic_growth_ltm/ltm_results.tsv`), the WRLD3 LTM smoke (`tests/wrld3_ltm_panic.rs` and any WRLD3 LTM golden), and the other scalar / pure-A2A LTM tests pass with **unchanged** expected values. (Pure-A2A and scalar models have no reducers and no per-element-equation link-score targets ⇒ no agg nodes, no element-graph change ⇒ identical behavior. If any of these regress, it's a bug introduced somewhere in Phases 1-6 — fix it, don't update the golden.)
- `cargo test --workspace` completes within the 3-minute wall-clock cap; the pre-commit hook (`scripts/pre-commit`) passes (Rust fmt + clippy + tests + TS lint/typecheck + WASM build + TS tests + pysimlin tests).
- GH #503 is closed with `gh issue close 503 --comment "..."` referencing the implementing commit(s) and noting the aggregate-node approach.
- GH #488's "Augmentation" checklist line for #503 is changed from `- [ ]` to `- [x]` (with the commit ref) via `gh issue edit 488`.

**Testing:** This task IS testing — run the suites.

**Steps:**
1. Run `cargo test --workspace` (or just `git commit` an empty-ish/docs commit and let the pre-commit hook run it under the 180s cap). Confirm green within the cap. If any scalar / pure-A2A test regressed, STOP — investigate and fix the underlying Phases-1-5 bug (do not update goldens to mask it).
2. Run the full pre-commit hook (`git commit ...`) — confirm it passes (fmt, clippy, Rust tests, TS lint/typecheck/test, WASM build, pysimlin).
3. `gh issue view 503` to recall the body; `gh issue close 503 --comment "Resolved by <commit-sha(s)>. Implemented via the aggregate-node approach (hoist each maximal inlined reducer into a synthetic \$⁚ltm⁚agg⁚{n} auxiliary; score pop[d]→agg→target by the chain rule; element-level cross-element loops; retired the ⁚wildcard/⁚dynamic per-shape link-score path) per docs/design-plans/2026-05-09-ltm-503-cross-element-agg.md."` (use the real commit SHAs from this branch's `git log`).
4. `gh issue view 488` to fetch the epic body; edit the `- [ ] #503 -- cross-element loops should normalize by Δ-aggregate...` line to `- [x] #503 -- ... (done in <commit-sha>)`; write it back with `gh issue edit 488 --body-file -` (pipe the edited body).
5. If Phases 1-5 surfaced any new limitation (multidim partial-fixed over-approximation, STDDEV/RANK delta-ratio more visible, etc.) that isn't already tracked, spawn the `track-issue` agent (`Task` tool, `subagent_type: "track-issue"`) with a description — do not silently drop it.

**Verification:**
- Run: `cargo test --workspace` — green within ~3 minutes.
- `git commit` — the pre-commit hook passes end-to-end.
- `gh issue view 503` — shows CLOSED with the resolving comment.
- `gh issue view 488` — the `#503` checklist box is `[x]`.

**Commit:** (this task's "commit" is the act of committing the docs/cleanup and letting the pre-commit hook gate the whole suite; the GH issue updates are not commits. If there's any residual source change, `engine: <description>`.)
<!-- END_TASK_3 -->

---

## Phase 6 done-when checklist

- [ ] AC5.1 — no `⁚wildcard`/`⁚dynamic` link-score vars emitted for any model; `link_score_var_name` / `parse_link_offsets` / `resolve_link_score_name_for_loop` no longer reference the suffixes; `ShapeRank` shrunk (or replaced by a bool); `db.rs:5002`'s suffix check removed; the dead tests removed/rewritten; `RefShape::Wildcard`/`DynamicIndex` enum variants kept (with an updated doc comment) as walker markers.
- [ ] AC5.2 — `shape_aware_source_ref`'s Wildcard/DynamicIndex TODO comment removed; its only special case (if any) is `FixedIndex`.
- [ ] AC6.1 — `simulates_population_ltm` and the WRLD3 LTM smoke (and other scalar / pure-A2A LTM tests) pass with unchanged expected values (no golden updates — if any regress, it's a bug to fix).
- [ ] AC6.2 — `cargo test --workspace` within the 3-minute cap; the pre-commit hook passes.
- [ ] AC7.1 — `docs/design/ltm--loops-that-matter.md` describes the aggregate-node treatment + element-level cross-element loop scoring + the Phase-3 naming; `src/simlin-engine/CLAUDE.md` (+ `AGENTS.md`) reflects `$⁚ltm⁚agg⁚{n}`, the retired Wildcard path, `LtmSyntheticVar.equation: datamodel::Equation` (and gains a freshness date); the `2026-04-25-ltm-per-ref-elem-graph.md` postscript records the re-measured SCC/loop counts; the `measurement_postscript_*` test rustdocs updated; `check-docs.py` passes.
- [ ] AC7.2 — GH #503 closed (manually, no auto-close keyword) referencing the implementing commit(s) and the agg-node approach; epic #488's `#503` checklist box ticked.
- [ ] tech-debt.md #20/#26/#34 RESOLVED-note prose updated with forward-pointers + commit hashes; any new limitation surfaced by Phases 1-5 filed via the `track-issue` agent.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt -- --check` clean.
