"""Simulation run results and analysis."""

from __future__ import annotations

import contextlib
from dataclasses import dataclass
from typing import TYPE_CHECKING

import numpy as np
import pandas as pd

from .analysis import Loop, LoopPolarity
from .errors import SimlinRuntimeError
from .types import TimeSpec

if TYPE_CHECKING:
    from numpy.typing import NDArray

    from .sim import Sim


@dataclass(frozen=True)
class DominantPeriod:
    """Time period where specific loops dominate model behavior.

    DominantPeriod represents a contiguous time interval during which
    a specific set of feedback loops collectively explain the majority
    of the model's behavior. These periods are identified by the loop
    dominance analysis algorithm (Loops That Matter / LTM).
    """

    dominant_loops: tuple[str, ...]
    """Loop IDs that dominate during this period (e.g., ('R1', 'B2', 'U3'))"""

    start_time: float
    """Period start time"""

    end_time: float
    """Period end time"""

    def duration(self) -> float:
        """Calculate the duration of this period.

        Returns:
            Duration in time units (end_time - start_time)
        """
        return self.end_time - self.start_time

    def contains_loop(self, loop_id: str) -> bool:
        """Check if a specific loop dominates during this period.

        Args:
            loop_id: Loop identifier to check (e.g., 'R1', 'B2', 'U3')

        Returns:
            True if the loop is in dominant_loops, False otherwise
        """
        return loop_id in self.dominant_loops


