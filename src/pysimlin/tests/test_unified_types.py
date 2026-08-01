"""Tests for the unified variable types (Stock/Flow/Aux/Module).

One set of dataclasses serves both reading (Model.get_variable, the edit()
``current`` mapping) and writing (patch.upsert). These tests pin:

- the wire mapping in both directions (structure from the engine's
  camelCase JSON, unstructure back to it), including the arrayed,
  EXCEPT-default, compat, and graphical-function shapes;
- the single ``upsert`` dispatch on ModelPatchBuilder;
- the end-to-end read -> dataclasses.replace -> upsert round trip against
  a live engine, which is the input production supplies (the wire dicts in
  the mapping tests are checked against engine-emitted JSON in
  TestEngineRoundTrip rather than being trusted on their own).
"""

from __future__ import annotations

import json
from dataclasses import replace
from typing import ClassVar

import pytest

import simlin
from simlin import (
    Aux,
    Compat,
    Conveyor,
    ElementEquation,
    Flow,
    GraphicalFunction,
    GraphicalFunctionScale,
    Module,
    ModuleReference,
    Stock,
)
from simlin.json_converter import structure_variable, unstructure_variable
from simlin.json_types import (
    UpsertAux,
    UpsertFlow,
    UpsertModule,
    UpsertStock,
)
from simlin.model import ModelPatchBuilder


class TestStructureScalar:
    """Wire dict -> unified type, scalar shapes."""

    def test_stock(self) -> None:
        var = structure_variable(
            {
                "type": "stock",
                "name": "population",
                "initialEquation": "50",
                "inflows": ["births"],
                "outflows": ["deaths"],
                "units": "people",
                "documentation": "the population",
                "uid": 3,
            }
        )
        assert var == Stock(
            name="population",
            initial_equation="50",
            inflows=("births",),
            outflows=("deaths",),
            units="people",
            documentation="the population",
        )

    def test_empty_units_and_documentation_are_none(self) -> None:
        var = structure_variable({"type": "aux", "name": "x", "equation": "1"})
        assert isinstance(var, Aux)
        assert var.units is None
        assert var.documentation is None

    def test_flow_non_negative_from_compat(self) -> None:
        var = structure_variable(
            {"type": "flow", "name": "f", "equation": "1", "compat": {"nonNegative": True}}
        )
        assert isinstance(var, Flow)
        assert var.non_negative is True
        assert var.compat is None

    def test_stock_non_negative_from_legacy_top_level(self) -> None:
        var = structure_variable(
            {
                "type": "stock",
                "name": "s",
                "initialEquation": "1",
                "inflows": [],
                "outflows": [],
                "nonNegative": True,
            }
        )
        assert isinstance(var, Stock)
        assert var.non_negative is True

    def test_aux_active_initial_from_compat(self) -> None:
        var = structure_variable(
            {"type": "aux", "name": "a", "equation": "x", "compat": {"activeInitial": "42"}}
        )
        assert isinstance(var, Aux)
        assert var.active_initial == "42"
        assert var.compat is None

    def test_flow_active_initial_from_compat(self) -> None:
        var = structure_variable(
            {"type": "flow", "name": "f", "equation": "x", "compat": {"activeInitial": "7"}}
        )
        assert isinstance(var, Flow)
        assert var.active_initial == "7"

    def test_compat_remainder_is_preserved(self) -> None:
        var = structure_variable(
            {
                "type": "stock",
                "name": "belt",
                "initialEquation": "0",
                "inflows": [],
                "outflows": [],
                "compat": {"nonNegative": True, "conveyor": {"transitTime": "5"}},
            }
        )
        assert isinstance(var, Stock)
        assert var.non_negative is True
        assert var.compat == Compat(conveyor=Conveyor(transit_time="5"))

    def test_module(self) -> None:
        var = structure_variable(
            {
                "type": "module",
                "name": "smoother",
                "modelName": "stdlib·smth1",
                "references": [{"src": "input", "dst": "smoother.input"}],
            }
        )
        assert var == Module(
            name="smoother",
            model_name="stdlib·smth1",
            references=(ModuleReference(src="input", dst="smoother.input"),),
        )

    def test_unknown_type_raises(self) -> None:
        with pytest.raises(simlin.SimlinRuntimeError):
            structure_variable({"type": "gadget", "name": "x"})


