// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#ifndef SIMLIN_ENGINE2_H
#define SIMLIN_ENGINE2_H

// Generated with cbindgen. Do not modify by hand.

#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <stddef.h>
#include <stdint.h>

#define SIMLIN_VARTYPE_STOCK (1 << 0)

#define SIMLIN_VARTYPE_FLOW (1 << 1)

#define SIMLIN_VARTYPE_AUX (1 << 2)

#define SIMLIN_VARTYPE_MODULE (1 << 3)

// Loop polarity for C API
typedef enum {
  SIMLIN_LOOP_POLARITY_REINFORCING = 0,
  SIMLIN_LOOP_POLARITY_BALANCING = 1,
  SIMLIN_LOOP_POLARITY_UNDETERMINED = 2,
} SimlinLoopPolarity;

// Link polarity for C API
typedef enum {
  SIMLIN_LINK_POLARITY_POSITIVE = 0,
  SIMLIN_LINK_POLARITY_NEGATIVE = 1,
  SIMLIN_LINK_POLARITY_UNKNOWN = 2,
} SimlinLinkPolarity;

// The LTM loop-enumeration mode a simulation resolved to.
//
// `Disabled` means the simulation was created without LTM (`enable_ltm =
// false`), so no loop enumeration ran. `Exhaustive` means every elementary
// circuit was enumerated (Johnson). `Discovery` means the model tripped the
// SCC-size gate (or discovery was requested directly) and loops are ranked
// by the per-timestep strongest-path heuristic instead. Without this signal
// a caller cannot tell why an LTM-enabled run produced empty or different
// loop results.
typedef enum {
  SIMLIN_LTM_MODE_DISABLED = 0,
  SIMLIN_LTM_MODE_EXHAUSTIVE = 1,
  SIMLIN_LTM_MODE_DISCOVERY = 2,
} SimlinLtmMode;

// Error codes for the C API
typedef enum {
  // Success - no error
  SIMLIN_ERROR_CODE_NO_ERROR = 0,
  SIMLIN_ERROR_CODE_DOES_NOT_EXIST = 1,
  SIMLIN_ERROR_CODE_XML_DESERIALIZATION = 2,
  SIMLIN_ERROR_CODE_VENSIM_CONVERSION = 3,
  SIMLIN_ERROR_CODE_PROTOBUF_DECODE = 4,
  SIMLIN_ERROR_CODE_INVALID_TOKEN = 5,
  SIMLIN_ERROR_CODE_UNRECOGNIZED_EOF = 6,
  SIMLIN_ERROR_CODE_UNRECOGNIZED_TOKEN = 7,
  SIMLIN_ERROR_CODE_EXTRA_TOKEN = 8,
  SIMLIN_ERROR_CODE_UNCLOSED_COMMENT = 9,
  SIMLIN_ERROR_CODE_UNCLOSED_QUOTED_IDENT = 10,
  SIMLIN_ERROR_CODE_EXPECTED_NUMBER = 11,
  SIMLIN_ERROR_CODE_UNKNOWN_BUILTIN = 12,
  SIMLIN_ERROR_CODE_BAD_BUILTIN_ARGS = 13,
  SIMLIN_ERROR_CODE_EMPTY_EQUATION = 14,
  SIMLIN_ERROR_CODE_BAD_MODULE_INPUT_DST = 15,
  SIMLIN_ERROR_CODE_BAD_MODULE_INPUT_SRC = 16,
  SIMLIN_ERROR_CODE_NOT_SIMULATABLE = 17,
  SIMLIN_ERROR_CODE_BAD_TABLE = 18,
  SIMLIN_ERROR_CODE_BAD_SIM_SPECS = 19,
  SIMLIN_ERROR_CODE_NO_ABSOLUTE_REFERENCES = 20,
  SIMLIN_ERROR_CODE_CIRCULAR_DEPENDENCY = 21,
  SIMLIN_ERROR_CODE_ARRAYS_NOT_IMPLEMENTED = 22,
  SIMLIN_ERROR_CODE_MULTI_DIMENSIONAL_ARRAYS_NOT_IMPLEMENTED = 23,
  SIMLIN_ERROR_CODE_BAD_DIMENSION_NAME = 24,
  SIMLIN_ERROR_CODE_BAD_MODEL_NAME = 25,
  SIMLIN_ERROR_CODE_MISMATCHED_DIMENSIONS = 26,
  SIMLIN_ERROR_CODE_ARRAY_REFERENCE_NEEDS_EXPLICIT_SUBSCRIPTS = 27,
  SIMLIN_ERROR_CODE_DUPLICATE_VARIABLE = 28,
  SIMLIN_ERROR_CODE_UNKNOWN_DEPENDENCY = 29,
  SIMLIN_ERROR_CODE_VARIABLES_HAVE_ERRORS = 30,
  SIMLIN_ERROR_CODE_UNIT_DEFINITION_ERRORS = 31,
  SIMLIN_ERROR_CODE_GENERIC = 32,
  SIMLIN_ERROR_CODE_UNIT_MISMATCH = 33,
  SIMLIN_ERROR_CODE_BAD_OVERRIDE = 34,
} SimlinErrorCode;

// Error kind categorizing where in the project the error originates.
typedef enum {
  SIMLIN_ERROR_KIND_PROJECT = 0,
  SIMLIN_ERROR_KIND_MODEL = 1,
  SIMLIN_ERROR_KIND_VARIABLE = 2,
  SIMLIN_ERROR_KIND_UNITS = 3,
  SIMLIN_ERROR_KIND_SIMULATION = 4,
} SimlinErrorKind;

// Unit error kind for distinguishing types of unit-related errors.
typedef enum {
  // Not a unit error
  SIMLIN_UNIT_ERROR_KIND_NOT_APPLICABLE = 0,
  // Syntax error in unit string definition
  SIMLIN_UNIT_ERROR_KIND_DEFINITION = 1,
  // Dimensional analysis mismatch
  SIMLIN_UNIT_ERROR_KIND_CONSISTENCY = 2,
  // Inference error spanning multiple variables
  SIMLIN_UNIT_ERROR_KIND_INFERENCE = 3,
} SimlinUnitErrorKind;

