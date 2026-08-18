"""Wire-level dataclasses for the simlin patch API.

These types back the JSON patch format understood by the engine
(src/simlin-engine/src/json.rs): patch operations, views, sim specs, and
project structure. Variables themselves are represented by the unified
public types in ``simlin.types`` (Stock, Flow, Aux, Module); the converter
in ``json_converter`` maps those to and from the engine's wire shape.

``Model``/``Project`` here are construction envelopes (``Project.new``
builds its initial empty-project JSON from them) and patch payload carriers
-- NOT a lossless mirror of ``Project.serialize_json()``. In particular the
unified variable types deliberately carry no ``uid`` (the engine preserves
or mints uids on upsert), so structuring a full serialized project through
these types and unstructuring it back would orphan view-element and
loop-metadata uid references. For lossless project persistence, keep the
``serialize_json()`` bytes.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Union

# Runtime import (not TYPE_CHECKING): cattrs resolves these dataclasses'
# string annotations against this module's namespace when structuring the
# patch envelope types.
from .types import Aux, Flow, Module, Stock  # noqa: TC001

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
class AuxViewElement:
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
    arc: float | None = None
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
    AuxViewElement,
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
    view_box: Rect | None = None
    # Zoom factor (1.0 = 100%, 2.0 = twice as big); 0.0 means the default of 1.0.
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


# Project structure types


@dataclass
class Dimension:
    """A dimension for subscripted variables."""

    name: str
    elements: list[str] = field(default_factory=list)
    size: int = 0
    maps_to: str | None = None


@dataclass
class Unit:
    """A unit definition."""

    name: str
    equation: str = ""
    disabled: bool = False
    aliases: list[str] = field(default_factory=list)


@dataclass
class LoopMetadata:
    """Metadata for a feedback loop."""

    uids: list[int] = field(default_factory=list)
    deleted: bool = False
    name: str = ""
    description: str = ""


@dataclass
class MacroSpec:
    """Marks a model as a callable macro template and records its calling convention."""

    parameters: list[str] = field(default_factory=list)
    primary_output: str = ""
    additional_outputs: list[str] = field(default_factory=list)


@dataclass
class ModelGroup:
    """Semantic/organizational group for categorizing model variables."""

    name: str
    doc: str | None = None
    parent: str | None = None
    members: list[str] = field(default_factory=list)
    run_enabled: bool = False


@dataclass
class Model:
    """A model in the project."""

    name: str
    stocks: list[Stock] = field(default_factory=list)
    flows: list[Flow] = field(default_factory=list)
    auxiliaries: list[Aux] = field(default_factory=list)
    modules: list[Module] = field(default_factory=list)
    sim_specs: SimSpecs | None = None
    views: list[View] = field(default_factory=list)
    loop_metadata: list[LoopMetadata] = field(default_factory=list)
    groups: list[ModelGroup] = field(default_factory=list)
    macro_spec: MacroSpec | None = None


@dataclass
class Project:
    """A complete system dynamics project."""

    name: str
    sim_specs: SimSpecs
    models: list[Model] = field(default_factory=list)
    dimensions: list[Dimension] = field(default_factory=list)
    units: list[Unit] = field(default_factory=list)


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

    aux: Aux


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
class UpdateStockFlows:
    """Operation to replace a stock's inflow and outflow lists.

    A targeted alternative to re-upserting the whole stock: only the flow
    wiring changes, every other field of the stock is preserved by the
    engine (``patch.rs::apply_update_stock_flows``).
    """

    ident: str
    inflows: list[str] = field(default_factory=list)
    outflows: list[str] = field(default_factory=list)


@dataclass
class SetLoopName:
    """Operation to pin (name) a feedback loop by its variable set.

    Naming a loop forces the LTM engine to ALWAYS score it -- even in
    discovery mode, whose reported list is capped and may not name it (the
    LOOPSCORE capability from the LTM literature). ``variables`` lists the
    variable names forming the loop's cycle (order does not matter; the cycle
    is recovered from the model's causal graph).
    """

    variables: list[str]
    name: str
    description: str | None = None


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
    UpdateStockFlows,
    SetLoopName,
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
