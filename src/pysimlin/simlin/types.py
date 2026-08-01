"""Public data structures for the simlin package.

The variable classes here (Stock, Flow, Aux, Module) are the single
representation used everywhere pysimlin surfaces model structure: reading
(``Model.get_variable``, ``Model.variables``, the ``current`` mapping inside
``Model.edit()``) and writing (``patch.upsert``). Read a variable, derive an
updated copy with ``dataclasses.replace``, and upsert it back.

All classes are frozen: mutating an attribute raises, which makes the
"modified a variable and expected the model to change" mistake impossible.
Sequence-valued fields accept any sequence (lists included) and are
normalized to tuples on construction.
"""

from __future__ import annotations

from dataclasses import dataclass, fields
from typing import Union


def _freeze_sequences(obj: object) -> None:
    """Coerce list values of tuple-typed dataclass fields to tuples.

    Lets callers write ``inflows=["births"]`` while the stored value is
    immutable. Only converts lists; tuples pass through untouched.
    """
    for f in fields(obj):  # type: ignore[arg-type]
        val = getattr(obj, f.name)
        if isinstance(val, list):
            object.__setattr__(obj, f.name, tuple(val))


@dataclass(frozen=True)
class TimeSpec:
    """Time specification for simulation."""

    start: float
    """Simulation start time"""

    stop: float
    """Simulation stop time"""

    dt: float
    """Time step for simulation"""

    units: str | None = None
    """Time units (if specified)"""


@dataclass(frozen=True)
class GraphicalFunctionScale:
    """Scale for graphical function axes."""

    min: float
    """Minimum value for axis"""

    max: float
    """Maximum value for axis"""


@dataclass(frozen=True)
class GraphicalFunction:
    """
    A graphical/table function (lookup table).

    Represents a piecewise function defined by data points.
    Used in table functions and WITH LOOKUP expressions.
    """

    y_points: tuple[float, ...]
    """Y coordinates (function values)"""

    x_points: tuple[float, ...] | None = None
    """X coordinates. If None, uses implicit x scale from 0 to len(y_points)-1"""

    kind: str = "continuous"
    """Interpolation: 'continuous', 'discrete', or 'extrapolate'"""

    x_scale: GraphicalFunctionScale | None = None
    """X axis scale. None means the engine derives it from the data."""

    y_scale: GraphicalFunctionScale | None = None
    """Y axis scale. None means the engine derives it from the data."""

    def __post_init__(self) -> None:
        _freeze_sequences(self)


@dataclass(frozen=True)
class DataSource:
    """External data reference (GET DIRECT DATA/CONSTANTS/LOOKUPS/SUBSCRIPT)."""

    kind: str
    file: str
    tab_or_delimiter: str
    row_or_col: str
    cell: str


@dataclass(frozen=True)
class Conveyor:
    """A conveyor stock's options.

    Fields hold XMILE expression strings; ``None``/``False`` means the option
    was absent (the documented default applies).
    """

    transit_time: str
    capacity: str | None = None
    inflow_limit: str | None = None
    sample: str | None = None
    arrest: str | None = None
    discrete: bool = False
    batch_integrity: bool = False
    one_at_a_time: bool = False
    exponential_leak: bool = False
    ignore_earlier_zone_losses: bool = False


@dataclass(frozen=True)
class Leakage:
    """Marks a flow as a conveyor leakage outflow.

    A marker-only leakage (all fields default) means the leak fraction comes
    from the flow's equation rather than an explicit ``fraction``.
    """

    fraction: str | None = None
    integers: bool = False
    zone_start: str | None = None
    zone_end: str | None = None


# The five valid SpreadFlow variants, matching the engine's serialization.
SPREADFLOW_TYPES = ("beginning", "even", "dest", "dist", "source")


@dataclass(frozen=True)
class SpreadFlow:
    """Conveyor inflow-placement method.

    ``distribution`` is required exactly when ``type`` is ``"dist"``.
    """

    type: str
    distribution: str | None = None


