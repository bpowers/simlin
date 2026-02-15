"""Tests for the top-level pysimlin API."""

from pathlib import Path

import pandas as pd
import pytest

import simlin
from simlin import Model, Project, Run


class TestLoadFunction:
    """Test the top-level load() function."""

    def test_load_returns_model(self, teacup_stmx_path: Path) -> None:
        """Test that load() returns a Model instance."""
        assert teacup_stmx_path.exists(), f"Test file not found: {teacup_stmx_path}"

        model = simlin.load(teacup_stmx_path)
        assert isinstance(model, Model)

    def test_load_with_str_path(self, teacup_stmx_path: Path) -> None:
        """Test that load() accepts string paths."""
        assert teacup_stmx_path.exists(), f"Test file not found: {teacup_stmx_path}"

        model = simlin.load(str(teacup_stmx_path))
        assert isinstance(model, Model)

    def test_load_with_path_object(self, teacup_stmx_path: Path) -> None:
        """Test that load() accepts Path objects."""
        assert teacup_stmx_path.exists(), f"Test file not found: {teacup_stmx_path}"

        model = simlin.load(teacup_stmx_path)
        assert isinstance(model, Model)

    def test_load_model_has_project(self, teacup_stmx_path: Path) -> None:
        """Test that loaded model has project reference."""
        assert teacup_stmx_path.exists(), f"Test file not found: {teacup_stmx_path}"

        model = simlin.load(teacup_stmx_path)
        assert model._project is not None
        assert isinstance(model._project, Project)

    def test_load_model_base_case(self, teacup_stmx_path: Path) -> None:
        """Test that base_case is accessible on loaded model."""
        assert teacup_stmx_path.exists(), f"Test file not found: {teacup_stmx_path}"

        model = simlin.load(teacup_stmx_path)
        run = model.base_case
        assert isinstance(run, Run)
        assert len(run.results) > 0
        assert isinstance(run.results, pd.DataFrame)

    def test_load_model_has_structure(self, teacup_stmx_path: Path) -> None:
        """Test that loaded model has structural properties."""
        assert teacup_stmx_path.exists(), f"Test file not found: {teacup_stmx_path}"

        model = simlin.load(teacup_stmx_path)

        # Should have structural properties
        assert isinstance(model.variables, tuple)
        assert isinstance(model.loops, tuple)

        # Should have at least some variables
        assert len(model.variables) > 0

    def test_load_xmile_format(self, teacup_xmile_path: Path) -> None:
        """Test loading XMILE format."""
        assert teacup_xmile_path.exists(), f"Test file not found: {teacup_xmile_path}"

        model = simlin.load(teacup_xmile_path)
        assert isinstance(model, Model)
        assert len(model.variables) > 0

    def test_load_json_format(self, logistic_growth_json_path: Path) -> None:
        """Test loading JSON format."""
        assert logistic_growth_json_path.exists(), (
            f"Test file not found: {logistic_growth_json_path}"
        )

        model = simlin.load(logistic_growth_json_path)
        assert isinstance(model, Model)
        assert len(model.variables) > 0

    def test_load_nonexistent_file_raises(self) -> None:
        """Test that loading a nonexistent file raises an error."""
        from simlin import SimlinImportError

        with pytest.raises(SimlinImportError, match="not found"):
            simlin.load("/nonexistent/file.stmx")


