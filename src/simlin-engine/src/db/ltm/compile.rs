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

use crate::compiler::fragment::{DepKind, DepShape, FragmentInput, lower_fragment};
use crate::db::var_fragment::{
    dep_head, dimensions_named, implicit_dep_shape, is_implicit_global, source_dep_shape,
};
use crate::db::{
    Db, LtmLinkId, LtmSyntheticVar, RefShape, SourceModel, SourceProject, VarFragmentResult,
    build_module_inputs, canonical_module_input_set, compile_phase_to_per_var_bytecodes,
    extract_tables_from_source_var, lowered_variable_by_name, model_implicit_var_by_name,
    model_variable_by_name, module_dep_shape, module_input_prefix, parse_source_variable,
    project_converted_dimensions, project_dimensions_context, project_units_context,
    variable_dimensions,
};

use super::parse::{parse_ltm_equation, scalarize_ltm_equation};
use super::{
    LtmEquation, LtmImplicitVarMeta, model_ltm_implicit_var_info, model_ltm_var_name_index,
    model_ltm_variables,
};

/// Resolve the declared shape of a dependency visible to an LTM fragment.
///
/// The LTM compiler has four production namespaces: source variables, source
/// parse helpers, LTM parse helpers, and LTM synthetic variables. Every
/// fragment constructor uses this projection so absence from
/// `SourceModel::variables` never implies scalar storage.
fn ltm_dependency_shape(
    db: &dyn Db,
    name: &str,
    model: SourceModel,
    project: SourceProject,
) -> Option<DepShape> {
    if let Some(source) = model_variable_by_name(db, model, name.to_string()) {
        return Some(source_dep_shape(db, source, project));
    }
    if let Some(meta) = model_implicit_var_by_name(db, model, project, name.to_string()) {
        return Some(implicit_dep_shape(db, project, &meta));
    }
    if let Some(meta) = model_ltm_implicit_var_info(db, model, project).get(name) {
        return Some(if meta.is_module {
            module_dep_shape(db, project, meta.model_name.as_deref().unwrap_or(""))
        } else {
            DepShape::var(dimensions_named(
                meta.variable.equation_dims(),
                project_dimensions_context(db, project),
            ))
        });
    }
    let idx = *model_ltm_var_name_index(db, model, project).get(name)?;
    let variable = &model_ltm_variables(db, model, project).vars[idx];
    Some(DepShape::var(dimensions_named(
        &variable.dimensions,
        project_dimensions_context(db, project),
    )))
}

