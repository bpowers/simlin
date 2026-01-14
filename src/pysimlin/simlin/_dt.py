"""Utilities for parsing and validating dt (time step) values.

dt can be specified as:
- A positive number (int or float): 0.25, 1, 0.5
- A reciprocal notation string: "1/4" (meaning dt = 0.25)
- A plain numeric string: "0.25", "1"
"""

from __future__ import annotations

import re
from typing import Any

# Pattern for reciprocal notation like "1/4" or "0.5/2"
_DT_RECIPROCAL_PATTERN = re.compile(r"^(\d+(?:\.\d+)?)/(\d+(?:\.\d+)?)$")


def validate_dt(value: Any) -> str:
    """Validate dt value and return normalized string representation.

    Use this for user-provided dt values where invalid input should raise an error.

    Args:
        value: The dt value - can be int, float, or string in "n" or "1/n" format

    Returns:
        Normalized string representation of dt

    Raises:
        ValueError: If the value is not a valid dt format
    """
    if isinstance(value, (int, float)):
        if value <= 0:
            raise ValueError(f"dt must be positive, got {value}")
        return str(value)

    if isinstance(value, str):
        value = value.strip()
        if not value:
            raise ValueError("dt cannot be an empty string")

        # Check for reciprocal format like "1/4"
        match = _DT_RECIPROCAL_PATTERN.match(value)
        if match:
            numerator = float(match.group(1))
            denominator = float(match.group(2))
            if denominator == 0:
                raise ValueError("dt denominator cannot be zero")
            if numerator <= 0:
                raise ValueError(f"dt numerator must be positive, got {numerator}")
            if denominator < 0:
                raise ValueError(f"dt denominator must be positive, got {denominator}")
            return value

        # Try to parse as a plain number
        try:
            num_value = float(value)
            if num_value <= 0:
                raise ValueError(f"dt must be positive, got {value}")
            return value
        except ValueError:
            raise ValueError(
                f"Invalid dt format: {value!r}. Expected a positive number or "
                f"reciprocal notation like '1/4'"
            )

    raise ValueError(f"dt must be a number or string, got {type(value).__name__}")


def parse_dt(dt_str: str, default: float = 1.0) -> float:
    """Parse a dt string into a float value.

    Use this for parsing stored dt values where we want lenient behavior
    (defaulting to a safe value rather than raising errors).

    Args:
        dt_str: The dt string to parse (e.g., "0.25", "1/4", "")
        default: Default value to return if dt_str is empty or invalid

    Returns:
        The parsed dt value as a float

    Raises:
        ValueError: If dt contains division by zero (this is always an error)
    """
    if not dt_str:
        return default

    # Check for reciprocal format like "1/4"
    if "/" in dt_str:
        match = _DT_RECIPROCAL_PATTERN.match(dt_str)
        if match:
            numerator = float(match.group(1))
            denominator = float(match.group(2))
            if denominator == 0:
                raise ValueError("Invalid dt: division by zero in reciprocal notation")
            result = numerator / denominator
            if result <= 0:
                return default
            return result
        # Invalid reciprocal format, use default
        return default

    # Try to parse as a plain number
    try:
        result = float(dt_str)
        if result <= 0:
            return default
        return result
    except ValueError:
        return default
