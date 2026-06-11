// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Property tests for the variable-level <-> element-level causal graph
//! projection invariant (AC1.4).
//!
//! The element graph is meant to be a *finer* view of the variable graph:
//! every element-level edge `from[d] -> to[e]` projects to a variable-level
//! edge `from -> to` after stripping subscripts and deduplicating. Synthetic
//! `$⁚ltm⁚agg⁚{n}` aggregate nodes (minted for INLINED reducer
//! subexpressions like `1 + SUM(v0[*])`) are spliced out of the projection
//! first -- `from -> agg -> to` collapses to `from -> to`, the same trim
//! real consumers apply to reported loops/links -- so the invariant is
//! stated over the trimmed graph. The variable-level edge set must equal
//! that projection: if the element builder ever drops a class of edges or
//! invents one without a variable-level counterpart, this property fails.
//!
//! The projection invariant alone is NOT sensitive to the GH #533 bug class
//! (a both-scalar fast path emitting a direct `scale -> total` edge instead
//! of the `scale -> agg` hop projects to the SAME variable edge), so the
//! properties additionally assert per-reducer agg-hop routing derived from
//! the spec: every inlined reducer must have ONE agg node carrying all its
//! source-row / scalar-feeder / target hops, and NO reducer source (arrayed
//! row or scalar feeder -- all reference their target solely inside the
//! reducer) may have a direct element edge to any target node
//! (`check_spec_agg_expectations`).
//!
//! Vacuity guard (GH #739): the base `project_spec_strategy` only sometimes
//! generates inlined-reducer patterns, so a second property runs over
//! `forced_reducer_spec_strategy`, which ALWAYS injects at least one
//! inlined-reducer pattern (covering both `emit_agg_routed_edges` arms:
//! arrayed source rows and the empty-`from_dims` scalar feeder, against
//! both arrayed and scalar targets). That property asserts a synthetic agg
//! node exists for every generated spec, so the ThroughAgg coverage can
//! never silently go vacuous again. A deterministic companion test pins the
//! same expectations on a fixed spec containing every reducer pattern.
//!
//! This file is a regression guard: it must pass on the existing
//! collapsing classifier today and continue to pass after the Phase 2
//! AST-walking refactor lands. It deliberately does NOT exercise the
//! over-expansion bug that AC1.1 / AC1.3 exist to catch (over-expansion
//! preserves the projection invariant by construction); see those tests
//! for fixed-index per-reference truthfulness.

use proptest::prelude::*;
use std::collections::BTreeSet;

use crate::db::{
    ElementCausalEdgesResult, SimlinDb, model_causal_edges, model_element_causal_edges,
    sync_from_datamodel,
};
use crate::ltm_agg::is_synthetic_agg_name;
use crate::test_common::TestProject;

/// One reference pattern in a generated equation. `var_idx` is the index
/// of the source variable to reference (always strictly less than the
/// referencing variable's own index, to keep the dependency DAG acyclic).
///
/// The reducer patterns are deliberately SUM-only: every reducer here is
/// hoisted, so the per-reducer agg-hop expectations assume `ThroughAgg`
/// routing. If a later task adds RANK patterns to the strategy, their
/// expectations must encode the DE-HOISTED `Direct` routing instead --
/// RANK is array-valued and never hoisted (GH #771,
/// `ltm_agg::reducer_is_hoistable`).
#[derive(Debug, Clone)]
enum RefPattern {
    /// `vN` -- bare same-element reference.
    Bare { var_idx: usize },
    /// `SUM(vN[*])` -- wildcard reducer over the source's dimension.
    WildcardReducer { var_idx: usize },
    /// `vN[ELEM_K]` -- fixed-index reference to one literal element.
    FixedIndex { var_idx: usize, elem_idx: usize },
    /// `vA + vB` -- binary sum of two bare arrayed refs.
    SumOfTwo { left: usize, right: usize },
    /// Constant scalar like `10`. Produces zero source edges; included to
    /// give the strategy variety so generated models aren't all dependency
    /// chains.
    Constant,
    /// `scalar_const + vN` -- a scalar reference plus a bare arrayed ref.
    /// Only emitted when the project includes a scalar variable.
    ScalarPlusBare { var_idx: usize },
    /// `1 + SUM(vN[*])` -- an INLINED whole-extent reducer (the reducer is a
    /// sub-expression, not the whole RHS), so a synthetic `$⁚ltm⁚agg⁚{n}`
    /// node is minted and the reference is `ThroughAgg`-routed: arrayed
    /// source rows feed the agg, the agg broadcasts to the target. Contrast
    /// `WildcardReducer`, the variable-backed whole-RHS form that mints NO
    /// synthetic node.
    InlinedReducer { var_idx: usize },
    /// `1 + SUM(vN[*] * scalar_const)` -- an inlined reducer with a SCALAR
    /// feeder multiplied in. The `scalar_const` reference exercises the
    /// empty-`from_dims` arm of `emit_agg_routed_edges` (`scalar_const ->
    /// agg`); with a scalar target (see [`ScalarReducerTarget`]) this is the
    /// GH #533 both-scalar shape. Only emitted when the project includes a
    /// scalar variable.
    InlinedReducerScalarFeeder { var_idx: usize },
}

