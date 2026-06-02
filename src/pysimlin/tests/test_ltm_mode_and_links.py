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


class TestRunLtmDegradation:
    """Model.run() degrades gracefully when LTM cannot be enabled, and explains
    discovery mode instead of silently returning an empty loop list."""

    def test_run_falls_back_when_ltm_compile_fails(
        self, logistic_model: simlin.Model, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """When the LTM-instrumented compile fails (e.g. a very large model
        exceeding the engine's bytecode slot limit), run() must warn, retry
        without LTM, and still return correct results -- not raise."""
        from simlin.errors import SimlinRuntimeError

        real_simulate = simlin.Model.simulate

        def failing_ltm_simulate(
            self: simlin.Model,
            overrides: dict[str, float] | None = None,
            enable_ltm: bool = False,
        ) -> simlin.Sim:
            if enable_ltm:
                raise SimlinRuntimeError(
                    "Create simulation failed: model 'main' requires 171498 result "
                    "slots, which exceeds the bytecode VM's addressable limit"
                )
            return real_simulate(self, overrides=overrides, enable_ltm=enable_ltm)

        monkeypatch.setattr(simlin.Model, "simulate", failing_ltm_simulate)

        with pytest.warns(RuntimeWarning, match="loop analysis"):
            run = logistic_model.run(analyze_loops=True)

        # The fallback run is a real, correct simulation without LTM.
        assert run.ltm_mode == "disabled"
        assert not run.results.empty

    def test_run_falls_back_when_ltm_run_to_end_fails(
        self, logistic_model: simlin.Model, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """The engine defers compile errors from sim creation to run time
        (sim_new stores them as vm_error; run_to_end reports them) -- this is
        exactly how the too-many-result-slots error surfaces on C-LEARN. The
        fallback must cover that path, not just simulate()-time failures."""
        from simlin.errors import SimlinRuntimeError

        real_run_to_end = simlin.Sim.run_to_end
        calls = {"n": 0}

        def failing_first_run(self: simlin.Sim) -> None:
            calls["n"] += 1
            if calls["n"] == 1:
                raise SimlinRuntimeError(
                    "Run simulation to end failed: model 'main' requires 171597 "
                    "result slots, which exceeds the bytecode VM's addressable limit"
                )
            real_run_to_end(self)

        monkeypatch.setattr(simlin.Sim, "run_to_end", failing_first_run)

        with pytest.warns(RuntimeWarning, match="loop analysis"):
            run = logistic_model.run(analyze_loops=True)

        assert run.ltm_mode == "disabled"
        assert not run.results.empty
        assert calls["n"] == 2, "the run must have been retried without LTM"

    def test_run_does_not_swallow_non_ltm_failures(
        self, logistic_model: simlin.Model, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        """A model that cannot compile at all (with or without LTM) must still
        raise -- the fallback only covers LTM-specific failures."""
        from simlin.errors import SimlinRuntimeError

        def always_failing_simulate(
            self: simlin.Model,
            overrides: dict[str, float] | None = None,
            enable_ltm: bool = False,
        ) -> simlin.Sim:
            raise SimlinRuntimeError("model is broken")

        monkeypatch.setattr(simlin.Model, "simulate", always_failing_simulate)

        with pytest.raises(SimlinRuntimeError, match="model is broken"):
            logistic_model.run(analyze_loops=True)

    def test_run_warns_on_discovery_mode_with_no_loops(self) -> None:
        """A model that auto-flips to discovery mode (and has no pinned loops)
        gets a warning explaining why run.loops is empty and what to do about
        it; without the warning, an empty loop list is indistinguishable from
        a loop-free model."""
        model = _large_scc_model(51)
        with pytest.warns(RuntimeWarning, match="discovery"):
            run = model.run(analyze_loops=True)
        assert run.ltm_mode == "discovery"
        assert run.loops == ()

    def test_run_does_not_warn_in_exhaustive_mode(self, logistic_model: simlin.Model) -> None:
        """Small models (exhaustive mode) run without any warning."""
        import warnings as warnings_module

        with warnings_module.catch_warnings():
            warnings_module.simplefilter("error")  # any warning -> test failure
            run = logistic_model.run(analyze_loops=True)
        assert run.ltm_mode == "exhaustive"

    def test_run_does_not_warn_when_loops_disabled(self) -> None:
        """analyze_loops=False on a discovery-scale model is silent: the user
        explicitly opted out of loop analysis."""
        import warnings as warnings_module

        model = _large_scc_model(51)
        with warnings_module.catch_warnings():
            warnings_module.simplefilter("error")
            run = model.run(analyze_loops=False)
        assert run.ltm_mode == "disabled"

    def test_run_rejects_removed_time_range_params(self, logistic_model: simlin.Model) -> None:
        """run() no longer accepts time_range/dt: they were silently ignored
        (accepted but never applied), which is worse than a loud TypeError.
        Use project.set_sim_specs() to change simulation time bounds."""
        with pytest.raises(TypeError):
            logistic_model.run(time_range=(0.0, 5.0))  # type: ignore[call-arg]
        with pytest.raises(TypeError):
            logistic_model.run(dt=0.5)  # type: ignore[call-arg]


class TestLtmDiagnosticsThroughGetErrors:
    """GH #466: LTM diagnostics (the auto-flip-to-discovery warning et al.) are
    reachable through project.get_errors() once a simulation has been created
    with enable_ltm=True, and stay hidden otherwise.

    pysimlin's project.get_errors() routes straight through the
    simlin_project_get_errors FFI, and model.simulate(enable_ltm=True) routes
    through simlin_sim_new(enable_ltm=true), so the binding inherits the fix
    end to end without any pysimlin-side plumbing.
    """

    @staticmethod
    def _has_discovery_warning(details: list[simlin.ErrorDetail]) -> bool:
        return any("discovery mode" in (d.message or "") for d in details)

    def test_auto_flip_warning_surfaces_after_ltm_simulate(self) -> None:
        model = _large_scc_model(51)
        project = model.project
        assert project is not None

        # Before any LTM sim, no LTM diagnostic should surface.
        assert not self._has_discovery_warning(project.get_errors())

        # Creating an LTM-enabled sim records that LTM was requested.
        with model.simulate(enable_ltm=True):
            pass

        assert self._has_discovery_warning(project.get_errors()), (
            "auto-flip 'discovery mode' warning must reach project.get_errors() "
            "after an enable_ltm=True simulation"
        )

    def test_no_ltm_diagnostics_without_ltm_request(self) -> None:
        model = _large_scc_model(51)
        project = model.project
        assert project is not None

        # A non-LTM sim must not make the LTM warning appear.
        with model.simulate(enable_ltm=False):
            pass

        assert not self._has_discovery_warning(project.get_errors()), (
            "no LTM diagnostics when LTM was never requested"
        )

    def test_warning_surfaces_via_run_with_loop_analysis(self) -> None:
        """The high-level Model.run(analyze_loops=True) path also creates an
        LTM sim, so the auto-flip warning becomes reachable through
        project.get_errors() afterward."""
        model = _large_scc_model(51)
        project = model.project
        assert project is not None

        with pytest.warns(RuntimeWarning, match="discovery"):
            model.run(analyze_loops=True)

        assert self._has_discovery_warning(project.get_errors())


def _rk4_feedback_model() -> simlin.Model:
    """Build a single-stock feedback-loop model that uses RK4 integration.

    The model simulates fine without LTM, but an LTM-enabled compile is rejected
    (the flow-to-stock link-score formula assumes Euler -- GH #486).
    """
    project = simlin.Project.new(name="rk4_feedback", sim_start=0.0, sim_stop=10.0, dt=1.0)
    project.set_sim_specs(sim_method="rk4")
    model = project.main_model
    with model.edit() as (_current, patch):
        from simlin.json_types import Auxiliary, Flow, Stock

        patch.upsert_aux(Auxiliary(name="birth_rate", equation="0.02"))
        patch.upsert_stock(
            Stock(name="population", initial_equation="100", inflows=["births"], outflows=[])
        )
        patch.upsert_flow(Flow(name="births", equation="population * birth_rate"))
    return model


class TestLtmOverlayNotProjectError:
    """GH #466 follow-up: LTM is an analysis overlay, not part of the project's
    intrinsic validity. A latched LTM run on an RK4 model (whose LTM compile the
    GH #486 guard rejects) must NOT make get_errors()/check() report the model as
    broken -- it simulates fine without LTM. This is the reviewer's exact repro.
    """

    def test_rk4_run_then_get_errors_is_clean(self) -> None:
        model = _rk4_feedback_model()
        project = model.project
        assert project is not None

        # Baseline: a fresh RK4 model has no errors.
        assert project.get_errors() == []

        # run() defaults to analyze_loops=True -> LTM sim -> ltm_requested latch.
        # The LTM compile fails under RK4, so run() degrades to a non-LTM run
        # (emitting a warning); the run itself succeeds.
        with pytest.warns(RuntimeWarning):
            run = model.run()
        assert not run.results.empty

        # The regression: the RK4 model still reports no errors. The non-Euler
        # rejection is an LTM-overlay concern, not a project error.
        assert project.get_errors() == [], (
            "a latched LTM run on an RK4 model that simulates fine must not make "
            "get_errors report the non-Euler rejection as a project error"
        )

    def test_rk4_run_then_check_reports_no_errors(self) -> None:
        model = _rk4_feedback_model()

        with pytest.warns(RuntimeWarning):
            model.run()

        issues = model.check()
        error_issues = [i for i in issues if i.severity == "error"]
        assert error_issues == [], (
            f"check() must report no error-severity issues on a fine RK4 model "
            f"after an LTM run; got {error_issues}"
        )


class TestLtmAdvisorySeverity:
    """GH #466 follow-up (severity): the auto-flip advisory that get_errors now
    surfaces must carry 'warning' severity through check(), not 'error'.
    """

    def test_auto_flip_advisory_has_warning_severity(self) -> None:
        model = _large_scc_model(51)
        project = model.project
        assert project is not None

        # The advisory is a warning, not an error, at the ErrorDetail level.
        with model.simulate(enable_ltm=True):
            pass
        details = project.get_errors()
        advisory = next(d for d in details if "discovery mode" in (d.message or ""))
        assert advisory.severity == simlin.ErrorSeverity.WARNING

    def test_auto_flip_advisory_check_severity_is_warning(self) -> None:
        model = _large_scc_model(51)

        # Drive an LTM run so the advisory is latched and reachable.
        with pytest.warns(RuntimeWarning, match="discovery"):
            model.run(analyze_loops=True)

        issues = model.check()
        advisory = next(i for i in issues if "discovery mode" in i.message)
        assert advisory.severity == "warning", (
            "the auto-flip advisory must surface as a warning in check(), not an error"
        )
        # And it must not be misclassified as an error-severity issue.
        assert all(i.severity != "error" for i in issues if "discovery mode" in i.message)
