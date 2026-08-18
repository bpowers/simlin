"""cattrs converter configuration for JSON serialization.

Maps the unified public variable types (``simlin.types``: Stock, Flow, Aux,
Module) to and from the engine's wire JSON (src/simlin-engine/src/json.rs),
and configures cattrs for the remaining wire dataclasses (views, sim specs,
patches).

The variable mapping is deliberately hand-written rather than generic
field-by-field serialization, because the public types diverge from the wire
on purpose:

- ``dimensions`` / ``element_equations`` / ``has_except_default`` fold into
  the wire's nested ``arrayedEquation`` object;
- ``units``/``documentation`` are ``None`` when unspecified publicly, empty
  strings on the wire;
- legacy top-level booleans (``nonNegative`` et al.) and legacy
  arrayed-level ``activeInitial`` merge into the one public ``Compat``;
- ``uid`` is never written: the engine preserves an existing variable's uid
  when an upsert payload has none and mints one for new variables
  (src/simlin-engine/src/patch.rs, upsert_variable).
"""

from __future__ import annotations

from dataclasses import MISSING, fields
from typing import TYPE_CHECKING, Any, Union

if TYPE_CHECKING:
    from collections.abc import Callable

    from .types import Variable

import cattrs

from .errors import SimlinRuntimeError
from .json_types import (
    AliasViewElement,
    AuxViewElement,
    CloudViewElement,
    DeleteVariable,
    DeleteView,
    Dimension,
    FlowPoint,
    FlowViewElement,
    JsonModelOperation,
    JsonModelPatch,
    JsonProjectOperation,
    JsonProjectPatch,
    LinkPoint,
    LinkViewElement,
    LoopMetadata,
    MacroSpec,
    Model,
    ModelGroup,
    ModuleViewElement,
    Project,
    Rect,
    RenameVariable,
    SetLoopName,
    SetSimSpecs,
    SimSpecs,
    StockViewElement,
    Unit,
    UpdateStockFlows,
    UpsertAux,
    UpsertFlow,
    UpsertModule,
    UpsertStock,
    UpsertView,
    View,
    ViewElement,
)
from .types import (
    SPREADFLOW_TYPES,
    Aux,
    Compat,
    Conveyor,
    DataSource,
    ElementEquation,
    Flow,
    GraphicalFunction,
    GraphicalFunctionScale,
    Leakage,
    Module,
    ModuleReference,
    Queue,
    SpreadFlow,
    Stock,
)


def _to_camel_case(snake_str: str) -> str:
    """Convert a snake_case string to camelCase."""
    components = snake_str.split("_")
    return components[0] + "".join(x.title() for x in components[1:])


# ---------------------------------------------------------------------------
# Graphical functions
# ---------------------------------------------------------------------------


def _structure_gf(d: dict[str, Any]) -> GraphicalFunction:
    points = d.get("points")
    if points:
        x_points: tuple[float, ...] | None = tuple(p[0] for p in points)
        y_points: tuple[float, ...] = tuple(p[1] for p in points)
    else:
        x_points = None
        y_points = tuple(d.get("yPoints") or ())
    x_scale_dict = d.get("xScale")
    y_scale_dict = d.get("yScale")
    return GraphicalFunction(
        y_points=y_points,
        x_points=x_points,
        kind=d.get("kind") or "continuous",
        x_scale=GraphicalFunctionScale(min=x_scale_dict["min"], max=x_scale_dict["max"])
        if x_scale_dict is not None
        else None,
        y_scale=GraphicalFunctionScale(min=y_scale_dict["min"], max=y_scale_dict["max"])
        if y_scale_dict is not None
        else None,
    )


def _unstructure_gf(gf: GraphicalFunction) -> dict[str, Any]:
    d: dict[str, Any] = {}
    if gf.x_points is not None:
        d["points"] = [[x, y] for x, y in zip(gf.x_points, gf.y_points, strict=True)]
    elif gf.y_points:
        d["yPoints"] = list(gf.y_points)
    if gf.kind:
        d["kind"] = gf.kind
    if gf.x_scale is not None:
        d["xScale"] = {"min": gf.x_scale.min, "max": gf.x_scale.max}
    if gf.y_scale is not None:
        d["yScale"] = {"min": gf.y_scale.min, "max": gf.y_scale.max}
    return d


# ---------------------------------------------------------------------------
# Compat (wire) <-> Compat (public)
# ---------------------------------------------------------------------------


def _check_spreadflow(sf_type: str, distribution: str | None) -> None:
    if sf_type not in SPREADFLOW_TYPES:
        valid = ", ".join(SPREADFLOW_TYPES)
        raise ValueError(f"Unknown spreadflow type: {sf_type!r}. Expected one of: {valid}")
    if sf_type == "dist" and distribution is None:
        raise ValueError("spreadflow type 'dist' requires a distribution")


def _merged_compat_dict(d: dict[str, Any], arrayed: dict[str, Any] | None = None) -> dict[str, Any]:
    """A variable's wire compat dict, merged with the legacy spellings.

    Old JSON carries nonNegative/canBeModuleInput/isPublic at the top level of
    the variable; new JSON carries them inside compat. Both are never
    meaningfully set at once, so OR is safe (mirrors the engine's own legacy
    handling in json.rs). Similarly, old JSON could carry activeInitial on the
    arrayed equation's compat; the engine only emits it at the variable level
    (compat_to_json) but ingests both, variable level winning -- mirrored here.
    """
    c = dict(d.get("compat") or {})
    for key in ("nonNegative", "canBeModuleInput", "isPublic"):
        if d.get(key):
            c[key] = True
    if not c.get("activeInitial"):
        arrayed_compat = (arrayed or {}).get("compat") or {}
        if arrayed_compat.get("activeInitial"):
            c["activeInitial"] = arrayed_compat["activeInitial"]
    return c


