"""Tests for the Model class."""

import pytest
from pathlib import Path
import simlin
from simlin import Project, Model, SimlinRuntimeError, SimlinCompilationError
from simlin import pb


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
        found_empty = False
        for name in var_names:
            deps = test_model.get_incoming_links(name)
            if len(deps) == 0:
                found_empty = True
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
            assert hasattr(link, 'from_var')
            assert hasattr(link, 'to_var')
            assert hasattr(link, 'polarity')
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
            heat_loss = current["Heat Loss to Room"]
            heat_loss.flow.equation.scalar.equation = "0"
            patch.upsert_flow(heat_loss.flow)

        project_proto = pb.Project()
        project_proto.ParseFromString(model.project.serialize())
        model_proto = project_proto.models[0]
        flow_proto = next(
            var.flow
            for var in model_proto.variables
            if var.flow.ident in {"Heat Loss to Room", "heat_loss_to_room"}
        )

        assert flow_proto.equation.scalar.equation == "0"

    def test_edit_context_dry_run_does_not_commit(self, mdl_model_path) -> None:
        """dry_run=True should validate without mutating the project."""
        model = simlin.load(mdl_model_path)

        original = pb.Project()
        original.ParseFromString(model.project.serialize())
        original_flow = next(
            var.flow
            for var in original.models[0].variables
            if var.flow.ident in {"Heat Loss to Room", "heat_loss_to_room"}
        )

        with model.edit(dry_run=True, allow_errors=True) as (current, patch):
            flow = current["Heat Loss to Room"]
            flow.flow.equation.scalar.equation = "0"
            patch.upsert_flow(flow.flow)

        project_proto = pb.Project()
        project_proto.ParseFromString(model.project.serialize())
        flow_proto = next(
            var.flow
            for var in project_proto.models[0].variables
            if var.flow.ident in {"Heat Loss to Room", "heat_loss_to_room"}
        )

        assert flow_proto.equation.scalar.equation == original_flow.equation.scalar.equation

    def test_edit_context_invalid_patch_raises(self, xmile_model_path) -> None:
        """Invalid edits should raise and leave the project unchanged."""
        model = simlin.load(xmile_model_path)

        before = pb.Project()
        before.ParseFromString(model.project.serialize())

        with pytest.raises((SimlinRuntimeError, SimlinCompilationError)):
            with model.edit() as (_, patch):
                bad_aux = pb.Variable.Aux()
                bad_aux.ident = "bad_variable"
                bad_aux.equation.scalar.equation = "?? invalid expression"
                patch.upsert_aux(bad_aux)

        after = pb.Project()
        after.ParseFromString(model.project.serialize())
        assert after == before


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

    def test_structural_properties_cached(self, test_model: Model) -> None:
        """Test that structural properties are cached."""
        stocks1 = test_model.stocks
        stocks2 = test_model.stocks
        assert stocks1 is stocks2

        flows1 = test_model.flows
        flows2 = test_model.flows
        assert flows1 is flows2

        time_spec1 = test_model.time_spec
        time_spec2 = test_model.time_spec
        assert time_spec1 is time_spec2


class TestModelSimulationMethods:
    """Test the new simulation methods of Model."""

    def test_simulate_method(self, test_model: Model) -> None:
        """Test simulate() method returns Sim."""
        from simlin import Sim
        sim = test_model.simulate()
        assert isinstance(sim, Sim)

    def test_simulate_with_overrides(self, test_model: Model) -> None:
        """Test simulate() with variable overrides."""
        from simlin import Sim
        var_names = [v.name for v in test_model.variables]
        if not var_names:
            pytest.skip("No variables in model")

        overrides = {var_names[0]: 42.0}
        sim = test_model.simulate(overrides=overrides)
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

    def test_run_with_overrides(self, test_model: Model) -> None:
        """Test run() with variable overrides."""
        from simlin.run import Run
        var_names = [v.name for v in test_model.variables]
        if not var_names:
            pytest.skip("No variables in model")

        overrides = {var_names[0]: 123.0}
        run = test_model.run(overrides=overrides, analyze_loops=False)
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
            assert hasattr(issue, 'severity')
            assert hasattr(issue, 'message')
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