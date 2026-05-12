# LTM Arrays Hardening — Phase 1 Implementation Plan

**Goal:** Introduce one salsa-tracked reference-site classification IR (`model_ltm_reference_sites`) that becomes the only place a causal edge's access shape and aggregate-node routing are decided, and consolidate the array-reducer recognition set (currently restated five times) into one `reducer_kind` table. Behavior-preserving — every existing LTM test stays green with no test edits, and the one checked-in golden trace is byte-unchanged.

**Architecture:** A new `db.rs` submodule `db_ltm_ir.rs` (mirroring the existing `db_analysis.rs` / `db_ltm.rs` salsa-module precedent) hosts `model_ltm_reference_sites(db, model, project) -> LtmReferenceSitesResult`. It consumes `enumerate_agg_nodes` (which stays the sole decider of "is this subexpression a hoistable maximal reducer") and `reconstruct_model_variables`, walks each variable's `Expr2` AST once, and buckets each `Var`/`Subscript` reference by its `(from, to)` causal edge into a `Vec<ClassifiedSite>` (`shape` + `target_element` + `routing ∈ {Direct, ThroughAgg{agg}}`). `model_element_causal_edges`, `model_edge_shapes`, and `model_ltm_variables` become pure readers of the IR — none re-walks the AST for shape/routing, none restates the `routed_aggs` filter. Separately, `ltm_agg.rs` gains `reducer_kind` / `ReducerKind` (the existing 3-variant enum, moved here) + a monotone predicate; `builtin_is_array_reducer` is deleted and `reducer_source_vars`, `agg_reducer_is_monotone`, `ltm_augment::classify_reducer`, `ltm_augment::is_array_reducer_name` become thin readers.

**Tech Stack:** Rust; salsa incremental-computation framework (`#[salsa::tracked]` attribute API, `SourceModel` / `SourceProject` `#[salsa::input]` handles); the crate's `Expr2` AST (`src/ast/expr2.rs`), `BuiltinFn<Expr>` (`src/builtins.rs`), `Ident<Canonical>` (`src/common.rs`).

