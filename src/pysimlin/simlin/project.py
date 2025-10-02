"""Project class for loading and managing system dynamics models."""

from __future__ import annotations

from typing import List, Optional, TYPE_CHECKING, Any, Self, Mapping
from types import TracebackType
from pathlib import Path

from ._ffi import (
    ffi,
    lib,
    string_to_c,
    c_to_string,
    free_c_string,
    check_error,
    _register_finalizer,
    get_error_string,
)
from .errors import SimlinImportError, SimlinRuntimeError, ErrorCode, ErrorDetail
from .analysis import Loop, LoopPolarity
from . import pb

# JSON format constants
JSON_FORMAT_SIMLIN = "simlin"
JSON_FORMAT_SDAI = "sd-ai"


def _collect_error_details(c_details: Any) -> List[ErrorDetail]:
    """Convert a C SimlinErrorDetails pointer into Python ErrorDetail objects.

    Note: This function does NOT free the C memory. The caller is responsible
    for calling simlin_free_error_details() on the original pointer.
    """
    if c_details == ffi.NULL:
        return []

    errors: List[ErrorDetail] = []
    for i in range(c_details.count):
        c_detail = c_details.errors[i]
        # Note: c_to_string creates Python strings from C strings without freeing the C memory.
        # The C strings are owned by the SimlinErrorDetails structure and will be freed
        # when simlin_free_error_details is called.
        errors.append(
            ErrorDetail(
                code=ErrorCode(c_detail.code),
                message=c_to_string(c_detail.message) or "",
                model_name=c_to_string(c_detail.model_name),
                variable_name=c_to_string(c_detail.variable_name),
                start_offset=c_detail.start_offset,
                end_offset=c_detail.end_offset,
            )
        )
    return errors


