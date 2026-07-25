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
//! This is where the two stages are built from salsa inputs. It is its own
//! file, rather than more of `db/query.rs`, so that a second construction site
//! has to arrive as a new file instead of hiding as a few more lines in a
//! grab-bag module -- the drift this box exists to delete. One other copy
//! survives for now: `Project::from_salsa` still builds its own stages inline,
//! and migrating it to read these queries is the follow-on commit. Where the
//! two bodies used to disagree (the stdlib `implicit` test, the stdlib
//! module-ident set, and whether duplicate-canonical-ident model errors are
//! recorded) this one takes `from_salsa`'s behaviour, so that migration is a
//! deletion rather than a merge.
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

use std::collections::HashMap;

use crate::common::{Canonical, Ident};
use crate::datamodel;
use crate::db::{
    Db, ModuleIdentContext, SourceModel, SourceProject, model_duplicate_variables,
    model_module_ident_context, parse_source_variable_with_module_context, project_datamodel_dims,
    project_dimensions_context, project_units_context,
};
use crate::model::{ModelStage0, ModelStage1, ScopeStage0, VariableStage0};

// Test-only per-thread execution counters for the two stage queries.
//
// Pointer equality of a `returns(ref)` memo does NOT prove a query body did
// not run: salsa backdates a re-executed query whose value compares equal, and
// the memo keeps its address across that backdating. Counting body entries is
// the only evidence that separates "read the memo" from "rebuilt it and found
// it equal" -- and "built at most once per revision" is exactly the GH #966
// claim.
//
// Thread-local rather than a global atomic so that a PARALLEL test run cannot
// charge one test's queries to another, and so no lock is needed. That alone
// does not isolate a measuring test: under `--test-threads=1` libtest runs
// every test on the same thread, so the counters carry over. The isolation that
// actually holds in both modes is `reset_stage_executions()` at the start of
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
}

/// How many times each stage query body has run on this thread.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StageExecutions {
    pub(crate) stage0: usize,
    pub(crate) stage1: usize,
}

/// Zero both counters (test-only).
///
/// This is what isolates a measuring test, in BOTH libtest modes: the counters
/// are per-thread, and `--test-threads=1` puts every test on one thread, so a
/// test that does not reset would see whatever earlier tests left behind. Call
/// it after building the `SimlinDb` and syncing, so a stage the fixture setup
/// happened to demand is not charged to the measured work.
#[cfg(test)]
pub(crate) fn reset_stage_executions() {
    STAGE0_EXECUTIONS.with(|c| c.set(0));
    STAGE1_EXECUTIONS.with(|c| c.set(0));
}

/// Read the current counters (test-only).
#[cfg(test)]
pub(crate) fn stage_executions() -> StageExecutions {
    StageExecutions {
        stage0: STAGE0_EXECUTIONS.with(|c| c.get()),
        stage1: STAGE1_EXECUTIONS.with(|c| c.get()),
    }
}

/// Record one entry into a stage query's body (test-only).
#[cfg(test)]
fn note_execution(counter: &'static std::thread::LocalKey<std::cell::Cell<usize>>) {
    counter.with(|c| c.set(c.get() + 1));
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
/// parse result. It is kept because it is the rule `Project::from_salsa` has
/// always used and the rule the datamodel-driven `ModelStage0::new` uses for an
/// implicit model: one rule means the two paths hit the SAME
/// `ModuleIdentContext` and share one `parse_source_variable_with_module_context`
/// cache entry, instead of minting a second set of parses under a second key.
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

/// Every project model's Stage0, keyed by canonical model name: the `models`
/// map a [`ScopeStage0`] lowering scope needs.
///
/// Both readers -- `model_stage1` below and the conveyor parameter check in
/// `db::units`, which lowers its synthetic parameter auxes in the same scope --
/// need the identical map. Building it in two places would re-introduce, one
/// level up, exactly the duplicated construction this module exists to remove.
pub(crate) fn project_models_stage0(
    db: &dyn Db,
    project: SourceProject,
) -> HashMap<Ident<Canonical>, &ModelStage0> {
    project
        .models(db)
        .values()
        .map(|src_model| {
            let s0 = model_stage0(db, *src_model, project);
            (s0.ident.clone(), s0)
        })
        .collect()
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
/// The lowering scope carries the WHOLE-PROJECT Stage0 map because
/// `ModelStage1::new` resolves a `module·output` reference's dimensions by
/// following the module to its target model's variables. Only models this one
/// instantiates can be followed, so the map is wider than it needs to be and
/// every model's lowered stage currently depends on every other model's parse
/// results; narrowing it to the module-reachable closure is the follow-on to
/// GH #966, deliberately left out of this commit so the caching change is
/// behavior-preserving on its own.
#[salsa::tracked(returns(ref))]
pub(crate) fn model_stage1(db: &dyn Db, model: SourceModel, project: SourceProject) -> ModelStage1 {
    #[cfg(test)]
    note_execution(&STAGE1_EXECUTIONS);

    let model_s0 = model_stage0(db, model, project);
    let mut models_s0 = project_models_stage0(db, project);

    // Make sure the lowering scope can see THIS model's own variables.
    //
    // Every caller today reaches this query through a `project.models(db)`
    // entry, so the map already holds the identical memo and this insert is
    // inert -- a pointer write of a value that is already there. It is here for
    // the case that is not visible from the call sites: salsa handles are
    // REUSED across syncs (`PersistentSyncState` threads the prior
    // `SourceModel`s into the next sync), so a handle can outlive its place in
    // the project's canonical-name map -- after the model is renamed, or
    // deleted while something still holds it.
    //
    // What that would cost, concretely: with no self entry,
    // `ArrayContext::get_model` returns `None`, so `get_dimensions` returns
    // `None` for every reference in the model, and every arrayed equation lowers
    // as though it were scalar. That compiles, simulates, and is wrong -- the
    // exact failure class this plan exists to delete, which is why it is
    // repaired rather than detected.
    //
    // A hard `assert!` was considered and rejected: libsimlin builds with
    // panic=abort, so an assert that ever fired would abort the user's process
    // (a WASM tab, an MCP server) over something recoverable.
    models_s0.insert(model_s0.ident.clone(), model_s0);

    let scope = ScopeStage0 {
        models: &models_s0,
        dimensions: project_dimensions_context(db, project),
        model_name: model_s0.ident.as_str(),
    };
    ModelStage1::new(&scope, model_s0)
}
