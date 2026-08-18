"""Tests for the Model class."""

from __future__ import annotations

import json
from dataclasses import replace
from typing import TYPE_CHECKING, Any

import pytest

import simlin
from simlin import (
    VARTYPE_AUX,
    VARTYPE_FLOW,
    VARTYPE_STOCK,
    Aux,
    Model,
    SimlinRuntimeError,
)

if TYPE_CHECKING:
    from pathlib import Path


@pytest.fixture
def test_model(xmile_model_path) -> Model:
    """Create a test model from XMILE file."""
    return simlin.load(xmile_model_path)


def _canonical(name: str) -> str:
    """Approximate the engine's identifier canonicalization for
    spelling-insensitive name matching. XMILE display names embed literal
    backslash-n sequences for line breaks; those, like spaces, canonicalize
    to underscores."""
    return name.lower().replace("\\n", "_").replace(" ", "_")


class TestModelVariables:
    """Test working with model variables."""

    def test_get_incoming_links(self, test_model: Model) -> None:
        """Test getting incoming links for variables."""
        var_names = [v.name for v in test_model.variables]

        # Test at least one variable if available
        if var_names:
            deps = test_model.get_incoming_links(var_names[0])
            assert isinstance(deps, list)
            for dep in deps:
                assert isinstance(dep, str)

    def test_get_incoming_links_nonexistent_raises(self, test_model: Model) -> None:
        """Test that getting links for nonexistent variable raises error."""
        with pytest.raises(SimlinRuntimeError):
            test_model.get_incoming_links("nonexistent_variable_xyz_123")


class TestModelLinks:
    """Test model causal link analysis."""

    def test_get_links(self, test_model: Model) -> None:
        """Test getting all causal links."""
        links = test_model.get_links()
        assert isinstance(links, list)

        for link in links:
            assert hasattr(link, "from_var")
            assert hasattr(link, "to_var")
            assert hasattr(link, "polarity")
            assert isinstance(link.from_var, str)
            assert isinstance(link.to_var, str)
            # Static analysis doesn't have scores
            assert link.score is None

    def test_link_str_representation(self, test_model: Model) -> None:
        """Test string representation of links."""
        links = test_model.get_links()
        if links:
            link_str = str(links[0])
            assert "--" in link_str
            assert links[0].from_var in link_str
            assert links[0].to_var in link_str


class TestModelSimulation:
    """Test creating simulations from models."""

    def test_multiple_sims(self, test_model: Model) -> None:
        """Test creating multiple simulations from the same model."""
        sim1 = test_model.simulate()
        sim2 = test_model.simulate()
        assert sim1 is not sim2
        # Both should be valid
        sim1.run_to_end()
        sim2.run_to_end()


class TestModelContextManager:
    """Test context manager functionality for models."""

    def test_context_manager_basic_usage(self, xmile_model_path) -> None:
        """Test basic context manager usage."""
        model = simlin.load(xmile_model_path)
        with model:
            assert model is not None
            assert len(model.variables) > 0
            # Model should be usable inside the context
            var_names = [v.name for v in model.variables]
            assert len(var_names) > 0

    def test_context_manager_returns_self(self, test_model: Model) -> None:
        """Test that __enter__ returns self."""
        with test_model as ctx_model:
            assert ctx_model is test_model

    def test_context_manager_explicit_cleanup(self, test_model: Model) -> None:
        """Test that __exit__ performs explicit cleanup."""
        from simlin._ffi import ffi

        original_ptr = test_model._ptr

        # Use as context manager
        with test_model:
            pass

        # After context exit, pointer should be NULL
        assert test_model._ptr == ffi.NULL
        assert original_ptr != ffi.NULL  # Original was valid

    def test_context_manager_with_exception(self, xmile_model_path) -> None:
        """Test context manager cleanup when exception occurs."""
        from simlin._ffi import ffi

        model = simlin.load(xmile_model_path)

        try:
            with model:
                # Simulate an exception
                raise ValueError("Test exception")
        except ValueError:
            pass

        # Even with exception, cleanup should occur
        assert model._ptr == ffi.NULL

    def test_nested_context_managers(self, xmile_model_path) -> None:
        """Test nested context managers with model and sim."""
        model = simlin.load(xmile_model_path)
        with model:
            # Model should be usable inside context
            assert len(model.variables) > 0
            sim = model.simulate()
            assert sim is not None


