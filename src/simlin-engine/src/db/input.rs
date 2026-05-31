// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The salsa INPUT layer: the interned key types
//! (`LtmLinkId`/`ModuleIdentContext`/`ModuleInputSet`), the variable-kind
//! tag (`SourceVariableKind`), the three `#[salsa::input]` structs
//! (`SourceProject`/`SourceModel`/`SourceVariable`) that hold the synced
//! datamodel field-by-field for fine-grained invalidation, the
//! `source_var_is_table_only` lookup-only predicate, and the
//! `datamodel_variable_from_source` re-assembly the parser consumes.

use std::collections::{BTreeSet, HashMap};

use super::*;
use crate::common::{Canonical, Ident};

// ── Interned identifiers ───────────────────────────────────────────────

/// Interned identity for a causal link between two variables.
/// Used as a key for per-link tracked functions.
#[salsa::interned(debug)]
pub struct LtmLinkId<'db> {
    #[returns(ref)]
    pub link_from: String,
    #[returns(ref)]
    pub link_to: String,
}

#[salsa::interned(debug)]
pub struct ModuleIdentContext<'db> {
    #[returns(ref)]
    pub idents: Vec<String>,
}

/// Interned identity for a module instance's input-variable wiring: the
/// sorted, canonical names of the variables a parent supplies to a sub-model
/// instance (the `isModuleInput(...)` set). Replaces the per-query
/// `Vec<String>` module-input key that salsa hashed string-by-string on every
/// lookup, and the `Option::None`/empty-`Vec` "no inputs" sentinel: because
/// salsa interning deduplicates, the empty set (`ModuleInputSet::empty`) is a
/// single id shared across all no-input callers, so the common no-inputs case
/// collapses to one cache entry per query rather than one per caller.
#[salsa::interned(debug)]
pub struct ModuleInputSet<'db> {
    #[returns(ref)]
    pub names: Vec<String>,
}

impl<'db> ModuleInputSet<'db> {
    /// The canonical no-inputs key. Because interning deduplicates, this is the
    /// same id every time, so it shares one cache entry across all callers.
    pub fn empty(db: &'db dyn Db) -> Self {
        ModuleInputSet::new(db, Vec::new())
    }

    /// Build a `ModuleInputSet` from the canonical module-input idents the
    /// dependency/assembly logic consumes. The stored `names` are the sorted
    /// canonical strings, so a round-trip back through `canonical_input_set`
    /// (or `Ident::new`, idempotent on an already-canonical string) reproduces
    /// the original `BTreeSet<Ident<Canonical>>` exactly.
    pub fn from_canonical_set(db: &'db dyn Db, inputs: &BTreeSet<Ident<Canonical>>) -> Self {
        // `BTreeSet` already iterates in sorted order, so the resulting `Vec`
        // is sorted; collecting from it preserves the canonical ordering the
        // interning key relies on for deduplication.
        let names: Vec<String> = inputs.iter().map(|id| id.as_str().to_owned()).collect();
        ModuleInputSet::new(db, names)
    }

    /// Build a `ModuleInputSet` from raw (possibly non-canonical, unsorted)
    /// module-input name strings, canonicalizing and sorting them. This is the
    /// exact inverse of `ModuleInputSet::names` for an interned set built from
    /// canonical idents (canonicalization is idempotent on canonical strings),
    /// and reproduces the old `canonical_module_input_set` derivation so the
    /// dependency classification is byte-identical.
    pub fn from_names(db: &'db dyn Db, names: &[String]) -> Self {
        let canonical = canonical_module_input_set(names);
        ModuleInputSet::from_canonical_set(db, &canonical)
    }

    /// Reconstruct the `BTreeSet<Ident<Canonical>>` the assembly/dependency
    /// logic consumes. The exact inverse of `from_canonical_set`: each stored
    /// name is already canonical, so `Ident::new` is idempotent.
    pub fn canonical_input_set(self, db: &'db dyn Db) -> BTreeSet<Ident<Canonical>> {
        self.names(db)
            .iter()
            .map(|name| Ident::<Canonical>::new(name))
            .collect()
    }
}

// ── Variable kind ──────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum SourceVariableKind {
    Stock,
    Flow,
    Aux,
    Module,
}

