"""Tests for LTM loop score polarity.

These tests verify that the loop score polarity matches the LTM papers:
- Reinforcing (R) loops have POSITIVE loop scores
- Balancing (B) loops have NEGATIVE loop scores
"""

import math

import numpy as np

from simlin import Project
from simlin.analysis import POLARITY_CONFIDENCE_THRESHOLD, LoopPolarity


class TestLtmReinforcingLoop:
    """Test that reinforcing loops produce positive loop scores."""

    def test_exponential_growth_loop_has_positive_score(self) -> None:
        """Test that a simple exponential growth model produces positive loop scores.

        Model: population -> births -> population (reinforcing)
        births = population * birth_rate
        """
        # Create a project with a reinforcing loop
        project = Project.new(
            name="test_reinforcing",
            sim_start=0.0,
            sim_stop=5.0,
            dt=0.25,
        )

        model = project.main_model

        # Add the reinforcing loop structure using the edit context
        with model.edit() as (_current, patch):
            from simlin.json_types import Auxiliary, Flow, Stock

            patch.upsert_stock(
                Stock(
                    name="population",
                    initial_equation="100",
                    inflows=["births"],
                    outflows=[],
                )
            )
            patch.upsert_flow(
                Flow(
                    name="births",
                    equation="population * birth_rate",
                )
            )
            patch.upsert_aux(
                Auxiliary(
                    name="birth_rate",
                    equation="0.1",
                )
            )

        # Run simulation with LTM enabled
        run = model.run(analyze_loops=True)

        # Check that we have a reinforcing loop
        loops = run.loops
        assert len(loops) >= 1, "Should have at least one loop"

        r_loops = [lp for lp in loops if lp.polarity == LoopPolarity.REINFORCING]
        assert len(r_loops) >= 1, "Should have at least one reinforcing loop"

        # Get the absolute loop score for the reinforcing loop
        r_loop = r_loops[0]

        # Access the simulation to get the absolute loop score
        sim = model.simulate(enable_ltm=True)
        sim.run_to_end()

        loop_score_var = f"$\u205altm\u205aloop_score\u205a{r_loop.id}"
        loop_scores = sim.get_series(loop_score_var)

        # Filter out NaN and zero values (initial timesteps and equilibrium)
        valid_scores = [s for s in loop_scores if not math.isnan(s) and s != 0.0]

        assert len(valid_scores) > 0, "Should have some valid loop score values"

        # ALL valid scores for a reinforcing loop should be POSITIVE
        for score in valid_scores:
            assert score > 0.0, (
                f"Reinforcing loop score should be positive, got {score}. "
                f"All scores: {valid_scores}"
            )


class TestLtmBalancingLoop:
    """Test that balancing loops produce negative loop scores."""

    def test_goal_seeking_loop_has_negative_score(self) -> None:
        """Test that a goal-seeking model produces negative loop scores.

        Model: level -> gap -> adjustment -> level (balancing)
        gap = goal - level (negative polarity: level up -> gap down)
        adjustment = gap / adjustment_time
        """
        # Create a project with a balancing loop
        project = Project.new(
            name="test_balancing",
            sim_start=0.0,
            sim_stop=5.0,
            dt=0.25,
        )

        model = project.main_model

        # Add the balancing loop structure
        with model.edit() as (_current, patch):
            from simlin.json_types import Auxiliary, Flow, Stock

            patch.upsert_aux(
                Auxiliary(
                    name="goal",
                    equation="100",
                )
            )
            patch.upsert_stock(
                Stock(
                    name="level",
                    initial_equation="50",
                    inflows=["adjustment"],
                    outflows=[],
                )
            )
            patch.upsert_aux(
                Auxiliary(
                    name="gap",
                    equation="goal - level",
                )
            )
            patch.upsert_aux(
                Auxiliary(
                    name="adjustment_time",
                    equation="5",
                )
            )
            patch.upsert_flow(
                Flow(
                    name="adjustment",
                    equation="gap / adjustment_time",
                )
            )

        # Run simulation with LTM enabled
        run = model.run(analyze_loops=True)

        # Check that we have a balancing loop
        loops = run.loops
        assert len(loops) >= 1, "Should have at least one loop"

        b_loops = [lp for lp in loops if lp.polarity == LoopPolarity.BALANCING]
        assert len(b_loops) >= 1, "Should have at least one balancing loop"

        # Get the absolute loop score for the balancing loop
        b_loop = b_loops[0]

        # Access the simulation to get the absolute loop score
        sim = model.simulate(enable_ltm=True)
        sim.run_to_end()

        loop_score_var = f"$\u205altm\u205aloop_score\u205a{b_loop.id}"
        loop_scores = sim.get_series(loop_score_var)

        # Filter out NaN and zero values (initial timesteps and equilibrium)
        valid_scores = [s for s in loop_scores if not math.isnan(s) and s != 0.0]

        assert len(valid_scores) > 0, "Should have some valid loop score values"

        # ALL valid scores for a balancing loop should be NEGATIVE
        for score in valid_scores:
            assert score < 0.0, (
                f"Balancing loop score should be negative, got {score}. All scores: {valid_scores}"
            )


