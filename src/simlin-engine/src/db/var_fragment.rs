// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The explicit-variable constructor of `compiler::fragment::FragmentInput`
//! (`explicit_fragment_input`) and the dependency-shape helpers every
//! constructor in `db/` composes.
//!
//! A fragment's input is the variable in its `Expr2` form plus the SHAPE of
//! every name it can reference -- dimensions, and whether the name is a plain
//! variable or a module instance (`DepShape`). Each shape is looked up through
//! the per-variable firewall queries (`model_variable_by_name`,
//! `model_implicit_var_by_name`, `variable_dimensions`, `model_shape` for a
//! module's sub-model), never by reading a whole-model map, so a fragment's
//! salsa dependencies are exactly the names it looks up.
//!
//! Because a plain function cannot accumulate salsa diagnostics, the
//! authoritative payloads are returned beside its optional input and emitted
//! by the tracked caller (`compile_var_fragment`). There are six distinct
//! outcomes:
//!
//! * a malformed unit-string error is *non-fatal* -- it is recorded but
//!   compilation of the variable continues;
//! * an equation parse error, an AST-lowering error, an unknown
//!   dependency, and a graphical-function table-build error are each
//!   *fatal* -- they return no fragment input while retaining every diagnostic;
//! * a per-phase lowering failure (`lower_fragment`'s `Err`) is
//!   *phase-local* -- only that phase's bytecode is dropped while the other
//!   phases still compile; the caller reports it per phase.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};

use crate::canonicalize;
use crate::common::{Canonical, Ident, IdentMap};
use crate::compiler::fragment::{DepShape, FragmentInput};
use crate::db::{
    Db, Diagnostic, DiagnosticCategory, DiagnosticSeverity, ImplicitVarMeta, ModuleInputSet,
    SourceModel, SourceProject, SourceVariable, SourceVariableKind, build_module_inputs,
    canonical_module_input_set, extract_tables_from_source_var, model_implicit_var_by_name,
    model_variable_by_name, module_dep_shape, module_input_prefix, parse_source_variable,
    project_converted_dimensions, project_dimensions_context, variable_dimensions,
    variable_direct_dependencies,
};
use crate::dimensions::{Dimension, DimensionsContext};

/// Result of preparing an explicit fragment. Diagnostics are in production
/// emission order; `input == None` means one of them is fatal.
pub(crate) struct ExplicitFragment<'db> {
    pub diagnostics: Vec<Diagnostic>,
    pub input: Option<Box<FragmentInput<'db>>>,
}

/// The four implicit globals lower to `LoadGlobalVar` at fixed absolute slots
/// and never go through a fragment's dependency shapes.
pub(crate) fn is_implicit_global(name: &str) -> bool {
    matches!(name, "time" | "dt" | "initial_time" | "final_time")
}

/// Split a dependency reference into the name a fragment resolves it through
/// and whether it is `·`-qualified: a leading `·` (a parent-scope reference,
/// as XMILE spells it) is stripped, and a qualified `m·x` yields `m`, the
/// module instance the read relocates through -- its sub-model variable is
/// resolved at lowering from the instance's shape.
pub(crate) fn dep_head(dep: &str) -> (&str, bool) {
    let effective = dep.strip_prefix('\u{00B7}').unwrap_or(dep);
    match effective.find('\u{00B7}') {
        Some(pos) => (&effective[..pos], true),
        None => (effective, false),
    }
}

/// Resolve datamodel dimension names to the project's `Dimension`s. A name the
/// project does not declare is dropped: the declaring variable's own fragment
/// reports it, and a shape with fewer axes fails loudly at lowering rather than
/// sizing storage it does not have.
pub(crate) fn dimensions_named(
    names: &[String],
    dim_context: &DimensionsContext,
) -> Vec<Dimension> {
    names
        .iter()
        .filter_map(|name| {
            dim_context
                .get(&crate::common::CanonicalDimensionName::from_raw(name))
                .cloned()
        })
        .collect()
}

