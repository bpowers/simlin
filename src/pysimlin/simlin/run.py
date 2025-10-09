"""Simulation run results and analysis."""

from dataclasses import dataclass


@dataclass(frozen=True)
class DominantPeriod:
    """
    Time period where specific loops dominate model behavior.

    DominantPeriod represents a contiguous time interval during which
    a specific set of feedback loops collectively explain the majority
    of the model's behavior. These periods are identified by the loop
    dominance analysis algorithm (Loops That Matter / LTM).
    """

    dominant_loops: tuple[str, ...]
    """Loop IDs that dominate during this period (e.g., ('R1', 'B2'))"""

    start_time: float
    """Period start time"""

    end_time: float
    """Period end time"""

    def duration(self) -> float:
        """
        Calculate the duration of this period.

        Returns:
            Duration in time units (end_time - start_time)
        """
        return self.end_time - self.start_time

    def contains_loop(self, loop_id: str) -> bool:
        """
        Check if a specific loop dominates during this period.

        Args:
            loop_id: Loop identifier to check (e.g., 'R1', 'B2')

        Returns:
            True if the loop is in dominant_loops, False otherwise
        """
        return loop_id in self.dominant_loops


class Run:
    """
    Results and analysis from a single simulation run.

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
        >>> run.results['population'].plot()
        >>> print(f"Final value: {run.results['population'].iloc[-1]}")
        >>>
        >>> # Analyze loop dominance
        >>> for period in run.dominant_periods:
        ...     print(f"{period.start_time}-{period.end_time}: {period.dominant_loops}")
        >>>
        >>> # Compare scenarios
        >>> import pandas as pd
        >>> policy_run = model.run(overrides={'tax_rate': 0.3})
        >>> comparison = pd.DataFrame({
        ...     'baseline': run.results['gdp'],
        ...     'policy': policy_run.results['gdp']
        ... })
        >>> comparison.plot()
    """

    pass
