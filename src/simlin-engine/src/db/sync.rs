// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Datamodel -> salsa-input sync: the `SyncResult`/`SyncedModel`/
//! `SyncedVariable` handle maps, the `Clone`-able `PersistentSyncState`/
//! `PersistentModelState`/`PersistentVariableState` snapshots threaded
//! between sync calls, the stdlib-input builder (`build_stdlib_models`),
//! the fresh (`sync_from_datamodel`) and incremental
//! (`sync_from_datamodel_incremental`) sync entry points plus their
//! per-variable helpers (`source_variable_from_datamodel`,
//! `update_source_variable`), the macro-declaration extractor
//! (`macro_declarations_from_datamodel`), and the `maps_to`/`mappings`
//! reachability closure (`expand_maps_to_chains`) the parser uses to size a
//! variable's dimension dependency.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use super::*;

// ── Sync result ────────────────────────────────────────────────────────

/// Result of syncing a datamodel::Project into the salsa database.
/// Maps canonical names to their salsa input handles for subsequent lookups.
pub struct SyncResult {
    pub project: SourceProject,
    pub models: HashMap<String, SyncedModel>,
}

pub struct SyncedModel {
    pub source: SourceModel,
    pub variables: HashMap<String, SyncedVariable>,
    pub is_stdlib: bool,
}

pub struct SyncedVariable {
    pub source: SourceVariable,
}

// ── Persistent sync state ──────────────────────────────────────────────
//
// Owned, Clone-able snapshots of the SyncResult handles, stored between
// sync calls and reused across salsa revisions within the same database
// instance.

/// Stores salsa input handles between sync calls so that
/// `sync_from_datamodel_incremental` can reuse them instead of
/// creating fresh inputs (which would invalidate all cached queries).
#[derive(Clone)]
pub struct PersistentSyncState {
    pub project: SourceProject,
    pub models: HashMap<String, PersistentModelState>,
}

#[derive(Clone)]
pub struct PersistentModelState {
    pub source_model: SourceModel,
    pub variables: HashMap<String, PersistentVariableState>,
    /// True when this entry came from the stdlib, false for user-defined models.
    pub is_stdlib: bool,
}

impl PersistentModelState {
    /// Reconstitute a `SyncedModel` from the stored handles.
    ///
    /// Used both by `PersistentSyncState::to_sync_result` and by the fresh
    /// `sync_from_datamodel` path when splicing the cached stdlib models into
    /// the returned `SyncResult`.
    pub(crate) fn to_synced_model(&self) -> SyncedModel {
        let variables = self
            .variables
            .iter()
            .map(|(vname, pv)| {
                (
                    vname.clone(),
                    SyncedVariable {
                        source: pv.source_var,
                    },
                )
            })
            .collect();
        SyncedModel {
            source: self.source_model,
            variables,
            is_stdlib: self.is_stdlib,
        }
    }
}

#[derive(Clone)]
pub struct PersistentVariableState {
    pub source_var: SourceVariable,
}

impl PersistentSyncState {
    /// Reconstitute a `SyncResult` from the stored handles.
    pub fn to_sync_result(&self) -> SyncResult {
        SyncResult {
            project: self.project,
            models: self
                .models
                .iter()
                .map(|(name, pm)| (name.clone(), pm.to_synced_model()))
                .collect(),
        }
    }

    fn from_sync_result(sync: &SyncResult) -> Self {
        PersistentSyncState {
            project: sync.project,
            models: sync
                .models
                .iter()
                .map(|(name, sm)| {
                    let variables = sm
                        .variables
                        .iter()
                        .map(|(vname, sv)| {
                            (
                                vname.clone(),
                                PersistentVariableState {
                                    source_var: sv.source,
                                },
                            )
                        })
                        .collect();
                    (
                        name.clone(),
                        PersistentModelState {
                            source_model: sm.source,
                            variables,
                            is_stdlib: sm.is_stdlib,
                        },
                    )
                })
                .collect(),
        }
    }
}

// ── Sync function ──────────────────────────────────────────────────────

