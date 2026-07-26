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
//! Those consumers read a SCOPE, not the project: [`model_scope_models`] is the
//! model plus the models it can reach through module instantiation, and it is
//! the map both `model_stage1` and `check_model_units` build over. With the
//! whole-project map the caching was still quadratic in READS, and worse, every
//! model's lowered stage and unit check depended on every other model's parse
//! results, so one keystroke anywhere invalidated all of them.
//!
//! This is the ONLY place in the crate the two stages are built from salsa
//! inputs. It is its own file, rather than more of `db/query.rs`, so that a
//! second construction site has to arrive as a new file instead of hiding as a
//! few more lines in a grab-bag module -- the drift this exists to delete.
//! `Project::from_salsa` used to carry a second copy (silently disagreeing on
//! three fields: the stdlib `implicit` test, the stdlib module-ident set, and
//! whether duplicate-canonical-ident model errors are recorded); this body took
//! `from_salsa`'s behaviour in all three, so retiring that copy was a deletion
//! rather than a merge, and `from_salsa` now clones these memos.
//!
//! Every other place in the crate that builds either stage, exhaustively --
//! this file exists to be where that list is right, so keep it complete:
//!
//! PRODUCTION (three, none a whole-model salsa build, each deliberate):
//!
//!   - `db::var_fragment::lower_var_fragment` (`ModelStage0` literal) and
//!     `db::ltm::compile` (ditto) build a per-variable MINI Stage0 holding only
//!     that variable's dependencies, then call `lower_variable` directly rather
//!     than `ModelStage1::new`. Pointing those at this query would add a
//!     project-wide dependency edge to every fragment compile -- the opposite of
//!     the goal.
//!   - `db::units::check_conveyor_param_units` calls `ModelStage1::new` on a
//!     CLONE of the cached Stage0 augmented with synthetic conveyor-parameter
//!     auxes. The augmented stage is a throwaway on purpose; see that module's
//!     header.
//!
//! `#[cfg(test)]` (the oracle surface, all database-free):
//!
//!   - `ModelStage0::new` / `new_in_project` build from a `datamodel::Model`
//!     with no database at all, which these queries cannot do. They are the
//!     independent oracle `db::stages_tests` checks this module against.
//!   - Four `ModelStage1::new` call sites lower those oracle Stage0s:
//!     `model.rs` (the dependency-resolution and `enumerate_modules` tests) and
//!     `db::stages_tests::datamodel_driven_stage1s` (the whole-project lowering
//!     oracle). They take a `ScopeStage0` the test builds by hand, so they are
//!     construction sites for the stage TYPE but never for a cached value.
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
use crate::datamodel;
use crate::db::{
    Db, ModuleIdentContext, SourceModel, SourceProject, model_duplicate_variables,
    model_module_ident_context, parse_source_variable_with_module_context, project_datamodel_dims,
    project_dimensions_context, project_units_context,
};
use crate::model::{ModelStage0, ModelStage1, ScopeStage0, VariableStage0};
use crate::variable::Variable;

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

/// The module-ident names a model's variable parses must treat as module-backed
/// ON TOP OF the ones `model_module_ident_context` derives from the model's own
/// module variables and module-call equations.
///
/// For a stdlib model that is EVERY variable name. Inside a submodule some
/// variables are module inputs whose values arrive through a transient array
/// and have no persistent slot, so `PREVIOUS(module_input)` must first capture
/// the current value into a scalar helper aux rather than compile a `LoadPrev`
/// against a slot that does not exist.
///
/// No shipped stdlib body calls `PREVIOUS`/`INIT`, so today this changes no
/// parse result. It is kept because it is the rule the datamodel-driven
/// `ModelStage0::new` uses for an implicit model, and the rule
/// `Project::from_salsa` used while it still built its own stages: one rule
/// means every path reaching a stdlib model's parse hits the SAME
/// `ModuleIdentContext` and shares one
/// `parse_source_variable_with_module_context` cache entry, instead of minting
/// a second set of parses under a second key.
///
/// `pub(super)` for the same reason as [`model_is_stdlib`]: the rule is inert
/// in the stage VALUE today, so `db::stages_tests` pins it here.
pub(super) fn extra_module_idents(db: &dyn Db, model: SourceModel, is_stdlib: bool) -> Vec<String> {
    if is_stdlib {
        model.variables(db).keys().cloned().collect()
    } else {
        vec![]
    }
}

