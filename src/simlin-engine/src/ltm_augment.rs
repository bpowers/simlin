// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! LTM project augmentation - adds synthetic variables for link and loop scores
//!
//! This module generates synthetic variables for Loops That Matter (LTM) analysis.
//! The generated equations use the intrinsic two-argument `PREVIOUS(value, initial)`
//! function. First- and second-timestep guards are expressed explicitly with
//! `TIME = INITIAL_TIME` and `PREVIOUS(TIME, INITIAL_TIME) = INITIAL_TIME`.

use crate::ast::{Expr0, IndexExpr0, print_eqn};
use crate::builtins::UntypedBuiltinFn;
use crate::canonicalize;
use crate::common::{Canonical, Ident};
use crate::datamodel::{self, Equation};
use crate::lexer::LexerType;
use crate::ltm::{CyclePartitions, Loop, normalize_module_ref};
use crate::variable::{Variable, identifier_set};
use std::collections::{HashMap, HashSet};

use crate::db::RefShape;

/// Classify an `Expr0` subscript's shape based on its indices.
///
/// Mirrors `db_analysis::resolve_literal_index`'s classification logic but at
/// the `Expr0` (parsed-AST) level — used by `wrap_non_matching_in_previous`
/// before subscripts have been lowered to `Expr2`. Each input string in
/// `source_dim_elements` is the canonical lowercase element name for the
/// corresponding source dimension, in source-declared order.
///
/// Rules:
/// - any `IndexExpr0::Wildcard` → `RefShape::Wildcard`
/// - all indices are literal element names that match the source's
///   declared elements (or parseable integer literals for indexed
///   dimensions) → `RefShape::FixedIndex(canonical_names)`
/// - otherwise (StarRange, DimPosition, Range, non-literal Expr, or a
///   literal that doesn't match) → `RefShape::DynamicIndex`
///
/// The match tries each index against the dimension at that position
/// first, then falls back to scanning all dimensions. This keeps the
/// classifier robust when callers pass dimensions in source-declared
/// order but the subscript indices may not align 1:1 with dimension
/// positions in unusual cases. Defensive `DynamicIndex` for unknown
/// names ensures the worst case is over-conservative wrapping rather
/// than incorrectly matching the live shape.
/// Whether a single subscript index is a "literal element" reference --
/// i.e., a `Var` naming a known dimension element or an integer literal.
/// These are dimension references at runtime, not variable references,
/// and must not be PREVIOUS-wrapped even when their textual form
/// collides with a user-variable name.
///
/// `position` is the index's 0-based position in the subscript; literal
/// `Var` names are matched against the dimension at that position first
/// and then against any dimension as a fallback (mirroring
/// `classify_expr0_subscript_shape`'s match rules).
fn is_literal_element_index(
    idx: &IndexExpr0,
    position: usize,
    source_dim_elements: &[Vec<String>],
) -> bool {
    resolve_literal_element_index(idx, position, source_dim_elements).is_some()
}

/// Resolve a single subscript index to a literal element name, mirroring
/// `db_analysis::resolve_literal_index` (the Expr2 sibling) so both
/// classifiers agree on what counts as a "literal element". The two
/// must stay in sync: the edge emitter uses the Expr2 classifier and
/// the partial-equation builder uses this Expr0 sibling -- if they
/// disagree (for example on out-of-range integer literals), the edge
/// emitter classifies as `DynamicIndex` while the partial builder
/// classifies as `FixedIndex(...)`, the shape comparison in
/// `wrap_non_matching_in_previous` fails, and the live reference is
/// wrapped in `PREVIOUS()` -- silently zeroing the link score.
///
/// Element names appear as `Var` nodes; integer literals appear as
/// `Const` nodes whose text is the integer. Either form is validated
/// by membership in `source_dim_elements`. For an indexed dim of size
/// N, `dimension_element_names` produces `["1", "2", ..., "N"]`, so a
/// `Const("999", ...)` over an indexed dim of size 5 won't match and
/// falls through to `None`. Matching prefers the dim at the index's
/// position, falling back to any dim if not found there.
fn resolve_literal_element_index(
    idx: &IndexExpr0,
    position: usize,
    source_dim_elements: &[Vec<String>],
) -> Option<String> {
    let candidate = match idx {
        IndexExpr0::Expr(Expr0::Var(name, _)) => canonicalize(name.as_str()).into_owned(),
        IndexExpr0::Expr(Expr0::Const(s, _, _)) => {
            // Only integer literals could be element references; other
            // constants (floats, strings, etc.) never are.
            s.parse::<u32>().ok()?;
            s.clone()
        }
        _ => return None,
    };
    let matches_position = position < source_dim_elements.len()
        && source_dim_elements[position]
            .iter()
            .any(|e| e == &candidate);
    let matches_any = !matches_position
        && source_dim_elements
            .iter()
            .any(|dim| dim.iter().any(|e| e == &candidate));
    if matches_position || matches_any {
        Some(candidate)
    } else {
        None
    }
}

fn classify_expr0_subscript_shape(
    indices: &[IndexExpr0],
    source_dim_elements: &[Vec<String>],
) -> RefShape {
    if indices
        .iter()
        .any(|idx| matches!(idx, IndexExpr0::Wildcard(_)))
    {
        return RefShape::Wildcard;
    }
    let mut elems = Vec::with_capacity(indices.len());
    for (i, idx) in indices.iter().enumerate() {
        // Use the same resolver as `is_literal_element_index` so this
        // classifier and the Expr2 sibling
        // (`db_analysis::resolve_literal_index`) agree on what counts
        // as a literal element. Integer literals are validated against
        // `source_dim_elements` (which contains `["1", ..., "size"]`
        // for indexed dims), so out-of-range integers like `pop[999]`
        // over a size-5 indexed dim fall through to `DynamicIndex` --
        // matching what the edge emitter sees and avoiding the
        // shape-mismatch that would zero out the partial.
        match resolve_literal_element_index(idx, i, source_dim_elements) {
            Some(elem) => elems.push(elem),
            None => return RefShape::DynamicIndex,
        }
    }
    RefShape::FixedIndex(elems)
}

/// Walk an `Expr0` tree and wrap variable references in `PREVIOUS()` except
/// those whose access shape matches the live shape for the given source.
///
/// `live_source` identifies the source variable whose live shape is held
/// out from `PREVIOUS` wrapping. `live_shape` declares which AST occurrences
/// of that source remain live; all other occurrences (and all references
/// to other sources in the same expression) are wrapped.
///
/// `other_deps` is the set of canonical idents for non-`live_source`
/// dependencies that must be wrapped; nodes referencing names not in this
/// set and not equal to `live_source` are left alone (function names and
/// unknown identifiers). Indices of subscripts are recursively transformed
/// even when the outer subscript matches the live shape, so nested
/// references like `arr[other_var]` still get wrapped.
pub(crate) fn wrap_non_matching_in_previous(
    expr: Expr0,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    other_deps: &HashSet<Ident<Canonical>>,
    source_dim_elements: &[Vec<String>],
) -> Expr0 {
    match expr {
        Expr0::Const(..) => expr,
        Expr0::Var(ref ident, loc) => {
            let canonical = Ident::new(ident.as_str());
            if &canonical == live_source {
                // The bare-Var occurrence matches `Bare`. Any other live
                // shape (FixedIndex / Wildcard / DynamicIndex) doesn't
                // match a bare reference, so we wrap.
                if matches!(live_shape, RefShape::Bare) {
                    expr
                } else {
                    Expr0::App(UntypedBuiltinFn("PREVIOUS".to_string(), vec![expr]), loc)
                }
            } else if other_deps.contains(&canonical) {
                Expr0::App(UntypedBuiltinFn("PREVIOUS".to_string(), vec![expr]), loc)
            } else {
                expr
            }
        }
        Expr0::Subscript(ident, indices, loc) => {
            // Classify the subscript's shape using the ORIGINAL indices
            // BEFORE recursing into them. If a user variable shares a
            // name with a dimension element (e.g., a variable also named
            // `NYC`), recursing first would rewrite `Var(NYC)` as
            // `App(PREVIOUS, [Var(NYC)])`, and then classification would
            // fall through to `DynamicIndex`, breaking a live FixedIndex
            // shape match.
            let canonical = Ident::new(ident.as_str());
            let subscript_shape = classify_expr0_subscript_shape(&indices, source_dim_elements);
            if &canonical == live_source && &subscript_shape == live_shape {
                // Live reference: the OUTER subscript stays unwrapped.
                // Decide per-index whether to recurse:
                //
                //   - Literal element refs (Var matching a dim element,
                //     or an integer literal in an indexed dim) are
                //     dimension references at runtime; leave them
                //     verbatim so a variable/element name collision
                //     doesn't wrap them.
                //
                //   - Wildcard tokens (`*`) have no inner content to
                //     wrap; recursing is a no-op so doing it for
                //     uniformity is fine.
                //
                //   - Non-literal indices (expressions like `idx + helper`
                //     in `RefShape::DynamicIndex`) are computational
                //     content; recurse so any `other_deps` referenced
                //     inside get held at PREVIOUS for ceteris-paribus.
                //
                // Without the per-index split, DynamicIndex live refs
                // would skip wrapping inner deps and the partial
                // equation would no longer be ceteris-paribus.
                let indices: Vec<IndexExpr0> = indices
                    .into_iter()
                    .enumerate()
                    .map(|(i, idx)| {
                        if is_literal_element_index(&idx, i, source_dim_elements) {
                            idx
                        } else {
                            wrap_index_non_matching_in_previous(
                                idx,
                                live_source,
                                live_shape,
                                other_deps,
                                source_dim_elements,
                            )
                        }
                    })
                    .collect();
                return Expr0::Subscript(ident, indices, loc);
            }
            // Non-live reference: recurse into indices so any nested
            // user-variable references get wrapped, then build the new
            // subscript. If the outer ident is itself a dep, wrap the
            // whole thing.
            let indices: Vec<IndexExpr0> = indices
                .into_iter()
                .map(|idx| {
                    wrap_index_non_matching_in_previous(
                        idx,
                        live_source,
                        live_shape,
                        other_deps,
                        source_dim_elements,
                    )
                })
                .collect();
            let subscript = Expr0::Subscript(ident, indices, loc);
            if &canonical == live_source || other_deps.contains(&canonical) {
                Expr0::App(
                    UntypedBuiltinFn("PREVIOUS".to_string(), vec![subscript]),
                    loc,
                )
            } else {
                subscript
            }
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            let args = args
                .into_iter()
                .map(|a| {
                    wrap_non_matching_in_previous(
                        a,
                        live_source,
                        live_shape,
                        other_deps,
                        source_dim_elements,
                    )
                })
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, inner, loc) => Expr0::Op1(
            op,
            Box::new(wrap_non_matching_in_previous(
                *inner,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
            )),
            loc,
        ),
        Expr0::Op2(op, lhs, rhs, loc) => Expr0::Op2(
            op,
            Box::new(wrap_non_matching_in_previous(
                *lhs,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
            )),
            Box::new(wrap_non_matching_in_previous(
                *rhs,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
            )),
            loc,
        ),
        Expr0::If(cond, then_expr, else_expr, loc) => Expr0::If(
            Box::new(wrap_non_matching_in_previous(
                *cond,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
            )),
            Box::new(wrap_non_matching_in_previous(
                *then_expr,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
            )),
            Box::new(wrap_non_matching_in_previous(
                *else_expr,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
            )),
            loc,
        ),
    }
}

