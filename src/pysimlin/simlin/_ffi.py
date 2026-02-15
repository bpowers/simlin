"""Low-level FFI helpers and lifecycle management for simlin.

Thread-safety: The module-level ``_finalizer_refs`` dictionary is
protected by ``_refs_lock`` so that ``_register_finalizer`` is safe to
call from any thread, including free-threaded Python (PEP 703 / 3.13t+).
"""

from __future__ import annotations

import threading
import weakref
from typing import TYPE_CHECKING, Any

# Import the compiled CFFI extension
from ._clib import ffi, lib

if TYPE_CHECKING:
    from .errors import ErrorDetail

# Lock protecting module-level mutable state (_finalizer_refs).
# Required for free-threaded Python where the GIL no longer serialises
# access to pure-Python containers.
_refs_lock = threading.Lock()

# Registry used by tests to observe outstanding objects.
_finalizer_refs: weakref.WeakValueDictionary[int, Any] = weakref.WeakValueDictionary()


def _register_finalizer(obj: Any, cleanup_func: Any, *args: Any) -> None:
    """Register a one-shot weak-ref finalizer and track *obj* for tests."""
    # weakref.finalize guarantees the callback runs at most once.
    finalizer = weakref.finalize(obj, cleanup_func, *args)
    obj._finalizer = finalizer
    with _refs_lock:
        _finalizer_refs[id(obj)] = obj


def string_to_c(s: str | None) -> Any:
    """Convert Python string to C string.

    Note: The returned memory is managed by CFFI and will be garbage collected.
    For long-lived usage or when passing to C functions that might store the pointer,
    consider using a different approach.
    """
    if s is None:
        return ffi.NULL
    return ffi.new("char[]", s.encode("utf-8"))


def c_to_string(c_str: Any) -> str | None:
    if c_str == ffi.NULL:
        return None
    return ffi.string(c_str).decode("utf-8")


def free_c_string(c_str: Any) -> None:
    if c_str != ffi.NULL:
        lib.simlin_free_string(c_str)


def get_error_string(error_code: Any) -> str:
    """Get error string from SimlinErrorCode.

    Args:
        error_code: Either an int or SimlinErrorCode enum value

    Returns:
        String description of the error
    """
    c_str = lib.simlin_error_str(error_code)
    if c_str == ffi.NULL:
        return f"Unknown error code: {error_code}"
    # Note: simlin_error_str returns a const static string that should NOT be freed
    return ffi.string(c_str).decode("utf-8")


def extract_error_details(err_ptr: Any) -> list[Any]:
    """Extract error details from a SimlinError pointer.

    Args:
        err_ptr: Pointer to a SimlinError structure

    Returns:
        List of ErrorDetail objects
    """
    from .errors import ErrorCode, ErrorDetail, ErrorKind, UnitErrorKind

    if err_ptr == ffi.NULL:
        return []

    details = []
    count = lib.simlin_error_get_detail_count(err_ptr)
    for i in range(count):
        c_detail = lib.simlin_error_get_detail(err_ptr, i)
        if c_detail != ffi.NULL:
            details.append(
                ErrorDetail(
                    code=ErrorCode(c_detail.code),
                    message=c_to_string(c_detail.message) or "",
                    model_name=c_to_string(c_detail.model_name),
                    variable_name=c_to_string(c_detail.variable_name),
                    start_offset=c_detail.start_offset,
                    end_offset=c_detail.end_offset,
                    kind=ErrorKind(c_detail.kind),
                    unit_error_kind=UnitErrorKind(c_detail.unit_error_kind),
                )
            )
    return details


