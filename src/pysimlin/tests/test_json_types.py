"""Property-based tests for JSON type serialization.

These tests ensure:
1. JSON roundtrip fidelity (Python -> JSON -> Python)
2. Schema compliance (generated JSON validates against docs/simlin-project.schema.json)
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

from simlin.json_converter import converter
from simlin.json_types import (
    ArrayedEquation,
    Auxiliary,
    Compat,
    Conveyor,
    DataSource,
    DeleteVariable,
    ElementEquation,
    Flow,
    GraphicalFunction,
    GraphicalFunctionScale,
    JsonModelPatch,
    JsonProjectPatch,
    Leakage,
    MacroSpec,
    Model,
    ModelGroup,
    Module,
    ModuleReference,
    Queue,
    RenameVariable,
    SpreadFlow,
    Stock,
    UpsertFlow,
    UpsertStock,
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
    use_points = draw(st.booleans())
    if use_points:
        num_points = draw(st.integers(min_value=2, max_value=10))
        points = []
        for i in range(num_points):
            x = float(i)
            y = draw(
                st.floats(min_value=-100, max_value=100, allow_nan=False, allow_infinity=False)
            )
            points.append((x, y))
        y_points: list[float] = []
    else:
        points = []
        num_y = draw(st.integers(min_value=2, max_value=10))
        y_points = [
            draw(st.floats(min_value=-100, max_value=100, allow_nan=False, allow_infinity=False))
            for _ in range(num_y)
        ]

    kind = draw(st.sampled_from(["continuous", "discrete", "extrapolate", ""]))
    x_scale = draw(st.one_of(st.none(), graphical_function_scale_strategy()))
    y_scale = draw(st.one_of(st.none(), graphical_function_scale_strategy()))

    return GraphicalFunction(
        points=points,
        y_points=y_points,
        kind=kind,
        x_scale=x_scale,
        y_scale=y_scale,
    )


@st.composite
def stock_strategy(draw: Any) -> Stock:
    """Generate a stock variable."""
    nn = draw(st.booleans())
    cbmi = draw(st.booleans())
    is_pub = draw(st.booleans())
    compat = (
        Compat(non_negative=nn, can_be_module_input=cbmi, is_public=is_pub)
        if (nn or cbmi or is_pub)
        else None
    )
    return Stock(
        name=draw(ident_strategy()),
        inflows=draw(st.lists(ident_strategy(), min_size=0, max_size=3)),
        outflows=draw(st.lists(ident_strategy(), min_size=0, max_size=3)),
        uid=draw(st.integers(min_value=0, max_value=10000)),
        initial_equation=draw(equation_strategy()),
        units=draw(st.sampled_from(["", "widgets", "people", "dollars"])),
        documentation=draw(st.sampled_from(["", "A stock variable", "This accumulates over time"])),
        arrayed_equation=None,  # Keep simple for now
        compat=compat,
    )


@st.composite
def flow_strategy(draw: Any) -> Flow:
    """Generate a flow variable."""
    has_gf = draw(st.booleans())
    gf = draw(graphical_function_strategy()) if has_gf else None
    nn = draw(st.booleans())
    cbmi = draw(st.booleans())
    is_pub = draw(st.booleans())
    compat = (
        Compat(non_negative=nn, can_be_module_input=cbmi, is_public=is_pub)
        if (nn or cbmi or is_pub)
        else None
    )

    return Flow(
        name=draw(ident_strategy()),
        uid=draw(st.integers(min_value=0, max_value=10000)),
        equation=draw(equation_strategy()),
        units=draw(st.sampled_from(["", "widgets/year", "people/month"])),
        graphical_function=gf,
        documentation=draw(st.sampled_from(["", "A flow variable"])),
        arrayed_equation=None,
        compat=compat,
    )


@st.composite
def auxiliary_strategy(draw: Any) -> Auxiliary:
    """Generate an auxiliary variable."""
    has_gf = draw(st.booleans())
    gf = draw(graphical_function_strategy()) if has_gf else None
    cbmi = draw(st.booleans())
    is_pub = draw(st.booleans())
    compat = Compat(can_be_module_input=cbmi, is_public=is_pub) if (cbmi or is_pub) else None

    return Auxiliary(
        name=draw(ident_strategy()),
        uid=draw(st.integers(min_value=0, max_value=10000)),
        equation=draw(equation_strategy()),
        units=draw(st.sampled_from(["", "dimensionless", "ratio"])),
        graphical_function=gf,
        documentation=draw(st.sampled_from(["", "An auxiliary variable"])),
        arrayed_equation=None,
        compat=compat,
    )


@st.composite
def module_strategy(draw: Any) -> Module:
    """Generate a module."""
    num_refs = draw(st.integers(min_value=0, max_value=3))
    refs = [
        ModuleReference(src=draw(ident_strategy()), dst=draw(ident_strategy()))
        for _ in range(num_refs)
    ]
    cbmi = draw(st.booleans())
    is_pub = draw(st.booleans())
    compat = Compat(can_be_module_input=cbmi, is_public=is_pub) if (cbmi or is_pub) else None

    return Module(
        name=draw(ident_strategy()),
        model_name=draw(ident_strategy()),
        uid=draw(st.integers(min_value=0, max_value=10000)),
        units=draw(st.sampled_from(["", "widgets"])),
        documentation=draw(st.sampled_from(["", "A module"])),
        references=refs,
        compat=compat,
    )


# Roundtrip tests


class TestJsonRoundtrip:
    """Tests for JSON serialization roundtrip."""

    @given(stock=stock_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_stock_roundtrip(self, stock: Stock) -> None:
        """Stock dataclass roundtrips through JSON correctly."""
        json_dict = converter.unstructure(stock)
        json_str = json.dumps(json_dict)
        parsed = json.loads(json_str)
        reconstructed = converter.structure(parsed, Stock)
        assert stock == reconstructed

    @given(flow=flow_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_flow_roundtrip(self, flow: Flow) -> None:
        """Flow dataclass roundtrips through JSON correctly."""
        json_dict = converter.unstructure(flow)
        json_str = json.dumps(json_dict)
        parsed = json.loads(json_str)
        reconstructed = converter.structure(parsed, Flow)
        assert flow == reconstructed

    @given(aux=auxiliary_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_auxiliary_roundtrip(self, aux: Auxiliary) -> None:
        """Auxiliary dataclass roundtrips through JSON correctly."""
        json_dict = converter.unstructure(aux)
        json_str = json.dumps(json_dict)
        parsed = json.loads(json_str)
        reconstructed = converter.structure(parsed, Auxiliary)
        assert aux == reconstructed

    @given(module=module_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_module_roundtrip(self, module: Module) -> None:
        """Module dataclass roundtrips through JSON correctly."""
        json_dict = converter.unstructure(module)
        json_str = json.dumps(json_dict)
        parsed = json.loads(json_str)
        reconstructed = converter.structure(parsed, Module)
        assert module == reconstructed


class TestMacroSpecRoundtrip:
    """Verifies macros.AC1.4 (Python half): a macro-bearing Model with a
    populated MacroSpec round-trips losslessly through the json_converter,
    using camelCase keys."""

    def test_macro_spec_roundtrip(self) -> None:
        """A Model carrying a populated MacroSpec roundtrips, and the
        unstructured dict uses camelCase keys."""
        model = Model(
            name="smooth_macro",
            auxiliaries=[Auxiliary(name="output", equation="input * gain")],
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
        model = Model(name="ordinary", auxiliaries=[Auxiliary(name="x", equation="1")])

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
            auxiliaries=[Auxiliary(name="output", equation="1")],
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
        model = Model(name="ungrouped", auxiliaries=[Auxiliary(name="x", equation="1")])

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

    @given(ident=ident_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_delete_variable_roundtrip(self, ident: str) -> None:
        """DeleteVariable operation roundtrips through JSON."""
        op = DeleteVariable(ident=ident)
        patch = JsonModelPatch(name="test_model", ops=[op])
        project_patch = JsonProjectPatch(models=[patch])

        json_dict = converter.unstructure(project_patch)
        json_str = json.dumps(json_dict)
        parsed = json.loads(json_str)
        reconstructed = converter.structure(parsed, JsonProjectPatch)

        assert len(reconstructed.models) == 1
        assert len(reconstructed.models[0].ops) == 1
        reconstructed_op = reconstructed.models[0].ops[0]
        assert isinstance(reconstructed_op, DeleteVariable)
        assert reconstructed_op.ident == ident

    @given(from_ident=ident_strategy(), to_ident=ident_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_rename_variable_roundtrip(self, from_ident: str, to_ident: str) -> None:
        """RenameVariable operation roundtrips through JSON."""
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
        json_dict = converter.unstructure(stock)
        self._validate_against_def(json_dict, "Stock")

    @given(flow=flow_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_flow_validates_against_schema(self, flow: Flow) -> None:
        """Generated Flow JSON validates against the schema."""
        json_dict = converter.unstructure(flow)
        self._validate_against_def(json_dict, "Flow")

    @given(aux=auxiliary_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_auxiliary_validates_against_schema(self, aux: Auxiliary) -> None:
        """Generated Auxiliary JSON validates against the schema."""
        json_dict = converter.unstructure(aux)
        self._validate_against_def(json_dict, "Auxiliary")

    @given(module=module_strategy())
    @settings(max_examples=50, deadline=None, suppress_health_check=[HealthCheck.too_slow])
    def test_module_validates_against_schema(self, module: Module) -> None:
        """Generated Module JSON validates against the schema."""
        json_dict = converter.unstructure(module)
        self._validate_against_def(json_dict, "Module")

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

    def test_optional_string_empty_vs_none(self) -> None:
        """Empty string should only be omitted if it equals the default."""
        # For Flow, equation defaults to "" so empty string should be omitted
        flow_empty = Flow(name="test", equation="")
        result_empty = converter.unstructure(flow_empty)
        assert "equation" not in result_empty, "equation='' should be omitted (equals default)"

        # Non-empty equation should be included
        flow_value = Flow(name="test", equation="x + 1")
        result_value = converter.unstructure(flow_value)
        assert result_value.get("equation") == "x + 1"

    def test_optional_bool_false_vs_default(self) -> None:
        """False should only be omitted if it equals the default."""
        # Compat with all-default fields should be omitted
        flow_default = Flow(name="test", compat=Compat())
        result_default = converter.unstructure(flow_default)
        assert "compat" not in result_default, "compat with all defaults should be omitted"

        # Compat with non-default should be included
        flow_nn = Flow(name="test", compat=Compat(non_negative=True))
        result_nn = converter.unstructure(flow_nn)
        assert result_nn.get("compat", {}).get("nonNegative") is True

    def test_empty_graphical_function_not_dropped(self) -> None:
        """An empty GraphicalFunction should not be elided like Compat."""
        flow = Flow(name="test", graphical_function=GraphicalFunction())
        result = converter.unstructure(flow)
        assert "graphicalFunction" in result, "empty GraphicalFunction should not be dropped"


class TestLegacyCompatMerge:
    """Tests that legacy top-level booleans merge with compat."""

    def test_legacy_booleans_preserved_when_compat_has_active_initial(self) -> None:
        """Legacy nonNegative/canBeModuleInput/isPublic must not be dropped
        when compat exists only for activeInitial."""
        stock_json: dict[str, Any] = {
            "name": "pop",
            "initialEquation": "100",
            "inflows": [],
            "outflows": [],
            "compat": {"activeInitial": "50"},
            "nonNegative": True,
            "canBeModuleInput": True,
            "isPublic": True,
        }
        stock = converter.structure(stock_json, Stock)
        assert stock.compat is not None
        assert stock.compat.non_negative is True, "legacy nonNegative lost"
        assert stock.compat.can_be_module_input is True, "legacy canBeModuleInput lost"
        assert stock.compat.is_public is True, "legacy isPublic lost"
        assert stock.compat.active_initial == "50"

    def test_flow_legacy_merge(self) -> None:
        flow_json: dict[str, Any] = {
            "name": "rate",
            "equation": "10",
            "compat": {"activeInitial": "5"},
            "nonNegative": True,
            "isPublic": True,
        }
        flow = converter.structure(flow_json, Flow)
        assert flow.compat is not None
        assert flow.compat.non_negative is True
        assert flow.compat.is_public is True

    def test_aux_legacy_merge(self) -> None:
        aux_json: dict[str, Any] = {
            "name": "val",
            "equation": "1",
            "compat": {"activeInitial": "0"},
            "canBeModuleInput": True,
        }
        aux = converter.structure(aux_json, Auxiliary)
        assert aux.compat is not None
        assert aux.compat.can_be_module_input is True


class TestCompatConveyorWireFormat:
    """Wire-format round-trip tests for the conveyor/queue Compat fields.

    The input dicts below are byte-for-byte what the Rust serializer in
    src/simlin-engine/src/json.rs emits (camelCase keys, skip-if-None /
    skip-if-false omission).  Each test structures the JSON into the Python
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
        stock = converter.structure(stock_json, Stock)
        assert stock.compat is not None
        assert stock.compat.conveyor == Conveyor(
            transit_time="4",
            capacity="1200",
            inflow_limit="500",
            discrete=True,
            one_at_a_time=True,
            exponential_leak=True,
        )
        assert converter.unstructure(stock) == stock_json

    def test_conveyor_all_fields_roundtrip(self) -> None:
        """Every Conveyor field survives, including sample/arrest and the
        batchIntegrity/ignoreEarlierZoneLosses booleans."""
        conveyor_json: dict[str, Any] = {
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
        conveyor = converter.structure(conveyor_json, Conveyor)
        assert converter.unstructure(conveyor) == conveyor_json

    def test_conveyor_minimal_omits_defaults(self) -> None:
        """A transit-time-only conveyor emits only transitTime."""
        conveyor = Conveyor(transit_time="4")
        assert converter.unstructure(conveyor) == {"transitTime": "4"}
        assert converter.structure({"transitTime": "4"}, Conveyor) == conveyor

    def test_queue_stock_json_roundtrip(self) -> None:
        """A queue stock's marker (compat.queue == {}) survives."""
        stock_json: dict[str, Any] = {
            "name": "backlog",
            "inflows": ["arriving"],
            "outflows": ["processing"],
            "initialEquation": "0",
            "compat": {"queue": {}},
        }
        stock = converter.structure(stock_json, Stock)
        assert stock.compat is not None
        assert stock.compat.queue == Queue()
        assert converter.unstructure(stock) == stock_json

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
        flow = converter.structure(flow_json, Flow)
        assert flow.compat is not None
        assert flow.compat.leakage == Leakage(
            fraction="0.1", integers=True, zone_start="1", zone_end="2"
        )
        assert converter.unstructure(flow) == flow_json

    def test_leakage_flow_marker_only_roundtrip(self) -> None:
        """A marker-only leak flow (leakage == {}, the equation-carries-the-
        fraction encoding Stella uses) survives -- {} is falsy in Python, so
        a truthiness check would silently drop it."""
        flow_json: dict[str, Any] = {
            "name": "contagious_deaths",
            "equation": "0.01",
            "compat": {"nonNegative": True, "leakage": {}},
        }
        flow = converter.structure(flow_json, Flow)
        assert flow.compat is not None
        assert flow.compat.leakage == Leakage()
        assert flow.compat.non_negative is True
        assert converter.unstructure(flow) == flow_json

    @pytest.mark.parametrize("variant", ["beginning", "even", "dest", "source"])
    def test_spreadflow_unit_variants_roundtrip(self, variant: str) -> None:
        """The four payload-free spreadflow variants serialize as
        {"type": <variant>} with no distribution key."""
        flow_json: dict[str, Any] = {
            "name": "arriving",
            "equation": "250",
            "compat": {"spreadflow": {"type": variant}},
        }
        flow = converter.structure(flow_json, Flow)
        assert flow.compat is not None
        assert flow.compat.spreadflow == SpreadFlow(type=variant)
        assert converter.unstructure(flow) == flow_json

    def test_spreadflow_dist_roundtrip(self) -> None:
        """The dist variant is adjacently tagged: type + distribution."""
        flow_json: dict[str, Any] = {
            "name": "arriving",
            "equation": "250",
            "compat": {"spreadflow": {"type": "dist", "distribution": "1,2,1"}},
        }
        flow = converter.structure(flow_json, Flow)
        assert flow.compat is not None
        assert flow.compat.spreadflow == SpreadFlow(type="dist", distribution="1,2,1")
        assert converter.unstructure(flow) == flow_json

    def test_spreadflow_unknown_type_rejected(self) -> None:
        """An unknown spreadflow type raises instead of passing through."""
        with pytest.raises(Exception, match="spreadflow"):
            converter.structure({"type": "sideways"}, SpreadFlow)

    def test_spreadflow_dist_without_distribution_rejected(self) -> None:
        """A dist spreadflow without its distribution payload raises on both
        the structure and unstructure sides."""
        with pytest.raises(Exception, match="dist"):
            converter.structure({"type": "dist"}, SpreadFlow)
        with pytest.raises(Exception, match="dist"):
            converter.unstructure(SpreadFlow(type="dist"))

    def test_overflow_flow_roundtrip(self) -> None:
        """A queue outflow's overflow marker survives."""
        flow_json: dict[str, Any] = {
            "name": "overflowing",
            "compat": {"overflow": True},
        }
        flow = converter.structure(flow_json, Flow)
        assert flow.compat is not None
        assert flow.compat.overflow is True
        assert converter.unstructure(flow) == flow_json

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
        aux = converter.structure(aux_json, Auxiliary)
        assert aux.compat is not None
        assert aux.compat.data_source == DataSource(
            kind="data", file="sales.csv", tab_or_delimiter=",", row_or_col="1", cell="A2"
        )
        assert converter.unstructure(aux) == aux_json

    def test_compat_dataclass_full_roundtrip(self) -> None:
        """A Compat carrying every field round-trips Python -> JSON -> Python."""
        compat = Compat(
            active_initial="50",
            non_negative=True,
            can_be_module_input=True,
            is_public=True,
            data_source=DataSource(
                kind="constants", file="c.csv", tab_or_delimiter="\t", row_or_col="A", cell="B1"
            ),
            conveyor=Conveyor(transit_time="4", capacity="10"),
            leakage=Leakage(fraction="0.05"),
            spreadflow=SpreadFlow(type="dist", distribution="1,1"),
            queue=Queue(),
            overflow=True,
        )
        json_dict = converter.unstructure(compat)
        parsed = json.loads(json.dumps(json_dict))
        assert converter.structure(parsed, Compat) == compat

    def test_compat_omits_default_conveyor_fields(self) -> None:
        """New Compat fields are omitted when default (no spurious nulls)."""
        compat = Compat(non_negative=True)
        json_dict = converter.unstructure(compat)
        assert json_dict == {"nonNegative": True}

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
            "name": "students",
            "inflows": [],
            "outflows": [],
            "compat": {"conveyor": {"transitTime": "4"}},
            "nonNegative": True,
        }
        stock = converter.structure(stock_json, Stock)
        assert stock.compat is not None
        assert stock.compat.non_negative is True
        assert stock.compat.conveyor == Conveyor(transit_time="4")


