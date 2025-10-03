"""Tests for the Project class."""

import pytest
from pathlib import Path
import simlin
from simlin import Project, SimlinImportError, ErrorCode
from simlin import pb


class TestProjectLoading:
    """Test loading projects from various formats."""
    
    def test_load_from_xmile(self, xmile_model_data: bytes) -> None:
        """Test loading a project from XMILE format."""
        project = Project.from_xmile(xmile_model_data)
        assert project is not None
        assert len(project.get_model_names()) > 0
    
    def test_load_from_mdl(self, mdl_model_data: bytes) -> None:
        """Test loading a project from MDL format."""
        project = Project.from_mdl(mdl_model_data)
        assert project is not None
        assert len(project.get_model_names()) > 0

    def test_load_from_json(self, json_model_data: bytes) -> None:
        """Test loading a project from JSON format."""
        project = Project.from_json(json_model_data)
        assert project is not None
        assert len(project.get_model_names()) > 0

    def test_load_from_json_sdai_format(self) -> None:
        """Test loading a project from SDAI JSON format."""
        sdai_json = b"""{
            "variables": [
                {
                    "type": "stock",
                    "name": "inventory",
                    "equation": "50",
                    "units": "widgets",
                    "inflows": ["production"],
                    "outflows": ["sales"]
                },
                {
                    "type": "flow",
                    "name": "production",
                    "equation": "10",
                    "units": "widgets/month"
                },
                {
                    "type": "flow",
                    "name": "sales",
                    "equation": "8",
                    "units": "widgets/month"
                },
                {
                    "type": "variable",
                    "name": "target_inventory",
                    "equation": "100",
                    "units": "widgets"
                }
            ],
            "specs": {
                "startTime": 0.0,
                "stopTime": 10.0,
                "dt": 1.0,
                "timeUnits": "months"
            }
        }"""
        project = Project.from_json(sdai_json, format=simlin.JSON_FORMAT_SDAI)
        assert project is not None
        assert len(project.get_model_names()) > 0

    def test_load_from_json_invalid_format(self, json_model_data: bytes) -> None:
        """Test that invalid format string raises ValueError."""
        with pytest.raises(ValueError, match="Invalid format"):
            Project.from_json(json_model_data, format="invalid")

    def test_load_from_json_default_format(self, json_model_data: bytes) -> None:
        """Test that default format is simlin format."""
        # Should work with default format parameter
        project = Project.from_json(json_model_data)
        assert project is not None
        # Should be equivalent to explicitly specifying simlin format
        project2 = Project.from_json(json_model_data, format=simlin.JSON_FORMAT_SIMLIN)
        assert project2 is not None

    def test_load_from_file_xmile(self, xmile_model_path: Path) -> None:
        """Test loading a project from an XMILE file."""
        project = Project.from_file(xmile_model_path)
        assert project is not None
        assert len(project.get_model_names()) > 0

    def test_load_from_file_json(self, json_model_path: Path) -> None:
        """Test loading a project from a JSON file."""
        project = Project.from_file(json_model_path)
        assert project is not None
        assert len(project.get_model_names()) > 0

    def test_load_logistic_growth_json(self) -> None:
        """Test loading the logistic growth model from JSON file."""
        repo_root = Path(__file__).parent.parent.parent.parent
        test_file = repo_root / "test" / "logistic-growth.sd.json"
        assert test_file.exists(), f"Test file not found: {test_file}"

        project = Project.from_file(test_file)
        assert project is not None
        assert len(project.get_model_names()) > 0
        model = project.get_model()
        assert model is not None

    def test_load_empty_data_raises(self) -> None:
        """Test that loading empty data raises an error."""
        with pytest.raises(SimlinImportError, match="Empty"):
            Project.from_xmile(b"")
        
        with pytest.raises(SimlinImportError, match="Empty"):
            Project.from_mdl(b"")
    
    def test_load_invalid_data_raises(self) -> None:
        """Test that loading invalid data raises an error."""
        with pytest.raises(SimlinImportError):
            Project.from_xmile(b"not valid xml")
        
        with pytest.raises(SimlinImportError):
            Project.from_mdl(b"not a valid model")
    
    def test_load_nonexistent_file_raises(self) -> None:
        """Test that loading a nonexistent file raises an error."""
        with pytest.raises(SimlinImportError, match="not found"):
            Project.from_file(Path("/nonexistent/file.stmx"))


