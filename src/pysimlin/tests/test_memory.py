"""Comprehensive memory leak testing for pysimlin C extension.

This module contains stress tests designed to detect memory leaks and
reference counting issues in the pysimlin Python C extension package.
"""

import contextlib
import gc
import sys
import weakref

import pytest

import simlin
from simlin import SimlinError, SimlinImportError, SimlinRuntimeError
from simlin._ffi import _finalizer_refs, ffi


class TestObjectCreationDestruction:
    """Test rapid creation and destruction of objects to detect memory leaks."""

    def test_rapid_project_creation_destruction(self, xmile_model_path) -> None:
        """Create and destroy many projects rapidly to test memory management."""
        projects = []
        project_ids = []
        initial_ref_count = len(_finalizer_refs)

        # Create many projects
        for _ in range(100):
            model = simlin.load(xmile_model_path)
            project = model.project
            projects.append(project)
            project_ids.append(id(project))

        # Check that finalizer refs were registered for every project we created.
        missing_finalizers = [pid for pid in project_ids if pid not in _finalizer_refs]
        assert not missing_finalizers, f"Missing finalizers for project ids: {missing_finalizers}"

        # Clear references
        projects.clear()
        gc.collect()

        # Allow some leeway for GC timing, but should be close to initial count
        remaining_refs = len(_finalizer_refs)
        assert remaining_refs <= initial_ref_count + 10, (
            f"Too many finalizer refs remaining: {remaining_refs}"
        )

    def test_rapid_model_creation_destruction(self, xmile_model_path) -> None:
        """Create and destroy many models rapidly."""
        models = []
        initial_ref_count = len(_finalizer_refs)

        for _ in range(100):
            model = simlin.load(xmile_model_path)
            project = model.project
            model = project.get_model()
            models.append(model)

        # Clear references and force garbage collection
        models.clear()
        gc.collect()

        # Should not have excessive finalizer refs
        remaining_refs = len(_finalizer_refs)
        assert remaining_refs <= initial_ref_count + 20, (
            f"Too many finalizer refs remaining: {remaining_refs}"
        )

    def test_rapid_sim_creation_destruction(self, xmile_model_path) -> None:
        """Create and destroy many simulations rapidly."""
        sims = []
        initial_ref_count = len(_finalizer_refs)

        for _ in range(100):
            model = simlin.load(xmile_model_path)
            project = model.project
            model = project.get_model()
            sim = model.simulate()
            sims.append(sim)

        # Clear references and force garbage collection
        sims.clear()
        gc.collect()

        # Should not have excessive finalizer refs
        remaining_refs = len(_finalizer_refs)
        assert remaining_refs <= initial_ref_count + 30, (
            f"Too many finalizer refs remaining: {remaining_refs}"
        )

    def test_nested_object_creation(self, xmile_model_path) -> None:
        """Test creating objects in nested loops to stress memory management."""
        initial_ref_count = len(_finalizer_refs)

        for _i in range(20):
            model = simlin.load(xmile_model_path)
            project = model.project
            for _j in range(5):
                model = project.get_model()
                for _k in range(3):
                    sim = model.simulate()
                    # Use the sim to ensure it's not optimized away
                    var_names = [v.name for v in model.variables]
                    if var_names:
                        with contextlib.suppress(SimlinRuntimeError):
                            sim.run_to_end()

        gc.collect()

        # Should not leak significant amounts of memory
        remaining_refs = len(_finalizer_refs)
        assert remaining_refs <= initial_ref_count + 50, (
            f"Too many finalizer refs remaining: {remaining_refs}"
        )


