"""cattrs converter configuration for JSON serialization.

Configures cattrs to serialize Python dataclasses to JSON matching
the Rust serde expectations in libsimlin.
"""

from __future__ import annotations

from dataclasses import MISSING, fields
from typing import Any, Callable, Union

import cattrs

from .json_types import (
    AliasViewElement,
    ArrayedEquation,
    Auxiliary,
    AuxiliaryViewElement,
    CloudViewElement,
    DeleteVariable,
    DeleteView,
    ElementEquation,
    Flow,
    FlowPoint,
    FlowViewElement,
    GraphicalFunction,
    GraphicalFunctionScale,
    JsonModelOperation,
    JsonModelPatch,
    JsonProjectOperation,
    JsonProjectPatch,
    LinkPoint,
    LinkViewElement,
    Module,
    ModuleReference,
    ModuleViewElement,
    Rect,
    RenameVariable,
    SetSimSpecs,
    SimSpecs,
    Stock,
    StockViewElement,
    UpsertAux,
    UpsertFlow,
    UpsertModule,
    UpsertStock,
    UpsertView,
    View,
    ViewElement,
)


def _make_omit_default_hook(
    cls: type,
    conv: cattrs.Converter,
    required_fields: set[str] | None = None,
) -> Callable[[Any], dict[str, Any]]:
    """Create an unstructure hook that omits fields equal to their defaults.

    Pre-computes field information at registration time for performance.
    Only omits a value if it equals the field's declared default. This preserves
    meaningful values like 0.0 for optional numeric fields when the default is None.

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
    field_info: list[tuple[str, Any, bool]] = []
    for fld in fields(cls):
        # Compute default value
        if fld.default is not MISSING:
            default = fld.default
        elif fld.default_factory is not MISSING:
            default = fld.default_factory()
        else:
            default = _NO_DEFAULT

        is_required = fld.name in required_fields
        field_info.append((fld.name, default, is_required))

    def unstructure(obj: Any) -> dict[str, Any]:
        result: dict[str, Any] = {}
        for name, default, is_required in field_info:
            val = getattr(obj, name)

            # Always include required fields
            if is_required:
                result[name] = conv.unstructure(val)
                continue

            # Always include fields without defaults
            if default is _NO_DEFAULT:
                result[name] = conv.unstructure(val)
                continue

            # Skip if value equals the field's default (handles None, 0, "", [], False correctly)
            if val == default:
                continue

            # Include all other values
            result[name] = conv.unstructure(val)

        return result

    return unstructure


def _create_converter() -> cattrs.Converter:
    """Create and configure a cattrs converter for JSON serialization."""
    conv = cattrs.Converter()

    # Register handlers for types that need special handling

    # Handle GraphicalFunction.points as list of [x, y] arrays (matching Rust)
    def unstructure_gf(gf: GraphicalFunction) -> dict[str, Any]:
        result: dict[str, Any] = {}
        if gf.points:
            result["points"] = [[p[0], p[1]] for p in gf.points]
        if gf.y_points:
            result["y_points"] = gf.y_points
        if gf.kind:
            result["kind"] = gf.kind
        if gf.x_scale is not None:
            result["x_scale"] = conv.unstructure(gf.x_scale)
        if gf.y_scale is not None:
            result["y_scale"] = conv.unstructure(gf.y_scale)
        return result

    def structure_gf(d: dict[str, Any], _: type) -> GraphicalFunction:
        points = [(p[0], p[1]) for p in d.get("points", [])]
        return GraphicalFunction(
            points=points,
            y_points=d.get("y_points", []),
            kind=d.get("kind", ""),
            x_scale=conv.structure(d["x_scale"], GraphicalFunctionScale)
            if "x_scale" in d
            else None,
            y_scale=conv.structure(d["y_scale"], GraphicalFunctionScale)
            if "y_scale" in d
            else None,
        )

    conv.register_unstructure_hook(GraphicalFunction, unstructure_gf)
    conv.register_structure_hook(GraphicalFunction, structure_gf)

    # Handle RenameVariable: from_ -> from
    def unstructure_rename(rv: RenameVariable) -> dict[str, Any]:
        return {"from": rv.from_, "to": rv.to}

    def structure_rename(d: dict[str, Any], _: type) -> RenameVariable:
        return RenameVariable(from_=d["from"], to=d["to"])

    conv.register_unstructure_hook(RenameVariable, unstructure_rename)
    conv.register_structure_hook(RenameVariable, structure_rename)

    # Handle JsonModelOperation tagged union
    # Rust expects: {"type": "upsert_stock", "payload": {"stock": {...}}}
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
        UpsertStock, _make_upsert_unstructure_hook("upsert_stock", "stock")
    )
    conv.register_unstructure_hook(
        UpsertFlow, _make_upsert_unstructure_hook("upsert_flow", "flow")
    )
    conv.register_unstructure_hook(
        UpsertAux, _make_upsert_unstructure_hook("upsert_aux", "aux")
    )
    conv.register_unstructure_hook(
        UpsertModule, _make_upsert_unstructure_hook("upsert_module", "module")
    )

    def unstructure_delete_variable(op: DeleteVariable) -> dict[str, Any]:
        return {"type": "delete_variable", "payload": {"ident": op.ident}}

    conv.register_unstructure_hook(DeleteVariable, unstructure_delete_variable)

    def unstructure_rename_variable(op: RenameVariable) -> dict[str, Any]:
        return {"type": "rename_variable", "payload": {"from": op.from_, "to": op.to}}

    conv.register_unstructure_hook(RenameVariable, unstructure_rename_variable)

    def unstructure_upsert_view(op: UpsertView) -> dict[str, Any]:
        return {
            "type": "upsert_view",
            "payload": {"index": op.index, "view": conv.unstructure(op.view)},
        }

    conv.register_unstructure_hook(UpsertView, unstructure_upsert_view)

    def unstructure_delete_view(op: DeleteView) -> dict[str, Any]:
        return {"type": "delete_view", "payload": {"index": op.index}}

    conv.register_unstructure_hook(DeleteView, unstructure_delete_view)

    # Valid model operation type names for error messages
    _valid_model_op_types = (
        "upsert_stock",
        "upsert_flow",
        "upsert_aux",
        "upsert_module",
        "delete_variable",
        "rename_variable",
        "upsert_view",
        "delete_view",
    )

    # Structure hook for parsing tagged JSON back to concrete types
    def structure_model_op(d: dict[str, Any], _: type) -> JsonModelOperation:
        type_name = d["type"]
        payload = d["payload"]

        if type_name == "upsert_stock":
            return UpsertStock(stock=conv.structure(payload["stock"], Stock))
        elif type_name == "upsert_flow":
            return UpsertFlow(flow=conv.structure(payload["flow"], Flow))
        elif type_name == "upsert_aux":
            return UpsertAux(aux=conv.structure(payload["aux"], Auxiliary))
        elif type_name == "upsert_module":
            return UpsertModule(module=conv.structure(payload["module"], Module))
        elif type_name == "delete_variable":
            return DeleteVariable(ident=payload["ident"])
        elif type_name == "rename_variable":
            return RenameVariable(from_=payload["from"], to=payload["to"])
        elif type_name == "upsert_view":
            return UpsertView(
                index=payload["index"], view=conv.structure(payload["view"], View)
            )
        elif type_name == "delete_view":
            return DeleteView(index=payload["index"])
        else:
            valid = ", ".join(_valid_model_op_types)
            raise ValueError(
                f"Unknown model operation type: {type_name!r}. "
                f"Expected one of: {valid}"
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
        ],
        structure_model_op,
    )

    # Handle JsonProjectOperation tagged union
    # Rust expects: {"type": "set_sim_specs", "payload": {"sim_specs": {...}}}
    _valid_project_op_types = ("set_sim_specs",)

    def unstructure_project_op(op: JsonProjectOperation) -> dict[str, Any]:
        if isinstance(op, SetSimSpecs):
            return {
                "type": "set_sim_specs",
                "payload": {"sim_specs": conv.unstructure(op.sim_specs)},
            }
        valid = ", ".join(_valid_project_op_types)
        raise ValueError(
            f"Unknown project operation type: {type(op).__name__}. "
            f"Expected one of: {valid}"
        )

    def structure_project_op(d: dict[str, Any], _: type) -> JsonProjectOperation:
        type_name = d["type"]
        payload = d["payload"]
        if type_name == "set_sim_specs":
            return SetSimSpecs(sim_specs=conv.structure(payload["sim_specs"], SimSpecs))
        valid = ", ".join(_valid_project_op_types)
        raise ValueError(
            f"Unknown project operation type: {type_name!r}. "
            f"Expected one of: {valid}"
        )

    conv.register_unstructure_hook(Union[SetSimSpecs], unstructure_project_op)
    conv.register_structure_hook(Union[SetSimSpecs], structure_project_op)

    # Handle ViewElement tagged union
    # Rust expects: {"type": "stock", "uid": 1, "name": "foo", ...} (internally tagged)
    _view_element_cls_to_name: dict[type, str] = {
        StockViewElement: "stock",
        FlowViewElement: "flow",
        AuxiliaryViewElement: "aux",
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
            result = conv.unstructure(elem)
            result["type"] = _view_element_cls_to_name[elem_type]
            return result
        valid_types = ", ".join(t.__name__ for t in _view_element_cls_to_name)
        raise ValueError(
            f"Unknown view element type: {elem_type.__name__}. "
            f"Expected one of: {valid_types}"
        )

    def structure_view_element(d: dict[str, Any], _: type) -> ViewElement:
        type_name = d.get("type")
        data = {k: v for k, v in d.items() if k != "type"}

        if type_name in _view_element_name_to_cls:
            return conv.structure(data, _view_element_name_to_cls[type_name])
        valid_names = ", ".join(_view_element_name_to_cls.keys())
        raise ValueError(
            f"Unknown view element type: {type_name!r}. "
            f"Expected one of: {valid_names}"
        )

    conv.register_unstructure_hook(
        Union[
            StockViewElement,
            FlowViewElement,
            AuxiliaryViewElement,
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
            AuxiliaryViewElement,
            CloudViewElement,
            LinkViewElement,
            ModuleViewElement,
            AliasViewElement,
        ],
        structure_view_element,
    )

    # Register omit-default hooks for variable types
    # These skip fields that match their defaults (matching Rust's skip_serializing_if)
    # Required fields are always included (based on the JSON schema)
    type_required_fields: dict[type, set[str]] = {
        Stock: {"name", "inflows", "outflows"},
        Flow: {"name"},
        Auxiliary: {"name"},
        Module: {"name", "model_name"},
        SimSpecs: {"start_time", "end_time"},
        ArrayedEquation: {"dimensions"},
        ElementEquation: {"subscript", "equation"},
        ModuleReference: {"src", "dst"},
        FlowPoint: {"x", "y"},
        LinkPoint: {"x", "y"},
        Rect: {"x", "y", "width", "height"},
        StockViewElement: {"uid", "name", "x", "y"},
        FlowViewElement: {"uid", "name", "x", "y", "points"},
        AuxiliaryViewElement: {"uid", "name", "x", "y"},
        CloudViewElement: {"uid", "flow_uid", "x", "y"},
        LinkViewElement: {"uid", "from_uid", "to_uid"},
        ModuleViewElement: {"uid", "name", "x", "y"},
        AliasViewElement: {"uid", "alias_of_uid", "x", "y"},
        View: {"elements"},
    }

    for cls, required in type_required_fields.items():
        conv.register_unstructure_hook(cls, _make_omit_default_hook(cls, conv, required))

    # GraphicalFunctionScale: unstructure and structure
    conv.register_unstructure_hook(
        GraphicalFunctionScale, lambda x: {"min": x.min, "max": x.max}
    )
    conv.register_structure_hook(
        GraphicalFunctionScale,
        lambda d, _: GraphicalFunctionScale(min=d["min"], max=d["max"]),
    )

    # ElementEquation: handle optional graphical_function
    def structure_element_equation(d: dict[str, Any], _: type) -> ElementEquation:
        gf = None
        if "graphical_function" in d and d["graphical_function"]:
            gf = conv.structure(d["graphical_function"], GraphicalFunction)
        return ElementEquation(
            subscript=d["subscript"],
            equation=d.get("equation", ""),
            initial_equation=d.get("initial_equation", ""),
            graphical_function=gf,
        )

    conv.register_structure_hook(ElementEquation, structure_element_equation)

    # ArrayedEquation: handle elements list with nested types
    def structure_arrayed_equation(d: dict[str, Any], _: type) -> ArrayedEquation:
        elements = None
        if "elements" in d and d["elements"]:
            elements = [conv.structure(e, ElementEquation) for e in d["elements"]]
        return ArrayedEquation(
            dimensions=d.get("dimensions", []),
            equation=d.get("equation"),
            initial_equation=d.get("initial_equation"),
            elements=elements,
        )

    conv.register_structure_hook(ArrayedEquation, structure_arrayed_equation)

    # ModuleReference: simple structure
    conv.register_structure_hook(
        ModuleReference,
        lambda d, _: ModuleReference(src=d["src"], dst=d["dst"]),
    )

    # Stock: handle nested types
    def structure_stock(d: dict[str, Any], _: type) -> Stock:
        arrayed_equation = None
        if "arrayed_equation" in d and d["arrayed_equation"]:
            arrayed_equation = conv.structure(d["arrayed_equation"], ArrayedEquation)
        return Stock(
            name=d["name"],
            inflows=d.get("inflows", []),
            outflows=d.get("outflows", []),
            uid=d.get("uid", 0),
            initial_equation=d.get("initial_equation", ""),
            units=d.get("units", ""),
            non_negative=d.get("non_negative", False),
            documentation=d.get("documentation", ""),
            can_be_module_input=d.get("can_be_module_input", False),
            is_public=d.get("is_public", False),
            arrayed_equation=arrayed_equation,
        )

    conv.register_structure_hook(Stock, structure_stock)

    # Flow: handle nested types
    def structure_flow(d: dict[str, Any], _: type) -> Flow:
        gf = None
        if "graphical_function" in d and d["graphical_function"]:
            gf = conv.structure(d["graphical_function"], GraphicalFunction)
        arrayed_equation = None
        if "arrayed_equation" in d and d["arrayed_equation"]:
            arrayed_equation = conv.structure(d["arrayed_equation"], ArrayedEquation)
        return Flow(
            name=d["name"],
            uid=d.get("uid", 0),
            equation=d.get("equation", ""),
            units=d.get("units", ""),
            non_negative=d.get("non_negative", False),
            graphical_function=gf,
            documentation=d.get("documentation", ""),
            can_be_module_input=d.get("can_be_module_input", False),
            is_public=d.get("is_public", False),
            arrayed_equation=arrayed_equation,
        )

    conv.register_structure_hook(Flow, structure_flow)

    # Auxiliary: handle nested types
    def structure_auxiliary(d: dict[str, Any], _: type) -> Auxiliary:
        gf = None
        if "graphical_function" in d and d["graphical_function"]:
            gf = conv.structure(d["graphical_function"], GraphicalFunction)
        arrayed_equation = None
        if "arrayed_equation" in d and d["arrayed_equation"]:
            arrayed_equation = conv.structure(d["arrayed_equation"], ArrayedEquation)
        return Auxiliary(
            name=d["name"],
            uid=d.get("uid", 0),
            equation=d.get("equation", ""),
            initial_equation=d.get("initial_equation", ""),
            units=d.get("units", ""),
            graphical_function=gf,
            documentation=d.get("documentation", ""),
            can_be_module_input=d.get("can_be_module_input", False),
            is_public=d.get("is_public", False),
            arrayed_equation=arrayed_equation,
        )

    conv.register_structure_hook(Auxiliary, structure_auxiliary)

    # Module: handle references list
    def structure_module(d: dict[str, Any], _: type) -> Module:
        references = [
            conv.structure(ref, ModuleReference)
            for ref in d.get("references", [])
        ]
        return Module(
            name=d["name"],
            model_name=d["model_name"],
            uid=d.get("uid", 0),
            units=d.get("units", ""),
            documentation=d.get("documentation", ""),
            references=references,
            can_be_module_input=d.get("can_be_module_input", False),
            is_public=d.get("is_public", False),
        )

    conv.register_structure_hook(Module, structure_module)

    return conv


# Global converter instance
converter = _create_converter()
