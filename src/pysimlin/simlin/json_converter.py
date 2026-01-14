"""cattrs converter configuration for JSON serialization.

Configures cattrs to serialize Python dataclasses to JSON matching
the Rust serde expectations in libsimlin.
"""

from __future__ import annotations

from typing import Any, Union, get_args, get_origin

import cattrs
from cattrs.gen import make_dict_unstructure_fn, override

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


def _is_default(val: Any, default: Any) -> bool:
    """Check if a value equals its default."""
    return val == default


def _make_omit_default_hook(
    cls: type,
    conv: cattrs.Converter,
    required_fields: set[str] | None = None,
) -> Any:
    """Create an unstructure hook that omits default values.

    Args:
        cls: The dataclass type
        conv: The cattrs converter
        required_fields: Set of field names that must always be included (even if empty)
    """
    if required_fields is None:
        required_fields = set()

    def unstructure(obj: Any) -> dict[str, Any]:
        result = {}
        for fld in cls.__dataclass_fields__.values():
            val = getattr(obj, fld.name)
            default = fld.default if fld.default is not fld.default_factory else None
            if fld.default_factory is not fld.default_factory:
                default = fld.default_factory()

            # Always include required fields
            if fld.name in required_fields:
                result[fld.name] = conv.unstructure(val)
                continue

            # Skip if value equals default
            if default is not None and val == default:
                continue
            # Skip empty strings
            if val == "":
                continue
            # Skip zero numbers
            if val == 0 or val == 0.0:
                continue
            # Skip False booleans
            if val is False:
                continue
            # Skip empty lists
            if isinstance(val, list) and len(val) == 0:
                continue
            # Skip None
            if val is None:
                continue

            # Unstructure nested values
            result[fld.name] = conv.unstructure(val)

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
    _op_type_map: dict[type, tuple[str, str]] = {
        UpsertStock: ("upsert_stock", "stock"),
        UpsertFlow: ("upsert_flow", "flow"),
        UpsertAux: ("upsert_aux", "aux"),
        UpsertModule: ("upsert_module", "module"),
        DeleteVariable: ("delete_variable", "ident"),
        RenameVariable: ("rename_variable", None),  # type: ignore[dict-item]
        UpsertView: ("upsert_view", None),  # type: ignore[dict-item]
        DeleteView: ("delete_view", "index"),
    }

    def unstructure_model_op(op: JsonModelOperation) -> dict[str, Any]:
        for op_cls, (type_name, _) in _op_type_map.items():
            if isinstance(op, op_cls):
                if op_cls == RenameVariable:
                    payload = conv.unstructure(op)
                elif op_cls == UpsertView:
                    payload = {"index": op.index, "view": conv.unstructure(op.view)}
                elif op_cls == DeleteVariable:
                    payload = {"ident": op.ident}
                elif op_cls == DeleteView:
                    payload = {"index": op.index}
                else:
                    # UpsertStock, UpsertFlow, UpsertAux, UpsertModule
                    field_name = _op_type_map[op_cls][1]
                    inner_val = getattr(op, field_name)
                    payload = {field_name: conv.unstructure(inner_val)}
                return {"type": type_name, "payload": payload}
        raise ValueError(f"Unknown operation type: {type(op)}")

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
            return UpsertView(index=payload["index"], view=conv.structure(payload["view"], View))
        elif type_name == "delete_view":
            return DeleteView(index=payload["index"])
        else:
            raise ValueError(f"Unknown operation type: {type_name}")

    # Register for the Union type
    conv.register_unstructure_hook(
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
        unstructure_model_op,
    )
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
    def unstructure_project_op(op: JsonProjectOperation) -> dict[str, Any]:
        if isinstance(op, SetSimSpecs):
            return {
                "type": "set_sim_specs",
                "payload": {"sim_specs": conv.unstructure(op.sim_specs)},
            }
        raise ValueError(f"Unknown project operation type: {type(op)}")

    def structure_project_op(d: dict[str, Any], _: type) -> JsonProjectOperation:
        type_name = d["type"]
        payload = d["payload"]
        if type_name == "set_sim_specs":
            return SetSimSpecs(sim_specs=conv.structure(payload["sim_specs"], SimSpecs))
        raise ValueError(f"Unknown project operation type: {type_name}")

    conv.register_unstructure_hook(Union[SetSimSpecs], unstructure_project_op)
    conv.register_structure_hook(Union[SetSimSpecs], structure_project_op)

    # Handle ViewElement tagged union
    # Rust expects: {"type": "stock", "uid": 1, "name": "foo", ...} (internally tagged)
    _view_element_map: dict[type, str] = {
        StockViewElement: "stock",
        FlowViewElement: "flow",
        AuxiliaryViewElement: "aux",
        CloudViewElement: "cloud",
        LinkViewElement: "link",
        ModuleViewElement: "module",
        AliasViewElement: "alias",
    }

    def unstructure_view_element(elem: ViewElement) -> dict[str, Any]:
        for elem_cls, type_name in _view_element_map.items():
            if isinstance(elem, elem_cls):
                result = conv.unstructure(elem)
                result["type"] = type_name
                return result
        raise ValueError(f"Unknown view element type: {type(elem)}")

    def structure_view_element(d: dict[str, Any], _: type) -> ViewElement:
        type_name = d.get("type")
        data = {k: v for k, v in d.items() if k != "type"}

        type_to_cls: dict[str, type] = {
            "stock": StockViewElement,
            "flow": FlowViewElement,
            "aux": AuxiliaryViewElement,
            "cloud": CloudViewElement,
            "link": LinkViewElement,
            "module": ModuleViewElement,
            "alias": AliasViewElement,
        }

        if type_name in type_to_cls:
            return conv.structure(data, type_to_cls[type_name])
        raise ValueError(f"Unknown view element type: {type_name}")

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

    # Simple types that don't need special handling
    conv.register_unstructure_hook(
        GraphicalFunctionScale, lambda x: {"min": x.min, "max": x.max}
    )

    return conv


# Global converter instance
converter = _create_converter()
