// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#ifndef SIMLIN_ENGINE2_H
#define SIMLIN_ENGINE2_H

#ifdef __cplusplus
extern "C" {
#endif

#include <stddef.h>
#include <stdint.h>

typedef enum {
    SIMLIN_ERR_NO_ERROR = 0,
    SIMLIN_ERR_DOES_NOT_EXIST = 1,
    SIMLIN_ERR_XML_DESERIALIZATION = 2,
    SIMLIN_ERR_VENSIM_CONVERSION = 3,
    SIMLIN_ERR_PROTOBUF_DECODE = 4,
    SIMLIN_ERR_INVALID_TOKEN = 5,
    SIMLIN_ERR_UNRECOGNIZED_EOF = 6,
    SIMLIN_ERR_UNRECOGNIZED_TOKEN = 7,
    SIMLIN_ERR_EXTRA_TOKEN = 8,
    SIMLIN_ERR_UNCLOSED_COMMENT = 9,
    SIMLIN_ERR_UNCLOSED_QUOTED_IDENT = 10,
    SIMLIN_ERR_EXPECTED_NUMBER = 11,
    SIMLIN_ERR_UNKNOWN_BUILTIN = 12,
    SIMLIN_ERR_BAD_BUILTIN_ARGS = 13,
    SIMLIN_ERR_EMPTY_EQUATION = 14,
    SIMLIN_ERR_BAD_MODULE_INPUT_DST = 15,
    SIMLIN_ERR_BAD_MODULE_INPUT_SRC = 16,
    SIMLIN_ERR_NOT_SIMULATABLE = 17,
    SIMLIN_ERR_BAD_TABLE = 18,
    SIMLIN_ERR_BAD_SIM_SPECS = 19,
    SIMLIN_ERR_NO_ABSOLUTE_REFERENCES = 20,
    SIMLIN_ERR_CIRCULAR_DEPENDENCY = 21,
    SIMLIN_ERR_ARRAYS_NOT_IMPLEMENTED = 22,
    SIMLIN_ERR_MULTI_DIMENSIONAL_ARRAYS_NOT_IMPLEMENTED = 23,
    SIMLIN_ERR_BAD_DIMENSION_NAME = 24,
    SIMLIN_ERR_BAD_MODEL_NAME = 25,
    SIMLIN_ERR_MISMATCHED_DIMENSIONS = 26,
    SIMLIN_ERR_ARRAY_REFERENCE_NEEDS_EXPLICIT_SUBSCRIPTS = 27,
    SIMLIN_ERR_DUPLICATE_VARIABLE = 28,
    SIMLIN_ERR_UNKNOWN_DEPENDENCY = 29,
    SIMLIN_ERR_VARIABLES_HAVE_ERRORS = 30,
    SIMLIN_ERR_UNIT_DEFINITION_ERRORS = 31,
    SIMLIN_ERR_GENERIC = 32,
} SimlinErrorCode;

typedef enum {
    SIMLIN_LOOP_REINFORCING = 0,
    SIMLIN_LOOP_BALANCING = 1,
} SimlinLoopPolarity;

typedef struct SimlinProject_s SimlinProject;
typedef struct SimlinSim_s SimlinSim;

typedef struct {
    char *id;
    char **variables;
    size_t var_count;
    SimlinLoopPolarity polarity;
} SimlinLoop;

typedef struct {
    SimlinLoop *loops;
    size_t count;
} SimlinLoops;

/// simlin_error_str returns a string representation of an error code.
/// The returned string must not be freed or modified.
const char *simlin_error_str(int err);

/// simlin_project_open opens a project from protobuf data.
/// If an error occurs, the function returns NULL and if the err parameter
/// is not NULL, details of the error are placed in it.
SimlinProject *simlin_project_open(const uint8_t *data, size_t len, int *err);
void simlin_project_ref(SimlinProject *project);
void simlin_project_unref(SimlinProject *project);

/// simlin_project_enable_ltm enables Loop Thinking Method analysis on the project.
/// Returns 0 on success, error code on failure.
int simlin_project_enable_ltm(SimlinProject *project);

/// simlin_sim_new creates a new simulation context for the named model.
/// If model_name is NULL, the context is created for the default/root
/// model in the project.
SimlinSim *simlin_sim_new(SimlinProject *project, const char *model_name);
void simlin_sim_ref(SimlinSim *sim);
void simlin_sim_unref(SimlinSim *sim);

int simlin_sim_run_to(SimlinSim *sim, double time);
int simlin_sim_run_to_end(SimlinSim *sim);
int simlin_sim_get_stepcount(SimlinSim *sim);
int simlin_sim_get_varcount(SimlinSim *sim);
int simlin_sim_get_varnames(SimlinSim *sim, const char **result, size_t max);

int simlin_sim_reset(SimlinSim *sim);

int simlin_sim_get_value(SimlinSim *sim, const char *name, double *result);
int simlin_sim_set_value(SimlinSim *sim, const char *name, double val);
int simlin_sim_get_series(SimlinSim *sim, const char *name, double *results, size_t len);

/// simlin_analyze_get_loops returns all feedback loops in the project.
/// The returned SimlinLoops must be freed with simlin_free_loops.
SimlinLoops *simlin_analyze_get_loops(SimlinProject *project);
void simlin_free_loops(SimlinLoops *loops);

/// simlin_analyze_get_rel_loop_score gets the relative loop score time series.
/// Returns the number of values written, or -1 on error.
int simlin_analyze_get_rel_loop_score(SimlinSim *sim, const char *loop_id, 
                                       double *results, size_t len);

/// simlin_free_string frees a string returned by the API.
void simlin_free_string(char *s);

/// Memory management functions for use across FFI boundary
void *simlin_malloc(size_t size);
void simlin_free(void *ptr);

#ifdef __cplusplus
}
#endif

#endif // SIMLIN_ENGINE2_H