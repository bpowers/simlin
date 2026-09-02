// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
// Both queries only read salsa inputs and hand the results to the pure
// model-lowering core (`crate::model`): the parsing, dimension resolution and
// variable lowering they orchestrate all live there.

//! The two name-keyed, pre-layout model-compilation stages, as salsa queries.
//!
//! `ModelStage0` (each variable parsed to `Expr0`, module references still
//! unresolved) and `ModelStage1` (those variables lowered to `Expr2` against
//! the project's dimension context) are cheap to describe and expensive to
//! build, and every consumer that wanted one used to build its own. In
//! particular `db::units::check_model_units` is tracked PER MODEL yet rebuilt
//! both stages for EVERY project model on each call, so collecting a project's
//! unit diagnostics cost `M x (whole-project Stage0 + Stage1)` -- quadratic in
//! the model count (GH #966). Caching them here as `returns(ref)` queries makes
//! each model's stages one memoized value that consumers READ.
//!
//! The unit pass reads a SCOPE, not the project: [`model_scope_models`] is the
//! model plus the models it can reach through module instantiation, the map
//! `check_model_units` builds its inference over. The scope is that closure and
//! never the project: a unit check that read every model's stage would depend
//! on every model's parse results, so one keystroke anywhere would re-run every
//! unit check. A model's lowered stage reads only its own Stage0.
//!
//! This is the ONLY place in the crate the two stages are built from salsa
//! inputs. It is its own file, rather than more of `db/query.rs`, so that a
//! second construction site has to arrive as a new file instead of hiding as a
//! few more lines in a grab-bag module. A second copy is exactly the drift this
//! exists to prevent: two builders of the same stage silently disagree on the
//! small decisions (the stdlib `implicit` test), and every consumer then reads
//! whichever one it happened to call.
//!
//! Every other place in the crate that builds either stage, exhaustively --
//! this file exists to be where that list is right, so keep it complete:
//!
//! PRODUCTION: none. The per-variable constructors (`db::var_fragment`,
//! `db::fragment_compile`, `db::ltm::compile`) lower each variable under an
//! `ast::LoweringScope` of its own dependencies' shapes and never build a
//! stage; pointing them at these queries would add a whole-model dependency
//! edge to every fragment compile -- the opposite of the goal.
//! `db::units::check_conveyor_param_units` lowers its synthetic parameter auxes
//! one at a time under the cached Stage0's shapes and reads the cached Stage1;
//! see that module's header.
//!
//! `#[cfg(test)]` (the oracle surface, all database-free):
//!
//!   - `ModelStage0::new` / `new_in_project` build from a `datamodel::Model`
//!     with no database at all, which these queries cannot do. They are the
//!     independent oracle `db::stages_tests` checks this module against.
//!   - `db::stages_tests` lowers those oracle Stage0s through `ModelStage1::new`
//!     (`datamodel_driven_stage1s`, the lowering oracle): a construction site
//!     for the stage TYPE but never for a cached value.
//!
//! **Memory.** `returns(ref)` means salsa RETAINS one `ModelStage0` and one
//! `ModelStage1` per model for as long as their memos stay valid, where the old
//! code built them transiently and dropped them at the end of each
//! `check_model_units` call. That retention is the point of the change -- it is
//! what makes the stages readable instead of rebuildable -- but it is a real new
//! resident footprint, roughly two lowered copies of every equation in the
//! project (Stage0's `Expr0` plus Stage1's `Expr2`, both name-keyed and
//! pre-layout). On a C-LEARN-scale project that is the dominant term in this
//! module's cost; the old code's cost was CPU instead. Anything added to either
//! stage is paid per model per revision, so keep them to what lowering needs.
//!
//! Like `db::units`, this is a submodule of `db` reached as `crate::db::...`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::common::{Canonical, Ident};
use crate::db::{
    Db, SourceModel, SourceProject, parse_source_variable, project_dimensions_context,
};
use crate::model::{ModelStage0, ModelStage1, VariableStage0};
use crate::variable::VarKind;

