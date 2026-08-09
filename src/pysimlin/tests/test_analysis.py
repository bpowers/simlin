"""Tests for analysis types."""

import warnings

import numpy as np
import pytest

from simlin import Link, LinkPolarity, Loop, LoopPolarity


class TestLinkPolarity:
    """Test LinkPolarity enum."""

    def test_link_polarity_str(self) -> None:
        """Test string representation of link polarities."""
        assert str(LinkPolarity.POSITIVE) == "+"
        assert str(LinkPolarity.NEGATIVE) == "-"
        assert str(LinkPolarity.UNKNOWN) == "?"


class TestLink:
    """Test Link dataclass."""

    def test_link_with_score(self) -> None:
        """Test Link with LTM score data."""
        scores = np.array([0.1, 0.2, 0.3, 0.4, 0.5])
        link = Link(from_var="A", to_var="B", polarity=LinkPolarity.NEGATIVE, score=scores)

        assert link.has_score()
        assert link.average_score() == pytest.approx(0.3)
        assert link.max_score() == pytest.approx(0.5)

    def test_link_without_score(self) -> None:
        """Test Link without score data."""
        link = Link(from_var="X", to_var="Y", polarity=LinkPolarity.UNKNOWN)

        assert not link.has_score()
        assert link.average_score() is None
        assert link.max_score() is None

    def test_link_str(self) -> None:
        """Test string representation of Link."""
        link = Link(from_var="input", to_var="output", polarity=LinkPolarity.POSITIVE)

        str_repr = str(link)
        assert "input" in str_repr
        assert "output" in str_repr
        assert "+" in str_repr or "--+--" in str_repr

    def test_link_average_score_with_nan(self) -> None:
        """Test Link average_score handles NaN values correctly."""
        scores = np.array([np.nan, np.nan, 0.2, 0.3, 0.4])
        link = Link(from_var="A", to_var="B", polarity=LinkPolarity.POSITIVE, score=scores)

        avg = link.average_score()
        assert avg is not None
        assert avg == pytest.approx(0.3)

    def test_link_max_score_with_nan(self) -> None:
        """Test Link max_score handles NaN values correctly."""
        scores = np.array([np.nan, 0.1, 0.5, 0.2, 0.3])
        link = Link(from_var="A", to_var="B", polarity=LinkPolarity.NEGATIVE, score=scores)

        max_score = link.max_score()
        assert max_score is not None
        assert max_score == pytest.approx(0.5)

    def test_link_average_score_all_nan(self) -> None:
        """Test Link average_score returns NaN when all values are NaN."""
        scores = np.array([np.nan, np.nan, np.nan])
        link = Link(from_var="A", to_var="B", polarity=LinkPolarity.POSITIVE, score=scores)

        avg = link.average_score()
        assert avg is not None
        assert np.isnan(avg)

    def test_link_average_score_all_nan_no_warning(self) -> None:
        """An all-NaN score must not leak a numpy RuntimeWarning to callers.

        On large models (e.g. C-LEARN) many causal links have an
        all-NaN score series; reducing them with bare np.nanmean spams
        'Mean of empty slice' warnings even though NaN is the intended
        return value.
        """
        scores = np.array([np.nan, np.nan, np.nan])
        link = Link(from_var="A", to_var="B", polarity=LinkPolarity.POSITIVE, score=scores)

        with warnings.catch_warnings():
            warnings.simplefilter("error", RuntimeWarning)
            avg = link.average_score()
        assert avg is not None
        assert np.isnan(avg)

    def test_link_max_score_all_nan_no_warning(self) -> None:
        """An all-NaN score must not leak a numpy RuntimeWarning from max_score."""
        scores = np.array([np.nan, np.nan, np.nan])
        link = Link(from_var="A", to_var="B", polarity=LinkPolarity.POSITIVE, score=scores)

        with warnings.catch_warnings():
            warnings.simplefilter("error", RuntimeWarning)
            mx = link.max_score()
        assert mx is not None
        assert np.isnan(mx)

    def test_link_relative_score_defaults_none(self) -> None:
        """A Link constructed without a relative score has None for it (GH #652)."""
        link = Link(from_var="A", to_var="B", polarity=LinkPolarity.POSITIVE)
        assert link.relative_score is None
        assert link.average_relative_score() is None

    def test_link_average_relative_score(self) -> None:
        """average_relative_score reduces the relative series over finite entries."""
        rel = np.array([0.2, 0.4, 0.6])
        link = Link(
            from_var="A",
            to_var="B",
            polarity=LinkPolarity.POSITIVE,
            score=np.array([1.0, 2.0, 3.0]),
            relative_score=rel,
        )
        assert link.average_relative_score() == pytest.approx(0.4)

    def test_link_average_relative_score_with_nan(self) -> None:
        """average_relative_score excludes NaN entries, no warning leaked."""
        rel = np.array([np.nan, 0.2, 0.4])
        link = Link(
            from_var="A",
            to_var="B",
            polarity=LinkPolarity.POSITIVE,
            relative_score=rel,
        )
        with warnings.catch_warnings():
            warnings.simplefilter("error", RuntimeWarning)
            avg = link.average_relative_score()
        assert avg == pytest.approx(0.3)

    def test_link_average_relative_score_all_nan(self) -> None:
        """An all-NaN relative series returns NaN (not None), warning-free."""
        rel = np.array([np.nan, np.nan])
        link = Link(
            from_var="A",
            to_var="B",
            polarity=LinkPolarity.POSITIVE,
            relative_score=rel,
        )
        with warnings.catch_warnings():
            warnings.simplefilter("error", RuntimeWarning)
            avg = link.average_relative_score()
        assert avg is not None
        assert np.isnan(avg)

    def test_relative_score_ranks_above_raw_for_degenerate_link(self) -> None:
        """The GH #652 ranking workflow in miniature.

        Two links into a near-constant target have astronomically large RAW
        scores; one link into an active target has a modest raw score. Ranking
        by ``|average_score()|`` (raw) degenerately puts the near-constant
        links on top, while ranking by ``|average_relative_score()|`` puts the
        active target's dominant link above the degenerate one.
        """
        near = Link(
            from_var="p1",
            to_var="near_const",
            polarity=LinkPolarity.POSITIVE,
            score=np.array([6.4e22, 6.4e22]),
            # near_const's two inputs (6.4e22, 1.1e22) sum to 7.5e22.
            relative_score=np.array([6.4e22 / 7.5e22, 6.4e22 / 7.5e22]),
        )
        active = Link(
            from_var="x",
            to_var="active",
            polarity=LinkPolarity.POSITIVE,
            score=np.array([0.9, 0.9]),
            relative_score=np.array([0.9, 0.9]),
        )

        def raw_key(link: Link) -> float:
            return abs(link.average_score() or 0.0)

        def rel_key(link: Link) -> float:
            return abs(link.average_relative_score() or 0.0)

        # Raw ranking is degenerate: the near-constant link wins.
        assert raw_key(near) > raw_key(active)
        # Relative ranking restores order: the active link wins.
        assert rel_key(active) > rel_key(near)