def _public_compat(c: dict[str, Any]) -> Compat | None:
    """The public Compat for a (merged) wire compat dict.

    The nested object checks are ``is not None`` rather than truthiness: a
    marker-only leakage or queue serializes as ``{}`` (falsy) and must not
    be dropped. An empty ``activeInitial`` normalizes to None, matching the
    engine's own ``.filter(|s| !s.is_empty())`` when it ingests the field.
    """
    ds = c.get("dataSource")
    conveyor = c.get("conveyor")
    leakage = c.get("leakage")
    spreadflow = c.get("spreadflow")
    compat = Compat(
        active_initial=c.get("activeInitial") or None,
        non_negative=bool(c.get("nonNegative")),
        can_be_module_input=bool(c.get("canBeModuleInput")),
        is_public=bool(c.get("isPublic")),
        data_source=DataSource(
            kind=ds["kind"],
            file=ds["file"],
            tab_or_delimiter=ds["tabOrDelimiter"],
            row_or_col=ds["rowOrCol"],
            cell=ds["cell"],
        )
        if ds is not None
        else None,
        conveyor=Conveyor(
            transit_time=conveyor["transitTime"],
            capacity=conveyor.get("capacity"),
            inflow_limit=conveyor.get("inflowLimit"),
            sample=conveyor.get("sample"),
            arrest=conveyor.get("arrest"),
            discrete=conveyor.get("discrete", False),
            batch_integrity=conveyor.get("batchIntegrity", False),
            one_at_a_time=conveyor.get("oneAtATime", False),
            exponential_leak=conveyor.get("exponentialLeak", False),
            ignore_earlier_zone_losses=conveyor.get("ignoreEarlierZoneLosses", False),
        )
        if conveyor is not None
        else None,
        leakage=Leakage(
            fraction=leakage.get("fraction"),
            integers=leakage.get("integers", False),
            zone_start=leakage.get("zoneStart"),
            zone_end=leakage.get("zoneEnd"),
        )
        if leakage is not None
        else None,
        spreadflow=_structure_spreadflow_dict(spreadflow) if spreadflow is not None else None,
        queue=Queue() if c.get("queue") is not None else None,
        overflow=bool(c.get("overflow")),
    )
    return None if compat == Compat() else compat


def _structure_spreadflow_dict(d: dict[str, Any]) -> SpreadFlow:
    sf_type = d["type"]
    distribution = d.get("distribution")
    _check_spreadflow(sf_type, distribution)
    # A stray distribution on a non-dist variant is not part of the wire
    # format; drop it so unstructure(structure(x)) is canonical.
    return SpreadFlow(type=sf_type, distribution=distribution if sf_type == "dist" else None)


def _unstructure_compat(compat: Compat | None) -> dict[str, Any]:
    """The wire compat dict for a variable's public Compat.

    Returns ``{}`` when nothing is set; callers omit the empty dict. The
    legacy top-level boolean spellings are never written -- everything goes
    inside the compat object, the engine's canonical location.
    """
    d: dict[str, Any] = {}
    if compat is None:
        return d
    if compat.active_initial:
        d["activeInitial"] = compat.active_initial
    if compat.non_negative:
        d["nonNegative"] = True
    if compat.can_be_module_input:
        d["canBeModuleInput"] = True
    if compat.is_public:
        d["isPublic"] = True
    if compat.data_source is not None:
        ds = compat.data_source
        d["dataSource"] = {
            "kind": ds.kind,
            "file": ds.file,
            "tabOrDelimiter": ds.tab_or_delimiter,
            "rowOrCol": ds.row_or_col,
            "cell": ds.cell,
        }
    if compat.conveyor is not None:
        conveyor = compat.conveyor
        cd: dict[str, Any] = {"transitTime": conveyor.transit_time}
        if conveyor.capacity is not None:
            cd["capacity"] = conveyor.capacity
        if conveyor.inflow_limit is not None:
            cd["inflowLimit"] = conveyor.inflow_limit
        if conveyor.sample is not None:
            cd["sample"] = conveyor.sample
        if conveyor.arrest is not None:
            cd["arrest"] = conveyor.arrest
        if conveyor.discrete:
            cd["discrete"] = True
        if conveyor.batch_integrity:
            cd["batchIntegrity"] = True
        if conveyor.one_at_a_time:
            cd["oneAtATime"] = True
        if conveyor.exponential_leak:
            cd["exponentialLeak"] = True
        if conveyor.ignore_earlier_zone_losses:
            cd["ignoreEarlierZoneLosses"] = True
        d["conveyor"] = cd
    if compat.leakage is not None:
        leakage = compat.leakage
        ld: dict[str, Any] = {}
        if leakage.fraction is not None:
            ld["fraction"] = leakage.fraction
        if leakage.integers:
            ld["integers"] = True
        if leakage.zone_start is not None:
            ld["zoneStart"] = leakage.zone_start
        if leakage.zone_end is not None:
            ld["zoneEnd"] = leakage.zone_end
        d["leakage"] = ld
    if compat.spreadflow is not None:
        sf = compat.spreadflow
        _check_spreadflow(sf.type, sf.distribution)
        if sf.type == "dist":
            d["spreadflow"] = {"type": "dist", "distribution": sf.distribution}
        else:
            d["spreadflow"] = {"type": sf.type}
    if compat.queue is not None:
        d["queue"] = {}
    if compat.overflow:
        d["overflow"] = True
    return d