impl SourceVariableKind {
    pub(crate) fn from_datamodel_variable(var: &datamodel::Variable) -> Self {
        match var {
            datamodel::Variable::Stock(_) => SourceVariableKind::Stock,
            datamodel::Variable::Flow(_) => SourceVariableKind::Flow,
            datamodel::Variable::Aux(_) => SourceVariableKind::Aux,
            datamodel::Variable::Module(_) => SourceVariableKind::Module,
        }
    }
}

// ── Input types ────────────────────────────────────────────────────────

#[salsa::input]
pub struct SourceProject {
    #[returns(ref)]
    pub name: String,
    #[returns(ref)]
    pub sim_specs: datamodel::SimSpecs,
    #[returns(ref)]
    pub dimensions: Vec<datamodel::Dimension>,
    #[returns(ref)]
    pub units: Vec<datamodel::Unit>,
    #[returns(ref)]
    pub model_names: Vec<String>,
    #[returns(ref)]
    pub models: HashMap<String, SourceModel>,
    /// The ordered, pre-dedup macro-declaration list: one entry per
    /// *project*-declared model (NOT stdlib models), in datamodel
    /// declaration order, carrying the model's CANONICAL name and its
    /// `macro_spec.clone()`. This is the minimal raw data
    /// `project_macro_registry` needs to re-derive the AC5.3 duplicate /
    /// collision verdict (Passes 1-2 of `MacroRegistry::build`), which
    /// `models` -- a name-keyed `HashMap` that collapses duplicate /
    /// colliding model names -- cannot supply. Declaration order is
    /// load-bearing: the build error reports the FIRST-detected duplicate /
    /// collision, so the list must preserve the datamodel's model order.
    /// `datamodel::MacroSpec` derives `salsa::Update`, so this field type is
    /// well-formed. See `crate::db::macro_registry`.
    #[returns(ref)]
    pub macro_declarations: Vec<(String, Option<datamodel::MacroSpec>)>,
    /// Whether LTM (Loops That Matter) synthetic variable compilation is
    /// enabled. When true, `compute_layout` allocates slots and
    /// `assemble_module` compiles fragments for LTM variables.
    pub ltm_enabled: bool,
    /// When true, use discovery mode (`model_ltm_variables` with all links)
    /// which generates scores for every causal edge, not just edges in detected
    /// loops.
    pub ltm_discovery_mode: bool,
}

#[salsa::input]
pub struct SourceModel {
    #[returns(ref)]
    pub name: String,
    #[returns(ref)]
    pub variable_names: Vec<String>,
    #[returns(ref)]
    pub variables: HashMap<String, SourceVariable>,
    /// Per-model sim_specs override (None means use project-level specs)
    #[returns(ref)]
    pub model_sim_specs: Option<datamodel::SimSpecs>,
    /// `Some` iff this model is a callable macro template. On the salsa
    /// input so `project_macro_registry` is keyed on the macro-marked
    /// models (editing a non-macro variable does not invalidate it).
    #[returns(ref)]
    pub macro_spec: Option<datamodel::MacroSpec>,
}

#[salsa::input]
pub struct SourceVariable {
    #[returns(ref)]
    pub ident: String,
    #[returns(ref)]
    pub equation: datamodel::Equation,
    pub kind: SourceVariableKind,
    #[returns(ref)]
    pub units: Option<String>,
    #[returns(ref)]
    pub gf: Option<datamodel::GraphicalFunction>,
    #[returns(ref)]
    pub inflows: Vec<String>,
    #[returns(ref)]
    pub outflows: Vec<String>,
    #[returns(ref)]
    pub module_refs: Vec<datamodel::ModuleReference>,
    #[returns(ref)]
    pub model_name: String,
    pub non_negative: bool,
    pub can_be_module_input: bool,
    #[returns(ref)]
    pub compat: datamodel::Compat,
}

