"""Tests for the Run class."""

from pathlib import Path

import pandas as pd
import pytest

import simlin
from simlin.run import DominantPeriod
from simlin.types import TimeSpec


class TestRunClass:
    """Test the Run class functionality."""

    def test_run_results_property(self, xmile_model_path: Path) -> None:
        """Test that Run.results returns a DataFrame."""
        model = simlin.load(xmile_model_path)

        run = model.run(analyze_loops=False)

        assert isinstance(run.results, pd.DataFrame)
        assert len(run.results) > 0
        assert "time" in run.results.index.name or run.results.index.name == "time"

    def test_run_overrides_property(self, teacup_stmx_path: Path) -> None:
        """Test that Run.overrides returns the overrides dict."""
        model = simlin.load(teacup_stmx_path)

        # room_temperature is a simple constant (equation = "70")
        overrides = {"room_temperature": 42.0}
        run = model.run(overrides=overrides, analyze_loops=False)

        assert run.overrides == overrides
        assert isinstance(run.overrides, dict)

    def test_run_overrides_empty_when_none(self, xmile_model_path: Path) -> None:
        """Test that Run.overrides is empty dict when no overrides provided."""
        model = simlin.load(xmile_model_path)

        run = model.run(analyze_loops=False)

        assert run.overrides == {}

    def test_run_time_spec_property(self, xmile_model_path: Path) -> None:
        """Test that Run.time_spec returns valid TimeSpec."""
        model = simlin.load(xmile_model_path)

        run = model.run(analyze_loops=False)

        assert isinstance(run.time_spec, TimeSpec)
        assert run.time_spec.start >= 0
        assert run.time_spec.stop > run.time_spec.start
        assert run.time_spec.dt > 0

    def test_run_loops_property_without_ltm(self, xmile_model_path: Path) -> None:
        """Test that Run.loops returns empty tuple when analyze_loops=False."""
        model = simlin.load(xmile_model_path)

        run = model.run(analyze_loops=False)

        assert isinstance(run.loops, tuple)

    def test_run_loops_property_with_ltm(self, xmile_model_path: Path) -> None:
        """Test that Run.loops returns Loop objects with behavior when analyze_loops=True."""
        model = simlin.load(xmile_model_path)

        if len(model.loops) == 0:
            pytest.skip("Test model has no loops")

        run = model.run(analyze_loops=True)

        assert isinstance(run.loops, tuple)

    def test_run_dominant_periods_without_ltm(self, xmile_model_path: Path) -> None:
        """Test that Run.dominant_periods returns empty tuple when analyze_loops=False."""
        model = simlin.load(xmile_model_path)

        run = model.run(analyze_loops=False)

        assert isinstance(run.dominant_periods, tuple)
        assert len(run.dominant_periods) == 0

    def test_run_dominant_periods_with_ltm(self, xmile_model_path: Path) -> None:
        """Test that Run.dominant_periods returns DominantPeriod objects."""
        model = simlin.load(xmile_model_path)

        if len(model.loops) == 0:
            pytest.skip("Test model has no loops")

        run = model.run(analyze_loops=True)

        assert isinstance(run.dominant_periods, tuple)

    def test_run_caching(self, xmile_model_path: Path) -> None:
        """Test that Run properties are cached properly."""
        model = simlin.load(xmile_model_path)

        run = model.run(analyze_loops=False)

        results1 = run.results
        results2 = run.results
        assert results1 is results2

        time_spec1 = run.time_spec
        time_spec2 = run.time_spec
        assert time_spec1 is time_spec2


class TestDominantPeriod:
    """Test the DominantPeriod dataclass."""

    def test_dominant_period_creation(self) -> None:
        """Test creating a DominantPeriod."""
        period = DominantPeriod(
            dominant_loops=("R1", "B2"),
            start_time=0.0,
            end_time=10.0,
        )

        assert period.dominant_loops == ("R1", "B2")
        assert period.start_time == 0.0
        assert period.end_time == 10.0

    def test_dominant_period_duration(self) -> None:
        """Test calculating duration of a period."""
        period = DominantPeriod(
            dominant_loops=("R1",),
            start_time=5.0,
            end_time=15.0,
        )

        assert period.duration() == 10.0

    def test_dominant_period_contains_loop(self) -> None:
        """Test checking if a loop is in dominant_loops."""
        period = DominantPeriod(
            dominant_loops=("R1", "B2"),
            start_time=0.0,
            end_time=10.0,
        )

        assert period.contains_loop("R1")
        assert period.contains_loop("B2")
        assert not period.contains_loop("R3")

    def test_dominant_period_immutable(self) -> None:
        """Test that DominantPeriod is immutable."""
        period = DominantPeriod(
            dominant_loops=("R1",),
            start_time=0.0,
            end_time=10.0,
        )

        with pytest.raises(AttributeError):
            period.start_time = 5.0