class TestReferenceCountingEdgeCases:
    """Test edge cases in reference counting that could lead to leaks."""

    def test_circular_reference_prevention(self, xmile_model_path) -> None:
        """Test that circular references don't prevent cleanup."""
        model = simlin.load(xmile_model_path)
        project = model.project
        model = project.get_model()

        # Create weak references to detect cleanup
        project_ref = weakref.ref(project)
        model_ref = weakref.ref(model)

        # Create potential circular reference by storing references
        model._test_project_ref = project
        project._test_model_ref = model

        # Clear main references
        del project
        del model
        gc.collect()

        # Both objects should be garbage collected despite artificial circular refs
        assert project_ref() is None, "Project not garbage collected"
        assert model_ref() is None, "Model not garbage collected"

    def test_multiple_references_to_same_object(self, xmile_model_path) -> None:
        """Test multiple Python references to the same underlying C object."""
        model = simlin.load(xmile_model_path)
        project = model.project

        # Get the same model multiple times
        model1 = project.get_model()
        model2 = project.get_model()

        # These should be different Python objects wrapping potentially same C object
        assert model1 is not model2

        # Create weak references
        model1_ref = weakref.ref(model1)
        model2_ref = weakref.ref(model2)

        # Delete one reference
        del model1
        gc.collect()

        # First should be collected, second should still exist
        assert model1_ref() is None
        assert model2_ref() is not None

        # Clean up
        del model2
        del project
        gc.collect()

        assert model2_ref() is None

    def test_reference_to_destroyed_parent(self, xmile_model_path) -> None:
        """Test behavior when parent object is destroyed before child."""
        model = simlin.load(xmile_model_path)
        project = model.project
        model = project.get_model()
        sim = model.simulate()

        # Create weak references
        weakref.ref(project)
        weakref.ref(model)
        sim_ref = weakref.ref(sim)

        # Delete parent objects first
        del project
        del model
        gc.collect()

        # Sim should still be usable
        assert sim_ref() is not None
        var_names = sim.get_var_names()
        assert isinstance(var_names, list)

        # Clean up sim
        del sim
        gc.collect()

        # All should be collected eventually
        assert sim_ref() is None

    def test_exception_during_construction(self, tmp_path, xmile_model_path) -> None:
        """Test that exceptions during object construction don't leak memory."""
        initial_ref_count = len(_finalizer_refs)

        # Test invalid project creation
        for _ in range(50):
            try:
                invalid_file = tmp_path / "invalid.stmx"
                invalid_file.write_bytes(b"invalid data")
                simlin.load(invalid_file)
            except SimlinRuntimeError:
                pass

        gc.collect()

        # Should not have leaked finalizer refs
        final_ref_count = len(_finalizer_refs)
        assert final_ref_count <= initial_ref_count + 5


class TestFinalizerBehavior:
    """Test proper finalizer behavior and cleanup."""

    def test_finalizer_registration(self, xmile_model_path) -> None:
        """Test that finalizers are properly registered for all objects."""
        initial_ref_count = len(_finalizer_refs)

        model = simlin.load(xmile_model_path)
        project = model.project
        model = project.get_model()
        sim = model.simulate()

        # Should have registered finalizers for all objects
        current_ref_count = len(_finalizer_refs)
        assert current_ref_count >= initial_ref_count + 3

        # Get object IDs for tracking
        project_id = id(project)
        model_id = id(model)
        sim_id = id(sim)

        # Objects should be in finalizer registry
        assert project_id in _finalizer_refs
        assert model_id in _finalizer_refs
        assert sim_id in _finalizer_refs

        # Clean up
        del project
        del model
        del sim
        gc.collect()

        # Objects should be removed from registry
        assert project_id not in _finalizer_refs
        assert model_id not in _finalizer_refs
        assert sim_id not in _finalizer_refs

    def test_finalizer_execution_order(self, xmile_model_path) -> None:
        """Test that finalizers execute properly regardless of cleanup order."""
        initial_ref_count = len(_finalizer_refs)

        # Create objects
        model = simlin.load(xmile_model_path)
        project = model.project
        model = project.get_model()
        sim = model.simulate()

        # Store the IDs for tracking
        project_id = id(project)
        model_id = id(model)
        sim_id = id(sim)

        # Verify objects are tracked
        assert project_id in _finalizer_refs
        assert model_id in _finalizer_refs
        assert sim_id in _finalizer_refs

        # Delete in different order than creation
        del sim
        del project
        del model
        gc.collect()

        # All objects should be cleaned up regardless of deletion order
        assert project_id not in _finalizer_refs
        assert model_id not in _finalizer_refs
        assert sim_id not in _finalizer_refs

        # No memory leaks
        final_ref_count = len(_finalizer_refs)
        assert final_ref_count == initial_ref_count

    def test_finalizer_with_gc_disabled(self, xmile_model_path) -> None:
        """Test finalizer behavior when garbage collection is disabled."""
        initial_ref_count = len(_finalizer_refs)

        # Disable automatic garbage collection
        gc.disable()
        try:
            objects = []
            for _ in range(20):
                model = simlin.load(xmile_model_path)
                project = model.project
                model = project.get_model()
                objects.extend([project, model])

            # Clear references but don't collect
            objects.clear()

            # Manually trigger collection
            gc.collect()
        finally:
            gc.enable()

        # All objects should be cleaned up
        final_ref_count = len(_finalizer_refs)
        assert final_ref_count <= initial_ref_count + 5


