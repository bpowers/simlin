// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The per-variable lowering memo of an explicit variable
//! (`lowered_source_variable`), the explicit-variable constructor of
//! `compiler::fragment::FragmentInput` (`explicit_fragment_input`) and the
//! dependency-shape helpers every constructor in `db/` composes.
//!
//! A fragment's input is the variable in its `Expr2` form plus the SHAPE of
//! every name it can reference -- dimensions, and whether the name is a plain
//! variable or a module instance (`DepShape`). Each shape is looked up through
//! the per-variable firewall queries (`model_variable_by_name`,
//! `model_implicit_var_by_name`, `variable_dimensions`, `model_shape` for a
//! module's sub-model), never by reading a whole-model map, so a fragment's
//! salsa dependencies are exactly the names it looks up. The memo lowers the
//! equation under those same shapes and is the one owner of the variable's
//! `Expr2` form: the fragment borrows it, and unit checking and the LTM
//! describers hold its `Arc` (`db::model_lowered_variables`).
//!
//! The LTM overlay (`db::LtmOverlay`) reaches a fragment through exactly one
//! of those shapes, a module instance's (`module_dep_shape`: the sub-model's
//! layout, which carries its LTM section under `On`). So a fragment is keyed
//! on the overlay only where it resolves one: [`fragment_reads_module`] is
//! that question, [`fragment_overlay`] turns it into the key a caller asks
//! for, and each constructor asserts the two agree with the shapes it built,
//! since an under-approximation would be a silent miscompile.
//!
//! Because a plain function cannot accumulate salsa diagnostics, the
//! diagnostics the constructor would emit are returned **as data**
//! (`ExplicitFragment::diagnostics`) and replayed by the tracked caller
//! (`compile_var_fragment`). This is where a variable's context-free
//! `Variable::diagnostics` become `Diagnostic`s: the constructor knows the
//! model and the variable, and everything it raises is an `Error`. There are
//! six distinct diagnostic outcomes:
//!
//! * a malformed unit-string error is *non-fatal* -- it is reported but
//!   compilation of the variable continues (`input` is still built);
//! * an equation parse error, an AST-lowering error, an unknown
//!   dependency, and a graphical-function table-build error are each
//!   *fatal* -- they abort this variable's compilation (`input` is `None`,
//!   whole-variable `None` at the caller);
//! * a per-phase lowering failure (`lower_fragment`'s `Err`) is
//!   *phase-local* -- only that phase's bytecode is dropped while the other
//!   phases still compile; the caller reports it per phase.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::ast::LoweringScope;
use crate::canonicalize;
use crate::common::{Canonical, Ident, IdentMap};
use crate::compiler::fragment::{DepShape, FragmentInput};
use crate::db::{
    Db, Diagnostic, DiagnosticError, DiagnosticSeverity, ImplicitVarMeta, LtmOverlay,
    ModuleInputSet, ParsedVariableResult, SourceModel, SourceProject, SourceVariable,
    SourceVariableKind, VariableDeps, canonical_module_input_set, model_implicit_var_by_name,
    model_variable_by_name, module_dep_shape, parse_source_variable, project_converted_dimensions,
    project_dimensions_context, variable_dimensions, variable_direct_dependencies, variable_tables,
};
use crate::dimensions::{Dimension, DimensionsContext};
use crate::variable::Variable;

/// Outcome of [`explicit_fragment_input`]: every diagnostic the constructor
/// raised, in emission order (the malformed-unit rows first, then the first
/// fatal site's), and the fragment's input when no diagnostic was fatal.
pub(crate) struct ExplicitFragment<'db> {
    pub diagnostics: Vec<Diagnostic>,
    /// Boxed: a `FragmentInput` holds its maps inline, and the constructor's
    /// failure paths return the struct without one.
    pub input: Option<Box<FragmentInput<'db>>>,
}

