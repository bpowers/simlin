# C-LEARN Residual — Phase 5: Native external-data provider wiring (forward-looking; cuttable)

**Goal:** Native (filesystem-capable) targets resolve Vensim `GET DIRECT *` external-data functions, so large Vensim models that depend on external CSV data import and simulate with real values via the CLI.

**Architecture:** The entire data-provider stack already exists and is wired end-to-end through `open_vensim_with_data` (including a libsimlin FFI and ~7 passing engine tests). The only gap is `simlin-cli::open_model`, which calls the plain `open_vensim(&contents)` and drops the model's parent directory, so a `GET DIRECT` model fails to open from the CLI. The fix builds a `FilesystemDataProvider` rooted at the model file's parent directory and passes it via `open_vensim_with_data`. This is a ~5-line change at one site plus a fixture and tests.

**Scope:** Phase 5 of 5 from `docs/design-plans/2026-05-19-clearn-residual.md`. **Forward-looking and cleanly cuttable: C-LEARN needs NO external data (all its lookup data is inline), so this phase does not affect residual closure (Phases 1-4) and may be dropped without consequence.** It serves other large Vensim models.

**Tech Stack:** Rust (`simlin-cli`, `simlin-engine`). `simlin-cli` already enables the `file_io` feature.

**Codebase verified:** 2026-05-20 (branch `clearn-residual`, off `main`@`2ed93950`).

---

## Acceptance Criteria Coverage

This phase implements and tests:

### clearn-residual.AC5: Native targets resolve external-data functions (forward-looking)
- **clearn-residual.AC5.1 Success:** On a native (`file_io`) build, a model using `GET DIRECT DATA` (or `GET DIRECT LOOKUPS`/`CONSTANTS`) imported via the CLI resolves values from a companion file located relative to the model path.
- **clearn-residual.AC5.2 Success:** The resolved external data drives simulation (the fixture asserts a downstream value that reflects the external data, not a zeroed series).
- **clearn-residual.AC5.3 Edge:** A missing or unreadable data file produces a clear diagnostic rather than a silent `0+0` zeroing.
- **clearn-residual.AC5.4 Constraint:** WASM / libsimlin builds remain on the null provider (behavior unchanged); a tracked follow-up issue captures their data-supply API.

---

## Verified ground truth (read before starting)

Confirmed by investigation on 2026-05-20.