class TestHasExceptDefault:
    """ArrayedEquation.hasExceptDefault mirrors json.rs (Option<bool>)."""

    @pytest.mark.parametrize("value", [True, False])
    def test_has_except_default_roundtrip(self, value: bool) -> None:
        aux_json: dict[str, Any] = {
            "name": "arr",
            "arrayedEquation": {
                "dimensions": ["Region"],
                "equation": "1",
                "hasExceptDefault": value,
            },
        }
        aux = converter.structure(aux_json, Auxiliary)
        assert aux.arrayed_equation is not None
        assert aux.arrayed_equation.has_except_default is value
        assert converter.unstructure(aux) == aux_json

    def test_has_except_default_absent_stays_absent(self) -> None:
        """Legacy JSON without hasExceptDefault restores None and stays
        omitted on re-serialization (matching serde's Option::is_none skip)."""
        eq = converter.structure({"dimensions": ["Region"], "equation": "1"}, ArrayedEquation)
        assert eq.has_except_default is None
        assert "hasExceptDefault" not in converter.unstructure(eq)


class TestElementCompatFullFields:
    """Element- and array-level compat must structure the full Compat, not
    just activeInitial."""

    def test_element_equation_compat_keeps_non_negative(self) -> None:
        ee = converter.structure(
            {"subscript": "east", "equation": "5", "compat": {"nonNegative": True}},
            ElementEquation,
        )
        assert ee.compat is not None
        assert ee.compat.non_negative is True

    def test_arrayed_equation_compat_keeps_non_negative(self) -> None:
        eq = converter.structure(
            {"dimensions": ["Region"], "equation": "1", "compat": {"nonNegative": True}},
            ArrayedEquation,
        )
        assert eq.compat is not None
        assert eq.compat.non_negative is True


