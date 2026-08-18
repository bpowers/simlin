"""Analysis types for the simlin package."""

from __future__ import annotations

from collections.abc import Iterable  # noqa: TC003 -- see below
from dataclasses import dataclass
from enum import IntEnum
from typing import TYPE_CHECKING

import numpy as np

# `Iterable` is imported at RUNTIME (the noqa above suppresses ruff's TC003)
# because it annotates the PUBLIC `links_by_target` signature: with a
# TYPE_CHECKING-only import, `typing.get_type_hints(simlin.links_by_target)`
# -- and any doc generator or type-driven framework built on it -- raises
# NameError. (The `NDArray` annotations below stay TYPE_CHECKING-only; that
# is the ecosystem-wide numpy convention and predates this module.)

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
      (or discovery was requested directly), so loops are found
      post-simulation from the recorded link scores instead of enumerated.

    ``str(mode)`` yields the lowercase name (``"disabled"`` / ``"exhaustive"``
    / ``"discovery"``), which is what :attr:`Run.ltm_mode` returns.
    """

    DISABLED = 0
    EXHAUSTIVE = 1
    DISCOVERY = 2

    def __str__(self) -> str:
        return self.name.lower()


class LoopPolarity(IntEnum):
    """Polarity of a feedback loop.

    The polarity indicates how the loop affects the system:
    - REINFORCING (R): Loop amplifies changes (positive loop score)
    - BALANCING (B): Loop counteracts changes (negative loop score)
    - MOSTLY_REINFORCING (Rux): Mixed-sign runtime scores but predominantly
      reinforcing (polarity confidence at or above the engine's cutoff)
    - MOSTLY_BALANCING (Bux): Mixed-sign runtime scores but predominantly
      balancing (polarity confidence at or above the engine's cutoff)
    - UNDETERMINED (U): Loop polarity cannot be determined; mixed-sign
      runtime scores with neither polarity dominant

    Classification of a runtime score series -- including the confidence
    cutoff separating Rux/Bux from U -- happens engine-side in
    `LoopPolarity::from_runtime_scores` / `POLARITY_CONFIDENCE_THRESHOLD`
    (`src/simlin-engine/src/ltm/types.rs`); Python only carries the label
    and the confidence the engine computed, so there is no second
    implementation to drift.

    All five integer values mirror the C FFI `SimlinLoopPolarity` 1:1
    (GH #495): the FFI no longer coalesces MOSTLY_* down to R/B, and carries
    a polarity-confidence ratio alongside the polarity (see
    `Loop.polarity_confidence`). On the structural loop surface the FFI still
    only emits R/B/U (it has no runtime scores); the MOSTLY_* variants and
    intermediate confidences arrive on the discovery surface.
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
    a value in ``[-1, 1]`` comparable between the inputs of ONE target.
    ``None`` exactly when :attr:`score` is ``None``; otherwise the same
    shape as :attr:`score`.  For ranking see :func:`links_by_target` and
    :attr:`scored_input_count` (GH #998): a target with a single scored
    input reads ``±1`` by construction.
    """

    scored_input_count: int = 0
    """The size of :attr:`relative_score`'s normalization group (GH #998):
    how many CONTRIBUTING links share this link's :attr:`to_var` target,
    itself included; ``0`` when this link never contributes -- no score
    series, or an all-NaN one (an all-NaN series adds no summand to any
    step's denominator, so it is no competition; such series are common on
    large exhaustive-mode models).

    A group of ONE reads exactly ``±1`` at every step **by construction**
    (there is nothing else to normalize against), so ranking links globally
    by ``abs(average_relative_score())`` floats such no-competition links to
    the top -- on C-LEARN, 58 of the global top 100 were single-input
    targets.  Group links by target (:func:`links_by_target`) and rank
    within a group; use this field to detect the trivial groups.  Per-step
    residual: a link that is NaN at only SOME steps counts here yet leaves
    its siblings momentarily unopposed at those steps."""

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

        Unlike :meth:`average_score`, the relative score is normalized per
        target into ``[-1, 1]``: the fraction of the target's change
        attributable to this input (GH #652). That makes it meaningful for
        comparing the inputs *of one target* against each other.

        .. warning::

            **It is not a global importance ranking.** The normalization is
            per target, so a link into a target with only ONE scored input
            (:attr:`scored_input_count` == 1) reads exactly ``±1`` at every
            step **by construction**, regardless of whether that target
            matters. Sorting all links by ``abs(average_relative_score())``
            therefore floats the links with no competition to the top.
            Observed on C-LEARN: 143 of 949 scored links have a single-input
            target, and 58 of the global top 100 by this metric were such
            links.

            To rank links, group by target with :func:`links_by_target`
            (which sorts within each group) and read the groups; a genuinely
            global ranking would need to weight each target by its own
            importance, which the relative score deliberately divides out.

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


def links_by_target(links: Iterable[Link]) -> dict[str, tuple[Link, ...]]:
    """Group links by target, ranked within each group (GH #998).

    The relative link score is a share of its TARGET's change, so links are
    comparable only against the other inputs of the same target -- ranking
    the whole model's links by ``abs(average_relative_score())`` is
    dominated by targets with a single scored input, whose share is ``±1``
    by construction.  This helper makes the grouping structural: it returns
    ``{target: (links sorted by |average_relative_score()| descending)}``,
    so a cross-target comparison is something the caller has to build
    deliberately rather than fall into.

    Unscored links (``relative_score is None``) sort last within their
    group.  Iterate targets and read each group's top links; a group of one
    (see :attr:`Link.scored_input_count`) carries no ranking information.

    Example:
        >>> by_target = links_by_target(sim.get_links())
        >>> for target, group in by_target.items():
        ...     if len(group) > 1:
        ...         print(target, group[0])
    """
    grouped: dict[str, list[Link]] = {}
    for link in links:
        grouped.setdefault(link.to_var, []).append(link)

    def sort_key(link: Link) -> float:
        avg = link.average_relative_score()
        if avg is None or np.isnan(avg):
            return -1.0
        return abs(avg)

    return {
        target: tuple(sorted(group, key=sort_key, reverse=True))
        for target, group in grouped.items()
    }


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
    MOSTLY_BALANCING (Bux), or UNDETERMINED (U). All five arrive through the C
    FFI verbatim (GH #495) -- the MOSTLY_* ("Rux"/"Bux") variants are no longer
    coalesced onto REINFORCING/BALANCING. They occur on the runtime surfaces
    (``Run.loops``, ``Sim.get_loops_runtime``, and discovery), where the
    polarity is classified from runtime score series; see
    :attr:`polarity_confidence`."""

    polarity_confidence: float = 1.0
    """Polarity-confidence ratio in ``[0.0, 1.0]`` behind :attr:`polarity`
    (GH #495): ``1.0`` for a clean reinforcing/balancing loop, a value below
    ``1.0`` for a mixed-sign MOSTLY_REINFORCING/MOSTLY_BALANCING loop, and
    ``0.0`` for an UNDETERMINED one. On the structural ``Model.loops`` surface
    this is ``1.0``/``0.0`` by design (links are either all signed or some are
    unknown). The default ``1.0`` matches the structural "fully determined"
    convention for the rare construction path that omits it."""

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
    carry partition metadata (structural ``Model.loops``).
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
    """Result of post-simulation loop *discovery* (`Model.analyze`).

    Discovery is the "Loops That Matter" analysis (Eberlein & Schoenberg,
    2020) run over the recorded link scores: instead of exhaustively
    enumerating every structural feedback loop -- which is empty for large
    models that auto-flip to discovery mode -- it finds the loops that drive
    behavior. Each discovered `Loop` carries its `behavior_time_series` (the
    per-step importance series), and `dominant_periods` records which loops
    dominate during each interval.

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
    """Intervals where a specific set of loops dominates behavior, computed
    PER CYCLE PARTITION.

    A loop's ``behavior_time_series`` is its share *within its own cycle
    partition* (see :class:`Partition`) and is not comparable between
    partitions -- a loop ALONE in its partition reads exactly ``±1`` at every
    active step by construction -- so the engine selects dominance within
    each partition independently and tags every period with
    :attr:`~simlin.DominantPeriod.partition` (indexing :attr:`partitions`,
    the same space as :attr:`Loop.partition`). The tuple carries one period
    timeline per partition, most-competitive partition first, consistent
    with the competitive-first ranking of :attr:`loops`. A lone loop's
    (trivially always-dominant) periods are still reported, confined to its
    own partition's timeline -- filter on partitions whose
    :attr:`Partition.loop_count` is above 1 to drop them."""

    truncated: bool = False
    """True when the `timeout` elapsed before discovery finished, so
    `loops`/`dominant_periods` may be partial. This is the wall-clock time
    budget -- distinct from `agg_recovery_truncated`."""

    agg_recovery_truncated: bool = False
    """True when discovery's cross-element-through-aggregate loop recovery hit
    its reducer-loop-count budget, so some cross-agg reducer loops are absent
    from `loops`. This is the *structural*-completeness signal (mirroring
    exhaustive mode's analogous warning), distinct from `truncated` (the
    wall-clock time budget)."""

    partitions: tuple[Partition, ...] = ()
    """The cycle partitions referenced by ``loops`` (each loop's
    ``partition`` indexes this tuple). Dense, in first-appearance order over
    the ranked loop list; result-scoped. Group loops by partition to present
    each feedback subsystem separately -- importance is only comparable
    within a partition."""

    enumeration_complete: bool = False
    """True when discovery ENUMERATED the whole candidate universe rather than
    sampling it, so `loops` is the exact selection from every loop that can
    ever score. False means a shortest-path fallback generated the candidates
    -- because the enumeration's budgets or the `timeout` did not allow it to
    finish -- and `loops` is a SAMPLE. Check this before reading an absent
    loop as evidence the model has none."""

    retained_loops: int = 0
    """How many loops passed discovery's importance filter, before the report
    cap truncated `loops`. Equal to ``len(loops)`` when the cap did not bind;
    larger when it did, which is the only signal that `loops` is a
    most-important-first prefix rather than the whole retained set."""

    universe_loops: int | None = None
    """How many ever-simultaneously-active feedback loops the candidate
    universe holds -- the population each loop's importance is a share of --
    or None when `enumeration_complete` is False, since a sampled analysis has
    no universe to report. None and 0 are different claims: 0 means the model
    genuinely has no scorable loop."""