/// The shape of a source variable: a module instance's sub-model shape, or the
/// variable's declared dimensions.
pub(crate) fn source_dep_shape(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
) -> DepShape {
    if var.kind(db) == SourceVariableKind::Module {
        module_dep_shape(db, project, var.model_name(db))
    } else {
        DepShape::var(variable_dimensions(db, var, project).clone())
    }
}

/// The shape of a parse-synthesized implicit helper: a module instance's
/// sub-model shape, or the helper's declared dimensions (an arrayed
/// `PREVIOUS`/`INIT` capture, GH #541; every other helper is scalar).
pub(crate) fn implicit_dep_shape(
    db: &dyn Db,
    project: SourceProject,
    meta: &ImplicitVarMeta,
) -> DepShape {
    if meta.is_module {
        module_dep_shape(db, project, meta.model_name.as_deref().unwrap_or(""))
    } else {
        DepShape::var(dimensions_named(
            &meta.dimensions,
            project_dimensions_context(db, project),
        ))
    }
}

/// The shape of the variable `name` denotes in `model` -- a source variable or
/// a parse-synthesized helper, each through its firewall query -- or `None`
/// when the model declares neither.
pub(crate) fn model_dep_shape(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    name: &str,
) -> Option<DepShape> {
    if let Some(var) = model_variable_by_name(db, model, name.to_string()) {
        return Some(source_dep_shape(db, var, project));
    }
    model_implicit_var_by_name(db, model, project, name.to_string())
        .map(|meta| implicit_dep_shape(db, project, &meta))
}

/// Whether `flow_ident` names a flow that is DRIVEN by a special-stock
/// expansion pass: an outflow of a stock carrying the `<conveyor>` or `<queue>`
/// marker. Such a flow's value comes from the native pass (which writes its slot
/// each step), not from an equation, so its absence of an `<eqn>` is
/// spec-sanctioned rather than a modeling error (docs/design/conveyors.md,
/// docs/design/queues.md).
///
/// Determined structurally over the model's stocks. In a VALID conveyor the
/// outflow set is exactly {primary, leaks}; a queue drives every outflow. The
/// only non-driven outflow a conveyor can carry is a rejected SECOND non-leak
/// outflow (F11), whose model the expansion rejects loudly on the simulation
/// channel regardless -- so treating every conveyor/queue outflow as driven
/// never hides a real problem in an otherwise-valid model, and keeps the guard
/// a simple, marker-only structural rule.
///
/// Reads each stock's `kind`/`outflows`/`compat` through salsa inputs, so the
/// enclosing `compile_var_fragment` gains a dependency on the owning stock's
/// marker: dropping the `<conveyor>`/`<queue>` block re-emits the flow's
/// `EmptyEquation` diagnostic (pinned by
/// `test_conveyor_marker_removal_reinstates_empty_equation`).
///
/// Salsa-tracked for the same firewall reason as `model_variable_by_name`:
/// answering the question needs a scan of every stock, so an untracked helper
/// would give its caller a dependency on the whole variables map. Tracked, the
/// `bool` backdates and only a genuine change of the flow's driven-ness
/// invalidates the fragment.
#[salsa::tracked(returns(clone))]
pub(crate) fn flow_is_special_stock_driven(
    db: &dyn Db,
    model: SourceModel,
    flow_ident: String,
) -> bool {
    let flow_canon = canonicalize(&flow_ident);
    model.variables(db).values().any(|sv| {
        sv.kind(db) == SourceVariableKind::Stock
            && sv
                .outflows(db)
                .iter()
                .any(|o| canonicalize(o) == flow_canon)
            && {
                let compat = sv.compat(db);
                compat.conveyor.is_some() || compat.queue.is_some()
            }
    })
}

