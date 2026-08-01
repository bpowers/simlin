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
        """Without LTM, Run.loops carries the structural loops (structural
        polarity, no behavior series), not an empty tuple, when the model has
        enumerable feedback loops.

        This pins the LTM-disabled fallback: ``Run.loops`` flows through the
        engine's ``model_detected_loops`` (structural, no LTM dependency), so
        the loop set and polarity match ``Model.loops`` exactly, and only the
        runtime ``behavior_time_series`` is absent.
        """
        model = simlin.load(xmile_model_path)
        structural = model.loops

        run = model.run(analyze_loops=False)

        assert isinstance(run.loops, tuple)
        # Same loops as the structural surface (id + structural polarity).
        assert {loop.id for loop in run.loops} == {loop.id for loop in structural}
        structural_polarity = {loop.id: loop.polarity for loop in structural}
        for loop in run.loops:
            assert loop.polarity == structural_polarity[loop.id]
            # No runtime score series was emitted, so no behavior data.
            assert loop.behavior_time_series is None
        if len(structural) == 0:
            assert run.loops == ()

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

    def test_run_dominant_periods_are_per_partition(self, logistic_growth_ltm_path: Path) -> None:
        """GH #998 (surface 1, runtime path): Run.dominant_periods selects
        dominance within each cycle partition and tags every period with the
        partition it describes -- an isolated single-loop partition (relative
        score identically -1 while active) no longer smothers the competitive
        partition's timeline."""
        from simlin.json_types import Flow, Stock

        model = simlin.load(logistic_growth_ltm_path)
        with model.edit() as (_current, patch):
            patch.upsert_flow(Flow(name="iso_out", equation="iso * 0.1"))
            patch.upsert_stock(
                Stock(name="iso", initial_equation="50", inflows=[], outflows=["iso_out"])
            )
        run = model.run(analyze_loops=True)

        lone_loop = next(loop for loop in run.loops if any("iso" in v for v in loop.variables))
        competitive_partitions = {
            loop.partition
            for loop in run.loops
            if loop.partition is not None and loop.partition != lone_loop.partition
        }
        assert competitive_partitions, "the logistic loops keep their own partition"

        periods = run.dominant_periods
        assert periods, "periods must exist"
        assert any(p.partition in competitive_partitions for p in periods), (
            f"the competitive partition must have its own periods: {periods}"
        )
        for period in periods:
            if lone_loop.id in period.dominant_loops:
                assert period.partition == lone_loop.partition, (
                    "the lone loop is confined to its own partition's periods"
                )

    def test_group_loops_for_dominance_solo_none_on_partitioned_surface(self) -> None:
        """GH #998: on a partition-bearing surface each partition-None loop
        (module-internal; relative score +/-1 by construction) forms its OWN
        dominance group; only a surface with NO partition metadata pools them.

        The fixture hand-builds Loop objects because the function's whole
        input contract is the `partition` field, whose production values are
        exactly `None` or a dense int -- both shapes covered here.
        """
        from simlin.analysis import Loop, LoopPolarity
        from simlin.run import Run

        def loop(loop_id: str, partition: int | None) -> Loop:
            return Loop(
                id=loop_id,
                variables=("a", "b"),
                polarity=LoopPolarity.REINFORCING,
                partition=partition,
            )

        mixed = [loop("r1", 0), loop("mod_a", None), loop("b1", 0), loop("mod_b", None)]
        groups = Run._group_loops_for_dominance(mixed)
        assert [(p, [lo.id for lo in g]) for p, g in groups] == [
            (0, ["r1", "b1"]),
            (None, ["mod_a"]),
            (None, ["mod_b"]),
        ], "None loops must be solo groups when any loop carries a partition"

        flat = [loop("l1", None), loop("l2", None)]
        groups = Run._group_loops_for_dominance(flat)
        assert [(p, [lo.id for lo in g]) for p, g in groups] == [
            (None, ["l1", "l2"]),
        ], "an all-None surface keeps the single flat group"

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
