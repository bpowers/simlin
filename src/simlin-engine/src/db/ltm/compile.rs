// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Per-equation compilation of LTM synthetic variables to symbolic
//! bytecodes.
//!
//! This is the emission side of the LTM pipeline: the per-link salsa
//! fragment (`compile_ltm_var_fragment`), the shape-aware link-score
//! equation-text query (`link_score_equation_text_shaped`), the two LTM
//! constructors of `compiler::fragment::FragmentInput` (`ltm_fragment_input`
//! for a synthetic variable, `ltm_implicit_fragment_input` for a helper its
//! parse synthesized) with the emitters over them
//! (`compile_ltm_equation_fragment`, `compile_ltm_implicit_var_fragment`),
//! the synthetic-fragment selector (`compile_ltm_synthetic_fragment`), and
//! the compile-failure diagnostic pass (`model_ltm_fragment_diagnostics`).

use std::collections::{BTreeSet, HashMap};

use crate::canonicalize;
use crate::common::{Canonical, Ident, IdentMap};
use crate::datamodel;

use crate::compiler::fragment::{DepShape, FragmentInput, lower_fragment};
use crate::db::var_fragment::{
    dep_head, dimensions_named, implicit_dep_shape, is_implicit_global, source_dep_shape,
};
use crate::db::{
    Db, LtmLinkId, LtmSyntheticVar, RefShape, SourceModel, SourceProject, SourceVariableKind,
    VarFragmentResult, build_module_inputs, canonical_module_input_set,
    compile_phase_to_per_var_bytecodes, extract_tables_from_source_var, model_implicit_var_info,
    model_module_ident_context, module_dep_shape, module_input_prefix,
    parse_source_variable_with_module_context, project_converted_dimensions,
    project_dimensions_context, project_units_context, reconstruct_single_variable,
    variable_dimensions,
};

use super::parse::{parse_ltm_equation, scalarize_ltm_equation};
use super::{
    LtmEquation, LtmImplicitVarMeta, ltm_module_idents, model_ltm_implicit_var_info,
    model_ltm_var_name_index, model_ltm_variables,
};

/// Compile a single LTM synthetic variable's equation to symbolic
/// bytecodes.
///
/// This is the per-link compilation granularity that enables incremental
/// recomputation: when a variable's equation changes, salsa only
/// recompiles fragments for affected links. Equation edits that don't
/// change the dependency set return their cached fragment (AC1.2).
///
/// LTM equations are pure scalar aux equations that may reference:
/// - Model variables (stocks, flows, auxes) from the parent model
/// - Other LTM variables (loop scores referencing link scores)
/// - Implicit helper/module variables created during parsing
/// - Implicit time/dt/initial_time/final_time variables
///
/// Parsed LTM equations may synthesize helper auxes for PREVIOUS/INIT
/// and may also expand stdlib module calls, so those implicit vars need
/// to be handled the same way as in `compile_var_fragment`.
///
/// The equation is sourced from the per-shape query
/// [`link_score_equation_text_shaped`] with the `Bare` shape -- the SAME query
/// `model_ltm_variables` emits and reports the standard scalar Bare score from
/// -- so the compiled fragment and the reported/serialized equation are
/// single-sourced and cannot drift. (The former `(from, to)`-keyed
/// `link_score_equation_text` query was a second derivation of the same score;
/// it passed an empty dims context and `dim_ctx=None`, which the prior stage
/// had to re-align occurrence-stream-by-occurrence-stream to keep byte-identical
/// -- routing through the shaped twin retires that parallel derivation
/// entirely.) This fragment stays keyed by `(from, to)` for per-link
/// incrementality; the shaped query is keyed by `(from, to, Bare)`, so an edit
/// to an unrelated edge backdates both and this fragment's bytecode is reused.
///
/// The shaped query returns the equation shaped to the TARGET (an `ApplyToAll`
/// for a Bare read into an arrayed A2A target), but this `(from, to)`-keyed
/// fragment is consumed ONLY by `compile_ltm_synthetic_fragment`'s sub-case (a)
/// -- the dims-empty SCALAR Bare score, which the emission loop reports as
/// `retarget_ltm_equation_dims(shaped_raw, [])` and lays out with one slot.
/// `scalarize_ltm_equation` here is exactly that retarget for the empty-dims
/// case (identical for `Scalar`/`ApplyToAll`/`Arrayed` inputs) and a no-op for a
/// scalar target, so the compiled fragment stays byte-identical to the reported
/// equation. Without it, a scalar feeder read bare beside a hoisted reducer in
/// an arrayed target (`share[D1] = SUM(arr[*] * scale) + scale`) would compile
/// the raw `ApplyToAll` -- writing element offsets past the 1-slot layout -- and
/// abort the whole model with `NotSimulatable`, while the reported score stayed
/// scalar (the compiled-vs-reported drift this single-sourcing exists to kill).
#[salsa::tracked(returns(ref))]
pub fn compile_ltm_var_fragment(
    db: &dyn Db,
    link_id: LtmLinkId<'_>,
    model: SourceModel,
    project: SourceProject,
) -> Option<VarFragmentResult> {
    #[cfg(test)]
    crate::db::note_fragment_execution(
        crate::db::FragmentExecKind::Ltm,
        &format!("{}\u{2192}{}", link_id.link_from(db), link_id.link_to(db)),
    );

    let ShapedLinkScore::Scored { var: lsv, .. } =
        link_score_equation_text_shaped(db, link_id, RefShape::Bare, model, project)
    else {
        return None;
    };

    let equation = scalarize_ltm_equation(lsv.equation.clone());
    compile_ltm_equation_fragment(db, &lsv.name, &equation, model, project, None)
}

// How many times `link_score_equation_text_shaped`'s body has run on this
// thread (test-only).
//
// The query documents a per-involved-variable incrementality claim, and only a
// body-entry count can check it: salsa BACKDATES a re-executed query whose
// value compares equal, so the memo neither moves nor changes and pointer
// equality reads identical whether the body ran or not. Thread-local for the
// same reasons `db::stages`' counters are -- see the note there, including the
// warning about what happens if this subtree is ever parallelized.
#[cfg(test)]
thread_local! {
    static SHAPED_LINK_SCORE_EXECUTIONS: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
}

/// Zero the counter (test-only). Call it after the fixture is synced and
/// first compiled, so setup work is not charged to the measured edit.
#[cfg(test)]
pub(crate) fn reset_shaped_link_score_executions() {
    SHAPED_LINK_SCORE_EXECUTIONS.with(|c| c.set(0));
}

/// Read the counter (test-only).
#[cfg(test)]
pub(crate) fn shaped_link_score_executions() -> usize {
    SHAPED_LINK_SCORE_EXECUTIONS.with(|c| c.get())
}

/// Outcome of [`link_score_equation_text_shaped`] for one
/// `(from, to, shape)` tuple.
///
/// The shaped equation-text query has three distinct terminal states that
/// the emission loop MUST tell apart -- collapsing them into a bare
/// `Option` (the GH #780 defect) made a `PartialEquationError` skip
/// indistinguishable from a benign "no variable here", so the
/// `unscoreable_edges` recording the partial-equation class needs never
/// fired and dependent loop scores degraded to warned constant-0 stubs:
///
/// - [`Scored`](ShapedLinkScore::Scored) -- the link-score variable was
///   built; emit it.
/// - [`Unscoreable`](ShapedLinkScore::Unscoreable) -- a
///   [`PartialEquationError`](crate::ltm_augment::PartialEquationError)
///   (the GH #311 parse class, the GH #526/T7 both-legs-doomed
///   mismatched-dep class, or the GH #779 bare-reducer-feeder decline)
///   made this `(from, to)` edge unscoreable. The
///   loud `Warning` was already accumulated by the query; the caller MUST
///   record the edge in `unscoreable_edges` so loop scores traversing it
///   are DROPPED (the #758 contract), not stubbed.
/// - [`NoVariable`](ShapedLinkScore::NoVariable) -- no variable for benign
///   structural reasons (the target could not be reconstructed, or a
///   module link has no composite/output to score). NOT an unscoreable
///   edge: the caller skips it silently, exactly as the pre-fix `None`
///   did, and loop scores through it are unaffected.
///
/// Surfacing the partial-equation signal through the query's RETURN value
/// (rather than a side channel) keeps it consistent under salsa caching:
/// the salsa accumulator already replays the `Warning` on every cache hit,
/// and the caller re-reads this memoized value -- and so re-inserts into
/// the freshly-rebuilt `unscoreable_edges` set -- on every
/// `model_ltm_variables` evaluation, whether the query body re-ran or not.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub enum ShapedLinkScore {
    /// The link-score variable was generated. `freeze_helpers` carries the
    /// GH #995 array-freeze helper variables the score's partial references
    /// (usually empty); the emission loop pushes them alongside the score,
    /// deduplicated by their content-derived names.
    Scored {
        /// Boxed to keep the enum small next to its dataless variants
        /// (clippy `large_enum_variant`); `LtmSyntheticVar` carries whole
        /// parsed equations.
        var: Box<LtmSyntheticVar>,
        freeze_helpers: Vec<LtmSyntheticVar>,
    },
    /// A `PartialEquationError` made the edge unscoreable; the warning is
    /// accumulated and the caller records the edge in `unscoreable_edges`.
    Unscoreable,
    /// No variable for benign structural reasons; not an unscoreable edge.
    NoVariable,
}

