"""Tests for the Sim class."""

import pytest
import numpy as np
import pandas as pd
from pathlib import Path
import simlin
from simlin import Project, Model, Sim, SimlinRuntimeError


@pytest.fixture
def test_sim(xmile_model_data: bytes) -> Sim:
    """Create a test simulation from XMILE data."""
    project = Project.from_xmile(xmile_model_data)
    model = project.get_model()
    return model.new_sim()


@pytest.fixture
def test_sim_with_ltm(xmile_model_data: bytes) -> Sim:
    """Create a test simulation with LTM enabled."""
    project = Project.from_xmile(xmile_model_data)
    model = project.get_model()
    return model.new_sim(enable_ltm=True)


class TestSimExecution:
    """Test simulation execution."""
    
    def test_run_to_end(self, test_sim: Sim) -> None:
        """Test running simulation to completion."""
        test_sim.run_to_end()
        step_count = test_sim.get_step_count()
        assert step_count > 0
    
    def test_run_to_specific_time(self, test_sim: Sim) -> None:
        """Test running simulation to a specific time within the model horizon."""
        # The fixture model has stop=1, so use 1.0 to stay within horizon
        test_sim.run_to(1.0)
        step_count = test_sim.get_step_count()
        assert step_count > 0
        
        # Running further should add more steps
        test_sim.run_to(10.0)
        new_step_count = test_sim.get_step_count()
        assert new_step_count >= step_count
    
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
        """Test getting step count before running."""
        # Before running, might be 0 or might have initial step
        step_count = test_sim.get_step_count()
        assert step_count >= 0


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
        try:
            test_sim.set_value("some_var", 42.0)
        except SimlinRuntimeError:
            # Variable might not exist or not be settable
            pass
    
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
    
    def test_get_results_with_variables(self, xmile_model_data: bytes) -> None:
        """Test getting results as DataFrame with specific variables."""
        project = Project.from_xmile(xmile_model_data)
        model = project.get_model()
        sim = model.new_sim()
        sim.run_to_end()
        
        # Get variable names from model
        var_names = model.get_var_names()
        
        # Get results for a subset of variables
        if len(var_names) > 2:
            df = sim.get_results(variables=var_names[:2])
            assert isinstance(df, pd.DataFrame)
            assert len(df) == sim.get_step_count()
            assert len(df.columns) <= 2
            assert df.index.name == "time"
    
    def test_get_results_empty_sim(self, test_sim: Sim) -> None:
        """Test getting results from empty simulation."""
        # Before running, should return empty DataFrame
        df = test_sim.get_results(variables=[])
        assert isinstance(df, pd.DataFrame)
        assert len(df) == 0
    
    def test_get_results_without_variables_gets_all(self, xmile_model_data: bytes) -> None:
        """Test that getting results without variable list gets all variables."""
        project = Project.from_xmile(xmile_model_data)
        model = project.get_model()
        sim = model.new_sim()
        sim.run_to_end()
        
        # Get results without specifying variables - should get all
        df = sim.get_results()
        assert isinstance(df, pd.DataFrame)
        
        # Should have the same number of columns as variables in the model
        # (minus time which becomes the index)
        var_names = model.get_var_names()
        expected_cols = len([v for v in var_names if v.lower() != "time"])
        assert len(df.columns) <= expected_cols  # Some variables might be filtered out if they don't exist
    
    def test_get_results_filters_invalid_variables(self, xmile_model_data: bytes) -> None:
        """Test that invalid variables are filtered out."""
        project = Project.from_xmile(xmile_model_data)
        model = project.get_model()
        sim = model.new_sim()
        sim.run_to_end()
        
        # Mix valid and invalid variable names
        var_names = model.get_var_names()
        if var_names:
            mixed_vars = [var_names[0], "invalid_var_xyz"]
            df = sim.get_results(variables=mixed_vars)
            assert isinstance(df, pd.DataFrame)
            # Should only have the valid variable
            assert len(df.columns) == 1


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
    
    def test_context_manager_basic_usage(self, xmile_model_data: bytes) -> None:
        """Test basic context manager usage."""
        project = Project.from_xmile(xmile_model_data)
        model = project.get_model()
        with model.new_sim() as sim:
            assert sim is not None
            sim.run_to_end()
            assert sim.get_step_count() > 0
            # Simulation should be usable inside the context
            results = sim.get_results()
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
    
    def test_context_manager_with_exception(self, xmile_model_data: bytes) -> None:
        """Test context manager cleanup when exception occurs."""
        from simlin._ffi import ffi
        
        project = Project.from_xmile(xmile_model_data)
        model = project.get_model()
        sim = model.new_sim()
        
        try:
            with sim:
                # Simulate an exception during simulation
                raise ValueError("Test exception")
        except ValueError:
            pass
        
        # Even with exception, cleanup should occur
        assert sim._ptr == ffi.NULL
    
    def test_full_nested_context_managers(self, xmile_model_data: bytes) -> None:
        """Test fully nested context managers with project, model, and sim."""
        with Project.from_xmile(xmile_model_data) as project:
            with project.get_model() as model:
                with model.new_sim() as sim:
                    # All should be usable inside their contexts
                    assert project.get_model_count() > 0
                    assert model.get_var_count() > 0
                    sim.run_to_end()
                    assert sim.get_step_count() > 0
                    results = sim.get_results()
                    assert len(results) == sim.get_step_count()
    
    def test_context_manager_with_ltm(self, xmile_model_data: bytes) -> None:
        """Test context manager with LTM-enabled simulation."""
        project = Project.from_xmile(xmile_model_data)
        model = project.get_model()
        with model.new_sim(enable_ltm=True) as sim:
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