class TestProjectModels:
    """Test working with models in a project."""
    
    def test_get_model_count(self, xmile_model_data: bytes) -> None:
        """Test getting the number of models through names."""
        project = Project.from_xmile(xmile_model_data)
        names = project.get_model_names()
        assert len(names) >= 1
        assert isinstance(len(names), int)
    
    def test_get_model_names(self, xmile_model_data: bytes) -> None:
        """Test getting model names."""
        project = Project.from_xmile(xmile_model_data)
        names = project.get_model_names()
        assert isinstance(names, list)
        # Names list has been validated above
        for name in names:
            assert isinstance(name, str)
    
    def test_get_default_model(self, xmile_model_data: bytes) -> None:
        """Test getting the default model."""
        project = Project.from_xmile(xmile_model_data)
        model = project.get_model()
        assert model is not None
        from simlin import Model
        assert isinstance(model, Model)
    
    def test_get_named_model(self, xmile_model_data: bytes) -> None:
        """Test getting a model by name."""
        project = Project.from_xmile(xmile_model_data)
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
        assert model.get_var_count() == 4
        assert set(model.get_var_names()) == {"time", "dt", "initial_time", "final_time"}

    def test_get_nonexistent_model_raises(self, xmile_model_data: bytes) -> None:
        """Test that getting a nonexistent model raises an error."""
        project = Project.from_xmile(xmile_model_data)
        with pytest.raises(SimlinImportError, match="not found"):
            project.get_model("nonexistent_model_name_xyz")


class TestProjectAnalysis:
    """Test project analysis functions."""
    
    def test_get_loops(self, xmile_model_data: bytes) -> None:
        """Test getting feedback loops."""
        project = Project.from_xmile(xmile_model_data)
        loops = project.get_loops()
        assert isinstance(loops, list)
        # Not all models have loops
        for loop in loops:
            assert hasattr(loop, 'id')
            assert hasattr(loop, 'variables')
            assert hasattr(loop, 'polarity')
    
    def test_get_errors(self, xmile_model_data: bytes) -> None:
        """Test getting project errors."""
        project = Project.from_xmile(xmile_model_data)
        errors = project.get_errors()
        assert isinstance(errors, list)
        # Valid models might have no errors
        for error in errors:
            assert hasattr(error, 'code')
            assert hasattr(error, 'message')


class TestProjectSerialization:
    """Test project serialization and export."""
    
    def test_serialize_to_protobuf(self, xmile_model_data: bytes) -> None:
        """Test serializing a project to protobuf."""
        project = Project.from_xmile(xmile_model_data)
        pb_data = project.serialize()
        assert isinstance(pb_data, bytes)
        assert len(pb_data) > 0
        
        # Should be able to reload
        project2 = Project.from_protobin(pb_data)
        assert len(project2.get_model_names()) == len(project.get_model_names())
    
    def test_export_to_xmile(self, xmile_model_data: bytes) -> None:
        """Test exporting a project to XMILE."""
        project = Project.from_xmile(xmile_model_data)
        xmile_data = project.to_xmile()
        assert isinstance(xmile_data, bytes)
        assert len(xmile_data) > 0
        assert b"<xmile" in xmile_data or b"<?xml" in xmile_data
    
    def test_round_trip_protobuf(self, xmile_model_data: bytes) -> None:
        """Test round-trip through protobuf format."""
        project1 = Project.from_xmile(xmile_model_data)
        pb_data = project1.serialize()
        project2 = Project.from_protobin(pb_data)
        
        assert len(project2.get_model_names()) == len(project1.get_model_names())
        assert project2.get_model_names() == project1.get_model_names()