/// Build the ordered, pre-dedup macro-declaration list for
/// `SourceProject::macro_declarations`: one entry per *project*-declared
/// model (stdlib models are added later and excluded here), in datamodel
/// declaration order, carrying the model's canonical name and its
/// `macro_spec.clone()`.
///
/// Declaration order is load-bearing: `MacroRegistry::build` reports the
/// FIRST-detected duplicate macro name / macro-model collision, and the
/// canonical-name-keyed `models` map collapses the very duplicate / colliding
/// names that validation needs -- so the demand-driven `project_macro_registry`
/// query reconstructs the model list from this ordered raw data.
fn macro_declarations_from_datamodel(
    project: &datamodel::Project,
) -> Vec<(String, Option<datamodel::MacroSpec>)> {
    project
        .models
        .iter()
        .map(|m| (canonicalize(&m.name).into_owned(), m.macro_spec.clone()))
        .collect()
}

/// Build the immutable stdlib model inputs ONCE, for `SimlinDb::stdlib_models`.
///
/// Creates a `SourceModel`/`SourceVariable` salsa input set for every
/// `crate::stdlib::MODEL_NAMES` entry (SMOOTH/DELAY/TREND/systems_*), exactly
/// as the old per-sync stdlib loop did, returning the `PersistentModelState`
/// handles keyed by canonical name plus the ordered `(canonical, display)`
/// name list. Stdlib models are never macros (the
/// registry only tracks project macros; stdlib lookup goes through
/// `stdlib_descriptor`), so each `macro_spec` is `None`.
pub(crate) fn build_stdlib_models(db: &SimlinDb) -> StdlibModels {
    let mut by_canonical: HashMap<String, PersistentModelState> = HashMap::new();
    let mut ordered: Vec<(String, String)> = Vec::with_capacity(crate::stdlib::MODEL_NAMES.len());

    for stdlib_name in crate::stdlib::MODEL_NAMES {
        let full_name = format!("stdlib\u{205A}{stdlib_name}");
        let canonical = canonicalize(&full_name).into_owned();
        let dm_model = crate::stdlib::get(stdlib_name).unwrap();

        let mut variables = HashMap::new();
        let mut source_var_map = HashMap::new();
        for dm_var in &dm_model.variables {
            let canonical_var_name = canonicalize(dm_var.get_ident()).into_owned();
            let source_var = source_variable_from_datamodel(db, dm_var);
            source_var_map.insert(canonical_var_name.clone(), source_var);
            variables.insert(canonical_var_name, PersistentVariableState { source_var });
        }
        let mut variable_names: Vec<String> = source_var_map.keys().cloned().collect();
        variable_names.sort();
        let source_model = SourceModel::new(
            db,
            full_name.clone(),
            variable_names,
            source_var_map,
            dm_model.sim_specs.clone(),
            None,
        );

        by_canonical.insert(
            canonical.clone(),
            PersistentModelState {
                source_model,
                variables,
                is_stdlib: true,
            },
        );
        ordered.push((canonical, full_name));
    }

    StdlibModels {
        by_canonical,
        ordered,
    }
}

/// Populate salsa inputs from a `datamodel::Project`.
///
/// Creates `SourceProject`, `SourceModel`, and `SourceVariable` inputs in
/// the database, keyed by canonical name.
pub fn sync_from_datamodel(db: &SimlinDb, project: &datamodel::Project) -> SyncResult {
    let model_names: Vec<String> = project.models.iter().map(|m| m.name.clone()).collect();

    let mut models = HashMap::new();
    let mut source_model_map: HashMap<String, SourceModel> = HashMap::new();

    for dm_model in &project.models {
        let canonical_model_name = canonicalize(&dm_model.name).into_owned();

        let mut variables = HashMap::new();
        let mut source_var_map = HashMap::new();

        for dm_var in &dm_model.variables {
            let canonical_var_name = canonicalize(dm_var.get_ident()).into_owned();

            let source_var = source_variable_from_datamodel(db, dm_var);
            source_var_map.insert(canonical_var_name.clone(), source_var);

            variables.insert(canonical_var_name, SyncedVariable { source: source_var });
        }

        // variable_names must use canonical names to match source_var_map keys
        let mut variable_names: Vec<String> = source_var_map.keys().cloned().collect();
        variable_names.sort();

        let model_sim_specs = dm_model.sim_specs.clone();
        let source_model = SourceModel::new(
            db,
            dm_model.name.clone(),
            variable_names,
            source_var_map,
            model_sim_specs,
            dm_model.macro_spec.clone(),
        );

        source_model_map.insert(canonical_model_name.clone(), source_model);

        models.insert(
            canonical_model_name,
            SyncedModel {
                source: source_model,
                variables,
                is_stdlib: false,
            },
        );
    }

    // Splice in the db's one-shot stdlib models so incremental compilation can
    // find them when resolving implicit module references (DELAY, SMOOTH,
    // etc.). The handles are built once per db session and reused on every
    // sync, so salsa never re-creates a stdlib input (see
    // `SimlinDb::stdlib_models`). A user model whose canonical name collides
    // with a stdlib name shadows it (preserving the prior `contains_key`
    // precedence). Stdlib display names are appended after the user names, in
    // `MODEL_NAMES` order.
    let mut model_names = model_names;
    let stdlib = db.stdlib_models();
    for (canonical, full_name) in &stdlib.ordered {
        if source_model_map.contains_key(canonical) {
            continue;
        }
        let pm = &stdlib.by_canonical[canonical];
        source_model_map.insert(canonical.clone(), pm.source_model);
        models.insert(canonical.clone(), pm.to_synced_model());
        model_names.push(full_name.clone());
    }

    let source_project = SourceProject::new(
        db,
        project.name.clone(),
        project.sim_specs.clone(),
        project.dimensions.clone(),
        project.units.clone(),
        model_names,
        source_model_map,
        macro_declarations_from_datamodel(project),
        false,
        false,
    );

    SyncResult {
        project: source_project,
        models,
    }
}

