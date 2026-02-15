"""Simulation class for running system dynamics simulations.

Thread-safety: Each ``Sim`` owns a ``threading.Lock`` that protects
its ``_ptr`` and ``_ran`` flag.  The underlying Rust layer also uses a
per-object Mutex for the simulation state, so the Python lock only
guards Python-level invariants.
"""

from __future__ import annotations

import threading
from typing import TYPE_CHECKING, Any, Self

import numpy as np

from ._ffi import (
    _register_finalizer,
    c_to_string,
    check_out_error,
    ffi,
    free_c_string,
    lib,
    string_to_c,
)
from .analysis import Link, LinkPolarity
from .errors import SimlinRuntimeError

if TYPE_CHECKING:
    from types import TracebackType

    from numpy.typing import NDArray

    from .model import Model
    from .run import Run


class Sim:
    """Represents a simulation instance.

    A Sim object manages the execution of a system dynamics simulation,
    including running the simulation, accessing results, and performing
    interventions during the run.

    Thread-safety: individual instances are safe to use from multiple
    threads.  All public methods acquire an internal lock before
    touching ``_ptr``.
    """

    def __init__(self, ptr: Any, model: Model, overrides: dict[str, float] | None = None) -> None:
        """Initialize a Sim from a C pointer and Model reference."""
        if ptr == ffi.NULL:
            raise ValueError("Cannot create Sim from NULL pointer")
        self._lock = threading.Lock()
        self._ptr = ptr
        self._model = model
        self._overrides: dict[str, float] = overrides or {}
        self._ran = False
        _register_finalizer(self, lib.simlin_sim_unref, ptr)

    def _check_alive(self) -> None:
        """Raise if the underlying C object has been freed.

        Must be called while ``_lock`` is held.
        """
        if self._ptr == ffi.NULL:
            raise SimlinRuntimeError("Sim has been closed")

    @property
    def time(self) -> float:
        """Current simulation time.

        Returns:
            Current time value

        Raises:
            SimlinRuntimeError: If unable to get current time
        """
        try:
            return self.get_value("time")
        except SimlinRuntimeError:
            return 0.0

    def run_to(self, time: float) -> None:
        """Run the simulation to the specified time.

        Args:
            time: The simulation time to run to

        Raises:
            SimlinRuntimeError: If the simulation fails
        """
        with self._lock:
            self._check_alive()
            err_ptr = ffi.new("SimlinError **")
            lib.simlin_sim_run_to(self._ptr, time, err_ptr)
            check_out_error(err_ptr, f"Run simulation to time {time}")
            self._ran = True

    def run_to_end(self) -> None:
        """Run the simulation to completion.

        Runs the simulation from its current state to the final time
        specified in the model's simulation specifications.

        Raises:
            SimlinRuntimeError: If the simulation fails
        """
        with self._lock:
            self._check_alive()
            err_ptr = ffi.new("SimlinError **")
            lib.simlin_sim_run_to_end(self._ptr, err_ptr)
            check_out_error(err_ptr, "Run simulation to end")
            self._ran = True

    def reset(self) -> None:
        """Reset the simulation to its initial state.

        This allows re-running the simulation with different parameters
        or interventions.

        Raises:
            SimlinRuntimeError: If the reset fails
        """
        with self._lock:
            self._check_alive()
            err_ptr = ffi.new("SimlinError **")
            lib.simlin_sim_reset(self._ptr, err_ptr)
            check_out_error(err_ptr, "Reset simulation")
            self._ran = False

    def get_var_names(self) -> list[str]:
        """Return the simulation's flattened variable names."""
        with self._lock:
            self._check_alive()
            count_ptr = ffi.new("uintptr_t *")
            err_ptr = ffi.new("SimlinError **")
            lib.simlin_sim_get_var_count(self._ptr, count_ptr, err_ptr)
            check_out_error(err_ptr, "Get sim variable count")

            count = int(count_ptr[0])
            if count == 0:
                return []

            name_ptrs = ffi.new("char *[]", count)
            written_ptr = ffi.new("uintptr_t *")
            err_ptr2 = ffi.new("SimlinError **")
            lib.simlin_sim_get_var_names(
                self._ptr, name_ptrs, count, written_ptr, err_ptr2
            )
            check_out_error(err_ptr2, "Get sim variable names")

            names: list[str] = []
            for i in range(written_ptr[0]):
                if name_ptrs[i] != ffi.NULL:
                    name = ffi.string(name_ptrs[i]).decode("utf-8")
                    free_c_string(name_ptrs[i])
                    names.append(name)
            return names

    def get_step_count(self) -> int:
        """Get the number of time steps in the simulation results.

        Returns:
            The number of time steps

        Raises:
            SimlinRuntimeError: If the operation fails
        """
        with self._lock:
            self._check_alive()
            count_ptr = ffi.new("uintptr_t *")
            err_ptr = ffi.new("SimlinError **")
            lib.simlin_sim_get_stepcount(self._ptr, count_ptr, err_ptr)
            check_out_error(err_ptr, "Get step count")
            return int(count_ptr[0])

    def get_value(self, name: str) -> float:
        """Get the current value of a variable.

        Args:
            name: The variable name

        Returns:
            The current value

        Raises:
            SimlinRuntimeError: If the variable doesn't exist
        """
        with self._lock:
            self._check_alive()
            c_name = string_to_c(name)
            value_ptr = ffi.new("double *")
            err_ptr = ffi.new("SimlinError **")

            lib.simlin_sim_get_value(self._ptr, c_name, value_ptr, err_ptr)
            check_out_error(err_ptr, f"Get value for '{name}'")

            return float(value_ptr[0])

    def set_value(self, name: str, value: float) -> None:
        """Set the value of a variable.

        The behavior depends on the simulation state:
        - Before first run_to: Sets initial value
        - During simulation: Sets value for next iteration
        - After run_to_end: Raises error

        Args:
            name: The variable name
            value: The new value

        Raises:
            SimlinRuntimeError: If the variable doesn't exist or can't be set
        """
        with self._lock:
            self._check_alive()
            c_name = string_to_c(name)
            err_ptr = ffi.new("SimlinError **")
            lib.simlin_sim_set_value(self._ptr, c_name, value, err_ptr)
            check_out_error(err_ptr, f"Set value for '{name}'")

    def get_series(self, name: str) -> NDArray[np.float64]:
        """Get the time series for a variable.

        Args:
            name: The variable name

        Returns:
            NumPy array of values over time

        Raises:
            SimlinRuntimeError: If the variable doesn't exist
        """
        with self._lock:
            self._check_alive()
            step_count = self._get_step_count_unlocked()
            if step_count <= 0:
                return np.array([], dtype=np.float64)

            c_name = string_to_c(name)
            results = np.zeros(step_count, dtype=np.float64)
            out_written_ptr = ffi.new("uintptr_t *")
            err_ptr = ffi.new("SimlinError **")

            lib.simlin_sim_get_series(
                self._ptr,
                c_name,
                ffi.cast("double *", ffi.from_buffer(results)),
                step_count,
                out_written_ptr,
                err_ptr,
            )
            check_out_error(err_ptr, f"Get series for '{name}'")

            return results

    def _get_step_count_unlocked(self) -> int:
        """Get step count without acquiring the lock.  Caller must hold ``_lock``."""
        count_ptr = ffi.new("uintptr_t *")
        err_ptr = ffi.new("SimlinError **")
        lib.simlin_sim_get_stepcount(self._ptr, count_ptr, err_ptr)
        check_out_error(err_ptr, "Get step count")
        return int(count_ptr[0])

    def get_links(self) -> list[Link]:
        """Get all causal links from the simulation.

        If the simulation was run with LTM enabled, link scores will be included.

        Returns:
            List of Link objects with optional score data
        """
        with self._lock:
            self._check_alive()
            err_ptr = ffi.new("SimlinError **")
            links_ptr = lib.simlin_analyze_get_links(self._ptr, err_ptr)
            check_out_error(err_ptr, "Get links")

        if links_ptr == ffi.NULL:
            return []

        try:
            if links_ptr.count == 0:
                return []

            links = []
            for i in range(links_ptr.count):
                c_link = links_ptr.links[i]

                # Convert score array if present
                score = None
                if c_link.score_len > 0 and c_link.score != ffi.NULL:
                    score = np.zeros(c_link.score_len, dtype=np.float64)
                    for j in range(c_link.score_len):
                        score[j] = c_link.score[j]

                link = Link(
                    from_var=c_to_string(getattr(c_link, "from")) or "",
                    to_var=c_to_string(c_link.to) or "",
                    polarity=LinkPolarity(c_link.polarity),
                    score=score,
                )
                links.append(link)

            return links

        finally:
            lib.simlin_free_links(links_ptr)

    def get_relative_loop_score(self, loop_id: str) -> NDArray[np.float64]:
        """Get the relative loop score time series for a specific loop.

        This requires the simulation to have been run with enable_ltm=True.

        Args:
            loop_id: The identifier of the loop

        Returns:
            NumPy array of relative loop scores over time

        Raises:
            SimlinRuntimeError: If LTM wasn't enabled or loop doesn't exist
        """
        with self._lock:
            self._check_alive()
            step_count = self._get_step_count_unlocked()
            if step_count <= 0:
                return np.array([], dtype=np.float64)

            c_loop_id = string_to_c(loop_id)
            results = np.zeros(step_count, dtype=np.float64)
            out_written_ptr = ffi.new("uintptr_t *")
            err_ptr = ffi.new("SimlinError **")

            lib.simlin_analyze_get_relative_loop_score(
                self._ptr,
                c_loop_id,
                ffi.cast("double *", ffi.from_buffer(results)),
                step_count,
                out_written_ptr,
                err_ptr,
            )
            check_out_error(err_ptr, f"Get relative loop score for '{loop_id}'")

            return results

    def get_run(self) -> Run:
        """Get simulation results as a Run object.

        Loop analysis is included if the simulation was created with enable_ltm=True.
        Can be called before run_to_end() to get partial results.

        Returns:
            Run object with results and analysis

        Example:
            >>> with model.simulate(enable_ltm=True) as sim:
            ...     sim.run_to_end()
            ...     run = sim.get_run()
            ...     print(run.dominant_periods)
        """
        from .run import Run

        loops_structural = self._model.loops
        return Run(self, self._overrides, loops_structural)

    def __enter__(self) -> Self:
        """Context manager entry point."""
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: TracebackType | None,
    ) -> None:
        """Context manager exit point with explicit cleanup."""
        with self._lock:
            finalizer = getattr(self, "_finalizer", None)
            if finalizer and getattr(finalizer, "alive", False):
                finalizer()
            self._ptr = ffi.NULL

    def __repr__(self) -> str:
        """Return a string representation of the Sim."""
        try:
            step_count = self.get_step_count()
            return f"<Sim with {step_count} time step(s)>"
        except Exception:
            return "<Sim (not run)>"