class TestModelEditing:
    """Tests for the model editing context manager."""

    def test_edit_context_applies_flow_changes(self, mdl_model_path) -> None:
        """Patches created inside edit() should apply when the context exits."""
        model = simlin.load(mdl_model_path)

        with model.edit(allow_errors=True) as (current, patch):
            patch.upsert(replace(current["heat_loss_to_room"], equation="0"))

        # Verify via JSON serialization
        project_json = json.loads(model.project.serialize_json().decode("utf-8"))
        flow_dict = next(
            f for f in project_json["models"][0]["flows"] if f["name"] == "heat_loss_to_room"
        )
        assert flow_dict.get("equation", "") == "0"

    def test_edit_update_stock_flows_rewires_without_touching_other_fields(
        self, teacup_stmx_path
    ) -> None:
        """updateStockFlows replaces only the flow lists; the engine keeps
        every other stock field (initial equation, units, docs)."""
        from simlin import Flow, Stock

        model = simlin.load(teacup_stmx_path)
        baseline_final = model.run(analyze_loops=False).results["teacup_temperature"].iloc[-1]
        stock = model.get_variable("teacup_temperature")
        assert isinstance(stock, Stock)
        assert stock.outflows == ("heat_loss_to_room",)
        assert stock.inflows == ()

        with model.edit() as (_, patch):
            patch.upsert(Flow(name="reheating", equation="1"))
            patch.update_stock_flows(
                "teacup_temperature", inflows=["reheating"], outflows=stock.outflows
            )

        after = model.get_variable("teacup_temperature")
        assert isinstance(after, Stock)
        assert after.inflows == ("reheating",)
        assert after.outflows == ("heat_loss_to_room",)
        assert after.initial_equation == stock.initial_equation
        assert after.units == stock.units
        assert after.documentation == stock.documentation
        # And the wiring is real: the new inflow changes behaviour.
        final = model.run(analyze_loops=False).results["teacup_temperature"].iloc[-1]
        assert final > baseline_final

    def test_edit_update_stock_flows_unknown_stock_rejected(self, teacup_stmx_path) -> None:
        from simlin import SimlinRuntimeError

        model = simlin.load(teacup_stmx_path)
        with pytest.raises(SimlinRuntimeError), model.edit() as (_, patch):
            patch.update_stock_flows("no_such_stock", inflows=[], outflows=[])

    def test_edit_context_dry_run_does_not_commit(self, mdl_model_path) -> None:
        """dry_run=True should validate without mutating the project."""
        model = simlin.load(mdl_model_path)

        # Get original equation via JSON
        original_json = json.loads(model.project.serialize_json().decode("utf-8"))
        original_flow = next(
            f for f in original_json["models"][0]["flows"] if f["name"] == "heat_loss_to_room"
        )
        original_equation = original_flow.get("equation", "")

        with model.edit(dry_run=True, allow_errors=True) as (current, patch):
            patch.upsert(replace(current["heat_loss_to_room"], equation="0"))

        # Verify equation unchanged via JSON
        after_json = json.loads(model.project.serialize_json().decode("utf-8"))
        after_flow = next(
            f for f in after_json["models"][0]["flows"] if f["name"] == "heat_loss_to_room"
        )
        assert after_flow.get("equation", "") == original_equation

    def test_edit_context_invalid_patch_raises(self, xmile_model_path) -> None:
        """Invalid edits should raise and leave the project unchanged."""
        model = simlin.load(xmile_model_path)

        before_json = model.project.serialize_json()

        with (
            pytest.raises(SimlinRuntimeError),
            model.edit() as (_, patch),
        ):
            patch.upsert(
                Aux(
                    name="bad_variable",
                    equation="?? invalid expression",
                )
            )

        after_json = model.project.serialize_json()
        assert after_json == before_json

    def test_edit_context_allow_errors_collects_errors(self, xmile_model_path) -> None:
        """allow_errors=True should collect errors without raising."""
        model = simlin.load(xmile_model_path)

        # This should not raise - errors are collected
        with model.edit(allow_errors=True) as (_, patch):
            bad_aux = Aux(
                name="bad_variable",
                equation="?? invalid expression",
            )
            patch.upsert(bad_aux)

        # The variable should be added despite the error
        project_json = json.loads(model.project.serialize_json().decode("utf-8"))
        aux_names = [a["name"] for a in project_json["models"][0].get("auxiliaries", [])]
        assert "bad_variable" in aux_names

    def test_edit_context_dry_run_with_invalid_raises(self, xmile_model_path) -> None:
        """dry_run=True should still raise on invalid patches when allow_errors=False."""
        model = simlin.load(xmile_model_path)

        before_json = model.project.serialize_json()

        with (
            pytest.raises(SimlinRuntimeError),
            model.edit(dry_run=True) as (_, patch),
        ):
            patch.upsert(
                Aux(
                    name="bad_variable",
                    equation="?? invalid expression",
                )
            )

        # Verify project unchanged
        after_json = model.project.serialize_json()
        assert after_json == before_json

    def test_edit_context_dry_run_allow_errors_validates_only(self, xmile_model_path) -> None:
        """dry_run=True with allow_errors=True should validate without mutating."""
        model = simlin.load(xmile_model_path)

        before_json = model.project.serialize_json()

        # Should not raise, should not mutate
        with model.edit(dry_run=True, allow_errors=True) as (_, patch):
            bad_aux = Aux(
                name="bad_variable",
                equation="?? invalid expression",
            )
            patch.upsert(bad_aux)

        # Project should be unchanged
        after_json = model.project.serialize_json()
        assert after_json == before_json

    def test_edit_preserves_conveyor_marker(self, conveyor_model_path) -> None:
        """Re-upserting a conveyor stock through edit() must not demote it to
        a plain stock (GH #882: the converter silently dropped compat.conveyor)."""
        model = simlin.load(conveyor_model_path)

        with model.edit() as (current, patch):
            patch.upsert(replace(current["Students"], documentation="cohort pipeline"))

        # Match on the canonical form so this test is insensitive to display
        # name spelling (patch application preserves it since GH #890).
        after = json.loads(model.project.serialize_json().decode("utf-8"))
        stock = next(s for s in after["models"][0]["stocks"] if _canonical(s["name"]) == "students")
        assert stock.get("documentation") == "cohort pipeline"
        conveyor = stock.get("compat", {}).get("conveyor")
        assert conveyor is not None, "conveyor marker dropped by edit round-trip"
        assert conveyor["transitTime"] == "4"
        assert conveyor["capacity"] == "1200"
        assert conveyor["oneAtATime"] is True

    def test_edit_preserves_leakage_and_spreadflow_markers(self, covid_conveyor_model_path) -> None:
        """Leak/spreadflow flow markers from a real Stella model survive an
        unrelated documentation edit round-tripped through the converter."""
        model = simlin.load(covid_conveyor_model_path)

        before = json.loads(model.project.serialize_json().decode("utf-8"))
        flows = before["models"][0]["flows"]
        leak_flow = next(
            f
            for f in flows
            if "leakage" in f.get("compat", {}) and "spreadflow" in f.get("compat", {})
        )
        name = leak_flow["name"]

        with model.edit(allow_errors=True) as (current, patch):
            patch.upsert(replace(current[name], documentation="edited"))

        after = json.loads(model.project.serialize_json().decode("utf-8"))
        flow_after = next(
            f for f in after["models"][0]["flows"] if _canonical(f["name"]) == _canonical(name)
        )
        assert flow_after.get("documentation") == "edited"
        compat_after = flow_after.get("compat", {})
        assert compat_after.get("leakage") == leak_flow["compat"]["leakage"], (
            "leakage marker dropped by edit round-trip"
        )
        assert compat_after.get("spreadflow") == leak_flow["compat"]["spreadflow"], (
            "spreadflow marker dropped by edit round-trip"
        )
        assert compat_after.get("nonNegative") == leak_flow["compat"].get("nonNegative")

    def test_edit_preserves_display_names(
        self, conveyor_model_path, covid_conveyor_model_path
    ) -> None:
        """An upsert must not rewrite the variable's display name: the stored
        name keeps its casing and XMILE backslash-n line breaks (GH #890)."""
        model = simlin.load(conveyor_model_path)

        with model.edit() as (current, patch):
            patch.upsert(replace(current["Students"], documentation="x"))

        after = json.loads(model.project.serialize_json().decode("utf-8"))
        names = [s["name"] for s in after["models"][0]["stocks"]]
        assert "Students" in names, f"display name destroyed: {names}"

        # A flow whose Stella display name embeds a literal backslash-n line
        # break must survive an unrelated documentation edit verbatim.
        model = simlin.load(covid_conveyor_model_path)
        before = json.loads(model.project.serialize_json().decode("utf-8"))
        name = next(f["name"] for f in before["models"][0]["flows"] if "\\n" in f["name"])

        with model.edit(allow_errors=True) as (current, patch):
            patch.upsert(replace(current[name], documentation="edited"))

        after = json.loads(model.project.serialize_json().decode("utf-8"))
        names_after = [f["name"] for f in after["models"][0]["flows"]]
        assert name in names_after, f"line-break display name destroyed: {names_after}"

    def test_apply_patch_json_invalid_json_raises(self, xmile_model_path) -> None:
        """Malformed JSON should raise an error."""
        model = simlin.load(xmile_model_path)

        with pytest.raises(SimlinRuntimeError):
            model.project._apply_patch_json(b"{ not valid json }")

    def test_apply_patch_json_returns_errors_when_allowed(self, xmile_model_path) -> None:
        """apply_patch_json with allow_errors=True should return error details."""
        import json as json_module

        from simlin.errors import ErrorCode, ErrorDetail
        from simlin.json_converter import converter
        from simlin.json_types import JsonModelPatch, JsonProjectPatch, UpsertAux

        model = simlin.load(xmile_model_path)

        # Create a patch with an invalid equation (??? is not valid syntax)
        bad_aux = Aux(name="broken_var", equation="??? totally invalid")
        patch = JsonProjectPatch(
            models=[JsonModelPatch(name=model._name or "main", ops=[UpsertAux(aux=bad_aux)])]
        )
        patch_json = json_module.dumps(converter.unstructure(patch)).encode("utf-8")

        errors = model.project._apply_patch_json(patch_json, allow_errors=True)

        # Verify errors are collected and contain meaningful diagnostic info
        assert isinstance(errors, list)
        assert len(errors) > 0, "Expected errors to be collected for invalid equation"

        # Verify at least one error has meaningful information about the failure
        error = errors[0]
        assert isinstance(error, ErrorDetail)
        assert error.code != ErrorCode.NO_ERROR, "Error should have a non-zero error code"
        # The error should reference the variable with the bad equation
        assert error.variable_name == "broken_var", (
            f"Expected variable_name='broken_var', got '{error.variable_name}'"
        )


