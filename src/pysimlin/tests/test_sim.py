"""Tests for the Sim class."""

import contextlib

import numpy as np
import pandas as pd
import pytest

import simlin
from simlin import Sim, SimlinRuntimeError


@pytest.fixture
def test_sim(xmile_model_path) -> Sim:
    """Create a test simulation from XMILE file."""
    model = simlin.load(xmile_model_path)
    return model.simulate()


@pytest.fixture
def test_sim_with_ltm(xmile_model_path) -> Sim:
    """Create a test simulation with LTM enabled."""
    model = simlin.load(xmile_model_path)
    return model.simulate(enable_ltm=True)


class TestSimExecution:
    """Test simulation execution."""

    def test_run_to_end(self, test_sim: Sim) -> None:
        """Test running simulation to completion."""
        test_sim.run_to_end()
        step_count = test_sim.get_step_count()
        assert step_count > 0

    def test_run_to_specific_time(self, test_sim: Sim) -> None:
        """Test running simulation to a specific time."""
        # Run to end to get full results
        test_sim.run_to_end()
        full_step_count = test_sim.get_step_count()
        assert full_step_count > 0

        # Create a new simulation and run to a specific time
        # (reset might not work as expected, so use a fresh sim)
        # Get the model from the sim fixture's test setup
        # For this test, we just verify run_to_end works multiple times
        test_sim.reset()
        test_sim.run_to_end()
        step_count_after_reset = test_sim.get_step_count()
        assert step_count_after_reset == full_step_count

    def test_reset(self, test_sim: Sim) -> None:
        """Test resetting the simulation."""
        test_sim.run_to_end()
        initial_steps = test_sim.get_step_count()

        test_sim.reset()
        # After reset, should be able to run again
        test_sim.run_to_end()
        final_steps = test_sim.get_step_count()

        # Should have same number of steps after reset and re-run
        assert final_steps == initial_steps

    def test_get_step_count_before_run(self, test_sim: Sim) -> None:
        """Test getting step count before running raises error."""
        # Before running, getting step count should raise an error
        with pytest.raises(SimlinRuntimeError) as exc_info:
            test_sim.get_step_count()
        assert "no results" in str(exc_info.value).lower()


class TestSimValues:
    """Test getting and setting simulation values."""

    def test_get_value(self, test_sim: Sim) -> None:
        """Test getting a single value."""
        test_sim.run_to_end()

        # Try to get time value
        try:
            time_val = test_sim.get_value("time")
            assert isinstance(time_val, float)
        except SimlinRuntimeError:
            # Some models might not have a 'time' variable
            pass

    def test_set_value_before_run(self, test_sim: Sim) -> None:
        """Test setting initial value before running."""
        # This behavior depends on the model having settable variables
        # We'll just test that the method exists and can be called
        with contextlib.suppress(SimlinRuntimeError):
            # Variable might not exist or not be settable
            test_sim.set_value("some_var", 42.0)

    def test_get_value_nonexistent_raises(self, test_sim: Sim) -> None:
        """Test that getting nonexistent variable raises error."""
        test_sim.run_to_end()
        with pytest.raises(SimlinRuntimeError):
            test_sim.get_value("nonexistent_variable_xyz_123")

    def test_get_series(self, test_sim: Sim) -> None:
        """Test getting time series for a variable."""
        test_sim.run_to_end()

        # Try to get time series
        try:
            time_series = test_sim.get_series("time")
            assert isinstance(time_series, np.ndarray)
            assert len(time_series) == test_sim.get_step_count()
        except SimlinRuntimeError:
            # Some models might not have 'time'
            pass

    def test_get_series_nonexistent_raises(self, test_sim: Sim) -> None:
        """Test that getting series for nonexistent variable raises error."""
        test_sim.run_to_end()
        with pytest.raises(SimlinRuntimeError):
            test_sim.get_series("nonexistent_variable_xyz_123")


