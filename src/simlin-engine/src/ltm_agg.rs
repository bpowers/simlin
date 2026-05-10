// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Aggregate-node enumeration for LTM (Loops That Matter).
//!
//! An "aggregate node" is the conceptual stand-in for an inlined array-reducer
//! subexpression (`SUM(pop[*])`, `MEAN(...)`, ...). Phase 5 of the
//! cross-element-aggregate-scoring design treats each *maximal* reducer
//! subexpression in a model's equations as an implicit synthetic auxiliary
//! named `$⁚ltm⁚agg⁚{n}`, so that causality routes `source[d] → agg → target`
//! instead of all-pairs `source[d] → target[e]`.
//!
//! Two consumers share this enumeration:
//! - `model_element_causal_edges` reroutes a Wildcard/DynamicIndex reducer
//!   reference through the agg node.
//! - `model_ltm_variables` emits the `$⁚ltm⁚agg⁚{n}` auxiliaries plus the two
//!   link-score families.
//!
//! Because both consumers must see *identical* agg names, the enumeration is
//! salsa-tracked and fully deterministic: variables are visited in canonical
//! sorted order, each variable's AST is walked left-to-right depth-first, and
//! synthetic names are assigned `$⁚ltm⁚agg⁚0`, `1`, ... in first-encounter
//! order. AST-identical *synthetic* reducer subexpressions dedupe to a single
//! agg node (canonicalization is via printed equation text, since `Expr2` is
//! not `Eq`). Variable-backed aggs are never deduped (see below).
//!
//! Two kinds of aggregate node:
//! - **Synthetic** (`is_synthetic == true`): the reducer is a *sub-expression*
//!   of a larger equation (`share[r] = pop[r] / SUM(pop[*])`). A
//!   `$⁚ltm⁚agg⁚{n}` auxiliary is minted to hold its value. Two inline uses
//!   of the same reducer text share one synthetic node (dedup by canonical
//!   text via `synthetic_by_key`).
//! - **Variable-backed** (`is_synthetic == false`): the reducer is the
//!   *entire* dt-equation of a scalar or apply-to-all variable
//!   (`total_population = SUM(population[*])`, `row_sum[D1] = SUM(matrix[D1,*])`).
//!   That variable *is* the aggregate node; no synthetic is minted. Each such
//!   variable is its own distinct agg node -- variable-backed aggs are never
//!   deduped and never reused by an inline use of the same reducer text (an
//!   inline use must get its own *synthetic* node, since the element-graph
//!   reroute and the link-score emitter both filter to `is_synthetic` aggs;
//!   reusing the variable-backed node would silently leave the inline reducer
//!   on the conservative direct-scoring path, with the outcome depending on
//!   whether the whole-RHS reducer happened to be declared first).
//!
//! Cases deliberately *not* recognized here yet (the conservative
//! full-cross-product element graph is left in place; tracked as tech debt):
//! - A reducer over an explicit *slice* used as a sub-expression
//!   (`x[r] = ... + SUM(pop[NYC, *])`): the slice pinning would need to ride on
//!   the agg's source descriptor, which this enumerator does not yet track.
//! - A *partial* reduce used as a sub-expression
//!   (`x[D1] = ... + SUM(matrix[D1, *])`): the result-axis dims would need to
//!   be derived from the enclosing apply-to-all context.
//!
//! Whole-RHS partial reduces (`row_sum[D1] = SUM(matrix[D1,*])`) *are*
//! recognized — the variable is the agg, and `result_dims` carries its dims —
//! but the element-graph reroute leaves the conservative full-cross-product in
//! place for variable-backed aggs (the edges to/from a real variable node
//! already exist via the normal reference walker).

use std::collections::HashMap;

use crate::ast::{Ast, Expr2, IndexExpr2};
use crate::builtins::BuiltinFn;
use crate::common::{Canonical, Ident, canonicalize};
use crate::db::{
    Db, SourceModel, SourceProject, project_datamodel_dims, reconstruct_model_variables,
};

/// Prefix for synthetic aggregate-node names: `$⁚ltm⁚agg⁚{n}`.
///
/// The `⁚` is U+205A (TWO DOT PUNCTUATION), matching the separator used for
/// every other LTM synthetic-variable family (`$⁚ltm⁚link_score⁚...`, etc.).
pub(crate) const AGG_NAME_PREFIX: &str = "$\u{205A}ltm\u{205A}agg\u{205A}";

/// Build the canonical name for the `n`th synthetic aggregate node.
pub(crate) fn synthetic_agg_name(n: usize) -> String {
    format!("{AGG_NAME_PREFIX}{n}")
}

/// One aggregate node: the stand-in for a maximal reducer subexpression.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct AggNode {
    /// The aggregate node's name. For a synthetic agg this is
    /// `$⁚ltm⁚agg⁚{n}`; for a variable-backed agg this is the owning
    /// variable's canonical name (`total_population`, `row_sum`, ...).
    pub name: String,
    /// The reducer subexpression rendered as equation text, e.g.
    /// `"sum(pop[*])"`. This is the canonical (`Loc`-insensitive) key the
    /// node was deduped on; `expr2_to_string` lowercases idents and
    /// normalizes whitespace, so textually-distinct-but-AST-identical
    /// subexpressions collapse to one node.
    pub equation_text: String,
    /// Canonical names of the model variables the reducer reads (sorted,
    /// deduplicated). For `SUM(a[*] + b[*])` this is `["a", "b"]`.
    pub source_vars: Vec<String>,
    /// The aggregate's result-axis dimension names, in datamodel casing
    /// (e.g. `["D1"]` for `row_sum[D1] = SUM(matrix[D1,*])`). Empty for a
    /// scalar reducer (`SUM(pop[*])`). Always empty for synthetic aggs in
    /// this phase (only whole-RHS partial reduces carry result dims, and
    /// those are variable-backed).
    pub result_dims: Vec<String>,
    /// `true` when a `$⁚ltm⁚agg⁚{n}` auxiliary must be minted to hold this
    /// value; `false` when the owning variable already *is* the aggregate
    /// node (its entire dt-equation is exactly this reducer).
    pub is_synthetic: bool,
}