class TestModelStructuralProperties:
    """Test structural properties of Model."""

    def test_variables_property(self, test_model: Model) -> None:
        """Test that variables property returns tuple of Stock/Flow/Aux objects."""
        variables = test_model.variables
        assert isinstance(variables, tuple)
        assert len(variables) > 0

        from simlin.types import Aux, Flow, Stock

        for var in variables:
            assert isinstance(var, (Stock, Flow, Aux))
            assert isinstance(var.name, str)
            assert len(var.name) > 0

    def test_get_var_names(self, test_model: Model) -> None:
        """Test get_var_names returns canonical variable names."""
        names = test_model.get_var_names()
        assert isinstance(names, list)
        assert len(names) > 0
        for name in names:
            assert isinstance(name, str)

    def test_get_var_names_with_type_mask(self, teacup_stmx_path) -> None:
        """Test get_var_names with type_mask filtering."""
        model = simlin.load(teacup_stmx_path)

        stock_names = model.get_var_names(type_mask=VARTYPE_STOCK)
        assert len(stock_names) > 0
        for name in stock_names:
            var = model.get_variable(name)
            from simlin.types import Stock

            assert isinstance(var, Stock)

        flow_names = model.get_var_names(type_mask=VARTYPE_FLOW)
        assert len(flow_names) > 0
        for name in flow_names:
            var = model.get_variable(name)
            from simlin.types import Flow

            assert isinstance(var, Flow)

        aux_names = model.get_var_names(type_mask=VARTYPE_AUX)
        assert len(aux_names) > 0
        for name in aux_names:
            var = model.get_variable(name)
            from simlin.types import Aux

            assert isinstance(var, Aux)

    def test_get_var_names_with_filter(self, teacup_stmx_path) -> None:
        """Test get_var_names with substring filter."""
        model = simlin.load(teacup_stmx_path)

        # Filter for variables containing "temperature"
        temp_names = model.get_var_names(filter_str="temperature")
        assert len(temp_names) > 0
        for name in temp_names:
            assert "temperature" in name

    def test_time_spec_property(self, test_model: Model) -> None:
        """Test that time_spec property returns TimeSpec."""
        from simlin.types import TimeSpec

        time_spec = test_model.time_spec
        assert isinstance(time_spec, TimeSpec)
        assert time_spec.start >= 0
        assert time_spec.stop > time_spec.start
        assert time_spec.dt > 0

    def test_loops_property(self, test_model: Model) -> None:
        """Test that loops property returns tuple of Loop objects."""
        loops = test_model.loops
        assert isinstance(loops, tuple)

        for loop in loops:
            from simlin.analysis import Loop

            assert isinstance(loop, Loop)
            assert isinstance(loop.id, str)
            assert isinstance(loop.variables, tuple)
            assert loop.behavior_time_series is None

    def test_loops_carry_partition_index(self, test_model: Model) -> None:
        """Every structural loop carries a partition (None or non-negative int)
        indexing model.loop_partitions (GH #685)."""
        loops = test_model.loops
        partitions = test_model.loop_partitions
        assert isinstance(partitions, tuple)
        for loop in loops:
            assert loop.partition is None or isinstance(loop.partition, int)
            if loop.partition is not None:
                assert 0 <= loop.partition < len(partitions)

    def test_structural_properties_consistent(self, test_model: Model) -> None:
        """Test that structural properties return equal results across calls."""
        vars1 = test_model.variables
        vars2 = test_model.variables
        assert vars1 == vars2

        names1 = test_model.get_var_names()
        names2 = test_model.get_var_names()
        assert names1 == names2

        time_spec1 = test_model.time_spec
        time_spec2 = test_model.time_spec
        assert time_spec1 == time_spec2


