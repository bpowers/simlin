"""Model class for working with system dynamics models.

Thread-safety: ``Model`` instances own a ``threading.Lock`` that
protects cached properties and the underlying ``_ptr``.  For methods
that call into the parent ``Project`` (which has its own lock), the
model lock is released before acquiring the project lock to prevent
lock-ordering deadlocks.
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
    GraphicalFunction as JsonGraphicalFunction,
)
from .json_types import (
    Model as JsonModel,
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
        project = self._model._project
        if project is None:
            raise SimlinRuntimeError("Model is not attached to a Project")

        # Get project state as JSON
        json_bytes = project.serialize_json()
        project_dict = json.loads(json_bytes.decode("utf-8"))

        model_dict = None
        for candidate in project_dict.get("models", []):
            if candidate["name"] == self._model._name or not self._model._name:
                model_dict = candidate
                break

        if model_dict is None:
            raise SimlinRuntimeError(
                f"Model '{self._model._name or 'default'}' not found in project serialization"
            )

        self._model._name = model_dict["name"]
        self._patch = ModelPatchBuilder(model_dict["name"])

        # Build current variable dict from JSON using converter.structure()
        self._current = {}
        for stock_dict in model_dict.get("stocks", []):
            stock = converter.structure(stock_dict, JsonStock)
            self._current[stock.name] = stock
        for flow_dict in model_dict.get("flows", []):
            flow = converter.structure(flow_dict, JsonFlow)
            self._current[flow.name] = flow
        for aux_dict in model_dict.get("auxiliaries", []):
            aux = converter.structure(aux_dict, JsonAuxiliary)
            self._current[aux.name] = aux
        for module_dict in model_dict.get("modules", []):
            module = converter.structure(module_dict, JsonModule)
            self._current[module.name] = module

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

        self._cached_model_json: JsonModel | None = None
        self._cached_stocks: tuple[Stock, ...] | None = None
        self._cached_flows: tuple[Flow, ...] | None = None
        self._cached_auxs: tuple[Aux, ...] | None = None
        self._cached_time_spec: TimeSpec | None = None
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
        # Validate variable exists to provide a clear Pythonic error
        names = [v.name for v in self.variables]
        if var_name not in names:
            raise SimlinRuntimeError(f"Variable not found: {var_name}")

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

    def _get_model_json(self) -> JsonModel:
        """Get this model's JSON representation as a dataclass (cached).

        Uses double-checked locking: acquires ``_lock`` only to read/write
        the cache.  The potentially expensive ``serialize_json()`` call on
        the parent ``Project`` runs *without* holding the model lock to
        prevent lock-ordering deadlocks.
        """
        with self._lock:
            if self._cached_model_json is not None:
                return self._cached_model_json

        if self._project is None:
            raise SimlinRuntimeError("Model is not attached to a Project")

        # Call project.serialize_json() WITHOUT holding self._lock to avoid
        # lock-ordering issues (project has its own lock).
        project_json = json.loads(self._project.serialize_json().decode("utf-8"))

        result: JsonModel | None = None
        for model_dict in project_json.get("models", []):
            if model_dict["name"] == self._name or not self._name:
                result = converter.structure(model_dict, JsonModel)
                break

        if result is None:
            raise SimlinRuntimeError(f"Model '{self._name}' not found in project")

        with self._lock:
            # Double-checked: another thread may have populated the cache
            # while we were computing.
            if self._cached_model_json is None:
                self._cached_model_json = result
            return self._cached_model_json

    def _invalidate_caches(self) -> None:
        """Invalidate all cached data. Called after model edits."""
        with self._lock:
            self._cached_model_json = None
            self._cached_stocks = None
            self._cached_flows = None
            self._cached_auxs = None
            self._cached_time_spec = None
            self._cached_base_case = None

    def _extract_equation(
        self,
        top_level: str,
        arrayed: Any | None,
        field: str = "equation",
    ) -> str:
        """Extract equation from JSON, handling apply-to-all arrayed equations.

        For arrayed variables with apply-to-all equations, the top-level equation
        field is empty and the actual equation is in arrayed_equation.equation.

        Note: For stocks, the initial equation is stored in arrayed_equation.equation
        (not arrayed_equation.initial_equation) because in XMILE, the <eqn> tag
        for stocks represents the initial value, and the serializer maps this to
        the "equation" field in ArrayedEquation for consistency.

        Args:
            top_level: The top-level equation string (may be empty)
            arrayed: The arrayed_equation object (may be None)
            field: Which field to read from arrayed - use "equation" for flows/auxs
                   and also for stock initial equations (see note above)

        Returns:
            The equation string, preferring top-level if non-empty
        """
        if top_level:
            return top_level
        if arrayed is not None:
            arrayed_eq = getattr(arrayed, field, None)
            if arrayed_eq:
                return arrayed_eq
        return ""

    def _parse_json_graphical_function(self, gf: JsonGraphicalFunction) -> GraphicalFunction:
        """Parse a JSON GraphicalFunction into a types dataclass."""
        # Handle points format (list of [x, y] pairs)
        if gf.points:
            x_points: tuple[float, ...] | None = tuple(p[0] for p in gf.points)
            y_points: tuple[float, ...] = tuple(p[1] for p in gf.points)
        else:
            x_points = None
            y_points = tuple(gf.y_points) if gf.y_points else ()

        x_scale = GraphicalFunctionScale(
            min=gf.x_scale.min if gf.x_scale else 0.0,
            max=gf.x_scale.max if gf.x_scale else float(len(y_points) - 1) if y_points else 0.0,
        )
        y_scale = GraphicalFunctionScale(
            min=gf.y_scale.min if gf.y_scale else 0.0,
            max=gf.y_scale.max if gf.y_scale else 1.0,
        )

        return GraphicalFunction(
            x_points=x_points,
            y_points=y_points,
            x_scale=x_scale,
            y_scale=y_scale,
            kind=gf.kind or "continuous",
        )

    @property
    def stocks(self) -> tuple[Stock, ...]:
        """Stock variables (immutable tuple).

        Returns:
            Tuple of Stock objects representing all stocks in the model
        """
        with self._lock:
            if self._cached_stocks is not None:
                return self._cached_stocks

        model = self._get_model_json()
        result = tuple(
            Stock(
                name=s.name,
                initial_equation=self._extract_equation(
                    s.initial_equation, s.arrayed_equation, "equation"
                ),
                inflows=tuple(s.inflows),
                outflows=tuple(s.outflows),
                units=s.units or None,
                documentation=s.documentation or None,
                dimensions=tuple(s.arrayed_equation.dimensions) if s.arrayed_equation else (),
                non_negative=s.non_negative,
            )
            for s in model.stocks
        )

        with self._lock:
            if self._cached_stocks is None:
                self._cached_stocks = result
            return self._cached_stocks

    @property
    def flows(self) -> tuple[Flow, ...]:
        """Flow variables (immutable tuple).

        Returns:
            Tuple of Flow objects representing all flows in the model
        """
        with self._lock:
            if self._cached_flows is not None:
                return self._cached_flows

        model = self._get_model_json()
        flows_list = []

        for f in model.flows:
            gf = None
            if f.graphical_function:
                gf = self._parse_json_graphical_function(f.graphical_function)

            flow = Flow(
                name=f.name,
                equation=self._extract_equation(f.equation, f.arrayed_equation),
                units=f.units or None,
                documentation=f.documentation or None,
                dimensions=tuple(f.arrayed_equation.dimensions) if f.arrayed_equation else (),
                non_negative=f.non_negative,
                graphical_function=gf,
            )
            flows_list.append(flow)

        result = tuple(flows_list)

        with self._lock:
            if self._cached_flows is None:
                self._cached_flows = result
            return self._cached_flows

    @property
    def auxs(self) -> tuple[Aux, ...]:
        """Auxiliary variables (immutable tuple).

        Returns:
            Tuple of Aux objects representing all auxiliary variables in the model
        """
        with self._lock:
            if self._cached_auxs is not None:
                return self._cached_auxs

        model = self._get_model_json()
        auxs_list = []

        for a in model.auxiliaries:
            gf = None
            if a.graphical_function:
                gf = self._parse_json_graphical_function(a.graphical_function)

            # Extract equations, handling apply-to-all arrayed equations
            equation = self._extract_equation(a.equation, a.arrayed_equation)
            initial_eq = self._extract_equation(
                a.initial_equation, a.arrayed_equation, "initial_equation"
            )

            aux = Aux(
                name=a.name,
                equation=equation,
                initial_equation=initial_eq or None,
                units=a.units or None,
                documentation=a.documentation or None,
                dimensions=tuple(a.arrayed_equation.dimensions) if a.arrayed_equation else (),
                graphical_function=gf,
            )
            auxs_list.append(aux)

        result = tuple(auxs_list)

        with self._lock:
            if self._cached_auxs is None:
                self._cached_auxs = result
            return self._cached_auxs

    @property
    def variables(self) -> tuple[Stock | Flow | Aux, ...]:
        """All variables in the model.

        Returns stocks + flows + auxs combined as an immutable tuple.

        Returns:
            Tuple of all variable objects (Stock, Flow, or Aux)
        """
        return self.stocks + self.flows + self.auxs

    @property
    def time_spec(self) -> TimeSpec:
        """Time bounds and step size.

        Returns:
            TimeSpec with simulation time configuration
        """
        with self._lock:
            if self._cached_time_spec is not None:
                return self._cached_time_spec

        if self._project is None:
            raise SimlinRuntimeError("Model is not attached to a Project")

        project_json = json.loads(self._project.serialize_json().decode("utf-8"))
        sim_specs = project_json["simSpecs"]

        result = TimeSpec(
            start=sim_specs.get("startTime", 0.0),
            stop=sim_specs.get("endTime", 10.0),
            dt=parse_dt(sim_specs.get("dt", "1")),
            units=sim_specs.get("timeUnits") or None,
        )

        with self._lock:
            if self._cached_time_spec is None:
                self._cached_time_spec = result
            return self._cached_time_spec

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
        for stock in self.stocks:
            if stock.name == variable:
                inflows_str = ", ".join(stock.inflows) if stock.inflows else "no inflows"
                outflows_str = ", ".join(stock.outflows) if stock.outflows else "no outflows"
                return (
                    f"{stock.name} is a stock with initial value {stock.initial_equation}, "
                    f"increased by {inflows_str}, decreased by {outflows_str}"
                )

        for flow in self.flows:
            if flow.name == variable:
                return f"{flow.name} is a flow computed as {flow.equation}"

        for aux in self.auxs:
            if aux.name == variable:
                if aux.initial_equation:
                    return (
                        f"{aux.name} is an auxiliary variable computed as {aux.equation} "
                        f"with initial value {aux.initial_equation}"
                    )
                return f"{aux.name} is an auxiliary variable computed as {aux.equation}"

        raise SimlinRuntimeError(f"Variable '{variable}' not found in model")

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
            var_count = len(self.variables)
            name = f" '{self._name}'" if self._name else ""
            return f"<Model{name} with {var_count} variable(s)>"
        except Exception:
            return "<Model (invalid)>"