class TestNullValueHandling:
    """Tests for correct handling of explicit null values in JSON."""

    def test_graphical_function_with_explicit_null_scales(self) -> None:
        """GraphicalFunction should accept explicit null for xScale/yScale.

        The JSON schema allows null for optional fields. When incoming JSON
        explicitly sets xScale or yScale to null, we should treat it as None,
        not raise an error.
        """
        # JSON with explicit null values for xScale and yScale
        json_with_null_scales = {
            "points": [[0.0, 1.0], [1.0, 2.0]],
            "kind": "continuous",
            "xScale": None,
            "yScale": None,
        }
        gf = converter.structure(json_with_null_scales, GraphicalFunction)
        assert gf.x_scale is None
        assert gf.y_scale is None
        assert gf.points == [(0.0, 1.0), (1.0, 2.0)]
        assert gf.kind == "continuous"

    def test_graphical_function_with_null_x_scale_only(self) -> None:
        """GraphicalFunction should handle null xScale with valid yScale."""
        json_with_mixed = {
            "points": [[0.0, 1.0], [1.0, 2.0]],
            "xScale": None,
            "yScale": {"min": 0.0, "max": 10.0},
        }
        gf = converter.structure(json_with_mixed, GraphicalFunction)
        assert gf.x_scale is None
        assert gf.y_scale is not None
        assert gf.y_scale.min == 0.0
        assert gf.y_scale.max == 10.0

    def test_graphical_function_with_null_y_scale_only(self) -> None:
        """GraphicalFunction should handle valid xScale with null yScale."""
        json_with_mixed = {
            "points": [[0.0, 1.0], [1.0, 2.0]],
            "xScale": {"min": -5.0, "max": 5.0},
            "yScale": None,
        }
        gf = converter.structure(json_with_mixed, GraphicalFunction)
        assert gf.x_scale is not None
        assert gf.x_scale.min == -5.0
        assert gf.x_scale.max == 5.0
        assert gf.y_scale is None

    def test_graphical_function_without_scale_keys(self) -> None:
        """GraphicalFunction should handle missing x_scale/y_scale keys."""
        # Keys are completely absent (different from explicit null)
        json_without_scales = {
            "points": [[0.0, 1.0], [1.0, 2.0]],
            "kind": "discrete",
        }
        gf = converter.structure(json_without_scales, GraphicalFunction)
        assert gf.x_scale is None
        assert gf.y_scale is None
        assert gf.kind == "discrete"


