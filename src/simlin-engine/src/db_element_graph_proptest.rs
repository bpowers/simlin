// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Property tests for the variable-level <-> element-level causal graph
//! projection invariant (AC1.4).
//!
//! The element graph is meant to be a *finer* view of the variable graph:
//! every element-level edge `from[d] -> to[e]` projects to a variable-level
//! edge `from -> to` after stripping subscripts and deduplicating. The
//! variable-level edge set must equal that projection -- if the element
//! builder ever drops a class of edges or invents one without a variable-
//! level counterpart, this property fails.
//!
//! This file is a Day-1 regression guard: it must pass on the existing
//! collapsing classifier today and continue to pass after the Phase 2
//! AST-walking refactor lands. It deliberately does NOT exercise the
//! over-expansion bug that AC1.1 / AC1.3 exist to catch (over-expansion
//! preserves the projection invariant by construction); see those tests
//! for fixed-index per-reference truthfulness.

use proptest::prelude::*;
use std::collections::BTreeSet;

use crate::db::{SimlinDb, model_causal_edges, model_element_causal_edges, sync_from_datamodel};
use crate::test_common::TestProject;

/// One reference pattern in a generated equation. `var_idx` is the index
/// of the source variable to reference (always strictly less than the
/// referencing variable's own index, to keep the dependency DAG acyclic).
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
}

/// Hand-crafted spec for one generated arrayed variable: which patterns it
/// uses for its equation. The `dim_idx` field is unused today (single-
/// dimension models only) but reserved for future multi-dim extension.
#[derive(Debug, Clone)]
struct ArrayedVarSpec {
    pattern: RefPattern,
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
    ];
    if include_scalar {
        variants.push(
            (0..max_src)
                .prop_map(|j| RefPattern::ScalarPlusBare { var_idx: j })
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
            pattern_strategies.prop_map(move |patterns| ProjectSpec {
                dim_size,
                var_specs: patterns
                    .into_iter()
                    .map(|pattern| ArrayedVarSpec { pattern })
                    .collect(),
                include_scalar,
            })
        },
    )
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
/// by stripping subscripts and deduplicating.
fn project_to_variable_edges(
    elem_edges: &crate::db::ElementCausalEdgesResult,
) -> BTreeSet<(String, String)> {
    let mut projected = BTreeSet::new();
    for (from, targets) in &elem_edges.edges {
        let from_var = strip_subscript(from).to_string();
        for to in targets {
            let to_var = strip_subscript(to).to_string();
            projected.insert((from_var.clone(), to_var));
        }
    }
    projected
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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// AC1.4: stripping subscripts from element-level edges and
    /// deduplicating must reproduce the variable-level edge set exactly.
    ///
    /// Each generated case builds an arrayed `TestProject`, syncs it
    /// through salsa, computes both edge views, and asserts set equality
    /// of the projection. With_cases is set to 32 because each case runs
    /// the full salsa parse / dependency / element-expansion pipeline; at
    /// 32 cases the test stays well under the 2s/test budget on debug
    /// builds while still sampling a meaningful slice of the pattern bag.
    #[test]
    fn element_edges_project_to_variable_edges(spec in project_spec_strategy()) {
        let project = build_project(&spec);
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
    }
}
