"""Runtime polarity is classified on the partition-relative loop-score series.

The engine primitive behind ``Run.loops`` (``reclassify_loops_from_results``)
feeds the classifier each loop's share of its cycle partition, bounded per
step, so the confidence is the dominance-weighted time share of each sign.
``exhaustive_competing_sign_flip_reclassifies_to_rux`` in the engine holds the
hand calculation for this fixture; here the same numbers arrive through the
FFI and ``Run.loops``.
"""

from __future__ import annotations

import numpy as np
import pytest

import simlin
from simlin import LoopPolarity
from simlin.types import Aux, Flow, Stock


def _competing_sign_flip_model(flipping: str) -> simlin.Model:
    """A stock with an inflow loop and an outflow loop; the ``flipping`` loop's
    gain g is +0.02, flipped to -0.02 for t in [100, 102), while the other
    loop's gain d rises from 0.01 to 2 in that window and carries the
    partition."""
    inflow_gain, outflow_gain = ("g", "d") if flipping == "inflow" else ("d", "g")
    project = simlin.Project.new(name="competing_flip", sim_start=0.0, sim_stop=110.0, dt=0.25)
    model = project.main_model
    with model.edit() as (_current, patch):
        patch.upsert(Aux(name="g", equation="IF TIME < 100 OR TIME >= 102 THEN 0.02 ELSE -0.02"))
        patch.upsert(Aux(name="d", equation="IF TIME < 100 OR TIME >= 102 THEN 0.01 ELSE 2"))
        patch.upsert(Stock(name="s", initial_equation="100", inflows=["f_in"], outflows=["f_out"]))
        patch.upsert(Flow(name="f_in", equation=f"s * {inflow_gain}"))
        patch.upsert(Flow(name="f_out", equation=f"s * {outflow_gain}"))
    return model


class TestRuntimePolarityOnTheRelativeBase:
    @pytest.mark.parametrize(
        ("flipping", "expected", "minority_sign"),
        [
            ("inflow", LoopPolarity.MOSTLY_REINFORCING, -1),
            ("outflow", LoopPolarity.MOSTLY_BALANCING, 1),
        ],
    )
    def test_competing_sign_flip_is_mostly_its_dominant_sign(
        self, flipping: str, expected: LoopPolarity, minority_sign: int
    ) -> None:
        model = _competing_sign_flip_model(flipping)
        run = model.run(analyze_loops=True)
        assert run.ltm_mode == "exhaustive"
        flow = "f_in" if flipping == "inflow" else "f_out"
        loop = next(lp for lp in run.loops if flow in lp.variables)
        assert loop.polarity == expected
        assert 0.99 <= loop.polarity_confidence < 1.0
        assert loop.polarity_confidence == pytest.approx(0.9994, abs=1e-3)
        # The behavior series is the relative series the label was read from:
        # eight steps carry the minority sign, at a share of about one percent.
        series = loop.behavior_time_series
        assert series is not None
        minority = series[np.sign(series) == minority_sign]
        assert minority.size == 8
        assert np.all(np.abs(minority) < 0.02)

    def test_lone_loop_confidence_is_the_time_share_of_its_sign(self) -> None:
        """A loop alone in its partition has a relative score of exactly +1/-1
        while active, so the confidence is the time share of its sign: this
        loop is reinforcing for the ~400 steps before t = 100 and balancing
        for the 40 after, roughly nine percent of its active life, which is
        Undetermined at a confidence of about 0.82.  The counts are read off
        the series the engine classified rather than pinned, because the
        number of active startup steps is an engine detail this test does not
        own."""
        project = simlin.Project.new(name="lone_flip", sim_start=0.0, sim_stop=110.0, dt=0.25)
        model = project.main_model
        with model.edit() as (_current, patch):
            patch.upsert(Aux(name="g", equation="IF TIME < 100 THEN 0.02 ELSE -0.0001"))
            patch.upsert(Aux(name="d", equation="IF TIME < 100 THEN 0 ELSE 50 * (TIME - 100)"))
            patch.upsert(Stock(name="s", initial_equation="100", inflows=["f"], outflows=[]))
            patch.upsert(Flow(name="f", equation="s * g + d"))
        run = model.run(analyze_loops=True)
        (loop,) = run.loops
        series = loop.behavior_time_series
        assert series is not None
        assert np.all(np.isin(series, [-1.0, 0.0, 1.0]))
        positive = int((series > 0).sum())
        negative = int((series < 0).sum())
        # Balancing at each of the 40 steps in (100, 110]; reinforcing at every
        # other active step.  The run saves 441 steps and the loop is active at
        # all of them but the startup step or two the flow-to-stock score needs.
        assert negative == 40
        active = positive + negative
        assert 439 <= active <= 441
        time_share = abs(positive - negative) / active
        assert 0.8 < time_share < 0.83
        assert loop.polarity == LoopPolarity.UNDETERMINED
        assert loop.polarity_confidence == pytest.approx(time_share)
