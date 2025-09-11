"""Tests for analysis types."""

import pytest
import numpy as np
from simlin import LinkPolarity, LoopPolarity, Link, Loop


class TestLinkPolarity:
    """Test LinkPolarity enum."""
    
    def test_link_polarity_values(self) -> None:
        """Test that link polarities have expected values."""
        assert LinkPolarity.POSITIVE == 0
        assert LinkPolarity.NEGATIVE == 1
        assert LinkPolarity.UNKNOWN == 2
    
    def test_link_polarity_str(self) -> None:
        """Test string representation of link polarities."""
        assert str(LinkPolarity.POSITIVE) == "+"
        assert str(LinkPolarity.NEGATIVE) == "-"
        assert str(LinkPolarity.UNKNOWN) == "?"


class TestLoopPolarity:
    """Test LoopPolarity enum."""
    
    def test_loop_polarity_values(self) -> None:
        """Test that loop polarities have expected values."""
        assert LoopPolarity.REINFORCING == 0
        assert LoopPolarity.BALANCING == 1
    
    def test_loop_polarity_str(self) -> None:
        """Test string representation of loop polarities."""
        assert str(LoopPolarity.REINFORCING) == "R"
        assert str(LoopPolarity.BALANCING) == "B"


class TestLink:
    """Test Link dataclass."""
    
    def test_link_creation(self) -> None:
        """Test creating Link instances."""
        link = Link(
            from_var="population",
            to_var="births",
            polarity=LinkPolarity.POSITIVE
        )
        
        assert link.from_var == "population"
        assert link.to_var == "births"
        assert link.polarity == LinkPolarity.POSITIVE
        assert link.score is None
    
    def test_link_with_score(self) -> None:
        """Test Link with LTM score data."""
        scores = np.array([0.1, 0.2, 0.3, 0.4, 0.5])
        link = Link(
            from_var="A",
            to_var="B",
            polarity=LinkPolarity.NEGATIVE,
            score=scores
        )
        
        assert link.has_score()
        assert link.average_score() == pytest.approx(0.3)
        assert link.max_score() == pytest.approx(0.5)
    
    def test_link_without_score(self) -> None:
        """Test Link without score data."""
        link = Link(
            from_var="X",
            to_var="Y",
            polarity=LinkPolarity.UNKNOWN
        )
        
        assert not link.has_score()
        assert link.average_score() is None
        assert link.max_score() is None
    
    def test_link_str(self) -> None:
        """Test string representation of Link."""
        link = Link(
            from_var="input",
            to_var="output",
            polarity=LinkPolarity.POSITIVE
        )
        
        str_repr = str(link)
        assert "input" in str_repr
        assert "output" in str_repr
        assert "+" in str_repr or "--+--" in str_repr


class TestLoop:
    """Test Loop dataclass."""
    
    def test_loop_creation(self) -> None:
        """Test creating Loop instances."""
        loop = Loop(
            id="R1",
            variables=["population", "births", "birth_rate"],
            polarity=LoopPolarity.REINFORCING
        )
        
        assert loop.id == "R1"
        assert len(loop.variables) == 3
        assert loop.polarity == LoopPolarity.REINFORCING
    
    def test_loop_str(self) -> None:
        """Test string representation of Loop."""
        loop = Loop(
            id="B1",
            variables=["stock", "outflow", "desired_stock"],
            polarity=LoopPolarity.BALANCING
        )
        
        str_repr = str(loop)
        assert "B1" in str_repr
        assert "B" in str_repr or "BALANCING" in str_repr
        assert "stock" in str_repr
        assert "->" in str_repr
    
    def test_loop_len(self) -> None:
        """Test __len__ method of Loop."""
        loop = Loop(
            id="L1",
            variables=["A", "B", "C", "D"],
            polarity=LoopPolarity.REINFORCING
        )
        
        assert len(loop) == 4
    
    def test_loop_contains_variable(self) -> None:
        """Test checking if a variable is in the loop."""
        loop = Loop(
            id="L2",
            variables=["var1", "var2", "var3"],
            polarity=LoopPolarity.BALANCING
        )
        
        assert loop.contains_variable("var1")
        assert loop.contains_variable("var2")
        assert loop.contains_variable("var3")
        assert not loop.contains_variable("var4")
        assert not loop.contains_variable("nonexistent")