_TWO_PARTITION_XMILE = """<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><vendor>Test</vendor><product version="1.0">Test</product></header>
  <sim_specs method="euler"><start>0</start><stop>5</stop><dt>1</dt></sim_specs>
  <model>
    <variables>
      <stock name="pop_a"><eqn>100</eqn><inflow>births_a</inflow></stock>
      <flow name="births_a"><eqn>pop_a * 0.02</eqn></flow>
      <stock name="pop_b"><eqn>50</eqn><inflow>births_b</inflow></stock>
      <flow name="births_b"><eqn>pop_b * 0.03</eqn></flow>
    </variables>
  </model>
</xmile>"""


class TestModelPartitions:
    """Cycle-partition metadata on the exhaustive/structural loop surface (GH #685)."""

    @pytest.fixture
    def two_partition_model(self, tmp_path: Path) -> Model:
        p = tmp_path / "two_partition.stmx"
        p.write_text(_TWO_PARTITION_XMILE)
        return simlin.load(p)

    def test_two_independent_loops_get_distinct_partitions(
        self, two_partition_model: Model
    ) -> None:
        loops = two_partition_model.loops
        partitions = two_partition_model.loop_partitions
        assert len(loops) == 2, "two independent loops expected"
        assert len(partitions) == 2, "two disjoint stock SCCs => two partitions"
        idxs = {loop.partition for loop in loops}
        assert None not in idxs, "both loops must resolve a partition"
        assert len(idxs) == 2, "independent loops must be in distinct partitions"
        stock_sets = {frozenset(part.stocks) for part in partitions}
        assert any("pop_a" in s for s in stock_sets)
        assert any("pop_b" in s for s in stock_sets)

    def test_exhaustive_and_discovery_partitions_agree_on_stock_sets(
        self, two_partition_model: Model
    ) -> None:
        """The partition's stock SET is a cross-surface key:
        Model.loop_partitions and Model.analyze().partitions must agree on the
        stock sets (indices are result-scoped and may differ). Since GH #746
        both surfaces partition at element granularity, so this holds for
        scalar and arrayed models alike (this fixture is scalar; the arrayed
        twin is pinned engine-side)."""
        exhaustive_sets = {frozenset(part.stocks) for part in two_partition_model.loop_partitions}
        analysis = two_partition_model.analyze()
        discovery_sets = {frozenset(part.stocks) for part in analysis.partitions}
        assert exhaustive_sets, "exhaustive surface must report partitions"
        assert discovery_sets, "discovery surface must report partitions"
        assert exhaustive_sets == discovery_sets


