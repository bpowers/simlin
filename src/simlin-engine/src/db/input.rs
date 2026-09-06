// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The salsa INPUT layer: the interned key types
//! (`LtmLinkId`/`ModuleInputSet`), the variable-kind
//! tag (`SourceVariableKind`), the three `#[salsa::input]` structs
//! (`SourceProject`/`SourceModel`/`SourceVariable`) that hold the synced
//! datamodel field-by-field for fine-grained invalidation, the
//! `source_var_is_table_only` lookup-only predicate, and the borrowed
//! `VariableSource` view the parser consumes.

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

// ── Pinned loops ───────────────────────────────────────────────────────

/// A modeler-pinned feedback loop, identified by the *set* of variables it
/// passes through. This is the salsa-input projection of a non-deleted
/// `datamodel::LoopMetadata`: its `uids` are resolved to canonical variable
/// names at sync time (UIDs live only on the datamodel `Variable`s and are
/// never synced into the db), so the LTM queries can reconstruct the loop's
/// cycle from `model_causal_edges` alone.
///
/// Pinning lets a practitioner force a specific loop to ALWAYS be scored,
/// regardless of whether post-simulation discovery reported it -- the
/// `LOOPSCORE` capability from the LTM papers (section 10), built on the
/// existing loop-naming primitive rather than a new equation builtin.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PinnedLoopSpec {
    /// The user-supplied loop name. Preserved so callers can map the
    /// generated `pin{n}` loop id back to a human label.
    pub name: String,
    /// Canonical variable names forming the loop's variable set, sorted and
    /// deduplicated so the spec is order-independent (a loop's identity is
    /// its node set; the cycle order is recovered from the causal graph).
    pub variables: Vec<String>,
    /// The user-supplied description (empty when none was given).
    pub description: String,
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
    /// See `crate::db::macro_registry`.
    #[returns(ref)]
    pub macro_declarations: Vec<(String, Option<datamodel::MacroSpec>)>,
    /// When true, use discovery mode (`model_ltm_variables` with all links)
    /// which generates scores for every causal edge, not just edges in detected
    /// loops.
    #[returns(clone)]
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
    /// The ordered, pre-dedup variable-ident list: one entry per datamodel
    /// variable in declaration order, carrying the AS-WRITTEN ident. This is
    /// the raw data `model_duplicate_variables` needs to detect two variables
    /// whose names canonicalize to the same ident (GH #885) --
    /// `variable_names`/`variables` are canonically keyed and collapse
    /// exactly those twins, the same collapse
    /// `SourceProject::macro_declarations` exists to undo for model names.
    /// Declaration order is load-bearing: diagnostics list the colliding
    /// spellings in document order.
    #[returns(ref)]
    pub declared_variable_idents: Vec<String>,
    /// Per-model sim_specs override (None means use project-level specs)
    #[returns(ref)]
    pub model_sim_specs: Option<datamodel::SimSpecs>,
    /// `Some` iff this model is a callable macro template. On the salsa
    /// input so `project_macro_registry` is keyed on the macro-marked
    /// models (editing a non-macro variable does not invalidate it).
    #[returns(ref)]
    pub macro_spec: Option<datamodel::MacroSpec>,
    /// Modeler-pinned feedback loops, resolved from the model's non-deleted
    /// `loop_metadata` (UIDs -> canonical variable names) at sync time. The
    /// LTM pipeline reads this to always emit a `loop_score` for each pinned
    /// loop, even in discovery mode, whose report is capped and, when the
    /// enumeration cannot complete, a sample.
    #[returns(ref)]
    pub pinned_loops: Vec<PinnedLoopSpec>,
}

#[salsa::input]
pub struct SourceVariable {
    #[returns(ref)]
    pub ident: String,
    #[returns(ref)]
    pub equation: datamodel::Equation,
    #[returns(clone)]
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
    /// A `Module` variable's referenced target model; empty for every other
    /// kind. NOT the owning model, which is `owner_model`.
    #[returns(ref)]
    pub model_name: String,
    /// The canonical name of the model this variable belongs to, set at
    /// sync. Carried by name rather than as a `SourceModel` handle because a
    /// model's `variables` map is a constructor argument of the model, so the
    /// variables exist before their model does, and a salsa input field can
    /// only be set afterwards through `&mut`, which the fresh sync path does
    /// not hold. `db::variable_owner_model` resolves it to the handle.
    #[returns(ref)]
    pub owner_model: String,
    #[returns(clone)]
    pub non_negative: bool,
    #[returns(clone)]
    pub can_be_module_input: bool,
    #[returns(ref)]
    pub compat: datamodel::Compat,
}

