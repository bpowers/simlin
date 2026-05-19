# Element-Level Cycle Resolution — Test Requirements

This document maps **every** acceptance criterion from
[`docs/design-plans/2026-05-18-element-cycle-resolution.md`](../../design-plans/2026-05-18-element-cycle-resolution.md)
(the full set `element-cycle-resolution.AC1.1` … `element-cycle-resolution.AC8.3`
— 32 criteria) to either a concrete automated test or a documented human
verification approach. Each entry is cross-referenced to the implementation
phase file and task that implements the test.

The AC text below is copied **literally** from each phase file's "Acceptance
Criteria Coverage" section (which itself copies the design's "Acceptance
Criteria" section), with the **AC6.1 and AC6.4** wording taken from
[`phase_05.md`](phase_05.md)'s "Acceptance Criteria Coverage (AC6.1 corrected
per user decision)" section — the user-approved 2026-05-18 correction
(`source[base_i + offset[i]]` over the full source array, out-of-range → NaN,
**no modulo / no `rem_euclid`**) supersedes the design doc's `rem_euclid`
wording. This is intentional and verified — it is **not** a gap.

## Conventions and scope notes (intentional, do not flag as gaps)

These planning decisions are honored throughout this document; they are
documented in the phase files' "Design deviations" / "CRITICAL design
correction" / "Design contradiction resolved" sections and are deliberate:

1. **AC6.1 / AC6.4 are corrected.** Genuine Vensim `VECTOR ELM MAP` =
   `source[base_i + offset[i]]` over the full source array, out-of-range →
   `:NA:`/NaN, **no modulo, no `rem_euclid`** (Ventana official reference).
   The corrected wording from [`phase_05.md`](phase_05.md) is used, not the
   design doc's `(i + offset[i]) mod n` / `rem_euclid` wording.
2. **AC8.1 "un-`#[ignore]`d" means "un-stub".** Per the resolved contradiction
   in [`phase_07.md`](phase_07.md), `simulates_clearn` **keeps `#[test]
   #[ignore]`** (runtime-class) and is run explicitly via
   `cargo test --release -- --ignored simulates_clearn`. It is **not** in the
   default capped `cargo test` suite. "Un-stub" = it transitions from a
   permanently-skipped placeholder to a real test that compiles, runs, and
   passes when invoked explicitly. AC8.1's automated test is the `#[ignore]`d
   `simulates_clearn` run explicitly.
