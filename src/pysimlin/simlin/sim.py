"""Simulation class for running system dynamics simulations."""

from typing import List, Optional, Dict, Any, TYPE_CHECKING, Self
from types import TracebackType
import numpy as np
from numpy.typing import NDArray
import pandas as pd

from ._ffi import ffi, lib, string_to_c, c_to_string, check_out_error, _register_finalizer
from .errors import SimlinRuntimeError, ErrorCode
from .analysis import Link, LinkPolarity

if TYPE_CHECKING:
    from .model import Model


class Sim:
    """
    Represents a simulation instance.
    
    A Sim object manages the execution of a system dynamics simulation,
    including running the simulation, accessing results, and performing
    interventions during the run.
    """
    
    def __init__(self, ptr: Any, model: "Model", overrides: Optional[Dict[str, float]] = None) -> None:
        """Initialize a Sim from a C pointer and Model reference."""
        if ptr == ffi.NULL:
            raise ValueError("Cannot create Sim from NULL pointer")
        self._ptr = ptr
        self._model = model
        self._overrides: Dict[str, float] = overrides or {}
        self._ran = False
        _register_finalizer(self, lib.simlin_sim_unref, ptr)

    @property
    def time(self) -> float:
        """
        Current simulation time.

        Returns:
            Current time value

        Raises:
            SimlinRuntimeError: If unable to get current time
        """
        try:
            return self.get_value('time')
        except SimlinRuntimeError:
            return 0.0
    
    def run_to(self, time: float) -> None:
        """
        Run the simulation to the specified time.

        Args:
            time: The simulation time to run to

        Raises:
            SimlinRuntimeError: If the simulation fails
        """
        err_ptr = ffi.new("SimlinError **")
        lib.simlin_sim_run_to(self._ptr, time, err_ptr)
        check_out_error(err_ptr, f"Run simulation to time {time}")
        self._ran = True
    
    def run_to_end(self) -> None:
        """
        Run the simulation to completion.

        Runs the simulation from its current state to the final time
        specified in the model's simulation specifications.

        Raises:
            SimlinRuntimeError: If the simulation fails
        """
        err_ptr = ffi.new("SimlinError **")
        lib.simlin_sim_run_to_end(self._ptr, err_ptr)
        check_out_error(err_ptr, "Run simulation to end")
        self._ran = True
    
    def reset(self) -> None:
        """
        Reset the simulation to its initial state.

        This allows re-running the simulation with different parameters
        or interventions.

        Raises:
            SimlinRuntimeError: If the reset fails
        """
        err_ptr = ffi.new("SimlinError **")
        lib.simlin_sim_reset(self._ptr, err_ptr)
        check_out_error(err_ptr, "Reset simulation")
        self._ran = False

    def get_var_names(self) -> list[str]:
        """Return the model's variable names (convenience method)."""
        return [v.name for v in self._model.variables]
    
    def get_step_count(self) -> int:
        """
        Get the number of time steps in the simulation results.

        Returns:
            The number of time steps

        Raises:
            SimlinRuntimeError: If the operation fails
        """
        count_ptr = ffi.new("uintptr_t *")
        err_ptr = ffi.new("SimlinError **")
        lib.simlin_sim_get_stepcount(self._ptr, count_ptr, err_ptr)
        check_out_error(err_ptr, "Get step count")
        return int(count_ptr[0])
    
    def get_value(self, name: str) -> float:
        """
        Get the current value of a variable.

        Args:
            name: The variable name

        Returns:
            The current value

        Raises:
            SimlinRuntimeError: If the variable doesn't exist
        """
        c_name = string_to_c(name)
        value_ptr = ffi.new("double *")
        err_ptr = ffi.new("SimlinError **")

        lib.simlin_sim_get_value(self._ptr, c_name, value_ptr, err_ptr)
        check_out_error(err_ptr, f"Get value for '{name}'")

        return float(value_ptr[0])
    
    def set_value(self, name: str, value: float) -> None:
        """
        Set the value of a variable.

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
        c_name = string_to_c(name)
        err_ptr = ffi.new("SimlinError **")
        lib.simlin_sim_set_value(self._ptr, c_name, value, err_ptr)
        check_out_error(err_ptr, f"Set value for '{name}'")
    
    def get_series(self, name: str) -> NDArray[np.float64]:
        """
        Get the time series for a variable.

        Args:
            name: The variable name

        Returns:
            NumPy array of values over time

        Raises:
            SimlinRuntimeError: If the variable doesn't exist
        """
        step_count = self.get_step_count()
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
            err_ptr
        )
        check_out_error(err_ptr, f"Get series for '{name}'")

        return results

    def get_links(self) -> List[Link]:
        """
        Get all causal links from the simulation.

        If the simulation was run with LTM enabled, link scores will be included.

        Returns:
            List of Link objects with optional score data
        """
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
                    from_var=c_to_string(getattr(c_link, 'from')) or "",
                    to_var=c_to_string(c_link.to) or "",
                    polarity=LinkPolarity(c_link.polarity),
                    score=score
                )
                links.append(link)

            return links

        finally:
            lib.simlin_free_links(links_ptr)
    
    def get_relative_loop_score(self, loop_id: str) -> NDArray[np.float64]:
        """
        Get the relative loop score time series for a specific loop.

        This requires the simulation to have been run with enable_ltm=True.

        Args:
            loop_id: The identifier of the loop

        Returns:
            NumPy array of relative loop scores over time

        Raises:
            SimlinRuntimeError: If LTM wasn't enabled or loop doesn't exist
        """
        step_count = self.get_step_count()
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
            err_ptr
        )
        check_out_error(err_ptr, f"Get relative loop score for '{loop_id}'")

        return results

    def get_run(self) -> "Run":
        """
        Get simulation results as a Run object.

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
    
    def __exit__(self, exc_type: Optional[type[BaseException]], exc_val: Optional[BaseException], exc_tb: Optional[TracebackType]) -> None:
        """Context manager exit point with explicit cleanup."""
        finalizer = getattr(self, "_finalizer", None)
        if finalizer and getattr(finalizer, "alive", False):
            finalizer()
        self._ptr = ffi.NULL
    
    def __repr__(self) -> str:
        """Return a string representation of the Sim."""
        try:
            step_count = self.get_step_count()
            return f"<Sim with {step_count} time step(s)>"
        except:
            return "<Sim (not run)>"