class TestLoop:
    """Test Loop dataclass."""

    def test_loop_str(self) -> None:
        """Test string representation of Loop."""
        loop = Loop(
            id="B1",
            variables=("stock", "outflow", "desired_stock"),
            polarity=LoopPolarity.BALANCING,
        )

        str_repr = str(loop)
        assert "B1" in str_repr
        assert "B" in str_repr or "BALANCING" in str_repr
        assert "stock" in str_repr
        assert "->" in str_repr

    def test_loop_average_importance_with_data(self) -> None:
        """Test average_importance with behavioral data."""
        behavior = np.array([0.1, 0.2, 0.3, 0.4, 0.5])
        loop = Loop(
            id="R1",
            variables=("a", "b", "c"),
            polarity=LoopPolarity.REINFORCING,
            behavior_time_series=behavior,
        )

        avg = loop.average_importance()
        assert avg is not None
        assert avg == pytest.approx(0.3)

    def test_loop_average_importance_with_negative_values(self) -> None:
        """Test average_importance with negative values (uses absolute value)."""
        behavior = np.array([-0.5, 0.5, -0.3, 0.3])
        loop = Loop(
            id="B1",
            variables=("x", "y"),
            polarity=LoopPolarity.BALANCING,
            behavior_time_series=behavior,
        )

        avg = loop.average_importance()
        assert avg is not None
        assert avg == pytest.approx(0.4)

    def test_loop_average_importance_without_data(self) -> None:
        """Test average_importance without behavioral data."""
        loop = Loop(
            id="R1",
            variables=("a", "b"),
            polarity=LoopPolarity.REINFORCING,
        )

        assert loop.average_importance() is None

    def test_loop_average_importance_empty_array(self) -> None:
        """Test average_importance with empty array."""
        loop = Loop(
            id="R1",
            variables=("a", "b"),
            polarity=LoopPolarity.REINFORCING,
            behavior_time_series=np.array([]),
        )

        assert loop.average_importance() is None

    def test_loop_max_importance_with_data(self) -> None:
        """Test max_importance with behavioral data."""
        behavior = np.array([0.1, 0.5, 0.2, 0.3])
        loop = Loop(
            id="R1",
            variables=("a", "b", "c"),
            polarity=LoopPolarity.REINFORCING,
            behavior_time_series=behavior,
        )

        max_imp = loop.max_importance()
        assert max_imp is not None
        assert max_imp == pytest.approx(0.5)

    def test_loop_max_importance_with_negative_values(self) -> None:
        """Test max_importance with negative values (uses absolute value)."""
        behavior = np.array([0.2, -0.8, 0.3, -0.1])
        loop = Loop(
            id="B1",
            variables=("x", "y"),
            polarity=LoopPolarity.BALANCING,
            behavior_time_series=behavior,
        )

        max_imp = loop.max_importance()
        assert max_imp is not None
        assert max_imp == pytest.approx(0.8)

    def test_loop_max_importance_without_data(self) -> None:
        """Test max_importance without behavioral data."""
        loop = Loop(
            id="R1",
            variables=("a", "b"),
            polarity=LoopPolarity.REINFORCING,
        )

        assert loop.max_importance() is None

    def test_loop_max_importance_empty_array(self) -> None:
        """Test max_importance with empty array."""
        loop = Loop(
            id="R1",
            variables=("a", "b"),
            polarity=LoopPolarity.REINFORCING,
            behavior_time_series=np.array([]),
        )

        assert loop.max_importance() is None

    def test_loop_average_importance_with_nan(self) -> None:
        """Test average_importance handles NaN values correctly."""
        behavior = np.array([np.nan, np.nan, 0.2, 0.3, 0.4])
        loop = Loop(
            id="R1",
            variables=("population", "births"),
            polarity=LoopPolarity.REINFORCING,
            behavior_time_series=behavior,
        )

        avg = loop.average_importance()
        assert avg is not None
        assert avg == pytest.approx(0.3)

    def test_loop_max_importance_with_nan(self) -> None:
        """Test max_importance handles NaN values correctly."""
        behavior = np.array([np.nan, 0.1, 0.5, 0.2, 0.3])
        loop = Loop(
            id="B1",
            variables=("stock", "flow"),
            polarity=LoopPolarity.BALANCING,
            behavior_time_series=behavior,
        )

        max_imp = loop.max_importance()
        assert max_imp is not None
        assert max_imp == pytest.approx(0.5)

    def test_loop_average_importance_with_nan_and_negative(self) -> None:
        """Test average_importance with NaN and negative values."""
        behavior = np.array([np.nan, np.nan, -0.5, 0.3, 0.4])
        loop = Loop(
            id="R1",
            variables=("x", "y"),
            polarity=LoopPolarity.REINFORCING,
            behavior_time_series=behavior,
        )

        avg = loop.average_importance()
        assert avg is not None
        assert avg == pytest.approx(0.4)

    def test_loop_average_importance_all_nan(self) -> None:
        """Test average_importance returns NaN when all values are NaN."""
        behavior = np.array([np.nan, np.nan, np.nan])
        loop = Loop(
            id="R1",
            variables=("a", "b"),
            polarity=LoopPolarity.REINFORCING,
            behavior_time_series=behavior,
        )

        avg = loop.average_importance()
        assert avg is not None
        assert np.isnan(avg)

    def test_loop_average_importance_all_nan_no_warning(self) -> None:
        """An all-NaN behavior series must not leak a RuntimeWarning."""
        behavior = np.array([np.nan, np.nan, np.nan])
        loop = Loop(
            id="R1",
            variables=("a", "b"),
            polarity=LoopPolarity.REINFORCING,
            behavior_time_series=behavior,
        )

        with warnings.catch_warnings():
            warnings.simplefilter("error", RuntimeWarning)
            avg = loop.average_importance()
        assert avg is not None
        assert np.isnan(avg)

    def test_loop_max_importance_all_nan_no_warning(self) -> None:
        """An all-NaN behavior series must not leak a RuntimeWarning from max_importance."""
        behavior = np.array([np.nan, np.nan, np.nan])
        loop = Loop(
            id="R1",
            variables=("a", "b"),
            polarity=LoopPolarity.REINFORCING,
            behavior_time_series=behavior,
        )

        with warnings.catch_warnings():
            warnings.simplefilter("error", RuntimeWarning)
            mx = loop.max_importance()
        assert mx is not None
        assert np.isnan(mx)
