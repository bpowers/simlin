"""Tests for error handling."""

from simlin import (
    ErrorCode,
    ErrorDetail,
    SimlinRuntimeError,
)


class TestErrorTypes:
    """Test error type hierarchy."""

    def test_runtime_error_with_details(self) -> None:
        """SimlinRuntimeError carries per-variable diagnostics on ``details``."""
        details = [
            ErrorDetail(
                code=ErrorCode.CIRCULAR_DEPENDENCY,
                message="Circular dependency detected",
                model_name="test_model",
                variable_name="var_a",
            )
        ]

        err = SimlinRuntimeError("edit rejected", ErrorCode.CIRCULAR_DEPENDENCY, details)
        assert err.code == ErrorCode.CIRCULAR_DEPENDENCY
        assert len(err.details) == 1
        assert err.details[0].code == ErrorCode.CIRCULAR_DEPENDENCY

        bare = SimlinRuntimeError("no details")
        assert bare.details == []


class TestErrorDetail:
    """Test ErrorDetail rendering."""

    def test_error_detail_str(self) -> None:
        """``str(detail)`` assembles only the parts the detail actually has.

        This is what a caller prints when diagnosing a rejected edit, and it
        is conditional on four optional fields; both the fully-populated and
        the code-and-message-only shapes are covered here.
        """
        detail1 = ErrorDetail(
            code=ErrorCode.CIRCULAR_DEPENDENCY,
            message="Circular dependency",
            model_name="model1",
            variable_name="var1",
            start_offset=5,
            end_offset=10,
        )

        str_repr = str(detail1)
        assert "CIRCULAR_DEPENDENCY" in str_repr
        assert "model1" in str_repr
        assert "var1" in str_repr
        assert "Circular dependency" in str_repr
        assert "5:10" in str_repr

        detail2 = ErrorDetail(code=ErrorCode.GENERIC, message="Simple error")

        str_repr2 = str(detail2)
        assert "GENERIC" in str_repr2
        assert "Simple error" in str_repr2
        assert "in model" not in str_repr2
        assert "for variable" not in str_repr2
