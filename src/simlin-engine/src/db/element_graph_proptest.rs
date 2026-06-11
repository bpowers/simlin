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
//! Two further shape families ride the same conventions (the 2026-06-11
//! shape-expressiveness design): an iterated+literal MIXED subscript
//! (`wide[Dim, young]` inside an A2A-over-`Dim` equation -- the GH #525
//! `PerElement` family, including the BROADCAST case where the target also
//! iterates `Age`) and a SUBSET reducer (`SUM(v[*:Sub])`, a StarRange over
//! a proper subdimension -- GH #766). Their expected edges derive from
//! `read_slice_rows`, the design's single row-derivation source of truth
//! (invariant I4), and each family has its own forced strategy + property
//! (`forced_mixed_ref_specs_expand_to_pinned_diagonal`,
//! `forced_subset_reducer_specs_route_subset_rows`) so the GH #739 vacuity
//! guard applies: a strategy that silently stops generating the shape fails
//! the forced property's spec-level guard instead of quietly shrinking the
//! sampled corpus. Deterministic companions hand-pin literal edge names so
//! a `read_slice_rows` regression cannot mask itself inside the derived
//! property expectations.
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
    /// `1 + SUM(vN[*:Sub])` -- an INLINED reducer whose StarRange names the
    /// proper subdimension `Sub` (the first `subset_size` elements of `Dim`,
    /// GH #766): the hoisted agg's `Reduced` axis carries the subset, so
    /// only the subset rows feed the synthetic agg node; the unread rows
    /// get NO edge into it (and no direct edge to the target either). Only
    /// emitted when `subset_size >= 1` (i.e. `dim_size >= 2` -- a
    /// single-element dimension has no proper subdimension).
    InlinedSubsetReducer { var_idx: usize },
}

/// Decomposed inlined-reducer pattern: which source it reduces, whether a
/// scalar feeder is multiplied in, and whether the StarRange reads only the
/// proper subdimension `Sub` (GH #766).
struct InlinedReducerShape {
    var_idx: usize,
    feeder: bool,
    subset: bool,
}