/// [`crate::variable::is_lookup_only`] over the salsa inputs -- the one owner
/// of the rule, asked here about a `SourceVariable` (issue #606).
///
/// It reads exactly the two fields the predicate needs, not the whole
/// [`crate::variable::VariableSource`] view, so a lookup-only verdict does not
/// gain a dependency on the variable's flows, units or compat flags.
///
/// Salsa-tracked so its `bool` output backdates: callers in tracked contexts
/// (`build_var_info` -> `model_dependency_graph`, `flattened_offsets`)
/// must NOT gain a fine-grained dependency on a variable's equation TEXT, which
/// would invalidate the dependency graph on every unrelated equation edit.
#[salsa::tracked(returns(clone))]
pub(crate) fn source_var_is_table_only(db: &dyn Db, var: SourceVariable) -> bool {
    crate::variable::is_lookup_only(var.equation(db), var.gf(db).as_ref())
}

/// Is `canonical_model_name` one of the stdlib models `db::sync` splices into
/// every project?
///
/// The `stdlib⁚` prefix alone is NOT sufficient. It uses a punctuation
/// separator that ordinary model creation never produces, but an import can
/// still carry a model whose name has the prefix and a suffix naming no stdlib
/// model; flagging that model as a template would skip a user model's unit
/// check. Requiring the suffix to be a real stdlib model name keeps the flag
/// on exactly the models the stdlib splice introduced.
///
/// This is the ONE stdlib test in the engine's diagnostic path (GH #988): the
/// unit-check skip gate (`db::units`), the module-input fallback rule
/// (`db::diagnostic`) and the sub-model initials rule (`db::dep_graph`) all
/// call [`source_model_is_stdlib`] rather than carrying a looser spelling.
pub(crate) fn model_is_stdlib(canonical_model_name: &str) -> bool {
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

/// The parser's borrowed view of a variable's salsa input fields.
///
/// Every field is a borrow of the `SourceVariable`'s stored value, so a parse
/// costs no re-assembly and no deep clone of a kind-tagged
/// `datamodel::Variable`. The fields the salsa input does not carry --
/// `documentation`, `ai_state`, `uid` -- are not in the view either: parsing
/// and lowering never read them.
///
/// `compat.non_negative`/`can_be_module_input` are taken from the dedicated
/// scalar input fields (the canonical source for those flags after sync), not
/// from the stored `compat`.
///
/// One field is rewritten rather than borrowed. A conveyor stock's `<eqn>` may
/// be a §7.2 explicit init list ("100, 200, 300"), which is not a scalar
/// expression. The special build path (`conveyor_compile::expand_conveyors`)
/// parses the list and compiles the stock with a constant raw-sum placeholder;
/// mirroring that rewrite here makes the salsa DIAGNOSTIC path (which parses
/// the UN-expanded project) accept exactly the equations the runtime accepts
/// instead of flagging a valid list as a parse error. A malformed list (or a
/// non-list) is left untouched: the ordinary parse diagnostic fires, and the
/// special path adds the precise `ConveyorInitListUnsupported` rejection. The
/// ordinary COMPILE path is unaffected -- it hard-rejects any un-expanded
/// conveyor marker before using this equation.
pub fn variable_source(db: &dyn Db, var: SourceVariable) -> crate::variable::VariableSource<'_> {
    let equation = var.equation(db);
    let compat = var.compat(db);
    let equation = if var.kind(db) == SourceVariableKind::Stock && compat.conveyor.is_some() {
        match crate::conveyor_compile::explicit_init_list(var.ident(db), equation) {
            Ok(Some((_spec, placeholder))) => std::borrow::Cow::Owned(placeholder),
            _ => std::borrow::Cow::Borrowed(equation),
        }
    } else {
        std::borrow::Cow::Borrowed(equation)
    };

    crate::variable::VariableSource {
        ident: var.ident(db),
        equation,
        kind: var.kind(db),
        units: var.units(db).as_deref(),
        gf: var.gf(db).as_ref(),
        inflows: var.inflows(db),
        outflows: var.outflows(db),
        module_refs: var.module_refs(db),
        model_name: var.model_name(db),
        non_negative: var.non_negative(db),
        can_be_module_input: var.can_be_module_input(db),
        active_initial: compat.active_initial.as_deref(),
    }
}
