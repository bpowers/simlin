"""Low-level FFI helpers and lifecycle management for simlin."""

from typing import Optional, Any, List
import weakref

# Import the compiled CFFI extension
from ._clib import ffi, lib

# Registry used by tests to observe outstanding objects
_finalizer_refs: "weakref.WeakValueDictionary[int, Any]" = weakref.WeakValueDictionary()


def _register_finalizer(obj: Any, cleanup_func: Any, *args: Any) -> None:
    """Register a one-shot finalizer and track the object for tests."""
    # weakref.finalize guarantees the callback runs at most once.
    finalizer = weakref.finalize(obj, cleanup_func, *args)
    setattr(obj, "_finalizer", finalizer)
    _finalizer_refs[id(obj)] = obj


def string_to_c(s: Optional[str]) -> Any:
    """Convert Python string to C string.

    Note: The returned memory is managed by CFFI and will be garbage collected.
    For long-lived usage or when passing to C functions that might store the pointer,
    consider using a different approach.
    """
    if s is None:
        return ffi.NULL
    return ffi.new("char[]", s.encode("utf-8"))


def c_to_string(c_str: Any) -> Optional[str]:
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


def extract_error_details(err_ptr: Any) -> List[Any]:
    """Extract error details from a SimlinError pointer.

    Args:
        err_ptr: Pointer to a SimlinError structure

    Returns:
        List of ErrorDetail objects
    """
    from .errors import ErrorDetail, ErrorCode

    if err_ptr == ffi.NULL:
        return []

    details = []
    count = lib.simlin_error_get_detail_count(err_ptr)
    for i in range(count):
        c_detail = lib.simlin_error_get_detail(err_ptr, i)
        if c_detail != ffi.NULL:
            details.append(ErrorDetail(
                code=ErrorCode(c_detail.code),
                message=c_to_string(c_detail.message) or "",
                model_name=c_to_string(c_detail.model_name),
                variable_name=c_to_string(c_detail.variable_name),
                start_offset=c_detail.start_offset,
                end_offset=c_detail.end_offset,
            ))
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

    from .errors import SimlinRuntimeError, SimlinCompilationError, ErrorCode

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
        from .errors import SimlinRuntimeError, ErrorCode
        error_str = get_error_string(result)
        code = None
        try:
            code = ErrorCode(result)
        except ValueError:
            code = None
        raise SimlinRuntimeError(f"{operation} failed: {error_str}", code)


__all__ = [
    "ffi",
    "lib",
    "string_to_c",
    "c_to_string",
    "free_c_string",
    "get_error_string",
    "extract_error_details",
    "check_out_error",
    "check_error",
    "_register_finalizer",
    "_finalizer_refs",
]