class TestContextManagerCleanup:
    """Test context manager cleanup functionality."""

    def test_context_manager_explicit_cleanup(self, xmile_model_path) -> None:
        """Test that context managers perform explicit cleanup."""
        project_ptr = None
        model_ptr = None

        model = simlin.load(xmile_model_path)
        project = model.project

        with project:
            project_ptr = project._ptr
            assert project_ptr != ffi.NULL

            with project.get_model() as model:
                model_ptr = model._ptr
                assert model_ptr != ffi.NULL

                # Objects should be valid inside context
                assert len(model.variables) > 0

            # Model should be cleaned up after context exit
            assert model._ptr == ffi.NULL

        # Project should be cleaned up after context exit
        assert project._ptr == ffi.NULL

    def test_nested_context_manager_exception_safety(self, xmile_model_path) -> None:
        """Test context manager cleanup when exceptions occur."""

        model = simlin.load(xmile_model_path)
        project = model.project

        try:
            with project, project.get_model() as model:
                # Simulate an exception
                raise ValueError("Test exception")
        except ValueError:
            pass

        # Both objects should be cleaned up despite exception
        assert project._ptr == ffi.NULL
        assert model._ptr == ffi.NULL

    def test_context_manager_multiple_entries(self, xmile_model_path) -> None:
        """Test that context managers can be entered multiple times safely."""
        model = simlin.load(xmile_model_path)
        project = model.project

        # First context entry
        with project as p1:
            assert p1 is project
            assert project._ptr != ffi.NULL
            len(project.get_model().variables)

        # Should be cleaned up
        assert project._ptr == ffi.NULL

        # Second entry should fail gracefully or recreate if needed
        try:
            with project:
                # This might fail if the object is truly cleaned up
                pass
        except (SimlinError, ValueError):
            # Expected behavior for cleaned up object
            pass

    def test_context_manager_with_simulation(self, xmile_model_path) -> None:
        """Test context manager cleanup with running simulations."""
        model = simlin.load(xmile_model_path)
        project = model.project

        with project, project.get_model() as model:
            sim = model.simulate()
            var_names = [v.name for v in model.variables]

            if var_names:
                try:
                    sim.run_to_end()
                    # Get some results to use the simulation
                    time_values = sim.get_series("time")
                    assert len(time_values) > 0
                except SimlinRuntimeError:
                    pass  # Some models may have compilation errors

        # All objects should be properly cleaned up