/// Lower one source variable to `Expr2` through the same dependency-granular
/// context used by fragment compilation.
///
/// Unit analysis and fragment compilation both read this tracked projection.
/// The memo owns one lowered variable, rather than embedding a second copy in a
/// cached whole-model value, and its dependency lookups are firewalled by name.
#[salsa::tracked(returns(ref))]
pub(crate) fn lowered_source_variable(
    db: &dyn Db,
    var: SourceVariable,
    model: SourceModel,
    project: SourceProject,
) -> std::sync::Arc<crate::variable::Variable> {
    let var_ident = var.ident(db).clone();
    let model_name = model.name(db);
    let parsed = parse_source_variable(db, var, project);
    if var.kind(db) == SourceVariableKind::Module {
        return std::sync::Arc::new(
            crate::model::resolve_parsed_module(
                &parsed.variable,
                build_module_inputs(
                    model_name,
                    &module_input_prefix(&canonicalize(&var_ident)),
                    var.module_refs(db)
                        .iter()
                        .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
                ),
            )
            .expect("a source variable classified Module parses to VarKind::Module"),
        );
    }

    let deps = variable_direct_dependencies(db, var, project, ModuleInputSet::empty(db));
    let mut local_names: BTreeSet<String> = deps
        .dependencies
        .iter()
        .filter(|dependency| dependency.target.module_path.is_empty())
        .map(|dependency| dependency.target.variable.as_str().to_owned())
        .collect();
    local_names.extend(deps.referenced_tables.iter().cloned());
    if var.kind(db) == SourceVariableKind::Stock {
        local_names.extend(
            var.inflows(db)
                .iter()
                .chain(var.outflows(db).iter())
                .map(|flow| canonicalize(flow).into_owned()),
        );
    }

    let mut parsed_vars = HashMap::new();
    parsed_vars.insert(Ident::new(&var_ident), Cow::Borrowed(&parsed.variable));
    for dep_name in local_names {
        if let Some(dep_var) = model_variable_by_name(db, model, dep_name.clone()) {
            let dep_parsed = parse_source_variable(db, dep_var, project);
            parsed_vars.insert(Ident::new(&dep_name), Cow::Borrowed(&dep_parsed.variable));
        }
    }
    let lowering_model = crate::model::LoweringModel {
        variables: parsed_vars,
    };
    let models = [(Ident::new(model_name), lowering_model)]
        .into_iter()
        .collect();
    let scope = crate::model::LoweringScope {
        models: &models,
        dimensions: project_dimensions_context(db, project),
        model_name,
    };
    std::sync::Arc::new(crate::model::lower_variable(&scope, &parsed.variable))
}