/// Compute the per-shape link score equation text for a single causal link.
///
/// This is the sole derivation of a link score's equation text. Keyed on
/// `(from, to, shape)`, it emits one variable per unique shape in the
/// target's AST so per-shape link scores can be ceteris-paribus scored
/// against their actual reference site. `model_ltm_variables` calls it once
/// per unique shape, and the standard scalar `Bare` score's fragment
/// compiler ([`compile_ltm_var_fragment`]) reads the SAME `(from, to, Bare)`
/// result -- so the compiled fragment and the reported/serialized equation
/// are single-sourced and cannot drift. (A former `(from, to)`-keyed
/// `link_score_equation_text` query was a second derivation of the same
/// score; it was deleted in favour of routing every consumer through this
/// query.)
///
/// Returns a [`ShapedLinkScore`] (NOT a bare `Option`) so the emission loop
/// can distinguish a `PartialEquationError`-driven unscoreable skip from a
/// benign missing variable -- see that type's docs and GH #780.
///
/// Module-involved links delegate to `module_link_score_equation` for the
/// module formulas (composite reference / black-box delta-ratio). Their
/// equations are independent of `shape`, but the variable name still
/// carries the suffix so the emission loop can keep one entry per
/// (from, to, shape) tuple in the `Vec<LtmSyntheticVar>`.
///
/// `lsv.dimensions` is left empty here; the caller (the link emission
/// loop) sets dimensions per the link-score-dimensions policy after
/// receiving the value.
///
/// Salsa-tracked so a per-shape link score is recomputed only when the
/// involved variables (and their shape-classifying dimensions) change.
/// Lives in `db/ltm/compile.rs` rather than `db.rs` so the latter stays
/// under the project's per-file line cap.
#[salsa::tracked(returns(ref))]
pub fn link_score_equation_text_shaped<'db>(
    db: &'db dyn Db,
    link_id: LtmLinkId<'db>,
    shape: RefShape,
    model: SourceModel,
    project: SourceProject,
) -> ShapedLinkScore {
    use crate::common::{Canonical, Ident};
    use crate::db::LtmSyntheticVar;
    use crate::db::module_link_score_equation;

    #[cfg(test)]
    SHAPED_LINK_SCORE_EXECUTIONS.with(|c| c.set(c.get() + 1));

    let from_name = link_id.link_from(db);
    let to_name = link_id.link_to(db);
    let from_ident = Ident::<Canonical>::new(from_name);
    let to_ident = Ident::<Canonical>::new(to_name);

    let from_var = reconstruct_single_variable(db, model, project, from_name);
    // A target that cannot be reconstructed is a benign structural skip
    // (degenerate edge), NOT a partial-equation failure -- no `Warning`, no
    // unscoreable-edge recording. Loop scores through such an edge are
    // unaffected, exactly as the pre-GH #780 `None` behaved.
    let Some(to_var) = reconstruct_single_variable(db, model, project, to_name) else {
        return ShapedLinkScore::NoVariable;
    };

    let var_name = crate::ltm_augment::link_score_var_name(from_name, to_name, &shape);

    let from_is_module = from_var.as_ref().is_some_and(|v| v.is_module());
    let to_is_module = to_var.is_module();

    // Module-involved links: shape doesn't change the equation (modules
    // are scalar nodes in the causal graph; the composite-reference /
    // ceteris-paribus / unit-transfer formulas don't reach into the AST).
    // Delegate to the shared `module_link_score_equation` helper, and key
    // the synthetic variable by the shape-driven name so the emission
    // loop's per-shape map works. A `None` here (a passthrough module with
    // no composite or
    // output port to score -- see `module_link_score_equation`) is a benign
    // structural skip, NOT an unscoreable edge.
    if from_is_module || to_is_module {
        return match module_link_score_equation(
            db,
            model,
            project,
            from_name,
            to_name,
            from_var.as_ref(),
            &to_var,
        )
        .map(|equation| LtmSyntheticVar {
            name: var_name,
            equation,
            dimensions: vec![],
            compile_directly: false,
        }) {
            // Module-link partials thread no dep-dims table (their GH #526
            // check keeps the permissive legacy collapse), so no array
            // freeze can be materialized on this arm.
            Some(lsv) => ShapedLinkScore::Scored {
                var: Box::new(lsv),
                freeze_helpers: vec![],
            },
            None => ShapedLinkScore::NoVariable,
        };
    }

    // Standard ceteris-paribus formula for non-module links.
    //
    // Build the source's per-dimension element lists so the per-shape
    // partial-equation builder can validate literal-index names like
    // `[NYC]` against the source's actual dimensions. For scalar sources
    // this is empty, which is the right input for Bare-shape calls (no
    // subscripts to classify).
    let source_dim_elements: Vec<Vec<String>> =
        if let Some(from_sv) = model.variables(db).get(from_name) {
            variable_dimensions(db, *from_sv, project)
                .iter()
                .map(crate::ltm_augment::dimension_element_names)
                .collect()
        } else {
            // Implicit variables (SMOOTH/DELAY expansions) aren't in
            // source_vars and are scalar by construction.
            Vec::new()
        };

    let mut all_vars = HashMap::new();
    if let Some(ref fv) = from_var {
        all_vars.insert(from_ident.clone(), fv.clone());
    }
    all_vars.insert(to_ident.clone(), to_var.clone());
    // The project's `DimensionsContext` is threaded into the GH #511
    // iterated-dimension recognition for the mapped-dimension case
    // (`x[State]` over a source declared with `Region`, `State` maps to
    // `Region`); the cached context depends only on the salsa-tracked
    // dimensions input, so this fn is recomputed when a dimension's
    // mappings change.
    let dim_ctx = project_dimensions_context(db, project);
    // Declared dims of the target's ARRAY deps (canonical name -> dims),
    // threaded into the GH #526 other-dep correspondence check so a
    // transposed / position-mismatched non-live array dep is never
    // collapsed to a wrong-element bare `PREVIOUS(dep)`. Scalar deps and
    // deps with no resolvable source variable (implicit/synthetic names)
    // are omitted -- the recognizer keeps its permissive legacy collapse
    // for those.
    let dep_dims: HashMap<String, Vec<crate::dimensions::Dimension>> = to_var
        .ast()
        .map(|ast| {
            use crate::ast::Ast;
            let target_ast_dims: &[crate::dimensions::Dimension] = match ast {
                Ast::Scalar(_) => &[],
                Ast::ApplyToAll(dims, _) | Ast::Arrayed(dims, _, _, _) => dims,
            };
            crate::variable::identifier_set(ast, target_ast_dims, None)
                .into_iter()
                .filter_map(|dep| {
                    let sv = model.variables(db).get(dep.as_str())?;
                    if sv.kind(db) == SourceVariableKind::Module {
                        return None;
                    }
                    let dims = variable_dimensions(db, *sv, project);
                    (!dims.is_empty()).then(|| (dep.as_str().to_string(), dims.clone()))
                })
                .collect()
        })
        .unwrap_or_default();
    // Test-only seam. At GH #780 time this query's `PartialEquationError`
    // terminal (below) was unreachable through any compiling model -- every
    // doom on the shaped path was either recovered by
    // `shaped_guard_form_text`'s changed-last fallback or pinned to a
    // concrete element upstream (the GH #780 reachability probe) -- so the
    // [`force_partial_equation_error`] override was added to exercise the
    // unscoreable-edge contract end-to-end (per
    // docs/dev/rust.md#test-time-budgets -- a test-only override and a tiny
    // fixture, not a contrived production input). The GH #779
    // bare-reducer-feeder decline has since made the terminal LIVE-reachable
    // (`bare_feeder_of_unhoisted_reducer_declines_loudly`); the seam is
    // retained because it can doom ONE arbitrary edge of a multi-edge model
    // independent of any equation shape, which the surgical-degradation
    // tests still need. (The AGG-HALF feeder emitter's DUPLICATE-DIM
    // `UnfreezablePartial` bail -- `pin_iterated_dim_indices`, PR #787 --
    // was its only live square-source caller, but the GH #778/#785 decline
    // now skips that whole shape at agg minting, so that specific terminal
    // is unreachable defense-in-depth; see
    // `square_source_duplicate_dim_reducer_is_loudly_skipped`.)
    #[cfg(test)]
    if force_partial_equation_error(from_name, to_name) {
        let err = crate::ltm_augment::PartialEquationError::new(
            "<test-forced partial-equation failure (GH #780)>",
        );
        super::emit_ltm_partial_equation_warning(db, model, &var_name, &err);
        return ShapedLinkScore::Unscoreable;
    }

    // The generator returns the equation already tagged with the target's
    // dimensionality (`Scalar`, `ApplyToAll`, or `Arrayed`). `dimensions`
    // and `compile_directly` are left at defaults here; the emission loop
    // in `model_ltm_variables` (`emit_per_shape_link_scores`) overwrites
    // `dimensions`, the equation's dimension names, and `compile_directly`
    // (set when `shape` is not `Bare`) with the per-shape policy result.
    // A `PartialEquationError` means the target's equation text could not
    // be rendered as a compilable ceteris-paribus partial -- the GH #311
    // parse class, the GH #526/T7 both-legs-doomed mismatched-dep class, or
    // the GH #779 bare-reducer-feeder decline.
    // Warn, and report the edge as `Unscoreable` so the emission loop
    // records it in `unscoreable_edges` and DROPS dependent loop scores
    // (GH #758/#780); emitting a silently non-ceteris-paribus link score
    // would compile cleanly, so `model_ltm_fragment_diagnostics` would not
    // catch it, and degrading dependent loops to warned constant-0 stubs
    // would look like legitimate values.
    // The target's per-occurrence access-shape IR (the single classifier
    // family the ceteris-paribus wrap consumes). Empty for a target with no
    // recorded occurrences (a structural edge); the wrap then makes no
    // shape-driven decision.
    let ref_sites = crate::db::ltm_ir::model_ltm_reference_sites(db, model, project);
    let to_occurrences: &[crate::db::ltm_ir::OccurrenceSite] = ref_sites
        .occurrences
        .get(to_name)
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    let mut raw_freeze_helpers = Vec::new();
    let equation = match crate::ltm_augment::generate_link_score_equation_for_link(
        &from_ident,
        &to_ident,
        &shape,
        &source_dim_elements,
        &to_var,
        &all_vars,
        Some(dim_ctx),
        Some(&dep_dims),
        to_occurrences,
        &mut raw_freeze_helpers,
    ) {
        Ok(eqn) => eqn,
        Err(err) => {
            super::emit_ltm_partial_equation_warning(db, model, &var_name, &err);
            return ShapedLinkScore::Unscoreable;
        }
    };

    ShapedLinkScore::Scored {
        var: Box::new(LtmSyntheticVar {
            name: var_name,
            equation,
            dimensions: vec![],
            compile_directly: false,
        }),
        freeze_helpers: raw_freeze_helpers
            .into_iter()
            .map(freeze_helper_var)
            .collect(),
    }
}

/// Convert a wrap-produced [`crate::ltm_augment::ArrayFreezeHelper`] into the
/// synthetic variable the emission loop registers: a per-element
/// (`LtmEquation::Arrayed`) aux whose every arm is a statically-subscripted
/// `PREVIOUS` read (`LoadPrev`), sized by its `dimensions` for layout, and
/// compiled verbatim (`compile_directly` -- the (from, to)-keyed salsa path
/// has no meaning for a helper).
pub(super) fn freeze_helper_var(h: crate::ltm_augment::ArrayFreezeHelper) -> LtmSyntheticVar {
    // A scalar whole-dep helper (a frozen SCALAR reference in a view
    // position) has no dims and exactly one arm.
    let equation = if h.dims.is_empty() {
        let body = h
            .arms
            .into_iter()
            .next()
            .map(|(_, body)| body)
            .unwrap_or_else(|| "0".to_string());
        LtmEquation::scalar(body)
    } else {
        LtmEquation::arrayed(h.dims.clone(), h.arms, None, false)
    };
    LtmSyntheticVar {
        name: h.name,
        equation,
        dimensions: h.dims,
        compile_directly: true,
    }
}

// Test-only override: forces [`link_score_equation_text_shaped`] to report
// the sentinel edge as a `PartialEquationError` (GH #780). Scoped by an
// active `ForcePartialEquationErrorGuard`; off in production builds. The
// forced edge is matched by `(from, to)` exactly, so a test can doom ONE
// causal edge of a multi-edge model and assert the surgical degradation
// (the doomed edge's loops drop; the rest keep their scores).
#[cfg(test)]
thread_local! {
    static FORCE_PARTIAL_ERROR_EDGE: std::cell::RefCell<Option<(String, String)>> =
        const { std::cell::RefCell::new(None) };
}

