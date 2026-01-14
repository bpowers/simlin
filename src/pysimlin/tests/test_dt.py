"""Tests for the _dt module (dt parsing and validation)."""

import pytest
from simlin._dt import validate_dt, parse_dt


class TestValidateDt:
    """Tests for validate_dt() function."""

    def test_accepts_positive_int(self) -> None:
        """validate_dt should accept positive integers."""
        assert validate_dt(1) == "1"
        assert validate_dt(4) == "4"

    def test_accepts_positive_float(self) -> None:
        """validate_dt should accept positive floats."""
        assert validate_dt(0.25) == "0.25"
        assert validate_dt(1.5) == "1.5"

    def test_accepts_numeric_string(self) -> None:
        """validate_dt should accept numeric strings."""
        assert validate_dt("0.25") == "0.25"
        assert validate_dt("1") == "1"
        assert validate_dt("  0.5  ") == "0.5"  # strips whitespace

    def test_accepts_reciprocal_notation(self) -> None:
        """validate_dt should accept reciprocal notation like '1/4'."""
        assert validate_dt("1/4") == "1/4"
        assert validate_dt("0.5/2") == "0.5/2"

    def test_rejects_zero(self) -> None:
        """validate_dt should reject zero."""
        with pytest.raises(ValueError, match="must be positive"):
            validate_dt(0)
        with pytest.raises(ValueError, match="must be positive"):
            validate_dt(0.0)

    def test_rejects_negative(self) -> None:
        """validate_dt should reject negative values."""
        with pytest.raises(ValueError, match="must be positive"):
            validate_dt(-1)
        with pytest.raises(ValueError, match="must be positive"):
            validate_dt(-0.5)

    def test_rejects_empty_string(self) -> None:
        """validate_dt should reject empty strings."""
        with pytest.raises(ValueError, match="cannot be an empty string"):
            validate_dt("")
        with pytest.raises(ValueError, match="cannot be an empty string"):
            validate_dt("   ")

    def test_rejects_invalid_string(self) -> None:
        """validate_dt should reject invalid string formats."""
        with pytest.raises(ValueError, match="Invalid dt format"):
            validate_dt("not-a-number")
        with pytest.raises(ValueError, match="Invalid dt format"):
            validate_dt("abc")

    def test_rejects_division_by_zero(self) -> None:
        """validate_dt should reject division by zero in reciprocal notation."""
        with pytest.raises(ValueError, match="denominator cannot be zero"):
            validate_dt("1/0")

    def test_rejects_negative_in_reciprocal_notation(self) -> None:
        """validate_dt should reject negative values in reciprocal notation."""
        # Negative numerator is treated as invalid format (not matched by pattern)
        with pytest.raises(ValueError, match="Invalid dt format"):
            validate_dt("-1/4")
        # Negative denominator is also treated as invalid format
        with pytest.raises(ValueError, match="Invalid dt format"):
            validate_dt("1/-4")

    def test_rejects_non_numeric_types(self) -> None:
        """validate_dt should reject non-numeric types."""
        with pytest.raises(ValueError, match="must be a number or string"):
            validate_dt({"value": 0.25})
        with pytest.raises(ValueError, match="must be a number or string"):
            validate_dt([0.25])
        with pytest.raises(ValueError, match="must be a number or string"):
            validate_dt(None)


class TestParseDt:
    """Tests for parse_dt() function."""

    def test_parses_numeric_string(self) -> None:
        """parse_dt should parse numeric strings."""
        assert parse_dt("0.25") == 0.25
        assert parse_dt("1") == 1.0
        assert parse_dt("1.5") == 1.5

    def test_parses_reciprocal_notation(self) -> None:
        """parse_dt should parse reciprocal notation."""
        assert parse_dt("1/4") == 0.25
        assert parse_dt("1/2") == 0.5
        assert parse_dt("2/4") == 0.5

    def test_empty_string_returns_default(self) -> None:
        """parse_dt should return default for empty strings."""
        assert parse_dt("") == 1.0
        assert parse_dt("", default=0.5) == 0.5

    def test_invalid_string_returns_default(self) -> None:
        """parse_dt should return default for invalid strings."""
        assert parse_dt("not-a-number") == 1.0
        assert parse_dt("abc", default=0.25) == 0.25

    def test_division_by_zero_raises(self) -> None:
        """parse_dt should raise ValueError for division by zero."""
        with pytest.raises(ValueError, match="division by zero"):
            parse_dt("1/0")

    def test_negative_result_returns_default(self) -> None:
        """parse_dt should return default for negative results."""
        assert parse_dt("-1") == 1.0
        assert parse_dt("-0.5", default=0.25) == 0.25
