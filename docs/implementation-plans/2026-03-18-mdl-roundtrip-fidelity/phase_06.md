# MDL Roundtrip Fidelity Implementation Plan

**Goal:** Improve MDL writer fidelity so Vensim .mdl files roundtrip through Simlin with format preserved.

**Architecture:** Two-layer approach: (1) enrich datamodel with Vensim-specific metadata at parse time, (2) enhance writer to consume that metadata. Changes span datamodel.rs, protobuf schema, serde.rs, MDL parser, and MDL writer.

**Tech Stack:** Rust, protobuf (prost), cargo

**Scope:** 6 phases from original design (phases 1-6)

**Codebase verified:** 2026-03-18

---

## Acceptance Criteria Coverage

This phase adds the comprehensive integration test:

### mdl-roundtrip-fidelity.AC5: Test coverage
- **mdl-roundtrip-fidelity.AC5.1 Success:** `mdl_roundtrip` test is registered in Cargo.toml and runs with `cargo test`
- **mdl-roundtrip-fidelity.AC5.2 Success:** Format test roundtrips mark2.mdl and asserts per-view element lines match as unordered sets (with only documented normalizations)
- **mdl-roundtrip-fidelity.AC5.3 Success:** Existing roundtrip and simulation tests continue to pass

Also provides end-to-end verification for all previously implemented ACs:

### mdl-roundtrip-fidelity.AC1: Multi-view MDL output
- **mdl-roundtrip-fidelity.AC1.1 Success:** mark2.mdl roundtrip produces exactly 2 views with names `*1 housing` and `*2 investments`
- **mdl-roundtrip-fidelity.AC1.2 Success:** Each view contains the correct elements -- every element line from the original view appears in the corresponding output view (unordered set comparison)
- **mdl-roundtrip-fidelity.AC1.4 Success:** Each view has its own font specification line matching the original

### mdl-roundtrip-fidelity.AC3: Lookup fidelity
- **mdl-roundtrip-fidelity.AC3.1 Success:** Lookup invocations emit as `table_name ( input )` not `LOOKUP(table_name, input)`
- **mdl-roundtrip-fidelity.AC3.2 Success:** Explicit lookup range bounds are preserved

### mdl-roundtrip-fidelity.AC4: Equation formatting
- **mdl-roundtrip-fidelity.AC4.1 Success:** Short equations use inline format
- **mdl-roundtrip-fidelity.AC4.3 Success:** Variable name casing on equation LHS matches original

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->

<!-- START_TASK_1 -->
### Task 1: Verify mdl_roundtrip test registration

**Verifies:** mdl-roundtrip-fidelity.AC5.1

**Files:**
- Read: `src/simlin-engine/Cargo.toml`

**Implementation:**

The `mdl_roundtrip` test is already registered in `Cargo.toml` as a `[[test]]` entry (confirmed by codebase investigation). The test file is at `src/simlin-engine/tests/mdl_roundtrip.rs`.

No changes needed for registration. The new test function will be added to the existing test file.

**Verification:**

```bash
cargo test -p simlin-engine --test mdl_roundtrip -- --list
```

Should list the existing test functions. AC5.1 is already satisfied.

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Write format roundtrip test for mark2.mdl

**Verifies:** mdl-roundtrip-fidelity.AC5.2, mdl-roundtrip-fidelity.AC1.1, mdl-roundtrip-fidelity.AC1.2, mdl-roundtrip-fidelity.AC1.4, mdl-roundtrip-fidelity.AC3.1, mdl-roundtrip-fidelity.AC3.2, mdl-roundtrip-fidelity.AC4.1, mdl-roundtrip-fidelity.AC4.3

**Files:**
- Modify: `src/simlin-engine/tests/mdl_roundtrip.rs`

**Implementation:**

Add a new test function `mdl_format_roundtrip` (or similar) that:

1. Reads `test/bobby/vdf/econ/mark2.mdl` using `resolve_path("test/bobby/vdf/econ/mark2.mdl")`
2. Parses via `mdl::parse_mdl(&source)`
3. Writes back via `mdl::project_to_mdl(&project)`
4. Parses the original and output into comparable structures
5. Asserts structural equivalence

**Assertions to make:**

**View structure (AC1.1):**
- Split output on view header pattern (`*N name`) -- assert exactly 2 views
- Assert view names are `1 housing` and `2 investments`

**Per-view element matching (AC1.2):**
- Extract element lines from each view section (lines between view header and next header/terminator)
- Parse each element line into its fields
- Compare as unordered sets against the original mark2.mdl view sections
- Document any normalization applied (e.g., floating-point rounding of x/y coordinates)

**Font lines (AC1.4):**
- Assert each view section contains the font line matching `Verdana|10`

**Equation section spot-checks:**
- AC3.1: Assert the output contains `federal funds rate lookup ( Time )` (or equivalent lookup call from mark2.mdl) and does NOT contain `LOOKUP(`
- AC3.2: Assert lookup definitions preserve explicit bounds (e.g., `[(0,0)-(300,10)]` not recomputed)
- AC4.1: Assert at least one short equation uses inline format (e.g., `average repayment rate = 0.03`)
- AC4.3: Assert at least one variable has original casing (search for capitalized variable name from mark2.mdl)

**Testing approach:**
Follow the existing test pattern in mdl_roundtrip.rs: collect failures into a `Vec<String>` and panic at the end with all failures. Use helper functions for parsing views and extracting element lines.

**Verification:**

```bash
cargo test -p simlin-engine --test mdl_roundtrip mdl_format_roundtrip
```

Test passes with all assertions.

**Commit:** `engine: add MDL format roundtrip test for mark2.mdl`

<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Verify all tests pass

**Verifies:** mdl-roundtrip-fidelity.AC5.3

**Verification:**

```bash
cargo test -p simlin-engine
```

All existing roundtrip tests (`mdl_to_mdl_roundtrip`, `xmile_to_mdl_roundtrip`) and simulation tests continue to pass alongside the new format test.

If any existing tests fail due to the formatting changes (e.g., the semantic roundtrip now produces differently-formatted but semantically equivalent MDL), investigate and fix:
- If the semantic equivalence check fails because of casing differences in equations: the semantic comparison should already normalize casing. If not, update the comparison.
- If existing tests expected specific MDL formatting patterns: update the expected patterns to match the new formatting.

**Commit:** `engine: verify all MDL roundtrip tests pass with format improvements`

<!-- END_TASK_3 -->

<!-- END_SUBCOMPONENT_A -->
