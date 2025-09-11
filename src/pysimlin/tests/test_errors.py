"""Tests for error handling."""

import pytest
from simlin import (
    SimlinError,
    SimlinCompilationError,
    SimlinRuntimeError,
    SimlinImportError,
    ErrorCode,
    ErrorDetail,
    error_code_to_string,
)


class TestErrorTypes:
    """Test error type hierarchy."""
    
    def test_error_inheritance(self) -> None:
        """Test that error types inherit correctly."""
        assert issubclass(SimlinCompilationError, SimlinError)
        assert issubclass(SimlinRuntimeError, SimlinError)
        assert issubclass(SimlinImportError, SimlinError)
        assert issubclass(SimlinError, Exception)
    
    def test_error_creation(self) -> None:
        """Test creating error instances."""
        err1 = SimlinError("test error")
        assert str(err1) == "test error"
        assert err1.code is None
        
        err2 = SimlinError("test error", ErrorCode.BAD_TABLE)
        assert err2.code == ErrorCode.BAD_TABLE
    
    def test_compilation_error_with_details(self) -> None:
        """Test compilation error with error details."""
        details = [
            ErrorDetail(
                code=ErrorCode.CIRCULAR_DEPENDENCY,
                message="Circular dependency detected",
                model_name="test_model",
                variable_name="var_a"
            )
        ]
        
        err = SimlinCompilationError("Compilation failed", details)
        assert len(err.errors) == 1
        assert err.errors[0].code == ErrorCode.CIRCULAR_DEPENDENCY


class TestErrorCode:
    """Test ErrorCode enum."""
    
    def test_error_code_values(self) -> None:
        """Test that error codes have expected values."""
        assert ErrorCode.NO_ERROR == 0
        assert ErrorCode.DOES_NOT_EXIST == 1
        assert ErrorCode.CIRCULAR_DEPENDENCY == 21
        assert ErrorCode.GENERIC == 32
    
    def test_error_code_to_string(self) -> None:
        """Test converting error codes to strings."""
        assert error_code_to_string(0) == "No Error"
        assert error_code_to_string(1) == "Does Not Exist"
        assert error_code_to_string(21) == "Circular Dependency"
        
        # Unknown error code
        unknown = error_code_to_string(999)
        assert "Unknown" in unknown
        assert "999" in unknown


class TestErrorDetail:
    """Test ErrorDetail dataclass."""
    
    def test_error_detail_creation(self) -> None:
        """Test creating ErrorDetail instances."""
        detail = ErrorDetail(
            code=ErrorCode.UNKNOWN_DEPENDENCY,
            message="Variable 'x' not found",
            model_name="main",
            variable_name="y",
            start_offset=10,
            end_offset=15
        )
        
        assert detail.code == ErrorCode.UNKNOWN_DEPENDENCY
        assert detail.message == "Variable 'x' not found"
        assert detail.model_name == "main"
        assert detail.variable_name == "y"
        assert detail.start_offset == 10
        assert detail.end_offset == 15
    
    def test_error_detail_optional_fields(self) -> None:
        """Test that optional fields default correctly."""
        detail = ErrorDetail(
            code=ErrorCode.GENERIC,
            message="Generic error"
        )
        
        assert detail.model_name is None
        assert detail.variable_name is None
        assert detail.start_offset == 0
        assert detail.end_offset == 0
    
    def test_error_detail_str(self) -> None:
        """Test string representation of ErrorDetail."""
        detail1 = ErrorDetail(
            code=ErrorCode.CIRCULAR_DEPENDENCY,
            message="Circular dependency",
            model_name="model1",
            variable_name="var1",
            start_offset=5,
            end_offset=10
        )
        
        str_repr = str(detail1)
        assert "CIRCULAR_DEPENDENCY" in str_repr
        assert "model1" in str_repr
        assert "var1" in str_repr
        assert "Circular dependency" in str_repr
        assert "5:10" in str_repr
        
        detail2 = ErrorDetail(
            code=ErrorCode.GENERIC,
            message="Simple error"
        )
        
        str_repr2 = str(detail2)
        assert "GENERIC" in str_repr2
        assert "Simple error" in str_repr2