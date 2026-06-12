// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! LTM (Loops That Matter) variable generation.
//!
//! This is the orchestration root of the LTM subtree. `model_ltm_variables`
//! (the salsa-tracked entry point) drives loop enumeration and link-score
//! emission for a model's synthetic LTM variables; the per-concern work
//! lives in submodules:
//!
//! * `parse` -- parsing + `datamodel::Equation`-shaping helpers.
//! * `compile` -- per-equation compilation to symbolic bytecodes, the
//!   per-shape link-score equation-text query, and the compile-failure
//!   diagnostic pass.
//! * `loops` -- output-port discovery + the tiered loop builders +
//!   cross-element-through-aggregate loop recovery.
//! * `link_scores` -- the per-edge / per-shape / through-aggregate link-score
//!   emitters (lifted verbatim out of `model_ltm_variables`).

use std::collections::{HashMap, HashSet};

use crate::canonicalize;
use crate::common::{Canonical, Ident};
use crate::datamodel;
use crate::ltm::strip_subscript;

use super::{
    Db, SourceModel, SourceProject, SourceVariable, SourceVariableKind, compute_layout,
    model_causal_edges, model_implicit_var_info, project_datamodel_dims, project_units_context,
    reconstruct_single_variable,
};

mod compile;
mod link_scores;
mod loops;
mod parse;
mod pinned;

// Re-export the LTM surface other `db` submodules (and the `db.rs` root's
// `use ltm::*` / `pub use ltm::{...}` blocks) reach. The directory keeps the
// internal helpers `pub(super)` within the `ltm` subtree; only the names that
// escape it are widened here.
// `compile_ltm_var_fragment` / `link_score_equation_text_shaped` keep the `pub`
// surface the `db.rs` root re-exports with `pub use ltm::{...}`.
#[cfg(test)]
pub(crate) use compile::ForcePartialEquationErrorGuard;
pub use compile::{ShapedLinkScore, compile_ltm_var_fragment, link_score_equation_text_shaped};
pub(crate) use compile::{
    compile_ltm_implicit_var_fragment, compile_ltm_synthetic_fragment,
    model_ltm_fragment_diagnostics,
};
pub(crate) use link_scores::emit_ltm_partial_equation_warning;
#[cfg(test)]
pub(crate) use link_scores::ltm_partial_equation_warning_message;
pub(crate) use loops::build_loops_from_tiered;
// The single row/slot derivation (invariant I4 of the shape-expressiveness
// design), re-exported for `db::analysis::emit_edges_for_reference`'s
// `PerElement` arm (GH #525) so the element graph's
// diagonal-with-pinned-axes expansion and the link-score emitters derive
// rows from one function.
pub(crate) use loops::{ReadSliceRow, read_slice_rows};
// The cross-element-through-aggregate petal-stitching core, shared by the
// exhaustive recovery (`recover_cross_agg_loops`) and discovery
// (`ltm_finding`, GH #696) so both enumerate exactly the same cross-agg loops.
pub(crate) use loops::sub_model_output_ports;
pub(crate) use loops::{
    StitchPetal, collect_agg_petals, cross_agg_loop_budget, stitch_cross_agg_petals,
};
pub(crate) use parse::scalarize_ltm_equation;
pub(crate) use pinned::model_pinned_loops;

// Test-only re-exports. These names are consumed solely by the LTM test
// modules -- `ltm_tests` (mounted here, reached via `super::`) and the
// db-root-mounted `ltm_unified_tests` (reached through `db.rs`'s `use ltm::*`
// glob) -- so they would warn as unused in a non-test lib build.
#[cfg(test)]
pub(crate) use compile::LtmFragmentFailureGuard;
#[cfg(test)]
pub(crate) use compile::compile_ltm_equation_fragment;
#[cfg(test)]
pub(crate) use loops::{AggLoopBudgetGuard, MAX_CROSS_AGG_LOOPS, build_element_level_loops};

// Bare names used by `model_ltm_variables`'s body (kept call-site-identical
// after lifting the link-score emitters and moving loop/parse helpers out).
use link_scores::{
    emit_agg_to_target_link_scores, emit_link_scores_for_edge, emit_source_to_agg_link_scores,
};
pub(crate) use loops::recover_agg_hop_polarities;
use parse::parse_ltm_equation;

/// The single integration method the assembled simulation actually runs, when
/// it is NOT Euler.
///
/// LTM's 2023 flow-to-stock link-score formula
/// (`PREVIOUS(flow) - PREVIOUS(PREVIOUS(flow))`) only aligns its numerator to
/// the causal interval that drove the stock change from t-1 to t under Euler
/// integration; under RK2/RK4 the sub-stepped stock update breaks that
/// alignment and the link scores become mathematically meaningless. The VM
/// and wasm backends both genuinely honor RK2/RK4 (distinct stepping loops),
/// so the bad scores would look plausible while being wrong.
///
/// A `CompiledSimulation` has exactly ONE `Specs.method`, resolved by
/// `assemble_simulation` from the MAIN (root) model's `model_sim_specs`
/// override else the project specs. A submodel's own `model_sim_specs` is
/// never consulted by the VM, so the GH #486 guard must resolve and apply that
/// single main-governed method -- NOT each model's own specs (which is the
/// blocker the per-model resolution had). `root_model` is the model named in
/// `assemble_simulation(.., main_model_name)`. Returns `None` for Euler (the
/// supported case), `Some(method)` otherwise.
pub(super) fn effective_non_euler_method(
    db: &dyn Db,
    root_model: SourceModel,
    project: SourceProject,
) -> Option<datamodel::SimMethod> {
    let method = match root_model.model_sim_specs(db) {
        Some(specs) => specs.sim_method,
        None => project.sim_specs(db).sim_method,
    };
    match method {
        datamodel::SimMethod::Euler => None,
        other => Some(other),
    }
}

/// Whether `model_ltm_variables` emits at least one flow-to-stock link score
/// for this model -- the EXACT precondition the GH #486/#663 non-Euler guard
/// must gate on.
///
/// A flow-to-stock link score is the only LTM synthetic var the Euler-only
/// `PREVIOUS(flow) - PREVIOUS(PREVIOUS(flow))` numerator drives; under RK2/RK4
/// it is mathematically meaningless. The guard previously gated on "the model
/// has a stock", but that over-rejects a loop-free model (GH #663): in
/// exhaustive mode LTM scores only the edges of detected feedback loops, so an
/// open-loop stock (a constant inflow that never reads the stock back) emits
/// NO flow-to-stock score and nothing is corrupted.
///
/// Crucially this is mode-aware where a loop-presence proxy is NOT: in
/// DISCOVERY mode (user-forced or auto-flipped) and in any model with input
/// ports, `model_ltm_variables` scores ALL causal edges, so it DOES emit a
/// flow-to-stock score for an open-loop stock's `flow → stock` edge even
/// though that stock is in no loop. Reading the emitted var set directly is
/// therefore the only sound test: it cannot under-reject a discovery-mode
/// model with a stock the way "has any loop" would.
///
/// A causal edge into a stock can only originate from one of its flows
/// (`model_causal_edges` adds `flow → stock` edges and nothing else points at
/// a stock -- a stock's equation is its initial value, not `inflow-outflow`),
/// so "a link-score var whose `to` endpoint is a stock" is exactly "a
/// flow-to-stock score". `link_score_edge_endpoints` strips any element
/// subscript, so an arrayed stock's per-element score matches its base name.
///
/// Bounded on the guard's path: `model_ltm_variables` is the same query the
/// Euler assembly path runs unconditionally (its cost is capped by the
/// auto-flip-to-discovery gate and the circuit budget), so the guard is not
/// adding an unbounded computation. On the rejection path the guard runs BEFORE
/// `assemble_module`, so it computes the query rather than hitting a cache; but
/// it pays that at most once per instantiated stock-bearing model, and any
/// later assembly of the same model gets the salsa cache hit.
pub(super) fn model_emits_flow_to_stock_score(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> bool {
    let stocks = &crate::db::model_causal_edges(db, model, project).stocks;
    if stocks.is_empty() {
        return false;
    }
    let ltm = model_ltm_variables(db, model, project);
    ltm.vars
        .iter()
        .any(|v| link_score_edge_endpoints(&v.name).is_some_and(|(_from, to)| stocks.contains(&to)))
}

/// Human-readable name of a non-Euler integration method, used in the GH #486
/// diagnostic so the message names the offending method concretely.
fn sim_method_display_name(method: datamodel::SimMethod) -> &'static str {
    match method {
        datamodel::SimMethod::Euler => "Euler",
        datamodel::SimMethod::RungeKutta2 => "RK2 (2nd-order Runge-Kutta)",
        datamodel::SimMethod::RungeKutta4 => "RK4 (4th-order Runge-Kutta)",
    }
}

/// The GH #486 rejection message: LTM was requested on a simulation whose
/// (main-model-governed) integration method is non-Euler. Returned as the
/// `assemble_simulation` `Err` so it reaches `simlin_sim_new`,
/// `simlin_project_get_errors` (the `vm_error` channel), and the wasm backend
/// (`WasmGenError::Unsupported`) -- the sim-compile path never produces
/// silently-wrong scores.
pub(super) fn ltm_non_euler_diagnostic_message(method: datamodel::SimMethod) -> String {
    format!(
        "LTM (Loops That Matter) analysis requires Euler integration, but this model uses \
         {}.  The flow-to-stock link-score formula assumes the Euler update \
         `stock(t) = stock(t-1) + dt * flow(t-1)`, so its numerator \
         `PREVIOUS(flow) - PREVIOUS(PREVIOUS(flow))` only aligns to the causal interval that \
         drove the stock change under Euler.  Higher-order integrators sub-step the stock \
         update, so the link scores would be mathematically meaningless.  Switch the \
         integration method to Euler, or disable LTM analysis.",
        sim_method_display_name(method),
    )
}

/// THE shared stateless predicate for the LTM early-return gates (GH #748,
/// GH #749): a model is stateless when it has no parent-level stocks, no
/// PREVIOUS-lagged dt dependency, AND no (transitively) stock-carrying
/// module instance.
///
/// Both `model_ltm_mode`'s stateless ⇒ `Exhaustive` arm and
/// `model_ltm_variables`' early return consume THIS function (the latter
/// adding its `!has_input_ports` leg on top), so the two gates cannot drift
/// apart: a future state source added here is seen by both queries -- and
/// therefore by `model_detected_loops`, which reads `model_ltm_mode`. #748
/// itself was exactly such a two-site predicate change, and the
/// `test_ltm_mode_and_variables_agree_on_stateless_gate` parity guard pins
/// the agreement.
///
/// `edges.stocks` is parent-level only, so the parent-level test alone is
/// wrong for a root like `driver -> sub_model(with internal INTEG) ->
/// reader -> driver`: genuine feedback and genuine state, zero parent-level
/// stocks -- bailing on it made LTM silently emit nothing for that model
/// class (no link scores, no loop scores, and pinned loops were never even
/// validated). The module leg only runs for parent-stock-free models (the
/// `&&` short-circuits on the common stock-bearing case).
///
/// The lagged leg (GH #749): `PREVIOUS(x)` is discrete state -- a one-DT
/// memory the LTM reference (section 7) explicitly treats as state -- so a
/// stockless cycle broken by a PREVIOUS-lagged reference (`a = PREVIOUS(b);
/// b = a * k`) is genuine feedback that COMPILES (the dep-graph ordering
/// gate prunes PREVIOUS edges), falsifying the old "any stockless cycle is
/// a compile-time circular dependency" rationale. Bailing on such a model
/// made `model_ltm_variables` emit nothing (and drop pins without a
/// diagnostic) while `model_detected_loops` still enumerated the loop.
fn model_is_stateless(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    edges: &super::CausalEdgesResult,
) -> bool {
    edges.stocks.is_empty()
        && !model_has_lagged_dt_deps(db, model, project)
        && !modules_carry_state(db, project, edges)
}

