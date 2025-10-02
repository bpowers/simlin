"""
Simlin - Python bindings for the Simlin system dynamics simulation engine.

This package provides a Pythonic interface to the Simlin simulation engine,
allowing you to load, run, and analyze system dynamics models.
"""

__version__ = "0.1.0"

from .errors import (
    SimlinError,
    SimlinCompilationError,
    SimlinRuntimeError,
    SimlinImportError,
    ErrorCode,
    ErrorDetail,
    error_code_to_string,
)
from .analysis import (
    LinkPolarity,
    LoopPolarity,
    Link,
    Loop,
)
from .project import Project, JSON_FORMAT_SIMLIN, JSON_FORMAT_SDAI
from .model import Model
from .sim import Sim
from . import pb

__all__ = [
    # Main classes
    "Project",
    "Model",
    "Sim",
    # Errors
    "SimlinError",
    "SimlinCompilationError",
    "SimlinRuntimeError",
    "SimlinImportError",
    "ErrorCode",
    "ErrorDetail",
    "error_code_to_string",
    # Analysis types
    "LinkPolarity",
    "LoopPolarity",
    "Link",
    "Loop",
    # JSON format constants
    "JSON_FORMAT_SIMLIN",
    "JSON_FORMAT_SDAI",
    "pb",
]
