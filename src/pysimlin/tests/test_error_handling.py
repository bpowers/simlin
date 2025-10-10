"""Tests for error handling and error string management."""

import gc
import pytest

import simlin
from simlin import Project, SimlinImportError, SimlinRuntimeError
from simlin._ffi import ffi, lib, get_error_string, _finalizer_refs


class TestErrorStringHandling:
    """Test that error string handling doesn't cause crashes or memory issues."""

    def test_get_error_string_various_codes(self) -> None:
        """Test get_error_string with various error codes."""
        # Test valid error codes (from ErrorCode enum: 0-32)
        for code in [0, 1, 2, 3, 4, 5, 10, 15, 20, 25, 30, 32]:
            msg = get_error_string(code)
            assert isinstance(msg, str)
            assert len(msg) > 0

    def test_error_string_const_static(self) -> None:
        """Verify that simlin_error_str returns static strings that shouldn't be freed."""
        # This test verifies our understanding that simlin_error_str returns
        # const static strings. If this crashes, our assumption is wrong.
        for i in range(10):
            error_code = 2  # XML_DESERIALIZATION error
            c_str = lib.simlin_error_str(error_code)
            assert c_str != ffi.NULL

            # Convert to Python string multiple times (shouldn't crash)
            for j in range(5):
                python_str = ffi.string(c_str).decode("utf-8")
                assert len(python_str) > 0
                assert "XML" in python_str or "deserialization" in python_str.lower()

            # Do NOT call lib.simlin_free_string(c_str) - it's a static string!

    def test_import_error_handling(self, tmp_path) -> None:
        """Test that import errors are handled correctly."""
        # Test invalid XMILE
        invalid_xmile = tmp_path / "invalid.stmx"
        invalid_xmile.write_bytes(b"not xml")
        with pytest.raises(SimlinRuntimeError) as exc_info:
            simlin.load(invalid_xmile)
        assert "failed" in str(exc_info.value).lower()
        assert exc_info.value.code is not None

        # Test invalid MDL
        invalid_mdl = tmp_path / "invalid.mdl"
        invalid_mdl.write_bytes(b"invalid mdl")
        with pytest.raises(SimlinRuntimeError) as exc_info:
            simlin.load(invalid_mdl)
        assert "failed" in str(exc_info.value).lower()
        assert exc_info.value.code is not None

    def test_error_handling_stress(self, tmp_path) -> None:
        """Stress test error handling to check for crashes or leaks."""
        initial_refs = len(_finalizer_refs)

        # Generate many errors rapidly
        for i in range(100):
            # Try various invalid imports
            for j, invalid_data in enumerate([b"", b"x", b"not xml", b"\x00" * 10, b"\xFF" * 10]):
                invalid_file = tmp_path / f"invalid_{i}_{j}.stmx"
                invalid_file.write_bytes(invalid_data)
                try:
                    simlin.load(invalid_file)
                except SimlinRuntimeError:
                    pass  # Expected

        # Force garbage collection
        gc.collect()

        # Check that we haven't leaked too many finalizer refs
        final_refs = len(_finalizer_refs)
        # Allow some tolerance for objects that may still be in scope
        assert final_refs <= initial_refs + 10, f"Too many finalizer refs: {final_refs - initial_refs}"

    def test_model_error_paths(self, xmile_model_path) -> None:
        """Test error handling in model operations."""
        model = simlin.load(xmile_model_path)

        # Test error when accessing non-existent variable
        with pytest.raises(SimlinRuntimeError) as exc_info:
            model.get_incoming_links("nonexistent_variable_xyz_123")
        assert "Variable not found" in str(exc_info.value)

    def test_error_detail_collection(self, tmp_path) -> None:
        """Test that error details are collected properly without memory issues."""
        # Create a model with intentional errors
        bad_xmile = b"""<?xml version='1.0' encoding='utf-8'?>
        <xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
            <model name="test">
                <variables>
                    <aux name="BadVar">
                        <eqn>NonExistentVar * 2</eqn>
                    </aux>
                </variables>
            </model>
        </xmile>"""

        bad_file = tmp_path / "bad.stmx"
        bad_file.write_bytes(bad_xmile)
        model = simlin.load(bad_file)
        project = model.project
        errors = project.get_errors()

        # Should have compilation errors
        assert len(errors) > 0
        for error in errors:
            assert error.code is not None
            assert isinstance(error.message, str)
            # Model name and variable name may be None or string
            if error.model_name:
                assert isinstance(error.model_name, str)
            if error.variable_name:
                assert isinstance(error.variable_name, str)