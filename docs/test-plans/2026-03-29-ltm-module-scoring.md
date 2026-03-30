# LTM Module Composite Scoring -- Human Test Plan

## Prerequisites

- Development machine with Rust toolchain, `pnpm`, and the Simlin monorepo
- Run `./scripts/dev-init.sh`
- All non-ignored tests passing: `cargo test -p simlin-engine --features "testing,file_io"`
- 6 `#[ignore]` tests expected (documented stdlib layout resolution limitation)

## Phase 1: Compilation Pipeline Verification

| Step | Command | Expected |
|------|---------|----------|
| 1.1 | `cargo test -p simlin-engine test_ltm_smooth_model_compiles_with_ltm --features testing` | SMOOTH feedback model compiles with LTM, layout has more slots |
| 1.2 | `cargo test -p simlin-engine test_ltm_delay_model_compiles --features testing` | DELAY1 feedback model compiles with LTM |
| 1.3 | `cargo test -p simlin-engine test_ltm_passthrough_module_compiles --features testing` | Passthrough module compiles, main model has LTM offsets |
| 1.4 | `cargo test -p simlin-engine test_ltm_multiple_smooth_instances_compile --features testing` | Two SMOOTH instances produce independent link_score vars |
| 1.5 | `cargo test -p simlin-engine test_model_ltm_variables_stdlib_module --features testing` | Stdlib smth1 produces link_score, pathway, composite vars. No `ilink`. |
| 1.6 | `cargo test -p simlin-engine test_model_ltm_variables_passthrough_module --features testing` | Passthrough (no stocks) produces empty LTM vars |

## Phase 2: VM Integration (Non-Module Models)

| Step | Command | Expected |
|------|---------|----------|
| 2.1 | `cargo test -p simlin-engine --features "testing,file_io" test_feedback_loop_exhaustive_vm` | Non-zero link scores, loop_score and rel_loop_score vars |
| 2.2 | `cargo test -p simlin-engine --features "testing,file_io" test_feedback_loop_discovery_vm` | Link scores present, discover_loops finds loops |
| 2.3 | `cargo test -p simlin-engine --features "testing,file_io" test_reinforcing_feedback_loop_vm` | Reinforcing loop: non-zero link scores |
| 2.4 | `cargo test -p simlin-engine --features "testing,file_io" test_multiple_feedback_loops_vm` | >= 6 link_score vars, >= 2 loop_score vars |
| 2.5 | `cargo test -p simlin-engine --features "testing,file_io" simulates_population_ltm` | Golden data validation passes |

## Phase 3: VM Integration (User-Defined Modules)

| Step | Command | Expected |
|------|---------|----------|
| 3.1 | `cargo test -p simlin-engine --features "testing,file_io" test_user_defined_module_ltm_vm` | Composite scores under growth_model.* namespace, input_signal composite present, loop/rel_loop scores |
| 3.2 | `cargo test -p simlin-engine --features "testing,file_io" test_nested_module_ltm_vm` | Nested composites under processor.*, chained nesting keys, non-zero nested link scores |
| 3.3 | `cargo test -p simlin-engine --features "testing,file_io" test_passthrough_module_ltm_vm` | No composite scores, link scores for non-module edges |

## Phase 4: Migration Verification (Legacy Tests on VM Path)

| Step | Command | Expected |
|------|---------|----------|
| 4.1 | `cargo test -p simlin-engine --features "testing,file_io" discovery_logistic_growth_finds_both_loops` | Exactly 2 loops involving population |
| 4.2 | `cargo test -p simlin-engine --features "testing,file_io" discovery_cross_validates_with_exhaustive` | Same loops in both modes |
| 4.3 | `cargo test -p simlin-engine --features "testing,file_io" discovery_arms_race_3party` | 7 exhaustive loops, discovery finds all 7 |
| 4.4 | `cargo test -p simlin-engine --features "testing,file_io" hero_culture_loop_sign_continuity` | No suspicious sign discontinuities |
| 4.5 | `cargo test -p simlin-engine --features "testing,file_io" test_independent_subsystems_partitioned_relative_scores` | Each relative score abs value = 1.0 |
| 4.6 | `cargo test -p simlin-engine --features "testing,file_io" test_coupled_two_stock_single_partition` | Single partition, relative loop scores exist |

## Phase 5: Code Deletion Verification

| Step | Action | Expected |
|------|--------|----------|
| 5.1 | Search for `fn with_ltm` in `project.rs` | No matches |
| 5.2 | Search for `generate_ltm_variables`, `SyntheticVariables`, `CompositePortMap` in `ltm_augment.rs` | No matches |
| 5.3 | Search for `generate_module_link_score_equation` in engine source | No matches (inlined into link_score_equation_text) |
| 5.4 | Search for `cfg.*testing` + LTM terms in engine source | No matches |
| 5.5 | `cargo build --workspace` | No LTM-related dead-code warnings |

## Human Verification Required

| ID | What | Steps |
|----|------|-------|
| HV1 | Documentation accuracy | Review `docs/design/ltm--loops-that-matter.md` for correct architecture description, no stale refs. Review `src/simlin-engine/CLAUDE.md` for correct `db_ltm.rs` description and test file listing. |
| HV2 | Salsa cache invalidation | Run `cargo test -p simlin-engine test_ltm_caching --features testing`. Confirm all 4 caching tests pass. |
| HV3 | Discovery parser prefix assumption | Verify `strip_prefix("$\u{205A}ltm\u{205A}link_score\u{205A}")` naturally excludes interpunct-prefixed sub-model link scores. |

## Acceptance Criteria Traceability

| AC | Automated Test | Manual Step |
|----|---------------|-------------|
| AC1.1 | `test_ltm_smooth_model_compiles_with_ltm`; `test_smooth_goal_seeking_ltm` (#[ignore]) | 1.1 |
| AC1.2 | `test_ltm_delay_model_compiles` | 1.2 |
| AC1.3 | `test_user_defined_module_ltm_vm` | 3.1 |
| AC1.4 | `test_nested_module_ltm_vm` | 3.2 |
| AC1.5 | `test_user_defined_module_ltm_vm` (exhaustive mode) | 3.1 |
| AC1.6 | `test_feedback_loop_discovery_vm`; `test_smooth_model_discovery_mode` (#[ignore]) | 2.2 |
| AC1.7 | `test_model_ltm_variables_passthrough_module` + `test_passthrough_module_ltm_vm` | 1.6, 3.3 |
| AC1.8 | `test_ltm_multiple_smooth_instances_compile`; `test_multiple_smooth_instances` (#[ignore]) | 1.4 |
| AC2.1 | 10 non-module tests in simulate_ltm.rs | Phase 2, 4 |
| AC2.2 | 1 active + 6 #[ignore] | Phase 3, 4 |
| AC2.3 | `simulates_population_ltm` | 2.5 |
| AC2.4 | Codebase search | 5.1 |
| AC3.1 | Codebase search | 5.1 |
| AC3.2 | Codebase search | 5.2, 5.3 |
| AC3.3 | `cargo build --workspace` | 5.5 |
| AC3.4 | Codebase search | 5.4 |
| AC4.1 | `test_user_defined_module_ltm_vm` | 3.1 |
| AC4.2 | `test_nested_module_ltm_vm` | 3.2 |
| AC4.3 | `test_nested_module_ltm_vm` (chained interpunct) | 3.2 |
| AC4.4 | `test_model_ltm_variables_stdlib_module`; `test_discovery_submodel_link_scores_excluded_from_search` (#[ignore]) | 1.5 |
