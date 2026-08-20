"""
Simlin - Python bindings for the Simlin system dynamics simulation engine.

This package provides a Pythonic interface to the Simlin simulation engine,
allowing you to load, run, and analyze system dynamics models.
"""

from importlib.metadata import PackageNotFoundError
from importlib.metadata import version as _dist_version
from pathlib import Path
from typing import TYPE_CHECKING, Any, Union

try:
    __version__ = _dist_version("pysimlin")
except PackageNotFoundError:
    # Imported from the source tree with no installed distribution (the
    # in-place `setup.py build_ext` development path). There is no version to
    # report, and inventing one is how this drifted before.
    __version__ = "0.0.0+unknown"

from ._formats import FileFormat
from ._sync import ChangeEvent
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
from .diagram import Diagram
from .errors import (
    ErrorCode,
    ErrorDetail,
    ErrorSeverity,
    SimlinAssetError,
    SimlinDependencyError,
    SimlinError,
    SimlinImportError,
    SimlinRuntimeError,
    SimlinWriteError,
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

if TYPE_CHECKING:
    # The redundant alias keeps ``simlin.ModelWidget`` visible to type
    # checkers as an explicit re-export while it stays out of ``__all__``.
    from .widget import ModelWidget as ModelWidget


def __getattr__(name: str) -> Any:
    # ``ModelWidget`` is exported lazily: anywidget/ipywidgets are the
    # OPTIONAL ``notebook`` extra (``pip install "pysimlin[notebook]"``), and
    # importing them costs a few hundred milliseconds that scripts, servers,
    # and tests which never display a widget should not pay.
    # ``Model.widget()`` and displaying a model import it on demand the same
    # way; without the extra the import raises ``SimlinDependencyError``.
    # It is deliberately NOT in ``__all__``: ``from simlin import *`` resolves
    # every listed name, and a base install would raise on this one.
    if name == "ModelWidget":
        from ._widget_core import import_widget_module

        return import_widget_module().ModelWidget
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")


def load(path: Union[str, Path]) -> Model:
    """
    Load a system dynamics model from file into memory.

    Supports XMILE (.stmx, .xmile, .xml), Vensim MDL (.mdl), Simlin JSON
    (.sd.json / .json), SD-AI JSON (.json, detected by content), and
    protobuf (.pb); an unknown suffix is resolved from the file contents.
    Always returns the default/main model. For multi-model projects,
    access other models via model.project.get_model(name).

    The result is in-memory: ``model.path`` is ``None`` and edits stay in
    memory until ``model.project.save_as(...)``. Use :func:`open` for a
    model that keeps writing itself back to its file.

    Args:
        path: Path to model file

    Returns:
        The main/default model

    Raises:
        SimlinImportError: if the file does not exist or its format cannot
            be determined
        SimlinRuntimeError: if the engine cannot parse the file

    Example:
        >>> import simlin
        >>> model = simlin.load("population.stmx")
        >>> print(f"Model has {len(model.get_var_names())} variables")
        >>> model.base_case.results["population"].plot()
    """
    from ._formats import resolve_read_format
    from .project import _read_model_file

    p = Path(path)
    data = _read_model_file(p)
    fmt = resolve_read_format(p, data)
    return Project._from_bytes(data, fmt).get_model()


# ``simlin.open`` deliberately mirrors the builtin's name for files, so it is
# left out of ``__all__``: ``from simlin import *`` must not shadow ``open``.
def open(
    path: Union[str, Path],
    *,
    autosave: bool = True,
    watch: bool = True,
) -> Model:
    """
    Open a model file as a file-backed model.

    Like :func:`load`, but the returned model's project remembers the
    file: ``model.path`` is set, every accepted edit bumps
    ``model.revision`` and (with ``autosave``) is written straight back to
    the file in its own format, and (with ``watch``) changes made to the
    file by other tools -- Claude Code, the ``simlin`` MCP server, ``git
    checkout`` -- are loaded in place so ``model.run()`` reflects them.

    Args:
        path: Path to the model file
        autosave: Write each accepted change to ``path`` immediately
            (otherwise ``model.dirty`` is set until ``model.save()``)
        watch: Poll ``path`` every 0.5 s for external changes

    Returns:
        The main/default model

    Example:
        >>> import simlin
        >>> model = simlin.open("population.stmx")
        >>> with model.edit() as (current, patch):
        ...     patch.upsert(replace(current["birth_rate"], equation="0.04"))
        >>> model.revision
        1
    """
    return Project.open(path, autosave=autosave, watch=watch).get_model()


__all__ = [
    "JSON_FORMAT_SDAI",
    "JSON_FORMAT_SIMLIN",
    "VARTYPE_AUX",
    "VARTYPE_FLOW",
    "VARTYPE_MODULE",
    "VARTYPE_STOCK",
    "Analysis",
    "Aux",
    "ChangeEvent",
    "Compat",
    "Conveyor",
    "DataSource",
    "Diagram",
    "DominantPeriod",
    "ElementEquation",
    "ErrorCode",
    "ErrorDetail",
    "ErrorSeverity",
    "FileFormat",
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
    "SimlinAssetError",
    "SimlinDependencyError",
    "SimlinError",
    "SimlinImportError",
    "SimlinRuntimeError",
    "SimlinWriteError",
    "SpreadFlow",
    "Stock",
    "TimeSpec",
    "UnitIssue",
    "Variable",
    "links_by_target",
    "load",
    "load_vdf",
]
