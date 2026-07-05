"""Tests for Vensim VDF import (simlin.load_vdf)."""

from __future__ import annotations

import math
import os
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

import simlin
from simlin.errors import SimlinImportError, SimlinRuntimeError


def _repo_root() -> Path:
    """Walk up to the repo root (tests -> pysimlin -> src -> root), honoring
    SIMLIN_REPO_ROOT for CI consistency with conftest.get_repo_root."""
    if env_root := os.environ.get("SIMLIN_REPO_ROOT"):
        return Path(env_root)
    return Path(__file__).parent.parent.parent.parent


def vdf_path(*parts: str) -> Path:
    return _repo_root() / "test" / "bobby" / "vdf" / Path(*parts)


class TestLoadVdfRunFile:
    """Scalar simulation-run VDF (water/Current.vdf)."""

    def test_returns_dataframe_with_time_index(self) -> None:
        df = simlin.load_vdf(vdf_path("water", "Current.vdf"))
        assert isinstance(df, pd.DataFrame)
        assert df.index.name == "time"
        assert len(df) == 21
        assert df.index[0] == 0.0
        assert df.index[-1] == 20.0

    def test_pins_series_values(self) -> None:
        df = simlin.load_vdf(vdf_path("water", "Current.vdf"))

        # Pinned Vensim outputs: water_level rises toward the desired level
        # of 1.0 with adjustment time 2 and dt 1.
        water_level = df["water_level"]
        assert water_level.iloc[0] == 0.0
        assert water_level.iloc[10] == pytest.approx(0.9500000476837158, abs=1e-12)
        assert water_level.iloc[-1] == pytest.approx(0.999951183795929, abs=1e-12)

        gap = df["gap"]
        assert gap.iloc[0] == pytest.approx(1.0, abs=1e-12)
        assert gap.iloc[10] == pytest.approx(0.04999995231628418, abs=1e-12)

        assert df["desired_water_level"].iloc[0] == 1.0

    def test_columns_are_canonicalized(self) -> None:
        df = simlin.load_vdf(vdf_path("water", "Current.vdf"))
        assert "water_level" in df.columns
        assert "desired_water_level" in df.columns
        # time is the index, never a column.
        assert "time" not in df.columns


class TestLoadVdfArrayedRunFile:
    """Arrayed simulation-run VDF (subscripts/subscripts.vdf)."""

    def test_arrayed_columns_use_element_labels(self) -> None:
        df = simlin.load_vdf(vdf_path("subscripts", "subscripts.vdf"))

        # The engine canonicalizes arrayed columns as "name[element]".
        for col in ["a_stock[a]", "a_stock[b]", "a_stock[c]"]:
            assert col in df.columns, f"missing column {col}"
        for col in ["net_flow[a]", "net_flow[b]", "net_flow[c]"]:
            assert col in df.columns, f"missing column {col}"

    def test_pins_arrayed_series_values(self) -> None:
        df = simlin.load_vdf(vdf_path("subscripts", "subscripts.vdf"))

        assert len(df) == 24
        assert df["a_stock[a]"].iloc[0] == 1.0
        assert df["a_stock[b]"].iloc[0] == 2.0
        assert df["a_stock[c]"].iloc[0] == 3.0
        assert df["a_stock[a]"].iloc[-1] == pytest.approx(456400.21875)
        assert df["a_stock[c]"].iloc[-1] == pytest.approx(741651.75)
        assert df["other_const[b]"].iloc[0] == pytest.approx(0.30000001192092896)


class TestLoadVdfStaleNameTable:
    """econ/risk2.vdf has stale name-table entries (GH #839): before the fix
    the record keyed to 'perceived inflation rate' resolved to the
    shifted-by-one name 'inflation elasticity of risky behavior' (a constant
    5.0). Guard the fix end-to-end through the FFI."""

    def test_perceived_inflation_rate_is_correctly_labeled(self) -> None:
        df = simlin.load_vdf(vdf_path("econ", "risk2.vdf"))

        series = df["perceived_inflation_rate"]
        assert series.iloc[0] == pytest.approx(1.5288300514221191)
        assert series.iloc[-1] == pytest.approx(-2.4034740924835205)
        # The mislabeling would have made this a constant; it is not.
        assert series.nunique() > 100

        # The name it was previously confused with is a real constant.
        assert df["inflation_elasticity_of_risky_behavior"].iloc[0] == 5.0
        assert df["inflation_elasticity_of_risky_behavior"].nunique() == 1


class TestLoadVdfDataset:
    """Dataset-container VDF (econ/data.vdf), auto-detected by magic."""

    def test_dataset_series_pinned_values(self) -> None:
        df = simlin.load_vdf(vdf_path("econ", "data.vdf"))

        assert len(df) == 225
        assert df.index[0] == pytest.approx(1990.0)
        assert df.index[-1] == pytest.approx(2008.6700439453125)

        # Pinned from the engine's test_dataset_vdf_extracts_reference_series.
        cpi = df["consumer_price_index"]
        assert cpi.iloc[0] == pytest.approx(127.4000015258789)
        assert cpi.iloc[-1] == pytest.approx(218.7830047607422)

        fed_funds = df["federal_funds_rate"]
        assert fed_funds.iloc[0] == pytest.approx(8.229999542236328)
        assert fed_funds.iloc[-1] == pytest.approx(1.809999942779541)

        # Leading NaN (no stored value before the first data point).
        inflation = df["inflation_rate"]
        assert math.isnan(inflation.iloc[0])
        assert inflation.iloc[12] == pytest.approx(5.38116979598999)

    def test_dataset_columns(self) -> None:
        df = simlin.load_vdf(vdf_path("econ", "data.vdf"))
        for col in [
            "consumer_price_index",
            "federal_funds_rate",
            "home_price_index",
            "real_inflation_rate",
        ]:
            assert col in df.columns