/// Whether an active [`ForcePartialEquationErrorGuard`] marks `(from, to)`
/// as a forced-`PartialEquationError` edge (test-only; GH #780). Shared
/// across the LTM module so every shaped/per-element generator call site
/// honours the same override -- the salsa query here AND the direct
/// `generate_scalar_to_element_equation` call site in `link_scores.rs`.
#[cfg(test)]
pub(super) fn force_partial_equation_error(from: &str, to: &str) -> bool {
    FORCE_PARTIAL_ERROR_EDGE.with(|c| {
        c.borrow()
            .as_ref()
            .is_some_and(|(f, t)| f == from && t == to)
    })
}

/// RAII guard (test-only) installing a forced-`PartialEquationError` edge
/// for the current thread, restored on drop so a panicking test does not
/// leak it. Because `link_score_equation_text_shaped` and
/// `model_ltm_variables` are salsa-memoized, the guard must outlive every
/// LTM query in the test it controls, and each test must use a fresh `db`
/// (the override is not a salsa input, so a memoized result would otherwise
/// survive regardless of guard state) -- the same discipline
/// [`crate::db::ltm::AggLoopBudgetGuard`] documents.
#[cfg(test)]
pub(crate) struct ForcePartialEquationErrorGuard {
    prev: Option<(String, String)>,
}

