# VM Vector Operations - Human Test Plan

## Automated Test Summary

All acceptance criteria have automated test coverage:

| Criterion | Test Location | Status |
|-----------|--------------|--------|
| AC1.1 VSSUM | `simulate.rs::simulates_vector_simple_mdl` + `vector_simple.dat` | PASS |
| AC1.2 VSMIN/MEAN/MAX/PROD | `simulate.rs::simulates_vector_simple_mdl` + `vector_simple.dat` | PASS |
| AC1.3 Empty selection | `simulate.rs::simulates_vector_simple_mdl` + `vector_simple.dat` (vs_empty=9999) | PASS |
| AC1.4 VectorElmMap basic | `simulate.rs` + `compiler_vector.rs` + `array_tests.rs` | PASS |
| AC1.5 VectorElmMap OOB | `array_tests.rs::vector_elm_map_tests` (6 tests) | PASS |
| AC1.6 VectorSortOrder asc | `simulate.rs` + `vector_simple.dat` + `compiler_vector.rs` | PASS |
| AC1.7 VectorSortOrder desc | `simulate.rs` + `vector_simple.dat` + `compiler_vector.rs` | PASS |
| AC1.8 AllocateAvailable | `simulate.rs::simulates_allocate_*` + `compiler_vector.rs` | PASS |
| AC2.1 SmallVec | Code inspection (all scratch buffers use SmallVec<[T; 32]>) | PASS |
| AC2.2 Pre-allocated temp | Code inspection (write_temp_id -> temp_offsets) | PASS |
| AC3.1-AC3.3 VM+interp parity | `simulate.rs` (3 tests upgraded from interpreter-only) | PASS |
| AC3.4 .dat validation | `vector_simple.dat` includes f, g, l, m computed values | PASS |
| AC4.1-AC4.3 A2A hoisting | `compiler_vector.rs` (9 tests: structural + correctness) | PASS |

## Manual Verification Checklist

- [ ] Run `cargo test -p simlin-engine` and confirm 2417+ tests pass
- [ ] Run `cargo test --features file_io --test simulate` and confirm 48 tests pass, 0 fail
- [ ] Run `cargo test -p simlin-engine --test compiler_vector` and confirm 9 tests pass
- [ ] Spot-check `vector_simple.dat` values against hand computations in `phase_06.md`
- [ ] Verify no `TodoArrayBuiltin` errors remain for VectorSelect, VectorElmMap, VectorSortOrder, or AllocateAvailable
- [ ] Confirm `alloc.rs` functions are `pub` and imported by both `interpreter.rs` and `vm.rs`