/// The four implicit globals lower to `LoadGlobalVar` at fixed absolute slots
/// and never go through a fragment's dependency shapes.
pub(crate) fn is_implicit_global(name: &str) -> bool {
    matches!(name, "time" | "dt" | "initial_time" | "final_time")
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

/// The shape of a plain (non-module) source variable: its declared
/// dimensions. The one owner of that rule; every arm that resolves a source
/// variable's shape calls it, so the declared-dimensions-to-shape step
/// cannot be stated differently in two places.
pub(crate) fn source_dimensions(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
) -> DepShape {
    DepShape::var(variable_dimensions(db, var, project).clone())
}

/// The shape of a plain (non-module) implicit helper: its declared
/// dimensions (a structural apply-to-all capture; every other helper is
/// scalar). The one owner of that rule, as `source_dimensions` is for a
/// source variable.
pub(crate) fn helper_dimensions(
    db: &dyn Db,
    project: SourceProject,
    meta: &ImplicitVarMeta,
) -> DepShape {
    DepShape::var(dimensions_named(
        &meta.dimensions,
        project_dimensions_context(db, project),
    ))
}

/// The shape of a source variable: a module instance's sub-model shape, or the
/// variable's declared dimensions.
pub(crate) fn source_dep_shape(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
    overlay: LtmOverlay,
) -> DepShape {
    if var.kind(db) == SourceVariableKind::Module {
        module_dep_shape(db, project, var.model_name(db), overlay)
    } else {
        source_dimensions(db, var, project)
    }
}

/// The shape of a parse-synthesized implicit helper: a module instance's
/// sub-model shape, or the helper's declared dimensions (a structural
/// apply-to-all capture; every other helper is scalar).
pub(crate) fn implicit_dep_shape(
    db: &dyn Db,
    project: SourceProject,
    meta: &ImplicitVarMeta,
    overlay: LtmOverlay,
) -> DepShape {
    if meta.is_module {
        module_dep_shape(
            db,
            project,
            meta.model_name.as_deref().unwrap_or(""),
            overlay,
        )
    } else {
        helper_dimensions(db, project, meta)
    }
}

/// The head of every name a variable resolves through, each with what the
/// model declares it as: the projection of a variable's reads the lowering
/// memos carry and the fragment constructors read the compiler's shapes from.
pub(crate) type ResolvedHeads = Vec<(Ident<Canonical>, DeclaredName)>;

/// What `model` declares a name as: one of its source variables, or a helper a
/// parse of one of them synthesized -- each through its firewall query
/// (`model_variable_by_name`, `model_implicit_var_by_name`), so a reader
/// depends on the one name it looked up. The one owner of name resolution for
/// every fragment constructor and lowering memo; the two shapes a resolved
/// name has are its methods.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum DeclaredName {
    Source(SourceVariable),
    /// Boxed: almost every head a variable references is a source variable
    /// (an 8-byte id), and the memos retain one head per referenced name.
    Helper(Box<ImplicitVarMeta>),
}

impl DeclaredName {
    pub(crate) fn resolve(
        db: &dyn Db,
        model: SourceModel,
        project: SourceProject,
        name: &str,
    ) -> Option<Self> {
        if let Some(var) = model_variable_by_name(db, model, name.to_string()) {
            return Some(DeclaredName::Source(var));
        }
        model_implicit_var_by_name(db, model, project, name.to_string())
            .as_ref()
            .map(|meta| DeclaredName::Helper(Box::new(meta.clone())))
    }

    /// Whether the name is a module instance: an explicit `Module` variable,
    /// or a helper the parse expanded a module-function call into. The one
    /// name-level question [`shape`](Self::shape) branches on, and hence the
    /// one that decides whether the reader's shape depends on the overlay.
    pub(crate) fn is_module(&self, db: &dyn Db) -> bool {
        match self {
            DeclaredName::Source(var) => var.kind(db) == SourceVariableKind::Module,
            DeclaredName::Helper(meta) => meta.is_module,
        }
    }

