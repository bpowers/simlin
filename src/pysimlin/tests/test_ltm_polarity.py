"""Tests for LTM loop score polarity.

These tests verify that the loop score polarity matches the LTM papers:
- Reinforcing (R) loops have POSITIVE loop scores
- Balancing (B) loops have NEGATIVE loop scores
"""

import math

import numpy as np

from simlin import Project
from simlin.analysis import LoopPolarity


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


class TestLtmUndeterminedPolarity:
    """Test the from_runtime_scores classification method."""

    def test_from_runtime_scores_undetermined(self) -> None:
        """Test that from_runtime_scores correctly classifies mixed-sign scores."""
        # Create an array with both positive and negative values
        mixed_scores = np.array([float("nan"), 1.0, 2.0, -1.0, -2.0, 3.0])
        polarity = LoopPolarity.from_runtime_scores(mixed_scores)
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


class TestLoopPolarityEnum:
    """Test the LoopPolarity enum."""

    def test_polarity_string_representation(self) -> None:
        """Test that polarity string representation is correct."""
        assert str(LoopPolarity.REINFORCING) == "R"
        assert str(LoopPolarity.BALANCING) == "B"
        assert str(LoopPolarity.UNDETERMINED) == "U"

    def test_polarity_values(self) -> None:
        """Test that polarity integer values match FFI."""
        assert LoopPolarity.REINFORCING == 0
        assert LoopPolarity.BALANCING == 1
        assert LoopPolarity.UNDETERMINED == 2


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