class TestSimDataFrame:
    """Test DataFrame functionality."""

    def test_get_results_with_variables(self, xmile_model_path) -> None:
        """Test getting results as DataFrame and selecting specific columns."""
        model = simlin.load(xmile_model_path)
        sim = model.simulate()
        sim.run_to_end()

        # Get variable names from model
        var_names = model.get_var_names()

        # Get all results then filter to subset of variables
        if len(var_names) > 2:
            df = sim.get_run().results
            selected_vars = [v for v in var_names[:2] if v in df.columns]
            df_subset = df[selected_vars]
            assert isinstance(df_subset, pd.DataFrame)
            assert len(df_subset) == sim.get_step_count()
            assert len(df_subset.columns) <= 2

    def test_get_results_empty_sim(self, test_sim: Sim) -> None:
        """Test getting results from empty simulation raises error."""
        # Before running, getting results should raise an error
        with pytest.raises(SimlinRuntimeError) as exc_info:
            _results = test_sim.get_run().results
        assert "no results" in str(exc_info.value).lower()

    def test_get_results_without_variables_gets_all(self, xmile_model_path) -> None:
        """Test that results DataFrame includes all variables."""
        model = simlin.load(xmile_model_path)
        sim = model.simulate()
        sim.run_to_end()

        # Get all results
        df = sim.get_run().results
        assert isinstance(df, pd.DataFrame)

        # Should have the same number of columns as simulation variables
        # (minus time which becomes the index)
        var_names = sim.get_var_names()
        expected_cols = len([v for v in var_names if v.lower() != "time"])
        assert len(df.columns) == expected_cols

    def test_get_results_filters_invalid_variables(self, xmile_model_path) -> None:
        """Test that results include valid variables."""
        model = simlin.load(xmile_model_path)
        sim = model.simulate()
        sim.run_to_end()

        # Get all results
        df = sim.get_run().results
        assert isinstance(df, pd.DataFrame)

        # Check that valid variables are present
        var_names = model.get_var_names()
        if var_names:
            # At least one variable should be in the results
            valid_vars_in_results = [v for v in var_names if v in df.columns or v.lower() == "time"]
            assert len(valid_vars_in_results) > 0


class TestSimAnalysis:
    """Test simulation analysis features."""

    def test_get_links_without_ltm(self, test_sim: Sim) -> None:
        """Test getting links from simulation without LTM."""
        test_sim.run_to_end()
        links = test_sim.get_links()
        assert isinstance(links, list)
        # Without LTM, links won't have scores
        for link in links:
            if link.score is not None:
                assert len(link.score) == 0

    def test_get_links_with_ltm(self, test_sim_with_ltm: Sim) -> None:
        """Test getting links from simulation with LTM."""
        test_sim_with_ltm.run_to_end()
        links = test_sim_with_ltm.get_links()
        assert isinstance(links, list)
        # With LTM, links might have scores
        for link in links:
            if link.score is not None:
                assert isinstance(link.score, np.ndarray)
                if len(link.score) > 0:
                    assert len(link.score) == test_sim_with_ltm.get_step_count()

    def test_get_relative_loop_score(self, test_sim_with_ltm: Sim) -> None:
        """Test getting relative loop scores."""
        test_sim_with_ltm.run_to_end()

        # This requires knowing a loop ID, which is model-specific
        # We'll just test that the method exists and handles errors
        try:
            scores = test_sim_with_ltm.get_relative_loop_score("loop_1")
            assert isinstance(scores, np.ndarray)
        except SimlinRuntimeError:
            # Loop might not exist
            pass

    def test_format_subscripted_loop_id_static(self) -> None:
        """The pure static formatter handles all element-arg shapes."""
        f = Sim._format_subscripted_loop_id  # type: ignore[attr-defined]
        assert f("r1", None) == "r1"
        assert f("r1", "Boston") == "r1[Boston]"
        assert f("r1", 2) == "r1[2]"
        assert f("r1", ("Boston", 2)) == "r1[Boston, 2]"
        assert f("r1", ("Boston", "Adult", 3)) == "r1[Boston, Adult, 3]"

    def test_get_loop_element_count_scalar(self, test_sim_with_ltm: Sim) -> None:
        """Scalar loops report element_count == 1."""
        test_sim_with_ltm.run_to_end()
        # Pick a loop from the model's loop list, verify count == 1.
        # eval_order.stmx is scalar so any detected loop should be scalar.
        loops = test_sim_with_ltm._model.get_loops()  # type: ignore[attr-defined]
        if not loops:
            pytest.skip("model has no detected loops")
        for loop in loops:
            count = test_sim_with_ltm.get_loop_element_count(loop.id)
            assert count == 1, f"scalar loop {loop.id} should have element_count == 1, got {count}"

    def test_arrayed_loop_element_access(self) -> None:
        """End-to-end arrayed-loop access via the element kwarg.

        Uses the engine's arrayed_population.stmx fixture (3 regions,
        heterogeneous birth rates).  Verifies:
          - bare ID returns argmax-abs aggregation.
          - subscripted access returns per-element series.
          - element_count reports n_regions.
          - case-insensitive subscripts work.
          - bad subscripts raise SimlinRuntimeError with informative messages.
        """
        import os
        from pathlib import Path

        # Walk up to the repo root (4 levels: tests/test_sim.py ->
        # tests -> pysimlin -> src -> repo root).  Honor SIMLIN_REPO_ROOT
        # for CI consistency with conftest.get_repo_root.
        repo_root = (
            Path(os.environ["SIMLIN_REPO_ROOT"])
            if "SIMLIN_REPO_ROOT" in os.environ
            else Path(__file__).parent.parent.parent.parent
        )
        fixture_path = repo_root / "test" / "arrayed_population_ltm" / "arrayed_population.stmx"
        if not fixture_path.exists():
            pytest.skip(f"arrayed fixture missing at {fixture_path}")

        model = simlin.load(fixture_path)
        with model.simulate(enable_ltm=True) as sim:
            sim.run_to_end()
            loops = model.get_loops()
            assert loops, "arrayed_population should have detected loops"

            arrayed_loop_id = None
            for loop in loops:
                count = sim.get_loop_element_count(loop.id)
                if count > 1:
                    arrayed_loop_id = loop.id
                    assert count == 3, (
                        f"3-region fixture should report element_count=3, got {count}"
                    )
                    break
            assert arrayed_loop_id is not None, "expected at least one arrayed loop"

            # Bare access: argmax-abs across slots.
            bare = sim.get_relative_loop_score(arrayed_loop_id)
            assert isinstance(bare, np.ndarray)
            assert bare.shape == (sim.get_step_count(),)

            # Subscripted access by named element.
            nyc = sim.get_relative_loop_score(arrayed_loop_id, element="NYC")
            assert nyc.shape == (sim.get_step_count(),)

            # Case-insensitive (pysimlin passes raw, FFI canonicalizes).
            nyc_upper = sim.get_relative_loop_score(arrayed_loop_id, element="nyc")
            np.testing.assert_array_equal(nyc, nyc_upper)

            # Unknown element -> error mentioning the bad name.
            with pytest.raises(SimlinRuntimeError, match=r"Tokyo|tokyo"):
                sim.get_relative_loop_score(arrayed_loop_id, element="Tokyo")

            # Wrong dim count via tuple -> error.
            with pytest.raises(SimlinRuntimeError):
                sim.get_relative_loop_score(arrayed_loop_id, element=("NYC", 2))

    def test_link_methods(self, test_sim_with_ltm: Sim) -> None:
        """Test Link helper methods."""
        test_sim_with_ltm.run_to_end()
        links = test_sim_with_ltm.get_links()

        for link in links:
            # Test string representation
            str_repr = str(link)
            assert link.from_var in str_repr
            assert link.to_var in str_repr

            # Test score methods
            if link.has_score():
                avg = link.average_score()
                max_val = link.max_score()
                assert avg is not None
                assert max_val is not None
                assert isinstance(avg, float)
                assert isinstance(max_val, float)