def _coerce_dt(value: Any) -> pb.Dt:
    """Coerce input into a Dt protobuf message."""
    if isinstance(value, pb.Dt):
        dt = pb.Dt()
        dt.CopyFrom(value)
        return dt

    if isinstance(value, Mapping):
        dt = pb.Dt()
        for key, field_value in value.items():
            if key == "value":
                dt.value = float(field_value)
            elif key == "is_reciprocal":
                dt.is_reciprocal = bool(field_value)
            else:
                raise ValueError(f"Unknown Dt field: {key}")
        return dt

    if isinstance(value, (int, float)):
        dt = pb.Dt()
        dt.value = float(value)
        return dt

    raise TypeError("dt values must be a Dt message, mapping, or number")

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
            error_msg = get_error_string(error_code)
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
            error_msg = get_error_string(error_code)
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
            error_msg = get_error_string(error_code)
            raise SimlinImportError(f"Failed to import MDL: {error_msg}", ErrorCode(error_code))

        return cls(project_ptr)

    @classmethod
    def from_json(cls, data: bytes, format: str = JSON_FORMAT_SIMLIN) -> "Project":
        """
        Load a project from JSON format.

        Args:
            data: The JSON data
            format: The JSON format to use. Must be one of:
                - "simlin" (default): Native Simlin JSON format
                - "sd-ai": SDAI JSON format for AI-generated models

        Returns:
            A new Project instance

        Raises:
            SimlinImportError: If the data cannot be parsed
            ValueError: If format is not a valid JSON format string
        """
        if not data:
            raise SimlinImportError("Empty JSON data")

        # Validate and convert format
        if format == JSON_FORMAT_SIMLIN:
            c_format = lib.SIMLIN_JSON_FORMAT_NATIVE
        elif format == JSON_FORMAT_SDAI:
            c_format = lib.SIMLIN_JSON_FORMAT_SDAI
        else:
            raise ValueError(
                f"Invalid format: {format}. Must be '{JSON_FORMAT_SIMLIN}' or '{JSON_FORMAT_SDAI}'"
            )

        err_ptr = ffi.new("int *")
        c_data = ffi.new("uint8_t[]", data)

        project_ptr = lib.simlin_project_json_open(c_data, len(data), c_format, err_ptr)

        if project_ptr == ffi.NULL:
            error_code = err_ptr[0]
            error_msg = get_error_string(error_code)
            raise SimlinImportError(f"Failed to import JSON: {error_msg}", ErrorCode(error_code))

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
        elif suffix == ".json":
            return cls.from_json(data)
        else:
            # Try to auto-detect based on content
            if data.startswith(b"<?xml") or data.startswith(b"<xmile"):
                return cls.from_xmile(data)
            elif data.startswith(b"{"):
                return cls.from_json(data)
            else:
                # Default to protobuf
                return cls.from_protobin(data)

    @classmethod
    def new(
        cls,
        *,
        name: str = "simlin project",
        sim_start: float = 0.0,
        sim_stop: float = 10.0,
        dt: float = 1.0,
        time_units: str = "",
    ) -> "Project":
        """Create a new, empty project using default simulation settings.

        Args:
            name: Project name recorded in the metadata.
            model_name: Name of the initial (empty) model.
            sim_start: Simulation start time.
            sim_stop: Simulation stop time.
            dt: Simulation time step (Euler method by default).
            time_units: Optional time unit label.

        Returns:
            A new Project instance ready for editing.
        """

        project_proto = pb.Project()
        project_proto.name = name

        sim_specs = project_proto.sim_specs
        sim_specs.start = float(sim_start)
        sim_specs.stop = float(sim_stop)
        sim_specs.dt.value = float(dt)
        sim_specs.sim_method = pb.EULER
        if time_units:
            sim_specs.time_units = str(time_units)

        model_proto = project_proto.models.add()
        model_proto.name = "main"

        project = cls.from_protobin(project_proto.SerializeToString())
        return project

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

        names = self.get_model_names()
        if name:
            if name not in names:
                raise SimlinImportError(f"Model not found: {name}")
            resolved_name = name
        else:
            if not names:
                raise SimlinImportError("Project contains no models")
            resolved_name = names[0]

        c_name = string_to_c(resolved_name) if name else ffi.NULL
        model_ptr = lib.simlin_project_get_model(self._ptr, c_name)
        if model_ptr == ffi.NULL:
            raise SimlinImportError(f"Model not found: {name or 'default'}")

        return Model(model_ptr, project=self, name=resolved_name)
    
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
            return _collect_error_details(details_ptr)
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

    def _apply_patch(
        self,
        patch: pb.ProjectPatch,
        *,
        dry_run: bool = False,
        allow_errors: bool = False,
    ) -> List[ErrorDetail]:
        """Apply a patch, surfacing validation details as Python exceptions."""

        if not patch.project_ops and not patch.models:
            return []

        patch_bytes = patch.SerializeToString()
        c_patch = ffi.new("uint8_t[]", patch_bytes)
        errors_ptr = ffi.new("SimlinErrorDetails **")

        result = lib.simlin_project_apply_patch(
            self._ptr,
            c_patch,
            len(patch_bytes),
            dry_run,
            allow_errors,
            errors_ptr,
        )

        errors: List[ErrorDetail] = []
        if errors_ptr[0] != ffi.NULL:
            errors = _collect_error_details(errors_ptr[0])
            lib.simlin_free_error_details(errors_ptr[0])

        if result == ErrorCode.NO_ERROR.value and not errors:
            return []

        try:
            code = ErrorCode(result)
        except ValueError:
            code = None

        if result != ErrorCode.NO_ERROR.value:
            message = f"Patch failed: {get_error_string(result)}"
            exc = SimlinRuntimeError(message, code)
            setattr(exc, "errors", errors)
            setattr(exc, "dry_run", dry_run)
            setattr(exc, "allow_errors", allow_errors)
            raise exc

        if errors and not allow_errors:
            first_code = errors[0].code if errors else None
            message = (
                "Patch dry run reported validation errors"
                if dry_run
                else "Patch produced validation errors"
            )
            exc = SimlinRuntimeError(message, first_code)
            setattr(exc, "errors", errors)
            setattr(exc, "dry_run", dry_run)
            setattr(exc, "allow_errors", allow_errors)
            raise exc

        return errors

    def set_sim_specs(self, **kwargs: Any) -> None:
        """Update the project's simulation specifications using protobuf-compatible kwargs."""

        if not kwargs:
            raise ValueError("set_sim_specs requires at least one field")

        specs = pb.SimSpecs()
        fields = dict(kwargs)

        sim_specs_msg = fields.pop("sim_specs", None)
        if sim_specs_msg is not None:
            if not isinstance(sim_specs_msg, pb.SimSpecs):
                raise TypeError("sim_specs must be a pb.SimSpecs message")
            specs.CopyFrom(sim_specs_msg)
            if fields:
                raise ValueError("Pass either a sim_specs message or individual fields, not both")
        else:
            for field_name, value in fields.items():
                if field_name in {"start", "stop"}:
                    setattr(specs, field_name, float(value))
                elif field_name == "time_units":
                    specs.time_units = str(value)
                elif field_name == "sim_method":
                    specs.sim_method = int(value)
                elif field_name in {"dt", "save_step"}:
                    if value is None:
                        specs.ClearField(field_name)
                    else:
                        getattr(specs, field_name).CopyFrom(_coerce_dt(value))
                else:
                    raise ValueError(f"Unknown SimSpecs field: {field_name}")

        patch = pb.ProjectPatch()
        op = patch.project_ops.add()
        op.set_sim_specs.sim_specs.CopyFrom(specs)

        self._apply_patch(patch)
    
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
