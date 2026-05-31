"""End-to-end tests for pinning a feedback loop (the LOOPSCORE capability).

A pinned loop is named by its variable set via the edit/patch API
(`patch.set_loop_name`). The LTM engine then ALWAYS scores it -- even in
discovery mode, where the heuristic search emits no per-loop score at all. The
pinned loop appears in `model.loops` / `run.loops`, and its score is readable
by its `pin{n}` id through `Sim.get_relative_loop_score`.
"""

from __future__ import annotations

import numpy as np

import simlin
from simlin.json_types import Flow, Stock


def _pinned_loop_in_discovery_model() -> simlin.Model:
    """Build a model with a >50-node SCC (forcing discovery mode) plus a small
    two-stock loop pinned by name.

    The big ring trips the auto-flip gate (so exhaustive enumeration is
    skipped); the pinned a<->b loop is the only loop scored.
    """
    project = simlin.Project.new(name="pinned_discovery", sim_start=0.0, sim_stop=5.0, dt=0.25)
    model = project.main_model
    with model.edit() as (_current, patch):
        ring = 60
        for i in range(ring):
            nxt = (i + 1) % ring
            patch.upsert_flow(Flow(name=f"f{i}", equation=f"stock_{nxt} * 0.001"))
            patch.upsert_stock(
                Stock(name=f"stock_{i}", initial_equation="10", inflows=[f"f{i}"], outflows=[])
            )
        # Small two-stock reinforcing loop: a -> to_b -> b -> to_a -> a.
        patch.upsert_stock(Stock(name="a", initial_equation="100", inflows=["to_a"], outflows=[]))
        patch.upsert_stock(Stock(name="b", initial_equation="100", inflows=["to_b"], outflows=[]))
        patch.upsert_flow(Flow(name="to_b", equation="a * 0.05"))
        patch.upsert_flow(Flow(name="to_a", equation="b * 0.05"))
        # Pin the a<->b loop in the same patch (upserts run first, assigning
        # the UIDs SetLoopName resolves against).
        patch.set_loop_name("ab loop", ["a", "to_b", "b", "to_a"])
    return model


def _small_two_loop_model() -> simlin.Model:
    """A small two-loop population model (stays in exhaustive mode)."""
    project = simlin.Project.new(name="two_loop", sim_start=0.0, sim_stop=20.0, dt=0.25)
    model = project.main_model
    with model.edit() as (_current, patch):
        patch.upsert_stock(
            Stock(
                name="population",
                initial_equation="100",
                inflows=["births"],
                outflows=["deaths"],
            )
        )
        patch.upsert_flow(Flow(name="births", equation="population * 0.08"))
        from simlin.json_types import Auxiliary

        patch.upsert_aux(Auxiliary(name="crowding", equation="population / 1000"))
        patch.upsert_flow(Flow(name="deaths", equation="population * crowding"))
    return model


class TestPinViaEditApi:
    """The edit/patch API can pin a loop by name."""

    def test_set_loop_name_op_round_trips(self) -> None:
        # Pinning a loop should not raise and the model should compile.
        model = _small_two_loop_model()
        with model.edit() as (_current, patch):
            patch.set_loop_name("growth", ["population", "births"])
        # Re-fetch via a fresh run to confirm the model is still simulatable.
        run = model.run(analyze_loops=True)
        assert run.ltm_mode == "exhaustive"


class TestPinnedLoopInDiscoveryMode:
    """The headline capability: a pinned loop is scored even in discovery mode."""

    def test_pinned_loop_surfaces_and_is_scored(self) -> None:
        model = _pinned_loop_in_discovery_model()
        run = model.run(analyze_loops=True)

        # The big ring forces discovery mode.
        assert run.ltm_mode == "discovery"

        # The pinned loop appears in run.loops (with behavior data) even though
        # discovery emits no other loop scores.
        loop_ids = {loop.id for loop in run.loops}
        assert "pin1" in loop_ids, f"pinned loop must surface in run.loops; got {loop_ids}"
        assert loop_ids == {"pin1"}, (
            f"only the pinned loop should surface in discovery mode; got {loop_ids}"
        )

        pin_loop = next(loop for loop in run.loops if loop.id == "pin1")
        # The pinned loop carries its behavior (relative-loop-score) series.
        assert pin_loop.behavior_time_series is not None
        assert len(pin_loop.behavior_time_series) > 0
        assert np.all(np.isfinite(pin_loop.behavior_time_series))
        # It is the only loop, so its relative score is +/-1 once active.
        nonzero = pin_loop.behavior_time_series[pin_loop.behavior_time_series != 0.0]
        assert nonzero.size > 0, "pinned loop should have non-zero behavior"
        assert np.allclose(np.abs(nonzero), 1.0)

        # The loop is also structurally present (no behavior) on model.loops.
        structural_ids = {loop.id for loop in model.loops}
        assert "pin1" in structural_ids

    def test_pinned_loop_readable_by_id_via_sim(self) -> None:
        model = _pinned_loop_in_discovery_model()
        with model.simulate(enable_ltm=True) as sim:
            sim.run_to_end()
            series = sim.get_relative_loop_score("pin1")
            assert series.size > 0
            assert np.all(np.isfinite(series))
            assert sim.get_loop_element_count("pin1") == 1
