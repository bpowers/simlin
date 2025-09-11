"""Simulation class for running system dynamics simulations."""

from typing import List, Optional, Dict, Any, TYPE_CHECKING, Self
from types import TracebackType
import numpy as np
from numpy.typing import NDArray
import pandas as pd

from ._ffi import ffi, lib, string_to_c, c_to_string, check_error, _register_finalizer
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
    
    def __init__(self, ptr: Any, model: "Model") -> None:
        """Initialize a Sim from a C pointer and Model reference."""
        if ptr == ffi.NULL:
            raise ValueError("Cannot create Sim from NULL pointer")
        self._ptr = ptr
        self._model = model
        self._ran = False
        _register_finalizer(self, lib.simlin_sim_unref, ptr)
    
    def run_to(self, time: float) -> None:
        """
        Run the simulation to the specified time.
        
        Args:
            time: The simulation time to run to
            
        Raises:
            SimlinRuntimeError: If the simulation fails
        """
        result = lib.simlin_sim_run_to(self._ptr, time)
        check_error(result, f"Run simulation to time {time}")
        self._ran = True
    
    def run_to_end(self) -> None:
        """
        Run the simulation to completion.
        
        Runs the simulation from its current state to the final time
        specified in the model's simulation specifications.
        
        Raises:
            SimlinRuntimeError: If the simulation fails
        """
        result = lib.simlin_sim_run_to_end(self._ptr)
        check_error(result, "Run simulation to end")
        self._ran = True
    
    def reset(self) -> None:
        """
        Reset the simulation to its initial state.
        
        This allows re-running the simulation with different parameters
        or interventions.
        
        Raises:
            SimlinRuntimeError: If the reset fails
        """
        result = lib.simlin_sim_reset(self._ptr)
        check_error(result, "Reset simulation")
        self._ran = False

    def get_var_names(self) -> list[str]:
        """Return the model's variable names (convenience method)."""
        return self._model.get_var_names()
    
    def get_step_count(self) -> int:
        """
        Get the number of time steps in the simulation results.
        
        Returns:
            The number of time steps
            
        Raises:
            SimlinRuntimeError: If the operation fails
        """
        count = lib.simlin_sim_get_stepcount(self._ptr)
        if count < 0:
            count = 0
        if count == 0 and getattr(self, "_ran", False):
            return 1
        return int(count)
    
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
        
        result = lib.simlin_sim_get_value(self._ptr, c_name, value_ptr)
        check_error(result, f"Get value for '{name}'")
        
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
        result = lib.simlin_sim_set_value(self._ptr, c_name, value)
        check_error(result, f"Set value for '{name}'")
    
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
        
        result = lib.simlin_sim_get_series(
            self._ptr, 
            c_name,
            ffi.cast("double *", ffi.from_buffer(results)),
            step_count
        )
        check_error(result, f"Get series for '{name}'")
        
        return results
    
    
    def get_results(self, variables: Optional[List[str]] = None) -> pd.DataFrame:
        """
        Get simulation results as a pandas DataFrame.
        
        This is the key differentiator from the Go API - returns all simulation
        results in a convenient DataFrame format for analysis.
        
        Args:
            variables: Optional list of variable names to include.
                      If None, includes all available variables from the model.
                      
        Returns:
            DataFrame with time as index and variables as columns
            
        Raises:
            SimlinRuntimeError: If getting results fails
        """
        if variables is None:
            variables = self._model.get_var_names()
        
        step_count = self.get_step_count()
        if step_count <= 0:
            return pd.DataFrame()
        
        # Get time series (assuming 'time' is always available)
        try:
            time_series = self.get_series("time")
        except SimlinRuntimeError:
            # If 'time' doesn't exist, create a synthetic time index
            time_series = np.arange(step_count, dtype=np.float64)
        
        # Build dictionary of series
        data: Dict[str, NDArray[np.float64]] = {}
        
        for var_name in variables:
            if var_name.lower() == "time":
                continue  # Skip time, it's the index
            try:
                data[var_name] = self.get_series(var_name)
            except SimlinRuntimeError:
                # Skip variables that don't exist or can't be retrieved
                pass
        
        # Create DataFrame with time as index
        df = pd.DataFrame(data, index=time_series)
        df.index.name = "time"
        
        return df
    
    def get_links(self) -> List[Link]:
        """
        Get all causal links from the simulation.
        
        If the simulation was run with LTM enabled, link scores will be included.
        
        Returns:
            List of Link objects with optional score data
        """
        links_ptr = lib.simlin_analyze_get_links(self._ptr)
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
        
        result = lib.simlin_analyze_get_relative_loop_score(
            self._ptr,
            c_loop_id,
            ffi.cast("double *", ffi.from_buffer(results)),
            step_count
        )
        check_error(result, f"Get relative loop score for '{loop_id}'")
        
        return results
    
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
