"""Static polarity of a loop through a module's input port.

The engine composes the sub-model's own link polarities along every pathway
from the entry port to the output the parent reads, so the pre-simulation
``Model.loops`` label of a loop through a delay-time port is the sign the
runtime series shows -- a DELAY3's delay time reaches its output through
``stock / (delay_time / 3)``, a balancing link.
"""

from __future__ import annotations

import pytest

import simlin
from simlin import LoopPolarity
from simlin.types import Aux, Flow, Stock


def _delay_time_loop_model(flow_equation: str) -> simlin.Model:
    """``s -> tau -> module -> f -> s`` with the module's delay time closing
    the loop."""
    project = simlin.Project.new(name="delay_port", sim_start=0.0, sim_stop=30.0, dt=0.5)
    model = project.main_model
    with model.edit() as (_current, patch):
        patch.upsert(Stock(name="s", initial_equation="50", inflows=["f"], outflows=[]))
        patch.upsert(Aux(name="tau", equation="2 + 0.02 * s"))
        patch.upsert(Aux(name="inp", equation="10"))
        patch.upsert(Flow(name="f", equation=flow_equation))
    return model


class TestModuleInputPortPolarity:
    def test_delay3_delay_time_loop_is_balancing_before_and_after_simulation(self) -> None:
        model = _delay_time_loop_model("DELAY3(inp, tau)")
        structural = model.loops
        assert len(structural) == 1
        assert structural[0].id == "b1"
        assert structural[0].polarity == LoopPolarity.BALANCING
        assert structural[0].polarity_confidence == pytest.approx(1.0)

        run = model.run(analyze_loops=True)
        assert run.ltm_mode == "exhaustive"
        assert len(run.loops) == 1
        assert run.loops[0].id == "b1"
        assert run.loops[0].polarity == LoopPolarity.BALANCING

    def test_smth1_input_port_loop_stays_reinforcing(self) -> None:
        project = simlin.Project.new(name="smth_input", sim_start=0.0, sim_stop=30.0, dt=0.5)
        model = project.main_model
        with model.edit() as (_current, patch):
            patch.upsert(Stock(name="s", initial_equation="50", inflows=["f"], outflows=[]))
            patch.upsert(Flow(name="f", equation="SMTH1(0.1 * s, 4)"))
        structural = model.loops
        assert [(lp.id, lp.polarity) for lp in structural] == [("r1", LoopPolarity.REINFORCING)]
        run = model.run(analyze_loops=True)
        assert [(lp.id, lp.polarity) for lp in run.loops] == [("r1", LoopPolarity.REINFORCING)]
