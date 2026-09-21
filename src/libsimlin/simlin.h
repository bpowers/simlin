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

// Loop polarity for C API.
//
// `MostlyReinforcing`/`MostlyBalancing` ("Rux"/"Bux" in the LTM literature)
// are the mixed-sign runtime polarities the engine determines when a loop has
// expressed both signs over a simulation but one dominates with high
// confidence; they are reported here verbatim rather than coalesced down to
// `Reinforcing`/`Balancing` (GH #495).  The companion
// `SimlinLoop.polarity_confidence` / `SimlinDiscoveredLoop.polarity_confidence`
// carries the `[0.0, 1.0]` confidence ratio behind the classification.
typedef enum {
  SIMLIN_LOOP_POLARITY_REINFORCING = 0,
  SIMLIN_LOOP_POLARITY_BALANCING = 1,
  SIMLIN_LOOP_POLARITY_UNDETERMINED = 2,
  // "Rux" -- mixed-sign runtime scores, predominantly reinforcing.
  SIMLIN_LOOP_POLARITY_MOSTLY_REINFORCING = 3,
  // "Bux" -- mixed-sign runtime scores, predominantly balancing.
  SIMLIN_LOOP_POLARITY_MOSTLY_BALANCING = 4,
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
// SCC-size gate (or discovery was requested directly) and loops are found
// post-simulation from the recorded link scores instead. Without this signal
// a caller cannot tell why an LTM-enabled run produced empty or different
// loop results.
typedef enum {
  SIMLIN_LTM_MODE_DISABLED = 0,
  SIMLIN_LTM_MODE_EXHAUSTIVE = 1,
  SIMLIN_LTM_MODE_DISCOVERY = 2,
} SimlinLtmMode;

// Which part of an element a hit lands on.
typedef enum {
  // The element itself: a shape, a flow's pipe or valve, a link's line.
  SIMLIN_HIT_PART_BODY = 0,
  // A flow's sink end, or a link's arrowhead.
  SIMLIN_HIT_PART_ARROWHEAD = 1,
  // A flow's source end.
  SIMLIN_HIT_PART_SOURCE = 2,
  // The element's name label.
  SIMLIN_HIT_PART_LABEL = 3,
} SimlinHitPart;

// The tool a host's toolbar arms.
typedef enum {
  SIMLIN_TOOL_NONE = 0,
  SIMLIN_TOOL_STOCK = 1,
  SIMLIN_TOOL_FLOW = 2,
  SIMLIN_TOOL_AUX = 3,
  SIMLIN_TOOL_LINK = 4,
  SIMLIN_TOOL_MODULE = 5,
} SimlinTool;

typedef enum {
  SIMLIN_POINTER_KIND_TOUCH = 0,
  SIMLIN_POINTER_KIND_PENCIL = 1,
  SIMLIN_POINTER_KIND_MOUSE = 2,
} SimlinPointerKind;

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
  SIMLIN_ERROR_CODE_NO_APP_IN_UNITS = 35,
  SIMLIN_ERROR_CODE_NO_SUBSCRIPT_IN_UNITS = 36,
  SIMLIN_ERROR_CODE_NO_IF_IN_UNITS = 37,
  SIMLIN_ERROR_CODE_NO_UNARY_OP_IN_UNITS = 38,
  SIMLIN_ERROR_CODE_BAD_BINARY_OP_IN_UNITS = 39,
  SIMLIN_ERROR_CODE_NO_CONST_IN_UNITS = 40,
  SIMLIN_ERROR_CODE_EXPECTED_INTEGER = 41,
  SIMLIN_ERROR_CODE_EXPECTED_INTEGER_ONE = 42,
  SIMLIN_ERROR_CODE_DUPLICATE_UNIT = 43,
  SIMLIN_ERROR_CODE_EXPECTED_MODULE = 44,
  SIMLIN_ERROR_CODE_EXPECTED_IDENT = 45,
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

// The format `simlin_project_check_save` checks a save in.
typedef enum {
  SIMLIN_SAVE_FORMAT_MDL = 0,
  SIMLIN_SAVE_FORMAT_XMILE = 1,
  SIMLIN_SAVE_FORMAT_JSON = 2,
  SIMLIN_SAVE_FORMAT_JSON_SDAI = 3,
  SIMLIN_SAVE_FORMAT_PROTOBUF = 4,
} SimlinSaveFormat;

// A live drag over the view as it was when the drag began.
typedef struct SimlinGesture SimlinGesture;

// One agent's work on one model: the evidence ids it has been given and what
// it last read (`simlin_engine::tools::Session`), over the model it was made
// for.
typedef struct SimlinToolSession SimlinToolSession;

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
  // Polarity-confidence ratio in `[0.0, 1.0]` behind `polarity` (GH #495):
  // `1.0` for a clean `Reinforcing`/`Balancing` loop, `0.0` for
  // `Undetermined`.  On the STRUCTURAL `simlin_analyze_get_loops` surface
  // this is `1.0`/`0.0` by design (a loop's links are either all signed or
  // at least one is unknown); the mixed-sign `MostlyReinforcing`/
  // `MostlyBalancing` variants with intermediate confidence appear on the
  // discovery surface (`SimlinDiscoveredLoop`).  Adding this `f64` grew the
  // struct additively (8-byte alignment pushed it past the old 20 bytes);
  // `simlin_sizeof_loop` and the `@simlin/engine` `LOOP_SIZE`/`readLoops`
  // offsets track the new size.
  double polarity_confidence;
  // RESULT-SCOPED index into `SimlinLoops.partitions` naming the loop's
  // cycle partition, or -1 for a loop whose stocks resolve to no
  // parent-level partition (a pure module-internal loop).  A single index
  // suffices because a feedback loop's stocks form one strongly-connected
  // set (mirroring `SimlinDiscoveredLoop.partition`).  Indices are dense,
  // assigned in first-appearance order over this `SimlinLoops` list; they
  // identify partitions within ONE result only and are not stable across
  // runs or model edits -- key on the partition's stock-name SET for a
  // durable identity.  Both this exhaustive surface and the discovery
  // surface (`SimlinDiscoveredLoop.partition`) partition stocks at
  // ELEMENT granularity (`population[nyc]`; plain names for scalar
  // models), so the stock set is a usable cross-surface key for arrayed
  // models too (GH #746; before that fix this surface partitioned at
  // variable granularity and the sets matched only for scalar models).
  // Adding this `i32` grew the struct additively past its old
  // 32 bytes (`simlin_sizeof_loop` and the `@simlin/engine`
  // `LOOP_SIZE`/`readLoops` offsets track the new size).
  int32_t partition;
} SimlinLoop;

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