/// Whether any of the model's own (parent-level) variables carries a
/// PREVIOUS-lagged dt dependency -- the lagged-state leg of
/// [`model_is_stateless`] (GH #749).
///
/// Checks `previous_only` references (`dt_previous_referenced_vars`): a
/// reference that appears both inside and outside `PREVIOUS(...)` keeps its
/// instantaneous edge, so a cycle through it could only compile if some
/// OTHER edge breaks it -- and that breaking edge is itself previous-only
/// (or a stock, which the first leg already caught). `previous_only`
/// presence is therefore exactly the conservative superset of "a
/// PREVIOUS-lagged feedback cycle can exist here". A lagged-but-acyclic
/// model runs the normal enumeration path: in exhaustive mode it scores
/// nothing (no loops, so no link or loop scores -- same output as the
/// bail), but in DISCOVERY mode it now emits link scores for every causal
/// edge where the old gate emitted none. That difference is deliberate
/// stock/lag parity: a no-feedback STOCK model never bailed either, and
/// discovery's contract is "score all edges of any state-carrying model".
///
/// Deliberately parent-level only: module-INTERNAL lagged state is not
/// counted (mirroring pin validation, which cannot see inside a module's
/// pathway either), so the scored and pinned surfaces agree on that shape
/// -- both treat a module whose only state is PREVIOUS as stateless
/// (GH #773). A parent-level lag OF a module output (`PREVIOUS(m.out)`)
/// IS counted: the previous_only entry is the parent variable's own.
///
/// Uses the same empty module-ident context / empty input set as
/// `model_causal_edges`, so the per-variable dependency queries are shared
/// salsa cache hits.
fn model_has_lagged_dt_deps(db: &dyn Db, model: SourceModel, project: SourceProject) -> bool {
    let empty_ctx = super::ModuleIdentContext::new(db, vec![]);
    let empty_inputs = super::ModuleInputSet::empty(db);
    model.variables(db).values().any(|sv| {
        !matches!(
            sv.kind(db),
            SourceVariableKind::Stock | SourceVariableKind::Module
        ) && !super::variable_direct_dependencies(db, *sv, project, empty_ctx, empty_inputs)
            .dt_previous_referenced_vars
            .is_empty()
    })
}

/// Whether any module instance this model references (transitively) carries
/// state: a stock in the instance's sub-model, or in any module that
/// sub-model references in turn. The module leg of [`model_is_stateless`] --
/// call that, not this, in gate logic.
///
/// Checking `dynamic_modules` non-emptiness instead would be over-inclusive:
/// a model whose only module is a stockless passthrough has no state
/// anywhere, so any causal cycle through it is an instantaneous algebraic
/// loop (a `CircularDependency` compile error, not feedback) and the early
/// bail -- with its "always Exhaustive, no mode flip possible" invariant --
/// remains correct for it.
///
/// The walk visits each distinct sub-MODEL definition once (not each
/// instance), guarded by a visited set so module recursion (invalid, but
/// defended against) cannot loop. `model_causal_edges` is salsa-cached, so
/// each step is a map lookup.
fn modules_carry_state(
    db: &dyn Db,
    project: SourceProject,
    edges: &super::CausalEdgesResult,
) -> bool {
    let project_models = project.models(db);
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = edges.dynamic_modules.values().cloned().collect();
    while let Some(model_name) = queue.pop() {
        let canonical = canonicalize(&model_name).into_owned();
        if !visited.insert(canonical.clone()) {
            continue;
        }
        let Some(sub_model) = project_models.get(canonical.as_str()) else {
            continue;
        };
        let sub_edges = model_causal_edges(db, *sub_model, project);
        if !sub_edges.stocks.is_empty() {
            return true;
        }
        queue.extend(sub_edges.dynamic_modules.values().cloned());
    }
    false
}

/// The model's full variable-name set, for the LTM equation parse path.
///
/// Threaded into `parse_ltm_equation` so PREVIOUS/INIT can accept a
/// non-shadowed bare element name as a static subscript index instead of
/// synthesizing a helper aux per call site (GH #654: C-LEARN's generated LTM
/// equations carry ~24k such call sites). Salsa-tracked because the LTM
/// fragment compilers parse equations once per synthetic variable -- tens of
/// thousands of times on large models -- and rebuilding a several-thousand-
/// entry set per parse would be pure waste.
///
/// Every LTM parse site MUST pass the same set: `model_ltm_implicit_var_info`
/// (which decides which helpers exist and get layout slots) and the fragment
/// compilers / `assemble_module` (which compile them) have to agree on
/// whether a given PREVIOUS argument synthesizes a helper.
#[salsa::tracked(returns(ref))]
pub(super) fn ltm_model_var_names(
    db: &dyn Db,
    model: SourceModel,
    _project: SourceProject,
) -> HashSet<Ident<Canonical>> {
    model
        .variables(db)
        .keys()
        .map(|name| Ident::new(name))
        .collect()
}

/// Salsa-tracked: the LTM fragment compilers consult this once per synthetic
/// variable (tens of thousands of times on large models), and rebuilding the
/// set from every source variable per call was a measurable fraction of LTM
/// compile time (GH #655).
#[salsa::tracked(returns(ref))]
pub(super) fn ltm_module_idents(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashSet<Ident<Canonical>> {
    let source_vars = model.variables(db);
    let mut module_idents: HashSet<Ident<Canonical>> = source_vars
        .iter()
        .filter_map(|(name, source_var)| {
            if source_var.kind(db) == SourceVariableKind::Module {
                Some(Ident::new(name))
            } else {
                None
            }
        })
        .collect();

    for (name, meta) in model_implicit_var_info(db, model, project).iter() {
        if meta.is_module {
            module_idents.insert(Ident::new(name));
        }
    }

    module_idents
}

/// The canonical cyclic rotation of a loop's **variable-level** node
/// sequence -- the dedup key for matching a pinned loop against an enumerated
/// one.
///
/// Loop links may carry element-level subscripts (`"pop[nyc]"`) on either end;
/// stripping them collapses to the variable cycle, so a pinned loop named at
/// variable granularity matches the enumerated loop over the same variables
/// regardless of which element the enumerator happened to visit.
/// `canonical_rotation` makes the comparison rotation-invariant (a loop has no
/// distinguished start), so `[a, b, c]` and `[b, c, a]` produce the same key.
fn canonical_variable_rotation(l: &crate::ltm::Loop) -> Vec<String> {
    let seq: Vec<String> = l
        .links
        .iter()
        .map(|link| strip_subscript(link.from.as_str()).to_string())
        .collect();
    crate::ltm::canonical_rotation(&seq)
}

/// Extract the `(from, to)` variable-level endpoints of a link-score var name,
/// or `None` for any other synthetic var.
///
/// A link-score var is named `$⁚ltm⁚link_score⁚{from}→{to}` where `{from}` may
/// carry a `[...]` FixedIndex subscript; both ends collapse to the
/// variable-level edge so the pin emitter can tell which `(from, to)` causal
/// edges already have a link score.
fn link_score_edge_endpoints(name: &str) -> Option<(String, String)> {
    let rest = name.strip_prefix("$\u{205A}ltm\u{205A}link_score\u{205A}")?;
    let (from, to) = rest.split_once('\u{2192}')?;
    Some((
        strip_subscript(from).to_string(),
        strip_subscript(to).to_string(),
    ))
}

/// A per-link reference override for the loop-score equation builder, keyed by
/// `(loop_id, link_index)` -> pre-quoted reference text.
///
/// PR #684: the base `input → module` link score (and `module → module`) is a
/// composite or unit-transfer fallback that picks ONE output port for the
/// whole module. A *loop* through the module enters at one input port and
/// exits at one specific output port; scoring it against the wrong port flips
/// the loop's polarity (a passthrough exposing `pos = input·0.02` and
/// `neg = -input` whose loop reads only `pos`). The composite cannot fix this
/// because it max-abs-selects across ALL of the module's pathways (here `neg`,
/// magnitude 1, beats `pos`, magnitude 0.02). So for each loop link we select
/// the exact pathway the loop traverses (entry port -> exit port) and override
/// the loop-score equation's reference to that link.
type LoopLinkOverrides = HashMap<(String, usize), String>;

/// Result of [`compute_module_link_overrides`].
struct ModuleLinkOverrides {
    /// Per-`(loop_id, link_index)` pre-quoted reference text.
    overrides: LoopLinkOverrides,
    /// The alias synthetic vars (deduped by name) the overrides reference.
    alias_vars: Vec<super::LtmSyntheticVar>,
}

/// Read the output port a (non-module) variable `y` reads off module `m`.
///
/// `y`'s equation references the module output via interpunct notation
/// `m·{port}`; return `{port}`. LTM-internal references (`m·$⁚ltm⁚…`) are
/// excluded -- the loop traverses a real model output, never a synthetic.
/// Returns the unique such port, or `None` when `y` reads zero or several
/// (a multi-output read in one variable is ambiguous and left to the base
/// link score's fallback).
fn module_exit_port_for_reader(
    module_name: &str,
    reader: &crate::variable::Variable,
) -> Option<String> {
    let ast = reader.ast()?;
    let deps = crate::variable::identifier_set(ast, &[], None);
    let prefix = format!("{module_name}\u{00B7}");
    let mut found: Option<String> = None;
    for dep in deps {
        let Some(port) = dep.as_str().strip_prefix(&prefix) else {
            continue;
        };
        // Skip the module's synthetic LTM internals (`m·$⁚ltm⁚…`).
        if port.starts_with('$') {
            continue;
        }
        if found.is_some() {
            // Two distinct output ports read by the same variable: ambiguous.
            return None;
        }
        found = Some(port.to_string());
    }
    found
}

/// The selection equation + accumulator helpers that pick the pathway with the
/// largest absolute score among `pathway_refs` (already parent-qualified +
/// quoted-bare names like `m·$⁚ltm⁚path⁚input_val⁚0`).
///
/// Mirrors [`super::generate_max_abs_selection`]'s left fold, but names its
/// accumulators so they sort into LTM evaluation category 1 (`link_score`)
/// rather than category 3 (`path`): the accumulators reference SUB-model
/// pathway vars (`m·…`, current-step once the parent evaluates module `m`) and
/// the final alias is itself a category-1 var that the loop score (category 2)
/// reads, so every accumulator MUST evaluate before the alias and before the
/// loop score. The accumulator infix `⁚viaacc⁚` sorts before the alias's
/// `⁚via⁚` terminal within category 1 (after `via`, `a` < the `⁚` separator),
/// independent of the port / variable names. This is why we do NOT reuse
/// `generate_max_abs_selection` directly here (its `⁚path⁚`-named accumulators
/// would land in category 3, after the loop score).
fn max_abs_alias_selection(
    edge_key: &str,
    exit_port: &str,
    pathway_refs: &[String],
) -> (String, Vec<super::LtmSyntheticVar>) {
    let select_step = |a: &str, b: &str| -> String {
        format!("if ABS(\"{a}\") >= ABS(\"{b}\") then \"{a}\" else \"{b}\"")
    };
    let acc_name = |i: usize| {
        format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}{edge_key}\u{205A}viaacc\u{205A}{exit_port}\u{205A}{i:06}"
        )
    };
    match pathway_refs {
        [] => ("0".to_string(), vec![]),
        [only] => (format!("\"{only}\""), vec![]),
        [p0, p1] => (select_step(p0, p1), vec![]),
        [p0, p1, rest @ .., last] => {
            let mut helpers: Vec<super::LtmSyntheticVar> = Vec::with_capacity(rest.len() + 1);
            let mut selection = select_step(p0, p1);
            for next in rest {
                let acc = acc_name(helpers.len());
                helpers.push(super::LtmSyntheticVar {
                    name: acc.clone(),
                    equation: datamodel::Equation::Scalar(selection),
                    dimensions: vec![],
                    // Like the alias, these accumulator names parse as a
                    // `(from, to)` link score, so they must compile verbatim
                    // rather than re-derive through the salsa `(from,to)` path.
                    compile_directly: true,
                });
                selection = select_step(&acc, next);
            }
            let final_acc = acc_name(helpers.len());
            helpers.push(super::LtmSyntheticVar {
                name: final_acc.clone(),
                equation: datamodel::Equation::Scalar(selection),
                dimensions: vec![],
                compile_directly: true,
            });
            (select_step(&final_acc, last), helpers)
        }
    }
}

