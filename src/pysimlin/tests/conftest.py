"""Pytest configuration and shared fixtures."""

import pytest
from pathlib import Path
from typing import Generator


@pytest.fixture
def fixtures_dir() -> Path:
    """Return the path to the test fixtures directory."""
    return Path(__file__).parent / "fixtures"


@pytest.fixture
def xmile_model_path(fixtures_dir: Path) -> Path:
    """Return path to a test XMILE model."""
    return fixtures_dir / "eval_order.stmx"


@pytest.fixture
def mdl_model_path(fixtures_dir: Path) -> Path:
    """Return path to a test MDL model."""
    return fixtures_dir / "teacup.mdl"


@pytest.fixture
def xmile_model_data(xmile_model_path: Path) -> bytes:
    """Load XMILE model data."""
    return xmile_model_path.read_bytes()


@pytest.fixture
def mdl_model_data(mdl_model_path: Path) -> bytes:
    """Load MDL model data."""
    return mdl_model_path.read_bytes()


@pytest.fixture
def json_model_path(fixtures_dir: Path) -> Path:
    """Return path to a test JSON model."""
    return fixtures_dir / "simple.json"


@pytest.fixture
def json_model_data(json_model_path: Path) -> bytes:
    """Load JSON model data."""
    return json_model_path.read_bytes()