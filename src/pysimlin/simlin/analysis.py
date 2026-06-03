"""Analysis types for the simlin package."""

from __future__ import annotations

from dataclasses import dataclass
from enum import IntEnum
from typing import TYPE_CHECKING

import numpy as np

if TYPE_CHECKING:
    from numpy.typing import NDArray

    from .run import DominantPeriod


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


class LtmMode(IntEnum):
    """The loop-enumeration mode a simulation resolved to under LTM.

    Integer values mirror the C ``SimlinLtmMode`` enum:

    - ``DISABLED`` (0): the simulation was created without LTM
      (``enable_ltm=False``), so no loop enumeration ran.
    - ``EXHAUSTIVE`` (1): every elementary feedback circuit was enumerated
      (Johnson). Used for small models.
    - ``DISCOVERY`` (2): the model's causal graph exceeded the SCC-size gate
      (or discovery was requested directly), so loops are ranked by the
      per-timestep strongest-path heuristic instead of enumerated.

    ``str(mode)`` yields the lowercase name (``"disabled"`` / ``"exhaustive"``
    / ``"discovery"``), which is what :attr:`Run.ltm_mode` returns.
    """

    DISABLED = 0
    EXHAUSTIVE = 1
    DISCOVERY = 2

    def __str__(self) -> str:
        return self.name.lower()


# Threshold above which a loop with mixed-sign runtime scores is classified
# as MOSTLY_REINFORCING / MOSTLY_BALANCING (Rux / Bux) instead of UNDETERMINED.
# Mirrors `POLARITY_CONFIDENCE_THRESHOLD` in `src/simlin-engine/src/ltm.rs`;
# both implementations use the `>= 0.99` form of the cutoff described in
# Schoenberg & Eberlein (2020) "Seamlessly Integrating LTM" / Schoenberg
# (2020) thesis section 7.6 (the cited papers describe it as "above 0.99").
POLARITY_CONFIDENCE_THRESHOLD: float = 0.99