class TestModelSimulationMethods:
    """Test the new simulation methods of Model."""

    def test_simulate_method(self, test_model: Model) -> None:
        """Test simulate() method returns Sim."""
        from simlin import Sim

        sim = test_model.simulate()
        assert isinstance(sim, Sim)

    def test_simulate_with_overrides(self, teacup_stmx_path) -> None:
        """simulate(overrides=...) reaches the engine and changes behavior."""
        model = simlin.load(teacup_stmx_path)

        # room_temperature is a simple constant (equation = "70")
        with model.simulate(overrides={"room_temperature": 42.0}) as sim:
            sim.run_to_end()
            overridden = sim.get_series("teacup_temperature")

        base = model.base_case.results["teacup_temperature"]

        assert overridden[0] == pytest.approx(base.iloc[0]), (
            "the stock's initial value is unchanged"
        )
        assert overridden[-1] != pytest.approx(base.iloc[-1]), (
            "cooling toward a 42-degree room must not land where cooling toward 70 does"
        )

    def test_simulate_with_ltm(self, test_model: Model) -> None:
        """Test simulate() with LTM enabled."""
        from simlin import Sim

        sim = test_model.simulate(enable_ltm=True)
        assert isinstance(sim, Sim)

    def test_run_with_overrides(self, teacup_stmx_path) -> None:
        """Test run() with variable overrides."""
        from simlin.run import Run

        model = simlin.load(teacup_stmx_path)

        # room_temperature is a simple constant (equation = "70")
        overrides = {"room_temperature": 123.0}
        run = model.run(overrides=overrides, analyze_loops=False)
        assert isinstance(run, Run)
        assert run.overrides == overrides

    def test_run_with_analyze_loops(self, test_model: Model) -> None:
        """analyze_loops=True turns LTM on for the run.

        The flag has to reach the engine: with it set the run resolves a real
        loop-enumeration mode, whereas the default run reports "disabled"
        (pinned by TestGetRunLifetime in test_sim.py).
        """
        run = test_model.run(analyze_loops=True)
        assert run.ltm_mode in ("exhaustive", "discovery")

    def test_base_case_cached(self, test_model: Model) -> None:
        """Test that base_case is cached."""
        base1 = test_model.base_case
        base2 = test_model.base_case
        assert base1 is base2

    def test_base_case_has_no_overrides(self, test_model: Model) -> None:
        """Test that base_case has empty overrides."""
        base_case = test_model.base_case
        assert base_case.overrides == {}

    def test_base_case_has_results(self, test_model: Model) -> None:
        """Test that base_case has results."""
        import pandas as pd

        base_case = test_model.base_case
        assert isinstance(base_case.results, pd.DataFrame)
        assert len(base_case.results) > 0