impl RefPattern {
    /// `Some((source_var_idx, has_scalar_feeder))` when the pattern is an
    /// inlined (synthetic-agg-minting) reducer.
    fn inlined_reducer_parts(&self) -> Option<(usize, bool)> {
        match self {
            RefPattern::InlinedReducer { var_idx } => Some((*var_idx, false)),
            RefPattern::InlinedReducerScalarFeeder { var_idx } => Some((*var_idx, true)),
            _ => None,
        }
    }
}

/// Hand-crafted spec for one generated arrayed variable: which patterns it
/// uses for its equation. The `dim_idx` field is unused today (single-
/// dimension models only) but reserved for future multi-dim extension.
#[derive(Debug, Clone)]
struct ArrayedVarSpec {
    pattern: RefPattern,
}

/// Optional trailing SCALAR variable `total` whose equation embeds an
/// inlined reducer over arrayed variable `v{var_idx}` -- the scalar-TARGET
/// arm of `ThroughAgg` routing (`agg -> total`, with `target_element`
/// pinning degenerate). With `with_scalar_feeder` the equation is
/// `1 + SUM(v{var_idx}[*] * scalar_const)`: the `(scalar_const, total)`
/// causal edge is then both-scalar AND `ThroughAgg`-classified -- exactly
/// the GH #533 fast-path shape. Kept separate from `var_specs` (appended
/// last, referenced by nothing) so arrayed patterns never have to reason
/// about scalar predecessors.
#[derive(Debug, Clone)]
struct ScalarReducerTarget {
    var_idx: usize,
    with_scalar_feeder: bool,
}

/// Spec for the whole generated project. The `var_specs[i]` slot's
/// pattern can only reference indices strictly less than `i`.
#[derive(Debug, Clone)]
struct ProjectSpec {
    /// Number of elements in the single dimension. The generator picks
    /// from `{1, 2, 3, 5}` to cover edge cases (single-element dim) and
    /// modest dim sizes without blowing up cartesian-product edge counts.
    dim_size: usize,
    /// Per-arrayed-variable equation specs. `var_specs.len()` is between
    /// 2 and 5 inclusive.
    var_specs: Vec<ArrayedVarSpec>,
    /// Whether the project includes one scalar variable named
    /// `scalar_const`. When true, `ScalarPlusBare` patterns become valid
    /// for arrayed variables; when false, the generator avoids them.
    include_scalar: bool,
    /// Optional scalar `total` variable with an inlined-reducer equation
    /// (see [`ScalarReducerTarget`]). `with_scalar_feeder` is only set when
    /// `include_scalar` is.
    scalar_reducer_target: Option<ScalarReducerTarget>,
}

/// Element names for the synthetic dimension. The generator tops out at
/// dim_size = 5, so we only need names through `e4`. Lowercase matches
/// the canonical form the salsa pipeline uses for edge keys.
const ELEM_NAMES: &[&str] = &["a", "b", "c", "d", "e"];

