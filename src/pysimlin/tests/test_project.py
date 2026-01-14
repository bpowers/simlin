"""Tests for the Project class."""

import json
import pytest
from pathlib import Path
import simlin
from simlin import Project, SimlinImportError, ErrorCode



class TestProjectModels:
    """Test working with models in a project."""
    
    def test_get_model_count(self, xmile_model_path) -> None:
        """Test getting the number of models through names."""
        model = simlin.load(xmile_model_path)
        project = model.project
        names = project.get_model_names()
        assert len(names) >= 1
        assert isinstance(len(names), int)
    
    def test_get_model_names(self, xmile_model_path) -> None:
        """Test getting model names."""
        model = simlin.load(xmile_model_path)
        project = model.project
        names = project.get_model_names()
        assert isinstance(names, list)
        # Names list has been validated above
        for name in names:
            assert isinstance(name, str)
    
    def test_get_default_model(self, xmile_model_path) -> None:
        """Test getting the default model."""
        model = simlin.load(xmile_model_path)
        project = model.project
        model = project.get_model()
        assert model is not None
        from simlin import Model
        assert isinstance(model, Model)
    
    def test_get_named_model(self, xmile_model_path) -> None:
        """Test getting a model by name."""
        model = simlin.load(xmile_model_path)
        project = model.project
        names = project.get_model_names()
        if names:
            model = project.get_model(names[0])
            assert model is not None

    def test_new_project_creates_blank_model(self) -> None:
        """Project.new() should create a blank project with a single empty model."""
        project = Project.new(name="example")
        assert project.get_model_names() == ["main"]
        assert project.get_errors() == []

        model = project.get_model()
        # A blank model has no user-defined variables (builtin simulation variables
        # like time, dt, etc. are not exposed through the variables property)
        assert len(model.variables) == 0

    def test_get_nonexistent_model_raises(self, xmile_model_path) -> None:
        """Test that getting a nonexistent model raises an error."""
        model = simlin.load(xmile_model_path)
        project = model.project
        with pytest.raises(SimlinImportError, match="not found"):
            project.get_model("nonexistent_model_name_xyz")


class TestProjectAnalysis:
    """Test project analysis functions."""
    
    def test_get_loops(self, xmile_model_path) -> None:
        """Test getting feedback loops."""
        model = simlin.load(xmile_model_path)
        project = model.project
        loops = project.get_loops()
        assert isinstance(loops, list)
        # Not all models have loops
        for loop in loops:
            assert hasattr(loop, 'id')
            assert hasattr(loop, 'variables')
            assert hasattr(loop, 'polarity')
    
    def test_get_errors(self, xmile_model_path) -> None:
        """Test getting project errors."""
        model = simlin.load(xmile_model_path)
        project = model.project
        errors = project.get_errors()
        assert isinstance(errors, list)
        # Valid models might have no errors
        for error in errors:
            assert hasattr(error, 'code')
            assert hasattr(error, 'message')


class TestProjectSerialization:
    """Test project serialization and export."""

    def test_serialize_to_protobuf(self, xmile_model_path) -> None:
        """Test serializing a project to protobuf."""
        model = simlin.load(xmile_model_path)
        project = model.project
        pb_data = project.serialize_protobuf()
        assert isinstance(pb_data, bytes)
        assert len(pb_data) > 0

    def test_serialize_to_json(self, xmile_model_path) -> None:
        """Test serializing a project to JSON."""
        model = simlin.load(xmile_model_path)
        project = model.project
        json_data = project.serialize_json()
        assert isinstance(json_data, bytes)
        assert len(json_data) > 0

        # Verify it's valid JSON
        project_dict = json.loads(json_data.decode("utf-8"))
        assert "name" in project_dict
        assert "sim_specs" in project_dict
        assert "models" in project_dict
        assert len(project_dict["models"]) > 0

    def test_export_to_xmile(self, xmile_model_path) -> None:
        """Test exporting a project to XMILE."""
        model = simlin.load(xmile_model_path)
        project = model.project
        xmile_data = project.to_xmile()
        assert isinstance(xmile_data, bytes)
        assert len(xmile_data) > 0
        assert b"<xmile" in xmile_data or b"<?xml" in xmile_data