class LoopPolarity(IntEnum):
    """Polarity of a feedback loop.

    The polarity indicates how the loop affects the system:
    - REINFORCING (R): Loop amplifies changes (positive loop score)
    - BALANCING (B): Loop counteracts changes (negative loop score)
    - MOSTLY_REINFORCING (Rux): Mixed-sign runtime scores but predominantly
      reinforcing (confidence at or above POLARITY_CONFIDENCE_THRESHOLD)
    - MOSTLY_BALANCING (Bux): Mixed-sign runtime scores but predominantly
      balancing (confidence at or above POLARITY_CONFIDENCE_THRESHOLD)
    - UNDETERMINED (U): Loop polarity cannot be determined; mixed-sign
      runtime scores with neither polarity dominant

    Integer values 0-2 mirror the C FFI; values 3 and 4 are Python-only
    classifications produced by `from_runtime_scores` (the FFI does not
    surface a polarity-confidence ratio yet, so structural loops never
    arrive as MOSTLY_*).
    """

    REINFORCING = 0
    BALANCING = 1
    UNDETERMINED = 2
    MOSTLY_REINFORCING = 3
    MOSTLY_BALANCING = 4

    def __str__(self) -> str:
        if self == LoopPolarity.REINFORCING:
            return "R"
        elif self == LoopPolarity.BALANCING:
            return "B"
        elif self == LoopPolarity.MOSTLY_REINFORCING:
            return "Rux"
        elif self == LoopPolarity.MOSTLY_BALANCING:
            return "Bux"
        else:
            return "U"

    @classmethod
    def from_runtime_scores(cls, scores: NDArray[np.float64]) -> LoopPolarity | None:
        """Classify loop polarity based on actual runtime loop score values.

        Mirrors `LoopPolarity::from_runtime_scores` in
        `src/simlin-engine/src/ltm.rs`.  The polarity confidence
        ``|r - |b|| / (r + |b|)`` (Schoenberg & Eberlein, 2020) drives
        the classification:

        - All valid (non-NaN, non-zero) scores positive -> REINFORCING
        - All valid scores negative -> BALANCING
        - Mixed signs and confidence at or above
          POLARITY_CONFIDENCE_THRESHOLD -> MOSTLY_REINFORCING /
          MOSTLY_BALANCING based on which side dominates the magnitude
          tally
        - Mixed signs and confidence below the threshold -> UNDETERMINED
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

        positive_sum = float(valid_scores[valid_scores > 0].sum())
        negative_sum_abs = float(-valid_scores[valid_scores < 0].sum())

        denom = positive_sum + negative_sum_abs
        if denom <= 0.0:
            # Mathematically unreachable: at least one filtered-in score
            # is non-zero, so the magnitude sum is strictly positive.
            # Guard anyway so a hostile array of subnormals can't surface
            # a divide-by-zero NaN.
            return None

        confidence = abs(positive_sum - negative_sum_abs) / denom

        has_positive = positive_sum > 0.0
        has_negative = negative_sum_abs > 0.0

        if has_positive and not has_negative:
            return cls.REINFORCING
        if has_negative and not has_positive:
            return cls.BALANCING
        if confidence >= POLARITY_CONFIDENCE_THRESHOLD:
            # Equal-magnitude r and |b| would yield confidence 0, which
            # cannot pass the threshold check for any threshold > 0, so
            # the dominant side is always strictly larger here.
            if positive_sum > negative_sum_abs:
                return cls.MOSTLY_REINFORCING
            return cls.MOSTLY_BALANCING
        return cls.UNDETERMINED


@dataclass
class Link:
    """Represents a causal link between two variables."""

    from_var: str
    to_var: str
    polarity: LinkPolarity
    score: NDArray[np.float64] | None = None
    relative_score: NDArray[np.float64] | None = None
    """Relative LTM link-score series (GH #652).

    The raw :attr:`score` normalized, per target and per timestep, against
    the sum of ``|score|`` over **all** of :attr:`to_var`'s scored inputs --
    a value in ``[-1, 1]`` that *is* comparable across the model. Use this
    (via :meth:`average_relative_score`), not the raw :attr:`score`, when
    ranking links by importance. ``None`` exactly when :attr:`score` is
    ``None``; otherwise the same shape as :attr:`score`.
    """

    def __str__(self) -> str:
        """Return a human-readable string representation."""
        pol_str = str(self.polarity)
        return f"{self.from_var} --{pol_str}--> {self.to_var}"

    def has_score(self) -> bool:
        """Check if this link has LTM score data."""
        return self.score is not None and len(self.score) > 0

    def average_score(self) -> float | None:
        """Calculate the average RAW score across all time steps.

        .. warning::

            Raw link scores are **not comparable across different target
            variables** and are unusable for ranking links globally. The raw
            score divides by the change in the *target* variable, so a link
            into a near-constant target (a parameter, an equilibrium) produces
            an astronomically large score even when the link is unimportant
            (GH #652). Ranking links by ``|average_score()|`` surfaces these
            numerically degenerate links instead of the meaningful ones.

            To rank links by importance, use :meth:`average_relative_score`
            (or the :attr:`relative_score` series), which normalizes per
            target into a comparable ``[-1, 1]`` value.

        Returns ``None`` when there is no score series, and ``NaN``
        when every step is ``NaN`` (a link that never produced a
        defined score). The reduction runs over the finite subset so
        the all-``NaN`` case does not leak numpy's "Mean of empty
        slice" RuntimeWarning -- on large models a majority of causal
        links can have all-``NaN`` scores.
        """
        return self._average(self.score)

    def average_relative_score(self) -> float | None:
        """Average the **relative** link score across all time steps.

        This is the importance metric to rank links by: unlike
        :meth:`average_score`, the relative score is normalized per target
        into ``[-1, 1]`` and is comparable across the whole model (the
        fraction of the target's change attributable to this input, GH #652).
        Rank links by ``abs(link.average_relative_score() or 0.0)`` to find
        the most important causal links.

        Returns ``None`` when there is no relative-score series, and ``NaN``
        when every step is ``NaN``; the reduction runs over the finite subset
        so the all-``NaN`` case stays warning-free.
        """
        return self._average(self.relative_score)

    @staticmethod
    def _average(series: NDArray[np.float64] | None) -> float | None:
        """Mean of a score series over its finite (non-NaN) entries.

        Returns ``None`` for an absent/empty series and ``NaN`` when every
        entry is ``NaN``, avoiding numpy's empty-slice RuntimeWarning.
        """
        if series is None or len(series) == 0:
            return None
        valid = series[~np.isnan(series)]
        if valid.size == 0:
            return float("nan")
        return float(valid.mean())

    def max_score(self) -> float | None:
        """Get the maximum RAW score across all time steps.

        See :meth:`average_score` for why raw scores are not comparable
        across targets; for ranking use the relative score.

        Returns ``None`` when there is no score series, and ``NaN``
        when every step is ``NaN``; the reduction runs over the finite
        subset so the all-``NaN`` case stays warning-free.
        """
        if self.score is None or len(self.score) == 0:
            return None
        valid = self.score[~np.isnan(self.score)]
        if valid.size == 0:
            return float("nan")
        return float(valid.max())


@dataclass(frozen=True)
class Partition:
    """One cycle partition referenced by a discovery result's loops.

    A cycle partition is a group of stocks connected by feedback (a strongly
    connected component of the stock-to-stock reachability graph). Relative
    loop scores are normalized *within* a partition, so loop importance is
    only comparable between partition-mates -- group loops by
    :attr:`Loop.partition` to present them partition-by-partition (e.g. lead
    with the model's giant component).
    """

    stocks: tuple[str, ...]
    """The partition's stock names (element-level for arrayed models, e.g.
    ``population[nyc]``), sorted lexicographically."""

    loop_count: int
    """Number of loops in the returned loop list that belong to this
    partition."""


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
    """Loop polarity: REINFORCING (R), BALANCING (B), MOSTLY_REINFORCING (Rux),
    MOSTLY_BALANCING (Bux), or UNDETERMINED (U). MOSTLY_* values only arise
    from `LoopPolarity.from_runtime_scores`; the C FFI surface coalesces them
    onto REINFORCING/BALANCING because it has no polarity-confidence field."""

    behavior_time_series: NDArray[np.float64] | None = None
    """
    Loop's contribution to model behavior over time, as the SIGNED relative
    loop score in ``[-1, 1]``: the loop's share of its cycle partition's total
    absolute loop score, with sign preserved (a balancing loop reads negative).
    Comparable across loops, so ``abs(...)`` ranks loops by dominance.
    None for structural loops; populated for loops from ``Run`` objects and
    ``Model.analyze()``.
    """

    name: str | None = None
    """Human-meaningful loop name the modeler assigned via ``set_loop_name``,
    or ``None`` when the loop has no assigned name (every enumerated loop).
    A pinned loop's ``id`` is just ``pin1``/``pin2``/...; this carries the
    label the modeler chose so it can be displayed instead of the bare id."""

    partition: int | None = None
    """RESULT-SCOPED index into :attr:`Analysis.partitions` naming this loop's
    cycle partition, or ``None`` -- both for loops with no parent-level
    partition (pure module-internal loops) and for loop surfaces that don't
    carry partition metadata (structural ``Model.loops``, ``Run.loops``).
    Indices are dense, assigned in first-appearance order over the ranked
    loop list; they identify partitions within ONE analysis result only and
    are not stable across runs or model edits -- key on
    :attr:`Partition.stocks` for a durable identity."""

    def __str__(self) -> str:
        """Return a human-readable string representation."""
        var_chain = " -> ".join(self.variables)
        if self.variables:
            var_chain += f" -> {self.variables[0]}"
        name_part = f' "{self.name}"' if self.name else ""
        return f"Loop {self.id}{name_part} ({self.polarity}): {var_chain}"

    def __len__(self) -> int:
        """Return the number of variables in the loop."""
        return len(self.variables)

    def contains_variable(self, var_name: str) -> bool:
        """Check if a variable is part of this loop."""
        return var_name in self.variables

    def average_importance(self) -> float | None:
        """
        Average importance across simulation.

        Computes the mean of the absolute value of the behavior time series
        (the signed relative loop score in ``[-1, 1]``), so the result is in
        ``[0, 1]`` and comparable across loops -- a higher value means the loop
        is more dominant on average.
        Returns None if behavior_time_series is not available (structural loops).

        Returns:
            Average importance score, or None if no behavioral data

        Example:
            >>> important_loops = [
            ...     l for l in run.loops if l.average_importance() and l.average_importance() > 0.1
            ... ]
        """
        if self.behavior_time_series is None or len(self.behavior_time_series) == 0:
            return None
        abs_series = np.abs(self.behavior_time_series)
        valid = abs_series[~np.isnan(abs_series)]
        if valid.size == 0:
            return float("nan")
        return float(valid.mean())

    def max_importance(self) -> float | None:
        """
        Maximum importance during simulation.

        Computes the maximum of the absolute value of the behavior time series
        (the signed relative loop score in ``[-1, 1]``), so the result is in
        ``[0, 1]``: the peak share of partition activity this loop ever drives.
        Returns None if behavior_time_series is not available (structural loops).

        Returns:
            Maximum importance score, or None if no behavioral data

        Example:
            >>> peak_importance = max(l.max_importance() for l in run.loops if l.max_importance())
        """
        if self.behavior_time_series is None or len(self.behavior_time_series) == 0:
            return None
        abs_series = np.abs(self.behavior_time_series)
        valid = abs_series[~np.isnan(abs_series)]
        if valid.size == 0:
            return float("nan")
        return float(valid.max())


@dataclass(frozen=True)
class Analysis:
    """Result of strongest-path loop *discovery* (`Model.analyze`).

    Discovery is the heuristic "Loops That Matter" algorithm
    (Eberlein & Schoenberg, 2020): instead of exhaustively enumerating every
    feedback loop -- which is empty for large models that auto-flip to
    discovery mode -- it finds the loops that drive behavior. Each discovered
    `Loop` carries its `behavior_time_series` (the per-step importance series),
    and `dominant_periods` records which loops dominate during each interval.

    `truncated` is True when discovery hit its `timeout` before finishing, so
    `loops`/`dominant_periods` may be partial. Discovery on very large models
    can be infeasibly slow, so `Model.analyze` is an explicit, opt-in,
    timeout-guarded call -- it is never run automatically by `Model.run`.
    """

    loops: tuple[Loop, ...]
    """Discovered loops, ranked competitive-first: loops that share their
    cycle partition with at least one other discovered loop come first,
    ordered by mean ``abs`` relative importance (descending); loops trivially
    ALONE in their partition -- whose relative score is exactly ``±1`` at
    every active step *by construction* (e.g. an isolated stock-decay loop)
    -- come after all competing loops. Each loop carries its signed
    relative-loop-score ``behavior_time_series`` and its ``partition``
    index."""

    dominant_periods: tuple[DominantPeriod, ...]
    """Intervals where a specific set of loops dominates behavior."""

    truncated: bool = False
    """True when the `timeout` elapsed before discovery finished."""

    partitions: tuple[Partition, ...] = ()
    """The cycle partitions referenced by ``loops`` (each loop's
    ``partition`` indexes this tuple). Dense, in first-appearance order over
    the ranked loop list; result-scoped. Group loops by partition to present
    each feedback subsystem separately -- importance is only comparable
    within a partition."""