#[cfg(test)]
impl ForcePartialEquationErrorGuard {
    pub(crate) fn new(from: &str, to: &str) -> Self {
        let prev =
            FORCE_PARTIAL_ERROR_EDGE.with(|c| c.replace(Some((from.to_string(), to.to_string()))));
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for ForcePartialEquationErrorGuard {
    fn drop(&mut self) {
        FORCE_PARTIAL_ERROR_EDGE.with(|c| {
            *c.borrow_mut() = self.prev.take();
        });
    }
}

/// Result of [`lower_ltm_variable`]: the lowered variable plus the
/// dependency classification of its lowered AST, computed once during
/// lowering. Callers reuse `dep_idents`/`referenced_tables` to build their
/// dependency shapes instead of re-running `classify_dependencies` on the
/// returned variable -- the classification is a per-fragment AST walk, and
/// duplicating it across every LTM fragment was a measurable slice of
/// C-LEARN's LTM compile time.
struct LoweredLtmVariable {
    variable: crate::variable::Variable,
    /// `classify_dependencies(..).all` of the lowered AST
    /// (`Variable::ast()`, which for the Aux-parsed Vars LTM produces is
    /// the dt AST). Identifier sets are lowering-scope-independent, so
    /// this is valid for the returned `variable` whether or not the
    /// scoped re-lower ran.
    ///
    /// ORDERED, not a `HashSet` -- **deliberately, even though no consumer is
    /// order-sensitive.** Keep it that way; the reasoning is not "something
    /// breaks if you don't", so a reader who checks only that will wrongly
    /// conclude it is free to change.
    ///
    /// Both consumers walk this set to build per-dependency shapes, which land
    /// in an ident-keyed map (first-inserted-wins over distinct idents) and an
    /// ident-keyed map of tables, and `Compiler::new` sorts the table idents it
    /// lays graphical functions out from -- so iteration order does not reach
    /// the emitted fragment. The rule this upholds is the reason to keep it: a
    /// query's intermediate state must be a function of its inputs, not of the
    /// hash seed. That is the same rule `db::assemble::temp_sizes_by_id`
    /// upholds, and the class of defect it prevents (GH #595) does not
    /// announce itself -- it surfaces as salsa backdating quietly failing or a
    /// compiled artifact that is not reproducible run to run, neither of which
    /// a test would attribute back to here. It costs nothing, and the explicit
    /// and implicit constructors (`db::var_fragment`, `db::fragment_compile`)
    /// walk a `BTreeSet` too.
    ///
    /// This does NOT explain the separately-reported nondeterministic
    /// *invalidation* of `compile_ltm_var_fragment`: salsa verifies a
    /// dependency SET, which an ordering cannot alter.
    dep_idents: BTreeSet<Ident<Canonical>>,
    /// `classify_dependencies(..).referenced_tables` of the same AST.
    referenced_tables: BTreeSet<String>,
}

/// `true` when the lowered AST contains a construct whose compilation
/// consumes the Expr2 `ArrayBounds` that only the dependency-aware
/// lowering scope can recover -- i.e. a Pass-1 temp-decomposition site.
///
/// This is [`lower_ltm_variable`]'s gate for the scoped re-lower, and it
/// must be sound against `ast::expr3`'s Pass-1 decomposition set -- NOT
/// the agg-hoistable reducer set (`ltm_agg::reducer_kind_from_name`),
/// which differs: `SIZE` is never hoisted into an agg (its link score is
/// constant 0) yet Pass-1 decomposes its argument exactly like `SUM`'s, and
/// `RANK` -- array-valued, routed through its own LTM agg path (GH #776) --
/// has a Pass-1-decomposed array argument too (`ArgKind::Array`, GH #995).
/// Deriving the original (text-scan) gate from
/// the wrong set silently stubbed any fragment embedding
/// `SIZE(<array expression>)` -- the demonstrated GH #738 round-2
/// regression, pinned by
/// `ltm_array_agg::size_reducer_previous_helper_compiles_and_is_correct`.
fn ast_contains_pass1_decomposition_site(ast: &crate::ast::Ast<crate::ast::Expr2>) -> bool {
    use crate::ast::Ast;
    match ast {
        Ast::Scalar(e) | Ast::ApplyToAll(_, e) => expr_contains_pass1_decomposition_site(e),
        Ast::Arrayed(_, elements, default, _) => {
            elements
                .values()
                .any(expr_contains_pass1_decomposition_site)
                || default
                    .as_ref()
                    .is_some_and(expr_contains_pass1_decomposition_site)
        }
    }
}

/// Expression-level walk for [`ast_contains_pass1_decomposition_site`].
///
/// Sound BY CONSTRUCTION: a Pass-1 decomposition site is a builtin with a
/// non-scalar argument position in the signature table
/// (`BuiltinFn::arg_kinds`) -- an array operand, which
/// `transform_builtin_inner`'s `maybe_decompose_array_arg_inner` turns into an
/// `AssignTemp` (`SUM` / 1-arg `MEAN` / `STDDEV` / `SIZE` / 1-arg `MIN` / 1-arg
/// `MAX` / `RANK` / `VECTOR SELECT` / `VECTOR ELM MAP` / `VECTOR SORT ORDER` /
/// `ALLOCATE AVAILABLE` / `ALLOCATE BY PRIORITY`), or a lookup's table
/// position, which `transform_inner`'s arrayed-GF apply decomposition reads (a
/// LOOKUP-family call whose *table* operand carries multi-element bounds;
/// flagged for every lookup since the table's arrayedness is exactly what the
/// recovered bounds determine). Pass 1 reads the same kinds, so the gate
/// cannot drift from it, and a new `BuiltinFn` variant is classified in the
/// table or fails to compile there.
/// `pass1_gate_covers_each_decomposition_builtin` pins the classification.
///
/// The one bounds consumer deliberately NOT gated on is the non-A2A Op2
/// dimension-reordering pass (`compiler::context`'s Op2 lowering): it
/// requires a whole-array Op2 *result* outside any reducer, which in a
/// scalar LTM equation is ill-typed under either lowering, and in an
/// A2A/per-element LTM equation is unreachable (per-element expansion
/// lowers with `active_dimension` set, which skips the pass). A gated-out
/// fragment therefore compiles byte-identically to its empty-scope
/// (pre-GH #738) lowering.
fn expr_contains_pass1_decomposition_site(expr: &crate::ast::Expr2) -> bool {
    use crate::ast::{Expr2, IndexExpr2};
    match expr {
        Expr2::Const(..) | Expr2::Var(..) => false,
        Expr2::Subscript(_, indices, _, _) => indices.iter().any(|idx| match idx {
            IndexExpr2::Expr(e) => expr_contains_pass1_decomposition_site(e),
            IndexExpr2::Range(l, r, _) => {
                expr_contains_pass1_decomposition_site(l)
                    || expr_contains_pass1_decomposition_site(r)
            }
            IndexExpr2::Wildcard(_)
            | IndexExpr2::StarRange(_, _)
            | IndexExpr2::DimPosition(_, _) => false,
        }),
        Expr2::App(builtin, _, _) => {
            use crate::builtins::ArgKind;
            if builtin
                .arg_kinds()
                .iter()
                .any(|kind| !matches!(kind, ArgKind::Scalar))
            {
                return true;
            }
            // A decomposition site can hide anywhere in a non-decomposing
            // builtin's arguments (`ABS(SUM(a[*] * 2))`).
            builtin
                .args()
                .into_iter()
                .any(expr_contains_pass1_decomposition_site)
        }
        Expr2::Op1(_, e, _, _) => expr_contains_pass1_decomposition_site(e),
        Expr2::Op2(_, l, r, _, _) => {
            expr_contains_pass1_decomposition_site(l) || expr_contains_pass1_decomposition_site(r)
        }
        Expr2::If(c, t, f, _, _) => {
            expr_contains_pass1_decomposition_site(c)
                || expr_contains_pass1_decomposition_site(t)
                || expr_contains_pass1_decomposition_site(f)
        }
    }
}

/// Lower a parsed LTM Stage0 variable with a lowering scope that can
/// resolve the dimensions of its model-variable dependencies (GH #738).
///
/// Expr1 -> Expr2 lowering computes each subexpression's `ArrayBounds` via
/// `ArrayContext::get_dimensions`, which reads `ScopeStage0.models`. Pass-1
/// temp decomposition (`Pass1Context::needs_decomposition`) gates on those
/// bounds: a reducer over an array *expression* (`SUM(pop[*] * scale)`) is
/// hoisted into an `AssignTemp` only when the Op2 carries them. With an
/// empty scope the bounds are never computed, the array expression stays
/// inline under the reducer, and codegen rejects the fragment ("Cannot push
/// view for expression type ..."), silently stubbing the LTM variable to a
/// constant 0. Mirrors `explicit_fragment_input`'s minimal-`ModelStage0`
/// construction for ordinary per-variable fragments.
///
/// Strategy: lower once with an empty scope (cheap, and byte-identical to
/// the populated-scope lowering when no dependency is arrayed -- the scope
/// only feeds `get_dimensions`, which returns `None` for scalars either
/// way); only when the lowered AST contains a Pass-1 temp-decomposition
/// site ([`ast_contains_pass1_decomposition_site`]) AND an arrayed
/// dependency is present, re-lower with a scope carrying the parsed Stage0
/// variables of self plus the deps. The dependency identifier set is
/// scope-independent (the scope affects only bounds metadata), so the
/// classification computed on the preliminary lowering is returned
/// alongside whichever lowering wins.
///
/// An arrayed dependency can be a model source variable OR an arrayed
/// implicit helper aux synthesized while parsing an LTM equation (the GH
/// #541 `PREVIOUS(<bare arrayed name>)` capture, which a ceteris-paribus
/// link score references inside its reducer). `equation_implicits` carries
/// the implicits from the caller's own parse; cross-equation helper refs
/// resolve through the cached `model_ltm_implicit_var_info` registry.
///
/// Boundary: dependencies that are neither model source variables nor LTM
/// parse-time implicit helpers stay OUTSIDE the lowering scope and lower
/// with unresolved (scalar) bounds, exactly as before GH #738. That
/// notably includes other LTM *synthetic* variables -- e.g. an A2A link
/// score referenced by a loop score -- which is sound because loop and
/// relative-score equations reference those deps only in plain products,
/// never inside reducers; their multi-slot layout is handled separately by
/// the compile stage's dimension-aware dependency shapes (the LTM-var dep
/// branch in `ltm_fragment_input`, tech-debt #34). `·`-dotted
/// module-output refs likewise stay outside (they are not flat variables).
fn lower_ltm_variable(
    db: &dyn Db,
    parsed_variable: &crate::model::VariableStage0,
    equation_implicits: &[datamodel::Variable],
    model: SourceModel,
    project: SourceProject,
) -> LoweredLtmVariable {
    let dim_context = project_dimensions_context(db, project);
    let empty_models = HashMap::new();
    let empty_scope = crate::model::ScopeStage0 {
        models: &empty_models,
        dimensions: dim_context,
        model_name: "",
    };
    let prelim = crate::model::lower_variable(&empty_scope, parsed_variable);

    // Classify dependencies ONCE on the preliminary lowering; the set is
    // scope-independent, so it serves both the re-lower decision below and
    // the caller's dependency-shape construction. `Variable::ast()` is the
    // right (and only needed) source: every LTM Stage0 input here is an
    // Aux-parsed Var whose dt AST is its sole AST, and even a hypothetical
    // stock-shaped input is covered because `ast()` returns a Stock's init
    // AST.
    let classification = prelim
        .ast()
        .map(|ast| crate::variable::classify_dependencies(ast, &[], None));
    let (dep_idents, referenced_tables) = match classification {
        Some(c) => (c.all.into_iter().collect(), c.referenced_tables),
        None => (BTreeSet::new(), BTreeSet::new()),
    };

    // Structural gate: without a Pass-1 temp-decomposition site in the
    // lowered AST, the Expr2 bounds the scoped re-lower would recover
    // cannot change the compile outcome -- skip the per-dep arrayedness
    // lookups and the second lowering entirely (the common case: most
    // link/loop scores contain no reducer even on heavily arrayed models).
    if !prelim
        .ast()
        .is_some_and(ast_contains_pass1_decomposition_site)
    {
        return LoweredLtmVariable {
            variable: prelim,
            dep_idents,
            referenced_tables,
        };
    }

    // Dependencies of the LTM equation (data-flow deps plus referenced
    // lookup tables -- an arrayed graphical function's per-element apply
    // also needs its dimensions resolved). `·`-dotted module-output refs
    // are not flat variables and keep resolving to scalar (None) exactly
    // as before.
    let mut dep_names: BTreeSet<&str> = BTreeSet::new();
    for dep in dep_idents
        .iter()
        .map(|d| d.as_str())
        .chain(referenced_tables.iter().map(|s| s.as_str()))
    {
        let effective = dep.strip_prefix('\u{00B7}').unwrap_or(dep);
        if !effective.contains('\u{00B7}') {
            dep_names.insert(effective);
        }
    }

    let source_vars = model.variables(db);
    let ltm_implicit_info = model_ltm_implicit_var_info(db, model, project);
    // Resolve a dep that is an LTM-parse-time implicit helper aux to its
    // datamodel form (modules are scalar nodes in equations; only helper
    // auxes can be arrayed).
    let find_implicit_dm = |name: &str| -> Option<&datamodel::Variable> {
        equation_implicits
            .iter()
            .find(|v| canonicalize(v.get_ident()) == name)
            .or_else(|| {
                ltm_implicit_info
                    .get(name)
                    .filter(|meta| !meta.is_module)
                    .map(|meta| &meta.variable)
            })
    };
    let dm_var_is_arrayed = |v: &datamodel::Variable| {
        matches!(
            v.get_equation(),
            Some(datamodel::Equation::ApplyToAll(..) | datamodel::Equation::Arrayed(..))
        )
    };
    // An ARRAYED sibling LTM var referenced as a dep -- today that is the
    // GH #995 freeze helper, a whole-array operand of a vector builtin
    // (`VECTOR SELECT("$⁚ltm⁚freeze⁚…", …)`). The Pass-1 temp decomposition
    // below can only materialize a computed array argument (`helper * k`) if
    // the lowering scope knows the helper's dims; without them the reference
    // lowers as a scalar and codegen rejects the fragment ("expected array
    // expression"). Same registry lookup (and the same safety argument) as
    // the LTM-var dep branch in `ltm_fragment_input`: this runs from
    // fragment compilation, strictly after `model_ltm_variables` completed.
    let find_arrayed_ltm_dep = |name: &str| -> Option<Vec<String>> {
        let idx = *model_ltm_var_name_index(db, model, project).get(name)?;
        let lsv = &model_ltm_variables(db, model, project).vars[idx];
        (!lsv.dimensions.is_empty()).then(|| lsv.dimensions.clone())
    };

    let any_arrayed_dep = dep_names.iter().any(|name| {
        source_vars
            .get(*name)
            .is_some_and(|sv| !variable_dimensions(db, *sv, project).is_empty())
            || find_implicit_dm(name).is_some_and(dm_var_is_arrayed)
            || find_arrayed_ltm_dep(name).is_some()
    });
    if !any_arrayed_dep {
        return LoweredLtmVariable {
            variable: prelim,
            dep_idents,
            referenced_tables,
        };
    }

    let model_name_str = model.name(db);
    let module_ctx = model_module_ident_context(db, model, project, vec![]);
    let dim_ctx = project_dimensions_context(db, project);
    let units_ctx = project_units_context(db, project);
    let mut stage0_vars: HashMap<Ident<Canonical>, crate::model::VariableStage0> = HashMap::new();
    stage0_vars.insert(Ident::new(parsed_variable.ident()), parsed_variable.clone());
    for dep_name in &dep_names {
        if let Some(dep_sv) = source_vars.get(*dep_name) {
            let dep_parsed =
                parse_source_variable_with_module_context(db, *dep_sv, project, module_ctx);
            stage0_vars.insert(Ident::new(dep_name), dep_parsed.variable.clone());
        } else if let Some(implicit_dm) = find_implicit_dm(dep_name) {
            // Nested implicits of an implicit are registered (and compiled)
            // in their own right; here only the dep's own dimensions matter.
            let mut nested = Vec::new();
            let dep_parsed =
                crate::variable::parse_var(dim_ctx, implicit_dm, &mut nested, units_ctx, |mi| {
                    Ok(Some(mi.clone()))
                });
            stage0_vars.insert(Ident::new(dep_name), dep_parsed);
        } else if let Some(ltm_dims) = find_arrayed_ltm_dep(dep_name) {
            // An arrayed sibling LTM var (the GH #995 freeze helper): a
            // zero-bodied dims-only stub -- only the dep's dimensions matter
            // to the lowering, exactly like the implicit branch above.
            let stub = datamodel::Variable::Aux(datamodel::Aux {
                ident: (*dep_name).to_string(),
                equation: datamodel::Equation::ApplyToAll(ltm_dims, "0".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            });
            let mut nested = Vec::new();
            let dep_parsed =
                crate::variable::parse_var(dim_ctx, &stub, &mut nested, units_ctx, |mi| {
                    Ok(Some(mi.clone()))
                });
            stage0_vars.insert(Ident::new(dep_name), dep_parsed);
        }
    }

    let mini_model = crate::model::ModelStage0 {
        ident: Ident::new(model_name_str),
        display_name: model_name_str.to_string(),
        variables: stage0_vars,
        implicit: false,
        // Single-variable fragment lowering only; not a macro template.
        is_macro: false,
        macro_params: vec![],
    };
    let mut models: HashMap<Ident<Canonical>, &crate::model::ModelStage0> = HashMap::new();
    models.insert(Ident::new(model_name_str), &mini_model);
    let scope = crate::model::ScopeStage0 {
        models: &models,
        dimensions: dim_context,
        model_name: model_name_str,
    };
    LoweredLtmVariable {
        variable: crate::model::lower_variable(&scope, parsed_variable),
        dep_idents,
        referenced_tables,
    }
}

/// Build the fragment input of one LTM synthetic variable: parse its typed
/// equation (running the SAME implicit-module / PREVIOUS-INIT visitor the
/// ordinary variable parse runs), lower it with a scope that can resolve its
/// arrayed dependencies' bounds (GH #738), and resolve the shape of every name
/// it references. `Err` carries the reason the generated equation did not
/// parse.
///
/// LTM equations are scalar (or A2A) aux equations that may reference model
/// variables from the parent model, other LTM variables (a loop score reads
/// link scores; an A2A loop score must see an A2A link score's dimensions so
/// the compiler emits per-element fetches rather than collapsing every slot to
/// slot 0, tech-debt #34), implicit helper/module variables synthesized while
/// parsing this or another LTM equation (an ARRAYED capture helper -- the GH
/// #541 arrayed `PREVIOUS`/`INIT` capture, extended to array-valued builtin
/// subtrees like `rank(pop, 1)` by GH #742 -- needs its dimensions so the
/// consuming `helper[dim·elem]` subscript resolves), the model's own
/// SMOOTH/DELAY instances and capture helpers, and the implicit time globals.
/// A name that is none of those resolves to a scalar shape, so the reference
/// compiles (to a bare slot read) and assembly's `fragment_vars_in_layout`
/// filter decides whether it is in this model's layout -- the sub-model
/// stdlib-instance case.
///
/// The variant of `equation` determines the variable's slot count: a
/// `Scalar` equation gets 1 slot; an `ApplyToAll`/`Arrayed` equation
/// gets `product(dim_lengths)` slots and is compiled with the A2A /
/// per-element expansion the compiler applies to those variants.
pub(crate) fn ltm_fragment_input<'db>(
    db: &'db dyn Db,
    var_name: &str,
    equation: &LtmEquation,
    model: SourceModel,
    project: SourceProject,
) -> Result<FragmentInput<'db>, String> {
    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);

    let module_idents = ltm_module_idents(db, model, project);
    let model_var_names = super::ltm_model_var_names(db, model, project);

