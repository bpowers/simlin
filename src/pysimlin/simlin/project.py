"""Project class for loading and managing system dynamics models.

Thread-safety: Each ``Project`` instance owns a ``threading.Lock``
that protects its ``_ptr`` field and serialisation calls.  The
underlying Rust layer already uses per-object Mutexes so the Python
lock only needs to guard Python-level invariants (e.g. the pointer not
being set to ``NULL`` between a liveness check and the FFI call).
"""

from __future__ import annotations

import dataclasses
import json
import threading
from typing import TYPE_CHECKING, Any, Self

from ._dt import validate_dt
from ._ffi import (
    _register_finalizer,
    c_to_string,
    check_out_error,
    extract_error_details,
    ffi,
    free_c_string,
    lib,
    string_to_c,
)
from ._ffi import (
    apply_patch_json as _ffi_apply_patch_json,
)
from ._ffi import (
    open_json as _ffi_open_json,
)
from ._ffi import (
    render_png as _ffi_render_png,
)
from ._ffi import (
    render_svg as _ffi_render_svg,
)
from ._ffi import (
    serialize_json as _ffi_serialize_json,
)
from .errors import ErrorDetail, SimlinImportError, SimlinRuntimeError
from .json_converter import converter
from .json_types import (
    JsonProjectPatch,
    SetSimSpecs,
)
from .json_types import (
    Model as JsonModel,
)
from .json_types import (
    Project as JsonProject,
)
from .json_types import (
    SimSpecs as JsonSimSpecs,
)

if TYPE_CHECKING:
    from types import TracebackType

    from .model import Model

# JSON format constants
JSON_FORMAT_SIMLIN = "simlin"
JSON_FORMAT_SDAI = "sd-ai"


def _collect_error_details(err_ptr: Any) -> list[ErrorDetail]:
    """Convert a C SimlinError pointer into Python ErrorDetail objects.

    Note: This function does NOT free the C memory. The caller is responsible
    for calling simlin_error_free() on the original pointer.
    """
    return extract_error_details(err_ptr)