class TestLoadVdfMalformedInput:
    """Malformed input must raise a Python exception, never crash."""

    def test_missing_file_raises_import_error(self, tmp_path: Path) -> None:
        with pytest.raises(SimlinImportError, match="not found"):
            simlin.load_vdf(tmp_path / "nope.vdf")

    def test_empty_file_raises(self, tmp_path: Path) -> None:
        empty = tmp_path / "empty.vdf"
        empty.write_bytes(b"")
        with pytest.raises(SimlinRuntimeError, match="magic"):
            simlin.load_vdf(empty)

    def test_garbage_bytes_raise(self, tmp_path: Path) -> None:
        garbage = tmp_path / "garbage.vdf"
        garbage.write_bytes(bytes([0xAB] * 512))
        with pytest.raises(SimlinRuntimeError):
            simlin.load_vdf(garbage)

    def test_truncated_run_file_raises_or_degrades(self, tmp_path: Path) -> None:
        data = vdf_path("water", "Current.vdf").read_bytes()
        for length in [4, 16, 100, 0x80, 1000]:
            truncated = tmp_path / f"truncated_{length}.vdf"
            truncated.write_bytes(data[:length])
            # Short prefixes must raise; a long-enough prefix may still parse
            # (degraded), but must never abort the interpreter.
            try:
                df = simlin.load_vdf(truncated)
            except SimlinRuntimeError:
                continue
            assert isinstance(df, pd.DataFrame)

    def test_truncated_dataset_file_raises_or_degrades(self, tmp_path: Path) -> None:
        data = vdf_path("econ", "data.vdf").read_bytes()
        for length in [4, 16, 100, 0x80, 1000]:
            truncated = tmp_path / f"truncated_{length}.vdf"
            truncated.write_bytes(data[:length])
            try:
                df = simlin.load_vdf(truncated)
            except SimlinRuntimeError:
                continue
            assert isinstance(df, pd.DataFrame)

    def test_run_magic_on_dataset_body_raises_not_crashes(self, tmp_path: Path) -> None:
        # Force the dataset body down the run-file parser by rewriting the
        # magic: the mismatched section layout must surface as an error (or
        # a degraded parse), never a crash.
        data = bytearray(vdf_path("econ", "data.vdf").read_bytes())
        data[0:4] = bytes([0x7F, 0xF7, 0x17, 0x52])
        crossed = tmp_path / "crossed.vdf"
        crossed.write_bytes(bytes(data))
        try:
            df = simlin.load_vdf(crossed)
        except SimlinRuntimeError:
            return
        assert isinstance(df, pd.DataFrame)

    def test_dataset_magic_on_run_body_raises_not_crashes(self, tmp_path: Path) -> None:
        data = bytearray(vdf_path("water", "Current.vdf").read_bytes())
        data[0:4] = bytes([0x7F, 0xF7, 0x17, 0x41])
        crossed = tmp_path / "crossed.vdf"
        crossed.write_bytes(bytes(data))
        try:
            df = simlin.load_vdf(crossed)
        except SimlinRuntimeError:
            return
        assert isinstance(df, pd.DataFrame)

    def test_zero_time_point_run_file_raises_not_aborts(self, tmp_path: Path) -> None:
        # Coordinated corruption: header time-point count (0x78) and the
        # Time block's u16 count both zeroed. Before the zero-step guard
        # this reached an index panic in the engine's build_results, which
        # under the release panic=abort profile would take the interpreter
        # down with it (catch_unwind cannot intercept an abort).
        data = bytearray(vdf_path("water", "Current.vdf").read_bytes())
        offset_table_start = int.from_bytes(data[0x60:0x64], "little")
        time_block = int.from_bytes(data[offset_table_start : offset_table_start + 4], "little")
        data[0x78:0x7C] = (0).to_bytes(4, "little")
        data[time_block : time_block + 2] = (0).to_bytes(2, "little")
        zero_step = tmp_path / "zero_step.vdf"
        zero_step.write_bytes(bytes(data))
        with pytest.raises(SimlinRuntimeError, match="zero saved time points"):
            simlin.load_vdf(zero_step)


class TestLoadVdfDataFrameConventions:
    """The DataFrame must match Run.results conventions."""

    def test_matches_run_results_shape_conventions(self) -> None:
        df = simlin.load_vdf(vdf_path("water", "Current.vdf"))
        assert df.index.name == "time"
        assert df.index.dtype == np.float64
        for dtype in df.dtypes:
            assert dtype == np.float64

    def test_accepts_str_and_path(self) -> None:
        p = vdf_path("water", "Current.vdf")
        df_from_path = simlin.load_vdf(p)
        df_from_str = simlin.load_vdf(str(p))
        pd.testing.assert_frame_equal(df_from_path, df_from_str)
