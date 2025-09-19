"""Tests for the Model class."""

import pytest
from pathlib import Path
import simlin
from simlin import Project, Model, SimlinRuntimeError, AuxVariable, FlowVariable
from simlin._generated import project_io_pb2 as project_io


@pytest.fixture
def test_model(xmile_model_data: bytes) -> Model:
    """Create a test model from XMILE data."""
    project = Project.from_xmile(xmile_model_data)
    return project.get_model()


class TestModelVariables:
    """Test working with model variables."""
    
    def test_get_var_count_indirect(self, test_model: Model) -> None:
        """Test getting the number of variables indirectly through get_var_names."""
        names = test_model.get_var_names()
        count = len(names)
        assert isinstance(count, int)
        assert count > 0
    
    def test_get_var_names(self, test_model: Model) -> None:
        """Test getting variable names."""
        names = test_model.get_var_names()
        assert isinstance(names, list)
        assert len(names) > 0
        for name in names:
            assert isinstance(name, str)
            assert len(name) > 0
    
    def test_get_incoming_links(self, test_model: Model) -> None:
        """Test getting incoming links for variables."""
        var_names = test_model.get_var_names()
        
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
        var_names = test_model.get_var_names()
        
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
        sim = test_model.new_sim()
        assert sim is not None
        from simlin import Sim
        assert isinstance(sim, Sim)
    
    def test_new_sim_with_ltm(self, test_model: Model) -> None:
        """Test creating a simulation with LTM enabled."""
        sim = test_model.new_sim(enable_ltm=True)
        assert sim is not None
        from simlin import Sim
        assert isinstance(sim, Sim)
    
    def test_multiple_sims(self, test_model: Model) -> None:
        """Test creating multiple simulations from the same model."""
        sim1 = test_model.new_sim()
        sim2 = test_model.new_sim()
        assert sim1 is not sim2
        # Both should be valid
        sim1.run_to_end()
        sim2.run_to_end()


class TestModelContextManager:
    """Test context manager functionality for models."""
    
    def test_context_manager_basic_usage(self, xmile_model_data: bytes) -> None:
        """Test basic context manager usage."""
        project = Project.from_xmile(xmile_model_data)
        with project.get_model() as model:
            assert model is not None
            assert model.get_var_count() > 0
            # Model should be usable inside the context
            var_names = model.get_var_names()
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
    
    def test_context_manager_with_exception(self, xmile_model_data: bytes) -> None:
        """Test context manager cleanup when exception occurs."""
        from simlin._ffi import ffi
        
        project = Project.from_xmile(xmile_model_data)
        model = project.get_model()
        
        try:
            with model:
                # Simulate an exception
                raise ValueError("Test exception")
        except ValueError:
            pass
        
        # Even with exception, cleanup should occur
        assert model._ptr == ffi.NULL
    
    def test_nested_context_managers(self, xmile_model_data: bytes) -> None:
        """Test nested context managers with project and model."""
        with Project.from_xmile(xmile_model_data) as project:
            with project.get_model() as model:
                # Both should be usable inside their contexts
                assert len(project.get_model_names()) > 0
                assert model.get_var_count() > 0
                sim = model.new_sim()
                assert sim is not None


class TestModelEditing:
    """Tests for the model editing context manager."""

    def test_edit_context_applies_flow_changes(self, mdl_model_data: bytes) -> None:
        """Patches created inside edit() should apply when the context exits."""
        project = Project.from_mdl(mdl_model_data)
        model = project.get_model()

        with model.edit(allow_errors=True) as (current, patch):
            heat_loss = current["Heat Loss to Room"]
            assert isinstance(heat_loss, FlowVariable)
            heat_loss.set_equation("0")
            patch.upsert(heat_loss)

        project_proto = project_io.Project()
        project_proto.ParseFromString(project.serialize())
        model_proto = project_proto.models[0]
        flow_proto = next(
            var.flow
            for var in model_proto.variables
            if var.flow.ident in {"Heat Loss to Room", "heat_loss_to_room"}
        )

        assert flow_proto.equation.scalar.equation == "0"

    def test_edit_context_dry_run_does_not_commit(self, mdl_model_data: bytes) -> None:
        """dry_run=True should validate without mutating the project."""
        project = Project.from_mdl(mdl_model_data)
        model = project.get_model()

        original = project_io.Project()
        original.ParseFromString(project.serialize())
        original_flow = next(
            var.flow
            for var in original.models[0].variables
            if var.flow.ident in {"Heat Loss to Room", "heat_loss_to_room"}
        )

        with model.edit(dry_run=True, allow_errors=True) as (current, patch):
            flow = current["Heat Loss to Room"]
            assert isinstance(flow, FlowVariable)
            flow.set_equation("0")
            patch.upsert(flow)

        project_proto = project_io.Project()
        project_proto.ParseFromString(project.serialize())
        flow_proto = next(
            var.flow
            for var in project_proto.models[0].variables
            if var.flow.ident in {"Heat Loss to Room", "heat_loss_to_room"}
        )

        assert flow_proto.equation.scalar.equation == original_flow.equation.scalar.equation

    def test_edit_context_invalid_patch_raises(self, xmile_model_data: bytes) -> None:
        """Invalid edits should raise and leave the project unchanged."""
        project = Project.from_xmile(xmile_model_data)
        model = project.get_model()

        before = project_io.Project()
        before.ParseFromString(project.serialize())

        with pytest.raises(SimlinRuntimeError):
            with model.edit() as (_, patch):
                bad_aux = AuxVariable.new("bad_variable")
                bad_aux.set_equation("?? invalid expression")
                patch.upsert(bad_aux)

        after = project_io.Project()
        after.ParseFromString(project.serialize())
        assert after == before


class TestModelRepr:
    """Test string representation of models."""
    
    def test_repr(self, test_model: Model) -> None:
        """Test __repr__ method."""
        repr_str = repr(test_model)
        assert "Model" in repr_str
        assert "variable" in repr_str.lower()