class TestCompleteWorkflow:
    """Test the complete pysimlin workflow from load to analysis."""

    def test_complete_workflow(self, teacup_stmx_path: Path) -> None:
        """Test the complete pysimlin workflow from load to analysis."""
        assert teacup_stmx_path.exists(), f"Test file not found: {teacup_stmx_path}"

        # Load model
        model = simlin.load(teacup_stmx_path)

        # Access structure
        assert len(model.variables) > 0

        # Check that we have expected variables
        var_names = {v.name for v in model.variables}
        assert len(var_names) > 0

        # Run base case
        base_run = model.base_case
        assert len(base_run.results) > 0
        assert isinstance(base_run.results, pd.DataFrame)

        # Verify results have time index
        assert base_run.results.index.name == "time"

        # Verify base case has no overrides
        assert base_run.overrides == {}

        # Verify time spec
        assert base_run.time_spec.start >= 0
        assert base_run.time_spec.stop > base_run.time_spec.start
        assert base_run.time_spec.dt > 0

    def test_workflow_with_run_overrides(self, teacup_stmx_path: Path) -> None:
        """Test running model with overrides."""
        assert teacup_stmx_path.exists(), f"Test file not found: {teacup_stmx_path}"

        model = simlin.load(teacup_stmx_path)

        # Override room_temperature, which is a simple constant (equation = "70")
        custom_run = model.run(overrides={"room_temperature": 42.0}, analyze_loops=False)

        assert isinstance(custom_run, Run)
        assert len(custom_run.results) > 0
        assert custom_run.overrides == {"room_temperature": 42.0}

    def test_workflow_base_case_vs_custom(self, teacup_stmx_path: Path) -> None:
        """Test comparing base case with custom run."""
        assert teacup_stmx_path.exists(), f"Test file not found: {teacup_stmx_path}"

        model = simlin.load(teacup_stmx_path)

        # Get base case
        base_run = model.base_case

        # Override room_temperature, which is a simple constant (equation = "70")
        custom_run = model.run(overrides={"room_temperature": 99.0}, analyze_loops=False)

        # Both should be Run objects
        assert isinstance(base_run, Run)
        assert isinstance(custom_run, Run)

        # Both should have results
        assert len(base_run.results) > 0
        assert len(custom_run.results) > 0

        # Should have same columns
        assert set(base_run.results.columns) == set(custom_run.results.columns)

        # Base case should have no overrides
        assert base_run.overrides == {}

        # Custom run should have overrides
        assert custom_run.overrides == {"room_temperature": 99.0}

    def test_workflow_multiple_runs(self, teacup_stmx_path: Path) -> None:
        """Test creating multiple runs from the same model."""
        assert teacup_stmx_path.exists(), f"Test file not found: {teacup_stmx_path}"

        model = simlin.load(teacup_stmx_path)

        # Create multiple runs
        run1 = model.run(analyze_loops=False)
        run2 = model.run(analyze_loops=False)
        run3 = model.base_case

        # All should be Run instances
        assert isinstance(run1, Run)
        assert isinstance(run2, Run)
        assert isinstance(run3, Run)

        # All should have results
        assert len(run1.results) > 0
        assert len(run2.results) > 0
        assert len(run3.results) > 0

    def test_workflow_access_loops(self, teacup_stmx_path: Path) -> None:
        """Test accessing loop information."""
        assert teacup_stmx_path.exists(), f"Test file not found: {teacup_stmx_path}"

        model = simlin.load(teacup_stmx_path)

        # Structural loops (no behavior data)
        model_loops = model.loops
        assert isinstance(model_loops, tuple)

        for loop in model_loops:
            from simlin import Loop

            assert isinstance(loop, Loop)
            assert isinstance(loop.id, str)
            assert isinstance(loop.variables, tuple)
            assert loop.behavior_time_series is None

        # Run loops (with behavior data)
        run = model.run(analyze_loops=True)
        run_loops = run.loops
        assert isinstance(run_loops, tuple)

        # If there are loops, they should have behavior data
        for loop in run_loops:
            from simlin import Loop

            assert isinstance(loop, Loop)
            assert loop.behavior_time_series is not None

    def test_workflow_structural_properties_immutable(self, teacup_stmx_path: Path) -> None:
        """Test that structural properties are immutable."""
        assert teacup_stmx_path.exists(), f"Test file not found: {teacup_stmx_path}"

        model = simlin.load(teacup_stmx_path)

        # Variables are frozen dataclasses
        if model.variables:
            var = model.variables[0]
            with pytest.raises(AttributeError):
                var.name = "modified"

    def test_workflow_results_dataframe_properties(self, teacup_stmx_path: Path) -> None:
        """Test properties of the results DataFrame."""
        assert teacup_stmx_path.exists(), f"Test file not found: {teacup_stmx_path}"

        model = simlin.load(teacup_stmx_path)
        run = model.base_case

        # Results should be a DataFrame
        assert isinstance(run.results, pd.DataFrame)

        # Index should be named 'time'
        assert run.results.index.name == "time"

        # Should have at least one column
        assert len(run.results.columns) > 0

        # All columns should be strings (variable names)
        for col in run.results.columns:
            assert isinstance(col, str)

        # Index should be numeric (time values)
        assert pd.api.types.is_numeric_dtype(run.results.index)

        # All values should be numeric
        for col in run.results.columns:
            assert pd.api.types.is_numeric_dtype(run.results[col])


class TestLoadWithDifferentFormats:
    """Test loading models in different formats."""

    def test_load_mdl_format(self, mdl_model_path: Path) -> None:
        """Test loading Vensim MDL format."""
        assert mdl_model_path.exists(), f"Test file not found: {mdl_model_path}"

        model = simlin.load(mdl_model_path)
        assert isinstance(model, Model)
        assert len(model.variables) > 0

        # Should have base_case ready
        base_case = model.base_case
        assert isinstance(base_case, Run)
        assert len(base_case.results) > 0


class TestLoadExportedName:
    """Test that load is properly exported."""

    def test_load_in_all(self) -> None:
        """Test that load is in __all__."""
        assert "load" in simlin.__all__

    def test_load_importable(self) -> None:
        """Test that load can be imported from simlin."""
        from simlin import load

        assert callable(load)

    def test_load_has_docstring(self) -> None:
        """Test that load has a proper docstring."""
        from simlin import load

        assert load.__doc__ is not None
        assert "Load a system dynamics model" in load.__doc__
        assert "Example:" in load.__doc__
