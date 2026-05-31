"""Tests for the surfaced LTM loop-enumeration mode and macro/module-internal
link collapsing.

Task A: ``Run.ltm_mode`` / ``Sim.get_ltm_mode()`` expose whether LTM ran in
exhaustive (Johnson enumeration) or discovery (strongest-path heuristic) mode,
or was disabled entirely.

Task B: ``Sim.get_links(include_internal=False)`` (the default) collapses
macro/module-internal synthetic nodes out of the causal graph while preserving
the contribution that flows *through* them as a composite edge.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

import pytest

import simlin
from simlin import LtmMode

if TYPE_CHECKING:
    from pathlib import Path


@pytest.fixture
def logistic_model(logistic_growth_ltm_path: Path) -> simlin.Model:
    """Load the small logistic-growth LTM model (a real XMILE fixture)."""
    return simlin.load(logistic_growth_ltm_path)


def _smooth_feedback_model() -> simlin.Model:
    """Build a model with a SMTH1 macro inside a feedback loop.

    Mirrors the engine's `smooth_polarity` fixture: a stock `level` is driven
    by an `adjustment` flow that closes a balancing loop through a smoothed
    reading of the stock. SMTH1 expands to a stdlib module, so the causal graph
    gains a synthetic `$:smoothed_level:0:smth1`-style module node (with `:` the
    reserved U+205A separator) and a synthetic arg helper.
    """
    project = simlin.Project.new(name="smooth_feedback", sim_start=0.0, sim_stop=10.0, dt=1.0)
    model = project.main_model
    with model.edit() as (_current, patch):
        from simlin.json_types import Auxiliary, Flow, Stock

        patch.upsert_aux(Auxiliary(name="goal", equation="100"))
        patch.upsert_stock(
            Stock(name="level", initial_equation="50", inflows=["adjustment"], outflows=[])
        )
        patch.upsert_aux(Auxiliary(name="smoothed_level", equation="SMTH1(level, 3)"))
        patch.upsert_aux(Auxiliary(name="gap", equation="goal - smoothed_level"))
        patch.upsert_flow(Flow(name="adjustment", equation="gap / 5"))
    return model


def _large_scc_model(total_nodes: int) -> simlin.Model:
    """Build a single-SCC chain of `total_nodes` variables.

    Mirrors the engine's `build_chain_scc_project`: a stock closes a chain of
    auxes through a flow, forming one strongly-connected component whose node
    count is `total_nodes`. Above MAX_LTM_SCC_NODES (50) the LTM pipeline
    auto-flips to discovery mode.
    """
    assert total_nodes >= 3
    aux_count = total_nodes - 2
    project = simlin.Project.new(name="large_scc", sim_start=0.0, sim_stop=3.0, dt=1.0)
    model = project.main_model
    with model.edit() as (_current, patch):
        from simlin.json_types import Auxiliary, Flow, Stock

        for i in range(aux_count):
            equation = "cap_stock" if i + 1 == aux_count else f"aux_{i + 1}"
            patch.upsert_aux(Auxiliary(name=f"aux_{i}", equation=equation))
        patch.upsert_flow(Flow(name="cap_flow", equation="aux_0"))
        patch.upsert_stock(
            Stock(name="cap_stock", initial_equation="0", inflows=["cap_flow"], outflows=[])
        )
    return model


class TestLtmMode:
    """Run.ltm_mode / Sim.get_ltm_mode() surface the resolved enumeration mode."""

    def test_exhaustive_on_logistic_growth(self, logistic_model: simlin.Model) -> None:
        """A small model enumerates loops exhaustively (the task's named case)."""
        run = logistic_model.run(analyze_loops=True)
        assert run.ltm_mode == "exhaustive"

    def test_sim_get_ltm_mode_returns_enum(self, logistic_model: simlin.Model) -> None:
        with logistic_model.simulate(enable_ltm=True) as sim:
            assert sim.get_ltm_mode() is LtmMode.EXHAUSTIVE

    def test_disabled_when_loops_not_analyzed(self, logistic_model: simlin.Model) -> None:
        """Without LTM the mode is `disabled`, distinguishing it from an empty
        loop set."""
        run = logistic_model.run(analyze_loops=False)
        assert run.ltm_mode == "disabled"

    def test_sim_disabled_without_ltm(self, logistic_model: simlin.Model) -> None:
        with logistic_model.simulate(enable_ltm=False) as sim:
            assert sim.get_ltm_mode() is LtmMode.DISABLED

    def test_discovery_on_large_scc(self) -> None:
        """A model whose SCC exceeds MAX_LTM_SCC_NODES (50) auto-flips to
        discovery mode -- the signal a user otherwise cannot observe."""
        model = _large_scc_model(51)
        run = model.run(analyze_loops=True)
        assert run.ltm_mode == "discovery"

    def test_stays_exhaustive_just_below_threshold(self) -> None:
        model = _large_scc_model(49)
        run = model.run(analyze_loops=True)
        assert run.ltm_mode == "exhaustive"


class TestGetLinksCollapse:
    """get_links() collapses macro/module-internal synthetic nodes by default."""

    @staticmethod
    def _is_synthetic(name: str) -> bool:
        # Real model variables never start with `$`; every macro/module/LTM
        # internal carries the reserved synthetic prefix (`$` + U+205A).
        return name.startswith("$")

    def test_default_view_hides_synthetic_nodes(self) -> None:
        model = _smooth_feedback_model()
        with model.simulate(enable_ltm=True) as sim:
            sim.run_to_end()
            links = sim.get_links()  # include_internal defaults to False

        for link in links:
            assert not self._is_synthetic(link.from_var), (
                f"collapsed view leaked synthetic source {link.from_var!r}"
            )
            assert not self._is_synthetic(link.to_var), (
                f"collapsed view leaked synthetic target {link.to_var!r}"
            )

    def test_collapse_preserves_through_edge(self) -> None:
        """The chain `level -> <synthetic smth1 module> -> smoothed_level`
        collapses to one composite edge `level -> smoothed_level` -- the macro's
        contribution is preserved, not deleted (LTM ref 6.4)."""
        model = _smooth_feedback_model()
        with model.simulate(enable_ltm=True) as sim:
            sim.run_to_end()
            collapsed = sim.get_links()

        edges = {(lk.from_var, lk.to_var) for lk in collapsed}
        assert ("level", "smoothed_level") in edges, (
            f"composite level -> smoothed_level edge missing; got {sorted(edges)}"
        )

    def test_include_internal_exposes_raw_graph(self) -> None:
        """include_internal=True returns the raw graph with synthetic nodes,
        and there are strictly more edges than the collapsed view (the
        synthetic chain is split into halves)."""
        model = _smooth_feedback_model()
        with model.simulate(enable_ltm=True) as sim:
            sim.run_to_end()
            raw = sim.get_links(include_internal=True)
            collapsed = sim.get_links(include_internal=False)

        raw_nodes = {lk.from_var for lk in raw} | {lk.to_var for lk in raw}
        assert any(self._is_synthetic(n) for n in raw_nodes), (
            "raw view should expose at least one synthetic macro node"
        )
        # Collapsing the two-edge `level -> smth1 -> smoothed_level` chain (plus
        # dropping the dangling synthetic arg-helper edge) yields fewer edges.
        assert len(collapsed) < len(raw)

    def test_collapsed_composite_carries_score(self) -> None:
        """The collapsed edge through the macro carries a composite score
        series (the product/strongest-path score), not a dropped/None score."""
        model = _smooth_feedback_model()
        with model.simulate(enable_ltm=True) as sim:
            sim.run_to_end()
            collapsed = sim.get_links()

        through = next(
            lk for lk in collapsed if (lk.from_var, lk.to_var) == ("level", "smoothed_level")
        )
        assert through.score is not None
        assert len(through.score) > 0
