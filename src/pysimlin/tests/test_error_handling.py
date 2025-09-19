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
        # Test valid error codes
        for code in [0, 1, 2, 3, 4, 5]:
            msg = get_error_string(code)
            assert isinstance(msg, str)
            assert len(msg) > 0

        # Test invalid/unknown error codes
        for code in [-1, -2, 100, 999]:
            msg = get_error_string(code)
            assert isinstance(msg, str)
            # Should return something like "Unknown error code: X"
            if "Unknown" not in msg:
                # It's a valid error code, just verify it's a string
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

    def test_import_error_handling(self) -> None:
        """Test that import errors are handled correctly."""
        # Test invalid XMILE
        with pytest.raises(SimlinImportError) as exc_info:
            Project.from_xmile(b"not xml")
        assert "Failed to import XMILE" in str(exc_info.value)
        assert exc_info.value.code is not None

        # Test invalid MDL
        with pytest.raises(SimlinImportError) as exc_info:
            Project.from_mdl(b"invalid mdl")
        assert "Failed to import" in str(exc_info.value)
        assert exc_info.value.code is not None

        # Test invalid protobuf
        with pytest.raises(SimlinImportError) as exc_info:
            Project.from_protobin(b"not protobuf")
        assert "Failed to open project" in str(exc_info.value)
        assert exc_info.value.code is not None

    def test_error_handling_stress(self) -> None:
        """Stress test error handling to check for crashes or leaks."""
        initial_refs = len(_finalizer_refs)

        # Generate many errors rapidly
        for i in range(100):
            # Try various invalid imports
            for invalid_data in [b"", b"x", b"not xml", b"\x00" * 10, b"\xFF" * 10]:
                try:
                    Project.from_xmile(invalid_data)
                except SimlinImportError:
                    pass  # Expected

                try:
                    Project.from_mdl(invalid_data)
                except SimlinImportError:
                    pass  # Expected

                try:
                    Project.from_protobin(invalid_data)
                except SimlinImportError:
                    pass  # Expected

        # Force garbage collection
        gc.collect()

        # Check that we haven't leaked too many finalizer refs
        final_refs = len(_finalizer_refs)
        # Allow some tolerance for objects that may still be in scope
        assert final_refs <= initial_refs + 10, f"Too many finalizer refs: {final_refs - initial_refs}"

    def test_model_error_paths(self, xmile_model_data: bytes) -> None:
        """Test error handling in model operations."""
        project = Project.from_xmile(xmile_model_data)
        model = project.get_model()

        # Test error when accessing non-existent variable
        with pytest.raises(SimlinRuntimeError) as exc_info:
            model.get_incoming_links("nonexistent_variable_xyz_123")
        assert "Variable not found" in str(exc_info.value)

    def test_error_detail_collection(self) -> None:
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

        project = Project.from_xmile(bad_xmile)
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