/// The result of enumerating every aggregate node in a model.
///
/// Deterministic by construction so salsa caches it stably: `aggs` is in
/// first-encounter order over the canonical-sorted variable list,
/// `synthetic_by_key` maps the canonical reducer text to the index of the
/// *synthetic* agg minted for it, and `by_var` maps each variable's
/// canonical name to the indices of the aggs that appear in its equation
/// (so the element-graph reroute can ask "which agg of `to` reads `from`?").
///
/// Dedup-by-key applies to *synthetic* aggs only. Two inline uses of the
/// same reducer text collapse to one `$⁚ltm⁚agg⁚{n}` node. A *variable-
/// backed* agg (the whole dt-equation of a scalar/A2A variable is exactly
/// one reducer) is never deduped -- each such variable genuinely is its own
/// aggregate node, so two whole-RHS reducers with identical text yield two
/// distinct variable-backed aggs, and an inline use of a reducer never
/// reuses a variable-backed agg of the same text (which would otherwise be
/// filtered out by the `is_synthetic` checks downstream, leaving the inline
/// reducer on the conservative direct-scoring path -- a name-ordering bug).
#[derive(Clone, Debug, PartialEq, Eq, Default, salsa::Update)]
pub struct AggNodesResult {
    /// Aggregate nodes in first-encounter (deterministic) order.
    pub aggs: Vec<AggNode>,
    /// Canonical reducer text -> index into `aggs` of the *synthetic* agg
    /// minted for that text. Variable-backed aggs do not participate.
    pub synthetic_by_key: HashMap<String, usize>,
    /// Variable canonical name -> indices into `aggs` of the aggregate
    /// subexpressions occurring in that variable's dt-equation (both
    /// synthetic and variable-backed). A synthetic agg that appears in two
    /// variables' equations (AST-identical → deduped) is referenced from
    /// both variables' entries.
    pub by_var: HashMap<String, Vec<usize>>,
}

impl AggNodesResult {
    /// Look up the *synthetic* aggregate node minted for a canonical
    /// reducer text. Returns `None` for a text that only ever appears as a
    /// variable's whole dt-equation (variable-backed aggs are not keyed
    /// here -- look them up via [`Self::aggs_in_var`] on the owning
    /// variable instead).
    pub fn agg_for_key(&self, key: &str) -> Option<&AggNode> {
        self.synthetic_by_key.get(key).map(|&i| &self.aggs[i])
    }

    /// Iterate the aggregate nodes occurring in `var_name`'s dt-equation.
    pub fn aggs_in_var<'a>(&'a self, var_name: &str) -> impl Iterator<Item = &'a AggNode> {
        self.by_var
            .get(var_name)
            .into_iter()
            .flat_map(move |idxs| idxs.iter().map(move |&i| &self.aggs[i]))
    }
}