    let parsed = parse_ltm_equation(
        var_name,
        equation,
        dim_context,
        Some(module_idents),
        Some(model_var_names),
    );
    if let Some(errs) = parsed.variable.equation_errors()
        && !errs.is_empty()
    {
        return Err(format!(
            "the generated equation did not parse: {}",
            errs.iter()
                .map(|e| format!("{:?} at {}..{}", e.code, e.start, e.end))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }

    // `lower_ltm_variable` threads the dependencies (model variables and
    // arrayed parse-time helpers) into the lowering scope so array bounds
    // resolve (GH #738), and hands back the dependency classification it
    // computed so the lowered AST is not walked again here.
    let LoweredLtmVariable {
        variable: lowered,
        dep_idents,
        referenced_tables,
    } = lower_ltm_variable(db, &parsed.variable, &parsed.implicit_vars, model, project);

    let var_name_canonical = canonicalize(var_name).into_owned();
    let var_ident: Ident<Canonical> = Ident::new(&var_name_canonical);
    let source_vars = model.variables(db);
    let implicit_info = model_implicit_var_info(db, model, project);
    let ltm_implicit_info = model_ltm_implicit_var_info(db, model, project);

    // A helper this equation's own parse synthesized, by canonical name.
    let parsed_implicit = |name: &str| {
        parsed
            .implicit_vars
            .iter()
            .find(|v| canonicalize(v.get_ident()) == name)
    };
    let helper_dims = |helper: &datamodel::Variable| match helper.get_equation() {
        Some(
            datamodel::Equation::ApplyToAll(dim_names, _)
            | datamodel::Equation::Arrayed(dim_names, _, _, _),
        ) => dimensions_named(dim_names, dim_context),
        _ => Vec::new(),
    };

    let mut deps: IdentMap<Ident<Canonical>, DepShape> = Default::default();
    deps.insert(
        var_ident,
        DepShape::var(
            lowered
                .get_dimensions()
                .map(<[crate::dimensions::Dimension]>::to_vec)
                .unwrap_or_default(),
        ),
    );
    for dep in &dep_idents {
        let (head, _qualified) = dep_head(dep.as_str());
        if head == var_name_canonical || is_implicit_global(head) || deps.contains_key(head) {
            continue;
        }
        let shape = if let Some(sv) = source_vars.get(head) {
            source_dep_shape(db, *sv, project)
        } else if let Some(helper) = parsed_implicit(head) {
            match helper {
                datamodel::Variable::Module(dm_module) => {
                    module_dep_shape(db, project, &dm_module.model_name)
                }
                _ => DepShape::var(helper_dims(helper)),
            }
        } else if let Some(meta) = implicit_info.get(head) {
            implicit_dep_shape(db, project, meta)
        } else if let Some(meta) = ltm_implicit_info.get(head) {
            if meta.is_module {
                module_dep_shape(db, project, meta.model_name.as_deref().unwrap_or(""))
            } else {
                DepShape::var(helper_dims(&meta.variable))
            }
        } else if let Some(&idx) = model_ltm_var_name_index(db, model, project).get(head) {
            // Another LTM synthetic variable. The indexed lookup matters: most
            // unresolved deps here are PREVIOUS-helper names that are NOT LTM
            // vars, and a linear scan over all LTM vars per dep was O(N^2)
            // across a model's compile (~145k lookups over 6.7k vars on
            // C-LEARN).
            let lsv = &model_ltm_variables(db, model, project).vars[idx];
            DepShape::var(dimensions_named(&lsv.dimensions, dim_context))
        } else {
            DepShape::var(Vec::new())
        };
        deps.insert(Ident::new(head), shape);
    }

    // Lookup-table references (issue #606): a `LOOKUP(table, x)` call's table
    // argument is not a data-flow dep, but the fragment still needs (a) the
    // table's shape so lowering resolves the table ident, and (b) the table's
    // graphical-function data so the Lookup opcode gets a base_gf. Without
    // both, the fragment fails to compile and the link score silently reads a
    // constant 0 -- the failure mode behind WRLD3's identically-zero
    // table-mediated link scores (food_per_capita -> lifetime_multiplier_from_food
    // and 50+ siblings). Module-namespaced tables can't be referenced from LTM
    // equations.
    let mut tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>> = HashMap::new();
    for table_name in &referenced_tables {
        let (head, qualified) = dep_head(table_name);
        if qualified {
            continue;
        }
        let Some(table_sv) = source_vars.get(head) else {
            continue;
        };
        let table_data = extract_tables_from_source_var(db, table_sv, project);
        if !table_data.is_empty() {
            tables.insert(Ident::new(head), table_data);
        }
        deps.entry(Ident::new(head))
            .or_insert_with(|| source_dep_shape(db, *table_sv, project));
    }

    Ok(FragmentInput::new(
        lowered,
        deps,
        tables,
        BTreeSet::new(),
        Ident::new(model.name(db)),
        converted_dims,
        dim_context,
    ))
}

/// Compile an arbitrary LTM `Equation` to symbolic bytecodes: its
/// [`ltm_fragment_input`], lowered and emitted through the same emission
/// entry point the explicit and implicit paths use.
///
/// Shared implementation used by `compile_ltm_var_fragment` (link scores)
/// and the loop/relative score compilation in `assemble_module`.
///
/// `why`, when supplied, receives a human-readable reason on failure. The
/// three ways this function can fail -- a parse error in the generated text,
/// a lowering `Err`, and codegen declining to emit -- otherwise all collapse
/// into the same `None`/`flow_bytecodes: None`, so a caller reporting the
/// failure could say only *that* it happened. That cost was concrete: ~1,600
/// failures on one real model with no way to tell which construct was
/// responsible short of instrumenting this function by hand. Callers that only
/// want the fragment pass `None` and pay nothing.
pub(crate) fn compile_ltm_equation_fragment(
    db: &dyn Db,
    var_name: &str,
    equation: &LtmEquation,
    model: SourceModel,
    project: SourceProject,
    mut why: Option<&mut Option<String>>,
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::CompiledVarFragment;

    #[cfg(test)]
    crate::db::note_fragment_execution(crate::db::FragmentExecKind::LtmBody, var_name);

    let input = match ltm_fragment_input(db, var_name, equation, model, project) {
        Ok(input) => input,
        Err(reason) => {
            if let Some(slot) = why.as_deref_mut() {
                *slot = Some(reason);
            }
            return None;
        }
    };

    // LTM vars are always flow-phase only (scalar auxes, not stocks)
    let flow_bytecodes = match lower_fragment(&input, false) {
        Ok(var_result) => {
            if why.is_some() {
                match crate::db::assemble::compile_phase_to_per_var_bytecodes_reporting(
                    &input.emit_ctx(),
                    &var_result.ast,
                ) {
                    Ok(bytecodes) => Some(bytecodes),
                    Err(err) => {
                        if let Some(slot) = why.as_deref_mut() {
                            *slot = Some(err);
                        }
                        None
                    }
                }
            } else {
                compile_phase_to_per_var_bytecodes(&input.emit_ctx(), &var_result.ast)
            }
        }
        Err(err) => {
            if let Some(slot) = why {
                *slot = Some(lowering_failure_reason(&input, &err));
            }
            None
        }
    };

    Some(VarFragmentResult {
        fragment: CompiledVarFragment {
            ident: canonicalize(var_name).into_owned(),
            initial_bytecodes: None,
            flow_bytecodes,
            stock_bytecodes: None,
        },
        // LTM synthetic vars use PREVIOUS -- always dynamic; not classified
        // for run-invariance.
        flow_invariance: None,
    })
}

/// The `why` text for a phase that `lower_fragment` refused.
///
/// A lowering `Err` of `empty_equation` is usually a MASK: the variable
/// reached lowering with no AST because the earlier scope-lowering rejected
/// the equation and left `ast: None`. Report that upstream rejection when it
/// is there, since "empty equation" describes a formula that was printed in
/// full.
fn lowering_failure_reason(input: &FragmentInput<'_>, err: &crate::common::Error) -> String {
    let lowered_errs = input.target.equation_errors().unwrap_or_default();
    if lowered_errs.is_empty() {
        format!("could not be lowered: {err}")
    } else {
        format!(
            "could not be lowered: {}",
            lowered_errs
                .iter()
                .map(|e| format!("{:?} at {}..{}", e.code, e.start, e.end))
                .collect::<Vec<_>>()
                .join("; ")
        )
    }
}

/// Select-and-compile a single LTM synthetic variable's flow-phase
/// fragment, exactly as `assemble_module`'s LTM pass does.
///
/// Most synthetic equations are compiled verbatim from `ltm_var.equation`
/// (`compile_direct`); the one exception is the standard scalar Bare
/// `from→to` link score, which routes through the salsa-cached
/// `(from, to)`-keyed `compile_ltm_var_fragment` (itself sourced from the
/// per-shape `link_score_equation_text_shaped(.., Bare)` query) so an equation
/// edit that does not change the dependency set reuses the cached fragment.
///
/// Returns `None` -- or a `VarFragmentResult` whose `flow_bytecodes` is
/// `None` -- when the synthetic equation fails to parse or compile.
/// `assemble_module` silently drops such failures (the variable keeps its
/// layout slot but no bytecode writes it, so it reads a constant 0);
/// [`model_ltm_fragment_diagnostics`] calls this to detect those failures
/// and surface them as `Warning`s instead of letting them masquerade as
/// a correct zero score.
pub(crate) fn compile_ltm_synthetic_fragment(
    db: &dyn Db,
    ltm_var: &LtmSyntheticVar,
    model: SourceModel,
    project: SourceProject,
) -> Option<VarFragmentResult> {
    // GH #547: a test-scoped forced failure, so the fragment-diagnostics
    // positive tests exercise the diagnostic pass without depending on a
    // real fragment-compile bug existing (every such bug eventually gets
    // fixed, which used to break the positive fixture).
    #[cfg(test)]
    {
        let forced = LTM_FRAGMENT_FAILURE_OVERRIDE.with(|c| {
            c.borrow()
                .as_deref()
                .is_some_and(|pat| ltm_var.name.contains(pat))
        });
        if forced {
            return None;
        }
    }
    // Compile this LTM var's already-prepared equation verbatim.
    // Used for everything except the standard scalar Bare `from→to`
    // link score, which goes through the salsa-cached
    // `compile_ltm_var_fragment` path below: that path re-derives the
    // equation from `link_score_equation_text_shaped(.., Bare)` (always scalar,
    // Bare; per-shape dimensions, element subscripts and reducer
    // substitutions are applied later in `model_ltm_variables`), so
    // for anything that carries those it would produce the wrong (or
    // a degenerate) fragment.
    let compile_direct = || {
        compile_ltm_equation_fragment(db, &ltm_var.name, &ltm_var.equation, model, project, None)
    };
    const LINK_SCORE_PREFIX: &str = "$\u{205A}ltm\u{205A}link_score\u{205A}";
    if ltm_var.name.starts_with(LINK_SCORE_PREFIX) {
        if ltm_var.dimensions.is_empty() {
            // Scalar link score. Sub-cases:
            // (a) Standard scalar Bare score (from→to): use salsa-cached fragment.
            // (b) Cross-dimensional per-source-element score (from[elem]→to,
            //     try_cross_dimensional_link_scores) or per-target-element
            //     score (from→to[elem], try_scalar_to_arrayed_link_scores):
            //     compile directly. The equation is unique per element and the
            //     (from, to)-keyed salsa path can't round-trip the bracketed
            //     name back to a user variable (it'd drop the fragment and stub
            //     the var to zero).
            // (d) Aggregate-node link score (from = $⁚ltm⁚agg⁚n, or to =
            //     $⁚ltm⁚agg⁚n): compile directly. The (from, to)-keyed salsa
            //     path would `reconstruct_single_variable` the synthetic agg
            //     name, get `None`, and emit a degenerate ceteris-paribus
            //     equation against the *target's* original (reducer-bearing)
            //     equation -- which the agg name appears nowhere in -- so the
            //     numerator collapses to zero. `model_ltm_variables` already
            //     produced the correct reducer-substituted equation in
            //     `ltm_var.equation`; use it verbatim.
            // (e) Non-Bare-shaped scalar score (`Wildcard`/`DynamicIndex`
            //     reference into a scalar target, e.g. `total = arr[idx]`):
            //     `emit_per_shape_link_scores` set `compile_directly` because
            //     the salsa path re-derives with `RefShape::Bare`, wrapping
            //     the subscript in `PREVIOUS()` and zeroing the numerator.
            let suffix = &ltm_var.name[LINK_SCORE_PREFIX.len()..];
            let arrow_pos = suffix.find('\u{2192}');
            let from_to: Option<(&str, &str)> =
                arrow_pos.map(|arrow| (&suffix[..arrow], &suffix[arrow + '\u{2192}'.len_utf8()..]));
            // Any `[` -- on either side of the arrow -- marks an
            // element-pinned equation that ltm_var.equation already
            // carries verbatim; the (from, to)-keyed salsa path can't
            // round-trip the bracketed name back to a user variable.
            let has_element_subscript = suffix.contains('[');
            let touches_synthetic_agg = from_to.is_some_and(|(from_name, to_name)| {
                crate::ltm_agg::is_synthetic_agg_name(from_name)
                    || crate::ltm_agg::is_synthetic_agg_name(to_name)
            });

            if has_element_subscript || touches_synthetic_agg || ltm_var.compile_directly {
                compile_direct()
            } else if let Some((from_name, to_name)) = from_to {
                let link_id = LtmLinkId::new(db, from_name.to_string(), to_name.to_string());
                compile_ltm_var_fragment(db, link_id, model, project)
                    .as_ref()
                    .cloned()
            } else {
                compile_direct()
            }
        } else {
            // A2A link score: the equation is the dimension-tagged
            // ApplyToAll/Arrayed variant, not the scalar one the
            // salsa-cached path would re-derive.
            compile_direct()
        }
    } else {
        // Loop scores and relative loop scores.
        compile_direct()
    }
}

/// The salsa-memoized entry point for one LTM synthetic variable's fragment,
/// keyed by its INDEX into `model_ltm_variables(..).vars`.
///
/// [`compile_ltm_synthetic_fragment`] routes only the scalar `Bare` `from->to`
/// score through a memoized query ([`compile_ltm_var_fragment`], keyed by the
/// link); every element-pinned, aggregate-touching, A2A or loop score takes the
/// plain-function `compile_direct` path. Both walkers over the variable list --
/// `assemble_module`'s pass 3 and [`model_ltm_fragment_diagnostics`] -- then
/// compiled those from scratch, independently. On C-LEARN that is 5,985 of
/// 7,125 variables, roughly half of a full compile stage, paid a second time on
/// every `simlin_project_get_errors` / MCP `read_model`, and twice more on every
/// MCP `edit_model` (which runs a pre- and a post-edit diagnostic pass).
///
/// Keyed by INDEX rather than by name because the index is what both walkers
/// already have, and because it keeps this a salsa FIREWALL: the query reads
/// the whole-model `model_ltm_variables`, so it re-executes on any edit, but its
/// VALUE is one fragment -- so salsa backdates it whenever that variable's
/// fragment is unchanged and `assemble_module` is not re-run. Same shape, and
/// the same reason, as `reconstruct_named_variable` over
/// `reconstruct_model_variables`.
///
/// An out-of-range index yields `None`, which is also what a variable whose
/// fragment failed to compile yields; callers treat both as "no fragment",
/// exactly as they treated a `None` from the direct path.
///
/// PRIVATE on purpose: [`compile_ltm_fragment_for`] is the only way in, so the
/// index-to-variable coupling is checked at every call site rather than relied
/// on. Widening this back to `pub(crate)` re-opens the hole that wrapper exists
/// to close.
#[salsa::tracked(returns(ref))]
fn compile_ltm_fragment_at(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    index: usize,
) -> Option<VarFragmentResult> {
    let ltm_vars = model_ltm_variables(db, model, project);
    let ltm_var = ltm_vars.vars.get(index)?;
    compile_ltm_synthetic_fragment(db, ltm_var, model, project)
}

/// [`compile_ltm_fragment_at`] plus a debug-only check that `index` still names
/// the variable the caller believes it does.
///
/// The index IS the identity, deliberately: a name argument would join the
/// salsa cache key and defeat the firewall the query's rustdoc describes. But
/// nothing in the signature or the types ties a caller's `index` to the
/// `LtmSyntheticVar` it walked it out of, so a third caller -- or any
/// reordering of `LtmVariablesResult::vars` between the walk and the call --
/// would file a fragment under the wrong name, and both consumers treat a
/// mismatch as an ordinary "no fragment" rather than as an error. Nothing would
/// report it.
///
/// Both callers already hold the variable, so they can pay a debug-only
/// assertion and make the coupling CHECKABLE rather than conventional. The
/// check costs nothing in release, and the query keeps its index-only key.
// `expected` is read only by the debug assertion below, so a release build
// sees it as unused. Keep it in the signature regardless: it is what forces a
// caller to have the variable in hand, which is the coupling being checked.
#[cfg_attr(not(debug_assertions), allow(unused_variables))]
pub(crate) fn compile_ltm_fragment_for<'db>(
    db: &'db dyn Db,
    model: SourceModel,
    project: SourceProject,
    index: usize,
    expected: &LtmSyntheticVar,
) -> &'db Option<VarFragmentResult> {
    #[cfg(debug_assertions)]
    {
        let resolved = model_ltm_variables(db, model, project)
            .vars
            .get(index)
            .map(|v| v.name.as_str());
        debug_assert_eq!(
            resolved,
            Some(expected.name.as_str()),
            "compile_ltm_fragment_at is keyed by index alone, so a caller's \
             index and its LtmSyntheticVar must come from the same walk of the \
             same `vars` vector; index {index} resolves to {resolved:?}"
        );
    }
    compile_ltm_fragment_at(db, model, project, index)
}

