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
  SIMLIN_ERROR_CODE_UNIT_MISMATCH = 33,
} SimlinErrorCode;

typedef enum {
  SIMLIN_LINK_POLARITY_POSITIVE = 0,
  SIMLIN_LINK_POLARITY_NEGATIVE = 1,
  SIMLIN_LINK_POLARITY_UNKNOWN = 2,
} SimlinLinkPolarity;

typedef enum {
  SIMLIN_LOOP_POLARITY_REINFORCING = 0,
  SIMLIN_LOOP_POLARITY_BALANCING = 1,
  SIMLIN_LOOP_POLARITY_UNDETERMINED = 2,
} SimlinLoopPolarity;

typedef enum {
  SIMLIN_JSON_FORMAT_NATIVE = 0,
  SIMLIN_JSON_FORMAT_SDAI = 1,
} SimlinJsonFormat;

typedef enum {
  SIMLIN_ERROR_KIND_PROJECT = 0,
  SIMLIN_ERROR_KIND_MODEL = 1,
  SIMLIN_ERROR_KIND_VARIABLE = 2,
  SIMLIN_ERROR_KIND_UNITS = 3,
  SIMLIN_ERROR_KIND_SIMULATION = 4,
} SimlinErrorKind;

typedef enum {
  SIMLIN_UNIT_ERROR_KIND_NOT_APPLICABLE = 0,
  SIMLIN_UNIT_ERROR_KIND_DEFINITION = 1,
  SIMLIN_UNIT_ERROR_KIND_CONSISTENCY = 2,
  SIMLIN_UNIT_ERROR_KIND_INFERENCE = 3,
} SimlinUnitErrorKind;

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
  const char *message;
  const char *model_name;
  const char *variable_name;
  uint16_t start_offset;
  uint16_t end_offset;
  SimlinErrorKind kind;
  SimlinUnitErrorKind unit_error_kind;
} SimlinErrorDetail;

typedef struct SimlinError SimlinError;
typedef SimlinError **OutError;

const char *simlin_error_str(SimlinErrorCode err);
void simlin_error_free(SimlinError *err);
SimlinErrorCode simlin_error_get_code(const SimlinError *err);
const char *simlin_error_get_message(const SimlinError *err);
uintptr_t simlin_error_get_detail_count(const SimlinError *err);
const SimlinErrorDetail *simlin_error_get_details(const SimlinError *err);
const SimlinErrorDetail *simlin_error_get_detail(const SimlinError *err, uintptr_t index);
SimlinProject *simlin_project_open_protobuf(const uint8_t *data, uintptr_t len, OutError out_error);
SimlinProject *simlin_project_open_json(const uint8_t *data, uintptr_t len, SimlinJsonFormat format, OutError out_error);
void simlin_project_ref(SimlinProject *project);
void simlin_project_unref(SimlinProject *project);
void simlin_project_get_model_count(SimlinProject *project, uintptr_t *out_count, OutError out_error);
void simlin_project_get_model_names(SimlinProject *project, char **result, uintptr_t max, uintptr_t *out_written, OutError out_error);
void simlin_project_add_model(SimlinProject *project, const char *model_name, OutError out_error);
SimlinModel *simlin_project_get_model(SimlinProject *project, const char *model_name, OutError out_error);
void simlin_model_ref(SimlinModel *model);
void simlin_model_unref(SimlinModel *model);
void simlin_model_get_var_count(SimlinModel *model, uintptr_t *out_count, OutError out_error);
void simlin_model_get_var_names(SimlinModel *model, char **result, uintptr_t max, uintptr_t *out_written, OutError out_error);
void simlin_model_get_incoming_links(SimlinModel *model, const char *var_name, char **result, uintptr_t max, uintptr_t *out_written, OutError out_error);
SimlinLinks *simlin_model_get_links(SimlinModel *model, OutError out_error);
SimlinSim *simlin_sim_new(SimlinModel *model, bool enable_ltm, OutError out_error);
void simlin_sim_ref(SimlinSim *sim);
void simlin_sim_unref(SimlinSim *sim);
void simlin_sim_run_to(SimlinSim *sim, double time, OutError out_error);
void simlin_sim_run_to_end(SimlinSim *sim, OutError out_error);
void simlin_sim_get_stepcount(SimlinSim *sim, uintptr_t *out_count, OutError out_error);
void simlin_sim_reset(SimlinSim *sim, OutError out_error);
void simlin_sim_get_value(SimlinSim *sim, const char *name, double *out_value, OutError out_error);
void simlin_sim_set_value(SimlinSim *sim, const char *name, double val, OutError out_error);
void simlin_sim_set_value_by_offset(SimlinSim *sim, uintptr_t offset, double val, OutError out_error);
void simlin_sim_get_offset(SimlinSim *sim, const char *name, uintptr_t *out_offset, OutError out_error);
void simlin_sim_get_series(SimlinSim *sim, const char *name, double *results_ptr, uintptr_t len, uintptr_t *out_written, OutError out_error);
void simlin_free_string(char *s);
SimlinLoops *simlin_analyze_get_loops(SimlinProject *project, OutError out_error);
void simlin_free_loops(SimlinLoops *loops);
SimlinLinks *simlin_analyze_get_links(SimlinSim *sim, OutError out_error);
void simlin_free_links(SimlinLinks *links);
void simlin_analyze_get_relative_loop_score(SimlinSim *sim, const char *loop_id, double *results_ptr, uintptr_t len, uintptr_t *out_written, OutError out_error);
void simlin_analyze_get_rel_loop_score(SimlinSim *sim, const char *loop_id, double *results_ptr, uintptr_t len, uintptr_t *out_written, OutError out_error);
uint8_t *simlin_malloc(uintptr_t size);
void simlin_free(uint8_t *ptr);
SimlinProject *simlin_project_open_xmile(const uint8_t *data, uintptr_t len, OutError out_error);
SimlinProject *simlin_project_open_vensim(const uint8_t *data, uintptr_t len, OutError out_error);
void simlin_project_serialize_xmile(SimlinProject *project, uint8_t **out_buffer, uintptr_t *out_len, OutError out_error);
void simlin_project_serialize_protobuf(SimlinProject *project, uint8_t **out_buffer, uintptr_t *out_len, OutError out_error);
void simlin_project_serialize_json(SimlinProject *project, SimlinJsonFormat format, uint8_t **out_buffer, uintptr_t *out_len, OutError out_error);
void simlin_project_apply_patch(SimlinProject *project, const uint8_t *patch_data, uintptr_t patch_len, bool dry_run, bool allow_errors, SimlinError **out_collected_errors, OutError out_error);
SimlinError *simlin_project_get_errors(SimlinProject *project, OutError out_error);
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
