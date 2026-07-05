"""Vensim VDF (binary simulation data) import.

VDF is Vensim's binary output format. Two container kinds exist and both are
supported here, auto-detected from the file magic: simulation-run files
(including sensitivity runs) and imported dataset files. Parsing happens in
the Rust engine (``simlin_results_open_vdf``); this module only marshals the
resulting named time series into a pandas DataFrame shaped exactly like
:attr:`simlin.Run.results`.
"""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING, Any

import numpy as np
import pandas as pd

from ._ffi import check_out_error, ffi, free_c_string, lib, string_to_c
from .errors import SimlinImportError

if TYPE_CHECKING:
    from numpy.typing import NDArray

__all__ = ["load_vdf"]


def load_vdf(path: str | Path) -> pd.DataFrame:
    """Load a Vensim VDF simulation-output file as a DataFrame.

    Supports simulation-run files, sensitivity-run files, and imported
    dataset files (the container kind is auto-detected from the file magic).

    The returned DataFrame has the same shape conventions as
    :attr:`Run.results <simlin.Run.results>`: the index is simulation time
    (named ``time``), columns are canonicalized variable names (lowercase,
    spaces become underscores), and arrayed variables appear as one column
    per element named like ``"stock[element]"``. Time points a sparse VDF
    block does not cover are ``NaN``.

    Args:
        path: Path to a ``.vdf`` file

    Returns:
        DataFrame with time as index and one column per saved variable

    Raises:
        SimlinImportError: If the file does not exist
        SimlinRuntimeError: If the file is not a valid VDF (bad magic,
            truncated, or corrupt)

    Example:
        >>> import simlin
        >>> df = simlin.load_vdf("Current.vdf")
        >>> df["water_level"].plot()
        >>> print(df["water_level"].iloc[-1])
    """
    path = Path(path)
    if not path.exists():
        raise SimlinImportError(f"File not found: {path}")

    data = path.read_bytes()
    c_data = ffi.new("uint8_t[]", data)
    err_ptr = ffi.new("SimlinError **")
    results_ptr = lib.simlin_results_open_vdf(c_data, len(data), err_ptr)
    check_out_error(err_ptr, f"Load VDF from {path}")

    try:
        return _results_to_dataframe(results_ptr)
    finally:
        lib.simlin_results_unref(results_ptr)


def _results_to_dataframe(results_ptr: Any) -> pd.DataFrame:
    """Read every named series out of a SimlinResults handle into a DataFrame."""
    err_ptr = ffi.new("SimlinError **")
    step_count_ptr = ffi.new("uintptr_t *")
    lib.simlin_results_get_stepcount(results_ptr, step_count_ptr, err_ptr)
    check_out_error(err_ptr, "Get VDF step count")
    step_count = int(step_count_ptr[0])

    names = _get_var_names(results_ptr)

    if step_count <= 0:
        empty = np.array([], dtype=np.float64)
        df = pd.DataFrame(
            {name: empty.copy() for name in names if name != "time"},
            index=empty,
        )
        df.index.name = "time"
        return df

    time_index = (
        _get_series(results_ptr, "time", step_count)
        if "time" in names
        else np.arange(step_count, dtype=np.float64)
    )

    data = {name: _get_series(results_ptr, name, step_count) for name in names if name != "time"}

    df = pd.DataFrame(data, index=time_index)
    df.index.name = "time"
    return df


def _get_var_names(results_ptr: Any) -> list[str]:
    err_ptr = ffi.new("SimlinError **")
    count_ptr = ffi.new("uintptr_t *")
    lib.simlin_results_get_var_count(results_ptr, count_ptr, err_ptr)
    check_out_error(err_ptr, "Get VDF variable count")

    count = int(count_ptr[0])
    if count == 0:
        return []

    name_ptrs = ffi.new("char *[]", count)
    written_ptr = ffi.new("uintptr_t *")
    err_ptr2 = ffi.new("SimlinError **")
    lib.simlin_results_get_var_names(results_ptr, name_ptrs, count, written_ptr, err_ptr2)
    check_out_error(err_ptr2, "Get VDF variable names")

    names: list[str] = []
    for i in range(written_ptr[0]):
        if name_ptrs[i] != ffi.NULL:
            names.append(ffi.string(name_ptrs[i]).decode("utf-8"))
            free_c_string(name_ptrs[i])
    return names


def _get_series(results_ptr: Any, name: str, step_count: int) -> NDArray[np.float64]:
    c_name = string_to_c(name)
    values = np.zeros(step_count, dtype=np.float64)
    written_ptr = ffi.new("uintptr_t *")
    err_ptr = ffi.new("SimlinError **")
    lib.simlin_results_get_series(
        results_ptr,
        c_name,
        ffi.cast("double *", ffi.from_buffer(values)),
        step_count,
        written_ptr,
        err_ptr,
    )
    check_out_error(err_ptr, f"Get VDF series for '{name}'")
    return values