def check_out_error(out_error_ptr: Any, operation: str = "operation") -> None:
    """Check an out_error pointer and raise exception if error present.

    Args:
        out_error_ptr: Pointer to OutError (SimlinError **) to check
        operation: Description of the operation that failed (for error message)

    Raises:
        SimlinCompilationError: If compilation errors with details are present
        SimlinRuntimeError: For other errors
    """
    if out_error_ptr[0] == ffi.NULL:
        return

    err = out_error_ptr[0]
    code = lib.simlin_error_get_code(err)
    msg_ptr = lib.simlin_error_get_message(err)
    message = c_to_string(msg_ptr) or "Unknown error"
    details = extract_error_details(err)

    lib.simlin_error_free(err)

    from .errors import ErrorCode, SimlinCompilationError, SimlinRuntimeError

    try:
        error_code = ErrorCode(code)
    except ValueError:
        error_code = None

    if details and error_code == ErrorCode.VARIABLES_HAVE_ERRORS:
        raise SimlinCompilationError(f"{operation} failed: {message}", details)
    else:
        raise SimlinRuntimeError(f"{operation} failed: {message}", error_code)


def check_error(result: int, operation: str = "operation") -> None:
    """Legacy error checking for int return codes.

    Args:
        result: Integer error code (0 = success, non-zero = error)
        operation: Description of the operation that failed (for error message)

    Raises:
        SimlinRuntimeError: If result is non-zero
    """
    if result != 0:
        from .errors import ErrorCode, SimlinRuntimeError

        error_str = get_error_string(result)
        code = None
        try:
            code = ErrorCode(result)
        except ValueError:
            code = None
        raise SimlinRuntimeError(f"{operation} failed: {error_str}", code)


def apply_patch_json(
    project_ptr: Any,
    patch_json: bytes,
    dry_run: bool,
    allow_errors: bool,
) -> list[ErrorDetail]:
    """Apply a JSON patch to a project.

    Args:
        project_ptr: Pointer to a SimlinProject
        patch_json: JSON-encoded patch data (UTF-8 bytes)
        dry_run: If True, validate without applying changes
        allow_errors: If True, collect errors instead of failing on first error

    Returns:
        List of ErrorDetail objects for collected validation errors

    Raises:
        SimlinRuntimeError or SimlinCompilationError: If operation fails
    """
    c_patch = ffi.new("uint8_t[]", patch_json)
    out_collected_errors_ptr = ffi.new("SimlinError **")
    err_ptr = ffi.new("SimlinError **")

    lib.simlin_project_apply_patch(
        project_ptr,
        c_patch,
        len(patch_json),
        dry_run,
        allow_errors,
        out_collected_errors_ptr,
        err_ptr,
    )
    errors: list[ErrorDetail] = []
    errors_ptr = out_collected_errors_ptr[0]
    try:
        check_out_error(err_ptr, "Apply JSON patch")
        if errors_ptr != ffi.NULL:
            errors = extract_error_details(errors_ptr)
        return errors
    finally:
        if errors_ptr != ffi.NULL:
            lib.simlin_error_free(errors_ptr)


def model_get_var_json(model_ptr: Any, var_name: str) -> bytes | None:
    """Get a single variable from a model as tagged JSON.

    Args:
        model_ptr: Pointer to a SimlinModel
        var_name: Name of the variable to query

    Returns:
        JSON bytes for the variable, or None if the variable does not exist

    Raises:
        SimlinRuntimeError: If the operation fails for a reason other than
            the variable not existing
    """
    output_ptr = ffi.new("uint8_t **")
    output_len_ptr = ffi.new("uintptr_t *")
    err_ptr = ffi.new("SimlinError **")
    c_name = string_to_c(var_name)

    lib.simlin_model_get_var_json(
        model_ptr,
        c_name,
        output_ptr,
        output_len_ptr,
        err_ptr,
    )

    if err_ptr[0] != ffi.NULL:
        err = err_ptr[0]
        code = lib.simlin_error_get_code(err)
        from .errors import ErrorCode

        if code == ErrorCode.DOES_NOT_EXIST.value:
            lib.simlin_error_free(err)
            return None
        # Re-pack and let check_out_error raise
        err_ptr_recheck = ffi.new("SimlinError **")
        err_ptr_recheck[0] = err
        check_out_error(err_ptr_recheck, f"Get variable JSON for '{var_name}'")

    try:
        return bytes(ffi.buffer(output_ptr[0], output_len_ptr[0]))
    finally:
        lib.simlin_free(output_ptr[0])