/// Build the fragment input of one source variable: parse it, lower its
/// equation to `Expr2`, and resolve the shape of every name it references.
///
/// `module_input_names` is the module instance's input wiring. The dependency
/// set itself is built input-agnostic (the empty `ModuleInputSet`) so both
/// branches of an `isModuleInput` conditional stay compilable.
pub(crate) fn explicit_fragment_input<'db>(
    db: &'db dyn Db,
    var: SourceVariable,
    model: SourceModel,
    project: SourceProject,
    module_input_names: &[String],
) -> ExplicitFragment<'db> {
    let var_ident = var.ident(db).clone();
    let model_name = model.name(db);
    let parsed = parse_source_variable(db, var, project);

    // Unit definition errors are syntax errors in the unit string (e.g. "bad
    // units here!!!") recorded by the parse; they do not stop compilation.
    let unit_diagnostics: Vec<Diagnostic> = parsed
        .variable
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.category == DiagnosticCategory::UnitDefinition)
        .cloned()
        .collect();

    // Parse errors are fatal -- every one is accumulated before bailing out.
    //
    // A pass-driven flow -- a conveyor stock's primary/leak outflow or ANY
    // outflow of a queue stock -- is spec-sanctioned to carry no `<eqn>`: the
    // native conveyor/queue expansion pass writes its slot each step (the
    // expansion gives the flow a placeholder `0` equation, conveyor_compile.rs /
    // queue_compile.rs). This diagnostic path runs over the UN-expanded
    // datamodel, so without a marker-aware guard every driven flow would be
    // reported as a phantom `EmptyEquation` Error on a model that simulates
    // correctly (docs/design/conveyors.md, docs/design/queues.md). Suppress ONLY
    // the empty-equation code, and ONLY for such a flow -- a genuine parse error
    // on a driven flow, or an empty equation on any non-driven variable, still
    // surfaces. `flow_is_special_stock_driven` reads the owning stock's compat
    // through salsa inputs, so removing the `<conveyor>`/`<queue>` marker
    // invalidates this fragment and the `EmptyEquation` Error reappears.
    let parse_errors: Vec<Diagnostic> = parsed
        .variable
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.category != DiagnosticCategory::UnitDefinition)
        .cloned()
        .collect();
    if !parse_errors.is_empty() {
        let suppress_empty = var.kind(db) == SourceVariableKind::Flow
            && parse_errors
                .iter()
                .any(|diagnostic| diagnostic.code == crate::common::ErrorCode::EmptyEquation)
            && flow_is_special_stock_driven(db, model, var_ident.clone());
        let fatal_diagnostics: Vec<Diagnostic> = parse_errors
            .into_iter()
            .filter(|diagnostic| {
                !(suppress_empty && diagnostic.code == crate::common::ErrorCode::EmptyEquation)
            })
            .collect();
        // Even when every parse diagnostic is the marker-sanctioned empty
        // equation, there is no source AST to compile. Stop preparation after
        // suppressing those rows; continuing would make `lower_fragment`
        // manufacture a second EmptyEquation diagnostic for the same flow.
        return ExplicitFragment {
            diagnostics: unit_diagnostics
                .into_iter()
                .chain(fatal_diagnostics)
                .collect(),
            input: None,
        };
    }

    let deps = variable_direct_dependencies(db, var, project, ModuleInputSet::empty(db));

    // Per-call memo over `model_variable_by_name`. The salsa firewall query is
    // the SOURCE of truth for the lookup (that is what gives the fragment a
    // dependency on the named variable rather than on the whole variables map);
    // this only stops the name loops below from asking it the same question
    // several times, which on a dependency-heavy model was the whole measurable
    // cost of the narrowing.
    let mut resolved: HashMap<String, Option<SourceVariable>> = HashMap::new();
    let mut resolve_var = |name: &str| -> Option<SourceVariable> {
        if let Some(hit) = resolved.get(name) {
            return *hit;
        }
        let found = model_variable_by_name(db, model, name.to_string());
        resolved.insert(name.to_string(), found);
        found
    };

    // A bare reference to a standalone lookup-only table -- the table used as a
    // value rather than called via `LOOKUP(table, x)` -- has no scalar value of
    // its own and is rejected (issue #606). After the table-reference /
    // data-flow-dependency split (`referenced_tables`), a lookup-only table can
    // ONLY reach the structured data dependencies via such a bare `Var(table)`
    // reference (a real call lands in `referenced_tables`), so its presence in
    // the dependency sets is exactly this error.
    {
        let referenced: BTreeSet<&crate::db::DepTarget> =
            deps.dependencies.iter().map(|dep| &dep.target).collect();
        let bare_table_diags: Vec<Diagnostic> = referenced
            .into_iter()
            .filter_map(|dep| {
                if !dep.module_path.is_empty() {
                    return None;
                }
                let dep_name = dep.variable.as_str();
                let dep_sv = resolve_var(dep_name)?;
                crate::db::source_var_is_table_only(db, dep_sv).then(|| {
                    Diagnostic::engine(
                        crate::common::Error::new(
                            crate::common::ErrorKind::Model,
                            crate::common::ErrorCode::LookupReferencedWithoutArgument,
                            Some(format!(
                                "'{dep_name}' is a lookup table and must be called with an \
                             argument, e.g. {dep_name}(x) or LOOKUP({dep_name}, x)"
                            )),
                        ),
                        DiagnosticSeverity::Error,
                    )
                })
            })
            .collect();
        if !bare_table_diags.is_empty() {
            return ExplicitFragment {
                diagnostics: unit_diagnostics
                    .into_iter()
                    .chain(bare_table_diags)
                    .collect(),
                input: None,
            };
        }
    }

    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);

    // Every name the fragment references: the two phases' data-flow
    // dependencies, the lookup tables it calls (a table reference is a layout
    // reference -- codegen needs the table's identity -- not a data-flow
    // dependency, so it lives in `referenced_tables`, issue #606), and a
    // stock's inflows and outflows (read by its update expression).
    let data_targets: BTreeSet<&crate::db::DepTarget> =
        deps.dependencies.iter().map(|dep| &dep.target).collect();
    let mut local_names: BTreeSet<String> = data_targets
        .iter()
        .filter(|target| target.module_path.is_empty())
        .map(|target| target.variable.as_str().to_owned())
        .collect();
    local_names.extend(deps.referenced_tables.iter().cloned());
    let stock_flows: Vec<String> = if var.kind(db) == SourceVariableKind::Stock {
        var.inflows(db)
            .iter()
            .chain(var.outflows(db).iter())
            .map(|flow| canonicalize(flow).into_owned())
            .collect()
    } else {
        Vec::new()
    };
    local_names.extend(stock_flows.iter().cloned());
    let lowered = lowered_source_variable(db, var, model, project);

    // Errors introduced during AST lowering (e.g. `MismatchedDimensions` from
    // the Expr2/Expr3 lowering) land on the lowered variable, not the parsed
    // one, so they are checked separately.
    let lowering_diags: Vec<Diagnostic> = lowered
        .diagnostics
        .iter()
        .skip(parsed.variable.diagnostics.len())
        .cloned()
        .collect();
    if !lowering_diags.is_empty() {
        return ExplicitFragment {
            diagnostics: unit_diagnostics.into_iter().chain(lowering_diags).collect(),
            input: None,
        };
    }

    // The shape of every name the fragment can reference, itself included.
    // Nothing here carries an offset in this model: the model's layout is
    // assigned at assembly and lowering names its references. That is what
    // makes a fragment position-independent, and hence what lets ONE salsa
    // cache entry per variable serve both the diagnostic pass and assembly and
    // survive unrelated variables coming and going. First-inserted-wins, and
    // the variable's own entry comes first.
    let self_ident: Ident<Canonical> = Ident::new(&var_ident);
    let self_shape = if var.kind(db) == SourceVariableKind::Module {
        module_dep_shape(db, project, var.model_name(db))
    } else {
        DepShape::var(
            lowered
                .get_dimensions()
                .map(<[Dimension]>::to_vec)
                .unwrap_or_default(),
        )
    };
    let mut dep_shapes: IdentMap<Ident<Canonical>, DepShape> = Default::default();
    dep_shapes.insert(self_ident.clone(), self_shape);
    for target in &data_targets {
        let head = target.local_node().as_str();
        let qualified = !target.module_path.is_empty();
        if head == var_ident.as_str() || is_implicit_global(head) || dep_shapes.contains_key(head) {
            continue;
        }
        let shape = match resolve_var(head) {
            Some(dep_sv) => Some(source_dep_shape(db, dep_sv, project)),
            None => model_implicit_var_by_name(db, model, project, head.to_string())
                .map(|meta| implicit_dep_shape(db, project, &meta)),
        };
        match shape {
            Some(shape) => {
                dep_shapes.insert(Ident::new(head), shape);
            }
            // A qualified name whose instance the model does not declare
            // (`module.output` after the module was deleted) is refused at
            // lowering, as `DoesNotExist` on the referencing phase.
            None if qualified => {}
            None => {
                // Neither a source variable nor an implicit helper: an unknown
                // dependency. Point the error at the reference site, and name
                // the reference: the span alone leaves a reader of a rename or
                // a deletion guessing which of several names went missing, and
                // the diagnostic's `variable` is the REFERRING variable, not
                // this one.
                let loc = parsed
                    .variable
                    .ast()
                    .and_then(|ast| ast.get_var_loc(head))
                    .unwrap_or_default();
                return ExplicitFragment {
                    diagnostics: unit_diagnostics
                        .into_iter()
                        .chain(std::iter::once(Diagnostic::equation(
                            crate::common::EquationError::detailed(
                                crate::common::ErrorCode::UnknownDependency,
                                loc.start,
                                loc.end,
                                format!("'{head}' is not a variable of model '{model_name}'"),
                            ),
                            DiagnosticSeverity::Error,
                        )))
                        .collect(),
                    input: None,
                };
            }
        }
    }
    for flow in &stock_flows {
        if flow == &var_ident || dep_shapes.contains_key(flow.as_str()) {
            continue;
        }
        if let Some(dep_sv) = resolve_var(flow) {
            dep_shapes.insert(Ident::new(flow), source_dep_shape(db, dep_sv, project));
        }
    }
    for table_name in &deps.referenced_tables {
        let (head, qualified) = dep_head(table_name);
        if qualified || dep_shapes.contains_key(head) {
            continue;
        }
        if let Some(dep_sv) = resolve_var(head) {
            dep_shapes.insert(Ident::new(head), source_dep_shape(db, dep_sv, project));
        }
    }
    // The implicit module instances this variable's parse synthesized
    // (`INIT(x)` creates `$⁚x⁚0⁚init`, whose output the rewritten equation
    // reads as `$⁚x⁚0⁚init·output`): the read relocates through the instance.
    for implicit_var in &parsed.implicit_vars {
        if let Some(dm_module) = implicit_var.module() {
            dep_shapes
                .entry(Ident::new(dm_module.ident()))
                .or_insert_with(|| module_dep_shape(db, project, dm_module.model_name()));
        }
    }

    // Graphical-function tables: the variable's own (a build error is fatal
    // rather than silently dropped, which would shift table indices and make
    // lookups read the wrong table at runtime) and those of the tables it calls
    // through `LOOKUP(dep, x)`, which codegen needs to emit the `Lookup`.
    let mut tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>> = HashMap::new();
    let gf_tables = lowered.tables();
    if !gf_tables.is_empty() {
        match gf_tables
            .iter()
            .map(|t| crate::compiler::Table::new(&var_ident, t))
            .collect::<crate::Result<Vec<_>>>()
        {
            Ok(ts) if !ts.is_empty() => {
                tables.insert(self_ident, ts);
            }
            Err(table_err) => {
                return ExplicitFragment {
                    diagnostics: unit_diagnostics
                        .into_iter()
                        .chain(std::iter::once(Diagnostic::model(
                            table_err,
                            DiagnosticSeverity::Error,
                        )))
                        .collect(),
                    input: None,
                };
            }
            _ => {}
        }
    }
    for dep_name in &local_names {
        if tables.contains_key(dep_name.as_str()) {
            continue;
        }
        if let Some(dep_sv) = resolve_var(dep_name) {
            let dep_tables = extract_tables_from_source_var(db, &dep_sv, project);
            if !dep_tables.is_empty() {
                tables.insert(Ident::new(dep_name), dep_tables);
            }
        }
    }

    ExplicitFragment {
        diagnostics: unit_diagnostics,
        // The salsa memo owns this Arc for at least the input's query-scoped
        // lifetime; borrowing it keeps compilation on that exact payload.
        input: Some(Box::new(FragmentInput::new(
            Cow::Borrowed(lowered),
            dep_shapes,
            tables,
            canonical_module_input_set(module_input_names),
            Ident::new(model_name),
            converted_dims,
            dim_context,
        ))),
    }
}