#[cfg(test)]
thread_local! {
    /// Test-only forced-failure pattern for
    /// [`compile_ltm_synthetic_fragment`] and (GH #741)
    /// [`compile_ltm_implicit_var_fragment`], scoped by an active
    /// [`LtmFragmentFailureGuard`] (GH #547): any LTM synthetic variable or
    /// implicit helper whose (canonical) name contains the pattern is
    /// treated as a compile failure (`None`), so the positive tests for
    /// [`model_ltm_fragment_diagnostics`] are decoupled from the lifetime
    /// of any real fragment-compile bug. Mirrors `AGG_LOOP_BUDGET_OVERRIDE`
    /// in `db/ltm/loops.rs`.
    static LTM_FRAGMENT_FAILURE_OVERRIDE: std::cell::RefCell<Option<String>> =
        const { std::cell::RefCell::new(None) };
}

/// RAII guard (test-only) that forces [`compile_ltm_synthetic_fragment`] and
/// [`compile_ltm_implicit_var_fragment`] to fail for any synthetic variable
/// or implicit helper whose name contains `pattern`, for the current thread
/// for the guard's lifetime; the previous override is restored on drop (so a
/// panicking test does not leak it to the next test reusing the thread).
///
/// Because `model_ltm_fragment_diagnostics` (and `assemble_module`) are
/// salsa-memoized, the guard must outlive every call in the test whose
/// failures it forces, and the test must use a fresh `db` (a memoized
/// result computed under a different override would otherwise be returned
/// regardless of the guard's state). Same caveat as `AggLoopBudgetGuard`.
#[cfg(test)]
pub(crate) struct LtmFragmentFailureGuard {
    prev: Option<String>,
}