@dataclass(frozen=True)
class Queue:
    """A queue stock marker (no options)."""


@dataclass(frozen=True)
class Compat:
    """Advanced Vensim/XMILE compatibility options for a variable.

    This is the escape hatch for features without first-class fields on the
    variable classes: conveyors and queues (on stocks), leakage/spreadflow/
    overflow (on flows), external data sources, and module-interface flags.

    Note: a stock or flow's non-negativity and an aux or flow's ACTIVE
    INITIAL are NOT here -- they are first-class fields on the variable
    classes (``non_negative``, ``active_initial``).
    """

    can_be_module_input: bool = False
    is_public: bool = False
    data_source: DataSource | None = None
    conveyor: Conveyor | None = None
    leakage: Leakage | None = None
    spreadflow: SpreadFlow | None = None
    queue: Queue | None = None
    overflow: bool = False


@dataclass(frozen=True)
class ElementEquation:
    """One element's equation for an arrayed variable defined element-by-element."""

    subscript: str
    """The element name(s) this equation applies to (e.g. ``"boston"``)."""

    equation: str
    """The equation for this element."""

    active_initial: str | None = None
    """Element-level ACTIVE INITIAL expression, if any."""

    graphical_function: GraphicalFunction | None = None
    """Element-level graphical function, if any."""


@dataclass(frozen=True)
class ModuleReference:
    """A connection between a module's sub-model and the parent model.

    ``src`` and ``dst`` are variable identifiers; a dotted name
    (``"mymodule.input"``) addresses a variable inside the sub-model.
    """

    src: str
    dst: str


@dataclass(frozen=True)
class Stock:
    """
    A stock (level, accumulation) variable.

    Stocks represent accumulations in a system dynamics model. They integrate
    their net flow (inflows minus outflows) over time. Stock values can only
    change through flows.
    """

    name: str
    """Variable name"""

    initial_equation: str = ""
    """Initial value expression.

    For arrayed stocks defined element-by-element, this is the common initial
    expression when every element shares the same text (or the EXCEPT default
    when has_except_default is set), and empty otherwise (see
    element_equations)."""

    inflows: tuple[str, ...] = ()
    """Names of flows that increase this stock"""

    outflows: tuple[str, ...] = ()
    """Names of flows that decrease this stock"""

    units: str | None = None
    """Units (if specified)"""

    documentation: str | None = None
    """Documentation/comments"""

    dimensions: tuple[str, ...] = ()
    """Dimension names for arrayed variables (empty if scalar)"""

    non_negative: bool = False
    """Whether this stock is constrained to be non-negative"""

    element_equations: tuple[ElementEquation, ...] = ()
    """Per-element initial expressions for arrayed stocks defined
    element-by-element. Empty for scalar and apply-to-all stocks."""

    has_except_default: bool | None = None
    """EXCEPT-default status of ``initial_equation`` when ``element_equations``
    is set. ``None``: no stored default (a non-empty ``initial_equation`` is
    the common per-element text, for display only). ``True``: it is a live
    EXCEPT default the element equations override. ``False``: it is a dead
    default kept for Vensim round-trip fidelity."""

    compat: Compat | None = None
    """Advanced options (conveyor/queue markers, external data, ...)."""

    def __post_init__(self) -> None:
        _freeze_sequences(self)


