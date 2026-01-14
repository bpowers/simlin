"""JSON-compatible dataclasses for the simlin patch API.

These types match the Rust JSON types in src/simlin-engine/src/json.rs
and are used for serializing patches to send to the simulation engine.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Optional, Union


# Supporting types


@dataclass
class GraphicalFunctionScale:
    """Scale for graphical function axes."""

    min: float
    max: float


@dataclass
class GraphicalFunction:
    """A graphical/table function (lookup table)."""

    points: list[tuple[float, float]] = field(default_factory=list)
    y_points: list[float] = field(default_factory=list)
    kind: str = ""
    x_scale: Optional[GraphicalFunctionScale] = None
    y_scale: Optional[GraphicalFunctionScale] = None


@dataclass
class ElementEquation:
    """An element-specific equation for arrayed variables."""

    subscript: str
    equation: str
    initial_equation: str = ""
    graphical_function: Optional[GraphicalFunction] = None


@dataclass
class ArrayedEquation:
    """Equation structure for arrayed/subscripted variables."""

    dimensions: list[str] = field(default_factory=list)
    equation: Optional[str] = None
    initial_equation: Optional[str] = None
    elements: Optional[list[ElementEquation]] = None


@dataclass
class ModuleReference:
    """A reference mapping between module input/output and parent model variable."""

    src: str
    dst: str


# Variable types


@dataclass
class Stock:
    """A stock (level, accumulation) variable for JSON serialization."""

    name: str
    inflows: list[str] = field(default_factory=list)
    outflows: list[str] = field(default_factory=list)
    uid: int = 0
    initial_equation: str = ""
    units: str = ""
    non_negative: bool = False
    documentation: str = ""
    can_be_module_input: bool = False
    is_public: bool = False
    arrayed_equation: Optional[ArrayedEquation] = None


@dataclass
class Flow:
    """A flow (rate) variable for JSON serialization."""

    name: str
    uid: int = 0
    equation: str = ""
    units: str = ""
    non_negative: bool = False
    graphical_function: Optional[GraphicalFunction] = None
    documentation: str = ""
    can_be_module_input: bool = False
    is_public: bool = False
    arrayed_equation: Optional[ArrayedEquation] = None


@dataclass
class Auxiliary:
    """An auxiliary (intermediate calculation) variable for JSON serialization."""

    name: str
    uid: int = 0
    equation: str = ""
    initial_equation: str = ""
    units: str = ""
    graphical_function: Optional[GraphicalFunction] = None
    documentation: str = ""
    can_be_module_input: bool = False
    is_public: bool = False
    arrayed_equation: Optional[ArrayedEquation] = None


@dataclass
class Module:
    """A module (submodel) variable for JSON serialization."""

    name: str
    model_name: str
    uid: int = 0
    units: str = ""
    documentation: str = ""
    references: list[ModuleReference] = field(default_factory=list)
    can_be_module_input: bool = False
    is_public: bool = False


# View types


@dataclass
class FlowPoint:
    """A point in a flow's visual representation."""

    x: float
    y: float
    attached_to_uid: int = 0


@dataclass
class LinkPoint:
    """A point in a link's visual representation."""

    x: float
    y: float


@dataclass
class Rect:
    """A rectangle for view bounds."""

    x: float
    y: float
    width: float
    height: float


@dataclass
class StockViewElement:
    """Visual element for a stock."""

    uid: int
    name: str
    x: float
    y: float
    label_side: str = ""


@dataclass
class FlowViewElement:
    """Visual element for a flow."""

    uid: int
    name: str
    x: float
    y: float
    points: list[FlowPoint] = field(default_factory=list)
    label_side: str = ""


@dataclass
class AuxiliaryViewElement:
    """Visual element for an auxiliary variable."""

    uid: int
    name: str
    x: float
    y: float
    label_side: str = ""


@dataclass
class CloudViewElement:
    """Visual element for a cloud (source/sink)."""

    uid: int
    flow_uid: int
    x: float
    y: float


@dataclass
class LinkViewElement:
    """Visual element for a causal link."""

    uid: int
    from_uid: int
    to_uid: int
    arc: Optional[float] = None
    multi_points: list[LinkPoint] = field(default_factory=list)


@dataclass
class ModuleViewElement:
    """Visual element for a module."""

    uid: int
    name: str
    x: float
    y: float
    label_side: str = ""


@dataclass
class AliasViewElement:
    """Visual element for an alias (ghost)."""

    uid: int
    alias_of_uid: int
    x: float
    y: float
    label_side: str = ""


# Union type for view elements
ViewElement = Union[
    StockViewElement,
    FlowViewElement,
    AuxiliaryViewElement,
    CloudViewElement,
    LinkViewElement,
    ModuleViewElement,
    AliasViewElement,
]


@dataclass
class View:
    """A view/diagram in the model."""

    elements: list[ViewElement] = field(default_factory=list)
    kind: str = ""
    view_box: Optional[Rect] = None
    zoom: float = 0.0


# Simulation specs


@dataclass
class SimSpecs:
    """Simulation specification."""

    start_time: float
    end_time: float
    dt: str = ""
    save_step: float = 0.0
    method: str = ""
    time_units: str = ""


# Patch operation types


@dataclass
class UpsertStock:
    """Operation to insert or update a stock variable."""

    stock: Stock


@dataclass
class UpsertFlow:
    """Operation to insert or update a flow variable."""

    flow: Flow


@dataclass
class UpsertAux:
    """Operation to insert or update an auxiliary variable."""

    aux: Auxiliary


@dataclass
class UpsertModule:
    """Operation to insert or update a module."""

    module: Module


@dataclass
class DeleteVariable:
    """Operation to delete a variable by identifier."""

    ident: str


@dataclass
class RenameVariable:
    """Operation to rename a variable."""

    from_: str  # "from" is a reserved word in Python
    to: str


@dataclass
class UpsertView:
    """Operation to insert or update a view at a specific index."""

    index: int
    view: View


@dataclass
class DeleteView:
    """Operation to delete a view at a specific index."""

    index: int


@dataclass
class SetSimSpecs:
    """Operation to set simulation specifications."""

    sim_specs: SimSpecs


# Union types for operations
JsonModelOperation = Union[
    UpsertStock,
    UpsertFlow,
    UpsertAux,
    UpsertModule,
    DeleteVariable,
    RenameVariable,
    UpsertView,
    DeleteView,
]

JsonProjectOperation = Union[SetSimSpecs]


# Patch structures


@dataclass
class JsonModelPatch:
    """A patch containing operations for a specific model."""

    name: str
    ops: list[JsonModelOperation] = field(default_factory=list)


@dataclass
class JsonProjectPatch:
    """A patch containing project-level and model-level operations."""

    project_ops: list[JsonProjectOperation] = field(default_factory=list)
    models: list[JsonModelPatch] = field(default_factory=list)
