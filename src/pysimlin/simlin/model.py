"""Model class for working with system dynamics models."""

from __future__ import annotations

import json
from typing import Dict, List, Optional, Tuple, TYPE_CHECKING, Any, Self, Union
from types import TracebackType

from ._ffi import ffi, lib, string_to_c, c_to_string, free_c_string, _register_finalizer, check_out_error
from .errors import SimlinRuntimeError, ErrorCode
from .analysis import Link, LinkPolarity, Loop
from .types import Stock, Flow, Aux, TimeSpec, GraphicalFunction, GraphicalFunctionScale, ModelIssue
from . import pb
from .json_types import (
    Stock as JsonStock,
    Flow as JsonFlow,
    Auxiliary as JsonAuxiliary,
    Module as JsonModule,
    View as JsonView,
    JsonModelPatch,
    JsonProjectPatch,
    JsonModelOperation,
    UpsertStock,
    UpsertFlow,
    UpsertAux,
    UpsertModule,
    DeleteVariable,
    RenameVariable,
    UpsertView,
    DeleteView,
    GraphicalFunction as JsonGraphicalFunction,
    GraphicalFunctionScale as JsonGraphicalFunctionScale,
    ArrayedEquation as JsonArrayedEquation,
    ElementEquation as JsonElementEquation,
    ModuleReference as JsonModuleReference,
)
from .json_converter import converter

if TYPE_CHECKING:
    from .sim import Sim
    from .project import Project
    from .run import Run


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


def _parse_graphical_function_from_json(gf_dict: dict[str, Any]) -> JsonGraphicalFunction:
    """Parse a graphical function from JSON dict."""
    points: list[tuple[float, float]] = []
    if "points" in gf_dict:
        points = [(p[0], p[1]) for p in gf_dict["points"]]

    x_scale = None
    if "x_scale" in gf_dict:
        x_scale = JsonGraphicalFunctionScale(
            min=gf_dict["x_scale"]["min"],
            max=gf_dict["x_scale"]["max"],
        )

    y_scale = None
    if "y_scale" in gf_dict:
        y_scale = JsonGraphicalFunctionScale(
            min=gf_dict["y_scale"]["min"],
            max=gf_dict["y_scale"]["max"],
        )

    return JsonGraphicalFunction(
        points=points,
        y_points=gf_dict.get("y_points", []),
        kind=gf_dict.get("kind", ""),
        x_scale=x_scale,
        y_scale=y_scale,
    )


def _parse_arrayed_equation_from_json(ae_dict: dict[str, Any]) -> JsonArrayedEquation:
    """Parse an arrayed equation from JSON dict."""
    elements = None
    if "elements" in ae_dict and ae_dict["elements"]:
        elements = []
        for elem in ae_dict["elements"]:
            gf = None
            if "graphical_function" in elem and elem["graphical_function"]:
                gf = _parse_graphical_function_from_json(elem["graphical_function"])
            elements.append(JsonElementEquation(
                subscript=elem["subscript"],
                equation=elem.get("equation", ""),
                initial_equation=elem.get("initial_equation", ""),
                graphical_function=gf,
            ))

    return JsonArrayedEquation(
        dimensions=ae_dict.get("dimensions", []),
        equation=ae_dict.get("equation"),
        initial_equation=ae_dict.get("initial_equation"),
        elements=elements,
    )


def _stock_from_json(d: dict[str, Any]) -> JsonStock:
    """Convert a stock JSON dict to a JsonStock dataclass."""
    arrayed_equation = None
    if "arrayed_equation" in d and d["arrayed_equation"]:
        arrayed_equation = _parse_arrayed_equation_from_json(d["arrayed_equation"])

    return JsonStock(
        name=d["name"],
        inflows=d.get("inflows", []),
        outflows=d.get("outflows", []),
        uid=d.get("uid", 0),
        initial_equation=d.get("initial_equation", ""),
        units=d.get("units", ""),
        non_negative=d.get("non_negative", False),
        documentation=d.get("documentation", ""),
        can_be_module_input=d.get("can_be_module_input", False),
        is_public=d.get("is_public", False),
        arrayed_equation=arrayed_equation,
    )


