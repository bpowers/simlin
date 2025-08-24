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

// Loop polarity for C API
typedef enum {
  SIMLIN_LOOP_POLARITY_REINFORCING = 0,
  SIMLIN_LOOP_POLARITY_BALANCING = 1,
} SimlinLoopPolarity;

// Opaque project structure
typedef struct {
  uint8_t _private[0];
} SimlinProject;

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

// Error detail structure containing error message and location
typedef struct {
  SimlinErrorCode code;
  char *message;
  char *model_name;
  char *variable_name;
  uint16_t start_offset;
  uint16_t end_offset;
} SimlinErrorDetail;

// Collection of error details
typedef struct {
  SimlinErrorDetail *errors;
  uintptr_t count;
} SimlinErrorDetails;

#ifdef __cplusplus
extern "C" {
#endif // __cplusplus

// simlin_error_str returns a string representation of an error code.
// The returned string must not be freed or modified.
const char *simlin_error_str(int err);

// simlin_project_open opens a project from protobuf data.
// If an error occurs, the function returns NULL and if the err parameter
// is not NULL, details of the error are placed in it.
//
// # Safety
// - `data` must be a valid pointer to at least `len` bytes
// - `err` may be null
SimlinProject *simlin_project_open(const uint8_t *data, uintptr_t len, int *err);

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

// Enables LTM (Loops That Matter) analysis on a project
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
int simlin_project_enable_ltm(SimlinProject *project);

// Creates a new simulation context
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `model_name` may be null (uses default model)
SimlinSim *simlin_sim_new(SimlinProject *project, const char *model_name);

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
int simlin_sim_run_to(SimlinSim *sim, double time);

// Runs the simulation to completion
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
int simlin_sim_run_to_end(SimlinSim *sim);

// Gets the number of time steps in the results
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
int simlin_sim_get_stepcount(SimlinSim *sim);

// Gets the number of variables in the model
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
int simlin_sim_get_varcount(SimlinSim *sim);

// Gets the variable names
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - `result` must be a valid pointer to an array of at least `max` char pointers
// - The returned strings are owned by the simulation and must not be freed
int simlin_sim_get_varnames(SimlinSim *sim, const char **result, uintptr_t max);

// Resets the simulation to its initial state
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
int simlin_sim_reset(SimlinSim *sim);

// Gets a single value from the simulation
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - `name` must be a valid C string
// - `result` must be a valid pointer to a double
int simlin_sim_get_value(SimlinSim *sim, const char *name, double *result);

// Sets a value in the simulation
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - `name` must be a valid C string
int simlin_sim_set_value(SimlinSim *sim, const char *name, double val);

// Sets the value for a variable at the last saved timestep by offset
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
int simlin_sim_set_value_by_offset(SimlinSim *sim, uintptr_t offset, double val);

// Gets the column offset for a variable by name
//
// Returns the column offset for a variable name at the current context, or -1 if not found.
// This canonicalizes the name and resolves in the VM if present, otherwise in results.
// Intended for debugging/tests to verify name→offset resolution.
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - `name` must be a valid C string
int simlin_sim_get_offset(SimlinSim *sim, const char *name);

// Gets a time series for a variable
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim
// - `name` must be a valid C string
// - `results_ptr` must point to allocated memory of at least `len` doubles
int simlin_sim_get_series(SimlinSim *sim, const char *name, double *results_ptr, uintptr_t len);

// Frees a string returned by the API
//
// # Safety
// - `s` must be a valid pointer returned by simlin_sim_get_varnames
void simlin_free_string(char *s);

// Gets all feedback loops in the project
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - The returned SimlinLoops must be freed with simlin_free_loops
SimlinLoops *simlin_analyze_get_loops(SimlinProject *project);

// Frees a SimlinLoops structure
//
// # Safety
// - `loops` must be a valid pointer returned by simlin_analyze_get_loops
void simlin_free_loops(SimlinLoops *loops);

// Gets the relative loop score time series for a specific loop
//
// # Safety
// - `sim` must be a valid pointer to a SimlinSim that has been run to completion
// - `loop_id` must be a valid C string
// - `results` must be a valid pointer to an array of at least `len` doubles
int simlin_analyze_get_rel_loop_score(SimlinSim *sim,
                                      const char *loop_id,
                                      double *results_ptr,
                                      uintptr_t len);

uint8_t *simlin_malloc(uintptr_t size);

// Frees memory allocated by simlin_malloc
//
// # Safety
// - `ptr` must be a valid pointer returned by simlin_malloc, or null
// - The pointer must not be used after calling this function
void simlin_free(uint8_t *ptr);

// simlin_import_xmile opens a project from XMILE/STMX format data.
// If an error occurs, the function returns NULL and if the err parameter
// is not NULL, details of the error are placed in it.
//
// # Safety
// - `data` must be a valid pointer to at least `len` bytes
// - `err` may be null
SimlinProject *simlin_import_xmile(const uint8_t *data, uintptr_t len, int *err);

// simlin_import_mdl opens a project from Vensim MDL format data.
// If an error occurs, the function returns NULL and if the err parameter
// is not NULL, details of the error are placed in it.
//
// # Safety
// - `data` must be a valid pointer to at least `len` bytes
// - `err` may be null
SimlinProject *simlin_import_mdl(const uint8_t *data, uintptr_t len, int *err);

// simlin_export_xmile exports a project to XMILE format.
// Returns 0 on success, error code on failure.
// Caller must free output with simlin_free().
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
// - `output` and `output_len` must be valid pointers
int simlin_export_xmile(SimlinProject *project, uint8_t **output, uintptr_t *output_len);

// Get all errors in a project including static analysis and compilation errors
// Returns NULL if no errors, caller must free with simlin_free_error_details
//
// # Safety
// - `project` must be a valid pointer to a SimlinProject
SimlinErrorDetails *simlin_project_get_errors(SimlinProject *project);

// Free error details returned by the API
//
// # Safety
// - `details` must be a valid pointer returned by simlin_project_get_errors
void simlin_free_error_details(SimlinErrorDetails *details);

// Free a single error detail
//
// # Safety
// - `detail` must be a valid pointer to a SimlinErrorDetail
void simlin_free_error_detail(SimlinErrorDetail *detail);

#ifdef __cplusplus
}  // extern "C"
#endif  // __cplusplus

#endif  /* SIMLIN_ENGINE2_H */