/// Strategy: pick a reference pattern for variable `var_idx` given the
/// project's dim size and whether a scalar exists.
///
/// `var_idx == 0` collapses to `Constant` (no earlier variables to ref).
/// Higher indices choose freely from the bag, with the source index
/// uniformly drawn from `0..var_idx` so every earlier variable is fair
/// game.
fn pattern_strategy(
    var_idx: usize,
    dim_size: usize,
    include_scalar: bool,
) -> BoxedStrategy<RefPattern> {
    if var_idx == 0 {
        return Just(RefPattern::Constant).boxed();
    }
    let max_src = var_idx; // exclusive upper bound
    let mut variants: Vec<BoxedStrategy<RefPattern>> = vec![
        Just(RefPattern::Constant).boxed(),
        (0..max_src)
            .prop_map(|j| RefPattern::Bare { var_idx: j })
            .boxed(),
        (0..max_src)
            .prop_map(|j| RefPattern::WildcardReducer { var_idx: j })
            .boxed(),
        (0..max_src, 0..dim_size)
            .prop_map(|(j, k)| RefPattern::FixedIndex {
                var_idx: j,
                elem_idx: k,
            })
            .boxed(),
        (0..max_src, 0..max_src)
            .prop_map(|(a, b)| RefPattern::SumOfTwo { left: a, right: b })
            .boxed(),
        (0..max_src)
            .prop_map(|j| RefPattern::InlinedReducer { var_idx: j })
            .boxed(),
    ];
    if include_scalar {
        variants.push(
            (0..max_src)
                .prop_map(|j| RefPattern::ScalarPlusBare { var_idx: j })
                .boxed(),
        );
        variants.push(
            (0..max_src)
                .prop_map(|j| RefPattern::InlinedReducerScalarFeeder { var_idx: j })
                .boxed(),
        );
    }
    proptest::strategy::Union::new(variants).boxed()
}

/// Strategy: generate a complete project spec.
fn project_spec_strategy() -> impl Strategy<Value = ProjectSpec> {
    let dim_strategy = prop_oneof![Just(1usize), Just(2usize), Just(3usize), Just(5usize)];
    let var_count_strategy = 2usize..=5usize;
    let scalar_strategy = any::<bool>();

    (dim_strategy, var_count_strategy, scalar_strategy).prop_flat_map(
        |(dim_size, var_count, include_scalar)| {
            // Build the per-variable pattern strategies with the
            // dim_size/include_scalar context closed over.
            let pattern_strategies: Vec<BoxedStrategy<RefPattern>> = (0..var_count)
                .map(|i| pattern_strategy(i, dim_size, include_scalar))
                .collect();
            // Optional scalar reducer target: roughly a third of specs get
            // one, sourcing from any arrayed var (it is appended last and
            // referenced by nothing, so acyclicity is preserved). The
            // scalar-feeder form requires the scalar variable to exist.
            let scalar_target_strategy =
                proptest::option::weighted(0.34, (0..var_count, any::<bool>()));
            (pattern_strategies, scalar_target_strategy).prop_map(
                move |(patterns, scalar_target)| ProjectSpec {
                    dim_size,
                    var_specs: patterns
                        .into_iter()
                        .map(|pattern| ArrayedVarSpec { pattern })
                        .collect(),
                    include_scalar,
                    scalar_reducer_target: scalar_target.map(|(var_idx, feeder)| {
                        ScalarReducerTarget {
                            var_idx,
                            with_scalar_feeder: feeder && include_scalar,
                        }
                    }),
                },
            )
        },
    )
}

/// Which inlined-reducer shape `forced_reducer_spec_strategy` injects.
/// The four kinds cover both `emit_agg_routed_edges` arms (arrayed source
/// rows; empty-`from_dims` scalar feeder) against both target shapes
/// (arrayed `v{i}`; scalar `total`).
#[derive(Debug, Clone, Copy)]
enum ForcedReducerKind {
    ArrayedTarget,
    ArrayedTargetScalarFeeder,
    ScalarTarget,
    ScalarTargetScalarFeeder,
}