// List of loops returned by analysis
typedef struct {
  SimlinLoop *loops;
  uintptr_t count;
  // The cycle partitions referenced by `loops` (each loop's `partition`
  // indexes this array).  Dense, in first-appearance order over the loop
  // list; result-scoped.  Reuses `SimlinDiscoveredPartition` so the
  // exhaustive/pinned loop surface reports partitions in the same shape as
  // the discovery surface.  Both surfaces partition stocks at element
  // granularity (see `SimlinLoop.partition`), so the stock SETS are a
  // usable cross-surface key for scalar and arrayed models alike.
  // Appended after `loops`/`count` so the existing container offsets the TS
  // reader uses are unchanged.
  SimlinDiscoveredPartition *partitions;
  uintptr_t partition_count;
} SimlinLoops;

// Opaque model structure
typedef struct {
  uint8_t _private[0];
} SimlinModel;

// Opaque error structure returned by the API
typedef struct {
  uint8_t _private[0];
} SimlinError;

// Opaque simulation structure
typedef struct {
  uint8_t _private[0];
} SimlinSim;

// A single loop found by post-simulation LTM loop discovery.
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
  // Polarity-confidence ratio in `[0.0, 1.0]` behind `polarity` (GH #495):
  // `1.0` for a clean `Reinforcing`/`Balancing` loop, a value below 1.0 for
  // a mixed-sign `MostlyReinforcing`/`MostlyBalancing` loop, `0.0` for
  // `Undetermined`.  This is the high-value confidence surface: discovery
  // classifies loops from runtime score series, so the Rux/Bux variants and
  // their intermediate confidences actually appear here.
  double polarity_confidence;
} SimlinDiscoveredLoop;

// A time interval during which a specific set of loops dominates behavior.
//
// Dominance is computed WITHIN a cycle partition (GH #998): a loop's
// importance series is its share of its own partition's total, so
// cross-partition ranking is not well-defined and a loop alone in its
// partition would read exactly 1.0 at every active step.  Each period
// therefore says which partition it describes, and a result carries one
// period timeline per partition (partition-major order, most-competitive
// partition first).
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
  // RESULT-SCOPED index into `SimlinDiscoveryResult.partitions` naming the
  // cycle partition this period describes -- the same index space as
  // `SimlinDiscoveredLoop.partition` -- or -1 for a period of a loop with
  // no parent-level partition (a module-internal loop, which competes only
  // against itself, mirroring the ranking's per-loop Solo groups).
  // Appended additively (GH #998).
  int32_t partition;
} SimlinDominantPeriod;

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
  // Non-zero when discovery hit its wall-clock `budget_ms` before finishing,
  // so `loops`/`periods` may be partial.
  bool truncated;
  // Non-zero when discovery's cross-element-through-aggregate loop recovery
  // (GH #696) hit its reducer-loop-count budget, so some cross-agg reducer
  // loops are absent from `loops`.  Distinct from `truncated` (the wall-clock
  // time budget): this is the structural-completeness signal (GH #515/#696)
  // that mirrors exhaustive mode's analogous salsa Warning, surfacing the
  // completeness asymmetry that previously left discovery callers blind.
  bool agg_recovery_truncated;
  // Non-zero when discovery's candidate generation was the union-graph
  // circuit enumeration AND it ran to completion: `loops` is then the
  // retention/ranking pipeline's selection from the PROVABLY COMPLETE set
  // of loops that can ever score, so discovery was exact rather than
  // heuristic (exact for cross-aggregate reducer loops too only while
  // `agg_recovery_truncated` is also false: those are stitched under their
  // own budget).  Zero means the shortest-path fallback generated the
  // candidates -- an explicit SAMPLE of the loop universe -- because the
  // enumeration's budgets or `budget_ms` did not allow it to finish.  Read
  // this before treating an absent loop as evidence the model has none.
  //
  // Meaningless (`false` by construction) when `analysis_error` is
  // non-NULL: analysis never reached candidate generation at all, so this
  // is not "a sample" in the sense above -- check `analysis_error` first.
  bool enumeration_complete;
  // How many loops passed discovery's retention filter, BEFORE the
  // reported-loop cap truncated `loops`.  Equal to `loop_count` when the
  // cap did not bind, and above it when it did -- the signal that `loops`
  // is a coverage-aware SUBSET of the loops worth reporting (each step's
  // dominant loop per competing partition is guaranteed a slot while those
  // dominant loops fit the cap, the rest
  // is filled by mean importance): presented in importance order, but not
  // a strict most-important-first prefix.
  uintptr_t retained_loops;
  // The size of the candidate universe: how many DISTINCT loops' mass the
  // discovery denominators sum -- the ever-simultaneously-active
  // elementary cycles the enumeration found, minus any non-representative
  // duplicate the retention pass merges into a single reported loop, plus
  // any cross-aggregate loop stitched together from disjoint elementary
  // pieces -- which is the population every reported loop's importance is
  // measured against.  `-1` when `enumeration_complete` is zero, since a
  // sampled report has no universe to describe; the two fields always
  // agree, and the sentinel keeps "the fallback ran" distinct from a
  // genuinely empty universe (`0`).  Also `-1` when `analysis_error` is
  // non-NULL (analysis never ran, so there is no universe of any kind).
  int64_t universe_loops;
  // Non-NULL when the model could not be compiled or analyzed for LTM at
  // all -- a malformed equation, an unresolved reference, or a hard
  // compile failure.  When set, every OTHER field describes an
  // analysis that never started: `loops`/`periods`/`partitions` are
  // empty, `loop_count`/`period_count`/`partition_count`/`retained_loops`
  // are `0`, `enumeration_complete` is `false`, and `universe_loops` is
  // `-1` -- the SAME shape a genuinely sampled (fallback) run with zero
  // discovered loops would report, which is why `analysis_error` is the
  // field to check FIRST.  The three outcomes, in the order to test them:
  //
  // 1. **Never ran**: `analysis_error` non-NULL.  Nothing below is
  //    meaningful; the message names the compile failure.
  // 2. **Sampled**: `analysis_error` NULL, `enumeration_complete` is
  //    `false`.  `loops` is an explicit SAMPLE of the loop universe (the
  //    shortest-path fallback ran because the exact enumeration's budgets
  //    or the caller's `budget_ms` did not allow it to finish);
  //    `universe_loops` is `-1` here too, but for a different reason (a
  //    sample has no universe to describe, not that analysis never ran).
  // 3. **Exact**: `analysis_error` NULL, `enumeration_complete` is
  //    `true`.  `loops` is the retention/ranking selection from the
  //    PROVABLY COMPLETE candidate universe; `universe_loops` names that
  //    universe's size.
  //
  // Owned; freed alongside the rest of the result by
  // `simlin_free_discovery_result`.
  char *analysis_error;
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
  // inputs -- a value in `[-1, 1]` (GH #652).  Comparable between the
  // inputs of ONE target; see `scored_input_count` for the cross-target
  // ranking caveat.  When non-NULL its length equals `score_len`.
  double *relative_score;
  uintptr_t relative_score_len;
  // The size of `relative_score`'s normalization group (GH #998): how
  // many CONTRIBUTING links share this link's `to` target, itself
  // included; 0 when this link never contributes (no score series, or an
  // all-NaN one -- an all-NaN series adds no summand to any step's
  // denominator, so it is no competition).  A group of ONE reads exactly
  // `±1` at every step BY CONSTRUCTION -- ranking links globally by
  // `|relative_score|` floats such no-competition links to the top (58 of
  // C-LEARN's global top 100 were single-input targets).  Group links by
  // `to` and rank within a group; use this field to detect the trivial
  // groups.  Per-step residual: a link NaN at SOME steps counts here yet
  // leaves its siblings momentarily unopposed at those steps -- a scalar
  // cannot carry that.  Appended additively (`simlin_sizeof_link` and the
  // `@simlin/engine` `LINK_SIZE`/`readLinks` offsets track it).
  uintptr_t scored_input_count;
} SimlinLink;