# ---------------------------------------------------------------------------
# Arrayed equations
# ---------------------------------------------------------------------------


def _structure_element(d: dict[str, Any]) -> ElementEquation:
    compat = d.get("compat") or {}
    gf = d.get("graphicalFunction")
    return ElementEquation(
        subscript=d.get("subscript", ""),
        equation=d.get("equation", ""),
        active_initial=compat.get("activeInitial") or None,
        graphical_function=_structure_gf(gf) if gf is not None else None,
    )


def _unstructure_element(elem: ElementEquation) -> dict[str, Any]:
    d: dict[str, Any] = {"subscript": elem.subscript, "equation": elem.equation}
    if elem.active_initial:
        d["compat"] = {"activeInitial": elem.active_initial}
    if elem.graphical_function is not None:
        d["graphicalFunction"] = _unstructure_gf(elem.graphical_function)
    return d


def _resolve_arrayed_read(
    flat_equation: str,
    arrayed: dict[str, Any],
    *,
    legacy_initial: bool = False,
) -> tuple[str, tuple[str, ...], tuple[ElementEquation, ...], bool | None]:
    """Fold a wire ``arrayedEquation`` into the unified read shape.

    Returns ``(equation, dimensions, element_equations, has_except_default)``.

    ``has_except_default`` mirrors the engine (json.rs): the wire flag when
    present, else the legacy inference that a *stored* default equation --
    ``Option::is_some``, so an explicitly empty string counts -- is a live
    EXCEPT default; ``None`` only when no default is stored at all.

    The resolved equation: with a stored default (``has_except_default`` not
    ``None``), it is exactly that default, empty included -- substituting
    anything else would corrupt the stored state on write-back. Otherwise it
    is the flat wire equation, the legacy stock ``initialEquation`` old
    Go-produced JSON carried, or -- for element-by-element variables where
    every element shares the same text -- that common text as a display
    convenience (the shape the Vensim importer produces for apply-to-all
    equations).
    """
    dimensions = tuple(arrayed.get("dimensions") or ())
    elements = tuple(_structure_element(e) for e in arrayed.get("elements") or ())
    default_eq: str | None = arrayed.get("equation")
    if legacy_initial and arrayed.get("initialEquation"):
        default_eq = arrayed.get("initialEquation")

    has_except: bool | None = None
    if elements:
        hed_wire = arrayed.get("hasExceptDefault")
        if hed_wire is not None:
            has_except = bool(hed_wire)
        elif default_eq is not None:
            has_except = True

    if has_except is not None:
        equation = default_eq or ""
    else:
        common = ""
        if elements:
            first = elements[0].equation
            if all(e.equation == first for e in elements):
                common = first
        equation = flat_equation or default_eq or common
    return equation, dimensions, elements, has_except


def _arrayed_dict(
    dimensions: tuple[str, ...],
    equation: str,
    elements: tuple[ElementEquation, ...],
    has_except_default: bool | None,
) -> dict[str, Any] | None:
    """The wire ``arrayedEquation`` dict, or None for a scalar variable.

    With element equations present, the flat ``equation`` is written as the
    arrayed default exactly when ``has_except_default`` says one is stored
    (empty text included -- the wire distinguishes ``Some("")`` from absent).
    When it is ``None``, a non-empty ``equation`` is the hoisted common
    per-element text, which is display-only and must not become a stored
    default.
    """
    if not dimensions:
        if elements:
            raise ValueError("element_equations require dimensions to be set")
        return None
    d: dict[str, Any] = {"dimensions": list(dimensions)}
    if elements:
        if has_except_default is not None:
            d["equation"] = equation
            d["hasExceptDefault"] = has_except_default
        d["elements"] = [_unstructure_element(e) for e in elements]
    else:
        d["equation"] = equation
    return d


# ---------------------------------------------------------------------------
# Variables
# ---------------------------------------------------------------------------


def _structure_stock(d: dict[str, Any]) -> Stock:
    arrayed = d.get("arrayedEquation") or {}
    equation, dimensions, elements, has_except = _resolve_arrayed_read(
        d.get("initialEquation", ""), arrayed, legacy_initial=True
    )
    return Stock(
        name=d["name"],
        initial_equation=equation,
        inflows=tuple(d.get("inflows") or ()),
        outflows=tuple(d.get("outflows") or ()),
        units=d.get("units") or None,
        documentation=d.get("documentation") or None,
        dimensions=dimensions,
        element_equations=elements,
        has_except_default=has_except,
        compat=_public_compat(_merged_compat_dict(d, arrayed)),
    )


def _structure_flow(d: dict[str, Any]) -> Flow:
    arrayed = d.get("arrayedEquation") or {}
    equation, dimensions, elements, has_except = _resolve_arrayed_read(
        d.get("equation", ""), arrayed
    )
    gf = d.get("graphicalFunction")
    return Flow(
        name=d["name"],
        equation=equation,
        units=d.get("units") or None,
        documentation=d.get("documentation") or None,
        dimensions=dimensions,
        graphical_function=_structure_gf(gf) if gf is not None else None,
        element_equations=elements,
        has_except_default=has_except,
        compat=_public_compat(_merged_compat_dict(d, arrayed)),
    )