impl RefPattern {
    /// `Some(shape)` when the pattern is an inlined (synthetic-agg-minting)
    /// reducer.
    fn inlined_reducer_parts(&self) -> Option<InlinedReducerShape> {
        match self {
            RefPattern::InlinedReducer { var_idx } => Some(InlinedReducerShape {
                var_idx: *var_idx,
                feeder: false,
                subset: false,
            }),
            RefPattern::InlinedReducerScalarFeeder { var_idx } => Some(InlinedReducerShape {
                var_idx: *var_idx,
                feeder: true,
                subset: false,
            }),
            RefPattern::InlinedSubsetReducer { var_idx } => Some(InlinedReducerShape {
                var_idx: *var_idx,
                feeder: false,
                subset: true,
            }),
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
    /// `SUM(v{j}[*:Sub])` instead of `SUM(v{j}[*])` (GH #766): only the
    /// `Sub` rows feed the agg. Only set when the spec's `subset_size >= 1`.
    with_subset: bool,
}

/// Optional trailing pair of variables exercising the GH #525 `PerElement`
/// family: a 2-D source `wide[Dim, Age] = v{var_idx}[Dim]` (a Bare
/// reference broadcast over `Age`) plus a target whose equation references
/// `wide` through a MIXED iterated+literal subscript, `wide[Dim, {age}]`.
/// With `broadcast` false the target is `mixed[Dim]` (the Iterated dims
/// equal the target's -- rows and slots 1:1); with `broadcast` true it is
/// `mixed[Dim, Age]` (the Iterated dims a strict SUBSET of the target's, so
/// each pinned row feeds every `Age` element of its `Dim` slot). Appended
/// last and referenced by nothing, so acyclicity is preserved.
#[derive(Debug, Clone)]
struct MixedRefTarget {
    var_idx: usize,
    /// Index into [`AGE_ELEM_NAMES`]: which `Age` element the reference pins.
    age_idx: usize,
    broadcast: bool,
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
    /// Number of elements in the proper subdimension `Sub` (the first
    /// `subset_size` entries of `Dim`'s elements). `0` when `dim_size == 1`
    /// (no proper subdimension exists); otherwise `1 <= subset_size <
    /// dim_size`, so `Sub` is always PROPER -- a same-cardinality "sub"
    /// would normalize back to the full extent (`AxisRead` invariant I3)
    /// and make the subset routing expectations wrong. Subset patterns are
    /// only generated when `subset_size >= 1`, and the `Sub` dimension is
    /// only declared when one of them is present.
    subset_size: usize,
    /// Optional GH #525 mixed iterated+literal reference block (see
    /// [`MixedRefTarget`]).
    mixed_ref_target: Option<MixedRefTarget>,
}

impl ProjectSpec {
    /// `true` when any equation embeds an inlined (synthetic-agg-minting)
    /// reducer -- the spec-level vacuity guard for the forced-reducer
    /// property (equivalent to `expected_agg_routings` being non-empty:
    /// that function derives exactly one routing per such reducer).
    fn has_inlined_reducer(&self) -> bool {
        self.scalar_reducer_target.is_some()
            || self
                .var_specs
                .iter()
                .any(|v| v.pattern.inlined_reducer_parts().is_some())
    }

    /// `true` when any inlined reducer reads through the proper
    /// subdimension `Sub` (GH #766) -- the spec-level vacuity guard for the
    /// forced-subset property.
    fn has_subset_reducer(&self) -> bool {
        self.var_specs
            .iter()
            .any(|v| v.pattern.inlined_reducer_parts().is_some_and(|s| s.subset))
            || self
                .scalar_reducer_target
                .as_ref()
                .is_some_and(|t| t.with_subset)
    }
}

/// Element names for the synthetic dimension. The generator tops out at
/// dim_size = 5, so we only need names through `e4`. Lowercase matches
/// the canonical form the salsa pipeline uses for edge keys.
const ELEM_NAMES: &[&str] = &["a", "b", "c", "d", "e"];

/// Element names for the fixed second dimension `Age` declared by the
/// GH #525 mixed-reference block. Disjoint from [`ELEM_NAMES`] so a pinned
/// `Age` literal can never be confused with a `Dim` element.
const AGE_ELEM_NAMES: &[&str] = &["young", "old"];

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
    has_subset: bool,
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
    if has_subset {
        variants.push(
            (0..max_src)
                .prop_map(|j| RefPattern::InlinedSubsetReducer { var_idx: j })
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
            // A proper subdimension needs at least two parent elements;
            // `subset_size == 0` marks "unavailable" for dim_size == 1.
            let subset_strategy: BoxedStrategy<usize> = if dim_size >= 2 {
                (1..dim_size).boxed()
            } else {
                Just(0usize).boxed()
            };
            subset_strategy.prop_flat_map(move |subset_size| {
                // Build the per-variable pattern strategies with the
                // dim_size/include_scalar/subset context closed over.
                let pattern_strategies: Vec<BoxedStrategy<RefPattern>> = (0..var_count)
                    .map(|i| pattern_strategy(i, dim_size, include_scalar, subset_size >= 1))
                    .collect();
                // Optional scalar reducer target: roughly a third of specs
                // get one, sourcing from any arrayed var (it is appended
                // last and referenced by nothing, so acyclicity is
                // preserved). The scalar-feeder form requires the scalar
                // variable to exist; the subset form a proper subdimension.
                let scalar_target_strategy =
                    proptest::option::weighted(0.34, (0..var_count, any::<bool>(), any::<bool>()));
                // Optional GH #525 mixed-reference block, same one-third
                // weighting (appended last, referenced by nothing).
                let mixed_strategy = proptest::option::weighted(
                    0.34,
                    (0..var_count, 0..AGE_ELEM_NAMES.len(), any::<bool>()),
                );
                (pattern_strategies, scalar_target_strategy, mixed_strategy).prop_map(
                    move |(patterns, scalar_target, mixed)| ProjectSpec {
                        dim_size,
                        var_specs: patterns
                            .into_iter()
                            .map(|pattern| ArrayedVarSpec { pattern })
                            .collect(),
                        include_scalar,
                        scalar_reducer_target: scalar_target.map(|(var_idx, feeder, subset)| {
                            ScalarReducerTarget {
                                var_idx,
                                with_scalar_feeder: feeder && include_scalar,
                                with_subset: subset && subset_size >= 1,
                            }
                        }),
                        subset_size,
                        mixed_ref_target: mixed.map(|(var_idx, age_idx, broadcast)| {
                            MixedRefTarget {
                                var_idx,
                                age_idx,
                                broadcast,
                            }
                        }),
                    },
                )
            })
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
                        with_subset: false,
                    });
                }
            }
            spec
        })
}

