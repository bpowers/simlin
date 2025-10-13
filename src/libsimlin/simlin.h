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
} SimlinErrorCode;

// JSON format specifier for C API
typedef enum {
  SIMLIN_JSON_FORMAT_NATIVE = 0,
  SIMLIN_JSON_FORMAT_SDAI = 1,
} SimlinJsonFormat;

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
} SimlinLoopPolarity;

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
const char *simlin_error_str(SimlinErrorCode err);

void simlin_error_free(SimlinError *err);

SimlinErrorCode simlin_error_get_code(const SimlinError *err);

const char *simlin_error_get_message(const SimlinError *err);

uintptr_t simlin_error_get_detail_count(const SimlinError *err);

const SimlinErrorDetail *simlin_error_get_details(const SimlinError *err);

const SimlinErrorDetail *simlin_error_get_detail(const SimlinError *err, uintptr_t index);

// simlin_project_open opens a project from protobuf data.
// Returns NULL and populates `out_error` on failure.
//
// # Safety
// - `data` must be a valid pointer to at least `len` bytes
// - `out_error` may be null
SimlinProject *simlin_project_open(const uint8_t *data, uintptr_t len, SimlinError ** out_error);

// simlin_project_json_open opens a project from JSON data.
//
// # Safety
// - `data` must be a valid pointer to at least `len` bytes of UTF-8 JSON
// - `out_error` may be null
SimlinProject *simlin_project_json_open(const uint8_t *data,
                                        uintptr_t len,
                                        SimlinJsonFormat format,
                                        SimlinError ** out_error);

// Increments the reference count of a project
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
void simlin_project_ref(SimlinProject *project);

// Decrements the reference count and frees the project if it reaches zero
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
                                    SimlinError ** out_error);

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
                                    SimlinError ** out_error);

// Adds a new model to a project
//
// Creates a new empty model with the given name and adds it to the project.
// The model will have no variables initially.
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `model_name` must be a valid C string
//
// # Returns
// - 0 on success
// - SimlinErrorCode::Generic if project or model_name is null or empty
// - SimlinErrorCode::DuplicateVariable if a model with that name already exists
void simlin_project_add_model(SimlinProject *project, const char *model_name, SimlinError ** out_error);

// Gets a model from a project by name
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `model_name` may be null (uses default model)
// - The returned model must be freed with simlin_model_unref
SimlinModel *simlin_project_get_model(SimlinProject *project,
                                      const char *model_name,
                                      SimlinError ** out_error);

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
void simlin_model_get_var_count(SimlinModel *model, uintptr_t *out_count, SimlinError ** out_error);

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
                                SimlinError ** out_error);

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
                                     SimlinError ** out_error);

// Gets all causal links in a model
//
// Returns all causal links detected in the model.
// This includes flow-to-stock, stock-to-flow, and auxiliary-to-auxiliary links.
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
// - The returned SimlinLinks must be freed with simlin_free_links
SimlinLinks *simlin_model_get_links(SimlinModel *model, SimlinError ** out_error);

// Creates a new simulation context
//
// # Safety
// - `model` must be a valid pointer to a SimlinModel
SimlinSim *simlin_sim_new(SimlinModel *model, bool enable_ltm, SimlinError ** out_error);

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
void simlin_sim_run_to(SimlinSim *sim, double time, SimlinError ** out_error);

// Runs the simulation to completion
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
void simlin_sim_run_to_end(SimlinSim *sim, SimlinError ** out_error);

// Gets the number of time steps in the results
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
void simlin_sim_get_stepcount(SimlinSim *sim, uintptr_t *out_count, SimlinError ** out_error);

// Resets the simulation to its initial state
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
void simlin_sim_reset(SimlinSim *sim, SimlinError ** out_error);

// Gets a single value from the simulation
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - `name` must be a valid C string
// - `result` must be a valid pointer to a double
void simlin_sim_get_value(SimlinSim *sim, const char *name, double *out_value, SimlinError ** out_error);

// Sets a value in the simulation
//
// This function sets values at different phases of simulation:
// - Before first run_to: Sets initial value to be used when simulation starts
// - During simulation (after run_to): Sets value in current data for next iteration
// - After run_to_end: Returns error (simulation complete)
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - `name` must be a valid C string
void simlin_sim_set_value(SimlinSim *sim, const char *name, double val, SimlinError ** out_error);

// Sets the value for a variable at the last saved timestep by offset
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
void simlin_sim_set_value_by_offset(SimlinSim *sim,
                                    uintptr_t offset,
                                    double val,
                                    SimlinError ** out_error);

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
                           SimlinError ** out_error);

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
                           SimlinError ** out_error);

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
SimlinLoops *simlin_analyze_get_loops(SimlinProject *project, SimlinError ** out_error);

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
SimlinLinks *simlin_analyze_get_links(SimlinSim *sim, SimlinError ** out_error);

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
                                            SimlinError ** out_error);