#[cfg(test)]
impl LtmFragmentFailureGuard {
    pub(crate) fn new(pattern: &str) -> Self {
        let prev = LTM_FRAGMENT_FAILURE_OVERRIDE.with(|c| c.replace(Some(pattern.to_string())));
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for LtmFragmentFailureGuard {
    fn drop(&mut self) {
        LTM_FRAGMENT_FAILURE_OVERRIDE.with(|c| *c.borrow_mut() = self.prev.take());
    }
}

/// Salsa-tracked diagnostic pass that compiles every LTM synthetic
/// variable -- and every LTM *implicit helper* (GH #741) -- the way
/// `assemble_module` does and emits a `Warning` for each one whose
/// fragment fails to compile.
///
/// Why this exists: `assemble_module` silently drops a synthetic
/// fragment that fails to compile -- the variable keeps its layout slot
/// but no bytecode ever writes it, so it reads a constant 0. That silent
/// stubbing masks correctness bugs in the LTM augmentation layer (an
/// arrayed flow-to-stock link score that compiled to 0 and produced
/// plausible-but-wrong loop scores went unnoticed precisely because of
/// this). Surfacing the failure makes a degraded LTM analysis *visible*
/// instead of silently wrong. The implicit helpers (the PREVIOUS/INIT
/// capture auxes `builtins_visitor::make_temp_arg` synthesizes while
/// parsing LTM equations, `$⁚$⁚ltm⁚…⁚arg{n}`) ride the exact same
/// silent-drop assembly path, and a dropped helper corrupts every link
/// score that reads it -- with, before GH #741, no diagnostic anywhere.
///
/// Severity is `Warning`, not `Error`: LTM is opt-in, the rest of the
/// model still simulates, and a hard error would break compilation of
/// every `ltm_enabled` model that hits a single bad fragment. This
/// mirrors the auto-flip-to-discovery warning in `model_ltm_variables`.
///
/// `model_all_diagnostics` drives this when `ltm_enabled`, so the
/// warning reaches `collect_all_diagnostics` exactly when the auto-flip
/// warning does. (GH #466 tracks the separate plumbing gap: the
/// diagnostic-collection FFI paths leave `ltm_enabled` false by default,
/// so neither this warning nor the auto-flip warning reaches
/// `simlin_project_get_errors` today.)
///
/// Only the layout-independent compile failure is reported here. A
/// fragment that compiles but whose variable references do not resolve
/// in the model's layout is the documented sub-model dedup case
/// (`assemble_module`'s `fragment_vars_in_layout` drop), where the root
/// model emits an equivalent fragment under qualified names -- that drop
/// is intentionally left silent.
#[salsa::tracked]
pub fn model_ltm_fragment_diagnostics(db: &dyn Db, model: SourceModel, project: SourceProject) {
    use salsa::Accumulator;

    use crate::db::{CompilationDiagnostic, Diagnostic, DiagnosticError, DiagnosticSeverity};

    let ltm_vars = model_ltm_variables(db, model, project);
    for (index, ltm_var) in ltm_vars.vars.iter().enumerate() {
        // Through the memoized per-index query, so this pass READS assembly's
        // fragments rather than compiling its own copies.
        let fragment = compile_ltm_fragment_for(db, model, project, index, ltm_var);
        // A fragment is usable only if it compiled *and* produced
        // flow-phase bytecodes. `compile_ltm_equation_fragment` returns
        // `Some(_)` with `flow_bytecodes: None` when the synthetic
        // equation parses but fails to lower or compile.
        let compiled_ok = fragment
            .as_ref()
            .is_some_and(|r| r.fragment.flow_bytecodes.is_some());
        if compiled_ok {
            continue;
        }
        // Recover WHY. `compile_ltm_synthetic_fragment` discards the reason
        // (it has three failure legs and one `None`), so re-run the direct
        // compile with the reason slot wired up. For every variable that took
        // the `compile_direct` branch above -- which is every element-pinned
        // equation, i.e. all of the arrayed-model failures this was written
        // for -- that is the identical call. For one that took the
        // salsa-cached `(from, to)` branch the re-derived equation can differ,
        // so the reason is reported as indicative rather than as the failure.
        let mut reason: Option<String> = None;
        let direct_agrees = compile_ltm_equation_fragment(
            db,
            &ltm_var.name,
            &ltm_var.equation,
            model,
            project,
            Some(&mut reason),
        )
        .is_none_or(|r| r.fragment.flow_bytecodes.is_none());
        let detail = match (&reason, direct_agrees) {
            (Some(r), true) => format!(" Reason: {r}."),
            (Some(r), false) => format!(
                " Reason (from recompiling its own equation, which DOES compile -- \
                 the cached re-derived equation is what failed): {r}."
            ),
            (None, _) => String::new(),
        };
        let msg = format!(
            "LTM synthetic variable '{}' failed to compile; it keeps a \
             layout slot but no bytecode, so it evaluates to a constant 0. \
             Any loop or link score derived from it is silently degraded. \
             This usually means the LTM augmentation layer emitted an \
             equation the compiler rejected.{}",
            ltm_var.name, detail,
        );
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: Some(ltm_var.name.clone()),
            error: DiagnosticError::Assembly(msg),
            severity: DiagnosticSeverity::Warning,
        })
        .accumulate(db);
    }

    // GH #741: probe the LTM implicit helpers the same way. `assemble_module`
    // compiles each via `compile_ltm_implicit_var_fragment` and silently
    // skips a `None` (or a fragment with no bytecode for the helper's
    // value-bearing phase), so the helper keeps its layout slot, nothing
    // writes it, and it reads a constant 0 at runtime.
    //
    // Like the synthetic-var leg above, only the COMPILE failure is reported:
    // a helper that compiles but is then dropped by assembly's layout check
    // (`fragment_vars_in_layout` in `db/assemble.rs`'s LTM-implicit loop) is
    // still silent -- the #683-class gap (absent cross-module idents), which
    // remains open for the helper leg too.
    //
    // Input-set boundary: assembly compiles each helper with the module
    // INSTANCE's input names. This pass is keyed per (model, project) -- no
    // instance context -- so it probes with the empty input set, mirroring
    // `model_all_diagnostics`' `compile_var_fragment` probe ("module inputs
    // are empty because we are not in an assembly context"). For the ROOT
    // model assembly's input set IS empty, so the probe is byte-identical to
    // assembly there. For a sub-model instance with inputs the probe is an
    // approximation, but compile success cannot diverge: the input set only
    // flips how a resolved name is loaded (`ModuleInput` slot vs a slot
    // read -- every dependency has a shape in the fragment's input either
    // way), never whether the equation compiles.
    //
    // Iteration is name-sorted so warning order is deterministic, matching
    // the assembly loop.
    let ltm_implicit = model_ltm_implicit_var_info(db, model, project);
    if ltm_implicit.is_empty() {
        return;
    }
    let mut implicit_names: Vec<&String> = ltm_implicit.keys().collect();
    implicit_names.sort();
    for im_name in implicit_names {
        let meta = &ltm_implicit[im_name];
        let mut helper_reason: Option<String> = None;
        let fragment = compile_ltm_implicit_var_fragment(
            db,
            meta,
            model,
            project,
            &[],
            Some(&mut helper_reason),
        );
        // The helper's value-bearing phase must have produced bytecode:
        // `compile_ltm_implicit_var_fragment` returns `Some` even when every
        // phase failed (each phase is compiled independently and a failed one
        // is just `None` in the fragment), and `assemble_module` appends only
        // the phases that exist to the runlists. A plain aux helper (the
        // PREVIOUS-capture case, the only kind LTM parsing produces today) is
        // recomputed each step via its flow bytecode; a stock or module
        // helper is advanced via its stock bytecode.
        //
        // Defense-in-depth boundary: this is deliberately blind to the INIT
        // phase. A helper whose flow phase compiles while its init phase
        // fails would pass unchecked and `PREVIOUS(helper)` would read 0 at
        // t=0 only. Both phases compile from the same lowered equation, so a
        // divergent failure is likely unreachable; if one ever surfaces,
        // extend this check to `initial_bytecodes`.
        let compiled_ok = fragment.as_ref().is_some_and(|r| {
            if meta.is_stock || meta.is_module {
                r.fragment.stock_bytecodes.is_some()
            } else {
                r.fragment.flow_bytecodes.is_some()
            }
        });
        if compiled_ok {
            continue;
        }
        let helper_detail = match &helper_reason {
            Some(r) => format!(" Reason: {r}."),
            None => String::new(),
        };
        let msg = format!(
            "LTM implicit helper '{}' (synthesized while parsing LTM variable \
             '{}') failed to compile; it keeps a layout slot but no bytecode, \
             so it evaluates to a constant 0. Every link or loop score that \
             reads it is silently degraded. This usually means the LTM \
             augmentation layer emitted an equation the compiler rejected.{}",
            im_name, meta.ltm_parent_name, helper_detail,
        );
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: Some(im_name.clone()),
            error: DiagnosticError::Assembly(msg),
            severity: DiagnosticSeverity::Warning,
        })
        .accumulate(db);
    }
}

/// Build the fragment input of one implicit helper an LTM equation's parse
/// synthesized (a PREVIOUS/INIT capture aux, or -- should an LTM equation ever
/// contain a module-function call -- a module instance): the helper's lowered
/// form and the shape of every name it references.
///
/// The helper rides on its `LtmImplicitVarMeta` (captured when
/// `model_ltm_implicit_var_info` parsed the LTM equations), so no parent
/// equation is re-parsed. A capture helper's dependencies are the model
/// variables its expression reads (including `module·port` outputs, which
/// resolve through the module's shape); a name that is neither a model variable
/// nor a module instance -- another LTM variable or helper -- resolves to a
/// scalar shape, so the reference compiles to a bare slot read and assembly's
/// layout filter judges it. `None` when the helper's equation does not parse
/// (the diagnostic pass reports the missing bytecode).
pub(crate) fn ltm_implicit_fragment_input<'db>(
    db: &'db dyn Db,
    meta: &LtmImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
    module_input_names: &[String],
) -> Option<FragmentInput<'db>> {
    let implicit_dm_var = &meta.variable;
    let implicit_name = canonicalize(implicit_dm_var.get_ident()).into_owned();
    let var_ident: Ident<Canonical> = Ident::new(&implicit_name);

    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);
    let units_ctx = project_units_context(db, project);

    let mut dummy_implicits = Vec::new();
    let parsed_implicit = crate::variable::parse_var(
        dim_context,
        implicit_dm_var,
        &mut dummy_implicits,
        units_ctx,
        |mi| Ok(Some(mi.clone())),
    );
    if parsed_implicit
        .equation_errors()
        .is_some_and(|e| !e.is_empty())
    {
        return None;
    }

    let source_vars = model.variables(db);
    let mut deps: IdentMap<Ident<Canonical>, DepShape> = Default::default();
    let mut tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>> = HashMap::new();

    let lowered = if meta.is_module {
        // A module-typed helper is its wiring (a module has no equation); its
        // dependencies are the sources its inputs read.
        let datamodel::Variable::Module(dm_module) = implicit_dm_var else {
            return None;
        };
        deps.insert(
            var_ident.clone(),
            module_dep_shape(db, project, &dm_module.model_name),
        );
        let ltm_implicit_all = model_ltm_implicit_var_info(db, model, project);
        for mr in &dm_module.references {
            let src = canonicalize(&mr.src);
            let (head, qualified) = dep_head(&src);
            if head == implicit_name || is_implicit_global(head) || deps.contains_key(head) {
                continue;
            }
            let shape = if qualified {
                // `module_var·output`: the instance the read relocates through,
                // another module-typed LTM implicit variable.
                match ltm_implicit_all.get(head) {
                    Some(ref_meta) if ref_meta.is_module => {
                        module_dep_shape(db, project, ref_meta.model_name.as_deref().unwrap_or(""))
                    }
                    _ => continue,
                }
            } else if let Some(dep_sv) = source_vars.get(head) {
                source_dep_shape(db, *dep_sv, project)
            } else {
                // Another LTM var or implicit helper: scalar.
                DepShape::var(Vec::new())
            };
            deps.insert(Ident::new(head), shape);
        }
        crate::variable::Variable::module_instance(
            var_ident,
            Ident::new(&dm_module.model_name),
            build_module_inputs(
                model.name(db),
                &module_input_prefix(&implicit_name),
                dm_module
                    .references
                    .iter()
                    .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
            ),
        )
    } else {
        // Same dependency-aware lowering scope as `ltm_fragment_input` (GH
        // #738): a synthesized helper aux whose equation embeds a reducer over
        // an array expression needs its deps' dimensions resolvable for Pass-1
        // temp decomposition. The classification comes back from the same
        // lowering, so the lowered AST is not walked again.
        let LoweredLtmVariable {
            variable: lowered,
            dep_idents,
            referenced_tables,
        } = lower_ltm_variable(db, &parsed_implicit, &dummy_implicits, model, project);
        // An arrayed capture helper occupies one slot per element.
        deps.insert(
            var_ident,
            DepShape::var(
                lowered
                    .get_dimensions()
                    .map(<[crate::dimensions::Dimension]>::to_vec)
                    .unwrap_or_default(),
            ),
        );
        // No lowered AST -> no dependency shapes: if the scoped re-lower
        // surfaced an equation error, `lowered.ast()` is `None` and the
        // fragment compiles to nothing anyway.
        let (dep_idents, referenced_tables) = if lowered.ast().is_some() {
            (dep_idents, referenced_tables)
        } else {
            (BTreeSet::new(), BTreeSet::new())
        };
        let implicit_info = model_implicit_var_info(db, model, project);
        for dep in &dep_idents {
            let (head, qualified) = dep_head(dep.as_str());
            if head == implicit_name || is_implicit_global(head) || deps.contains_key(head) {
                continue;
            }
            let shape = if qualified {
                // `module·port`: the instance the read relocates through -- one
                // of the model's SMOOTH/DELAY instances, or an explicit module
                // variable of the parent model.
                if let Some(im_meta) = implicit_info.get(head).filter(|m| m.is_module) {
                    implicit_dep_shape(db, project, im_meta)
                } else if let Some(dep_sv) = source_vars
                    .get(head)
                    .filter(|sv| sv.kind(db) == SourceVariableKind::Module)
                {
                    source_dep_shape(db, *dep_sv, project)
                } else {
                    continue;
                }
            } else if let Some(dep_sv) = source_vars.get(head) {
                source_dep_shape(db, *dep_sv, project)
            } else {
                DepShape::var(Vec::new())
            };
            deps.insert(Ident::new(head), shape);
        }
        // Referenced lookup tables: shape + graphical-function data, so a
        // `lookup(table, ...)` inside a synthesized helper compiles (issue
        // #606; see `ltm_fragment_input`).
        for table_name in &referenced_tables {
            let (head, qualified) = dep_head(table_name);
            if qualified {
                continue;
            }
            let Some(table_sv) = source_vars.get(head) else {
                continue;
            };
            let table_data = extract_tables_from_source_var(db, table_sv, project);
            if !table_data.is_empty() {
                tables.insert(Ident::new(head), table_data);
            }
            deps.entry(Ident::new(head))
                .or_insert_with(|| source_dep_shape(db, *table_sv, project));
        }
        lowered
    };

    Some(FragmentInput::new(
        lowered,
        deps,
        tables,
        canonical_module_input_set(module_input_names),
        Ident::new(model.name(db)),
        converted_dims,
        dim_context,
    ))
}