/// Strategy: a [`ProjectSpec`] GUARANTEED to contain at least one inlined
/// reducer, by injecting one of the four [`ForcedReducerKind`] shapes into
/// a base spec. This is the structural non-vacuity guard for GH #739: the
/// base strategy only *sometimes* generates reducer patterns, so the
/// `ThroughAgg` property runs over this strategy and additionally asserts a
/// synthetic agg node exists for every case.
fn forced_reducer_spec_strategy() -> impl Strategy<Value = ProjectSpec> {
    let kind_strategy = prop_oneof![
        Just(ForcedReducerKind::ArrayedTarget),
        Just(ForcedReducerKind::ArrayedTargetScalarFeeder),
        Just(ForcedReducerKind::ScalarTarget),
        Just(ForcedReducerKind::ScalarTargetScalarFeeder),
    ];
    (
        project_spec_strategy(),
        kind_strategy,
        any::<prop::sample::Index>(),
        any::<prop::sample::Index>(),
    )
        .prop_map(|(mut spec, kind, slot_idx, src_idx)| {
            let n = spec.var_specs.len(); // >= 2 by construction
            match kind {
                ForcedReducerKind::ArrayedTarget | ForcedReducerKind::ArrayedTargetScalarFeeder => {
                    // Overwrite one arrayed var's pattern (index >= 1 so a
                    // strictly-earlier source exists; acyclicity preserved).
                    let i = 1 + slot_idx.index(n - 1);
                    let j = src_idx.index(i);
                    let feeder = matches!(kind, ForcedReducerKind::ArrayedTargetScalarFeeder);
                    if feeder {
                        spec.include_scalar = true;
                    }
                    spec.var_specs[i].pattern = if feeder {
                        RefPattern::InlinedReducerScalarFeeder { var_idx: j }
                    } else {
                        RefPattern::InlinedReducer { var_idx: j }
                    };
                }
                ForcedReducerKind::ScalarTarget | ForcedReducerKind::ScalarTargetScalarFeeder => {
                    let feeder = matches!(kind, ForcedReducerKind::ScalarTargetScalarFeeder);
                    if feeder {
                        spec.include_scalar = true;
                    }
                    spec.scalar_reducer_target = Some(ScalarReducerTarget {
                        var_idx: src_idx.index(n),
                        with_scalar_feeder: feeder,
                    });
                }
            }
            spec
        })
}

/// Render a `RefPattern` to an equation string referencing earlier
/// variables by their canonical names (`v0`, `v1`, ...).
fn render_pattern(pattern: &RefPattern) -> String {
    match pattern {
        RefPattern::Bare { var_idx } => format!("v{var_idx}"),
        RefPattern::WildcardReducer { var_idx } => format!("SUM(v{var_idx}[*])"),
        RefPattern::FixedIndex { var_idx, elem_idx } => {
            format!("v{var_idx}[{}]", ELEM_NAMES[*elem_idx])
        }
        RefPattern::SumOfTwo { left, right } => format!("v{left} + v{right}"),
        RefPattern::Constant => "10".to_string(),
        RefPattern::ScalarPlusBare { var_idx } => format!("scalar_const + v{var_idx}"),
        RefPattern::InlinedReducer { var_idx } => format!("1 + SUM(v{var_idx}[*])"),
        RefPattern::InlinedReducerScalarFeeder { var_idx } => {
            format!("1 + SUM(v{var_idx}[*] * scalar_const)")
        }
    }
}

/// Render the scalar `total` variable's inlined-reducer equation.
fn render_scalar_reducer_target(target: &ScalarReducerTarget) -> String {
    let j = target.var_idx;
    if target.with_scalar_feeder {
        format!("1 + SUM(v{j}[*] * scalar_const)")
    } else {
        format!("1 + SUM(v{j}[*])")
    }
}