void simlin_analyze_get_rel_loop_score(SimlinSim *sim,
                                       const char *loop_id,
                                       double *results_ptr,
                                       uintptr_t len,
                                       uintptr_t *out_written,
                                       SimlinError ** out_error);

uint8_t *simlin_malloc(uintptr_t size);

// Frees memory allocated by simlin_malloc
//
// # Safety
// - `ptr` must be a valid pointer returned by simlin_malloc, or null
// - The pointer must not be used after calling this function
void simlin_free(uint8_t *ptr);

// simlin_import_xmile opens a project from XMILE/STMX format data.
//
// # Safety
// - `data` must be a valid pointer to at least `len` bytes
// - `out_error` may be null
SimlinProject *simlin_import_xmile(const uint8_t *data, uintptr_t len, SimlinError ** out_error);

// simlin_import_mdl opens a project from Vensim MDL format data.
//
// # Safety
// - `data` must be a valid pointer to at least `len` bytes
// - `out_error` may be null
SimlinProject *simlin_import_mdl(const uint8_t *data, uintptr_t len, SimlinError ** out_error);

// simlin_export_xmile exports a project to XMILE format.
// Returns 0 on success, error code on failure.
// Caller must free output with simlin_free().
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `output` and `output_len` must be valid pointers
void simlin_export_xmile(SimlinProject *project,
                         uint8_t **out_buffer,
                         uintptr_t *out_len,
                         SimlinError ** out_error);

// Serializes a project to binary protobuf format
//
// Returns the project's datamodel serialized as protobuf bytes.
// This is the native format expected by simlin_project_open.
// Useful for saving projects or transferring them between systems.
//
// Returns 0 on success, error code on failure.
// Caller must free output with simlin_free().
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `output` and `output_len` must be valid pointers
void simlin_project_serialize(SimlinProject *project,
                              uint8_t **out_buffer,
                              uintptr_t *out_len,
                              SimlinError ** out_error);

// Serializes a project to JSON format.
//
// The resulting buffer contains UTF-8 JSON matching the native Simlin JSON
// schema. Only the native format is supported.
//
// The caller takes ownership of `out_buffer` and must free it with
// `simlin_free`.
void simlin_project_serialize_json(SimlinProject *project,
                                   SimlinJsonFormat format,
                                   uint8_t **out_buffer,
                                   uintptr_t *out_len,
                                   SimlinError **out_error);

// Applies a patch to the project datamodel.
//
// The patch is encoded as a `project_io.Patch` protobuf message. The caller can
// request a dry run (which performs validation without committing) and control
// whether errors are permitted. When `allow_errors` is false, any static or
// simulation error will cause the patch to be rejected.
//
// On success returns `SimlinErrorCode::NoError`. On failure returns an error
// code describing why the patch could not be applied. When `out_errors` is not
// Applies a patch to the project datamodel.
//
// On success returns without populating `out_error`. When `out_collected_errors` is
// non-null it receives a pointer to a `SimlinError` describing all detected issues; callers
// must free it with `simlin_error_free`.
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `patch_data` must be a valid pointer to at least `patch_len` bytes
// - `out_collected_errors` and `out_error` may be null
void simlin_project_apply_patch(SimlinProject *project,
                                const uint8_t *patch_data,
                                uintptr_t patch_len,
                                bool dry_run,
                                bool allow_errors,
                                SimlinError **out_collected_errors,
                                SimlinError ** out_error);

// Applies a patch described by the native JSON format to the project.
//
// Only the native JSON format is accepted. The payload must be UTF-8 encoded.
// Semantics match `simlin_project_apply_patch`.
void simlin_project_apply_patch_json(SimlinProject *project,
                                     const uint8_t *patch_data,
                                     uintptr_t patch_len,
                                     SimlinJsonFormat format,
                                     bool dry_run,
                                     bool allow_errors,
                                     SimlinError **out_collected_errors,
                                     SimlinError **out_error);

// Get all errors in a project including static analysis and compilation errors
//
// Returns NULL if no errors exist in the project. This function collects all
// static errors (equation parsing, unit checking, etc.) and also attempts to
// compile the "main" model to find any compilation-time errors.
//
// The caller must free the returned error details using `simlin_free_error_details`.
//
// # Example Usage (C)
// ```c
// SimlinErrorDetails* errors = simlin_project_get_errors(project);
// if (errors != NULL) {
//     for (size_t i = 0; i < errors->count; i++) {
//         SimlinErrorDetail* error = &errors->errors[i];
//         printf("Error %d", error->code);
//         if (error->model_name != NULL) {
//             printf(" in model %s", error->model_name);
//         }
//         if (error->variable_name != NULL) {
//             printf(" for variable %s", error->variable_name);
//         }
//         printf("\n");
//     }
//     simlin_free_error_details(errors);
// } else {
//     // Project has no errors and is ready to simulate
// }
// ```
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - The returned pointer must be freed with `simlin_free_error_details`
SimlinError *simlin_project_get_errors(SimlinProject *project, SimlinError ** out_error);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* SIMLIN_ENGINE2_H */