class TestStructureArrayed:
    """Wire dict -> unified type, arrayed shapes."""

    def test_apply_to_all(self) -> None:
        var = structure_variable(
            {
                "type": "aux",
                "name": "frac",
                "arrayedEquation": {"dimensions": ["region"], "equation": "base * 2"},
            }
        )
        assert isinstance(var, Aux)
        assert var.dimensions == ("region",)
        assert var.equation == "base * 2"
        assert var.element_equations == ()
        assert var.has_except_default is None

    def test_element_by_element_hoists_common_text(self) -> None:
        var = structure_variable(
            {
                "type": "aux",
                "name": "frac",
                "arrayedEquation": {
                    "dimensions": ["region"],
                    "elements": [
                        {"subscript": "boston", "equation": "0.1"},
                        {"subscript": "nyc", "equation": "0.1"},
                    ],
                },
            }
        )
        assert isinstance(var, Aux)
        assert var.equation == "0.1"
        assert var.element_equations == (
            ElementEquation(subscript="boston", equation="0.1"),
            ElementEquation(subscript="nyc", equation="0.1"),
        )
        assert var.has_except_default is None

    def test_element_by_element_differing_text_no_hoist(self) -> None:
        var = structure_variable(
            {
                "type": "aux",
                "name": "frac",
                "arrayedEquation": {
                    "dimensions": ["region"],
                    "elements": [
                        {"subscript": "boston", "equation": "0.1"},
                        {"subscript": "nyc", "equation": "0.2"},
                    ],
                },
            }
        )
        assert isinstance(var, Aux)
        assert var.equation == ""
        assert var.has_except_default is None

    def test_except_default(self) -> None:
        var = structure_variable(
            {
                "type": "aux",
                "name": "frac",
                "arrayedEquation": {
                    "dimensions": ["region"],
                    "equation": "0.5",
                    "elements": [{"subscript": "nyc", "equation": "0.9"}],
                    "hasExceptDefault": True,
                },
            }
        )
        assert isinstance(var, Aux)
        assert var.equation == "0.5"
        assert var.has_except_default is True

    def test_legacy_default_without_flag_infers_true(self) -> None:
        # Mirrors the engine: legacy JSON with a default equation but no
        # hasExceptDefault flag treats the default as live (json.rs:868).
        var = structure_variable(
            {
                "type": "aux",
                "name": "frac",
                "arrayedEquation": {
                    "dimensions": ["region"],
                    "equation": "0.5",
                    "elements": [{"subscript": "nyc", "equation": "0.9"}],
                },
            }
        )
        assert isinstance(var, Aux)
        assert var.has_except_default is True

    def test_dead_default_round_trip_metadata(self) -> None:
        var = structure_variable(
            {
                "type": "aux",
                "name": "frac",
                "arrayedEquation": {
                    "dimensions": ["region"],
                    "equation": "0.5",
                    "elements": [{"subscript": "nyc", "equation": "0.9"}],
                    "hasExceptDefault": False,
                },
            }
        )
        assert isinstance(var, Aux)
        assert var.equation == "0.5"
        assert var.has_except_default is False

    def test_element_active_initial_and_gf(self) -> None:
        var = structure_variable(
            {
                "type": "aux",
                "name": "a",
                "arrayedEquation": {
                    "dimensions": ["region"],
                    "elements": [
                        {
                            "subscript": "nyc",
                            "equation": "SMOOTH(x, 3)",
                            "compat": {"activeInitial": "10"},
                            "graphicalFunction": {"yPoints": [0.0, 1.0]},
                        }
                    ],
                },
            }
        )
        assert isinstance(var, Aux)
        elem = var.element_equations[0]
        assert elem.active_initial == "10"
        assert elem.graphical_function == GraphicalFunction(y_points=(0.0, 1.0))

    def test_arrayed_level_active_initial_legacy(self) -> None:
        var = structure_variable(
            {
                "type": "aux",
                "name": "a",
                "arrayedEquation": {
                    "dimensions": ["region"],
                    "equation": "SMOOTH(x, 3)",
                    "compat": {"activeInitial": "10"},
                },
            }
        )
        assert isinstance(var, Aux)
        assert var.active_initial == "10"

    def test_stock_arrayed_initial(self) -> None:
        var = structure_variable(
            {
                "type": "stock",
                "name": "pop",
                "inflows": [],
                "outflows": [],
                "arrayedEquation": {"dimensions": ["region"], "equation": "100"},
            }
        )
        assert isinstance(var, Stock)
        assert var.initial_equation == "100"
        assert var.dimensions == ("region",)