/// Build a TestProject from a spec.
///
/// One named dimension `Dim` of size `dim_size` is created with the first
/// `dim_size` entries of `ELEM_NAMES`. Each variable `vN` is added as an
/// arrayed aux over `Dim` with the equation rendered from its spec. The
/// optional `scalar_const` aux is constant `1` when present.
fn build_project(spec: &ProjectSpec) -> TestProject {
    let elements: Vec<&str> = ELEM_NAMES.iter().take(spec.dim_size).copied().collect();
    let mut project = TestProject::new("proptest_proj").named_dimension("Dim", &elements);
    if spec.include_scalar {
        project = project.scalar_aux("scalar_const", "1");
    }
    for (i, var_spec) in spec.var_specs.iter().enumerate() {
        let name = format!("v{i}");
        let eq = render_pattern(&var_spec.pattern);
        project = project.array_aux(&format!("{name}[Dim]"), &eq);
    }
    if let Some(target) = &spec.scalar_reducer_target {
        project = project.scalar_aux("total", &render_scalar_reducer_target(target));
    }
    project
}

/// Strip the subscript suffix from an element-graph node name.
///
/// `"v0[a]"` -> `"v0"`; `"v0[a,b]"` -> `"v0"`; bare names like `"scalar_const"`
/// pass through unchanged. The parser is intentionally trivial: edge keys
/// always end in `]` if subscripted, with no `]` characters elsewhere in
/// the name.
fn strip_subscript(name: &str) -> &str {
    match name.find('[') {
        Some(idx) => &name[..idx],
        None => name,
    }
}

/// Compute the variable-level edge set from an `ElementCausalEdgesResult`
/// by stripping subscripts, deduplicating, and splicing out synthetic
/// `$⁚ltm⁚agg⁚{n}` aggregate nodes.
///
/// The variable-level graph never contains synthetic agg nodes (real
/// consumers trim them from reported loops/links), so the projection
/// collapses each agg the same way: every `from -> agg` / `agg -> to` pair
/// splices to `from -> to` -- the full cross product of the agg's sources x
/// targets, since one agg can have multiple of each (AST-identical reducer
/// texts in different variables dedupe to ONE agg node). The splice runs to
/// a fixpoint, one agg at a time, so a hypothetical `agg -> agg` chain
/// collapses transitively rather than leaking a synthetic name into the
/// result. Stripping subscripts first folds an arrayed agg's `agg[slot]`
/// nodes onto the bare agg name before the splice.
fn project_to_variable_edges(elem_edges: &ElementCausalEdgesResult) -> BTreeSet<(String, String)> {
    let mut edges = BTreeSet::new();
    for (from, targets) in &elem_edges.edges {
        let from_var = strip_subscript(from).to_string();
        for to in targets {
            let to_var = strip_subscript(to).to_string();
            edges.insert((from_var.clone(), to_var));
        }
    }
    // Not a `while let`: the scrutinee's `edges.iter()` borrow would live
    // across the body, conflicting with the `retain`/`insert` mutations; a
    // `let ... else break` statement ends the borrow before the body runs.
    #[allow(clippy::while_let_loop)]
    loop {
        let Some(agg) = edges
            .iter()
            .flat_map(|(f, t)| [f.as_str(), t.as_str()])
            .find(|n| is_synthetic_agg_name(n))
            .map(str::to_string)
        else {
            break;
        };
        let sources: Vec<String> = edges
            .iter()
            .filter(|(f, t)| *t == agg && *f != agg)
            .map(|(f, _)| f.clone())
            .collect();
        let targets: Vec<String> = edges
            .iter()
            .filter(|(f, t)| *f == agg && *t != agg)
            .map(|(_, t)| t.clone())
            .collect();
        edges.retain(|(f, t)| *f != agg && *t != agg);
        for s in &sources {
            for t in &targets {
                edges.insert((s.clone(), t.clone()));
            }
        }
    }
    edges
}