fn wrap_index_non_matching_in_previous(
    index: IndexExpr0,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    other_deps: &HashSet<Ident<Canonical>>,
    source_dim_elements: &[Vec<String>],
) -> IndexExpr0 {
    match index {
        IndexExpr0::Expr(e) => IndexExpr0::Expr(wrap_non_matching_in_previous(
            e,
            live_source,
            live_shape,
            other_deps,
            source_dim_elements,
        )),
        IndexExpr0::Range(l, r, loc) => IndexExpr0::Range(
            wrap_non_matching_in_previous(
                l,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
            ),
            wrap_non_matching_in_previous(
                r,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
            ),
            loc,
        ),
        other => other,
    }
}

/// Build a partial equation for a per-shape link score.
///
/// Parses `equation_text`, computes the set of "other" deps (everything
/// in `deps` other than `live_source`, also dropping module-prefixed
/// references that normalize to `live_source`), and then walks the AST
/// wrapping every reference to those other deps in `PREVIOUS()`. The
/// reference to `live_source` is left live only at occurrences whose
/// shape matches `live_shape`; other occurrences of `live_source` are
/// wrapped too.
///
/// The function always canonicalizes via parse + `print_eqn`, even when
/// no wrapping happens, so the result is always in the canonical equation
/// format expected by downstream parsing. The performance impact is
/// negligible because LTM equations are short.
pub(crate) fn build_partial_equation_shaped(
    equation_text: &str,
    deps: &HashSet<Ident<Canonical>>,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    source_dim_elements: &[Vec<String>],
) -> String {
    let other_deps: HashSet<Ident<Canonical>> = deps
        .iter()
        .filter(|d| *d != live_source && normalize_module_ref(d) != *live_source)
        .cloned()
        .collect();

    let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) else {
        return equation_text.to_lowercase();
    };

    let transformed = wrap_non_matching_in_previous(
        ast,
        live_source,
        live_shape,
        &other_deps,
        source_dim_elements,
    );
    print_eqn(&transformed)
}

/// Quote an identifier for use in an equation string.
/// Identifiers with special characters (like $, ⁚) need double quotes.
pub(crate) fn quote_ident(ident: &str) -> String {
    if ident.chars().all(|c| c.is_alphanumeric() || c == '_') {
        ident.to_string()
    } else {
        format!("\"{ident}\"")
    }
}

/// Compute the canonical synthetic-variable name for a per-shape link score.
///
/// Naming convention (Phase 3):
/// - `Bare`: `$⁚ltm⁚link_score⁚{from}→{to}` — the legacy A2A/scalar form,
///   unchanged.
/// - `FixedIndex(elems)`: `$⁚ltm⁚link_score⁚{from}[{elems_joined}]→{to}` —
///   the per-element prefixed-from form already used by
///   `try_cross_dimensional_link_scores`.
/// - `Wildcard`: `$⁚ltm⁚link_score⁚{from}→{to}⁚wildcard` — ALWAYS suffixed,
///   even when no Bare reference coexists. This makes the link score name a
///   stable function of `(from, to, shape)` so the discovery parser doesn't
///   need to reason about per-model collisions.
/// - `DynamicIndex`: `$⁚ltm⁚link_score⁚{from}→{to}⁚dynamic` — analogous to
///   Wildcard.
///
/// The Unicode separators `\u{205A}` (TWO DOT PUNCTUATION) and `\u{2192}`
/// (RIGHTWARDS ARROW) are intentional: they collide with no legal
/// identifier, so the generated names cannot be confused with user
/// variables. The `parse_link_offsets` discovery parser strips the
/// `⁚wildcard` / `⁚dynamic` suffix before resolving offsets.
/// Suffix appended to the `to` name for `Wildcard`-shape link scores.
/// Always appended, regardless of whether other shapes coexist.
pub(crate) const LINK_SCORE_WILDCARD_SUFFIX: &str = "\u{205A}wildcard";

/// Suffix appended to the `to` name for `DynamicIndex`-shape link scores.
/// Always appended, regardless of whether other shapes coexist.
pub(crate) const LINK_SCORE_DYNAMIC_SUFFIX: &str = "\u{205A}dynamic";

/// All shape suffixes that `link_score_var_name` may append. The discovery
/// parser strips one of these from the end of the `to` name before
/// resolving offsets.
pub(crate) const LINK_SCORE_SHAPE_SUFFIXES: &[&str] =
    &[LINK_SCORE_WILDCARD_SUFFIX, LINK_SCORE_DYNAMIC_SUFFIX];

pub(crate) fn link_score_var_name(from: &str, to: &str, shape: &RefShape) -> String {
    let from_part = match shape {
        RefShape::FixedIndex(elems) => format!("{}[{}]", from, elems.join(",")),
        _ => from.to_string(),
    };
    let to_part = match shape {
        RefShape::Wildcard => format!("{}{}", to, LINK_SCORE_WILDCARD_SUFFIX),
        RefShape::DynamicIndex => format!("{}{}", to, LINK_SCORE_DYNAMIC_SUFFIX),
        _ => to.to_string(),
    };
    format!(
        "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}",
        from_part, to_part
    )
}

/// Generate absolute loop score variables for all loops.
///
/// Emits one `$⁚ltm⁚loop_score⁚{id}` synthetic aux per loop (product of
/// the loop's link scores).  Relative loop scores are no longer emitted
/// here: the per-partition `rel_loop_score` was O(P²) text per partition
/// and dominated compile memory on dense models (see
/// `docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md`).  The
/// normalization now happens post-simulation in
/// [`crate::ltm_post::compute_rel_loop_scores`].  `partitions` is still
/// accepted so the signature matches the call site's precomputed data
/// and to keep the option open for future partition-aware emission
/// (e.g., a bounded per-partition denominator aux if we ever need one).
pub(crate) fn generate_loop_score_variables(
    loops: &[Loop],
    partitions: &CyclePartitions,
    emitted_link_score_names: &HashSet<String>,
) -> HashMap<Ident<Canonical>, datamodel::Variable> {
    let mut loop_vars = HashMap::new();

    // Tracing is opt-in via LTM_BENCH_TRACE=1.  When disabled, the only
    // per-iteration overhead is an integer add for the byte counter and
    // one branch-predictor-friendly zero-compare, so production cost is
    // negligible; when enabled the tracer logs every 10_000 loops so we
    // can slope-fit equation-text growth and correlate it with RSS.
    let trace_on = std::env::var("LTM_BENCH_TRACE").is_ok();
    let mut loop_score_bytes: u64 = 0;

    if trace_on {
        eprintln!(
            "[ltm-trace] generate_loop_score_variables start loops={} partitions={} \
             rss_mib={:.1}",
            loops.len(),
            partitions.partitions.len(),
            read_rss_mib().unwrap_or(0.0),
        );
    }

    for (i, loop_item) in loops.iter().enumerate() {
        let var_name = format!("$⁚ltm⁚loop_score⁚{}", loop_item.id);
        let equation = generate_loop_score_equation(loop_item, emitted_link_score_names);
        loop_score_bytes += equation.len() as u64;
        let ltm_var = create_aux_variable(&var_name, &equation);
        loop_vars.insert(Ident::new(&var_name), ltm_var);
        if trace_on && should_trace(i + 1) {
            eprintln!(
                "[ltm-trace] pass=loop_score i={} cum_loop_bytes={} rss_mib={:.1}",
                i + 1,
                loop_score_bytes,
                read_rss_mib().unwrap_or(0.0),
            );
        }
    }

    if trace_on {
        eprintln!(
            "[ltm-trace] generate_loop_score_variables done loops={} loop_bytes={} \
             rss_mib={:.1}",
            loops.len(),
            loop_score_bytes,
            read_rss_mib().unwrap_or(0.0),
        );
    }

    loop_vars
}

/// Decide whether iteration `n` (1-based) should emit a trace line.
///
/// We want early iterations densely (so we see the scaling curve
/// even if we OOM before completing the first 10_000 loops on a dense
/// partition) and later iterations sparsely (so we don't spam the log
/// for millions of loops).  Rule: log on every power of two up to and
/// including 8192, then every 10_000 after that.  Powers of two give
/// ~14 lines of early-curve data; 10_000 cadence gives steady-state
/// measurements during long runs.
fn should_trace(n: usize) -> bool {
    if n == 0 {
        return false;
    }
    if n <= 8192 {
        n.is_power_of_two()
    } else {
        n.is_multiple_of(10_000) || n.is_power_of_two()
    }
}

/// Resident-set size in MiB, or `None` if the kernel does not expose
/// `/proc/self/status` (e.g. non-Linux or wasm builds).  Used only by
/// the `LTM_BENCH_TRACE` instrumentation above, so an unavailable
/// reading degrades to a zero in the log rather than failing.
#[cfg(target_os = "linux")]
fn read_rss_mib() -> Option<f64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb: u64 = rest.trim().trim_end_matches(" kB").trim().parse().ok()?;
            return Some(kb as f64 / 1024.0);
        }
    }
    None
}

#[cfg(not(target_os = "linux"))]
fn read_rss_mib() -> Option<f64> {
    None
}