fn source_variable_from_datamodel(db: &SimlinDb, var: &datamodel::Variable) -> SourceVariable {
    let ident = var.get_ident().to_string();
    let kind = SourceVariableKind::from_datamodel_variable(var);

    let equation = var
        .get_equation()
        .cloned()
        .unwrap_or_else(|| datamodel::Equation::Scalar(String::new()));

    let units = var.get_units().cloned();

    let gf = match var {
        datamodel::Variable::Flow(f) => f.gf.clone(),
        datamodel::Variable::Aux(a) => a.gf.clone(),
        _ => None,
    };

    let inflows = match var {
        datamodel::Variable::Stock(s) => s.inflows.clone(),
        _ => Vec::new(),
    };

    let outflows = match var {
        datamodel::Variable::Stock(s) => s.outflows.clone(),
        _ => Vec::new(),
    };

    let (module_refs, referenced_model_name) = match var {
        datamodel::Variable::Module(m) => (m.references.clone(), m.model_name.clone()),
        _ => (Vec::new(), String::new()),
    };

    let non_negative = match var {
        datamodel::Variable::Stock(s) => s.compat.non_negative,
        datamodel::Variable::Flow(f) => f.compat.non_negative,
        _ => false,
    };

    let can_be_module_input = var.can_be_module_input();

    let compat = match var {
        datamodel::Variable::Stock(s) => s.compat.clone(),
        datamodel::Variable::Flow(f) => f.compat.clone(),
        datamodel::Variable::Aux(a) => a.compat.clone(),
        datamodel::Variable::Module(m) => m.compat.clone(),
    };

    SourceVariable::new(
        db,
        ident,
        equation,
        kind,
        units,
        gf,
        inflows,
        outflows,
        module_refs,
        referenced_model_name,
        non_negative,
        can_be_module_input,
        compat,
    )
}

// ── Incremental sync ───────────────────────────────────────────────────