class Project:
    """Represents a simulation project containing one or more models.

    A project is the top-level container for system dynamics models.
    It can be loaded from various formats (XMILE, Vensim MDL, protobuf)
    and provides access to models and analysis functions.

    Thread-safety: individual instances are safe to use from multiple
    threads.  All public methods acquire an internal lock before
    touching ``_ptr``.
    """

    def __init__(self, ptr: Any) -> None:
        """Initialize a Project from a C pointer."""
        if ptr == ffi.NULL:
            raise ValueError("Cannot create Project from NULL pointer")
        self._lock = threading.Lock()
        self._ptr = ptr
        _register_finalizer(self, lib.simlin_project_unref, ptr)

    def _check_alive(self) -> None:
        """Raise if the underlying C object has been freed.

        Must be called while ``_lock`` is held.
        """
        if self._ptr == ffi.NULL:
            raise SimlinRuntimeError("Project has been closed")

    @classmethod
    def new(
        cls,
        *,
        name: str = "simlin project",
        sim_start: float = 0.0,
        sim_stop: float = 10.0,
        dt: float = 1.0,
        time_units: str = "",
    ) -> Project:
        """Create a new, empty project using default simulation settings.

        Args:
            name: Project name recorded in the metadata.
            sim_start: Simulation start time.
            sim_stop: Simulation stop time.
            dt: Simulation time step (Euler method by default).
            time_units: Optional time unit label.

        Returns:
            A new Project instance ready for editing.
        """
        sim_specs = JsonSimSpecs(
            start_time=float(sim_start),
            end_time=float(sim_stop),
            dt=str(dt),
            method="euler",
            time_units=time_units if time_units else "",
        )
        project = JsonProject(
            name=name,
            sim_specs=sim_specs,
            models=[JsonModel(name="main")],
        )
        json_data = json.dumps(converter.unstructure(project)).encode("utf-8")
        project_ptr = _ffi_open_json(json_data)
        return cls(project_ptr)

    def __get_model_count(self) -> int:
        """Internal method to get the number of models in the project.

        Caller must hold ``_lock``.
        """
        count_ptr = ffi.new("uintptr_t *")
        err_ptr = ffi.new("SimlinError **")
        lib.simlin_project_get_model_count(self._ptr, count_ptr, err_ptr)
        check_out_error(err_ptr, "Get model count")
        return int(count_ptr[0])

    def get_model_names(self) -> list[str]:
        """Get the names of all models in the project.

        Returns:
            List of model names
        """
        with self._lock:
            self._check_alive()
            count = self.__get_model_count()
            if count == 0:
                return []

            # Allocate array for C string pointers
            c_names = ffi.new("char *[]", count)
            out_written_ptr = ffi.new("uintptr_t *")
            err_ptr = ffi.new("SimlinError **")

            lib.simlin_project_get_model_names(self._ptr, c_names, count, out_written_ptr, err_ptr)
            check_out_error(err_ptr, "Get model names")

            written = int(out_written_ptr[0])
            if written != count:
                for i in range(count):
                    if c_names[i] != ffi.NULL:
                        free_c_string(c_names[i])
                raise SimlinImportError(
                    f"Failed to get model names: got {written}, expected {count}"
                )

            # Convert to Python strings and free C memory
            names: list[str] = []
            for i in range(count):
                if c_names[i] != ffi.NULL:
                    name = c_to_string(c_names[i])
                    free_c_string(c_names[i])
                    if name is not None:
                        names.append(name)

            return names

    def get_model(self, name: str = "") -> Model:
        """Get a model from the project by name.

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

        with self._lock:
            self._check_alive()
            c_name = string_to_c(resolved_name) if name else ffi.NULL
            err_ptr = ffi.new("SimlinError **")
            model_ptr = lib.simlin_project_get_model(self._ptr, c_name, err_ptr)
            check_out_error(err_ptr, f"Get model '{name or 'default'}'")

        return Model(model_ptr, project=self, name=resolved_name)

    @property
    def models(self) -> tuple[Model, ...]:
        """All models in this project (immutable tuple).

        Returns:
            Tuple of all Model objects in the project

        Example:
            >>> for model in project.models:
            ...     print(model._name)
        """
        model_names = self.get_model_names()
        return tuple(self.get_model(name) for name in model_names)

    @property
    def main_model(self) -> Model:
        """The main/default model.

        Returns:
            The first/main model in the project

        Raises:
            SimlinImportError: If the project has no models

        Example:
            >>> model = project.main_model
        """
        return self.get_model()

    def get_errors(self) -> list[ErrorDetail]:
        """Get all errors in the project (compilation and validation).

        Returns:
            List of ErrorDetail objects, or empty list if no errors
        """
        with self._lock:
            self._check_alive()
            err_ptr = ffi.new("SimlinError **")
            error_ptr = lib.simlin_project_get_errors(self._ptr, err_ptr)
            check_out_error(err_ptr, "Get errors")

        if error_ptr == ffi.NULL:
            return []

        try:
            return _collect_error_details(error_ptr)
        finally:
            lib.simlin_error_free(error_ptr)

    def to_xmile(self) -> bytes:
        """Export the project to XMILE format.

        Returns:
            The XMILE XML data as bytes

        Raises:
            SimlinImportError: If export fails
        """
        with self._lock:
            self._check_alive()
            output_ptr = ffi.new("uint8_t **")
            output_len_ptr = ffi.new("uintptr_t *")
            err_ptr = ffi.new("SimlinError **")

            lib.simlin_project_serialize_xmile(self._ptr, output_ptr, output_len_ptr, err_ptr)
            check_out_error(err_ptr, "Export to XMILE")

            if output_ptr[0] == ffi.NULL:
                raise SimlinImportError("Export returned null output")

            try:
                return bytes(ffi.buffer(output_ptr[0], output_len_ptr[0]))
            finally:
                lib.simlin_free(output_ptr[0])

    def _apply_patch_json(
        self,
        patch_json: bytes,
        *,
        dry_run: bool = False,
        allow_errors: bool = False,
    ) -> list[ErrorDetail]:
        """Apply a JSON patch, surfacing validation details as Python exceptions.

        Args:
            patch_json: JSON-encoded patch data (UTF-8 bytes)
            dry_run: If True, validate without applying changes
            allow_errors: If True, collect errors instead of failing on first error

        Returns:
            List of ErrorDetail objects for collected validation errors

        Raises:
            SimlinRuntimeError or SimlinCompilationError: If operation fails
        """
        with self._lock:
            self._check_alive()
            errors = _ffi_apply_patch_json(self._ptr, patch_json, dry_run, allow_errors)

        if errors and not allow_errors:
            first_code = errors[0].code if errors else None
            message = (
                "Patch dry run reported validation errors"
                if dry_run
                else "Patch produced validation errors"
            )
            exc = SimlinRuntimeError(message, first_code)
            exc.errors = errors  # type: ignore[attr-defined]
            exc.dry_run = dry_run  # type: ignore[attr-defined]
            exc.allow_errors = allow_errors  # type: ignore[attr-defined]
            raise exc

        return errors

    def serialize_json(self) -> bytes:
        """Serialize the project to JSON format.

        Returns:
            JSON-encoded project data (UTF-8 bytes)

        Raises:
            SimlinRuntimeError: If serialization fails
        """
        with self._lock:
            self._check_alive()
            return _ffi_serialize_json(self._ptr)

    def set_sim_specs(self, **kwargs: Any) -> None:
        """Update the project's simulation specifications.

        Args:
            start: Simulation start time (float)
            stop: Simulation stop time (float)
            dt: Time step (float or string)
            save_step: Save step interval (float)
            sim_method: Simulation method (0 for "euler", 1 for "rk4", or string)
            time_units: Time units string
        """
        if not kwargs:
            raise ValueError("set_sim_specs requires at least one field")

        # Read current specs via JSON
        project_json = json.loads(self.serialize_json().decode("utf-8"))
        current = converter.structure(project_json["simSpecs"], JsonSimSpecs)

        # Map from legacy protobuf-style field names to JSON field names
        field_mapping = {"start": "start_time", "stop": "end_time", "sim_method": "method"}

        # Build updates dict
        updates: dict[str, Any] = {}
        for key, value in kwargs.items():
            json_key = field_mapping.get(key, key)
            if json_key == "dt":
                updates["dt"] = validate_dt(value)
            elif json_key == "save_step":
                updates["save_step"] = float(value) if value is not None else 0.0
            elif json_key == "method":
                method_map = {0: "euler", 1: "rk4"}
                if isinstance(value, int):
                    updates["method"] = method_map.get(value, "euler")
                else:
                    updates["method"] = str(value).lower()
            elif json_key in {"start_time", "end_time"}:
                updates[json_key] = float(value)
            elif json_key == "time_units":
                updates["time_units"] = str(value) if value else ""
            else:
                raise ValueError(f"Unknown SimSpecs field: {key}")

        new_specs = dataclasses.replace(current, **updates)

        # Apply patch using JSON
        patch = JsonProjectPatch(project_ops=[SetSimSpecs(sim_specs=new_specs)])
        patch_json = json.dumps(converter.unstructure(patch)).encode("utf-8")
        self._apply_patch_json(patch_json)

    def serialize_protobuf(self) -> bytes:
        """Serialize the project to binary protobuf format.

        Returns:
            The protobuf binary data

        Raises:
            SimlinImportError: If serialization fails
        """
        with self._lock:
            self._check_alive()
            output_ptr = ffi.new("uint8_t **")
            output_len_ptr = ffi.new("uintptr_t *")
            err_ptr = ffi.new("SimlinError **")

            lib.simlin_project_serialize_protobuf(self._ptr, output_ptr, output_len_ptr, err_ptr)
            check_out_error(err_ptr, "Project serialization")

            if output_ptr[0] == ffi.NULL:
                raise SimlinImportError("Serialize returned null output")

            try:
                return bytes(ffi.buffer(output_ptr[0], output_len_ptr[0]))
            finally:
                lib.simlin_free(output_ptr[0])

    def render_svg(self, model_name: str = "main") -> bytes:
        """Render a model's stock-and-flow diagram as SVG.

        Args:
            model_name: Name of the model to render (default: ``"main"``)

        Returns:
            SVG data as UTF-8 encoded bytes

        Raises:
            SimlinRuntimeError: If the model doesn't exist or rendering fails
        """
        with self._lock:
            self._check_alive()
            return _ffi_render_svg(self._ptr, model_name)

    def render_svg_string(self, model_name: str = "main") -> str:
        """Render a model's stock-and-flow diagram as an SVG string.

        Convenience wrapper around :meth:`render_svg` that decodes the
        result to a Python string.

        Args:
            model_name: Name of the model to render (default: ``"main"``)

        Returns:
            SVG string
        """
        return self.render_svg(model_name).decode("utf-8")

    def render_png(
        self,
        model_name: str = "main",
        *,
        width: int = 0,
        height: int = 0,
    ) -> bytes:
        """Render a model's stock-and-flow diagram as a PNG image.

        Pass ``width=0`` and ``height=0`` (or omit them) to use the SVG's
        intrinsic dimensions. When only one dimension is non-zero the other
        is derived from the aspect ratio. When both are non-zero, ``width``
        takes precedence.

        Args:
            model_name: Name of the model to render (default: ``"main"``)
            width: Target width in pixels (0 for intrinsic)
            height: Target height in pixels (0 for intrinsic)

        Returns:
            PNG image data as bytes

        Raises:
            SimlinRuntimeError: If the model doesn't exist or rendering fails
        """
        with self._lock:
            self._check_alive()
            return _ffi_render_png(self._ptr, model_name, width, height)

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
        """Return a string representation of the Project."""
        try:
            with self._lock:
                self._check_alive()
                model_count = self.__get_model_count()
            return f"<Project with {model_count} model(s)>"
        except Exception:
            return "<Project (invalid)>"