class TestProjectContextManager:
    """Test context manager functionality for projects."""
    
    def test_context_manager_basic_usage(self, xmile_model_data: bytes) -> None:
        """Test basic context manager usage."""
        with Project.from_xmile(xmile_model_data) as project:
            assert project is not None
            assert len(project.get_model_names()) > 0
            # Project should be usable inside the context
            model = project.get_model()
            assert model is not None
    
    def test_context_manager_returns_self(self, xmile_model_data: bytes) -> None:
        """Test that __enter__ returns self."""
        project = Project.from_xmile(xmile_model_data)
        with project as ctx_project:
            assert ctx_project is project
    
    def test_context_manager_explicit_cleanup(self, xmile_model_data: bytes) -> None:
        """Test that __exit__ performs explicit cleanup."""
        from simlin._ffi import ffi
        
        project = Project.from_xmile(xmile_model_data)
        original_ptr = project._ptr
        
        # Use as context manager
        with project:
            pass
        
        # After context exit, pointer should be NULL
        assert project._ptr == ffi.NULL
        assert original_ptr != ffi.NULL  # Original was valid
    
    def test_context_manager_with_exception(self, xmile_model_data: bytes) -> None:
        """Test context manager cleanup when exception occurs."""
        from simlin._ffi import ffi
        
        project = Project.from_xmile(xmile_model_data)
        
        try:
            with project:
                # Simulate an exception
                raise ValueError("Test exception")
        except ValueError:
            pass
        
        # Even with exception, cleanup should occur
        assert project._ptr == ffi.NULL
    
    def test_non_context_manager_usage_still_works(self, xmile_model_data: bytes) -> None:
        """Test that objects still work without context manager."""
        # Should work exactly as before without using 'with'
        project = Project.from_xmile(xmile_model_data)
        assert len(project.get_model_names()) > 0
        model = project.get_model()
        assert model is not None
        # Cleanup will still happen through finalizer


class TestProjectEditing:
    """Tests for editing project-level metadata."""

    def test_set_sim_specs_updates_project(self, xmile_model_data: bytes) -> None:
        """set_sim_specs should update the serialized simulation specs."""
        project = Project.from_xmile(xmile_model_data)

        project.set_sim_specs(
            start=0.0,
            stop=42.0,
            dt={"value": 0.25, "is_reciprocal": False},
            save_step=pb.Dt(value=0.5, is_reciprocal=False),
            sim_method=pb.SimMethod.EULER,
            time_units="Minutes",
        )

        project_proto = pb.Project()
        project_proto.ParseFromString(project.serialize())

        assert project_proto.sim_specs.start == pytest.approx(0.0)
        assert project_proto.sim_specs.stop == pytest.approx(42.0)
        assert project_proto.sim_specs.dt.value == pytest.approx(0.25)
        assert project_proto.sim_specs.dt.is_reciprocal is False
        assert project_proto.sim_specs.save_step.value == pytest.approx(0.5)
        assert project_proto.sim_specs.sim_method == pb.SimMethod.EULER
        assert project_proto.sim_specs.time_units == "Minutes"

    def test_set_sim_specs_rejects_invalid_dt(self, xmile_model_data: bytes) -> None:
        """Invalid dt types should raise a TypeError before reaching the engine."""
        project = Project.from_xmile(xmile_model_data)

        with pytest.raises(TypeError):
            project.set_sim_specs(dt="not-a-dt")


class TestProjectRepr:
    """Test string representation of projects."""
    
    def test_repr(self, xmile_model_data: bytes) -> None:
        """Test __repr__ method."""
        project = Project.from_xmile(xmile_model_data)
        repr_str = repr(project)
        assert "Project" in repr_str
        assert "model" in repr_str.lower()