    /// The shape the compiler resolves the name through: a module instance's
    /// sub-model shape, or the declared dimensions.
    ///
    /// A module instance's shape is its sub-model's layout, which depends on
    /// the `overlay` (the sub-model carries its LTM section under it), so a
    /// reader of this shape is keyed on the overlay too; a plain variable's
    /// shape is its dimensions, the same under either.
    pub(crate) fn shape(
        &self,
        db: &dyn Db,
        project: SourceProject,
        overlay: LtmOverlay,
    ) -> DepShape {
        match self {
            DeclaredName::Source(var) => source_dep_shape(db, *var, project, overlay),
            DeclaredName::Helper(meta) => implicit_dep_shape(db, project, meta, overlay),
        }
    }

    /// The shape the `Expr2` tier lowers under: the declared dimensions of a
    /// plain variable or helper, and NOTHING for a module instance. `lower_ast`
    /// reads only a shape's dimensions, of which an instance has none, while
    /// an instance's compiler shape is its sub-model's LAYOUT (`db::model_shape`,
    /// a recursive query): reading it from a lowering memo would put every
    /// sub-model's layout in every cross-module reader's dependency cone, and
    /// on a module cycle -- which the unit pass reaches, its scope being an
    /// iterative worklist for exactly that reason -- turn the lowering into
    /// salsa's dependency-graph panic.
    ///
    /// Overlay-independent by construction: it never reads a sub-model's
    /// layout, which is also what keeps the lowering memos that read it
    /// keyed on the variable alone.
    pub(crate) fn dimensions_shape(&self, db: &dyn Db, project: SourceProject) -> Option<DepShape> {
        match self {
            DeclaredName::Source(var) => (var.kind(db) != SourceVariableKind::Module)
                .then(|| source_dimensions(db, *var, project)),
            DeclaredName::Helper(meta) => {
                (!meta.is_module).then(|| helper_dimensions(db, project, meta))
            }
        }
    }
}

/// Whether a fragment assembled from these pieces resolves a module
/// instance's shape: the variable is itself an instance, one of the `heads`
/// its equation resolves through is one, or its parse minted one
/// (`implicit_vars`, whose instances are keys of the fragment's shapes
/// whether or not the equation reads their output -- see `compiler_shapes`).
///
/// The pure form of [`fragment_reads_module`], so the tracked predicate and
/// the constructors' assertions state the rule once. It reads only what its
/// callers have already read: the variable's and each source head's `kind`
/// (which `source_dep_shape` reads for every head), a helper head's
/// `is_module` field, and the parse memo's `implicit_vars`.
pub(crate) fn resolves_a_module_shape(
    db: &dyn Db,
    self_is_module: bool,
    heads: &[(Ident<Canonical>, DeclaredName)],
    implicit_vars: &[crate::capture::ImplicitVar],
) -> bool {
    self_is_module
        || heads.iter().any(|(_, declared)| declared.is_module(db))
        || implicit_vars
            .iter()
            .any(crate::capture::ImplicitVar::is_module)
}

/// Whether `var`'s fragment resolves a module instance's shape
/// ([`resolves_a_module_shape`]) -- equivalently, whether its value depends
/// on the LTM overlay, since a module instance's shape is the sub-model's
/// layout (`module_dep_shape`, which carries that model's LTM section under
/// `On`) and every other shape is a dimension list, the same under either.
/// [`fragment_overlay`] turns it into the overlay a fragment is keyed on.
///
/// Tracked, and reading only memos the fragment itself depends on (the
/// parse, the lowering, the heads' kinds), for the reason every projection
/// query in `db/` is tracked: the answer is a `bool` that backdates across
/// every edit leaving it unchanged, where the lowering memo it is derived
/// from moves with the equation's text. A caller resolving a fragment's key
/// through it therefore gains a dependency that an ordinary equation edit
/// does not invalidate.
///
/// The one place it demands more than the fragment does is a variable whose
/// equation did not PARSE: the constructor bails before lowering it, where
/// this still asks for the (total, and then trivial) lowering memo. Such a
/// fragment is `None` under either overlay, so the key it lands on does not
/// matter -- only that the answer stays correct.
#[salsa::tracked(returns(copy))]
pub(crate) fn fragment_reads_module(
    db: &dyn Db,
    var: SourceVariable,
    model: SourceModel,
    project: SourceProject,
) -> bool {
    // An instance's OWN shape is its sub-model's layout, whatever it reads.
    if var.kind(db) == SourceVariableKind::Module {
        return true;
    }
    let parsed = parse_source_variable(db, var, project);
    let lowered = lowered_source_variable(db, var, model, project);
    resolves_a_module_shape(db, false, &lowered.heads, &parsed.implicit_vars)
}