// Collection of links
typedef struct {
  SimlinLink *links;
  uintptr_t count;
} SimlinLinks;

// A press, resolved by the host: where it landed (model coordinates), what it
// hit (`simlin_model_hit_test`), the armed tool, the selection before it, and
// the pointer that made it.
typedef struct {
  double x;
  double y;
  // Whether the press landed on an element; `hit_uid` and `hit_part` are
  // read only when it did.
  bool has_hit;
  int32_t hit_uid;
  SimlinHitPart hit_part;
  SimlinTool tool;
  // The selection before the press: `selection_len` uids (NULL when zero).
  const int32_t *selection;
  uintptr_t selection_len;
  // A selection modifier (Shift or Command) is held.
  bool toggle;
  SimlinPointerKind pointer;
  // How far outside a drop target the pointer may be and still land on it,
  // in model units (the host's touch slop divided by the zoom).
  double target_slop;
} SimlinPress;

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
  // The bare human-readable reason without the source snippet or the
  // model/variable summary line that `message` carries (e.g. "the equation
  // computes to units 'people', but the variable's specified units are
  // 'person'"), and without the code's name. Never NULL on a detail
  // libsimlin reports: when the raising site wrote no reason of its own (a
  // parse error, whose reason `message` shows as a snippet), it says what
  // the code means. Appended additively: existing field offsets are
  // unchanged.
  const char *details;
} SimlinErrorDetail;

// Opaque project structure
typedef struct {
  uint8_t _private[0];
} SimlinProject;

// Opaque standalone results structure (e.g. an imported VDF file)
typedef struct {
  uint8_t _private[0];
} SimlinResults;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

// Get the feedback loops detected in a model, with STRUCTURAL polarity.
//
// The polarity reported here comes from static analysis of the loops' link
// signs (`model_detected_loops`): a loop is Reinforcing/Balancing only when
// every link has a determined sign (confidence 1.0), Undetermined when any
// link is unknown (confidence 0.0).  No simulation is required -- this works
// off a `SimlinModel` alone.  For the RUNTIME polarity (Rux/Bux from the
// post-sim `loop_score` series, per the LTM papers' confidence gate) use
// `simlin_analyze_get_loops_runtime`, which takes a run `SimlinSim`.
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - The returned SimlinLoops must be freed with simlin_free_loops
SimlinLoops *simlin_analyze_get_loops(SimlinModel *model, SimlinError **out_error);

// Get the feedback loops detected in a model, with RUNTIME polarity derived
// from a completed simulation's per-step loop-score series.
//
// This is the sim-bearing sibling of `simlin_analyze_get_loops` (which takes
// only a `SimlinModel` and reports STRUCTURAL polarity).  It builds the same
// exhaustive `model_detected_loops` set, then runs the engine's
// `reclassify_loops_from_results` primitive (GH #679) over it: for every loop
// whose `$⁚ltm⁚loop_score⁚{id}` series exists in the results, the loop's
// polarity and confidence are overwritten by
// `crate::ltm::LoopPolarity::from_runtime_scores` over the loop's
// partition-RELATIVE series (its per-step share of its cycle partition, the
// same series `simlin_analyze_get_relative_loop_score` returns): the LTM
// papers' Rux/Bux/U classification with the 0.99 confidence gate, on a
// bounded, dominance-weighted base.  A loop whose runtime score
// is never active keeps its structural classification.  Loop IDs are stable
// (a `u1` stays `u1` even after its polarity flips to Reinforcing).
//
// This is the FIRST production caller of `reclassify_loops_from_results`: it
// is the only path on which the exhaustive loop surface can report Rux/Bux,
// or a runtime sign flip (e.g. a structurally-Undetermined loop that the
// simulation shows is single-signed), which the structural surface can never
// express.
//
// **A2A (arrayed) semantics**: the engine primitive concatenates ALL element
// slots of an arrayed loop's `loop_score` series into one sample set before
// classifying, so an A2A loop with one reinforcing element and one balancing
// element classifies Undetermined -- the whole-loop behavior rather than any
// single slot.  pysimlin's `Run.loops` is built directly on this primitive
// (it adds only the per-step behavior series on top), so it reports exactly
// this all-slots classification.  See the note on
// `engine::db::reclassify_loops_from_results`.
//
// Requires `sim` to have been created with `enable_ltm = true` and run to
// completion (it must hold `Results`); otherwise an error is reported through
// `out_error`.  When LTM was not enabled the `loop_score` series are absent,
// so the result degenerates to the structural classification.
//
// The loop LIST is the project's current contents; the slot width each
// loop's scores are read with is the sim's own compile-era snapshot
// (`SimState::loop_partitions`), as `simlin_analyze_get_relative_loop_score`
// resolves against.  After an edit that changed the loop structure
// (`simlin_project_apply_patch`, `simlin_project_replace_contents`) the two
// therefore mix: a loop added since the run has no column and keeps its
// structural label, and a loop whose id was renumbered reads the column its
// id had.  Re-run before analyzing.
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim that has been run.
// - The returned SimlinLoops must be freed with simlin_free_loops.
SimlinLoops *simlin_analyze_get_loops_runtime(SimlinSim *sim, SimlinError **out_error);

