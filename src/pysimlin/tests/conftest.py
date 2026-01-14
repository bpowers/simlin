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


@pytest.fixture
def teacup_stmx_path(fixtures_dir: Path) -> Path:
    """Return path to teacup STMX model."""
    return fixtures_dir / "teacup.stmx"


@pytest.fixture
def teacup_xmile_path(fixtures_dir: Path) -> Path:
    """Return path to teacup XMILE model."""
    return fixtures_dir / "teacup.xmile"


@pytest.fixture
def logistic_growth_json_path() -> Path:
    """Return path to logistic growth JSON model."""
    return Path(__file__).parent / "logistic-growth.sd.json"


@pytest.fixture
def subscripted_model_path() -> Path:
    """Return path to a model with subscripted (arrayed) variables."""
    # This model has flows with apply-to-all equations
    # Path from tests/conftest.py to simlin repo root is 4 levels up
    return Path(__file__).parent.parent.parent.parent / "test" / "test-models" / "tests" / "subscript_multiples" / "test_multiple_subscripts.stmx"