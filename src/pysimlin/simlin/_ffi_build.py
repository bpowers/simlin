"""CFFI build configuration for simlin."""

import os
import platform
from pathlib import Path
from cffi import FFI

ffibuilder = FFI()

# Paths
repo_root = Path(__file__).resolve().parents[3]
libsimlin_dir = repo_root / "src" / "libsimlin"
header_path = libsimlin_dir / "simlin.h"

# C declarations for CFFI (subset matching the public header)
cdef_content = """
typedef size_t uintptr_t;  // align with header semantics for cffi

typedef enum {
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

typedef enum {
  SIMLIN_LINK_POLARITY_POSITIVE = 0,
  SIMLIN_LINK_POLARITY_NEGATIVE = 1,
  SIMLIN_LINK_POLARITY_UNKNOWN = 2,
} SimlinLinkPolarity;

typedef enum {
  SIMLIN_LOOP_POLARITY_REINFORCING = 0,
  SIMLIN_LOOP_POLARITY_BALANCING = 1,
} SimlinLoopPolarity;

typedef struct SimlinModel SimlinModel;
typedef struct SimlinProject SimlinProject;
typedef struct SimlinSim SimlinSim;

typedef struct {
  char *from;
  char *to;
  SimlinLinkPolarity polarity;
  double *score;
  uintptr_t score_len;
} SimlinLink;

typedef struct {
  SimlinLink *links;
  uintptr_t count;
} SimlinLinks;

typedef struct {
  char *id;
  char **variables;
  uintptr_t var_count;
  SimlinLoopPolarity polarity;
} SimlinLoop;

typedef struct {
  SimlinLoop *loops;
  uintptr_t count;
} SimlinLoops;

typedef struct {
  SimlinErrorCode code;
  char *message;
  char *model_name;
  char *variable_name;
  uint16_t start_offset;
  uint16_t end_offset;
} SimlinErrorDetail;

typedef struct {
  SimlinErrorDetail *errors;
  uintptr_t count;
} SimlinErrorDetails;

const char *simlin_error_str(int err);
SimlinProject *simlin_project_open(const uint8_t *data, uintptr_t len, int *err);
void simlin_project_ref(SimlinProject *project);
void simlin_project_unref(SimlinProject *project);
int simlin_project_get_model_count(SimlinProject *project);
int simlin_project_get_model_names(SimlinProject *project, char **result, uintptr_t max);
SimlinModel *simlin_project_get_model(SimlinProject *project, const char *model_name);
void simlin_model_ref(SimlinModel *model);
void simlin_model_unref(SimlinModel *model);
int simlin_model_get_var_count(SimlinModel *model);
int simlin_model_get_var_names(SimlinModel *model, char **result, uintptr_t max);
int simlin_model_get_incoming_links(SimlinModel *model, const char *var_name, char **result, uintptr_t max);
SimlinLinks *simlin_model_get_links(SimlinModel *model);
SimlinSim *simlin_sim_new(SimlinModel *model, bool enable_ltm);
void simlin_sim_ref(SimlinSim *sim);
void simlin_sim_unref(SimlinSim *sim);
int simlin_sim_run_to(SimlinSim *sim, double time);
int simlin_sim_run_to_end(SimlinSim *sim);
int simlin_sim_get_stepcount(SimlinSim *sim);
int simlin_sim_reset(SimlinSim *sim);
int simlin_sim_get_value(SimlinSim *sim, const char *name, double *result);
int simlin_sim_set_value(SimlinSim *sim, const char *name, double val);
int simlin_sim_set_value_by_offset(SimlinSim *sim, uintptr_t offset, double val);
int simlin_sim_get_offset(SimlinSim *sim, const char *name);
int simlin_sim_get_series(SimlinSim *sim, const char *name, double *results_ptr, uintptr_t len);
void simlin_free_string(char *s);
SimlinLoops *simlin_analyze_get_loops(SimlinProject *project);
void simlin_free_loops(SimlinLoops *loops);
SimlinLinks *simlin_analyze_get_links(SimlinSim *sim);
void simlin_free_links(SimlinLinks *links);
int simlin_analyze_get_relative_loop_score(SimlinSim *sim, const char *loop_id, double *results_ptr, uintptr_t len);
int simlin_analyze_get_rel_loop_score(SimlinSim *sim, const char *loop_id, double *results_ptr, uintptr_t len);
uint8_t *simlin_malloc(uintptr_t size);
void simlin_free(uint8_t *ptr);
SimlinProject *simlin_import_xmile(const uint8_t *data, uintptr_t len, int *err);
SimlinProject *simlin_import_mdl(const uint8_t *data, uintptr_t len, int *err);
int simlin_export_xmile(SimlinProject *project, uint8_t **output, uintptr_t *output_len);
int simlin_project_serialize(SimlinProject *project, uint8_t **output, uintptr_t *output_len);
SimlinErrorDetails *simlin_project_get_errors(SimlinProject *project);
void simlin_free_error_details(SimlinErrorDetails *details);
void simlin_free_error_detail(SimlinErrorDetail *detail);
"""

ffibuilder.cdef(cdef_content)


def get_library_path() -> str:
    lib_name = "libsimlin.a"
    # Common locations: native release, cross-target release, then debug
    workspace_target = repo_root / "target"
    candidates = [
        # Workspace targets (default for cargo in a workspace)
        workspace_target / "release" / lib_name,
        *[p for p in workspace_target.glob("*/release/" + lib_name)],
        # Crate-local targets (if building in crate dir)
        libsimlin_dir / "target" / "release" / lib_name,
        *[p for p in (libsimlin_dir / "target").glob("*/release/" + lib_name)],
        libsimlin_dir / "target" / "debug" / lib_name,
    ]
    for path in candidates:
        if path.exists():
            return str(path)
    raise RuntimeError(
        f"libsimlin library not found. Build with: cd {libsimlin_dir} && cargo build --release"
    )


asan_enabled = str(os.environ.get("ASAN", "")).lower() in ("1", "true", "yes", "on")

extra_compile_args = []
extra_link_args = []

if platform.system() == "Linux":
    extra_link_args.append("-lm")
    extra_link_args.append("-lstdc++")

if asan_enabled:
    # Ensure the CFFI module is compiled and linked with ASan when consuming a
    # sanitized libsimlin.a. This avoids unresolved sanitizer symbols.
    extra_compile_args.append("-fsanitize=address")
    extra_link_args.append("-fsanitize=address")

ffibuilder.set_source(
    "simlin._clib",
    """
    #include <stdint.h>
    #include <stdbool.h>
    #include "simlin.h"
    """,
    include_dirs=[str(libsimlin_dir)],
    libraries=[],
    extra_objects=[get_library_path()],
    extra_link_args=extra_link_args,
    extra_compile_args=extra_compile_args,
)


if __name__ == "__main__":
    ffibuilder.compile(verbose=True)