/// Update a single `SourceVariable`'s fields via salsa setters, only
/// touching fields whose values actually changed.
fn update_source_variable(
    db: &mut SimlinDb,
    source_var: SourceVariable,
    dm_var: &datamodel::Variable,
) {
    use salsa::Setter;

    let new_ident = dm_var.get_ident().to_string();
    if *source_var.ident(&*db) != new_ident {
        source_var.set_ident(db).to(new_ident);
    }

    let new_equation = dm_var
        .get_equation()
        .cloned()
        .unwrap_or_else(|| datamodel::Equation::Scalar(String::new()));
    if *source_var.equation(&*db) != new_equation {
        source_var.set_equation(db).to(new_equation);
    }

    let new_kind = SourceVariableKind::from_datamodel_variable(dm_var);
    if source_var.kind(&*db) != new_kind {
        source_var.set_kind(db).to(new_kind);
    }

    let new_units = dm_var.get_units().cloned();
    if *source_var.units(&*db) != new_units {
        source_var.set_units(db).to(new_units);
    }

    let new_gf = match dm_var {
        datamodel::Variable::Flow(f) => f.gf.clone(),
        datamodel::Variable::Aux(a) => a.gf.clone(),
        _ => None,
    };
    if *source_var.gf(&*db) != new_gf {
        source_var.set_gf(db).to(new_gf);
    }

    let new_inflows = match dm_var {
        datamodel::Variable::Stock(s) => s.inflows.clone(),
        _ => Vec::new(),
    };
    if *source_var.inflows(&*db) != new_inflows {
        source_var.set_inflows(db).to(new_inflows);
    }

    let new_outflows = match dm_var {
        datamodel::Variable::Stock(s) => s.outflows.clone(),
        _ => Vec::new(),
    };
    if *source_var.outflows(&*db) != new_outflows {
        source_var.set_outflows(db).to(new_outflows);
    }

    let (new_module_refs, new_model_name) = match dm_var {
        datamodel::Variable::Module(m) => (m.references.clone(), m.model_name.clone()),
        _ => (Vec::new(), String::new()),
    };
    if *source_var.module_refs(&*db) != new_module_refs {
        source_var.set_module_refs(db).to(new_module_refs);
    }
    if *source_var.model_name(&*db) != new_model_name {
        source_var.set_model_name(db).to(new_model_name);
    }

    let new_non_negative = match dm_var {
        datamodel::Variable::Stock(s) => s.compat.non_negative,
        datamodel::Variable::Flow(f) => f.compat.non_negative,
        _ => false,
    };
    if source_var.non_negative(&*db) != new_non_negative {
        source_var.set_non_negative(db).to(new_non_negative);
    }

    let new_can_be_module_input = dm_var.can_be_module_input();
    if source_var.can_be_module_input(&*db) != new_can_be_module_input {
        source_var
            .set_can_be_module_input(db)
            .to(new_can_be_module_input);
    }

    let new_compat = match dm_var {
        datamodel::Variable::Stock(s) => s.compat.clone(),
        datamodel::Variable::Flow(f) => f.compat.clone(),
        datamodel::Variable::Aux(a) => a.compat.clone(),
        datamodel::Variable::Module(m) => m.compat.clone(),
    };
    if *source_var.compat(&*db) != new_compat {
        source_var.set_compat(db).to(new_compat);
    }
}

