"""Low-level FFI helpers and lifecycle management for simlin."""

from typing import Optional, Any
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


def get_error_string(error_code: int) -> str:
    c_str = lib.simlin_error_str(error_code)
    if c_str == ffi.NULL:
        return f"Unknown error code: {error_code}"
    return ffi.string(c_str).decode("utf-8")


def check_error(result: int, operation: str = "operation") -> None:
    if result != 0:
        from .errors import SimlinRuntimeError, ErrorCode
        error_str = get_error_string(result)
        code = None
        try:
            code = ErrorCode(result)
        except Exception:
            code = None
        raise SimlinRuntimeError(f"{operation} failed: {error_str}", code)


__all__ = [
    "ffi",
    "lib",
    "string_to_c",
    "c_to_string",
    "free_c_string",
    "get_error_string",
    "check_error",
    "_register_finalizer",
    "_finalizer_refs",
]
