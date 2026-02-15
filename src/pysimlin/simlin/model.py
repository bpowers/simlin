"""Model class for working with system dynamics models.

Thread-safety: ``Model`` instances own a ``threading.Lock`` that
protects the underlying ``_ptr`` and the ``_cached_base_case`` field.
The targeted FFI queries (``model_get_var_json``, etc.) operate
directly on the model pointer, so there is no need for double-checked
locking or cross-lock ordering with the parent ``Project``.
"""

from __future__ import annotations

import json
import threading
from typing import TYPE_CHECKING, Any, Self, Union

from ._dt import parse_dt
from ._ffi import (
    _register_finalizer,
    c_to_string,
    check_out_error,
    ffi,
    free_c_string,
    lib,
    model_get_sim_specs_json,
    model_get_var_json,
    model_get_var_names,
    string_to_c,
)
from .analysis import Link, LinkPolarity, Loop
from .errors import ErrorCode, SimlinRuntimeError
from .json_converter import converter
from .json_types import (
    Auxiliary as JsonAuxiliary,
)
from .json_types import (
    DeleteVariable,
    DeleteView,
    JsonModelOperation,
    JsonModelPatch,
    JsonProjectPatch,
    RenameVariable,
    UpsertAux,
    UpsertFlow,
    UpsertModule,
    UpsertStock,
    UpsertView,
)
from .json_types import (
    Flow as JsonFlow,
)
from .json_types import (
    Module as JsonModule,
)
from .json_types import (
    Stock as JsonStock,
)
from .json_types import (
    View as JsonView,
)
from .types import (
    Aux,
    Flow,
    GraphicalFunction,
    GraphicalFunctionScale,
    ModelIssue,
    Stock,
    TimeSpec,
    UnitIssue,
)

if TYPE_CHECKING:
    from types import TracebackType

    from .project import Project
    from .run import Run
    from .sim import Sim


# Type for variable in the edit context current dict
JsonVariable = Union[JsonStock, JsonFlow, JsonAuxiliary, JsonModule]


def _parse_graphical_function_dict(gf_dict: dict[str, Any]) -> GraphicalFunction:
    """Parse a graphical function JSON dict into a types dataclass."""
    points = gf_dict.get("points")
    if points:
        x_points: tuple[float, ...] | None = tuple(p[0] for p in points)
        y_points: tuple[float, ...] = tuple(p[1] for p in points)
    else:
        raw_y = gf_dict.get("yPoints", [])
        x_points = None
        y_points = tuple(raw_y) if raw_y else ()

    x_scale_dict = gf_dict.get("xScale")
    y_scale_dict = gf_dict.get("yScale")

    x_scale = GraphicalFunctionScale(
        min=x_scale_dict["min"] if x_scale_dict else 0.0,
        max=x_scale_dict["max"]
        if x_scale_dict
        else (float(len(y_points) - 1) if y_points else 0.0),
    )
    y_scale = GraphicalFunctionScale(
        min=y_scale_dict["min"] if y_scale_dict else 0.0,
        max=y_scale_dict["max"] if y_scale_dict else 1.0,
    )

    return GraphicalFunction(
        x_points=x_points,
        y_points=y_points,
        x_scale=x_scale,
        y_scale=y_scale,
        kind=gf_dict.get("kind") or "continuous",
    )


def _stock_from_dict(d: dict[str, Any]) -> Stock:
    """Convert a tagged JSON variable dict (type=stock) to a Stock."""
    arrayed = d.get("arrayedEquation")
    initial_eq = d.get("initialEquation", "")
    if not initial_eq and arrayed:
        # For stocks, the initial value can come from two arrayed fields:
        # - initialEquation: JSON-sourced data with an explicit initial field
        # - equation: XMILE-sourced data (where <eqn> IS the initial value)
        initial_eq = arrayed.get("initialEquation", "") or arrayed.get("equation", "")
    dimensions: tuple[str, ...] = ()
    if arrayed:
        dimensions = tuple(arrayed.get("dimensions", []))
    return Stock(
        name=d["name"],
        initial_equation=initial_eq,
        inflows=tuple(d.get("inflows", [])),
        outflows=tuple(d.get("outflows", [])),
        units=d.get("units") or None,
        documentation=d.get("documentation") or None,
        dimensions=dimensions,
        non_negative=d.get("nonNegative", False),
    )