/// The two facts [`model_stage0`] derives about a model before it parses
/// anything: whether it is a stdlib template, and the interned module-ident
/// context its variables must be parsed under.
pub(super) struct Stage0Context<'db> {
    pub(super) is_stdlib: bool,
    pub(super) module_idents: ModuleIdentContext<'db>,
}

/// Derive [`Stage0Context`] for `model`.
///
/// The two are derived TOGETHER, in one `pub(super)` function, on purpose. The
/// stdlib rule feeds both `ModelStage0::implicit` and the extra module idents,
/// and when they were derived separately inside the query body the extra-ident
/// half was untestable: reverting it to `vec![]` at the call site changed no
/// stage value and no test failed, because the rule is inert in the parse
/// result today (see [`extra_module_idents`]). `model_stage0` cannot obtain
/// `implicit` without also obtaining the context, so the wiring now has exactly
/// one place to go wrong, and `db::stages_tests` pins that place on the
/// INTERNED context identity rather than on a parse result that cannot differ.
pub(super) fn model_stage0_context<'db>(
    db: &'db dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> Stage0Context<'db> {
    let is_stdlib = source_model_is_stdlib(db, model);
    Stage0Context {
        is_stdlib,
        module_idents: model_module_ident_context(
            db,
            model,
            project,
            extra_module_idents(db, model, is_stdlib),
        ),
    }
}

/// The models `model`'s lowering and unit inference can reach: itself, plus the
/// transitive closure of the models its module variables instantiate. Keyed by
/// canonical model name, valued by the project's handle for that name.
///
/// This is the SCOPE of both `model_stage1` below and `check_model_units`'
/// inference map. It is narrower than the project on purpose: with the
/// whole-project map, every model's lowered stage and unit check depended on
/// every other model's parse results, so any edit anywhere invalidated all of
/// them (the follow-on GH #966 named and deferred).
///
/// # Why the closure is the right width
///
/// The closure is a SUPERSET of what any consumer can consult, not an exact
/// fit: too wide costs incrementality, too narrow is a silent wrong answer, so
/// where the two disagree this errs wide. Two lowering steps read another
/// model's Stage0, and only these two:
///
///   - `ArrayContext::get_variable` (`ast/mod.rs`) resolves a dotted
///     `module·var` reference by looking the MODULE VARIABLE up in the current
///     model and recursing into the model it names. The `model_name` edges below
///     are exactly the edges that walk can follow, so for THIS consumer the
///     closure is exactly its reach.
///   - `resolve_relative` (`model.rs`), reached from `lower_variable`'s module
///     arm, resolves a module input's `src`. It predates modules being able to
///     carry an ident different from their target model's name and keys each
///     dotted component as a MODEL name (see its `module ident == model name`
///     TODO). That is why a module's own IDENT is an edge here as well when it
///     happens to name a project model -- otherwise a project with both would
///     start reporting a `BadModuleInputSrc` that the whole-project map did not.
///     Those ident edges are what make the closure strictly WIDER than
///     `ArrayContext`'s reach.
///
/// `units_infer::gen_all_constraints` reads its map at one site
/// (`self.models.get(model_name)`, for module targets), recursing along the
/// target edges and DECLINING a back edge already on its `InstantiationPath`, so
/// it visits a subset of this closure and the same closure covers it.
///
/// Where the ident edges leave a residual: `resolve_relative` walks components
/// of an arbitrary `src` STRING, so a `src` naming a model that is neither a
/// module target nor a module ident of the model would resolve under a
/// whole-project map and is a `BadModuleInputSrc` here. That is judged
/// malformed -- a well-formed XMILE `connect from` names a module instance in
/// the same model -- and three things support it, none of which is "the corpus
/// is green":
///
///   - **A corpus run could not have caught it.** The `BadModuleInputSrc` this
///     mints lands on a `ModelStage1` variable's `errors` and reaches no
///     production diagnostic: `db::assemble::build_module_inputs` does not
///     validate `src`, and nothing on the unit path reads `ModelStage1` variable
///     errors. So a green corpus is not evidence here, and citing it would
///     mislead the next person narrowing something.
///   - **The user-facing warning is scope-independent.** The
///     `BadModuleInputSrc` a modeller actually sees comes from
///     `db::diagnostic::model_module_wiring_diagnostics`, which reads the salsa
///     INPUTS (`model.variables`, `project.models`, `svar.module_refs`) and
///     never a lowering scope. Narrowing this map can therefore neither create
///     nor suppress one (`db::module_wiring_tests`).
///   - **Direct measurement.** Every `resolve_module_input` call was
///     instrumented and counted under the narrowed scope: 560 of 560 resolved
///     across the `file_io` integration corpus, and 970 across the lib suite
///     with exactly one failure -- `db::module_wiring_tests::dangling_src_warns`,
///     which deliberately wires a `src` naming no variable at all and whose
///     scope map held both project models. That is a fixture asserting the
///     warning, not a scope miss.
///
/// The review added a fourth, independently: a dual-lowering oracle inside
/// `model_stage1` that lowered every model under both this scope and the
/// whole-project one and diffed `variables`, across the corpus and the lib
/// suite, finding zero user-model divergence.
///
/// # Implicit modules are IN, and that is load bearing
///
/// The edges come from each model's Stage0 `variables`, which holds the
/// SMOOTH/DELAY/TREND and macro-call modules that builtin expansion synthesized
/// alongside the declared ones. `db::project_module_graph` deliberately omits
/// those (it only needs the edges that can close a user cycle), so it is the
/// wrong source here: a macro call expands into a module targeting the macro's
/// own model, which is an ordinary user model that can perfectly well be
/// arrayed. Dropping it would make `get_dimensions` return `None` and lower the
/// reference as a scalar -- compilable, plausible, and wrong.
///
/// Stdlib templates are in the closure on the same rule, rather than being
/// spliced in wholesale: a model that instantiates none of them does not stage
/// any, and one that instantiates `smth1` gets exactly `stdlib⁚smth1`.
/// `db::stages_tests::omitting_stdlib_models_from_the_lowering_scope_is_inert_today`
/// records why the wholesale alternative would be safe TODAY; this rule does not
/// depend on that staying true.
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
            let Variable::Module {
                ident, model_name, ..
            } = var
            else {
                continue;
            };
            for target in [model_name, ident] {
                if !visited.insert(target.clone()) {
                    continue;
                }
                if let Some(next) = project_models.get(target.as_str()) {
                    scope.insert(target.clone(), *next);
                    queue.push(*next);
                }
            }
        }
    }

    scope
}

