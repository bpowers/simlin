"""Tests for the Model class."""

import json
from pathlib import Path

import pytest

import simlin
from simlin import Model, SimlinCompilationError, SimlinRuntimeError
from simlin.json_types import Auxiliary as JsonAuxiliary


@pytest.fixture
def test_model(xmile_model_path) -> Model:
    """Create a test model from XMILE file."""
    return simlin.load(xmile_model_path)


class TestModelVariables:
    """Test working with model variables."""

    def test_get_var_count_via_variables(self, test_model: Model) -> None:
        """Test getting the number of variables via variables property."""
        count = len(test_model.variables)
        assert isinstance(count, int)
        assert count > 0

    def test_get_var_names_via_variables(self, test_model: Model) -> None:
        """Test getting variable names via variables property."""
        names = [v.name for v in test_model.variables]
        assert isinstance(names, list)
        assert len(names) > 0
        for name in names:
            assert isinstance(name, str)
            assert len(name) > 0

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

    def test_get_incoming_links_empty(self, test_model: Model) -> None:
        """Test that some variables might have no dependencies."""
        var_names = [v.name for v in test_model.variables]

        # Find a constant or time variable that has no deps
        for name in var_names:
            deps = test_model.get_incoming_links(name)
            if len(deps) == 0:
                break

        # Most models have at least one variable with no dependencies
        # (like constants or time)


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

    def test_new_sim_default(self, test_model: Model) -> None:
        """Test creating a simulation with default settings."""
        sim = test_model.simulate()
        assert sim is not None
        from simlin import Sim

        assert isinstance(sim, Sim)

    def test_new_sim_with_ltm(self, test_model: Model) -> None:
        """Test creating a simulation with LTM enabled."""
        sim = test_model.simulate(enable_ltm=True)
        assert sim is not None
        from simlin import Sim

        assert isinstance(sim, Sim)

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
            heat_loss = current["heat_loss_to_room"]
            heat_loss.equation = "0"
            patch.upsert_flow(heat_loss)

        # Verify via JSON serialization
        project_json = json.loads(model.project.serialize_json().decode("utf-8"))
        flow_dict = next(
            f for f in project_json["models"][0]["flows"] if f["name"] == "heat_loss_to_room"
        )
        assert flow_dict.get("equation", "") == "0"

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
            flow = current["heat_loss_to_room"]
            flow.equation = "0"
            patch.upsert_flow(flow)

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
            pytest.raises((SimlinRuntimeError, SimlinCompilationError)),
            model.edit() as (_, patch),
        ):
            patch.upsert_aux(
                JsonAuxiliary(
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
            bad_aux = JsonAuxiliary(
                name="bad_variable",
                equation="?? invalid expression",
            )
            patch.upsert_aux(bad_aux)

        # The variable should be added despite the error
        project_json = json.loads(model.project.serialize_json().decode("utf-8"))
        aux_names = [a["name"] for a in project_json["models"][0].get("auxiliaries", [])]
        assert "bad_variable" in aux_names

    def test_edit_context_dry_run_with_invalid_raises(self, xmile_model_path) -> None:
        """dry_run=True should still raise on invalid patches when allow_errors=False."""
        model = simlin.load(xmile_model_path)

        before_json = model.project.serialize_json()

        with (
            pytest.raises((SimlinRuntimeError, SimlinCompilationError)),
            model.edit(dry_run=True) as (_, patch),
        ):
            patch.upsert_aux(
                JsonAuxiliary(
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
            bad_aux = JsonAuxiliary(
                name="bad_variable",
                equation="?? invalid expression",
            )
            patch.upsert_aux(bad_aux)

        # Project should be unchanged
        after_json = model.project.serialize_json()
        assert after_json == before_json

    def test_apply_patch_json_invalid_json_raises(self, xmile_model_path) -> None:
        """Malformed JSON should raise an error."""
        model = simlin.load(xmile_model_path)

        with pytest.raises((SimlinRuntimeError, SimlinCompilationError)):
            model.project._apply_patch_json(b"{ not valid json }")

    def test_apply_patch_json_returns_errors_when_allowed(self, xmile_model_path) -> None:
        """apply_patch_json with allow_errors=True should return error details."""
        import json as json_module

        from simlin.errors import ErrorCode, ErrorDetail
        from simlin.json_converter import converter
        from simlin.json_types import Auxiliary, JsonModelPatch, JsonProjectPatch, UpsertAux

        model = simlin.load(xmile_model_path)

        # Create a patch with an invalid equation (??? is not valid syntax)
        bad_aux = Auxiliary(name="broken_var", equation="??? totally invalid")
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


class TestModelRepr:
    """Test string representation of models."""

    def test_repr(self, test_model: Model) -> None:
        """Test __repr__ method."""
        repr_str = repr(test_model)
        assert "Model" in repr_str
        assert "variable" in repr_str.lower()


class TestModelStructuralProperties:
    """Test the new structural properties of Model."""

    def test_stocks_property(self, test_model: Model) -> None:
        """Test that stocks property returns tuple of Stock objects."""
        stocks = test_model.stocks
        assert isinstance(stocks, tuple)

        for stock in stocks:
            from simlin.types import Stock

            assert isinstance(stock, Stock)
            assert isinstance(stock.name, str)
            assert isinstance(stock.initial_equation, str)
            assert isinstance(stock.inflows, tuple)
            assert isinstance(stock.outflows, tuple)

    def test_flows_property(self, test_model: Model) -> None:
        """Test that flows property returns tuple of Flow objects."""
        flows = test_model.flows
        assert isinstance(flows, tuple)

        for flow in flows:
            from simlin.types import Flow

            assert isinstance(flow, Flow)
            assert isinstance(flow.name, str)
            assert isinstance(flow.equation, str)

    def test_auxs_property(self, test_model: Model) -> None:
        """Test that auxs property returns tuple of Aux objects."""
        auxs = test_model.auxs
        assert isinstance(auxs, tuple)

        for aux in auxs:
            from simlin.types import Aux

            assert isinstance(aux, Aux)
            assert isinstance(aux.name, str)
            assert isinstance(aux.equation, str)

    def test_variables_property(self, test_model: Model) -> None:
        """Test that variables property combines stocks, flows, and auxs."""
        variables = test_model.variables
        assert isinstance(variables, tuple)

        stocks_count = len(test_model.stocks)
        flows_count = len(test_model.flows)
        auxs_count = len(test_model.auxs)

        assert len(variables) == stocks_count + flows_count + auxs_count

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

    def test_structural_properties_consistent(self, test_model: Model) -> None:
        """Test that structural properties return equal results across calls."""
        stocks1 = test_model.stocks
        stocks2 = test_model.stocks
        assert stocks1 == stocks2

        flows1 = test_model.flows
        flows2 = test_model.flows
        assert flows1 == flows2

        time_spec1 = test_model.time_spec
        time_spec2 = test_model.time_spec
        assert time_spec1 == time_spec2


class TestModelSimulationMethods:
    """Test the new simulation methods of Model."""

    def test_simulate_method(self, test_model: Model) -> None:
        """Test simulate() method returns Sim."""
        from simlin import Sim

        sim = test_model.simulate()
        assert isinstance(sim, Sim)

    def test_simulate_with_overrides(self, teacup_stmx_path) -> None:
        """Test simulate() with variable overrides."""
        from simlin import Sim

        model = simlin.load(teacup_stmx_path)

        # room_temperature is a simple constant (equation = "70")
        overrides = {"room_temperature": 42.0}
        sim = model.simulate(overrides=overrides)
        assert isinstance(sim, Sim)

    def test_simulate_with_ltm(self, test_model: Model) -> None:
        """Test simulate() with LTM enabled."""
        from simlin import Sim

        sim = test_model.simulate(enable_ltm=True)
        assert isinstance(sim, Sim)

    def test_run_method(self, test_model: Model) -> None:
        """Test run() method returns Run."""
        from simlin.run import Run

        run = test_model.run(analyze_loops=False)
        assert isinstance(run, Run)

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
        """Test run() with loop analysis."""
        from simlin.run import Run

        run = test_model.run(analyze_loops=True)
        assert isinstance(run, Run)

    def test_base_case_property(self, test_model: Model) -> None:
        """Test base_case property returns Run."""
        from simlin.run import Run

        base_case = test_model.base_case
        assert isinstance(base_case, Run)

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

    def test_check_method_returns_tuple(self, test_model: Model) -> None:
        """Test that check() returns a tuple."""
        issues = test_model.check()
        assert isinstance(issues, tuple)

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
        stocks = test_model.stocks
        if not stocks:
            pytest.skip("No stocks in test model")

        stock_name = stocks[0].name
        explanation = test_model.explain(stock_name)
        assert isinstance(explanation, str)
        assert stock_name in explanation
        assert "stock" in explanation

    def test_explain_flow(self, test_model: Model) -> None:
        """Test explain() for a flow variable."""
        flows = test_model.flows
        if not flows:
            pytest.skip("No flows in test model")

        flow_name = flows[0].name
        explanation = test_model.explain(flow_name)
        assert isinstance(explanation, str)
        assert flow_name in explanation
        assert "flow" in explanation

    def test_explain_aux(self, test_model: Model) -> None:
        """Test explain() for an auxiliary variable."""
        auxs = test_model.auxs
        if not auxs:
            pytest.skip("No auxiliary variables in test model")

        aux_name = auxs[0].name
        explanation = test_model.explain(aux_name)
        assert isinstance(explanation, str)
        assert aux_name in explanation
        assert "auxiliary" in explanation

    def test_explain_nonexistent_raises(self, test_model: Model) -> None:
        """Test explain() raises error for nonexistent variable."""
        with pytest.raises(SimlinRuntimeError) as exc_info:
            test_model.explain("nonexistent_variable_xyz")

        assert "not found" in str(exc_info.value).lower()
        assert "nonexistent_variable_xyz" in str(exc_info.value)

    def test_explain_includes_initial_equation_for_stocks(self, test_model: Model) -> None:
        """Test that stock explanation includes initial value."""
        stocks = test_model.stocks
        if not stocks:
            pytest.skip("No stocks in test model")

        stock_name = stocks[0].name
        explanation = test_model.explain(stock_name)
        assert "initial value" in explanation

    def test_explain_includes_equation_for_flows(self, test_model: Model) -> None:
        """Test that flow explanation includes equation."""
        flows = test_model.flows
        if not flows:
            pytest.skip("No flows in test model")

        flow_name = flows[0].name
        explanation = test_model.explain(flow_name)
        assert "computed as" in explanation


class TestArrayedEquations:
    """Test extraction of arrayed (subscripted) variable equations."""

    def test_flow_with_apply_to_all_equation(self, subscripted_model_path: Path) -> None:
        """Arrayed flows using apply-to-all equations should expose the actual equation.

        For arrayed variables, XMILE stores equations in different places depending
        on how they're defined. For "apply-to-all" equations (same formula for all
        subscript elements), the equation is stored in arrayed_equation.equation
        rather than the top-level equation field.
        """
        model = simlin.load(subscripted_model_path)

        # Find the arrayed flows
        flows_by_name = {f.name: f for f in model.flows}

        # These flows have apply-to-all equations in the test model
        assert "Inflow A" in flows_by_name or "inflow_a" in flows_by_name
        inflow_a = flows_by_name.get("Inflow A") or flows_by_name.get("inflow_a")
        assert inflow_a is not None

        # The equation should be non-empty (extracted from arrayed_equation)
        assert inflow_a.equation, "Arrayed flow equation should not be empty"
        assert "Rate_A" in inflow_a.equation or "rate_a" in inflow_a.equation.lower()

    def test_stock_with_apply_to_all_initial(self, subscripted_model_path: Path) -> None:
        """Arrayed stocks should expose their initial equation."""
        model = simlin.load(subscripted_model_path)

        stocks_by_name = {s.name: s for s in model.stocks}

        assert "Stock A" in stocks_by_name or "stock_a" in stocks_by_name
        stock_a = stocks_by_name.get("Stock A") or stocks_by_name.get("stock_a")
        assert stock_a is not None

        # The initial equation should be extracted (it's "0" in this model)
        assert stock_a.initial_equation == "0"


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

    def test_get_variable_matches_stocks_property(self, teacup_stmx_path: Path) -> None:
        """get_variable should return data consistent with the stocks property."""
        model = simlin.load(teacup_stmx_path)
        for stock in model.stocks:
            var = model.get_variable(stock.name)
            assert var is not None
            assert var == stock

    def test_get_variable_matches_flows_property(self, teacup_stmx_path: Path) -> None:
        """get_variable should return data consistent with the flows property."""
        model = simlin.load(teacup_stmx_path)
        for flow in model.flows:
            var = model.get_variable(flow.name)
            assert var is not None
            assert var == flow

    def test_get_variable_matches_auxs_property(self, teacup_stmx_path: Path) -> None:
        """get_variable should return data consistent with the auxs property."""
        model = simlin.load(teacup_stmx_path)
        for aux in model.auxs:
            var = model.get_variable(aux.name)
            assert var is not None
            assert var == aux


class TestStocksOnlyReturnsStocks:
    """Verify the stocks property only returns stock-type variables."""

    def test_stocks_are_all_stock_type(self, teacup_stmx_path: Path) -> None:
        """Every element of stocks should be a Stock instance."""
        from simlin.types import Stock

        model = simlin.load(teacup_stmx_path)
        for s in model.stocks:
            assert isinstance(s, Stock), f"Expected Stock, got {type(s).__name__}"

    def test_no_flows_in_stocks(self, teacup_stmx_path: Path) -> None:
        """The stocks property should not include flows."""
        model = simlin.load(teacup_stmx_path)
        stock_names = {s.name for s in model.stocks}
        flow_names = {f.name for f in model.flows}
        assert stock_names.isdisjoint(flow_names)

    def test_no_auxs_in_stocks(self, teacup_stmx_path: Path) -> None:
        """The stocks property should not include auxiliary variables."""
        model = simlin.load(teacup_stmx_path)
        stock_names = {s.name for s in model.stocks}
        aux_names = {a.name for a in model.auxs}
        assert stock_names.isdisjoint(aux_names)


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