// Severity of an error detail. Distinguishes hard errors (the model cannot be
// simulated, or a value is wrong) from advisory warnings (the model is still
// usable, e.g. the LTM auto-flip-to-discovery advisory). Defaults to `Error`
// so the common case (compile/parse/unit errors) keeps its meaning; the LTM
// diagnostic pipeline marks its advisories `Warning` so callers (pysimlin's
// `check()`, the TS engine) can present them without claiming the model is
// broken.
typedef enum {
  SIMLIN_ERROR_SEVERITY_ERROR = 0,
  SIMLIN_ERROR_SEVERITY_WARNING = 1,
} SimlinErrorSeverity;

// JSON format specifier for C API
typedef enum {
  SIMLIN_JSON_FORMAT_NATIVE = 0,
  SIMLIN_JSON_FORMAT_SDAI = 1,
} SimlinJsonFormat;

// A single feedback loop
typedef struct {
  char *id;
  char **variables;
  uintptr_t var_count;
  SimlinLoopPolarity polarity;
  // Human-meaningful loop name the modeler assigned via `SetLoopName`
  // (pysimlin `set_loop_name`), or NULL when the loop has no assigned
  // name.  The struct grew additively for this field (mirroring how
  // `SimlinLink` gained `relative_score`); `simlin_sizeof_loop` and the
  // `@simlin/engine` `LOOP_SIZE`/`readLoops` offsets track it.
  char *name;
} SimlinLoop;

// List of loops returned by analysis
typedef struct {
  SimlinLoop *loops;
  uintptr_t count;
} SimlinLoops;

// Opaque model structure
typedef struct {
  uint8_t _private[0];
} SimlinModel;

// Opaque error structure returned by the API
typedef struct {
  uint8_t _private[0];
} SimlinError;

// A single loop discovered via the strongest-path LTM discovery algorithm.
//
// This mirrors `SimlinLoop` but adds a per-timestep `importance` series.
// We do NOT reuse `SimlinLoop` (despite the score-on-loop suggestion in the
// task brief): `SimlinLoop` has no score field, and adding one would change
// its wasm32 layout (which `@simlin/engine` asserts against `simlin_sizeof_loop`).
// A separate struct keeps the discovery surface from disturbing the existing
// structural-loop ABI that TypeScript/Python read.
typedef struct {
  // Deterministic loop id (`r1`, `b1`, `u1`, ...).
  char *id;
  // Variable names around the loop, with the first variable repeated at the
  // end so the chain closes.  `var_count` entries.
  char **variables;
  uintptr_t var_count;
  SimlinLoopPolarity polarity;
  // Per-timestep |importance| series (length `importance_len`, matching the
  // analysis time array).  Owned `f64` buffer freed with the loop.
  double *importance;
  uintptr_t importance_len;
  // Human-meaningful loop name the modeler assigned via `SetLoopName`
  // (pysimlin `set_loop_name`), or NULL when the loop has no assigned
  // name.  Owned `c_char` buffer freed with the loop.
  char *name;
  // RESULT-SCOPED index into `SimlinDiscoveryResult.partitions` naming the
  // loop's cycle partition, or -1 for a loop whose stocks resolve to no
  // parent-level partition (a pure module-internal loop).  Indices are
  // dense, assigned in first-appearance order over the ranked loop list;
  // they identify partitions within ONE discovery result only and are not
  // stable across runs or model edits.
  int32_t partition;
} SimlinDiscoveredLoop;

// A time interval during which a specific set of loops dominates behavior.
typedef struct {
  // Start time of this period.
  double start;
  // End time of this period.
  double end;
  // Names of the dominant loops during this period (`dominant_loop_count`).
  char **dominant_loops;
  uintptr_t dominant_loop_count;
  // Combined relative score of the dominant loops.
  double combined_score;
} SimlinDominantPeriod;

// One cycle partition referenced by a discovery result's loops: a group of
// stocks connected by feedback, within which relative loop scores are
// normalized and therefore comparable.  Lets callers group/filter loops
// partition-by-partition (e.g. lead with the model's giant component).
typedef struct {
  // The partition's stock names (element-level for arrayed models),
  // sorted lexicographically.  `stock_count` entries.
  char **stocks;
  uintptr_t stock_count;
  // Number of loops in the returned loop list that belong to this
  // partition.
  uintptr_t loop_count;
} SimlinDiscoveredPartition;

// The cohesive output of one discovery run: discovered loops, dominant
// periods, and whether the time budget elapsed before discovery finished.
//
// Returning loops + periods + truncated together is a deliberate exception to
// libsimlin's "keep the FFI small/orthogonal, no bulk endpoints" rule: these
// three are the single result of ONE expensive analysis run, not a batch
// convenience.  Splitting them across separate FFIs would force the caller to
// re-run discovery (the costly part) once per output.
typedef struct {
  SimlinDiscoveredLoop *loops;
  uintptr_t loop_count;
  SimlinDominantPeriod *periods;
  uintptr_t period_count;
  // The cycle partitions referenced by `loops` (each loop's `partition`
  // indexes this array).  Dense, in first-appearance order over the
  // ranked loop list; result-scoped.
  SimlinDiscoveredPartition *partitions;
  uintptr_t partition_count;
  // Non-zero when discovery hit its `budget_ms` before finishing.
  bool truncated;
} SimlinDiscoveryResult;

// Single causal link structure
typedef struct {
  char *from;
  char *to;
  SimlinLinkPolarity polarity;
  // Raw LTM link-score series (length `score_len`), or NULL when LTM was
  // not enabled / the edge has no score column.  The raw score divides by
  // the change in `to`, so it is NOT comparable across different targets
  // and is unusable for ranking links globally -- use `relative_score`
  // (GH #652).
  double *score;
  uintptr_t score_len;
  // Relative LTM link-score series (length `relative_score_len`), or NULL
  // when `score` is NULL.  The raw score normalized, per target and per
  // timestep, against the sum of `|score|` over all of `to`'s scored
  // inputs -- a value in `[-1, 1]` that IS comparable across targets and
  // is the correct key for ranking links by importance (GH #652).  When
  // non-NULL its length equals `score_len`.
  double *relative_score;
  uintptr_t relative_score_len;
} SimlinLink;

// Collection of links
typedef struct {
  SimlinLink *links;
  uintptr_t count;
} SimlinLinks;

// Opaque simulation structure
typedef struct {
  uint8_t _private[0];
} SimlinSim;