/// Compile a single LTM synthetic variable's equation to symbolic
/// bytecodes.
///
/// This is the per-link compilation granularity that enables incremental
/// recomputation: when a variable's equation changes, salsa only
/// recompiles fragments for affected links. Equation edits that don't
/// change the dependency set return their cached fragment (AC1.2).
///
/// LTM equations are pure aux equations, scalar or dimensioned, that may reference:
/// - Model variables (stocks, flows, auxes) from the parent model
/// - Other LTM variables (loop scores referencing link scores)
/// - PREVIOUS/INIT capture helpers created during LTM parsing
/// - Implicit time/dt/initial_time/final_time variables
///
/// Parsed LTM equations may synthesize helper auxes for PREVIOUS/INIT. Source
/// stdlib modules have already been expanded before LTM equation generation;
/// their qualified output reads resolve through the source implicit registry.
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
// same reason the fragment execution record is thread-local: parallel query
// execution would require moving this measurement to shared atomic state.
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
///   made this `(from, to)` edge unscoreable. The variant carries the loud
///   `Warning` fact; the caller MUST append it in producer order and record the
///   edge in `unscoreable_edges` so loop scores traversing it are DROPPED (the
///   #758 contract), not stubbed.
/// - [`NoVariable`](ShapedLinkScore::NoVariable) -- no variable for benign
///   structural reasons (the target could not be reconstructed, or a
///   module link has no composite/output to score). NOT an unscoreable
///   edge: the caller skips it silently, exactly as the pre-fix `None`
///   did, and loop scores through it are unaffected.
///
/// Surfacing both the partial-equation signal and its diagnostic through the
/// query's RETURN value keeps them consistent under salsa caching: the caller
/// re-reads the memoized value, appends its warning fact, and re-inserts the
/// edge into the freshly rebuilt `unscoreable_edges` set on every
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
    /// A `PartialEquationError` made the edge unscoreable. The caller records
    /// the edge in `unscoreable_edges` and appends this pure warning fact to
    /// the owning model's ordered diagnostic facts.
    Unscoreable(crate::db::Diagnostic),
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

    let from_var = lowered_variable_by_name(db, model, project, from_name);
    // A target that cannot be reconstructed is a benign structural skip
    // (degenerate edge), NOT a partial-equation failure -- no `Warning`, no
    // unscoreable-edge recording. Loop scores through such an edge are
    // unaffected, exactly as the pre-GH #780 `None` behaved.
    let Some(to_var) = lowered_variable_by_name(db, model, project, to_name) else {
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
        super::endpoint_dimensions(db, from_name, model, project)
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
    // deps with no resolvable explicit-or-implicit endpoint are omitted -- the
    // recognizer keeps its permissive legacy collapse for those.
    let dep_dims: HashMap<String, Vec<crate::dimensions::Dimension>> = to_var
        .ast()
        .map(|ast| {
            use crate::ast::Ast;
            let target_ast_dims: &[crate::dimensions::Dimension] = match ast {
                Ast::Scalar(_) => &[],
                Ast::ApplyToAll(dims, _) | Ast::Arrayed(dims, _, _, _) => dims,
            };
            crate::variable::expression_transform_names(ast, target_ast_dims)
                .value_candidates
                .into_iter()
                .filter_map(|dep| {
                    let dims = super::endpoint_dimensions(db, dep.as_str(), model, project)?;
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
        return ShapedLinkScore::Unscoreable(super::ltm_partial_equation_warning(
            db, model, &var_name, from_name, to_name, &err,
        ));
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
    let to_occurrences =
        crate::db::ltm_ir::model_ltm_occurrences_by_name(db, model, project, to_name.to_string());

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
            return ShapedLinkScore::Unscoreable(super::ltm_partial_equation_warning(
                db, model, &var_name, from_name, to_name, &err,
            ));
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
/// lowering. Callers reuse `dep_targets`/`referenced_tables` to build their
/// dependency shapes instead of re-running `classify_dependencies` on the
/// returned variable -- the classification is a per-fragment AST walk, and
/// duplicating it across every LTM fragment was a measurable slice of
/// C-LEARN's LTM compile time.
struct LoweredLtmVariable {
    variable: crate::variable::Variable,
    /// Structurally resolved dependency targets of the lowered AST
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
    dep_targets: BTreeSet<crate::db::DepTarget>,
    /// `classify_dependencies(..).referenced_tables` of the same AST.
    referenced_tables: BTreeSet<String>,
}

fn source_implicit_module_model_name(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    head: &str,
) -> Option<String> {
    model_implicit_var_by_name(db, model, project, head.to_string())
        .filter(|meta| meta.is_module)
        .and_then(|meta| meta.model_name)
        .map(|model_name| canonicalize(&model_name).into_owned())
}

/// `true` when the lowered AST contains a construct whose compilation
/// consumes the Expr2 `ArrayBounds` that only the dependency-aware
/// lowering scope can recover -- an array operand or lookup table position.
///
/// This is [`lower_ltm_variable`]'s gate for the scoped re-lower, and it
/// must be sound against `BuiltinFn::arg_kinds` -- not the aggregate-node
/// reducer set (`ltm_agg::reducer_kind_from_name`),
/// which differs: `SIZE` is never hoisted into an agg (its link score is
/// constant 0) yet consumes an array argument exactly like `SUM`, and
/// `RANK` -- array-valued, routed through its own LTM agg path (GH #776) --
/// also has an `ArgKind::Array` argument (GH #995).
/// Deriving the original (text-scan) gate from
/// the wrong set silently stubbed any fragment embedding
/// `SIZE(<array expression>)` -- the demonstrated GH #738 round-2
/// regression, pinned by
/// `ltm_array_agg::size_reducer_previous_helper_compiles_and_is_correct`.
fn ast_requires_array_operand_bounds(ast: &crate::ast::Ast<crate::ast::Expr2>) -> bool {
    use crate::ast::Ast;
    match ast {
        Ast::Scalar(e) | Ast::ApplyToAll(_, e) => expr_requires_array_operand_bounds(e),
        Ast::Arrayed(_, elements, default, _) => {
            elements.values().any(expr_requires_array_operand_bounds)
                || default
                    .as_ref()
                    .is_some_and(expr_requires_array_operand_bounds)
        }
    }
}

/// Expression-level walk for [`ast_requires_array_operand_bounds`].
///
/// Sound by construction: every builtin with a non-scalar argument position
/// in `BuiltinFn::arg_kinds` needs its source dimensions while subscript
/// lowering constructs the operand view. That includes every array operand
/// and every lookup table position. The final materializer consumes the same
/// signature rows, so a new `BuiltinFn` variant must state its argument kinds
/// before it compiles. `array_operand_bounds_gate_covers_each_consumer` pins
/// the exhaustive classification.
///
/// The one bounds consumer deliberately NOT gated on is the non-A2A Op2
/// dimension-reordering pass (`compiler::context`'s Op2 lowering): it
/// requires a whole-array Op2 *result* outside any reducer, which in a
/// scalar LTM equation is ill-typed under either lowering, and in an
/// A2A/per-element LTM equation is unreachable (per-element expansion
/// lowers with `active_dimension` set, which skips the pass). A gated-out
/// fragment therefore compiles byte-identically to its empty-scope
/// (pre-GH #738) lowering.
fn expr_requires_array_operand_bounds(expr: &crate::ast::Expr2) -> bool {
    use crate::ast::{Expr2, IndexExpr2};
    match expr {
        Expr2::Const(..) | Expr2::Var(..) => false,
        Expr2::Subscript(_, indices, _, _) => indices.iter().any(|idx| match idx {
            IndexExpr2::Expr(e) => expr_requires_array_operand_bounds(e),
            IndexExpr2::Range(l, r, _) => {
                expr_requires_array_operand_bounds(l) || expr_requires_array_operand_bounds(r)
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
            // An array consumer can hide anywhere in a scalar builtin's
            // arguments (`ABS(SUM(a[*] * 2))`).
            builtin
                .args()
                .into_iter()
                .any(expr_requires_array_operand_bounds)
        }
        Expr2::Op1(_, e, _, _) => expr_requires_array_operand_bounds(e),
        Expr2::Op2(_, l, r, _, _) => {
            expr_requires_array_operand_bounds(l) || expr_requires_array_operand_bounds(r)
        }
        Expr2::If(c, t, f, _, _) => {
            expr_requires_array_operand_bounds(c)
                || expr_requires_array_operand_bounds(t)
                || expr_requires_array_operand_bounds(f)
        }
    }
}

/// Lower one parsed LTM variable with a transient scope that can
/// resolve the dimensions of its model-variable dependencies (GH #738).
///
/// Expr1 -> Expr2 lowering computes each subexpression's `ArrayBounds` via
/// `ArrayContext::get_dimensions`, which reads `LoweringScope.models`. The final
/// array-operand materializer needs those bounds to size a computed operand
/// such as `SUM(pop[*] * scale)`. With an empty scope the bounds are absent and
/// codegen rejects the inline expression instead of guessing a view, which
/// would silently stub the LTM variable to zero. This mirrors
/// `explicit_fragment_input`'s minimal `LoweringModel` construction for ordinary
/// per-variable fragments.
///
/// Strategy: lower once with an empty scope (cheap, and byte-identical to
/// the populated-scope lowering when no dependency is arrayed -- the scope
/// only feeds `get_dimensions`, which returns `None` for scalars either
/// way); only when the lowered AST contains an array operand or table position
/// ([`ast_requires_array_operand_bounds`]) AND an arrayed
/// dependency is present, re-lower with a scope carrying the parsed
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
    parsed_variable: &crate::model::ParsedVariable,
    equation_implicits: &[crate::capture::ImplicitVar],
    model: SourceModel,
    project: SourceProject,
) -> LoweredLtmVariable {
    let dim_context = project_dimensions_context(db, project);
    let empty_models = HashMap::new();
    let empty_scope = crate::model::LoweringScope {
        models: &empty_models,
        dimensions: dim_context,
        model_name: "",
    };
    let prelim = crate::model::lower_variable(&empty_scope, parsed_variable);

    // Classify dependencies ONCE on the preliminary lowering; the set is
    // scope-independent, so it serves both the re-lower decision below and
    // the caller's dependency-shape construction. `Variable::ast()` is the
    // right (and only needed) source: every parsed LTM input here is an
    // Aux-parsed Var whose dt AST is its sole AST, and even a hypothetical
    // stock-shaped input is covered because `ast()` returns a Stock's init
    // AST.
    let classification = prelim
        .ast()
        .map(|ast| crate::variable::classify_dependencies(ast, &[], None));
    let (dep_targets, referenced_tables) = match classification {
        Some(c) => (
            c.occurrences
                .into_iter()
                .map(|occurrence| {
                    crate::db::query::resolve_dependency_target_with_module_lookup(
                        db,
                        Some(model),
                        project,
                        Some(equation_implicits),
                        &occurrence.ident,
                        |head| source_implicit_module_model_name(db, model, project, head),
                    )
                })
                .collect(),
            c.referenced_tables,
        ),
        None => (BTreeSet::new(), BTreeSet::new()),
    };

    // Structural gate: without an array consumer in the lowered AST, the
    // Expr2 bounds the scoped re-lower would recover
    // cannot change the compile outcome -- skip the per-dep arrayedness
    // lookups and the second lowering entirely (the common case: most
    // link/loop scores contain no reducer even on heavily arrayed models).
    if !prelim.ast().is_some_and(ast_requires_array_operand_bounds) {
        return LoweredLtmVariable {
            variable: prelim,
            dep_targets,
            referenced_tables,
        };
    }

    // Local values plus lookup holders whose array shape may affect the scoped
    // re-lower. Qualified outputs remain structured and are excluded by their
    // non-empty module path. A metadata-unresolved qualified token can remain
    // a local identity; the ordinary per-name lookups below simply find no
    // declared shape for it.
    let mut local_shape_candidates: BTreeSet<Ident<Canonical>> = dep_targets
        .iter()
        .filter(|target| target.module_path.is_empty())
        .map(|target| target.variable.clone())
        .collect();
    local_shape_candidates.extend(referenced_tables.iter().map(|name| Ident::new(name)));

    let ltm_implicit_info = model_ltm_implicit_var_info(db, model, project);
    // Resolve a dep that is an LTM-parse-time implicit helper aux to its
    // datamodel form (modules are scalar nodes in equations; only helper
    // auxes can be arrayed).
    let find_implicit_dm = |name: &str| -> Option<crate::capture::ImplicitVar> {
        equation_implicits
            .iter()
            .find(|v| canonicalize(v.ident()) == name)
            .cloned()
            .or_else(|| {
                model_implicit_var_by_name(db, model, project, name.to_string())
                    .filter(|meta| !meta.is_module)
                    .and_then(|meta| {
                        meta.find_in(parse_source_variable(db, meta.parent_source_var, project))
                            .cloned()
                    })
            })
            .or_else(|| {
                ltm_implicit_info
                    .get(name)
                    .filter(|meta| !meta.is_module)
                    .map(|meta| meta.variable.clone())
            })
    };
    let dm_var_is_arrayed = |v: &crate::capture::ImplicitVar| !v.equation_dims().is_empty();
    // An ARRAYED sibling LTM var referenced as a dep -- today that is the
    // GH #995 freeze helper, a whole-array operand of a vector builtin
    // (`VECTOR SELECT("$⁚ltm⁚freeze⁚…", …)`). The final materializer
    // can only materialize a computed array argument (`helper * k`) if
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

    let any_arrayed_dep = local_shape_candidates.iter().any(|name| {
        model_variable_by_name(db, model, name.as_str().to_string())
            .is_some_and(|sv| !variable_dimensions(db, sv, project).is_empty())
            || find_implicit_dm(name.as_str())
                .as_ref()
                .is_some_and(dm_var_is_arrayed)
            || find_arrayed_ltm_dep(name.as_str()).is_some()
    });
    if !any_arrayed_dep {
        return LoweredLtmVariable {
            variable: prelim,
            dep_targets,
            referenced_tables,
        };
    }

    let model_name_str = model.name(db);
    let dim_ctx = project_dimensions_context(db, project);
    let units_ctx = project_units_context(db, project);
    let mut parsed_vars: HashMap<Ident<Canonical>, crate::model::ParsedVariable> = HashMap::new();
    parsed_vars.insert(Ident::new(parsed_variable.ident()), parsed_variable.clone());
    for dep_name in &local_shape_candidates {
        if let Some(dep_sv) = model_variable_by_name(db, model, dep_name.as_str().to_string()) {
            let dep_parsed = parse_source_variable(db, dep_sv, project);
            parsed_vars.insert(dep_name.clone(), dep_parsed.variable.clone());
        } else if let Some(implicit_dep) = find_implicit_dm(dep_name.as_str()) {
            // Nested implicits of an implicit are registered (and compiled)
            // in their own right; here only the dep's own dimensions matter.
            let dep_parsed = implicit_dep.parsed_variable(dim_ctx);
            parsed_vars.insert(dep_name.clone(), dep_parsed);
        } else if let Some(ltm_dims) = find_arrayed_ltm_dep(dep_name.as_str()) {
            // An arrayed sibling LTM var (the GH #995 freeze helper): a
            // zero-bodied dims-only stub -- only the dep's dimensions matter
            // to the lowering, exactly like the implicit branch above.
            let stub = datamodel::Variable::Aux(datamodel::Aux {
                ident: dep_name.as_str().to_string(),
                equation: datamodel::Equation::ApplyToAll(ltm_dims, "0".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            });
            let mut nested = Vec::new();
            let dep_ctx = crate::variable::ParseContext::new(dim_ctx, units_ctx);
            let dep_parsed =
                crate::variable::parse_var(&dep_ctx, &stub, &mut nested, |mi| Ok(Some(mi.clone())));
            parsed_vars.insert(dep_name.clone(), dep_parsed);
        }
    }

    let mini_model = crate::model::LoweringModel {
        variables: parsed_vars
            .into_iter()
            .map(|(name, variable)| (name, std::borrow::Cow::Owned(variable)))
            .collect(),
    };
    let models = [(Ident::new(model_name_str), mini_model)]
        .into_iter()
        .collect();
    let scope = crate::model::LoweringScope {
        models: &models,
        dimensions: dim_context,
        model_name: model_name_str,
    };
    LoweredLtmVariable {
        variable: crate::model::lower_variable(&scope, parsed_variable),
        dep_targets,
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
/// slot 0, tech-debt #34), PREVIOUS/INIT capture helpers synthesized while
/// parsing this or another LTM equation (an ARRAYED capture helper -- the GH
/// #541 arrayed capture, extended to array-valued builtin subtrees like
/// `rank(pop, 1)` by GH #742 -- needs its dimensions so the consuming
/// `helper[dim·elem]` subscript resolves), the model's own SMOOTH/DELAY
/// instances and ordinary capture helpers, and the implicit time globals.
/// A name absent from all four namespaces has no shape. It is omitted from the
/// fragment scope so lowering reports the unknown dependency instead of
/// inventing scalar storage.
///
/// The variant of `equation` determines the variable's slot count: a
/// `Scalar` equation gets 1 slot; an `ApplyToAll`/`Arrayed` equation
/// gets `product(dim_lengths)` slots and is compiled with the A2A /
/// per-element expansion the compiler applies to those variants.
pub(crate) struct LtmFragmentInput<'db> {
    input: FragmentInput<'db>,
    dependency_targets: BTreeSet<crate::db::DepTarget>,
}

impl<'db> std::ops::Deref for LtmFragmentInput<'db> {
    type Target = FragmentInput<'db>;

    fn deref(&self) -> &Self::Target {
        &self.input
    }
}

pub(crate) fn ltm_fragment_input<'db>(
    db: &'db dyn Db,
    var_name: &str,
    equation: &LtmEquation,
    model: SourceModel,
    project: SourceProject,
) -> Result<LtmFragmentInput<'db>, Vec<crate::db::Diagnostic>> {
    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);

    let model_var_names = super::ltm_model_var_names(db, model, project);

    let parsed = parse_ltm_equation(var_name, equation, dim_context, Some(model_var_names));
    if parsed.variable.has_fatal_diagnostics() {
        let arm_diagnostics: Vec<_> = equation
            .arm_parse_errors_with_elements()
            .into_iter()
            .map(|(element, error)| {
                let diagnostic =
                    crate::db::Diagnostic::equation(error, crate::db::DiagnosticSeverity::Warning);
                match element {
                    Some(element) => diagnostic.with_element(element),
                    None => diagnostic,
                }
            })
            .collect();
        return Err(if arm_diagnostics.is_empty() {
            parsed.variable.diagnostics.clone()
        } else {
            arm_diagnostics
        });
    }

    // `lower_ltm_variable` threads the dependencies (model variables and
    // arrayed parse-time helpers) into the lowering scope so array bounds
    // resolve (GH #738), and hands back the dependency classification it
    // computed so the lowered AST is not walked again here.
    let LoweredLtmVariable {
        variable: lowered,
        dep_targets,
        referenced_tables,
    } = lower_ltm_variable(db, &parsed.variable, &parsed.implicit_vars, model, project);

    let var_name_canonical = canonicalize(var_name).into_owned();
    let var_ident: Ident<Canonical> = Ident::new(&var_name_canonical);
    // A helper this equation's own parse synthesized, by canonical name.
    let parsed_implicit = |name: &str| {
        parsed
            .implicit_vars
            .iter()
            .find(|v| canonicalize(v.ident()) == name)
    };
    let helper_dims = |helper: &crate::capture::ImplicitVar| {
        dimensions_named(helper.equation_dims(), dim_context)
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
    for dep in &dep_targets {
        let head = dep.local_node().as_str();
        if head == var_name_canonical || is_implicit_global(head) || deps.contains_key(head) {
            continue;
        }
        let shape = if let Some(helper) = parsed_implicit(head) {
            match helper.module() {
                Some(dm_module) => module_dep_shape(db, project, dm_module.model_name()),
                None => DepShape::var(helper_dims(helper)),
            }
        } else {
            let Some(shape) = ltm_dependency_shape(db, head, model, project) else {
                // Missing metadata is not evidence of a scalar. Omitting the
                // shape lets lowering report the unknown dependency instead of
                // assigning it an invented scalar shape.
                continue;
            };
            shape
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
        let Some(table_sv) = model_variable_by_name(db, model, head.to_string()) else {
            continue;
        };
        let table_data = extract_tables_from_source_var(db, &table_sv, project);
        if !table_data.is_empty() {
            tables.insert(Ident::new(head), table_data);
        }
        deps.entry(Ident::new(head))
            .or_insert_with(|| source_dep_shape(db, table_sv, project));
    }

    // This variable is lowered from the transient synthesized equation above;
    // no per-variable memo owns it, so the input must carry its Arc.
    Ok(LtmFragmentInput {
        input: FragmentInput::new(
            std::borrow::Cow::Owned(std::sync::Arc::new(lowered)),
            deps,
            tables,
            BTreeSet::new(),
            Ident::new(model.name(db)),
            converted_dims,
            dim_context,
        ),
        dependency_targets: dep_targets,
    })
}

/// Compile an arbitrary LTM `Equation` to symbolic bytecodes: its
/// [`ltm_fragment_input`], lowered and emitted through the same emission
/// entry point the explicit and implicit paths use.
///
/// Shared implementation used by `compile_ltm_var_fragment` (link scores)
/// and the loop/relative score compilation in `assemble_module`.
///
/// `diagnostics`, when supplied, receives the original typed failures in
/// producer order. Parse and lowering codes, spans, module paths and raw detail
/// remain intact; codegen refusals, which expose only a reason, become assembly
/// diagnostics. Callers that only want the fragment pass `None` and pay
/// nothing.
pub(crate) fn compile_ltm_equation_fragment(
    db: &dyn Db,
    var_name: &str,
    equation: &LtmEquation,
    model: SourceModel,
    project: SourceProject,
    mut diagnostics: Option<&mut Vec<crate::db::Diagnostic>>,
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::CompiledVarFragment;

    #[cfg(test)]
    crate::db::note_fragment_execution(crate::db::FragmentExecKind::LtmBody, var_name);

    let input = match ltm_fragment_input(db, var_name, equation, model, project) {
        Ok(input) => input,
        Err(failures) => {
            if let Some(output) = diagnostics.as_deref_mut() {
                extend_ltm_failures(output, failures, model.name(db), var_name, None);
            }
            return None;
        }
    };

    // LTM vars are always flow-phase only (scalar auxes, not stocks)
    let flow_bytecodes = match lower_fragment(&input, false) {
        Ok(var_result) => {
            if diagnostics.is_some() {
                match crate::db::assemble::compile_phase_to_per_var_bytecodes_reporting(
                    &input.emit_ctx(),
                    &var_result.ast,
                ) {
                    Ok(bytecodes) => Some(bytecodes),
                    Err(err) => {
                        if let Some(output) = diagnostics.as_deref_mut() {
                            extend_ltm_failures(
                                output,
                                [crate::db::Diagnostic::assembly(
                                    err,
                                    crate::db::DiagnosticSeverity::Warning,
                                )],
                                model.name(db),
                                var_name,
                                None,
                            );
                        }
                        None
                    }
                }
            } else {
                compile_phase_to_per_var_bytecodes(&input.emit_ctx(), &var_result.ast)
            }
        }
        Err(err) => {
            if let Some(output) = diagnostics {
                extend_ltm_failures(
                    output,
                    ltm_lowering_diagnostics(&input, &err),
                    model.name(db),
                    var_name,
                    None,
                );
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
        flow_locally_invariant: None,
    })
}

/// Preserve the raising-stage diagnostics for a phase `lower_fragment`
/// refused. An empty AST is commonly the consequence of an earlier scoped
/// lowering error, so those diagnostics take precedence over the derivative
/// `EmptyEquation` error.
fn ltm_lowering_diagnostics(
    input: &LtmFragmentInput<'_>,
    err: &crate::common::Error,
) -> Vec<crate::db::Diagnostic> {
    let lowered_diagnostics: Vec<_> = input
        .target
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == crate::db::DiagnosticSeverity::Error)
        .cloned()
        .collect();
    if lowered_diagnostics.is_empty() {
        let dependency_targets: Vec<_> = input.dependency_targets.iter().collect();
        vec![crate::db::fragment_compile::fragment_lowering_diagnostic(
            err,
            input.target.as_ref(),
            &dependency_targets,
        )]
    } else {
        lowered_diagnostics
    }
}

/// Append failures in first-producer order while collapsing only byte-for-byte
/// identical payloads. `Diagnostic` equality includes source identity, element,
/// module path, category, code, span, related sources and raw detail, so two
/// affected slots or instances cannot collapse accidentally.
fn extend_ltm_failures(
    output: &mut Vec<crate::db::Diagnostic>,
    failures: impl IntoIterator<Item = crate::db::Diagnostic>,
    model: &str,
    variable: &str,
    owner: Option<&str>,
) {
    for diagnostic in failures {
        let mut diagnostic = diagnostic.with_context(model.to_string(), Some(variable.to_string()));
        if let Some(owner) = owner {
            diagnostic = diagnostic.with_owner(owner.to_string());
        }
        diagnostic.severity = crate::db::DiagnosticSeverity::Warning;
        if !output.contains(&diagnostic) {
            output.push(diagnostic);
        }
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
            //     path would fail to resolve the synthetic agg through the
            //     explicit-or-implicit per-name lowering query,
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
/// VALUE is one fragment, so salsa backdates it whenever that variable's
/// fragment is unchanged and `assemble_module` is not re-run.
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
/// `assemble_module` does and returns a `Warning` fact for each one whose
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
/// `model_all_diagnostics` drives this when `ltm_enabled` and emits the returned
/// facts together with `model_ltm_variables`' own facts. Public diagnostic
/// adapters temporarily restore a project's latched LTM request while
/// collecting, so the warnings are visible without making LTM part of
/// intrinsic project compilability.
///
/// Only the layout-independent compile failure is reported here. A
/// fragment that compiles but whose variable references do not resolve
/// in the model's layout is the documented sub-model dedup case
/// (`assemble_module`'s `fragment_vars_in_layout` drop), where the root
/// model emits an equivalent fragment under qualified names -- that drop
/// is intentionally left silent.
#[salsa::tracked(returns(ref))]
pub fn model_ltm_fragment_diagnostics(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> Vec<crate::db::Diagnostic> {
    use crate::db::{Diagnostic, DiagnosticSeverity};

    let mut diagnostics = Vec::new();
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
        // Recover the typed failure payload. `compile_ltm_synthetic_fragment`
        // returns only the fragment, so re-run the direct compile with its
        // diagnostic output wired up. For every variable that took
        // the `compile_direct` branch above -- which is every element-pinned
        // equation, i.e. all of the arrayed-model failures this was written
        // for -- that is the identical call. For one that took the
        // salsa-cached `(from, to)` branch the re-derived equation can differ,
        // so the reason is reported as indicative rather than as the failure.
        let mut failures = Vec::new();
        let direct_agrees = compile_ltm_equation_fragment(
            db,
            &ltm_var.name,
            &ltm_var.equation,
            model,
            project,
            Some(&mut failures),
        )
        .is_none_or(|r| r.fragment.flow_bytecodes.is_none());
        let base = format!(
            "LTM synthetic variable '{}' failed to compile; it keeps a \
             layout slot but no bytecode, so it evaluates to a constant 0. \
             Any loop or link score derived from it is silently degraded. \
             This usually means the LTM augmentation layer emitted an equation \
             the compiler rejected.",
            ltm_var.name,
        );
        if failures.is_empty() {
            diagnostics.push(
                Diagnostic::assembly(base, DiagnosticSeverity::Warning)
                    .with_context(model.name(db).clone(), Some(ltm_var.name.clone())),
            );
        } else {
            for diagnostic in failures {
                let reason = diagnostic.reason().map(|reason| {
                    if direct_agrees {
                        format!(" Reason: {reason}.")
                    } else {
                        format!(
                            " Reason (from recompiling its own equation; the cached re-derived \
                             equation is what failed): {reason}."
                        )
                    }
                });
                diagnostics.push(
                    diagnostic
                        .with_display_details(format!("{base}{}", reason.unwrap_or_default())),
                );
            }
        }
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
        return diagnostics;
    }
    let mut implicit_names: Vec<&String> = ltm_implicit.keys().collect();
    implicit_names.sort();
    for im_name in implicit_names {
        let meta = &ltm_implicit[im_name];
        let mut helper_failures = Vec::new();
        let fragment = compile_ltm_implicit_var_fragment(
            db,
            meta,
            model,
            project,
            &[],
            Some(&mut helper_failures),
        );
        // Every phase the helper's value consumers require must have produced
        // bytecode:
        // `compile_ltm_implicit_var_fragment` returns `Some` even when every
        // phase failed (each phase is compiled independently and a failed one
        // is just `None` in the fragment), and `assemble_module` appends only
        // the phases that exist to the runlists. PREVIOUS captures are
        // refreshed via flows, INIT captures populate initial storage, and a
        // capture shared by both consumers requires both. Other aux helpers
        // use flows; a stock or module helper is advanced via stocks.
        let compiled_ok = ltm_implicit_fragment_compiled_ok(meta, fragment.as_ref());
        if compiled_ok {
            continue;
        }
        let base = format!(
            "LTM implicit helper '{}' (synthesized while parsing LTM variable \
             '{}') failed to compile; it keeps a layout slot but no bytecode, \
             so it evaluates to a constant 0. Every link or loop score that \
             reads it is silently degraded. This usually means the LTM \
             augmentation layer emitted an equation the compiler rejected.",
            im_name, meta.ltm_parent_name,
        );
        if helper_failures.is_empty() {
            diagnostics.push(
                Diagnostic::assembly(base, DiagnosticSeverity::Warning)
                    .with_context(model.name(db).clone(), Some(im_name.clone()))
                    .with_owner(meta.ltm_parent_name.clone()),
            );
        } else {
            for diagnostic in helper_failures {
                let reason = diagnostic
                    .reason()
                    .map(|reason| format!(" Reason: {reason}."));
                diagnostics.push(
                    diagnostic
                        .with_display_details(format!("{base}{}", reason.unwrap_or_default())),
                );
            }
        }
    }
    diagnostics
}

/// Whether an LTM helper emitted its value-bearing phase and every phase a
/// capture consumer demands. Other helper kinds retain their stock/flow
/// classification.
pub(super) fn ltm_implicit_fragment_compiled_ok(
    meta: &LtmImplicitVarMeta,
    fragment: Option<&VarFragmentResult>,
) -> bool {
    fragment.is_some_and(|result| {
        if meta.is_stock || meta.is_module {
            result.fragment.stock_bytecodes.is_some()
        } else if let Some(capture) = meta.variable.capture() {
            let kind = capture.kind();
            (!kind.needs_initials() || result.fragment.initial_bytecodes.is_some())
                && (!kind.needs_flows() || result.fragment.flow_bytecodes.is_some())
        } else {
            result.fragment.flow_bytecodes.is_some()
        }
    })
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
/// resolve through the module's shape). Names in the sibling LTM-variable and
/// LTM-helper namespaces retain their declared shapes; a name absent from every
/// namespace is left unresolved so lowering fails loudly. A rejected helper
/// returns its original typed diagnostics to the optional reporting path.
pub(crate) fn ltm_implicit_fragment_input<'db>(
    db: &'db dyn Db,
    meta: &LtmImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
    module_input_names: &[String],
) -> Result<LtmFragmentInput<'db>, Vec<crate::db::Diagnostic>> {
    let implicit_dm_var = &meta.variable;
    let implicit_name = canonicalize(implicit_dm_var.ident()).into_owned();
    let var_ident: Ident<Canonical> = Ident::new(&implicit_name);

    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);

    // Every helper carries enough parsed data to build its stage directly.
    let parsed_implicit = implicit_dm_var.parsed_variable(dim_context);
    if parsed_implicit.has_fatal_diagnostics() {
        return Err(parsed_implicit.diagnostics.clone());
    }

    let mut deps: IdentMap<Ident<Canonical>, DepShape> = Default::default();
    let mut tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>> = HashMap::new();
    let dependency_targets;

    let lowered = if meta.is_module {
        // A module-typed helper is its wiring (a module has no equation); its
        // dependencies are the sources its inputs read.
        let Some(dm_module) = implicit_dm_var.module() else {
            return Err(vec![crate::db::Diagnostic::assembly(
                format!(
                    "LTM helper '{implicit_name}' is marked as a module without module metadata"
                ),
                crate::db::DiagnosticSeverity::Warning,
            )]);
        };
        let mut module_targets = BTreeSet::new();
        deps.insert(
            var_ident.clone(),
            module_dep_shape(db, project, dm_module.model_name()),
        );
        for mr in dm_module.references() {
            let target = crate::db::query::resolve_dependency_target_with_module_lookup(
                db,
                Some(model),
                project,
                None,
                &Ident::new(&mr.src),
                |head| source_implicit_module_model_name(db, model, project, head),
            );
            module_targets.insert(target.clone());
            let head = target.local_node().as_str();
            let qualified = !target.module_path.is_empty();
            if head == implicit_name || is_implicit_global(head) || deps.contains_key(head) {
                continue;
            }
            let Some(shape) = ltm_dependency_shape(db, head, model, project) else {
                continue;
            };
            if qualified && !matches!(&shape.kind, DepKind::Module { .. }) {
                continue;
            }
            deps.insert(Ident::new(head), shape);
        }
        let lowered = crate::variable::Variable::module_instance(
            var_ident,
            Ident::new(dm_module.model_name()),
            build_module_inputs(
                model.name(db),
                &module_input_prefix(&implicit_name),
                dm_module
                    .references()
                    .iter()
                    .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
            ),
        );
        dependency_targets = module_targets;
        lowered
    } else {
        // Same dependency-aware lowering scope as `ltm_fragment_input` (GH
        // #738): a synthesized helper aux whose equation embeds a reducer over
        // an array expression needs its deps' dimensions resolvable for final
        // materialization. The classification comes back from the same
        // lowering, so the lowered AST is not walked again.
        let LoweredLtmVariable {
            variable: lowered,
            dep_targets,
            referenced_tables,
        } = lower_ltm_variable(db, &parsed_implicit, &[], model, project);
        dependency_targets = dep_targets.clone();
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
        let (dep_targets, referenced_tables) = if lowered.ast().is_some() {
            (dep_targets, referenced_tables)
        } else {
            (BTreeSet::new(), BTreeSet::new())
        };
        for dep in &dep_targets {
            let head = dep.local_node().as_str();
            let qualified = !dep.module_path.is_empty();
            if head == implicit_name || is_implicit_global(head) || deps.contains_key(head) {
                continue;
            }
            let Some(shape) = ltm_dependency_shape(db, head, model, project) else {
                continue;
            };
            if qualified && !matches!(&shape.kind, DepKind::Module { .. }) {
                continue;
            }
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
            let Some(table_sv) = model_variable_by_name(db, model, head.to_string()) else {
                continue;
            };
            let table_data = extract_tables_from_source_var(db, &table_sv, project);
            if !table_data.is_empty() {
                tables.insert(Ident::new(head), table_data);
            }
            deps.entry(Ident::new(head))
                .or_insert_with(|| source_dep_shape(db, table_sv, project));
        }
        lowered
    };

    // LTM helper metadata and its lowered value are local to this constructor,
    // unlike ordinary parse helpers backed by `lowered_implicit_variable`.
    Ok(LtmFragmentInput {
        input: FragmentInput::new(
            std::borrow::Cow::Owned(std::sync::Arc::new(lowered)),
            deps,
            tables,
            canonical_module_input_set(module_input_names),
            Ident::new(model.name(db)),
            converted_dims,
            dim_context,
        ),
        dependency_targets,
    })
}

/// Compile a single implicit variable from an LTM equation to symbolic
/// bytecodes: its [`ltm_implicit_fragment_input`], lowered per phase and
/// emitted through the same emission entry point every other fragment uses.
///
/// This is analogous to `compile_implicit_var_fragment` but for implicit
/// variables generated by LTM equation parsing rather than by
/// SourceVariable parsing. Capture phase presence follows its snapshot
/// consumers; other helpers retain their initial plus flow/stock compilation.
/// Assembly appends them to the runlists by bytecode presence because they are
/// not part of the dependency graph.
pub(crate) fn compile_ltm_implicit_var_fragment(
    db: &dyn Db,
    meta: &LtmImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
    module_input_names: &[String],
    mut diagnostics: Option<&mut Vec<crate::db::Diagnostic>>,
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

    let input = match ltm_implicit_fragment_input(db, meta, model, project, module_input_names) {
        Ok(input) => input,
        Err(failures) => {
            if let Some(output) = diagnostics.as_deref_mut() {
                extend_ltm_failures(
                    output,
                    failures,
                    model.name(db),
                    &implicit_name,
                    Some(&meta.ltm_parent_name),
                );
            }
            return None;
        }
    };
    let emit_ctx = input.emit_ctx();
    let mut compile_phase = |is_initial: bool, report_failure: bool| -> Option<PerVarBytecodes> {
        match lower_fragment(&input, is_initial) {
            Ok(var_result) if report_failure && diagnostics.is_some() => {
                match crate::db::assemble::compile_phase_to_per_var_bytecodes_reporting(
                    &emit_ctx,
                    &var_result.ast,
                ) {
                    Ok(bytecodes) => Some(bytecodes),
                    Err(err) => {
                        if let Some(output) = diagnostics.as_deref_mut() {
                            extend_ltm_failures(
                                output,
                                [crate::db::Diagnostic::assembly(
                                    err,
                                    crate::db::DiagnosticSeverity::Warning,
                                )],
                                model.name(db),
                                &implicit_name,
                                Some(&meta.ltm_parent_name),
                            );
                        }
                        None
                    }
                }
            }
            Ok(var_result) => compile_phase_to_per_var_bytecodes(&emit_ctx, &var_result.ast),
            Err(err) => {
                if report_failure && let Some(output) = diagnostics.as_deref_mut() {
                    extend_ltm_failures(
                        output,
                        ltm_lowering_diagnostics(&input, &err),
                        model.name(db),
                        &implicit_name,
                        Some(&meta.ltm_parent_name),
                    );
                }
                None
            }
        }
    };

    let capture = meta.variable.capture();
    let capture_needs_initials = capture.is_some_and(|capture| capture.kind().needs_initials());
    let initial_bytecodes = if capture.is_none_or(|capture| capture.kind().needs_initials()) {
        compile_phase(true, capture_needs_initials)
    } else {
        None
    };

    // Every required-phase failure is retained. Identical diagnostics from a
    // combined capture's initial and flow phases collapse in producer order.
    let flow_bytecodes =
        if !meta.is_stock && capture.is_none_or(|capture| capture.kind().needs_flows()) {
            compile_phase(false, true)
        } else {
            None
        };

    let stock_bytecodes = if meta.is_stock || meta.is_module {
        compile_phase(false, true)
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
        flow_locally_invariant: None,
    })
}

#[cfg(test)]
mod array_operand_bounds_gate_tests {
    use super::expr_requires_array_operand_bounds;
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

    /// Every builtin whose signature consumes an array or table must flag the
    /// gate, while the scalar near-misses (n-ary MEAN, 2-arg MIN/MAX) must not
    /// flag it on their own. The signature table makes new variants exhaustive;
    /// this test pins the classification of existing variants. SIZE is an
    /// important row because a reducer-only list once omitted it (GH #738).
    #[test]
    fn array_operand_bounds_gate_covers_each_consumer() {
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
                expr_requires_array_operand_bounds(&app(builtin)),
                "{name} consumes array bounds and must flag the gate"
            );
        }

        let decomposing_rank = app(BuiltinFn::Rank(c(), c()));
        assert!(
            expr_requires_array_operand_bounds(&decomposing_rank),
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
                !expr_requires_array_operand_bounds(&app(builtin)),
                "{name} consumes no array bounds and must not flag the gate alone"
            );
        }
    }

    /// An array consumer nested inside a scalar construct
    /// (a builtin argument, an Op2 operand, a subscript index) must still
    /// flag the gate because the materializer walks every expression edge.
    #[test]
    fn array_operand_bounds_gate_finds_nested_consumers() {
        let nested_in_builtin = app(BuiltinFn::Abs(Box::new(app(BuiltinFn::Sum(c())))));
        assert!(expr_requires_array_operand_bounds(&nested_in_builtin));

        let nested_in_op2 = Expr2::Op2(
            crate::ast::BinaryOp::Mul,
            c(),
            Box::new(app(BuiltinFn::Size(c()))),
            None,
            Loc::default(),
        );
        assert!(expr_requires_array_operand_bounds(&nested_in_op2));

        let nested_in_subscript = Expr2::Subscript(
            Ident::<Canonical>::new("a"),
            vec![IndexExpr2::Expr(app(BuiltinFn::Sum(c())))],
            None,
            Loc::default(),
        );
        assert!(expr_requires_array_operand_bounds(&nested_in_subscript));

        let plain = Expr2::Op2(
            crate::ast::BinaryOp::Add,
            c(),
            Box::new(app(BuiltinFn::Previous(c(), c()))),
            None,
            Loc::default(),
        );
        assert!(!expr_requires_array_operand_bounds(&plain));
    }
}