/// Which subset-reducer shape `forced_subset_reducer_spec_strategy`
/// injects: the arrayed-target pattern, the scalar `total` target, or the
/// scalar target with the feeder multiplied INSIDE the subset slice
/// (`1 + SUM(v{j}[*:Sub] * scalar_const)` -- the feeder-acceptance and
/// subset interaction in one shape).
#[derive(Debug, Clone, Copy)]
enum ForcedSubsetKind {
    ArrayedTarget,
    ScalarTarget,
    ScalarTargetScalarFeeder,
}

/// Strategy: a [`ProjectSpec`] GUARANTEED to contain at least one subset
/// (`*:Sub`) reducer (GH #766) -- the structural non-vacuity guard for the
/// subset family (GH #739): the base strategy only *sometimes* draws the
/// pattern, so the subset property runs over this strategy and asserts
/// `has_subset_reducer()` for every case.
fn forced_subset_reducer_spec_strategy() -> impl Strategy<Value = ProjectSpec> {
    let kind_strategy = prop_oneof![
        Just(ForcedSubsetKind::ArrayedTarget),
        Just(ForcedSubsetKind::ScalarTarget),
        Just(ForcedSubsetKind::ScalarTargetScalarFeeder),
    ];
    (
        project_spec_strategy(),
        kind_strategy,
        any::<prop::sample::Index>(),
        any::<prop::sample::Index>(),
    )
        .prop_map(|(mut spec, kind, slot_idx, src_idx)| {
            if spec.subset_size == 0 {
                // dim_size 1 has no proper subdimension; widening the dim
                // keeps every already-generated pattern valid (`FixedIndex`
                // element indices only gain headroom).
                spec.dim_size = 2;
                spec.subset_size = 1;
            }
            let n = spec.var_specs.len(); // >= 2 by construction
            match kind {
                ForcedSubsetKind::ArrayedTarget => {
                    // Overwrite one arrayed var's pattern (index >= 1 so a
                    // strictly-earlier source exists; acyclicity preserved).
                    let i = 1 + slot_idx.index(n - 1);
                    let j = src_idx.index(i);
                    spec.var_specs[i].pattern = RefPattern::InlinedSubsetReducer { var_idx: j };
                }
                ForcedSubsetKind::ScalarTarget | ForcedSubsetKind::ScalarTargetScalarFeeder => {
                    let feeder = matches!(kind, ForcedSubsetKind::ScalarTargetScalarFeeder);
                    if feeder {
                        spec.include_scalar = true;
                    }
                    spec.scalar_reducer_target = Some(ScalarReducerTarget {
                        var_idx: src_idx.index(n),
                        with_scalar_feeder: feeder,
                        with_subset: true,
                    });
                }
            }
            spec
        })
}