// Error detail structure containing contextual information for failures.
typedef struct {
  SimlinErrorCode code;
  const char *message;
  const char *model_name;
  const char *variable_name;
  uint16_t start_offset;
  uint16_t end_offset;
  SimlinErrorKind kind;
  SimlinUnitErrorKind unit_error_kind;
  SimlinErrorSeverity severity;
} SimlinErrorDetail;

// Opaque project structure
typedef struct {
  uint8_t _private[0];
} SimlinProject;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

// Get the feedback loops detected in a model
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - The returned SimlinLoops must be freed with simlin_free_loops
SimlinLoops *simlin_analyze_get_loops(SimlinModel *model, SimlinError **out_error);

// Frees a SimlinLoops structure
//
// # Safety
// - `loops` must be a valid pointer returned by simlin_analyze_get_loops
void simlin_free_loops(SimlinLoops *loops);

// Run strongest-path LTM loop discovery on a model and return the discovered
// loops (with per-step importance series), the dominant periods, and a
// truncation flag, as one `SimlinDiscoveryResult`.
//
// `budget_ms` bounds the wall-clock time spent in discovery's per-timestep
// DFS sweep; `0` means unlimited.  When the budget elapses before discovery
// finishes, `truncated` is set and the returned loops/periods reflect only
// the timesteps processed so far.  Discovery on very large models can be
// infeasibly slow (GH #647), so the budget lets callers bound it.
//
// This deliberately returns loops + periods + truncated together rather than
// as three orthogonal FFIs (see the `SimlinDiscoveryResult` doc comment):
// they are the cohesive output of ONE expensive analysis run, not a batch
// convenience, so splitting them would force re-running discovery per output.
//
// # Safety
// - `model` must be a valid pointer to a `SimlinModel`.
// - The returned `SimlinDiscoveryResult` must be freed with
//   `simlin_free_discovery_result`.
SimlinDiscoveryResult *simlin_analyze_discover_loops(SimlinModel *model,
                                                     uint64_t budget_ms,
                                                     SimlinError **out_error);

// Frees a `SimlinDiscoveryResult` returned by `simlin_analyze_discover_loops`.
//
// # Safety
// - `result` must be a valid pointer returned by `simlin_analyze_discover_loops`
//   (or NULL, in which case this is a no-op).
void simlin_free_discovery_result(SimlinDiscoveryResult *result);

// Gets all causal links in a model
//
// Returns all causal links detected in the model.
// This includes flow-to-stock, stock-to-flow, and auxiliary-to-auxiliary links.
// If the simulation has been run with LTM enabled, link scores will be included.
//
// `include_internal` selects the view: when `false`, macro/module-internal
// synthetic nodes (`$⁚{var}⁚{n}⁚{func}`, `$⁚ltm⁚agg⁚{n}`, etc.) are collapsed
// out -- each chain `X -> internal -> Y` becomes one composite edge `X -> Y`
// carrying the composite (largest-magnitude path) link score, so the
// through-contribution is preserved (LTM ref 6.4).  When `true`, the raw
// causal graph (including every synthetic node) is returned.
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - The returned SimlinLinks must be freed with simlin_free_links
SimlinLinks *simlin_analyze_get_links(SimlinSim *sim,
                                      bool include_internal,
                                      SimlinError **out_error);

// Reports the LTM loop-enumeration mode the simulation resolved to.
//
// Returns `Disabled` when the sim was created with `enable_ltm = false` (or
// compilation failed before LTM could run), `Exhaustive` when every
// elementary circuit was enumerated, and `Discovery` when the model tripped
// the SCC-size gate (or discovery was requested directly) so loops are
// ranked by the per-timestep strongest-path heuristic.  The mode is captured
// at `simlin_sim_new` time, so it is available without running the
// simulation.
//
// On a NULL `sim` the function reports the error through `out_error` and
// returns `Disabled`.
//
// # Safety
// - `sim` must be a valid pointer to a `SimlinSim`.
SimlinLtmMode simlin_sim_get_ltm_mode(SimlinSim *sim, SimlinError **out_error);

// Gets all causal links in a model with LTM link-score series derived from
// a wasm-produced result slab.
//
// This is the wasm-backend twin of `simlin_analyze_get_links`: instead of
// reading the `Results` off a `SimlinSim`'s `SimState`, it rebuilds them
// from a `(slab, WasmLayout)` pair produced by running the blob returned
// from `simlin_model_compile_to_wasm(model, ltm_enabled=true, ..)`.  Both
// FFI functions funnel through the same `analyze_links_core` so the link
// set and per-link score series agree to within the underlying VM/wasm
// numeric tolerance.
//
// The slab is the host-extracted bytes starting at the blob's
// `results_offset` (the f64-array image of the results region, little-endian).
// Its byte length encodes how many rows the blob has actually written:
// `saved_steps * n_slots * 8`, where `saved_steps` is the live `G_SAVED`
// counter the blob exposes (which equals `n_chunks` after a full run but is
// 0 for a fresh or just-reset sim and `< n_chunks` mid-run via `run_to`).
// Passing the slab at its saved length -- not its `n_chunks * n_slots * 8`
// capacity -- keeps the analytic core from seeing uninit/stale tail rows
// and mirrors what `simlin_sim_get_series` already does on the VM side.
// The layout buffer is the bytes returned in `simlin_model_compile_to_wasm`'s
// `out_layout`.  Both buffers are owned by the caller and only read; this
// function copies them as needed.
//
// Because the links analysis is structure-driven (the unique `(from, to)`
// edges come from `model_causal_edges`, which has no LTM dependency), this
// function does not need to toggle `ltm_enabled` on the salsa db -- it
// only needs the wasm-produced score columns from the slab.  The
// `recompute_ltm_snapshots` dance happens only in the rel-loop-score
// counterpart.
//
// # Safety
// - `model` must be a valid pointer to a `SimlinModel`.
// - `slab_ptr` must be a non-NULL pointer to `slab_len` valid bytes; the
//   buffer is read but not retained.
// - `layout_ptr` must be a non-NULL pointer to `layout_len` valid bytes
//   produced by `WasmLayout::serialize` (i.e. the `out_layout` buffer of
//   `simlin_model_compile_to_wasm`).
// - The returned `SimlinLinks` must be freed with `simlin_free_links`.
//
// `include_internal` matches `simlin_analyze_get_links`: `false` collapses
// macro/module-internal synthetic nodes (preserving the composite
// through-contribution); `true` returns the raw causal graph.
SimlinLinks *simlin_analyze_links_from_wasm_results(SimlinModel *model,
                                                    const uint8_t *slab_ptr,
                                                    uintptr_t slab_len,
                                                    const uint8_t *layout_ptr,
                                                    uintptr_t layout_len,
                                                    bool include_internal,
                                                    SimlinError **out_error);