class TestErrorPathMemoryLeaks:
    """Test memory leaks in error conditions and exception paths."""

    def test_import_error_no_leak(self, tmp_path) -> None:
        """Test that import errors don't leak memory."""
        initial_ref_count = len(_finalizer_refs)

        error_count = 0
        for i in range(100):
            try:
                # Try various invalid inputs
                invalid_xmile = tmp_path / f"invalid_{i}_xmile.stmx"
                invalid_xmile.write_bytes(b"not xml")
                simlin.load(invalid_xmile)
            except SimlinRuntimeError:
                error_count += 1

            try:
                invalid_mdl = tmp_path / f"invalid_{i}_mdl.mdl"
                invalid_mdl.write_bytes(b"invalid mdl")
                simlin.load(invalid_mdl)
            except SimlinRuntimeError:
                error_count += 1

        assert error_count > 0  # Ensure we actually tested error paths

        gc.collect()

        # Should not have leaked memory
        final_ref_count = len(_finalizer_refs)
        assert final_ref_count <= initial_ref_count + 5

    def test_runtime_error_no_leak(self, xmile_model_path) -> None:
        """Test that runtime errors don't leak memory."""
        initial_ref_count = len(_finalizer_refs)

        for _ in range(50):
            model = simlin.load(xmile_model_path)
            project = model.project
            model = project.get_model()

            # Try to access non-existent variables to trigger errors
            with contextlib.suppress(SimlinRuntimeError):
                model.get_incoming_links("nonexistent_variable_xyz")

            # Try invalid simulation operations
            sim = model.simulate()
            with contextlib.suppress(SimlinRuntimeError):
                # Try to get results for non-existent variable
                sim.get_series("nonexistent_var")

        gc.collect()

        # Should not have leaked memory
        final_ref_count = len(_finalizer_refs)
        assert final_ref_count <= initial_ref_count + 10

    def test_file_not_found_no_leak(self) -> None:
        """Test that file not found errors don't leak memory."""
        initial_ref_count = len(_finalizer_refs)

        for _ in range(50):
            with contextlib.suppress(SimlinRuntimeError, SimlinImportError):
                simlin.load("/nonexistent/path/file.stmx")

        gc.collect()

        # Should not have leaked memory
        final_ref_count = len(_finalizer_refs)
        assert final_ref_count <= initial_ref_count + 5

    def test_corrupted_data_no_leak(self, tmp_path) -> None:
        """Test that corrupted data doesn't leak memory."""
        initial_ref_count = len(_finalizer_refs)

        # Generate various corrupted data patterns
        corrupted_patterns = [
            b"",  # Empty
            b"\x00" * 100,  # Null bytes
            b"\xff" * 100,  # High bytes
            b"<?xml version='1.0'?><incomplete",  # Incomplete XML
            b"<?xml version='1.0'?><xmile><model></xmile>",  # Invalid structure
            b"A" * 10000,  # Large invalid data
        ]

        for i, pattern in enumerate(corrupted_patterns):
            for j in range(10):
                try:
                    invalid_xmile = tmp_path / f"corrupted_{i}_{j}_xmile.stmx"
                    invalid_xmile.write_bytes(pattern)
                    simlin.load(invalid_xmile)
                except SimlinRuntimeError:
                    pass

                try:
                    invalid_mdl = tmp_path / f"corrupted_{i}_{j}_mdl.mdl"
                    invalid_mdl.write_bytes(pattern)
                    simlin.load(invalid_mdl)
                except SimlinRuntimeError:
                    pass

        gc.collect()

        # Should not have leaked memory
        final_ref_count = len(_finalizer_refs)
        assert final_ref_count <= initial_ref_count + 10