class TestModelUtilities:
    """Test utility methods of Model."""

    def test_check_method_on_valid_model(self, test_model: Model) -> None:
        """Test check() on a valid model returns empty or valid issues."""
        issues = test_model.check()
        assert isinstance(issues, tuple)

        for issue in issues:
            from simlin import ModelIssue

            assert isinstance(issue, ModelIssue)
            assert hasattr(issue, "severity")
            assert hasattr(issue, "message")
            assert isinstance(issue.severity, str)
            assert isinstance(issue.message, str)

    def test_explain_stock(self, test_model: Model) -> None:
        """Test explain() for a stock variable."""
        from simlin.types import Stock

        stock = next((v for v in test_model.variables if isinstance(v, Stock)), None)
        if stock is None:
            pytest.skip("No stocks in test model")

        explanation = test_model.explain(stock.name)
        assert isinstance(explanation, str)
        assert stock.name in explanation
        assert "stock" in explanation

    def test_explain_flow(self, test_model: Model) -> None:
        """Test explain() for a flow variable."""
        from simlin.types import Flow

        flow = next((v for v in test_model.variables if isinstance(v, Flow)), None)
        if flow is None:
            pytest.skip("No flows in test model")

        explanation = test_model.explain(flow.name)
        assert isinstance(explanation, str)
        assert flow.name in explanation
        assert "flow" in explanation

    def test_explain_aux(self, test_model: Model) -> None:
        """Test explain() for an auxiliary variable."""
        from simlin.types import Aux

        aux = next((v for v in test_model.variables if isinstance(v, Aux)), None)
        if aux is None:
            pytest.skip("No auxiliary variables in test model")

        explanation = test_model.explain(aux.name)
        assert isinstance(explanation, str)
        assert aux.name in explanation
        assert "auxiliary" in explanation

    def test_explain_module(self, modules_model_path) -> None:
        """explain() has a Module arm too -- the fourth of its four type arms.

        Modules only appear in a multi-model project, so this needs a model
        the other explain() tests (which use the single-model fixture) cannot
        reach.
        """
        from simlin.types import Module

        model = simlin.load(modules_model_path)

        module = next(v for v in model.variables if isinstance(v, Module))
        explanation = model.explain(module.name)
        assert module.name in explanation
        assert module.model_name in explanation

    def test_explain_nonexistent_raises(self, test_model: Model) -> None:
        """Test explain() raises error for nonexistent variable."""
        with pytest.raises(SimlinRuntimeError) as exc_info:
            test_model.explain("nonexistent_variable_xyz")

        assert "not found" in str(exc_info.value).lower()
        assert "nonexistent_variable_xyz" in str(exc_info.value)


class TestArrayedEquations:
    """Test extraction of arrayed (subscripted) variable equations."""

    def test_flow_with_apply_to_all_equation(self, subscripted_model_path: Path) -> None:
        """Arrayed flows using apply-to-all equations should expose the actual equation.

        For arrayed variables, XMILE stores equations in different places depending
        on how they're defined. For "apply-to-all" equations (same formula for all
        subscript elements), the equation is stored in arrayed_equation.equation
        rather than the top-level equation field.
        """
        from simlin.types import Flow

        model = simlin.load(subscripted_model_path)

        flows_by_name = {v.name: v for v in model.variables if isinstance(v, Flow)}

        assert "Inflow A" in flows_by_name or "inflow_a" in flows_by_name
        inflow_a = flows_by_name.get("Inflow A") or flows_by_name.get("inflow_a")
        assert inflow_a is not None

        assert inflow_a.equation, "Arrayed flow equation should not be empty"
        assert "Rate_A" in inflow_a.equation or "rate_a" in inflow_a.equation.lower()

    def test_stock_with_apply_to_all_initial(self, subscripted_model_path: Path) -> None:
        """Arrayed stocks should expose their initial equation."""
        from simlin.types import Stock

        model = simlin.load(subscripted_model_path)

        stocks_by_name = {v.name: v for v in model.variables if isinstance(v, Stock)}

        assert "Stock A" in stocks_by_name or "stock_a" in stocks_by_name
        stock_a = stocks_by_name.get("Stock A") or stocks_by_name.get("stock_a")
        assert stock_a is not None

        assert stock_a.initial_equation == "0"

    def test_per_element_equations_exposed(self, cross_element_ltm_path: Path) -> None:
        """Arrayed variables defined element-by-element expose every element's
        equation via element_equations.

        Before this field existed, a per-element arrayed variable showed an
        empty .equation and there was NO way to see its formulas at all.
        """
        from simlin.types import Aux

        model = simlin.load(cross_element_ltm_path)
        var = model.get_variable("migration_pressure")
        assert isinstance(var, Aux)

        # Subscript keys are canonical (lowercase) names, consistent with the
        # rest of the API; equation text preserves the authored casing.
        subs = {e.subscript: e.equation for e in var.element_equations}
        assert set(subs) == {"nyc", "boston"}
        assert "population[NYC]" in subs["nyc"]
        assert "population[Boston]" in subs["boston"]
        # The two elements have different formulas, so there is no single
        # representative equation.
        assert var.equation == ""

    def test_per_element_stock_initials_exposed(self, cross_element_ltm_path: Path) -> None:
        """Per-element stock initial values are exposed via element_equations."""
        from simlin.types import Stock

        model = simlin.load(cross_element_ltm_path)
        var = model.get_variable("population")
        assert isinstance(var, Stock)

        subs = {e.subscript: e.equation for e in var.element_equations}
        assert subs == {"nyc": "1000", "boston": "500"}
        assert var.initial_equation == ""

    def test_identical_per_element_equations_collapse(self) -> None:
        """When every element carries the same equation text -- the shape the
        Vensim importer produces for apply-to-all equations -- .equation reports
        that common text instead of being empty."""
        from simlin.json_converter import structure_variable
        from simlin.types import ElementEquation

        d: dict[str, Any] = {
            "type": "aux",
            "name": "atm_conc_co2",
            "arrayedEquation": {
                "dimensions": ["scenario"],
                "elements": [
                    {"subscript": "Deterministic", "equation": "C_in_Atmosphere[scenario] * ppm"},
                    {"subscript": "High", "equation": "C_in_Atmosphere[scenario] * ppm"},
                    {"subscript": "Low", "equation": "C_in_Atmosphere[scenario] * ppm"},
                ],
            },
        }
        aux = structure_variable(d)
        assert isinstance(aux, Aux)
        assert aux.equation == "C_in_Atmosphere[scenario] * ppm"
        assert aux.element_equations == (
            ElementEquation(subscript="Deterministic", equation="C_in_Atmosphere[scenario] * ppm"),
            ElementEquation(subscript="High", equation="C_in_Atmosphere[scenario] * ppm"),
            ElementEquation(subscript="Low", equation="C_in_Atmosphere[scenario] * ppm"),
        )

    def test_scalar_variables_have_empty_element_equations(self, teacup_stmx_path: Path) -> None:
        """Scalar variables report an empty element_equations tuple."""
        model = simlin.load(teacup_stmx_path)
        var = model.get_variable("room_temperature")
        assert var is not None
        assert var.element_equations == ()