// Frees a SimlinLinks structure
//
// # Safety
// - `links` must be valid pointer returned by simlin_analyze_get_links
void simlin_free_links(SimlinLinks *links);

// Compute a loop's relative-loop-score series from a wasm-produced result
// slab.
//
// The wasm-backend twin of `simlin_analyze_get_relative_loop_score`.  Both
// FFIs funnel through `rel_loop_score_series` (extracted in Subcomponent A)
// over an `engine::Results` and the `(loop_partitions, loop_element_index)`
// snapshots, so the per-loop time series they produce cannot diverge by
// construction.
//
// Unlike the links twin (task 4), the rel-loop-score path needs the
// snapshots that only `model_ltm_variables` produces when the
// `SourceProject` salsa input has `ltm_enabled = true`.  This function
// runs the salsa queries through `recompute_ltm_snapshots`, which uses
// an `LtmEnabledGuard` to set the flag for the duration of the queries
// and unconditionally restore it on guard drop.  The reset is mandatory:
// the flag lives on a shared `SourceProject` input consumed by every
// other operation on the project, and leaking it would silently change
// the next consumer's analysis.
//
// The `loop_id` is parsed in the FFI shell (the engine-side core takes
// a base id + `(element_index, n_slots)` pair); a bare id on a scalar
// loop resolves to slot 0, a bare id on an arrayed loop resolves to the
// argmax-abs aggregator across all slots, and a subscripted id
// (`r1[Boston]`, `r1[Boston, 2]`) resolves to a specific slot via
// `LoopElementIndex::resolve`.  See `resolve_loop_query` for the
// resolution shared with the VM FFI.
//
// The series is copied into `results_ptr` clamped to `len` entries; the
// number written is reported through `out_written`, matching the out-buffer
// semantics of `simlin_analyze_get_relative_loop_score`.  The number written
// is bounded by the slab's row count -- callers should pass the saved-rows
// slab (`saved_steps * n_slots * 8` bytes), not the blob's full capacity,
// for the same reason as the links twin above.
//
// # Safety
// - `model` must be a valid pointer to a `SimlinModel`.
// - `slab_ptr` / `layout_ptr` are the byte buffers produced by the wasm
//   blob's results region and `simlin_model_compile_to_wasm`'s `out_layout`,
//   respectively; both are read but not retained.
// - `loop_id` must be a valid null-terminated C string.
// - `results_ptr` must point to a writable array of at least `len` doubles.
// - `out_written` must be a writable `*mut usize`.
// - `out_error` may be null or a writable `**mut SimlinError`.
void simlin_analyze_rel_loop_score_from_wasm_results(SimlinModel *model,
                                                     const uint8_t *slab_ptr,
                                                     uintptr_t slab_len,
                                                     const uint8_t *layout_ptr,
                                                     uintptr_t layout_len,
                                                     const char *loop_id,
                                                     double *results_ptr,
                                                     uintptr_t len,
                                                     uintptr_t *out_written,
                                                     SimlinError **out_error);

// Gets the relative loop score time series for a specific loop
//
// Renamed for clarity from simlin_analyze_get_rel_loop_score
//
// The relative score normalizes a loop's raw `loop_score` against the
// magnitudes of all loops sharing its cycle-partition, so it reads as the
// loop's fractional contribution to behavior.
//
// **Lone-pin caveat**: a modeler-pinned loop (`pin{n}` id) occupies its own
// single-slot partition. When it is the only loop scored there -- always so
// in discovery mode (no enumerated loop scores exist), and in exhaustive mode
// when it is the lone loop through its stock -- the relative score degenerates
// to exactly `+1`/`-1` (active/inactive) because there is nothing else to
// normalize against. For a lone pin the RAW `loop_score` series
// (`simlin_sim_get_series("$⁚ltm⁚loop_score⁚pin{n}")`) is the informative one.
// Multiple pins on stocks in the same SCC partition normalize against each
// other normally. See `engine::ltm_post::compute_rel_loop_scores`.
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim that has been run to completion
// - `loop_id` must be a valid C string
// - `results` must be a valid pointer to an array of at least `len` doubles
void simlin_analyze_get_relative_loop_score(SimlinSim *sim,
                                            const char *loop_id,
                                            double *results_ptr,
                                            uintptr_t len,
                                            uintptr_t *out_written,
                                            SimlinError **out_error);

// # Safety
//
// - `sim` must be a valid pointer to a SimlinSim object
// - `loop_id` must be a valid null-terminated C string
// - `results_ptr` must point to a valid array of at least `len` doubles
// - `out_written` must be a valid pointer to a usize
// - `out_error` may be null or a valid pointer to a SimlinError pointer
void simlin_analyze_get_rel_loop_score(SimlinSim *sim,
                                       const char *loop_id,
                                       double *results_ptr,
                                       uintptr_t len,
                                       uintptr_t *out_written,
                                       SimlinError **out_error);

// Get the number of element slots a loop's `loop_score` series occupies.
//
// For scalar loops this is 1; for arrayed (A2A) loops it equals the
// product of the loop's dimension lengths.  Used by callers (pysimlin,
// the TS engine) to detect whether a loop supports subscripted access
// (`r1[Boston]`) or only bare ID access.
//
// Errors with `DoesNotExist` if the loop_id is not present in the
// snapshot captured at `simlin_sim_new` time -- typically because the
// sim was created with `enable_ltm = false`, the loop was added in a
// later patch (the snapshot is bound to compilation-era loops), or
// the LTM pipeline auto-flipped to discovery mode (which doesn't
// emit loop_score variables).
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - `loop_id` must be a valid null-terminated C string
// - `out_element_count` must be a valid pointer to a usize
// - `out_error` may be null or a valid pointer to a SimlinError pointer
void simlin_analyze_get_loop_element_count(SimlinSim *sim,
                                           const char *loop_id,
                                           uintptr_t *out_element_count,
                                           SimlinError **out_error);

// simlin_error_str returns a string representation of an error code.
// The returned string must not be freed or modified.
//
// Accepts a u32 discriminant rather than an enum to safely handle invalid values
// from C/WASM callers. Returns "unknown_error" for invalid discriminants.
const char *simlin_error_str(uint32_t err);