/// One inlined reducer's expected agg routing, derived from the spec:
/// every node in `sources` must feed the SAME synthetic agg node, which
/// must feed every node in `targets`; and each pair in `forbidden_direct`
/// must have NO direct element edge.
///
/// `forbidden_direct` is the FULL `sources x targets` cross product: every
/// generated inlined-reducer equation references its sources (the arrayed
/// rows AND the scalar feeder) solely inside the reducer -- each generated
/// variable has exactly one pattern, so no spec can give the same
/// `(source, target)` pair a second, non-reducer reference site -- which
/// makes every direct source->target element edge illegitimate. Forbidding
/// them closes both regression directions: the swap direction (GH #533: an
/// agg hop replaced by a direct edge) and the additive direction (correct
/// hops PLUS a spurious direct edge, which the projection invariant cannot
/// see because both graphs collapse to the same variable edge and the
/// agg-hop existence check uses subset semantics).
#[derive(Debug)]
struct ExpectedAggRouting {
    sources: Vec<String>,
    targets: Vec<String>,
    forbidden_direct: Vec<(String, String)>,
}

/// The full `sources x targets` cross product, for
/// [`ExpectedAggRouting::forbidden_direct`].
fn cross_product(sources: &[String], targets: &[String]) -> Vec<(String, String)> {
    sources
        .iter()
        .flat_map(|s| targets.iter().map(move |t| (s.clone(), t.clone())))
        .collect()
}

/// Derive every inlined reducer's expected agg routing from the spec.
/// All generated reducers are whole-extent over the single dimension, so
/// the agg is scalar (bare name): sources are every element row of the
/// reduced variable (plus `scalar_const` for the feeder forms) and targets
/// are every element of an arrayed target or the bare `total` node.
fn expected_agg_routings(spec: &ProjectSpec) -> Vec<ExpectedAggRouting> {
    let elems: Vec<&str> = ELEM_NAMES.iter().take(spec.dim_size).copied().collect();
    let source_rows =
        |j: usize| -> Vec<String> { elems.iter().map(|e| format!("v{j}[{e}]")).collect() };
    let mut routings = Vec::new();
    for (i, var_spec) in spec.var_specs.iter().enumerate() {
        if let Some((j, feeder)) = var_spec.pattern.inlined_reducer_parts() {
            let mut sources = source_rows(j);
            let targets: Vec<String> = elems.iter().map(|e| format!("v{i}[{e}]")).collect();
            if feeder {
                sources.push("scalar_const".to_string());
            }
            routings.push(ExpectedAggRouting {
                forbidden_direct: cross_product(&sources, &targets),
                sources,
                targets,
            });
        }
    }
    if let Some(target) = &spec.scalar_reducer_target {
        let mut sources = source_rows(target.var_idx);
        if target.with_scalar_feeder {
            sources.push("scalar_const".to_string());
        }
        let targets = vec!["total".to_string()];
        routings.push(ExpectedAggRouting {
            forbidden_direct: cross_product(&sources, &targets),
            sources,
            targets,
        });
    }
    routings
}

/// `true` when the raw element graph contains the edge `from -> to`.
fn has_element_edge(elem_edges: &ElementCausalEdgesResult, from: &str, to: &str) -> bool {
    elem_edges
        .edges
        .get(from)
        .is_some_and(|targets| targets.contains(to))
}

