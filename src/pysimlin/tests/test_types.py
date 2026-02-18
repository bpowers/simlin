"""Tests for types module."""

import pytest

from simlin import (
    Aux,
    DominantPeriod,
    Flow,
    GraphicalFunction,
    GraphicalFunctionScale,
    Stock,
    TimeSpec,
)


class TestTimeSpec:
    """Test TimeSpec dataclass."""

    def test_timespec_creation(self) -> None:
        """Test creating TimeSpec instances."""
        ts = TimeSpec(start=0.0, stop=100.0, dt=0.25)

        assert ts.start == 0.0
        assert ts.stop == 100.0
        assert ts.dt == 0.25
        assert ts.units is None

    def test_timespec_with_units(self) -> None:
        """Test TimeSpec with units."""
        ts = TimeSpec(start=1990.0, stop=2050.0, dt=1.0, units="years")

        assert ts.start == 1990.0
        assert ts.stop == 2050.0
        assert ts.dt == 1.0
        assert ts.units == "years"

    def test_timespec_immutable(self) -> None:
        """Test that TimeSpec is immutable."""
        ts = TimeSpec(start=0.0, stop=10.0, dt=0.1)

        with pytest.raises(AttributeError):
            ts.start = 5.0  # type: ignore


class TestGraphicalFunctionScale:
    """Test GraphicalFunctionScale dataclass."""

    def test_scale_creation(self) -> None:
        """Test creating GraphicalFunctionScale instances."""
        scale = GraphicalFunctionScale(min=0.0, max=100.0)

        assert scale.min == 0.0
        assert scale.max == 100.0

    def test_scale_immutable(self) -> None:
        """Test that GraphicalFunctionScale is immutable."""
        scale = GraphicalFunctionScale(min=0.0, max=100.0)

        with pytest.raises(AttributeError):
            scale.min = 10.0  # type: ignore


class TestGraphicalFunction:
    """Test GraphicalFunction dataclass."""

    def test_graphical_function_creation(self) -> None:
        """Test creating GraphicalFunction instances."""
        x_scale = GraphicalFunctionScale(min=0.0, max=10.0)
        y_scale = GraphicalFunctionScale(min=0.0, max=1.0)

        gf = GraphicalFunction(
            x_points=(0.0, 5.0, 10.0),
            y_points=(0.0, 0.5, 1.0),
            x_scale=x_scale,
            y_scale=y_scale,
        )

        assert gf.x_points == (0.0, 5.0, 10.0)
        assert gf.y_points == (0.0, 0.5, 1.0)
        assert gf.x_scale.min == 0.0
        assert gf.y_scale.max == 1.0
        assert gf.kind == "continuous"

    def test_graphical_function_no_x_points(self) -> None:
        """Test GraphicalFunction with implicit x points."""
        x_scale = GraphicalFunctionScale(min=0.0, max=10.0)
        y_scale = GraphicalFunctionScale(min=0.0, max=1.0)

        gf = GraphicalFunction(
            x_points=None,
            y_points=(0.0, 0.5, 1.0),
            x_scale=x_scale,
            y_scale=y_scale,
            kind="discrete",
        )

        assert gf.x_points is None
        assert gf.y_points == (0.0, 0.5, 1.0)
        assert gf.kind == "discrete"

    def test_graphical_function_immutable(self) -> None:
        """Test that GraphicalFunction is immutable."""
        x_scale = GraphicalFunctionScale(min=0.0, max=10.0)
        y_scale = GraphicalFunctionScale(min=0.0, max=1.0)

        gf = GraphicalFunction(
            x_points=(0.0, 5.0, 10.0),
            y_points=(0.0, 0.5, 1.0),
            x_scale=x_scale,
            y_scale=y_scale,
        )

        with pytest.raises(AttributeError):
            gf.kind = "extrapolate"  # type: ignore

    def test_tuple_immutability(self) -> None:
        """Test that tuple fields are properly immutable."""
        x_scale = GraphicalFunctionScale(min=0.0, max=10.0)
        y_scale = GraphicalFunctionScale(min=0.0, max=1.0)

        gf = GraphicalFunction(
            x_points=(0.0, 5.0, 10.0),
            y_points=(0.0, 0.5, 1.0),
            x_scale=x_scale,
            y_scale=y_scale,
        )

        assert isinstance(gf.x_points, tuple)
        assert isinstance(gf.y_points, tuple)


class TestStock:
    """Test Stock dataclass."""

    def test_stock_creation(self) -> None:
        """Test creating Stock instances."""
        stock = Stock(
            name="population",
            initial_equation="1000",
            inflows=("births",),
            outflows=("deaths",),
        )

        assert stock.name == "population"
        assert stock.initial_equation == "1000"
        assert stock.inflows == ("births",)
        assert stock.outflows == ("deaths",)
        assert stock.units is None
        assert stock.documentation is None
        assert stock.dimensions == ()
        assert stock.non_negative is False

    def test_stock_with_all_fields(self) -> None:
        """Test Stock with all fields populated."""
        stock = Stock(
            name="inventory",
            initial_equation="100",
            inflows=("production", "returns"),
            outflows=("sales", "waste"),
            units="items",
            documentation="Inventory on hand",
            dimensions=("region",),
            non_negative=True,
        )

        assert stock.name == "inventory"
        assert stock.initial_equation == "100"
        assert stock.inflows == ("production", "returns")
        assert stock.outflows == ("sales", "waste")
        assert stock.units == "items"
        assert stock.documentation == "Inventory on hand"
        assert stock.dimensions == ("region",)
        assert stock.non_negative is True

    def test_stock_immutable(self) -> None:
        """Test that Stock is immutable."""
        stock = Stock(
            name="population",
            initial_equation="1000",
            inflows=("births",),
            outflows=("deaths",),
        )

        with pytest.raises(AttributeError):
            stock.name = "new_name"  # type: ignore

    def test_stock_tuple_fields(self) -> None:
        """Test that Stock uses tuples for sequence fields."""
        stock = Stock(
            name="population",
            initial_equation="1000",
            inflows=("births",),
            outflows=("deaths",),
            dimensions=("region", "age_group"),
        )

        assert isinstance(stock.inflows, tuple)
        assert isinstance(stock.outflows, tuple)
        assert isinstance(stock.dimensions, tuple)


