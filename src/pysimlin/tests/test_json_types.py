"""Property-based tests for JSON type serialization.

These tests ensure:
1. JSON roundtrip fidelity (Python -> JSON -> Python)
2. Schema compliance (generated JSON validates against doc/simlin-project.schema.json)
"""

import json
from pathlib import Path
from typing import Any

import pytest
from hypothesis import given, strategies as st, settings, assume

try:
    import jsonschema

    HAS_JSONSCHEMA = True
except ImportError:
    HAS_JSONSCHEMA = False

from simlin.json_types import (
    Stock,
    Flow,
    Auxiliary,
    Module,
    ModuleReference,
    GraphicalFunction,
    GraphicalFunctionScale,
    ArrayedEquation,
    ElementEquation,
    JsonProjectPatch,
    JsonModelPatch,
    UpsertStock,
    UpsertFlow,
    UpsertAux,
    DeleteVariable,
    RenameVariable,
)
from simlin.json_converter import converter


# Load the JSON schema
SCHEMA_PATH = Path(__file__).parents[3] / "doc" / "simlin-project.schema.json"
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
    return Stock(
        name=draw(ident_strategy()),
        inflows=draw(st.lists(ident_strategy(), min_size=0, max_size=3)),
        outflows=draw(st.lists(ident_strategy(), min_size=0, max_size=3)),
        uid=draw(st.integers(min_value=0, max_value=10000)),
        initial_equation=draw(equation_strategy()),
        units=draw(st.sampled_from(["", "widgets", "people", "dollars"])),
        non_negative=draw(st.booleans()),
        documentation=draw(st.sampled_from(["", "A stock variable", "This accumulates over time"])),
        can_be_module_input=draw(st.booleans()),
        is_public=draw(st.booleans()),
        arrayed_equation=None,  # Keep simple for now
    )


@st.composite
def flow_strategy(draw: Any) -> Flow:
    """Generate a flow variable."""
    has_gf = draw(st.booleans())
    gf = draw(graphical_function_strategy()) if has_gf else None

    return Flow(
        name=draw(ident_strategy()),
        uid=draw(st.integers(min_value=0, max_value=10000)),
        equation=draw(equation_strategy()),
        units=draw(st.sampled_from(["", "widgets/year", "people/month"])),
        non_negative=draw(st.booleans()),
        graphical_function=gf,
        documentation=draw(st.sampled_from(["", "A flow variable"])),
        can_be_module_input=draw(st.booleans()),
        is_public=draw(st.booleans()),
        arrayed_equation=None,
    )


@st.composite
def auxiliary_strategy(draw: Any) -> Auxiliary:
    """Generate an auxiliary variable."""
    has_gf = draw(st.booleans())
    gf = draw(graphical_function_strategy()) if has_gf else None
    has_initial = draw(st.booleans())

    return Auxiliary(
        name=draw(ident_strategy()),
        uid=draw(st.integers(min_value=0, max_value=10000)),
        equation=draw(equation_strategy()),
        initial_equation=draw(equation_strategy()) if has_initial else "",
        units=draw(st.sampled_from(["", "dimensionless", "ratio"])),
        graphical_function=gf,
        documentation=draw(st.sampled_from(["", "An auxiliary variable"])),
        can_be_module_input=draw(st.booleans()),
        is_public=draw(st.booleans()),
        arrayed_equation=None,
    )


@st.composite
def module_strategy(draw: Any) -> Module:
    """Generate a module."""
    num_refs = draw(st.integers(min_value=0, max_value=3))
    refs = [
        ModuleReference(src=draw(ident_strategy()), dst=draw(ident_strategy()))
        for _ in range(num_refs)
    ]

    return Module(
        name=draw(ident_strategy()),
        model_name=draw(ident_strategy()),
        uid=draw(st.integers(min_value=0, max_value=10000)),
        units=draw(st.sampled_from(["", "widgets"])),
        documentation=draw(st.sampled_from(["", "A module"])),
        references=refs,
        can_be_module_input=draw(st.booleans()),
        is_public=draw(st.booleans()),
    )


# Roundtrip tests


class TestJsonRoundtrip:
    """Tests for JSON serialization roundtrip."""

    @given(stock=stock_strategy())
    @settings(max_examples=50)
    def test_stock_roundtrip(self, stock: Stock) -> None:
        """Stock dataclass roundtrips through JSON correctly."""
        json_dict = converter.unstructure(stock)
        json_str = json.dumps(json_dict)
        parsed = json.loads(json_str)
        reconstructed = converter.structure(parsed, Stock)
        assert stock == reconstructed

    @given(flow=flow_strategy())
    @settings(max_examples=50)
    def test_flow_roundtrip(self, flow: Flow) -> None:
        """Flow dataclass roundtrips through JSON correctly."""
        json_dict = converter.unstructure(flow)
        json_str = json.dumps(json_dict)
        parsed = json.loads(json_str)
        reconstructed = converter.structure(parsed, Flow)
        assert flow == reconstructed

    @given(aux=auxiliary_strategy())
    @settings(max_examples=50)
    def test_auxiliary_roundtrip(self, aux: Auxiliary) -> None:
        """Auxiliary dataclass roundtrips through JSON correctly."""
        json_dict = converter.unstructure(aux)
        json_str = json.dumps(json_dict)
        parsed = json.loads(json_str)
        reconstructed = converter.structure(parsed, Auxiliary)
        assert aux == reconstructed

    @given(module=module_strategy())
    @settings(max_examples=50)
    def test_module_roundtrip(self, module: Module) -> None:
        """Module dataclass roundtrips through JSON correctly."""
        json_dict = converter.unstructure(module)
        json_str = json.dumps(json_dict)
        parsed = json.loads(json_str)
        reconstructed = converter.structure(parsed, Module)
        assert module == reconstructed


class TestPatchRoundtrip:
    """Tests for patch operation roundtrip."""

    @given(stock=stock_strategy())
    @settings(max_examples=50)
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
    @settings(max_examples=50)
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
    @settings(max_examples=50)
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
    @settings(max_examples=50)
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
    @settings(max_examples=50)
    def test_stock_validates_against_schema(self, stock: Stock) -> None:
        """Generated Stock JSON validates against the schema."""
        json_dict = converter.unstructure(stock)
        self._validate_against_def(json_dict, "Stock")

    @given(flow=flow_strategy())
    @settings(max_examples=50)
    def test_flow_validates_against_schema(self, flow: Flow) -> None:
        """Generated Flow JSON validates against the schema."""
        json_dict = converter.unstructure(flow)
        self._validate_against_def(json_dict, "Flow")

    @given(aux=auxiliary_strategy())
    @settings(max_examples=50)
    def test_auxiliary_validates_against_schema(self, aux: Auxiliary) -> None:
        """Generated Auxiliary JSON validates against the schema."""
        json_dict = converter.unstructure(aux)
        self._validate_against_def(json_dict, "Auxiliary")

    @given(module=module_strategy())
    @settings(max_examples=50)
    def test_module_validates_against_schema(self, module: Module) -> None:
        """Generated Module JSON validates against the schema."""
        json_dict = converter.unstructure(module)
        self._validate_against_def(json_dict, "Module")


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
        # For Flow, non_negative defaults to False
        flow_default = Flow(name="test", non_negative=False)
        result_default = converter.unstructure(flow_default)
        assert "nonNegative" not in result_default, (
            "nonNegative=False should be omitted (equals default)"
        )

        # True should be included
        flow_true = Flow(name="test", non_negative=True)
        result_true = converter.unstructure(flow_true)
        assert result_true.get("nonNegative") is True


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