/// [`model_scope_models`] resolved to Stage0s: the `models` map a
/// [`ScopeStage0`] lowering scope needs.
///
/// Both readers -- `model_stage1` below and the conveyor parameter check in
/// `db::units`, which lowers its synthetic parameter auxes in the same scope --
/// need the identical map. Building it in two places would re-introduce, one
/// level up, exactly the duplicated construction this module exists to remove.
///
/// The self entry is REPAIRED rather than assumed. Every caller reaches this
/// with a handle the project's model map holds under the same name, so the
/// insert below is inert -- a pointer write of a value already there. It is here
/// for the case the call sites cannot show: a handle that outlived its place in
/// that map (renamed, or deleted while something still holds it). With no self
/// entry `ArrayContext::get_model` returns `None`, so `get_dimensions` returns
/// `None` for every reference in the model and every arrayed equation lowers as
/// though it were scalar. That compiles, simulates, and is wrong -- the exact
/// failure class this plan exists to delete, which is why it is repaired rather
/// than detected. A hard `assert!` was considered and rejected: libsimlin builds
/// with panic=abort, so an assert that ever fired would abort the user's process
/// (a WASM tab, an MCP server) over something recoverable.
///
/// One BEHAVIOUR DELTA rides on that repair, so it is recorded rather than left
/// to read as a pure substitution. `db::units::check_conveyor_param_units` used
/// the un-repaired whole-project map, so it GAINS the self-entry repair by moving
/// here. It is a strict improvement -- that function lowers an `aug_ms0` built
/// from THIS model's Stage0, and it now lowers it in a scope that is guaranteed
/// to contain this model, which is what it always assumed -- but it is a change,
/// reachable only on the same stale-handle path the repair exists for.
pub(crate) fn model_scope_stage0(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashMap<Ident<Canonical>, &ModelStage0> {
    let mut models_s0: HashMap<Ident<Canonical>, &ModelStage0> =
        model_scope_models(db, model, project)
            .values()
            .map(|src_model| {
                let s0 = model_stage0(db, *src_model, project);
                (s0.ident.clone(), s0)
            })
            .collect();
    let model_s0 = model_stage0(db, model, project);
    models_s0.insert(model_s0.ident.clone(), model_s0);
    models_s0
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

    let units_ctx = project_units_context(db, project);
    let dm_dims = project_datamodel_dims(db, project);

    let display_name = model.name(db);
    let ident: Ident<Canonical> = Ident::new(display_name);
    let Stage0Context {
        is_stdlib,
        module_idents,
    } = model_stage0_context(db, model, project);

    let src_vars = model.variables(db);
    let mut var_list: Vec<VariableStage0> = Vec::with_capacity(src_vars.len());
    let mut implicit_dm: Vec<datamodel::Variable> = Vec::new();
    for svar in src_vars.values() {
        let parsed = parse_source_variable_with_module_context(db, *svar, project, module_idents);
        var_list.push(parsed.variable.clone());
        implicit_dm.extend(parsed.implicit_vars.iter().cloned());
    }

    // The implicit variables SMOOTH/DELAY/TREND expansion synthesized have no
    // `SourceVariable` of their own, so they are parsed directly here. They are
    // plain stocks/flows/auxes and module instances, never module CALLS, so the
    // expansion cannot recurse -- assert that rather than silently dropping a
    // second generation into the `nested` sink.
    let mut nested_implicit: Vec<datamodel::Variable> = Vec::new();
    var_list.extend(implicit_dm.into_iter().map(|dm_var| {
        crate::variable::parse_var(dm_dims, &dm_var, &mut nested_implicit, units_ctx, |mi| {
            Ok(Some(mi.clone()))
        })
    }));
    debug_assert!(
        nested_implicit.is_empty(),
        "implicit vars should not produce further implicit vars"
    );

    let variables: HashMap<Ident<Canonical>, VariableStage0> = var_list
        .into_iter()
        .map(|v| (Ident::new(v.ident()), v))
        .collect();

    ModelStage0 {
        ident,
        display_name: display_name.clone(),
        variables,
        // Two declared variables whose names canonicalize identically already
        // collapsed last-wins on the canonical-keyed `variables` map above, so
        // it cannot detect the twin; the memoized groups derive from the raw
        // pre-dedup declared-ident list instead (GH #885/#891).
        errors: crate::common::duplicate_variable_errors_from_groups(
            display_name,
            model_duplicate_variables(db, model),
        ),
        implicit: is_stdlib,
        is_macro: model.macro_spec(db).is_some(),
        macro_params: crate::model::macro_param_idents(model.macro_spec(db).as_ref()),
    }
}

/// A model's lowered stage: `model_stage0` with every variable's AST lowered to
/// `Expr2` against the project's dimension context.
///
/// The lowering scope is [`model_scope_models`] -- this model plus the models it
/// can reach through module instantiation -- because `ModelStage1::new` resolves
/// a `module·output` reference's dimensions by following the module to its
/// target model's variables. Nothing outside that closure is consultable, so a
/// model's lowered stage does not depend on the rest of the project and an
/// unrelated model's edit does not invalidate it.
///
/// Three fixtures in `db::stages_tests` are what can see this scope going wrong,
/// and they separate the mistakes a narrowing can make:
///
///   - `arrayed_module_project` -- `main` reduces over an ARRAYED output of its
///     direct module target `sub_a`, so a scope holding only the model itself
///     lowers that reference as a scalar. (Before it existed, emptying this map
///     entirely left every test in the crate green.)
///   - `chain_project` -- `main -> sub_a -> sub_c`, with `main` reducing over
///     `sub_a.sub_c.out_by_region`. `sub_c` is reachable only THROUGH `sub_a`,
///     so a scope of "self + direct module targets" is caught here and nowhere
///     else. The closure must be TRANSITIVE.
///   - `omitting_stdlib_models_from_the_lowering_scope_is_inert_today` -- a
///     narrowing that drops the `stdlib⁚*` models wholesale is currently
///     harmless, and that test asserts the precise reason (no stdlib template is
///     arrayed or instantiates a module). The closure does not take that
///     shortcut -- it keeps the stdlib models a lowering can actually reach --
///     so the test now guards a road not taken rather than this code.
#[salsa::tracked(returns(ref))]
pub(crate) fn model_stage1(db: &dyn Db, model: SourceModel, project: SourceProject) -> ModelStage1 {
    #[cfg(test)]
    note_execution(&STAGE1_EXECUTIONS);

    let model_s0 = model_stage0(db, model, project);
    let models_s0 = model_scope_stage0(db, model, project);

    let scope = ScopeStage0 {
        models: &models_s0,
        dimensions: project_dimensions_context(db, project),
        model_name: model_s0.ident.as_str(),
    };
    ModelStage1::new(&scope, model_s0)
}