/// Compute per-exit-port pathway-selection overrides for every loop link
/// `(x → m)` where `m` is a module and the exit port is determinable.
///
/// For each such link the parent recomputes the sub-model's pathway map
/// exactly as the sub-model's own emission does (same salsa-cached
/// `model_causal_edges` + `sub_model_output_ports`, so the pathway indices
/// match the emitted `$⁚ltm⁚path⁚{entry}⁚{idx}` vars index-for-index -- this is
/// why Part S's deterministic output-port sort must land first), then selects
/// the pathway(s) ending at the exit port the loop traverses:
///
/// * exactly one pathway -> reference `m·$⁚ltm⁚path⁚{entry}⁚{idx}` directly;
/// * several -> a max-abs selection over them (an alias + helpers);
/// * none -> `"0"` (truthful: no causal transfer entry->exit). NOTE: pathway-
///   budget truncation (GH #649, surfaced by the `pathways_truncated` Warning)
///   can also leave the matching pathway out of the recomputed map, producing
///   `"0"` here. Treat a `pathways_truncated` model's `0` as degraded, not
///   authoritative.
///
/// Each override emits ONE alias synthetic var (deduped by name across loops);
/// the loop-score equation builder consults the returned `(loop_id, link_index)`
/// map first via the threaded override.
#[allow(clippy::too_many_arguments)]
fn compute_module_link_overrides(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    loops: &[crate::ltm::Loop],
    edges_result: &super::CausalEdgesResult,
    source_vars: &HashMap<String, SourceVariable>,
) -> ModuleLinkOverrides {
    use crate::common::{Canonical, Ident};
    use crate::ltm::normalize_module_ref;

    let mut overrides: LoopLinkOverrides = HashMap::new();
    let mut alias_by_name: std::collections::BTreeMap<String, super::LtmSyntheticVar> =
        std::collections::BTreeMap::new();

    // Resolve a module variable's sub-model name and recompute its pathway map,
    // cached per-module so repeated links across loops don't re-enumerate.
    let mut pathway_cache: HashMap<String, Option<crate::ltm::ModulePathways>> = HashMap::new();
    let mut sub_model_pathways = |module_name: &str| -> Option<crate::ltm::ModulePathways> {
        if let Some(cached) = pathway_cache.get(module_name) {
            return cached.clone();
        }
        let sub_model_name = edges_result
            .dynamic_modules
            .get(module_name)
            .cloned()
            .or_else(|| {
                source_vars
                    .get(module_name)
                    .map(|sv| sv.model_name(db).to_string())
            });
        let result = sub_model_name.and_then(|name| {
            let canonical = canonicalize(&name);
            let sub_model = *project.models(db).get(canonical.as_ref())?;
            // Same salsa-cached inputs the sub-model's own emission uses, so the
            // pathway Vec order (and therefore the `{idx}` in the emitted
            // `$⁚ltm⁚path⁚{port}⁚{idx}` names) is identical index-for-index. The
            // shared `sub_model_output_ports` is what guarantees the stdlib
            // `output` convention and the user-model port scan can't skew apart.
            let output_ports = sub_model_output_ports(db, sub_model, project);
            if output_ports.is_empty() {
                return None;
            }
            let sub_edges = model_causal_edges(db, sub_model, project);
            let (pathways, _truncated) =
                super::module_input_pathways_from_edges(sub_edges, &output_ports);
            Some(pathways)
        });
        pathway_cache.insert(module_name.to_string(), result.clone());
        result
    };

    for loop_item in loops {
        let n = loop_item.links.len();
        if n == 0 {
            continue;
        }
        for i in 0..n {
            let link = &loop_item.links[i];
            let next = &loop_item.links[(i + 1) % n];
            let from = strip_subscript(link.from.as_str());
            let module_name = strip_subscript(link.to.as_str());

            // The cycle must be sequential (`link.to == next.from`) for the
            // exit port read off `next` to belong to this module hop. Loop
            // links are emitted in traversal order, so this holds, but guard
            // against a non-sequential link list rather than reading a port
            // off an unrelated edge.
            if strip_subscript(next.from.as_str()) != module_name {
                continue;
            }

            // `to` must be a module node.
            let is_module = edges_result.dynamic_modules.contains_key(module_name)
                || source_vars
                    .get(module_name)
                    .is_some_and(|sv| sv.kind(db) == SourceVariableKind::Module);
            if !is_module {
                continue;
            }

            // Entry port: the module's input whose (normalized) src equals
            // `from` -- the same match `module_link_score_equation` makes. When
            // `from` feeds MORE THAN ONE input port of the module, the collapsed
            // `from -> module` edge is genuinely ambiguous (no single entry
            // pathway to override against), so skip it and leave the base link
            // score (its composite reference) in place -- mirroring
            // `module_exit_port_for_reader`'s multi-match -> None semantics and
            // the discovery-side `recompute_module_input_edge_series` (GH #698 /
            // PR #705 r3353459409).
            let module_var = reconstruct_single_variable(db, model, project, module_name);
            let Some(crate::variable::Variable::Module { inputs, .. }) = module_var else {
                continue;
            };
            let from_ident = Ident::<Canonical>::new(from);
            let mut matching = inputs
                .iter()
                .filter(|inp| normalize_module_ref(&inp.src) == from_ident);
            let Some(entry_port) = matching.next().map(|inp| inp.dst.as_str().to_string()) else {
                continue;
            };
            if matching.next().is_some() {
                // A second input port is also fed by `from`: ambiguous entry.
                continue;
            }

            // Exit port from the next link `(m → y)`.
            let y = strip_subscript(next.to.as_str());
            let exit_port = {
                let y_is_module = edges_result.dynamic_modules.contains_key(y)
                    || source_vars
                        .get(y)
                        .is_some_and(|sv| sv.kind(db) == SourceVariableKind::Module);
                if y_is_module {
                    // `y` is a module: m's output feeds y's input port(s). y's
                    // ModuleInput src is the qualified `m·{port}`; the exit port
                    // is the `{port}` whose normalized ref is `m`. If `y` reads
                    // TWO DISTINCT output ports of `m` on different inputs, the
                    // collapsed `m -> y` edge has no unique exit port -- decline
                    // (ambiguous) and leave the base link score in place,
                    // mirroring `module_exit_port_for_reader`'s multi-match ->
                    // None semantics and the discovery-side
                    // `recompute_module_input_edge_series` (GH #698 / PR #705
                    // r3353597299). Two inputs naming the SAME `m·port` are NOT
                    // ambiguous: a unique distinct port is fine.
                    let y_var = reconstruct_single_variable(db, model, project, y);
                    let module_ident = Ident::<Canonical>::new(module_name);
                    match y_var {
                        Some(crate::variable::Variable::Module { inputs: y_in, .. }) => {
                            let mut exit: Option<String> = None;
                            let mut ambiguous = false;
                            for inp in &y_in {
                                if normalize_module_ref(&inp.src) != module_ident {
                                    continue;
                                }
                                let Some((_, port)) = inp.src.as_str().split_once('\u{00B7}')
                                else {
                                    continue;
                                };
                                match &exit {
                                    Some(prev) if prev != port => {
                                        ambiguous = true;
                                        break;
                                    }
                                    Some(_) => {}
                                    None => exit = Some(port.to_string()),
                                }
                            }
                            if ambiguous { None } else { exit }
                        }
                        _ => None,
                    }
                } else {
                    reconstruct_single_variable(db, model, project, y)
                        .and_then(|y_var| module_exit_port_for_reader(module_name, &y_var))
                }
            };
            let Some(exit_port) = exit_port else {
                continue;
            };

            // Recompute the sub-model's pathway map and select the pathway(s)
            // through `entry_port` that terminate at `exit_port`.
            let Some(pathways) = sub_model_pathways(module_name) else {
                continue;
            };
            let entry_ident = Ident::<Canonical>::new(&entry_port);
            let exit_ident = Ident::<Canonical>::new(&exit_port);
            let Some(port_pathways) = pathways.get(&entry_ident) else {
                // The sub-model exposes no pathway from this input port at all;
                // leave the base link score (its unit-transfer fallback) in
                // place rather than overriding to a wrong `0`.
                continue;
            };

            // Parent-qualified pathway references, in the sub-model's emission
            // index order, for pathways ending at `exit_port`.
            let matching_refs: Vec<String> = port_pathways
                .iter()
                .enumerate()
                .filter_map(|(idx, path_links)| {
                    let ends_at_exit = path_links.last().is_some_and(|l| l.to == exit_ident);
                    if ends_at_exit {
                        Some(format!(
                            "{module_name}\u{00B7}$\u{205A}ltm\u{205A}path\u{205A}{}\u{205A}{idx}",
                            entry_port
                        ))
                    } else {
                        None
                    }
                })
                .collect();

            // Edge key for stable alias / accumulator naming.
            let edge_key = format!("{from}\u{2192}{module_name}");
            let alias_name = format!(
                "$\u{205A}ltm\u{205A}link_score\u{205A}{edge_key}\u{205A}via\u{205A}{exit_port}"
            );

            let (alias_eqn, helpers) = if matching_refs.is_empty() {
                // No pathway connects entry to exit: no causal transfer (or a
                // truncated pathway budget -- see the fn doc). Score 0.
                ("0".to_string(), vec![])
            } else {
                max_abs_alias_selection(&edge_key, &exit_port, &matching_refs)
            };

            for h in helpers {
                alias_by_name.entry(h.name.clone()).or_insert(h);
            }
            alias_by_name
                .entry(alias_name.clone())
                .or_insert_with(|| super::LtmSyntheticVar {
                    name: alias_name.clone(),
                    equation: datamodel::Equation::Scalar(alias_eqn),
                    dimensions: vec![],
                    // Compile the prepared equation verbatim: the name parses as
                    // a `(from, to)` link score (`s→m⁚via⁚pos` => from="s",
                    // to="m⁚via⁚pos"), so without this `compile_ltm_synthetic_
                    // fragment` would route it through the salsa `(from,to)` path
                    // and re-derive a degenerate ceteris-paribus fragment that
                    // ignores the pathway-selection equation entirely.
                    compile_directly: true,
                });

            overrides.insert((loop_item.id.clone(), i), format!("\"{alias_name}\""));
        }
    }

    ModuleLinkOverrides {
        overrides,
        alias_vars: alias_by_name.into_values().collect(),
    }
}

