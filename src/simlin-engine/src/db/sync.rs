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
//! `update_source_variable`), the three extractions of what a datamodel
//! yields the inputs (`SourceProjectFields`, `SourceModelFields`,
//! `SourceVariableFields`) with the key of what a simulation reads of them
//! (`simulation_key`), the macro-declaration extractor
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
    fn to_synced_model(&self) -> SyncedModel {
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

/// Project a model's `loop_metadata` into the salsa-input `PinnedLoopSpec`
/// list, resolving each non-deleted entry's variable UIDs to canonical
/// variable names.
///
/// UIDs live only on the datamodel `Variable`s and are never synced into the
/// db, so we must resolve them here at sync time. A UID with no matching
/// variable (a stale reference after a delete, or a pin written against
/// view-element uids) is dropped from that loop's set and recorded on the
/// spec as unresolved; the LTM pin-resolution query then validates whatever
/// survives against the causal graph (an incomplete set fails the cycle
/// check and surfaces a diagnostic that names the entry's uids rather than
/// scoring a partial loop). Deleted entries are excluded entirely -- a
/// deleted pin contributes no `loop_score`.
fn pinned_loops_from_datamodel(model: &datamodel::Model) -> Vec<PinnedLoopSpec> {
    use std::collections::BTreeSet;

    if model.loop_metadata.is_empty() {
        return Vec::new();
    }

    let uid_to_name: HashMap<i32, String> = model
        .variables
        .iter()
        .filter_map(|v| {
            crate::patch::variable_uid(v).map(|uid| (uid, canonicalize(v.get_ident()).into_owned()))
        })
        .collect();

    model
        .loop_metadata
        .iter()
        .filter(|lm| !lm.deleted)
        .map(|lm| {
            // Dedup + sort: a loop's identity is its node SET, so the spec is
            // order-independent. The cycle order is recovered from the causal
            // graph in `model_pinned_loops`.
            let variables: Vec<String> = lm
                .uids
                .iter()
                .filter_map(|uid| uid_to_name.get(uid).cloned())
                .collect::<BTreeSet<String>>()
                .into_iter()
                .collect();
            let unresolved_uids: Vec<i32> = lm
                .uids
                .iter()
                .filter(|uid| !uid_to_name.contains_key(uid))
                .copied()
                .collect::<BTreeSet<i32>>()
                .into_iter()
                .collect();
            PinnedLoopSpec {
                name: lm.name.clone(),
                variables,
                uids: lm.uids.clone(),
                unresolved_uids,
                model_variables_carry_uids: !uid_to_name.is_empty(),
                description: lm.description.clone(),
            }
        })
        .collect()
}

/// Build the ordered, pre-dedup as-written variable-ident list for
/// `SourceModel::declared_variable_idents` (GH #885): one entry per datamodel
/// variable, in declaration order. This is the raw data the
/// duplicate-canonical-ident check needs, which the canonical-keyed
/// `variables` map collapses.
fn declared_variable_idents(model: &datamodel::Model) -> Vec<String> {
    model
        .variables
        .iter()
        .map(|v| v.get_ident().to_string())
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
            let source_var = source_variable_from_datamodel(db, dm_var, &canonical);
            source_var_map.insert(canonical_var_name.clone(), source_var);
            variables.insert(canonical_var_name, PersistentVariableState { source_var });
        }
        let mut variable_names: Vec<String> = source_var_map.keys().cloned().collect();
        variable_names.sort();
        // A stdlib model is no macro and carries no loop metadata, so its
        // extraction has neither.
        let fields = SourceModelFields::from_datamodel(&dm_model);
        let source_model = SourceModel::new(
            db,
            full_name.clone(),
            variable_names,
            source_var_map,
            fields.declared_variable_idents,
            fields.sim_specs,
            fields.macro_spec,
            fields.pinned_loops,
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

            let source_var = source_variable_from_datamodel(db, dm_var, &canonical_model_name);
            source_var_map.insert(canonical_var_name.clone(), source_var);

            variables.insert(canonical_var_name, SyncedVariable { source: source_var });
        }

        // variable_names must use canonical names to match source_var_map keys
        let mut variable_names: Vec<String> = source_var_map.keys().cloned().collect();
        variable_names.sort();

        let fields = SourceModelFields::from_datamodel(dm_model);
        let source_model = SourceModel::new(
            db,
            fields.name,
            variable_names,
            source_var_map,
            fields.declared_variable_idents,
            fields.sim_specs,
            fields.macro_spec,
            fields.pinned_loops,
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

    let fields = SourceProjectFields::from_datamodel(project);
    let source_project = SourceProject::new(
        db,
        fields.name,
        fields.sim_specs,
        fields.dimensions,
        fields.units,
        model_names,
        source_model_map,
        fields.macro_declarations,
        false,
    );

    SyncResult {
        project: source_project,
        models,
    }
}

/// The `SourceProject` field values a `datamodel::Project` yields, its models
/// aside (their handles are built as the sync walks them).
///
/// With [`SourceModelFields`] and [`SourceVariableFields`], the one statement
/// of which datamodel field becomes which salsa input field: the fresh sync,
/// the incremental sync and [`simulation_key`] all read these, so what the
/// compiler can read of a datamodel is listed once.
struct SourceProjectFields {
    name: String,
    sim_specs: datamodel::SimSpecs,
    dimensions: Vec<datamodel::Dimension>,
    units: Vec<datamodel::Unit>,
    macro_declarations: Vec<(String, Option<datamodel::MacroSpec>)>,
}

impl SourceProjectFields {
    fn from_datamodel(project: &datamodel::Project) -> Self {
        SourceProjectFields {
            name: project.name.clone(),
            sim_specs: project.sim_specs.clone(),
            dimensions: project.dimensions.clone(),
            units: project.units.clone(),
            macro_declarations: macro_declarations_from_datamodel(project),
        }
    }
}

/// The `SourceModel` field values a `datamodel::Model` yields, its variables
/// aside (their handles are built as the sync walks them).
struct SourceModelFields {
    name: String,
    declared_variable_idents: Vec<String>,
    sim_specs: Option<datamodel::SimSpecs>,
    macro_spec: Option<datamodel::MacroSpec>,
    pinned_loops: Vec<PinnedLoopSpec>,
}

impl SourceModelFields {
    fn from_datamodel(model: &datamodel::Model) -> Self {
        SourceModelFields {
            name: model.name.clone(),
            declared_variable_idents: declared_variable_idents(model),
            sim_specs: model.sim_specs.clone(),
            macro_spec: model.macro_spec.clone(),
            pinned_loops: pinned_loops_from_datamodel(model),
        }
    }
}

/// The `SourceVariable` field values a `datamodel::Variable` yields.
///
/// One statement of "which datamodel field becomes which salsa input field",
/// read by both the fresh-sync constructor and the incremental updater below.
/// Owned rather than borrowed because every field is stored into a salsa input:
/// the fresh path moves them in, and the incremental path compares each against
/// the stored value before setting it.
struct SourceVariableFields {
    ident: String,
    equation: datamodel::Equation,
    kind: SourceVariableKind,
    units: Option<String>,
    gf: Option<datamodel::GraphicalFunction>,
    inflows: Vec<String>,
    outflows: Vec<String>,
    repeated_inflows: Vec<String>,
    repeated_outflows: Vec<String>,
    module_refs: Vec<datamodel::ModuleReference>,
    /// A `Module` variable's referenced target model; empty for every other
    /// kind (NOT the owning model, which is `owner_model`).
    referenced_model_name: String,
    /// The canonical name of the model the variable belongs to.
    owner_model: String,
    non_negative: bool,
    can_be_module_input: bool,
    compat: datamodel::Compat,
}

impl SourceVariableFields {
    fn from_datamodel(var: &datamodel::Variable, owner_model: &str) -> Self {
        // The sets the compiler integrates, and what the lists repeat, for the
        // `RepeatedStockFlow` advisory. Every salsa reader of a stock's flows
        // reads these inputs, so this is where the set is taken for them.
        let (inflows, outflows) = match var {
            datamodel::Variable::Stock(s) => (
                datamodel::distinct_stock_flows(&s.inflows),
                datamodel::distinct_stock_flows(&s.outflows),
            ),
            _ => Default::default(),
        };
        let (module_refs, referenced_model_name) = match var {
            datamodel::Variable::Module(m) => (m.references.clone(), m.model_name.clone()),
            _ => (Vec::new(), String::new()),
        };
        SourceVariableFields {
            ident: var.get_ident().to_string(),
            equation: var
                .get_equation()
                .cloned()
                .unwrap_or_else(|| datamodel::Equation::Scalar(String::new())),
            kind: SourceVariableKind::from_datamodel_variable(var),
            units: var.get_units().cloned(),
            gf: match var {
                datamodel::Variable::Flow(f) => f.gf.clone(),
                datamodel::Variable::Aux(a) => a.gf.clone(),
                _ => None,
            },
            inflows: inflows.flows,
            outflows: outflows.flows,
            repeated_inflows: inflows.repeated,
            repeated_outflows: outflows.repeated,
            module_refs,
            referenced_model_name,
            owner_model: owner_model.to_string(),
            // Only a stock and a flow carry the non-negativity flag.
            non_negative: match var {
                datamodel::Variable::Stock(s) => s.compat.non_negative,
                datamodel::Variable::Flow(f) => f.compat.non_negative,
                _ => false,
            },
            can_be_module_input: var.can_be_module_input(),
            compat: match var {
                datamodel::Variable::Stock(s) => s.compat.clone(),
                datamodel::Variable::Flow(f) => f.compat.clone(),
                datamodel::Variable::Aux(a) => a.compat.clone(),
                datamodel::Variable::Module(m) => m.compat.clone(),
            },
        }
    }
}

fn source_variable_from_datamodel(
    db: &SimlinDb,
    var: &datamodel::Variable,
    owner_model: &str,
) -> SourceVariable {
    let f = SourceVariableFields::from_datamodel(var, owner_model);
    SourceVariable::new(
        db,
        f.ident,
        f.equation,
        f.kind,
        f.units,
        f.gf,
        f.inflows,
        f.outflows,
        f.repeated_inflows,
        f.repeated_outflows,
        f.module_refs,
        f.referenced_model_name,
        f.owner_model,
        f.non_negative,
        f.can_be_module_input,
        f.compat,
    )
}

#[cfg(test)]
#[path = "sync_flow_list_tests.rs"]
mod flow_list_tests;

// ── Simulation key ─────────────────────────────────────────────────────

/// A project as a simulation reads it, as a number: equal for two projects
/// whose compiled simulations and runs are the same, and (but for a hash
/// collision) different otherwise.
///
/// It is taken from the sync's own extractions, so it covers exactly what the
/// compiler can read: a field of the datamodel that is no salsa input
/// (a diagram, a sector, a variable's notes, provenance, the source file)
/// cannot change a compile and is in no key. Every field of the three
/// extractions is named below and either hashed or left out with the reason
/// it changes no run; an extraction that grows a field does not compile here
/// until the field is put on one side. The types the fields hold are taken
/// apart the same way, down to their numbers and strings.
///
/// Units are checked, never simulated, so a variable's units, the unit
/// definitions and the time units are left out, and an edit of them leaves a
/// run of the model fresh.
///
/// Text is hashed as written. An equation respelled, or a variable renamed in
/// case only, simulates the same and has another key: the key never calls two
/// different simulations the same, and may call the same simulation different.
/// Telling a respelling from a change takes a parse of every equation, which a
/// key taken at every revision does not pay for the run it would save.
#[cfg(feature = "agent_tools")]
pub(crate) fn simulation_key(project: &datamodel::Project) -> u64 {
    let mut key = KeyHasher::default();

    let SourceProjectFields {
        // A label.
        name: _,
        sim_specs,
        dimensions,
        // Unit definitions are read by the unit check alone.
        units: _,
        // Each model's canonical name and macro spec, hashed with the model.
        macro_declarations: _,
    } = SourceProjectFields::from_datamodel(project);
    key.sim_specs(&sim_specs);
    key.each(&dimensions, KeyHasher::dimension);

    // In declaration order: which model a name falls back to is the first
    // that is no macro.
    key.count(project.models.len());
    for model in &project.models {
        let canonical_model_name = canonicalize(&model.name).into_owned();
        let SourceModelFields {
            name,
            // The variables' spellings in declaration order, which only the
            // duplicate-name report reads; each variable's own is hashed below.
            declared_variable_idents: _,
            sim_specs,
            macro_spec,
            pinned_loops,
        } = SourceModelFields::from_datamodel(model);
        key.text(&name);
        key.optional(sim_specs.as_ref(), KeyHasher::sim_specs);
        key.optional(macro_spec.as_ref(), KeyHasher::macro_spec);
        // A pinned loop is scored in every analysis of a run, which the run
        // keeps.
        key.each(&pinned_loops, KeyHasher::pinned_loop);

        // By canonical name: the order variables are declared in changes no
        // layout and no runlist.
        let mut variables: Vec<(String, SourceVariableFields)> = model
            .variables
            .iter()
            .map(|var| {
                (
                    canonicalize(var.get_ident()).into_owned(),
                    SourceVariableFields::from_datamodel(var, &canonical_model_name),
                )
            })
            .collect();
        variables.sort_by(|a, b| a.0.cmp(&b.0));
        key.count(variables.len());
        for (_, fields) in &variables {
            let SourceVariableFields {
                ident,
                equation,
                kind,
                // Checked, never simulated.
                units: _,
                gf,
                inflows,
                outflows,
                // What the flow lists repeat, which only the advisory reads;
                // the sets the engine integrates are `inflows`/`outflows`.
                repeated_inflows: _,
                repeated_outflows: _,
                module_refs,
                referenced_model_name,
                // The model's name, hashed above.
                owner_model: _,
                non_negative,
                can_be_module_input,
                compat,
            } = fields;
            key.text(ident);
            key.equation(equation);
            key.kind(kind);
            key.optional(gf.as_ref(), KeyHasher::graphical_function);
            key.each(inflows, |key, flow| key.text(flow));
            key.each(outflows, |key, flow| key.text(flow));
            key.each(module_refs, KeyHasher::module_reference);
            key.text(referenced_model_name);
            key.flag(*non_negative);
            key.flag(*can_be_module_input);
            key.compat(compat);
        }
    }
    key.finish()
}

/// The hasher [`simulation_key`] feeds: one method per type it takes apart,
/// each naming every field of its type, so a field added to one of them does
/// not compile until it is hashed or left out here.
#[cfg(feature = "agent_tools")]
#[derive(Default)]
struct KeyHasher(std::collections::hash_map::DefaultHasher);

#[cfg(feature = "agent_tools")]
impl KeyHasher {
    fn finish(self) -> u64 {
        std::hash::Hasher::finish(&self.0)
    }

    fn text(&mut self, text: &str) {
        std::hash::Hash::hash(text, &mut self.0);
    }

    /// A number by its bits, so that a NaN is equal to itself.
    fn number(&mut self, number: f64) {
        std::hash::Hash::hash(&number.to_bits(), &mut self.0);
    }

    fn flag(&mut self, flag: bool) {
        std::hash::Hash::hash(&flag, &mut self.0);
    }

    fn count(&mut self, count: usize) {
        std::hash::Hash::hash(&count, &mut self.0);
    }

    fn optional<T: ?Sized>(&mut self, value: Option<&T>, hash: impl FnOnce(&mut Self, &T)) {
        match value {
            None => self.count(0),
            Some(value) => {
                self.count(1);
                hash(self, value);
            }
        }
    }

    fn each<T>(&mut self, items: &[T], hash: impl Fn(&mut Self, &T)) {
        self.count(items.len());
        for item in items {
            hash(self, item);
        }
    }

    fn optional_text(&mut self, text: &Option<String>) {
        self.optional(text.as_deref(), KeyHasher::text);
    }

    /// A DT or save step by its value: a reciprocal is the same step as the
    /// number it is the reciprocal of.
    fn dt(&mut self, dt: &datamodel::Dt) {
        self.number(match dt {
            datamodel::Dt::Dt(value) => *value,
            datamodel::Dt::Reciprocal(value) => 1.0 / *value,
        });
    }

    fn sim_specs(&mut self, specs: &datamodel::SimSpecs) {
        let datamodel::SimSpecs {
            start,
            stop,
            dt,
            save_step,
            sim_method,
            // Read by the unit check alone.
            time_units: _,
        } = specs;
        self.number(*start);
        self.number(*stop);
        self.dt(dt);
        self.optional(save_step.as_ref(), KeyHasher::dt);
        self.count(match sim_method {
            datamodel::SimMethod::Euler => 0,
            datamodel::SimMethod::RungeKutta2 => 1,
            datamodel::SimMethod::RungeKutta4 => 2,
        });
    }

    fn kind(&mut self, kind: &SourceVariableKind) {
        self.count(match kind {
            SourceVariableKind::Stock => 0,
            SourceVariableKind::Flow => 1,
            SourceVariableKind::Aux => 2,
            SourceVariableKind::Module => 3,
        });
    }

    fn module_reference(&mut self, reference: &datamodel::ModuleReference) {
        let datamodel::ModuleReference { src, dst } = reference;
        self.text(src);
        self.text(dst);
    }

    fn dimension(&mut self, dimension: &datamodel::Dimension) {
        let datamodel::Dimension {
            name,
            elements,
            mappings,
            parent,
        } = dimension;
        self.text(name);
        match elements {
            datamodel::DimensionElements::Indexed(size) => {
                self.count(0);
                self.count(*size as usize);
            }
            datamodel::DimensionElements::Named(names) => {
                self.count(1);
                self.each(names, |key, name| key.text(name));
            }
        }
        self.each(mappings, KeyHasher::dimension_mapping);
        self.optional_text(parent);
    }

    fn dimension_mapping(&mut self, mapping: &datamodel::DimensionMapping) {
        let datamodel::DimensionMapping {
            target,
            element_map,
        } = mapping;
        self.text(target);
        self.each(element_map, |key, (from, to)| {
            key.text(from);
            key.text(to);
        });
    }

    fn macro_spec(&mut self, spec: &datamodel::MacroSpec) {
        let datamodel::MacroSpec {
            parameters,
            primary_output,
            additional_outputs,
        } = spec;
        self.each(parameters, |key, name| key.text(name));
        self.text(primary_output);
        self.each(additional_outputs, |key, name| key.text(name));
    }

    /// A pinned loop is the variables it runs through, which every loop
    /// analysis of a run scores, under the name the analysis reports it by.
    fn pinned_loop(&mut self, pinned: &PinnedLoopSpec) {
        let PinnedLoopSpec {
            name,
            variables,
            // The uids as written, the ones that name no variable, and
            // whether any variable carries one: read only to word the
            // diagnostic for a pin that cannot be scored, which is no part
            // of a run. What the uids resolve to is `variables`.
            uids: _,
            unresolved_uids: _,
            model_variables_carry_uids: _,
            // Read by nothing the engine computes.
            description: _,
        } = pinned;
        self.text(name);
        self.each(variables, |key, name| key.text(name));
    }

    fn equation(&mut self, equation: &datamodel::Equation) {
        match equation {
            datamodel::Equation::Scalar(text) => {
                self.count(0);
                self.text(text);
            }
            datamodel::Equation::ApplyToAll(dimensions, text) => {
                self.count(1);
                self.each(dimensions, |key, name| key.text(name));
                self.text(text);
            }
            datamodel::Equation::Arrayed(dimensions, elements, default, apply_default) => {
                self.count(2);
                self.each(dimensions, |key, name| key.text(name));
                self.each(elements, |key, (element, text, initial, table)| {
                    key.text(element);
                    key.text(text);
                    key.optional_text(initial);
                    key.optional(table.as_ref(), KeyHasher::graphical_function);
                });
                self.optional_text(default);
                self.flag(*apply_default);
            }
        }
    }

    fn graphical_function(&mut self, table: &datamodel::GraphicalFunction) {
        let datamodel::GraphicalFunction {
            kind,
            x_points,
            y_points,
            x_scale,
            y_scale,
        } = table;
        self.count(match kind {
            datamodel::GraphicalFunctionKind::Continuous => 0,
            datamodel::GraphicalFunctionKind::Extrapolate => 1,
            datamodel::GraphicalFunctionKind::Discrete => 2,
        });
        self.optional(x_points.as_deref(), |key, points: &[f64]| {
            key.each(points, |key, x| key.number(*x));
        });
        self.each(y_points, |key, y| key.number(*y));
        // The x scale is where the points are when they are not written out
        // (`variable::parse_table` spreads them over it); with x points
        // written, no lookup reads it.
        if x_points.is_none() {
            self.scale(x_scale);
        }
        // The y scale is how a table is drawn; no lookup reads it.
        let _ = y_scale;
    }

    fn scale(&mut self, scale: &datamodel::GraphicalFunctionScale) {
        let datamodel::GraphicalFunctionScale { min, max } = scale;
        self.number(*min);
        self.number(*max);
    }

    fn compat(&mut self, compat: &datamodel::Compat) {
        let datamodel::Compat {
            active_initial,
            non_negative,
            can_be_module_input,
            visibility,
            data_source,
            conveyor,
            leakage,
            spreadflow,
            queue,
            overflow,
        } = compat;
        self.optional_text(active_initial);
        self.flag(*non_negative);
        self.flag(*can_be_module_input);
        self.count(match visibility {
            datamodel::Visibility::Private => 0,
            datamodel::Visibility::Public => 1,
        });
        self.optional(data_source.as_ref(), KeyHasher::data_source);
        self.optional(conveyor.as_ref(), KeyHasher::conveyor);
        self.optional(leakage.as_ref(), KeyHasher::leakage);
        self.optional(spreadflow.as_ref(), KeyHasher::spread_flow);
        self.optional(queue.as_ref(), |_, queue| {
            let datamodel::Queue {} = queue;
        });
        self.flag(*overflow);
    }

    fn data_source(&mut self, source: &datamodel::DataSource) {
        let datamodel::DataSource {
            kind,
            file,
            tab_or_delimiter,
            row_or_col,
            cell,
        } = source;
        self.count(match kind {
            datamodel::DataSourceKind::Data => 0,
            datamodel::DataSourceKind::Constants => 1,
            datamodel::DataSourceKind::Lookups => 2,
            datamodel::DataSourceKind::Subscript => 3,
        });
        self.text(file);
        self.text(tab_or_delimiter);
        self.text(row_or_col);
        self.text(cell);
    }

    fn conveyor(&mut self, conveyor: &datamodel::Conveyor) {
        let datamodel::Conveyor {
            transit_time,
            capacity,
            inflow_limit,
            sample,
            arrest,
            discrete,
            batch_integrity,
            one_at_a_time,
            exponential_leak,
            ignore_earlier_zone_losses,
        } = conveyor;
        self.text(transit_time);
        self.optional_text(capacity);
        self.optional_text(inflow_limit);
        self.optional_text(sample);
        self.optional_text(arrest);
        self.flag(*discrete);
        self.flag(*batch_integrity);
        self.flag(*one_at_a_time);
        self.flag(*exponential_leak);
        self.flag(*ignore_earlier_zone_losses);
    }

    fn leakage(&mut self, leakage: &datamodel::Leakage) {
        let datamodel::Leakage {
            fraction,
            integers,
            zone_start,
            zone_end,
        } = leakage;
        self.optional_text(fraction);
        self.flag(*integers);
        self.optional_text(zone_start);
        self.optional_text(zone_end);
    }

    fn spread_flow(&mut self, spreadflow: &datamodel::SpreadFlow) {
        match spreadflow {
            datamodel::SpreadFlow::Beginning => self.count(0),
            datamodel::SpreadFlow::Even => self.count(1),
            datamodel::SpreadFlow::Dest => self.count(2),
            datamodel::SpreadFlow::Dist(equation) => {
                self.count(3);
                self.text(equation);
            }
            datamodel::SpreadFlow::Source => self.count(4),
        }
    }
}

#[cfg(all(test, feature = "agent_tools"))]
#[path = "simulation_key_tests.rs"]
mod simulation_key_tests;

// ── Incremental sync ───────────────────────────────────────────────────

/// Update a single `SourceVariable`'s fields via salsa setters, only
/// touching fields whose values actually changed.
///
/// "Only what changed" is load-bearing, not an optimization: a salsa setter
/// bumps the input's revision whether or not the value differs, so setting
/// every field unconditionally would invalidate every query keyed on this
/// variable on every sync.
fn update_source_variable(
    db: &mut SimlinDb,
    source_var: SourceVariable,
    dm_var: &datamodel::Variable,
    owner_model: &str,
) {
    use salsa::Setter;

    let f = SourceVariableFields::from_datamodel(dm_var, owner_model);

    if *source_var.ident(&*db) != f.ident {
        source_var.set_ident(db).to(f.ident);
    }
    if *source_var.equation(&*db) != f.equation {
        source_var.set_equation(db).to(f.equation);
    }
    if source_var.kind(&*db) != f.kind {
        source_var.set_kind(db).to(f.kind);
    }
    if *source_var.units(&*db) != f.units {
        source_var.set_units(db).to(f.units);
    }
    if *source_var.gf(&*db) != f.gf {
        source_var.set_gf(db).to(f.gf);
    }
    if *source_var.inflows(&*db) != f.inflows {
        source_var.set_inflows(db).to(f.inflows);
    }
    if *source_var.outflows(&*db) != f.outflows {
        source_var.set_outflows(db).to(f.outflows);
    }
    if *source_var.repeated_inflows(&*db) != f.repeated_inflows {
        source_var.set_repeated_inflows(db).to(f.repeated_inflows);
    }
    if *source_var.repeated_outflows(&*db) != f.repeated_outflows {
        source_var.set_repeated_outflows(db).to(f.repeated_outflows);
    }
    if *source_var.module_refs(&*db) != f.module_refs {
        source_var.set_module_refs(db).to(f.module_refs);
    }
    if *source_var.model_name(&*db) != f.referenced_model_name {
        source_var.set_model_name(db).to(f.referenced_model_name);
    }
    if *source_var.owner_model(&*db) != f.owner_model {
        source_var.set_owner_model(db).to(f.owner_model);
    }
    if source_var.non_negative(&*db) != f.non_negative {
        source_var.set_non_negative(db).to(f.non_negative);
    }
    if source_var.can_be_module_input(&*db) != f.can_be_module_input {
        source_var
            .set_can_be_module_input(db)
            .to(f.can_be_module_input);
    }
    if *source_var.compat(&*db) != f.compat {
        source_var.set_compat(db).to(f.compat);
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

    // Update SourceProject fields. The macro declarations are re-derived from
    // the datamodel models (duplicates / collisions are invisible once models
    // collapse into the name-keyed map below); the demand-driven
    // `project_macro_registry` query reads them to re-derive the build error.
    let SourceProjectFields {
        name,
        sim_specs,
        dimensions,
        units,
        macro_declarations,
    } = SourceProjectFields::from_datamodel(project);
    if *source_project.name(&*db) != name {
        source_project.set_name(db).to(name);
    }
    if *source_project.sim_specs(&*db) != sim_specs {
        source_project.set_sim_specs(db).to(sim_specs);
    }
    if *source_project.dimensions(&*db) != dimensions {
        source_project.set_dimensions(db).to(dimensions);
    }
    if *source_project.units(&*db) != units {
        source_project.set_units(db).to(units);
    }
    if *source_project.macro_declarations(&*db) != macro_declarations {
        source_project
            .set_macro_declarations(db)
            .to(macro_declarations);
    }

    // model_names updated below after stdlib models are added

    // Process models
    let mut new_models = HashMap::new();

    for dm_model in &project.models {
        let canonical_model_name = canonicalize(&dm_model.name).into_owned();

        if let Some(prev_model) = prev.models.get(&canonical_model_name) {
            // Existing model: update via setters
            let source_model = prev_model.source_model;
            let SourceModelFields {
                name,
                declared_variable_idents: new_declared,
                sim_specs,
                macro_spec,
                pinned_loops,
            } = SourceModelFields::from_datamodel(dm_model);

            if *source_model.name(&*db) != name {
                source_model.set_name(db).to(name);
            }
            if *source_model.model_sim_specs(&*db) != sim_specs {
                source_model.set_model_sim_specs(db).to(sim_specs);
            }
            if *source_model.macro_spec(&*db) != macro_spec {
                source_model.set_macro_spec(db).to(macro_spec);
            }
            if *source_model.pinned_loops(&*db) != pinned_loops {
                source_model.set_pinned_loops(db).to(pinned_loops);
            }

            // Process variables
            let mut new_vars = HashMap::new();
            let mut source_var_map = HashMap::new();

            for dm_var in &dm_model.variables {
                let canonical_var_name = canonicalize(dm_var.get_ident()).into_owned();

                if let Some(prev_var) = prev_model.variables.get(&canonical_var_name) {
                    let source_var = prev_var.source_var;
                    update_source_variable(db, source_var, dm_var, &canonical_model_name);
                    source_var_map.insert(canonical_var_name.clone(), source_var);

                    new_vars.insert(canonical_var_name, PersistentVariableState { source_var });
                } else {
                    // New variable
                    let source_var =
                        source_variable_from_datamodel(&*db, dm_var, &canonical_model_name);
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
            if *source_model.declared_variable_idents(&*db) != new_declared {
                source_model
                    .set_declared_variable_idents(db)
                    .to(new_declared);
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
                let source_var =
                    source_variable_from_datamodel(&*db, dm_var, &canonical_model_name);
                source_var_map.insert(canonical_var_name.clone(), source_var);

                new_vars.insert(canonical_var_name, PersistentVariableState { source_var });
            }

            // variable_names must use canonical names to match source_var_map keys
            let mut variable_names: Vec<String> = source_var_map.keys().cloned().collect();
            variable_names.sort();

            let fields = SourceModelFields::from_datamodel(dm_model);
            let source_model = SourceModel::new(
                &*db,
                fields.name,
                variable_names,
                source_var_map,
                fields.declared_variable_idents,
                fields.sim_specs,
                fields.macro_spec,
                fields.pinned_loops,
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