class TestLtmModuleBoundaryReclassification:
    """GH #679 regression guard: `Run.loops` already reclassifies an
    exhaustive-mode loop whose static polarity is Undetermined from its runtime
    loop-score series (this is pre-existing pysimlin behavior, predating the
    GH #679 engine work).  This test pins the module-boundary case that GH #679
    called out -- a loop through a module whose static polarity is Unknown --
    so the existing Python reclassification cannot silently regress."""

    def test_module_loop_undetermined_structural_reclassifies_to_reinforcing(
        self,
    ) -> None:
        """A loop that runs `s -> SMTH1(s) -> parabola(effect) -> s` is
        structurally Undetermined (the parabola makes the smoothed-output ->
        effect link Unknown), but the simulation stays on the rising arm of
        the parabola so the runtime loop score is single-signed positive.

        `Model.loops` (pre-simulation, structural) reports `U`; `Run.loops`
        (post-simulation) must report `R`, with the loop id unchanged."""
        project = Project.new(
            name="module_parabola",
            sim_start=0.0,
            sim_stop=8.0,
            dt=1.0,
        )
        model = project.main_model
        with model.edit() as (_current, patch):
            from simlin.json_types import Auxiliary, Flow, Stock

            patch.upsert_stock(
                Stock(name="s", initial_equation="100", inflows=["growth"], outflows=[])
            )
            patch.upsert_aux(Auxiliary(name="smoothed", equation="SMTH1(s, 3)"))
            # A parabola in the smoothed output: the smoothed value appears
            # with conflicting signs, so the static analyzer cannot sign the
            # smoothed -> effect link and the loop is structurally Undetermined.
            patch.upsert_aux(
                Auxiliary(
                    name="effect",
                    equation="smoothed * (1000 - smoothed) / 100000",
                )
            )
            patch.upsert_flow(Flow(name="growth", equation="effect"))

        # The structural surface (no runtime data) classifies the loop as U.
        structural = model.loops
        assert len(structural) == 1, "expected exactly one feedback loop"
        assert structural[0].polarity == LoopPolarity.UNDETERMINED, (
            "static polarity through the parabola-consumed module output is "
            f"Undetermined, got {structural[0].polarity}"
        )
        structural_id = structural[0].id

        run = model.run(analyze_loops=True)
        assert run.ltm_mode == "exhaustive"
        loops = run.loops
        assert len(loops) == 1, "expected exactly one behavioral loop"
        loop = loops[0]
        assert loop.polarity == LoopPolarity.REINFORCING, (
            "the runtime loop score is single-signed positive, so the loop must "
            f"reclassify to Reinforcing, not stay Undetermined; got {loop.polarity}"
        )
        assert loop.id == structural_id, (
            "the loop id must stay stable across runtime reclassification "
            f"(structural {structural_id} vs behavioral {loop.id})"
        )


