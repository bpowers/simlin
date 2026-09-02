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

use crate::compiler::fragment::{DepShape, FragmentInput, lower_fragment};
use crate::db::var_fragment::{
    dep_head, dimensions_named, implicit_dep_shape, is_implicit_global, model_dep_shape,
    source_dep_shape,
};
use crate::db::{
    Db, LtmLinkId, LtmSyntheticVar, RefShape, SourceModel, SourceProject, SourceVariableKind,
    VarFragmentResult, build_module_inputs, canonical_module_input_set,
    compile_phase_to_per_var_bytecodes, extract_tables_from_source_var, model_implicit_var_info,
    module_dep_shape, module_input_prefix, project_converted_dimensions,
    project_dimensions_context, reconstruct_single_variable,
};

use super::parse::{parse_ltm_equation, scalarize_ltm_equation};
use super::{
    LtmEquation, LtmImplicitVarMeta, endpoint_dimensions, model_ltm_implicit_var_info,
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
    let source_dim_elements: Vec<Vec<String>> = endpoint_dimensions(db, model, project, from_name)
        .unwrap_or_default()
        .iter()
        .map(crate::ltm_augment::dimension_element_names)
        .collect();

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
                    let dims = endpoint_dimensions(db, model, project, dep.as_str())?;
                    (!dims.is_empty()).then(|| (dep.as_str().to_string(), dims))
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
    /// the dt AST).
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

/// Lower a parsed LTM Stage0 variable bounds-free (a `LoweringScope` with no
/// shapes), and classify its dependencies once.
///
/// The shapes feed only `ArrayContext::get_dimensions`, which the
/// `Expr1 -> Expr2` lowering reads to compute `ArrayBounds`, and nothing the
/// fragment compiler needs comes from those bounds: every dependency's shape
/// reaches lowering through `FragmentInput.deps` (`Context::dims_of`), the
/// bare-array rewrite and the subscript lowering read that shape first,
/// materialization is decided on the lowered `compiler::Expr`
/// (`compiler::array_operand`), and the remaining bounds consumers are
/// refusals (an array-valued subscript index, `Expr2`'s bounds unification).
/// An arrayed dependency therefore lowers the same whether the scope knows it
/// or not -- C-LEARN's LTM artifact is byte-identical with a populated scope
/// and without one -- so the bounds-free lowering is the only one. The GH #738
/// shape (`SUM(pop[*] * scale)` under a scalar target) is pinned end to end by
/// `ltm_unified_tests::scalar_target_agg_over_array_expression_fragments_compile`
/// and `ltm_array_agg::size_reducer_previous_helper_compiles_and_is_correct`.
fn lower_ltm_variable(
    db: &dyn Db,
    parsed_variable: &crate::model::VariableStage0,
    project: SourceProject,
) -> LoweredLtmVariable {
    let dim_context = project_dimensions_context(db, project);
    let shapes = IdentMap::default();
    let scope = crate::ast::LoweringScope {
        dimensions: dim_context,
        shapes: &shapes,
        model_name: "",
    };
    let prelim = crate::model::lower_variable(&scope, parsed_variable);

    // Classify dependencies ONCE on the lowering, for the caller's
    // dependency-shape construction. `Variable::ast()` is the
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

    LoweredLtmVariable {
        variable: prelim,
        dep_idents,
        referenced_tables,
    }
}

/// Build the fragment input of one LTM synthetic variable: parse its typed
/// equation (running the SAME implicit-module / PREVIOUS-INIT visitor the
/// ordinary variable parse runs), lower it, and resolve the shape of every
/// name it references. `Err` carries the reason the generated equation did not
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

    let model_var_names = super::ltm_model_var_names(db, model, project);

    let parsed = parse_ltm_equation(var_name, equation, dim_context, model_var_names);
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

    // `lower_ltm_variable` hands back the dependency classification it
    // computed so the lowered AST is not walked again here.
    let LoweredLtmVariable {
        variable: lowered,
        dep_idents,
        referenced_tables,
    } = lower_ltm_variable(db, &parsed.variable, project);

    let var_name_canonical = canonicalize(var_name).into_owned();
    let var_ident: Ident<Canonical> = Ident::new(&var_name_canonical);
    let source_vars = model.variables(db);

    // A helper this equation's own parse synthesized, by canonical name: an
    // arbitrary equation's helpers (a loop score compiled outside
    // `model_ltm_variables`) are in no model-wide map.
    let parsed_implicit = |name: &str| {
        parsed
            .implicit_vars
            .iter()
            .find(|v| canonicalize(v.ident()) == name)
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
        let shape = if let Some(helper) = parsed_implicit(head) {
            match helper.module() {
                Some(dm_module) => module_dep_shape(db, project, &dm_module.model_name),
                None => DepShape::var(dimensions_named(helper.equation_dims(), dim_context)),
            }
        } else {
            ltm_dep_shape(db, model, project, head).unwrap_or_else(|| DepShape::var(Vec::new()))
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

/// The shape of a name an LTM equation or an LTM helper references:
/// [`model_dep_shape`]'s answer for an explicit variable or one of the model's
/// own helpers, plus the two kinds only a generated equation can reference --
/// an LTM helper (a structural capture is arrayed) and an LTM synthetic
/// variable. `None` for a name nothing declares. One statement, so a helper's
/// fragment and its parent's shape a shared dependency identically: a
/// generated capture that reads a sibling capture arrayed over the parent's
/// dimensions lowers `sibling[Dim]` against those dimensions.
fn ltm_dep_shape(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    head: &str,
) -> Option<DepShape> {
    if let Some(shape) = model_dep_shape(db, model, project, head) {
        return Some(shape);
    }
    let dim_context = project_dimensions_context(db, project);
    if let Some(meta) = model_ltm_implicit_var_info(db, model, project).get(head) {
        return Some(if meta.is_module {
            module_dep_shape(db, project, meta.model_name.as_deref().unwrap_or(""))
        } else {
            DepShape::var(dimensions_named(meta.variable.equation_dims(), dim_context))
        });
    }
    // Another LTM synthetic variable. The indexed lookup matters: most
    // unresolved deps here are PREVIOUS-helper names that are NOT LTM vars,
    // and a linear scan over all LTM vars per dep was O(N^2) across a
    // model's compile (~145k lookups over 6.7k vars on C-LEARN).
    let idx = *model_ltm_var_name_index(db, model, project).get(head)?;
    let lsv = &model_ltm_variables(db, model, project).vars[idx];
    Some(DepShape::var(dimensions_named(
        &lsv.dimensions,
        dim_context,
    )))
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
                .map(|e| match &e.details {
                    Some(reason) => format!("{:?} at {}..{}: {reason}", e.code, e.start, e.end),
                    None => format!("{:?} at {}..{}", e.code, e.start, e.end),
                })
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
/// capture auxes `builtins_visitor::hoist_capture` synthesizes while
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
        // Every phase the helper's consumers demand must have produced
        // bytecode: `compile_ltm_implicit_var_fragment` returns `Some` even
        // when a phase failed (each is compiled independently and a failed one
        // is just `None` in the fragment), and `assemble_module` appends only
        // the phases that exist to the runlists.
        if ltm_helper_phases_present(meta, fragment.as_ref()) {
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

/// Whether an LTM helper's fragment holds bytecode for every phase its
/// consumers demand: a capture's phases are its kind (`CaptureKind`), a
/// module instance is advanced through its stock bytecode, and any other
/// helper -- a hoisted argument, should an LTM equation ever contain a
/// module-function call -- is recomputed each step through its flow bytecode.
/// A demanded phase without bytecode is a helper that keeps its layout slot
/// and reads as a constant 0, which `model_ltm_fragment_diagnostics` reports.
pub(super) fn ltm_helper_phases_present(
    meta: &LtmImplicitVarMeta,
    fragment: Option<&VarFragmentResult>,
) -> bool {
    let Some(result) = fragment else {
        return false;
    };
    if meta.is_module {
        return result.fragment.stock_bytecodes.is_some();
    }
    let (initials, flows) = ltm_helper_phase_demand(meta);
    (!initials || result.fragment.initial_bytecodes.is_some())
        && (!flows || result.fragment.flow_bytecodes.is_some())
}

/// The `(initials, flows)` phases a non-module LTM helper is evaluated in: a
/// capture's kind, and both for any other helper.
fn ltm_helper_phase_demand(meta: &LtmImplicitVarMeta) -> (bool, bool) {
    match meta.variable.capture().map(|c| c.kind()) {
        Some(kind) => (kind.needs_initials(), kind.needs_flows()),
        None => (true, true),
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
    let implicit_name = canonicalize(implicit_dm_var.ident()).into_owned();
    let var_ident: Ident<Canonical> = Ident::new(&implicit_name);

    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);

    let parsed_implicit = implicit_dm_var.parsed_variable(dim_context);
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
        let dm_module = implicit_dm_var.module()?;
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
        // Lowered as `ltm_fragment_input` lowers its equation
        // (`lower_ltm_variable`: bounds-free; the shapes built below carry
        // every dimension the compiler reads). The classification comes back
        // from the same lowering, so the lowered AST is not walked again.
        let LoweredLtmVariable {
            variable: lowered,
            dep_idents,
            referenced_tables,
        } = lower_ltm_variable(db, &parsed_implicit, project);
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
        // No lowered AST -> no dependency shapes: if lowering surfaced an
        // equation error, `lowered.ast()` is `None` and the
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
            } else {
                ltm_dep_shape(db, model, project, head).unwrap_or_else(|| DepShape::var(Vec::new()))
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
/// SourceVariable parsing. An LTM helper is not in the dependency graph:
/// assembly appends it to the runlists by bytecode presence, so the phases
/// compiled here ARE its runlist membership -- a capture's kind
/// (`ltm_helper_phase_demand`), the stock phase for a module instance.
pub(crate) fn compile_ltm_implicit_var_fragment(
    db: &dyn Db,
    meta: &LtmImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
    module_input_names: &[String],
    mut why: Option<&mut Option<String>>,
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::{CompiledVarFragment, PerVarBytecodes};

    let implicit_name = canonicalize(meta.variable.ident()).into_owned();

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
    // The first demanded phase to fail leaves its reason in `why`, when the
    // caller wired one: that is what `model_ltm_fragment_diagnostics` reports
    // for the helper that would otherwise read as a silent constant 0.
    let mut emit = |is_initial: bool| -> Option<PerVarBytecodes> {
        match lower_fragment(&input, is_initial) {
            Ok(var_result) => match why.as_deref_mut() {
                Some(slot) => {
                    match crate::db::assemble::compile_phase_to_per_var_bytecodes_reporting(
                        &emit_ctx,
                        &var_result.ast,
                    ) {
                        Ok(bytecodes) => Some(bytecodes),
                        Err(err) => {
                            slot.get_or_insert(err);
                            None
                        }
                    }
                }
                None => compile_phase_to_per_var_bytecodes(&emit_ctx, &var_result.ast),
            },
            Err(err) => {
                if let Some(slot) = why.as_deref_mut() {
                    slot.get_or_insert(lowering_failure_reason(&input, &err));
                }
                None
            }
        }
    };

    // A module instance is evaluated in every phase, like an explicit one:
    // `EvalModule` runs the sub-model's initials, flows and stocks in turn.
    let (initials, flows) = if meta.is_module {
        (true, true)
    } else {
        ltm_helper_phase_demand(meta)
    };
    let initial_bytecodes = initials.then(|| emit(true)).flatten();
    let flow_bytecodes = flows.then(|| emit(false)).flatten();
    let stock_bytecodes = meta.is_module.then(|| emit(false)).flatten();

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