**Scope:** Phase 1 of 8 (GH #520). No dependencies; first phase.

**Codebase verified:** 2026-05-12 (codebase-investigator report; see "Codebase notes" at the end of this file for the verified line numbers and the design-assumption corrections).

---

## Acceptance Criteria Coverage

This phase implements and tests:

### ltm-arrays-hardening.AC1: #520 — unified classification IR (behavior-preserving)
- **ltm-arrays-hardening.AC1.1 Success:** `model_ltm_reference_sites` is a salsa-tracked function returning, per `(from, to)` causal edge, a `Vec<ClassifiedSite>` (shape + target_element + routing); `model_element_causal_edges` and `model_ltm_variables` both read it and neither contains its own `Expr2` AST walk for reference shape nor its own `routed_aggs` filter — the inline `route_through_agg = !routed_aggs.is_empty() && site.in_reducer` decision and the byte-identical `aggs_in_var(to).filter(...)` filter exist in exactly one place (the IR builder). Verified by the IR-driven `model_element_causal_edges` / `model_ltm_variables` passing the existing `db_element_graph_tests` and `db_ltm_*_tests` suites.
- **ltm-arrays-hardening.AC1.2 Success:** `builtin_is_array_reducer` no longer exists; the array-reducer set and its `Linear`/`Nonlinear`/`Constant` + `is_monotone` classification are defined in exactly one place (`reducer_kind` in `ltm_agg.rs`); `agg_reducer_is_monotone`, `ltm_augment::classify_reducer`, and `ltm_augment::is_array_reducer_name` are thin readers of it. Verified by a unit test exercising each `BuiltinFn` reducer variant (SUM, 1-arg MEAN, 2-arg MEAN, 1-arg MIN/MAX, 2-arg MIN/MAX, STDDEV, RANK, SIZE) through `reducer_kind`.
- **ltm-arrays-hardening.AC1.3 Success (behavior preserved):** every reducer-bearing, scalar, and pure-A2A golden LTM fixture (`logistic_growth_ltm`, `cross_element_ltm`, the WRLD3 LTM smoke, the partial-reduce model, and any others) produces byte-identical results before and after Phase 1; `cargo test --workspace` passes within the 3-minute cap.
- **ltm-arrays-hardening.AC1.4 Edge:** a reducer reference over a `StarRange` (`x[*..*]`) extent is classified consistently by the element graph and the link-score emitter (routed through the agg, with no separate Bare-named link score) — the latent `RefShape`-vs-`expr_is_full_extent` disagreement is gone. Verified by a unit test (no current golden fixture exercises this).
- **ltm-arrays-hardening.AC1.5 Edge:** a `SIZE` reducer reference, and a reducer over a scalar source, classify as `Direct` (never `ThroughAgg`) — `enumerate_agg_nodes` mints no agg for either, and the IR's routing reflects that.

---

## Codebase notes (verified 2026-05-12)

These are the verified facts the tasks below rely on. The implementor MUST re-confirm line numbers (they drift) but the structure is current as of the verification date.

- **Module layout.** `lib.rs` declares `pub mod ltm; pub mod ltm_agg; pub mod ltm_augment; pub mod ltm_finding; pub mod ltm_post;`. `db_analysis.rs` and `db_ltm.rs` are **submodules of `db.rs`** (declared `mod db_analysis;` / `mod db_ltm;` inside `src/simlin-engine/src/db.rs:15-39`, with selective `pub use` / `pub(crate) use` re-exports). `ltm/mod.rs` has submodules `graph`, `indexed`, `partitions`, `polarity`, `types`, `tests`. So the new IR module goes in as `mod db_ltm_ir;` in `db.rs`, alongside `db_analysis` / `db_ltm`.
- **Salsa pattern.** Tracked functions use `#[salsa::tracked]` (or `#[salsa::tracked(returns(ref))]` for large owned results). `Db` trait: `db.rs:54-55` (`#[salsa::db] pub trait Db: salsa::Database {}`). Signature shape for analysis functions: `pub fn foo(db: &dyn Db, model: SourceModel, project: SourceProject) -> Result`. `model` / `project` are `#[salsa::input]` handles (`SourceModel` `db.rs:210`, `SourceProject` `db.rs:186`), not `Arc<...>` / `ModelStage*`. Tests bridge via `let sync = crate::db::sync_from_datamodel(&db, &datamodel); let source_model = sync.models["main"].source; let source_project = sync.project;`.
- **The three AST walkers being unified:**
  1. `db_analysis.rs` — `collect_reference_sites(target_var, source_ident, source_is_arrayed, source_dims) -> Vec<ReferenceSite>` (private, `db_analysis.rs:268`) → `collect_in_expr(...)` (`db_analysis.rs:370`, recursive `Expr2` walk) → `classify_subscript_shape(indices, source_dims) -> RefShape` (`db_analysis.rs:541`) → `resolve_literal_index(idx, source_dims) -> Option<String>` (`db_analysis.rs:171`). `ReferenceSite { shape: RefShape, target_element: Option<String>, in_reducer: bool }` is at `db_analysis.rs:141` (`pub(crate)`). `collect_in_expr` sets `child_in_reducer = in_reducer || builtin_is_array_reducer(builtin)` at `db_analysis.rs:451`. `collect_reference_shapes(...)` (`db_analysis.rs:252`, `pub(crate)`, re-exported `db.rs:27`) is the dedup-to-`Vec<RefShape>` wrapper. `model_element_causal_edges` (`db_analysis.rs:1543`, `#[salsa::tracked(returns(ref))] -> ElementCausalEdgesResult`) calls `collect_reference_sites` per `(target, source)` pair, then for each `site` does `routed_aggs = agg_nodes.aggs_in_var(to_name).filter(|a| a.is_synthetic && a.source_vars.iter().any(|s| s == from_name)).collect()` (`db_analysis.rs:1716-1719`) and `route_through_agg = !routed_aggs.is_empty() && site.in_reducer` (`db_analysis.rs:1722`); `route_through_agg` → emit `from[..]->agg` + `agg->to[e]` per routed agg; else → `emit_edges_for_reference(from_name, to_name, from_dims, to_dims, &site.shape, site.target_element.as_deref(), element_edges)` (`db_analysis.rs:602`).
  2. `ltm_agg.rs` — `enumerate_agg_nodes` (`ltm_agg.rs:167`, `#[salsa::tracked(returns(ref))] -> AggNodesResult`) walks each variable's `Expr2` once; `AggNodesResult { aggs: Vec<AggNode>, synthetic_by_key: HashMap<String /*canonical reducer text*/, usize>, by_var: HashMap<String /*owner var*/, Vec<usize>> }` (`ltm_agg.rs:127`); `AggNode { name, equation_text, source_vars, result_dims, is_synthetic }` (`ltm_agg.rs:82`); methods `agg_for_key(&str) -> Option<&AggNode>` (`ltm_agg.rs:148`), `aggs_in_var(&str) -> impl Iterator<Item=&AggNode>` (`ltm_agg.rs:153`). **`enumerate_agg_nodes` stays as-is** (it remains the hoisting decider); the IR consults its result.
  3. `db_ltm.rs` — `model_ltm_variables` (`db_ltm.rs:2932`, `#[salsa::tracked(returns(ref))] -> super::LtmVariablesResult`) has a nested `enumerate_shapes(db, source_vars, from, to, model, project) -> Option<Vec<RefShape>>` (`db_ltm.rs:3666`) that calls `crate::db::collect_reference_shapes`; `emit_per_shape_link_scores` (`db_ltm.rs:3735`) uses it; `emit_link_scores_for_edge` (`db_ltm.rs:4089`) restates the `routed_aggs` filter at `db_ltm.rs:4101-4104` (byte-identical to `db_analysis.rs:1716-1719` modulo `to`/`to_name`, `from`/`from_name` locals). `emit_source_to_agg_link_scores` (`db_ltm.rs:3812`) and `emit_agg_to_target_link_scores` (`db_ltm.rs:3885`) emit the two halves; the agg auxes (`$⁚ltm⁚agg⁚{n}`) are emitted around `db_ltm.rs:3060-3080`.
- **`RefShape`** (`db_analysis.rs:92`, re-exported `crate::db::RefShape` at `db.rs:24`): `enum RefShape { Bare, FixedIndex(Vec<String>), Wildcard, DynamicIndex }` (derives `Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, salsa::Update`). `FixedIndex` carries the resolved canonical element names, one per subscript position. **Phase 1 keeps `RefShape` exactly as-is** (Phase 4 retires the `Wildcard` arms).
- **`emit_edges_for_reference` arms** (`db_analysis.rs:602`): scalar source → single `from` node → all `to` nodes; `Bare` → reduction (arrayed→scalar), diagonal (`target_element.is_some()` → `expand_same_element` ∩ legal `to` nodes), or `expand_same_element` partial-dimension broadcast; `FixedIndex(elems)` → one `from[elems...]` key → every `to` element node; `Wildcard | DynamicIndex` → full cross product (`cartesian_element_names(from)` × `to` nodes). Helpers `expand_same_element` (`db_analysis.rs:770`), `cartesian_element_names` (`db_analysis.rs:727`).
- **The five reducer-recognition sites** (the unified `reducer_kind` replaces/feeds all of them):
  1. `db_analysis.rs:344` `builtin_is_array_reducer(builtin: &BuiltinFn<Expr2>) -> bool` — `Sum | Stddev | Rank => true; Mean(args) => args.len()==1; Min(_,None) | Max(_,None) => true; _ => false` — **excludes SIZE**. **Delete it in Phase 1**; `collect_in_expr`'s `child_in_reducer` check becomes `reducer_kind(builtin)` is `Some(Linear | Nonlinear)`.
  2. `ltm_agg.rs:568` `reducer_source_vars(builtin, variables) -> Option<Vec<String>>` — its inner `is_reducer` match is the same set (excludes SIZE) plus the "≥1 arrayed source" requirement. Keep its source-extraction job; replace the recognition arm with `reducer_kind`.
  3. `ltm_agg.rs:713` `agg_reducer_is_monotone(equation_text: &str) -> bool` — `t.starts_with("sum(" | "mean(" | "min(" | "max(")`. Used only at `db_ltm.rs:2894` (`recover_agg_hop_polarities`). Becomes a thin reader of the consolidated monotone predicate.
  4. `ltm_augment.rs:158` `is_array_reducer_name(name: &str, arity: usize) -> bool` — `sum | stddev | size | rank` at any arity; `mean | min | max` at arity 1 — **includes SIZE**. Used at `ltm_augment.rs:367` inside `build_partial_equation_shaped`'s `Expr0` walk. Becomes a thin reader.
  5. `ltm_augment.rs:1832` `classify_builtin_if_references_source` (private, called via `classify_reducer` at `ltm_augment.rs:1767`, which returns `Option<(ReducerKind, &'static str /*uppercase name*/, bool /*is_bare*/)>`) — `Sum => Linear; Mean(any arity) => Linear; Min(_,None) => Nonlinear; Max(_,None) => Nonlinear; Stddev => Nonlinear; Rank => Nonlinear; Size => Constant` — **includes SIZE → Constant; multi-arg MEAN counts here (unlike sites 1/2/4)**. The recognition arm becomes `reducer_kind`.
  - **Design-assumption correction:** `agg_reducer_is_monotone` is in `ltm_agg.rs` only, NOT `ltm_augment.rs` (the design prose said both). `classify_reducer` returns a 3-tuple, not bare `ReducerKind`. The "five" sites are not uniform on SIZE / multi-arg MEAN; `reducer_kind` must pick canonical answers (recommended below).
  - `ReducerKind` currently lives in `ltm_augment.rs:1719` (`enum ReducerKind { Linear, Nonlinear, Constant }`, `pub(crate)`). Phase 1 **moves it to `ltm_agg.rs`** and `ltm_augment` re-exports (`pub(crate) use crate::ltm_agg::ReducerKind;`).
  - There is a sixth hand-enumerated reducer-set occurrence in `ltm/polarity.rs:349-392` (`analyze_link_polarity`'s builtin arms). It is a polarity concern, not a hoisting concern; **out of scope for Phase 1** (Phase 7 touches `analyze_link_polarity`). Do not consolidate it here.
- **Supporting types:** `Ident<Canonical>` — `common.rs:1041` (default `State = Canonical`; `Canonical` zero-sized marker at `common.rs:1033`). `Expr2` — `ast/expr2.rs:139` (`Const, Var(Ident<Canonical>,..), App(BuiltinFn<Expr2>,..), Subscript(Ident<Canonical>, Vec<IndexExpr2>,..), Op1, Op2, If`). `IndexExpr2` — `ast/expr2.rs:74` (`Wildcard(Loc), StarRange(CanonicalDimensionName, Loc), Range(Expr2,Expr2,Loc), DimPosition(u32,Loc), Expr(Expr2)`). `Ast<Expr>` — `ast/mod.rs:31` (`Scalar(Expr), ApplyToAll(Vec<Dimension>, Expr), Arrayed(Vec<Dimension>, HashMap<CanonicalElementName, Expr>, Option<Expr> /*EXCEPT default*/, bool /*has_except_default*/)`). `BuiltinFn<Expr>` — `builtins.rs:57` (generic; `Mean(Vec<Expr>)`, `Min(Box<Expr>, Option<Box<Expr>>)`, `Max(Box<Expr>, Option<Box<Expr>>)`, `Sum(Box<Expr>)`, `Stddev(Box<Expr>)`, `Rank(Box<Expr>, Box<Expr>)`, `Size(Box<Expr>)`, ...); `BuiltinFn::name(&self) -> &'static str` at `builtins.rs:115` (lowercase); `walk_builtin_expr` / `BuiltinContents` at `builtins.rs:434` / `builtins.rs:429` (`Min`/`Max` only yield their `Some(_)` 2nd arg to the callback). `crate::ltm_agg::AGG_NAME_PREFIX` const at `ltm_agg.rs:74`; `synthetic_agg_name` `ltm_agg.rs:77`; `is_synthetic_agg_name` `ltm_agg.rs:698`.
- **`StarRange` disagreement (the AC1.4 bug):** `classify_subscript_shape` does NOT match `IndexExpr2::StarRange(..)` (only `Wildcard(_)`), and `resolve_literal_index` returns `None` for `StarRange`, so `x[*..*]` classifies as `RefShape::DynamicIndex`. But `ltm_agg::expr_is_full_extent` (`ltm_agg.rs:643`) treats `Wildcard(_) | StarRange(_,_)` BOTH as full extent, so `enumerate_agg_nodes` hoists `SUM(x[*..*])`. Today the `route_through_agg` reroute papers over it (the site is `in_reducer`, routes to the agg, the `DynamicIndex` shape never reaches the cross-product fallback for that case). The unified IR picks **one** extent check: `IndexExpr2::Wildcard(_) | IndexExpr2::StarRange(_, _)` ⇒ this index is a "full extent" / reducer-style access. When ALL of a subscript's indices are full-extent → treat as `RefShape::Wildcard`; when SOME are full-extent and the rest literal → that is the *sliced reducer* case which today is `DynamicIndex` and Phase 4 will hoist — for Phase 1, classify it exactly as `classify_subscript_shape` does today (any `Wildcard(_)` index ⇒ `Wildcard`; else literal/`FixedIndex` or `DynamicIndex`) BUT additionally treat a subscript whose indices are *all* `Wildcard(_) | StarRange(_,_)` as `Wildcard` (this is the only behavior change, and it only affects whether an all-`StarRange` reducer reference gets a stray `DynamicIndex`-shape direct edge in addition to its agg routing — which, per AC1.4, it should not). Confirm no current test/fixture exercises a bare `x[*..*]` reducer reference before relying on byte-unchanged; if one does, it is the bug being fixed — document it (risk R1).
- **Tests that become Phase-1 regression guards (keep and pass; do NOT rewrite):**
  - `db_analysis.rs` inline module `collect_reference_sites_tests` (`db_analysis.rs:2520-2756`): `ref_site_bare_a2a`, `ref_site_fixed_index`, `ref_site_wildcard_reducer`, `ref_site_bare_arrayed_arg_is_in_reducer`, `ref_site_mixed_bare_and_wildcard`, `ref_site_reducer_and_direct_dynamic_index`, `ref_site_size_arg_is_not_in_reducer`, `ref_site_two_arg_min_is_not_a_reducer`, `ref_site_nested_reducer_arg_stays_in_reducer`. These pin the `(shape, in_reducer)` contract per AST site. **They keep using `collect_reference_sites`** (which becomes the IR builder's internal per-variable helper, or stays a thin wrapper) — i.e. preserve `collect_reference_sites` (or an equivalent `pub(crate)` helper) so these tests compile; OR port them to assert against `model_ltm_reference_sites` output. Pick the lower-churn option; either way they must assert the same facts.
  - `db_analysis.rs` inline module `emit_edges_for_reference_tests` (`db_analysis.rs:2758+`) — the `emit_edges_for_reference` truth table per `RefShape`. `emit_edges_for_reference` is unchanged in Phase 1.
  - `ltm_agg.rs` `mod tests` (`ltm_agg.rs:767+`): `reducer_over_scalar_source_is_not_hoisted` (`:1236`), `size_reducer_is_not_hoisted` (`:1263`), `slice_reducer_subexpression_is_not_hoisted` (`:1287`), `full_wildcard_reducer_subexpression_is_still_hoisted` (`:1312`), plus ~13 others. `enumerate_agg_nodes` behavior is unchanged → these stay green untouched.
  - `db_element_graph_tests.rs` (`src/simlin-engine/src/db_element_graph_tests.rs`, wired in via `db_analysis.rs:3291-3293`) — `model_element_causal_edges` truth tables incl. `scalar_model_produces_identical_element_graph`, `arrayed_to_scalar_via_sum`, `cross_element_loop_through_sum_reducer`, `element_graph_wildcard_reducer_plus_bare_truthful`, `element_graph_whole_rhs_scalar_reducer_is_its_own_agg_node`.
  - `db_element_graph_proptest.rs` (`#[cfg(test)] mod db_element_graph_proptest;` in `lib.rs:31-32`) — the variable↔element edge projection invariant; its header says it was written as a Day-1 guard for exactly this refactor. Must keep passing.
  - `db_ltm_tests.rs` (wired at `db_ltm.rs:4451`), `db_ltm_unified_tests.rs` (wired at `db.rs:5993`; incl. `per_shape_link_scores_for_share_with_sum`, `no_wildcard_or_dynamic_link_scores_for_reducer_models`, `agg_aux_emitted_for_hoisted_reducer`, `cross_element_loop_through_agg_is_recovered`), `db_ltm_module_tests.rs` (wired at `db.rs:5990`), `ltm_augment.rs` `mod tests` (incl. `classify_reducer` tests referencing `ReducerKind::Linear/Nonlinear/Constant`).
- **Testing conventions** (cite in tasks): `src/simlin-engine/CLAUDE.md` (Tests section — "keep this file up to date when adding/removing/reorganizing modules"), `docs/dev/rust.md#test-time-budgets` (under-2s-per-test target, 3-min `cargo test --workspace` cap; **no `--no-verify`**), `docs/dev/commands.md`. No snapshot framework (no `insta` / `expect-test` / `--bless`); golden trace = `test/logistic_growth_ltm/ltm_results.tsv` compared at 5% rel tolerance by `tests/simulate_ltm.rs::ensure_ltm_results`; everything else is structural assertions or hand-computed values in code; "byte-identical golden data" ⇒ all LTM tests pass with **no test-file edits**. `TestProject` builder lives in `src/simlin-engine/src/test_common.rs` (`pub mod`, usable from unit + integration tests; `compile_incremental` / `run_vm_incremental` / `assert_compiles_incremental`); `testutils.rs` has `x_aux` etc.; LTM compile helpers `compile_ltm_incremental` / `compile_ltm_incremental_with_partitions` / `compile_ltm_discovery_incremental` are in `tests/simulate_ltm.rs`. The pre-commit hook runs (Rust pipeline) `cargo fmt --check` → cbindgen-header freshness diff → `cargo clippy --all-targets --all-features -- -D warnings` ∥ `RUST_BACKTRACE=1 timeout --kill-after=30 180 cargo test` (plain `cargo test`, not `--workspace`, not `--features file_io`); CI runs `cargo test --workspace` under `timeout-minutes: 3`. Run the file_io-gated LTM integration tests yourself with `cargo test -p simlin-engine --features file_io --test simulate_ltm` when iterating.

---

## Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Consolidated reducer table — `reducer_kind` / `ReducerKind` in `ltm_agg.rs`

**Verifies:** ltm-arrays-hardening.AC1.2 (the `reducer_kind` single-table + variant unit test).

**Files:**
- Modify: `src/simlin-engine/src/ltm_agg.rs` (add the table; this file already owns the agg machinery — `enumerate_agg_nodes`, `reducer_source_vars`, `agg_reducer_is_monotone`, `AGG_NAME_PREFIX`).
- Modify: `src/simlin-engine/src/ltm_augment.rs` (remove the local `enum ReducerKind` at `ltm_augment.rs:1719`; add `pub(crate) use crate::ltm_agg::ReducerKind;` so existing references compile).
- Test: `src/simlin-engine/src/ltm_agg.rs` `#[cfg(test)] mod tests` (the existing block at `ltm_agg.rs:767+`).

**Implementation:**
1. Move `enum ReducerKind { Linear, Nonlinear, Constant }` from `ltm_augment.rs` into `ltm_agg.rs` as `pub(crate)` (keep the variant doc comments; keep `#[derive(Debug, Clone, PartialEq, Eq)]`). `ltm_augment.rs` adds `pub(crate) use crate::ltm_agg::ReducerKind;`.
2. Add the single recognition table to `ltm_agg.rs`:
   - `pub(crate) fn reducer_kind_from_name(name: &str, arity: usize) -> Option<ReducerKind>` — the canonical, lowercase-name + arity decider:
     - `"sum"` ⇒ `Some(Linear)`
     - `"mean"` and `arity == 1` ⇒ `Some(Linear)` (multi-arg MEAN is a scalar mean-of-arguments, not a reducer ⇒ `None`)
     - `"min" | "max"` and `arity == 1` ⇒ `Some(Nonlinear)` (2-arg MIN/MAX is scalar ⇒ `None`)
     - `"stddev"` ⇒ `Some(Nonlinear)`
     - `"rank"` ⇒ `Some(Nonlinear)` (arity-insensitive — `Rank(arr, dir)` is a reducer)
     - `"size"` ⇒ `Some(Constant)`
     - `_` ⇒ `None`
   - `pub(crate) fn reducer_kind<E>(builtin: &BuiltinFn<E>) -> Option<ReducerKind>` — computes `(builtin.name(), arity)` and delegates. Arity: `Mean(args) => args.len()`; `Min(_, opt) | Max(_, opt) => 1 + opt.is_some() as usize`; everything else `reducer_kind_from_name` ignores arity for ⇒ pass any sane value (e.g. `1`). Generic over `E` because it only inspects structure/arity, not the contained expressions — this lets `collect_in_expr` (`BuiltinFn<Expr2>`) and `classify_reducer` (`BuiltinFn<Expr2>`) and any future `BuiltinFn<Expr0>` caller share it.
   - `pub(crate) fn reducer_name_is_monotone(name: &str) -> bool` — the single monotone predicate: `matches!(name, "sum" | "mean" | "min" | "max")` (the algebraically-monotone reducers; STDDEV/RANK/SIZE are not). `pub(crate) fn reducer_is_monotone<E>(builtin: &BuiltinFn<E>) -> bool` — `reducer_kind(builtin).is_some() && reducer_name_is_monotone(builtin.name())`. **Implement it this way — the name-keyed predicate next to `reducer_kind` — and do NOT split `ReducerKind` finer.** Add a one-line `//` comment on `reducer_name_is_monotone` explaining why the 3-variant `ReducerKind` doesn't carry `is_monotone` directly: "`Nonlinear` lumps the monotone 1-arg MIN/MAX with the non-monotone STDDEV/RANK, so monotonicity is keyed on the reducer name, not the kind — but it lives here next to `reducer_kind`, so AC1.2's 'the array-reducer set and its `Linear`/`Nonlinear`/`Constant` + `is_monotone` classification are defined in exactly one place' holds." (Splitting `ReducerKind` into e.g. `Linear` / `MonotoneNonlinear` / `Nonlinear` / `Constant` would also work but ripples through every `ReducerKind` match in `ltm_augment`/`db_ltm` for no real gain — the AC's "Linear or 1-arg MIN/MAX" comment describes exactly the name-keyed predicate.)
   - `pub(crate) fn reducer_is_hoistable<E>(builtin: &BuiltinFn<E>) -> bool` — `matches!(reducer_kind(builtin), Some(ReducerKind::Linear | ReducerKind::Nonlinear))` (i.e. recognized AND not `Constant`; SIZE is recognized but never hoisted and never sets `in_reducer`). This is the exact replacement for `builtin_is_array_reducer` and for `reducer_source_vars`'s `is_reducer` arm.
3. Keep `synthetic_agg_name` / `is_synthetic_agg_name` / `AGG_NAME_PREFIX` unchanged.

**Testing:** Add a unit test in `ltm_agg.rs`'s `mod tests` that exercises `reducer_kind` (via `BuiltinFn<Expr2>` literals — or `BuiltinFn::<i32>` if simpler, since `reducer_kind` is generic) for every variant the AC names and asserts the result:
- SUM ⇒ `Some(Linear)`; 1-arg MEAN ⇒ `Some(Linear)`; 2-arg MEAN ⇒ `None`; 1-arg MIN ⇒ `Some(Nonlinear)`; 2-arg MIN ⇒ `None`; 1-arg MAX ⇒ `Some(Nonlinear)`; 2-arg MAX ⇒ `None`; STDDEV ⇒ `Some(Nonlinear)`; RANK ⇒ `Some(Nonlinear)`; SIZE ⇒ `Some(Constant)`; a non-reducer (e.g. `Abs`) ⇒ `None`.
- Also assert `reducer_is_monotone` / `reducer_name_is_monotone`: true for sum/mean(1-arg)/min(1-arg)/max(1-arg); false for stddev/rank/size; and `reducer_is_hoistable`: true for sum/mean(1-arg)/min(1-arg)/max(1-arg)/stddev/rank; false for size and 2-arg min/max.

**Verification:**
- `cargo test -p simlin-engine ltm_agg::tests::` — the new test passes; the existing `ltm_agg` tests still pass.
- `cargo build -p simlin-engine` — `ltm_augment.rs`'s `ReducerKind` re-export compiles.

**Commit:** `engine: add consolidated reducer_kind table to ltm_agg`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Rewire the five reducer-recognition sites onto `reducer_kind`

**Verifies:** ltm-arrays-hardening.AC1.2 (the existing reducer/agg/partial tests stay green through the consolidated table — `builtin_is_array_reducer` gone, all readers thin).

**Files:**
- Modify: `src/simlin-engine/src/db_analysis.rs` — delete `builtin_is_array_reducer` (`db_analysis.rs:344`); in `collect_in_expr` (`db_analysis.rs:451`) change `child_in_reducer = in_reducer || builtin_is_array_reducer(builtin)` to `child_in_reducer = in_reducer || crate::ltm_agg::reducer_is_hoistable(builtin)`. (Note: this walker is moving to `db_ltm_ir.rs` in Task 3 — making this swap here first keeps Task 2 self-contained and behavior-preserving; Task 3 carries the swapped version along.)
- Modify: `src/simlin-engine/src/ltm_agg.rs` — in `reducer_source_vars` (`ltm_agg.rs:568`) replace the inline `is_reducer` match with `reducer_is_hoistable(builtin)` (keep the "≥1 arrayed source variable among the args" requirement and the source-extraction logic); rewrite `agg_reducer_is_monotone(equation_text)` (`ltm_agg.rs:713`) to extract the leading lowercase function name from `equation_text` (everything before the first `(`, trimmed) and return `reducer_name_is_monotone(that_name)` (this keeps it a `&str`-taking helper for `recover_agg_hop_polarities` but sources the monotone set from the one place; the prior behavior — `starts_with("sum(" | "mean(" | "min(" | "max(")` — is preserved exactly).
- Modify: `src/simlin-engine/src/ltm_augment.rs` — `is_array_reducer_name(name, arity)` (`ltm_augment.rs:158`) becomes `crate::ltm_agg::reducer_kind_from_name(name, arity).is_some()` (this preserves "includes SIZE; mean/min/max only at arity 1; sum/stddev/rank/size any arity"); `classify_builtin_if_references_source` (`ltm_augment.rs:1832`, the recognition arm inside `classify_reducer`'s tree) becomes: match the builtin → if it references the source and `reducer_kind(builtin)` is `Some(kind)`, return `Some((kind, builtin.name().to_ascii_uppercase_static_or_map(), is_bare))` — i.e., replace the hand-rolled per-builtin match with `reducer_kind` + the existing source-reference check, deriving the uppercase name from `builtin.name()` (map lowercase → the existing `&'static str` uppercase literals; `classify_reducer` still returns `Option<(ReducerKind, &'static str, bool)>`). Keep `expr_references_var` / the `is_bare` derivation unchanged.

**Implementation notes:**
- This task is pure mechanical rewiring. The behavior of every site is preserved exactly (the canonical answers in `reducer_kind` were chosen to match: SIZE excluded from `reducer_is_hoistable` ⇒ `builtin_is_array_reducer`/`reducer_source_vars` behavior unchanged; SIZE included in `reducer_kind_from_name(...).is_some()` ⇒ `is_array_reducer_name` behavior unchanged; SIZE ⇒ `Constant` ⇒ `classify_reducer` behavior unchanged; multi-arg MEAN is `None` in `reducer_kind` but `classify_builtin_if_references_source` previously matched `Mean(any arity)` — **double-check this one**: the design's `reducer_kind` says multi-arg MEAN ⇒ `None`, so `classify_reducer` would now return `None` for `MEAN(a, b)` whereas before it returned `Some((Linear, "MEAN", ..))`. Is `classify_reducer` ever called with a multi-arg `MEAN` referencing the source? Inspect `classify_reducer`'s call sites (`db_ltm.rs:3812` `emit_source_to_agg_link_scores`, possibly others). A multi-arg `MEAN(a, b)` is `(a+b)/2` — a scalar function, not an array reduce; the agg machinery only ever feeds `classify_reducer` an agg whose equation is a *recognized array reducer* (`enumerate_agg_nodes` only hoists `reducer_is_hoistable` subexpressions, which excludes multi-arg MEAN), so in practice `classify_reducer` is never reached with a multi-arg MEAN-over-source. **Confirm this** by reading the call sites; if confirmed, the `None` answer is fine and behavior-preserving. If `classify_reducer` is somehow reached for a non-hoisted reducer-name with multi-arg, add a `#[cfg(test)]`-pinned note and keep the old multi-arg-MEAN-counts behavior for that one call by special-casing in `classify_builtin_if_references_source` — but the expected outcome is "not reached", documented in a code comment.)

**Testing:** No new tests. Run the existing suites:
- `cargo test -p simlin-engine` — `ltm_agg` tests (incl. the new Task-1 test), `ltm_augment` tests (incl. `classify_reducer` tests), `db_analysis` tests, `db_ltm_tests`, `db_ltm_unified_tests`, `db_ltm_module_tests`, `db_element_graph_tests`, `db_element_graph_proptest` — all green.

**Verification:**
- `cargo clippy -p simlin-engine --all-targets --all-features -- -D warnings` — no warnings (no dead `builtin_is_array_reducer`, no unused imports).
- `cargo test -p simlin-engine` — green.

**Commit:** `engine: route reducer recognition through reducer_kind; drop builtin_is_array_reducer`

<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-5) -->
<!-- START_TASK_3 -->
### Task 3: New module `db_ltm_ir.rs` — `model_ltm_reference_sites` and the `ClassifiedSite` IR

**Verifies:** ltm-arrays-hardening.AC1.1 (the IR exists, salsa-tracked, per-`(from,to)` `Vec<ClassifiedSite>`), ltm-arrays-hardening.AC1.4 (consistent `StarRange` classification), ltm-arrays-hardening.AC1.5 (SIZE / scalar-source reducer ⇒ `Direct`).

**Files:**
- Create: `src/simlin-engine/src/db_ltm_ir.rs`.
- Modify: `src/simlin-engine/src/db.rs` — add `mod db_ltm_ir;` near the `mod db_analysis;` / `mod db_ltm;` lines (`db.rs:15-39`); add `pub(crate) use db_ltm_ir::{model_ltm_reference_sites, ClassifiedSite, SiteRouting, AggRef, LtmReferenceSitesResult};` (or whatever subset later tasks need).
- Modify: `src/simlin-engine/src/db_analysis.rs` — move `collect_reference_sites`, `collect_in_expr`, `classify_subscript_shape`, `resolve_literal_index` (and the `ReferenceSite` struct) into `db_ltm_ir.rs` as internal helpers (or leave thin `pub(crate)` shims in `db_analysis.rs` that delegate, *only* if the `ref_site_*` tests are easier kept there — prefer the move + porting the tests). `RefShape` and `emit_edges_for_reference` and `expand_same_element` / `cartesian_element_names` STAY in `db_analysis.rs` (the IR imports `RefShape` via `crate::db::RefShape`).
- Modify: `src/simlin-engine/src/CLAUDE.md` — add `db_ltm_ir.rs` to the module list in the Tests/modules section (per the file's own "keep up to date" note).
- Test: `src/simlin-engine/src/db_ltm_ir_tests.rs` (new file; wire in via `#[cfg(test)] #[path = "db_ltm_ir_tests.rs"] mod db_ltm_ir_tests;` at the bottom of `db_ltm_ir.rs`) — OR put the tests in an inline `#[cfg(test)] mod tests` if small. Also: keep/port the `ref_site_*` assertions (see Codebase notes).

**Implementation — the IR contract:**
```rust
// src/simlin-engine/src/db_ltm_ir.rs

/// One classified reference site for a `(from, to)` causal edge.
/// Successor of `db_analysis::ReferenceSite`, generalized to carry agg routing.
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub(crate) struct ClassifiedSite {
    pub shape: crate::db::RefShape,                 // Bare / FixedIndex(elems) / Wildcard / DynamicIndex
    pub target_element: Option<String>,             // Some(elem) when the ref sits in an `Ast::Arrayed` per-element slot
    pub routing: SiteRouting,
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub(crate) enum SiteRouting {
    Direct,                                         // consumers use `shape` / `target_element` directly
    ThroughAgg { agg: AggRef },                     // consumers route `from[..]->agg` + `agg->to[e]`; `shape` is the (Wildcard-ish) syntactic shape but is ignored
}

/// Index into `AggNodesResult.aggs`. The IR records the *synthetic* agg a
/// `ThroughAgg` site routes through; deduped across sites in a consumer when needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub(crate) struct AggRef(pub usize);

#[derive(Debug, Clone, Default, PartialEq, Eq, salsa::Update)]
pub(crate) struct LtmReferenceSitesResult {
    /// Every `(from-var, to-var)` causal edge with ≥1 AST reference, in canonical order.
    /// Keys use the same string identity as the element/causal-edge maps (canonical var names).
    pub sites: std::collections::HashMap<(String, String), Vec<ClassifiedSite>>,
}

#[salsa::tracked(returns(ref))]
pub(crate) fn model_ltm_reference_sites(
    db: &dyn crate::db::Db,
    model: crate::db::SourceModel,
    project: crate::db::SourceProject,
) -> LtmReferenceSitesResult {
    // 1. agg_nodes = crate::ltm_agg::enumerate_agg_nodes(db, model, project)  // the sole hoisting decider
    // 2. variables = crate::db::reconstruct_model_variables(db, model, project)  // lowered Variable map (the existing helper, re-exported at db.rs:27)
    // 3. for each `to` variable, in canonical-sorted order:
    //      walk its `Ast<Expr2>` (both the dt `ast` and — matching the existing walker — only the dt ast; init is not a causal edge for LTM purposes; confirm against `collect_reference_sites` which handles `Ast::Scalar`/`ApplyToAll`/`Arrayed` of the dt eqn + the EXCEPT default expr) once:
    //        - track `in_reducer_depth` and, when inside a reducer subexpression, the *canonical text* of the *maximal* reducer subexpression we are inside (so we can map a reference to the agg `enumerate_agg_nodes` minted for it: `agg_nodes.synthetic_by_key.get(&canonical_text)` -> Some(agg_idx) means `ThroughAgg{AggRef(agg_idx)}`; None — e.g. SIZE, or a reducer over only scalar sources, or a non-hoisted sliced reducer in pre-Phase-4 code — means `Direct`).
    //          Use `crate::ltm_agg::reducer_is_hoistable(builtin)` to decide whether descending into this builtin opens (or extends) a reducer subexpression — exactly the `child_in_reducer` rule. The "maximal reducer subexpression" is the outermost hoistable-reducer App in a chain (matching how `enumerate_agg_nodes` keys its synthetic_by_key); a nested inner reducer stays under the outer one's key.
    //        - for `Expr2::Var(ident, ..)` matching some source `from`: push `ClassifiedSite { shape: Bare, target_element, routing: <ThroughAgg if currently inside a hoisted reducer subexpr whose key is in synthetic_by_key, else Direct> }` into `sites[(from, to)]`.
    //        - for `Expr2::Subscript(ident, indices, ..)` matching some source `from`: `shape = classify_subscript_shape(indices, from_dims)` (the moved helper, with the AC1.4 fix: any `IndexExpr2::Wildcard(_) | IndexExpr2::StarRange(_, _)` index makes it `Wildcard` if *all* indices are full-extent; otherwise the existing literal/`FixedIndex`/`DynamicIndex` logic; `resolve_literal_index` unchanged so a dimension-name subscript still ⇒ `DynamicIndex` — Phase 3 changes that). `routing` as above.
    //        - `Ast::Arrayed(_, per_elem_map, except_default, _)`: walk each per-element expr with `target_element = Some(that_elem)`, and the `except_default` expr with `target_element = None` (matching `collect_reference_sites`).
    //      Determine `from_dims` / `from_is_arrayed` from the `variables` map (the existing walker passes `source_is_arrayed`/`source_dims` in; here we look them up per source as we encounter references).
    // 4. return LtmReferenceSitesResult { sites }.
}
```
- **Determinism (salsa requirement):** iterate variables in canonical-sorted order and do a left-to-right DFS over each AST, exactly like `enumerate_agg_nodes` and the current `model_element_causal_edges` — the `sites` HashMap values must be in a stable order. (The keys are a HashMap so iteration order there doesn't matter as long as consumers don't rely on it; if a consumer needs sorted edges it sorts keys itself, as today.)
- **`StarRange` (AC1.4 / R1):** the moved `classify_subscript_shape` adds: if every index is `IndexExpr2::Wildcard(_) | IndexExpr2::StarRange(_, _)` ⇒ `RefShape::Wildcard`. (The all-`Wildcard(_)` case already returned `Wildcard`; this adds the all-`StarRange` and mixed-`Wildcard`/`StarRange` cases. A *partially* literal subscript with one `StarRange` index stays `DynamicIndex` for now — Phase 4 hoists it.) Before relying on byte-unchanged, grep `test/` and the test files for `[*..*]` / `*:` reducer references; if any exist, that test's expected output may legitimately change — investigate and document.
- **SIZE / scalar-source reducer (AC1.5):** because `enumerate_agg_nodes` mints no agg for `SIZE(...)` or a reducer over only scalar sources, `synthetic_by_key` has no entry for those subexpressions ⇒ the IR records `routing = Direct` for references inside them. Also: a `SIZE` arg should NOT contribute to `in_reducer_depth` (it is not `reducer_is_hoistable`), matching `ref_site_size_arg_is_not_in_reducer`.
- Keep `model`/`project` types and the `#[salsa::tracked(returns(ref))]` shape identical to `model_edge_shapes` / `model_element_causal_edges`.

**Testing:** New unit tests in `db_ltm_ir_tests.rs` (or inline), using `TestProject` + `sync_from_datamodel` (the established pattern; see the `collect_reference_sites_tests` helper for a template):
- Port the `ref_site_*` assertions onto `model_ltm_reference_sites`: for each fixture (`births[Region] = population * 0.1`; `relative_pop[Region] = population / population[NYC]`; `total = SUM(population[*])`; `total = SUM(pop)` with `pop` arrayed; `share[Region] = population / SUM(population[*])`; `x = SUM(pop[*]) + pop[idx]`; `n = SIZE(pop[*])`; `floor[Region] = MIN(pop, 50) + MIN(pop[*])`; `grand_total = SUM(SUM(matrix[*, *]))`), assert the `(shape, target_element, routing)` of each site is what the old `(shape, target_element, in_reducer)` mapped to: `in_reducer && agg minted` ⇒ `ThroughAgg`; else `Direct` with the same shape. (Equivalently: keep the original `collect_reference_sites_tests` module if `collect_reference_sites` survives as the IR's internal helper, AND add one new test asserting the `routing` annotation lines up with `enumerate_agg_nodes`.)
- AC1.4: a model with `total = SUM(x[*..*])` (`x` arrayed over one dim): `model_ltm_reference_sites` site for `(x, total)` has `routing == ThroughAgg{..}` and (importantly) there is **no** additional `Direct`/`DynamicIndex` site for `(x, total)` — and the element graph (after Task 4) has only `x[*] -> agg -> total`, not a stray `x[d] -> total` cross-product. Assert it both at the IR level and (after Task 4) via `model_element_causal_edges`.
- AC1.5: `n = SIZE(pop[*])` — the `(pop, n)` site has `routing == Direct` (with `shape == Wildcard`); `total = SUM(s)` where `s` is scalar — the `(s, total)` site has `routing == Direct`, `shape == Bare`. Cross-check `enumerate_agg_nodes(...)` mints zero aggs for both (already covered by `size_reducer_is_not_hoisted` / `reducer_over_scalar_source_is_not_hoisted`, but assert the IR routing too).

**Verification:**
- `cargo test -p simlin-engine db_ltm_ir` — new tests pass.
- `cargo test -p simlin-engine` — everything still green (nothing yet *consumes* the IR; this task just adds it and moves the helpers).
- `cargo clippy -p simlin-engine --all-targets --all-features -- -D warnings`.

**Commit:** `engine: add model_ltm_reference_sites classification IR (db_ltm_ir)`

<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: `model_element_causal_edges` and `model_edge_shapes` become readers of the IR

**Verifies:** ltm-arrays-hardening.AC1.1 (element-graph side: no inline AST walk, no `routed_aggs` filter), ltm-arrays-hardening.AC1.3 (element-graph behavior byte-unchanged), ltm-arrays-hardening.AC1.4 (element-graph side of the `StarRange` consistency).

**Files:**
- Modify: `src/simlin-engine/src/db_analysis.rs`:
  - `model_element_causal_edges` (`db_analysis.rs:1543`): replace the per-`(target, source)` `collect_reference_sites` call + the inline `routed_aggs` filter (`:1716-1719`) + `route_through_agg` (`:1722`) with: `let ir = crate::db::model_ltm_reference_sites(db, model, project);` then `for ((from, to), classified_sites) in &ir.sites { for site in classified_sites { match &site.routing { SiteRouting::Direct => emit_edges_for_reference(from, to, from_dims, to_dims, &site.shape, site.target_element.as_deref(), &mut element_edges), SiteRouting::ThroughAgg { agg } => { /* emit from[..]->agg.name + agg.name->to[e], deduped: a BTreeSet<String> per node already dedups, and if you want to avoid re-emitting the same agg's edges across multiple ThroughAgg sites for the same (from,to), collect the unique AggRef set per (from,to) first */ } } } }` — exactly the prior two branches, now driven by `site.routing` instead of recomputing. Keep the structural flow→stock and module edges exactly as today (those are not AST references and the IR doesn't cover them — emit them the same way `model_element_causal_edges` does now). Look up `from_dims`/`to_dims` from `reconstruct_model_variables` the same way the current code does.
  - `model_edge_shapes` (`db_analysis.rs:1308`): replace its `collect_reference_shapes` call with a projection of the IR: `for ((from, to), sites) in &ir.sites { edge_shapes.entry((from.clone(), to.clone())).or_default().extend(sites.iter().map(|s| s.shape.clone())); }` then add the structural-edge `{Bare}` entries exactly as today. (If `model_edge_shapes` has no other behavior, this is the whole body. `classify_cycle` consumes its output unchanged.)
  - Delete the now-unused `collect_reference_shapes` if no callers remain (check `db_ltm.rs`'s `enumerate_shapes` — that goes away in Task 5; check `db.rs:27`'s re-export — drop it too if dead). If something still uses it, leave it as a thin IR-projection helper. (Phase 8 cleanup will catch stragglers, but prefer removing dead code now.)
  - `builtin_is_array_reducer` is already deleted (Task 2).
- Test: `src/simlin-engine/src/db_element_graph_tests.rs`, `db_element_graph_proptest.rs`, `db_analysis.rs` inline test modules — no edits expected; they must keep passing. (If `collect_reference_sites_tests` referenced a now-moved symbol, fix the `use` path only — no assertion changes.)

**Implementation notes:**
- This is the load-bearing behavior-preservation step on the element-graph side. The mapping is 1:1: `Direct` ↔ the old "not routed" branch (`emit_edges_for_reference` with the site's shape); `ThroughAgg` ↔ the old "routed" branch (`from->agg` + `agg->to[e]`). The only behavioral delta is AC1.4's `StarRange` fix, which strictly *removes* a stray cross-product edge that the old reroute already neutralized — so on existing fixtures, byte-unchanged. If `db_element_graph_proptest` or any `db_element_graph_tests` test fails, that is a real discrepancy — debug it (do not adjust the test); the most likely cause is a missed structural-edge case or an ordering difference, both fixable in the IR builder or the reader.

**Testing:** Run the element-graph suites:
- `cargo test -p simlin-engine db_element_graph` — `db_element_graph_tests` + `db_element_graph_proptest` green.
- `cargo test -p simlin-engine db_analysis` — the inline `collect_reference_sites_tests` / `emit_edges_for_reference_tests` / `edge_shapes_tests` / `classify_cycle_tests` / `tiered_circuits_tests` green.

**Verification:**
- `cargo test -p simlin-engine` — full engine suite green.
- `cargo clippy -p simlin-engine --all-targets --all-features -- -D warnings` — no dead `collect_reference_sites` / `collect_in_expr` / `routed_aggs` leftovers in `db_analysis.rs`.

**Commit:** `engine: drive element causal graph + edge shapes from model_ltm_reference_sites`

<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: `model_ltm_variables` link-score emission becomes a reader of the IR

**Verifies:** ltm-arrays-hardening.AC1.1 (link-score side: no inline AST walk, no `routed_aggs` filter), ltm-arrays-hardening.AC1.3 (LTM-variable behavior byte-unchanged), ltm-arrays-hardening.AC1.4 (link-score side of the `StarRange` consistency — no separate Bare-named link score for an all-`StarRange` reducer ref).

**Files:**
- Modify: `src/simlin-engine/src/db_ltm.rs`:
  - In `model_ltm_variables` (`db_ltm.rs:2932`) / `emit_link_scores_for_edge` (`db_ltm.rs:4089`) / `emit_per_shape_link_scores` (`db_ltm.rs:3735`): obtain `let ir = crate::db::model_ltm_reference_sites(db, model, project);` once. For each `(from, to)` edge being scored, partition `ir.sites[&(from, to)]` into the **direct shape set** (`{site.shape.clone() for site in sites if site.routing == Direct}`, deduped — this replaces `enumerate_shapes` / `collect_reference_shapes`) and the **routed agg set** (`{agg for ThroughAgg{agg} in sites}`, deduped → `Vec<&AggNode>` via `agg_nodes.aggs[agg.0]`, replacing the `routed_aggs` filter at `db_ltm.rs:4101-4104`). Then: if the routed agg set is non-empty → for each agg call `emit_source_to_agg_link_scores(from, agg, ..)` + `emit_agg_to_target_link_scores(agg, to, ..)` (subject to `skip_agg_halves` exactly as today) and set `skip_reducer_shapes = true`; always call `emit_per_shape_link_scores` over the *direct shape set* (skipping `Wildcard` when `skip_reducer_shapes`, exactly as today). Delete the nested `enumerate_shapes` fn (`db_ltm.rs:3666`) and the `routed_aggs` filter (`db_ltm.rs:4101-4104`).
  - `emit_source_to_agg_link_scores` / `emit_agg_to_target_link_scores` / the agg-aux emission / `recover_cross_agg_loops` / `try_cross_dimensional_link_scores` — unchanged in Phase 1.
- Test: `src/simlin-engine/src/db_ltm_tests.rs`, `db_ltm_unified_tests.rs`, `db_ltm_module_tests.rs`, `ltm_augment.rs` `mod tests` — no edits expected; must keep passing (incl. `per_shape_link_scores_for_share_with_sum`, `no_wildcard_or_dynamic_link_scores_for_reducer_models`, `agg_aux_emitted_for_hoisted_reducer`, `cross_element_loop_through_agg_is_recovered`, `loop_score_picks_emitted_shape_when_only_wildcard_exists`, `scalar_reducer_loop_score_uses_per_element_link_scores`).

**Implementation notes:**
- The mapping is again 1:1: the direct shape set = what `enumerate_shapes` produced (a deduped `Vec<RefShape>` from `collect_reference_shapes`, which projects `collect_reference_sites`'s `.shape`) **minus** any `Wildcard` shapes that belonged to a reference that's now `ThroughAgg` — and that's exactly the `skip_reducer_shapes` behavior. Since `route_through_agg` was `in_reducer && routed_aggs nonempty`, and a `Wildcard`-shaped reference is `in_reducer` (it's a `[*]` access inside a reducer) ⇒ such a reference is `ThroughAgg` in the IR ⇒ it's in the routed-agg set, not the direct shape set ⇒ `emit_per_shape_link_scores` doesn't see it ⇒ same as today's `skip_reducer_shapes`. A `Bare`-shaped reference that's *also* `in_reducer` (e.g. `SUM(pop)` where `pop` is arrayed) — that one's site is `ThroughAgg` (the agg `sum(pop)` was minted) ⇒ it's in the routed-agg set, not the direct shape set. Today: its shape `Bare` is in `enumerate_shapes`'s output, but `route_through_agg` was true so the link-score loop took the agg branch and `emit_per_shape_link_scores` with `skip_reducer_shapes=true` — which only skips `Wildcard`, NOT `Bare` — so today a `Bare` link score `from→to` *is also* emitted alongside the agg halves? **Check this carefully against `no_wildcard_or_dynamic_link_scores_for_reducer_models` and `agg_aux_emitted_for_hoisted_reducer`** — if today's code emits both the `Bare` per-shape link score and the agg halves for `SUM(pop_arrayed)`, then the IR must keep that `Bare` site in the direct shape set even though it's `ThroughAgg` for the *element graph*. This is the one spot where "element-graph routing" and "link-score routing" might not be identical: the element graph routes a `Bare`-in-reducer ref through the agg (no `pop[d]->to[e]` diagonal), but the link scorer might still emit a `Bare` `pop→to` link score. **Resolve by reading the current `emit_link_scores_for_edge` + `emit_per_shape_link_scores` carefully**: if `skip_reducer_shapes` drops only `Wildcard`, then `Bare`-in-reducer refs DO get a per-shape link score today, and the IR must surface them. Concretely: `emit_per_shape_link_scores` should be fed `{site.shape for sites where site.routing == Direct OR (site.routing == ThroughAgg AND site.shape != Wildcard)}` — i.e. drop `Wildcard` shapes (always — they're the reducer accesses) but keep `Bare`/`FixedIndex` even from `ThroughAgg` sites. Equivalently, mirror the *exact* current predicate. Get this right; it's the subtle part. (The cleanest framing: `emit_per_shape_link_scores` filters out `Wildcard` shapes when there are routed aggs; otherwise it emits every distinct shape from every site regardless of routing — which is exactly `collect_reference_shapes`'s output minus `Wildcard`. So: direct shape set fed to `emit_per_shape_link_scores` = `{s.shape for all sites}` deduped, with `Wildcard` removed iff routed-agg set is non-empty. That's the literal translation of today's behavior and makes the element-graph/link-score "agreement" be: same `routing` data, but the link scorer additionally keeps non-`Wildcard` shapes from `ThroughAgg` sites. Document this in a code comment in the IR consumer.)
- If a test fails, debug the discrepancy — do not edit the test.

**Testing:** Run the LTM-variable suites:
- `cargo test -p simlin-engine db_ltm` — `db_ltm_tests`, `db_ltm_unified_tests`, `db_ltm_module_tests` green.
- `cargo test -p simlin-engine ltm_augment` — `mod tests` green.

**Verification:**
- `cargo test -p simlin-engine` — full engine suite green.
- `cargo clippy -p simlin-engine --all-targets --all-features -- -D warnings` — no dead `enumerate_shapes` / `routed_aggs` / `collect_reference_shapes` leftovers in `db_ltm.rs`.

**Commit:** `engine: drive LTM link-score emission from model_ltm_reference_sites`

<!-- END_TASK_5 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_6 -->
### Task 6: Phase-1 verification — byte-unchanged golden data, full workspace test, dead-code sweep

**Verifies:** ltm-arrays-hardening.AC1.3 (golden LTM fixtures byte-unchanged; `cargo test --workspace` within the 3-minute cap), and re-confirms AC1.1 / AC1.2 / AC1.4 / AC1.5 hold together.

**Files:** none changed in this task except possibly small clippy fixes / a CLAUDE.md touch-up.

**Implementation / verification steps:**
1. **No-edit golden check.** Confirm the only checked-in LTM golden file `test/logistic_growth_ltm/ltm_results.tsv` and every LTM test file is unmodified by Phases-1 commits (`git diff --stat main..HEAD -- test/ '*_tests.rs' tests/simulate_ltm.rs` shows no changes to expected-output data or test assertions — only the moved `ref_site_*` module's `use` paths, if any). If any test assertion *was* changed, that contradicts "behavior-preserving" — STOP, re-examine the relevant Task, and only proceed if the change is the documented AC1.4 `StarRange` fix (in which case record the per-test reasoning here and in the commit; risk R1).
2. **LTM integration tests with the data feature:** `cargo test -p simlin-engine --features file_io --test simulate_ltm` — all pass (incl. `simulates_population_ltm` against the `ltm_results.tsv` golden at 5% tol; `test_cross_element_ltm_*`; `test_partial_reduce_cross_element_loop`). `cargo test -p simlin-engine --features file_io --test simulate` — unaffected, still pass. `cargo test -p simlin-engine --test wrld3_ltm_panic` — passes (the "WRLD3 LTM smoke").
3. **Full workspace + timing:** `time cargo test --workspace` — green, completes under 3 minutes wall-clock (matching the pre-commit/CI cap). If it regressed past the cap, the IR added cost somewhere — profile (`cargo test --workspace 2>&1 | grep 'finished in'`) and reduce; the IR's once-per-variable walk should be *cheaper* than the prior per-edge re-walks, so a regression means a salsa-caching mistake (e.g. the IR result struct not implementing `salsa::Update` cheaply, or a consumer re-deriving instead of reading the tracked result).
4. **Dead-code sweep:** `cargo clippy --workspace --all-targets --all-features -- -D warnings` — clean. Specifically confirm gone: `builtin_is_array_reducer`, the inline `route_through_agg` / `routed_aggs` in `db_analysis.rs`, the nested `enumerate_shapes` and the `routed_aggs` filter in `db_ltm.rs`, the `ltm_augment::ReducerKind` local enum (now a re-export), `collect_reference_shapes` if unused. Confirm `enumerate_agg_nodes` / `synthetic_agg_name` / `is_synthetic_agg_name` / `RefShape` / `emit_edges_for_reference` are unchanged.
5. **Commit + run the hook.** `git add -A && git commit -m "engine: verify Phase 1 reference-site IR is behavior-preserving"` — the pre-commit hook runs `cargo fmt --check`, the cbindgen-header diff, `cargo clippy --all-targets --all-features -- -D warnings`, and `timeout 180 cargo test`; fix anything it reports and re-commit. (No `--no-verify`, ever.) If the only thing this task changed is verification with no code edits, fold it into Task 5's commit instead and just do the checks here.

**Commit:** `engine: verify Phase 1 reference-site IR is behavior-preserving` (or fold into Task 5).

<!-- END_TASK_6 -->

---

## Phase 1 done when

- `model_ltm_reference_sites` is a `#[salsa::tracked]` function in `db_ltm_ir.rs` returning per-`(from,to)` `Vec<ClassifiedSite>` (shape + target_element + routing); `model_element_causal_edges`, `model_edge_shapes`, and `model_ltm_variables` are pure readers of it — no inline `Expr2` walk for shape, no `route_through_agg` / `routed_aggs` filter, no nested `enumerate_shapes` (AC1.1).
- `builtin_is_array_reducer` is gone; the array-reducer set + `Linear`/`Nonlinear`/`Constant` + monotone classification live only in `ltm_agg.rs`'s `reducer_kind` / `reducer_kind_from_name` / monotone predicate; `reducer_source_vars`, `agg_reducer_is_monotone`, `ltm_augment::classify_reducer`, `ltm_augment::is_array_reducer_name` are thin readers; the `reducer_kind` variant unit test passes (AC1.2).
- An all-`StarRange` reducer reference classifies as `Wildcard` / routes through the agg with no stray direct edge or Bare link score (AC1.4); a `SIZE` reference and a reducer over a scalar source classify as `Direct` (AC1.5); the cross-checking tests (`ref_site_*` / `*_reducer_is_not_hoisted` / `db_element_graph_tests` / `db_element_graph_proptest`) are kept and pass as IR regression guards.
- Every reducer-bearing / scalar / pure-A2A golden LTM fixture is byte-unchanged (no test-file edits; `ltm_results.tsv` test passes); `cargo test --workspace` passes within the 3-minute cap; the pre-commit hook passes (AC1.3).
- `src/simlin-engine/CLAUDE.md` lists the new `db_ltm_ir.rs` module.