class TestRuntimeLoopsFfiAgreesWithPython:
    """GH #679/#685: `Run.loops` is the single authoritative runtime loop
    surface -- its polarity / confidence / partition come from the engine's
    `reclassify_loops_from_results` primitive (bound as `Sim.get_loops_runtime`,
    the all-slots Rust source of truth), with `behavior_time_series` attached on
    top.  These tests pin that `Run.loops` reports the engine's runtime
    classification (not the static structural one) and that the low-level
    `Sim.get_loops_runtime` primitive it builds on still behaves."""

    def _sign_flipping_logistic(self) -> Project:
        """A logistic loop whose runtime loop_score straddles zero: the
        `stock -> net` link is reinforcing while `stock < K/2` and balancing
        once it saturates, so the static analyzer cannot sign it (structurally
        Undetermined) but the simulation expresses both signs."""
        project = Project.new(
            name="logistic_flip",
            sim_start=0.0,
            sim_stop=20.0,
            dt=0.125,
        )
        model = project.main_model
        with model.edit() as (_current, patch):
            from simlin.json_types import Auxiliary, Flow, Stock

            patch.upsert_aux(Auxiliary(name="r", equation="0.8"))
            patch.upsert_aux(Auxiliary(name="k", equation="1000"))
            patch.upsert_stock(
                Stock(name="stock", initial_equation="100", inflows=["net"], outflows=[])
            )
            patch.upsert_flow(Flow(name="net", equation="r * stock * (1 - stock / k)"))
        return project

    def test_run_loops_match_sim_primitive_on_scalar_loop(self) -> None:
        """For a scalar sign-flipping loop, `Run.loops` must report the same
        polarity / confidence / id as the engine primitive it is built on
        (`Sim.get_loops_runtime`); `Run.loops` adds only the behavior series."""
        project = self._sign_flipping_logistic()
        model = project.main_model

        run = model.run(analyze_loops=True)
        assert run.ltm_mode == "exhaustive"

        run_loops = run.loops
        primitive_loops = run._sim.get_loops_runtime()
        assert len(run_loops) == 1, "logistic model has one feedback loop"
        assert len(primitive_loops) == 1, "primitive returns the same single loop"

        run_loop = run_loops[0]
        prim_loop = primitive_loops[0]

        assert run_loop.id == prim_loop.id, (
            "loop id must be stable between Run.loops and the engine primitive "
            f"(run {run_loop.id} vs primitive {prim_loop.id})"
        )
        # Run.loops sources polarity straight from the engine primitive, so the
        # label and confidence must match it exactly.
        assert run_loop.polarity == prim_loop.polarity, (
            "Run.loops polarity must equal the engine primitive's "
            f"({run_loop.polarity} vs {prim_loop.polarity})"
        )
        assert run_loop.polarity_confidence == prim_loop.polarity_confidence
        # ...and unlike the primitive, Run.loops carries the behavior series.
        assert run_loop.behavior_time_series is not None

    def test_run_loops_differs_from_structural(self) -> None:
        """`Run.loops` must report a label/confidence the STRUCTURAL surface
        (`Model.loops`) cannot: the structural loop is Undetermined at
        confidence 0.0, the runtime one carries a genuine dominance ratio."""
        project = self._sign_flipping_logistic()
        model = project.main_model

        structural = model.loops
        assert len(structural) == 1
        assert structural[0].polarity == LoopPolarity.UNDETERMINED
        assert structural[0].polarity_confidence == 0.0

        run = model.run(analyze_loops=True)
        run_loop = run.loops[0]
        # A strictly-intermediate confidence is only reachable when the runtime
        # loop_score series carries both signs -- evidence of the sign flip.
        assert 0.0 < run_loop.polarity_confidence < 1.0, (
            "the sign-straddling loop must reclassify to a strictly intermediate "
            f"confidence, got {run_loop.polarity_confidence}"
        )

    def test_runtime_loops_empty_without_ltm(self) -> None:
        """When LTM is disabled the loop_score series are absent, so the engine
        primitive leaves the structural loops untouched; the FFI still returns
        them (the structural set) rather than erroring."""
        project = self._sign_flipping_logistic()
        sim = project.main_model.simulate(enable_ltm=False)
        sim.run_to_end()
        # No LTM -> no loop_score columns -> structural classification preserved.
        loops = sim.get_loops_runtime()
        assert len(loops) == 1
        assert loops[0].polarity == LoopPolarity.UNDETERMINED

    def test_run_loops_arrayed_uses_engine_all_slots_classification(self) -> None:
        """On an ARRAYED (A2A) model, `Run.loops` must report the engine's
        all-slots runtime classification, not a slot-0-only one.

        This pins the GH #679/#685 consolidation: `Run.loops` sources polarity /
        confidence / partition from the engine primitive
        (`Sim.get_loops_runtime`), which concatenates ALL element slots of an
        arrayed loop's `loop_score` before classifying.  Before the
        consolidation `Run.loops` reclassified off slot 0 only, which could
        disagree with the engine on a sign-heterogeneous arrayed loop; now the
        two surfaces are the same source of truth by construction.
        """
        import os
        from pathlib import Path

        import simlin

        repo_root = (
            Path(os.environ["SIMLIN_REPO_ROOT"])
            if "SIMLIN_REPO_ROOT" in os.environ
            else Path(__file__).parent.parent.parent.parent
        )
        fixture_path = repo_root / "test" / "arrayed_population_ltm" / "arrayed_population.stmx"
        if not fixture_path.exists():
            import pytest

            pytest.skip(f"arrayed fixture missing at {fixture_path}")

        model = simlin.load(fixture_path)
        run = model.run(analyze_loops=True)

        run_loops = run.loops
        assert run_loops, "arrayed_population should expose runtime loops"

        # The engine all-slots primitive is the source of truth; Run.loops must
        # match it loop-for-loop on polarity / confidence / partition, adding
        # only the behavior series on top.
        primitive_by_id = {lp.id: lp for lp in run._sim.get_loops_runtime()}
        assert primitive_by_id, "engine primitive should return the same loops"

        arrayed_seen = False
        for loop in run_loops:
            assert loop.id in primitive_by_id, (
                f"Run.loops id {loop.id} not present in the engine primitive set"
            )
            prim = primitive_by_id[loop.id]
            assert loop.polarity == prim.polarity, (
                f"Run.loops polarity for {loop.id} must equal the engine all-slots "
                f"classification ({loop.polarity} vs {prim.polarity})"
            )
            assert loop.polarity_confidence == prim.polarity_confidence
            assert loop.partition == prim.partition
            # The behavior series is attached on top of the engine polarity.
            assert loop.behavior_time_series is not None
            if run._sim.get_loop_element_count(loop.id) > 1:
                arrayed_seen = True

        assert arrayed_seen, "expected at least one arrayed (multi-slot) loop"