class TestProjectContextManager:
    """Test context manager functionality for projects."""
    
    def test_context_manager_basic_usage(self, xmile_model_path) -> None:
        """Test basic context manager usage."""
        model = simlin.load(xmile_model_path)
        project = model.project
        with project:
            assert project is not None
            assert len(project.get_model_names()) > 0
            # Project should be usable inside the context
            model = project.get_model()
            assert model is not None
    
    def test_context_manager_returns_self(self, xmile_model_path) -> None:
        """Test that __enter__ returns self."""
        model = simlin.load(xmile_model_path)
        project = model.project
        with project as ctx_project:
            assert ctx_project is project
    
    def test_context_manager_explicit_cleanup(self, xmile_model_path) -> None:
        """Test that __exit__ performs explicit cleanup."""
        from simlin._ffi import ffi
        
        model = simlin.load(xmile_model_path)
        project = model.project
        original_ptr = project._ptr
        
        # Use as context manager
        with project:
            pass
        
        # After context exit, pointer should be NULL
        assert project._ptr == ffi.NULL
        assert original_ptr != ffi.NULL  # Original was valid
    
    def test_context_manager_with_exception(self, xmile_model_path) -> None:
        """Test context manager cleanup when exception occurs."""
        from simlin._ffi import ffi
        
        model = simlin.load(xmile_model_path)
        project = model.project
        
        try:
            with project:
                # Simulate an exception
                raise ValueError("Test exception")
        except ValueError:
            pass
        
        # Even with exception, cleanup should occur
        assert project._ptr == ffi.NULL
    
    def test_non_context_manager_usage_still_works(self, xmile_model_path) -> None:
        """Test that objects still work without context manager."""
        # Should work exactly as before without using 'with'
        model = simlin.load(xmile_model_path)
        project = model.project
        assert len(project.get_model_names()) > 0
        model = project.get_model()
        assert model is not None
        # Cleanup will still happen through finalizer


class TestProjectEditing:
    """Tests for editing project-level metadata."""

    def test_set_sim_specs_updates_project(self, xmile_model_path) -> None:
        """set_sim_specs should update the serialized simulation specs."""
        model = simlin.load(xmile_model_path)
        project = model.project

        project.set_sim_specs(
            start=0.0,
            stop=42.0,
            dt=0.25,
            save_step=0.5,
            sim_method=1,  # RK4 (non-default, so it will be serialized)
            time_units="Minutes",
        )

        # Verify via JSON
        project_json = json.loads(project.serialize_json().decode("utf-8"))
        sim_specs = project_json["sim_specs"]

        assert sim_specs["start_time"] == pytest.approx(0.0)
        assert sim_specs["end_time"] == pytest.approx(42.0)
        assert sim_specs["dt"] == "0.25"
        assert sim_specs["save_step"] == pytest.approx(0.5)
        assert sim_specs["method"] == "rk4"
        assert sim_specs["time_units"] == "Minutes"

    def test_new_project_creates_valid_json(self) -> None:
        """Project.new() should create a project with valid JSON representation."""
        project = Project.new(name="test", sim_start=5.0, sim_stop=20.0, dt=0.5)

        project_json = json.loads(project.serialize_json().decode("utf-8"))

        assert project_json["name"] == "test"
        assert project_json["sim_specs"]["start_time"] == 5.0
        assert project_json["sim_specs"]["end_time"] == 20.0
        assert project_json["sim_specs"]["dt"] == "0.5"
        assert len(project_json["models"]) == 1
        assert project_json["models"][0]["name"] == "main"

    def test_set_sim_specs_validates_dt_positive(self, xmile_model_path) -> None:
        """set_sim_specs should reject non-positive dt values."""
        model = simlin.load(xmile_model_path)
        project = model.project

        with pytest.raises(ValueError, match="must be positive"):
            project.set_sim_specs(dt=0)

        with pytest.raises(ValueError, match="must be positive"):
            project.set_sim_specs(dt=-1.0)

    def test_set_sim_specs_validates_dt_format(self, xmile_model_path) -> None:
        """set_sim_specs should reject invalid dt string formats."""
        model = simlin.load(xmile_model_path)
        project = model.project

        with pytest.raises(ValueError, match="Invalid dt format"):
            project.set_sim_specs(dt="not-a-number")

        with pytest.raises(ValueError, match="cannot be an empty string"):
            project.set_sim_specs(dt="")

    def test_set_sim_specs_validates_dt_division_by_zero(self, xmile_model_path) -> None:
        """set_sim_specs should reject division by zero in reciprocal dt."""
        model = simlin.load(xmile_model_path)
        project = model.project

        with pytest.raises(ValueError, match="denominator cannot be zero"):
            project.set_sim_specs(dt="1/0")

    def test_set_sim_specs_accepts_reciprocal_dt(self, xmile_model_path) -> None:
        """set_sim_specs should accept valid reciprocal dt notation."""
        model = simlin.load(xmile_model_path)
        project = model.project

        project.set_sim_specs(dt="1/4")

        project_json = json.loads(project.serialize_json().decode("utf-8"))
        assert project_json["sim_specs"]["dt"] == "1/4"


class TestProjectRepr:
    """Test string representation of projects."""
    
    def test_repr(self, xmile_model_path) -> None:
        """Test __repr__ method."""
        model = simlin.load(xmile_model_path)
        project = model.project
        repr_str = repr(project)
        assert "Project" in repr_str
        assert "model" in repr_str.lower()
