"""Tests for explicit, opt-in loop discovery via Model.analyze().

Discovery (the strongest-path "Loops That Matter" algorithm) is exposed as
Model.analyze(timeout=None) -> Analysis.  It is deliberately separate from
Model.run(): run() never triggers discovery, because discovery can be slow or
infeasible on large models.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

import numpy as np
import pytest

import simlin
from simlin import Analysis, DominantPeriod, Loop

if TYPE_CHECKING:
    from pathlib import Path


@pytest.fixture
def logistic_model(logistic_growth_ltm_path: Path) -> simlin.Model:
    """Load the small logistic-growth LTM model."""
    return simlin.load(logistic_growth_ltm_path)


class TestAnalyzeDiscovery:
    """Model.analyze() runs discovery and returns an Analysis."""

    def test_returns_analysis(self, logistic_model: simlin.Model) -> None:
        analysis = logistic_model.analyze()
        assert isinstance(analysis, Analysis)
        assert not analysis.truncated

    def test_discovers_loops_with_importance(self, logistic_model: simlin.Model) -> None:
        analysis = logistic_model.analyze()

        assert len(analysis.loops) > 0, "discovery should find loops on logistic growth"

        for loop in analysis.loops:
            assert isinstance(loop, Loop)
            assert loop.id, "each discovered loop must have an id"
            assert len(loop.variables) >= 2, "loop chains contain at least two variables"
            # The importance series is populated from the FFI importance array.
            assert loop.behavior_time_series is not None, (
                f"discovered loop {loop.id} must carry an importance series"
            )
            assert len(loop.behavior_time_series) > 0
            assert np.all(np.isfinite(loop.behavior_time_series))
            # average_importance() reads behavior_time_series; it must be a real
            # number for a discovered loop (not None, as it is for structural).
            avg = loop.average_importance()
            assert avg is not None

    def test_discovered_loop_variables_have_no_trailing_repeat(
        self, logistic_model: simlin.Model
    ) -> None:
        # Discovered loops must use the same bare-node-sequence convention as
        # structural loops (Model.loops): the closing node is implied, NOT stored
        # as a trailing repeat of the first variable. Loop.__str__ closes the
        # chain itself, so a stored repeat would double the closing node and
        # Loop.__len__ would overcount.
        analysis = logistic_model.analyze()
        assert analysis.loops

        structural_ids = {lp.id for lp in logistic_model.loops}

        for loop in analysis.loops:
            vars = loop.variables
            assert len(vars) >= 2
            assert vars[0] != vars[-1], (
                f"discovered loop {loop.id} stores a trailing repeat: {vars}"
            )
            assert len(loop) == len(vars)
            # __str__ closes the chain exactly once (no '... -> A -> A' tail).
            assert not str(loop).endswith(f"-> {vars[0]} -> {vars[0]}")

        # Same node count as the structural enumeration for loops present in both.
        for loop in analysis.loops:
            if loop.id in structural_ids:
                structural = next(lp for lp in logistic_model.loops if lp.id == loop.id)
                assert len(loop.variables) == len(structural.variables)

    def test_discovers_dominant_periods(self, logistic_model: simlin.Model) -> None:
        analysis = logistic_model.analyze()

        assert len(analysis.dominant_periods) > 0, "logistic growth should have dominant periods"
        for period in analysis.dominant_periods:
            assert isinstance(period, DominantPeriod)
            assert period.start_time <= period.end_time
            assert len(period.dominant_loops) > 0, "a period must name dominant loops"

    def test_timeout_seconds_completes(self, logistic_model: simlin.Model) -> None:
        # A generous timeout on a tiny model completes without truncation.
        analysis = logistic_model.analyze(timeout=60.0)
        assert not analysis.truncated
        assert len(analysis.loops) > 0


class TestAnalyzeOptIn:
    """analyze() is opt-in: run() does not populate discovery automatically."""

    def test_run_does_not_trigger_discovery(self, logistic_model: simlin.Model) -> None:
        # A normal run exposes only structural-loop behavior; it has no
        # discovery interface, and calling run() must not require/trigger
        # analyze().  We assert that Run carries no `truncated`/`Analysis`
        # surface and that analyze() is a distinct call returning Analysis.
        run = logistic_model.run()
        assert not hasattr(run, "truncated")
        assert not isinstance(run, Analysis)

        analysis = logistic_model.analyze()
        assert isinstance(analysis, Analysis)

    def test_analyze_is_idempotent_and_independent(self, logistic_model: simlin.Model) -> None:
        # Repeated analyze() calls must each return a fresh, consistent result
        # (the FFI restores the LTM flags on the shared project each time).
        first = logistic_model.analyze()
        second = logistic_model.analyze()
        assert len(first.loops) == len(second.loops)
        assert {lp.id for lp in first.loops} == {lp.id for lp in second.loops}


class TestAnalyzeTruncation:
    """A tiny timeout truncates discovery without hanging."""

    def test_tiny_timeout_truncates(self, large_horizon_model: simlin.Model) -> None:
        # A 1ms timeout on a model with a very long time horizon makes the
        # per-timestep discovery sweep exceed the budget, so the result is
        # marked truncated.  The contract is the flag plus a prompt return.
        analysis = large_horizon_model.analyze(timeout=0.001)
        assert analysis.truncated

    def test_negative_timeout_rejected(self, logistic_model: simlin.Model) -> None:
        with pytest.raises(ValueError, match="non-negative"):
            logistic_model.analyze(timeout=-1.0)


@pytest.fixture
def large_horizon_model() -> simlin.Model:
    """Build a large-horizon balancing model in memory for truncation tests.

    A goal-seeking model over 200k saved timesteps keeps values bounded while
    making the per-timestep discovery sweep reliably exceed a 1ms budget, so a
    tiny timeout truncates deterministically.
    """
    from simlin.json_types import Auxiliary, Flow, Stock

    project = simlin.Project.new(
        name="large_horizon",
        sim_start=0.0,
        sim_stop=200_000.0,
        dt=1.0,
    )
    model = project.main_model
    with model.edit() as (_current, patch):
        patch.upsert_stock(
            Stock(
                name="population",
                initial_equation="10",
                inflows=["adjustment"],
                outflows=[],
            )
        )
        patch.upsert_flow(
            Flow(
                name="adjustment",
                equation="(goal - population) * 0.1",
            )
        )
        patch.upsert_aux(Auxiliary(name="goal", equation="1000"))
    return model
