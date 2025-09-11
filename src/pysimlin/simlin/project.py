"""Project class for loading and managing system dynamics models."""

from typing import List, Optional, TYPE_CHECKING, Any, Self
from types import TracebackType
from pathlib import Path

from ._ffi import ffi, lib, string_to_c, c_to_string, free_c_string, check_error, _register_finalizer
from .errors import SimlinImportError, ErrorCode, ErrorDetail
from .analysis import Loop, LoopPolarity

if TYPE_CHECKING:
    from .model import Model


class Project:
    """
    Represents a simulation project containing one or more models.
    
    A project is the top-level container for system dynamics models.
    It can be loaded from various formats (XMILE, Vensim MDL, protobuf)
    and provides access to models and analysis functions.
    """
    
    def __init__(self, ptr: Any) -> None:
        """Initialize a Project from a C pointer."""
        if ptr == ffi.NULL:
            raise ValueError("Cannot create Project from NULL pointer")
        self._ptr = ptr
        _register_finalizer(self, lib.simlin_project_unref, ptr)
    
    @classmethod
    def from_protobin(cls, data: bytes) -> "Project":
        """
        Load a project from binary protobuf format.
        
        Args:
            data: The protobuf binary data
            
        Returns:
            A new Project instance
            
        Raises:
            SimlinImportError: If the data cannot be parsed
        """
        if not data:
            raise SimlinImportError("Empty project data")
        
        err_ptr = ffi.new("int *")
        c_data = ffi.new("uint8_t[]", data)
        
        project_ptr = lib.simlin_project_open(c_data, len(data), err_ptr)
        
        if project_ptr == ffi.NULL:
            error_code = err_ptr[0]
            error_str = lib.simlin_error_str(error_code)
            error_msg = c_to_string(error_str) or f"Unknown error {error_code}"
            raise SimlinImportError(f"Failed to open project: {error_msg}", ErrorCode(error_code))
        
        return cls(project_ptr)
    
    @classmethod
    def from_xmile(cls, data: bytes) -> "Project":
        """
        Load a project from XMILE/STMX format.
        
        Args:
            data: The XMILE XML data
            
        Returns:
            A new Project instance
            
        Raises:
            SimlinImportError: If the data cannot be parsed
        """
        if not data:
            raise SimlinImportError("Empty XMILE data")
        
        err_ptr = ffi.new("int *")
        c_data = ffi.new("uint8_t[]", data)
        
        project_ptr = lib.simlin_import_xmile(c_data, len(data), err_ptr)
        
        if project_ptr == ffi.NULL:
            error_code = err_ptr[0]
            error_str = lib.simlin_error_str(error_code)
            error_msg = c_to_string(error_str) or f"Unknown error {error_code}"
            raise SimlinImportError(f"Failed to import XMILE: {error_msg}", ErrorCode(error_code))
        
        return cls(project_ptr)
    
    @classmethod
    def from_mdl(cls, data: bytes) -> "Project":
        """
        Load a project from Vensim MDL format.
        
        Args:
            data: The MDL text data
            
        Returns:
            A new Project instance
            
        Raises:
            SimlinImportError: If the data cannot be parsed
        """
        if not data:
            raise SimlinImportError("Empty MDL data")
        
        err_ptr = ffi.new("int *")
        c_data = ffi.new("uint8_t[]", data)
        
        project_ptr = lib.simlin_import_mdl(c_data, len(data), err_ptr)
        
        if project_ptr == ffi.NULL:
            error_code = err_ptr[0]
            error_str = lib.simlin_error_str(error_code) 
            error_msg = c_to_string(error_str) or f"Unknown error {error_code}"
            raise SimlinImportError(f"Failed to import MDL: {error_msg}", ErrorCode(error_code))
        
        return cls(project_ptr)
    
    @classmethod
    def from_file(cls, path: Path | str) -> "Project":
        """
        Load a project from a file, auto-detecting the format.
        
        Args:
            path: Path to the model file
            
        Returns:
            A new Project instance
            
        Raises:
            SimlinImportError: If the file cannot be loaded or parsed
        """
        path = Path(path)
        
        if not path.exists():
            raise SimlinImportError(f"File not found: {path}")
        
        data = path.read_bytes()
        suffix = path.suffix.lower()
        
        if suffix in (".xmile", ".stmx", ".xml"):
            return cls.from_xmile(data)
        elif suffix in (".mdl", ".vpm"):
            return cls.from_mdl(data)
        elif suffix in (".pb", ".bin", ".proto"):
            return cls.from_protobin(data)
        else:
            # Try to auto-detect based on content
            if data.startswith(b"<?xml") or data.startswith(b"<xmile"):
                return cls.from_xmile(data)
            else:
                # Default to protobuf
                return cls.from_protobin(data)
    
    def __get_model_count(self) -> int:
        """Internal method to get the number of models in the project."""
        count = lib.simlin_project_get_model_count(self._ptr)
        if count < 0:
            raise SimlinImportError("Failed to get model count")
        return count
    
    def get_model_names(self) -> List[str]:
        """
        Get the names of all models in the project.
        
        Returns:
            List of model names
        """
        count = self.__get_model_count()
        if count == 0:
            return []
        
        # Allocate array for C string pointers
        c_names = ffi.new("char *[]", count)
        
        result = lib.simlin_project_get_model_names(self._ptr, c_names, count)
        if result != count:
            raise SimlinImportError(f"Failed to get model names: got {result}, expected {count}")
        
        # Convert to Python strings and free C memory
        names = []
        for i in range(count):
            if c_names[i] != ffi.NULL:
                names.append(c_to_string(c_names[i]))
                free_c_string(c_names[i])
        
        return names
    
    def get_model(self, name: str = "") -> "Model":
        """
        Get a model from the project by name.
        
        Args:
            name: The model name, or empty string for the default/main model
            
        Returns:
            The requested Model instance
            
        Raises:
            SimlinImportError: If the model doesn't exist
        """
        from .model import Model
        
        # If a non-empty name is provided, validate against known names first
        if name:
            names = self.get_model_names()
            if name not in names:
                raise SimlinImportError(f"Model not found: {name}")
        
        c_name = string_to_c(name) if name else ffi.NULL
        model_ptr = lib.simlin_project_get_model(self._ptr, c_name)
        if model_ptr == ffi.NULL:
            raise SimlinImportError(f"Model not found: {name or 'default'}")
        
        return Model(model_ptr)
    
    def get_loops(self) -> List[Loop]:
        """
        Get all feedback loops in the project.
        
        Returns:
            List of Loop objects
        """
        loops_ptr = lib.simlin_analyze_get_loops(self._ptr)
        if loops_ptr == ffi.NULL:
            return []
        
        try:
            if loops_ptr.count == 0:
                return []
            
            loops = []
            for i in range(loops_ptr.count):
                c_loop = loops_ptr.loops[i]
                
                # Convert variables
                variables = []
                for j in range(c_loop.var_count):
                    var_name = c_to_string(c_loop.variables[j])
                    if var_name:
                        variables.append(var_name)
                
                loop = Loop(
                    id=c_to_string(c_loop.id) or f"loop_{i}",
                    variables=variables,
                    polarity=LoopPolarity(c_loop.polarity)
                )
                loops.append(loop)
            
            return loops
            
        finally:
            lib.simlin_free_loops(loops_ptr)
    
    def get_errors(self) -> List[ErrorDetail]:
        """
        Get all errors in the project (compilation and validation).
        
        Returns:
            List of ErrorDetail objects, or empty list if no errors
        """
        details_ptr = lib.simlin_project_get_errors(self._ptr)
        if details_ptr == ffi.NULL:
            return []
        
        try:
            if details_ptr.count == 0:
                return []
            
            errors = []
            for i in range(details_ptr.count):
                c_detail = details_ptr.errors[i]
                
                error = ErrorDetail(
                    code=ErrorCode(c_detail.code),
                    message=c_to_string(c_detail.message) or "",
                    model_name=c_to_string(c_detail.model_name),
                    variable_name=c_to_string(c_detail.variable_name),
                    start_offset=c_detail.start_offset,
                    end_offset=c_detail.end_offset
                )
                errors.append(error)
            
            return errors
            
        finally:
            lib.simlin_free_error_details(details_ptr)
    
    def to_xmile(self) -> bytes:
        """
        Export the project to XMILE format.
        
        Returns:
            The XMILE XML data as bytes
            
        Raises:
            SimlinImportError: If export fails
        """
        output_ptr = ffi.new("uint8_t **")
        # Use uintptr_t* to exactly match the C typedef used in cdef
        output_len_ptr = ffi.new("uintptr_t *")
        
        result = lib.simlin_export_xmile(self._ptr, output_ptr, output_len_ptr)
        check_error(result, "Export to XMILE")
        
        if output_ptr[0] == ffi.NULL:
            raise SimlinImportError("Export returned null output")
        
        try:
            # Copy the data to Python bytes
            return bytes(ffi.buffer(output_ptr[0], output_len_ptr[0]))
        finally:
            lib.simlin_free(output_ptr[0])
    
    def serialize(self) -> bytes:
        """
        Serialize the project to binary protobuf format.
        
        Returns:
            The protobuf binary data
            
        Raises:
            SimlinImportError: If serialization fails
        """
        output_ptr = ffi.new("uint8_t **")
        # Use uintptr_t* to exactly match the C typedef used in cdef
        output_len_ptr = ffi.new("uintptr_t *")
        
        result = lib.simlin_project_serialize(self._ptr, output_ptr, output_len_ptr)
        check_error(result, "Project serialization")
        
        if output_ptr[0] == ffi.NULL:
            raise SimlinImportError("Serialize returned null output")
        
        try:
            # Copy the data to Python bytes
            return bytes(ffi.buffer(output_ptr[0], output_len_ptr[0]))
        finally:
            lib.simlin_free(output_ptr[0])
    
    def __enter__(self) -> Self:
        """Context manager entry point."""
        return self
    
    def __exit__(self, exc_type: Optional[type[BaseException]], exc_val: Optional[BaseException], exc_tb: Optional[TracebackType]) -> None:
        """Context manager exit point with explicit cleanup."""
        # Run and disarm finalizer if present
        finalizer = getattr(self, "_finalizer", None)
        if finalizer and getattr(finalizer, "alive", False):
            finalizer()
        self._ptr = ffi.NULL
    
    def __repr__(self) -> str:
        """Return a string representation of the Project."""
        try:
            model_count = self.__get_model_count()
            return f"<Project with {model_count} model(s)>"
        except:
            return "<Project (invalid)>"
