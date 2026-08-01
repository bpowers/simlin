"""
Simlin - Python bindings for the Simlin system dynamics simulation engine.

This package provides a Pythonic interface to the Simlin simulation engine,
allowing you to load, run, and analyze system dynamics models.
"""

from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as _dist_version
from pathlib import Path
from typing import Union

try:
    __version__ = _dist_version("pysimlin")
except PackageNotFoundError:
    # Imported from the source tree with no installed distribution (the
    # in-place `setup.py build_ext` development path). There is no version to
    # report, and inventing one is how this drifted before.
    __version__ = "0.0.0+unknown"

from .analysis import (
    Analysis,
    Link,
    LinkPolarity,
    Loop,
    LoopPolarity,
    LtmMode,
    Partition,
    links_by_target,
)
from .errors import (
    ErrorCode,
    ErrorDetail,
    ErrorSeverity,
    SimlinCompilationError,
    SimlinError,
    SimlinImportError,
    SimlinRuntimeError,
    error_code_to_string,
)
from .model import VARTYPE_AUX, VARTYPE_FLOW, VARTYPE_MODULE, VARTYPE_STOCK, Model
from .project import JSON_FORMAT_SDAI, JSON_FORMAT_SIMLIN, Project
from .run import (
    DominantPeriod,
    Run,
)
from .sim import Sim
from .types import (
    Aux,
    Compat,
    Conveyor,
    DataSource,
    ElementEquation,
    Flow,
    GraphicalFunction,
    GraphicalFunctionScale,
    Leakage,
    ModelIssue,
    Module,
    ModuleReference,
    Queue,
    SpreadFlow,
    Stock,
    TimeSpec,
    UnitIssue,
    Variable,
)
from .vdf import load_vdf


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
        >>> print(f"Model has {len(model.get_var_names())} variables")
        >>> model.base_case.results["population"].plot()
    """
    from pathlib import Path as PathlibPath

    from ._ffi import check_out_error, ffi, lib

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
    "JSON_FORMAT_SDAI",
    "JSON_FORMAT_SIMLIN",
    "VARTYPE_AUX",
    "VARTYPE_FLOW",
    "VARTYPE_MODULE",
    "VARTYPE_STOCK",
    "Analysis",
    "Aux",
    "Compat",
    "Conveyor",
    "DataSource",
    "DominantPeriod",
    "ElementEquation",
    "ErrorCode",
    "ErrorDetail",
    "ErrorSeverity",
    "Flow",
    "GraphicalFunction",
    "GraphicalFunctionScale",
    "Leakage",
    "Link",
    "LinkPolarity",
    "Loop",
    "LoopPolarity",
    "LtmMode",
    "Model",
    "ModelIssue",
    "Module",
    "ModuleReference",
    "Partition",
    "Project",
    "Queue",
    "Run",
    "Sim",
    "SimlinCompilationError",
    "SimlinError",
    "SimlinImportError",
    "SimlinRuntimeError",
    "SpreadFlow",
    "Stock",
    "TimeSpec",
    "UnitIssue",
    "Variable",
    "error_code_to_string",
    "links_by_target",
    "load",
    "load_vdf",
]
