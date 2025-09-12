"""Model class for working with system dynamics models."""

from typing import List, Optional, TYPE_CHECKING, Any, Self
from types import TracebackType

from ._ffi import ffi, lib, string_to_c, c_to_string, free_c_string, _register_finalizer
from .errors import SimlinRuntimeError, ErrorCode
from .analysis import Link, LinkPolarity

if TYPE_CHECKING:
    from .sim import Sim


class Model:
    """
    Represents a system dynamics model within a project.
    
    A model contains variables, equations, and structure that define
    the system dynamics simulation. Models can be simulated by creating
    Sim instances.
    """
    
    def __init__(self, ptr: Any) -> None:
        """Initialize a Model from a C pointer."""
        if ptr == ffi.NULL:
            raise ValueError("Cannot create Model from NULL pointer")
        self._ptr = ptr
        _register_finalizer(self, lib.simlin_model_unref, ptr)
    
    def get_var_count(self) -> int:
        """Get the number of variables in the model."""
        count = lib.simlin_model_get_var_count(self._ptr)
        if count < 0:
            raise SimlinRuntimeError("Failed to get variable count")
        return count
    
    def get_var_names(self) -> List[str]:
        """
        Get the names of all variables in the model.
        
        Returns:
            List of variable names
            
        Raises:
            SimlinRuntimeError: If the operation fails
        """
        count = self.get_var_count()
        if count == 0:
            return []
        
        # Allocate array for C string pointers
        c_names = ffi.new("char *[]", count)
        
        result = lib.simlin_model_get_var_names(self._ptr, c_names, count)
        if result != count:
            raise SimlinRuntimeError(f"Failed to get variable names: got {result}, expected {count}")
        
        # Convert to Python strings and free C memory
        names = []
        for i in range(count):
            if c_names[i] != ffi.NULL:
                names.append(c_to_string(c_names[i]))
                free_c_string(c_names[i])
        
        return names
    
    def get_incoming_links(self, var_name: str) -> List[str]:
        """
        Get the dependencies (incoming links) for a given variable.
        
        For flows and auxiliary variables, returns dependencies from their equations.
        For stocks, returns dependencies from their initial value equation.
        
        Args:
            var_name: The name of the variable to query
            
        Returns:
            List of variable names that this variable depends on
            
        Raises:
            SimlinRuntimeError: If the variable doesn't exist or operation fails
        """
        # Validate variable exists to provide a clear Pythonic error
        names = self.get_var_names()
        if var_name not in names:
            raise SimlinRuntimeError(f"Variable not found: {var_name}")

        c_var_name = string_to_c(var_name)
        
        # First query the number of dependencies
        count = lib.simlin_model_get_incoming_links(self._ptr, c_var_name, ffi.NULL, 0)
        if count < 0:
            error_str = lib.simlin_error_str(-count)
            error_msg = c_to_string(error_str) or f"Unknown error {-count}"
            raise SimlinRuntimeError(
                f"Failed to get incoming links for '{var_name}': {error_msg}",
                ErrorCode(-count) if -count <= 32 else None
            )
        
        if count == 0:
            return []
        
        # Allocate array for dependency names
        c_deps = ffi.new("char *[]", count)
        
        # Get the actual dependencies
        actual_count = lib.simlin_model_get_incoming_links(self._ptr, c_var_name, c_deps, count)
        if actual_count < 0:
            error_str = lib.simlin_error_str(-actual_count)
            error_msg = c_to_string(error_str) or f"Unknown error {-actual_count}"
            raise SimlinRuntimeError(
                f"Failed to get incoming links for '{var_name}': {error_msg}",
                ErrorCode(-actual_count) if -actual_count <= 32 else None
            )
        
        if actual_count != count:
            raise SimlinRuntimeError(
                f"Failed to get incoming links for '{var_name}': count mismatch (expected {count}, got {actual_count})"
            )
        
        # Convert to Python strings and free C memory
        deps = []
        for i in range(count):
            if c_deps[i] != ffi.NULL:
                deps.append(c_to_string(c_deps[i]))
                free_c_string(c_deps[i])
        
        return deps
    
    def get_links(self) -> List[Link]:
        """
        Get all causal links in the model (static analysis).
        
        This returns the structural links in the model without simulation data.
        To get links with LTM scores, run a simulation with enable_ltm=True
        and call get_links() on the Sim instance.
        
        Returns:
            List of Link objects representing causal relationships
        """
        links_ptr = lib.simlin_model_get_links(self._ptr)
        if links_ptr == ffi.NULL:
            return []
        
        try:
            if links_ptr.count == 0:
                return []
            
            links = []
            for i in range(links_ptr.count):
                c_link = links_ptr.links[i]
                
                link = Link(
                    from_var=c_to_string(getattr(c_link, 'from')) or "",
                    to_var=c_to_string(c_link.to) or "",
                    polarity=LinkPolarity(c_link.polarity),
                    score=None  # No scores in static analysis
                )
                links.append(link)
            
            return links
            
        finally:
            lib.simlin_free_links(links_ptr)
    
    def new_sim(self, enable_ltm: bool = False) -> "Sim":
        """
        Create a new simulation instance from this model.
        
        Args:
            enable_ltm: Whether to enable Loops That Matter analysis.
                       This allows getting link scores and loop analysis after simulation.
                       
        Returns:
            A new Sim instance ready to run
            
        Raises:
            SimlinRuntimeError: If simulation creation fails
        """
        from .sim import Sim
        
        sim_ptr = lib.simlin_sim_new(self._ptr, enable_ltm)
        if sim_ptr == ffi.NULL:
            raise SimlinRuntimeError("Failed to create simulation")
        
        return Sim(sim_ptr, self)
    
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
        """Return a string representation of the Model."""
        try:
            var_count = self.get_var_count()
            return f"<Model with {var_count} variable(s)>"
        except:
            return "<Model (invalid)>"
