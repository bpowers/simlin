"""Data structures for the simlin package."""

from dataclasses import dataclass
from typing import Optional


@dataclass(frozen=True)
class TimeSpec:
    """Time specification for simulation."""

    start: float
    """Simulation start time"""

    stop: float
    """Simulation stop time"""

    dt: float
    """Time step for simulation"""

    units: Optional[str] = None
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

    x_points: Optional[tuple[float, ...]]
    """X coordinates. If None, uses implicit x scale from 0 to len(y_points)-1"""

    y_points: tuple[float, ...]
    """Y coordinates (function values)"""

    x_scale: GraphicalFunctionScale
    """X axis scale"""

    y_scale: GraphicalFunctionScale
    """Y axis scale"""

    kind: str = "continuous"
    """Interpolation: 'continuous', 'discrete', or 'extrapolate'"""


@dataclass(frozen=True)
class Stock:
    """
    A stock (level, accumulation) variable.

    Stocks represent accumulations in a system dynamics model. They integrate
    their net flow (inflows minus outflows) over time. Stock values can only
    change through flows.

    Immutable - modifying attributes will not change the underlying model.
    """

    name: str
    """Variable name"""

    initial_equation: str
    """Initial value expression"""

    inflows: tuple[str, ...]
    """Names of flows that increase this stock"""

    outflows: tuple[str, ...]
    """Names of flows that decrease this stock"""

    units: Optional[str] = None
    """Units (if specified)"""

    documentation: Optional[str] = None
    """Documentation/comments"""

    dimensions: tuple[str, ...] = ()
    """Dimension names for arrayed variables (empty if scalar)"""

    non_negative: bool = False
    """Whether this stock is constrained to be non-negative"""


@dataclass(frozen=True)
class Flow:
    """
    A flow (rate) variable.

    Flows represent rates of change in a system dynamics model. They determine
    how stocks change over time. Flows are computed at each time step based on
    their equations.

    Immutable - modifying attributes will not change the underlying model.
    """

    name: str
    """Variable name"""

    equation: str
    """Flow rate expression"""

    units: Optional[str] = None
    """Units (if specified)"""

    documentation: Optional[str] = None
    """Documentation/comments"""

    dimensions: tuple[str, ...] = ()
    """Dimension names for arrayed variables (empty if scalar)"""

    non_negative: bool = False
    """Whether this flow is constrained to be non-negative"""

    graphical_function: Optional[GraphicalFunction] = None
    """Graphical/table function if this uses WITH LOOKUP"""


@dataclass(frozen=True)
class Aux:
    """
    An auxiliary (intermediate calculation) variable.

    Auxiliary variables are computed values that help structure models and
    make equations more readable. They don't accumulate over time like stocks,
    but are recalculated at each time step.

    Some auxiliaries have memory (like those using DELAY or SMOOTH), in which
    case they have an initial_equation that sets their initial state.

    Immutable - modifying attributes will not change the underlying model.
    """

    name: str
    """Variable name"""

    equation: str
    """Equation defining this variable"""

    initial_equation: Optional[str] = None
    """Initial value equation (for variables with memory like DELAY, SMOOTH)"""

    units: Optional[str] = None
    """Units (if specified)"""

    documentation: Optional[str] = None
    """Documentation/comments"""

    dimensions: tuple[str, ...] = ()
    """Dimension names for arrayed variables (empty if scalar)"""

    graphical_function: Optional[GraphicalFunction] = None
    """Graphical/table function if this uses WITH LOOKUP"""


@dataclass(frozen=True)
class ModelIssue:
    """An issue found during model checking."""

    severity: str
    """Issue severity: 'error', 'warning', or 'info'"""

    message: str
    """Human-readable description of the issue"""

    variable: Optional[str] = None
    """Name of the variable with the issue (if applicable)"""

    suggestion: Optional[str] = None
    """Suggested fix for the issue (if available)"""


@dataclass(frozen=True)
class UnitIssue:
    """A dimensional analysis issue."""

    variable: str
    """Variable name with the unit issue"""

    message: str
    """Description of the unit issue"""

    expected_units: Optional[str] = None
    """Expected units for this variable"""

    actual_units: Optional[str] = None
    """Actual units computed for this variable"""