class TestLtmUndeterminedPolarity:
    """Test the from_runtime_scores classification method."""

    def test_from_runtime_scores_undetermined(self) -> None:
        """Mixed-sign scores below the confidence threshold are UNDETERMINED."""
        # |6 - 3| / (6 + 3) = 0.333 -> below threshold -> UNDETERMINED.
        mixed_scores = np.array([float("nan"), 1.0, 2.0, -1.0, -2.0, 3.0])
        polarity = LoopPolarity.from_runtime_scores(mixed_scores)
        assert polarity == LoopPolarity.UNDETERMINED

    def test_from_runtime_scores_undetermined_symmetric(self) -> None:
        """Equal-magnitude positive and negative scores are UNDETERMINED."""
        # |3 - 3| / (3 + 3) = 0.0 -> well below threshold.
        symmetric_scores = np.array([1.0, 2.0, -1.0, -2.0])
        polarity = LoopPolarity.from_runtime_scores(symmetric_scores)
        assert polarity == LoopPolarity.UNDETERMINED

    def test_from_runtime_scores_reinforcing(self) -> None:
        """Test that from_runtime_scores correctly classifies all-positive scores."""
        positive_scores = np.array([float("nan"), 1.0, 2.0, 3.0, 0.5])
        polarity = LoopPolarity.from_runtime_scores(positive_scores)
        assert polarity == LoopPolarity.REINFORCING

    def test_from_runtime_scores_balancing(self) -> None:
        """Test that from_runtime_scores correctly classifies all-negative scores."""
        negative_scores = np.array([float("nan"), -1.0, -2.0, -3.0, -0.5])
        polarity = LoopPolarity.from_runtime_scores(negative_scores)
        assert polarity == LoopPolarity.BALANCING

    def test_from_runtime_scores_mostly_reinforcing(self) -> None:
        """Mostly-positive scores with a tiny negative dip cross the threshold."""
        # |6.5 - 0.02| / (6.5 + 0.02) ~= 0.9939 -> above 0.99 threshold.
        scores = np.array([1.0, 1.5, 2.0, 0.5, -0.02, 1.5])
        polarity = LoopPolarity.from_runtime_scores(scores)
        assert polarity == LoopPolarity.MOSTLY_REINFORCING

    def test_from_runtime_scores_mostly_balancing(self) -> None:
        """Mostly-negative scores with a tiny positive blip cross the threshold."""
        # |0.02 - 6.5| / (0.02 + 6.5) ~= 0.9939 -> above 0.99 threshold,
        # negative dominant -> MOSTLY_BALANCING.
        scores = np.array([-1.0, -1.5, -2.0, -0.5, 0.02, -1.5])
        polarity = LoopPolarity.from_runtime_scores(scores)
        assert polarity == LoopPolarity.MOSTLY_BALANCING

    def test_from_runtime_scores_weakly_dominant_undetermined(self) -> None:
        """One-side dominance below the threshold stays UNDETERMINED."""
        # |3 - 1| / (3 + 1) = 0.5 -> below 0.99 threshold despite the
        # positive sum exceeding the negative sum.
        scores = np.array([3.0, -1.0])
        polarity = LoopPolarity.from_runtime_scores(scores)
        assert polarity == LoopPolarity.UNDETERMINED

    def test_from_runtime_scores_all_nan(self) -> None:
        """Test that from_runtime_scores returns None for all-NaN scores."""
        nan_scores = np.array([float("nan"), float("nan"), float("nan")])
        polarity = LoopPolarity.from_runtime_scores(nan_scores)
        assert polarity is None

    def test_from_runtime_scores_all_zero(self) -> None:
        """Test that from_runtime_scores returns None for all-zero scores."""
        zero_scores = np.array([0.0, 0.0, 0.0])
        polarity = LoopPolarity.from_runtime_scores(zero_scores)
        assert polarity is None

    def test_polarity_confidence_threshold_value(self) -> None:
        """The Python-side threshold must match the Rust constant."""
        assert POLARITY_CONFIDENCE_THRESHOLD == 0.99