// Returns the size of the SimlinLoop struct in bytes.
//
// Use this to validate ABI compatibility between Rust and JS/WASM consumers.
uintptr_t simlin_sizeof_loop(void);

// Returns the size of the SimlinLink struct in bytes.
//
// Use this to validate ABI compatibility between Rust and JS/WASM consumers.
uintptr_t simlin_sizeof_link(void);

// Returns the size of the SimlinErrorDetail struct in bytes.
//
// Use this to validate ABI compatibility between Rust and JS/WASM consumers.
uintptr_t simlin_sizeof_error_detail(void);

// Returns the size of a pointer on the current platform.
//
// Use this to validate ABI compatibility (expected 4 for wasm32).
uintptr_t simlin_sizeof_ptr(void);

// # Safety
//
// The pointer must have been created by a simlin function that returns a `*mut SimlinError`,
// must not be null, and must not have been freed already.
void simlin_error_free(SimlinError *err);

// # Safety
//
// The pointer must be either null or a valid `SimlinError` pointer that has not been freed.
SimlinErrorCode simlin_error_get_code(const SimlinError *err);

// # Safety
//
// The pointer must be either null or a valid `SimlinError` pointer that has not been freed.
// The returned string pointer is valid only as long as the error object is not freed.
const char *simlin_error_get_message(const SimlinError *err);

// # Safety
//
// The pointer must be either null or a valid `SimlinError` pointer that has not been freed.
uintptr_t simlin_error_get_detail_count(const SimlinError *err);

// # Safety
//
// The pointer must be either null or a valid `SimlinError` pointer that has not been freed.
// The returned array pointer is valid only as long as the error object is not freed.
const SimlinErrorDetail *simlin_error_get_details(const SimlinError *err);

// # Safety
//
// The pointer must be either null or a valid `SimlinError` pointer that has not been freed.
// The returned detail pointer is valid only as long as the error object is not freed.
const SimlinErrorDetail *simlin_error_get_detail(const SimlinError *err, uintptr_t index);

// Generate the best automatic layout for the named model and replace its
// views in-place.
//
// When `patch_json` is non-NULL, deserializes it as a JSON project patch
// and uses incremental layout (preserving existing element positions) if
// the model already has a non-empty view.  When NULL, always generates a
// full layout from scratch.
//
// Preserves the existing zoom level if the model already has a view with
// zoom > 0. Works on all targets including WASM (uses a serial fallback
// when rayon is unavailable).
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `model_name` must be a valid null-terminated UTF-8 string
// - `patch_json` may be null; when non-null must be a valid null-terminated UTF-8 JSON string
// - `out_error` may be null
void simlin_project_diagram_sync(SimlinProject *project,
                                 const char *model_name,
                                 const char *patch_json,
                                 SimlinError **out_error);

uint8_t *simlin_malloc(uintptr_t size);

// Frees memory allocated by simlin_malloc
//
// # Safety
// - `ptr` must be a valid pointer returned by simlin_malloc, or null
// - The pointer must not be used after calling this function
void simlin_free(uint8_t *ptr);

// Frees a string returned by the API
//
// # Safety
// - `s` must be a valid pointer returned by simlin API functions that return strings
void simlin_free_string(char *s);

// Compile the model to a self-contained WebAssembly module plus its layout.
//
// The emitted module exports its own linear `memory` and a `run` function
// that executes the whole simulation in one call, writing step-major result
// snapshots into a results region of its memory. This is an alternative to
// the bytecode VM intended for fast, repeated re-simulation (e.g. interactive
// parameter scrubbing): the host instantiates the module once and calls `run`
// on every change.
//
// Two buffers are returned via the malloc-return convention, each freed
// separately with `simlin_free`:
// - `out_wasm`/`out_wasm_len`: the wasm blob.
// - `out_layout`/`out_layout_len`: a self-describing, length-prefixed layout
//   buffer (all integers little-endian): `n_slots` (u64), `n_chunks` (u64),
//   `results_offset` (u64), `count` (u32), then per entry `name_len` (u32) +
//   UTF-8 name + `offset` (u64). A host strides one variable's `n_chunks`-long
//   series from the results region using `results_offset`, `n_slots`, and the
//   variable's `offset` from this map.
//
// Works from the model's datamodel alone -- no `SimlinSim` is required. Any
// compile or codegen failure stores a `SimlinError` (never panics across the
// boundary) and leaves both output buffers NULL.
//
// `ltm_enabled` and `ltm_discovery_mode` flip the same flags
// `simlin_project_enable_ltm` sets on a `SimlinProject`, but locally for this
// compile: the produced blob's layout includes the `$\u{205A}ltm\u{205A}*`
// synthetic series iff `ltm_enabled` is true.
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - `out_wasm`, `out_wasm_len`, `out_layout`, and `out_layout_len` must be
//   valid, non-null pointers
// - `out_error` may be null
void simlin_model_compile_to_wasm(SimlinModel *model,
                                  bool ltm_enabled,
                                  bool ltm_discovery_mode,
                                  uint8_t **out_wasm,
                                  uintptr_t *out_wasm_len,
                                  uint8_t **out_layout,
                                  uintptr_t *out_layout_len,
                                  SimlinError **out_error);

// Increments the reference count of a model
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
void simlin_model_ref(SimlinModel *model);

// Decrements the reference count and frees the model if it reaches zero
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
void simlin_model_unref(SimlinModel *model);

// Returns the resolved display name of this model.
//
// The returned string is owned by the caller and must be freed with
// `simlin_free_string`.
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
char *simlin_model_get_name(SimlinModel *model, SimlinError **out_error);

// Gets the number of datamodel-level variables in the model.
//
// # Parameters
// - `type_mask`: bitmask of `SIMLIN_VARTYPE_STOCK | FLOW | AUX | MODULE`. 0 means all types.
// - `filter`: canonicalized substring match. NULL or empty = no filter.
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
void simlin_model_get_var_count(SimlinModel *model,
                                uint32_t type_mask,
                                const char *filter,
                                uintptr_t *out_count,
                                SimlinError **out_error);