def model_get_vars_json(model_ptr: Any) -> bytes:
    """Get all variables from a model as a tagged JSON array.

    Args:
        model_ptr: Pointer to a SimlinModel

    Returns:
        JSON-encoded array of variable objects (UTF-8 bytes)

    Raises:
        SimlinRuntimeError: If the operation fails
    """
    output_ptr = ffi.new("uint8_t **")
    output_len_ptr = ffi.new("uintptr_t *")
    err_ptr = ffi.new("SimlinError **")

    lib.simlin_model_get_vars_json(
        model_ptr,
        output_ptr,
        output_len_ptr,
        err_ptr,
    )
    check_out_error(err_ptr, "Get all variables JSON")

    try:
        return bytes(ffi.buffer(output_ptr[0], output_len_ptr[0]))
    finally:
        lib.simlin_free(output_ptr[0])


def model_get_sim_specs_json(model_ptr: Any) -> bytes:
    """Get the effective sim specs for a model as JSON.

    Args:
        model_ptr: Pointer to a SimlinModel

    Returns:
        JSON-encoded sim specs (UTF-8 bytes)

    Raises:
        SimlinRuntimeError: If the operation fails
    """
    output_ptr = ffi.new("uint8_t **")
    output_len_ptr = ffi.new("uintptr_t *")
    err_ptr = ffi.new("SimlinError **")

    lib.simlin_model_get_sim_specs_json(
        model_ptr,
        output_ptr,
        output_len_ptr,
        err_ptr,
    )
    check_out_error(err_ptr, "Get sim specs JSON")

    try:
        return bytes(ffi.buffer(output_ptr[0], output_len_ptr[0]))
    finally:
        lib.simlin_free(output_ptr[0])


def serialize_json(project_ptr: Any) -> bytes:
    """Serialize a project to JSON format.

    Args:
        project_ptr: Pointer to a SimlinProject

    Returns:
        JSON-encoded project data (UTF-8 bytes)

    Raises:
        SimlinRuntimeError: If serialization fails
    """
    output_ptr = ffi.new("uint8_t **")
    output_len_ptr = ffi.new("uintptr_t *")
    err_ptr = ffi.new("SimlinError **")

    # SimlinJsonFormat::Native = 0
    lib.simlin_project_serialize_json(
        project_ptr,
        0,  # SIMLIN_JSON_FORMAT_NATIVE
        output_ptr,
        output_len_ptr,
        err_ptr,
    )
    check_out_error(err_ptr, "Project JSON serialization")

    try:
        return bytes(ffi.buffer(output_ptr[0], output_len_ptr[0]))
    finally:
        lib.simlin_free(output_ptr[0])


def open_json(json_data: bytes) -> Any:
    """Open a project from JSON data.

    Args:
        json_data: JSON-encoded project data (UTF-8 bytes)

    Returns:
        Pointer to SimlinProject

    Raises:
        SimlinRuntimeError: If parsing or opening fails
    """
    c_data = ffi.new("uint8_t[]", json_data)
    err_ptr = ffi.new("SimlinError **")

    # SIMLIN_JSON_FORMAT_NATIVE = 0
    project_ptr = lib.simlin_project_open_json(c_data, len(json_data), 0, err_ptr)
    check_out_error(err_ptr, "Open JSON project")

    return project_ptr


__all__ = [
    "_finalizer_refs",
    "_refs_lock",
    "_register_finalizer",
    "apply_patch_json",
    "c_to_string",
    "check_error",
    "check_out_error",
    "extract_error_details",
    "ffi",
    "free_c_string",
    "get_error_string",
    "lib",
    "model_get_sim_specs_json",
    "model_get_var_json",
    "model_get_vars_json",
    "open_json",
    "serialize_json",
    "string_to_c",
]