def _flow_from_json(d: dict[str, Any]) -> JsonFlow:
    """Convert a flow JSON dict to a JsonFlow dataclass."""
    gf = None
    if "graphical_function" in d and d["graphical_function"]:
        gf = _parse_graphical_function_from_json(d["graphical_function"])

    arrayed_equation = None
    if "arrayed_equation" in d and d["arrayed_equation"]:
        arrayed_equation = _parse_arrayed_equation_from_json(d["arrayed_equation"])

    return JsonFlow(
        name=d["name"],
        uid=d.get("uid", 0),
        equation=d.get("equation", ""),
        units=d.get("units", ""),
        non_negative=d.get("non_negative", False),
        graphical_function=gf,
        documentation=d.get("documentation", ""),
        can_be_module_input=d.get("can_be_module_input", False),
        is_public=d.get("is_public", False),
        arrayed_equation=arrayed_equation,
    )


def _auxiliary_from_json(d: dict[str, Any]) -> JsonAuxiliary:
    """Convert an auxiliary JSON dict to a JsonAuxiliary dataclass."""
    gf = None
    if "graphical_function" in d and d["graphical_function"]:
        gf = _parse_graphical_function_from_json(d["graphical_function"])

    arrayed_equation = None
    if "arrayed_equation" in d and d["arrayed_equation"]:
        arrayed_equation = _parse_arrayed_equation_from_json(d["arrayed_equation"])

    return JsonAuxiliary(
        name=d["name"],
        uid=d.get("uid", 0),
        equation=d.get("equation", ""),
        initial_equation=d.get("initial_equation", ""),
        units=d.get("units", ""),
        graphical_function=gf,
        documentation=d.get("documentation", ""),
        can_be_module_input=d.get("can_be_module_input", False),
        is_public=d.get("is_public", False),
        arrayed_equation=arrayed_equation,
    )


def _module_from_json(d: dict[str, Any]) -> JsonModule:
    """Convert a module JSON dict to a JsonModule dataclass."""
    references = []
    for ref in d.get("references", []):
        references.append(JsonModuleReference(src=ref["src"], dst=ref["dst"]))

    return JsonModule(
        name=d["name"],
        model_name=d["model_name"],
        uid=d.get("uid", 0),
        units=d.get("units", ""),
        documentation=d.get("documentation", ""),
        references=references,
        can_be_module_input=d.get("can_be_module_input", False),
        is_public=d.get("is_public", False),
    )


