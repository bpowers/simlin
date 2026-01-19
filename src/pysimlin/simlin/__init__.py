"""
Simlin - Python bindings for the Simlin system dynamics simulation engine.

This package provides a Pythonic interface to the Simlin simulation engine,
allowing you to load, run, and analyze system dynamics models.
"""

__version__ = "0.1.0"

from typing import Union
from pathlib import Path

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
from .types import (
    TimeSpec,
    GraphicalFunctionScale,
    GraphicalFunction,
    Stock,
    Flow,
    Aux,
    ModelIssue,
    UnitIssue,
)
from .run import (
    Run,
    DominantPeriod,
)
from .project import Project, JSON_FORMAT_SIMLIN, JSON_FORMAT_SDAI
from .model import Model
from .sim import Sim


def load(path: Union[str, Path]) -> Model:
    """
    Load a system dynamics model from file.

    Supports XMILE (.stmx, .xmile), Vensim MDL (.mdl), SDAI JSON, and native JSON formats.
    Always returns the default/main model. For multi-model projects,
    access other models via model.project.get_model(name).

    Args:
        path: Path to model file

    Returns:
        The main/default model

    Example:
        >>> import simlin
        >>> model = simlin.load("population.stmx")
        >>> print(f"Model has {len(model.stocks)} stocks")
        >>> model.base_case.results['population'].plot()
    """
    from pathlib import Path as PathlibPath
    from ._ffi import ffi, lib, check_out_error

    path = PathlibPath(path)

    if not path.exists():
        raise SimlinImportError(f"File not found: {path}")

    data = path.read_bytes()
    suffix = path.suffix.lower()

    # Determine the import function based on file extension
    c_data = ffi.new("uint8_t[]", data)
    err_ptr = ffi.new("SimlinError **")

    if suffix in (".xmile", ".stmx", ".xml"):
        project_ptr = lib.simlin_project_open_xmile(c_data, len(data), err_ptr)
    elif suffix in (".mdl", ".vpm"):
        project_ptr = lib.simlin_project_open_vensim(c_data, len(data), err_ptr)
    elif suffix in (".pb", ".bin", ".proto"):
        project_ptr = lib.simlin_project_open_protobuf(c_data, len(data), err_ptr)
    elif suffix == ".json":
        # Default to simlin JSON format
        c_format = lib.SIMLIN_JSON_FORMAT_NATIVE
        project_ptr = lib.simlin_project_open_json(c_data, len(data), c_format, err_ptr)
    else:
        # Try to auto-detect based on content
        if data.startswith(b"<?xml") or data.startswith(b"<xmile"):
            project_ptr = lib.simlin_project_open_xmile(c_data, len(data), err_ptr)
        elif data.startswith(b"{"):
            c_format = lib.SIMLIN_JSON_FORMAT_NATIVE
            project_ptr = lib.simlin_project_open_json(c_data, len(data), c_format, err_ptr)
        else:
            # Default to protobuf
            project_ptr = lib.simlin_project_open_protobuf(c_data, len(data), err_ptr)

    check_out_error(err_ptr, f"Load model from {path}")

    project = Project(project_ptr)
    return project.get_model()


__all__ = [
    # Top-level functions
    "load",
    # Main classes
    "Project",
    "Model",
    "Sim",
    "Run",
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
    "DominantPeriod",
    # Model structure types
    "TimeSpec",
    "GraphicalFunctionScale",
    "GraphicalFunction",
    "Stock",
    "Flow",
    "Aux",
    "ModelIssue",
    "UnitIssue",
    # JSON format constants
    "JSON_FORMAT_SIMLIN",
    "JSON_FORMAT_SDAI",
]