/// Generate the equation for a link score variable.
/// Exposed as `generate_link_score_equation_for_link` for use by tracked
/// functions in `db.rs`.
///
/// `shape` selects which AST occurrences of `from` remain live in the
/// partial equation; non-matching occurrences (and every reference to
/// other deps) are wrapped in `PREVIOUS()`. `source_dim_elements` carries
/// the source variable's dimension element names (one inner vec per
/// dimension, in source-declared order, canonical lowercase) so that
/// literal index names like `[NYC]` can be classified as `FixedIndex`
/// rather than the conservative `DynamicIndex` fallback.
///
/// Flow-to-stock links use a fixed structural formula and ignore both
/// `shape` and `source_dim_elements`.
pub(crate) fn generate_link_score_equation_for_link(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    to_var: &Variable,
    all_vars: &HashMap<Ident<Canonical>, Variable>,
) -> String {
    generate_link_score_equation(from, to, shape, source_dim_elements, to_var, all_vars)
}

/// Generate the equation for a link score variable
fn generate_link_score_equation(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    to_var: &Variable,
    all_vars: &HashMap<Ident<Canonical>, Variable>,
) -> String {
    // Check if this is a flow-to-stock link
    let is_flow_to_stock = matches!(to_var, Variable::Stock { .. })
        && matches!(
            all_vars.get(from),
            Some(Variable::Var { is_flow: true, .. })
        );

    // Check if this is a stock-to-flow link
    let is_stock_to_flow = matches!(all_vars.get(from), Some(Variable::Stock { .. }))
        && matches!(to_var, Variable::Var { is_flow: true, .. });

    if is_flow_to_stock {
        // Flow-to-stock uses a fixed structural formula -- no AST parse,
        // so neither `shape` nor `source_dim_elements` matter here.
        generate_flow_to_stock_equation(from.as_str(), to.as_str(), to_var)
    } else if is_stock_to_flow {
        // Use stock-to-flow formula
        generate_stock_to_flow_equation(from, to, shape, source_dim_elements, to_var)
    } else {
        // Use standard auxiliary-to-auxiliary formula
        generate_auxiliary_to_auxiliary_equation(from, to, shape, source_dim_elements, to_var)
    }
}

/// Generate auxiliary-to-auxiliary link score equation
fn generate_auxiliary_to_auxiliary_equation(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    to_var: &Variable,
) -> String {
    use crate::ast::Ast;

    // Get the equation text of the 'to' variable.  Prefer the AST when
    // available because the `eqn` field holds the *original* text (e.g.,
    // "SMTH1(x, 5)") while the AST holds the post-module-expansion form
    // (e.g., Var("$⁚s⁚0⁚smth1·output")).  Using the AST-derived text
    // ensures the identifiers in the equation match those in `deps`.
    let to_equation = if let Some(ast) = to_var.ast() {
        match ast {
            Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => crate::patch::expr2_to_string(expr),
            _ => match to_var {
                Variable::Stock {
                    eqn: Some(Equation::Scalar(eq)),
                    ..
                }
                | Variable::Var {
                    eqn: Some(Equation::Scalar(eq)),
                    ..
                } => eq.clone(),
                _ => "0".to_string(),
            },
        }
    } else {
        match to_var {
            Variable::Stock {
                eqn: Some(Equation::Scalar(eq)),
                ..
            }
            | Variable::Var {
                eqn: Some(Equation::Scalar(eq)),
                ..
            } => eq.clone(),
            _ => "0".to_string(),
        }
    };

    // Get dependencies of the 'to' variable
    let deps = if let Some(ast) = to_var.ast() {
        identifier_set(ast, &[], None)
    } else {
        HashSet::new()
    };

    let partial_eq =
        build_partial_equation_shaped(&to_equation, &deps, from, shape, source_dim_elements);

    let from_source_q = shape_aware_source_ref(from.as_str(), shape);
    let to_q = quote_ident(to.as_str());

    // Using SAFEDIV for both divisions
    // Note: We still need the outer check for when EITHER is zero, since we multiply the results
    let abs_part = format!(
        "ABS(SAFEDIV((({partial_eq}) - PREVIOUS({to_q})), ({to_q} - PREVIOUS({to_q})), 0))",
    );
    let sign_part = format!(
        "SIGN(SAFEDIV((({partial_eq}) - PREVIOUS({to_q})), ({from_source_q} - PREVIOUS({from_source_q})), 0))",
    );

    // Return 0 at the initial timestep when PREVIOUS values don't exist yet
    format!(
        "if \
            (TIME = INITIAL_TIME) \
            then 0 \
            else if \
                (({to_q} - PREVIOUS({to_q})) = 0) OR (({from_source_q} - PREVIOUS({from_source_q})) = 0) \
                then 0 \
                else {abs_part} * {sign_part}",
    )
}

/// Render the source reference that drives the link-score's denominator
/// (the SIGN normalizer and the early-return zero-guard) for a given
/// shape. The denominator must match the *live* source reference left
/// in `partial_eq` so SAFEDIV captures the same source the partial
/// isolates.
///
///   - `Bare` -> `from` (per-element under A2A; the partial keeps the
///     bare reference live, so per-element Δfrom is correct).
///   - `FixedIndex(elems)` -> `from[elems_joined]` rendered as a
///     subscript expression (NOT a quoted ident) so the LTM equation
///     parser interprets it as a per-element subscript matching the
///     `from[elem]` reference left live in `partial_eq`. Per-element-
///     target normalization must use Δfrom[elem], not Δfrom[r],
///     otherwise the cross-element sensitivity gets divided by the
///     wrong source delta and can flip sign or collapse to zero.
///   - `Wildcard` / `DynamicIndex` -> `from` (TODO: the truly principled
///     denominator would be the change in the aggregate
///     `SUM(from[*])` / `from[expr]` referenced live in the partial,
///     but that requires re-parsing partial_eq or threading the AST
///     subtree through. For now we keep the variable-level Δfrom; the
///     resulting under/over-counting is the same documented edge-
///     aliasing limitation already pinned by integration tests).
fn shape_aware_source_ref(from: &str, shape: &RefShape) -> String {
    match shape {
        RefShape::FixedIndex(elems) if !elems.is_empty() => {
            // Subscript syntax, NOT quote_ident: a literal `pop[nyc]`
            // parses as a Subscript node (per-element reference), while
            // `"pop[nyc]"` would parse as a quoted ident referring to
            // a synthetic variable that doesn't exist.
            format!("{}[{}]", quote_ident(from), elems.join(","))
        }
        _ => quote_ident(from),
    }
}

/// Generate flow-to-stock link score equation
fn generate_flow_to_stock_equation(flow: &str, stock: &str, stock_var: &Variable) -> String {
    // Check if this flow is an inflow or outflow
    let is_inflow = if let Variable::Stock { inflows, .. } = stock_var {
        inflows.iter().any(|f| f.as_str() == flow)
    } else {
        true // Default to inflow
    };

    let sign = if is_inflow { "" } else { "-" };

    // Per the corrected 2023 formula (Schoenberg et al., Eq. 3):
    //   LS(inflow -> S)  = |Delta(i) / (Delta(S_t) - Delta(S_{t-dt}))| * (+1)
    //   LS(outflow -> S) = |Delta(o) / (Delta(S_t) - Delta(S_{t-dt}))| * (-1)
    //
    // The polarity is structural (fixed +1/-1), not dynamic.  ABS ensures
    // the magnitude is always positive; the sign is applied outside.
    //
    // The numerator uses PREVIOUS values to align timing with the denominator.
    // At time t, the flow at t-1 (PREVIOUS(flow)) is what drove the stock change from t-1 to t.
    // We measure the change in that causal flow: flow(t-1) - flow(t-2).
    let numerator = format!("(PREVIOUS({flow}) - PREVIOUS(PREVIOUS({flow})))");
    let denominator = format!(
        "(({stock} - PREVIOUS({stock})) - (PREVIOUS({stock}) - PREVIOUS(PREVIOUS({stock}))))"
    );

    // Return 0 for the first two timesteps when we don't have enough history for second-order differences
    format!(
        "if \
            (TIME = INITIAL_TIME) OR (PREVIOUS(TIME, INITIAL_TIME) = INITIAL_TIME) \
            then 0 \
            else {sign}ABS(SAFEDIV({numerator}, {denominator}, 0))"
    )
}

/// Generate stock-to-flow link score equation
fn generate_stock_to_flow_equation(
    stock: &Ident<Canonical>,
    flow: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    flow_var: &Variable,
) -> String {
    // For stock-to-flow, we need to calculate how the stock influences the flow
    // This is similar to auxiliary-to-auxiliary but we know the 'from' is a stock

    // Get the flow equation text.  Prefer the AST when available because
    // it handles both Scalar and ApplyToAll (arrayed) equations, whereas
    // the raw `eqn` field only covers Scalar.  Without this, arrayed flows
    // fall through to "0" and produce a zero link score.
    use crate::ast::Ast;

    let flow_equation = if let Some(ast) = flow_var.ast() {
        match ast {
            Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => crate::patch::expr2_to_string(expr),
            _ => match flow_var {
                Variable::Var {
                    eqn: Some(Equation::Scalar(eq)),
                    ..
                } => eq.clone(),
                _ => "0".to_string(),
            },
        }
    } else {
        match flow_var {
            Variable::Var {
                eqn: Some(Equation::Scalar(eq)),
                ..
            } => eq.clone(),
            _ => "0".to_string(),
        }
    };

    // Get dependencies of the flow variable
    let deps = if let Some(ast) = flow_var.ast() {
        identifier_set(ast, &[], None)
    } else {
        HashSet::new()
    };

    let partial_eq =
        build_partial_equation_shaped(&flow_equation, &deps, stock, shape, source_dim_elements);

    // Link score formula from LTM paper: |Δxz/Δz| × sign(Δxz/Δx)
    // For stock-to-flow: x=stock, z=flow. The stock side respects
    // shape: a FixedIndex(elem) link score must normalize by
    // Δstock[elem], not the variable-level Δstock; otherwise the
    // SAFEDIV captures the wrong source delta (same bug class as the
    // auxiliary-to-auxiliary path -- see `shape_aware_source_ref`).
    let flow_diff = format!("({flow} - PREVIOUS({flow}))", flow = flow.as_str());
    let stock_source_q = shape_aware_source_ref(stock.as_str(), shape);
    let stock_diff = format!("({stock_source_q} - PREVIOUS({stock_source_q}))");
    let partial_change = format!(
        "(({partial_eq}) - PREVIOUS({flow}))",
        partial_eq = partial_eq,
        flow = flow.as_str()
    );

    let abs_part = format!("ABS(SAFEDIV({partial_change}, {flow_diff}, 0))");
    let sign_part = format!("SIGN(SAFEDIV({partial_change}, {stock_diff}, 0))");

    // Return 0 at the initial timestep when PREVIOUS values don't exist yet
    format!(
        "if \
            (TIME = INITIAL_TIME) \
            then 0 \
            else if \
                ({flow_diff} = 0) OR ({stock_diff} = 0) \
                then 0 \
                else {abs_part} * {sign_part}"
    )
}