class TestMemoryStressTesting:
    """Comprehensive stress tests for memory management."""

    def test_large_scale_object_churn(self, xmile_model_path) -> None:
        """Test large-scale creation and destruction of objects."""
        initial_ref_count = len(_finalizer_refs)

        for batch in range(10):
            objects = []

            # Create a batch of objects
            for _i in range(100):
                try:
                    model = simlin.load(xmile_model_path)
                    project = model.project
                    model = project.get_model()
                    sim = model.simulate()
                    objects.extend([project, model, sim])
                except (SimlinImportError, SimlinRuntimeError):
                    pass

            # Use objects to prevent optimization
            for obj in objects[::10]:  # Sample every 10th object
                if hasattr(obj, "variables"):
                    with contextlib.suppress(BaseException):
                        len(obj.variables)

            # Clear batch
            objects.clear()
            gc.collect()

            # Check memory periodically
            if batch % 5 == 0:
                current_ref_count = len(_finalizer_refs)
                # Allow some growth but not unbounded
                assert current_ref_count <= initial_ref_count + 50

        # Final cleanup check
        gc.collect()
        final_ref_count = len(_finalizer_refs)
        assert final_ref_count <= initial_ref_count + 20

    def test_concurrent_object_access(self, xmile_model_path) -> None:
        """Test concurrent access patterns that might reveal memory issues."""
        projects = []
        models = []
        sims = []

        # Create objects
        for _ in range(50):
            model = simlin.load(xmile_model_path)
            project = model.project
            model = project.get_model()
            sim = model.simulate()
            projects.append(project)
            models.append(model)
            sims.append(sim)

        # Interleaved access patterns
        for _i in range(10):
            # Access in different orders
            for j in range(0, len(projects), 3):
                if j < len(projects):
                    with contextlib.suppress(BaseException):
                        len(projects[j].get_model_names())

            for j in range(1, len(models), 3):
                if j < len(models):
                    with contextlib.suppress(BaseException):
                        [v.name for v in models[j].variables]

            for j in range(2, len(sims), 3):
                if j < len(sims):
                    with contextlib.suppress(BaseException):
                        sims[j].get_var_names()

        # Clear all at once
        projects.clear()
        models.clear()
        sims.clear()
        gc.collect()

    def test_memory_usage_bounds(self, xmile_model_path) -> None:
        """Test that memory usage stays within reasonable bounds."""
        if not hasattr(sys, "getsizeof"):
            pytest.skip("sys.getsizeof not available")

        initial_ref_count = len(_finalizer_refs)

        # Create objects and measure
        objects = []
        for i in range(100):
            model = simlin.load(xmile_model_path)
            project = model.project
            model = project.get_model()
            objects.extend([project, model])

            # Check periodically that finalizer registry isn't growing unbounded
            if i % 20 == 0:
                current_ref_count = len(_finalizer_refs)
                # Should grow roughly linearly with objects created
                expected_max = initial_ref_count + (i + 1) * 2 + 10
                assert current_ref_count <= expected_max

        # Clear and verify cleanup
        objects.clear()
        gc.collect()

        final_ref_count = len(_finalizer_refs)
        assert final_ref_count <= initial_ref_count + 10


class TestMemoryLeakDetection:
    """Tests specifically designed to catch memory leaks."""

    def test_repeated_operations_memory_stable(self, xmile_model_path) -> None:
        """Test that repeated operations don't cause unbounded memory growth."""
        # Baseline measurement
        gc.collect()
        baseline_refs = len(_finalizer_refs)

        # Perform operations multiple times
        for iteration in range(50):
            model = simlin.load(xmile_model_path)
            project = model.project
            model = project.get_model()

            # Perform various operations
            [v.name for v in model.variables]
            model.get_links()
            try:
                sim = model.simulate()
                sim.get_var_names()
                var_names = [v.name for v in model.variables]
                if var_names:
                    sim.run_to_end()
            except SimlinRuntimeError:
                pass

            # Explicit cleanup every few iterations
            if iteration % 10 == 0:
                gc.collect()
                current_refs = len(_finalizer_refs)
                # Memory usage should not grow unbounded
                assert current_refs <= baseline_refs + 30

        # Final cleanup
        gc.collect()
        final_refs = len(_finalizer_refs)
        assert final_refs <= baseline_refs + 10

    def test_string_handling_no_leak(self, xmile_model_path) -> None:
        """Test that string conversions don't leak memory."""
        initial_refs = len(_finalizer_refs)

        model = simlin.load(xmile_model_path)
        project = model.project
        model = project.get_model()

        # Exercise string operations heavily
        for _ in range(1000):
            var_names = [v.name for v in model.variables]
            for name in var_names[:5]:  # Limit to first 5 to avoid timeout
                try:
                    links = model.get_incoming_links(name)
                    # Force string operations
                    str(links)
                except SimlinRuntimeError:
                    pass

        # Clean up
        del model
        del project
        gc.collect()

        final_refs = len(_finalizer_refs)
        assert final_refs <= initial_refs + 5