class TestGetVariable:
    """Test the get_variable() method for single-variable lookup."""

    def test_get_stock_by_name(self, teacup_stmx_path: Path) -> None:
        """get_variable should return a Stock for stock variables."""
        from simlin.types import Stock

        model = simlin.load(teacup_stmx_path)
        var = model.get_variable("teacup_temperature")
        assert isinstance(var, Stock)
        assert var.name == "teacup temperature"
        assert var.initial_equation == "180"

    def test_get_flow_by_name(self, teacup_stmx_path: Path) -> None:
        """get_variable should return a Flow for flow variables."""
        from simlin.types import Flow

        model = simlin.load(teacup_stmx_path)
        var = model.get_variable("heat_loss_to_room")
        assert isinstance(var, Flow)
        assert var.name == "heat loss to room"

    def test_get_aux_by_name(self, teacup_stmx_path: Path) -> None:
        """get_variable should return an Aux for auxiliary variables."""
        from simlin.types import Aux

        model = simlin.load(teacup_stmx_path)
        var = model.get_variable("room_temperature")
        assert isinstance(var, Aux)
        assert var.name == "room temperature"
        assert var.equation == "70"

    def test_get_nonexistent_returns_none(self, teacup_stmx_path: Path) -> None:
        """get_variable should return None for nonexistent variables."""
        model = simlin.load(teacup_stmx_path)
        var = model.get_variable("this_does_not_exist_at_all")
        assert var is None

    def test_get_variable_with_units(self, teacup_stmx_path: Path) -> None:
        """get_variable should include units when present."""
        model = simlin.load(teacup_stmx_path)
        var = model.get_variable("teacup_temperature")
        assert var is not None
        assert var.units == "degrees"

    def test_get_variable_stock_has_flows(self, teacup_stmx_path: Path) -> None:
        """get_variable for a stock should include inflows and outflows."""
        from simlin.types import Stock

        model = simlin.load(teacup_stmx_path)
        var = model.get_variable("teacup_temperature")
        assert isinstance(var, Stock)
        assert "heat_loss_to_room" in var.outflows

    def test_get_variable_matches_variables_property(self, teacup_stmx_path: Path) -> None:
        """get_variable should return data consistent with the variables property."""
        model = simlin.load(teacup_stmx_path)
        for var in model.variables:
            looked_up = model.get_variable(var.name)
            assert looked_up is not None
            assert looked_up == var


class TestGetVarNamesTypeMask:
    """Verify get_var_names type_mask correctly partitions variable types."""

    def test_type_masks_disjoint(self, teacup_stmx_path: Path) -> None:
        """Stock, flow, and aux type masks should produce disjoint name sets."""
        model = simlin.load(teacup_stmx_path)
        stock_names = set(model.get_var_names(type_mask=VARTYPE_STOCK))
        flow_names = set(model.get_var_names(type_mask=VARTYPE_FLOW))
        aux_names = set(model.get_var_names(type_mask=VARTYPE_AUX))
        assert stock_names.isdisjoint(flow_names)
        assert stock_names.isdisjoint(aux_names)
        assert flow_names.isdisjoint(aux_names)

    def test_combined_mask_is_union(self, teacup_stmx_path: Path) -> None:
        """Combined type mask should return union of individual masks."""
        model = simlin.load(teacup_stmx_path)
        stock_names = set(model.get_var_names(type_mask=VARTYPE_STOCK))
        flow_names = set(model.get_var_names(type_mask=VARTYPE_FLOW))
        combined = set(model.get_var_names(type_mask=VARTYPE_STOCK | VARTYPE_FLOW))
        assert combined == stock_names | flow_names