/// Metadata about implicit variables generated by LTM equation parsing.
///
/// LTM equations may synthesize helper auxes for intrinsic PREVIOUS/INIT
/// routing and may also expand stdlib module calls such as SMOOTH/DELAY.
/// This structure collects those implicit variables across all LTM
/// equations in a model so that `compute_layout` can allocate slots and
/// `assemble_module` can compile them.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, salsa::Update)]
pub struct LtmImplicitVarMeta {
    /// Canonical name of the LTM variable that created this implicit var
    pub ltm_parent_name: String,
    /// Index into the parent's implicit_vars list
    pub index_in_parent: usize,
    /// Whether this implicit var is a stock
    pub is_stock: bool,
    /// Whether this implicit var is a module
    pub is_module: bool,
    /// Sub-model name if is_module is true
    pub model_name: Option<String>,
    /// Size in slots (for scalar vars: 1; for modules: sub-model n_slots)
    pub size: usize,
    /// The implicit variable itself, exactly as LTM equation parsing
    /// synthesized it. Carrying it here means downstream consumers
    /// (`assemble_module`'s LTM-implicit compile loop, the implicit fragment
    /// compiler, module-instance enumeration) read it directly instead of
    /// re-parsing the parent LTM equation -- which previously happened 2-3
    /// times per synthetic variable and was a measurable fraction of LTM
    /// compile time on large models (GH #655).
    pub variable: datamodel::Variable,
}