// Test-only per-thread execution counters for the two stage queries and the
// unit-check pass that reads them.
//
// Pointer equality of a `returns(ref)` memo does NOT prove a query body did
// not run: salsa backdates a re-executed query whose value compares equal, and
// the memo keeps its address across that backdating. Counting body entries is
// the only evidence that separates "read the memo" from "rebuilt it and found
// it equal" -- and "built at most once per revision" is exactly the GH #966
// claim, while "an unrelated model's edit does not invalidate this one" is the
// scope-narrowing claim.
//
// `check_model_units` counts here rather than in `db::units` so that one reset
// covers every counter a test in this area reads. A second mechanism next door
// would let a test reset half of them and measure the other half against
// whatever an earlier test left behind.
//
// Thread-local rather than a global atomic so that a PARALLEL test run cannot
// charge one test's queries to another, and so no lock is needed. That alone
// does not isolate a measuring test: under `--test-threads=1` libtest runs
// every test on the same thread, so the counters carry over. The isolation that
// actually holds in both modes is `reset_query_executions()` at the start of
// the measured region -- see its rustdoc.
//
// The other edge of thread-local, and the dangerous one: the bump happens
// INSIDE the query body, on whatever thread salsa ran it. Nothing in this
// subtree executes in parallel today, but if salsa's `par_map` (or any other
// fan-out) is ever adopted for the stage queries, a body that runs on a worker
// thread bumps that thread's counter and the measuring test never sees it. The
// count then comes in UNDER the expected one and the test fails -- except in
// the case that matters, where the #966 regression is a query rebuilding on
// other threads and the test reads a comfortable-looking small number and goes
// green. Anyone parallelizing here must move these to a shared atomic (or scope
// them to the salsa runtime) in the same change, not afterwards.
#[cfg(test)]
thread_local! {
    static STAGE0_EXECUTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static STAGE1_EXECUTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static UNIT_CHECK_EXECUTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times each counted query body has run on this thread.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct QueryExecutions {
    pub(crate) stage0: usize,
    pub(crate) stage1: usize,
    pub(crate) unit_check: usize,
}

/// Zero every counter (test-only).
///
/// This is what isolates a measuring test, in BOTH libtest modes: the counters
/// are per-thread, and `--test-threads=1` puts every test on one thread, so a
/// test that does not reset would see whatever earlier tests left behind. Call
/// it after building the `SimlinDb` and syncing, so a stage the fixture setup
/// happened to demand is not charged to the measured work.
#[cfg(test)]
pub(crate) fn reset_query_executions() {
    STAGE0_EXECUTIONS.with(|c| c.set(0));
    STAGE1_EXECUTIONS.with(|c| c.set(0));
    UNIT_CHECK_EXECUTIONS.with(|c| c.set(0));
}

/// Read the current counters (test-only).
#[cfg(test)]
pub(crate) fn query_executions() -> QueryExecutions {
    QueryExecutions {
        stage0: STAGE0_EXECUTIONS.with(|c| c.get()),
        stage1: STAGE1_EXECUTIONS.with(|c| c.get()),
        unit_check: UNIT_CHECK_EXECUTIONS.with(|c| c.get()),
    }
}

/// Record one entry into a counted query's body (test-only).
#[cfg(test)]
fn note_execution(counter: &'static std::thread::LocalKey<std::cell::Cell<usize>>) {
    counter.with(|c| c.set(c.get() + 1));
}

/// Record one entry into `db::units::check_model_units` (test-only).
#[cfg(test)]
pub(super) fn note_unit_check_execution() {
    note_execution(&UNIT_CHECK_EXECUTIONS);
}

/// Is `canonical_model_name` one of the stdlib models `db::sync` splices into
/// every project?
///
/// The `stdlib⁚` prefix alone is NOT sufficient. It uses a punctuation
/// separator that ordinary model creation never produces, but an import can
/// still carry a model whose name has the prefix and a suffix naming no stdlib
/// model; flagging that model implicit would mark a user model as a generic
/// template. Requiring the suffix to be a real stdlib model name keeps the flag
/// on exactly the models the stdlib splice introduced.
///
/// This is the ONE stdlib test in the engine's diagnostic path: `db::units`'s
/// unit-check skip gate calls [`source_model_is_stdlib`] rather than carrying a
/// second, looser spelling (GH #988). A model with the prefix and an unknown
/// suffix is a user model, so it IS unit-checked -- the gate exists to skip
/// generic templates, and that is not one.
///
/// `pub(super)` (not private) so `db::stages_tests` can pin the rule directly:
/// nothing on the unit-checking path reads `ModelStage0::implicit`, so a flip
/// back to the bare-prefix test is invisible in the stage VALUE.
pub(super) fn model_is_stdlib(canonical_model_name: &str) -> bool {
    canonical_model_name
        .strip_prefix("stdlib\u{205A}")
        .is_some_and(|suffix| crate::stdlib::MODEL_NAMES.contains(&suffix))
}

/// [`model_is_stdlib`] for a salsa model handle.
///
/// `SourceModel::name` holds the DISPLAY name, so it is canonicalized first --
/// the project's model map is canonically keyed, and an imported model spelled
/// `Stdlib⁚Smth1` is the same model as `stdlib⁚smth1`.
pub(crate) fn source_model_is_stdlib(db: &dyn Db, model: SourceModel) -> bool {
    model_is_stdlib(Ident::<Canonical>::new(model.name(db)).as_str())
}

/// The models `model`'s unit inference can reach: itself, plus the transitive
/// closure of the models its module variables instantiate. Keyed by canonical
/// model name, valued by the project's handle for that name.
///
/// This is the SCOPE of `check_model_units`' inference map. It is narrower
/// than the project on purpose: with the whole-project map, every model's unit
/// check depended on every other model's parse results, so any edit anywhere
/// invalidated all of them (the follow-on GH #966 named and deferred).
///
/// # Why the closure is the right width
///
/// The closure is a SUPERSET of what its consumers can consult, not an exact
/// fit: too wide costs incrementality, too narrow is a silent wrong answer, so
/// where the two disagree this errs wide. `units_infer::gen_all_constraints`
/// reads its map at one site (`self.models.get(model_name)`, for module
/// targets), recursing along the target edges and DECLINING a back edge already
/// on its `InstantiationPath`, so it visits a subset of this closure;
/// `check_model_units`' stdlib-argument check looks a module's target up by
/// name. Both follow `model_name` edges and nothing else, which is what the walk
/// below follows. A model's LOWERED stage (`model_stage1`) reads none of this:
/// it lowers under the model's own variables' shapes, and a module input's
/// `src` is not validated at lowering -- the `BadModuleInputSrc` a modeller sees
/// comes from `db::diagnostic::model_module_wiring_diagnostics`, which reads the
/// salsa INPUTS and never a scope (`db::module_wiring_tests`).
///
/// # Implicit modules are IN, and that is load bearing
///
/// The edges come from each model's Stage0 `variables`, which holds the
/// SMOOTH/DELAY/TREND and macro-call modules that builtin expansion synthesized
/// alongside the declared ones. `db::project_module_graph` deliberately omits
/// those (it only needs the edges that can close a user cycle), so it is the
/// wrong source here: a macro call expands into a module targeting the macro's
/// own model, and `units_infer` binds the call's argument units to that model's
/// parameters by recursing through the edge -- dropping it drops the constraint
/// and, with it, the diagnostic (`unit_checking_test::test_smth1_unit_mismatch_initial`
/// is the stdlib twin of that loss).
///
/// Stdlib templates are in the closure on the same rule, rather than being
/// spliced in wholesale: a model that instantiates none of them does not reach
/// any, and one that instantiates `smth1` gets exactly `stdlib⁚smth1`.
///
/// # Cycles
///
/// The walk is an iterative worklist over a visited set, NOT a recursive tracked
/// query: `a` instantiating `b` and `b` instantiating `a` is a project a user can
/// draw, and a recursive salsa query on that graph is an unrecoverable
/// dependency-graph panic rather than a diagnostic (GH #806). A model inside a
/// cycle yields its full REACHABLE set -- the strongly-connected component it
/// sits in AND everything downstream of that component, since the walk does not
/// stop at the back edge, it only declines to re-visit -- and each member's
/// stage lowers against all of it.
///
/// # What the walk costs
///
/// It reads each reachable model's Stage0 and scans its `variables` map, so one
/// execution is O(variables in the closure) and it re-executes whenever any of
/// those Stage0s changes -- i.e. on an ordinary equation edit, for every model
/// that reaches the edited one. The RESULT is unchanged by such an edit, so
/// salsa backdates it and nothing downstream is invalidated by the re-execution
/// itself; the cost is the scan. That is deliberate: a per-model
/// `module targets` projection in between would let the closure survive those
/// edits untouched, but it is a third query to keep in step with these two for a
/// scan of an already-materialized map, and the models whose scope is wide are
/// the models whose Stage1 has to be rebuilt anyway.
#[salsa::tracked(returns(ref))]
pub(crate) fn model_scope_models(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> BTreeMap<Ident<Canonical>, SourceModel> {
    let project_models = project.models(db);
    let root_ident: Ident<Canonical> = Ident::new(model.name(db));

    let mut scope: BTreeMap<Ident<Canonical>, SourceModel> = BTreeMap::new();
    if let Some(src_model) = project_models.get(root_ident.as_str()) {
        scope.insert(root_ident.clone(), *src_model);
    }

    // The root is walked through the handle the CALLER passed, not through the
    // project's entry for its name: a renamed-or-deleted model's handle outlives
    // its place in that map (`PersistentSyncState` threads handles across syncs),
    // and its own module targets are still what its lowering will consult.
    let mut visited: BTreeSet<Ident<Canonical>> = [root_ident].into_iter().collect();
    let mut queue: Vec<SourceModel> = vec![model];
    while let Some(src_model) = queue.pop() {
        for var in model_stage0(db, src_model, project).variables.values() {
            let VarKind::Module { model_name, .. } = &var.kind else {
                continue;
            };
            if !visited.insert(model_name.clone()) {
                continue;
            }
            if let Some(next) = project_models.get(model_name.as_str()) {
                scope.insert(model_name.clone(), *next);
                queue.push(*next);
            }
        }
    }

    scope
}

/// A model's parsed-but-unresolved stage, built from the salsa-cached
/// per-variable parse results.
///
/// Keyed on `(model, project)`: the project supplies the dimension list, the
/// units context and the macro registry every parse reads, so a model cannot be
/// staged without one.
#[salsa::tracked(returns(ref))]
pub(crate) fn model_stage0(db: &dyn Db, model: SourceModel, project: SourceProject) -> ModelStage0 {
    #[cfg(test)]
    note_execution(&STAGE0_EXECUTIONS);

    let dim_ctx = project_dimensions_context(db, project);

    let display_name = model.name(db);
    let ident: Ident<Canonical> = Ident::new(display_name);
    let is_stdlib = source_model_is_stdlib(db, model);

    let src_vars = model.variables(db);
    let mut var_list: Vec<VariableStage0> = Vec::with_capacity(src_vars.len());
    let mut implicit_dm: Vec<crate::capture::ImplicitVar> = Vec::new();
    for svar in src_vars.values() {
        let parsed = parse_source_variable(db, *svar, project);
        var_list.push(parsed.variable.clone());
        implicit_dm.extend(parsed.implicit_vars.iter().cloned());
    }

    // The helpers the parses synthesized have no `SourceVariable` of their
    // own, so they are built here.
    var_list.extend(implicit_dm.iter().map(|iv| iv.parsed_variable(dim_ctx)));

    let variables: HashMap<Ident<Canonical>, VariableStage0> = var_list
        .into_iter()
        .map(|v| (Ident::new(v.ident()), v))
        .collect();

    ModelStage0 {
        ident,
        display_name: display_name.clone(),
        variables,
        implicit: is_stdlib,
        is_macro: model.macro_spec(db).is_some(),
        macro_params: crate::model::macro_param_idents(model.macro_spec(db).as_ref()),
    }
}

/// A model's lowered stage: `model_stage0` with every variable's AST lowered to
/// `Expr2` against the project's dimension context and the model's own
/// variables' shapes (`ModelStage0::lowering_shapes`).
///
/// A module output read (`m·x`) carries no bounds at this tier -- the `Expr2`
/// lowering does not resolve module-output dimensions, see
/// `ast::LoweringScope` -- so the stage reads nothing of any other model, and
/// an edit to a module target leaves its instantiators' stages untouched while
/// still re-running their unit checks, which read the target's stage through
/// `model_scope_models`
/// (`db::stages_tests::a_module_targets_edit_invalidates_the_unit_check_and_not_the_instantiators_stage`).
#[salsa::tracked(returns(ref))]
pub(crate) fn model_stage1(db: &dyn Db, model: SourceModel, project: SourceProject) -> ModelStage1 {
    #[cfg(test)]
    note_execution(&STAGE1_EXECUTIONS);

    ModelStage1::new(
        project_dimensions_context(db, project),
        model_stage0(db, model, project),
    )
}