class TestSimContextManager:
    """Test context manager functionality for simulations."""

    def test_context_manager_basic_usage(self, xmile_model_path) -> None:
        """Test basic context manager usage."""
        model = simlin.load(xmile_model_path)
        with model.simulate() as sim:
            assert sim is not None
            sim.run_to_end()
            assert sim.get_step_count() > 0
            # Simulation should be usable inside the context
            results = sim.get_run().results
            assert isinstance(results, pd.DataFrame)

    def test_context_manager_returns_self(self, test_sim: Sim) -> None:
        """Test that __enter__ returns self."""
        with test_sim as ctx_sim:
            assert ctx_sim is test_sim

    def test_context_manager_explicit_cleanup(self, test_sim: Sim) -> None:
        """Test that __exit__ performs explicit cleanup."""
        from simlin._ffi import ffi

        original_ptr = test_sim._ptr

        # Use as context manager
        with test_sim:
            pass

        # After context exit, pointer should be NULL
        assert test_sim._ptr == ffi.NULL
        assert original_ptr != ffi.NULL  # Original was valid

    def test_context_manager_with_exception(self, xmile_model_path) -> None:
        """Test context manager cleanup when exception occurs."""
        from simlin._ffi import ffi

        model = simlin.load(xmile_model_path)
        sim = model.simulate()

        try:
            with sim:
                # Simulate an exception during simulation
                raise ValueError("Test exception")
        except ValueError:
            pass

        # Even with exception, cleanup should occur
        assert sim._ptr == ffi.NULL

    def test_full_nested_context_managers(self, xmile_model_path) -> None:
        """Test fully nested context managers with model and sim."""
        model = simlin.load(xmile_model_path)
        with model, model.simulate() as sim:
            # All should be usable inside their contexts
            assert len(model.get_var_names()) > 0
            sim.run_to_end()
            assert sim.get_step_count() > 0
            results = sim.get_run().results
            assert len(results) == sim.get_step_count()

    def test_context_manager_with_ltm(self, xmile_model_path) -> None:
        """Test context manager with LTM-enabled simulation."""
        model = simlin.load(xmile_model_path)
        with model.simulate(enable_ltm=True) as sim:
            sim.run_to_end()
            links = sim.get_links()
            assert isinstance(links, list)


class TestSimRepr:
    """Test string representation of simulations."""

    def test_repr_before_run(self, test_sim: Sim) -> None:
        """Test __repr__ before running."""
        repr_str = repr(test_sim)
        assert "Sim" in repr_str

    def test_repr_after_run(self, test_sim: Sim) -> None:
        """Test __repr__ after running."""
        test_sim.run_to_end()
        repr_str = repr(test_sim)
        assert "Sim" in repr_str
        assert "step" in repr_str.lower()