class TestVartypeConstants:
    """Verify VARTYPE_* constants match the C FFI values."""

    def test_constants_match_ffi(self) -> None:
        """Constants must match the SIMLIN_VARTYPE_* values from C header."""
        from simlin._ffi import lib

        assert VARTYPE_STOCK == lib.SIMLIN_VARTYPE_STOCK
        assert VARTYPE_FLOW == lib.SIMLIN_VARTYPE_FLOW
        assert VARTYPE_AUX == lib.SIMLIN_VARTYPE_AUX

    def test_constants_are_powers_of_two(self) -> None:
        """Each constant should be a single bit so they compose via bitwise OR."""
        from simlin import VARTYPE_MODULE

        for val in (VARTYPE_STOCK, VARTYPE_FLOW, VARTYPE_AUX, VARTYPE_MODULE):
            assert val > 0
            assert val & (val - 1) == 0, f"{val} is not a power of two"


class TestStockFromDict:
    """Unit tests for the stock wire-dict parsing in structure_variable."""

    def test_arrayed_stock_equation_as_initial(self) -> None:
        """XMILE-sourced stocks store their initial value in arrayedEquation.equation."""
        from simlin.json_converter import structure_variable

        d: dict[str, Any] = {
            "type": "stock",
            "name": "arrayed_stock",
            "initialEquation": "",
            "inflows": [],
            "outflows": [],
            "arrayedEquation": {
                "dimensions": ["Region"],
                "equation": "100",
            },
        }
        stock = structure_variable(d)
        assert isinstance(stock, simlin.Stock)
        assert stock.initial_equation == "100"

    def test_arrayed_stock_initial_equation_field(self) -> None:
        """JSON-sourced stocks can use arrayedEquation.initialEquation."""
        from simlin.json_converter import structure_variable

        d: dict[str, Any] = {
            "type": "stock",
            "name": "arrayed_stock",
            "initialEquation": "",
            "inflows": [],
            "outflows": [],
            "arrayedEquation": {
                "dimensions": ["Region"],
                "initialEquation": "200",
            },
        }
        stock = structure_variable(d)
        assert isinstance(stock, simlin.Stock)
        assert stock.initial_equation == "200"

    def test_arrayed_stock_initial_equation_preferred_over_equation(self) -> None:
        """When both are present, initialEquation takes precedence over equation."""
        from simlin.json_converter import structure_variable

        d: dict[str, Any] = {
            "type": "stock",
            "name": "arrayed_stock",
            "initialEquation": "",
            "inflows": [],
            "outflows": [],
            "arrayedEquation": {
                "dimensions": ["Region"],
                "equation": "fallback_value",
                "initialEquation": "preferred_value",
            },
        }
        stock = structure_variable(d)
        assert isinstance(stock, simlin.Stock)
        assert stock.initial_equation == "preferred_value"

    def test_top_level_initial_equation_takes_precedence(self) -> None:
        """Top-level initialEquation should be used when present."""
        from simlin.json_converter import structure_variable

        d: dict[str, Any] = {
            "type": "stock",
            "name": "scalar_stock",
            "initialEquation": "50",
            "inflows": [],
            "outflows": [],
        }
        stock = structure_variable(d)
        assert isinstance(stock, simlin.Stock)
        assert stock.initial_equation == "50"


class TestTimeSpecDirect:
    """Test the time_spec property using the direct FFI call."""

    def test_time_spec_values(self, teacup_stmx_path: Path) -> None:
        """time_spec should return correct start, stop, dt, and units."""
        from simlin.types import TimeSpec

        model = simlin.load(teacup_stmx_path)
        ts = model.time_spec
        assert isinstance(ts, TimeSpec)
        assert ts.start == 0.0
        assert ts.stop == 30.0
        assert ts.dt == 0.125
        assert ts.units is not None

    def test_time_spec_after_edit(self, teacup_stmx_path: Path) -> None:
        """time_spec should reflect changes after editing sim specs."""
        model = simlin.load(teacup_stmx_path)
        model.project.set_sim_specs(stop=50.0)
        ts = model.time_spec
        assert ts.stop == 50.0


class TestVarFromDict:
    """Unit tests for structure_variable type dispatch."""

    def test_module_type_returns_module(self) -> None:
        """Module-type variables are part of the unified public API."""
        from simlin.json_converter import structure_variable
        from simlin.types import Module

        d: dict[str, Any] = {"type": "module", "name": "sub", "modelName": "sub_model"}
        var = structure_variable(d)
        assert var == Module(name="sub", model_name="sub_model")

    def test_unknown_type_raises(self) -> None:
        """Unknown variable types should raise, not silently return None."""
        from simlin.json_converter import structure_variable

        d: dict[str, Any] = {"type": "bogus", "name": "x"}
        with pytest.raises(SimlinRuntimeError, match="unknown variable type"):
            structure_variable(d)