/// Resolve the link-score variable name a downstream consumer (loop
/// score, pathway score, composite score) should reference for a single
/// `(from, to)` edge.
///
/// `emit_per_shape_link_scores` emits names per-shape based on what the
/// target's AST contains: `pop→share` (Bare), `pop→share⁚wildcard`
/// (Wildcard), `pop[nyc]→share` (FixedIndex via element-level `from`
/// prefix), and so on. The downstream consumer doesn't carry the access
/// shape, so we resolve at equation-generation time by trying candidate
/// names in priority order against the set of names actually emitted.
///
/// Priority:
///
/// 1. `Bare` -- the canonical `{from}→{to}` form. Cross-dimensional
///    edges naturally produce a bracketed `from` like `"pop[nyc]"`,
///    which combines with Bare naming to match the per-element name
///    `try_cross_dimensional_link_scores` emits. This is also the only
///    correct choice when both Bare and a suffixed variant coexist, so
///    the documented edge-aliasing limitation stays consistent.
///
/// 2. `FixedIndex` -- e.g., `share[r] = pop[NYC]` where the only AST
///    occurrence of `pop` is a literal-element subscript. The emitted
///    name is `pop[nyc]→share`. We scan `emitted` for any name matching
///    `{from}[...]→{to}` (no shape suffix) and pick the lexicographically
///    first match, deterministically resolving multi-element targets
///    like `share[r] = pop[NYC] + pop[BOSTON]`. Picking one is the
///    documented edge-aliasing under-counting; here it manifests across
///    multiple FixedIndex variants.
///
/// 3. `Wildcard` (`⁚wildcard` suffix) -- e.g., `share[r] = SUM(pop[*])`
///    where the only AST occurrence of `pop` is inside a wildcard
///    reducer.
///
/// 4. `DynamicIndex` (`⁚dynamic` suffix) -- analogous to Wildcard for
///    expression-indexed subscripts.
///
/// If none of the candidates is in `emitted`, return the Bare canonical
/// name anyway and let the fragment compiler's stub-dep fallback fire.
/// That matches the pre-resolver behavior on the unreachable branch.
pub(crate) fn resolve_link_score_name_for_loop(
    from: &str,
    to: &str,
    emitted: &HashSet<String>,
) -> String {
    let bare = link_score_var_name(from, to, &RefShape::Bare);
    if emitted.contains(&bare) {
        return bare;
    }
    if let Some(fixed) = find_fixed_index_emitted_name(from, to, emitted) {
        return fixed;
    }
    let wildcard = link_score_var_name(from, to, &RefShape::Wildcard);
    if emitted.contains(&wildcard) {
        return wildcard;
    }
    let dynamic = link_score_var_name(from, to, &RefShape::DynamicIndex);
    if emitted.contains(&dynamic) {
        return dynamic;
    }
    bare
}

/// Scan `emitted` for any link-score variable name matching the
/// FixedIndex pattern `{prefix}{from}[...]→{to}` (no shape suffix).
/// Returns the lexicographically first match for determinism.
fn find_fixed_index_emitted_name(
    from: &str,
    to: &str,
    emitted: &HashSet<String>,
) -> Option<String> {
    let prefix = format!("$\u{205A}ltm\u{205A}link_score\u{205A}{from}[");
    let suffix = format!("]\u{2192}{to}");
    let mut matches: Vec<&String> = emitted
        .iter()
        .filter(|n| {
            n.starts_with(&prefix) && n.ends_with(&suffix) && n.len() > prefix.len() + suffix.len()
        })
        .collect();
    matches.sort();
    matches.first().map(|s| (*s).clone())
}

/// Generate the equation for a loop score variable.
///
/// The loop score is the product of all link scores in the loop. The
/// per-element distinction for cross-dimensional edges (e.g.,
/// `pop[nyc]→total_pop`) lives in `link.from` itself; for everything
/// else, the access shape is implicit in which name was actually
/// emitted by `emit_per_shape_link_scores`.
///
/// `emitted_link_score_names` carries every link-score variable name the
/// caller has emitted so far. For each loop link we try the canonical
/// Bare name first (since `try_cross_dimensional_link_scores` and the
/// common Bare-AST case both produce that form) and fall back to the
/// `⁚wildcard` and `⁚dynamic` shape suffixes when only those variants
/// exist (e.g., `share[r] = SUM(pop[*])` emits only the wildcard
/// variant of `pop→share`). Without this resolution the loop_score
/// equation would multiply against a missing variable and the fragment
/// compiler would silently insert a stub dep, dropping the link's
/// contribution.
fn generate_loop_score_equation(
    loop_item: &Loop,
    emitted_link_score_names: &HashSet<String>,
) -> String {
    let link_score_names: Vec<String> = loop_item
        .links
        .iter()
        .map(|link| {
            let name = resolve_link_score_name_for_loop(
                link.from.as_str(),
                link.to.as_str(),
                emitted_link_score_names,
            );
            // Double-quote the variable name so it can be parsed
            format!("\"{name}\"")
        })
        .collect();

    if link_score_names.is_empty() {
        "0".to_string()
    } else {
        link_score_names.join(" * ")
    }
}

/// Create an auxiliary variable with the given equation
fn create_aux_variable(name: &str, equation: &str) -> crate::datamodel::Variable {
    use crate::datamodel;

    datamodel::Variable::Aux(datamodel::Aux {
        ident: canonicalize(name).into_owned(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: "LTM".to_string(),
        units: None, // LTM scores are dimensionless by design, no need to declare
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat {
            visibility: datamodel::Visibility::Public,
            ..datamodel::Compat::default()
        },
    })
}

/// Classification of array-reducing builtins for cross-dimensional link score generation.
///
/// When an arrayed variable feeds a scalar target through a reducing function,
/// each element gets its own scalar link score. The reducer kind determines
/// the equation generation strategy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReducerKind {
    /// SUM, MEAN: partial derivative is algebraically simple.
    /// SUM: partial = PREVIOUS(target) + (source[d] - PREVIOUS(source[d]))
    /// MEAN: same as SUM but divided by the number of elements.
    Linear,
    /// MIN, MAX, STDDEV, RANK: must enumerate all elements explicitly,
    /// wrapping all elements except the current one in PREVIOUS.
    Nonlinear,
    /// SIZE: output is constant (depends only on dimension cardinality).
    /// Link score is always 0; skip generation entirely.
    Constant,
}

/// Collect element names from a dimension as owned strings.
///
/// For `Dimension::Named`, returns the canonical element names.
/// For `Dimension::Indexed`, returns one-based index strings ("1", "2", ...).
/// The engine uses 1-based indexing for indexed dimensions (see
/// `dimensions.rs` `SubscriptIterator` which formats as `elem + 1`).
pub(crate) fn dimension_element_names(dim: &crate::dimensions::Dimension) -> Vec<String> {
    match dim {
        crate::dimensions::Dimension::Named(_, named) => named
            .elements
            .iter()
            .map(|e| e.as_str().to_string())
            .collect(),
        crate::dimensions::Dimension::Indexed(_, size) => {
            (1..=*size).map(|i| i.to_string()).collect()
        }
    }
}

/// Examine the target variable's Expr2 AST to find the array-reducing function
/// applied to the source variable and classify it.
///
/// Walks the Expr2 tree looking for `Expr2::App(builtin, ...)` nodes where
/// the builtin is an array reducer and the argument references the source
/// variable (identified by canonical name). Returns the `ReducerKind`, the
/// uppercase function name (e.g., "SUM", "MIN"), and whether the reducer is
/// the top-level expression (`is_bare`).
///
/// When `is_bare` is false, the reducer is nested inside other arithmetic
/// (e.g., `2 * SUM(population[*])`). Callers should fall back to the
/// delta-ratio approach for nested reducers, because the algebraic shortcut
/// ignores the surrounding arithmetic and produces wrong link scores.
///
/// Returns `None` if no reducing builtin is found for the given source.
pub(crate) fn classify_reducer(
    target_var: &Variable,
    source_ident: &str,
) -> Option<(ReducerKind, &'static str, bool)> {
    use crate::ast::Ast;

    let ast = target_var.ast()?;
    let expr = match ast {
        Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => expr,
        // For arrayed targets with per-element equations, check the default
        // expression if available.
        Ast::Arrayed(_, _, default_expr, _) => default_expr.as_ref()?,
    };

    classify_reducer_in_expr(expr, source_ident, true)
}

/// Recursively search an Expr2 tree for a reducing builtin applied to
/// the source variable.
///
/// `is_top_level` tracks whether we are still at the root of the expression
/// tree. When `true` and the reducer is found at this node, `is_bare` in the
/// result is `true`. Once we recurse into sub-expressions (Op1, Op2, If,
/// non-reducer App arguments), `is_top_level` becomes `false` so any reducer
/// found deeper is correctly flagged as nested.
fn classify_reducer_in_expr(
    expr: &crate::ast::Expr2,
    source_ident: &str,
    is_top_level: bool,
) -> Option<(ReducerKind, &'static str, bool)> {
    use crate::ast::Expr2;

    match expr {
        Expr2::App(builtin, _, _) => {
            // Check if this builtin is a reducer whose argument references
            // the source variable.
            if let Some((kind, name)) = classify_builtin_if_references_source(builtin, source_ident)
            {
                return Some((kind, name, is_top_level));
            }
            // Even if this particular App node isn't the reducer we want,
            // recurse into its arguments to find nested reducers.
            // Any reducer found inside a non-reducer App is nested.
            let mut result = None;
            builtin.for_each_expr_ref(|sub_expr| {
                if result.is_none() {
                    result = classify_reducer_in_expr(sub_expr, source_ident, false);
                }
            });
            result
        }
        Expr2::Op1(_, inner, _, _) => classify_reducer_in_expr(inner, source_ident, false),
        Expr2::Op2(_, lhs, rhs, _, _) => classify_reducer_in_expr(lhs, source_ident, false)
            .or_else(|| classify_reducer_in_expr(rhs, source_ident, false)),
        Expr2::If(cond, then_e, else_e, _, _) => {
            classify_reducer_in_expr(cond, source_ident, false)
                .or_else(|| classify_reducer_in_expr(then_e, source_ident, false))
                .or_else(|| classify_reducer_in_expr(else_e, source_ident, false))
        }
        Expr2::Var(..) | Expr2::Const(..) | Expr2::Subscript(..) => None,
    }
}