class _ModelEditContext:
    def __init__(self, model: "Model", dry_run: bool, allow_errors: bool) -> None:
        self._model = model
        self._dry_run = dry_run
        self._allow_errors = allow_errors
        self._current: Dict[str, JsonVariable] = {}
        self._patch = ModelPatchBuilder(model._name or "")

    def __enter__(self) -> Tuple[Dict[str, JsonVariable], ModelPatchBuilder]:
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

        # Build current variable dict from JSON
        self._current = {}
        for stock_dict in model_dict.get("stocks", []):
            stock = _stock_from_json(stock_dict)
            self._current[stock.name] = stock
        for flow_dict in model_dict.get("flows", []):
            flow = _flow_from_json(flow_dict)
            self._current[flow.name] = flow
        for aux_dict in model_dict.get("auxiliaries", []):
            aux = _auxiliary_from_json(aux_dict)
            self._current[aux.name] = aux
        for module_dict in model_dict.get("modules", []):
            module = _module_from_json(module_dict)
            self._current[module.name] = module

        return self._current, self._patch

    def __exit__(
        self,
        exc_type: Optional[type[BaseException]],
        exc: Optional[BaseException],
        tb: Optional[TracebackType],
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

        return False


class Model:
    """
    Represents a system dynamics model within a project.
    
    A model contains variables, equations, and structure that define
    the system dynamics simulation. Models can be simulated by creating
    Sim instances.
    """
    
    def __init__(self, ptr: Any, project: Optional["Project"] = None, name: Optional[str] = None) -> None:
        """Initialize a Model from a C pointer."""
        if ptr == ffi.NULL:
            raise ValueError("Cannot create Model from NULL pointer")
        self._ptr = ptr
        self._project = project
        self._name = name or ""
        _register_finalizer(self, lib.simlin_model_unref, ptr)

        self._cached_project_proto: Optional[pb.Project] = None
        self._cached_stocks: Optional[tuple[Stock, ...]] = None
        self._cached_flows: Optional[tuple[Flow, ...]] = None
        self._cached_auxs: Optional[tuple[Aux, ...]] = None
        self._cached_time_spec: Optional[TimeSpec] = None
        self._cached_base_case: Optional["Run"] = None

    @property
    def project(self) -> Optional["Project"]:
        """
        The Project this model belongs to.

        Returns:
            The parent Project instance, or None if this model is not attached to a project
        """
        return self._project

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
        names = [v.name for v in self.variables]
        if var_name not in names:
            raise SimlinRuntimeError(f"Variable not found: {var_name}")

        c_var_name = string_to_c(var_name)

        # First query the number of dependencies
        out_written_ptr = ffi.new("uintptr_t *")
        err_ptr = ffi.new("SimlinError **")
        lib.simlin_model_get_incoming_links(self._ptr, c_var_name, ffi.NULL, 0, out_written_ptr, err_ptr)
        check_out_error(err_ptr, f"Get incoming links count for '{var_name}'")

        count = int(out_written_ptr[0])
        if count == 0:
            return []

        # Allocate array for dependency names
        c_deps = ffi.new("char *[]", count)
        out_written_ptr = ffi.new("uintptr_t *")
        err_ptr = ffi.new("SimlinError **")

        # Get the actual dependencies
        lib.simlin_model_get_incoming_links(self._ptr, c_var_name, c_deps, count, out_written_ptr, err_ptr)
        check_out_error(err_ptr, f"Get incoming links for '{var_name}'")

        actual_count = int(out_written_ptr[0])
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
                    from_var=c_to_string(getattr(c_link, 'from')) or "",
                    to_var=c_to_string(c_link.to) or "",
                    polarity=LinkPolarity(c_link.polarity),
                    score=None  # No scores in static analysis
                )
                links.append(link)

            return links

        finally:
            lib.simlin_free_links(links_ptr)

    def _get_project_proto(self) -> pb.Project:
        """Get cached protobuf project representation."""
        if self._cached_project_proto is None:
            if self._project is None:
                raise SimlinRuntimeError("Model is not attached to a Project")
            project_proto = pb.Project()
            project_proto.ParseFromString(self._project.serialize())
            self._cached_project_proto = project_proto
        return self._cached_project_proto

    def _get_model_proto(self) -> pb.Model:
        """Get this model's protobuf representation."""
        project_proto = self._get_project_proto()
        for model_proto in project_proto.models:
            if model_proto.name == self._name or not self._name:
                return model_proto
        raise SimlinRuntimeError(f"Model '{self._name}' not found in project")

    def _parse_graphical_function(self, gf_proto: pb.GraphicalFunction) -> GraphicalFunction:
        """Parse a protobuf GraphicalFunction into a dataclass."""
        x_points = tuple(gf_proto.x_points) if gf_proto.x_points else None
        y_points = tuple(gf_proto.y_points)

        x_scale = GraphicalFunctionScale(
            min=gf_proto.x_scale.min,
            max=gf_proto.x_scale.max,
        )
        y_scale = GraphicalFunctionScale(
            min=gf_proto.y_scale.min,
            max=gf_proto.y_scale.max,
        )

        kind_map = {
            pb.GraphicalFunction.CONTINUOUS: "continuous",
            pb.GraphicalFunction.DISCRETE: "discrete",
            pb.GraphicalFunction.EXTRAPOLATE: "extrapolate",
        }
        kind = kind_map.get(gf_proto.kind, "continuous")

        return GraphicalFunction(
            x_points=x_points,
            y_points=y_points,
            x_scale=x_scale,
            y_scale=y_scale,
            kind=kind,
        )

    def _extract_equation_string(self, eqn_proto: pb.Variable.Equation) -> Tuple[str, Optional[str]]:
        """
        Extract equation and initial_equation from protobuf Equation.

        Returns:
            Tuple of (equation, initial_equation)
        """
        which = eqn_proto.WhichOneof("equation")
        if which == "scalar":
            scalar = eqn_proto.scalar
            return scalar.equation, getattr(scalar, "initial_equation", None) or None
        elif which == "apply_to_all":
            ata = eqn_proto.apply_to_all
            return ata.equation, getattr(ata, "initial_equation", None) or None
        elif which == "arrayed":
            return "[arrayed]", None
        else:
            return "", None

    @property
    def stocks(self) -> tuple[Stock, ...]:
        """
        Stock variables (immutable tuple).

        Returns:
            Tuple of Stock objects representing all stocks in the model
        """
        if self._cached_stocks is None:
            model_proto = self._get_model_proto()
            stocks_list = []

            for var_proto in model_proto.variables:
                which = var_proto.WhichOneof("v")
                if which == "stock":
                    stock_proto = var_proto.stock
                    eqn, initial_eqn = self._extract_equation_string(stock_proto.equation)

                    dimensions = tuple(stock_proto.equation.apply_to_all.dimension_names) if stock_proto.equation.HasField("apply_to_all") else ()

                    stock = Stock(
                        name=stock_proto.ident,
                        initial_equation=initial_eqn or eqn,
                        inflows=tuple(stock_proto.inflows),
                        outflows=tuple(stock_proto.outflows),
                        units=stock_proto.units or None,
                        documentation=stock_proto.documentation or None,
                        dimensions=dimensions,
                        non_negative=stock_proto.non_negative,
                    )
                    stocks_list.append(stock)

            self._cached_stocks = tuple(stocks_list)
        return self._cached_stocks

    @property
    def flows(self) -> tuple[Flow, ...]:
        """
        Flow variables (immutable tuple).

        Returns:
            Tuple of Flow objects representing all flows in the model
        """
        if self._cached_flows is None:
            model_proto = self._get_model_proto()
            flows_list = []

            for var_proto in model_proto.variables:
                which = var_proto.WhichOneof("v")
                if which == "flow":
                    flow_proto = var_proto.flow
                    eqn, _ = self._extract_equation_string(flow_proto.equation)

                    dimensions = tuple(flow_proto.equation.apply_to_all.dimension_names) if flow_proto.equation.HasField("apply_to_all") else ()

                    gf = None
                    if flow_proto.HasField("gf"):
                        gf = self._parse_graphical_function(flow_proto.gf)

                    flow = Flow(
                        name=flow_proto.ident,
                        equation=eqn,
                        units=flow_proto.units or None,
                        documentation=flow_proto.documentation or None,
                        dimensions=dimensions,
                        non_negative=flow_proto.non_negative,
                        graphical_function=gf,
                    )
                    flows_list.append(flow)

            self._cached_flows = tuple(flows_list)
        return self._cached_flows

    @property
    def auxs(self) -> tuple[Aux, ...]:
        """
        Auxiliary variables (immutable tuple).

        Returns:
            Tuple of Aux objects representing all auxiliary variables in the model
        """
        if self._cached_auxs is None:
            model_proto = self._get_model_proto()
            auxs_list = []

            for var_proto in model_proto.variables:
                which = var_proto.WhichOneof("v")
                if which == "aux":
                    aux_proto = var_proto.aux
                    eqn, initial_eqn = self._extract_equation_string(aux_proto.equation)

                    dimensions = tuple(aux_proto.equation.apply_to_all.dimension_names) if aux_proto.equation.HasField("apply_to_all") else ()

                    gf = None
                    if aux_proto.HasField("gf"):
                        gf = self._parse_graphical_function(aux_proto.gf)

                    aux = Aux(
                        name=aux_proto.ident,
                        equation=eqn,
                        initial_equation=initial_eqn,
                        units=aux_proto.units or None,
                        documentation=aux_proto.documentation or None,
                        dimensions=dimensions,
                        graphical_function=gf,
                    )
                    auxs_list.append(aux)

            self._cached_auxs = tuple(auxs_list)
        return self._cached_auxs

    @property
    def variables(self) -> tuple[Union[Stock, Flow, Aux], ...]:
        """
        All variables in the model.

        Returns stocks + flows + auxs combined as an immutable tuple.

        Returns:
            Tuple of all variable objects (Stock, Flow, or Aux)
        """
        return self.stocks + self.flows + self.auxs

    @property
    def time_spec(self) -> TimeSpec:
        """
        Time bounds and step size.

        Returns:
            TimeSpec with simulation time configuration
        """
        if self._cached_time_spec is None:
            model_proto = self._get_model_proto()
            project_proto = self._get_project_proto()

            sim_specs = project_proto.sim_specs

            start = sim_specs.start
            stop = sim_specs.stop
            dt_value = sim_specs.dt.value if sim_specs.HasField("dt") else 1.0
            time_units = sim_specs.time_units if sim_specs.HasField("time_units") else None

            self._cached_time_spec = TimeSpec(
                start=start,
                stop=stop,
                dt=dt_value,
                units=time_units,
            )
        return self._cached_time_spec

    @property
    def loops(self) -> tuple[Loop, ...]:
        """
        Structural feedback loops (no behavior data).

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
        overrides: Optional[Dict[str, float]] = None,
        enable_ltm: bool = False,
    ) -> "Sim":
        """
        Create low-level simulation for step-by-step execution.

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
        from ._ffi import lib, ffi, check_out_error

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
        overrides: Optional[Dict[str, float]] = None,
        time_range: Optional[Tuple[float, float]] = None,
        dt: Optional[float] = None,
        analyze_loops: bool = True,
    ) -> "Run":
        """
        Run simulation with optional variable overrides.

        Args:
            overrides: Override values for any model variables (by name)
            time_range: (start, stop) time bounds (uses model defaults if None)
            dt: Time step (uses model default if None)
            analyze_loops: Whether to compute loop dominance analysis (LTM)

        Returns:
            Run object with results and analysis

        Example:
            >>> run = model.run(overrides={'birth_rate': 0.03})
            >>> run.results['population'].plot()
        """
        from .run import Run

        sim = self.simulate(overrides=overrides or {}, enable_ltm=analyze_loops)
        sim.run_to_end()

        loops_structural = self.loops

        return Run(sim, overrides or {}, loops_structural)

    @property
    def base_case(self) -> "Run":
        """
        Simulation results with default parameters.

        Computed on first access and cached.

        Returns:
            Run object with baseline simulation results

        Example:
            >>> model.base_case.results['population'].plot()
        """
        if self._cached_base_case is None:
            self._cached_base_case = self.run()
        return self._cached_base_case

    def check(self) -> tuple[ModelIssue, ...]:
        """
        Check model for common issues.

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

    def check_units(self) -> tuple["UnitIssue", ...]:
        """
        Check dimensional consistency of equations.

        Returns tuple of unit issues found.

        Returns:
            Tuple of UnitIssue objects, or empty tuple if no unit issues

        Example:
            >>> issues = model.check_units()
            >>> errors = [i for i in issues if i.expected_units != i.actual_units]
        """
        from .types import UnitIssue
        from .errors import ErrorCode

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
        """
        Get human-readable explanation of a variable.

        Args:
            variable: Variable name

        Returns:
            Textual description of what defines/drives this variable

        Example:
            >>> print(model.explain('population'))
            "population is a stock increased by births and decreased by deaths"

        Raises:
            SimlinRuntimeError: If variable doesn't exist
        """
        for stock in self.stocks:
            if stock.name == variable:
                inflows_str = ", ".join(stock.inflows) if stock.inflows else "no inflows"
                outflows_str = ", ".join(stock.outflows) if stock.outflows else "no outflows"
                return f"{stock.name} is a stock with initial value {stock.initial_equation}, increased by {inflows_str}, decreased by {outflows_str}"

        for flow in self.flows:
            if flow.name == variable:
                return f"{flow.name} is a flow computed as {flow.equation}"

        for aux in self.auxs:
            if aux.name == variable:
                if aux.initial_equation:
                    return f"{aux.name} is an auxiliary variable computed as {aux.equation} with initial value {aux.initial_equation}"
                else:
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
    
    def __exit__(self, exc_type: Optional[type[BaseException]], exc_val: Optional[BaseException], exc_tb: Optional[TracebackType]) -> None:
        """Context manager exit point with explicit cleanup."""
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
        except:
            return "<Model (invalid)>"