// Frees a SimlinLoops structure
//
// # Safety
// - `loops` must be a valid pointer returned by simlin_analyze_get_loops
void simlin_free_loops(SimlinLoops *loops);

// Run post-simulation LTM loop discovery on a model and return the
// discovered loops (with per-step importance series), the dominant periods,
// and a truncation flag, as one `SimlinDiscoveryResult`.
//
// `budget_ms` bounds the wall-clock time spent generating loop candidates;
// `0` means unlimited.  When the budget elapses before discovery finishes,
// `truncated` is set and the returned loops/periods reflect only the
// timesteps processed so far.  Discovery on very large models can be
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
// found post-simulation from the recorded link scores.  The mode is captured
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
// FFI functions funnel through the same `engine::analysis::model_links` so the link
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
// function reads no LTM derivation -- it only needs the wasm-produced score
// columns from the slab.  `ltm_snapshots` is read only by the rel-loop-score
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
// FFIs resolve the loop id against the `loop_element_index` snapshot and
// funnel through `rel_loop_score_series` over an `engine::Results` and the
// `loop_partitions` snapshot, so the per-loop time series they produce
// cannot diverge by construction.
//
// Unlike the links twin, the rel-loop-score path needs the snapshots
// `model_ltm_variables` derives (the per-loop partition map and slot
// metadata).  This function reads them through `ltm_snapshots`, at the
// project db's current revision; see that function for when they match the
// blob's layout.
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
// normalize against. For a lone pin the RAW `loop_score` series is the
// informative one: read it via `simlin_analyze_get_loop_score`, which takes
// the same loop-id syntax (per-element access included, GH #998).
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