/// Verify every expected agg routing of `spec` against the raw element
/// graph: for each inlined reducer there must exist ONE synthetic agg node
/// carrying all of its `sources -> agg` and `agg -> targets` hops, and none
/// of its `forbidden_direct` edges may exist. Returns a description of the
/// first violation, for `prop_assert!` messages.
///
/// This is the assertion that makes the suite sensitive to the GH #533 bug
/// class: a fast path that swaps the `scalar_const -> agg` hop for a direct
/// `scalar_const -> total` edge preserves the projection invariant (both
/// graphs project to the same variable edge) but fails BOTH halves of this
/// check. The `forbidden_direct` half ALSO covers the additive direction --
/// correct agg hops plus a spurious direct `source -> target` edge -- which
/// the agg-hop existence half alone would miss (subset semantics).
fn check_spec_agg_expectations(
    spec: &ProjectSpec,
    elem_edges: &ElementCausalEdgesResult,
) -> Result<(), String> {
    let agg_nodes: BTreeSet<&str> = elem_edges
        .edges
        .iter()
        .flat_map(|(from, targets)| {
            std::iter::once(from.as_str()).chain(targets.iter().map(String::as_str))
        })
        .filter(|n| is_synthetic_agg_name(n))
        .collect();
    for routing in expected_agg_routings(spec) {
        let satisfied = agg_nodes.iter().any(|agg| {
            routing
                .sources
                .iter()
                .all(|s| has_element_edge(elem_edges, s, agg))
                && routing
                    .targets
                    .iter()
                    .all(|t| has_element_edge(elem_edges, agg, t))
        });
        if !satisfied {
            return Err(format!(
                "no single synthetic agg node carries all hops {:?} -> agg -> {:?}; agg nodes = {:?}",
                routing.sources, routing.targets, agg_nodes
            ));
        }
        for (from, to) in &routing.forbidden_direct {
            if has_element_edge(elem_edges, from, to) {
                return Err(format!(
                    "reducer source has a direct element edge {from} -> {to} \
                     (must route only through the agg; GH #533 bug class or a \
                     spurious additive edge)"
                ));
            }
        }
    }
    Ok(())
}

/// Flatten a `CausalEdgesResult` to the same `BTreeSet<(from, to)>` shape
/// used by the projection so set comparison is direct.
fn flatten_variable_edges(var_edges: &crate::db::CausalEdgesResult) -> BTreeSet<(String, String)> {
    let mut flat = BTreeSet::new();
    for (from, targets) in &var_edges.edges {
        for to in targets {
            flat.insert((from.clone(), to.clone()));
        }
    }
    flat
}