/// [`fragment_reads_module`] for one of the implicit helpers a parse
/// synthesized, keyed on the helper's canonical name -- the only identity a
/// helper has, and the key `model_implicit_var_info`,
/// `lowered_implicit_variable` and `compile_implicit_var_fragment` all file
/// it under. `false` for a name the model synthesizes no helper of, which is
/// also the fragment those callers get (`ImplicitInputError::Absent`).
///
/// A helper mints no helpers of its own -- its parse is its parent's -- so
/// the instances it can resolve through are all among its heads.
#[salsa::tracked(returns(copy))]
pub(crate) fn implicit_fragment_reads_module(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    implicit_var_name: String,
) -> bool {
    let Some(meta) =
        model_implicit_var_by_name(db, model, project, implicit_var_name.clone()).as_ref()
    else {
        return false;
    };
    let Some(lowered) = crate::db::lowered_implicit_variable(db, model, project, implicit_var_name)
    else {
        return false;
    };
    resolves_a_module_shape(db, meta.is_module, &lowered.heads, &[])
}

/// The overlay `var`'s fragment is compiled and memoized under when a caller
/// asks for `requested`: `requested` where the fragment resolves a module
/// instance's shape ([`fragment_reads_module`]), `Off` otherwise, since the
/// fragment's value is then the same under either overlay and one memo
/// serves both.
///
/// Every production caller of the fragment queries resolves its key through
/// this, so no pass keys a fragment on an overlay the fragment does not
/// read. A plain (`Off`) compile short-circuits: it asks for the key it
/// would get anyway, so it never even demands the predicate.
pub(crate) fn fragment_overlay(
    db: &dyn Db,
    var: SourceVariable,
    model: SourceModel,
    project: SourceProject,
    requested: LtmOverlay,
) -> LtmOverlay {
    if requested == LtmOverlay::Off || fragment_reads_module(db, var, model, project) {
        requested
    } else {
        LtmOverlay::Off
    }
}

/// [`fragment_overlay`] for one of the implicit helpers a parse synthesized,
/// over [`implicit_fragment_reads_module`].
pub(crate) fn implicit_fragment_overlay(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    implicit_var_name: String,
    requested: LtmOverlay,
) -> LtmOverlay {
    if requested == LtmOverlay::Off
        || implicit_fragment_reads_module(db, model, project, implicit_var_name)
    {
        requested
    } else {
        LtmOverlay::Off
    }
}