class TestElementEquationCompat:
    """Tests for element-level compat roundtripping."""

    def test_element_equation_compat_roundtrip(self) -> None:
        """ElementEquation with compat.activeInitial roundtrips correctly."""
        ee = ElementEquation(
            subscript="north",
            equation="50",
            compat=Compat(active_initial="10"),
        )
        json_dict = converter.unstructure(ee)
        assert "compat" in json_dict
        assert json_dict["compat"]["activeInitial"] == "10"
        assert "activeInitial" not in json_dict

        reconstructed = converter.structure(json_dict, ElementEquation)
        assert reconstructed == ee

    def test_element_equation_no_compat_roundtrip(self) -> None:
        """ElementEquation without compat omits the field."""
        ee = ElementEquation(subscript="south", equation="75")
        json_dict = converter.unstructure(ee)
        assert "compat" not in json_dict

        reconstructed = converter.structure(json_dict, ElementEquation)
        assert reconstructed == ee

    def test_arrayed_flow_with_element_compat_roundtrip(self) -> None:
        """Flow with arrayed elements carrying compat roundtrips correctly."""
        flow = Flow(
            name="rate",
            arrayed_equation=ArrayedEquation(
                dimensions=["Region"],
                elements=[
                    ElementEquation(
                        subscript="east",
                        equation="supply_east",
                        compat=Compat(active_initial="init_east"),
                    ),
                    ElementEquation(
                        subscript="west",
                        equation="supply_west",
                    ),
                ],
            ),
        )
        json_dict = converter.unstructure(flow)
        json_str = json.dumps(json_dict)
        parsed = json.loads(json_str)
        reconstructed = converter.structure(parsed, Flow)
        assert reconstructed == flow

        elems = json_dict["arrayedEquation"]["elements"]
        assert elems[0]["compat"]["activeInitial"] == "init_east"
        assert "compat" not in elems[1]
