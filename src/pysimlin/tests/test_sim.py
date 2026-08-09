"""Tests for the Sim class."""

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

    def test_get_value_returns_final_value(self, test_sim: Sim) -> None:
        """get_value reports the variable's value at the end of the run.

        eval_order.stmx is a single aux ``auxiliary = 4 - 5 + 6`` simulated
        over t=0..1 with dt=1, so the expected values are exact.
        """
        test_sim.run_to_end()

        assert test_sim.get_value("auxiliary") == 5.0
        assert test_sim.get_value("time") == 1.0

    def test_get_value_nonexistent_raises(self, test_sim: Sim) -> None:
        """Test that getting nonexistent variable raises error."""
        test_sim.run_to_end()
        with pytest.raises(SimlinRuntimeError):
            test_sim.get_value("nonexistent_variable_xyz_123")

    def test_get_series_returns_one_value_per_step(self, test_sim: Sim) -> None:
        """get_series returns the whole run, one entry per saved step."""
        test_sim.run_to_end()

        series = test_sim.get_series("auxiliary")
        assert isinstance(series, np.ndarray)
        assert series.shape == (test_sim.get_step_count(),)
        np.testing.assert_array_equal(series, np.full(len(series), 5.0))
        np.testing.assert_array_equal(test_sim.get_series("time"), np.array([0.0, 1.0]))

    def test_set_value_before_run_sets_the_initial_value(self, teacup_stmx_path) -> None:
        """A pre-run set_value overrides a constant for the whole run.

        ``set_value`` is documented as setting the initial value when called
        before the first ``run_to``; teacup's ``room_temperature`` is a simple
        constant (equation "70"), which is what the engine accepts here -- a
        stock or computed variable is rejected as "not a simple constant".
        The override must both stick in the constant's own series and change
        the trajectory that depends on it.
        """
        model = simlin.load(teacup_stmx_path)

        sim = model.simulate()
        sim.set_value("room_temperature", 42.0)
        sim.run_to_end()

        baseline = model.simulate()
        baseline.run_to_end()

        overridden_room = sim.get_series("room_temperature")
        np.testing.assert_array_equal(overridden_room, np.full(len(overridden_room), 42.0))
        assert sim.get_series("teacup_temperature")[-1] != pytest.approx(
            baseline.get_series("teacup_temperature")[-1]
        )

    def test_set_value_nonexistent_raises(self, test_sim: Sim) -> None:
        """Setting an unknown variable is rejected rather than ignored."""
        with pytest.raises(SimlinRuntimeError):
            test_sim.set_value("nonexistent_variable_xyz_123", 42.0)

    def test_get_series_nonexistent_raises(self, test_sim: Sim) -> None:
        """Test that getting series for nonexistent variable raises error."""
        test_sim.run_to_end()
        with pytest.raises(SimlinRuntimeError):
            test_sim.get_series("nonexistent_variable_xyz_123")


class TestSimDataFrame:
    """Test DataFrame functionality."""

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


class TestGetRunLifetime:
    """A Run returned by get_run() must remain usable after its Sim closes.

    get_run() is documented as returning "simulation results as a Run
    object"; the natural usage is `with model.simulate() as sim: ...;
    run = sim.get_run()` followed by analysis of `run` outside the with
    block. That requires get_run() to eagerly snapshot every surface the
    Run exposes rather than lazily reading from the (soon closed) Sim.
    """

    def test_run_usable_after_sim_closed(self, xmile_model_path) -> None:
        """Every Run surface works after the simulate() context exits."""
        model = simlin.load(xmile_model_path)
        with model.simulate() as sim:
            sim.run_to_end()
            step_count = sim.get_step_count()
            run = sim.get_run()

        df = run.results
        assert isinstance(df, pd.DataFrame)
        assert len(df) == step_count
        assert isinstance(run.loops, tuple)
        assert isinstance(run.dominant_periods, tuple)
        assert run.ltm_mode == "disabled"
        assert run.time_spec.stop > run.time_spec.start
        assert run.overrides == {}

    def test_ltm_run_usable_after_sim_closed(self, xmile_model_path) -> None:
        """LTM surfaces (loops, ltm_mode) also survive Sim closure."""
        model = simlin.load(xmile_model_path)
        with model.simulate(enable_ltm=True) as sim:
            sim.run_to_end()
            run = sim.get_run()

        assert len(run.results) > 0
        assert run.ltm_mode in ("exhaustive", "discovery")
        assert isinstance(run.loops, tuple)
        assert isinstance(run.dominant_periods, tuple)

    def test_overrides_isolated_from_caller_mutation(self) -> None:
        """Mutating the caller's overrides dict must not alter a Run's record.

        Sim and Run must copy the overrides mapping at construction; aliasing
        the caller-owned dict would let post-run mutation rewrite what a saved
        Run reports it was simulated with.
        """
        from pathlib import Path

        fixture = Path(__file__).parent / "logistic-growth.sd.json"
        model = simlin.load(fixture)

        overrides = {"maximum_growth_rate": 0.12}
        with model.simulate(overrides=overrides) as sim:
            sim.run_to_end()
            run = sim.get_run()
        overrides["maximum_growth_rate"] = 999.0
        assert run.overrides == {"maximum_growth_rate": 0.12}

        overrides = {"maximum_growth_rate": 0.12}
        run = model.run(overrides=overrides, analyze_loops=False)
        overrides["maximum_growth_rate"] = 999.0
        assert run.overrides == {"maximum_growth_rate": 0.12}