// Gets the RAW loop score time series for a specific loop (GH #998).
//
// The raw sibling of `simlin_analyze_get_relative_loop_score`: same loop-id
// syntax (bare `r1`, or subscripted `r1[Boston]` / `r1[Boston, 2]` for
// arrayed loops), same slot resolution, but the series is the loop's raw
// `loop_score` with NO partition normalization.  A bare id on an arrayed
// loop returns the signed argmax-abs aggregate across slots (the dominant
// element's raw contribution at each step); a subscripted id returns that
// element's own series.  A step where EVERY slot is NaN stays NaN in the
// aggregate (honest raw data -- unlike the relative accessor's bare-id
// aggregate, whose 0.0-on-undefined matches its SAFEDIV "inactive"
// convention).
//
// This is the accessor a LONE PIN needs: a modeler-pinned loop alone in
// its cycle partition has a relative score of exactly `+1`/`-1` by
// construction (nothing else to normalize against), so its raw series is
// the informative one -- and reading the raw synthetic via
// `simlin_sim_get_series` resolves to element 0 only on an arrayed loop,
// silently disagreeing with the relative accessor's aggregate.  This FFI
// closes that gap with per-element raw access through the same subscript
// resolution the relative accessor uses.
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim that has been run to completion
// - `loop_id` must be a valid C string
// - `results_ptr` must be a valid pointer to an array of at least `len` doubles
// - `out_written` must be a writable `*mut usize`
// - `out_error` may be null or a writable `**mut SimlinError`
void simlin_analyze_get_loop_score(SimlinSim *sim,
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

// The element and part of the model's diagram that `(x, y)` (model
// coordinates) lands on, decided in `simlin_engine::editing::HitIndex`'s tiers:
// a body firmly holding the point, else an end handle within reach, else a
// label holding the point, else the nearest drawing within `tolerance` model
// units (the host's touch slop divided by the zoom). Writes `*out_hit = false`
// when nothing drawn is within reach. Refuses a model with no stock-and-flow
// view with `DoesNotExist`, as every editing entry point does.
//
// The view's index is built by the first hit test after the project changes
// and reused until it changes again (`ProjectContents`), so a hover at display
// rate costs in proportion to what is near the point. A hit test locks the
// project's datamodel, as the planners do, so while an edit holds the project
// it waits and answers from the contents the edit leaves: the contents a press
// is then planned against.
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - `out_hit`, `out_uid` and `out_part` must be valid, non-null pointers
// - `out_error` may be null
void simlin_model_hit_test(SimlinModel *model,
                           double x,
                           double y,
                           double tolerance,
                           bool *out_hit,
                           int32_t *out_uid,
                           SimlinHitPart *out_part,
                           SimlinError **out_error);

// Plan a tap. Writes a JSON object to a buffer the caller frees with
// `simlin_free`: `commit` (`"none"`, `"edit"` or `"select"`), `selection` (what
// the host adopts), `handoff` (a uid whose name editor opens once the edit
// lands, or null), `details` (the tap opens the element's details), `label`
// (the edit's name for history), and `patch` (the patch to apply for an
// `"edit"`, else null).
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - `press` must be a valid pointer to a SimlinPress
// - `out_buf` and `out_len` must be valid, non-null pointers
// - `out_error` may be null
void simlin_model_plan_tap(SimlinModel *model,
                           const SimlinPress *press,
                           uint8_t **out_buf,
                           uintptr_t *out_len,
                           SimlinError **out_error);

// Begin a drag. Returns the gesture, released with `simlin_gesture_unref`; or
// NULL, with `*out_error` set when the call failed and with no error when the
// press starts no drag (a finger on the empty canvas, which pans). The gesture
// plans against the view as it is now: a host that applies an edit while a
// drag is live ends the drag.
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - `press` must be a valid pointer to a SimlinPress
// - `out_error` may be null
SimlinGesture *simlin_gesture_begin(SimlinModel *model,
                                    const SimlinPress *press,
                                    SimlinError **out_error);

// Plan the frame with the pointer at `(x, y)` (model coordinates). Writes a
// JSON object to a buffer the gesture owns, valid until the next call on this
// gesture or its release: `kind` (the gesture, null while a press on a pipe has
// not yet moved), `commit`, `selection`, `target` (a drop target `{uid, valid}`
// to highlight, or null), `handoff`, `details`, `label`, `hidden` (uids of base
// scene elements the frame does not draw), and `elements` (scene elements, the
// `simlin_project_render_scene` contract, drawn among the base scene at their
// layers: a link's arc runs between the centers of what it connects, and the
// elements above it hide its ends).
//
// # Safety
// - `gesture` must be a valid pointer to a SimlinGesture
// - `out_buf` and `out_len` must be valid, non-null pointers
// - `out_error` may be null
void simlin_gesture_frame(SimlinGesture *gesture,
                          double x,
                          double y,
                          const uint8_t **out_buf,
                          uintptr_t *out_len,
                          SimlinError **out_error);

// Plan the release with the pointer at `(x, y)`: the frame the preview showed
// there. Writes the same JSON object as `simlin_model_plan_tap` to a buffer the
// caller frees with `simlin_free`, with `commit` `"none"` and `patch` null when
// the release changes nothing (an invalid drop, a drag back to where it
// started).
//
// # Safety
// - `gesture` must be a valid pointer to a SimlinGesture
// - `out_buf` and `out_len` must be valid, non-null pointers
// - `out_error` may be null
void simlin_gesture_commit(SimlinGesture *gesture,
                           double x,
                           double y,
                           uint8_t **out_buf,
                           uintptr_t *out_len,
                           SimlinError **out_error);

// Increment a gesture's reference count.
//
// # Safety
// - `gesture` must be a valid pointer to a SimlinGesture, or NULL
void simlin_gesture_ref(SimlinGesture *gesture);

// Decrement a gesture's reference count, releasing it at zero.
//
// # Safety
// - `gesture` must be a valid pointer to a SimlinGesture, or NULL
void simlin_gesture_unref(SimlinGesture *gesture);

// Plan moving `selection` (`selection_len` uids) by `(dx, dy)` model units,
// independent of the zoom: the frame a move-selection drag of the selection
// plans for that travel (`simlin_engine::editing::plan_move`), which a host
// plans to nudge the selection from the keyboard. Positioned elements move,
// flows follow their moved ends, a selected flow neither of whose ends moves
// slides its valve along its pipe, and a link moves only with its endpoints.
// Writes the same JSON object as `simlin_model_plan_tap` to a buffer the
// caller frees with `simlin_free`, with `kind` `"moveSelection"`, and with
// `commit` `"none"` and `patch` null when the move lands nothing: nothing
// moves (a lone link, an offset across a selected flow's straight pipe), a
// flow the move routes would break its invariants, or the offset overflows a
// coordinate.
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - `selection` must point to `selection_len` uids, or be NULL when it is zero
// - `out_buf` and `out_len` must be valid, non-null pointers
// - `out_error` may be null
void simlin_model_plan_move(SimlinModel *model,
                            const int32_t *selection,
                            uintptr_t selection_len,
                            double dx,
                            double dy,
                            uint8_t **out_buf,
                            uintptr_t *out_len,
                            SimlinError **out_error);

// The patch deleting `selection` (`selection_len` uids) from the model's
// diagram: the selected elements, the clouds of removed flows, the aliases of
// removed elements, every link touching a removed element, and a new cloud at
// every surviving flow end attached to a removed element; applying it deletes
// the variables and updates the stock lists. Written to a buffer the caller
// frees with `simlin_free`.
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - `selection` must point to `selection_len` uids, or be NULL when it is zero
// - `out_buf` and `out_len` must be valid, non-null pointers
// - `out_error` may be null
void simlin_model_plan_delete(SimlinModel *model,
                              const int32_t *selection,
                              uintptr_t selection_len,
                              uint8_t **out_buf,
                              uintptr_t *out_len,
                              SimlinError **out_error);

// The patch renaming the variable `old_name` to `new_name` (as typed, stored
// verbatim): its elements relabeled on the model's diagram when it has any --
// applying the edit then renames the variable -- else a direct rename. Written
// to a buffer the caller frees with `simlin_free`.
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - `old_name` and `new_name` must be valid NUL-terminated UTF-8 strings
// - `out_buf` and `out_len` must be valid, non-null pointers
// - `out_error` may be null
void simlin_model_plan_rename(SimlinModel *model,
                              const char *old_name,
                              const char *new_name,
                              uint8_t **out_buf,
                              uintptr_t *out_len,
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
// Uses the project's persistent incremental compiler -- no `SimlinSim` is
// required. Unchanged compilation reuses the same queries as VM creation when
// the requested discovery mode matches the project setting.
// Any compile or codegen failure stores a `SimlinError` and leaves both
// output buffers NULL.
//
// `ltm_enabled` selects the LTM overlay and latches the project's LTM
// diagnostic request, as `simlin_sim_new` does. `ltm_discovery_mode` overrides
// discovery for this compile only; the shared project's prior flag is restored
// on success and failure. Special stocks follow the VM's expansion path and
// LTM degradation contract.
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

// Gets the incoming links (dependencies) for a variable: the variables of
// this model it reads, itself or through the private helpers its equation
// synthesizes. A read of another model's variable through a module instance
// (`child.output`) is not a variable of this model and is not listed --
// neither the composite nor the instance; a diagram draws that link to the
// module box (`layout::rendered_dependency_ident`), this surface lists
// variables.
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
// through the same `engine::analysis::model_links`, so the model-level (structural,
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
//
// # Effect on live simulations
// - A `SimlinSim` created BEFORE a committed patch is a stale snapshot for
//   the `simlin_sim_*` entry points: it was compiled from the old contents
//   and keeps its results and its ability to `reset`/re-run against that
//   program.
// - The sim-bearing ANALYSIS entry points (`simlin_analyze_get_loops_runtime`,
//   `simlin_analyze_get_links`, ...) enumerate loops and links from the
//   project's CURRENT contents and read scores out of the stale sim's
//   results by loop id (the slot widths come from the sim's own snapshot),
//   so after a patch that changed the loop structure they mix old results
//   with the new model. Create and run a new sim after a patch before
//   analyzing -- the posture `simlin_project_replace_contents` documents.
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

// Create a new project: what a modeler starts from, and what a host copies a
// project into.
//
// The project is named `name` (NULL for no name) and holds one model, `main`,
// with an empty stock-and-flow view -- the editing entry points refuse a
// model that has none -- simulated from time 0 to 100 with a time step of 1
// by Euler's method. It is exactly the empty project the server creates for a
// new model (`src/server/project-creation.ts`, opened from its JSON), so
// every host starts from the same project.
//
// Nothing is compiled or synced until a query needs it, so a host copies a
// project with `simlin_project_replace_contents(simlin_project_new(..), src)`
// for the price of the handle: the copy shares `src`'s datamodel.
//
// Returns NULL and populates `out_error` when `name` is not valid UTF-8.
//
// # Safety
// - `name` must be NULL or a valid NUL-terminated C string
// - `out_error` may be null
// - The returned project must be freed with `simlin_project_unref`
SimlinProject *simlin_project_new(const char *name, SimlinError **out_error);

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

// Gets the project's revision: a counter every change to its contents
// advances (a committed patch, a view edit, a replace, an added model, a
// diagram sync), and no read does. Two calls that return the same revision
// saw the same contents, so a host that caches anything derived from the
// project -- an agent's last read of it, a chart of its last run -- can tell
// whether that cache still describes it. The converse does not hold: a
// revision may advance without a visible change.
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
void simlin_project_get_revision(SimlinProject *project,
                                 uint64_t *out_revision,
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

// Replace the contents of `dst` with the contents of `src`.
//
// `dst` shares `src`'s `datamodel::Project` rather than copying it: the two
// hold one datamodel until either is edited, and the edit copies it first, so
// neither ever sees the other's changes. A copy of a project, such as a host
// keeps for each undo step, therefore costs a reference count, and a project
// that is only copied, read or written never builds a salsa database. When
// `dst` has one, it is re-synced incrementally, so unchanged variables keep
// their cached compile fragments. `src` is only read (its refcount is not
// touched) and may be freed immediately afterwards.
//
// This is the in-place reload primitive: a caller that reloads a project
// from disk opens the new bytes with the matching `simlin_project_open_*`
// function into a scratch project and replaces the live project's contents
// from it, instead of building a new `SimlinProject` -- so it composes with
// every format the open functions support and never needs a per-format
// variant.
//
// # Effect on live handles
//
// A `SimlinModel` holds a pointer to its `SimlinProject` plus a model NAME,
// not a copy of the model, so:
//
// - Every existing `SimlinModel` handle on `dst` stays valid and observes
//   the NEW contents on its next call (variables, equations, sim specs,
//   diagnostics via `simlin_project_get_errors`, a fresh `simlin_sim_new`).
// - A handle whose model name is absent from the replacement is not
//   invalidated: queries through it return `BadModelName`, and a sim
//   created through it fails on its first run with `NotSimulatable` naming
//   the model (`simlin_sim_new` defers compile failures to the run, per its
//   own contract) -- until a model of that name exists again, at which point
//   the same handle works once more.
// - A `SimlinSim` created BEFORE the replace is a stale snapshot for the
//   simulation entry points (`simlin_sim_*`): it was compiled from the old
//   contents and keeps its results and its ability to `reset`/re-run against
//   that compiled program. The sim-bearing ANALYSIS entry points
//   (`simlin_analyze_get_loops_runtime`, `simlin_analyze_get_links`, ...)
//   are not snapshots: they enumerate loops/links from the project's CURRENT
//   contents and read scores out of the stale sim's results by position, so
//   after a replace they mix old results with the new model. Callers should
//   create and run a new sim after a replace before analyzing.
//
// # Locking
//
// `src`'s datamodel lock is taken alone, just long enough to share its
// datamodel, and released BEFORE `dst`'s locks are acquired -- so two threads
// replacing in opposite directions cannot deadlock, and `dst == src` (a
// permitted no-op re-sync) does not self-deadlock. `dst`'s datamodel and db
// locks are then held together, in the datamodel-then-db order used
// project-wide, across both the db re-sync and the datamodel swap, so no
// concurrent reader (`simlin_sim_new`, `simlin_project_get_errors`,
// `simlin_project_apply_patch`) observes the datamodel and the db
// disagreeing.
//
// # Safety
// - `dst` must be a valid pointer to a SimlinProject
// - `src` must be a valid pointer to a SimlinProject
// - `out_error` may be null
void simlin_project_replace_contents(SimlinProject *dst,
                                     const SimlinProject *src,
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

// Open a project from XMILE/STMX format data, also reporting what the file
// holds that the project does not keep
//
// The same open as `simlin_project_open_xmile`. The XMILE reader does not
// keep everything a file can hold: the objects on a view besides its
// diagram (a graph, a text box), interface pages, story mode, a standalone
// graphical function. So a save of the project, over the file or in any
// other format, leaves those out, and a host that saves needs to say so
// when the file opens.
//
// Each kind of loss, in each place it occurs, is one `Warning`-severity,
// wire-`Generic`, kind-`Model` detail on the aggregate `SimlinError` stored
// in `out_collected_errors` (NULL when the file loses nothing; pass NULL to
// discard them, and the open then costs what `simlin_project_open_xmile`
// costs). `message` is `"XMILE import: <reason>"` and `details` the bare
// reason, such as `2 sliders on interface page 1 are not kept: 'Birth Rate'
// and 'Population'`. The aggregate's own message counts the losses by kind
// over the whole file, for a host to show where a row per place would be
// too many: `3 graphs, 2 sliders, and 1 text box in this file are not
// kept`. The warnings describe the file as it was read, and the project
// does not keep them, so a host that shows them holds them itself.
//
// Returns NULL and populates `out_error` on failure, with
// `out_collected_errors` NULL.
//
// # Safety
// - `data` must be a valid pointer to at least `len` bytes
// - `out_collected_errors` may be null
// - `out_error` may be null
// - The returned project must be freed with `simlin_project_unref`
SimlinProject *simlin_project_open_xmile_with_warnings(const uint8_t *data,
                                                       uintptr_t len,
                                                       SimlinError **out_collected_errors,
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

// Open a project from Vensim MDL format data, also reporting what the file
// holds that the project does not keep
//
// The same open as `simlin_project_open_vensim`. The MDL reader does not
// keep everything a file can hold: a sketch's comments, graphs, sliders and
// images, and the custom graphs, tables and reports the file defines. So a
// save of the project, over the file or in any other format, leaves those
// out, and a host that saves needs to say so when the file opens.
//
// Each kind of loss is one `Warning`-severity, wire-`Generic`, kind-`Model`
// detail on the aggregate `SimlinError` stored in `out_collected_errors`
// (NULL when the file loses nothing; pass NULL to discard them, and the
// open then costs what `simlin_project_open_vensim` costs): one per kind
// and sketch view for what a modeler put on a view, and one per kind over
// the whole sketch for what follows from what the diagram does not draw
// (see `simlin_engine::mdl::parse_mdl_with_warnings`). `message` is `"MDL
// import: <reason>"` and `details` the bare reason, such as `29 comments on
// view 'View 1' are not kept, such as 'The World3 Model'`. The aggregate's
// own message counts the losses by kind over the whole file, for a host to
// show where a row per place would be too many: `29 comments, 3 graphs, and
// 7 sliders in this file are not kept`. The warnings describe the file as
// it was read, and the project does not keep them, so a host that shows
// them holds them itself.
//
// Returns NULL and populates `out_error` on failure, with
// `out_collected_errors` NULL.
//
// # Safety
// - `data` must be a valid pointer to at least `len` bytes
// - `out_collected_errors` may be null
// - `out_error` may be null
// - The returned project must be freed with `simlin_project_unref`
SimlinProject *simlin_project_open_vensim_with_warnings(const uint8_t *data,
                                                        uintptr_t len,
                                                        SimlinError **out_collected_errors,
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

// Opens a Vensim VDF (binary simulation data) file from a byte buffer.
//
// Auto-detects the container kind from the file magic: simulation-run files
// (`0x52`), sensitivity-run files (`0x53`), and dataset files (`0x41`) are
// all supported. On success returns a `SimlinResults` handle (release with
// `simlin_results_unref`); on failure returns NULL with an error stored in
// `out_error`.
//
// Malformed input reports an error rather than crashing: the engine's VDF
// readers are total on arbitrary bytes (pinned by the engine's
// truncation/corruption sweep test), and the `catch_unwind` here is
// defense-in-depth for unwind builds (release builds compile with
// panic=abort, where unwinding never starts).
//
// # Safety
// - `data` must point to `len` valid bytes (may be NULL only when `len` is 0)
SimlinResults *simlin_results_open_vdf(const uint8_t *data, uintptr_t len, SimlinError **out_error);

// Increments the reference count of a results handle
//
// # Safety
// - `results` must be a valid pointer to a SimlinResults
void simlin_results_ref(SimlinResults *results);

// Decrements the reference count and frees the results handle if it reaches zero
//
// # Safety
// - `results` must be a valid pointer to a SimlinResults
void simlin_results_unref(SimlinResults *results);

// Gets the number of time steps in the results
//
// # Safety
// - `results` must be a valid pointer to a SimlinResults
void simlin_results_get_stepcount(SimlinResults *results,
                                  uintptr_t *out_count,
                                  SimlinError **out_error);

// Gets the number of named series in the results (including `time`)
//
// # Safety
// - `results` must be a valid pointer to a SimlinResults
void simlin_results_get_var_count(SimlinResults *results,
                                  uintptr_t *out_count,
                                  SimlinError **out_error);

// Gets the (sorted) names of the series in the results.
//
// Call with `max == 0` to query the count without copying names.
//
// # Safety
// - `results` must be a valid pointer to a SimlinResults
// - `result` must be a valid pointer to an array of at least `max` char pointers
// - The returned strings are owned by the caller and must be freed with simlin_free_string
void simlin_results_get_var_names(SimlinResults *results,
                                  char **result,
                                  uintptr_t max,
                                  uintptr_t *out_written,
                                  SimlinError **out_error);

// Gets the time series for a named variable in the results
//
// # Safety
// - `results` must be a valid pointer to a SimlinResults
// - `name` must be a valid C string
// - `results_ptr` must point to allocated memory of at least `len` doubles
void simlin_results_get_series(SimlinResults *results,
                               const char *name,
                               double *results_ptr,
                               uintptr_t len,
                               uintptr_t *out_written,
                               SimlinError **out_error);

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
// - The project's datamodel is held in a `Mutex`, so concurrent readers serialize on it.
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

// Serialize a project to Vensim MDL format
//
// Exports the project's single model (plus any macro-marked models, emitted
// as `:MACRO:` blocks) as Vensim MDL text, including the sketch section for
// a model that has a diagram view. The output buffer contains UTF-8 text.
//
// The MDL surface cannot represent every Simlin construct. The engine's
// lossiness contract (`simlin_engine::mdl::project_to_mdl_with_warnings`)
// splits the gap in two, and this function surfaces both halves separately:
//
// - **Hard errors** (a project with more than one ordinary model, an
//   ordinary module instance, an unreconstructable macro cluster) fail the
//   export: `out_error` is set and no buffer is produced.
// - **Lossiness warnings** (a dropped non-negative flag, a discrete or
//   extrapolating lookup emitted in the closest representable form, a
//   truncated group name, ...) do NOT fail the export. The text is still
//   written, and each warning is reported as a `Warning`-severity detail on
//   `out_collected_errors` (an aggregate `SimlinError`, freed with
//   `simlin_error_free`; NULL when there were no warnings). Pass NULL for
//   `out_collected_errors` to discard the warnings. This mirrors how
//   `simlin_project_apply_patch` separates a rejection (`out_error`) from
//   the diagnostics it collected along the way (`out_collected_errors`).
//
// Caller must free the output buffer with `simlin_free`.
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `out_buffer` and `out_len` must be valid pointers
// - `out_collected_errors` may be null
// - `out_error` may be null
void simlin_project_serialize_mdl(SimlinProject *project,
                                  uint8_t **out_buffer,
                                  uintptr_t *out_len,
                                  SimlinError **out_collected_errors,
                                  SimlinError **out_error);

// Check whether saving a project in a format keeps what its model means
//
// Saves the project in `format` (a `SimlinSaveFormat`), reads the save back
// as a host would open it, and compares the model the save holds with the
// project's two ways (see `simlin_engine::save_check`):
//
// - its definition: the simulation specs, each dimension's elements, parent
//   and mappings, and each variable's kind, dimensions, elements, equations
//   (in the spelling the engine resolves them by), `:EXCEPT:` default,
//   initial values, graphical functions, flows and the flags that change a
//   simulation, whether or not the project simulates;
// - when the project simulates, every variable's series, value for value.
//
// When the check cannot be sure a save keeps the meaning, it reports a
// change. Units, documentation and views are not meaning; a save that
// loses them reports them through the writers' own warnings.
//
// Each change is a wire-`Generic` detail on the aggregate `SimlinError`
// stored in `out_changes`: kind `Variable` with `variable_name` set when one
// variable is at fault, else kind `Model`, with `model_name` set when the
// change is in one model. `message` is `"<format> save: <reason>"` and
// `details` the bare reason, such as `'demands1' is defined over dim2, not
// dim`. The aggregate's own message counts them: `Saving as Vensim MDL
// changes what this model means in 3 ways`.
//
// The verdict is `out_changes` itself: NULL when the save keeps the model's
// meaning, and otherwise the save must not be made in place. A detail's
// severity only grades its change: `Error` when the save's results differ
// from the project's now, `Warning` when only its definition does (an
// equation the run never reaches, a flag it never exercises). When neither
// the project nor its save simulates, every detail is a `Warning`, since
// there are no results to compare.
//
// `data_dir` is the directory the project's external data files are found
// in, as `simlin_project_open_vensim_with_data` takes it, so an MDL save's
// data references resolve as the project's did. Pass NULL for a project
// opened without one. As there, it is read only when the `file_io` feature
// is enabled.
//
// Fails, with `out_error` set and `out_changes` NULL, when `format` is not a
// `SimlinSaveFormat` or cannot hold the project at all (MDL holds one model),
// as the serialize functions fail.
//
// The check saves, reads and simulates the project and its save, so it
// takes as long as those do: a host runs it off its main thread. It holds
// the project's lock only to share its datamodel.
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `data_dir` may be null; when non-null it must point to `data_dir_len`
//   bytes of valid UTF-8 naming a directory
// - `out_changes` must be a valid pointer
// - `out_error` may be null
void simlin_project_check_save(SimlinProject *project,
                               uint32_t format,
                               const uint8_t *data_dir,
                               uintptr_t data_dir_len,
                               SimlinError **out_changes,
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
// A model without a stock-and-flow view (e.g. one built programmatically
// through the patch API) is rendered with an automatically generated
// layout; the generated view is transient and not persisted.
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

// Render a project model's diagram as a scene display list
//
// Returns the stock-and-flow diagram for the named model as a
// resolution-independent display list, UTF-8 JSON: each drawn element's
// shapes (rectangles, circles, and paths of move, line, cubic and close
// commands), label lines and sparkline slot, in draw order, with every SVG
// arc converted to cubics and every SVG transform applied. The geometry is
// the geometry `simlin_project_render_svg` draws; `docs/design/diagram-scene.md`
// is the format's contract.
//
// A model without a stock-and-flow view (e.g. one built programmatically
// through the patch API) is rendered with an automatically generated
// layout; the generated view is transient and not persisted.
//
// Caller must free output with `simlin_free`.
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `model_name` must be a valid null-terminated UTF-8 string
// - `out_buffer` and `out_len` must be valid pointers
// - `out_error` may be null
void simlin_project_render_scene(SimlinProject *project,
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
// A model without a stock-and-flow view (e.g. one built programmatically
// through the patch API) is rendered with an automatically generated
// layout; the generated view is transient and not persisted.
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
// `enable_ltm` requests Loops That Matter instrumentation. For an ordinary
// model this produces a sim whose results carry the LTM link/loop-score
// series. For a model containing a conveyor or queue stock, LTM is a
// documented degradation: the flow-to-stock link score treats a stock's net
// flow as its rate of change (plain INTEG), which neither special stock is,
// so the sim is created WITHOUT LTM instrumentation and
// `simlin_sim_get_ltm_mode` reports `Disabled`. `enable_ltm = true` is still
// honored as a request in that case:
// `simlin_project_get_errors` will surface a `ConveyorLtmDegraded` /
// `QueueLtmDegraded` `Warning` naming the offending stock, so the caller learns
// why scores are absent instead of the request being silently dropped.
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

// Sets the value for a simple-constant variable at the last saved timestep
// by data-buffer offset.
//
// This edits the LAST ROW of the saved results table in place -- it is only
// usable after `simlin_sim_run_to_end` has produced results, and it does NOT
// stage an override for the next `simlin_sim_reset` (use
// `simlin_sim_set_value` for the persistent-override contract).
//
// The offset is validated the same way `simlin_sim_set_value` validates a
// name: only an overridable constant -- a constant of the compiled program
// that no conveyor/queue pass writes, `SimState::is_overridable_offset` --
// is writable; any computed variable's offset rejects with `BadOverride` so
// saved simulation output cannot be silently rewritten.
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

// Write the tool catalog -- every tool's name, description, effect, and the
// JSON Schema of its input and of its output -- as UTF-8 JSON to a buffer the
// caller frees with `simlin_free`: `{"tools": [{"name", "description",
// "effect", "inputSchema", "outputSchema"}, ...]}`.
//
// # Safety
// - `out_buf` and `out_len` must be valid pointers
void simlin_tools_describe(uint8_t **out_buf, uintptr_t *out_len, SimlinError **out_error);

// Make a tool session over `model`, which it keeps alive until the session's
// last reference is dropped with `simlin_tool_session_unref`.
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
SimlinToolSession *simlin_tool_session_new(SimlinModel *model, SimlinError **out_error);

// Increment a tool session's reference count.
//
// # Safety
// - `session` must be a valid pointer to a SimlinToolSession, or NULL
void simlin_tool_session_ref(SimlinToolSession *session);

// Decrement a tool session's reference count, releasing it (and its reference
// to its model) at zero.
//
// # Safety
// - `session` must be a valid pointer to a SimlinToolSession, or NULL
void simlin_tool_session_unref(SimlinToolSession *session);

// Answer a call of the tool named `name` with `input` (`input_len` bytes of
// UTF-8 JSON; empty means `{}`), writing the output JSON to a buffer the
// caller frees with `simlin_free` and whether it is a refusal to
// `out_is_error`. A refusal names the rule the call broke and the repair,
// for the agent to read: a host hands it back as the tool's result, never as
// an exception that ends the agent's turn.
//
// A call reads the project as it is when the call starts, at that revision.
// It holds the session for the call, and the project's datamodel only while
// it takes the contents it answers from (shared, not copied) and their
// revision: it then answers under the database lock alone, so a host's hit
// tests, planners and revision reads, which lock only the datamodel, never
// wait behind an analysis. An entry point that holds the datamodel and waits
// for the database meanwhile -- an edit landing (`simlin_project_apply_patch`,
// or an undo's `simlin_project_replace_contents`), a simulation
// (`simlin_sim_new`), a read of the diagnostics, the others
// `SimlinProject::waiting_for_db` lists -- keeps those readers waiting with
// it, so the call stops for it between units of its work (a simulation, a
// stage of an analysis) and answers a refusal with `"interrupted": true` that
// kept nothing: the entry point waits at most one unit. A host that retries
// by itself does so once that work is done -- after an edit, at the next
// revision -- and never in a loop against a project that stays busy.
//
// # Safety
// - `session` must be a valid pointer to a SimlinToolSession
// - `name` must be a valid C string
// - `input` must point to `input_len` bytes, or be NULL when it is zero
// - `out_buf`, `out_len` and `out_is_error` must be valid pointers
void simlin_tool_session_call(SimlinToolSession *session,
                              const char *name,
                              const uint8_t *input,
                              uintptr_t input_len,
                              uint8_t **out_buf,
                              uintptr_t *out_len,
                              bool *out_is_error,
                              SimlinError **out_error);

// Write what changed in the session's model since the session's last
// `read_model` -- variables added, removed and changed (with which fields),
// and whether the sim specs changed -- as UTF-8 JSON to a buffer the caller
// frees with `simlin_free`, or `null` before the first read and when nothing
// did. A view edit changes nothing an agent read, so it is no change here.
// What a host tells an agent about the person's work before its next turn.
//
// # Safety
// - `session` must be a valid pointer to a SimlinToolSession
// - `out_buf` and `out_len` must be valid pointers
void simlin_tool_session_get_changes(SimlinToolSession *session,
                                     uint8_t **out_buf,
                                     uintptr_t *out_len,
                                     SimlinError **out_error);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* SIMLIN_ENGINE2_H */
