# Human Test Plan: MDL Full Compatibility

## Prerequisites

- Development environment initialized: `./scripts/dev-init.sh`
- All automated tests passing

## Phase 1: Verify Automated Test Suite Runs Clean

| Step | Command | Expected |
|------|---------|----------|
| 1.1 | `cargo test -p simlin-engine` | All tests pass, zero failures |
| 1.2 | `cargo test --features file_io,ext_data --test simulate` | All non-ignored tests pass |
| 1.3 | `cargo test --features file_io,xmutil -p simlin-engine --test mdl_equivalence` | `test_mdl_equivalence` passes; `test_clearn_equivalence` is ignored |
| 1.4 | `cargo test -p simlin-engine --test mdl_roundtrip` | All 3 roundtrip tests pass |
| 1.5 | `cargo test -p simlin-engine --test json_roundtrip` | All JSON roundtrip tests pass |

## Phase 2: SDEverywhere Model Simulation (AC1)

| Step | Command | Expected |
|------|---------|----------|
| 2.1 | `cargo test --features file_io,ext_data --test simulate simulates_arrayed_models_correctly -- --nocapture` | Each model prints its name and passes |
| 2.2 | `cargo test --features file_io --test simulate simulates_quantum_mdl -- --nocapture` | quantum.mdl simulates via MDL path |
| 2.3 | `cargo test --features file_io --test simulate simulates_sample_mdl -- --nocapture` | sample.mdl simulates via MDL path |
| 2.4 | `cargo test --features file_io --test simulate simulates_npv_mdl -- --nocapture` | npv.mdl simulates via MDL path |

## Phase 3: External Data Pipeline (AC3)

| Step | Command | Expected |
|------|---------|----------|
| 3.1 | `cargo test -p simlin-engine data_provider -- --nocapture` | All CSV provider tests pass |
| 3.2 | `cargo test -p simlin-engine --features file_io,ext_data excel_provider -- --nocapture` | Excel provider tests pass |
| 3.3 | `cargo test --features file_io --test simulate simulates_get_direct_data_scalar_csv -- --nocapture` | GET DIRECT DATA from CSV simulates correctly |
| 3.4 | `cargo test --features file_io --test simulate simulates_get_direct_constants_scalar_csv -- --nocapture` | GET DIRECT CONSTANTS from CSV simulates correctly |

## Phase 4: EXCEPT Support (AC4)

| Step | Command | Expected |
|------|---------|----------|
| 4.1 | `cargo test -p simlin-engine convert::variables::tests::test_except -- --nocapture` | All 5 EXCEPT conversion tests pass |
| 4.2 | `cargo test --features file_io --test simulate simulates_except_basic_mdl -- --nocapture` | EXCEPT equations simulate correctly |
| 4.3 | `cargo test -p simlin-engine json::tests::test_arrayed_equation_with_default_equation_roundtrip -- --nocapture` | JSON roundtrip preserves default_equation |

## Phase 5: Serialization Guardrails (AC7.3)

| Step | Command | Expected |
|------|---------|----------|
| 5.1 | `cargo test -p simlin-engine serde::test_protobuf_rejects -- --nocapture` | All 3 protobuf rejection tests pass |
| 5.2 | `cargo test -p simlin-engine xmile::test_xmile_rejects -- --nocapture` | All 3 XMILE rejection tests pass |

## Phase 6: Builtins (AC5)

| Step | Command | Expected |
|------|---------|----------|
| 6.1 | `cargo test -p simlin-engine vm::tests::test_sshape -- --nocapture` | SSHAPE midpoint=50, endpoints near 0/100 |
| 6.2 | `cargo test -p simlin-engine vm::tests::test_quantum -- --nocapture` | QUANTUM truncation correct for negative inputs |
| 6.3 | `cargo test -p simlin-engine builtins_visitor::tests::test_npv -- --nocapture` | NPV accumulation and discounting correct |

## Phase 7: Vector Operations (AC5.4)

| Step | Command | Expected |
|------|---------|----------|
| 7.1 | `cargo test --features file_io --test simulate simulates_vector_simple_mdl -- --nocapture` | Vector simple model passes |
| 7.2 | `cargo test --features file_io --test simulate simulates_allocate -- --nocapture` | Allocate model passes |
| 7.3 | `cargo test -p simlin-engine array_tests::sum_of_conditional -- --nocapture` | SUM(IF...) pattern correct |

## End-to-End Scenarios

| Step | Command | Expected |
|------|---------|----------|
| E2E.1 | `cargo test -p simlin-engine --test mdl_roundtrip mdl_to_mdl_roundtrip -- --nocapture` | MDL roundtrip produces identical projects |
| E2E.2 | `cargo test -p simlin-engine --test mdl_roundtrip xmile_to_mdl_roundtrip -- --nocapture` | XMILE-to-MDL conversion produces equivalent structure |
| E2E.3 | `cargo test --features file_io --test simulate simulates_get_direct_data_scalar_csv -- --nocapture` | Full MDL+DataProvider pipeline works |
| E2E.4 | `cargo test --features file_io,xmutil -p simlin-engine --test mdl_equivalence test_mdl_equivalence -- --nocapture` | Native parser matches xmutil for all models |

## Known Deferred Items (Human Verification)

These tests are `#[ignore]` and expected to fail until the blocking features are implemented:

| Item | Command | Expected Result |
|------|---------|-----------------|
| C-LEARN simulation | `cargo test --features file_io --test simulate simulates_clearn -- --ignored --nocapture` | Fails with UnknownBuiltin (macro expansion not implemented) |
| DELAY FIXED | `cargo test --features file_io --test simulate simulates_delayfixed_mdl -- --ignored --nocapture` | Fails with tolerance mismatch (delay1 approximation) |
| GET DATA BETWEEN TIMES | `cargo test --features file_io --test simulate simulates_getdata_mdl -- --ignored --nocapture` | Fails with descriptive error (normalizer issue) |

## Traceability Matrix

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC1.1 | simulates_arrayed_models_correctly, test_mdl_equivalence | 2.1 |
| AC1.2 | simulates_arrayed_models_correctly | 2.1-2.4 |
| AC2.1 | simulates_clearn (#[ignore]) | Deferred |
| AC2.2 | simulates_clearn (#[ignore]) | Deferred |
| AC3.1-3.5 | data_provider tests, csv/excel provider tests | 3.1-3.4 |
| AC4.1-4.3 | json roundtrip, except conversion, except simulation | 4.1-4.3 |
| AC5.1 | sshape/quantum VM tests, quantum_mdl | 6.1-6.2, 2.2 |
| AC5.2 | sample/npv tests (DELAY FIXED deferred) | 2.3-2.4, 6.3 |
| AC5.3 | getdata tests (#[ignore]) | Deferred |
| AC5.4 | vector_simple, allocate, sum_of_conditional | 7.1-7.3 |
| AC6.1-6.2 | sparse array, Na/NAN, equation type handling | 1.1 |
| AC7.1-7.2 | json roundtrip tests | 4.3, 1.5 |
| AC7.3 | protobuf/xmile rejection tests | 5.1-5.2 |
