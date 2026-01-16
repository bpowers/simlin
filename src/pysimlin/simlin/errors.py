"""Error handling for the simlin package."""

from enum import IntEnum
from typing import Optional, TypedDict
from dataclasses import dataclass


class ErrorCode(IntEnum):
    """Error codes from the Simlin engine."""

    NO_ERROR = 0
    DOES_NOT_EXIST = 1
    XML_DESERIALIZATION = 2
    VENSIM_CONVERSION = 3
    PROTOBUF_DECODE = 4
    INVALID_TOKEN = 5
    UNRECOGNIZED_EOF = 6
    UNRECOGNIZED_TOKEN = 7
    EXTRA_TOKEN = 8
    UNCLOSED_COMMENT = 9
    UNCLOSED_QUOTED_IDENT = 10
    EXPECTED_NUMBER = 11
    UNKNOWN_BUILTIN = 12
    BAD_BUILTIN_ARGS = 13
    EMPTY_EQUATION = 14
    BAD_MODULE_INPUT_DST = 15
    BAD_MODULE_INPUT_SRC = 16
    NOT_SIMULATABLE = 17
    BAD_TABLE = 18
    BAD_SIM_SPECS = 19
    NO_ABSOLUTE_REFERENCES = 20
    CIRCULAR_DEPENDENCY = 21
    ARRAYS_NOT_IMPLEMENTED = 22
    MULTI_DIMENSIONAL_ARRAYS_NOT_IMPLEMENTED = 23
    BAD_DIMENSION_NAME = 24
    BAD_MODEL_NAME = 25
    MISMATCHED_DIMENSIONS = 26
    ARRAY_REFERENCE_NEEDS_EXPLICIT_SUBSCRIPTS = 27
    DUPLICATE_VARIABLE = 28
    UNKNOWN_DEPENDENCY = 29
    VARIABLES_HAVE_ERRORS = 30
    UNIT_DEFINITION_ERRORS = 31
    GENERIC = 32
    UNIT_MISMATCH = 33


class ErrorKind(IntEnum):
    """Error kind categorizing where in the project the error originates."""

    PROJECT = 0
    MODEL = 1
    VARIABLE = 2
    UNITS = 3
    SIMULATION = 4


class UnitErrorKind(IntEnum):
    """Unit error kind for distinguishing types of unit-related errors."""

    NOT_APPLICABLE = 0
    DEFINITION = 1
    CONSISTENCY = 2
    INFERENCE = 3


class ErrorDetailDict(TypedDict, total=False):
    """Type definition for error details dictionary."""

    code: ErrorCode
    message: str
    model_name: str
    variable_name: str
    start_offset: int
    end_offset: int
    kind: ErrorKind
    unit_error_kind: UnitErrorKind


@dataclass
class ErrorDetail:
    """Detailed information about a compilation or validation error."""

    code: ErrorCode
    message: str
    model_name: Optional[str] = None
    variable_name: Optional[str] = None
    start_offset: int = 0
    end_offset: int = 0
    kind: ErrorKind = ErrorKind.VARIABLE
    unit_error_kind: UnitErrorKind = UnitErrorKind.NOT_APPLICABLE
    
    def __str__(self) -> str:
        """Return a human-readable string representation."""
        parts = [f"Error {self.code.name}"]
        
        if self.model_name:
            parts.append(f"in model '{self.model_name}'")
        
        if self.variable_name:
            parts.append(f"for variable '{self.variable_name}'")
            
        if self.message:
            parts.append(f": {self.message}")
            
        if self.start_offset or self.end_offset:
            parts.append(f" (at {self.start_offset}:{self.end_offset})")
            
        return " ".join(parts)


class SimlinError(Exception):
    """Base exception for all Simlin errors."""
    
    def __init__(self, message: str, code: Optional[ErrorCode] = None):
        super().__init__(message)
        self.code = code


class SimlinCompilationError(SimlinError):
    """Exception raised when model compilation fails."""
    
    def __init__(self, message: str, errors: Optional[list[ErrorDetail]] = None):
        super().__init__(message)
        self.errors = errors or []


class SimlinRuntimeError(SimlinError):
    """Exception raised during simulation execution."""
    pass


class SimlinImportError(SimlinError):
    """Exception raised when importing a model fails."""
    pass


def error_code_to_string(code: int) -> str:
    """Convert an error code to a human-readable string."""
    try:
        error = ErrorCode(code)
        return error.name.replace("_", " ").title()
    except ValueError:
        return f"Unknown Error Code ({code})"