- **`DataProvider` trait**: `src/simlin-engine/src/data_provider/mod.rs:16-75` (NOT feature-gated; `NullDataProvider` too). **`FilesystemDataProvider`**: `src/simlin-engine/src/data_provider/csv_provider.rs:16-547` (`#[cfg(feature = "file_io")]`); `FilesystemDataProvider::new(base_dir: impl Into<PathBuf>)` takes the root directory and sandboxes paths (rejects absolute/`..`, `canonicalize`s — a missing file returns an error, not a zero).
- **ONLY `GET DIRECT *` is wired**: `is_get_direct_ref` (`mdl/convert/external_data.rs:448-451`) and `parse_get_direct` (`:63-136`) recognize only `GET DIRECT DATA|CONSTANTS|LOOKUPS|SUBSCRIPT`. `GET XLS`, `GET DATA`, `GET 123`, `GET VDF` are normalized but NOT resolved (they pass through and fail compilation). **The fixture must use `GET DIRECT *` only.**
- **CSV always (`file_io`); Excel needs `ext_data` (calamine), which `simlin-cli` does NOT enable** (`simlin-cli/Cargo.toml:16` enables only `file_io`). **The fixture must be CSV-only** (a `.xls*` file would error with a "requires the 'ext_data' feature" message).
- **`open_vensim_with_data`**: `src/simlin-engine/src/compat.rs:63-74`. `open_vensim(contents)` is literally `open_vensim_with_data(contents, None)`. `open_vensim_with_data` is NOT feature-gated (only `FilesystemDataProvider` is).
- **The change site**: `src/simlin-cli/src/main.rs::open_model` (`:176-216`), `InputFormat::Vensim` arm calls `open_vensim(&contents)` at `:187`. `file_path` is computed at `:178-182`; `simlin-cli/Cargo.toml:16` already enables `file_io`. **Guard the stdin case**: `file_path` falls back to `/dev/stdin` (parent `/dev`), so only build a `FilesystemDataProvider` when the input is a real file; otherwise pass `None`.
- **No-provider behavior today is a HARD ERROR** (not a silent zero): `try_resolve_data_expr` (`external_data.rs:471-490`) returns `Err("external data file '...' referenced but no DataProvider configured")` → `open_vensim` returns `Err` → the CLI `die!`s. So the CLI currently CANNOT open a `GET DIRECT` model. (The design's AC5.3 "silent 0+0 zeroing" framing is inaccurate for `GET DIRECT *`; the genuine requirement is that a missing/unreadable *file* yields a CLEAR file-level diagnostic, which `FilesystemDataProvider` provides.)
- **Ready template**: `tests/simulate.rs::simulate_mdl_path_with_data` (`:786-812`, `#[cfg(feature = "file_io")]`) does exactly `FilesystemDataProvider::new(mdl_abs.parent()) → open_vensim_with_data(&contents, Some(&provider))` — the CLI change mirrors this. Inline-tempfile examples: `simulates_get_direct_data_scalar_csv` (`:2785`), `simulates_get_direct_constants_scalar_csv` (`:2848`), `simulates_get_direct_lookups_scalar_csv` (`:2888`).
- **Existing CSV fixtures** (passing, reusable): `test/sdeverywhere/models/directconst/` (model + `data/*.csv`, relative-subdir resolution), `test/sdeverywhere/models/directdata/` (`*.csv` + `.dat` reference), `directlookups/`, `directsubs/`. (Avoid `directdata`'s `.xlsx` path; use the CSV references.)
- **libsimlin already has the data FFI**: `simlin_project_open_vensim_with_data` (`src/libsimlin/src/project.rs:577`) builds a `FilesystemDataProvider` from a `data_dir` under `file_io`. So the AC5.4 follow-up is about WASM *callers* supplying data (the data-supply surface), not new FFI. Other native callers using the plain `open_vensim` (out of scope here): `pysimlin`, `simlin-serve` (`parse.rs:59`), `simlin-mcp-core` (`open.rs:129`).

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->

<!-- START_TASK_1 -->
### Task 1: Add a CLI external-data fixture test (RED)

**Verifies:** clearn-residual.AC5.1, clearn-residual.AC5.2

**Harness decision (resolve the fork before writing the test):** Use the **extracted-testable-function** approach as the primary, because it does not depend on the CLI's stdout format and gives a precise assertion. In **Task 1** you extract `open_model`'s `InputFormat::Vensim` arm into a small function `open_vensim_model(path: &Path, contents: &str) -> Result<datamodel::Project, ...>` that keeps the CURRENT null-provider behavior (`open_vensim(contents)`); in **Task 2** you change only that function's body to build the provider from the path's parent and call `open_vensim_with_data`. The Task-1 test calls `open_vensim_model` on the fixture, compiles + runs the returned project (mirror `tests/simulate.rs`'s compile/run helpers), and asserts the data-driven variable's series. Because the extraction lands in Task 1 with current behavior, the test **compiles and fails on behavior** (`open_vensim_model` returns `Err("... no DataProvider configured")`) until Task 2 wires the provider — a behavior-RED, not a compile error. (A `std::process::Command` CLI integration test is an acceptable alternative ONLY if extraction is undesirable; do not do both.)

**Fixture decision:** Use the existing `test/sdeverywhere/models/directconst/directconst.mdl` (+ its `data/*.csv`) as the primary fixture. Its `GET DIRECT CONSTANTS` references use the relative subdir `data/a.csv` (verified), so a `FilesystemDataProvider` rooted at the model directory resolves them. First confirm at kickoff that opening it needs no sdeverywhere spec file (the `*_spec.json` files are for the sdeverywhere toolchain, not the MDL importer). **Fallback (only if directconst needs a spec or an absolute path):** add a tiny purpose-built fixture under `test/test-models/` — a minimal `.mdl` with one `GET DIRECT CONSTANTS('vals.csv', ',', 'A', 'B')` reference + a `vals.csv`, plus a downstream variable that multiplies the constant — fully controlled and unambiguous.

**Files:**
- Modify: `src/simlin-cli/src/main.rs` — extract the `InputFormat::Vensim` arm's body into a small function `fn open_vensim_model(path: &Path, contents: &str) -> Result<datamodel::Project, ...>` that, IN THIS TASK, keeps the CURRENT behavior (`open_vensim(contents)`, null provider); have `open_model` call it. This makes the RED a behavior failure, not a compile error, and gives Task 2 a single body to change.
- Fixture: `test/sdeverywhere/models/directconst/` (reuse; primary) or a tiny new fixture under `test/test-models/` (fallback per the decision above).
- Test: `src/simlin-cli/tests/external_data.rs` (new integration test) calling the extracted `open_vensim_model`.

**Implementation:**
Extract `open_vensim_model` (current null-provider behavior) and write the RED test that calls it on the fixture, compiles + runs the returned project (mirror `tests/simulate.rs`'s compile/run helpers), and asserts the data-driven variable equals the CSV-derived value (tie the expected number to the actual CSV contents), NOT zero/NaN.

**Testing:**
- AC5.1/AC5.2: the data-driven variable's value matches the external CSV (not zeroed).

**Verification:**
Run: `cargo test -p simlin-cli external_data`
Expected: **FAILS** (RED) — `open_vensim_model` still uses the null provider, so it returns `Err("... no DataProvider configured")` (the test compiles and fails on behavior).

**Commit:** `cli: extract open_vensim_model and add failing GET DIRECT test`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Wire FilesystemDataProvider into the CLI (GREEN)

**Verifies:** clearn-residual.AC5.1, clearn-residual.AC5.2, clearn-residual.AC5.3

**Files:**
- Modify: `src/simlin-cli/src/main.rs` — imports (`:25`, add `open_vensim_with_data` and `FilesystemDataProvider`) and the body of the `open_vensim_model` function extracted in Task 1.

**Implementation:**
Change `open_vensim_model`'s body: when the input is a real file (not stdin), build `FilesystemDataProvider::new(<model file's parent directory>)` and call `open_vensim_with_data(contents, Some(&provider))`; otherwise (stdin) keep `open_vensim(contents)` (equivalently `open_vensim_with_data(contents, None)`). Mirror `simulate_mdl_path_with_data` (`tests/simulate.rs:786-812`). Guard the stdin/`/dev/stdin` case so its parent (`/dev`) is never used as a data root.

**Testing:**
- Task 1's test passes (GREEN): the CLI resolves the external CSV and simulates the data-driven value.
- AC5.3: add a test where the model references a NONEXISTENT CSV; assert the CLI emits a clear file-level diagnostic (the `FilesystemDataProvider` canonicalize/`not found` error surfaced through the CLI's error path), NOT a silent zero or a generic message.

**Verification:**
Run: `cargo test -p simlin-cli`
Expected: Task 1 test GREEN; the missing-file test asserts a clear diagnostic.
Run: `cargo build -p simlin-cli` and a manual `simlin-cli` invocation on the fixture (optional smoke).

**Commit:** `cli: resolve GET DIRECT external data via filesystem provider`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Confirm WASM/libsimlin unchanged and file the follow-up

**Verifies:** clearn-residual.AC5.4

**Files:**
- Verify only (no code change): WASM/libsimlin default callers still use the null provider (the plain `open_vensim`/`simlin_project_open_vensim` path). Confirm no behavior change there.
- File a tracked follow-up issue.

**Implementation:**
Confirm that this phase touched only `simlin-cli` (and a fixture/test): the WASM and libsimlin default paths are unchanged (libsimlin already exposes `simlin_project_open_vensim_with_data` for the data case; the no-data FFI and WASM remain on the null provider). Then file a tracked follow-up issue capturing the data-supply API for WASM callers (the FFI surface for supplying external data to browser/WASM consumers — the right design is its own question, explicitly out of scope here). Use the `track-issue` agent (the repo's convention) so duplicates are checked against GitHub issues and `docs/tech-debt.md`.

**Testing:**
- AC5.4: a grep/confirmation that no WASM/libsimlin default-path behavior changed; the follow-up issue exists.

**Verification:**
Run: `cargo test -p simlin-engine` and `cargo build` for the WASM target per repo conventions (or confirm the libsimlin/WASM open paths are untouched by this phase's diff).
Expected: unchanged behavior; follow-up issue filed.

**Commit:** `cli: note WASM/libsimlin data-supply follow-up` (or no commit if only an issue is filed — record the issue link in the phase notes).
<!-- END_TASK_3 -->

<!-- END_SUBCOMPONENT_A -->

---

## Phase completion criteria

- The CLI fixture imports and simulates with data resolved from the filesystem on a native build (AC5.1/AC5.2); a missing data file yields a clear diagnostic (AC5.3).
- WASM/libsimlin default paths remain on the null provider (AC5.4); the follow-up issue for the WASM/libsimlin data-supply API is filed.
- `cargo test -p simlin-cli` and `cargo test -p simlin-engine` are green.

## Cuttability

This phase is independent of Phases 1-4 and of C-LEARN residual closure. If time/scope requires, it can be dropped entirely without affecting the C-LEARN gate or any AC1-AC4 deliverable.

## No special-casing (hard constraint)

The fixture is a small `GET DIRECT *` CSV model independent of C-LEARN; the wiring is a general CLI change (any model with `GET DIRECT *` benefits). No code branches on a C-LEARN name/path.