/// Incrementally sync a `datamodel::Project` into an existing salsa
/// database, reusing previous input handles to preserve cached queries.
///
/// When `prev_state` is `None`, behaves like a fresh sync (creating all
/// inputs from scratch). When `Some`, reconstitutes existing handles
/// and uses salsa setters to update only changed fields, so that
/// downstream tracked functions for unchanged variables stay cached.
pub fn sync_from_datamodel_incremental(
    db: &mut SimlinDb,
    project: &datamodel::Project,
    prev_state: Option<&PersistentSyncState>,
) -> PersistentSyncState {
    use salsa::Setter;

    let prev = match prev_state {
        None => {
            let sync = sync_from_datamodel(db, project);
            return PersistentSyncState::from_sync_result(&sync);
        }
        Some(prev) => prev,
    };

    let source_project = prev.project;

    // Update SourceProject fields
    let new_name = project.name.clone();
    if *source_project.name(&*db) != new_name {
        source_project.set_name(db).to(new_name);
    }

    let new_sim_specs = project.sim_specs.clone();
    if *source_project.sim_specs(&*db) != new_sim_specs {
        source_project.set_sim_specs(db).to(new_sim_specs);
    }

    let new_dims: Vec<datamodel::Dimension> = project.dimensions.clone();
    if *source_project.dimensions(&*db) != new_dims {
        source_project.set_dimensions(db).to(new_dims);
    }

    let new_units: Vec<datamodel::Unit> = project.units.clone();
    if *source_project.units(&*db) != new_units {
        source_project.set_units(db).to(new_units);
    }

    // Re-derive the ordered, pre-dedup macro-declaration list from the
    // datamodel models (duplicates / collisions are invisible once models
    // collapse into the name-keyed map below). The demand-driven
    // `project_macro_registry` query reads this to re-derive the build error.
    let new_macro_declarations = macro_declarations_from_datamodel(project);
    if *source_project.macro_declarations(&*db) != new_macro_declarations {
        source_project
            .set_macro_declarations(db)
            .to(new_macro_declarations);
    }

    // model_names updated below after stdlib models are added

    // Process models
    let mut new_models = HashMap::new();

    for dm_model in &project.models {
        let canonical_model_name = canonicalize(&dm_model.name).into_owned();

        if let Some(prev_model) = prev.models.get(&canonical_model_name) {
            // Existing model: update via setters
            let source_model = prev_model.source_model;

            if *source_model.name(&*db) != dm_model.name {
                source_model.set_name(db).to(dm_model.name.clone());
            }

            let new_model_sim_specs = dm_model.sim_specs.clone();
            if *source_model.model_sim_specs(&*db) != new_model_sim_specs {
                source_model.set_model_sim_specs(db).to(new_model_sim_specs);
            }

            if *source_model.macro_spec(&*db) != dm_model.macro_spec {
                source_model
                    .set_macro_spec(db)
                    .to(dm_model.macro_spec.clone());
            }

            // Process variables
            let mut new_vars = HashMap::new();
            let mut source_var_map = HashMap::new();

            for dm_var in &dm_model.variables {
                let canonical_var_name = canonicalize(dm_var.get_ident()).into_owned();

                if let Some(prev_var) = prev_model.variables.get(&canonical_var_name) {
                    let source_var = prev_var.source_var;
                    update_source_variable(db, source_var, dm_var);
                    source_var_map.insert(canonical_var_name.clone(), source_var);

                    new_vars.insert(canonical_var_name, PersistentVariableState { source_var });
                } else {
                    // New variable
                    let source_var = source_variable_from_datamodel(&*db, dm_var);
                    source_var_map.insert(canonical_var_name.clone(), source_var);

                    new_vars.insert(canonical_var_name, PersistentVariableState { source_var });
                }
            }

            // variable_names must use canonical names to match source_var_map keys
            let mut variable_names: Vec<String> = source_var_map.keys().cloned().collect();
            variable_names.sort();

            // Update model's variable lists if they changed
            if *source_model.variable_names(&*db) != variable_names {
                source_model.set_variable_names(db).to(variable_names);
            }
            if *source_model.variables(&*db) != source_var_map {
                source_model.set_variables(db).to(source_var_map);
            }

            new_models.insert(
                canonical_model_name,
                PersistentModelState {
                    source_model,
                    variables: new_vars,
                    is_stdlib: false,
                },
            );
        } else {
            // New model: create fresh
            let mut new_vars = HashMap::new();
            let mut source_var_map = HashMap::new();

            for dm_var in &dm_model.variables {
                let canonical_var_name = canonicalize(dm_var.get_ident()).into_owned();
                let source_var = source_variable_from_datamodel(&*db, dm_var);
                source_var_map.insert(canonical_var_name.clone(), source_var);

                new_vars.insert(canonical_var_name, PersistentVariableState { source_var });
            }

            // variable_names must use canonical names to match source_var_map keys
            let mut variable_names: Vec<String> = source_var_map.keys().cloned().collect();
            variable_names.sort();

            let model_sim_specs = dm_model.sim_specs.clone();
            let source_model = SourceModel::new(
                &*db,
                dm_model.name.clone(),
                variable_names,
                source_var_map,
                model_sim_specs,
                dm_model.macro_spec.clone(),
            );

            new_models.insert(
                canonical_model_name,
                PersistentModelState {
                    source_model,
                    variables: new_vars,
                    is_stdlib: false,
                },
            );
        }
    }

    // Splice in the db's one-shot stdlib models. The handles were built once
    // per db session (see `SimlinDb::stdlib_models`) and are reused on every
    // sync, so salsa never re-creates a stdlib input -- a SMOOTH/DELAY
    // instantiation's compiled fragment stays cached across unrelated user
    // edits. The `Arc` is cloned to release the `&db` borrow before the
    // `&mut db` salsa setters below. A user model whose canonical name collides
    // with a stdlib name shadows it (preserving the prior `contains_key`
    // precedence).
    let stdlib = Arc::clone(db.stdlib_models());
    for (canonical, _full_name) in &stdlib.ordered {
        if new_models.contains_key(canonical) {
            continue;
        }
        // Cloning copies the stable stdlib salsa handles, NOT the underlying
        // inputs, so every synced project shares the identical stdlib inputs.
        new_models.insert(canonical.clone(), stdlib.by_canonical[canonical].clone());
    }

    // Update model_names to include stdlib. The display name is pushed for
    // every stdlib canonical now present in `new_models` (preserving the prior
    // behavior, where a user model shadowing a stdlib canonical still emits the
    // stdlib display name -- an extreme edge case kept byte-identical).
    let mut new_model_names: Vec<String> = project.models.iter().map(|m| m.name.clone()).collect();
    for (canonical, full_name) in &stdlib.ordered {
        if new_models.contains_key(canonical) {
            new_model_names.push(full_name.clone());
        }
    }
    if *source_project.model_names(&*db) != new_model_names {
        source_project.set_model_names(db).to(new_model_names);
    }

    // Update the project's models map
    let new_source_model_map: HashMap<String, SourceModel> = new_models
        .iter()
        .map(|(name, pm)| (name.clone(), pm.source_model))
        .collect();
    if *source_project.models(&*db) != new_source_model_map {
        source_project.set_models(db).to(new_source_model_map);
    }

    PersistentSyncState {
        project: source_project,
        models: new_models,
    }
}