def _flow_from_dict(d: dict[str, Any]) -> Flow:
    """Convert a tagged JSON variable dict (type=flow) to a Flow."""
    arrayed = d.get("arrayedEquation")
    equation = d.get("equation", "")
    if not equation and arrayed:
        equation = arrayed.get("equation", "")
    dimensions: tuple[str, ...] = ()
    if arrayed:
        dimensions = tuple(arrayed.get("dimensions", []))
    gf = None
    gf_dict = d.get("graphicalFunction")
    if gf_dict:
        gf = _parse_graphical_function_dict(gf_dict)
    return Flow(
        name=d["name"],
        equation=equation,
        units=d.get("units") or None,
        documentation=d.get("documentation") or None,
        dimensions=dimensions,
        non_negative=d.get("nonNegative", False),
        graphical_function=gf,
    )


def _aux_from_dict(d: dict[str, Any]) -> Aux:
    """Convert a tagged JSON variable dict (type=aux) to an Aux."""
    arrayed = d.get("arrayedEquation")
    equation = d.get("equation", "")
    if not equation and arrayed:
        equation = arrayed.get("equation", "")
    initial_eq = d.get("initialEquation", "")
    if not initial_eq and arrayed:
        initial_eq = arrayed.get("initialEquation", "")
    dimensions: tuple[str, ...] = ()
    if arrayed:
        dimensions = tuple(arrayed.get("dimensions", []))
    gf = None
    gf_dict = d.get("graphicalFunction")
    if gf_dict:
        gf = _parse_graphical_function_dict(gf_dict)
    return Aux(
        name=d["name"],
        equation=equation,
        initial_equation=initial_eq or None,
        units=d.get("units") or None,
        documentation=d.get("documentation") or None,
        dimensions=dimensions,
        graphical_function=gf,
    )


def _var_from_dict(d: dict[str, Any]) -> Stock | Flow | Aux | None:
    """Convert a tagged JSON variable dict to the appropriate type.

    Returns None for module-type variables since they are not represented
    in the public Stock/Flow/Aux type hierarchy.
    """
    var_type = d.get("type")
    if var_type == "stock":
        return _stock_from_dict(d)
    elif var_type == "flow":
        return _flow_from_dict(d)
    elif var_type == "aux":
        return _aux_from_dict(d)
    elif var_type == "module":
        return None
    else:
        raise SimlinRuntimeError(f"unknown variable type: {var_type!r}")


class ModelPatchBuilder:
    """Accumulates model operations before applying them as JSON."""

    def __init__(self, model_name: str) -> None:
        self._model_name = model_name
        self._ops: list[JsonModelOperation] = []

    @property
    def model_name(self) -> str:
        return self._model_name

    def has_operations(self) -> bool:
        return bool(self._ops)

    def build(self) -> JsonModelPatch:
        return JsonModelPatch(name=self._model_name, ops=list(self._ops))

    def upsert_stock(self, stock: JsonStock) -> JsonStock:
        self._ops.append(UpsertStock(stock=stock))
        return stock

    def upsert_flow(self, flow: JsonFlow) -> JsonFlow:
        self._ops.append(UpsertFlow(flow=flow))
        return flow

    def upsert_aux(self, aux: JsonAuxiliary) -> JsonAuxiliary:
        self._ops.append(UpsertAux(aux=aux))
        return aux

    def upsert_module(self, module: JsonModule) -> JsonModule:
        self._ops.append(UpsertModule(module=module))
        return module

    def delete_variable(self, ident: str) -> None:
        self._ops.append(DeleteVariable(ident=ident))

    def rename_variable(self, current_ident: str, new_ident: str) -> None:
        self._ops.append(RenameVariable(from_=current_ident, to=new_ident))

    def upsert_view(self, index: int, view: JsonView) -> JsonView:
        self._ops.append(UpsertView(index=index, view=view))
        return view

    def delete_view(self, index: int) -> None:
        self._ops.append(DeleteView(index=index))


