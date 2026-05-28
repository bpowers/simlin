# Loops That Matter on the wasm Backend (wasm-ltm) — Phase 2: Backend-agnostic analytic core + from-series FFI

**Goal:** The existing LTM analysis (causal links annotated with link scores; relative loop scores) runs unchanged over an `engine::Results` reconstructed from a wasm blob's result slab, behind two small new libsimlin FFI functions that share one Rust-side core with the existing VM-backed analyze functions.

**Architecture:** The LTM analytic math already lives in the engine crate (`ltm_post`, `db_analysis`) and is backend-agnostic — every consumer reaches the score series through `results.offsets.get(...)` + step-major striding. Phase 2 extracts the libsimlin *orchestration* (resolve structure under the db lock → drive the math over a `&Results`) into shared cores that both the VM FFI and the new from-series FFI call. The from-series functions rebuild a `Results` from `(slab, serialized WasmLayout)` (mirroring `Vm::into_results()`), reconstruct `SimSpecs` and the LTM loop snapshots from salsa exactly as `simlin_sim_new` does, then call those same cores. Because both backends funnel through one core, the analyses cannot diverge (the explicit anti-goal carried from #624).

**Tech Stack:** Rust, libsimlin FFI (malloc-and-return-buffer + byte-buffer-input conventions), salsa. Reuses engine `ltm_post` / `db_analysis` math. In-test blob execution via the DLR-FT interpreter (already a `libsimlin` dev-dependency, same pinned rev as Phase 1).

**Scope:** Phase 2 of 6.

**Codebase verified:** 2026-05-27

---

## Acceptance Criteria Coverage

This phase implements and tests:

### wasm-ltm.AC2: Analytic outputs match the VM within tolerance
- **wasm-ltm.AC2.1 Success:** `simlin_analyze_links_from_wasm_results` returns per-link scores equal to `simlin_analyze_get_links` (VM) for a scalar LTM model, within tolerance.
- **wasm-ltm.AC2.2 Success:** `simlin_analyze_rel_loop_score_from_wasm_results` equals `simlin_analyze_get_relative_loop_score` (VM) for each loop id, including subscripted ids.

### wasm-ltm.AC5: Engineering quality (cross-cutting)
- **wasm-ltm.AC5.1:** the VM and wasm analytic paths share one engine-level core; no analysis logic is reimplemented per-backend and none is reimplemented in TypeScript. The FFI grows by exactly the two `*_from_wasm_results` functions (no bulk/batch endpoint).

---

## Background: what exists today (verified 2026-05-27)

**The two VM analyze FFI functions to refactor (`src/libsimlin/src/analysis.rs`):**
- `simlin_analyze_get_links` (`:186-342`). Acquisition `:191-225`: `require_sim` → `sim_ref`; `model_ref = &*sim_ref.model`; locks `(*model_ref.project).db` + `.sync_state`; resolves `synced_model`. Structure (`:227-231`): `engine::db::model_causal_edges(&*db, synced.source, sync.project)` and `engine::db::compute_link_polarities(&*db, synced.source, sync.project)`. `unique_links` map built `:234-242`, then locks dropped `:246`. Score loop `:255-341`: gated on `sim_ref.enable_ltm && state.results.is_some()`; for each `(from,to)`, builds `format!("$\u{205A}ltm\u{205A}link_score\u{205A}{from}\u{2192}{to}")` (`:289-293`), canonicalizes, `results.offsets.get(...)` (`:298`), strides `for row in results.iter() { scores.push(row[offset]); }` (`:299-302`). Builds `SimlinLink` and returns `SimlinLinks` (`:324-341`).
- `simlin_analyze_get_relative_loop_score` (`:372-704`). Parses loop id (`:422-445`), locks `sim_ref.state` (`:460`), reads `state.loop_partitions` / `state.loop_element_index` / `state.results` / `state.cached_partition_denominators`, then computes via the FFI-local `ensure_denom_for_element` (`:589-622`, the exact `cache: &mut HashMap<(Option<usize>,usize), Vec<f64>>` shape) and engine helpers `compute_partition_denominator_for_element` / `compute_rel_loop_score_for_element` / argmax aggregation (`:615/640/678`).

**Salsa query signatures the shared links-core depends on (`src/simlin-engine/src/db_analysis.rs`):**
- `:970-975` — `#[salsa::tracked(returns(ref))] pub fn model_causal_edges(db: &dyn Db, model: SourceModel, project: SourceProject) -> CausalEdgesResult` — **returns a borrow** (`&CausalEdgesResult`) tied to the db lock.
- `:1627-1631` — `pub fn compute_link_polarities(db: &dyn Db, model: SourceModel, project: SourceProject) -> HashMap<(String, String), crate::ltm::LinkPolarity>` — **returns owned**, survives the lock drop.
- `CausalEdgesResult` (`:35-43`): `{ edges: HashMap<String, BTreeSet<String>>, stocks: BTreeSet<String>, dynamic_modules: HashMap<String, String> }` (canonical names).

**The engine math (already backend-agnostic — `src/simlin-engine/src/ltm_post.rs`):**
- `compute_partition_denominator_for_element<'a, I>(results: &Results, loop_id_slots: I, element_index: usize) -> Vec<f64>` where `I: IntoIterator<Item = (&'a str, usize)>` (`:434-459`).
- `compute_rel_loop_score_for_element(results: &Results, loop_id: &str, n_slots: usize, element_index: usize, denominator: &[f64]) -> Option<Vec<f64>>` (`:477-497`).
- Neither reads `results.specs` (verified): the link + rel-loop paths are pure striding.

**The snapshot reconstruction the from-series rel-loop path must mirror (`src/libsimlin/src/simulation.rs`):** `simlin_sim_new` at `:70` does `set_project_ltm_enabled(&mut db, source_project, enable_ltm)`, then `:84-89`:
```rust
let canonical = engine::canonicalize(&model_ref.model_name);
if let Some(sm) = sync.models.get(canonical.as_ref()) {
    let ltm_vars = engine::db::model_ltm_variables(&*db, sm.source, source_project);
    let project_dims = engine::db::project_datamodel_dims(&*db, source_project);
    let element_index = engine::ltm_post::build_loop_element_index(&ltm_vars.vars, project_dims);
    (ltm_vars.loop_partitions.clone(), element_index)
}
```
and resets the flag at `:136-138` (`set_project_ltm_enabled(..., false)`). **Skipping the `true` set yields empty `loop_partitions`; skipping the `false` reset leaks the flag into later ops sharing the same `SourceProject`.** Snapshot types: `loop_partitions: HashMap<String, Vec<Option<usize>>>` (`lib.rs:343`), `loop_element_index: HashMap<String, engine::ltm_post::LoopElementIndex>` (`lib.rs:352`).

**`SimSpecs` reconstruction (the from-series `Results` needs it; verified there is no single salsa query):** mirror `assemble_simulation` (`db.rs:5135-5142`):
```rust
let specs_dm = match source_model.model_sim_specs(db) {        // db.rs:5135, returns &Option<SourceSimSpecs>
    Some(ms) => engine::db::source_sim_specs_to_datamodel(ms),  // db.rs:591-592 (pub)
    None     => engine::db::source_sim_specs_to_datamodel(project.sim_specs(db)),  // db.rs:752
};
let specs: engine::SimSpecs = engine::vm::Specs::from(&specs_dm);  // engine::SimSpecs == results::Specs (lib.rs:133)
```

**`Vm::into_results()` (`vm.rs:897-906`) — the reconstruction template:** `Results { offsets: self.offsets.clone(), data: self.data.unwrap(), step_size: self.n_slots, step_count: self.n_chunks, specs: self.specs, is_vensim: false }`. `Results` fields are all `pub` (`results.rs:76-84`); `Results::iter()` is `data.chunks(step_size).take(step_count)` (`results.rs:170-172`); `TIME_OFF = 0` (`results.rs:10`).

**FFI conventions:**
- Byte-buffer **input** (host owns, callee `from_raw_parts` + copies): `simlin_project_open_protobuf` (`project.rs:42-79`, `data: *const u8, len: usize`).
- Variable-length **output**: `write_bytes_to_ffi_output` (`model.rs:65-86`); allocator/free in `memory.rs` (`simlin_malloc`, `simlin_free`).
- `SimlinLink` (`ffi.rs:99-105`) / `SimlinLinks` (`ffi.rs:109-112`); free via `simlin_free_links` (`analysis.rs:349-361`) → `drop_link` (`lib.rs:457-461`) → `drop_f64_array` (`lib.rs:445-450`). A from-series links result MUST allocate `score` arrays identically (`Box<[f64]>` → `as_mut_ptr` → `mem::forget`) so the existing `simlin_free_links` frees it correctly and TS `convertLinks` works unchanged.
- `SimlinModel` (`lib.rs:305-309`): `{ project: *const SimlinProject, model_name: Arc<String> }`. Reaches `db` + `sync_state` through `project`, and the model name — everything the from-series functions need; **no `SimlinSim` required.**
- `engine::canonicalize(name) -> Cow<str>` (`common.rs:364`, re-exported `lib.rs:127`).
- `WasmLayout::deserialize(bytes) -> Option<WasmLayout>` (`wasmgen/module.rs:218-253`); `var_offsets: Vec<(String, usize)>`, `n_slots`, `n_chunks`. The serialized `results_offset` is a *wasm linear-memory* byte offset, irrelevant to a host-extracted slab — the from-series API takes the slab **already extracted** (`n_chunks * n_slots` f64 starting at the blob's `results_offset`), not the whole memory image.

**Divergence from the design doc:** the design proposes one core `(db, model, source_project, results, snapshots, enable_ltm)`. Reality: `get_links` uses only structure + `Option<&Results>` (no snapshots); only `get_relative_loop_score` uses the snapshots. So this plan extracts **two** focused cores (one per analysis), each shared VM↔wasm. This satisfies AC5.1 ("one engine-level core ... no analysis logic reimplemented per-backend") more honestly than forcing both through one over-broad signature; document this in code comments.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Extract the shared links core; refactor `simlin_analyze_get_links` onto it

**Verifies:** wasm-ltm.AC5.1 (no behavior change — existing VM `get_links` tests must stay green)

**Files:**
- Modify: `src/libsimlin/src/analysis.rs` (extract a private core; rewire `simlin_analyze_get_links` `:186-342`)

**Implementation:**
1. Add a private `OwnedLink { from: String, to: String, polarity: engine::ltm::LinkPolarity, score: Option<Vec<f64>> }`.
2. Add `fn analyze_links_core(db: &dyn engine::db::Db, model: SourceModel, project: SourceProject, results: Option<&engine::Results>) -> Vec<OwnedLink>`:
   - Resolve `unique_links` from `model_causal_edges(db, model, project)` (dedupe edges → `(from,to)` owned Strings) and `polarities = compute_link_polarities(db, model, project)`.
   - For each `(from,to)`: build the link-score var name `format!("$\u{205A}ltm\u{205A}link_score\u{205A}{from}\u{2192}{to}")`, canonicalize, and `results.and_then(|r| r.offsets.get(&canonical).map(|&off| r.iter().map(|row| row[off]).collect::<Vec<f64>>()))` → `score`. Absent column (or `results` is `None`) ⇒ `score: None`. Polarity from `polarities.get(&(from,to))`.
   - Because `model_causal_edges` borrows the db, materialize `unique_links` into owned Strings *before* using `results` (the caller already drops the locks after this returns; the core itself only needs `db` while resolving structure).
3. Add `fn owned_links_to_ffi(links: Vec<OwnedLink>) -> *mut SimlinLinks` doing the existing `:324-341` malloc/`mem::forget`/`Box::into_raw` dance (score arrays via `Box<[f64]>` so `simlin_free_links` frees them).
4. Rewrite `simlin_analyze_get_links`: acquire `sim` → lock db + state → compute `results = if sim_ref.enable_ltm { state.results.as_ref() } else { None }` → call `analyze_links_core(&*db, synced.source, sync.project, results)` → `owned_links_to_ffi(...)`. Preserve the empty-`SimlinLinks` path (`:248-253`).

**Testing:** Behavior is unchanged; covered by existing VM `get_links` tests (e.g. `simlin-engine`/`libsimlin` LTM tests and the engine's `api.test.ts:599-610`). Add a focused libsimlin unit test only if one does not already assert VM `get_links` scores for a scalar LTM model.

**Verification:**
Run: `cargo test -p libsimlin analyze`
Expected: existing `get_links` tests pass; no behavior change.

**Commit:** `libsimlin: extract shared links core from simlin_analyze_get_links`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Extract the shared rel-loop-score core; refactor `simlin_analyze_get_relative_loop_score` onto it

**Verifies:** wasm-ltm.AC5.1 (no behavior change)

**Files:**
- Modify: `src/libsimlin/src/analysis.rs` (extract a private core; rewire `simlin_analyze_get_relative_loop_score` `:372-704`)

**Implementation:**
1. Add `fn rel_loop_score_series(results: &engine::Results, loop_partitions: &HashMap<String, Vec<Option<usize>>>, loop_element_index: &HashMap<String, engine::ltm_post::LoopElementIndex>, cache: &mut HashMap<(Option<usize>, usize), Vec<f64>>, loop_id: &str) -> Option<Vec<f64>>` containing the per-element denominator + `compute_rel_loop_score_for_element` + argmax-abs aggregation currently inline at `:589-697` (including the `ensure_denom_for_element` cache logic at `:589-622`).
2. Rewrite `simlin_analyze_get_relative_loop_score` to: parse the loop id (`:422-445`, unchanged) → lock `state` → call `rel_loop_score_series(&state.results.as_ref()?, &state.loop_partitions, &state.loop_element_index, &mut state.cached_partition_denominators, &loop_id)` (split-borrow `&mut state` as today) → copy the result into the out buffer (unchanged `:640-704`).

**Testing:** Behavior unchanged; covered by existing VM rel-loop-score tests (find them in `libsimlin`/`simlin-engine`; e.g. `tests/simulate_ltm.rs` rel-loop assertions and any libsimlin analyze tests). Add a focused libsimlin unit test if VM rel-loop-score is not already asserted for a subscripted loop id.

**Verification:**
Run: `cargo test -p libsimlin analyze && cargo test -p simlin-engine --features file_io --test simulate_ltm`
Expected: rel-loop-score tests pass unchanged.

**Commit:** `libsimlin: extract shared relative-loop-score core`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-5) -->

<!-- START_TASK_3 -->
### Task 3: Shared `Results`-from-slab and LTM-snapshot reconstruction helpers

**Verifies:** (test-support for AC2.1, AC2.2)

**Files:**
- Modify: `src/libsimlin/src/analysis.rs` (or a small new private `src/libsimlin/src/from_wasm.rs` module included from `analysis.rs`)

**Implementation:**
1. `fn results_from_layout_and_slab(db: &dyn engine::db::Db, model: SourceModel, project: SourceProject, layout: &engine::wasmgen::WasmLayout, slab: &[f64]) -> Result<engine::Results, SimlinError>`:
   - Validate `slab.len() == layout.n_chunks * layout.n_slots`; on mismatch return a `SimlinErrorCode::Generic` error.
   - `offsets`: `layout.var_offsets.iter().map(|(name, off)| (Ident::<Canonical>::from(engine::canonicalize(name).as_ref()), *off)).collect()`.
   - `data: slab.to_vec().into_boxed_slice()`, `step_size: layout.n_slots`, `step_count: layout.n_chunks`, `is_vensim: false`.
   - `specs`: rebuild via the salsa recipe in Background (`model_sim_specs` else `project.sim_specs` → `source_sim_specs_to_datamodel` → `Specs::from`).
2. `fn recompute_ltm_snapshots(db: &mut engine::db::SimlinDb, project: SourceProject, model: SourceModel, model_name: &str) -> (HashMap<String, Vec<Option<usize>>>, HashMap<String, engine::ltm_post::LoopElementIndex>)`:
   - `set_project_ltm_enabled(db, project, true)`; run the `:84-89` queries (`model_ltm_variables` + `project_datamodel_dims` + `build_loop_element_index`); **always** `set_project_ltm_enabled(db, project, false)` before returning (use a scope guard or a clear early-`let` so an early return cannot skip the reset). Document the reset's *why* (shared `SourceProject`).

**Testing:** exercised by Tasks 4-5.

**Verification:**
Run: `cargo build -p libsimlin`
Expected: compiles.

**Commit:** `libsimlin: add Results-from-slab and LTM-snapshot reconstruction helpers`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: `simlin_analyze_links_from_wasm_results` + FFI links parity test

**Verifies:** wasm-ltm.AC2.1

**Files:**
- Modify: `src/libsimlin/src/analysis.rs` (new FFI fn)
- Modify: the libsimlin C header (cbindgen — regen as in Phase 1 Task 2)
- Modify: `src/libsimlin/tests/wasm.rs` (parity test)

**Implementation:**
1. New FFI:
   ```rust
   #[unsafe(no_mangle)]
   pub unsafe extern "C" fn simlin_analyze_links_from_wasm_results(
       model: *mut SimlinModel,
       slab_ptr: *const u8, slab_len: usize,
       layout_ptr: *const u8, layout_len: usize,
       out_error: *mut *mut SimlinError,
   ) -> *mut SimlinLinks
   ```
   - `require_model(model)`; `from_raw_parts` the slab bytes → `&[f64]` (`slab_len` is bytes; reinterpret as `f64` with a length/alignment check, or accept `*const f64` + element count — pick one and document; protobuf-open uses `*const u8`, so keep `*const u8` + validate `slab_len % 8 == 0`).
   - `WasmLayout::deserialize(layout bytes)` → error on `None`.
   - Lock `(*model_ref.project).db` + `sync_state`; resolve `synced.source` + `sync.project`; `results_from_layout_and_slab(...)`; `analyze_links_core(&*db, synced.source, sync.project, Some(&results))`; `owned_links_to_ffi(...)`. Links structure is LTM-flag-independent, so **no** snapshot dance here.
2. Regenerate the C header.

**Testing (`src/libsimlin/tests/wasm.rs`):** `links_from_wasm_match_vm` — load a scalar LTM model (`test/logistic_growth_ltm/logistic_growth.stmx`) via the project-open path the existing `wasm.rs` tests use; (a) compile to wasm with `simlin_model_compile_to_wasm(model, /*ltm*/ true, false, ...)`, run the blob (DLR-FT, the `run_and_stride` pattern `:441-469`) into a `Vec<f64>` slab, call `simlin_analyze_links_from_wasm_results`; (b) VM oracle: `simlin_sim_new(model, /*enable_ltm*/ true)`, `run_to_end`, `simlin_analyze_get_links`. Assert same link set (`from,to,polarity`) and per-link `score` series equal within `1e-6`. Free both `SimlinLinks`.

**Verification:**
Run: `cargo test -p libsimlin --test wasm links_from_wasm_match_vm`
Expected: passes.

**Commit:** `libsimlin: add simlin_analyze_links_from_wasm_results`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: `simlin_analyze_rel_loop_score_from_wasm_results` + FFI rel-loop parity test

**Verifies:** wasm-ltm.AC2.2

**Files:**
- Modify: `src/libsimlin/src/analysis.rs` (new FFI fn)
- Modify: the libsimlin C header (cbindgen)
- Modify: `src/libsimlin/tests/wasm.rs` (parity test)

**Implementation:**
1. New FFI mirroring the out-parameter shape of `simlin_analyze_get_relative_loop_score`:
   ```rust
   #[unsafe(no_mangle)]
   pub unsafe extern "C" fn simlin_analyze_rel_loop_score_from_wasm_results(
       model: *mut SimlinModel,
       slab_ptr: *const u8, slab_len: usize,
       layout_ptr: *const u8, layout_len: usize,
       loop_id: *const c_char,
       results_ptr: *mut f64, len: usize, out_written: *mut usize,
       out_error: *mut *mut SimlinError,
   )
   ```
   - Deserialize layout; lock db (mutably) + sync_state; resolve `synced.source` + `sync.project`.
   - `recompute_ltm_snapshots(&mut db, project, source, model_name)` → `(loop_partitions, loop_element_index)`.
   - `results_from_layout_and_slab(...)`; `let mut cache = HashMap::new();` (throwaway — no persistent sim); `rel_loop_score_series(&results, &loop_partitions, &loop_element_index, &mut cache, loop_id_str)`.
   - Copy into `results_ptr`/`out_written` (clamp to `len`), matching the VM function's out-buffer semantics.
2. Regenerate the C header.

**Testing (`src/libsimlin/tests/wasm.rs`):** `rel_loop_score_from_wasm_matches_vm` — same model as Task 4; for each loop id (use `simlin_analyze_get_loops` to enumerate, **including a subscripted id** if the corpus has one), compute the rel-loop-score series both ways (from-wasm vs VM `simlin_analyze_get_relative_loop_score`) and assert equal within `1e-6`. (If no scalar corpus model has subscripted loop ids, defer the subscripted-id assertion to Phase 4's arrayed FFI parity and note it here.)

**Verification:**
Run: `cargo test -p libsimlin --test wasm rel_loop_score_from_wasm_matches_vm`
Then: `cargo test -p libsimlin`
Expected: passes; whole libsimlin suite green.

**Commit:** `libsimlin: add simlin_analyze_rel_loop_score_from_wasm_results`
<!-- END_TASK_5 -->

<!-- END_SUBCOMPONENT_B -->

---

## Phase 2 Done When

- `simlin_analyze_get_links` / `simlin_analyze_get_relative_loop_score` are refactored onto two shared cores; the from-series twins call the **same** cores; no analysis math is duplicated (**wasm-ltm.AC5.1**), and the FFI grew by exactly the two `*_from_wasm_results` functions.
- `simlin_analyze_links_from_wasm_results` per-link scores equal the VM `simlin_analyze_get_links` within `1e-6` for a scalar LTM model (**wasm-ltm.AC2.1**).
- `simlin_analyze_rel_loop_score_from_wasm_results` equals the VM `simlin_analyze_get_relative_loop_score` per loop id within `1e-6` (**wasm-ltm.AC2.2**); subscripted-id coverage is asserted here if the scalar corpus supports it, otherwise carried to Phase 4.
- `cargo test -p libsimlin` (incl. the new `wasm.rs` parity tests) and `cargo test --workspace` are green.