3. **Heavy C-LEARN tests are `#[ignore]`d / runtime-class.** AC7.* and AC8.1
   tests are `#[test] #[ignore]` and are run explicitly with
   `cargo test --release -- --ignored <name>`. They do **not** count against
   the 3-minute pre-commit / CI `cargo test` cap and are **not** in the
   default test set. The exact run commands are listed in the
   [Runtime-class / explicitly-run tests](#runtime-class--explicitly-run-tests)
   section.
4. **Some ACs are verified by NEW fixtures whose exact shape is empirically
   determined during execution.** AC2.4 (`init_recurrence.mdl` +
   `init_recurrence.dat`, [`phase_02.md`](phase_02.md) Task 9), AC3.1
   (`helper_recurrence.mdl` + `helper_recurrence.dat`,
   [`phase_03.md`](phase_03.md) Task 3): the test exists, but its fixture is
   constructed/verified during execution (the executor empirically confirms
   the converter produces the required SCC shape by inspecting
   `resolved_sccs` / `model_implicit_var_info`). Both have a **bounded-attempt
   (≈4-5 distinct shapes) + `track-issue` escalation** clause if the fixture
   cannot be built — the AC is not weakened or faked.
5. **Spike tasks verify nothing directly.** Phase 2 Task 1 (init-fragment
   representability) and Phase 5 Task 1 (base+stride) are de-risking spikes
   with `Verifies: none`; no AC maps to them. They gate downstream tasks
   (Phase 2 Task 1 has a HARD GATE — stop-and-surface, not autonomous
   fallback) but produce findings docs, not tests.

---

## AC1 — Single-variable self-recurrence resolves

Implemented and tested in **[`phase_01.md`](phase_01.md)** (Tarjan promotion +
single-variable self-recurrence).

| AC | Literal criterion text | Type | Test file | Test name / assertion (phase + task) |
|----|------------------------|------|-----------|--------------------------------------|
| **AC1.1** | **Success:** `test/sdeverywhere/models/self_recurrence/self_recurrence.mdl` compiles via the incremental path with no `CircularDependency`. | integration | `src/simlin-engine/tests/simulate.rs` | The rewritten `self_recurrence_self_token_resolves_to_real_name` test (renamed e.g. `self_recurrence_resolves_and_no_self_token_leak`) — asserts the compile `Result` is `Ok` (no `NotSimulatable`, no `CircularDependency`). phase_01 Task 3 (RED) → driven GREEN by Tasks 4-6 (folded into one green commit). Also `db_dep_graph_tests.rs`: single-variable self-recurrence `TestProject` ⇒ `has_cycle == false`, one `ResolvedScc` (Task 6). |
| **AC1.2** | **Success:** It simulates to the well-founded series `ecc[t1]=1, ecc[t2]=2, ecc[t3]=3` over its steps. | integration | `src/simlin-engine/tests/simulate.rs` | Same rewritten test: asserts the series in-test with the `element_series` helper (`tests/simulate.rs:1042`; call idiom copied from `previous_self_reference_still_resolves`) — reads `ecc[t1]`, `ecc[t2]`, `ecc[t3]` and asserts `[1.0, 2.0, 3.0]` (constant across both saved steps). phase_01 Task 3 + Task 6. |
| **AC1.3** | **Success:** `scc_components` is callable from production code (`pub(crate)`, not `#[cfg(test)]`) and the whole-variable happy path (acyclic models) is unaffected. | unit | `src/simlin-engine/src/db_dep_graph_tests.rs` | Promotion half: phase_01 Task 1 removes both `#[cfg(test)]` gates (`ltm/indexed.rs:207` + `ltm/mod.rs:58`); proven production-callable by Task 6 wiring. Happy-path-unaffected assertion: `db_dep_graph_tests.rs` — an unrelated acyclic model ⇒ empty `resolved_sccs`, `has_cycle == false` (Task 6); plus the byte-stable acyclic-control assertion in Task 9. |
| **AC1.4** | **Edge:** The emitted per-element run order is byte-identical across repeated compiles of the same model. | unit | `src/simlin-engine/src/db_dep_graph_tests.rs` | New determinism unit test alongside `dt_cycle_sccs_is_byte_stable_across_runs` (`db_dep_graph_tests.rs:204-222`): build the single-variable self-recurrence `TestProject`, compute the dependency graph **twice** on fresh databases, assert the two `resolved_sccs` (and their `element_order` vectors and `members` sets) are exactly equal. phase_01 Task 9. |
| **AC1.5** | **Failure:** A single-variable SCC whose induced element graph is genuinely cyclic (`x[dimA]=x[dimA]+1`) is NOT resolved — it still reports `CircularDependency`. | unit + integration | `src/simlin-engine/src/db_dep_graph_tests.rs`, `src/simlin-engine/tests/simulate.rs` | `db_dep_graph_tests.rs`: `x[dimA]=x[dimA]+1` ⇒ element self-loop ⇒ unresolved, `has_cycle == true`, empty `resolved_sccs` (phase_01 Task 5 + Task 6). End-to-end: the tightened `genuine_cycles_still_rejected` `x[dimA]=x[dimA]+1` case asserts `CircularDependency` specifically (phase_01 Task 7). |

---

## AC2 — Multi-variable recurrence SCC resolves

Implemented and tested in **[`phase_02.md`](phase_02.md)** (combined-fragment
lowering). Phase 2 Task 1 is a SPIKE (`Verifies: none`) — no AC maps to it; it
gates Task 6 with a HARD GATE.

| AC | Literal criterion text | Type | Test file | Test name / assertion (phase + task) |
|----|------------------------|------|-----------|--------------------------------------|
| **AC2.1** | **Success:** `test/sdeverywhere/models/ref/ref.mdl` compiles and simulates to its hand-computed per-element series. | integration | `src/simlin-engine/tests/simulate.rs` | New `#[test]` (e.g. `ref_mdl`) running `ref.mdl` through `simulate_mdl_path` (`tests/simulate.rs:286-305`), loading the sibling `ref.dat` via `load_expected_results_for_mdl`, comparing with `ensure_results`. Expected series `ce[t1]=1, ce[t2]=3, ce[t3]=5, ecc[t1]=2, ecc[t2]=4, ecc[t3]=6`. phase_02 Task 7 (must fail before Tasks 4-6, pass after). Combined-`Vec<Expr>` interleave unit-tested in phase_02 Task 4; combined-`PerVarBytecodes` well-formedness in Task 5. |
| **AC2.2** | **Success:** `test/sdeverywhere/models/interleaved/interleaved.mdl` compiles and simulates to its hand-computed values. | integration | `src/simlin-engine/tests/simulate.rs` | New `#[test]` running `interleaved.mdl` via `simulate_mdl_path`, comparing against `interleaved.dat` (all `1.0`, 101 steps). Whole-variable `a`↔`y` is a 2-cycle but element-wise `x → a[A1] → y → a[A2]` is acyclic. phase_02 Task 8 (fails before Tasks 4-6, passes after). |
| **AC2.3** | **Success:** The SCC is lowered as one combined fragment; variable offsets and the results offset map are unchanged vs. a hypothetical acyclic equivalent (per-variable result series remain individually addressable). | unit | `src/simlin-engine/src/db.rs` (`#[cfg(test)] mod tests`) / `db_dep_graph_tests.rs` | (a) phase_02 Task 4: given a hand-built two-member SCC + known `element_order`, assert the combined `Vec<Expr>` is exactly the slices in `element_order`, every original `AssignCurr` offset preserved, no expr dropped/duplicated. (b) phase_02 Task 6: focused `db.rs` `#[cfg(test)]` assertion that, for a resolved multi-variable SCC, the assembled module's results offset map for each member is identical to the offsets a hypothetical acyclic equivalent would get. (c) phase_02 Task 10: combined-fragment byte-stability test (compile `ref.mdl` twice on fresh DBs; bytecode/`resolved_sccs`/`element_order` byte-identical). |
| **AC2.4** | **Success:** Both dt-phase and init-phase recurrence SCCs are resolved (a fixture with an init-phase recurrence simulates correctly). | unit + integration | `src/simlin-engine/src/db_dep_graph_tests.rs`, `src/simlin-engine/tests/simulate.rs` | Unit: `db_dep_graph_tests.rs` — `init_walk_successors` returns BTreeSet-sorted, omits modules, includes stock deps, matches `db.rs:1209-1211` (phase_02 Task 2); an init-phase recurrence `TestProject` where a stock breaks the dt chain but the init relation has a forward element recurrence ⇒ `ResolvedScc { phase: Initial }`, dt no cycle; a genuine init element cycle ⇒ `CircularDependency` (phase_02 Task 3). Integration: a `#[test]` running the **NEW** `test/sdeverywhere/models/init_recurrence/init_recurrence.mdl` (+ hand-computed `init_recurrence.dat`) via `simulate_mdl_path` (phase_02 Task 9). **Fixture note:** the `.mdl`/`.dat` shape is empirically determined during execution (the executor confirms an init-only element SCC via `resolved_sccs`/the init verdict); bounded-attempt (≈4-5 shapes) + `track-issue` escalation if no shape isolates an init-only SCC. |
| **AC2.5** | **Edge:** `ref_interleaved_inter_variable_cycles_report_circular` is transitioned to assert correct simulation (not `CircularDependency`). | integration | `src/simlin-engine/tests/simulate.rs` | `ref_interleaved_inter_variable_cycles_report_circular` (`tests/simulate.rs:1356-1387`) transitioned to assert **correct simulation** for both `ref.mdl` and `interleaved.mdl` (or its intent folded into Tasks 7/8 with the correct-simulation assertions); the "no `UnknownDependency`/`DoesNotExist` leak" intent preserved as a guard if still meaningful. phase_02 Task 9. |
| **AC2.6** | **Failure:** `incremental_compilation_covers_all_models` and the existing model corpus stay green (no regression on non-recurrence models). | integration | `src/simlin-engine/tests/simulate.rs` | `incremental_compilation_covers_all_models` (`tests/simulate.rs:1525-1569`) — the existing 22-model `ALL_INCREMENTALLY_COMPILABLE_MODELS` + `TEST_MODELS` corpus gate stays green (verify-only, not weakened). phase_02 Task 10 (also Task 2's pure-refactor regression, Task 3). |

---

## AC3 — Synthetic-helper policy

Implemented and tested in **[`phase_03.md`](phase_03.md)** (synthetic-helper
sourcing policy).

| AC | Literal criterion text | Type | Test file | Test name / assertion (phase + task) |
|----|------------------------|------|-----------|--------------------------------------|
| **AC3.1** | **Success:** A well-founded recurrence whose SCC includes a synthetic helper (e.g. an INIT/PREVIOUS-helper-bearing recurrence) compiles and simulates when the helper is sourceable from its parent's `implicit_vars`. | unit + integration | `src/simlin-engine/src/db_dep_graph_tests.rs`, `src/simlin-engine/tests/simulate.rs` | Unit: `synthetic_helper_symbolic_fragment_is_parent_sourced` (`db_dep_graph_tests.rs`) — a `TestProject` whose recurrence SCC includes a synthetic helper; asserts `var_phase_symbolic_fragment_prod` returns `Some(PerVarBytecodes)` for the helper node (parent-`implicit_vars`-sourced) and `symbolic_phase_element_order` builds the SCC element graph with the helper present (phase_03 Task 2; the plan was reconciled in `a122fd1e` to the Phase 2 GH #575 symbolic accessor — `var_phase_lowered_exprs_prod` was removed as production-dead). Integration: `helper_recurrence_mdl_synthetic_helper_in_scc_simulates` (`simulate.rs`) running the **NEW** `test/sdeverywhere/models/helper_recurrence/helper_recurrence.mdl` (+ hand-computed `helper_recurrence.dat`, `ecc[t1]=1, ecc[t2]=2, ecc[t3]=4`) via `simulate_mdl_path`; structurally fails before Task 2 (helper unsourceable ⇒ `CircularDependency`), passes after (phase_03 Task 3). **Fixture note:** the fixture's exact shape (INIT/PREVIOUS/SMOOTH × subrange variant) was empirically determined during execution — the executor verified the converter pushes `$\u{205A}`-prefixed helpers into a resolved init-phase SCC by inspecting `resolved_sccs`/`model_implicit_var_info`; the as-built fixture is `ecc[t1]=1; ecc[tNext]=INITIAL(ecc[tPrev]*2)` (the literal `INIT(seed)` example produced no in-SCC helper — within-scope bounded-attempt iteration per phase_03 design-deviation 4, not an AC weakening). |
| **AC3.2** | **Failure:** An SCC with an in-cycle node that genuinely cannot be element-sourced falls back to `CircularDependency` — no panic, no silent miscompile. | unit | `src/simlin-engine/src/db_dep_graph_tests.rs` | `unsourceable_in_scc_node_falls_back_to_circular_no_panic` — a recurrence SCC where one in-cycle node is forced unsourceable via the `#[cfg(test)]` `UnsourceableVarsGuard` (RAII override making `var_phase_symbolic_fragment_prod` return `None` for a chosen in-SCC node), driven through the production `model_dependency_graph` path with a positive control on the same model: asserts the model is rejected with `CircularDependency`, **no panic**, empty `resolved_sccs` (the other members are NOT partially resolved). phase_03 Task 1 (loud-safe fallback first). |
| **AC3.3** | **Edge:** The `#[cfg(test)]` `array_producing_vars` accessor keeps its abort-on-no-`SourceVariable` contract (its test still passes unchanged). | unit | `src/simlin-engine/src/db_dep_graph_tests.rs` | The existing `array_producing_vars_flags_exactly_the_two_positive_cases` (locate by name) runs **unchanged** and must still pass — proves the separate `#[cfg(test)]` `array_producing_vars`/`var_noninitial_lowered_exprs` abort contract is intact and was not collaterally affected by removing the production-dead `var_phase_lowered_exprs_prod`; Phase 3 changes only the production *symbolic* accessor `var_phase_symbolic_fragment_prod`, never the `#[cfg(test)]` panic wrapper. phase_03 Task 1. |

---

## AC4 — Genuine cycles still rejected (hard rule)

Implemented and tested in **[`phase_01.md`](phase_01.md)** (Tasks 5, 7, 8).

| AC | Literal criterion text | Type | Test file | Test name / assertion (phase + task) |
|----|------------------------|------|-----------|--------------------------------------|
| **AC4.1** | **Failure:** `a=b+1; b=a+1` reports `CircularDependency` (scalar 2-cycle). | unit + integration | `src/simlin-engine/tests/simulate.rs`, `src/simlin-engine/src/db_dep_graph_tests.rs` | `genuine_cycles_still_rejected` (`tests/simulate.rs:1152-1219`) — the `a=b+1; b=a+1` case (`:1159-1167`) assertion is **kept unchanged**, must still report `CircularDependency` (scalar 2-cycle / multi-var SCC unresolved in Phase 1 and a genuine element 2-cycle). phase_01 Task 7. Unit: `db_dep_graph_tests.rs` — `a=b+1; b=a+1` ⇒ element 2-cycle ⇒ unresolved, `has_cycle == true`, empty `resolved_sccs` (Task 5/6). |
| **AC4.2** | **Failure:** `x[dimA]=x[dimA]+1` reports `CircularDependency` (genuine same-element self-cycle). | unit + integration | `src/simlin-engine/tests/simulate.rs`, `src/simlin-engine/src/db_dep_graph_tests.rs` | `genuine_cycles_still_rejected` `x[dimA]=x[dimA]+1` case (`:1192-1200`) — assertion **tightened** from `CircularDependency \| UnknownDependency \| DoesNotExist` to require `CircularDependency` **specifically**; the contradicting rationale comment block (`:1186-1191`, inline `~1214-1217`) rewritten in the **same commit** (CLAUDE.md comment-freshness). phase_01 Task 7. Unit: `db_dep_graph_tests.rs` — `x[dimA]=x[dimA]+1` ⇒ element self-loop ⇒ unresolved (Task 5/6). |
| **AC4.3** | **Success:** `genuine_cycles_still_rejected` passes unchanged; the `dt_cycle_sccs_engine_consistent` harness passes against the new invariant (element-acyclic ⇒ resolved/no diagnostic; element-cyclic ⇒ `CircularDependency`). | unit + integration | `src/simlin-engine/tests/simulate.rs`, `src/simlin-engine/src/db_dep_graph_tests.rs` | `genuine_cycles_still_rejected` stays green (phase_01 Task 7). Harness half: `dt_cycle_sccs_consistency_violation` (`db_dep_graph.rs:311-329`) + `dt_cycle_sccs_engine_consistent` (`:342-363`) re-pointed to the new invariant — for each instrumented SCC the engine raises `CircularDependency` **iff** that SCC is not in `resolved_sccs`; `db_dep_graph_tests.rs` consistency cases extended: single-variable self-recurrence ⇒ instrumented self-loop present **and** no `CircularDependency` (resolved); `a=b+1;b=a+1` ⇒ instrumented + `CircularDependency`. phase_01 Task 8. |

---

## AC5 — VECTOR SORT ORDER genuine Vensim semantics

Implemented and tested in **[`phase_04.md`](phase_04.md)** (genuine Vensim
0-based). Independent of Phases 1-3.

| AC | Literal criterion text | Type | Test file | Test name / assertion (phase + task) |
|----|------------------------|------|-----------|--------------------------------------|
| **AC5.1** | **Success:** `VECTOR SORT ORDER([2100,2010,2020], ascending)` yields the 0-based permutation `[1,2,0]` (matching genuine Vensim). | unit + integration | `src/simlin-engine/src/array_tests.rs`, `src/simlin-engine/tests/compiler_vector.rs`, `src/simlin-engine/tests/simulate.rs` | VM one-token fix `vm.rs:2386` `i+1`→`i` (phase_04 Task 1, behavior verified by Tasks 2-4's corrected literals). Corrected `array_tests.rs` cases: `vector_builtin_promotes_active_dim_ref_monolithic`/`_vm` (`mod flag_split_tests`) `vals[DimA]=[30,10,20]` asc ⇒ `&[1.0, 2.0, 0.0]` (Task 2). `vector_simple.dat` `l` column for `h=[2100,2010,2020]` asc ⇒ `l[a1]=1, l[a2]=2, l[a3]=0`, gated by `simulates_vector_simple_mdl` (`tests/simulate.rs:767-770`) (Task 4). |
| **AC5.2** | **Success:** Descending direction yields the correct 0-based permutation; ties preserve stable order consistent with genuine Vensim. | unit + integration | `src/simlin-engine/src/array_tests.rs`, `src/simlin-engine/tests/simulate.rs` | `vector_simple.dat` `m` column = `VECTOR SORT ORDER(h, descending)` of `[2100,2010,2020]` ⇒ `m[a1]=0, m[a2]=2, m[a3]=1`, gated by `simulates_vector_simple_mdl` (phase_04 Task 4). Descending + stable-tie cases in `array_tests.rs`/`compiler_vector.rs` (`vso_*` cases, phase_04 Tasks 2-3). Stable sort (Rust `sort_by`, `vm.rs:2390-2398`) is kept — deterministic, byte-stable; Vensim leaves ties unspecified (phase_04 design deviation 4). |
| **AC5.3** | **Edge:** The Simlin-authored fixtures that encoded 1-based output (`array_tests.rs` cases, `vector_simple.dat` `l`/`m`) are corrected to genuine Vensim and pass. | unit + integration | `src/simlin-engine/src/array_tests.rs`, `src/simlin-engine/tests/compiler_vector.rs`, `src/simlin-engine/tests/simulate.rs` | All Simlin-authored 1-based fixtures corrected to genuine 0-based and passing: `array_tests.rs` `mod flag_split_tests`, `mod different_builtin_override_tests` (VSO parts, lines 4010/4032), `mod dimension_dependent_scalar_arg_tests` (`assert_vso_dim_dep_results`, `vso_nested_direction_varies_by_dimension_vm`, `vso_except_direction_varies_by_dimension_vm`) with comments updated to 0-based (phase_04 Task 2); the **five** `tests/compiler_vector.rs` VSO tests (`vector_sort_order_a2a_*`, `nested_vector_sort_order_inside_sum_*`) (phase_04 Task 3); `vector_simple.dat` `l`/`m` columns (phase_04 Task 4). `Opcode::Rank` is **not** touched (genuine Vensim `RANK` is 1-based — phase_04 design deviation 3). |

---

## AC6 — VECTOR ELM MAP genuine Vensim semantics (AC6.1 / AC6.4 corrected)

Implemented and tested in **[`phase_05.md`](phase_05.md)** (base + full-source,
**no modulo**). Phase 5 Task 1 is a SPIKE (`Verifies: none`) — no AC maps to
it; it gates Task 2. AC6.1 / AC6.4 use the **corrected** wording from
phase_05's "Acceptance Criteria Coverage (AC6.1 corrected per user decision)"
section (supersedes the design doc's `rem_euclid`/`mod n` wording).

| AC | Literal criterion text (corrected per phase_05) | Type | Test file | Test name / assertion (phase + task) |
|----|------------------------|------|-----------|--------------------------------------|
| **AC6.1** | **Success (CORRECTED):** Result element `i` equals `source[base_i + offset[i]]` resolved over the **full source array** (last subscript varies fastest), where `base_i` is the position established by the first-argument element reference; an offset landing outside the source variable's full storage range yields `:NA:`/NaN (no modulo, no wraparound). *(This supersedes the design's `(i + offset[i]) mod n` / `rem_euclid` wording per the user-approved correction.)* | unit + integration | `src/simlin-engine/src/array_tests.rs`, `src/simlin-engine/tests/simulate.rs` | VM correction `vm.rs:2301-2356` (per-element base from arg-1 element ref + full-source-dimension stride resolution, OOB→NaN, **no `rem_euclid`/modulo**) — phase_05 Task 2. New focused `array_tests.rs` cross-dimension unit case (`f[DimA,DimB]=VECTOR ELM MAP(d[DimA,B1], a[DimA])` ⇒ `f[A1]=1, f[A2]=5, f[A3]=6`) — the new behavior, phase_05 Task 2. End-to-end gated by `vector.xmile` (Task 6) and `vector_simple.dat` `f`/`g` (Task 5). |
| **AC6.2** | **Success:** A cross-dimension source (`d[DimA,B1]` with offset reaching the `B2` column) resolves against the full source dimension, matching genuine Vensim `vector.dat`. | unit + integration | `src/simlin-engine/src/array_tests.rs`, `src/simlin-engine/tests/simulate.rs` | The cross-dimension `array_tests.rs` unit case above (offset reaches the `B2` column) — phase_05 Task 2. `vector_simple.dat` `f`/`g` corrected to genuine values (`f=[1,5,6]`; `g[A1,B1]=1,g[A1,B2]=4,g[A2,B1]=5,g[A2,B2]=2,g[A3,B1]=3,g[A3,B2]=6`), gated by `simulates_vector_simple_mdl` — phase_05 Task 5. `vector.xmile` `y=VECTOR ELM MAP(x[three],(DimA-1))` cross-dim scalar-source case, gated by `simulates_arrayed_models_correctly` — phase_05 Task 6. |
| **AC6.3** | **Success:** `test/sdeverywhere/models/vector/vector.xmile` is un-excluded and simulates to genuine-Vensim `vector.dat`. | integration | `src/simlin-engine/tests/simulate.rs` | `tests/simulate.rs:656` uncommented (`vector.xmile` entry in `TEST_SDEVERYWHERE_MODELS`); `simulates_arrayed_models_correctly` (`tests/simulate.rs:671-677`) picks it up and compares against real-Vensim `vector.dat` (`c[A1]=11,c[A2]=12,c[A3]=12`; `f`/`g` as Task 5; `y[A1]=3,y[A2]=4,y[A3]=5`) through all three `ensure_results` paths (VM, protobuf round-trip, XMILE round-trip). Stale exclusion comment block (`:651-657`) rewritten. phase_05 Task 6. |
| **AC6.4** | **Edge (CORRECTED):** Corrected `vector_elm_map_tests` and `vector_simple.dat` `f`/`g` columns pass against genuine Vensim values; out-of-range still yields NaN (genuine Vensim `:NA:`) — the existing OOB/negative→NaN cases are recomputed with the base added, not redesigned around wrap. | unit + integration | `src/simlin-engine/src/array_tests.rs`, `src/simlin-engine/tests/compiler_vector.rs`, `src/simlin-engine/tests/simulate.rs` | `mod vector_elm_map_tests` (`array_tests.rs:3474-3587`, `in_bounds_*`/`out_of_bounds_*`/`negative_offset_*` + `make_oob_project` helper) re-derived per-case under genuine semantics; OOB/negative keep NaN where `base_i + offset_i` is genuinely out of `[0, len)` (phase_05 Task 3). The 7 hoisting modules (`array_tests.rs:3589-4039`) + `compiler_vector.rs` VEM tests re-derived per fixture (phase_05 Task 4). `vector_simple.dat` `f`/`g` columns corrected, gated by `simulates_vector_simple_mdl` (phase_05 Task 5). |

---

## AC7 — C-LEARN structural gate + #363

Implemented and tested in **[`phase_06.md`](phase_06.md)**. The structural-gate
test and the LTM-discovery test are **`#[ignore]`d / runtime-class** and run
explicitly (see [Runtime-class section](#runtime-class--explicitly-run-tests)).
Depends on Phases 2, 3, 5.

| AC | Literal criterion text | Type | Test file | Test name / assertion (phase + task) |
|----|------------------------|------|-----------|--------------------------------------|
| **AC7.1** | **Success:** C-LEARN (`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`) compiles via the incremental path with no fatal `ModelError` (no `circular_dependency`; non-fatal unit-inference warnings allowed). | integration (`#[ignore]`d) | `src/simlin-engine/tests/simulate.rs` | New `#[test] #[ignore]` `compiles_and_runs_clearn_structural` (modeled on `simulates_wrld3_03`): reads C-LEARN, calls `compile_project_incremental` directly (not the `compile_vm` `.unwrap()` wrapper), asserts the compile `Result` is `Ok` — no fatal `ModelError`, specifically **no `circular_dependency`**; non-fatal unit-inference warnings allowed; on diagnostic, fail with collected diagnostics. phase_06 Task 1. **Run with:** `cargo test -p simlin-engine --features file_io --release -- --ignored compiles_and_runs_clearn_structural --nocapture`. |
| **AC7.2** | **Success:** The C-LEARN VM runs to FINAL TIME with no panic. | integration (`#[ignore]`d) | `src/simlin-engine/tests/simulate.rs` | Same `compiles_and_runs_clearn_structural`: `Vm::new(compiled).unwrap()` then `vm.run_to_end().unwrap()` (runs to FINAL TIME) — **no `catch_unwind`** (a post-gate panic must propagate as a hard test failure with backtrace). phase_06 Task 1; re-verified by phase_06 Task 3 (root-cause any post-gate panic; convert panic site to typed `Result::Err` per #363's prescribed fix). |
| **AC7.3** | **Success:** No core C-LEARN series is entirely NaN after the run. | integration (`#[ignore]`d) | `src/simlin-engine/tests/simulate.rs` | Same `compiles_and_runs_clearn_structural`: `vm.into_results()`, define "core series" as `Ref.vdf.offsets ∩ results.offsets` (parse `Ref.vdf` via `VdfFile::parse(...).to_results_via_records()`), assert for each matched ident at least one step is non-NaN (`(0..step_count).any(|s| !data[s*step_size+off].is_nan())`); fail listing any entirely-NaN matched idents. phase_06 Task 1 (dovetails AC8.2's NaN guard). |
| **AC7.4** | **Success:** The residual test-only `catch_unwind` for C-LEARN (`tests/ltm_discovery_large_models.rs:670`) is removed and its `clearn_*` test expects a clean compile result. | integration (`#[ignore]`d) | `src/simlin-engine/tests/ltm_discovery_large_models.rs` | `clearn_ltm_discovery_blocked_by_macro_expansion` (`:624-693`) renamed (e.g. `clearn_ltm_discovery_compiles`): `catch_unwind` at `:670` **removed** (compile called directly); contract **inverted** — with LTM discovery enabled (`set_project_ltm_enabled(true)` + `set_project_ltm_discovery_mode(true)`), assert `compile_project_incremental(...)` returns `Ok`; stale docstrings (`:624-647`, `CLEARN_MDL` const `:127-130`) rewritten; keeps `#[test] #[ignore]`. Verification also asserts **no `catch_unwind` remains** in `src/simlin-engine/tests/` (`rg catch_unwind src/simlin-engine/tests` ⇒ no hits). phase_06 Task 2. **Run with:** `cargo test -p simlin-engine --features xmutil --release -- --ignored clearn_ltm_discovery --nocapture`. |
| **AC7.5** | **Failure:** If a post-gate panic surfaces, it is a hard test failure (root-caused), not caught/ignored. | integration (`#[ignore]`d) | `src/simlin-engine/tests/simulate.rs` (+ converted-site unit test if a panic reproduces) | `compiles_and_runs_clearn_structural` deliberately does **NOT** wrap the run in `catch_unwind` — a post-gate panic propagates as a hard, root-caused failure with backtrace (phase_06 Task 1). phase_06 Task 3: run C-LEARN under a debug build with `RUST_BACKTRACE=1`; if a panic reproduces, root-cause it and convert the panic site to a typed `Result::Err` flowing through `NotSimulatable`/the diagnostic path, **plus a minimal unit test reproducing the converted condition** (so coverage does not depend on the heavy `#[ignore]`d test); if none reproduces, re-verify/record #363 status via `track-issue`. |

---

## AC8 — C-LEARN numeric finalization

Implemented and tested in **[`phase_07.md`](phase_07.md)**. AC8.1's test
(`simulates_clearn`) is **`#[ignore]`d / runtime-class** (un-stub, not literal
un-`#[ignore]`); AC8.2's guard test is fast/synthetic and **is** in the default
capped suite. Depends on Phase 6.

| AC | Literal criterion text | Type | Test file | Test name / assertion (phase + task) |
|----|------------------------|------|-----------|--------------------------------------|
| **AC8.1** | **Success:** `simulates_clearn` is un-`#[ignore]`d and passes — C-LEARN matches `test/xmutil_test_models/Ref.vdf` within the existing 1% cross-simulator tolerance. *(Interpreted per the resolved contradiction: un-stubbed; `#[ignore]` stays; passes when run explicitly via `--ignored`.)* | integration (`#[ignore]`d) | `src/simlin-engine/tests/simulate.rs` | `simulates_clearn` (`tests/simulate.rs:949`) — **keeps `#[test] #[ignore]`** + `// Run with: cargo test --release -- --ignored simulates_clearn` (`:946`); **un-stubbed** from a permanently-skipped placeholder to a real test: `open_vensim` → `compile_vm` → `Vm::new` → `run_to_end` → parse `Ref.vdf` → hardened `ensure_vdf_results`, passing within the **existing 1% tolerance** (literal `0.01`, `simulate.rs:173`, not loosened). phase_07 Task 3 (un-stub) + Task 4 (bounded numeric finalization; any general-fix-resistant residual filed via `track-issue`, not hacked, not tolerance-loosened). **Run with:** `cargo test -p simlin-engine --features file_io --release -- --ignored simulates_clearn --nocapture`. |
| **AC8.2** | **Failure:** `ensure_vdf_results` fails (does not vacuously pass) when fewer than a minimum number of variables match, or when a core series is entirely NaN / the NaN-skipped fraction exceeds the guard threshold (covered by a dedicated guard test with synthetic inputs). | unit (synthetic, default suite) | `src/simlin-engine/tests/simulate.rs` | New free `#[test]` `ensure_vdf_results_rejects_vacuous_comparisons` (RED-first; adds `Specs`/`Method` imports). Constructs synthetic `Results` literals (template `results.rs:276-300`) and asserts `ensure_vdf_results` **panics** via `std::panic::catch_unwind` *inside this synthetic guard test only* (legitimate test-of-a-panicking-assertion, distinct from the Phase 6-retired production `catch_unwind`) in: below-floor (0/1 matching ident), entirely-NaN core series, excessive NaN-skipped fraction; **positive control** (well-formed comparison) does NOT panic. Hardened `ensure_vdf_results` (`tests/simulate.rs:135-190`): matched-variable floor `MIN_MATCHED` (named const, documented), per-matched-variable NaN-skip counter, both as **additional** failure conditions (never relaxations). phase_07 Tasks 1 + 2. This test **runs in the default capped suite** (fast, synthetic). |
| **AC8.3** | **Edge:** The `simulates_clearn` stale comment is replaced with an accurate description; the engine default test suite stays green under the 3-minute cap (`simulates_clearn` runs explicitly by runtime class, not in the capped default set). | integration + meta | `src/simlin-engine/tests/simulate.rs` | Stale comment block (`tests/simulate.rs:912-946`) replaced with an accurate description (macro work + `MismatchedDimensions`/`UnknownDependency`/`DoesNotExist` cleared on this branch; the previously-fatal `CircularDependency` was the false whole-variable verdict, resolved by Phases 1-2; the test now does the full end-to-end numeric comparison and is `#[ignore]`d only for runtime class). `simulates_clearn` keeps `#[ignore]` so the default `cargo test` stays under the 3-minute cap. Meta-assertion verified by the pre-commit hook: `git commit` runs fmt/clippy/`cargo test` (180s cap) green, with `simulates_clearn` excluded by `#[ignore]`. phase_07 Task 3. |

---

## Human verification

Most criteria are fully automated. The items below require **human
verification** (or human-in-the-loop judgement) for the reasons stated; each
has a concrete manual approach. None of these are gaps in the plan — they are
inherent judgement points or meta-properties that an assertion cannot fully
capture.

### HV-1 — AC8.3 (and the cross-phase "Done When" cap claims): default suite stays under the 3-minute cap

- **Why not fully automated by an in-suite assertion:** "the engine default
  test suite stays green under the 3-minute wall-clock cap" is a property of
  the **whole suite's wall-clock**, not of any single test. No test asserts
  its own suite's total runtime; the bound is enforced operationally by the
  pre-commit hook / CI cap, which is environment- and machine-dependent.
- **Manual verification approach:** after the final commit of each phase,
  confirm the pre-commit hook completed (it runs `cargo test` under the 180s
  cap and fails the commit if exceeded — never bypassed with `--no-verify`).
  Independently, a human runs `time cargo test -p simlin-engine` (or the
  workspace `cargo test --workspace`) once and confirms wall-clock < 180s with
  all heavy C-LEARN tests `#[ignore]`d (i.e. excluded). Re-check after Phase 6
  (adds the `#[ignore]`d structural test) and Phase 7 (adds the fast synthetic
  guard test — confirm the *synthetic* guard test is fast and `simulates_clearn`
  remained `#[ignore]`d).
- **Note:** AC7.* / AC8.1 ACs (`#[ignore]`d) explicitly do **not** count
  against this cap; this HV only concerns the *default* (non-`--ignored`)
  set.

### HV-2 — AC2.4 / AC3.1 fixture authenticity (the `.mdl`/`.dat` are correct, not just self-consistent)

- **Why not fully automated:** the new fixtures `init_recurrence.mdl` /
  `init_recurrence.dat` (AC2.4, phase_02 Task 9) and
  `helper_recurrence.mdl` / `helper_recurrence.dat` (AC3.1, phase_03 Task 3)
  have **no external Vensim ground-truth `.dat`** — their expected values are
  **hand-computed by the plan author during execution**. An automated test
  can only check the engine matches the hand-computed `.dat`; it cannot prove
  the hand computation itself is correct, nor (without inspection) that the
  fixture genuinely exercises the intended path (an init-**only** element SCC
  for AC2.4; a `$\u{205A}`-prefixed synthetic helper **inside** the element
  SCC for AC3.1). The plans explicitly require empirical confirmation by
  inspecting `resolved_sccs` / the init verdict / `model_implicit_var_info`.
- **Manual verification approach:** a human reviews each new fixture's
  equation list and independently re-derives the expected per-element series
  by hand, confirming it equals the committed `.dat`. For AC2.4, a human
  confirms (via the executor's recorded `resolved_sccs`/verdict dump, or by
  re-running the dump) that the dt phase is acyclic via a stock break **and**
  the init phase produces a `ResolvedScc { phase: Initial }` (i.e. it is a
  genuine init-only SCC, not an incidental dt+init one). For AC3.1, a human
  confirms the SCC contains a `$\u{205A}`-prefixed member (synthetic helper)
  so the parent-`implicit_vars` sourcing path is genuinely exercised, not
  bypassed. If the bounded-attempt clause fired and a `track-issue` was filed
  instead, the human confirms the issue accurately records the shapes tried
  and the observed output, and decides whether to accept the tracked gap or
  iterate.

### HV-3 — AC8.1 numeric finalization root-causing (judging "general fix" vs "model-specific hack")

- **Why not fully automated:** `simulates_clearn` passing within 1% is a
  binary automated check, but **getting there** (phase_07 Task 4) is bounded
  numeric debugging. The CLAUDE.md hard rule "general engine fixes with no
  model-specific hacks" and "do not loosen the 1% tolerance / the new guards"
  are **qualitative engineering-judgement constraints** an assertion cannot
  enforce — a hack can make the test green just as a general fix can.
- **Manual verification approach:** a human reviews every source change made
  under phase_07 Task 4 and confirms each is a **general** engine fix (carries
  its own fast, model-agnostic unit test; is not gated on C-LEARN-specific
  identifiers/shapes; does not special-case the model). Confirm the literal
  `0.01` tolerance at `simulate.rs:173` is **unchanged** and the AC8.2 guards
  were not weakened to force a pass. Confirm any residual mismatch that
  resisted a general fix was filed via the `track-issue` agent (not silenced),
  and review that issue for accuracy.

### HV-4 — AC7.5 / AC7.2 #363 re-verification disposition

- **Why not fully automated:** AC7.2/AC7.5 are a **genuine re-verification**
  of #363 (the design's thesis "the cycle gate masks #363" is explicitly *not*
  a codebase-recorded fact). Whether a post-gate panic reproduces is unknown
  until run; the *response* (root-cause + convert panic site to typed `Err`;
  or, if none reproduces, comment on / record #363 via `track-issue` without
  closing unless the user directs) involves judgement and a GitHub-issue side
  effect that the automated tests do not assert.
- **Manual verification approach:** a human runs
  `compiles_and_runs_clearn_structural` (and the renamed
  `clearn_ltm_discovery_*`) under a debug build with `RUST_BACKTRACE=1` and
  inspects the outcome. If a panic reproduced and was converted to `Err`,
  confirm a focused unit test reproduces the converted condition and the
  pipeline now returns a clean diagnostic (not a panic). If no panic
  reproduced, confirm GitHub issue #363 has a re-verification comment recorded
  via `track-issue` (and was **not** closed unless the user explicitly
  directed). Confirm `rg catch_unwind src/simlin-engine/tests` returns no
  hits (AC7.4 mechanical check, but the *disposition* of #363 is the human
  judgement).

### HV-5 — Spike findings adequacy (gates AC2.* / AC6.*, but verify nothing themselves)

- **Why noted (not an AC, but load-bearing):** Phase 2 Task 1 and Phase 5
  Task 1 are spikes with `Verifies: none` — no AC maps to them — but Phase 2
  Task 1 has a **HARD GATE**: if the init combined fragment is concluded
  **not** representable by any mechanism, the implementation MUST STOP and
  surface to the user (and file via `track-issue`) before proceeding — a
  silent loud-safe init fallback is a plan-invalidating outcome (it strands
  AC2.4 and leaves C-LEARN's init cycle blocked, breaking Phase 6/7).
- **Manual verification approach:** a human reads
  `phase_02_spike_findings.md` and `phase_05_spike_findings.md` before the
  respective downstream tasks (Phase 2 Task 6; Phase 5 Task 2). For Phase 2:
  confirm the findings unambiguously state a **representable** init mechanism
  (where to inject, what synthetic ident, how `member_base + elem` offsets
  stay correct through `resolve_module`); if instead the findings hit the
  HARD GATE, confirm the implementation **stopped and surfaced to the user**
  (and filed a `track-issue`) rather than silently degrading. For Phase 5:
  confirm the findings state the exact base + stride computation and reproduce
  the genuine `vector.dat` / `vector_simple.dat` `f`/`g` values by hand.

---

## Runtime-class / explicitly-run tests

The following tests are **`#[test] #[ignore]`** (runtime-class) because
C-LEARN is a ~53k-line model: parse alone is ~4-5s release (far longer debug),
and full compile+run+VDF-compare is much more — well over the 3-minute
pre-commit/CI `cargo test` cap. They are therefore **excluded from the default
`cargo test` set** and are run **explicitly** with the exact commands below.
They do **not** count against the 180s cap; the pre-commit hook still runs
(fmt/clippy/non-`#[ignore]`d tests) on every commit and is never bypassed with
`--no-verify`.

| Test | File | ACs covered | Exact run command |
|------|------|-------------|-------------------|
| `compiles_and_runs_clearn_structural` | `src/simlin-engine/tests/simulate.rs` | AC7.1, AC7.2, AC7.3, AC7.5 | `cargo test -p simlin-engine --features file_io --release -- --ignored compiles_and_runs_clearn_structural --nocapture` |
| `clearn_ltm_discovery_compiles` (renamed from `clearn_ltm_discovery_blocked_by_macro_expansion`) | `src/simlin-engine/tests/ltm_discovery_large_models.rs` | AC7.4 | `cargo test -p simlin-engine --features xmutil --release -- --ignored clearn_ltm_discovery --nocapture` |
| `simulates_clearn` (kept `#[ignore]`d — "un-stub", not literal un-`#[ignore]`) | `src/simlin-engine/tests/simulate.rs` | AC8.1 | `cargo test -p simlin-engine --features file_io --release -- --ignored simulates_clearn --nocapture` |

**Not** runtime-class (runs in the default capped suite, fast):
`ensure_vdf_results_rejects_vacuous_comparisons` (AC8.2) — synthetic
`Results` literals, no C-LEARN parse; it is a normal `#[test]` and **must**
run within the 3-minute cap.

All other ACs (AC1.*, AC2.*, AC3.*, AC4.*, AC5.*, AC6.*) are verified by
tiny fixtures / unit tests with **no `#[ignore]`** — they run in the default
`cargo test` suite (per-test budget ≈2s debug, 5s ceiling) and are gated by
the pre-commit hook (`cargo test` under the 180s cap, never `--no-verify`).
The standard verification command for those is, per phase:
`cargo test -p simlin-engine --features file_io <name>` followed by
`git commit` (the pre-commit hook runs the full default `cargo test`).