/// Check if a BuiltinFn is an array reducer and its argument references the
/// source variable. Returns the `(ReducerKind, function_name)` if so.
fn classify_builtin_if_references_source(
    builtin: &crate::builtins::BuiltinFn<crate::ast::Expr2>,
    source_ident: &str,
) -> Option<(ReducerKind, &'static str)> {
    use crate::builtins::BuiltinFn;

    let canonical_source = canonicalize(source_ident);

    match builtin {
        BuiltinFn::Sum(arg) => {
            if expr_references_var(arg, canonical_source.as_ref()) {
                Some((ReducerKind::Linear, "SUM"))
            } else {
                None
            }
        }
        BuiltinFn::Mean(args) => {
            if args
                .iter()
                .any(|a| expr_references_var(a, canonical_source.as_ref()))
            {
                Some((ReducerKind::Linear, "MEAN"))
            } else {
                None
            }
        }
        // Single-arg MIN/MAX (no second argument) is the array reducer form.
        BuiltinFn::Min(arg, None) => {
            if expr_references_var(arg, canonical_source.as_ref()) {
                Some((ReducerKind::Nonlinear, "MIN"))
            } else {
                None
            }
        }
        BuiltinFn::Max(arg, None) => {
            if expr_references_var(arg, canonical_source.as_ref()) {
                Some((ReducerKind::Nonlinear, "MAX"))
            } else {
                None
            }
        }
        BuiltinFn::Stddev(arg) => {
            if expr_references_var(arg, canonical_source.as_ref()) {
                Some((ReducerKind::Nonlinear, "STDDEV"))
            } else {
                None
            }
        }
        BuiltinFn::Rank(arg, _) => {
            if expr_references_var(arg, canonical_source.as_ref()) {
                Some((ReducerKind::Nonlinear, "RANK"))
            } else {
                None
            }
        }
        BuiltinFn::Size(arg) => {
            if expr_references_var(arg, canonical_source.as_ref()) {
                Some((ReducerKind::Constant, "SIZE"))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Check if an Expr2 references a variable with the given canonical name,
/// either directly (Var) or via subscript (Subscript).
fn expr_references_var(expr: &crate::ast::Expr2, canonical_name: &str) -> bool {
    use crate::ast::Expr2;

    match expr {
        Expr2::Var(ident, _, _) => ident.as_str() == canonical_name,
        Expr2::Subscript(ident, _, _, _) => ident.as_str() == canonical_name,
        Expr2::App(builtin, _, _) => {
            let mut found = false;
            builtin.for_each_expr_ref(|sub_expr| {
                if !found {
                    found = expr_references_var(sub_expr, canonical_name);
                }
            });
            found
        }
        Expr2::Op1(_, inner, _, _) => expr_references_var(inner, canonical_name),
        Expr2::Op2(_, lhs, rhs, _, _) => {
            expr_references_var(lhs, canonical_name) || expr_references_var(rhs, canonical_name)
        }
        Expr2::If(cond, then_e, else_e, _, _) => {
            expr_references_var(cond, canonical_name)
                || expr_references_var(then_e, canonical_name)
                || expr_references_var(else_e, canonical_name)
        }
        Expr2::Const(..) => false,
    }
}

/// Generate a per-element link score equation for an arrayed-to-scalar edge.
///
/// For element `current_element` of source variable `source_var_name`,
/// produces the partial equation where ONLY `source[current_element]` varies
/// while all other elements are held at PREVIOUS values.
///
/// `reducer_kind` determines the generation strategy:
/// - `Linear`: algebraic shortcut (SUM/MEAN) avoids enumerating all elements
/// - `Nonlinear`: explicit element expansion with selective PREVIOUS wrapping
/// - `Constant`: caller should skip generation (SIZE always produces 0)
///
/// `reducer_name` is the uppercase function name ("MIN", "MAX", "STDDEV", "RANK")
/// used for nonlinear reducers when reconstructing the function call.
///
/// `is_bare` indicates whether the reducer is the entire target equation (true)
/// or is nested inside surrounding arithmetic like `2 * SUM(...)` (false).
/// When false, the algebraic shortcut would produce wrong link scores because
/// it ignores the surrounding arithmetic. In that case, the delta-ratio
/// fallback (using the target variable directly) is used instead.
pub(crate) fn generate_element_to_scalar_equation(
    source_var_name: &str,
    target_var_name: &str,
    current_element: &str,
    all_elements: &[String],
    reducer_kind: &ReducerKind,
    reducer_name: &str,
    is_bare: bool,
) -> String {
    let source_q = quote_ident(source_var_name);
    let target_q = quote_ident(target_var_name);
    let source_elem = format!("{source_q}[{current_element}]");

    let partial_eq = match reducer_kind {
        ReducerKind::Constant => {
            // SIZE is constant; caller should not generate link scores.
            // Return a zero equation as a defensive fallback.
            return "0".to_string();
        }
        _ if !is_bare => {
            // The reducer is nested inside surrounding arithmetic (e.g.,
            // `2 * SUM(population[*])` or `MAX(SUM(population[*]), 0)`).
            // The algebraic shortcut would ignore the surrounding expression
            // and produce wrong link scores. Fall back to the delta-ratio
            // approach: use the target variable directly, which measures the
            // ratio of actual target change to source element change. This is
            // approximate (like STDDEV/RANK) but avoids the wrong-multiplier
            // bug that the algebraic shortcut would introduce.
            target_q.to_string()
        }
        ReducerKind::Linear => generate_linear_partial(
            &source_q,
            &target_q,
            current_element,
            all_elements.len(),
            reducer_name,
        ),
        ReducerKind::Nonlinear => generate_nonlinear_partial(
            &source_q,
            &target_q,
            current_element,
            all_elements,
            reducer_name,
        ),
    };

    // Standard link score formula wrapping the partial equation.
    let abs_part = format!(
        "ABS(SAFEDIV(({partial_eq} - PREVIOUS({target_q})), ({target_q} - PREVIOUS({target_q})), 0))"
    );
    let sign_part = format!(
        "SIGN(SAFEDIV(({partial_eq} - PREVIOUS({target_q})), ({source_elem} - PREVIOUS({source_elem})), 0))"
    );

    format!(
        "if \
            (TIME = INITIAL_TIME) \
            then 0 \
            else if \
                (({target_q} - PREVIOUS({target_q})) = 0) OR (({source_elem} - PREVIOUS({source_elem})) = 0) \
                then 0 \
                else {abs_part} * {sign_part}"
    )
}

/// Generate the partial evaluation for a linear reducer (SUM or MEAN).
///
/// SUM: PREVIOUS(target) + (source[elem] - PREVIOUS(source[elem]))
/// MEAN: PREVIOUS(target) + (source[elem] - PREVIOUS(source[elem])) / N
fn generate_linear_partial(
    source_q: &str,
    target_q: &str,
    current_element: &str,
    n_elements: usize,
    reducer_name: &str,
) -> String {
    let delta =
        format!("({source_q}[{current_element}] - PREVIOUS({source_q}[{current_element}]))");

    match reducer_name.to_uppercase().as_str() {
        "MEAN" => {
            format!("PREVIOUS({target_q}) + {delta} / {n_elements}")
        }
        // SUM is the default linear case
        _ => {
            format!("PREVIOUS({target_q}) + {delta}")
        }
    }
}

/// Generate the partial evaluation for a nonlinear reducer.
///
/// For MIN/MAX (binary), nests 2-argument calls to enumerate all elements
/// with selective PREVIOUS wrapping. For STDDEV/RANK (which only accept
/// array arguments), falls back to the target variable directly -- the
/// link score then measures the delta-ratio between the target and the
/// element, which is the best available approximation when ceteris paribus
/// decomposition is not expressible in the equation language.
fn generate_nonlinear_partial(
    source_q: &str,
    target_q: &str,
    current_element: &str,
    all_elements: &[String],
    reducer_name: &str,
) -> String {
    match reducer_name.to_uppercase().as_str() {
        "MIN" | "MAX" => {
            // Nest binary calls: MIN(a, MIN(b, MIN(c, d))) etc.
            // Each element is either current (live) or wrapped in PREVIOUS.
            let args: Vec<String> = all_elements
                .iter()
                .map(|elem| {
                    if elem == current_element {
                        format!("{source_q}[{elem}]")
                    } else {
                        format!("PREVIOUS({source_q}[{elem}])")
                    }
                })
                .collect();

            let fn_name = reducer_name.to_uppercase();
            if args.len() == 1 {
                return args[0].clone();
            }
            // Build nested binary calls from right to left:
            // MIN(a, MIN(b, c)) for [a, b, c]
            let mut result = args[args.len() - 1].clone();
            for arg in args[..args.len() - 1].iter().rev() {
                result = format!("{fn_name}({arg}, {result})");
            }
            result
        }
        _ => {
            // STDDEV, RANK: cannot decompose into per-element scalar
            // expressions because these builtins only accept array
            // arguments. Fall back to the target variable itself, which
            // gives a delta-ratio link score (how much the target
            // changed relative to how much this element changed).
            target_q.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{CanonicalDimensionName, CanonicalElementName};
    use crate::dimensions::{Dimension, NamedDimension};

    fn make_named_dimension(name: &str, elements: &[&str]) -> Dimension {
        use std::collections::HashMap;
        let canonical_elements: Vec<CanonicalElementName> = elements
            .iter()
            .map(|e| CanonicalElementName::from_raw(e))
            .collect();
        let indexed: HashMap<CanonicalElementName, usize> = canonical_elements
            .iter()
            .enumerate()
            .map(|(i, e)| (e.clone(), i))
            .collect();
        Dimension::Named(
            CanonicalDimensionName::from_raw(name),
            NamedDimension {
                elements: canonical_elements,
                indexed_elements: indexed,
                maps_to: None,
                mappings: vec![],
            },
        )
    }

    fn make_indexed_dimension(name: &str, size: u32) -> Dimension {
        Dimension::Indexed(CanonicalDimensionName::from_raw(name), size)
    }

    /// Build a `HashSet<Ident<Canonical>>` from string slices for use in
    /// per-shape partial-equation tests. Each input string is canonicalized
    /// via `Ident::new`, matching the wrapping path that
    /// `build_partial_equation_shaped` exercises.
    fn deps_set(idents: &[&str]) -> HashSet<Ident<Canonical>> {
        idents.iter().map(|s| Ident::new(s)).collect()
    }

    /// Source-dimension element names for the per-shape partial-equation
    /// tests using a single `Region` dimension with elements `nyc` and
    /// `boston` (canonical lowercase form, in source-declared order).
    /// Used by `classify_expr0_subscript_shape` to validate that a literal
    /// subscript like `[NYC]` resolves to a known element.
    fn region_dim_elements() -> Vec<Vec<String>> {
        vec![vec!["nyc".to_string(), "boston".to_string()]]
    }

    /// Regression test for the integer-literal bounds asymmetry between
    /// the Expr0 and Expr2 classifiers. The Expr2 classifier
    /// (`db_analysis::resolve_literal_index`) validates integer
    /// literals against the indexed dimension's size and returns None
    /// (so the shape becomes `DynamicIndex`) for out-of-range values.
    /// The Expr0 classifier here previously accepted any `u32`-parseable
    /// `Const`, so `pop[999]` over an indexed dim of size 2 would
    /// classify as `FixedIndex(["999"])` here while the edge emitter
    /// classifies it as `DynamicIndex`. The shapes wouldn't match,
    /// the live reference would be wrapped in `PREVIOUS()`, and the
    /// link score would silently zero out.
    ///
    /// Both classifiers must agree -- so out-of-range integer literals
    /// classify as `DynamicIndex`.
    #[test]
    fn classify_expr0_rejects_out_of_range_integer_literal() {
        use crate::ast::{Expr0, IndexExpr0, Loc};

        // Indexed-style source_dim_elements: position 0 is an indexed
        // dim of size 2 (elements "1", "2"). "999" is out of range.
        let dims = vec![vec!["1".to_string(), "2".to_string()]];
        let indices = vec![IndexExpr0::Expr(Expr0::Const(
            "999".to_string(),
            999.0,
            Loc::default(),
        ))];

        let shape = classify_expr0_subscript_shape(&indices, &dims);
        assert_eq!(
            shape,
            RefShape::DynamicIndex,
            "out-of-range integer literal must classify as DynamicIndex \
             to agree with Expr2's resolve_literal_index; got {shape:?}",
        );

        // Same fixture: is_literal_element_index must also reject.
        assert!(
            !is_literal_element_index(&indices[0], 0, &dims),
            "is_literal_element_index must reject out-of-range integer literal",
        );

        // Sanity: the same classifier still accepts an in-range integer.
        let in_range = vec![IndexExpr0::Expr(Expr0::Const(
            "1".to_string(),
            1.0,
            Loc::default(),
        ))];
        let in_range_shape = classify_expr0_subscript_shape(&in_range, &dims);
        assert_eq!(
            in_range_shape,
            RefShape::FixedIndex(vec!["1".to_string()]),
            "in-range integer literal must classify as FixedIndex; got {in_range_shape:?}",
        );
    }

    // -- dimension_element_names tests --

    #[test]
    fn test_dimension_element_names_named() {
        let dim = make_named_dimension("Region", &["NYC", "Boston", "LA"]);
        let names = dimension_element_names(&dim);
        assert_eq!(names, vec!["nyc", "boston", "la"]);
    }

    #[test]
    fn test_dimension_element_names_indexed() {
        // Indexed dimensions use 1-based indexing to match the engine's
        // subscript formatting (see dimensions.rs SubscriptIterator).
        let dim = make_indexed_dimension("Index", 4);
        let names = dimension_element_names(&dim);
        assert_eq!(names, vec!["1", "2", "3", "4"]);
    }

    #[test]
    fn test_dimension_element_names_empty() {
        let dim = make_named_dimension("Empty", &[]);
        let names = dimension_element_names(&dim);
        assert!(names.is_empty());
    }

    #[test]
    fn test_dimension_element_names_indexed_zero() {
        let dim = make_indexed_dimension("Zero", 0);
        let names = dimension_element_names(&dim);
        assert!(names.is_empty());
    }

    // -- ReducerKind tests --

    #[test]
    fn test_reducer_kind_equality() {
        assert_eq!(ReducerKind::Linear, ReducerKind::Linear);
        assert_eq!(ReducerKind::Nonlinear, ReducerKind::Nonlinear);
        assert_eq!(ReducerKind::Constant, ReducerKind::Constant);
        assert_ne!(ReducerKind::Linear, ReducerKind::Nonlinear);
        assert_ne!(ReducerKind::Linear, ReducerKind::Constant);
        assert_ne!(ReducerKind::Nonlinear, ReducerKind::Constant);
    }

    #[test]
    fn test_reducer_kind_clone() {
        let kind = ReducerKind::Linear;
        let cloned = kind.clone();
        assert_eq!(kind, cloned);
    }

    // -- classify_reducer tests --

    use crate::ast::{Ast, Expr2, IndexExpr2};
    use crate::builtins::{BuiltinFn, Loc};

    /// Build a Variable::Var with a hand-built Expr2 AST.
    fn var_with_expr(expr: Expr2) -> Variable {
        Variable::Var {
            ident: Ident::new("target"),
            ast: Some(Ast::Scalar(expr)),
            init_ast: None,
            eqn: None,
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        }
    }

    /// Build an Expr2 representing `var_name[*]` (subscript with wildcard).
    fn subscript_wildcard(var_name: &str) -> Expr2 {
        Expr2::Subscript(
            Ident::new(var_name),
            vec![IndexExpr2::Wildcard(Loc::default())],
            None,
            Loc::default(),
        )
    }

    /// Build an Expr2 representing a plain variable reference.
    fn var_ref(name: &str) -> Expr2 {
        Expr2::Var(Ident::new(name), None, Loc::default())
    }

    #[test]
    fn test_classify_reducer_sum() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Linear, "SUM", true)));
    }

    #[test]
    fn test_classify_reducer_mean() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Mean(vec![inner]), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Linear, "MEAN", true)));
    }

    #[test]
    fn test_classify_reducer_min() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Min(Box::new(inner), None), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Nonlinear, "MIN", true)));
    }

    #[test]
    fn test_classify_reducer_max() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Max(Box::new(inner), None), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Nonlinear, "MAX", true)));
    }

    #[test]
    fn test_classify_reducer_stddev() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Stddev(Box::new(inner)), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Nonlinear, "STDDEV", true)));
    }

    #[test]
    fn test_classify_reducer_rank() {
        let inner = subscript_wildcard("population");
        let direction = Expr2::Const("1".to_string(), 1.0, Loc::default());
        let expr = Expr2::App(
            BuiltinFn::Rank(Box::new(inner), Box::new(direction)),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Nonlinear, "RANK", true)));
    }

    #[test]
    fn test_classify_reducer_size() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Size(Box::new(inner)), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Constant, "SIZE", true)));
    }

    #[test]
    fn test_classify_reducer_no_reducer() {
        // A plain addition: x + y
        let expr = Expr2::Op2(
            crate::ast::BinaryOp::Add,
            Box::new(var_ref("x")),
            Box::new(var_ref("y")),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "x");
        assert_eq!(result, None);
    }

    #[test]
    fn test_classify_reducer_wrong_source() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
        let var = var_with_expr(expr);
        // Looking for a different source variable
        let result = classify_reducer(&var, "other_var");
        assert_eq!(result, None);
    }

    #[test]
    fn test_classify_reducer_nested_in_expression() {
        // 2 * SUM(population[*]) + 1
        // Reducer is NOT at the top level, so is_bare should be false.
        let inner = subscript_wildcard("population");
        let sum_expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
        let two = Expr2::Const("2".to_string(), 2.0, Loc::default());
        let one = Expr2::Const("1".to_string(), 1.0, Loc::default());
        let mul = Expr2::Op2(
            crate::ast::BinaryOp::Mul,
            Box::new(two),
            Box::new(sum_expr),
            None,
            Loc::default(),
        );
        let expr = Expr2::Op2(
            crate::ast::BinaryOp::Add,
            Box::new(mul),
            Box::new(one),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Linear, "SUM", false)));
    }

    #[test]
    fn test_classify_reducer_nested_in_scalar_max() {
        // MAX(SUM(population[*]), 0) -- scalar MAX wrapping array SUM
        // The SUM is nested inside a non-reducer App, so is_bare should be false.
        let inner = subscript_wildcard("population");
        let sum_expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
        let zero = Expr2::Const("0".to_string(), 0.0, Loc::default());
        let expr = Expr2::App(
            BuiltinFn::Max(Box::new(sum_expr), Some(Box::new(zero))),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Linear, "SUM", false)));
    }

    #[test]
    fn test_classify_reducer_var_ref_no_subscript() {
        // SUM with a plain var reference (no subscript) should still match
        let inner = var_ref("population");
        let expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Linear, "SUM", true)));
    }

    #[test]
    fn test_classify_reducer_no_ast() {
        // Variable without an AST
        let var: Variable = Variable::Var {
            ident: Ident::new("target"),
            ast: None,
            init_ast: None,
            eqn: None,
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };
        let result = classify_reducer(&var, "population");
        assert_eq!(result, None);
    }

    #[test]
    fn test_classify_reducer_two_arg_min_not_reducer() {
        // MIN(x, y) with two args is NOT an array reducer
        let inner1 = var_ref("population");
        let inner2 = var_ref("threshold");
        let expr = Expr2::App(
            BuiltinFn::Min(Box::new(inner1), Some(Box::new(inner2))),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, None);
    }

    #[test]
    fn test_classify_reducer_two_arg_max_not_reducer() {
        // MAX(x, y) with two args is NOT an array reducer
        let inner1 = var_ref("population");
        let inner2 = var_ref("threshold");
        let expr = Expr2::App(
            BuiltinFn::Max(Box::new(inner1), Some(Box::new(inner2))),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, None);
    }

    // -- generate_element_to_scalar_equation tests --

    #[test]
    fn test_generate_sum_equation() {
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "total_pop",
            "nyc",
            &elements,
            &ReducerKind::Linear,
            "SUM",
            true,
        );
        // Should contain the algebraic shortcut
        assert!(eq.contains("PREVIOUS(total_pop)"), "equation: {eq}");
        assert!(eq.contains("population[nyc]"), "equation: {eq}");
        assert!(eq.contains("PREVIOUS(population[nyc])"), "equation: {eq}");
        // Should not enumerate other elements (algebraic shortcut avoids them)
        assert!(
            !eq.contains("[boston]"),
            "equation should not enumerate boston: {eq}"
        );
        assert!(
            !eq.contains("[la]"),
            "equation should not enumerate la: {eq}"
        );
    }

    #[test]
    fn test_generate_mean_equation() {
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "avg_pop",
            "nyc",
            &elements,
            &ReducerKind::Linear,
            "MEAN",
            true,
        );
        // MEAN divides by N
        assert!(eq.contains("/ 3"), "equation: {eq}");
        assert!(eq.contains("PREVIOUS(avg_pop)"), "equation: {eq}");
    }

    #[test]
    fn test_generate_min_equation() {
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "min_pop",
            "nyc",
            &elements,
            &ReducerKind::Nonlinear,
            "MIN",
            true,
        );
        // Should enumerate all elements with nested binary MIN calls
        assert!(eq.contains("population[nyc]"), "equation: {eq}");
        assert!(
            eq.contains("PREVIOUS(population[boston])"),
            "equation: {eq}"
        );
        assert!(eq.contains("PREVIOUS(population[la])"), "equation: {eq}");
        // Nested binary calls: MIN(a, MIN(b, c))
        assert!(
            eq.contains(
                "MIN(population[nyc], MIN(PREVIOUS(population[boston]), PREVIOUS(population[la])))"
            ),
            "equation: {eq}"
        );
    }

    #[test]
    fn test_generate_max_equation() {
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "max_pop",
            "boston",
            &elements,
            &ReducerKind::Nonlinear,
            "MAX",
            true,
        );
        // boston is the current element, so nyc and la are wrapped
        // Nested binary calls: MAX(a, MAX(b, c))
        assert!(
            eq.contains(
                "MAX(PREVIOUS(population[nyc]), MAX(population[boston], PREVIOUS(population[la])))"
            ),
            "equation: {eq}"
        );
    }

    #[test]
    fn test_generate_constant_returns_zero() {
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "size_pop",
            "nyc",
            &elements,
            &ReducerKind::Constant,
            "SIZE",
            true,
        );
        assert_eq!(eq, "0");
    }

    #[test]
    fn test_generate_nested_reducer_uses_delta_ratio() {
        // When the reducer is nested (is_bare=false), the equation should
        // fall back to the delta-ratio approach (using target directly)
        // instead of the algebraic shortcut.
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "total_pop",
            "nyc",
            &elements,
            &ReducerKind::Linear,
            "SUM",
            false, // nested reducer
        );
        // Should NOT use the algebraic shortcut (PREVIOUS(target) + delta)
        assert!(
            !eq.contains("PREVIOUS(total_pop) +"),
            "should not use algebraic shortcut for nested reducer: {eq}"
        );
        // Should still have the standard link score wrapping
        assert!(eq.contains("TIME = INITIAL_TIME"), "equation: {eq}");
        assert!(eq.contains("SAFEDIV("), "equation: {eq}");
        // The partial equation uses target directly (delta-ratio approach)
        assert!(
            eq.contains("(total_pop - PREVIOUS(total_pop))"),
            "should use target variable in delta-ratio: {eq}"
        );
    }

    #[test]
    fn test_generate_link_score_wrapping() {
        let elements = vec!["a".to_string(), "b".to_string()];
        let eq = generate_element_to_scalar_equation(
            "src",
            "tgt",
            "a",
            &elements,
            &ReducerKind::Linear,
            "SUM",
            true,
        );
        // Should have initial time guard
        assert!(eq.contains("TIME = INITIAL_TIME"), "equation: {eq}");
        // Should have zero-change guards
        assert!(eq.contains("(tgt - PREVIOUS(tgt)) = 0"), "equation: {eq}");
        assert!(
            eq.contains("(src[a] - PREVIOUS(src[a])) = 0"),
            "equation: {eq}"
        );
        // Should have ABS and SIGN parts
        assert!(eq.contains("ABS(SAFEDIV("), "equation: {eq}");
        assert!(eq.contains("SIGN(SAFEDIV("), "equation: {eq}");
    }

    #[test]
    fn test_generate_special_chars_quoted() {
        let elements = vec!["nyc".to_string()];
        let eq = generate_element_to_scalar_equation(
            "$\u{205A}ltm\u{205A}var",
            "total",
            "nyc",
            &elements,
            &ReducerKind::Linear,
            "SUM",
            true,
        );
        // Source name with special chars should be quoted
        assert!(eq.contains("\"$\u{205A}ltm\u{205A}var\""), "equation: {eq}");
    }

    // -- build_partial_equation_shaped: per-shape partial equation tests --
    //
    // Each test below pins the exact text that
    // `build_partial_equation_shaped` must return when handed a specific
    // `RefShape`. The expected strings were captured from `print_eqn` during
    // Task 0.5 reconnaissance and are already canonicalized: identifiers and
    // element names are lowercase (`print_ident` routes through
    // `canonicalize`), parsed function names are lowercase (the parser
    // lowercases function tokens at parse time, so `SUM` round-trips as
    // `sum`), synthesized `PREVIOUS` keeps uppercase (it's constructed as a
    // literal `"PREVIOUS"` `UntypedBuiltinFn`), binary operators get a
    // single space on each side, and parens are reintroduced for precedence.
    // Whitespace canonicalization happens entirely inside `print_eqn`, so the
    // assertions can use the literal expected string without any pre-trim.
    //
    // The Bare and Wildcard tests don't need `source_dim_elements` because
    // their classification doesn't depend on element-name lookups (Bare is a
    // top-level Var; Wildcard is detected from the `[*]` index alone). The
    // FixedIndex tests pass `region_dim_elements()` so
    // `classify_expr0_subscript_shape` can validate `[NYC]` and `[Boston]`
    // against the source's declared elements; otherwise both literal indices
    // would fall back to `DynamicIndex` and both subscripts would be wrapped.

    #[test]
    fn test_partial_equation_share_bare_shape() {
        // share[R] = population / SUM(population[*])
        // For the bare-Var reference (`population`), the bare ref stays live
        // and the wildcard reducer's source ref is wrapped in PREVIOUS().
        let equation = "population / SUM(population[*])";
        let deps = deps_set(&["population"]);
        let source = Ident::<Canonical>::new("population");
        let partial = build_partial_equation_shaped(equation, &deps, &source, &RefShape::Bare, &[]);
        assert_eq!(partial, "population / sum(PREVIOUS(population[*]))");
    }

    #[test]
    fn test_partial_equation_share_wildcard_shape() {
        // share[R] = population / SUM(population[*])
        // For the wildcard reducer's source ref (`population[*]`), the
        // wildcard stays live and the bare ref is wrapped in PREVIOUS().
        let equation = "population / SUM(population[*])";
        let deps = deps_set(&["population"]);
        let source = Ident::<Canonical>::new("population");
        let partial =
            build_partial_equation_shaped(equation, &deps, &source, &RefShape::Wildcard, &[]);
        assert_eq!(partial, "PREVIOUS(population) / sum(population[*])");
    }

    #[test]
    fn test_partial_equation_migration_pressure_fixed_nyc() {
        // migration_pressure[NYC] = (population[NYC] - population[Boston]) * 0.01
        // For the FixedIndex(nyc) shape, the `population[nyc]` reference stays
        // live and `population[boston]` is wrapped in PREVIOUS(). Element names
        // in the FixedIndex variant are lowercase canonical form -- they must
        // match the AST subscript text, which `print_ident` lowercases via
        // `canonicalize`.
        let equation = "(population[NYC] - population[Boston]) * 0.01";
        let deps = deps_set(&["population"]);
        let source = Ident::<Canonical>::new("population");
        let dims = region_dim_elements();
        let partial = build_partial_equation_shaped(
            equation,
            &deps,
            &source,
            &RefShape::FixedIndex(vec!["nyc".to_string()]),
            &dims,
        );
        assert_eq!(
            partial,
            "(population[nyc] - PREVIOUS(population[boston])) * 0.01"
        );
    }

    #[test]
    fn test_partial_equation_migration_pressure_fixed_boston() {
        // Same equation text as the NYC case -- the per-shape builder works
        // per (reference-site, shape) pair, so the input equation is the
        // host expression and the `live_shape` selects which subscripted
        // population ref survives. Here `FixedIndex(boston)` keeps
        // `population[boston]` live and wraps `population[nyc]`.
        let equation = "(population[NYC] - population[Boston]) * 0.01";
        let deps = deps_set(&["population"]);
        let source = Ident::<Canonical>::new("population");
        let dims = region_dim_elements();
        let partial = build_partial_equation_shaped(
            equation,
            &deps,
            &source,
            &RefShape::FixedIndex(vec!["boston".to_string()]),
            &dims,
        );
        assert_eq!(
            partial,
            "(PREVIOUS(population[nyc]) - population[boston]) * 0.01"
        );
    }

    // -- AC2.4: other-source refs always wrapped, unknown idents passthrough --
    //
    // The two tests below pin behavior for references that aren't the live
    // source. The first verifies that another known dep is wrapped regardless
    // of which shape is live. The second verifies that an identifier that
    // doesn't appear in `deps` (e.g., a typo or unresolved external) passes
    // through unchanged -- the per-shape builder doesn't treat unknown idents
    // as wrap candidates because they could be function names or noise that
    // downstream parsing will diagnose separately.

    #[test]
    fn partial_equation_other_source_always_wrapped() {
        // Equation has a reference to `helper` (other dep) plus the live
        // source `pop`. The `helper` reference must be wrapped regardless
        // of `live_shape`; `pop` stays live because the shape is `Bare`.
        let deps = deps_set(&["pop", "helper"]);
        let live = Ident::<Canonical>::new("pop");
        let shape = RefShape::Bare;
        let dims = region_dim_elements();

        let partial = build_partial_equation_shaped("pop * helper", &deps, &live, &shape, &dims);
        assert!(partial.contains("PREVIOUS(helper)"), "partial: {partial}");
        assert!(!partial.contains("PREVIOUS(pop)"), "partial: {partial}");
    }

    #[test]
    fn partial_equation_unknown_ident_unchanged() {
        // A reference to a variable not in `deps` (e.g., a typo or external)
        // is left alone -- it's not a known dep and shouldn't be wrapped.
        let deps = deps_set(&["pop"]);
        let live = Ident::<Canonical>::new("pop");
        let shape = RefShape::Bare;
        let dims = region_dim_elements();

        let partial = build_partial_equation_shaped("pop + unknown", &deps, &live, &shape, &dims);
        assert!(partial.contains("unknown"), "partial: {partial}");
        assert!(!partial.contains("PREVIOUS(unknown)"), "partial: {partial}");
    }

    // -- link_score_var_name: per-shape naming convention --
    //
    // The naming helper produces a stable name for each `(from, to, shape)`
    // tuple regardless of which other shapes coexist in the same model.
    // Bare uses the legacy canonical form; FixedIndex prefixes the source
    // with the bracketed element name(s); Wildcard and DynamicIndex always
    // append a stable suffix on the target side. The discovery parser
    // (Phase 3 Task 7) strips the suffix before looking up offsets.

    #[test]
    fn link_score_name_bare_canonical() {
        assert_eq!(
            link_score_var_name("pop", "births", &RefShape::Bare),
            "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}births"
        );
    }

    #[test]
    fn link_score_name_fixed_index() {
        let shape = RefShape::FixedIndex(vec!["nyc".to_string()]);
        assert_eq!(
            link_score_var_name("pop", "rel_pop", &shape),
            "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop"
        );
    }

    #[test]
    fn link_score_name_wildcard_always_suffixed() {
        // Suffix is unconditional - same name regardless of whether Bare
        // coexists. This is the resolution of code-review issues I2 + I6:
        // the suffix is a function of `(from, to, shape)` alone, so the
        // discovery parser needs no per-model collision analysis.
        assert_eq!(
            link_score_var_name("pop", "total", &RefShape::Wildcard),
            "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}total\u{205A}wildcard"
        );
        assert_eq!(
            link_score_var_name("pop", "share", &RefShape::Wildcard),
            "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share\u{205A}wildcard"
        );
    }

    #[test]
    fn link_score_name_dynamic_index_always_suffixed() {
        assert_eq!(
            link_score_var_name("pop", "tgt", &RefShape::DynamicIndex),
            "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}tgt\u{205A}dynamic"
        );
    }

    // -- generate_loop_score_equation: per-element link names --
    //
    // The per-element distinction lives in `link.from` itself (e.g.,
    // `"pop[nyc]"` for cross-dimensional edges in mixed/scalar loops).
    // generate_loop_score_equation uses Bare naming uniformly, so the
    // bracketed `from` flows through verbatim and the resulting
    // reference matches the per-element link score that
    // try_cross_dimensional_link_scores emits.
    #[test]
    fn loop_score_equation_uses_element_level_from_for_per_element_links() {
        use crate::ltm::{Link, LinkPolarity, Loop, LoopPolarity};

        let loop_item = Loop {
            id: "r1".to_string(),
            links: vec![
                Link {
                    from: Ident::<Canonical>::new("pop[nyc]"),
                    to: Ident::<Canonical>::new("rel_pop"),
                    polarity: LinkPolarity::Positive,
                },
                Link {
                    from: Ident::<Canonical>::new("rel_pop"),
                    to: Ident::<Canonical>::new("pop"),
                    polarity: LinkPolarity::Positive,
                },
            ],
            stocks: vec![],
            polarity: LoopPolarity::Reinforcing,
            dimensions: vec![],
        };

        // Pretend both candidates were emitted as Bare; the resolver
        // will pick the canonical form via Bare naming, so the
        // bracketed from flows through verbatim.
        let mut emitted = HashSet::new();
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop".to_string());
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}rel_pop\u{2192}pop".to_string());
        let eq = generate_loop_score_equation(&loop_item, &emitted);

        // Element-level from flows through Bare naming verbatim.
        assert!(
            eq.contains("\"$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop\""),
            "expected per-element link-score reference; got: {eq}"
        );
        // The closing link uses canonical Bare naming.
        assert!(
            eq.contains("\"$\u{205A}ltm\u{205A}link_score\u{205A}rel_pop\u{2192}pop\""),
            "expected Bare link-score reference (closing link); got: {eq}"
        );
        // Loop score is the product of the two references.
        assert!(eq.contains(" * "), "expected product join; got: {eq}");
    }

    /// Regression test: when only a `FixedIndex` variant is in `emitted`
    /// (e.g., `share[r] = pop[NYC]` -- only `pop[nyc]→share` is emitted),
    /// the resolver must pick that variant rather than fall back to the
    /// never-emitted Bare canonical name.
    #[test]
    fn resolver_picks_fixed_index_when_bare_not_emitted() {
        let mut emitted = HashSet::new();
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share".to_string());

        let chosen = resolve_link_score_name_for_loop("pop", "share", &emitted);
        assert_eq!(
            chosen, "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share",
            "resolver should pick the FixedIndex variant when Bare is not emitted",
        );
    }

    /// Regression test: when multiple FixedIndex variants exist (e.g.,
    /// `share[r] = pop[NYC] + pop[BOSTON]`), the resolver picks
    /// deterministically (lexicographically first). This documents the
    /// edge-aliasing limitation: only one variant contributes to the
    /// loop score.
    #[test]
    fn resolver_picks_fixed_index_deterministically_with_multiple_variants() {
        let mut emitted = HashSet::new();
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share".to_string());
        emitted
            .insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[boston]\u{2192}share".to_string());

        let chosen = resolve_link_score_name_for_loop("pop", "share", &emitted);
        // Lexicographic sort: "pop[boston]→share" < "pop[nyc]→share".
        assert_eq!(
            chosen, "$\u{205A}ltm\u{205A}link_score\u{205A}pop[boston]\u{2192}share",
            "resolver should pick the lexicographically first FixedIndex variant",
        );
    }

    /// Regression test: Bare must win when both Bare and a suffixed
    /// variant coexist (the documented edge-aliasing behavior).
    #[test]
    fn resolver_prefers_bare_over_other_shapes() {
        let mut emitted = HashSet::new();
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share".to_string());
        emitted.insert(
            "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share\u{205A}wildcard".to_string(),
        );
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share".to_string());

        let chosen = resolve_link_score_name_for_loop("pop", "share", &emitted);
        assert_eq!(
            chosen, "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share",
            "Bare must win when present, regardless of other variants",
        );
    }

    /// Regression test: bracketed `from` (cross-dimensional case) flows
    /// through Bare naming verbatim and must resolve to the matching
    /// per-element name emitted by `try_cross_dimensional_link_scores`.
    #[test]
    fn resolver_resolves_cross_dim_bracketed_from() {
        let mut emitted = HashSet::new();
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}total".to_string());

        let chosen = resolve_link_score_name_for_loop("pop[nyc]", "total", &emitted);
        assert_eq!(
            chosen, "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}total",
            "bracketed from + Bare should match the emitted per-element name",
        );
    }

    /// Regression test: when only the Wildcard variant is in `emitted`,
    /// the resolver must pick the suffixed name rather than the Bare
    /// canonical form. This is the case that breaks for fixtures like
    /// `share[r] = SUM(pop[*])` where the only AST shape for (pop, share)
    /// is Wildcard.
    #[test]
    fn loop_score_equation_falls_back_to_wildcard_when_bare_not_emitted() {
        use crate::ltm::{Link, LinkPolarity, Loop, LoopPolarity};

        let loop_item = Loop {
            id: "u1".to_string(),
            links: vec![Link {
                from: Ident::<Canonical>::new("pop"),
                to: Ident::<Canonical>::new("share"),
                polarity: LinkPolarity::Positive,
            }],
            stocks: vec![],
            polarity: LoopPolarity::Undetermined,
            dimensions: vec![],
        };

        let mut emitted = HashSet::new();
        // Only the wildcard variant is emitted.
        emitted.insert(
            "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share\u{205A}wildcard".to_string(),
        );

        let eq = generate_loop_score_equation(&loop_item, &emitted);
        assert!(
            eq.contains(
                "\"$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share\u{205A}wildcard\""
            ),
            "expected wildcard-suffixed reference when Bare is not emitted; got: {eq}"
        );
        assert!(
            !eq.contains("\"$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share\""),
            "must not reference the never-emitted Bare canonical name; got: {eq}"
        );
    }

    /// Regression test: a `DynamicIndex` live reference must still wrap
    /// inner index expressions that reference other deps. The buggy
    /// version skipped recursion for ANY live-shape match, which is
    /// correct for `FixedIndex` (literal element indices are dimension
    /// references, not deps) but wrong for `DynamicIndex` (the index is
    /// an expression that may reference deps which must be held at
    /// PREVIOUS for ceteris-paribus).
    #[test]
    fn partial_equation_dynamic_index_wraps_inner_deps() {
        // arr[idx + helper] with live_source=arr, live_shape=DynamicIndex.
        // The OUTER subscript is the live reference; idx and helper
        // inside the index expression are other deps and must be wrapped
        // in PREVIOUS for ceteris-paribus.
        let dims: Vec<Vec<String>> = vec![];
        let deps = deps_set(&["arr", "idx", "helper"]);
        let live = Ident::new("arr");
        let shape = RefShape::DynamicIndex;

        let partial =
            build_partial_equation_shaped("arr[idx + helper]", &deps, &live, &shape, &dims);

        assert!(
            partial.contains("PREVIOUS(idx)"),
            "idx must be wrapped in PREVIOUS for ceteris-paribus; got: {partial}",
        );
        assert!(
            partial.contains("PREVIOUS(helper)"),
            "helper must be wrapped in PREVIOUS for ceteris-paribus; got: {partial}",
        );
        // The outer arr[...] reference must stay live (no PREVIOUS wrap
        // around the whole subscript).
        assert!(
            !partial.contains("PREVIOUS(arr["),
            "live arr ref must not be wrapped; got: {partial}",
        );
    }

    /// Regression test: a literal-element subscript like `pop[NYC]` must
    /// classify as `FixedIndex(["nyc"])` even when a user variable named
    /// `nyc` exists and is in `other_deps`. The buggy implementation
    /// recursed into the indices first (wrapping `Var(NYC)` as
    /// `App(PREVIOUS, [Var(NYC)])`) and then classified the transformed
    /// indices, which fell through to `DynamicIndex` and broke the live
    /// FixedIndex match -- so the live reference got wrapped too.
    #[test]
    fn partial_equation_dimension_element_collides_with_variable_name() {
        let dims = region_dim_elements();
        // Both `pop` (live source) and `nyc` (user variable) are deps.
        // The literal subscript [NYC] must still classify as a dimension
        // element, not a wrapped variable reference.
        let deps = deps_set(&["pop", "nyc"]);
        let live = Ident::new("pop");
        let shape = RefShape::FixedIndex(vec!["nyc".to_string()]);

        let partial = build_partial_equation_shaped("pop[NYC]", &deps, &live, &shape, &dims);

        // The live reference must remain unwrapped.
        assert!(
            !partial.contains("PREVIOUS(pop"),
            "live FixedIndex reference unexpectedly wrapped; got: {partial}",
        );
        // The literal element subscript must remain unwrapped (NYC is a
        // dimension element here, not a runtime variable reference).
        assert!(
            !partial.contains("PREVIOUS(nyc)"),
            "literal element subscript wrongly treated as variable; got: {partial}",
        );
    }
}