/// Whether a source variable is a standalone lookup-only table: a
/// graphical-function holder with an empty-or-sentinel equation and no real
/// functional input. Such a variable is a *table indexed by an explicit input*
/// (`y = table(input)`), not a value-bearing variable -- it is excluded from the
/// runlist and produces no saved series (issue #606). This is the salsa-layer
/// twin of `crate::variable::var_is_lookup_only`, evaluated over the
/// `datamodel::Equation` + `datamodel::GraphicalFunction` representation; both
/// delegate to the shared `crate::variable::is_empty_or_sentinel` core (which
/// also accepts the legacy `"0+0"` sentinel for back-compat).
///
/// Salsa-tracked so its `bool` output backdates: callers in tracked contexts
/// (`build_var_info` -> `model_dependency_graph`, `calc_flattened_offsets`)
/// must NOT gain a fine-grained dependency on a variable's equation TEXT, which
/// would invalidate the dependency graph on every unrelated equation edit.
#[salsa::tracked]
pub(crate) fn source_var_is_table_only(db: &dyn Db, var: SourceVariable) -> bool {
    use crate::variable::is_empty_or_sentinel;
    match var.equation(db) {
        // Scalar / A2A: one equation string plus a variable-level gf.
        datamodel::Equation::Scalar(s) | datamodel::Equation::ApplyToAll(_, s) => {
            var.gf(db).is_some() && is_empty_or_sentinel(s)
        }
        // Arrayed: a pure per-element table holder iff it has tables (a
        // variable-level or any per-element gf) and EVERY element equation (and
        // the EXCEPT default, if any) is empty/sentinel. The per-element gf is
        // the 4th tuple field `(subscript, equation, gf_equation, gf)`.
        datamodel::Equation::Arrayed(_, elements, default, _) => {
            let has_tables =
                var.gf(db).is_some() || elements.iter().any(|(_, _, _, gf)| gf.is_some());
            has_tables
                && !elements.is_empty()
                && elements
                    .iter()
                    .all(|(_, eq, _, _)| is_empty_or_sentinel(eq))
                && default.as_deref().map(is_empty_or_sentinel).unwrap_or(true)
        }
    }
}

/// Re-assembles the kind-tagged `datamodel::Variable` from the split salsa
/// input fields for the parser.
///
/// Builds a `datamodel::Variable` from the per-field `SourceVariable` salsa
/// input for use with the existing parsing pipeline (parse_var,
/// lower_variable). The input stores the datamodel `Equation`/`GraphicalFunction`/
/// `ModuleReference` fields directly, so this is a cheap re-assembly into the
/// kind-tagged enum the parser expects rather than a structural conversion.
///
/// The fields the salsa input does not carry -- `documentation`, `ai_state`,
/// `uid` -- are reconstructed as empty/None: parsing and lowering ignore them,
/// so their absence is semantically identical to the original datamodel value.
/// `compat.non_negative`/`can_be_module_input` are taken from the dedicated
/// scalar input fields (the canonical source for those flags after sync).
pub fn datamodel_variable_from_source(db: &dyn Db, var: SourceVariable) -> datamodel::Variable {
    let ident = var.ident(db).clone();
    let equation = var.equation(db).clone();
    let units = var.units(db).clone();
    let non_negative = var.non_negative(db);
    let can_be_module_input = var.can_be_module_input(db);
    let mut compat = var.compat(db).clone();
    compat.non_negative = non_negative;
    compat.can_be_module_input = can_be_module_input;

    match var.kind(db) {
        SourceVariableKind::Stock => datamodel::Variable::Stock(datamodel::Stock {
            ident,
            equation,
            documentation: String::new(),
            units,
            inflows: var.inflows(db).clone(),
            outflows: var.outflows(db).clone(),
            ai_state: None,
            uid: None,
            compat,
        }),
        SourceVariableKind::Flow => datamodel::Variable::Flow(datamodel::Flow {
            ident,
            equation,
            documentation: String::new(),
            units,
            gf: var.gf(db).clone(),
            ai_state: None,
            uid: None,
            compat,
        }),
        SourceVariableKind::Aux => datamodel::Variable::Aux(datamodel::Aux {
            ident,
            equation,
            documentation: String::new(),
            units,
            gf: var.gf(db).clone(),
            ai_state: None,
            uid: None,
            compat,
        }),
        SourceVariableKind::Module => datamodel::Variable::Module(datamodel::Module {
            ident,
            model_name: var.model_name(db).clone(),
            documentation: String::new(),
            units,
            references: var.module_refs(db).clone(),
            compat,
            ai_state: None,
            uid: None,
        }),
    }
}