class TestLoopPolarityEnum:
    """Test the LoopPolarity enum."""

    def test_polarity_string_representation(self) -> None:
        """Test that polarity string representation is correct."""
        assert str(LoopPolarity.REINFORCING) == "R"
        assert str(LoopPolarity.BALANCING) == "B"
        assert str(LoopPolarity.UNDETERMINED) == "U"
        assert str(LoopPolarity.MOSTLY_REINFORCING) == "Rux"
        assert str(LoopPolarity.MOSTLY_BALANCING) == "Bux"

    def test_polarity_values(self) -> None:
        """Test that polarity integer values match the C FFI for all five
        variants.

        Since GH #495 the FFI surfaces all five `SimlinLoopPolarity` variants
        1:1 (no coalescing); these integer values mirror it exactly so
        `LoopPolarity(c_loop.polarity)` round-trips a Rux/Bux loop.
        """
        assert LoopPolarity.REINFORCING == 0
        assert LoopPolarity.BALANCING == 1
        assert LoopPolarity.UNDETERMINED == 2
        assert LoopPolarity.MOSTLY_REINFORCING == 3
        assert LoopPolarity.MOSTLY_BALANCING == 4


class TestStructuralPolarityClassification:
    """Test structural polarity classification via the engine.

    These tests verify that the conservative polarity classification works:
    all links must have known polarity for the loop to be classified as
    Reinforcing or Balancing.
    """

    def test_all_known_polarities_reinforcing(self) -> None:
        """Test that a loop with all known positive polarities is Reinforcing.

        Model: population -> births -> population (reinforcing)
        All links have known positive polarity.
        """
        project = Project.new(
            name="test_all_known_reinforcing",
            sim_start=0.0,
            sim_stop=5.0,
            dt=0.25,
        )

        model = project.main_model

        with model.edit() as (_current, patch):
            from simlin.json_types import Auxiliary, Flow, Stock

            patch.upsert_stock(
                Stock(
                    name="population",
                    initial_equation="100",
                    inflows=["births"],
                    outflows=[],
                )
            )
            patch.upsert_flow(
                Flow(
                    name="births",
                    equation="population * birth_rate",
                )
            )
            patch.upsert_aux(
                Auxiliary(
                    name="birth_rate",
                    equation="0.1",
                )
            )

        run = model.run(analyze_loops=True)
        loops = run.loops

        assert len(loops) >= 1, "Should have at least one loop"
        r_loops = [lp for lp in loops if lp.polarity == LoopPolarity.REINFORCING]
        assert len(r_loops) >= 1, "Simple exponential growth should have a Reinforcing loop"

    def test_all_known_polarities_balancing(self) -> None:
        """Test that a goal-seeking model has a Balancing loop.

        Model: level -> gap -> adjustment -> level (balancing)
        The gap equation (goal - level) introduces negative polarity.
        """
        project = Project.new(
            name="test_all_known_balancing",
            sim_start=0.0,
            sim_stop=5.0,
            dt=0.25,
        )

        model = project.main_model

        with model.edit() as (_current, patch):
            from simlin.json_types import Auxiliary, Flow, Stock

            patch.upsert_aux(
                Auxiliary(
                    name="goal",
                    equation="100",
                )
            )
            patch.upsert_stock(
                Stock(
                    name="level",
                    initial_equation="50",
                    inflows=["adjustment"],
                    outflows=[],
                )
            )
            patch.upsert_aux(
                Auxiliary(
                    name="gap",
                    equation="goal - level",
                )
            )
            patch.upsert_aux(
                Auxiliary(
                    name="adjustment_time",
                    equation="5",
                )
            )
            patch.upsert_flow(
                Flow(
                    name="adjustment",
                    equation="gap / adjustment_time",
                )
            )

        run = model.run(analyze_loops=True)
        loops = run.loops

        assert len(loops) >= 1, "Should have at least one loop"
        b_loops = [lp for lp in loops if lp.polarity == LoopPolarity.BALANCING]
        assert len(b_loops) >= 1, "Goal-seeking model should have a Balancing loop"


