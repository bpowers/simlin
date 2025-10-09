"""Model class for working with system dynamics models."""

from __future__ import annotations

from typing import Dict, List, Optional, Tuple, TYPE_CHECKING, Any, Self, Union
from types import TracebackType

from ._ffi import ffi, lib, string_to_c, c_to_string, free_c_string, _register_finalizer, get_error_string
from .errors import SimlinRuntimeError, ErrorCode
from .analysis import Link, LinkPolarity, Loop
from .types import Stock, Flow, Aux, TimeSpec, GraphicalFunction, GraphicalFunctionScale, ModelIssue
from . import pb

if TYPE_CHECKING:
    from .sim import Sim
    from .project import Project
    from .run import Run


def _variable_ident(variable: pb.Variable) -> str:
    kind = variable.WhichOneof("v")
    if kind is None:
        raise ValueError("Variable message has no assigned variant")
    ident = getattr(getattr(variable, kind), "ident", None)
    if not ident:
        raise ValueError("Variable missing identifier")
    return ident


def _copy_variable(variable: pb.Variable) -> pb.Variable:
    clone = pb.Variable()
    clone.CopyFrom(variable)
    return clone


class ModelPatchBuilder:
    """Accumulates model operations before applying them to the engine."""

    def __init__(self, model_name: str) -> None:
        self._patch = pb.ModelPatch()
        self._patch.name = model_name

    @property
    def model_name(self) -> str:
        return self._patch.name

    def has_operations(self) -> bool:
        return bool(self._patch.ops)

    def build(self) -> pb.ModelPatch:
        patch = pb.ModelPatch()
        patch.CopyFrom(self._patch)
        return patch

    def _add_op(self) -> pb.ModelOperation:
        return self._patch.ops.add()

    def upsert_stock(self, stock: pb.Variable.Stock) -> pb.Variable.Stock:
        op = self._add_op()
        op.upsert_stock.stock.CopyFrom(stock)
        return op.upsert_stock.stock

    def upsert_flow(self, flow: pb.Variable.Flow) -> pb.Variable.Flow:
        op = self._add_op()
        op.upsert_flow.flow.CopyFrom(flow)
        return op.upsert_flow.flow

    def upsert_aux(self, aux: pb.Variable.Aux) -> pb.Variable.Aux:
        op = self._add_op()
        op.upsert_aux.aux.CopyFrom(aux)
        return op.upsert_aux.aux

    def upsert_module(self, module: pb.Variable.Module) -> pb.Variable.Module:
        op = self._add_op()
        op.upsert_module.module.CopyFrom(module)
        return op.upsert_module.module

    def delete_variable(self, ident: str) -> None:
        op = self._add_op()
        op.delete_variable.ident = ident

    def rename_variable(self, current_ident: str, new_ident: str) -> None:
        op = self._add_op()
        setattr(op.rename_variable, "from", current_ident)
        op.rename_variable.to = new_ident

    def upsert_view(self, index: int, view: pb.View) -> pb.View:
        op = self._add_op()
        op.upsert_view.index = index
        op.upsert_view.view.CopyFrom(view)
        return op.upsert_view.view

    def delete_view(self, index: int) -> None:
        op = self._add_op()
        op.delete_view.index = index


class _ModelEditContext:
    def __init__(self, model: "Model", dry_run: bool, allow_errors: bool) -> None:
        self._model = model
        self._dry_run = dry_run
        self._allow_errors = allow_errors
        self._current: Dict[str, pb.Variable] = {}
        self._patch = ModelPatchBuilder(model._name or "")

    def __enter__(self) -> Tuple[Dict[str, pb.Variable], ModelPatchBuilder]:
        project = self._model._project
        if project is None:
            raise SimlinRuntimeError("Model is not attached to a Project")

        project_proto = pb.Project()
        project_proto.ParseFromString(project.serialize())

        model_proto = None
        for candidate in project_proto.models:
            if candidate.name == self._model._name or not self._model._name:
                model_proto = candidate
                break

        if model_proto is None:
            raise SimlinRuntimeError(
                f"Model '{self._model._name or 'default'}' not found in project serialization"
            )

        self._model._name = model_proto.name
        self._patch = ModelPatchBuilder(model_proto.name)
        self._current = {_variable_ident(var): _copy_variable(var) for var in model_proto.variables}
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

        project_patch = pb.ProjectPatch()
        model_patch = project_patch.models.add()
        model_patch.CopyFrom(self._patch.build())

        project._apply_patch(
            project_patch,
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
        count = lib.simlin_model_get_incoming_links(self._ptr, c_var_name, ffi.NULL, 0)
        if count < 0:
            error_msg = get_error_string(-count)
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
            error_msg = get_error_string(-actual_count)
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
        from ._ffi import lib, ffi
        from .errors import SimlinRuntimeError

        sim_ptr = lib.simlin_sim_new(self._ptr, enable_ltm)
        if sim_ptr == ffi.NULL:
            raise SimlinRuntimeError("Failed to create simulation")

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