class TestFlow:
    """Test Flow dataclass."""

    def test_flow_creation(self) -> None:
        """Test creating Flow instances."""
        flow = Flow(name="births", equation="population * birth_rate")

        assert flow.name == "births"
        assert flow.equation == "population * birth_rate"
        assert flow.units is None
        assert flow.documentation is None
        assert flow.dimensions == ()
        assert flow.non_negative is False
        assert flow.graphical_function is None

    def test_flow_with_graphical_function(self) -> None:
        """Test Flow with graphical function."""
        x_scale = GraphicalFunctionScale(min=0.0, max=100.0)
        y_scale = GraphicalFunctionScale(min=0.0, max=1.0)
        gf = GraphicalFunction(
            x_points=(0.0, 50.0, 100.0),
            y_points=(0.0, 0.5, 1.0),
            x_scale=x_scale,
            y_scale=y_scale,
        )

        flow = Flow(
            name="adjustment",
            equation="WITH LOOKUP(time, table)",
            graphical_function=gf,
        )

        assert flow.name == "adjustment"
        assert flow.graphical_function is not None
        assert flow.graphical_function.y_points == (0.0, 0.5, 1.0)

    def test_flow_immutable(self) -> None:
        """Test that Flow is immutable."""
        flow = Flow(name="births", equation="population * birth_rate")

        with pytest.raises(AttributeError):
            flow.equation = "new_equation"  # type: ignore

    def test_flow_tuple_dimensions(self) -> None:
        """Test that Flow uses tuples for dimensions."""
        flow = Flow(
            name="production",
            equation="capacity * utilization",
            dimensions=("region",),
        )

        assert isinstance(flow.dimensions, tuple)
        assert flow.dimensions == ("region",)


class TestAux:
    """Test Aux dataclass."""

    def test_aux_creation(self) -> None:
        """Test creating Aux instances."""
        aux = Aux(name="birth_rate", equation="0.03")

        assert aux.name == "birth_rate"
        assert aux.equation == "0.03"
        assert aux.active_initial is None
        assert aux.units is None
        assert aux.documentation is None
        assert aux.dimensions == ()
        assert aux.graphical_function is None

    def test_aux_with_active_initial(self) -> None:
        """Test Aux with active initial (for variables with memory)."""
        aux = Aux(
            name="smoothed_value",
            equation="SMOOTH3(input, delay_time)",
            active_initial="10",
            units="widgets",
            documentation="3rd order exponential smooth",
        )

        assert aux.name == "smoothed_value"
        assert aux.equation == "SMOOTH3(input, delay_time)"
        assert aux.active_initial == "10"
        assert aux.units == "widgets"
        assert aux.documentation == "3rd order exponential smooth"

    def test_aux_immutable(self) -> None:
        """Test that Aux is immutable."""
        aux = Aux(name="rate", equation="0.05")

        with pytest.raises(AttributeError):
            aux.name = "new_name"  # type: ignore

    def test_aux_tuple_dimensions(self) -> None:
        """Test that Aux uses tuples for dimensions."""
        aux = Aux(
            name="productivity",
            equation="base_productivity * factor",
            dimensions=("sector", "region"),
        )

        assert isinstance(aux.dimensions, tuple)
        assert aux.dimensions == ("sector", "region")


class TestDominantPeriod:
    """Test DominantPeriod dataclass."""

    def test_dominant_period_creation(self) -> None:
        """Test creating DominantPeriod instances."""
        period = DominantPeriod(
            dominant_loops=("R1", "R2"),
            start_time=0.0,
            end_time=25.0,
        )

        assert period.dominant_loops == ("R1", "R2")
        assert period.start_time == 0.0
        assert period.end_time == 25.0

    def test_dominant_period_duration(self) -> None:
        """Test DominantPeriod duration calculation."""
        period = DominantPeriod(
            dominant_loops=("B1",),
            start_time=10.0,
            end_time=35.0,
        )

        assert period.duration() == 25.0

    def test_dominant_period_contains_loop(self) -> None:
        """Test checking if a loop is in the dominant set."""
        period = DominantPeriod(
            dominant_loops=("R1", "B2", "R3"),
            start_time=0.0,
            end_time=10.0,
        )

        assert period.contains_loop("R1")
        assert period.contains_loop("B2")
        assert period.contains_loop("R3")
        assert not period.contains_loop("B1")
        assert not period.contains_loop("R4")

    def test_dominant_period_immutable(self) -> None:
        """Test that DominantPeriod is immutable."""
        period = DominantPeriod(
            dominant_loops=("R1",),
            start_time=0.0,
            end_time=10.0,
        )

        with pytest.raises(AttributeError):
            period.start_time = 5.0  # type: ignore

    def test_dominant_period_tuple_loops(self) -> None:
        """Test that dominant_loops is a tuple."""
        period = DominantPeriod(
            dominant_loops=("R1", "R2"),
            start_time=0.0,
            end_time=10.0,
        )

        assert isinstance(period.dominant_loops, tuple)