// Gets the datamodel-level variable names from the model.
//
// # Parameters
// - `type_mask`: bitmask of `SIMLIN_VARTYPE_STOCK | FLOW | AUX | MODULE`. 0 means all types.
// - `filter`: canonicalized substring match. NULL or empty = no filter.
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - `result` must be a valid pointer to an array of at least `max` char pointers
// - The returned strings are owned by the caller and must be freed with simlin_free_string
void simlin_model_get_var_names(SimlinModel *model,
                                uint32_t type_mask,
                                const char *filter,
                                char **result,
                                uintptr_t max,
                                uintptr_t *out_written,
                                SimlinError **out_error);

// Gets the incoming links (dependencies) for a variable
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - `var_name` must be a valid C string
// - `result` must be a valid pointer to an array of at least `max` char pointers (or null if max is 0)
// - The returned strings are owned by the caller and must be freed with simlin_free_string
//
// # Returns
// - If max == 0: returns the total number of dependencies (result can be null)
// - If max is too small: returns a negative error code
// - Otherwise: returns the number of dependencies written to result
void simlin_model_get_incoming_links(SimlinModel *model,
                                     const char *var_name,
                                     char **result,
                                     uintptr_t max,
                                     uintptr_t *out_written,
                                     SimlinError **out_error);

// Gets all causal links in a model
//
// Returns all causal links detected in the model, with their statically
// analyzed polarities. This includes flow-to-stock, stock-to-flow, and
// auxiliary-to-auxiliary links.
//
// The view matches `simlin_analyze_get_links`'s default
// (`include_internal = false`): macro/module-internal synthetic nodes are
// collapsed into composite real-variable edges. Both functions funnel
// through the same `analyze_links_core`, so the model-level (structural,
// score-less) and sim-level (scored) link sets cannot drift apart.
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - The returned SimlinLinks must be freed with simlin_free_links
SimlinLinks *simlin_model_get_links(SimlinModel *model, SimlinError **out_error);

// Gets the LaTeX representation of a variable's equation
//
// Returns the equation rendered as a LaTeX string, or NULL if the variable
// doesn't exist or doesn't have an equation (e.g., modules).
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - `ident` must be a valid C string
// - The returned string must be freed with simlin_free_string
char *simlin_model_get_latex_equation(SimlinModel *model,
                                      const char *ident,
                                      SimlinError **out_error);

// Gets a single variable from the model as tagged JSON.
//
// Returns JSON with a `"type"` discriminator (`"stock"`, `"flow"`, `"aux"`, `"module"`).
// Caller must free the output buffer with `simlin_free`.
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - `var_name` must be a valid C string
// - `out_buffer` and `out_len` must be valid pointers
void simlin_model_get_var_json(SimlinModel *model,
                               const char *var_name,
                               uint8_t **out_buffer,
                               uintptr_t *out_len,
                               SimlinError **out_error);

// Gets the effective sim specs for a model as JSON.
//
// Uses model-level sim_specs if present, otherwise falls back to
// the project-level sim_specs.
// Caller must free the output buffer with `simlin_free`.
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - `out_buffer` and `out_len` must be valid pointers
void simlin_model_get_sim_specs_json(SimlinModel *model,
                                     uint8_t **out_buffer,
                                     uintptr_t *out_len,
                                     SimlinError **out_error);

// Install the panic hook so that subsequent panics stash their message
// in a buffer readable via `simlin_get_panic_message()`.
//
// Call once from JS after WASM instantiation.
void simlin_init(void);

// Return the last panic message as a null-terminated C string, or null
// if no panic has been recorded.  The pointer is valid until the next
// panic or until `simlin_clear_panic_message()` is called.
//
// # Safety
// The returned pointer borrows the global buffer and must not be freed
// by the caller.
const char *simlin_get_panic_message(void);

// Clear the stored panic message.
void simlin_clear_panic_message(void);

// Applies a JSON patch to the project datamodel.
//
// # Safety
// - `project` must point to a valid `SimlinProject`.
// - `patch_data` must either be null with `patch_len == 0` or reference at
//   least `patch_len` bytes containing UTF-8 JSON.
// - `out_collected_errors` and `out_error` must be valid pointers for writing
//   error details and may be set to null on success.
//
// # Thread Safety
// - This function is thread-safe for concurrent calls with the same `project` pointer.
// - The underlying `datamodel::Project` is protected by a `Mutex`.
// - Multiple threads may safely modify the same project concurrently.
// - Different projects may also be patched concurrently from different threads safely.
//
// # Ownership and Mutation
// - When `dry_run` is false, this function modifies the project in-place.
// - When `dry_run` is true, the project remains unchanged and no modifications are committed.
// - The `project` pointer remains valid and usable after this function returns.
// - The project is not consumed or moved by this operation.
void simlin_project_apply_patch(SimlinProject *project,
                                const uint8_t *patch_data,
                                uintptr_t patch_len,
                                bool dry_run,
                                bool allow_errors,
                                SimlinError **out_collected_errors,
                                SimlinError **out_error);

// Open a project from binary protobuf data
//
// Deserializes a project from Simlin's native protobuf format. This is the
// recommended format for loading previously saved projects, as it preserves
// all project data with perfect fidelity.
//
// Returns NULL and populates `out_error` on failure.
//
// # Safety
// - `data` must be a valid pointer to at least `len` bytes
// - `out_error` may be null
// - The returned project must be freed with `simlin_project_unref`
SimlinProject *simlin_project_open_protobuf(const uint8_t *data,
                                            uintptr_t len,
                                            SimlinError **out_error);

// Open a project from JSON data
//
// Deserializes a project from JSON format. Supports two formats:
// - `SimlinJsonFormat::Native` (0): Simlin's native JSON representation
// - `SimlinJsonFormat::Sdai` (1): System Dynamics AI (SDAI) interchange format
//
// Returns NULL and populates `out_error` on failure.
//
// # Safety
// - `data` must be a valid pointer to at least `len` bytes of UTF-8 JSON
// - `out_error` may be null
// - The returned project must be freed with `simlin_project_unref`
// - `format` must be a valid discriminant (0 or 1), otherwise an error is returned
SimlinProject *simlin_project_open_json(const uint8_t *data,
                                        uintptr_t len,
                                        uint32_t format,
                                        SimlinError **out_error);

// Increment the reference count of a project
//
// Call this when you want to share a project handle with another component
// that will independently manage its lifetime.
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
void simlin_project_ref(SimlinProject *project);

// Decrement the reference count and free the project if it reaches zero
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
void simlin_project_unref(SimlinProject *project);