class Run:
    """Results and analysis from a single simulation run.

    This class bundles together time series data, loop analysis results,
    and metadata from a single simulation execution. It is returned by
    model.run() and model.base_case.

    Run objects provide access to:
    - Time series results for all variables (as pandas DataFrame)
    - Feedback loop analysis including behavioral importance over time
    - Dominant periods showing which loops drive behavior in each time interval
    - Metadata about the simulation configuration (overrides, time spec)

    Use standard pandas/numpy/matplotlib operations to analyze and visualize
    the results rather than custom methods. This makes Run objects composable
    with the broader Python data science ecosystem.

    Example:
        >>> model = simlin.load("model.stmx")
        >>> run = model.base_case
        >>> run.results["population"].plot()
        >>> print(f"Final value: {run.results['population'].iloc[-1]}")
        >>>
        >>> # Analyze loop dominance
        >>> for period in run.dominant_periods:
        ...     print(f"{period.start_time}-{period.end_time}: {period.dominant_loops}")
        >>>
        >>> # Compare scenarios
        >>> import pandas as pd
        >>> policy_run = model.run(overrides={"tax_rate": 0.3})
        >>> comparison = pd.DataFrame(
        ...     {"baseline": run.results["gdp"], "policy": policy_run.results["gdp"]}
        ... )
        >>> comparison.plot()
    """

    def __init__(
        self,
        sim: Sim,
        overrides: dict[str, float],
    ) -> None:
        """Initialize a Run from completed simulation.

        Args:
            sim: Completed Sim instance with results
            overrides: Variable overrides used for this run
        """
        self._sim = sim
        self._overrides = overrides
        self._cached_results: pd.DataFrame | None = None
        self._cached_loops: tuple[Loop, ...] | None = None
        self._cached_dominant_periods: tuple[DominantPeriod, ...] | None = None
        self._cached_time_spec: TimeSpec | None = None

    @property
    def results(self) -> pd.DataFrame:
        """Time series results as a pandas DataFrame.

        Index is simulation time. Columns are variable names.
        For arrayed variables, columns are named like "var[element]".

        Use standard pandas methods for analysis and visualization.

        Returns:
            DataFrame with time as index and variables as columns

        Example:
            >>> run.results["population"].plot()
            >>> print(run.results["population"].describe())
            >>> final_pop = run.results["population"].iloc[-1]
        """
        if self._cached_results is None:
            self._cached_results = self._build_results_dataframe()
        return self._cached_results

    def _build_results_dataframe(self) -> pd.DataFrame:
        """Build the results DataFrame from simulation data."""
        variables = self._sim.get_var_names()
        step_count = self._sim.get_step_count()

        if step_count <= 0:
            return pd.DataFrame()

        try:
            time_series = self._sim.get_series("time")
        except SimlinRuntimeError:
            time_series = np.arange(step_count, dtype=np.float64)

        data: dict[str, NDArray[np.float64]] = {}

        for var_name in variables:
            if var_name.lower() == "time":
                continue
            with contextlib.suppress(SimlinRuntimeError):
                data[var_name] = self._sim.get_series(var_name)

        df = pd.DataFrame(data, index=time_series)
        df.index.name = "time"

        return df

    @property
    def loops(self) -> tuple[Loop, ...]:
        """Feedback loops with RUNTIME polarity and behavior time series.

        This is the single authoritative runtime loop surface.  Each loop's
        ``polarity`` / ``polarity_confidence`` / ``partition`` come from the
        engine's in-process reclassification primitive
        (:meth:`Sim.get_loops_runtime` -> ``reclassify_loops_from_results``,
        GH #679), the Rust source of truth -- it classifies over ALL element
        slots of an arrayed loop's ``loop_score`` series, so an arrayed (A2A)
        loop is labeled by its whole-loop behavior rather than slot 0.  For
        scalar loops the all-slots classification is identical to slot 0.  On
        top of that polarity, each loop carries ``behavior_time_series`` -- its
        per-step relative loop score (its share of the cycle partition's
        activity), used for the dominance analysis.

        Returns empty tuple if analyze_loops=False was used.

        Returns:
            Tuple of Loop objects with runtime polarity and behavioral data

        Example:
            >>> most_important = max(run.loops, key=lambda l: l.average_importance() or 0)
            >>> print(f"Most important: {most_important.id}")
        """
        if self._cached_loops is None:
            self._cached_loops = self._populate_loop_behavior()
        return self._cached_loops

    @property
    def dominant_periods(self) -> tuple[DominantPeriod, ...]:
        """Time periods where specific loops dominate.

        Uses greedy algorithm to identify which loops explain the most
        variance in model behavior during each period.

        Returns empty tuple if analyze_loops=False was used.

        Returns:
            Tuple of DominantPeriod objects

        Example:
            >>> for period in run.dominant_periods:
            ...     print(f"t=[{period.start_time}, {period.end_time}]: {period.dominant_loops}")
        """
        if self._cached_dominant_periods is None:
            self._cached_dominant_periods = self._calculate_dominant_periods()
        return self._cached_dominant_periods

    @property
    def overrides(self) -> dict[str, float]:
        """Variable overrides used for this run.

        Empty dict if no overrides were specified.

        Returns:
            Dictionary mapping variable names to override values
        """
        return dict(self._overrides)

    @property
    def ltm_mode(self) -> str:
        """The LTM loop-enumeration mode this run resolved to.

        Reports whether Loops That Matter enumerated every feedback loop
        exhaustively (small models) or auto-flipped to the strongest-path
        discovery heuristic (large models whose causal graph exceeds the
        SCC-size gate). Without this signal an empty or unexpectedly small
        loop set is indistinguishable from a model that simply has few loops.

        Returns:
            ``"exhaustive"`` or ``"discovery"`` when LTM was enabled for this
            run, or ``"disabled"`` when it was not (e.g. ``analyze_loops=False``
            on :meth:`Model.run`, or ``enable_ltm=False`` on
            :meth:`Model.simulate`).

        Example:
            >>> run = model.run()
            >>> if run.ltm_mode == "discovery":
            ...     print("loops ranked heuristically; not exhaustively enumerated")
        """
        return str(self._sim.get_ltm_mode())

    @property
    def time_spec(self) -> TimeSpec:
        """Time specification used for this run.

        Returns:
            TimeSpec with start, stop, dt, and units

        Example:
            >>> print(f"Simulated from {run.time_spec.start} to {run.time_spec.stop}")
            >>> print(f"Time step: {run.time_spec.dt}")
        """
        if self._cached_time_spec is None:
            self._cached_time_spec = self._extract_time_spec()
        return self._cached_time_spec

    def _populate_loop_behavior(self) -> tuple[Loop, ...]:
        """Build the runtime loop surface: engine-classified polarity plus
        per-loop behavior time series.

        Polarity / ``polarity_confidence`` / ``partition`` are taken verbatim
        from the engine primitive (:meth:`Sim.get_loops_runtime`,
        ``reclassify_loops_from_results``), which classifies over all element
        slots of an arrayed loop -- the single source of truth.  On top of that
        we attach each loop's ``behavior_time_series`` (its relative loop score),
        which the engine primitive does not carry.  Loop ids are stable across
        the two, so the behavior series is keyed by id.

        Returns:
            Tuple of Loop objects with runtime polarity and
            behavior_time_series populated
        """
        runtime_loops = self._sim.get_loops_runtime()
        if not runtime_loops:
            return ()

        loops_with_behavior = []
        for runtime_loop in runtime_loops:
            try:
                behavior_ts = self._sim.get_relative_loop_score(runtime_loop.id)
            except SimlinRuntimeError:
                # No relative-score series (e.g. LTM disabled for this run): keep
                # the engine-classified loop without a behavior series.
                loops_with_behavior.append(runtime_loop)
                continue

            loops_with_behavior.append(
                Loop(
                    id=runtime_loop.id,
                    variables=runtime_loop.variables,
                    polarity=runtime_loop.polarity,
                    polarity_confidence=runtime_loop.polarity_confidence,
                    behavior_time_series=behavior_ts,
                    name=runtime_loop.name,
                    partition=runtime_loop.partition,
                )
            )

        return tuple(loops_with_behavior)

    def _calculate_dominant_periods(self, threshold: float = 0.5) -> tuple[DominantPeriod, ...]:
        """Calculate dominant periods using greedy algorithm.

        For each timestep, tries to find a set of same-polarity loops
        whose combined importance score exceeds the threshold.

        Args:
            threshold: Minimum combined score for dominance (default 0.5)

        Returns:
            Tuple of DominantPeriod objects
        """
        loops = self.loops
        if not loops:
            return ()

        if not any(loop.behavior_time_series is not None for loop in loops):
            return ()

        time_index = self.results.index
        if len(time_index) == 0:
            return ()

        dominant_loop_sets: list[frozenset[str]] = []

        for t_idx in range(len(time_index)):
            loop_scores = []
            for loop in loops:
                if loop.behavior_time_series is not None and len(loop.behavior_time_series) > t_idx:
                    score = loop.behavior_time_series[t_idx]
                    loop_scores.append((loop.id, loop.polarity, score))

            if not loop_scores:
                dominant_loop_sets.append(frozenset())
                continue

            loop_scores.sort(key=lambda x: abs(x[2]), reverse=True)

            # Group loops by effective polarity at this timestep.
            # MOSTLY_* loops are bucketed with their dominant pure cousin
            # (Rux -> reinforcing, Bux -> balancing) because the dominant
            # polarity matches the loop's behavior across most of the run.
            # For UNDETERMINED loops, derive polarity from the score sign.
            reinforcing_loops = []
            balancing_loops = []
            for lid, pol, score in loop_scores:
                if pol == LoopPolarity.REINFORCING or pol == LoopPolarity.MOSTLY_REINFORCING:
                    reinforcing_loops.append((lid, score))
                elif pol == LoopPolarity.BALANCING or pol == LoopPolarity.MOSTLY_BALANCING:
                    balancing_loops.append((lid, score))
                elif pol == LoopPolarity.UNDETERMINED:
                    # Derive polarity from score sign at this timestep
                    if score > 0:
                        reinforcing_loops.append((lid, score))
                    elif score < 0:
                        balancing_loops.append((lid, score))

            def try_polarity_group(
                loops_with_scores: list[tuple[str, float]],
            ) -> tuple[list[str], float]:
                selected: list[str] = []
                combined_score = 0.0
                for lid, score in loops_with_scores:
                    selected.append(lid)
                    combined_score += abs(score)
                    if combined_score >= threshold:
                        break
                return selected, combined_score

            r_selected, r_score = try_polarity_group(reinforcing_loops)
            b_selected, b_score = try_polarity_group(balancing_loops)

            if r_score >= b_score and r_score > 0:
                dominant_loop_sets.append(frozenset(r_selected))
            elif b_score > 0:
                dominant_loop_sets.append(frozenset(b_selected))
            else:
                dominant_loop_sets.append(frozenset())

        periods: list[DominantPeriod] = []
        if not dominant_loop_sets:
            return ()

        current_set = dominant_loop_sets[0]
        start_idx = 0

        for i in range(1, len(dominant_loop_sets)):
            if dominant_loop_sets[i] != current_set:
                if current_set:
                    periods.append(
                        DominantPeriod(
                            dominant_loops=tuple(sorted(current_set)),
                            start_time=float(time_index[start_idx]),
                            end_time=float(time_index[i - 1]),
                        )
                    )
                current_set = dominant_loop_sets[i]
                start_idx = i

        if current_set:
            periods.append(
                DominantPeriod(
                    dominant_loops=tuple(sorted(current_set)),
                    start_time=float(time_index[start_idx]),
                    end_time=float(time_index[-1]),
                )
            )

        return tuple(periods)

    def _extract_time_spec(self) -> TimeSpec:
        """Extract time specification from simulation results.

        Returns:
            TimeSpec with start, stop, dt from results
        """
        time_index = self.results.index
        if len(time_index) < 2:
            start = float(time_index[0]) if len(time_index) > 0 else 0.0
            return TimeSpec(start=start, stop=start, dt=1.0, units=None)

        start = float(time_index[0])
        stop = float(time_index[-1])
        dt = float(time_index[1] - time_index[0])

        return TimeSpec(start=start, stop=stop, dt=dt, units=None)