def _structure_aux(d: dict[str, Any]) -> Aux:
    arrayed = d.get("arrayedEquation") or {}
    equation, dimensions, elements, has_except = _resolve_arrayed_read(
        d.get("equation", ""), arrayed
    )
    gf = d.get("graphicalFunction")
    return Aux(
        name=d["name"],
        equation=equation,
        units=d.get("units") or None,
        documentation=d.get("documentation") or None,
        dimensions=dimensions,
        graphical_function=_structure_gf(gf) if gf is not None else None,
        element_equations=elements,
        has_except_default=has_except,
        compat=_public_compat(_merged_compat_dict(d, arrayed)),
    )


def _structure_module(d: dict[str, Any]) -> Module:
    return Module(
        name=d["name"],
        model_name=d["modelName"],
        units=d.get("units") or None,
        documentation=d.get("documentation") or None,
        references=tuple(
            ModuleReference(src=r["src"], dst=r["dst"]) for r in d.get("references") or ()
        ),
        compat=_public_compat(_merged_compat_dict(d)),
    )


def structure_variable(d: dict[str, Any]) -> Variable:
    """Parse a type-tagged wire variable dict into the unified type."""
    var_type = d.get("type")
    if var_type == "stock":
        return _structure_stock(d)
    elif var_type == "flow":
        return _structure_flow(d)
    elif var_type == "aux":
        return _structure_aux(d)
    elif var_type == "module":
        return _structure_module(d)
    else:
        raise SimlinRuntimeError(f"unknown variable type: {var_type!r}")


def _unstructure_stock(var: Stock) -> dict[str, Any]:
    d: dict[str, Any] = {"name": var.name}
    arrayed = _arrayed_dict(
        var.dimensions, var.initial_equation, var.element_equations, var.has_except_default
    )
    if arrayed is None and var.initial_equation:
        d["initialEquation"] = var.initial_equation
    if var.units:
        d["units"] = var.units
    d["inflows"] = list(var.inflows)
    d["outflows"] = list(var.outflows)
    if var.documentation:
        d["documentation"] = var.documentation
    if arrayed is not None:
        d["arrayedEquation"] = arrayed
    compat = _unstructure_compat(var.compat)
    if compat:
        d["compat"] = compat
    return d


def _unstructure_flow(var: Flow) -> dict[str, Any]:
    d: dict[str, Any] = {"name": var.name}
    arrayed = _arrayed_dict(
        var.dimensions, var.equation, var.element_equations, var.has_except_default
    )
    if arrayed is None and var.equation:
        d["equation"] = var.equation
    if var.units:
        d["units"] = var.units
    if var.graphical_function is not None:
        d["graphicalFunction"] = _unstructure_gf(var.graphical_function)
    if var.documentation:
        d["documentation"] = var.documentation
    if arrayed is not None:
        d["arrayedEquation"] = arrayed
    compat = _unstructure_compat(var.compat)
    if compat:
        d["compat"] = compat
    return d


def _unstructure_aux(var: Aux) -> dict[str, Any]:
    d: dict[str, Any] = {"name": var.name}
    arrayed = _arrayed_dict(
        var.dimensions, var.equation, var.element_equations, var.has_except_default
    )
    if arrayed is None and var.equation:
        d["equation"] = var.equation
    if var.units:
        d["units"] = var.units
    if var.graphical_function is not None:
        d["graphicalFunction"] = _unstructure_gf(var.graphical_function)
    if var.documentation:
        d["documentation"] = var.documentation
    if arrayed is not None:
        d["arrayedEquation"] = arrayed
    compat = _unstructure_compat(var.compat)
    if compat:
        d["compat"] = compat
    return d


def _unstructure_module(var: Module) -> dict[str, Any]:
    d: dict[str, Any] = {"name": var.name, "modelName": var.model_name}
    if var.units:
        d["units"] = var.units
    if var.documentation:
        d["documentation"] = var.documentation
    if var.references:
        d["references"] = [{"src": r.src, "dst": r.dst} for r in var.references]
    compat = _unstructure_compat(var.compat)
    if compat:
        d["compat"] = compat
    return d


def unstructure_variable(var: Variable) -> dict[str, Any]:
    """Serialize a unified variable to its wire dict (no ``type`` tag)."""
    if isinstance(var, Stock):
        return _unstructure_stock(var)
    elif isinstance(var, Flow):
        return _unstructure_flow(var)
    elif isinstance(var, Aux):
        return _unstructure_aux(var)
    elif isinstance(var, Module):
        return _unstructure_module(var)
    else:
        raise TypeError(f"expected Stock, Flow, Aux, or Module, got {type(var).__name__}")


# ---------------------------------------------------------------------------
# Generic omit-default machinery for the remaining wire dataclasses
# ---------------------------------------------------------------------------