// Gets the number of models in the project
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
void simlin_project_get_model_count(SimlinProject *project,
                                    uintptr_t *out_count,
                                    SimlinError **out_error);

// Gets the list of model names in the project
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `result` must be a valid pointer to an array of at least `max` char pointers
// - The returned strings are owned by the caller and must be freed with simlin_free_string
void simlin_project_get_model_names(SimlinProject *project,
                                    char **result,
                                    uintptr_t max,
                                    uintptr_t *out_written,
                                    SimlinError **out_error);

// Adds a new model to a project
//
// Creates a new empty model with the given name and adds it to the project.
// The model will have no variables initially.
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `modelName` must be a valid C string
//
// # Returns
// - 0 on success
// - SimlinErrorCode::Generic if project or modelName is null or empty
// - SimlinErrorCode::DuplicateVariable if a model with that name already exists
void simlin_project_add_model(SimlinProject *project,
                              const char *model_name,
                              SimlinError **out_error);

// Gets a model from a project by name
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `modelName` may be null (uses default model)
// - The returned model must be freed with simlin_model_unref
SimlinModel *simlin_project_get_model(SimlinProject *project,
                                      const char *model_name,
                                      SimlinError **out_error);

// Open a project from XMILE/STMX format data
//
// Parses and imports a system dynamics model from XMILE format, the industry
// standard interchange format for system dynamics models. Also supports the
// STMX variant used by Stella.
//
// Returns NULL and populates `out_error` on failure.
//
// # Safety
// - `data` must be a valid pointer to at least `len` bytes
// - `out_error` may be null
// - The returned project must be freed with `simlin_project_unref`
SimlinProject *simlin_project_open_xmile(const uint8_t *data,
                                         uintptr_t len,
                                         SimlinError **out_error);

// Open a project from Vensim MDL format data
//
// Parses and imports a system dynamics model from Vensim's MDL format.
// Returns NULL and populates `out_error` on failure.
//
// # Safety
// - `data` must be a valid pointer to at least `len` bytes
// - `out_error` may be null
// - The returned project must be freed with `simlin_project_unref`
SimlinProject *simlin_project_open_vensim(const uint8_t *data,
                                          uintptr_t len,
                                          SimlinError **out_error);

// Open a Vensim MDL model with external data file support.
//
// When `data_dir` is non-null and the `file_io` feature is enabled, a
// `FilesystemDataProvider` is created using that directory as the base path
// for resolving relative data file references. When `data_dir` is null,
// a `NullDataProvider` is used (any GET DIRECT DATA references will error).
//
// Returns NULL and populates `out_error` on failure.
//
// # Safety
// - `data` must be a valid pointer to at least `len` bytes of UTF-8 MDL text
// - `data_dir` may be null; when non-null it must point to `data_dir_len` bytes
//   of valid UTF-8 representing a directory path
// - `out_error` may be null
// - The returned project must be freed with `simlin_project_unref`
SimlinProject *simlin_project_open_vensim_with_data(const uint8_t *data,
                                                    uintptr_t len,
                                                    const uint8_t *data_dir,
                                                    uintptr_t data_dir_len,
                                                    SimlinError **out_error);

// Open a project from systems format data
//
// Parses and translates a system dynamics model from the systems format
// (`.txt` line-oriented notation). Returns NULL and populates `out_error`
// on failure.
//
// # Safety
// - `data` must be a valid pointer to at least `len` bytes
// - `out_error` may be null
// - The returned project must be freed with `simlin_project_unref`
SimlinProject *simlin_project_open_systems(const uint8_t *data,
                                           uintptr_t len,
                                           SimlinError **out_error);

// Check if a project's model can be simulated
//
// Returns true if the model can be simulated (i.e., can be compiled to a VM
// without errors), false otherwise. This is a quick check for the UI to determine
// if the "Run Simulation" button should be enabled.
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `model_name` may be null (defaults to "main") or must be a valid UTF-8 C string
bool simlin_project_is_simulatable(SimlinProject *project,
                                   const char *model_name,
                                   SimlinError **out_error);

// Get all errors in a project including static analysis and compilation errors
//
// Returns NULL if no errors exist in the project. This function collects all
// static errors (equation parsing, unit checking, etc.) and also attempts to
// compile the "main" model to find any compilation-time errors.
//
// The caller must free the returned error object using `simlin_error_free`.
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - The returned pointer must be freed with `simlin_error_free`
SimlinError *simlin_project_get_errors(SimlinProject *project, SimlinError **out_error);

// Serialize a project to binary protobuf format
//
// Serializes the project's datamodel to Simlin's native protobuf format.
// This is the recommended format for saving and restoring projects, as it
// preserves all project data with perfect fidelity. The serialized bytes
// can be loaded later with `simlin_project_open_protobuf`.
//
// Caller must free output with `simlin_free`.
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `out_buffer` and `out_len` must be valid pointers
// - `out_error` may be null
void simlin_project_serialize_protobuf(SimlinProject *project,
                                       uint8_t **out_buffer,
                                       uintptr_t *out_len,
                                       SimlinError **out_error);

// Serializes a project to JSON format.
//
// # Safety
// - `project` must point to a valid `SimlinProject`.
// - `out_buffer` and `out_len` must be valid pointers where the serialized
//   bytes and length will be written.
// - `out_error` must be a valid pointer for receiving error details and may
//   be set to null on success.
//
// # Thread Safety
// - This function is thread-safe for concurrent calls with the same `project` pointer.
// - The underlying `engine::Project` uses `Arc<ModelStage1>` and is protected by a `Mutex`.
// - Multiple threads may safely access the same project concurrently.
// - Different projects may also be serialized concurrently from different threads safely.
//
// # Ownership
// - Serialization creates a deep copy of the project datamodel via `clone()`.
// - The original `project` remains fully usable after serialization.
// - The returned buffer is exclusively owned by the caller and MUST be freed with `simlin_free`.
// - The caller is responsible for freeing the buffer even if subsequent operations fail.
//
// # Buffer Lifetime
// - The serialized JSON buffer remains valid until `simlin_free` is called on it.
// - Multiple serializations can be performed concurrently (separate buffers are independent).
// - It is safe to serialize the same project multiple times.
void simlin_project_serialize_json(SimlinProject *project,
                                   uint32_t format,
                                   bool include_stdlib,
                                   uint8_t **out_buffer,
                                   uintptr_t *out_len,
                                   SimlinError **out_error);