/// Expands a set of dimension names to include all dimensions reachable
/// via `maps_to` / `mappings` in either direction.
///
/// Forward: if A maps_to B, {A} → {A, B}.
/// Reverse: if A maps_to B, {B} → {B, A}.
///
/// The reverse direction is necessary when a variable declares its own
/// dimension as e.g. DimB but its equation references DimA via a cross-
/// dimension mapping (DimA → DimB). The per-element implicit variables
/// created for the SMTH/DELAY expansion use elements of DimA, so DimA
/// must be present in the DimensionsContext for the substitution to work.
pub(crate) fn expand_maps_to_chains(
    dim_names: &BTreeSet<String>,
    all_dims: &[datamodel::Dimension],
) -> BTreeSet<String> {
    // Dimension display names (`Dimension.name`, the as-written casing) and
    // mapping targets (`maps_to()` / `mappings[].target`, which the MDL/XMILE
    // importers canonicalize to lowercase) are NOT necessarily the same string,
    // so every reachability comparison and lookup here must be on the canonical
    // form. The returned set is keyed by display name (the caller filters the
    // datamodel dims with `expanded.contains(&d.name)`), so we resolve each
    // canonical target back through `canonical_to_display` before inserting.
    let canonical_to_display: HashMap<String, String> = all_dims
        .iter()
        .map(|d| (canonicalize(&d.name).into_owned(), d.name.clone()))
        .collect();
    let dim_map: HashMap<String, &datamodel::Dimension> = all_dims
        .iter()
        .map(|d| (canonicalize(&d.name).into_owned(), d))
        .collect();

    let mut expanded = dim_names.clone();
    let mut to_visit: Vec<String> = dim_names.iter().cloned().collect();
    while let Some(name) = to_visit.pop() {
        let name_canon = canonicalize(&name).into_owned();

        // `push_target` resolves a canonical mapping target to the display name
        // the caller's `expanded.contains(&d.name)` filter expects, falling back
        // to the canonical string when the target is not itself a declared
        // dimension (a defensive case the old `==` path also tolerated).
        let push_target =
            |expanded: &mut BTreeSet<String>, to_visit: &mut Vec<String>, target_canon: &str| {
                let display = canonical_to_display
                    .get(target_canon)
                    .cloned()
                    .unwrap_or_else(|| target_canon.to_string());
                if expanded.insert(display.clone()) {
                    to_visit.push(display);
                }
            };

        // Forward: follow maps_to and mappings targets from the current dim.
        if let Some(dim) = dim_map.get(&name_canon) {
            if let Some(target) = dim.maps_to() {
                push_target(&mut expanded, &mut to_visit, &canonicalize(target));
            }
            for mapping in &dim.mappings {
                push_target(&mut expanded, &mut to_visit, &canonicalize(&mapping.target));
            }
        }
        // Reverse: find any dimension that maps_to (or has a mapping targeting)
        // our current dim. This ensures that when a variable is subscripted by
        // DimB, the DimensionsContext also contains any DimA that maps to DimB,
        // so cross-dimension subscript substitution works in builtins_visitor.
        for source_dim in all_dims {
            let maps_to_current = source_dim
                .maps_to()
                .is_some_and(|t| canonicalize(t) == name_canon)
                || source_dim
                    .mappings
                    .iter()
                    .any(|m| canonicalize(&m.target) == name_canon);
            if maps_to_current && expanded.insert(source_dim.name.clone()) {
                to_visit.push(source_dim.name.clone());
            }
        }
    }
    expanded
}