@dataclass(frozen=True)
class Flow:
    """
    A flow (rate) variable.

    Flows represent rates of change in a system dynamics model. They determine
    how stocks change over time. Flows are computed at each time step based on
    their equations.
    """

    name: str
    """Variable name"""

    equation: str = ""
    """Flow rate expression.

    For arrayed flows defined element-by-element, this is the common equation
    when every element shares the same text (or the EXCEPT default when
    has_except_default is set), and empty otherwise (see element_equations)."""

    units: str | None = None
    """Units (if specified)"""

    documentation: str | None = None
    """Documentation/comments"""

    dimensions: tuple[str, ...] = ()
    """Dimension names for arrayed variables (empty if scalar)"""

    non_negative: bool = False
    """Whether this flow is constrained to be non-negative"""

    active_initial: str | None = None
    """Active initial equation (Vensim ACTIVE INITIAL)"""

    graphical_function: GraphicalFunction | None = None
    """Graphical/table function if this uses WITH LOOKUP"""

    element_equations: tuple[ElementEquation, ...] = ()
    """Per-element equations for arrayed flows defined element-by-element.
    Empty for scalar and apply-to-all flows."""

    has_except_default: bool | None = None
    """EXCEPT-default status of ``equation`` when ``element_equations`` is
    set (see Stock.has_except_default)."""

    compat: Compat | None = None
    """Advanced options (leakage/spreadflow/overflow markers, ...)."""

    def __post_init__(self) -> None:
        _freeze_sequences(self)


@dataclass(frozen=True)
class Aux:
    """
    An auxiliary (intermediate calculation) variable.

    Auxiliary variables are computed values that help structure models and
    make equations more readable. They don't accumulate over time like stocks,
    but are recalculated at each time step.

    Some auxiliaries have memory (like those using DELAY or SMOOTH), in which
    case they have an active_initial that sets their initial state.
    """

    name: str
    """Variable name"""

    equation: str = ""
    """Equation defining this variable.

    For arrayed auxiliaries defined element-by-element, this is the common
    equation when every element shares the same text (or the EXCEPT default
    when has_except_default is set), and empty otherwise (see
    element_equations)."""

    active_initial: str | None = None
    """Active initial equation (Vensim ACTIVE INITIAL)"""

    units: str | None = None
    """Units (if specified)"""

    documentation: str | None = None
    """Documentation/comments"""

    dimensions: tuple[str, ...] = ()
    """Dimension names for arrayed variables (empty if scalar)"""

    graphical_function: GraphicalFunction | None = None
    """Graphical/table function if this uses WITH LOOKUP"""

    element_equations: tuple[ElementEquation, ...] = ()
    """Per-element equations for arrayed auxiliaries defined
    element-by-element. Empty for scalar and apply-to-all variables."""

    has_except_default: bool | None = None
    """EXCEPT-default status of ``equation`` when ``element_equations`` is
    set (see Stock.has_except_default)."""

    compat: Compat | None = None
    """Advanced options (external data sources, module-interface flags, ...)."""

    def __post_init__(self) -> None:
        _freeze_sequences(self)


@dataclass(frozen=True)
class Module:
    """
    A module: an instance of another model in the project, wired to the
    parent model through input/output references.
    """

    name: str
    """Variable name of this module instance"""

    model_name: str
    """Name of the model this module instantiates"""

    units: str | None = None
    """Units (if specified)"""

    documentation: str | None = None
    """Documentation/comments"""

    references: tuple[ModuleReference, ...] = ()
    """Connections between parent-model variables and sub-model variables"""

    compat: Compat | None = None
    """Advanced options (module-interface flags)."""

    def __post_init__(self) -> None:
        _freeze_sequences(self)


Variable = Union[Stock, Flow, Aux, Module]
"""Any model variable: what ``Model.get_variable`` returns and
``patch.upsert`` accepts."""


@dataclass(frozen=True)
class ModelIssue:
    """An issue found during model checking."""

    severity: str
    """Issue severity: 'error', 'warning', or 'info'"""

    message: str
    """Human-readable description of the issue"""

    variable: str | None = None
    """Name of the variable with the issue (if applicable)"""

    suggestion: str | None = None
    """Suggested fix for the issue (if available)"""


@dataclass(frozen=True)
class UnitIssue:
    """A dimensional analysis issue."""

    variable: str
    """Variable name with the unit issue"""

    message: str
    """Description of the unit issue"""

    expected_units: str | None = None
    """Expected units for this variable"""

    actual_units: str | None = None
    """Actual units computed for this variable"""
