"""Analysis types for the simlin package."""

from enum import IntEnum
from typing import Optional
from dataclasses import dataclass
import numpy as np
from numpy.typing import NDArray


class LinkPolarity(IntEnum):
    """Polarity of a causal link."""
    
    POSITIVE = 0
    NEGATIVE = 1
    UNKNOWN = 2
    
    def __str__(self) -> str:
        if self == LinkPolarity.POSITIVE:
            return "+"
        elif self == LinkPolarity.NEGATIVE:
            return "-"
        else:
            return "?"


class LoopPolarity(IntEnum):
    """Polarity of a feedback loop.

    The polarity indicates how the loop affects the system:
    - REINFORCING (R): Loop amplifies changes (positive loop score)
    - BALANCING (B): Loop counteracts changes (negative loop score)
    - UNDETERMINED (U): Loop polarity cannot be determined or changes during simulation
    """

    REINFORCING = 0
    BALANCING = 1
    UNDETERMINED = 2

    def __str__(self) -> str:
        if self == LoopPolarity.REINFORCING:
            return "R"
        elif self == LoopPolarity.BALANCING:
            return "B"
        else:
            return "U"

    @classmethod
    def from_runtime_scores(cls, scores: "NDArray[np.float64]") -> Optional["LoopPolarity"]:
        """Classify loop polarity based on actual runtime loop score values.

        This function examines the loop score values from a simulation run
        and determines the appropriate polarity:
        - All valid (non-NaN, non-zero) scores positive -> Reinforcing
        - All valid scores negative -> Balancing
        - Mix of positive and negative -> Undetermined
        - No valid scores -> returns None

        Args:
            scores: Array of loop score values from simulation

        Returns:
            The runtime polarity classification, or None if no valid scores
        """
        # Filter out NaN and zero values
        valid_scores = scores[~np.isnan(scores) & (scores != 0.0)]

        if len(valid_scores) == 0:
            return None

        has_positive = np.any(valid_scores > 0)
        has_negative = np.any(valid_scores < 0)

        if has_positive and not has_negative:
            return cls.REINFORCING
        elif has_negative and not has_positive:
            return cls.BALANCING
        elif has_positive and has_negative:
            return cls.UNDETERMINED
        else:
            return None  # All zeros after filtering


@dataclass
class Link:
    """Represents a causal link between two variables."""
    
    from_var: str
    to_var: str
    polarity: LinkPolarity
    score: Optional[NDArray[np.float64]] = None
    
    def __str__(self) -> str:
        """Return a human-readable string representation."""
        pol_str = str(self.polarity)
        return f"{self.from_var} --{pol_str}--> {self.to_var}"
    
    def has_score(self) -> bool:
        """Check if this link has LTM score data."""
        return self.score is not None and len(self.score) > 0
    
    def average_score(self) -> Optional[float]:
        """Calculate the average score across all time steps."""
        if self.score is None or len(self.score) == 0:
            return None
        return float(np.nanmean(self.score))
    
    def max_score(self) -> Optional[float]:
        """Get the maximum score across all time steps."""
        if self.score is None or len(self.score) == 0:
            return None
        return float(np.nanmax(self.score))


@dataclass(frozen=True)
class Loop:
    """
    Represents a feedback loop.

    When obtained from Model.loops (structural), behavior_time_series is None.
    When obtained from Run.loops (behavioral), includes time series data showing
    the loop's contribution to model behavior at each time step.

    Immutable - modifying attributes will not change the model.
    """

    id: str
    """Loop identifier (e.g., 'R1', 'B2', 'U3')"""

    variables: tuple[str, ...]
    """Variables in this loop"""

    polarity: LoopPolarity
    """Loop polarity: REINFORCING (R), BALANCING (B), or UNDETERMINED (U)"""

    behavior_time_series: Optional[NDArray[np.float64]] = None
    """
    Loop's contribution to model behavior over time.
    None for structural loops, populated for loops from Run objects.
    """

    def __str__(self) -> str:
        """Return a human-readable string representation."""
        var_chain = " -> ".join(self.variables)
        if self.variables:
            var_chain += f" -> {self.variables[0]}"
        return f"Loop {self.id} ({self.polarity}): {var_chain}"

    def __len__(self) -> int:
        """Return the number of variables in the loop."""
        return len(self.variables)

    def contains_variable(self, var_name: str) -> bool:
        """Check if a variable is part of this loop."""
        return var_name in self.variables

    def average_importance(self) -> Optional[float]:
        """
        Average importance across simulation.

        Computes the mean of the absolute value of the behavior time series.
        Returns None if behavior_time_series is not available (structural loops).

        Returns:
            Average importance score, or None if no behavioral data

        Example:
            >>> important_loops = [l for l in run.loops if l.average_importance() and l.average_importance() > 0.1]
        """
        if self.behavior_time_series is None or len(self.behavior_time_series) == 0:
            return None
        return float(np.nanmean(np.abs(self.behavior_time_series)))

    def max_importance(self) -> Optional[float]:
        """
        Maximum importance during simulation.

        Computes the maximum of the absolute value of the behavior time series.
        Returns None if behavior_time_series is not available (structural loops).

        Returns:
            Maximum importance score, or None if no behavioral data

        Example:
            >>> peak_importance = max(l.max_importance() for l in run.loops if l.max_importance())
        """
        if self.behavior_time_series is None or len(self.behavior_time_series) == 0:
            return None
        return float(np.nanmax(np.abs(self.behavior_time_series)))