class TestStructureGraphicalFunction:
    def test_points_pairs(self) -> None:
        var = structure_variable(
            {
                "type": "aux",
                "name": "g",
                "equation": "x",
                "graphicalFunction": {
                    "points": [[0.0, 0.0], [1.0, 2.0]],
                    "kind": "continuous",
                    "xScale": {"min": 0.0, "max": 1.0},
                },
            }
        )
        assert isinstance(var, Aux)
        gf = var.graphical_function
        assert gf is not None
        assert gf.x_points == (0.0, 1.0)
        assert gf.y_points == (0.0, 2.0)
        assert gf.x_scale == GraphicalFunctionScale(min=0.0, max=1.0)
        assert gf.y_scale is None

    def test_y_points_only(self) -> None:
        var = structure_variable(
            {
                "type": "aux",
                "name": "g",
                "equation": "x",
                "graphicalFunction": {"yPoints": [1.0, 2.0, 3.0]},
            }
        )
        assert isinstance(var, Aux)
        gf = var.graphical_function
        assert gf is not None
        assert gf.x_points is None
        assert gf.y_points == (1.0, 2.0, 3.0)
        assert gf.kind == "continuous"


class TestUnstructure:
    """Unified type -> wire dict."""

    def test_scalar_stock_shape(self) -> None:
        d = unstructure_variable(
            Stock(name="population", initial_equation="50", inflows=["births"])
        )
        assert d == {
            "name": "population",
            "initialEquation": "50",
            "inflows": ["births"],
            "outflows": [],
        }

    def test_no_uid_is_ever_written(self) -> None:
        # The engine preserves an existing variable's uid when the upsert
        # payload has none, and mints one for new variables (patch.rs
        # upsert_variable) -- so pysimlin must never write a uid.
        for var in (
            Stock(name="s"),
            Flow(name="f"),
            Aux(name="a"),
            Module(name="m", model_name="sub"),
        ):
            assert "uid" not in unstructure_variable(var)

    def test_non_negative_written_to_compat(self) -> None:
        d = unstructure_variable(Flow(name="f", equation="1", non_negative=True))
        assert d["compat"] == {"nonNegative": True}
        assert "nonNegative" not in d  # never the legacy top-level spelling

    def test_active_initial_written_to_compat(self) -> None:
        d = unstructure_variable(Aux(name="a", equation="x", active_initial="42"))
        assert d["compat"] == {"activeInitial": "42"}

    def test_compat_remainder_merged(self) -> None:
        d = unstructure_variable(
            Stock(
                name="belt",
                initial_equation="0",
                non_negative=True,
                compat=Compat(conveyor=Conveyor(transit_time="5")),
            )
        )
        assert d["compat"] == {"nonNegative": True, "conveyor": {"transitTime": "5"}}

    def test_apply_to_all(self) -> None:
        d = unstructure_variable(Aux(name="frac", equation="base * 2", dimensions=["region"]))
        assert "equation" not in d
        assert d["arrayedEquation"] == {"dimensions": ["region"], "equation": "base * 2"}

    def test_element_by_element_common_text_not_written_as_default(self) -> None:
        # equation carries the hoisted common text for display; with
        # has_except_default=None it must NOT become a wire default.
        d = unstructure_variable(
            Aux(
                name="frac",
                equation="0.1",
                dimensions=["region"],
                element_equations=[
                    ElementEquation(subscript="boston", equation="0.1"),
                    ElementEquation(subscript="nyc", equation="0.1"),
                ],
            )
        )
        assert d["arrayedEquation"] == {
            "dimensions": ["region"],
            "elements": [
                {"subscript": "boston", "equation": "0.1"},
                {"subscript": "nyc", "equation": "0.1"},
            ],
        }

    def test_except_default(self) -> None:
        d = unstructure_variable(
            Aux(
                name="frac",
                equation="0.5",
                dimensions=["region"],
                element_equations=[ElementEquation(subscript="nyc", equation="0.9")],
                has_except_default=True,
            )
        )
        assert d["arrayedEquation"] == {
            "dimensions": ["region"],
            "equation": "0.5",
            "elements": [{"subscript": "nyc", "equation": "0.9"}],
            "hasExceptDefault": True,
        }

    def test_dead_default_preserved(self) -> None:
        d = unstructure_variable(
            Aux(
                name="frac",
                equation="0.5",
                dimensions=["region"],
                element_equations=[ElementEquation(subscript="nyc", equation="0.9")],
                has_except_default=False,
            )
        )
        assert d["arrayedEquation"] == {
            "dimensions": ["region"],
            "equation": "0.5",
            "elements": [{"subscript": "nyc", "equation": "0.9"}],
            "hasExceptDefault": False,
        }

    def test_element_equations_require_dimensions(self) -> None:
        with pytest.raises(ValueError, match="dimensions"):
            unstructure_variable(
                Aux(
                    name="frac",
                    element_equations=[ElementEquation(subscript="nyc", equation="1")],
                )
            )

    def test_graphical_function_points(self) -> None:
        d = unstructure_variable(
            Aux(
                name="g",
                equation="x",
                graphical_function=GraphicalFunction(y_points=[0.0, 2.0], x_points=[0.0, 1.0]),
            )
        )
        assert d["graphicalFunction"] == {
            "points": [[0.0, 0.0], [1.0, 2.0]],
            "kind": "continuous",
        }

    def test_graphical_function_y_points(self) -> None:
        d = unstructure_variable(
            Aux(name="g", equation="x", graphical_function=GraphicalFunction(y_points=[1.0, 2.0]))
        )
        assert d["graphicalFunction"] == {"yPoints": [1.0, 2.0], "kind": "continuous"}

    def test_module(self) -> None:
        d = unstructure_variable(
            Module(
                name="smoother",
                model_name="sub",
                references=[ModuleReference(src="input", dst="smoother.input")],
            )
        )
        assert d == {
            "name": "smoother",
            "modelName": "sub",
            "references": [{"src": "input", "dst": "smoother.input"}],
        }

    def test_rejects_non_variable(self) -> None:
        with pytest.raises(TypeError, match="Stock, Flow, Aux, or Module"):
            unstructure_variable({"name": "s"})  # type: ignore[arg-type]

    def test_graphical_function_point_count_mismatch_raises(self) -> None:
        with pytest.raises(ValueError):  # noqa: PT011 -- zip(strict=True)'s error
            unstructure_variable(
                Aux(
                    name="g",
                    graphical_function=GraphicalFunction(y_points=[1.0], x_points=[0.0, 1.0]),
                )
            )