def _make_omit_default_hook(
    cls: type,
    conv: cattrs.Converter,
    required_fields: set[str] | None = None,
) -> Callable[[Any], dict[str, Any]]:
    """Create an unstructure hook that omits fields equal to their defaults.

    Pre-computes field information at registration time for performance.
    Only omits a value if it equals the field's declared default. This preserves
    meaningful values like 0.0 for optional numeric fields when the default is None.

    Output field names are converted from snake_case to camelCase for JSON.

    Args:
        cls: The dataclass type
        conv: The cattrs converter
        required_fields: Set of field names that must always be included (even if default)
    """
    if required_fields is None:
        required_fields = set()

    # Sentinel for fields with no default (must always be included)
    _NO_DEFAULT = object()

    # Pre-compute field metadata at registration time
    # Store (python_name, json_name, default, is_required)
    field_info: list[tuple[str, str, Any, bool]] = []
    for fld in fields(cls):
        # Compute default value
        if fld.default is not MISSING:
            default = fld.default
        elif fld.default_factory is not MISSING:
            default = fld.default_factory()
        else:
            default = _NO_DEFAULT

        is_required = fld.name in required_fields
        json_name = _to_camel_case(fld.name)
        field_info.append((fld.name, json_name, default, is_required))

    def unstructure(obj: Any) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for py_name, json_name, default, is_required in field_info:
            val = getattr(obj, py_name)

            # Always include required fields
            if is_required:
                result[json_name] = conv.unstructure(val)
                continue

            # Always include fields without defaults
            if default is _NO_DEFAULT:
                result[json_name] = conv.unstructure(val)
                continue

            # Skip if value equals the field's default (handles None, 0, "", [], False correctly)
            if val == default:
                continue

            result[json_name] = conv.unstructure(val)

        return result

    return unstructure


