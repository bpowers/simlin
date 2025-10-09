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
)
from .run import (
    Run,
    DominantPeriod,
)
from .project import Project, JSON_FORMAT_SIMLIN, JSON_FORMAT_SDAI
from .model import Model
from .sim import Sim
from . import pb


def load(path: Union[str, Path]) -> Model:
    """
    Load a system dynamics model from file.

    Supports XMILE (.stmx, .xmile), SDAI JSON, and native JSON formats.
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
    project = Project.from_file(path)
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
    # JSON format constants
    "JSON_FORMAT_SIMLIN",
    "JSON_FORMAT_SDAI",
    "pb",
]