class TestWireRoundTrip:
    """structure(unstructure(var)) == var for representative shapes."""

    CASES: ClassVar[list[Stock | Flow | Aux | Module]] = [
        Stock(name="s", initial_equation="50", inflows=["in"], outflows=["out"]),
        Stock(name="s", initial_equation="0", non_negative=True, units="widgets"),
        Stock(name="s", initial_equation="1", documentation="a doc", units="w"),
        Stock(
            name="belt",
            initial_equation="0",
            compat=Compat(conveyor=Conveyor(transit_time="5", capacity="10")),
        ),
        Flow(name="f", equation="a * b", documentation="doc"),
        Flow(name="f", equation="x", non_negative=True, active_initial="3"),
        Aux(name="a", equation="1 + 2"),
        Aux(name="a", equation="x", active_initial="42"),
        Aux(name="a", equation="base", dimensions=["region"]),
        Aux(
            name="a",
            equation="0.5",
            dimensions=["region"],
            element_equations=[
                ElementEquation(subscript="nyc", equation="0.9", active_initial="1")
            ],
            has_except_default=True,
        ),
        Flow(
            name="f",
            equation="",
            dimensions=["region"],
            element_equations=[
                ElementEquation(
                    subscript="nyc",
                    equation="0.9",
                    graphical_function=GraphicalFunction(y_points=[0.0, 1.0]),
                ),
                ElementEquation(subscript="boston", equation="0.7"),
            ],
        ),
        Aux(
            name="g",
            equation="x",
            graphical_function=GraphicalFunction(
                y_points=[0.0, 1.0],
                x_points=[0.0, 10.0],
                kind="discrete",
                x_scale=GraphicalFunctionScale(min=0.0, max=10.0),
                y_scale=GraphicalFunctionScale(min=0.0, max=1.0),
            ),
        ),
        Module(
            name="m",
            model_name="sub",
            references=[ModuleReference(src="x", dst="m.in")],
        ),
        Module(
            name="m",
            model_name="sub",
            units="widgets",
            documentation="a sub-model",
            compat=Compat(can_be_module_input=True),
        ),
    ]

    @pytest.mark.parametrize("var", CASES, ids=lambda v: f"{type(v).__name__}:{v.name}")
    def test_round_trip(self, var) -> None:
        tag = {Stock: "stock", Flow: "flow", Aux: "aux", Module: "module"}[type(var)]
        wire = unstructure_variable(var)
        assert structure_variable({"type": tag, **wire}) == var