/// Enumerate every aggregate node (maximal reducer subexpression) in `model`.
///
/// Salsa-tracked: a pure function of `(db, model, project)` consuming the same
/// reconstructed ASTs the element-graph walker uses, so both consumers see an
/// identical map.
#[salsa::tracked(returns(ref))]
pub fn enumerate_agg_nodes(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> AggNodesResult {
    let variables = reconstruct_model_variables(db, model, project);
    let dm_dims = project_datamodel_dims(db, project);

    // Visit variables in canonical-sorted order for deterministic synthetic
    // naming. `reconstruct_model_variables` returns a HashMap, so the order
    // is not otherwise stable.
    let mut var_names: Vec<&Ident<Canonical>> = variables.keys().collect();
    var_names.sort();

    let mut result = AggNodesResult::default();
    let mut next_synthetic_n: usize = 0usize;

    for var_name in var_names {
        let var = &variables[var_name];
        let Some(ast) = var.ast() else {
            // Stocks (init-only AST) and modules have no dt-equation to walk.
            continue;
        };
        let var_name_str = var_name.as_str().to_string();
        // Datamodel-cased dims of the variable itself (used for the
        // whole-RHS-partial-reduce result dims).
        let dm_dims_ref = dm_dims.as_slice();
        let var_dim_names: Vec<String> = var
            .get_dimensions()
            .map(|dims| {
                dims.iter()
                    .map(|d| canonical_dim_to_datamodel(d.name(), dm_dims_ref))
                    .collect()
            })
            .unwrap_or_default();

        match ast {
            Ast::Scalar(expr) => {
                walk_var_equation(
                    expr,
                    &var_name_str,
                    /* var_is_arrayed = */ false,
                    &var_dim_names,
                    &variables,
                    &mut result,
                    &mut next_synthetic_n,
                );
            }
            Ast::ApplyToAll(_, expr) => {
                walk_var_equation(
                    expr,
                    &var_name_str,
                    /* var_is_arrayed = */ true,
                    &var_dim_names,
                    &variables,
                    &mut result,
                    &mut next_synthetic_n,
                );
            }
            Ast::Arrayed(_, per_elem, default_expr, _) => {
                // Per-element equations: each slot is its own (possibly
                // distinct) equation. A reducer that *is* an element's whole
                // RHS still mints a synthetic agg here -- the variable as a
                // whole is not the aggregate (different elements may reduce
                // differently). Visit slots in canonical element-key order
                // for determinism.
                let mut elem_keys: Vec<_> = per_elem.keys().collect();
                elem_keys.sort();
                for k in elem_keys {
                    walk_subexpr_for_aggs(
                        &per_elem[k],
                        &var_name_str,
                        &variables,
                        &mut result,
                        &mut next_synthetic_n,
                        /* in_reducer = */ false,
                    );
                }
                if let Some(default) = default_expr {
                    walk_subexpr_for_aggs(
                        default,
                        &var_name_str,
                        &variables,
                        &mut result,
                        &mut next_synthetic_n,
                        false,
                    );
                }
            }
        }
    }

    result
}

/// Walk the whole-RHS expression of a `Scalar` / `ApplyToAll` variable.
///
/// If the expression is *exactly* one maximal reducer App, the variable
/// itself is the aggregate node (no synthetic minted). Otherwise the
/// expression is walked for sub-expression reducers via
/// [`walk_subexpr_for_aggs`].
#[allow(clippy::too_many_arguments)]
fn walk_var_equation(
    expr: &Expr2,
    var_name: &str,
    var_is_arrayed: bool,
    var_dim_names: &[String],
    variables: &HashMap<Ident<Canonical>, crate::variable::Variable>,
    result: &mut AggNodesResult,
    next_synthetic_n: &mut usize,
) {
    if let Expr2::App(builtin, _, _) = expr
        && let Some(source_vars) = reducer_source_vars(builtin, variables)
    {
        // Whole-RHS reducer: the variable IS the aggregate node.
        let key = crate::patch::expr2_to_string(expr);
        // The agg node's result shape is the *reducer's* result shape, not
        // the owning variable's. A full reduce (`SUM(pop[*])`) collapses to a
        // scalar even when broadcast to an arrayed variable
        // (`share[Region] = SUM(pop[*])`): every element holds the same value,
        // so `result_dims` is `[]`. Only a *partial* reduce that still varies
        // per element of the variable -- a slice-reduce keyed by the active
        // A2A dimension, e.g. `rowsum[D1] = SUM(matrix[D1, *])` -- keeps the
        // variable's dims as its result dims.
        let result_dims = if var_is_arrayed && !reducer_is_full_reduce(builtin, variables) {
            var_dim_names.to_vec()
        } else {
            vec![]
        };
        register_agg(
            result,
            next_synthetic_n,
            &key,
            var_name,
            AggKind::VariableBacked {
                var_name: var_name.to_string(),
                result_dims,
            },
            source_vars,
        );
        return;
    }
    walk_subexpr_for_aggs(
        expr,
        var_name,
        variables,
        result,
        next_synthetic_n,
        /* in_reducer = */ false,
    );
}

/// Recursively walk an expression looking for *maximal* reducer
/// subexpressions (a reducer App not nested inside another reducer App).
///
/// `in_reducer` is `true` once we have descended into a reducer's argument:
/// any reducer found there is *not* maximal and is skipped (only the
/// outermost reducer becomes an agg), but the walk still continues into it to
/// collect the outer agg's source variables -- handled by the caller via
/// [`reducer_source_vars`], so here we simply stop minting once inside a
/// reducer.
fn walk_subexpr_for_aggs(
    expr: &Expr2,
    owner_var: &str,
    variables: &HashMap<Ident<Canonical>, crate::variable::Variable>,
    result: &mut AggNodesResult,
    next_synthetic_n: &mut usize,
    in_reducer: bool,
) {
    match expr {
        Expr2::Const(..) | Expr2::Var(..) => {}
        Expr2::Subscript(_, indices, _, _) => {
            for idx in indices {
                match idx {
                    IndexExpr2::Expr(e) => walk_subexpr_for_aggs(
                        e,
                        owner_var,
                        variables,
                        result,
                        next_synthetic_n,
                        in_reducer,
                    ),
                    IndexExpr2::Range(l, r, _) => {
                        walk_subexpr_for_aggs(
                            l,
                            owner_var,
                            variables,
                            result,
                            next_synthetic_n,
                            in_reducer,
                        );
                        walk_subexpr_for_aggs(
                            r,
                            owner_var,
                            variables,
                            result,
                            next_synthetic_n,
                            in_reducer,
                        );
                    }
                    IndexExpr2::Wildcard(_)
                    | IndexExpr2::StarRange(_, _)
                    | IndexExpr2::DimPosition(_, _) => {}
                }
            }
        }
        Expr2::App(builtin, _, _) => {
            if !in_reducer
                && let Some(source_vars) = reducer_source_vars(builtin, variables)
                && reducer_is_full_reduce(builtin, variables)
            {
                // Maximal reducer subexpression over the full extent of its
                // arrayed source(s) -> mint a synthetic agg. A slice-reduce
                // (`SUM(pop[NYC, *])`) is deliberately *not* hoisted as a
                // synthetic agg: the agg descriptor only carries the source
                // variable name, not which elements the slice reads, so both
                // the element-graph reroute and the per-element reducer link
                // scores would over-approximate the unread rows with nonzero
                // garbage. Such a subexpression stays conservatively
                // `Wildcard`-classified (tracked as tech debt). A *whole-RHS*
                // slice-reduce (`agg[D1] = SUM(matrix[D1, *])`) is still
                // recognized, but as a variable-backed agg via
                // `walk_var_equation`, not here.
                let key = crate::patch::expr2_to_string(expr);
                register_agg(
                    result,
                    next_synthetic_n,
                    &key,
                    owner_var,
                    AggKind::Synthetic,
                    source_vars,
                );
                // Descend with `in_reducer = true` so nested reducers are
                // not separately minted, but index expressions etc. are
                // still traversed.
                builtin.for_each_expr_ref(|sub| {
                    walk_subexpr_for_aggs(
                        sub,
                        owner_var,
                        variables,
                        result,
                        next_synthetic_n,
                        /* in_reducer = */ true,
                    )
                });
            } else {
                builtin.for_each_expr_ref(|sub| {
                    walk_subexpr_for_aggs(
                        sub,
                        owner_var,
                        variables,
                        result,
                        next_synthetic_n,
                        in_reducer,
                    )
                });
            }
        }
        Expr2::Op1(_, operand, _, _) => walk_subexpr_for_aggs(
            operand,
            owner_var,
            variables,
            result,
            next_synthetic_n,
            in_reducer,
        ),
        Expr2::Op2(_, left, right, _, _) => {
            walk_subexpr_for_aggs(
                left,
                owner_var,
                variables,
                result,
                next_synthetic_n,
                in_reducer,
            );
            walk_subexpr_for_aggs(
                right,
                owner_var,
                variables,
                result,
                next_synthetic_n,
                in_reducer,
            );
        }
        Expr2::If(cond, then_e, else_e, _, _) => {
            walk_subexpr_for_aggs(
                cond,
                owner_var,
                variables,
                result,
                next_synthetic_n,
                in_reducer,
            );
            walk_subexpr_for_aggs(
                then_e,
                owner_var,
                variables,
                result,
                next_synthetic_n,
                in_reducer,
            );
            walk_subexpr_for_aggs(
                else_e,
                owner_var,
                variables,
                result,
                next_synthetic_n,
                in_reducer,
            );
        }
    }
}

/// What sort of aggregate node a reducer subexpression maps to.
enum AggKind {
    /// A `$⁚ltm⁚agg⁚{n}` auxiliary must be minted.
    Synthetic,
    /// The owning variable already is the aggregate node.
    VariableBacked {
        var_name: String,
        result_dims: Vec<String>,
    },
}

/// Register an aggregate node for `key` (canonical reducer text) and record
/// the `owner_var` -> agg-index association.
///
/// Synthetic aggs dedup on `key` (two inline uses of the same reducer
/// collapse to one `$⁚ltm⁚agg⁚{n}`). Variable-backed aggs are never deduped
/// -- each whole-RHS-reducer variable is its own distinct agg node, and an
/// inline use never reuses a variable-backed agg of the same text (that
/// would leave the inline reducer off the synthetic-agg path the downstream
/// `is_synthetic` filters require).
///
/// Determinism: `next_synthetic_n` is incremented only on a *new* synthetic
/// mint, in first-encounter order over the canonical-sorted variable list,
/// so two consumers walking the same ASTs see identical names.
fn register_agg(
    result: &mut AggNodesResult,
    next_synthetic_n: &mut usize,
    key: &str,
    owner_var: &str,
    kind: AggKind,
    source_vars: Vec<String>,
) {
    let mut sorted_sources = source_vars;
    sorted_sources.sort();
    sorted_sources.dedup();
    let idx = match kind {
        AggKind::Synthetic => {
            if let Some(&existing) = result.synthetic_by_key.get(key) {
                existing
            } else {
                let name = synthetic_agg_name(*next_synthetic_n);
                *next_synthetic_n += 1;
                result.aggs.push(AggNode {
                    name,
                    equation_text: key.to_string(),
                    source_vars: sorted_sources,
                    result_dims: vec![],
                    is_synthetic: true,
                });
                let idx = result.aggs.len() - 1;
                result.synthetic_by_key.insert(key.to_string(), idx);
                idx
            }
        }
        AggKind::VariableBacked {
            var_name,
            result_dims,
        } => {
            // Each whole-RHS-reducer variable is its own aggregate node;
            // never deduped, and not entered in `synthetic_by_key`.
            result.aggs.push(AggNode {
                name: var_name,
                equation_text: key.to_string(),
                source_vars: sorted_sources,
                result_dims,
                is_synthetic: false,
            });
            result.aggs.len() - 1
        }
    };
    let entry = result.by_var.entry(owner_var.to_string()).or_default();
    if !entry.contains(&idx) {
        entry.push(idx);
    }
}

/// If `builtin` is an array-reducing function applied to at least one arrayed
/// model variable, return the set of model-variable names it reads
/// (recursively, across the reducer's arguments). Otherwise return `None`.
///
/// Recognized reducers: `SUM`, `MEAN` (single-argument array form), single-arg
/// `MIN`/`MAX`, `STDDEV`, `RANK`. `SIZE` is intentionally excluded -- its link
/// score is always 0, mirroring `try_cross_dimensional_link_scores`'s
/// `Some(vec![])` for SIZE -- so a `SIZE(...)` subexpression is not hoisted.
///
/// A reducer is only recognized when at least one of its source variables is
/// arrayed (a scalar argument to `SUM`/`MEAN` is a no-op the parser would
/// normally reject anyway, and is never hoisted).
fn reducer_source_vars(
    builtin: &BuiltinFn<Expr2>,
    variables: &HashMap<Ident<Canonical>, crate::variable::Variable>,
) -> Option<Vec<String>> {
    let is_reducer = match builtin {
        BuiltinFn::Sum(_) => true,
        // The single-argument form of MEAN is the array reducer; the
        // multi-argument form is an element-wise mean of scalars.
        BuiltinFn::Mean(args) => args.len() == 1,
        // Single-argument MIN/MAX (no second arg) is the array reducer form.
        BuiltinFn::Min(_, None) | BuiltinFn::Max(_, None) => true,
        BuiltinFn::Stddev(_) => true,
        BuiltinFn::Rank(_, _) => true,
        _ => false,
    };
    if !is_reducer {
        return None;
    }

    let mut sources: Vec<String> = Vec::new();
    builtin.for_each_expr_ref(|arg| collect_var_refs(arg, &mut sources));
    // `collect_var_refs` picks up every identifier appearing in the
    // expression, which inside a subscript includes dimension names
    // (`matrix[D1, *]`) and literal element names (`pop[NYC]`). Keep only
    // identifiers that are actually model variables.
    sources.retain(|name| variables.contains_key(&Ident::<Canonical>::new(name)));
    if sources.is_empty() {
        return None;
    }
    // Require at least one arrayed source. Module variables are scalar nodes
    // in the causal graph and never count as an arrayed reducer source.
    let has_arrayed_source = sources.iter().any(|name| {
        variables
            .get(&Ident::<Canonical>::new(name))
            .and_then(|v| v.get_dimensions())
            .map(|dims| !dims.is_empty())
            .unwrap_or(false)
    });
    if !has_arrayed_source {
        return None;
    }
    sources.sort();
    sources.dedup();
    Some(sources)
}

/// Whether every arrayed-source reference inside `builtin`'s arguments is a
/// *full*-extent access: a bare `Var(x)` or a subscript whose indices are
/// all wildcards/star-ranges (`x[*]`, `x[*, *]`). Returns `false` if any
/// source variable is referenced with an explicit element name, an integer
/// literal index, a range, or an active-dimension index -- i.e. a slice
/// (`x[NYC, *]`, `x[D1, *]`), which `enumerate_agg_nodes` does not hoist as
/// a synthetic agg (see the call site for why).
///
/// Indices that are not subscripts on a model variable (e.g. a literal-only
/// scalar argument the parser would normally reject) don't make a reducer a
/// slice; this only inspects subscripts whose head is a known model variable.
fn reducer_is_full_reduce(
    builtin: &BuiltinFn<Expr2>,
    variables: &HashMap<Ident<Canonical>, crate::variable::Variable>,
) -> bool {
    let mut full = true;
    builtin.for_each_expr_ref(|arg| {
        if !full {
            return;
        }
        if !expr_is_full_extent(arg, variables) {
            full = false;
        }
    });
    full
}

/// Recursive helper for [`reducer_is_full_reduce`]: `false` if `expr`
/// contains a subscript on a model variable that uses any non-wildcard index.
fn expr_is_full_extent(
    expr: &Expr2,
    variables: &HashMap<Ident<Canonical>, crate::variable::Variable>,
) -> bool {
    match expr {
        Expr2::Const(..) | Expr2::Var(..) => true,
        Expr2::Subscript(ident, indices, _, _) => {
            // Subscripts whose head is not a model variable can't be a
            // sliced source (and won't affect the agg's source set), so
            // ignore them.
            if variables.contains_key(&Ident::<Canonical>::new(ident.as_str())) {
                for idx in indices {
                    match idx {
                        IndexExpr2::Wildcard(_) | IndexExpr2::StarRange(_, _) => {}
                        // A literal element, integer index, range, dim
                        // position, or arbitrary expression pins (a slice
                        // of) the source -- not a full reduce.
                        IndexExpr2::Expr(_) | IndexExpr2::DimPosition(_, _) => return false,
                        IndexExpr2::Range(_, _, _) => return false,
                    }
                }
            }
            // Also descend into index expressions (a nested source ref).
            indices.iter().all(|idx| match idx {
                IndexExpr2::Expr(e) => expr_is_full_extent(e, variables),
                IndexExpr2::Range(l, r, _) => {
                    expr_is_full_extent(l, variables) && expr_is_full_extent(r, variables)
                }
                IndexExpr2::Wildcard(_)
                | IndexExpr2::StarRange(_, _)
                | IndexExpr2::DimPosition(_, _) => true,
            })
        }
        Expr2::App(builtin, _, _) => {
            let mut ok = true;
            builtin.for_each_expr_ref(|sub| {
                if ok && !expr_is_full_extent(sub, variables) {
                    ok = false;
                }
            });
            ok
        }
        Expr2::Op1(_, operand, _, _) => expr_is_full_extent(operand, variables),
        Expr2::Op2(_, left, right, _, _) => {
            expr_is_full_extent(left, variables) && expr_is_full_extent(right, variables)
        }
        Expr2::If(cond, then_e, else_e, _, _) => {
            expr_is_full_extent(cond, variables)
                && expr_is_full_extent(then_e, variables)
                && expr_is_full_extent(else_e, variables)
        }
    }
}

/// `true` when `name` is a synthetic aggregate-node name (`$⁚ltm⁚agg⁚{n}`).
pub(crate) fn is_synthetic_agg_name(name: &str) -> bool {
    name.starts_with(AGG_NAME_PREFIX)
}

/// `true` when an aggregate node's reducer is monotone *non-decreasing* in
/// each of its source elements: `SUM`, `MEAN`, `MIN`, `MAX`. Raising any
/// one element can only raise (or leave unchanged) the result -- so a
/// `source[d] → agg` hop through such a reducer has `Positive` polarity.
/// `STDDEV` and `RANK` are not monotone (raising an element can move the
/// result either way), so a hop through them stays `Unknown`-polarity.
///
/// Keyed on the canonical reducer text (`AggNode::equation_text`, which is
/// `print_eqn` output -- function names lowercased, no space before `(`).
/// Only the single-argument `MIN`/`MAX` forms are ever hoisted into an
/// aggregate node, so a leading `min(` / `max(` is always the reducer form.
pub(crate) fn agg_reducer_is_monotone(equation_text: &str) -> bool {
    let t = equation_text.trim_start();
    t.starts_with("sum(")
        || t.starts_with("mean(")
        || t.starts_with("min(")
        || t.starts_with("max(")
}

/// Collect the canonical names of all model variables referenced (directly or
/// via subscript) in `expr`, including inside nested builtins and index
/// expressions.
fn collect_var_refs(expr: &Expr2, out: &mut Vec<String>) {
    match expr {
        Expr2::Const(..) => {}
        Expr2::Var(ident, _, _) => out.push(ident.as_str().to_string()),
        Expr2::Subscript(ident, indices, _, _) => {
            out.push(ident.as_str().to_string());
            for idx in indices {
                match idx {
                    IndexExpr2::Expr(e) => collect_var_refs(e, out),
                    IndexExpr2::Range(l, r, _) => {
                        collect_var_refs(l, out);
                        collect_var_refs(r, out);
                    }
                    IndexExpr2::Wildcard(_)
                    | IndexExpr2::StarRange(_, _)
                    | IndexExpr2::DimPosition(_, _) => {}
                }
            }
        }
        Expr2::App(builtin, _, _) => builtin.for_each_expr_ref(|sub| collect_var_refs(sub, out)),
        Expr2::Op1(_, operand, _, _) => collect_var_refs(operand, out),
        Expr2::Op2(_, left, right, _, _) => {
            collect_var_refs(left, out);
            collect_var_refs(right, out);
        }
        Expr2::If(cond, then_e, else_e, _, _) => {
            collect_var_refs(cond, out);
            collect_var_refs(then_e, out);
            collect_var_refs(else_e, out);
        }
    }
}

/// Map a canonical dimension name back to its datamodel casing, falling back
/// to the canonical form if no datamodel dimension matches.
fn canonical_dim_to_datamodel(canonical: &str, dm_dims: &[crate::datamodel::Dimension]) -> String {
    dm_dims
        .iter()
        .find(|dm| canonicalize(dm.name()).as_ref() == canonical)
        .map(|dm| dm.name().to_string())
        .unwrap_or_else(|| canonical.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{SimlinDb, sync_from_datamodel};
    use crate::test_common::TestProject;

    /// Build a `TestProject`, sync into salsa, and return the enumerated
    /// aggregate nodes for the "main" model.
    fn agg_nodes(project: &TestProject) -> AggNodesResult {
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let source_model = sync.models["main"].source;
        let source_project = sync.project;
        enumerate_agg_nodes(&db, source_model, source_project).clone()
    }

    /// AC4.3: a variable whose entire dt-equation is exactly one reducer call
    /// (scalar) mints no synthetic agg -- the variable itself is the agg.
    #[test]
    fn whole_rhs_scalar_reducer_is_its_own_agg() {
        let project = TestProject::new("whole_rhs")
            .named_dimension("Region", &["NYC", "Boston", "LA"])
            .array_aux("population[Region]", "100")
            .scalar_aux("total_population", "SUM(population[*])");

        let result = agg_nodes(&project);

        // No `$⁚ltm⁚agg⁚{n}` minted.
        assert!(
            result.aggs.iter().all(|a| !a.is_synthetic),
            "whole-RHS scalar reducer must not mint a synthetic agg; got: {:?}",
            result.aggs
        );
        // The reducer maps to a variable-backed agg named `total_population`,
        // owned by `total_population`'s equation. (Variable-backed aggs are
        // resolved via `aggs_in_var`, not `agg_for_key` -- the latter is
        // synthetic-only, since two different scalars can each be `SUM(pop[*])`.)
        let agg = result
            .aggs_in_var("total_population")
            .find(|a| a.name == "total_population")
            .expect("expected a variable-backed agg owned by `total_population`");
        assert!(!agg.is_synthetic);
        assert_eq!(agg.source_vars, vec!["population".to_string()]);
        assert!(agg.result_dims.is_empty());
        // `agg_for_key` resolves only synthetic aggs, so it must not find this one.
        assert!(result.agg_for_key("sum(population[*])").is_none());
    }

    /// AC4.3 (arrayed variant): `agg[D1] = SUM(matrix[D1,*])` is whole-RHS, so
    /// the variable is the agg; `result_dims` carries `D1`.
    #[test]
    fn whole_rhs_arrayed_partial_reduce_is_its_own_agg() {
        let project = TestProject::new("whole_rhs_partial")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
            .array_aux_direct("agg", vec!["D1".into()], "SUM(matrix[D1, *])", None);

        let result = agg_nodes(&project);

        assert!(
            result.aggs.iter().all(|a| !a.is_synthetic),
            "whole-RHS arrayed reducer must not mint a synthetic agg; got: {:?}",
            result.aggs
        );
        let agg = result
            .aggs_in_var("agg")
            .next()
            .expect("expected an agg owned by `agg`");
        assert_eq!(agg.name, "agg");
        assert!(!agg.is_synthetic);
        assert_eq!(agg.source_vars, vec!["matrix".to_string()]);
        assert_eq!(agg.result_dims, vec!["D1".to_string()]);
    }

    /// AC4.3 (arrayed full-reduce broadcast): `share[Region] = SUM(pop[*])` is
    /// a whole-RHS reducer, so the variable is the agg -- but `SUM(pop[*])` is a
    /// *full* reduce (scalar result) merely broadcast to `[Region]`, so the
    /// agg's `result_dims` is `[]`, not `[Region]`. (Contrast with
    /// `agg[D1] = SUM(matrix[D1, *])`, a partial reduce that genuinely varies
    /// per `D1`.)
    #[test]
    fn whole_rhs_arrayed_full_reduce_broadcast_has_scalar_result_dims() {
        let project = TestProject::new("whole_rhs_broadcast")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            .array_aux("share[Region]", "SUM(pop[*])");

        let result = agg_nodes(&project);

        assert!(
            result.aggs.iter().all(|a| !a.is_synthetic),
            "whole-RHS reducer must not mint a synthetic agg; got: {:?}",
            result.aggs
        );
        let agg = result
            .aggs_in_var("share")
            .next()
            .expect("expected an agg owned by `share`");
        assert_eq!(agg.name, "share");
        assert!(!agg.is_synthetic);
        assert_eq!(agg.source_vars, vec!["pop".to_string()]);
        assert!(
            agg.result_dims.is_empty(),
            "a full reduce broadcast to an arrayed variable has scalar result dims, got: {:?}",
            agg.result_dims
        );
    }

    /// AC4.1 (the basic mint): `share[r] = pop[r] / SUM(pop[*])` mints one
    /// synthetic agg `$⁚ltm⁚agg⁚0` for the sub-expression `SUM(pop[*])`.
    #[test]
    fn subexpression_reducer_mints_one_synthetic_agg() {
        let project = TestProject::new("share_mint")
            .named_dimension("Region", &["NYC", "Boston", "LA"])
            .array_aux("pop[Region]", "100")
            .array_aux("share[Region]", "pop / SUM(pop[*])");

        let result = agg_nodes(&project);

        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "expected exactly one synthetic agg; got: {:?}",
            result.aggs
        );
        assert_eq!(synthetic[0].name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
        assert_eq!(synthetic[0].equation_text, "sum(pop[*])");
        assert_eq!(synthetic[0].source_vars, vec!["pop".to_string()]);
        assert!(synthetic[0].result_dims.is_empty());
        assert!(
            result
                .aggs_in_var("share")
                .any(|a| a.name == "$\u{205A}ltm\u{205A}agg\u{205A}0")
        );
    }

    /// P2 regression: an inline reducer (`share[r] = pop[r] / SUM(pop[*])`,
    /// which must mint a *synthetic* agg) sharing canonical text with a
    /// *whole-RHS* reducer of the same shape (`denom = SUM(pop[*])`, which
    /// is *variable-backed*) must NOT reuse the variable-backed agg --
    /// regardless of declaration order. Dedup-by-key applies to synthetic
    /// aggs only; variable-backed aggs are never deduped (a whole-RHS
    /// reducer variable is its own distinct agg node). Before the fix, with
    /// `denom` visited first (canonical-sorted: `denom` < `share`), the
    /// inline use found `by_key["sum(pop[*])"]` already populated by `denom`
    /// and reused it, so `share` got no synthetic agg and its reducer fell
    /// back to the conservative direct path.
    #[test]
    fn inline_reducer_does_not_reuse_variable_backed_agg() {
        let project = TestProject::new("inline_vs_var_backed")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            // `denom` (canonical-sorted first) is a whole-RHS reducer ->
            // variable-backed agg named `denom`.
            .scalar_aux("denom", "SUM(pop[*])")
            // `share` (visited after `denom`) uses the same reducer text as
            // a sub-expression -> must mint its own synthetic agg.
            .array_aux("share[Region]", "pop / SUM(pop[*])");

        let result = agg_nodes(&project);

        // The variable-backed agg `denom` exists and is not synthetic.
        // (`agg_for_key` now resolves only synthetic aggs, so look up the
        // variable-backed one through `by_var` instead.)
        let denom_agg = result
            .aggs_in_var("denom")
            .find(|a| a.name == "denom")
            .expect("expected a variable-backed agg owned by `denom`");
        assert!(
            !denom_agg.is_synthetic,
            "`denom`'s agg must be variable-backed"
        );
        assert_eq!(denom_agg.equation_text, "sum(pop[*])");

        // `share` must own a *synthetic* agg with the same reducer text.
        let share_agg = result
            .aggs_in_var("share")
            .find(|a| a.is_synthetic)
            .expect("expected a synthetic agg owned by `share`");
        assert_eq!(share_agg.name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
        assert_eq!(share_agg.equation_text, "sum(pop[*])");
        assert_eq!(share_agg.source_vars, vec!["pop".to_string()]);
        // `agg_for_key` resolves the reducer text to the *synthetic* agg.
        assert_eq!(
            result.agg_for_key("sum(pop[*])").map(|a| a.name.as_str()),
            Some("$\u{205A}ltm\u{205A}agg\u{205A}0")
        );

        // There must be exactly one synthetic agg and exactly one
        // variable-backed agg -- two distinct nodes despite identical text.
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        let var_backed_aggs: Vec<&AggNode> =
            result.aggs.iter().filter(|a| !a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "expected one synthetic agg, got: {:?}",
            result.aggs
        );
        assert_eq!(
            var_backed_aggs.len(),
            1,
            "expected one variable-backed agg, got: {:?}",
            result.aggs
        );
    }

    /// P2 regression (reverse declaration order): the same model as
    /// `inline_reducer_does_not_reuse_variable_backed_agg` but built so that
    /// the inline-use variable would be visited first if order mattered.
    /// `enumerate_agg_nodes` visits variables in canonical-sorted order, so
    /// `denom` < `share` always; this test instead uses different names
    /// (`a_share` < `z_denom`) to confirm the synthetic agg is minted when
    /// the inline use is encountered *before* the whole-RHS reducer.
    #[test]
    fn inline_reducer_mints_synthetic_when_visited_before_variable_backed() {
        let project = TestProject::new("inline_first")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            // `a_share` (canonical-sorted first) uses the reducer inline.
            .array_aux("a_share[Region]", "pop / SUM(pop[*])")
            // `z_denom` (visited after) is the whole-RHS reducer.
            .scalar_aux("z_denom", "SUM(pop[*])");

        let result = agg_nodes(&project);

        let share_agg = result
            .aggs_in_var("a_share")
            .find(|a| a.is_synthetic)
            .expect("expected a synthetic agg owned by `a_share`");
        assert_eq!(share_agg.name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
        assert_eq!(share_agg.equation_text, "sum(pop[*])");

        let denom_agg = result
            .aggs_in_var("z_denom")
            .find(|a| a.name == "z_denom")
            .expect("expected a variable-backed agg owned by `z_denom`");
        assert!(!denom_agg.is_synthetic);

        assert_eq!(result.aggs.iter().filter(|a| a.is_synthetic).count(), 1);
        assert_eq!(result.aggs.iter().filter(|a| !a.is_synthetic).count(), 1);
    }

    /// Two whole-RHS reducers with *identical* canonical text are two
    /// distinct variable-backed agg nodes (one per variable) -- never
    /// deduped, because each variable genuinely is its own aggregate.
    #[test]
    fn two_whole_rhs_reducers_same_text_are_distinct_aggs() {
        let project = TestProject::new("two_var_backed")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            .scalar_aux("total_a", "SUM(pop[*])")
            .scalar_aux("total_b", "SUM(pop[*])");

        let result = agg_nodes(&project);

        let var_backed: Vec<&AggNode> = result.aggs.iter().filter(|a| !a.is_synthetic).collect();
        assert_eq!(
            var_backed.len(),
            2,
            "two whole-RHS reducers must be two distinct variable-backed aggs; got: {:?}",
            result.aggs
        );
        let names: std::collections::HashSet<&str> =
            var_backed.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains("total_a"), "missing total_a: {names:?}");
        assert!(names.contains("total_b"), "missing total_b: {names:?}");
        // No synthetic aggs (neither reducer is a sub-expression).
        assert_eq!(result.aggs.iter().filter(|a| a.is_synthetic).count(), 0);
    }

    /// Two *inline* uses of the same reducer text still dedupe to one
    /// synthetic agg (the synthetic dedup-by-key path is preserved).
    #[test]
    fn two_inline_uses_same_text_dedupe_to_one_synthetic() {
        let project = TestProject::new("two_inline")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            .array_aux("share_a[Region]", "pop / SUM(pop[*])")
            .array_aux("share_b[Region]", "pop * 2 / SUM(pop[*])");

        let result = agg_nodes(&project);

        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "two inline uses of the same reducer must dedupe to one synthetic agg; got: {:?}",
            result.aggs
        );
        assert_eq!(synthetic[0].name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
        // Both variables reference the same deduped synthetic agg index.
        let a_idx = result.by_var.get("share_a").cloned().unwrap_or_default();
        let b_idx = result.by_var.get("share_b").cloned().unwrap_or_default();
        assert_eq!(a_idx, b_idx);
    }

    /// AC4.4 (nested reducers): `x = SUM(a[*]) / SUM(b[*])` mints two distinct
    /// synthetic agg nodes (`$⁚ltm⁚agg⁚0` for `SUM(a[*])`, `$⁚ltm⁚agg⁚1` for
    /// `SUM(b[*])`). The `/` is not a reducer; neither `SUM` is inside the
    /// other, so both are maximal.
    #[test]
    fn nested_reducers_mint_two_aggs() {
        let project = TestProject::new("nested")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("a[Region]", "10")
            .array_aux("b[Region]", "20")
            .scalar_aux("x", "SUM(a[*]) / SUM(b[*])");

        let result = agg_nodes(&project);

        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            2,
            "expected two synthetic aggs; got: {:?}",
            result.aggs
        );
        // First-encounter (left-to-right DFS) order: SUM(a[*]) then SUM(b[*]).
        assert_eq!(synthetic[0].name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
        assert_eq!(synthetic[0].equation_text, "sum(a[*])");
        assert_eq!(synthetic[0].source_vars, vec!["a".to_string()]);
        assert_eq!(synthetic[1].name, "$\u{205A}ltm\u{205A}agg\u{205A}1");
        assert_eq!(synthetic[1].equation_text, "sum(b[*])");
        assert_eq!(synthetic[1].source_vars, vec!["b".to_string()]);
    }

    /// AC4.4 (dedup): the same reducer subexpression appearing in two
    /// variables' equations (with whitespace/casing differences in the
    /// source text) maps to one synthetic agg node referenced by both.
    #[test]
    fn ast_identical_reducers_dedupe() {
        let project = TestProject::new("dedup")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            // Two different equations both contain SUM(pop[*]); the first is
            // spelled with extra spacing and uppercase.
            .array_aux("share_a[Region]", "pop / SUM( POP [ * ] )")
            .array_aux("share_b[Region]", "pop * 2 / sum(pop[*])");

        let result = agg_nodes(&project);

        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "AST-identical reducers must dedupe to one agg; got: {:?}",
            result.aggs
        );
        assert_eq!(synthetic[0].equation_text, "sum(pop[*])");
        // Both variables reference the same agg index.
        let a_idx: Vec<usize> = result.by_var.get("share_a").cloned().unwrap_or_default();
        let b_idx: Vec<usize> = result.by_var.get("share_b").cloned().unwrap_or_default();
        assert_eq!(a_idx.len(), 1);
        assert_eq!(b_idx.len(), 1);
        assert_eq!(
            a_idx, b_idx,
            "both variables must point at the same deduped agg index"
        );
    }

    /// Per-element `Ast::Arrayed` target with a different reducer per element:
    /// `x[a] = SUM(p[*]); x[b] = MEAN(p[*])` mints two synthetic agg nodes,
    /// one per element's reducer.
    #[test]
    fn per_element_arrayed_target_mints_one_agg_per_element_reducer() {
        let project = TestProject::new("per_elem")
            .named_dimension("D", &["a", "b"])
            .array_aux("p[D]", "1")
            .array_with_ranges_direct(
                "x",
                vec!["D".into()],
                vec![("a", "SUM(p[*])"), ("b", "MEAN(p[*])")],
                None,
            );

        let result = agg_nodes(&project);

        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            2,
            "per-element reducers must mint one agg per element; got: {:?}",
            result.aggs
        );
        let texts: std::collections::HashSet<&str> =
            synthetic.iter().map(|a| a.equation_text.as_str()).collect();
        assert!(texts.contains("sum(p[*])"), "missing sum(p[*]): {texts:?}");
        assert!(
            texts.contains("mean(p[*])"),
            "missing mean(p[*]): {texts:?}"
        );
        // Both are owned by `x`.
        let x_idx = result.by_var.get("x").cloned().unwrap_or_default();
        assert_eq!(x_idx.len(), 2);
    }

    /// Determinism: the same model built twice (or with variables declared in
    /// a different order) yields identical agg names assigned to the same
    /// subexpressions.
    #[test]
    fn enumeration_is_deterministic_under_variable_reordering() {
        // Two synthetic aggs: SUM(a[*]) and SUM(b[*]). Whichever variable
        // happens to be visited first is irrelevant -- we always visit in
        // canonical-name sorted order, and within an equation left-to-right.
        let build = |order_a_first: bool| {
            let mut p = TestProject::new("determinism")
                .named_dimension("Region", &["NYC", "Boston"])
                .array_aux("a[Region]", "10")
                .array_aux("b[Region]", "20");
            // `q` references SUM(a[*]) and SUM(b[*]); `r` references the same
            // pair. We add them in different orders to confirm the result is
            // identical.
            if order_a_first {
                p = p
                    .scalar_aux("q", "SUM(a[*]) + SUM(b[*])")
                    .scalar_aux("r", "SUM(a[*]) * SUM(b[*])");
            } else {
                p = p
                    .scalar_aux("r", "SUM(a[*]) * SUM(b[*])")
                    .scalar_aux("q", "SUM(a[*]) + SUM(b[*])");
            }
            agg_nodes(&p)
        };

        let r1 = build(true);
        let r2 = build(false);
        assert_eq!(
            r1.aggs, r2.aggs,
            "enumeration must be deterministic regardless of declaration order"
        );
        assert_eq!(r1.synthetic_by_key, r2.synthetic_by_key);
        // Specifically: SUM(a[*]) -> agg 0, SUM(b[*]) -> agg 1 (a < b, and
        // within q's equation SUM(a[*]) precedes SUM(b[*])).
        assert_eq!(
            r1.agg_for_key("sum(a[*])").map(|a| a.name.clone()),
            Some("$\u{205A}ltm\u{205A}agg\u{205A}0".to_string())
        );
        assert_eq!(
            r1.agg_for_key("sum(b[*])").map(|a| a.name.clone()),
            Some("$\u{205A}ltm\u{205A}agg\u{205A}1".to_string())
        );
    }

    /// A model with no reducers produces an empty result.
    #[test]
    fn model_without_reducers_has_no_aggs() {
        let project = TestProject::new("no_reducers")
            .stock("population", "100", &["births"], &["deaths"], None)
            .flow("births", "population * 0.1", None)
            .flow("deaths", "population * 0.05", None)
            .scalar_const("rate", 0.1);

        let result = agg_nodes(&project);
        assert!(
            result.aggs.is_empty(),
            "model without reducers must have no aggs; got: {:?}",
            result.aggs
        );
        assert!(result.synthetic_by_key.is_empty());
        assert!(result.by_var.is_empty());
    }

    /// A reducer over a *scalar* source is not hoisted (the parser would
    /// normally reject it anyway, but be defensive).
    #[test]
    fn reducer_over_scalar_source_is_not_hoisted() {
        // `SUM(s)` where `s` is scalar -- pathological, but must not mint an
        // agg. (We also keep a real arrayed reducer to confirm the
        // enumerator still finds the legitimate one.)
        let project = TestProject::new("scalar_reducer")
            .named_dimension("Region", &["NYC", "Boston"])
            .scalar_aux("s", "5")
            .array_aux("pop[Region]", "100")
            .scalar_aux("y", "SUM(s) + SUM(pop[*])");

        let result = agg_nodes(&project);
        // Only the arrayed reducer is recognized.
        assert!(
            result.agg_for_key("sum(pop[*])").is_some(),
            "the arrayed reducer must be recognized; got: {:?}",
            result.aggs
        );
        assert!(
            result.agg_for_key("sum(s)").is_none(),
            "a reducer over a scalar source must not be hoisted; got: {:?}",
            result.aggs
        );
    }

    /// SIZE is not hoisted -- its link score is always 0, matching
    /// `try_cross_dimensional_link_scores`'s `Some(vec![])` for SIZE.
    #[test]
    fn size_reducer_is_not_hoisted() {
        let project = TestProject::new("size_reducer")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            .scalar_aux("n", "SIZE(pop[*])");

        let result = agg_nodes(&project);
        assert!(
            result.aggs.is_empty(),
            "SIZE must not be hoisted as an agg; got: {:?}",
            result.aggs
        );
    }

    /// A reducer over an explicit *slice* used as a sub-expression
    /// (`x[r] = ... + SUM(pop[NYC, *])`) is NOT hoisted: the slice pinning
    /// would have to ride on the agg's source descriptor (which only carries
    /// the source variable name, not which elements the slice reads), so the
    /// element-graph reroute and the per-element reducer link scores would
    /// over-approximate to the whole array (the link score for the unread
    /// rows would be a nonzero garbage value instead of 0). Such a
    /// subexpression stays conservatively `Wildcard`-classified. Tracked as
    /// a follow-up.
    #[test]
    fn slice_reducer_subexpression_is_not_hoisted() {
        let project = TestProject::new("slice_subexpr")
            .named_dimension("Region", &["NYC", "Boston"])
            .named_dimension("Age", &["Adult", "Child"])
            .array_aux_direct("pop", vec!["Region".into(), "Age".into()], "10", None)
            .array_aux_direct(
                "x",
                vec!["Region".into()],
                "pop[NYC, Adult] + SUM(pop[NYC, *])",
                None,
            );

        let result = agg_nodes(&project);
        assert!(
            result.aggs.is_empty(),
            "a slice-reducer subexpression must not be hoisted; got: {:?}",
            result.aggs
        );
    }

    /// A whole-RHS slice/partial reduce (`agg[D1] = SUM(matrix[D1, *])`) IS
    /// recognized -- but as a variable-backed agg, not a synthetic one
    /// (covered by `whole_rhs_arrayed_partial_reduce_is_its_own_agg`). The
    /// carve-out above is specifically for the *sub-expression* case.
    #[test]
    fn full_wildcard_reducer_subexpression_is_still_hoisted() {
        // `SUM(matrix[*, *])` (all-wildcard, no literal pin) is a full
        // reduce and IS hoistable as a synthetic agg.
        let project = TestProject::new("full_wildcard_subexpr")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
            .scalar_aux("y", "5 + SUM(matrix[*, *])");

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "an all-wildcard reducer subexpression must mint one synthetic agg; got: {:?}",
            result.aggs
        );
        assert_eq!(synthetic[0].source_vars, vec!["matrix".to_string()]);
    }
}