/// Strategy: a [`ProjectSpec`] GUARANTEED to contain the GH #525
/// mixed-reference block (both the 1:1 and the broadcast target shapes,
/// via the injected `broadcast` flag) -- the structural non-vacuity guard
/// for the `PerElement` family (GH #739).
fn forced_mixed_ref_spec_strategy() -> impl Strategy<Value = ProjectSpec> {
    (
        project_spec_strategy(),
        any::<prop::sample::Index>(),
        any::<prop::sample::Index>(),
        any::<bool>(),
    )
        .prop_map(|(mut spec, src_idx, age_idx, broadcast)| {
            let n = spec.var_specs.len();
            spec.mixed_ref_target = Some(MixedRefTarget {
                var_idx: src_idx.index(n),
                age_idx: age_idx.index(AGE_ELEM_NAMES.len()),
                broadcast,
            });
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
        RefPattern::InlinedSubsetReducer { var_idx } => format!("1 + SUM(v{var_idx}[*:Sub])"),
    }
}

/// Render the scalar `total` variable's inlined-reducer equation.
fn render_scalar_reducer_target(target: &ScalarReducerTarget) -> String {
    let j = target.var_idx;
    let slice = if target.with_subset {
        format!("v{j}[*:Sub]")
    } else {
        format!("v{j}[*]")
    };
    if target.with_scalar_feeder {
        format!("1 + SUM({slice} * scalar_const)")
    } else {
        format!("1 + SUM({slice})")
    }
}

/// Build a TestProject from a spec.
///
/// One named dimension `Dim` of size `dim_size` is created with the first
/// `dim_size` entries of `ELEM_NAMES`. Each variable `vN` is added as an
/// arrayed aux over `Dim` with the equation rendered from its spec. The
/// optional `scalar_const` aux is constant `1` when present. When the spec
/// contains a subset reducer, the proper subdimension `Sub` (the first
/// `subset_size` elements of `Dim` -- a NAMED subdimension by element
/// containment, like the GH #766 fixtures' `Core ⊂ Region`) is declared;
/// when it contains the GH #525 mixed block, the `Age` dimension plus the
/// `wide`/`mixed` pair are appended last (so specs without the new shapes
/// build byte-identical projects to the pre-extension strategy).
fn build_project(spec: &ProjectSpec) -> TestProject {
    let elements: Vec<&str> = ELEM_NAMES.iter().take(spec.dim_size).copied().collect();
    let mut project = TestProject::new("proptest_proj").named_dimension("Dim", &elements);
    if spec.has_subset_reducer() {
        let sub_elements: Vec<&str> = ELEM_NAMES.iter().take(spec.subset_size).copied().collect();
        project = project.named_dimension("Sub", &sub_elements);
    }
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
    if let Some(mixed) = &spec.mixed_ref_target {
        project = project
            .named_dimension("Age", AGE_ELEM_NAMES)
            .array_aux("wide[Dim,Age]", &format!("v{}[Dim]", mixed.var_idx));
        let decl = if mixed.broadcast {
            "mixed[Dim,Age]"
        } else {
            "mixed[Dim]"
        };
        project = project.array_aux(
            decl,
            &format!("wide[Dim, {}] * 2", AGE_ELEM_NAMES[mixed.age_idx]),
        );
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
    /// Source rows that must NOT feed the matched agg node: a subset
    /// reducer's unread rows (`v{j}`'s elements outside `Sub`, GH #766).
    /// Empty for whole-extent reducers. The existence check requires ONE
    /// agg satisfying all three conditions (sources, targets, and no
    /// forbidden source), so a spurious unread-row edge on the true agg
    /// cannot be excused by some other node: no other agg carries the
    /// `agg -> targets` hops (each target is fed only by its own
    /// equation's reducers, and AST-identical texts dedupe to one node
    /// with one identical subset).
    forbidden_sources: Vec<String>,
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
/// Every generated reducer collapses its single `Dim` axis, so the agg is
/// scalar (bare name): sources are the element rows the reducer READS
/// (every row for the whole-extent forms, only the `Sub` rows for the
/// subset forms -- GH #766; plus `scalar_const` for the feeder forms) and
/// targets are every element of an arrayed target or the bare `total`
/// node -- the agg's full fan-out, since `result_dims` is empty for an
/// all-`Reduced` slice.
///
/// The read rows come from the SAME `read_slice_rows` derivation
/// production consumes (the shape-expressiveness design's invariant I4 --
/// the single row-derivation source of truth), with the per-axis access
/// stated independently from the spec (`Reduced{subset}`). The
/// deterministic companion tests hand-pin literal edge names so a
/// `read_slice_rows` regression cannot hide inside this shared derivation.
fn expected_agg_routings(
    spec: &ProjectSpec,
    dim_ctx: &crate::dimensions::DimensionsContext,
) -> Vec<ExpectedAggRouting> {
    use crate::ltm_agg::AxisRead;
    let elems: Vec<String> = ELEM_NAMES
        .iter()
        .take(spec.dim_size)
        .map(|e| e.to_string())
        .collect();
    let sub_elems: Vec<String> = ELEM_NAMES
        .iter()
        .take(spec.subset_size)
        .map(|e| e.to_string())
        .collect();
    let read_rows = |j: usize, subset: bool| -> Vec<String> {
        let axes = [AxisRead::Reduced {
            subset: if subset {
                Some(sub_elems.clone())
            } else {
                None
            },
        }];
        let rows = crate::db::ltm::read_slice_rows(&axes, std::slice::from_ref(&elems), dim_ctx)
            .expect("a Reduced-only slice over a declared dimension always yields rows");
        rows.iter().map(|r| format!("v{j}[{}]", r.row)).collect()
    };
    let unread_rows = |j: usize, subset: bool| -> Vec<String> {
        if !subset {
            return Vec::new();
        }
        elems
            .iter()
            .filter(|e| !sub_elems.contains(e))
            .map(|e| format!("v{j}[{e}]"))
            .collect()
    };
    let mut routings = Vec::new();
    let mut push_routing = |j: usize, feeder: bool, subset: bool, targets: Vec<String>| {
        let mut sources = read_rows(j, subset);
        let forbidden_sources = unread_rows(j, subset);
        if feeder {
            sources.push("scalar_const".to_string());
        }
        // An unread row may not bypass the agg either: include it in the
        // forbidden direct-source pool.
        let direct_pool: Vec<String> = sources
            .iter()
            .chain(forbidden_sources.iter())
            .cloned()
            .collect();
        routings.push(ExpectedAggRouting {
            forbidden_direct: cross_product(&direct_pool, &targets),
            forbidden_sources,
            sources,
            targets,
        });
    };
    for (i, var_spec) in spec.var_specs.iter().enumerate() {
        if let Some(shape) = var_spec.pattern.inlined_reducer_parts() {
            let targets: Vec<String> = elems.iter().map(|e| format!("v{i}[{e}]")).collect();
            push_routing(shape.var_idx, shape.feeder, shape.subset, targets);
        }
    }
    if let Some(target) = &spec.scalar_reducer_target {
        push_routing(
            target.var_idx,
            target.with_scalar_feeder,
            target.with_subset,
            vec!["total".to_string()],
        );
    }
    routings
}

/// The exact `wide -> mixed` element-edge set the GH #525 block must
/// produce: rows from `read_slice_rows` over the mixed reference's axes
/// (`[Iterated(Dim), Pinned(age)]` -- the same invariant-I4 derivation the
/// `PerElement` expansion arm consumes), each row feeding the target
/// element(s) whose `Dim` coordinate is the row's slot -- exactly
/// `mixed[{slot}]` for the 1:1 case, every `Age` element of
/// `mixed[{slot},_]` for the broadcast case (the Iterated dims a strict
/// subset of the target's). Empty when the spec has no mixed block.
fn expected_mixed_edges(
    spec: &ProjectSpec,
    dim_ctx: &crate::dimensions::DimensionsContext,
) -> BTreeSet<(String, String)> {
    use crate::ltm_agg::AxisRead;
    let Some(mixed) = &spec.mixed_ref_target else {
        return BTreeSet::new();
    };
    let dim_elems: Vec<String> = ELEM_NAMES
        .iter()
        .take(spec.dim_size)
        .map(|e| e.to_string())
        .collect();
    let age_elems: Vec<String> = AGE_ELEM_NAMES.iter().map(|e| e.to_string()).collect();
    let axes = [
        AxisRead::Iterated {
            dim: "dim".to_string(),
            source_dim: "dim".to_string(),
        },
        AxisRead::Pinned(AGE_ELEM_NAMES[mixed.age_idx].to_string()),
    ];
    let rows = crate::db::ltm::read_slice_rows(&axes, &[dim_elems, age_elems.clone()], dim_ctx)
        .expect("an Iterated+Pinned slice over declared dimensions always yields rows");
    let mut edges = BTreeSet::new();
    for r in &rows {
        let from = format!("wide[{}]", r.row);
        if mixed.broadcast {
            for age in &age_elems {
                edges.insert((from.clone(), format!("mixed[{},{}]", r.slot, age)));
            }
        } else {
            edges.insert((from.clone(), format!("mixed[{}]", r.slot)));
        }
    }
    edges
}

/// Every element edge whose endpoints strip to the `(from_var, to_var)`
/// variable pair -- the actual-side counterpart of `expected_mixed_edges`,
/// asserted EQUAL so both failure directions (missing diagonal rows, extra
/// cross-product rows) are caught.
fn edges_between_vars(
    elem_edges: &ElementCausalEdgesResult,
    from_var: &str,
    to_var: &str,
) -> BTreeSet<(String, String)> {
    let mut found = BTreeSet::new();
    for (from, targets) in &elem_edges.edges {
        if strip_subscript(from) != from_var {
            continue;
        }
        for to in targets {
            if strip_subscript(to) == to_var {
                found.insert((from.clone(), to.clone()));
            }
        }
    }
    found
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
/// carrying all of its `sources -> agg` and `agg -> targets` hops while
/// carrying NONE of its `forbidden_sources -> agg` hops (a subset reducer's
/// unread rows, GH #766), and none of its `forbidden_direct` edges may
/// exist. Returns a description of the first violation, for `prop_assert!`
/// messages.
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
    dim_ctx: &crate::dimensions::DimensionsContext,
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
    for routing in expected_agg_routings(spec, dim_ctx) {
        let satisfied = agg_nodes.iter().any(|agg| {
            routing
                .sources
                .iter()
                .all(|s| has_element_edge(elem_edges, s, agg))
                && routing
                    .targets
                    .iter()
                    .all(|t| has_element_edge(elem_edges, agg, t))
                && routing
                    .forbidden_sources
                    .iter()
                    .all(|s| !has_element_edge(elem_edges, s, agg))
        });
        if !satisfied {
            return Err(format!(
                "no single synthetic agg node carries all hops {:?} -> agg -> {:?} \
                 while excluding the unread rows {:?} (GH #766); agg nodes = {:?}",
                routing.sources, routing.targets, routing.forbidden_sources, agg_nodes
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

/// Shared per-spec pipeline + assertions for all the properties: build the
/// project, sync it through salsa, then check (1) the projection invariant
/// over the agg-collapsed element graph, (2) every inlined reducer's
/// expected agg routing (subset-aware, GH #766), and (3) the GH #525 mixed
/// block's exact pinned-diagonal edge set. Returns the raw element edges so
/// the forced properties can additionally assert structural existence.
fn check_spec(spec: &ProjectSpec) -> Result<ElementCausalEdgesResult, TestCaseError> {
    let project = build_project(spec);
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let source_model = sync.models["main"].source;
    let source_project = sync.project;

    let var_edges = model_causal_edges(&db, source_model, source_project);
    let elem_edges = model_element_causal_edges(&db, source_model, source_project);
    let dim_ctx = crate::db::project_dimensions_context(&db, source_project);

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

    if let Err(violation) = check_spec_agg_expectations(spec, dim_ctx, elem_edges) {
        return Err(TestCaseError::fail(format!(
            "agg routing violation: {violation}\nspec={spec:?}\nelement edges = {:?}",
            elem_edges.edges
        )));
    }

    // GH #525: the mixed block's element edges must be EXACTLY the
    // diagonal-with-pinned-axes rows -- never the cross-product, never a
    // missing row. Both sets are empty when the spec has no mixed block.
    let expected_mixed = expected_mixed_edges(spec, dim_ctx);
    let actual_mixed = edges_between_vars(elem_edges, "wide", "mixed");
    prop_assert_eq!(
        &actual_mixed,
        &expected_mixed,
        "mixed-reference edges are not the pinned diagonal: spec={:?}\nelement edges = {:?}",
        spec,
        elem_edges.edges
    );

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
        prop_assert!(
            spec.has_inlined_reducer(),
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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// GH #525 / GH #739 non-vacuity guard: over specs FORCED to contain
    /// the mixed iterated+literal reference block, the `wide -> mixed`
    /// element edges must be exactly the diagonal-with-pinned-axes rows
    /// (asserted inside `check_spec` against the `read_slice_rows`
    /// derivation -- covering both the 1:1 and the broadcast target
    /// shapes), and the block must structurally EXIST: in the spec (a
    /// stubbed-out injection fails the first assertion) and in the produced
    /// element graph (equations that silently stop expanding to pinned
    /// rows fail the second), so the `PerElement` coverage can never
    /// silently go vacuous.
    #[test]
    fn forced_mixed_ref_specs_expand_to_pinned_diagonal(
        spec in forced_mixed_ref_spec_strategy()
    ) {
        prop_assert!(
            spec.mixed_ref_target.is_some(),
            "forced strategy must inject the mixed-reference block: spec={:?}",
            spec
        );

        let elem_edges = check_spec(&spec)?;

        let actual_mixed = edges_between_vars(&elem_edges, "wide", "mixed");
        prop_assert!(
            !actual_mixed.is_empty(),
            "forced mixed spec produced no wide -> mixed element edges: spec={:?}\nelement edges = {:?}",
            spec,
            elem_edges.edges
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    /// GH #766 / GH #739 non-vacuity guard: over specs FORCED to contain a
    /// subset (`*:Sub`) reducer, the agg routing checks must hold -- only
    /// the subset rows feed the agg (the routing's `forbidden_sources`
    /// excludes the unread rows from the SAME matched node) and the agg
    /// fans out to every target element -- and a synthetic agg node must
    /// structurally exist, so the subset coverage can never silently go
    /// vacuous. A stubbed-out injection fails the spec-level guard.
    #[test]
    fn forced_subset_reducer_specs_route_subset_rows(
        spec in forced_subset_reducer_spec_strategy()
    ) {
        prop_assert!(
            spec.has_subset_reducer(),
            "forced strategy must inject at least one subset reducer: spec={:?}",
            spec
        );

        let elem_edges = check_spec(&spec)?;

        let has_agg_node = elem_edges.edges.iter().any(|(from, targets)| {
            is_synthetic_agg_name(from) || targets.iter().any(|t| is_synthetic_agg_name(t))
        });
        prop_assert!(
            has_agg_node,
            "forced-subset spec minted no synthetic agg node: spec={:?}\nelement edges = {:?}",
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
            with_subset: false,
        }),
        subset_size: 0,
        mixed_ref_target: None,
    };

    let project = build_project(&spec);
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let source_model = sync.models["main"].source;
    let source_project = sync.project;

    let var_edges = model_causal_edges(&db, source_model, source_project);
    let elem_edges = model_element_causal_edges(&db, source_model, source_project);
    let dim_ctx = crate::db::project_dimensions_context(&db, source_project);

    // The spec derives three expected routings (v1's, v2's, total's); all
    // must be carried by synthetic agg nodes, with no direct feeder edges.
    let routings = expected_agg_routings(&spec, dim_ctx);
    assert_eq!(routings.len(), 3, "spec must derive all three routings");
    check_spec_agg_expectations(&spec, dim_ctx, elem_edges)
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

/// Deterministic-companion pipeline helper: build the spec's project, sync
/// it through salsa, and return the flattened variable-level edge set plus
/// the raw element edges.
fn edges_for_spec(spec: &ProjectSpec) -> (BTreeSet<(String, String)>, ElementCausalEdgesResult) {
    let project = build_project(spec);
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let source_model = sync.models["main"].source;
    let source_project = sync.project;
    let var_edges = model_causal_edges(&db, source_model, source_project);
    let elem_edges = model_element_causal_edges(&db, source_model, source_project);
    (flatten_variable_edges(var_edges), elem_edges.clone())
}

/// Deterministic GH #766 companion: a fixed spec containing both subset-
/// reducer forms (arrayed target `v1 = 1 + SUM(v0[*:Sub])`; scalar target
/// `total = 1 + SUM(v0[*:Sub] * scalar_const)`, the feeder interaction)
/// over `Dim = {a, b, c}` with `Sub = {a, b}` must route ONLY the `Sub`
/// rows through the aggs. Edge names are hand-pinned literals --
/// deliberately independent of `read_slice_rows`, so a regression in the
/// shared derivation cannot mask itself inside the property expectations
/// that DO derive from it.
#[test]
fn subset_reducer_specs_route_only_subset_rows() {
    let spec = ProjectSpec {
        dim_size: 3,
        var_specs: vec![
            ArrayedVarSpec {
                pattern: RefPattern::Constant,
            },
            ArrayedVarSpec {
                pattern: RefPattern::InlinedSubsetReducer { var_idx: 0 },
            },
        ],
        include_scalar: true,
        scalar_reducer_target: Some(ScalarReducerTarget {
            var_idx: 0,
            with_scalar_feeder: true,
            with_subset: true,
        }),
        subset_size: 2,
        mixed_ref_target: None,
    };
    let (var_set, elem_edges) = edges_for_spec(&spec);

    // The unread row v0[c] (outside Sub = {a, b}) feeds NOTHING: not the
    // aggs, not the targets.
    assert!(
        elem_edges.edges.get("v0[c]").is_none_or(|t| t.is_empty()),
        "unread row v0[c] must have no outgoing edges: {:?}",
        elem_edges.edges
    );

    let agg_feeding = |target: &str| -> String {
        elem_edges
            .edges
            .iter()
            .find_map(|(from, targets)| {
                (is_synthetic_agg_name(from) && targets.contains(target)).then(|| from.clone())
            })
            .unwrap_or_else(|| panic!("no synthetic agg feeds {target}: {:?}", elem_edges.edges))
    };
    let incoming = |node: &str| -> BTreeSet<String> {
        elem_edges
            .edges
            .iter()
            .filter(|(_, targets)| targets.contains(node))
            .map(|(from, _)| from.clone())
            .collect()
    };
    let outgoing = |node: &str| -> BTreeSet<String> {
        elem_edges.edges.get(node).cloned().unwrap_or_default()
    };
    let set =
        |names: &[&str]| -> BTreeSet<String> { names.iter().map(|s| s.to_string()).collect() };

    // v1's agg: exactly the Sub rows feed it, and -- the fan-out matching
    // `result_dims` (empty for the all-Reduced slice, so a scalar agg
    // broadcast) -- it feeds every v1 element.
    let v1_agg = agg_feeding("v1[a]");
    assert_eq!(
        incoming(&v1_agg),
        set(&["v0[a]", "v0[b]"]),
        "exactly the Sub rows must feed v1's agg"
    );
    assert_eq!(
        outgoing(&v1_agg),
        set(&["v1[a]", "v1[b]", "v1[c]"]),
        "v1's scalar agg must broadcast to every target element"
    );

    // total's agg: the Sub rows plus the scalar feeder.
    let total_agg = agg_feeding("total");
    assert_ne!(
        v1_agg, total_agg,
        "distinct reducer texts mint distinct aggs"
    );
    assert_eq!(
        incoming(&total_agg),
        set(&["scalar_const", "v0[a]", "v0[b]"]),
        "exactly the Sub rows + the scalar feeder must feed total's agg"
    );
    assert_eq!(outgoing(&total_agg), set(&["total"]));

    // And the agg-collapsed projection matches the variable-level edges.
    assert_eq!(
        project_to_variable_edges(&elem_edges),
        var_set,
        "projection mismatch on the fixed subset spec"
    );
}

/// Deterministic GH #525 companion: fixed mixed-reference specs (the 1:1
/// target shape pinning `young`, and the broadcast shape pinning `old`)
/// must expand to EXACTLY the hand-pinned diagonal-with-pinned-axes edges.
/// Like the subset companion, the literal edge names are the independent
/// oracle for the `read_slice_rows`-derived property expectations.
#[test]
fn mixed_ref_specs_expand_to_pinned_diagonal() {
    let base = ProjectSpec {
        dim_size: 2,
        var_specs: vec![
            ArrayedVarSpec {
                pattern: RefPattern::Constant,
            },
            ArrayedVarSpec {
                pattern: RefPattern::Bare { var_idx: 0 },
            },
        ],
        include_scalar: false,
        scalar_reducer_target: None,
        subset_size: 1,
        mixed_ref_target: None,
    };
    let pin = |pairs: &[(&str, &str)]| -> BTreeSet<(String, String)> {
        pairs
            .iter()
            .map(|(f, t)| (f.to_string(), t.to_string()))
            .collect()
    };

    // 1:1 case: `mixed[Dim] = wide[Dim, young] * 2` -- the same-Dim
    // diagonal pinned at Age = young; never the cross-product rows
    // (`wide[a,old] -> mixed[a]`, `wide[a,young] -> mixed[b]`).
    let mut spec = base.clone();
    spec.mixed_ref_target = Some(MixedRefTarget {
        var_idx: 0,
        age_idx: 0,
        broadcast: false,
    });
    let (var_set, elem_edges) = edges_for_spec(&spec);
    assert_eq!(
        edges_between_vars(&elem_edges, "wide", "mixed"),
        pin(&[("wide[a,young]", "mixed[a]"), ("wide[b,young]", "mixed[b]"),]),
        "1:1 mixed reference must expand to the pinned diagonal: {:?}",
        elem_edges.edges
    );
    assert_eq!(
        project_to_variable_edges(&elem_edges),
        var_set,
        "projection mismatch on the 1:1 mixed spec"
    );

    // Broadcast case, pinning the OTHER Age element:
    // `mixed[Dim,Age] = wide[Dim, old] * 2` -- the Iterated dims (`Dim`)
    // are a strict subset of the target's, so each pinned row feeds BOTH
    // Age slots of its Dim coordinate (and no other Dim's slots).
    let mut spec = base;
    spec.mixed_ref_target = Some(MixedRefTarget {
        var_idx: 0,
        age_idx: 1,
        broadcast: true,
    });
    let (var_set, elem_edges) = edges_for_spec(&spec);
    assert_eq!(
        edges_between_vars(&elem_edges, "wide", "mixed"),
        pin(&[
            ("wide[a,old]", "mixed[a,young]"),
            ("wide[a,old]", "mixed[a,old]"),
            ("wide[b,old]", "mixed[b,young]"),
            ("wide[b,old]", "mixed[b,old]"),
        ]),
        "broadcast mixed reference must feed every Age slot of its Dim row: {:?}",
        elem_edges.edges
    );
    assert_eq!(
        project_to_variable_edges(&elem_edges),
        var_set,
        "projection mismatch on the broadcast mixed spec"
    );
}
