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

// Link polarity for C API
typedef enum {
  SIMLIN_LINK_POLARITY_POSITIVE = 0,
  SIMLIN_LINK_POLARITY_NEGATIVE = 1,
  SIMLIN_LINK_POLARITY_UNKNOWN = 2,
} SimlinLinkPolarity;

// Loop polarity for C API
typedef enum {
  SIMLIN_LOOP_POLARITY_REINFORCING = 0,
  SIMLIN_LOOP_POLARITY_BALANCING = 1,
  SIMLIN_LOOP_POLARITY_UNDETERMINED = 2,
} SimlinLoopPolarity;

// JSON format specifier for C API
typedef enum {
  SIMLIN_JSON_FORMAT_NATIVE = 0,
  SIMLIN_JSON_FORMAT_SDAI = 1,
} SimlinJsonFormat;

// Opaque error structure returned by the API
typedef struct {
  uint8_t _private[0];
} SimlinError;

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
} SimlinErrorDetail;

// Opaque project structure
typedef struct {
  uint8_t _private[0];
} SimlinProject;

// Opaque model structure
typedef struct {
  uint8_t _private[0];
} SimlinModel;

// Single causal link structure
typedef struct {
  char *from;
  char *to;
  SimlinLinkPolarity polarity;
  double *score;
  uintptr_t score_len;
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

// A single feedback loop
typedef struct {
  char *id;
  char **variables;
  uintptr_t var_count;
  SimlinLoopPolarity polarity;
} SimlinLoop;

// List of loops returned by analysis
typedef struct {
  SimlinLoop *loops;
  uintptr_t count;
} SimlinLoops;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

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

// Gets the number of variables in the model
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
void simlin_model_get_var_count(SimlinModel *model, uintptr_t *out_count, SimlinError **out_error);

// Gets the variable names from the model
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - `result` must be a valid pointer to an array of at least `max` char pointers
// - The returned strings are owned by the caller and must be freed with simlin_free_string
void simlin_model_get_var_names(SimlinModel *model,
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
// Returns all causal links detected in the model.
// This includes flow-to-stock, stock-to-flow, and auxiliary-to-auxiliary links.
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
// The value is applied inline during bytecode execution (both initials
// and flows phases) and is also written to the data buffer immediately
// so that `simlin_sim_get_value` reflects the change right away.
// Values persist across `simlin_sim_reset`. Call `simlin_sim_clear_values`
// to remove them.
//
// Can be called even when the VM has been consumed by `simlin_sim_run_to_end`;
// the value will be stored and applied to the next VM created on reset.
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - `name` must be a valid C string
void simlin_sim_set_value(SimlinSim *sim, const char *name, double val, SimlinError **out_error);

// Clears all persistent constant value settings, restoring original compiled values.
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
// Intended for debugging/tests to verify name→offset resolution.
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - `name` must be a valid C string
void simlin_sim_get_offset(SimlinSim *sim,
                           const char *name,
                           uintptr_t *out_offset,
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

// Frees a string returned by the API
//
// # Safety
// - `s` must be a valid pointer returned by simlin API functions that return strings
void simlin_free_string(char *s);

// Gets all feedback loops in the project
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - The returned SimlinLoops must be freed with simlin_free_loops
SimlinLoops *simlin_analyze_get_loops(SimlinProject *project, SimlinError **out_error);

// Frees a SimlinLoops structure
//
// # Safety
// - `loops` must be a valid pointer returned by simlin_analyze_get_loops
void simlin_free_loops(SimlinLoops *loops);

// Gets all causal links in a model
//
// Returns all causal links detected in the model.
// This includes flow-to-stock, stock-to-flow, and auxiliary-to-auxiliary links.
// If the simulation has been run with LTM enabled, link scores will be included.
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - The returned SimlinLinks must be freed with simlin_free_links
SimlinLinks *simlin_analyze_get_links(SimlinSim *sim, SimlinError **out_error);

// Frees a SimlinLinks structure
//
// # Safety
// - `links` must be valid pointer returned by simlin_analyze_get_links
void simlin_free_links(SimlinLinks *links);

// Gets the relative loop score time series for a specific loop
//
// Renamed for clarity from simlin_analyze_get_rel_loop_score
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

uint8_t *simlin_malloc(uintptr_t size);

// Frees memory allocated by simlin_malloc
//
// # Safety
// - `ptr` must be a valid pointer returned by simlin_malloc, or null
// - The pointer must not be used after calling this function
void simlin_free(uint8_t *ptr);

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
                                   uint8_t **out_buffer,
                                   uintptr_t *out_len,
                                   SimlinError **out_error);

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
// - The underlying `engine::Project` uses `Arc<ModelStage1>` and is protected by a `Mutex`.
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
// # Example Usage (C)
// ```c
// SimlinError* errors = simlin_project_get_errors(project, NULL);
// if (errors != NULL) {
//     uintptr_t count = simlin_error_get_detail_count(errors);
//     for (uintptr_t i = 0; i < count; i++) {
//         const SimlinErrorDetail* detail = simlin_error_get_detail(errors, i);
//         if (detail == NULL) {
//             continue;
//         }
//         printf("Error %d", detail->code);
//         if (detail->model_name != NULL) {
//             printf(" in model %s", detail->model_name);
//         }
//         if (detail->variable_name != NULL) {
//             printf(" for variable %s", detail->variable_name);
//         }
//         printf("\n");
//     }
//     simlin_error_free(errors);
// } else {
//     // Project has no errors and is ready to simulate
// }
// ```
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - The returned pointer must be freed with `simlin_error_free`
SimlinError *simlin_project_get_errors(SimlinProject *project, SimlinError **out_error);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* SIMLIN_ENGINE2_H */