// Serialize a project to XMILE format
//
// Exports a project to XMILE format, the industry standard interchange format
// for system dynamics models. The output buffer contains the XML document as
// UTF-8 encoded bytes.
//
// Caller must free output with `simlin_free`.
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `out_buffer` and `out_len` must be valid pointers
// - `out_error` may be null
void simlin_project_serialize_xmile(SimlinProject *project,
                                    uint8_t **out_buffer,
                                    uintptr_t *out_len,
                                    SimlinError **out_error);

// Serialize a project to systems format
//
// Exports a project to the systems format (`.txt` line-oriented notation).
// The output buffer contains the text as UTF-8 encoded bytes.
//
// Caller must free output with `simlin_free`.
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `out_buffer` and `out_len` must be valid pointers
// - `out_error` may be null
void simlin_project_serialize_systems(SimlinProject *project,
                                      uint8_t **out_buffer,
                                      uintptr_t *out_len,
                                      SimlinError **out_error);

// Render a project model's diagram as SVG
//
// Renders the stock-and-flow diagram for the named model to a standalone
// SVG document (UTF-8 encoded). The output includes embedded CSS styles
// and is suitable for display or export.
//
// Caller must free output with `simlin_free`.
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `model_name` must be a valid null-terminated UTF-8 string
// - `out_buffer` and `out_len` must be valid pointers
// - `out_error` may be null
void simlin_project_render_svg(SimlinProject *project,
                               const char *model_name,
                               uint8_t **out_buffer,
                               uintptr_t *out_len,
                               SimlinError **out_error);

// Render a project model's diagram as a PNG image
//
// Renders the stock-and-flow diagram for the named model to a PNG image.
// The SVG is generated internally and then rasterized with the Roboto Light
// font embedded in the binary. Pass `width = 0` and `height = 0` to use
// the SVG's intrinsic dimensions. When only one dimension is non-zero the
// other is derived from the aspect ratio. When both are non-zero, `width`
// takes precedence and `height` is derived from the aspect ratio.
//
// Only available with the `png_render` feature (on by default; the browser
// wasm artifact is built without it to keep the resvg/text-shaping stack
// out of the bundle browsers download).
//
// Caller must free output with `simlin_free`.
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `model_name` must be a valid null-terminated UTF-8 string
// - `out_buffer` and `out_len` must be valid pointers
// - `out_error` may be null
void simlin_project_render_png(SimlinProject *project,
                               const char *model_name,
                               uint32_t width,
                               uint32_t height,
                               uint8_t **out_buffer,
                               uintptr_t *out_len,
                               SimlinError **out_error);

// Creates a new simulation context
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
SimlinSim *simlin_sim_new(SimlinModel *model, bool enable_ltm, SimlinError **out_error);

// Increments the reference count of a simulation
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
void simlin_sim_ref(SimlinSim *sim);

// Decrements the reference count and frees the simulation if it reaches zero
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
void simlin_sim_unref(SimlinSim *sim);

// Runs the simulation to a specified time
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
void simlin_sim_run_to(SimlinSim *sim, double time, SimlinError **out_error);

// Runs the simulation to completion
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
void simlin_sim_run_to_end(SimlinSim *sim, SimlinError **out_error);

// Gets the number of time steps in the results
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
void simlin_sim_get_stepcount(SimlinSim *sim, uintptr_t *out_count, SimlinError **out_error);

// Resets the simulation to its initial state
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
void simlin_sim_reset(SimlinSim *sim, SimlinError **out_error);

// Runs just the initial-value evaluation phase of the simulation.
//
// After calling this, `simlin_sim_get_value` can read the t=0 values.
// Calling this multiple times is safe (it is idempotent).
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
void simlin_sim_run_initials(SimlinSim *sim, SimlinError **out_error);

// Gets a single value from the simulation
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - `name` must be a valid C string
// - `result` must be a valid pointer to a double
void simlin_sim_get_value(SimlinSim *sim,
                          const char *name,
                          double *out_value,
                          SimlinError **out_error);

// Sets a persistent value for a simple constant variable by name.
//
// The value persists across `simlin_sim_reset`. Call `simlin_sim_clear_values`
// to remove all overrides and restore compiled defaults.
//
// Can be called even when the VM has been consumed by `simlin_sim_run_to_end`;
// the value will be stored and applied to the next VM created on reset.
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - `name` must be a valid C string
void simlin_sim_set_value(SimlinSim *sim, const char *name, double val, SimlinError **out_error);

// Clears all constant value overrides, restoring original compiled values.
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
void simlin_sim_clear_values(SimlinSim *sim, SimlinError **out_error);

// Sets the value for a variable at the last saved timestep by offset
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
void simlin_sim_set_value_by_offset(SimlinSim *sim,
                                    uintptr_t offset,
                                    double val,
                                    SimlinError **out_error);

// Gets the column offset for a variable by name
//
// Returns the column offset for a variable name at the current context, or -1 if not found.
// This canonicalizes the name and resolves in the VM if present, otherwise in results.
// Intended for debugging/tests to verify name->offset resolution.
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - `name` must be a valid C string
void simlin_sim_get_offset(SimlinSim *sim,
                           const char *name,
                           uintptr_t *out_offset,
                           SimlinError **out_error);

// Gets the number of simulation-level variable names (flattened offsets).
//
// Available immediately after `simlin_sim_new` (no simulation run needed).
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
void simlin_sim_get_var_count(SimlinSim *sim, uintptr_t *out_count, SimlinError **out_error);

// Gets the simulation-level variable names (flattened offsets).
//
// Available immediately after `simlin_sim_new` (no simulation run needed).
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - `result` must be a valid pointer to an array of at least `max` char pointers
// - The returned strings are owned by the caller and must be freed with simlin_free_string
void simlin_sim_get_var_names(SimlinSim *sim,
                              char **result,
                              uintptr_t max,
                              uintptr_t *out_written,
                              SimlinError **out_error);

// Gets a time series for a variable
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - `name` must be a valid C string
// - `results_ptr` must point to allocated memory of at least `len` doubles
void simlin_sim_get_series(SimlinSim *sim,
                           const char *name,
                           double *results_ptr,
                           uintptr_t len,
                           uintptr_t *out_written,
                           SimlinError **out_error);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* SIMLIN_ENGINE2_H */