/// Shared per-spec pipeline + assertions for both properties: build the
/// project, sync it through salsa, then check (1) the projection invariant
/// over the agg-collapsed element graph and (2) every inlined reducer's
/// expected agg routing. Returns the raw element edges so the forced
/// property can additionally assert agg-node existence.
fn check_spec(spec: &ProjectSpec) -> Result<ElementCausalEdgesResult, TestCaseError> {
    let project = build_project(spec);
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let source_model = sync.models["main"].source;
    let source_project = sync.project;

    let var_edges = model_causal_edges(&db, source_model, source_project);
    let elem_edges = model_element_causal_edges(&db, source_model, source_project);

    let var_set = flatten_variable_edges(var_edges);
    let projected = project_to_variable_edges(elem_edges);

    prop_assert_eq!(
        &projected,
        &var_set,
        "projection mismatch: spec={:?}\nelement edges = {:?}\nvariable edges = {:?}",
        spec,
        elem_edges.edges,
        var_edges.edges
    );

    if let Err(violation) = check_spec_agg_expectations(spec, elem_edges) {
        return Err(TestCaseError::fail(format!(
            "agg routing violation: {violation}\nspec={spec:?}\nelement edges = {:?}",
            elem_edges.edges
        )));
    }

    Ok(elem_edges.clone())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// AC1.4: stripping subscripts from element-level edges, collapsing
    /// synthetic agg nodes, and deduplicating must reproduce the
    /// variable-level edge set exactly; and any inlined reducers the spec
    /// happens to contain must route through a single synthetic agg node.
    ///
    /// Each generated case builds an arrayed `TestProject`, syncs it
    /// through salsa, computes both edge views, and asserts set equality
    /// of the projection. With_cases is set to 32 because each case runs
    /// the full salsa parse / dependency / element-expansion pipeline; at
    /// 32 cases the test stays well under the 2s/test budget on debug
    /// builds while still sampling a meaningful slice of the pattern bag.
    #[test]
    fn element_edges_project_to_variable_edges(spec in project_spec_strategy()) {
        check_spec(&spec)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// GH #739 non-vacuity guard: over specs FORCED to contain at least one
    /// inlined reducer, the projection invariant and the per-reducer agg
    /// routing must hold, and -- structurally, for EVERY case -- a synthetic
    /// `$⁚ltm⁚agg⁚{n}` node must exist in the element graph. The base
    /// property covers reducers only when its strategy happens to draw one;
    /// this property makes ThroughAgg coverage impossible to lose silently.
    ///
    /// 16 cases (vs the base property's 32) keeps the module's total
    /// runtime in the same budget class while still sampling all four
    /// [`ForcedReducerKind`] shapes with high probability.
    #[test]
    fn forced_reducer_specs_route_through_aggs(spec in forced_reducer_spec_strategy()) {
        let routings = expected_agg_routings(&spec);
        prop_assert!(
            !routings.is_empty(),
            "forced strategy must inject at least one inlined reducer: spec={:?}",
            spec
        );

        let elem_edges = check_spec(&spec)?;

        let has_agg_node = elem_edges.edges.iter().any(|(from, targets)| {
            is_synthetic_agg_name(from) || targets.iter().any(|t| is_synthetic_agg_name(t))
        });
        prop_assert!(
            has_agg_node,
            "forced-reducer spec minted no synthetic agg node: spec={:?}\nelement edges = {:?}",
            spec,
            elem_edges.edges
        );
    }
}

/// Deterministic companion to the properties (GH #739): a fixed spec
/// containing every inlined-reducer pattern must mint synthetic agg nodes
/// and route the expected hops. Pins, concretely and seed-independently:
///
/// - arrayed-source rows -> agg -> arrayed target (`v1 = 1 + SUM(v0[*])`);
/// - the scalar feeder hop `scalar_const -> agg` for an arrayed target
///   (`v2 = 1 + SUM(v0[*] * scalar_const)`);
/// - the GH #533 both-scalar arm: `scalar_const -> agg -> total` with NO
///   direct `scalar_const -> total` edge
///   (`total = 1 + SUM(v1[*] * scalar_const)`);
/// - the agg-collapsed projection invariant over the same model.
///
/// The two scalar-feeder reducers reduce DIFFERENT sources (`v0` vs `v1`)
/// so their reducer texts differ and they mint distinct agg nodes (AST-
/// identical texts would dedupe to one).
#[test]
fn inlined_reducer_specs_route_through_synthetic_aggs() {
    let spec = ProjectSpec {
        dim_size: 2,
        var_specs: vec![
            ArrayedVarSpec {
                pattern: RefPattern::Constant,
            },
            ArrayedVarSpec {
                pattern: RefPattern::InlinedReducer { var_idx: 0 },
            },
            ArrayedVarSpec {
                pattern: RefPattern::InlinedReducerScalarFeeder { var_idx: 0 },
            },
        ],
        include_scalar: true,
        scalar_reducer_target: Some(ScalarReducerTarget {
            var_idx: 1,
            with_scalar_feeder: true,
        }),
    };

    let project = build_project(&spec);
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let source_model = sync.models["main"].source;
    let source_project = sync.project;

    let var_edges = model_causal_edges(&db, source_model, source_project);
    let elem_edges = model_element_causal_edges(&db, source_model, source_project);

    // The spec derives three expected routings (v1's, v2's, total's); all
    // must be carried by synthetic agg nodes, with no direct feeder edges.
    let routings = expected_agg_routings(&spec);
    assert_eq!(routings.len(), 3, "spec must derive all three routings");
    check_spec_agg_expectations(&spec, elem_edges)
        .unwrap_or_else(|violation| panic!("{violation}\nelement edges = {:?}", elem_edges.edges));

    // Belt-and-braces: the raw element graph really contains synthetic agg
    // nodes (the routing check above would also fail on an empty agg set,
    // but assert it directly so a future refactor of the checker can't
    // accidentally weaken this).
    assert!(
        elem_edges.edges.iter().any(|(from, targets)| {
            is_synthetic_agg_name(from) || targets.iter().any(|t| is_synthetic_agg_name(t))
        }),
        "fixed reducer spec minted no synthetic agg node: {:?}",
        elem_edges.edges
    );

    // And the agg-collapsed projection matches the variable-level edges.
    assert_eq!(
        project_to_variable_edges(elem_edges),
        flatten_variable_edges(var_edges),
        "projection mismatch on the fixed reducer spec"
    );
}
