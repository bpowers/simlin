"""Relative loop scores on a coupled arrayed model.

The engine normalizes every (loop, element) of a cycle partition against the
same partition sum -- sibling elements of an arrayed loop, other arrayed
loops' elements and scalar loops alike -- and both ``Sim.get_relative_loop_score``
and ``Run.loops`` read that one normalization.  ``test/cross_element_ltm`` is
the hand-computed case: its two regions are coupled through migration, so one
partition holds every loop of the model, and because both populations grow at
exactly 2% per step the raw loop scores are constant once active: +1 at both
elements of the births loop, -0.5 at NYC for the migration_out loop (0 at
Boston, whose migration flows are clamped to 0), +0.5 for the cross-element
migration_in loop, 0 for every other loop.  The partition sum is 3, so the
shares are 1/3, 1/3, -1/6 and +1/6, and their magnitudes sum to 1.
"""

from __future__ import annotations

import numpy as np
import pytest

import simlin

STEPS = (2, 10, 30)


def _loop_with_variables(loops, variables: set[str]):
    for loop in loops:
        if set(loop.variables) == variables:
            return loop
    raise AssertionError(
        f"no loop over {sorted(variables)}; loops: {[(lp.id, lp.variables) for lp in loops]}"
    )


@pytest.fixture
def cross_element_model(cross_element_ltm_path):
    return simlin.load(cross_element_ltm_path)


class TestCrossElementRelativeScores:
    def test_per_element_shares_normalize_over_the_whole_partition(
        self, cross_element_model
    ) -> None:
        loops = cross_element_model.loops
        births = _loop_with_variables(loops, {"population", "births"})
        out_loop = _loop_with_variables(
            loops, {"population", "migration_pressure", "migration_out"}
        )
        cross = _loop_with_variables(
            loops, {"population[nyc]", "migration_pressure[boston]", "migration_in[nyc]"}
        )

        with cross_element_model.simulate(enable_ltm=True) as sim:
            sim.run_to_end()
            assert sim.get_loop_element_count(births.id) == 2
            assert sim.get_loop_element_count(out_loop.id) == 2
            assert sim.get_loop_element_count(cross.id) == 1

            births_nyc = sim.get_relative_loop_score(births.id, element="NYC")
            births_boston = sim.get_relative_loop_score(births.id, element="Boston")
            out_nyc = sim.get_relative_loop_score(out_loop.id, element="NYC")
            out_boston = sim.get_relative_loop_score(out_loop.id, element="Boston")
            cross_series = sim.get_relative_loop_score(cross.id)

            for step in STEPS:
                assert births_nyc[step] == pytest.approx(1 / 3, abs=1e-9)
                assert births_boston[step] == pytest.approx(1 / 3, abs=1e-9)
                assert out_nyc[step] == pytest.approx(-1 / 6, abs=1e-9)
                assert out_boston[step] == pytest.approx(0.0, abs=1e-9)
                assert cross_series[step] == pytest.approx(1 / 6, abs=1e-9)

            # The partition identity over EVERY member: each element of each
            # loop, summed, is 1.
            for step in STEPS:
                total = 0.0
                for loop in loops:
                    n = sim.get_loop_element_count(loop.id)
                    if n == 1:
                        total += abs(sim.get_relative_loop_score(loop.id)[step])
                    else:
                        for element in ("NYC", "Boston"):
                            total += abs(
                                sim.get_relative_loop_score(loop.id, element=element)[step]
                            )
                assert total == pytest.approx(1.0, abs=1e-9)

    def test_run_loops_behavior_is_the_dominant_elements_share(self, cross_element_model) -> None:
        """``Run.loops`` carries the bare-id series: the argmax-abs across an
        arrayed loop's elements, read from the same per-element normalization.
        """
        run = cross_element_model.run(analyze_loops=True)
        assert run.ltm_mode == "exhaustive"
        loops = run.loops
        births = _loop_with_variables(loops, {"population", "births"})
        out_loop = _loop_with_variables(
            loops, {"population", "migration_pressure", "migration_out"}
        )
        cross = _loop_with_variables(
            loops, {"population[nyc]", "migration_pressure[boston]", "migration_in[nyc]"}
        )
        assert births.partition == out_loop.partition == cross.partition

        for step in STEPS:
            assert births.behavior_time_series[step] == pytest.approx(1 / 3, abs=1e-9)
            assert out_loop.behavior_time_series[step] == pytest.approx(-1 / 6, abs=1e-9)
            assert cross.behavior_time_series[step] == pytest.approx(1 / 6, abs=1e-9)
        assert np.all(np.isfinite(births.behavior_time_series))