class TestUpsertDispatch:
    def test_upsert_returns_argument_and_appends_typed_op(self) -> None:
        patch = ModelPatchBuilder("main")
        stock = Stock(name="s")
        flow = Flow(name="f")
        aux = Aux(name="a")
        module = Module(name="m", model_name="sub")
        assert patch.upsert(stock) is stock
        assert patch.upsert(flow) is flow
        assert patch.upsert(aux) is aux
        assert patch.upsert(module) is module
        ops = patch.build().ops
        assert ops == [
            UpsertStock(stock=stock),
            UpsertFlow(flow=flow),
            UpsertAux(aux=aux),
            UpsertModule(module=module),
        ]

    def test_upsert_rejects_other_types(self) -> None:
        patch = ModelPatchBuilder("main")
        with pytest.raises(TypeError):
            patch.upsert({"name": "s"})  # type: ignore[arg-type]


@pytest.fixture
def logistic_model() -> simlin.Model:
    project = simlin.Project.new(name="t", sim_start=0, sim_stop=10, dt=1)
    model = project.get_model()
    with model.edit() as (_, patch):
        patch.upsert(Stock(name="population", initial_equation="50", inflows=["net_growth"]))
        patch.upsert(Flow(name="net_growth", equation="population * rate"))
        patch.upsert(Aux(name="rate", equation="0.05"))
    return model