class _ModelEditContext:
    def __init__(self, model: Model, dry_run: bool, allow_errors: bool) -> None:
        self._model = model
        self._dry_run = dry_run
        self._allow_errors = allow_errors
        self._current: dict[str, JsonVariable] = {}
        self._patch = ModelPatchBuilder(model._name or "")

    def __enter__(self) -> tuple[dict[str, JsonVariable], ModelPatchBuilder]:
        with self._model._lock:
            self._model._check_alive()
            names = model_get_var_names(self._model._ptr)

        model_name = self._model._name
        self._patch = ModelPatchBuilder(model_name)

        self._current = {}
        for name in names:
            with self._model._lock:
                self._model._check_alive()
                raw = model_get_var_json(self._model._ptr, name)
            if raw is None:
                continue
            var_dict = json.loads(raw.decode("utf-8"))
            var_type = var_dict.get("type")
            display_name = var_dict.get("name", "")
            if var_type == "stock":
                self._current[display_name] = converter.structure(var_dict, JsonStock)
            elif var_type == "flow":
                self._current[display_name] = converter.structure(var_dict, JsonFlow)
            elif var_type == "aux":
                self._current[display_name] = converter.structure(var_dict, JsonAuxiliary)
            elif var_type == "module":
                self._current[display_name] = converter.structure(var_dict, JsonModule)

        return self._current, self._patch

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: TracebackType | None,
    ) -> bool:
        if exc_type is not None:
            return False

        if not self._patch.has_operations():
            return False

        project = self._model._project
        if project is None:
            raise SimlinRuntimeError("Model is not attached to a Project")

        # Build JSON patch
        project_patch = JsonProjectPatch(models=[self._patch.build()])
        patch_dict = converter.unstructure(project_patch)
        patch_json = json.dumps(patch_dict).encode("utf-8")

        project._apply_patch_json(
            patch_json,
            dry_run=self._dry_run,
            allow_errors=self._allow_errors,
        )

        # Invalidate caches since model state has changed
        self._model._invalidate_caches()

        return False