def _create_converter() -> cattrs.Converter:
    """Create and configure a cattrs converter for JSON serialization."""
    conv = cattrs.Converter()

    # Unified variable types: hand-written wire mapping
    conv.register_unstructure_hook(Stock, _unstructure_stock)
    conv.register_unstructure_hook(Flow, _unstructure_flow)
    conv.register_unstructure_hook(Aux, _unstructure_aux)
    conv.register_unstructure_hook(Module, _unstructure_module)
    conv.register_structure_hook(Stock, lambda d, _: _structure_stock(d))
    conv.register_structure_hook(Flow, lambda d, _: _structure_flow(d))
    conv.register_structure_hook(Aux, lambda d, _: _structure_aux(d))
    conv.register_structure_hook(Module, lambda d, _: _structure_module(d))

    conv.register_unstructure_hook(GraphicalFunction, _unstructure_gf)
    conv.register_structure_hook(GraphicalFunction, lambda d, _: _structure_gf(d))

    # Handle RenameVariable: from_ -> from
    def unstructure_rename(rv: RenameVariable) -> dict[str, Any]:
        return {"from": rv.from_, "to": rv.to}

    def structure_rename(d: dict[str, Any], _: type) -> RenameVariable:
        return RenameVariable(from_=d["from"], to=d["to"])

    conv.register_unstructure_hook(RenameVariable, unstructure_rename)
    conv.register_structure_hook(RenameVariable, structure_rename)

    # Handle JsonModelOperation tagged union
    # Rust expects: {"type": "upsertStock", "payload": {"stock": {...}}}
    # Register hooks on EACH CONCRETE TYPE to ensure correct serialization
    # regardless of how the op is accessed (directly or via Union type)

    def _make_upsert_unstructure_hook(
        type_name: str, field_name: str
    ) -> Callable[[Any], dict[str, Any]]:
        """Create an unstructure hook for upsert operations."""

        def hook(op: Any) -> dict[str, Any]:
            inner_val = getattr(op, field_name)
            payload = {field_name: conv.unstructure(inner_val)}
            return {"type": type_name, "payload": payload}

        return hook

    # Register hooks for each concrete operation type
    conv.register_unstructure_hook(
        UpsertStock, _make_upsert_unstructure_hook("upsertStock", "stock")
    )
    conv.register_unstructure_hook(UpsertFlow, _make_upsert_unstructure_hook("upsertFlow", "flow"))
    conv.register_unstructure_hook(UpsertAux, _make_upsert_unstructure_hook("upsertAux", "aux"))
    conv.register_unstructure_hook(
        UpsertModule, _make_upsert_unstructure_hook("upsertModule", "module")
    )

    def unstructure_delete_variable(op: DeleteVariable) -> dict[str, Any]:
        return {"type": "deleteVariable", "payload": {"ident": op.ident}}

    conv.register_unstructure_hook(DeleteVariable, unstructure_delete_variable)

    def unstructure_rename_variable(op: RenameVariable) -> dict[str, Any]:
        return {"type": "renameVariable", "payload": {"from": op.from_, "to": op.to}}

    conv.register_unstructure_hook(RenameVariable, unstructure_rename_variable)

    def unstructure_upsert_view(op: UpsertView) -> dict[str, Any]:
        return {
            "type": "upsertView",
            "payload": {"index": op.index, "view": conv.unstructure(op.view)},
        }

    conv.register_unstructure_hook(UpsertView, unstructure_upsert_view)

    def unstructure_delete_view(op: DeleteView) -> dict[str, Any]:
        return {"type": "deleteView", "payload": {"index": op.index}}

    conv.register_unstructure_hook(DeleteView, unstructure_delete_view)

    def unstructure_update_stock_flows(op: UpdateStockFlows) -> dict[str, Any]:
        return {
            "type": "updateStockFlows",
            "payload": {
                "ident": op.ident,
                "inflows": list(op.inflows),
                "outflows": list(op.outflows),
            },
        }

    conv.register_unstructure_hook(UpdateStockFlows, unstructure_update_stock_flows)

    def unstructure_set_loop_name(op: SetLoopName) -> dict[str, Any]:
        payload: dict[str, Any] = {
            "variables": list(op.variables),
            "name": op.name,
        }
        if op.description is not None:
            payload["description"] = op.description
        return {"type": "setLoopName", "payload": payload}

    conv.register_unstructure_hook(SetLoopName, unstructure_set_loop_name)

    # Valid model operation type names for error messages
    _valid_model_op_types = (
        "upsertStock",
        "upsertFlow",
        "upsertAux",
        "upsertModule",
        "deleteVariable",
        "renameVariable",
        "upsertView",
        "deleteView",
        "updateStockFlows",
        "setLoopName",
    )

    # Structure hook for parsing tagged JSON back to concrete types
    def structure_model_op(d: dict[str, Any], _: type) -> JsonModelOperation:
        type_name = d["type"]
        payload = d["payload"]

        if type_name == "upsertStock":
            return UpsertStock(stock=_structure_stock(payload["stock"]))
        elif type_name == "upsertFlow":
            return UpsertFlow(flow=_structure_flow(payload["flow"]))
        elif type_name == "upsertAux":
            return UpsertAux(aux=_structure_aux(payload["aux"]))
        elif type_name == "upsertModule":
            return UpsertModule(module=_structure_module(payload["module"]))
        elif type_name == "deleteVariable":
            return DeleteVariable(ident=payload["ident"])
        elif type_name == "renameVariable":
            return RenameVariable(from_=payload["from"], to=payload["to"])
        elif type_name == "upsertView":
            return UpsertView(index=payload["index"], view=conv.structure(payload["view"], View))
        elif type_name == "deleteView":
            return DeleteView(index=payload["index"])
        elif type_name == "updateStockFlows":
            return UpdateStockFlows(
                ident=payload["ident"],
                inflows=list(payload.get("inflows") or []),
                outflows=list(payload.get("outflows") or []),
            )
        elif type_name == "setLoopName":
            return SetLoopName(
                variables=list(payload["variables"]),
                name=payload["name"],
                description=payload.get("description"),
            )
        else:
            valid = ", ".join(_valid_model_op_types)
            raise ValueError(
                f"Unknown model operation type: {type_name!r}. Expected one of: {valid}"
            )

    # Register structure hook for Union type (used when parsing JSON)
    conv.register_structure_hook(
        Union[
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
        ],
        structure_model_op,
    )

    # Handle JsonProjectOperation tagged union
    # Rust expects: {"type": "setSimSpecs", "payload": {"simSpecs": {...}}}
    _valid_project_op_types = ("setSimSpecs",)

    def unstructure_project_op(op: JsonProjectOperation) -> dict[str, Any]:
        if isinstance(op, SetSimSpecs):
            return {
                "type": "setSimSpecs",
                "payload": {"simSpecs": conv.unstructure(op.sim_specs)},
            }
        valid = ", ".join(_valid_project_op_types)
        raise ValueError(
            f"Unknown project operation type: {type(op).__name__}. Expected one of: {valid}"
        )

    def structure_project_op(d: dict[str, Any], _: type) -> JsonProjectOperation:
        type_name = d["type"]
        payload = d["payload"]
        if type_name == "setSimSpecs":
            return SetSimSpecs(sim_specs=conv.structure(payload["simSpecs"], SimSpecs))
        valid = ", ".join(_valid_project_op_types)
        raise ValueError(f"Unknown project operation type: {type_name!r}. Expected one of: {valid}")

    conv.register_unstructure_hook(Union[SetSimSpecs], unstructure_project_op)
    conv.register_structure_hook(Union[SetSimSpecs], structure_project_op)

    # JsonProjectPatch: the wire key is camelCase ("projectOps"), which the
    # auto-generated hook would miss (it looks for "project_ops" and silently
    # structures an empty list).
    def structure_project_patch(d: dict[str, Any], _: type) -> JsonProjectPatch:
        return JsonProjectPatch(
            project_ops=[structure_project_op(o, SetSimSpecs) for o in d.get("projectOps", [])],
            models=[conv.structure(m, JsonModelPatch) for m in d.get("models", [])],
        )

    conv.register_structure_hook(JsonProjectPatch, structure_project_patch)

    # Handle ViewElement tagged union
    # Rust expects: {"type": "stock", "uid": 1, "name": "foo", ...} (internally tagged)
    _view_element_cls_to_name: dict[type, str] = {
        StockViewElement: "stock",
        FlowViewElement: "flow",
        AuxViewElement: "aux",
        CloudViewElement: "cloud",
        LinkViewElement: "link",
        ModuleViewElement: "module",
        AliasViewElement: "alias",
    }
    _view_element_name_to_cls: dict[str, type] = {
        v: k for k, v in _view_element_cls_to_name.items()
    }

    def unstructure_view_element(elem: ViewElement) -> dict[str, Any]:
        elem_type = type(elem)
        if elem_type in _view_element_cls_to_name:
            result: dict[str, Any] = conv.unstructure(elem)
            result["type"] = _view_element_cls_to_name[elem_type]
            return result
        valid_types = ", ".join(t.__name__ for t in _view_element_cls_to_name)
        raise ValueError(
            f"Unknown view element type: {elem_type.__name__}. Expected one of: {valid_types}"
        )

    def structure_view_element(d: dict[str, Any], _: Any) -> ViewElement:
        type_name = d.get("type")
        data = {k: v for k, v in d.items() if k != "type"}

        if type_name in _view_element_name_to_cls:
            return conv.structure(data, _view_element_name_to_cls[type_name])
        valid_names = ", ".join(_view_element_name_to_cls.keys())
        raise ValueError(
            f"Unknown view element type: {type_name!r}. Expected one of: {valid_names}"
        )

    conv.register_unstructure_hook(
        Union[
            StockViewElement,
            FlowViewElement,
            AuxViewElement,
            CloudViewElement,
            LinkViewElement,
            ModuleViewElement,
            AliasViewElement,
        ],
        unstructure_view_element,
    )
    conv.register_structure_hook(
        Union[
            StockViewElement,
            FlowViewElement,
            AuxViewElement,
            CloudViewElement,
            LinkViewElement,
            ModuleViewElement,
            AliasViewElement,
        ],
        structure_view_element,
    )

    # Structure hooks for individual view element types (reading camelCase JSON)
    def structure_stock_view_element(d: dict[str, Any], _: type) -> StockViewElement:
        return StockViewElement(
            uid=d["uid"],
            name=d["name"],
            x=d["x"],
            y=d["y"],
            label_side=d.get("labelSide", ""),
        )

    def structure_flow_view_element(d: dict[str, Any], _: type) -> FlowViewElement:
        points = [conv.structure(p, FlowPoint) for p in d.get("points", [])]
        return FlowViewElement(
            uid=d["uid"],
            name=d["name"],
            x=d["x"],
            y=d["y"],
            points=points,
            label_side=d.get("labelSide", ""),
        )

    def structure_aux_view_element(d: dict[str, Any], _: type) -> AuxViewElement:
        return AuxViewElement(
            uid=d["uid"],
            name=d["name"],
            x=d["x"],
            y=d["y"],
            label_side=d.get("labelSide", ""),
        )

    def structure_cloud_view_element(d: dict[str, Any], _: type) -> CloudViewElement:
        return CloudViewElement(
            uid=d["uid"],
            flow_uid=d["flowUid"],
            x=d["x"],
            y=d["y"],
        )

    def structure_link_view_element(d: dict[str, Any], _: type) -> LinkViewElement:
        multi_points: list[LinkPoint] = []
        if d.get("multiPoints"):
            multi_points = [conv.structure(p, LinkPoint) for p in d["multiPoints"]]
        return LinkViewElement(
            uid=d["uid"],
            from_uid=d["fromUid"],
            to_uid=d["toUid"],
            arc=d.get("arc"),
            multi_points=multi_points,
        )

    def structure_module_view_element(d: dict[str, Any], _: type) -> ModuleViewElement:
        return ModuleViewElement(
            uid=d["uid"],
            name=d["name"],
            x=d["x"],
            y=d["y"],
            label_side=d.get("labelSide", ""),
        )

    def structure_alias_view_element(d: dict[str, Any], _: type) -> AliasViewElement:
        return AliasViewElement(
            uid=d["uid"],
            alias_of_uid=d["aliasOfUid"],
            x=d["x"],
            y=d["y"],
            label_side=d.get("labelSide", ""),
        )

    def structure_flow_point(d: dict[str, Any], _: type) -> FlowPoint:
        return FlowPoint(
            x=d["x"],
            y=d["y"],
            attached_to_uid=d.get("attachedToUid", 0),
        )

    conv.register_structure_hook(StockViewElement, structure_stock_view_element)
    conv.register_structure_hook(FlowViewElement, structure_flow_view_element)
    conv.register_structure_hook(AuxViewElement, structure_aux_view_element)
    conv.register_structure_hook(CloudViewElement, structure_cloud_view_element)
    conv.register_structure_hook(LinkViewElement, structure_link_view_element)
    conv.register_structure_hook(ModuleViewElement, structure_module_view_element)
    conv.register_structure_hook(AliasViewElement, structure_alias_view_element)
    conv.register_structure_hook(FlowPoint, structure_flow_point)

    # Register omit-default hooks for wire types
    # These skip fields that match their defaults (matching Rust's skip_serializing_if)
    # Required fields are always included (based on the JSON schema)
    type_required_fields: dict[type, set[str]] = {
        SimSpecs: {"start_time", "end_time", "dt", "method"},
        FlowPoint: {"x", "y"},
        LinkPoint: {"x", "y"},
        Rect: {"x", "y", "width", "height"},
        StockViewElement: {"uid", "name", "x", "y"},
        FlowViewElement: {"uid", "name", "x", "y", "points"},
        AuxViewElement: {"uid", "name", "x", "y"},
        CloudViewElement: {"uid", "flow_uid", "x", "y"},
        LinkViewElement: {"uid", "from_uid", "to_uid"},
        ModuleViewElement: {"uid", "name", "x", "y"},
        AliasViewElement: {"uid", "alias_of_uid", "x", "y"},
        View: {"elements"},
    }

    for cls, required in type_required_fields.items():
        conv.register_unstructure_hook(cls, _make_omit_default_hook(cls, conv, required))

    # View elements are internally tagged on the wire ({"type": "stock", ...}).
    # The per-class unstructure hooks must add the tag themselves: cattrs
    # dispatches a list element by its concrete class, so the Union-level
    # tagging hook above is never consulted when unstructuring View.elements
    # (untagged elements are unparseable by the engine's serde).
    def _make_tagged_view_element_hook(
        base: Callable[[Any], dict[str, Any]], tag: str
    ) -> Callable[[Any], dict[str, Any]]:
        def hook(obj: Any) -> dict[str, Any]:
            d = base(obj)
            d["type"] = tag
            return d

        return hook

    for cls, tag in _view_element_cls_to_name.items():
        conv.register_unstructure_hook(
            cls,
            _make_tagged_view_element_hook(
                _make_omit_default_hook(cls, conv, type_required_fields[cls]), tag
            ),
        )

    # Dimension: simple structure
    def structure_dimension(d: dict[str, Any], _: type) -> Dimension:
        return Dimension(
            name=d["name"],
            elements=d.get("elements", []),
            size=d.get("size", 0),
            maps_to=d.get("mapsTo"),
        )

    conv.register_structure_hook(Dimension, structure_dimension)

    # Unit: simple structure
    def structure_unit(d: dict[str, Any], _: type) -> Unit:
        return Unit(
            name=d["name"],
            equation=d.get("equation", ""),
            disabled=d.get("disabled", False),
            aliases=d.get("aliases", []),
        )

    conv.register_structure_hook(Unit, structure_unit)

    # LoopMetadata: simple structure
    def structure_loop_metadata(d: dict[str, Any], _: type) -> LoopMetadata:
        return LoopMetadata(
            uids=d.get("uids", []),
            deleted=d.get("deleted", False),
            name=d.get("name", ""),
            description=d.get("description", ""),
        )

    conv.register_structure_hook(LoopMetadata, structure_loop_metadata)

    # MacroSpec: simple structure
    def structure_macro_spec(d: dict[str, Any], _: type) -> MacroSpec:
        return MacroSpec(
            parameters=d.get("parameters", []),
            primary_output=d.get("primaryOutput", ""),
            additional_outputs=d.get("additionalOutputs", []),
        )

    conv.register_structure_hook(MacroSpec, structure_macro_spec)

    # SimSpecs: handle all fields
    def structure_sim_specs(d: dict[str, Any], _: type) -> SimSpecs:
        return SimSpecs(
            start_time=d["startTime"],
            end_time=d["endTime"],
            dt=d.get("dt", ""),
            save_step=d.get("saveStep", 0.0),
            method=d.get("method", ""),
            time_units=d.get("timeUnits", ""),
        )

    conv.register_structure_hook(SimSpecs, structure_sim_specs)

    # View: handle elements list
    def structure_view(d: dict[str, Any], _: type) -> View:
        elements: list[ViewElement] = [
            structure_view_element(e, ViewElement) for e in d.get("elements", [])
        ]
        view_box = None
        if d.get("viewBox"):
            vb = d["viewBox"]
            view_box = Rect(x=vb["x"], y=vb["y"], width=vb["width"], height=vb["height"])
        return View(
            elements=elements,
            kind=d.get("kind", ""),
            view_box=view_box,
            zoom=d.get("zoom", 0.0),
        )

    conv.register_structure_hook(View, structure_view)

    # ModelGroup: simple structure (reads camelCase runEnabled)
    def structure_model_group(d: dict[str, Any], _: type) -> ModelGroup:
        return ModelGroup(
            name=d["name"],
            doc=d.get("doc"),
            parent=d.get("parent"),
            members=d.get("members", []),
            run_enabled=d.get("runEnabled", False),
        )

    conv.register_structure_hook(ModelGroup, structure_model_group)

    # Model: handle all nested types
    def structure_model(d: dict[str, Any], _: type) -> Model:
        stocks = [_structure_stock(s) for s in d.get("stocks", [])]
        flows = [_structure_flow(f) for f in d.get("flows", [])]
        auxiliaries = [_structure_aux(a) for a in d.get("auxiliaries", [])]
        modules = [_structure_module(m) for m in d.get("modules", [])]
        sim_specs = None
        if d.get("simSpecs"):
            sim_specs = conv.structure(d["simSpecs"], SimSpecs)
        views = [conv.structure(v, View) for v in d.get("views", [])]
        loop_metadata = [conv.structure(lm, LoopMetadata) for lm in d.get("loopMetadata", [])]
        groups = [conv.structure(g, ModelGroup) for g in d.get("groups", [])]
        macro_spec = conv.structure(d["macroSpec"], MacroSpec) if d.get("macroSpec") else None
        return Model(
            name=d["name"],
            stocks=stocks,
            flows=flows,
            auxiliaries=auxiliaries,
            modules=modules,
            sim_specs=sim_specs,
            views=views,
            loop_metadata=loop_metadata,
            groups=groups,
            macro_spec=macro_spec,
        )

    conv.register_structure_hook(Model, structure_model)

    # Project: handle all nested types
    def structure_project(d: dict[str, Any], _: type) -> Project:
        sim_specs = conv.structure(d["simSpecs"], SimSpecs)
        models = [conv.structure(m, Model) for m in d.get("models", [])]
        dimensions = [conv.structure(dim, Dimension) for dim in d.get("dimensions", [])]
        units = [conv.structure(u, Unit) for u in d.get("units", [])]
        return Project(
            name=d["name"],
            sim_specs=sim_specs,
            models=models,
            dimensions=dimensions,
            units=units,
        )

    conv.register_structure_hook(Project, structure_project)

    # Register omit-default hooks for project structure types
    additional_type_required_fields: dict[type, set[str]] = {
        Dimension: {"name"},
        Unit: {"name"},
        LoopMetadata: {"uids", "name"},
        MacroSpec: {"parameters", "primary_output"},
        ModelGroup: {"name"},
        Model: {"name", "stocks", "flows", "auxiliaries"},
        Project: {"name", "sim_specs"},
        JsonModelPatch: {"name"},
        JsonProjectPatch: set(),  # both fields have defaults
    }

    for cls, required in additional_type_required_fields.items():
        conv.register_unstructure_hook(cls, _make_omit_default_hook(cls, conv, required))

    return conv


# Global converter instance
converter: cattrs.Converter = _create_converter()