/// Cached implicit variable info for all LTM synthetic variables.
///
/// Parses each LTM equation to discover implicit helper/module variables,
/// caching the results. Both `compute_layout` and `assemble_module` read
/// this to allocate slots and compile fragments for those implicit vars
/// within LTM equations.
#[salsa::tracked(returns(ref))]
pub fn model_ltm_implicit_var_info(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashMap<String, LtmImplicitVarMeta> {
    if !project.ltm_enabled(db) {
        return HashMap::new();
    }

    let ltm_vars = model_ltm_variables(db, model, project);

    let dims = project_datamodel_dims(db, project);
    let units_ctx = project_units_context(db, project);
    let module_idents = ltm_module_idents(db, model, project);
    let model_var_names = ltm_model_var_names(db, model, project);

    let mut result = HashMap::new();

    for ltm_var in &ltm_vars.vars {
        let parsed = parse_ltm_equation(
            &ltm_var.name,
            &ltm_var.equation,
            dims,
            units_ctx,
            Some(module_idents),
            Some(model_var_names),
        );

        let project_models = project.models(db);

        for (idx, implicit_dm_var) in parsed.implicit_vars.iter().enumerate() {
            let im_name = canonicalize(implicit_dm_var.get_ident()).into_owned();
            let is_module = matches!(implicit_dm_var, datamodel::Variable::Module(_));
            let is_stock = matches!(implicit_dm_var, datamodel::Variable::Stock(_));
            let model_name = if let datamodel::Variable::Module(m) = implicit_dm_var {
                Some(m.model_name.clone())
            } else {
                None
            };
            let size = if is_module {
                model_name
                    .as_deref()
                    .and_then(|mn| {
                        let sub_canonical = canonicalize(mn);
                        project_models
                            .get(sub_canonical.as_ref())
                            .map(|sm| compute_layout(db, *sm, project).n_slots)
                    })
                    .unwrap_or(1)
            } else {
                // A non-module helper is usually a scalar aux (1 slot), but
                // an ARRAYED capture helper -- the GH #541 arrayed
                // `PREVIOUS`/`INIT` capture, extended to array-valued builtin
                // subtrees like `rank(pop, 1)` by GH #742 -- occupies
                // product(dim lengths) slots. Laid out at size 1 it would
                // overlap its successors' slots and consumers would read it
                // as scalar.
                ltm_implicit_helper_size(
                    crate::db::project_dimensions_context(db, project),
                    implicit_dm_var,
                )
            };

            result.insert(
                im_name,
                LtmImplicitVarMeta {
                    ltm_parent_name: ltm_var.name.clone(),
                    index_in_parent: idx,
                    is_stock,
                    is_module,
                    model_name,
                    size,
                    variable: implicit_dm_var.clone(),
                },
            );
        }
    }

    result
}

/// Slot count of a non-module LTM implicit helper: 1 for a scalar helper
/// aux, product(dim lengths) for an arrayed (`Equation::ApplyToAll` /
/// `Equation::Arrayed`) capture helper. An unknown dimension name degrades
/// to 1 per axis (defensive -- the helper's own fragment compile rejects it
/// loudly).
fn ltm_implicit_helper_size(
    dim_ctx: &crate::dimensions::DimensionsContext,
    var: &datamodel::Variable,
) -> usize {
    match var.get_equation() {
        Some(datamodel::Equation::ApplyToAll(dim_names, _))
        | Some(datamodel::Equation::Arrayed(dim_names, _, _, _)) => dim_names
            .iter()
            .map(|n| {
                let canonical = crate::common::CanonicalDimensionName::from_raw(n);
                dim_ctx.get(&canonical).map(|d| d.len()).unwrap_or(1)
            })
            .product::<usize>()
            .max(1),
        _ => 1,
    }
}

/// The module-typed projection of [`model_ltm_implicit_var_info`]: each
/// module-typed LTM implicit variable's canonical name mapped to its
/// sub-model name.
///
/// `compile_ltm_implicit_var_fragment` runs once per LTM implicit variable,
/// and a large arrayed model produces hundreds of thousands of those
/// (C-LEARN v77: ~145k PREVIOUS-helper auxes). Each run merges the
/// module-typed refs into its compilation context so cross-references
/// between module-typed implicit vars resolve -- but scanning the full
/// implicit-var map inside every run made LTM compilation O(K^2) in the
/// implicit-var count (tens of seconds of pure HashMap iteration on
/// C-LEARN). This query computes the projection once.
///
/// In the current architecture LTM equations are generated from
/// post-module-expansion ASTs and never contain module-function calls, so
/// this map is empty in practice; it exists so that if that ever changes,
/// the cross-reference resolution keeps working.
#[salsa::tracked(returns(ref))]
pub fn model_ltm_implicit_module_refs(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashMap<Ident<Canonical>, Ident<Canonical>> {
    let info = model_ltm_implicit_var_info(db, model, project);
    info.iter()
        .filter(|(_, meta)| meta.is_module)
        .filter_map(|(name, meta)| {
            meta.model_name
                .as_ref()
                .map(|mn| (Ident::new(name), Ident::new(mn.as_str())))
        })
        .collect()
}

/// Name -> first-occurrence-index lookup into [`model_ltm_variables`]'s
/// `vars` list, mirroring `vars.iter().find(|v| v.name == name)` semantics.
///
/// Fragment compilation resolves dependencies that may themselves be LTM
/// synthetic variables (an A2A loop score referencing link scores, a
/// composite referencing pathway scores). A linear scan over all LTM vars
/// per dependency lookup is O(N) per lookup and O(N^2) across a model's
/// full LTM compile (C-LEARN: ~145k dependency lookups over 6.7k vars), so
/// the index is built once and salsa-cached.
#[salsa::tracked(returns(ref))]
pub fn model_ltm_var_name_index(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashMap<String, usize> {
    let ltm_vars = model_ltm_variables(db, model, project);
    let mut index: HashMap<String, usize> = HashMap::with_capacity(ltm_vars.vars.len());
    for (i, v) in ltm_vars.vars.iter().enumerate() {
        // First occurrence wins, matching `.find()` on the vars list.
        index.entry(v.name.clone()).or_insert(i);
    }
    index
}

/// Resolve a model's loop-enumeration mode (exhaustive vs. discovery).
///
/// This is the single source of truth for the discovery gate, consulted by
/// both `model_ltm_variables` (which emits LTM synthetic vars + the auto-flip
/// `Warning`s) and `model_detected_loops` (the FFI loop surface). Factoring it
/// out keeps the two query surfaces from disagreeing: before this query,
/// `model_detected_loops` gated solely on the `causal_graph_with_modules` SCC
/// size and ignored both the user-requested `ltm_discovery_mode` flag and the
/// slow-path late-flip, so a small model with `ltm_discovery_mode` forced true
/// + a pinned loop would have `model_ltm_variables` emit only the pin's
/// `loop_score` while `model_detected_loops` ran full enumeration and dropped
/// the pin as a duplicate.
///
/// A model is in discovery mode when ANY of these holds (mirroring the inline
/// resolution in `model_ltm_variables`, in the same short-circuit order so the
/// expensive tiered enumeration is never run on a graph the cheap gates
/// already rejected):
///   1. the caller requested discovery (`project.ltm_discovery_mode`), or
///   2. the variable-level causal graph's largest SCC exceeds
///      `MAX_LTM_SCC_NODES` (the cheap early gate; variable-level Johnson at
///      that scale would enumerate millions of circuits), or
///   3. (only when 1 and 2 are false) a Johnson run inside the tiered
///      enumerator abandoned enumeration at `MAX_LTM_CIRCUITS`
///      (`tiered.truncated` -- a dense SCC under the node threshold can
///      still hold hundreds of millions of circuits), or
///   4. the cross-element / mixed slow-path subgraph's largest SCC exceeds
///      `MAX_LTM_SCC_NODES` (the late flip).
///
/// A stateless model -- the shared `model_is_stateless` predicate: no
/// parent-level stocks, no PREVIOUS-lagged dt dependency, AND no
/// module-internal state -- has no feedback loops, so no flip can occur: it
/// is always `Exhaustive`. `model_ltm_variables`' stateless early return
/// consumes the SAME predicate (plus its `!has_input_ports` leg) and reads
/// its bail `mode` from this query, so the two gates cannot drift. A
/// parent-stock-free model whose state lives inside modules is NOT
/// stateless (GH #748), and neither is one whose only state is a
/// PREVIOUS-lagged reference (GH #749 -- a one-DT memory is discrete
/// state): both fall through to the normal gates like any stock-bearing
/// model.
/// This query intentionally accumulates NO diagnostics -- the
/// auto-flip `Warning`s stay in `model_ltm_variables` so they are emitted once
/// (a `returns(ref)` tracked query read by multiple callers would otherwise
/// double-accumulate). `model_ltm_variables` sets its returned `mode` from
/// this query, so the two never drift.
#[salsa::tracked]
pub fn model_ltm_mode(db: &dyn Db, model: SourceModel, project: SourceProject) -> super::LtmMode {
    use super::{LtmMode, causal_graph_from_edges, model_causal_edges, model_loop_circuits_tiered};

    let edges_result = model_causal_edges(db, model, project);
    if model_is_stateless(db, model, project, edges_result) {
        return LtmMode::Exhaustive;
    }

    if project.ltm_discovery_mode(db) {
        return LtmMode::Discovery;
    }

    // Cheap early gate: the variable-level SCC. Running variable-level Johnson
    // (inside `model_loop_circuits_tiered`) on an SCC this large is exactly the
    // explosion the gate avoids, so it must be checked BEFORE the slow-path
    // tier is ever consulted.
    let var_scc_size = causal_graph_from_edges(edges_result).largest_scc_size();
    if var_scc_size > crate::ltm::MAX_LTM_SCC_NODES {
        return LtmMode::Discovery;
    }

    // Late flips: the variable-level SCC cleared the early gate, but either
    // (a) a Johnson run blew the circuit budget (`truncated` -- a dense SCC
    // defeats the node-count gate, since circuit count is super-exponential
    // in density rather than node count), or (b) the cross-element / mixed
    // slow-path subgraph blew past the SCC threshold (which the tiered
    // enumerator exposes without having run Johnson on it).
    let tiered = model_loop_circuits_tiered(db, model, project);
    if tiered.truncated || tiered.slow_path_largest_scc > crate::ltm::MAX_LTM_SCC_NODES {
        LtmMode::Discovery
    } else {
        LtmMode::Exhaustive
    }
}

/// Unified LTM variable generation for any model (root or sub-model).
///
/// Auto-detects sub-model behavior by checking for input ports with causal
/// pathways to output. Sub-models and discovery mode generate link scores
/// for ALL edges; exhaustive mode on root models generates link scores only
/// for edges in detected loops, plus loop/relative loop scores.
///
/// Pathway and composite scores are generated for models with input ports.
/// Module-containing loops are no longer filtered out because
/// `link_score_equation_text` now handles module links via composite refs.
#[salsa::tracked(returns(ref))]
pub fn model_ltm_variables(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> super::LtmVariablesResult {
    use crate::common::Ident;
    use crate::ltm::{CyclePartitions, Loop};
    use salsa::Accumulator;
    use std::collections::HashSet;

    use super::{
        CompilationDiagnostic, Diagnostic, DiagnosticError, DiagnosticSeverity, LtmSyntheticVar,
        LtmVariablesResult, causal_graph_from_edges, causal_graph_with_modules,
        generate_max_abs_selection, model_causal_edges, model_element_cycle_partitions,
        model_loop_circuits_tiered, model_pinned_loops, module_input_pathways_from_edges,
    };

    use super::LtmMode;

    let edges_result = model_causal_edges(db, model, project);

    // Determine output ports for this model and the internal input->output
    // pathways through them. Stdlib models always use the "output"
    // convention; for user-defined models the output ports come from which
    // internal variables are read from parent models via module·var syntax.
    //
    // This is computed BEFORE the stock-free early return because a
    // *passthrough* sub-model (no internal stocks) still has parent-visible
    // input->output pathways the parent's per-exit-port link score needs
    // scored (PR #684): its internals are a pure aux chain LTM scores
    // exactly, so it must emit pathway/composite vars even though it has no
    // stocks. A stateless ROOT model has no parent reading `module·var`, so
    // `sub_model_output_ports` is empty and `has_input_ports` is false -- the
    // early return below still fires for it. "Stateless" means no
    // parent-level stocks AND no module-internal state (the shared
    // `model_is_stateless` predicate, also consumed by `model_ltm_mode`): a
    // parent-stock-free root whose only state lives inside modules (a
    // SMOOTH/DELAY instance or a stock-carrying user sub-model) has genuine
    // feedback, so the predicate's module leg lets it through (GH #748).
    let output_ports = sub_model_output_ports(db, model, project);
    let (pathways, truncated_pathway_ports) = if output_ports.is_empty() {
        (HashMap::new(), Vec::new())
    } else {
        module_input_pathways_from_edges(edges_result, &output_ports)
    };
    let has_input_ports = !pathways.is_empty();

    if !has_input_ports && model_is_stateless(db, model, project, edges_result) {
        // A stateless model (no stocks anywhere -- parent-level or
        // module-internal -- and no PREVIOUS-lagged dependency, GH #749)
        // with no input-port pathways has nothing to score: no feedback
        // loops to enumerate (any causal cycle would be an instantaneous
        // algebraic loop, which is a compile error -- so no mode flip can
        // occur) and no module interface to expose.
        //
        // `mode` is read from `model_ltm_mode` rather than hardcoded: its
        // stateless arm fires on the SAME `model_is_stateless` predicate, so
        // by construction this reads `Exhaustive` -- but reading the shared
        // query keeps this surface and `model_detected_loops` (another
        // `model_ltm_mode` reader) structurally incapable of drifting, same
        // as the non-bail `mode` read at the bottom of this function.
        return LtmVariablesResult {
            vars: vec![],
            loop_partitions: indexmap::IndexMap::new(),
            agg_recovery_truncated: false,
            pathways_truncated: false,
            mode: model_ltm_mode(db, model, project),
        };
    }

    // GH #486's non-Euler rejection is NOT emitted here. The integration
    // method that the VM actually honors is a single, main-model-governed
    // property of the assembled simulation (`assemble_simulation`'s `Specs`
    // selection: the root model's `model_sim_specs` override else the project
    // specs -- a submodel's own override is dead), and this per-model query has
    // no main-model context. Emitting per-model with each model's own specs is
    // both wrong (a stock-free main that overrides to RK4 would be missed while
    // its stock-bearing submodel falls back to the project's Euler; conversely
    // a submodel that overrides to RK4 under a Euler main would be wrongly
    // rejected) and duplicative. The check lives once in `assemble_simulation`
    // against the resolved method; it surfaces through `compile_project_
    // incremental` to `simlin_sim_new`, `simlin_project_get_errors` (the
    // `vm_error` channel), and the wasm backend.

    // When the user explicitly requested discovery mode, honor it
    // directly. Otherwise auto-flip if either:
    //   1. The variable-level SCC exceeds `MAX_LTM_SCC_NODES`, or
    //   2. The slow-path (cross-element / mixed) element-level
    //      subgraph's SCC exceeds `MAX_LTM_SCC_NODES` after the tiered
    //      enumerator classifies cycles.
    //
    // Gating on the *variable-level* SCC (instead of the full
    // element-graph SCC) is the structural change that motivates this
    // PR: pure-A2A models with tens of variables over hundreds of
    // elements have a huge element-graph SCC but a small variable
    // SCC, and the new tiered enumerator produces no element-level
    // circuits for them. Today's gate fires anyway on the inflated
    // element-graph SCC, dropping these models into discovery mode
    // unnecessarily.
    //
    // The variable-level SCC check (Tarjan, O(V+E)) runs first so
    // models like WRLD3 (var SCC = 166) auto-flip without paying for
    // variable-level Johnson. Only models that pass that check pay
    // for the tiered enumerator, which is itself bounded: variable
    // Johnson sees at most `MAX_LTM_SCC_NODES` nodes, and slow-path
    // Johnson is skipped internally when the slow-path subgraph SCC
    // exceeds the threshold.
    //
    // Cliff A (~17 GB in legacy `build_element_level_loops` on
    // WRLD3-scale models) was tied to enumerating the full element
    // graph; the tiered path skips that entirely on pure-scalar /
    // pure-A2A models. Cliff B (rel_loop_score equation text) was
    // retired earlier when post-simulation normalization moved out of
    // the VM. See `docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md`
    // and `docs/design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md`.
    let is_discovery_user = project.ltm_discovery_mode(db);
    let var_scc_size = if is_discovery_user {
        0
    } else {
        let edges = model_causal_edges(db, model, project);
        causal_graph_from_edges(edges).largest_scc_size()
    };
    let var_auto_flipped = !is_discovery_user && var_scc_size > crate::ltm::MAX_LTM_SCC_NODES;

    if var_auto_flipped {
        let msg = format!(
            "LTM analysis auto-switched from exhaustive to discovery mode: \
             the variable-level causal graph's largest SCC has {} nodes, \
             exceeding MAX_LTM_SCC_NODES = {}.  Variable-level Johnson at \
             this scale would enumerate millions of circuits (see \
             docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md and \
             docs/design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md). \
             Per-loop scores are ranked post-simulation via the \
             strongest-path search; see \
             docs/design/ltm--loops-that-matter.md for the two-tier \
             strategy.",
            var_scc_size,
            crate::ltm::MAX_LTM_SCC_NODES,
        );
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: None,
            error: DiagnosticError::Assembly(msg),
            severity: DiagnosticSeverity::Warning,
        })
        .accumulate(db);
    }
    let mut is_discovery = is_discovery_user || var_auto_flipped;

    // `output_ports`, `pathways`, and `has_input_ports` were computed before
    // the stock-free early return above (a passthrough sub-model needs them).
    // GH #649: a module body shaped as a chain of diamonds has exponentially
    // many short internal pathways, each minting one `$⁚ltm⁚path⁚…` synthetic
    // variable. The enumerator caps that count per input port; when it does,
    // surface a `Warning` naming the module + clipped input port(s) (the human
    // channel) and ride the robust `pathways_truncated` flag out on the result.
    // The composite link score for a clipped port is then the max over the kept
    // pathway prefix only -- degraded, consistent with the macro composite's
    // heuristic nature, but never a panic or a silent zero.
    let pathways_truncated = !truncated_pathway_ports.is_empty();
    if pathways_truncated {
        let ports = truncated_pathway_ports
            .iter()
            .map(|p| p.as_str().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let msg = format!(
            "LTM module-pathway enumeration was truncated at the per-port budget \
             ({}): module \"{}\" has more internal input->output pathways than the \
             budget through input port(s) {}, so their composite link scores are \
             computed over a deterministic pathway prefix and may be degraded.",
            crate::ltm::module_pathway_budget(),
            model.name(db).as_str(),
            ports,
        );
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: None,
            error: DiagnosticError::Assembly(msg),
            severity: DiagnosticSeverity::Warning,
        })
        .accumulate(db);
    }

    let mut vars = Vec::new();

    // Fetch source variables and dimension metadata early -- needed by
    // both loop pre-computation (for dimension lookups) and link score
    // classification.
    let source_vars = model.variables(db);
    let dm_dims = project_datamodel_dims(db, project);

    // Phase 5: emit the synthetic aggregate-node auxiliaries
    // (`$⁚ltm⁚agg⁚{n}`). Each is a plain computed aux whose equation is one
    // maximal inlined reducer subexpression; the simulation evaluates it
    // each timestep (so `PREVIOUS(agg)` is available) and the two
    // link-score halves (`source[d] → agg`, `agg → target`) reference it.
    // A whole-RHS reducer (`total_population = SUM(population[*])`) mints
    // *no* synthetic -- the variable already is the aggregate node. The agg
    // vars are pushed before any link-score var so they sort first (the LTM
    // flow fragments are appended to the runlist in `vars` order, and the
    // `agg → target` link score reads the agg's current-step value -- so the
    // agg fragment must execute first in the same timestep). The category
    // function in the final `vars.sort_by` keeps them ahead of link scores.
    let agg_nodes = crate::ltm_agg::enumerate_agg_nodes(db, model, project);
    for agg in &agg_nodes.aggs {
        if !agg.is_synthetic {
            continue;
        }
        // The equation text is the canonical reducer subexpression. A
        // whole-extent or pinned-slice reducer (`SUM(pop[*])`,
        // `SUM(pop[NYC,*])`) has a scalar result; a partial-reduce slice over
        // an iterated dimension (`SUM(matrix[D1,*])` inside an A2A-over-`D1`
        // body) has `result_dims = [D1]` -- in an A2A-over-`D1` body
        // `matrix[D1,*]` is exactly "the `D1`-th row, all of axis 2", so this
        // evaluates correctly as the `Equation::ApplyToAll` body.
        let equation = if agg.result_dims.is_empty() {
            datamodel::Equation::Scalar(agg.equation_text.clone())
        } else {
            datamodel::Equation::ApplyToAll(agg.result_dims.clone(), agg.equation_text.clone())
        };
        vars.push(LtmSyntheticVar {
            name: agg.name.clone(),
            equation,
            dimensions: agg.result_dims.clone(),
            compile_directly: false,
        });
    }

    // Pre-compute loops for exhaustive mode using tiered loop
    // enumeration: variable-level Johnson runs first, and only the
    // cross-element / mixed slice descends into element-level Johnson
    // on the slow-path subgraph. Pure-A2A and pure-scalar cycles emit
    // a single Loop directly (one per variable-level cycle, with
    // `dimensions` set on the A2A case).
    //
    // See `docs/design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md`
    // for the architecture; the legacy element-graph Johnson path is
    // still available via `model_element_loop_circuits` for callers
    // outside this function.
    //
    // We also need the variable-level graph for polarity analysis
    // (`build_loops_from_tiered` calls `var_graph.calculate_polarity`
    // and `var_graph.find_stocks_in_loop`); the element-level graph
    // produced by `model_element_causal_edges` carries no variable
    // data.
    //
    // `agg_recovery_truncated` rides on the result so a model author
    // knows the cross-agg loop list is incomplete even if the `Warning`
    // never reaches them (#466); set inside the loop-building branch below.
    let mut agg_recovery_truncated = false;
    let loops: Option<Vec<Loop>> = if !is_discovery {
        let tiered = model_loop_circuits_tiered(db, model, project);
        // Late-stage auto-flip, case 1: a Johnson run inside the tiered
        // enumerator blew the circuit budget. A dense SCC well under the
        // node-count gates can still hold hundreds of millions of
        // elementary circuits (count is super-exponential in density);
        // uncapped enumeration OOMs before any SCC gate fires. The tiered
        // result carries empty (unreliable) circuit lists in that case --
        // it MUST be checked before the empty-fast-and-slow-path branch
        // below, which would otherwise read truncation as "no loops" and
        // return an exhaustive nothing-to-score result, silently
        // disagreeing with `model_ltm_mode`.
        if tiered.truncated {
            let msg = format!(
                "LTM analysis auto-switched from exhaustive to discovery mode: \
                 loop enumeration was abandoned at the circuit budget \
                 (MAX_LTM_CIRCUITS = {}).  The model's feedback structure is too \
                 dense for exhaustive per-loop scoring (circuit count grows \
                 super-exponentially with cycle density).  Per-loop scores are \
                 ranked post-simulation via the strongest-path search; see \
                 docs/design/ltm--loops-that-matter.md for the two-tier \
                 strategy.",
                crate::ltm::ltm_circuit_budget(),
            );
            CompilationDiagnostic(Diagnostic {
                model: model.name(db).clone(),
                variable: None,
                error: DiagnosticError::Assembly(msg),
                severity: DiagnosticSeverity::Warning,
            })
            .accumulate(db);
            is_discovery = true;
            None
        }
        // Late-stage auto-flip, case 2: the variable-level SCC was small
        // enough to clear the early gate, but the cross-element / mixed
        // subgraph (the slow path) blew past the threshold. The tiered
        // enumerator already skipped Johnson on the slow path in that
        // case (`slow_path` is empty); we just need to flip the
        // is_discovery flag so link-score generation, etc. follow the
        // discovery path.
        else if tiered.slow_path_largest_scc > crate::ltm::MAX_LTM_SCC_NODES {
            let msg = format!(
                "LTM analysis auto-switched from exhaustive to discovery mode: \
                 the cross-element / mixed slow-path subgraph's largest SCC has {} nodes, \
                 exceeding MAX_LTM_SCC_NODES = {}.  Per-loop scores are ranked \
                 post-simulation via the strongest-path search; see \
                 docs/design/ltm--loops-that-matter.md for the two-tier strategy.",
                tiered.slow_path_largest_scc,
                crate::ltm::MAX_LTM_SCC_NODES,
            );
            CompilationDiagnostic(Diagnostic {
                model: model.name(db).clone(),
                variable: None,
                error: DiagnosticError::Assembly(msg),
                severity: DiagnosticSeverity::Warning,
            })
            .accumulate(db);
            is_discovery = true;
            None
        } else if tiered.fast_path.is_empty() && tiered.slow_path.is_empty() {
            if !has_input_ports {
                // No loops and no input ports: nothing to score. `is_discovery`
                // was never flipped on this branch, so report exhaustive.
                return LtmVariablesResult {
                    vars: vec![],
                    loop_partitions: indexmap::IndexMap::new(),
                    agg_recovery_truncated: false,
                    pathways_truncated: false,
                    mode: LtmMode::Exhaustive,
                };
            }
            None
        } else {
            // Bind the budget once: `build_loops_from_tiered` enforces it and
            // the `Warning` text below reports it, and a `#[cfg(test)]`
            // override could in principle change between two reads.
            let cross_agg_budget = cross_agg_loop_budget();
            let var_graph = causal_graph_with_modules(db, model, project);
            let (mut detected, truncated_aggs) = build_loops_from_tiered(
                tiered,
                &var_graph,
                source_vars,
                db,
                model,
                project,
                dm_dims,
                cross_agg_budget,
            );
            // Surface a truncated cross-element-through-aggregate loop
            // recovery the same way the auto-flip-to-discovery gate surfaces
            // its mode change: a `Warning` (the human channel) plus the
            // robust `agg_recovery_truncated` flag on the result. The loop
            // list is incomplete -- the budget clipped which disjoint-petal
            // combinations were materialized. `truncated_aggs` names exactly
            // the aggregate nodes whose enumeration was clipped (sorted), so
            // the message points the author at the affected reducers rather
            // than every synthetic agg in the model.
            if !truncated_aggs.is_empty() {
                let msg = format!(
                    "LTM cross-element-through-aggregate loop recovery was truncated: a \
                     reducer in a feedback loop produces more disjoint-petal combinations \
                     than the loop budget ({}); the recovered loop list is incomplete. \
                     Affected aggregate node(s): {}.",
                    cross_agg_budget,
                    truncated_aggs.join(", "),
                );
                CompilationDiagnostic(Diagnostic {
                    model: model.name(db).clone(),
                    variable: None,
                    error: DiagnosticError::Assembly(msg),
                    severity: DiagnosticSeverity::Warning,
                })
                .accumulate(db);
            }
            agg_recovery_truncated = !truncated_aggs.is_empty();
            // GH #516: hops into/out of synthetic `$⁚ltm⁚agg⁚{n}` nodes come
            // back Unknown-polarity from the loop builders (the variable-level
            // graph has no agg node); recover the derivable cases here so
            // agg-traversing loops aren't all forced to Undetermined.
            recover_agg_hop_polarities(&mut detected, &var_graph, db, model, project);
            Some(detected)
        }
    } else {
        None
    };

    // An `IndexMap` so the iteration order is the loops' emission order:
    // enumerated loops are inserted below in `detected_loops` order (the
    // content-sorted order `assign_loop_ids` produced), then pinned loops in
    // pin order. The post-sim rel-loop-score denominator sums `|loop_score|`
    // in this order, so emission order keeps that IEEE-754 sum bit-for-bit
    // identical to the pre-#461 compile-time emitter (GH #468).
    let mut loop_partitions: indexmap::IndexMap<String, Vec<Option<usize>>> =
        indexmap::IndexMap::new();

    // GH #758: the (from, to) edges the conservative per-shape emitter
    // declined to score (arrayed endpoints whose dimensions don't
    // correspond -- see `emit_unscoreable_conservative_edge_warning`).
    // Such an edge has no link-score variable, so a loop-score product
    // through it could only be a guaranteed-zero stub (its fragment either
    // fail-warns on the subscripted missing name or silently multiplies a
    // 0 stub-dep): loop scores traversing any of these edges are dropped
    // below, covered by the edge's single Warning.
    let mut unscoreable_edges: HashSet<(String, String)> = HashSet::new();

    // Part 1: Link scores.
    // Sub-models and discovery mode need scores for ALL edges (pathways
    // reference arbitrary edges). Exhaustive root models only need
    // scores for edges that participate in loops.
    //
    // For each link score, classify the edge to determine whether the
    // score should be arrayed (A2A). When the target variable has
    // dimensions and either the source is scalar or shares the same
    // dimensions, the link score inherits the target's dimensions so
    // that per-element scores are computed via the A2A expansion.

    if has_input_ports || is_discovery {
        for (from, tos) in &edges_result.edges {
            for to in tos {
                emit_link_scores_for_edge(
                    db,
                    source_vars,
                    agg_nodes,
                    from,
                    to,
                    model,
                    project,
                    dm_dims,
                    /* skip_agg_halves = */ false,
                    &mut vars,
                    &mut unscoreable_edges,
                );
            }
        }
    } else if let Some(ref detected_loops) = loops {
        // Helper: look up the `AggNode` for a synthetic-agg node name.
        let agg_by_name = |name: &str| -> Option<&crate::ltm_agg::AggNode> {
            if crate::ltm_agg::is_synthetic_agg_name(name) {
                agg_nodes.aggs.iter().find(|a| a.name == name)
            } else {
                None
            }
        };
        let mut seen_links: HashSet<(String, String)> = HashSet::new();
        for loop_item in detected_loops {
            for link in &loop_item.links {
                // Loop links can carry element-level subscripts on either
                // end -- `link.from` for a per-source-element FixedIndex /
                // cross-dimensional edge ("pop[nyc]"), an agg-routed source
                // element, or (for cross-element loops) `link.to` for an A2A
                // target visited at a single element. The link-score helpers
                // and `source_vars` / `agg_nodes` lookups all key on the
                // variable-level name (the agg name has no subscript), so
                // strip the subscript from both ends for the dedup key and
                // the helper calls. Each helper emits the *full* link score
                // for the (var_from, var_to) edge -- per-element when the
                // target is arrayed -- so the loop-score equation's `[elem]`
                // subscript picks the slot the loop actually visits.
                let from_var_level = strip_subscript(link.from.as_str());
                let to_var_level = strip_subscript(link.to.as_str());
                let key = (from_var_level.to_string(), to_var_level.to_string());
                if !seen_links.insert(key) {
                    continue;
                }
                // A loop link whose target is a synthetic agg node is the
                // `source[d] → agg` half; one whose source is a synthetic agg
                // is the `agg → target` half. (The two halves of a hoisted
                // reducer reference appear as two consecutive loop links
                // `X → agg`, `agg → Y` -- the original `(X, Y)` causal edge
                // never appears in an element-level loop once routed.)
                if let Some(agg) = agg_by_name(to_var_level) {
                    emit_source_to_agg_link_scores(
                        db,
                        source_vars,
                        from_var_level,
                        agg,
                        model,
                        project,
                        &mut vars,
                    );
                } else if let Some(agg) = agg_by_name(from_var_level) {
                    emit_agg_to_target_link_scores(
                        db,
                        source_vars,
                        agg_nodes,
                        agg,
                        to_var_level,
                        model,
                        project,
                        &mut vars,
                    );
                } else {
                    emit_link_scores_for_edge(
                        db,
                        source_vars,
                        agg_nodes,
                        from_var_level,
                        to_var_level,
                        model,
                        project,
                        dm_dims,
                        /* skip_agg_halves = */ true,
                        &mut vars,
                        &mut unscoreable_edges,
                    );
                }
            }
        }
    }

    // Part 2: Loop scores and relative loop scores (exhaustive mode only).
    // Generated for any model with feedback loops, regardless of whether
    // it also has input ports. A model can be both a reusable sub-model
    // AND have internal loops that need scoring.
    //
    // Uses element-level cycle partitions so that cross-element feedback
    // is detected correctly (e.g., population[NYC] and population[Boston]
    // in the same partition when connected through migration).

    // Whether a loop traverses an edge the GH #758 gate declined to score
    // (checking both the representative link cycle and any per-slot link
    // cycles). Such a loop's score product would multiply a never-emitted
    // link-score name -- a guaranteed-zero stub -- so it is dropped from
    // scoring; the edge's single Warning covers the degradation. Takes the
    // edge set as a parameter (rather than capturing `unscoreable_edges`)
    // because the pinned-loop pass below still mutates the set between
    // calls.
    fn traverses_unscoreable(
        l: &crate::ltm::Loop,
        unscoreable_edges: &HashSet<(String, String)>,
    ) -> bool {
        let link_hits = |links: &[crate::ltm::Link]| {
            links.iter().any(|link| {
                unscoreable_edges.contains(&(
                    strip_subscript(link.from.as_str()).to_string(),
                    strip_subscript(link.to.as_str()).to_string(),
                ))
            })
        };
        link_hits(&l.links) || l.slot_links.iter().any(|(_, links)| link_hits(links))
    }

    if let Some(ref detected_loops) = loops {
        // GH #758: drop loops through unscoreable edges. The common case
        // (no unscoreable edge) borrows `detected_loops` unfiltered so the
        // hot path allocates nothing.
        let filtered_loops: Vec<crate::ltm::Loop>;
        let detected_loops: &[crate::ltm::Loop] = if unscoreable_edges.is_empty() {
            detected_loops
        } else {
            filtered_loops = detected_loops
                .iter()
                .filter(|l| !traverses_unscoreable(l, &unscoreable_edges))
                .cloned()
                .collect();
            &filtered_loops
        };
        let partitions_result = model_element_cycle_partitions(db, model, project);
        let partitions = CyclePartitions {
            partitions: partitions_result
                .partitions
                .iter()
                .map(|p| p.iter().map(|s| Ident::new(s)).collect())
                .collect(),
            stock_partition: partitions_result
                .stock_partition
                .iter()
                .map(|(k, v)| (Ident::new(k), *v))
                .collect(),
        };

        // Capture each loop's per-slot partition vector before consuming
        // `partitions` so post-sim `compute_rel_loop_scores*` can group slots
        // into the same `(partition, slot)` denominator bins.  The vector's
        // length must match the loop_score series' slot count -- 1 for a
        // scalar/cross-element/mixed loop, the dimension-element-space size
        // for an A2A loop -- which is the same `n_slots` that
        // `ltm_post::build_loop_element_index` derives from
        // `LtmSyntheticVar.dimensions` + the project dims; both feed
        // `compute_rel_loop_scores_per_element`, so a length mismatch would
        // desync the per-element normalization.
        for l in detected_loops.iter() {
            let parts = partitions.partition_for_loop(l, dm_dims);
            debug_assert!(
                {
                    let expected = if l.dimensions.is_empty() {
                        Some(1usize)
                    } else {
                        let n =
                            crate::ltm::loop_dimension_element_tuples(&l.dimensions, dm_dims).len();
                        // n == 0 only when `dm_dims` doesn't cover the loop's
                        // declared dimensions (a mid-edit inconsistency);
                        // `partition_for_loop` then falls back to the present
                        // suffixes and the length is not predictable here.
                        if n == 0 { None } else { Some(n) }
                    };
                    expected.is_none_or(|n| parts.len() == n)
                },
                "loop {:?}: per-slot partition vector length {} disagrees with the loop's slot \
                 count; it must equal `build_loop_element_index`'s n_slots (both feed \
                 `compute_rel_loop_scores_per_element`)",
                l.id,
                parts.len(),
            );
            loop_partitions.insert(l.id.clone(), parts);
        }

        // Per-exit-port pathway-selection overrides for loop links into a
        // module (PR #684): a loop entering a module at one input port and
        // exiting at one output port must be scored against the pathway it
        // actually traverses, not the module's arbitrary-port composite /
        // unit-transfer fallback. The alias synthetic vars these overrides
        // reference are pushed onto `vars` here so they compile and join the
        // `emitted_link_score_names` set below; the loop-score equation
        // builder consults the `(loop_id, link_index)` overrides first.
        let module_overrides = compute_module_link_overrides(
            db,
            model,
            project,
            detected_loops,
            edges_result,
            source_vars,
        );
        vars.extend(module_overrides.alias_vars.iter().cloned());

        // Build the set of link-score variable names emitted so far so
        // generate_loop_score_equation can resolve each loop link to a
        // name that actually exists. Without this, loops traversing
        // edges whose only AST shape is Wildcard or DynamicIndex would
        // reference a never-emitted Bare canonical name and the
        // fragment compiler would silently fall back to a stub dep.
        let emitted_link_score_names: HashSet<String> = vars
            .iter()
            .filter(|v| v.name.contains("\u{205A}link_score\u{205A}"))
            .map(|v| v.name.clone())
            .collect();
        let loop_vars = crate::ltm_augment::generate_loop_score_variables(
            detected_loops,
            &emitted_link_score_names,
            dm_dims,
            &module_overrides.overrides,
        );
        for (name, equation) in loop_vars {
            // The equation carries its own dimension shape (Scalar /
            // ApplyToAll / Arrayed); mirror it onto the layout-sizing
            // `dimensions` field.
            let dimensions = parse::ltm_equation_dimensions(&equation).to_vec();
            vars.push(LtmSyntheticVar {
                name,
                equation,
                dimensions,
                // Loop scores aren't link scores; `assemble_module` compiles
                // them directly via the non-link-score branch already.
                compile_directly: false,
            });
        }
    }

    // Pinned loops (the LOOPSCORE escape hatch, LTM ref section 10).
    //
    // A modeler pins a loop by naming its variable set; the engine then ALWAYS
    // emits that loop's `loop_score`, regardless of mode. This is the whole
    // point in discovery mode -- the heuristic search emits NO loop_score var
    // for any loop, so a pinned loop is the only way to score a specific loop
    // there. In exhaustive mode a pin usually duplicates an already-enumerated
    // loop, so we dedup against `loops` (by canonical variable-cycle rotation)
    // and skip re-emitting; the enumerated loop already carries a score under
    // its `r{n}`/`b{n}`/`u{n}` id.
    //
    // A pin's cycle is dimension-classified by `model_pinned_loops` (GH #653):
    // a pure-A2A pin carries `dimensions` and emits an arrayed (per-element)
    // loop score with a per-slot partition vector; a scalar pin emits a scalar
    // score with a single-slot partition. Both ride the same
    // `generate_loop_score_variables` / `partition_for_loop` machinery as
    // enumerated loops.
    let pinned = model_pinned_loops(db, model, project);
    for (name, reason) in &pinned.invalid {
        // Surface invalid pins the same way the auto-flip gate surfaces its
        // mode change: a `Warning`. Without this a typo'd or stale pin would
        // silently score nothing, which is the failure mode #466 warns about.
        let _ = name;
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: None,
            error: DiagnosticError::Assembly(reason.clone()),
            severity: DiagnosticSeverity::Warning,
        })
        .accumulate(db);
    }
    if !pinned.loops.is_empty() {
        // The variable-level node set of each already-emitted enumerated loop,
        // keyed by canonical rotation, so a pin that duplicates one is skipped.
        // (Empty in discovery mode -- `loops` is `None` there -- so every pin
        // is emitted, which is exactly the escape-hatch behavior we want.)
        let enumerated_rotations: HashSet<Vec<String>> = loops
            .as_ref()
            .map(|ls| ls.iter().map(canonical_variable_rotation).collect())
            .unwrap_or_default();

        // The element-keyed cycle partitions, so a pin resolves its per-slot
        // partition(s) the same way enumerated loops do: one entry per element
        // slot for a dimensioned (A2A) pin, a singleton for a scalar one.
        let partitions_result = model_element_cycle_partitions(db, model, project);
        let pin_partitions = CyclePartitions {
            partitions: partitions_result
                .partitions
                .iter()
                .map(|p| p.iter().map(|s| Ident::new(s)).collect())
                .collect(),
            stock_partition: partitions_result
                .stock_partition
                .iter()
                .map(|(k, v)| (Ident::new(k), *v))
                .collect(),
        };

        // Track which `(from, to)` edges already have a link-score var so a
        // pin only emits the link scores its cycle still needs (in exhaustive
        // mode the enumerated loop already emitted them; in discovery mode all
        // edges did). Keyed on the variable-level edge.
        let mut emitted_edges: HashSet<(String, String)> = vars
            .iter()
            .filter_map(|v| link_score_edge_endpoints(&v.name))
            .collect();

        // Helper: look up the `AggNode` for a synthetic-agg node name (a pin
        // whose cycle traverses an inlined reducer has agg hops in its links,
        // exactly like enumerated loops).
        let agg_by_name = |name: &str| -> Option<&crate::ltm_agg::AggNode> {
            if crate::ltm_agg::is_synthetic_agg_name(name) {
                agg_nodes.aggs.iter().find(|a| a.name == name)
            } else {
                None
            }
        };

        for pin in &pinned.loops {
            // Per scored loop: skip any whose variable-level cycle an
            // enumerated loop already scores (exhaustive mode -- the
            // enumerated emission is preferred, whatever shape the enumerator
            // gave it: A2A, per-element scalars, or cross-element). In
            // discovery mode `enumerated_rotations` is empty, so every scored
            // loop is emitted -- the escape-hatch behavior.
            let loops_to_emit: Vec<&Loop> = pin
                .loops
                .iter()
                .filter(|l| !enumerated_rotations.contains(&canonical_variable_rotation(l)))
                .collect();

            // GH #758: warn at most once per PIN (not per element-level
            // instance) when its cycle traverses an unscoreable edge -- a
            // cross-element pin can expand to many `pin{n}⁚{j}` instances,
            // and one warning per instance is the same cascade the
            // unscoreable-edge treatment exists to avoid.
            let mut warned_unscoreable_pin = false;
            for pin_loop in loops_to_emit {
                // Emit any link scores this loop's cycle needs that aren't
                // present, with the same per-link agg-hop dispatch the
                // enumerated path uses (links may carry element subscripts
                // and traverse synthetic agg nodes).
                for link in &pin_loop.links {
                    let from_var_level = strip_subscript(link.from.as_str());
                    let to_var_level = strip_subscript(link.to.as_str());
                    let key = (from_var_level.to_string(), to_var_level.to_string());
                    if !emitted_edges.insert(key) {
                        continue;
                    }
                    if let Some(agg) = agg_by_name(to_var_level) {
                        emit_source_to_agg_link_scores(
                            db,
                            source_vars,
                            from_var_level,
                            agg,
                            model,
                            project,
                            &mut vars,
                        );
                    } else if let Some(agg) = agg_by_name(from_var_level) {
                        emit_agg_to_target_link_scores(
                            db,
                            source_vars,
                            agg_nodes,
                            agg,
                            to_var_level,
                            model,
                            project,
                            &mut vars,
                        );
                    } else {
                        emit_link_scores_for_edge(
                            db,
                            source_vars,
                            agg_nodes,
                            from_var_level,
                            to_var_level,
                            model,
                            project,
                            dm_dims,
                            /* skip_agg_halves = */ true,
                            &mut vars,
                            &mut unscoreable_edges,
                        );
                    }
                }

                // GH #758: a pin whose cycle traverses an unscoreable edge
                // has no link score to multiply -- its loop score could only
                // be a guaranteed-zero stub (or a warned fragment failure).
                // Warn naming the pin (mirroring the invalid-pin treatment;
                // once per pin, not per instance) and skip the instance.
                // Checked AFTER the link-score emission above so the gate
                // has classified this cycle's edges even when no enumerated
                // loop visited them.
                if traverses_unscoreable(pin_loop, &unscoreable_edges) {
                    if !warned_unscoreable_pin {
                        warned_unscoreable_pin = true;
                        CompilationDiagnostic(Diagnostic {
                            model: model.name(db).clone(),
                            variable: None,
                            error: DiagnosticError::Assembly(format!(
                                "pinned loop '{}' traverses a causal edge whose link \
                                 score could not be computed (see the unscoreable-edge \
                                 warning); its affected instances are not scored",
                                pin.name
                            )),
                            severity: DiagnosticSeverity::Warning,
                        })
                        .accumulate(db);
                    }
                    continue;
                }

                // Register this loop's cycle partition(s): a per-slot vector
                // for a dimensioned (A2A) pin, a singleton for a scalar one --
                // the same `partition_for_loop` resolution enumerated loops
                // use.
                loop_partitions.insert(
                    pin_loop.id.clone(),
                    pin_partitions.partition_for_loop(pin_loop, dm_dims),
                );

                // Emit the pinned loop_score var. The equation is the product
                // of the cycle's link scores, resolved against the names
                // emitted so far -- identical machinery to enumerated loops,
                // including the dimension shaping (a dimensioned pin yields an
                // ApplyToAll / per-slot Arrayed equation exactly like an
                // enumerated A2A loop).
                let emitted_link_score_names: HashSet<String> = vars
                    .iter()
                    .filter(|v| v.name.contains("\u{205A}link_score\u{205A}"))
                    .map(|v| v.name.clone())
                    .collect();
                // Pinned loops pass an empty override map for now: the
                // per-exit-port module-link override (PR #684) is not yet
                // wired through the pin path. RESIDUAL GAP: a pin whose cycle
                // traverses a multi-output module still scores its input->module
                // link against the arbitrary-port base fallback. Pins through
                // single-output modules (the common case) are unaffected.
                let pin_loop_vars = crate::ltm_augment::generate_loop_score_variables(
                    std::slice::from_ref(pin_loop),
                    &emitted_link_score_names,
                    dm_dims,
                    &crate::ltm_augment::LoopLinkOverrides::new(),
                );
                for (lname, equation) in pin_loop_vars {
                    let dimensions = parse::ltm_equation_dimensions(&equation).to_vec();
                    vars.push(LtmSyntheticVar {
                        name: lname,
                        equation,
                        dimensions,
                        compile_directly: false,
                    });
                }
            }
        }
    }

    // Pathway and composite scores for models with input ports.
    //
    // Each pathway is a product of link-score references. Since
    // emit_per_shape_link_scores only emits the names that appear in
    // each target's AST, we resolve each link against the set of
    // already-emitted names with shape priority (Bare > FixedIndex >
    // Wildcard > DynamicIndex). Without this, an input-port model with
    // an edge like `share[r] = SUM(x[*])` would reference the never-
    // emitted Bare canonical name `x→share`, and the fragment
    // compiler's stub-dep fallback would silently drop that pathway's
    // contribution to the composite score.
    let pathway_emitted_names: HashSet<String> = vars
        .iter()
        .filter(|v| v.name.contains("\u{205A}link_score\u{205A}"))
        .map(|v| v.name.clone())
        .collect();
    for (input_port, port_pathways) in &pathways {
        let mut pathway_names = Vec::new();
        for (idx, pathway_links) in port_pathways.iter().enumerate() {
            let path_var_name = format!(
                "$\u{205A}ltm\u{205A}path\u{205A}{}\u{205A}{}",
                input_port.as_str(),
                idx
            );

            let link_score_refs: Vec<String> = pathway_links
                .iter()
                .map(|link| {
                    // Pathway links come from the variable-level causal
                    // graph (`enumerate_pathways_to_outputs`), so `link.to`
                    // never carries an element subscript and there is no
                    // visited-element to pass.
                    let resolved = crate::ltm_augment::resolve_link_score_name_for_loop(
                        link.from.as_str(),
                        link.to.as_str(),
                        &pathway_emitted_names,
                        None,
                    );
                    format!("\"{resolved}\"")
                })
                .collect();

            let equation = if link_score_refs.is_empty() {
                "0".to_string()
            } else {
                link_score_refs.join(" * ")
            };

            pathway_names.push(path_var_name.clone());
            vars.push(LtmSyntheticVar {
                name: path_var_name,
                equation: datamodel::Equation::Scalar(equation),
                dimensions: vec![],
                compile_directly: false,
            });
        }

        let composite_name = format!(
            "$\u{205A}ltm\u{205A}composite\u{205A}{}",
            input_port.as_str()
        );
        // The selection folds through O(1)-sized accumulator helper variables
        // (named under the port's `⁚path⁚` prefix so they sort -- and therefore
        // evaluate -- after the pathway vars they reference and before this
        // composite). Inlining the fold into one expression would double the
        // equation text per pathway; see `generate_max_abs_selection`.
        let (equation, acc_helpers) =
            generate_max_abs_selection(input_port.as_str(), &pathway_names);
        vars.extend(acc_helpers);
        vars.push(LtmSyntheticVar {
            name: composite_name,
            equation: datamodel::Equation::Scalar(equation),
            dimensions: vec![],
            compile_directly: false,
        });
    }

    // Sort by evaluation-order category so the VM's sequential flow
    // evaluation respects the dependency chain: composites reference paths
    // which reference loop scores which reference link scores, and link
    // scores referencing an aggregate node read its current-step value, so
    // the agg fragment must run first. Within each category, sort lexically
    // for determinism. (`compute_layout` section 3 re-sorts LTM vars purely
    // by name -- `$⁚ltm⁚agg⁚{n}` < `$⁚ltm⁚link_score⁚...` lexically, so the
    // agg gets its layout slot before any consumer there too -- but the
    // runlist order is what the same-timestep ordering hazard turns on, and
    // that comes from this sort.)
    vars.sort_by(|a, b| {
        fn category(name: &str) -> u8 {
            // The agg check uses the `$⁚ltm⁚agg⁚` *prefix*, not a substring
            // search: the `agg → target` link score is named
            // `$⁚ltm⁚link_score⁚$⁚ltm⁚agg⁚{n}→{to}` and contains `⁚agg⁚`,
            // but it is a link score (category 1) that must run *after* the
            // agg aux it references.
            if crate::ltm_agg::is_synthetic_agg_name(name) {
                0 // aggregate nodes: before everything that may reference them
            } else if name.contains("\u{205A}composite\u{205A}") {
                4
            } else if name.contains("\u{205A}path\u{205A}") {
                3
            } else if name.contains("\u{205A}loop_score\u{205A}") {
                2
            } else {
                1 // link_score and anything else
            }
        }
        category(&a.name)
            .cmp(&category(&b.name))
            .then_with(|| a.name.cmp(&b.name))
    });
    LtmVariablesResult {
        vars,
        loop_partitions,
        agg_recovery_truncated,
        pathways_truncated,
        // Read the resolved mode from the shared `model_ltm_mode` query rather
        // than re-deriving it from the local `is_discovery` flag, so this query
        // and `model_detected_loops` can never disagree about the discovery
        // gate. The two computations are identical by construction (the local
        // `is_discovery` drives the auto-flip `Warning`s and link-score
        // branching above; `model_ltm_mode` is the single source of truth for
        // the decision itself).
        mode: model_ltm_mode(db, model, project),
    }
}

#[cfg(test)]
#[path = "../ltm_tests.rs"]
mod ltm_tests;
