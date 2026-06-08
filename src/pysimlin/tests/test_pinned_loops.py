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

    def test_loop_name_recoverable_from_model_loops(self) -> None:
        # The human-meaningful name assigned via set_loop_name must be
        # recoverable on the corresponding Loop, so a caller need not remember
        # the pin order to map `pin1` back to its label.
        model = _small_two_loop_model()

        # Before pinning, every enumerated loop reports no assigned name.
        before = model.loops
        assert before, "the two-loop model has structural loops"
        assert all(loop.name is None for loop in before), "enumerated loops must have name == None"

        with model.edit() as (_current, patch):
            patch.set_loop_name("Ocean carbon uptake", ["population", "births"])

        named = [loop for loop in model.loops if loop.name is not None]
        assert len(named) == 1, "exactly the pinned loop carries a name"
        assert named[0].name == "Ocean carbon uptake"
        assert named[0].id.startswith("pin") or named[0].id.startswith(("r", "b", "u"))


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

        # The modeler-assigned name must survive the round-trip through run.loops
        # so a caller can recover the human-meaningful label, not just `pin1`.
        assert pin_loop.name == "ab loop"
        # __str__ includes the quoted name when present.
        assert '"ab loop"' in str(pin_loop)

        # The loop is also structurally present (no behavior) on model.loops,
        # carrying the same name.
        structural = {loop.id: loop for loop in model.loops}
        assert "pin1" in structural
        assert structural["pin1"].name == "ab loop"

    def test_pinned_loop_readable_by_id_via_sim(self) -> None:
        model = _pinned_loop_in_discovery_model()
        with model.simulate(enable_ltm=True) as sim:
            sim.run_to_end()
            series = sim.get_relative_loop_score("pin1")
            assert series.size > 0
            assert np.all(np.isfinite(series))
            assert sim.get_loop_element_count("pin1") == 1


def _arrayed_pin_in_discovery_model(arrayed_population_ltm_path) -> simlin.Model:
    """Load the arrayed population model, force discovery mode, and pin the
    arrayed births loop.

    The arrayed population model (`population[Region]` with births/deaths over
    NYC/Boston/LA) is small enough that LTM would enumerate it exhaustively, so
    a 60-stock scalar ring is patched in to trip the SCC auto-flip gate -- in
    discovery mode the pinned loop is the only loop scored at all (GH #653's
    headline scenario).
    """
    model = simlin.load(arrayed_population_ltm_path)
    with model.edit() as (_current, patch):
        ring = 60
        for i in range(ring):
            nxt = (i + 1) % ring
            patch.upsert_flow(Flow(name=f"f{i}", equation=f"ring_{nxt} * 0.001"))
            patch.upsert_stock(
                Stock(name=f"ring_{i}", initial_equation="10", inflows=[f"f{i}"], outflows=[])
            )
        # Pin the arrayed births loop (population -> births -> population, all
        # variables A2A over Region).
        patch.set_loop_name("regional growth", ["population", "births"])
    return model


class TestArrayedPinnedLoop:
    """GH #653: a pinned loop over arrayed variables is scored per element."""

    def test_arrayed_pin_scored_per_element(self, arrayed_population_ltm_path) -> None:
        model = _arrayed_pin_in_discovery_model(arrayed_population_ltm_path)

        with model.simulate(enable_ltm=True) as sim:
            sim.run_to_end()
            assert str(sim.get_ltm_mode()) == "discovery"

            # The arrayed pin occupies one slot per Region element.
            assert sim.get_loop_element_count("pin1") == 3

            # Per-element relative scores: NYC and Boston are growing (births
            # rate exceeds deaths rate), and the pin is the only scored loop in
            # their partitions, so their relative score is +1 once active. LA
            # sits at equilibrium (birth_rate == death_rate == 0.01), so every
            # link score -- and therefore the loop score -- is 0 there.
            for element, expect_active in [("NYC", True), ("Boston", True), ("LA", False)]:
                series = sim.get_relative_loop_score("pin1", element=element)
                assert series.size > 0, f"pin1[{element}] must have a score series"
                assert np.all(np.isfinite(series)), f"pin1[{element}] must be finite"
                nonzero = series[series != 0.0]
                if expect_active:
                    assert nonzero.size > 0, f"pin1[{element}] should be active"
                    assert np.allclose(np.abs(nonzero), 1.0), (
                        f"pin1[{element}] is the only scored loop in its partition, so its "
                        f"relative score is +/-1 while active; got {nonzero[:5]}"
                    )
                else:
                    assert nonzero.size == 0, (
                        f"pin1[{element}] is at equilibrium (births == deaths) and must "
                        f"score 0 at every step; got {nonzero[:5]}"
                    )

            # The unsubscripted form returns the argmax-abs aggregate across
            # the pin's element slots.
            by_name = sim.get_relative_loop_score("pin1", element="NYC")
            aggregate = sim.get_relative_loop_score("pin1")
            assert aggregate.size == by_name.size
            assert np.all(np.isfinite(aggregate))

    def test_arrayed_pin_surfaces_in_run_loops(self, arrayed_population_ltm_path) -> None:
        model = _arrayed_pin_in_discovery_model(arrayed_population_ltm_path)
        run = model.run(analyze_loops=True)

        assert run.ltm_mode == "discovery"
        loop_ids = {loop.id for loop in run.loops}
        assert "pin1" in loop_ids, f"the arrayed pin must surface in run.loops; got {loop_ids}"

        pin_loop = next(loop for loop in run.loops if loop.id == "pin1")
        # The behavior series (the argmax-abs aggregate across element slots
        # for an arrayed loop) is present and finite.
        assert pin_loop.behavior_time_series is not None
        assert np.all(np.isfinite(pin_loop.behavior_time_series))
        nonzero = pin_loop.behavior_time_series[pin_loop.behavior_time_series != 0.0]
        assert nonzero.size > 0, "the arrayed pin should have non-zero behavior"