class Model:
    """Represents a system dynamics model within a project.

    A model contains variables, equations, and structure that define
    the system dynamics simulation.  Models can be simulated by
    creating ``Sim`` instances.

    Thread-safety: individual instances are safe to use from multiple
    threads.  All public methods acquire an internal lock before
    touching mutable state.
    """

    def __init__(self, ptr: Any, project: Project | None = None, name: str | None = None) -> None:
        """Initialize a Model from a C pointer."""
        if ptr == ffi.NULL:
            raise ValueError("Cannot create Model from NULL pointer")
        self._lock = threading.Lock()
        self._ptr = ptr
        self._project = project
        self._name = name or ""
        _register_finalizer(self, lib.simlin_model_unref, ptr)

        self._cached_base_case: Run | None = None

    def _check_alive(self) -> None:
        """Raise if the underlying C object has been freed.

        Must be called while ``_lock`` is held.
        """
        if self._ptr == ffi.NULL:
            raise SimlinRuntimeError("Model has been closed")

    @property
    def project(self) -> Project | None:
        """The Project this model belongs to.

        Returns:
            The parent Project instance, or None if this model is not attached to a project
        """
        return self._project

    def get_variable(self, name: str) -> Stock | Flow | Aux | None:
        """Get a single variable by name, or None if not found.

        Args:
            name: The variable name to look up

        Returns:
            A Stock, Flow, or Aux object, or None if not found.
            Module-type variables also return None since they are not
            represented in the public type hierarchy.
        """
        with self._lock:
            self._check_alive()
            raw = model_get_var_json(self._ptr, name)

        if raw is None:
            return None
        var_dict = json.loads(raw.decode("utf-8"))
        return _var_from_dict(var_dict)

    def get_incoming_links(self, var_name: str) -> list[str]:
        """Get the dependencies (incoming links) for a given variable.

        For flows and auxiliary variables, returns dependencies from their equations.
        For stocks, returns dependencies from their initial value equation.

        Args:
            var_name: The name of the variable to query

        Returns:
            List of variable names that this variable depends on

        Raises:
            SimlinRuntimeError: If the variable doesn't exist or operation fails
        """
        with self._lock:
            self._check_alive()
            c_var_name = string_to_c(var_name)

            # First query the number of dependencies
            out_written_ptr = ffi.new("uintptr_t *")
            err_ptr = ffi.new("SimlinError **")
            lib.simlin_model_get_incoming_links(
                self._ptr, c_var_name, ffi.NULL, 0, out_written_ptr, err_ptr
            )
            check_out_error(err_ptr, f"Get incoming links count for '{var_name}'")

            count = int(out_written_ptr[0])
            if count == 0:
                return []

            # Allocate array for dependency names
            c_deps = ffi.new("char *[]", count)
            out_written_ptr = ffi.new("uintptr_t *")
            err_ptr = ffi.new("SimlinError **")

            # Get the actual dependencies
            lib.simlin_model_get_incoming_links(
                self._ptr, c_var_name, c_deps, count, out_written_ptr, err_ptr
            )
            check_out_error(err_ptr, f"Get incoming links for '{var_name}'")

            actual_count = int(out_written_ptr[0])
            if actual_count != count:
                for i in range(count):
                    if c_deps[i] != ffi.NULL:
                        free_c_string(c_deps[i])
                raise SimlinRuntimeError(
                    f"Failed to get incoming links for '{var_name}': "
                    f"count mismatch (expected {count}, got {actual_count})"
                )

            # Convert to Python strings and free C memory
            deps = []
            for i in range(count):
                if c_deps[i] != ffi.NULL:
                    deps.append(c_to_string(c_deps[i]))
                    free_c_string(c_deps[i])

            return deps

    def get_links(self) -> list[Link]:
        """Get all causal links in the model (static analysis).

        This returns the structural links in the model without simulation data.
        To get links with LTM scores, run a simulation with enable_ltm=True
        and call get_links() on the Sim instance.

        Returns:
            List of Link objects representing causal relationships
        """
        with self._lock:
            self._check_alive()
            err_ptr = ffi.new("SimlinError **")
            links_ptr = lib.simlin_model_get_links(self._ptr, err_ptr)
            check_out_error(err_ptr, "Get links")

        if links_ptr == ffi.NULL:
            return []

        try:
            if links_ptr.count == 0:
                return []

            links = []
            for i in range(links_ptr.count):
                c_link = links_ptr.links[i]

                link = Link(
                    from_var=c_to_string(getattr(c_link, "from")) or "",
                    to_var=c_to_string(c_link.to) or "",
                    polarity=LinkPolarity(c_link.polarity),
                    score=None,  # No scores in static analysis
                )
                links.append(link)

            return links

        finally:
            lib.simlin_free_links(links_ptr)

    def _invalidate_caches(self) -> None:
        """Invalidate all cached data. Called after model edits."""
        with self._lock:
            self._cached_base_case = None

    def get_var_names(self, type_mask: int = 0, filter_str: str | None = None) -> list[str]:
        """Get canonical variable names, optionally filtered.

        Args:
            type_mask: Bitmask of variable types (0 = all).
                Use VARTYPE_STOCK=1, VARTYPE_FLOW=2, VARTYPE_AUX=4, VARTYPE_MODULE=8.
            filter_str: Substring filter on canonicalized names. None = no filter.

        Returns:
            List of canonical variable name strings
        """
        with self._lock:
            self._check_alive()
            return model_get_var_names(self._ptr, type_mask, filter_str)

    @property
    def variables(self) -> tuple[Stock | Flow | Aux, ...]:
        """All variables in the model (stocks, flows, and auxs).

        Returns:
            Tuple of all variable objects (Stock, Flow, or Aux)
        """
        with self._lock:
            self._check_alive()
            names = model_get_var_names(self._ptr)

        result: list[Stock | Flow | Aux] = []
        for name in names:
            var = self.get_variable(name)
            if var is not None:
                result.append(var)
        return tuple(result)

    @property
    def time_spec(self) -> TimeSpec:
        """Time bounds and step size.

        Returns:
            TimeSpec with simulation time configuration
        """
        with self._lock:
            self._check_alive()
            raw = model_get_sim_specs_json(self._ptr)

        sim_specs = json.loads(raw.decode("utf-8"))

        return TimeSpec(
            start=sim_specs.get("startTime", 0.0),
            stop=sim_specs.get("endTime", 10.0),
            dt=parse_dt(sim_specs.get("dt", "1")),
            units=sim_specs.get("timeUnits") or None,
        )

    @property
    def loops(self) -> tuple[Loop, ...]:
        """Structural feedback loops (no behavior data).

        Returns an immutable tuple of Loop objects.
        For loops with behavior time series, use model.base_case.loops
        or run.loops from a specific simulation run.

        Returns:
            Tuple of Loop objects (structural only, no behavior data)
        """
        if self._project is None:
            return ()
        return tuple(self._project.get_loops())

    def simulate(
        self,
        overrides: dict[str, float] | None = None,
        enable_ltm: bool = False,
    ) -> Sim:
        """Create low-level simulation for step-by-step execution.

        Use this for gaming applications where you need to inspect state
        and modify variables during simulation. For batch analysis, use
        model.run() instead.

        Args:
            overrides: Variable value overrides
            enable_ltm: Enable Loops That Matter tracking

        Returns:
            Sim context manager for step-by-step execution

        Example:
            >>> with model.simulate() as sim:
            ...     sim.run_to_end()
            ...     run = sim.get_run()
        """
        from .sim import Sim

        with self._lock:
            self._check_alive()
            err_ptr = ffi.new("SimlinError **")
            sim_ptr = lib.simlin_sim_new(self._ptr, enable_ltm, err_ptr)
            check_out_error(err_ptr, "Create simulation")

        sim = Sim(sim_ptr, self, overrides or {})
        if overrides:
            for name, value in overrides.items():
                sim.set_value(name, value)
        return sim

    def run(
        self,
        overrides: dict[str, float] | None = None,
        time_range: tuple[float, float] | None = None,
        dt: float | None = None,
        analyze_loops: bool = True,
    ) -> Run:
        """Run simulation with optional variable overrides.

        Args:
            overrides: Override values for any model variables (by name)
            time_range: (start, stop) time bounds (uses model defaults if None)
            dt: Time step (uses model default if None)
            analyze_loops: Whether to compute loop dominance analysis (LTM)

        Returns:
            Run object with results and analysis

        Example:
            >>> run = model.run(overrides={"birth_rate": 0.03})
            >>> run.results["population"].plot()
        """
        from .run import Run

        sim = self.simulate(overrides=overrides or {}, enable_ltm=analyze_loops)
        sim.run_to_end()

        loops_structural = self.loops

        return Run(sim, overrides or {}, loops_structural)

    @property
    def base_case(self) -> Run:
        """Simulation results with default parameters.

        Computed on first access and cached.

        Returns:
            Run object with baseline simulation results

        Example:
            >>> model.base_case.results["population"].plot()
        """
        with self._lock:
            if self._cached_base_case is not None:
                return self._cached_base_case

        result = self.run()

        with self._lock:
            if self._cached_base_case is None:
                self._cached_base_case = result
            return self._cached_base_case

    def check(self) -> tuple[ModelIssue, ...]:
        """Check model for common issues.

        Returns tuple of warnings/errors about model structure, equations, etc.

        Returns:
            Tuple of ModelIssue objects, or empty tuple if no issues

        Example:
            >>> issues = model.check()
            >>> for issue in issues:
            ...     print(f"{issue.severity}: {issue.message}")
        """
        if self._project is None:
            return ()

        error_details = self._project.get_errors()
        issues = []

        for detail in error_details:
            severity = "error"

            issue = ModelIssue(
                severity=severity,
                message=detail.message,
                variable=detail.variable_name,
                suggestion=None,
            )
            issues.append(issue)

        return tuple(issues)

    def check_units(self) -> tuple[UnitIssue, ...]:
        """Check dimensional consistency of equations.

        Returns tuple of unit issues found.

        Returns:
            Tuple of UnitIssue objects, or empty tuple if no unit issues

        Example:
            >>> issues = model.check_units()
            >>> errors = [i for i in issues if i.expected_units != i.actual_units]
        """
        if self._project is None:
            return ()

        error_details = self._project.get_errors()
        unit_issues = []

        for detail in error_details:
            if detail.code == ErrorCode.UNIT_DEFINITION_ERRORS:
                issue = UnitIssue(
                    variable=detail.variable_name or "",
                    message=detail.message,
                    expected_units=None,
                    actual_units=None,
                )
                unit_issues.append(issue)

        return tuple(unit_issues)

    def explain(self, variable: str) -> str:
        """Get human-readable explanation of a variable.

        Args:
            variable: Variable name

        Returns:
            Textual description of what defines/drives this variable

        Example:
            >>> print(model.explain("population"))
            "population is a stock increased by births and decreased by deaths"

        Raises:
            SimlinRuntimeError: If variable doesn't exist
        """
        var = self.get_variable(variable)
        if var is None:
            raise SimlinRuntimeError(f"Variable '{variable}' not found in model")

        if isinstance(var, Stock):
            inflows_str = ", ".join(var.inflows) if var.inflows else "no inflows"
            outflows_str = ", ".join(var.outflows) if var.outflows else "no outflows"
            return (
                f"{var.name} is a stock with initial value {var.initial_equation}, "
                f"increased by {inflows_str}, decreased by {outflows_str}"
            )

        if isinstance(var, Flow):
            return f"{var.name} is a flow computed as {var.equation}"

        if isinstance(var, Aux):
            if var.initial_equation:
                return (
                    f"{var.name} is an auxiliary variable computed as {var.equation} "
                    f"with initial value {var.initial_equation}"
                )
            return f"{var.name} is an auxiliary variable computed as {var.equation}"

        raise AssertionError(f"unexpected variable type: {type(var)}")

    def edit(self, *, dry_run: bool = False, allow_errors: bool = False) -> _ModelEditContext:
        """Return a context manager for batching model edits."""
        if self._project is None:
            raise SimlinRuntimeError("Model is not attached to a Project")

        return _ModelEditContext(self, dry_run=dry_run, allow_errors=allow_errors)

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
        """Return a string representation of the Model."""
        try:
            var_count = len(self.get_var_names())
            name = f" '{self._name}'" if self._name else ""
            return f"<Model{name} with {var_count} variable(s)>"
        except Exception:
            return "<Model (invalid)>"