class TestPolarityConfidence:
    """GH #495: every loop carries a `polarity_confidence` ratio in [0, 1]
    threaded from the FFI, and a clean single-signed loop reports 1.0."""

    def _reinforcing_model(self) -> Project:
        project = Project.new(
            name="test_confidence",
            sim_start=0.0,
            sim_stop=5.0,
            dt=0.25,
        )
        model = project.main_model
        with model.edit() as (_current, patch):
            from simlin.json_types import Auxiliary, Flow, Stock

            patch.upsert_stock(
                Stock(
                    name="population",
                    initial_equation="100",
                    inflows=["births"],
                    outflows=[],
                )
            )
            patch.upsert_flow(Flow(name="births", equation="population * birth_rate"))
            patch.upsert_aux(Auxiliary(name="birth_rate", equation="0.1"))
        return project

    def test_structural_loops_carry_confidence_in_range(self) -> None:
        """Structural `Model.loops` populate `polarity_confidence` in [0, 1];
        a fully-signed loop reports 1.0 (the structural convention)."""
        project = self._reinforcing_model()
        loops = project.main_model.loops
        assert len(loops) >= 1
        for loop in loops:
            assert isinstance(loop.polarity_confidence, float)
            assert 0.0 <= loop.polarity_confidence <= 1.0
        r_loops = [lp for lp in loops if lp.polarity == LoopPolarity.REINFORCING]
        assert len(r_loops) >= 1
        assert r_loops[0].polarity_confidence == 1.0, (
            "a fully-signed reinforcing loop has structural confidence 1.0"
        )

    def test_discovery_loops_carry_confidence_in_range(self) -> None:
        """Behavioral `Run.loops` populate `polarity_confidence` in [0, 1];
        a single-signed reinforcing loop reports full confidence."""
        project = self._reinforcing_model()
        run = project.main_model.run(analyze_loops=True)
        loops = run.loops
        assert len(loops) >= 1
        for loop in loops:
            assert isinstance(loop.polarity_confidence, float)
            assert 0.0 <= loop.polarity_confidence <= 1.0
        r_loops = [lp for lp in loops if lp.polarity == LoopPolarity.REINFORCING]
        assert len(r_loops) >= 1
        assert r_loops[0].polarity_confidence == 1.0, (
            "a single-signed reinforcing loop scores confidence 1.0"
        )
