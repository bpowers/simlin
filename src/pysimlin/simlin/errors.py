"""Error handling for the simlin package."""

from dataclasses import dataclass
from enum import IntEnum


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
    BAD_OVERRIDE = 34
    NO_APP_IN_UNITS = 35
    NO_SUBSCRIPT_IN_UNITS = 36
    NO_IF_IN_UNITS = 37
    NO_UNARY_OP_IN_UNITS = 38
    BAD_BINARY_OP_IN_UNITS = 39
    NO_CONST_IN_UNITS = 40
    EXPECTED_INTEGER = 41
    EXPECTED_INTEGER_ONE = 42
    DUPLICATE_UNIT = 43
    EXPECTED_MODULE = 44
    EXPECTED_IDENT = 45


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


class ErrorSeverity(IntEnum):
    """Severity of an error detail.

    ERROR means the model cannot be simulated or a value is wrong; WARNING is an
    advisory (e.g. the LTM auto-flip-to-discovery notice) that does not make the
    model invalid.
    """

    ERROR = 0
    WARNING = 1


@dataclass
class ErrorDetail:
    """Detailed information about a compilation or validation error."""

    code: ErrorCode
    message: str
    model_name: str | None = None
    variable_name: str | None = None
    start_offset: int = 0
    end_offset: int = 0
    kind: ErrorKind = ErrorKind.VARIABLE
    unit_error_kind: UnitErrorKind = UnitErrorKind.NOT_APPLICABLE
    severity: ErrorSeverity = ErrorSeverity.ERROR
    # The bare human-readable reason without the source snippet or summary
    # line that `message` carries (e.g. "the equation computes to units
    # 'people', but the variable's specified units are 'person'"). None when
    # the error has no separate reason string.
    details: str | None = None

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

    def __init__(self, message: str, code: ErrorCode | None = None):
        super().__init__(message)
        self.code = code


class SimlinRuntimeError(SimlinError):
    """Exception raised when the engine rejects an operation.

    Covers everything the FFI reports through an out-error: a rejected model
    edit, a model that fails to compile, and simulation execution failures.
    ``details`` carries the underlying per-variable diagnostics when the
    engine provided any.
    """

    def __init__(
        self,
        message: str,
        code: ErrorCode | None = None,
        details: list[ErrorDetail] | None = None,
    ):
        super().__init__(message, code)
        self.details = details or []


class SimlinImportError(SimlinError):
    """Exception raised when importing a model fails."""

    pass


class SimlinWriteError(SimlinRuntimeError):
    """A change was applied to the in-memory project (its revision advanced
    and it is now ``dirty``) but writing the file failed.

    Raised by :meth:`Project._apply_snapshot` so a caller can tell "applied
    but unwritten" from "not applied" without inspecting the project;
    ``__cause__`` is the underlying ``OSError`` or conflict error, and
    ``revision`` the revision the change produced.  ``save()`` retries.
    """

    def __init__(self, message: str, revision: int):
        super().__init__(message)
        self.revision = revision


class SimlinDependencyError(SimlinError, ImportError):
    """An optional dependency this feature needs is not installed.

    The notebook editor (:meth:`simlin.Model.widget`, displaying a model)
    needs the ``notebook`` extra -- ``pip install "pysimlin[notebook]"`` --
    which a bare ``pip install pysimlin`` deliberately leaves out so
    scripts and servers that never display a model do not carry the
    anywidget/ipywidgets chain.  Also an :class:`ImportError`, so code that
    guards optional imports with ``except ImportError`` keeps working.  The
    message carries the install line for the running host.
    """


class SimlinAssetError(SimlinError):
    """A file that ships inside the pysimlin package -- the notebook widget's
    JS module or engine wasm -- is missing or cannot be delivered.

    Raised when a :class:`simlin.ModelWidget` is created (displaying a
    model), never on ``import simlin``; the message names the file and how
    to get it.
    """