/// Compile a single implicit variable from an LTM equation to symbolic
/// bytecodes: its [`ltm_implicit_fragment_input`], lowered per phase and
/// emitted through the same emission entry point every other fragment uses.
///
/// This is analogous to `compile_implicit_var_fragment` but for implicit
/// variables generated by LTM equation parsing rather than by
/// SourceVariable parsing. LTM implicit vars participate in whichever
/// phases their lowered form needs; assembly appends them to the runlists by
/// bytecode presence (they are not part of the dependency graph), so every
/// available phase is compiled.
pub(crate) fn compile_ltm_implicit_var_fragment(
    db: &dyn Db,
    meta: &LtmImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
    module_input_names: &[String],
    mut why: Option<&mut Option<String>>,
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::{CompiledVarFragment, PerVarBytecodes};

    let implicit_name = canonicalize(meta.variable.get_ident()).into_owned();

    // GH #741: the same test-scoped forced failure as
    // `compile_ltm_synthetic_fragment` (GH #547), extended to the implicit-
    // helper path so the positive tests for the implicit-helper leg of
    // `model_ltm_fragment_diagnostics` are decoupled from the lifetime of any
    // real helper-compile bug. Both assembly and the diagnostic pass call
    // through here, so a forced failure produces the same silently-stubbed
    // helper assembly would (and the Warning that now covers it).
    #[cfg(test)]
    {
        let forced = LTM_FRAGMENT_FAILURE_OVERRIDE.with(|c| {
            c.borrow()
                .as_deref()
                .is_some_and(|pat| implicit_name.contains(pat))
        });
        if forced {
            return None;
        }
    }

    let input = ltm_implicit_fragment_input(db, meta, model, project, module_input_names)?;
    let emit_ctx = input.emit_ctx();
    let compile_phase = |exprs: &[crate::compiler::Expr]| -> Option<PerVarBytecodes> {
        compile_phase_to_per_var_bytecodes(&emit_ctx, exprs)
    };

    let initial_bytecodes = match lower_fragment(&input, true) {
        Ok(var_result) => compile_phase(&var_result.ast),
        Err(_) => None,
    };

    // Only the value-bearing phase's reason is captured: that is the phase
    // `model_ltm_fragment_diagnostics` gates on, so it is the one whose
    // failure turns the helper into a silent constant 0.
    let flow_bytecodes = if !meta.is_stock {
        match lower_fragment(&input, false) {
            Ok(var_result) => {
                if why.is_some() {
                    match crate::db::assemble::compile_phase_to_per_var_bytecodes_reporting(
                        &emit_ctx,
                        &var_result.ast,
                    ) {
                        Ok(bytecodes) => Some(bytecodes),
                        Err(err) => {
                            if let Some(slot) = why.as_deref_mut() {
                                *slot = Some(err);
                            }
                            None
                        }
                    }
                } else {
                    compile_phase(&var_result.ast)
                }
            }
            Err(err) => {
                if let Some(slot) = why {
                    *slot = Some(lowering_failure_reason(&input, &err));
                }
                None
            }
        }
    } else {
        None
    };

    let stock_bytecodes = if meta.is_stock || meta.is_module {
        match lower_fragment(&input, false) {
            Ok(var_result) => compile_phase(&var_result.ast),
            Err(_) => None,
        }
    } else {
        None
    };

    Some(VarFragmentResult {
        fragment: CompiledVarFragment {
            ident: implicit_name,
            initial_bytecodes,
            flow_bytecodes,
            stock_bytecodes,
        },
        // LTM implicit helpers are always dynamic; not classified for
        // run-invariance.
        flow_invariance: None,
    })
}

#[cfg(test)]
mod pass1_gate_tests {
    use super::expr_contains_pass1_decomposition_site;
    use crate::ast::{Expr2, IndexExpr2, Loc};
    use crate::builtins::BuiltinFn;
    use crate::common::{Canonical, Ident};

    fn c() -> Box<Expr2> {
        Box::new(Expr2::Const(
            "0".to_string(),
            crate::ast::Literal::new(0.0),
            Loc::default(),
        ))
    }

    fn app(builtin: BuiltinFn<Expr2>) -> Expr2 {
        Expr2::App(builtin, None, Loc::default())
    }

    /// The guard test tying the gate to Pass-1's decomposition set
    /// (`ast::expr3::Pass1Context::transform_builtin_inner` /
    /// `transform_inner`'s arrayed-GF apply): every builtin Pass-1
    /// decomposes must flag the gate, and the non-decomposing near-misses
    /// (n-ary MEAN, 2-arg MIN/MAX) must not flag it on their own. The
    /// signature table is the compile-time half of this guard -- a new
    /// `BuiltinFn` variant fails to build until its argument kinds are
    /// stated there -- while this test pins the classification of the
    /// existing variants so a refactor cannot silently flip one (the
    /// round-2 GH #738 regression was exactly such a divergence: the gate
    /// was derived from the agg-hoistable reducer set, which omits SIZE).
    #[test]
    fn pass1_gate_covers_each_decomposition_builtin() {
        let decomposing: Vec<(&str, BuiltinFn<Expr2>)> = vec![
            ("sum", BuiltinFn::Sum(c())),
            ("mean_1arg", BuiltinFn::Mean(vec![*c()])),
            ("stddev", BuiltinFn::Stddev(c())),
            ("size", BuiltinFn::Size(c())),
            ("min_1arg", BuiltinFn::Min(c(), None)),
            ("max_1arg", BuiltinFn::Max(c(), None)),
            (
                "vector_select",
                BuiltinFn::VectorSelect(c(), c(), c(), c(), c()),
            ),
            ("vector_elm_map", BuiltinFn::VectorElmMap(c(), c())),
            ("vector_sort_order", BuiltinFn::VectorSortOrder(c(), c())),
            (
                "allocate_available",
                BuiltinFn::AllocateAvailable(c(), c(), c()),
            ),
            (
                "allocate_by_priority",
                BuiltinFn::AllocateByPriority(c(), c(), c(), c(), c()),
            ),
            ("lookup", BuiltinFn::Lookup(c(), c(), Loc::default())),
            (
                "lookup_forward",
                BuiltinFn::LookupForward(c(), c(), Loc::default()),
            ),
            (
                "lookup_backward",
                BuiltinFn::LookupBackward(c(), c(), Loc::default()),
            ),
        ];
        for (name, builtin) in decomposing {
            assert!(
                expr_contains_pass1_decomposition_site(&app(builtin)),
                "{name} is a Pass-1 decomposition site and must flag the gate"
            );
        }

        let decomposing_rank = app(BuiltinFn::Rank(c(), c()));
        assert!(
            expr_contains_pass1_decomposition_site(&decomposing_rank),
            "RANK's array argument decomposes like VECTOR SORT ORDER's"
        );

        // n-ary MEAN is the scalar mean of its arguments (every position is
        // `ArgKind::Scalar`), so it is not a decomposition site on its own.
        let non_decomposing: Vec<(&str, BuiltinFn<Expr2>)> = vec![
            ("mean_2arg", BuiltinFn::Mean(vec![*c(), *c()])),
            ("min_2arg", BuiltinFn::Min(c(), Some(c()))),
            ("max_2arg", BuiltinFn::Max(c(), Some(c()))),
            ("abs", BuiltinFn::Abs(c())),
            ("previous", BuiltinFn::Previous(c(), c())),
            ("init", BuiltinFn::Init(c())),
        ];
        for (name, builtin) in non_decomposing {
            assert!(
                !expr_contains_pass1_decomposition_site(&app(builtin)),
                "{name} is not a Pass-1 decomposition site and must not flag the gate alone"
            );
        }
    }

    /// A decomposition site nested inside a non-decomposing construct
    /// (a builtin argument, an Op2 operand, a subscript index) must still
    /// flag the gate -- the walk recurses everywhere Pass-1's transform
    /// recurses.
    #[test]
    fn pass1_gate_finds_nested_decomposition_sites() {
        let nested_in_builtin = app(BuiltinFn::Abs(Box::new(app(BuiltinFn::Sum(c())))));
        assert!(expr_contains_pass1_decomposition_site(&nested_in_builtin));

        let nested_in_op2 = Expr2::Op2(
            crate::ast::BinaryOp::Mul,
            c(),
            Box::new(app(BuiltinFn::Size(c()))),
            None,
            Loc::default(),
        );
        assert!(expr_contains_pass1_decomposition_site(&nested_in_op2));

        let nested_in_subscript = Expr2::Subscript(
            Ident::<Canonical>::new("a"),
            vec![IndexExpr2::Expr(app(BuiltinFn::Sum(c())))],
            None,
            Loc::default(),
        );
        assert!(expr_contains_pass1_decomposition_site(&nested_in_subscript));

        let plain = Expr2::Op2(
            crate::ast::BinaryOp::Add,
            c(),
            Box::new(app(BuiltinFn::Previous(c(), c()))),
            None,
            Loc::default(),
        );
        assert!(!expr_contains_pass1_decomposition_site(&plain));
    }
}