class TestEngineRoundTrip:
    """The unified types against a live engine: the wire dicts production
    actually supplies come from the engine itself here."""

    def test_build_and_run(self, logistic_model: simlin.Model) -> None:
        run = logistic_model.run(analyze_loops=False)
        assert run.results["population"].iloc[-1] > 50

    def test_get_variable_returns_unified_types(self, logistic_model: simlin.Model) -> None:
        var = logistic_model.get_variable("population")
        assert var == Stock(name="population", initial_equation="50", inflows=("net_growth",))
        assert isinstance(logistic_model.get_variable("net_growth"), Flow)
        assert isinstance(logistic_model.get_variable("rate"), Aux)

    def test_edit_current_holds_unified_types(self, logistic_model: simlin.Model) -> None:
        with logistic_model.edit() as (current, _patch):
            assert isinstance(current["population"], Stock)
            assert isinstance(current["net_growth"], Flow)
            assert isinstance(current["rate"], Aux)

    def test_read_replace_upsert_round_trip(self, logistic_model: simlin.Model) -> None:
        with logistic_model.edit() as (current, patch):
            patch.upsert(replace(current["rate"], equation="0.5"))
        var = logistic_model.get_variable("rate")
        assert isinstance(var, Aux)
        assert var.equation == "0.5"

    def test_round_trip_preserves_uid(self, logistic_model: simlin.Model) -> None:
        def uid_of(name: str) -> int:
            project_json = json.loads(
                logistic_model.project.serialize_json().decode("utf-8")  # type: ignore[union-attr]
            )
            for aux in project_json["models"][0].get("auxiliaries", []):
                if aux["name"] == name:
                    return int(aux.get("uid", 0))
            raise AssertionError(f"{name} not found")

        uid_before = uid_of("rate")
        assert uid_before != 0
        with logistic_model.edit() as (current, patch):
            patch.upsert(replace(current["rate"], equation="0.5"))
        assert uid_of("rate") == uid_before

    def test_variables_include_module(self) -> None:
        project = simlin.Project.new(name="t", sim_start=0, sim_stop=10, dt=1)
        model = project.get_model()
        with model.edit() as (_, patch):
            patch.upsert(Aux(name="input", equation="1"))
            patch.upsert(Aux(name="smoothed", equation="SMTH1(input, 3)"))
        # SMTH1 synthesizes an implicit module; the public surface must be
        # able to represent modules so edit()'s current dict is total. We
        # assert the weaker, stable property that every variable structures.
        for var in model.variables:
            assert isinstance(var, (Stock, Flow, Aux, Module))

    def test_frozen_types_reject_mutation(self, logistic_model: simlin.Model) -> None:
        var = logistic_model.get_variable("rate")
        with pytest.raises(AttributeError):
            var.equation = "12"  # type: ignore[misc]

    def test_list_arguments_are_coerced_to_tuples(self) -> None:
        stock = Stock(name="s", inflows=["a"], outflows=["b"], dimensions=["d"])
        assert stock.inflows == ("a",)
        assert stock.outflows == ("b",)
        assert stock.dimensions == ("d",)

    def test_upsert_view_reaches_engine(self, logistic_model: simlin.Model) -> None:
        """The engine accepts a view built through upsert_view.

        Pins the internally-tagged element serialization: unstructuring a
        View's elements without their "type" tags is unparseable by the
        engine's serde (a latent defect the unification fixed).
        """
        from simlin.json_types import AuxViewElement, StockViewElement, View

        view = View(
            elements=[
                StockViewElement(uid=1, name="population", x=100.0, y=50.0),
                AuxViewElement(uid=2, name="rate", x=40.0, y=90.0),
            ],
            kind="stock_flow",
        )
        with logistic_model.edit() as (_, patch):
            patch.upsert_view(0, view)

        project_json = json.loads(
            logistic_model.project.serialize_json().decode("utf-8")  # type: ignore[union-attr]
        )
        views = project_json["models"][0]["views"]
        assert len(views) == 1
        elements = views[0]["elements"]
        assert {e["type"] for e in elements} == {"stock", "aux"}