/// The shape of the variable `name` denotes in `model`, as the compiler
/// resolves it, or `None` when the model declares neither a variable nor a
/// helper of that name.
pub(crate) fn model_dep_shape(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    name: &str,
    overlay: LtmOverlay,
) -> Option<DepShape> {
    DeclaredName::resolve(db, model, project, name)
        .map(|declared| declared.shape(db, project, overlay))
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

/// Every name `var`'s equation resolves through: the head of each of its
/// reads (the module instance a qualified read relocates through, whose
/// sub-model variable is resolved at lowering from the instance's shape), the
/// lookup tables it calls (a table reference is a layout reference -- codegen
/// needs the table's identity -- not a data-flow dependency, so it lives in
/// `referenced_tables`, issue #606), and a stock's inflows and outflows (read
/// by its update expression).
fn referenced_names(
    db: &dyn Db,
    var: SourceVariable,
    deps: &VariableDeps,
) -> BTreeSet<Ident<Canonical>> {
    let mut names: BTreeSet<Ident<Canonical>> = deps
        .deps
        .heads()
        .into_iter()
        .chain(deps.referenced_tables.iter())
        .cloned()
        .collect();
    if var.kind(db) == SourceVariableKind::Stock {
        names.extend(
            var.inflows(db)
                .iter()
                .chain(var.outflows(db).iter())
                .map(|flow| Ident::new(flow)),
        );
    }
    names
}

/// The shape of a source variable as its OWN fragment's entry: a module
/// instance's sub-model shape, or the declared dimensions the parse resolved.
fn source_self_shape(
    db: &dyn Db,
    var: SourceVariable,
    project: SourceProject,
    parsed: &ParsedVariableResult,
    overlay: LtmOverlay,
) -> DepShape {
    if var.kind(db) == SourceVariableKind::Module {
        module_dep_shape(db, project, var.model_name(db), overlay)
    } else {
        DepShape::var(
            parsed
                .variable
                .get_dimensions()
                .map(<[Dimension]>::to_vec)
                .unwrap_or_default(),
        )
    }
}

/// Every name in `names` that `model` declares, resolved once (the variable
/// itself and the implicit globals skipped), and the first name the model
/// declares nowhere -- an unknown dependency, reported by the fragment
/// constructor. The instance a qualified read relocates through is proven by
/// the dependency query, so it resolves; a qualified spelling the query could
/// not prove (`module.output` after the module was deleted) is one local name
/// the model declares nowhere and is reported as such.
fn resolve_referenced_heads(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    self_ident: &str,
    names: &BTreeSet<Ident<Canonical>>,
) -> (ResolvedHeads, Option<Ident<Canonical>>) {
    let mut heads: ResolvedHeads = Vec::new();
    let mut unknown: Option<Ident<Canonical>> = None;
    for head in names {
        if head.as_str() == self_ident || is_implicit_global(head.as_str()) {
            continue;
        }
        match DeclaredName::resolve(db, model, project, head.as_str()) {
            Some(declared) => heads.push((head.clone(), declared)),
            None => {
                if unknown.is_none() {
                    unknown = Some(head.clone());
                }
            }
        }
    }
    (heads, unknown)
}

/// The shape of every name a source variable's equation can reference, itself
/// included, as the compiler resolves them: the map the compiler resolves
/// every reference through. Nothing here carries an offset in this model: the
/// model's layout is assigned at assembly and lowering names its references.
/// That is what makes a fragment position-independent, and hence what lets
/// ONE salsa cache entry per variable serve both the diagnostic pass and
/// assembly and survive unrelated variables coming and going. First-inserted
/// wins, and the variable's own entry comes first. The implicit module
/// instances the parse synthesized (`SMTH1(x, 3)` creates `$⁚v⁚0⁚smth1`, whose
/// output the rewritten equation reads as `$⁚v⁚0⁚smth1·output`) are keys too:
/// the read relocates through the instance.
fn compiler_shapes(
    db: &dyn Db,
    project: SourceProject,
    self_ident: &str,
    self_shape: DepShape,
    heads: &[(Ident<Canonical>, DeclaredName)],
    parsed: &ParsedVariableResult,
    overlay: LtmOverlay,
) -> IdentMap<Ident<Canonical>, DepShape> {
    let mut dep_shapes: IdentMap<Ident<Canonical>, DepShape> = Default::default();
    dep_shapes.insert(Ident::new(self_ident), self_shape);
    for (ident, declared) in heads {
        dep_shapes.insert(ident.clone(), declared.shape(db, project, overlay));
    }
    for implicit_var in &parsed.implicit_vars {
        if let Some(dm_module) = implicit_var.module() {
            dep_shapes
                .entry(Ident::new(&dm_module.ident))
                .or_insert_with(|| module_dep_shape(db, project, &dm_module.model_name, overlay));
        }
    }
    dep_shapes
}

/// The shape of every name an equation can reference as the `Expr2` tier
/// lowers under it: the equation's own declared dimensions and each resolved
/// head's (`DeclaredName::dimensions_shape`), so the tier reads a
/// dependency's dimensions from the same firewall answer the compiler reads
/// its shape from.
fn expr2_shapes(
    db: &dyn Db,
    project: SourceProject,
    self_ident: &str,
    self_dims: Option<&[Dimension]>,
    heads: &[(Ident<Canonical>, DeclaredName)],
) -> IdentMap<Ident<Canonical>, DepShape> {
    let mut shapes: IdentMap<Ident<Canonical>, DepShape> = Default::default();
    shapes.insert(
        Ident::new(self_ident),
        DepShape::var(self_dims.map(<[Dimension]>::to_vec).unwrap_or_default()),
    );
    for (ident, declared) in heads {
        if let Some(shape) = declared.dimensions_shape(db, project) {
            shapes.insert(ident.clone(), shape);
        }
    }
    shapes
}

/// One source variable lowered once, with the names its equation references
/// resolved once: what [`lowered_source_variable`] memoizes.
#[derive(Clone, PartialEq)]
pub(crate) struct LoweredSource {
    /// The variable in its `Expr2` form, lowered under the dimensions of
    /// `heads` ([`expr2_shapes`]).
    pub variable: Arc<Variable>,
    /// The head of every name the equation references that the model declares
    /// ([`resolve_referenced_heads`]): `explicit_fragment_input` projects the
    /// compiler's shapes and the tables it needs from these, so a variable's
    /// names are resolved once per revision rather than once by the memo and
    /// again by the fragment.
    pub heads: ResolvedHeads,
    /// The first name the model declares nowhere -- the unknown dependency
    /// `explicit_fragment_input` reports.
    pub unknown: Option<Ident<Canonical>>,
}

/// One source variable in its `Expr2` form, lowered once under the
/// dimensions of the names its equation references ([`expr2_shapes`]).
///
/// The one owner of a variable's lowered form. `explicit_fragment_input`
/// borrows it (`Cow::Borrowed`), and the unit check and the LTM describers hold
/// its `Arc` through `db::model_lowered_variables`, so the compile, diagnostic
/// and analysis paths lower each variable exactly once per revision
/// (`db::lowered_variable_tests`). The memo is keyed on the variable and its
/// owning model -- the model names the scope a module instance's wiring
/// resolves under -- and reads its dependencies' dimensions through the
/// firewall queries, so an equation edit re-lowers the edited variable alone:
/// a dependent lowers under the edited variable's SHAPE, which the edit leaves
/// unchanged.
///
/// A module instance is its wiring: `lower_variable`'s module arm resolves it
/// under the model's canonical name (`db::build_module_inputs`' `main` rule
/// compares canonical names, so a root model spelled `Main` wires as `main`
/// does); its input sources are still resolved, since the instance's fragment
/// reads their shapes.
///
/// Salsa retains the value for as long as it stays valid, in every mode: the
/// plain compile path pays that residency for a lowered tree it reads once,
/// where unit checking and LTM read it many times (the Phase 8.2 ledger row
/// of `docs/design-plans/2026-08-25-compiler-unification.md` records the
/// measured cost).
#[salsa::tracked(returns(ref))]
pub(crate) fn lowered_source_variable(
    db: &dyn Db,
    var: SourceVariable,
    model: SourceModel,
    project: SourceProject,
) -> LoweredSource {
    let parsed = parse_source_variable(db, var, project);
    let model_ident: Ident<Canonical> = Ident::new(model.name(db));
    let deps = variable_direct_dependencies(db, var, project, ModuleInputSet::empty(db));
    let names = referenced_names(db, var, deps);
    let (heads, unknown) = resolve_referenced_heads(db, model, project, var.ident(db), &names);
    let shapes = expr2_shapes(
        db,
        project,
        var.ident(db),
        parsed.variable.get_dimensions(),
        &heads,
    );
    let scope = LoweringScope {
        dimensions: project_dimensions_context(db, project),
        shapes: &shapes,
        model_name: model_ident.as_str(),
    };
    LoweredSource {
        variable: Arc::new(crate::model::lower_variable(&scope, &parsed.variable)),
        heads,
        unknown,
    }
}

/// Build the fragment input of one source variable: its lowered form
/// ([`lowered_source_variable`], borrowed) plus the shape of every name it
/// references as the compiler resolves it ([`compiler_shapes`]) and the tables
/// it calls.
///
/// `module_input_names` is the module instance's input wiring, which selects
/// the live `isModuleInput` branch at lowering; the parse and the dependency
/// set are instance-agnostic (the parse by construction, the dependency set
/// through the empty `ModuleInputSet`) so both branches of such a conditional
/// stay compilable and one memo of each serves every instance.
pub(crate) fn explicit_fragment_input<'db>(
    db: &'db dyn Db,
    var: SourceVariable,
    model: SourceModel,
    project: SourceProject,
    module_input_names: &[String],
    overlay: LtmOverlay,
) -> ExplicitFragment<'db> {
    let var_ident = var.ident(db).clone();
    let model_name = model.name(db);
    let parsed = parse_source_variable(db, var, project);

    // The one place an explicit variable's context-free diagnostics gain
    // their context: this constructor knows the model and the variable, and
    // everything it raises stops or degrades compilation, so it is an `Error`.
    let diagnostic = |error: DiagnosticError| Diagnostic {
        model: model_name.clone(),
        variable: Some(var_ident.clone()),
        owner: None,
        severity: DiagnosticSeverity::Error,
        error,
    };
    let fatal = |diagnostics: Vec<Diagnostic>| ExplicitFragment {
        diagnostics,
        input: None,
    };

    // Unit definition errors are syntax errors in the unit string (e.g. "bad
    // units here!!!") recorded by the parse; they do not stop compilation and
    // come first in the parse's own order.
    let mut diagnostics: Vec<Diagnostic> = parsed
        .variable
        .diagnostics
        .iter()
        .filter(|d| matches!(d, DiagnosticError::Unit(_)))
        .map(|d| diagnostic(d.clone()))
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
    let parse_failures: Vec<&DiagnosticError> = parsed.variable.fatal_diagnostics().collect();
    if !parse_failures.is_empty() {
        let suppress_empty = var.kind(db) == SourceVariableKind::Flow
            && parse_failures
                .iter()
                .any(|d| d.code() == crate::common::ErrorCode::EmptyEquation)
            && flow_is_special_stock_driven(db, model, var_ident.clone());
        diagnostics.extend(
            parse_failures
                .into_iter()
                .filter(|d| {
                    !(suppress_empty && d.code() == crate::common::ErrorCode::EmptyEquation)
                })
                .map(|d| diagnostic(d.clone())),
        );
        return fatal(diagnostics);
    }

    let deps = variable_direct_dependencies(db, var, project, ModuleInputSet::empty(db));
    let LoweredSource {
        variable: lowered,
        heads,
        unknown: unknown_head,
    } = lowered_source_variable(db, var, model, project);

    // A bare reference to a standalone lookup-only table -- the table used as a
    // value rather than called via `LOOKUP(table, x)` -- has no scalar value of
    // its own and is rejected (issue #606). After the table-reference /
    // data-flow-dependency split (`referenced_tables`), a lookup-only table can
    // ONLY be a read (`deps`) through such a bare `Var(table)` reference or as
    // the source a module input is wired from, which copies a slot the table
    // does not have (a real call lands in `referenced_tables`), so a read of
    // it is exactly this error.
    {
        let bare_table_diags: Vec<Diagnostic> = heads
            .iter()
            .filter_map(|(ident, declared)| {
                let DeclaredName::Source(dep_sv) = declared else {
                    return None;
                };
                let dep = ident.as_str();
                let referenced_bare = deps.deps.reads_local(ident);
                (referenced_bare && crate::db::source_var_is_table_only(db, *dep_sv)).then(|| {
                    diagnostic(DiagnosticError::Model(crate::common::Error::new(
                        crate::common::ErrorKind::Model,
                        crate::common::ErrorCode::LookupReferencedWithoutArgument,
                        Some(format!(
                            "'{dep}' is a lookup table and must be called with an \
                             argument, e.g. {dep}(x) or LOOKUP({dep}, x)"
                        )),
                    )))
                })
            })
            .collect();
        if !bare_table_diags.is_empty() {
            diagnostics.extend(bare_table_diags);
            return fatal(diagnostics);
        }
    }

    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);

    let self_shape = source_self_shape(db, var, project, parsed, overlay);
    let dep_shapes = compiler_shapes(db, project, &var_ident, self_shape, heads, parsed, overlay);

    // The overlay reaches this fragment only through a module shape in
    // `dep_shapes` -- every `module_dep_shape` call above put one there --
    // and `fragment_reads_module` is what the memo's key is resolved from
    // (`fragment_overlay`), so the two must agree exactly. An
    // UNDER-approximation is a silent miscompile: an `On` assembly would read
    // a plain-keyed fragment whose module offsets were resolved against the
    // sub-model's `Off` layout, with no bad id and no bad reference to
    // notice. An over-approximation only costs a second memo. Asserted rather
    // than trusted, and asserted over the shapes themselves rather than
    // re-deriving them, because the two statements sit in different
    // functions and nothing else would notice them drifting apart. The
    // assertion reads only what this constructor has already read, so the
    // salsa dependency set is the same in debug and release builds.
    debug_assert_eq!(
        dep_shapes.values().any(DepShape::is_module),
        resolves_a_module_shape(
            db,
            var.kind(db) == SourceVariableKind::Module,
            heads,
            &parsed.implicit_vars
        ),
        "'{var_ident}': `fragment_reads_module` disagrees with the shapes its fragment resolves"
    );

    // Errors introduced during AST lowering (e.g. `MismatchedDimensions` from
    // the Expr2 lowering) land on the lowered variable, not the parsed one, so
    // they are checked separately. A lowering refusal is about the equation
    // as written and outranks a name missing from the model around it, which
    // is reported next: at the reference site, naming the reference, since the
    // span alone leaves a reader of a rename or a deletion guessing which of
    // several names went missing.
    let lowering_failures: Vec<&DiagnosticError> = lowered.fatal_diagnostics().collect();
    if !lowering_failures.is_empty() {
        diagnostics.extend(lowering_failures.into_iter().map(|d| diagnostic(d.clone())));
        return fatal(diagnostics);
    }
    if let Some(head) = unknown_head {
        let loc = parsed
            .variable
            .ast()
            .and_then(|ast| ast.get_var_loc(head.as_str()))
            .unwrap_or_default();
        diagnostics.push(diagnostic(DiagnosticError::Equation(
            crate::common::EquationError::detailed(
                crate::common::ErrorCode::UnknownDependency,
                loc.start,
                loc.end,
                format!("'{head}' is not a variable of model '{model_name}'"),
            ),
        )));
        return fatal(diagnostics);
    }

    // Graphical-function tables: the variable's own (a build error is fatal
    // rather than silently dropped, which would shift table indices and make
    // lookups read the wrong table at runtime) and those of the tables it calls
    // through `LOOKUP(dep, x)`, which codegen needs to emit the `Lookup`.
    let self_ident: Ident<Canonical> = Ident::new(&var_ident);
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
                diagnostics.push(diagnostic(DiagnosticError::Model(table_err)));
                return fatal(diagnostics);
            }
            _ => {}
        }
    }
    for (ident, declared) in heads {
        let DeclaredName::Source(dep_sv) = declared else {
            continue;
        };
        let dep_tables = variable_tables(db, *dep_sv, project);
        if !dep_tables.is_empty() {
            tables.insert(ident.clone(), dep_tables.clone());
        }
    }

    ExplicitFragment {
        diagnostics,
        input: Some(Box::new(FragmentInput::new(
            Cow::Borrowed(lowered.as_ref()),
            dep_shapes,
            tables,
            canonical_module_input_set(module_input_names),
            Ident::new(model_name),
            converted_dims,
            dim_context,
        ))),
    }
}
