"""Property-based tests for JSON type serialization.

These tests ensure:
1. JSON roundtrip fidelity (Python -> JSON -> Python) for the unified
   variable types (simlin.types) and the wire/patch types (simlin.json_types)
2. Schema compliance (generated JSON validates against docs/simlin-project.schema.json)
3. Exact wire-format fidelity against dicts matching what the Rust
   serializer in src/simlin-engine/src/json.rs emits
"""

import json
from pathlib import Path
from typing import Any

import pytest
from hypothesis import HealthCheck, given, settings
from hypothesis import strategies as st

try:
    import jsonschema

    HAS_JSONSCHEMA = True
except ImportError:
    HAS_JSONSCHEMA = False

from simlin.json_converter import converter, structure_variable, unstructure_variable
from simlin.json_types import (
    DeleteVariable,
    JsonModelPatch,
    JsonProjectPatch,
    MacroSpec,
    Model,
    ModelGroup,
    RenameVariable,
    UpsertFlow,
    UpsertStock,
)
from simlin.types import (
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

# Load the JSON schema
SCHEMA_PATH = Path(__file__).parents[3] / "docs" / "simlin-project.schema.json"
if SCHEMA_PATH.exists():
    with open(SCHEMA_PATH) as f:
        PROJECT_SCHEMA = json.load(f)
else:
    PROJECT_SCHEMA = None


# Hypothesis strategies for generating valid data


@st.composite
def ident_strategy(draw: Any) -> str:
    """Generate valid identifiers."""
    first_char = draw(st.sampled_from("abcdefghijklmnopqrstuvwxyz"))
    rest = draw(st.text(alphabet="abcdefghijklmnopqrstuvwxyz0123456789_", min_size=0, max_size=15))
    return first_char + rest


@st.composite
def equation_strategy(draw: Any) -> str:
    """Generate simple valid equations."""
    return draw(
        st.sampled_from(
            [
                "0",
                "1",
                "42",
                "3.14159",
                "x",
                "x + y",
                "a * b",
                "TIME",
                "INIT(x)",
                "IF x > 0 THEN y ELSE z",
            ]
        )
    )


@st.composite
def graphical_function_scale_strategy(draw: Any) -> GraphicalFunctionScale:
    """Generate a graphical function scale."""
    min_val = draw(
        st.floats(min_value=-1000, max_value=1000, allow_nan=False, allow_infinity=False)
    )
    max_val = draw(
        st.floats(min_value=min_val + 0.001, max_value=1001, allow_nan=False, allow_infinity=False)
    )
    return GraphicalFunctionScale(min=min_val, max=max_val)


@st.composite
def graphical_function_strategy(draw: Any) -> GraphicalFunction:
    """Generate a graphical function."""
    num_points = draw(st.integers(min_value=2, max_value=6))
    y_points = tuple(
        draw(st.floats(min_value=-100, max_value=100, allow_nan=False, allow_infinity=False))
        for _ in range(num_points)
    )
    use_x_points = draw(st.booleans())
    x_points = tuple(float(i) for i in range(num_points)) if use_x_points else None

    kind = draw(st.sampled_from(["continuous", "discrete", "extrapolate"]))
    x_scale = draw(st.one_of(st.none(), graphical_function_scale_strategy()))
    y_scale = draw(st.one_of(st.none(), graphical_function_scale_strategy()))

    return GraphicalFunction(
        y_points=y_points,
        x_points=x_points,
        kind=kind,
        x_scale=x_scale,
        y_scale=y_scale,
    )


@st.composite
def compat_strategy(draw: Any) -> Compat | None:
    """Generate compatibility options (or None)."""
    active_initial = draw(st.sampled_from([None, "42"]))
    nn = draw(st.booleans())
    cbmi = draw(st.booleans())
    is_pub = draw(st.booleans())
    if not (active_initial or nn or cbmi or is_pub):
        return None
    return Compat(
        active_initial=active_initial,
        non_negative=nn,
        can_be_module_input=cbmi,
        is_public=is_pub,
    )


@st.composite
def stock_strategy(draw: Any) -> Stock:
    """Generate a stock variable."""
    return Stock(
        name=draw(ident_strategy()),
        initial_equation=draw(equation_strategy()),
        inflows=tuple(draw(st.lists(ident_strategy(), min_size=0, max_size=3))),
        outflows=tuple(draw(st.lists(ident_strategy(), min_size=0, max_size=3))),
        units=draw(st.sampled_from([None, "widgets", "people", "dollars"])),
        documentation=draw(st.sampled_from([None, "A stock variable"])),
        compat=draw(compat_strategy()),
    )


@st.composite
def flow_strategy(draw: Any) -> Flow:
    """Generate a flow variable."""
    has_gf = draw(st.booleans())
    gf = draw(graphical_function_strategy()) if has_gf else None

    return Flow(
        name=draw(ident_strategy()),
        equation=draw(equation_strategy()),
        units=draw(st.sampled_from([None, "widgets/year", "people/month"])),
        documentation=draw(st.sampled_from([None, "A flow variable"])),
        graphical_function=gf,
        compat=draw(compat_strategy()),
    )


@st.composite
def aux_strategy(draw: Any) -> Aux:
    """Generate an auxiliary variable."""
    has_gf = draw(st.booleans())
    gf = draw(graphical_function_strategy()) if has_gf else None

    return Aux(
        name=draw(ident_strategy()),
        equation=draw(equation_strategy()),
        units=draw(st.sampled_from([None, "dimensionless", "ratio"])),
        documentation=draw(st.sampled_from([None, "An auxiliary variable"])),
        graphical_function=gf,
        compat=draw(compat_strategy()),
    )


@st.composite
def module_strategy(draw: Any) -> Module:
    """Generate a module."""
    num_refs = draw(st.integers(min_value=0, max_value=3))
    refs = tuple(
        ModuleReference(src=draw(ident_strategy()), dst=draw(ident_strategy()))
        for _ in range(num_refs)
    )

    return Module(
        name=draw(ident_strategy()),
        model_name=draw(ident_strategy()),
        units=draw(st.sampled_from([None, "widgets"])),
        documentation=draw(st.sampled_from([None, "A module"])),
        references=refs,
        compat=draw(compat_strategy()),
    )


@st.composite
def arrayed_aux_strategy(draw: Any) -> Aux:
    """Generate an arrayed auxiliary: apply-to-all, element-by-element, or
    EXCEPT-default shaped."""
    shape = draw(st.sampled_from(["a2a", "elements", "except"]))
    if shape == "a2a":
        return Aux(
            name=draw(ident_strategy()),
            equation=draw(equation_strategy()),
            dimensions=("region",),
        )
    elements = tuple(
        ElementEquation(
            subscript=sub,
            equation=draw(equation_strategy()),
            active_initial=draw(st.sampled_from([None, "1"])),
        )
        for sub in ("boston", "nyc")
    )
    if shape == "elements":
        # The unified read hoists common element text into `equation`; a
        # hand-built value only round-trips when it matches that shape.
        first = elements[0].equation
        common = first if all(e.equation == first for e in elements) else ""
        return Aux(
            name=draw(ident_strategy()),
            equation=common,
            dimensions=("region",),
            element_equations=elements,
        )
    return Aux(
        name=draw(ident_strategy()),
        equation=draw(equation_strategy()),
        dimensions=("region",),
        element_equations=elements,
        has_except_default=draw(st.booleans()),
    )


# Roundtrip tests


class TestJsonRoundtrip:
    """Tests for JSON serialization roundtrip of the unified types."""

    @given(stock=stock_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_stock_roundtrip(self, stock: Stock) -> None:
        """Stock dataclass roundtrips through JSON correctly."""
        parsed = json.loads(json.dumps(unstructure_variable(stock)))
        assert structure_variable({"type": "stock", **parsed}) == stock

    @given(flow=flow_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_flow_roundtrip(self, flow: Flow) -> None:
        """Flow dataclass roundtrips through JSON correctly."""
        parsed = json.loads(json.dumps(unstructure_variable(flow)))
        assert structure_variable({"type": "flow", **parsed}) == flow

    @given(aux=aux_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_aux_roundtrip(self, aux: Aux) -> None:
        """Aux dataclass roundtrips through JSON correctly."""
        parsed = json.loads(json.dumps(unstructure_variable(aux)))
        assert structure_variable({"type": "aux", **parsed}) == aux

    @given(aux=arrayed_aux_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_arrayed_aux_roundtrip(self, aux: Aux) -> None:
        """Arrayed auxiliaries (all three shapes) roundtrip correctly."""
        parsed = json.loads(json.dumps(unstructure_variable(aux)))
        assert structure_variable({"type": "aux", **parsed}) == aux

    @given(module=module_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_module_roundtrip(self, module: Module) -> None:
        """Module dataclass roundtrips through JSON correctly."""
        parsed = json.loads(json.dumps(unstructure_variable(module)))
        assert structure_variable({"type": "module", **parsed}) == module


class TestMacroSpecRoundtrip:
    """Verifies macros.AC1.4 (Python half): a macro-bearing Model with a
    populated MacroSpec round-trips losslessly through the json_converter,
    using camelCase keys."""

    def test_macro_spec_roundtrip(self) -> None:
        """A Model carrying a populated MacroSpec roundtrips, and the
        unstructured dict uses camelCase keys."""
        model = Model(
            name="smooth_macro",
            auxiliaries=[Aux(name="output", equation="input * gain")],
            macro_spec=MacroSpec(
                parameters=["input", "gain"],
                primary_output="output",
                additional_outputs=["debug_trace"],
            ),
        )

        json_dict = converter.unstructure(model)

        # The unstructured dict uses camelCase keys.
        assert "macroSpec" in json_dict
        macro_dict = json_dict["macroSpec"]
        assert macro_dict["parameters"] == ["input", "gain"]
        assert macro_dict["primaryOutput"] == "output"
        assert macro_dict["additionalOutputs"] == ["debug_trace"]
        assert "primary_output" not in macro_dict
        assert "additional_outputs" not in macro_dict

        json_str = json.dumps(json_dict)
        parsed = json.loads(json_str)
        reconstructed = converter.structure(parsed, Model)

        assert reconstructed.macro_spec == model.macro_spec
        assert reconstructed.macro_spec is not None
        assert reconstructed.macro_spec.parameters == ["input", "gain"]
        assert reconstructed.macro_spec.primary_output == "output"
        assert reconstructed.macro_spec.additional_outputs == ["debug_trace"]
        # The macro body (ordinary variables) round-trips alongside the spec.
        assert reconstructed.auxiliaries == model.auxiliaries

    def test_macro_spec_omits_empty_additional_outputs(self) -> None:
        """additionalOutputs is omitted when empty and restored as []."""
        spec = MacroSpec(parameters=["input"], primary_output="output")

        json_dict = converter.unstructure(spec)
        assert json_dict["parameters"] == ["input"]
        assert json_dict["primaryOutput"] == "output"
        assert "additionalOutputs" not in json_dict

        reconstructed = converter.structure(json_dict, MacroSpec)
        assert reconstructed.additional_outputs == []
        assert reconstructed == spec

    def test_model_without_macro_spec_omits_key(self) -> None:
        """A non-macro Model has no macroSpec key and restores macro_spec=None."""
        model = Model(name="ordinary", auxiliaries=[Aux(name="x", equation="1")])

        json_dict = converter.unstructure(model)
        assert "macroSpec" not in json_dict

        reconstructed = converter.structure(json_dict, Model)
        assert reconstructed.macro_spec is None


class TestModelGroupRoundtrip:
    """Verifies a Model carrying ModelGroups round-trips losslessly through the
    json_converter, using camelCase keys (runEnabled)."""

    def test_model_group_roundtrip(self) -> None:
        """A Model with populated ModelGroups roundtrips; the unstructured group
        dict uses camelCase keys."""
        model = Model(
            name="grouped",
            auxiliaries=[Aux(name="output", equation="1")],
            groups=[
                ModelGroup(
                    name="core",
                    doc="core dynamics",
                    parent="root",
                    members=["output"],
                    run_enabled=True,
                )
            ],
        )

        json_dict = converter.unstructure(model)

        assert "groups" in json_dict
        group_dict = json_dict["groups"][0]
        assert group_dict["name"] == "core"
        assert group_dict["doc"] == "core dynamics"
        assert group_dict["parent"] == "root"
        assert group_dict["members"] == ["output"]
        assert group_dict["runEnabled"] is True
        assert "run_enabled" not in group_dict

        json_str = json.dumps(json_dict)
        parsed = json.loads(json_str)
        reconstructed = converter.structure(parsed, Model)

        assert reconstructed.groups == model.groups
        assert reconstructed == model

    def test_model_group_omits_defaults(self) -> None:
        """Optional ModelGroup fields are omitted when default and restored."""
        model = Model(name="grouped", groups=[ModelGroup(name="minimal")])

        json_dict = converter.unstructure(model)
        group_dict = json_dict["groups"][0]
        assert group_dict == {"name": "minimal"}

        reconstructed = converter.structure(json_dict, Model)
        assert reconstructed.groups == [ModelGroup(name="minimal")]

    def test_model_without_groups_omits_key(self) -> None:
        """A Model with no groups omits the key and restores groups=[]."""
        model = Model(name="ungrouped", auxiliaries=[Aux(name="x", equation="1")])

        json_dict = converter.unstructure(model)
        assert "groups" not in json_dict

        reconstructed = converter.structure(json_dict, Model)
        assert reconstructed.groups == []


class TestPatchRoundtrip:
    """Tests for patch operation roundtrip."""

    @given(stock=stock_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_upsert_stock_roundtrip(self, stock: Stock) -> None:
        """UpsertStock operation roundtrips through JSON."""
        op = UpsertStock(stock=stock)
        patch = JsonModelPatch(name="test_model", ops=[op])
        project_patch = JsonProjectPatch(models=[patch])

        json_dict = converter.unstructure(project_patch)
        json_str = json.dumps(json_dict)
        parsed = json.loads(json_str)
        reconstructed = converter.structure(parsed, JsonProjectPatch)

        assert len(reconstructed.models) == 1
        assert len(reconstructed.models[0].ops) == 1
        reconstructed_op = reconstructed.models[0].ops[0]
        assert isinstance(reconstructed_op, UpsertStock)
        assert reconstructed_op.stock == stock

    @given(flow=flow_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_upsert_flow_roundtrip(self, flow: Flow) -> None:
        """UpsertFlow operation roundtrips through JSON."""
        op = UpsertFlow(flow=flow)
        patch = JsonModelPatch(name="test_model", ops=[op])
        project_patch = JsonProjectPatch(models=[patch])

        json_dict = converter.unstructure(project_patch)
        json_str = json.dumps(json_dict)
        parsed = json.loads(json_str)
        reconstructed = converter.structure(parsed, JsonProjectPatch)

        assert len(reconstructed.models) == 1
        assert len(reconstructed.models[0].ops) == 1
        reconstructed_op = reconstructed.models[0].ops[0]
        assert isinstance(reconstructed_op, UpsertFlow)
        assert reconstructed_op.flow == flow

    @given(from_ident=ident_strategy(), to_ident=ident_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_rename_variable_roundtrip(self, from_ident: str, to_ident: str) -> None:
        """RenameVariable roundtrips through JSON, including the from_/"from" rename.

        Kept over the sibling op roundtrips because it is the only coverage of
        the PARSE direction of the ``from_`` -> ``"from"`` field mapping;
        TestPatchJsonFormat::test_rename_variable_format pins only the
        serialize direction.
        """
        op = RenameVariable(from_=from_ident, to=to_ident)
        patch = JsonModelPatch(name="test_model", ops=[op])
        project_patch = JsonProjectPatch(models=[patch])

        json_dict = converter.unstructure(project_patch)
        json_str = json.dumps(json_dict)
        parsed = json.loads(json_str)
        reconstructed = converter.structure(parsed, JsonProjectPatch)

        # Verify the "from" field is correctly serialized
        assert json_dict["models"][0]["ops"][0]["payload"]["from"] == from_ident

        assert len(reconstructed.models) == 1
        assert len(reconstructed.models[0].ops) == 1
        reconstructed_op = reconstructed.models[0].ops[0]
        assert isinstance(reconstructed_op, RenameVariable)
        assert reconstructed_op.from_ == from_ident
        assert reconstructed_op.to == to_ident


@pytest.mark.skipif(not HAS_JSONSCHEMA, reason="jsonschema not installed")
@pytest.mark.skipif(PROJECT_SCHEMA is None, reason="Schema file not found")
class TestSchemaCompliance:
    """Tests that generated JSON validates against the schema."""

    @staticmethod
    def _validate_against_def(instance: dict[str, Any], def_name: str) -> None:
        """Validate an instance against a $def in the schema, resolving refs properly."""
        # Create a schema that references the definition within the full schema
        schema_with_ref = {
            "$ref": f"#/$defs/{def_name}",
            "$defs": PROJECT_SCHEMA.get("$defs", {}),
        }
        jsonschema.validate(instance, schema_with_ref)

    @given(stock=stock_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_stock_validates_against_schema(self, stock: Stock) -> None:
        """Generated Stock JSON validates against the schema."""
        self._validate_against_def(unstructure_variable(stock), "Stock")

    @given(flow=flow_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_flow_validates_against_schema(self, flow: Flow) -> None:
        """Generated Flow JSON validates against the schema."""
        self._validate_against_def(unstructure_variable(flow), "Flow")

    @given(aux=aux_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_aux_validates_against_schema(self, aux: Aux) -> None:
        """Generated Aux JSON validates against the schema."""
        self._validate_against_def(unstructure_variable(aux), "Auxiliary")

    @given(aux=arrayed_aux_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_arrayed_aux_validates_against_schema(self, aux: Aux) -> None:
        """Generated arrayed Aux JSON validates against the schema."""
        self._validate_against_def(unstructure_variable(aux), "Auxiliary")

    @given(module=module_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_module_validates_against_schema(self, module: Module) -> None:
        """Generated Module JSON validates against the schema."""
        self._validate_against_def(unstructure_variable(module), "Module")

    def test_model_group_validates_against_schema(self) -> None:
        """Generated ModelGroup JSON validates against the schema."""
        group = ModelGroup(
            name="core",
            doc="core dynamics",
            parent="root",
            members=["a", "b"],
            run_enabled=True,
        )
        json_dict = converter.unstructure(group)
        self._validate_against_def(json_dict, "ModelGroup")


class TestPatchJsonFormat:
    """Tests that patch JSON matches the expected Rust format."""

    def test_upsert_stock_format(self) -> None:
        """UpsertStock should produce correctly tagged JSON."""
        stock = Stock(
            name="population", inflows=["births"], outflows=["deaths"], initial_equation="100"
        )
        op = UpsertStock(stock=stock)
        patch = JsonModelPatch(name="main", ops=[op])
        project_patch = JsonProjectPatch(models=[patch])

        json_dict = converter.unstructure(project_patch)

        # Verify structure matches Rust's serde format
        assert "models" in json_dict
        assert len(json_dict["models"]) == 1
        model_patch = json_dict["models"][0]
        assert model_patch["name"] == "main"
        assert len(model_patch["ops"]) == 1

        op_dict = model_patch["ops"][0]
        assert op_dict["type"] == "upsertStock"
        assert "payload" in op_dict
        assert "stock" in op_dict["payload"]
        assert op_dict["payload"]["stock"]["name"] == "population"

    def test_rename_variable_format(self) -> None:
        """RenameVariable should use 'from' not 'from_' in JSON."""
        op = RenameVariable(from_="old_name", to="new_name")
        patch = JsonModelPatch(name="main", ops=[op])
        project_patch = JsonProjectPatch(models=[patch])

        json_dict = converter.unstructure(project_patch)
        op_dict = json_dict["models"][0]["ops"][0]

        assert op_dict["type"] == "renameVariable"
        assert op_dict["payload"]["from"] == "old_name"
        assert op_dict["payload"]["to"] == "new_name"
        assert "from_" not in op_dict["payload"]

    def test_delete_variable_format(self) -> None:
        """DeleteVariable should produce correctly formatted JSON."""
        op = DeleteVariable(ident="unused_var")
        patch = JsonModelPatch(name="main", ops=[op])
        project_patch = JsonProjectPatch(models=[patch])

        json_dict = converter.unstructure(project_patch)
        op_dict = json_dict["models"][0]["ops"][0]

        assert op_dict["type"] == "deleteVariable"
        assert op_dict["payload"]["ident"] == "unused_var"


class TestOptionalFieldSerialization:
    """Tests for correct handling of optional fields with falsy values."""

    def test_optional_numeric_zero_is_preserved(self) -> None:
        """arc=0.0 should be serialized, not omitted like arc=None."""
        from simlin.json_types import LinkViewElement

        # arc=None (default) should be omitted
        link_default = LinkViewElement(uid=1, from_uid=2, to_uid=3, arc=None)
        result_default = converter.unstructure(link_default)
        assert "arc" not in result_default, "arc=None should be omitted (equals default)"

        # arc=0.0 should be INCLUDED (different from default None)
        link_zero = LinkViewElement(uid=1, from_uid=2, to_uid=3, arc=0.0)
        result_zero = converter.unstructure(link_zero)
        assert "arc" in result_zero, "arc=0.0 must be included (different from default None)"
        assert result_zero["arc"] == 0.0

    def test_empty_equation_is_omitted(self) -> None:
        """An empty flow equation is omitted from the wire dict."""
        result_empty = unstructure_variable(Flow(name="test", equation=""))
        assert "equation" not in result_empty

        result_value = unstructure_variable(Flow(name="test", equation="x + 1"))
        assert result_value.get("equation") == "x + 1"

    def test_all_default_compat_is_omitted(self) -> None:
        """A Compat with every field default contributes nothing to the wire."""
        result_default = unstructure_variable(Flow(name="test", compat=Compat()))
        assert "compat" not in result_default

        result_nn = unstructure_variable(Flow(name="test", compat=Compat(non_negative=True)))
        assert result_nn.get("compat") == {"nonNegative": True}

    def test_empty_graphical_function_not_dropped(self) -> None:
        """An empty GraphicalFunction should not be elided like Compat."""
        flow = Flow(name="test", graphical_function=GraphicalFunction(y_points=()))
        result = unstructure_variable(flow)
        assert "graphicalFunction" in result, "empty GraphicalFunction should not be dropped"


class TestLegacyCompatMerge:
    """Legacy top-level booleans merge with compat and hoist correctly."""

    def test_legacy_booleans_preserved_when_compat_has_active_initial(self) -> None:
        """Legacy nonNegative/canBeModuleInput/isPublic must not be dropped
        when compat exists only for activeInitial."""
        stock_json: dict[str, Any] = {
            "type": "stock",
            "name": "pop",
            "initialEquation": "100",
            "inflows": [],
            "outflows": [],
            "compat": {"activeInitial": "50"},
            "nonNegative": True,
            "canBeModuleInput": True,
            "isPublic": True,
        }
        stock = structure_variable(stock_json)
        assert isinstance(stock, Stock)
        assert stock.compat is not None
        assert stock.compat.non_negative is True, "legacy nonNegative lost"
        assert stock.compat.can_be_module_input is True, "legacy canBeModuleInput lost"
        assert stock.compat.is_public is True, "legacy isPublic lost"
        assert stock.compat.active_initial == "50"

    def test_flow_legacy_merge(self) -> None:
        flow_json: dict[str, Any] = {
            "type": "flow",
            "name": "rate",
            "equation": "10",
            "compat": {"activeInitial": "5"},
            "nonNegative": True,
            "isPublic": True,
        }
        flow = structure_variable(flow_json)
        assert isinstance(flow, Flow)
        assert flow.compat is not None
        assert flow.compat.non_negative is True
        assert flow.compat.active_initial == "5"
        assert flow.compat.is_public is True

    def test_aux_legacy_merge(self) -> None:
        aux_json: dict[str, Any] = {
            "type": "aux",
            "name": "val",
            "equation": "1",
            "compat": {"activeInitial": "0"},
            "canBeModuleInput": True,
        }
        aux = structure_variable(aux_json)
        assert isinstance(aux, Aux)
        assert aux.compat is not None
        assert aux.compat.can_be_module_input is True


class TestCompatConveyorWireFormat:
    """Wire-format round-trip tests for the conveyor/queue Compat fields.

    The input dicts below match byte-for-byte what the Rust serializer in
    src/simlin-engine/src/json.rs emits (camelCase keys, skip-if-None /
    skip-if-false omission).  Each test structures the JSON into the unified
    dataclass, unstructures it back, and asserts the output equals the input
    exactly -- so absent fields stay absent (no spurious nulls) and no field
    is silently dropped.
    """

    def test_conveyor_stock_json_roundtrip(self) -> None:
        """A conveyor stock's compat survives a structure/unstructure cycle."""
        stock_json: dict[str, Any] = {
            "name": "students",
            "inflows": ["matriculating"],
            "outflows": ["graduating"],
            "initialEquation": "1000",
            "compat": {
                "conveyor": {
                    "transitTime": "4",
                    "capacity": "1200",
                    "inflowLimit": "500",
                    "discrete": True,
                    "oneAtATime": True,
                    "exponentialLeak": True,
                }
            },
        }
        stock = structure_variable({"type": "stock", **stock_json})
        assert isinstance(stock, Stock)
        assert stock.compat is not None
        assert stock.compat.conveyor == Conveyor(
            transit_time="4",
            capacity="1200",
            inflow_limit="500",
            discrete=True,
            one_at_a_time=True,
            exponential_leak=True,
        )
        assert unstructure_variable(stock) == stock_json

    def test_conveyor_all_fields_roundtrip(self) -> None:
        """Every Conveyor field survives, including sample/arrest and the
        batchIntegrity/ignoreEarlierZoneLosses booleans."""
        stock_json: dict[str, Any] = {
            "name": "belt",
            "inflows": [],
            "outflows": [],
            "compat": {
                "conveyor": {
                    "transitTime": "tt",
                    "capacity": "cap",
                    "inflowLimit": "lim",
                    "sample": "s",
                    "arrest": "a",
                    "discrete": True,
                    "batchIntegrity": True,
                    "oneAtATime": True,
                    "exponentialLeak": True,
                    "ignoreEarlierZoneLosses": True,
                }
            },
        }
        stock = structure_variable({"type": "stock", **stock_json})
        assert isinstance(stock, Stock)
        assert unstructure_variable(stock) == stock_json

    def test_conveyor_minimal_omits_defaults(self) -> None:
        """A transit-time-only conveyor emits only transitTime."""
        stock = Stock(name="belt", compat=Compat(conveyor=Conveyor(transit_time="4")))
        assert unstructure_variable(stock)["compat"] == {"conveyor": {"transitTime": "4"}}

    def test_queue_stock_json_roundtrip(self) -> None:
        """A queue stock's marker (compat.queue == {}) survives."""
        stock_json: dict[str, Any] = {
            "name": "backlog",
            "inflows": ["arriving"],
            "outflows": ["processing"],
            "initialEquation": "0",
            "compat": {"queue": {}},
        }
        stock = structure_variable({"type": "stock", **stock_json})
        assert isinstance(stock, Stock)
        assert stock.compat is not None
        assert stock.compat.queue == Queue()
        assert unstructure_variable(stock) == stock_json

    def test_leakage_flow_explicit_fraction_roundtrip(self) -> None:
        """A leak flow with an explicit fraction and zone bounds survives."""
        flow_json: dict[str, Any] = {
            "name": "dropping_out",
            "equation": "0.1",
            "compat": {
                "leakage": {
                    "fraction": "0.1",
                    "integers": True,
                    "zoneStart": "1",
                    "zoneEnd": "2",
                }
            },
        }
        flow = structure_variable({"type": "flow", **flow_json})
        assert isinstance(flow, Flow)
        assert flow.compat is not None
        assert flow.compat.leakage == Leakage(
            fraction="0.1", integers=True, zone_start="1", zone_end="2"
        )
        assert unstructure_variable(flow) == flow_json

    def test_leakage_flow_marker_only_roundtrip(self) -> None:
        """A marker-only leak flow (leakage == {}, the equation-carries-the-
        fraction encoding Stella uses) survives -- {} is falsy in Python, so
        a truthiness check would silently drop it."""
        flow_json: dict[str, Any] = {
            "name": "contagious_deaths",
            "equation": "0.01",
            "compat": {"nonNegative": True, "leakage": {}},
        }
        flow = structure_variable({"type": "flow", **flow_json})
        assert isinstance(flow, Flow)
        assert flow.compat is not None
        assert flow.compat.non_negative is True
        assert flow.compat.leakage == Leakage()
        assert unstructure_variable(flow) == flow_json

    @pytest.mark.parametrize("variant", ["beginning", "even", "dest", "source"])
    def test_spreadflow_unit_variants_roundtrip(self, variant: str) -> None:
        """The four payload-free spreadflow variants serialize as
        {"type": <variant>} with no distribution key."""
        flow_json: dict[str, Any] = {
            "name": "arriving",
            "equation": "250",
            "compat": {"spreadflow": {"type": variant}},
        }
        flow = structure_variable({"type": "flow", **flow_json})
        assert isinstance(flow, Flow)
        assert flow.compat is not None
        assert flow.compat.spreadflow == SpreadFlow(type=variant)
        assert unstructure_variable(flow) == flow_json

    def test_spreadflow_dist_roundtrip(self) -> None:
        """The dist variant is adjacently tagged: type + distribution."""
        flow_json: dict[str, Any] = {
            "name": "arriving",
            "equation": "250",
            "compat": {"spreadflow": {"type": "dist", "distribution": "1,2,1"}},
        }
        flow = structure_variable({"type": "flow", **flow_json})
        assert isinstance(flow, Flow)
        assert flow.compat is not None
        assert flow.compat.spreadflow == SpreadFlow(type="dist", distribution="1,2,1")
        assert unstructure_variable(flow) == flow_json

    def test_spreadflow_unknown_type_rejected(self) -> None:
        """An unknown spreadflow type raises instead of passing through."""
        flow_json = {
            "type": "flow",
            "name": "f",
            "compat": {"spreadflow": {"type": "sideways"}},
        }
        with pytest.raises(Exception, match="spreadflow"):
            structure_variable(flow_json)

    def test_spreadflow_dist_without_distribution_rejected(self) -> None:
        """A dist spreadflow without its distribution payload raises on both
        the structure and unstructure sides."""
        with pytest.raises(Exception, match="dist"):
            structure_variable(
                {"type": "flow", "name": "f", "compat": {"spreadflow": {"type": "dist"}}}
            )
        with pytest.raises(Exception, match="dist"):
            unstructure_variable(Flow(name="f", compat=Compat(spreadflow=SpreadFlow(type="dist"))))

    def test_overflow_flow_roundtrip(self) -> None:
        """A queue outflow's overflow marker survives."""
        flow_json: dict[str, Any] = {
            "name": "overflowing",
            "compat": {"overflow": True},
        }
        flow = structure_variable({"type": "flow", **flow_json})
        assert isinstance(flow, Flow)
        assert flow.compat is not None
        assert flow.compat.overflow is True
        assert unstructure_variable(flow) == flow_json

    def test_data_source_roundtrip(self) -> None:
        """A variable's external dataSource survives."""
        aux_json: dict[str, Any] = {
            "name": "historic_sales",
            "equation": "0",
            "compat": {
                "dataSource": {
                    "kind": "data",
                    "file": "sales.csv",
                    "tabOrDelimiter": ",",
                    "rowOrCol": "1",
                    "cell": "A2",
                }
            },
        }
        aux = structure_variable({"type": "aux", **aux_json})
        assert isinstance(aux, Aux)
        assert aux.compat is not None
        assert aux.compat.data_source == DataSource(
            kind="data", file="sales.csv", tab_or_delimiter=",", row_or_col="1", cell="A2"
        )
        assert unstructure_variable(aux) == aux_json

    def test_compat_full_roundtrip_through_stock(self) -> None:
        """A Compat carrying every advanced field round-trips through a
        variable (the shape production serializes)."""
        stock = Stock(
            name="s",
            initial_equation="50",
            compat=Compat(
                active_initial="7",
                non_negative=True,
                can_be_module_input=True,
                is_public=True,
                data_source=DataSource(
                    kind="constants",
                    file="c.csv",
                    tab_or_delimiter="\t",
                    row_or_col="A",
                    cell="B1",
                ),
                conveyor=Conveyor(transit_time="4", capacity="10"),
                leakage=Leakage(fraction="0.05"),
                spreadflow=SpreadFlow(type="dist", distribution="1,1"),
                queue=Queue(),
                overflow=True,
            ),
        )
        parsed = json.loads(json.dumps(unstructure_variable(stock)))
        assert structure_variable({"type": "stock", **parsed}) == stock

    def test_conveyor_stock_survives_patch_roundtrip(self) -> None:
        """The full patch envelope preserves a conveyor stock -- the shape
        model.edit() actually sends to the engine."""
        stock = Stock(
            name="students",
            inflows=["matriculating"],
            outflows=["graduating"],
            initial_equation="1000",
            compat=Compat(conveyor=Conveyor(transit_time="4", one_at_a_time=True)),
        )
        patch = JsonProjectPatch(models=[JsonModelPatch(name="main", ops=[UpsertStock(stock)])])
        parsed = json.loads(json.dumps(converter.unstructure(patch)))
        reconstructed = converter.structure(parsed, JsonProjectPatch)
        op = reconstructed.models[0].ops[0]
        assert isinstance(op, UpsertStock)
        assert op.stock == stock

    def test_legacy_boolean_merge_preserves_conveyor(self) -> None:
        """The legacy top-level boolean merge must not rebuild Compat with
        only the four legacy fields, dropping the conveyor marker."""
        stock_json: dict[str, Any] = {
            "type": "stock",
            "name": "students",
            "inflows": [],
            "outflows": [],
            "compat": {"conveyor": {"transitTime": "4"}},
            "nonNegative": True,
        }
        stock = structure_variable(stock_json)
        assert isinstance(stock, Stock)
        assert stock.compat is not None
        assert stock.compat.non_negative is True
        assert stock.compat.conveyor == Conveyor(transit_time="4")


class TestElementLevelCompat:
    """Element-level compat carries ACTIVE INITIAL only.

    The engine deliberately reads nothing else from an element's compat
    (src/simlin-engine/src/json.rs, ElementEquation: "Per-element compat
    carries active_initial ONLY ... other fields hand-authored here are
    tolerated by serde and dropped"), so the unified ElementEquation models
    exactly that field.
    """

    def test_element_active_initial_roundtrip(self) -> None:
        aux_json: dict[str, Any] = {
            "name": "rate",
            "arrayedEquation": {
                "dimensions": ["region"],
                "elements": [
                    {
                        "subscript": "east",
                        "equation": "supply_east",
                        "compat": {"activeInitial": "init_east"},
                    },
                    {"subscript": "west", "equation": "supply_west"},
                ],
            },
        }
        aux = structure_variable({"type": "aux", **aux_json})
        assert isinstance(aux, Aux)
        assert aux.element_equations == (
            ElementEquation(subscript="east", equation="supply_east", active_initial="init_east"),
            ElementEquation(subscript="west", equation="supply_west"),
        )
        assert unstructure_variable(aux) == aux_json


class TestNullValueHandling:
    """Tests for correct handling of explicit null values in JSON."""

    def _gf_from_wire(self, gf_dict: dict[str, Any]) -> GraphicalFunction:
        var = structure_variable(
            {"type": "aux", "name": "g", "equation": "x", "graphicalFunction": gf_dict}
        )
        assert isinstance(var, Aux)
        assert var.graphical_function is not None
        return var.graphical_function

    def test_graphical_function_with_explicit_null_scales(self) -> None:
        """GraphicalFunction should accept explicit null for xScale/yScale.

        The JSON schema allows null for optional fields. When incoming JSON
        explicitly sets xScale or yScale to null, we should treat it as None,
        not raise an error.
        """
        gf = self._gf_from_wire(
            {
                "points": [[0.0, 1.0], [1.0, 2.0]],
                "kind": "continuous",
                "xScale": None,
                "yScale": None,
            }
        )
        assert gf.x_scale is None
        assert gf.y_scale is None
        assert gf.x_points == (0.0, 1.0)
        assert gf.y_points == (1.0, 2.0)
        assert gf.kind == "continuous"

    def test_graphical_function_with_null_x_scale_only(self) -> None:
        """GraphicalFunction should handle null xScale with valid yScale."""
        gf = self._gf_from_wire(
            {
                "points": [[0.0, 1.0], [1.0, 2.0]],
                "xScale": None,
                "yScale": {"min": 0.0, "max": 10.0},
            }
        )
        assert gf.x_scale is None
        assert gf.y_scale == GraphicalFunctionScale(min=0.0, max=10.0)

    def test_graphical_function_with_null_y_scale_only(self) -> None:
        """GraphicalFunction should handle valid xScale with null yScale."""
        gf = self._gf_from_wire(
            {
                "points": [[0.0, 1.0], [1.0, 2.0]],
                "xScale": {"min": -5.0, "max": 5.0},
                "yScale": None,
            }
        )
        assert gf.x_scale == GraphicalFunctionScale(min=-5.0, max=5.0)
        assert gf.y_scale is None

    def test_graphical_function_without_scale_keys(self) -> None:
        """GraphicalFunction should handle missing x_scale/y_scale keys."""
        gf = self._gf_from_wire({"points": [[0.0, 1.0], [1.0, 2.0]], "kind": "discrete"})
        assert gf.x_scale is None
        assert gf.y_scale is None
        assert gf.kind == "discrete"


class TestViewAndLoopOps:
    """Round-trips and error paths for the view / loop patch operations."""

    def _roundtrip_op(self, op: Any) -> Any:
        patch = JsonProjectPatch(models=[JsonModelPatch(name="main", ops=[op])])
        parsed = json.loads(json.dumps(converter.unstructure(patch)))
        return converter.structure(parsed, JsonProjectPatch).models[0].ops[0]

    def test_upsert_view_roundtrip(self) -> None:
        from simlin.json_types import (
            AliasViewElement,
            AuxViewElement,
            CloudViewElement,
            FlowPoint,
            FlowViewElement,
            LinkPoint,
            LinkViewElement,
            ModuleViewElement,
            Rect,
            StockViewElement,
            UpsertView,
            View,
        )

        view = View(
            elements=[
                StockViewElement(uid=1, name="population", x=10.0, y=20.0),
                FlowViewElement(
                    uid=2,
                    name="births",
                    x=30.0,
                    y=20.0,
                    points=[
                        FlowPoint(x=25.0, y=20.0, attached_to_uid=1),
                        FlowPoint(x=40.0, y=20.0),
                    ],
                ),
                AuxViewElement(uid=3, name="rate", x=50.0, y=40.0, label_side="right"),
                CloudViewElement(uid=4, flow_uid=2, x=45.0, y=20.0),
                LinkViewElement(uid=5, from_uid=3, to_uid=2, arc=45.0),
                LinkViewElement(
                    uid=6, from_uid=1, to_uid=3, multi_points=[LinkPoint(x=1.0, y=2.0)]
                ),
                ModuleViewElement(uid=7, name="sub", x=60.0, y=60.0),
                AliasViewElement(uid=8, alias_of_uid=3, x=70.0, y=70.0),
            ],
            kind="stock_flow",
            view_box=Rect(x=0.0, y=0.0, width=100.0, height=100.0),
            zoom=1.5,
        )
        op = self._roundtrip_op(UpsertView(index=0, view=view))
        assert isinstance(op, UpsertView)
        assert op.index == 0
        assert op.view == view

    def test_delete_view_roundtrip(self) -> None:
        from simlin.json_types import DeleteView

        op = self._roundtrip_op(DeleteView(index=2))
        assert op == DeleteView(index=2)

    def test_set_loop_name_roundtrip(self) -> None:
        from simlin.json_types import SetLoopName

        op = self._roundtrip_op(
            SetLoopName(variables=["population", "births"], name="growth", description="R1")
        )
        assert op == SetLoopName(
            variables=["population", "births"], name="growth", description="R1"
        )

    def test_set_loop_name_without_description_omits_key(self) -> None:
        from simlin.json_types import SetLoopName

        op_dict = converter.unstructure(SetLoopName(variables=["a"], name="n"))
        assert "description" not in op_dict["payload"]
        assert self._roundtrip_op(SetLoopName(variables=["a"], name="n")) == SetLoopName(
            variables=["a"], name="n"
        )

    def test_unknown_model_op_type_rejected(self) -> None:
        from simlin.json_types import JsonModelOperation

        bad_op = {"type": "frobnicate", "payload": {}}
        with pytest.raises(Exception, match="Unknown model operation type"):
            converter.structure(bad_op, JsonModelOperation)

    def test_unknown_project_op_type_rejected(self) -> None:
        bad = {"projectOps": [{"type": "frobnicate", "payload": {}}]}
        with pytest.raises(Exception, match="Unknown project operation type"):
            converter.structure(bad, JsonProjectPatch)

    def test_unknown_view_element_type_rejected(self) -> None:
        from simlin.json_types import View

        with pytest.raises(Exception, match="Unknown view element type"):
            converter.structure({"elements": [{"type": "banner", "uid": 1}]}, View)

    def test_set_sim_specs_roundtrip(self) -> None:
        from simlin.json_types import SetSimSpecs, SimSpecs

        patch = JsonProjectPatch(
            project_ops=[
                SetSimSpecs(
                    sim_specs=SimSpecs(start_time=0.0, end_time=10.0, dt="0.25", method="euler")
                )
            ]
        )
        parsed = json.loads(json.dumps(converter.unstructure(patch)))
        reconstructed = converter.structure(parsed, JsonProjectPatch)
        assert reconstructed.project_ops == patch.project_ops
