// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The specification of [`super::match_axes`], as one table whose rows are
//! derived from the matchers it replaced.
//!
//! Twelve places decided how two axis lists match before this one existed --
//! seven functions and five inline searches. Each row below names the arm it
//! comes from, so what those places did between them is what the single
//! precedence is held to; the rows marked DIVERGENCE are where two of them
//! disagreed on the same input, or where the union would have been unsafe,
//! and each is recorded in the design plan's "Phase 6a semantic divergences".
//!
//! | # | replaced matcher | arms it had, in its own order |
//! |---|---|---|
//! | A | `compiler::dimensions::allocate_implicit_axes_partial` | name, mapping (forward, forward-to-a-parent, reverse), size |
//! | B | `compiler::dimensions::allocate_implicit_axes` | A, requiring every axis |
//! | C | `compiler::dimensions::match_dimensions_with_mapping` | name, mapping (forward, reverse, common target), size |
//! | D | `compiler::dimensions::find_dimension_reordering` | name only, equal arity, bijection |
//! | E | `ast::Expr2::can_all_match` | name, size |
//! | F | `ast::Expr2::find_matching_dimension` | name, **size**, mapping (either direction) |
//! | G | `compiler::view_contains` / `named_dims` | identical shape, else name and size, refusing an unnamed or repeated axis |
//! | H | `compiler::context`'s Subscript arm (`use_name_matching` and the active-dimension lookup) | name, mapping (forward, forward-to-a-parent, reverse) |
//! | J | `compiler::context`'s dynamic-range arm | name, **subdimension** (either direction) or mapping (either direction) |
//! | K | `compiler::subscript::normalize_subscripts3`, `IndexExpr3::Expr` | name only |
//! | L | `compiler::subscript::normalize_subscripts3`, `IndexExpr3::Dimension` | name, mapping (either direction) |
//! | M | `compiler::context`'s `Expr3::Var` dimension-as-value arm | name, mapping (either direction) |
//!
//! Beyond those arms the table rows the three production [`AxisRelations`]
//! PROJECTIONS (`Row::projection`). Which rungs can FIRE is the caller's
//! projection rather than a property of the two axis lists, so every rung the
//! ordinal-resolving `DirectMappingsOnly` withholds has a `[None]` row beside
//! the row where the full context admits it, and every rung it keeps has a row
//! saying so. [`super::NoAxisRelations`] is rowed by the named tests below.
//!
//! `dimensions::match_dimensions_two_pass` is NOT in the table: it is the
//! runtime broadcast matcher (VM `LoadIterViewAt`, wasm `ViewDesc`), pairs
//! `dim_id`s where no `DimensionsContext` exists, and states a deliberately
//! weaker rule. [`super::match_axes_partial`]'s rustdoc records why the two
//! stay apart.

use super::{
    Axis, AxisMatch, AxisRelations, Dimension, DimensionsContext, DirectMappingsOnly,
    NoAxisRelations, SubdimensionRelations, axes_of, match_axes, match_axes_partial,
};
use crate::common::CanonicalDimensionName;
use crate::datamodel;

fn ctx_of(dims: &[datamodel::Dimension]) -> DimensionsContext {
    DimensionsContext::from(dims)
}

fn dim(ctx: &DimensionsContext, name: &str) -> Dimension {
    ctx.get(&CanonicalDimensionName::from_raw(name))
        .unwrap_or_else(|| panic!("dimension '{name}' not in context"))
        .clone()
}

fn named(name: &str, elements: &[&str]) -> datamodel::Dimension {
    datamodel::Dimension::named(
        name.to_string(),
        elements.iter().map(|e| e.to_string()).collect(),
    )
}

fn maps_to(mut d: datamodel::Dimension, target: &str) -> datamodel::Dimension {
    d.set_maps_to(target.to_string());
    d
}

fn maps_to_all(mut d: datamodel::Dimension, targets: &[&str]) -> datamodel::Dimension {
    d.mappings = targets
        .iter()
        .map(|t| datamodel::DimensionMapping {
            target: t.to_string(),
            element_map: vec![],
        })
        .collect();
    d
}

fn indexed(name: &str, size: u32) -> datamodel::Dimension {
    datamodel::Dimension::indexed(name.to_string(), size)
}

fn exact(target_idx: usize) -> Option<(usize, AxisMatch)> {
    Some((target_idx, AxisMatch::Exact))
}

fn mapped(target_idx: usize, via: &str) -> Option<(usize, AxisMatch)> {
    Some((
        target_idx,
        AxisMatch::Mapped {
            via: CanonicalDimensionName::from_raw(via),
        },
    ))
}

fn subdim(target_idx: usize) -> Option<(usize, AxisMatch)> {
    Some((target_idx, AxisMatch::Subdimension))
}

fn by_size(target_idx: usize) -> Option<(usize, AxisMatch)> {
    Some((target_idx, AxisMatch::BySize))
}

/// Which [`AxisRelations`] projection a row runs through.
///
/// WHICH rungs can fire is the caller's projection rather than a property of
/// the two axis lists, so it is part of the row. The three variants are the
/// three production projections; [`NoAxisRelations`] answers nothing and is
/// rowed by its own tests below.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Projection {
    /// The plain [`DimensionsContext`]: every rung except the subdimension
    /// one, which it does not admit (see [`AxisRelations::is_subdimension`]).
    Full,
    /// [`DirectMappingsOnly`], for a caller that resolves the paired element
    /// by ORDINAL. It withholds both INDIRECT correspondences -- the mapping
    /// onto a parent of the target, and the subdimension rung.
    DirectMappings,
    /// [`SubdimensionRelations`]: the full context plus the subdimension rung,
    /// for the one caller that acts on a partial correspondence.
    Subdimensions,
}

struct Row {
    /// What the row pins, and which replaced matcher's arm it comes from.
    name: &'static str,
    declared: Vec<datamodel::Dimension>,
    source: Vec<&'static str>,
    target: Vec<&'static str>,
    expect: Vec<Option<(usize, AxisMatch)>>,
    projection: Projection,
}

/// One row per arm of every matcher [`super::match_axes`] replaced.
///
/// The expected column is the ONE precedence -- exact name, declared mapping,
/// subdimension, size -- applied to that arm's input. Where the old matchers
/// disagreed, the row says so and names the design plan's divergence entry.
fn rows() -> Vec<Row> {
    vec![
        // ---- exact name: every matcher's first arm (A, B, C, D, E, F, G, H, J, K, L, M) ----
        Row {
            name: "exact_name_pairs_the_two_axes",
            declared: vec![named("a", &["a1", "a2"]), named("b", &["b1", "b2", "b3"])],
            source: vec!["a", "b"],
            target: vec!["b", "a"],
            projection: Projection::Full,
            expect: vec![exact(1), exact(0)],
        },
        // ---- declared mapping, forward: A, C, F, H, J, L, M ----
        Row {
            name: "a_forward_declared_mapping_pairs_them_via_the_target",
            declared: vec![
                named("dima", &["a1", "a2", "a3"]),
                maps_to(named("dimb", &["b1", "b2", "b3"]), "dima"),
            ],
            source: vec!["dimb"],
            target: vec!["dima"],
            projection: Projection::Full,
            expect: vec![mapped(0, "dima")],
        },
        // The same DIRECT mapping under the ordinal-resolving projection: it
        // is admitted there, because the ordinal read IS the documented
        // bare-reference rule for a directly mapped pair (GH #527 / #997).
        Row {
            name: "a_direct_mapping_still_pairs_for_an_ordinal_resolving_caller",
            declared: vec![
                named("dima", &["a1", "a2", "a3"]),
                maps_to(named("dimb", &["b1", "b2", "b3"]), "dima"),
            ],
            source: vec!["dimb"],
            target: vec!["dima"],
            projection: Projection::DirectMappings,
            expect: vec![mapped(0, "dima")],
        },
        // ---- declared mapping, reverse: A, C, F, H, J, L, M ----
        Row {
            name: "a_reverse_declared_mapping_pairs_them_via_the_source",
            declared: vec![
                maps_to(named("dima", &["a1", "a2", "a3"]), "dimb"),
                named("dimb", &["b1", "b2", "b3"]),
            ],
            source: vec!["dimb"],
            target: vec!["dima"],
            projection: Projection::Full,
            expect: vec![mapped(0, "dimb")],
        },
        // ---- declared mapping onto a PARENT of the target: A, H only.
        // DIVERGENCE for C, which had no such arm and left the axis
        // unmatched. ----
        Row {
            name: "a_mapping_onto_a_parent_of_the_target_pairs_them_via_that_parent",
            declared: vec![
                named("parent", &["p1", "p2", "p3", "p4"]),
                named("sub", &["p1", "p3"]),
                maps_to(named("src", &["s1", "s2", "s3", "s4"]), "parent"),
            ],
            source: vec!["src"],
            target: vec!["sub"],
            projection: Projection::Full,
            expect: vec![mapped(0, "parent")],
        },
        // The parent rung is an INDIRECT correspondence -- target -> parent ->
        // source rather than the target axis's own ordinal -- so
        // `DirectMappingsOnly` withholds it. Nothing else asserts the
        // withholding: the row above runs under the full context, which admits
        // it, and a projection that silently kept it would make
        // `make_dimension_subscripts` emit a dimension-name subscript for a
        // pairing whose element is not that dimension's ordinal.
        Row {
            name: "a_mapping_onto_a_parent_is_withheld_from_an_ordinal_resolving_caller",
            declared: vec![
                named("parent", &["p1", "p2", "p3", "p4"]),
                named("sub", &["p1", "p3"]),
                maps_to(named("src", &["s1", "s2", "s3", "s4"]), "parent"),
            ],
            source: vec!["src"],
            target: vec!["sub"],
            projection: Projection::DirectMappings,
            expect: vec![None],
        },
        // ---- both map onto one common dimension: C only.
        // DIVERGENCE for A, which had no such arm. ----
        Row {
            name: "two_axes_mapping_onto_one_common_dimension_pair_via_it",
            declared: vec![
                named("dima", &["a1", "a2", "a3"]),
                named("dimb", &["b1", "b2", "b3"]),
                named("dimc", &["c1", "c2", "c3"]),
                maps_to_all(named("dimx", &["x1", "x2", "x3"]), &["dimb", "dimc"]),
                maps_to_all(named("dimy", &["y1", "y2", "y3"]), &["dima", "dimc"]),
            ],
            source: vec!["dimx"],
            target: vec!["dimy"],
            projection: Projection::Full,
            expect: vec![mapped(0, "dimc")],
        },
        // Both mappings are DIRECT, so the shared-target rung survives the
        // ordinal-resolving projection.
        Row {
            name: "a_common_mapping_target_still_pairs_for_an_ordinal_resolving_caller",
            declared: vec![
                named("dima", &["a1", "a2", "a3"]),
                named("dimb", &["b1", "b2", "b3"]),
                named("dimc", &["c1", "c2", "c3"]),
                maps_to_all(named("dimx", &["x1", "x2", "x3"]), &["dimb", "dimc"]),
                maps_to_all(named("dimy", &["y1", "y2", "y3"]), &["dima", "dimc"]),
            ],
            source: vec!["dimx"],
            target: vec!["dimy"],
            projection: Projection::DirectMappings,
            expect: vec![mapped(0, "dimc")],
        },
        // ---- subdimension, either direction: J only, and only for a caller
        // that admits the rung.
        //
        // DIVERGENCE, recorded: the union would have given A, C and H a rung
        // they never had, and each of them resolves an ELEMENT through the
        // pairing. `out[Sub] = src` over `src[Parent]` is refused with
        // `MismatchedDimensions` today; pairing those axes compiles it into
        // the positional read `out[Sub] = src[Sub]` already produces (GH
        // #1029), turning a loud refusal into a silent wrong number. So the
        // rung is opt-in -- see `AxisRelations::is_subdimension`. ----
        Row {
            name: "a_subdimension_of_the_target_pairs_with_it_for_a_caller_that_admits_the_rung",
            declared: vec![
                named("parent", &["p1", "p2", "p3", "p4"]),
                named("sub", &["p1", "p3"]),
            ],
            source: vec!["sub"],
            target: vec!["parent"],
            projection: Projection::Subdimensions,
            expect: vec![subdim(0)],
        },
        Row {
            name: "a_parent_of_the_target_pairs_with_it_for_a_caller_that_admits_the_rung",
            declared: vec![
                named("parent", &["p1", "p2", "p3", "p4"]),
                named("sub", &["p1", "p3"]),
            ],
            source: vec!["parent"],
            target: vec!["sub"],
            projection: Projection::Subdimensions,
            expect: vec![subdim(0)],
        },
        Row {
            name: "a_subdimension_does_not_pair_for_a_caller_that_does_not_admit_the_rung",
            declared: vec![
                named("parent", &["p1", "p2", "p3", "p4"]),
                named("sub", &["p1", "p3"]),
            ],
            source: vec!["sub"],
            target: vec!["parent"],
            projection: Projection::Full,
            expect: vec![None],
        },
        Row {
            name: "a_subdimension_does_not_pair_for_an_ordinal_resolving_caller_either",
            declared: vec![
                named("parent", &["p1", "p2", "p3", "p4"]),
                named("sub", &["p1", "p3"]),
            ],
            source: vec!["sub"],
            target: vec!["parent"],
            projection: Projection::DirectMappings,
            expect: vec![None],
        },
        // ---- size, indexed dimensions only: A, C, E, F ----
        Row {
            name: "two_indexed_dimensions_of_one_length_pair_by_size",
            declared: vec![indexed("ia", 3), indexed("ib", 3)],
            source: vec!["ia"],
            target: vec!["ib"],
            projection: Projection::Full,
            expect: vec![by_size(0)],
        },
        Row {
            name: "named_dimensions_of_one_length_do_not_pair_by_size",
            declared: vec![
                named("cities", &["boston", "seattle"]),
                named("products", &["widgets", "gadgets"]),
            ],
            source: vec!["cities"],
            target: vec!["products"],
            projection: Projection::Full,
            expect: vec![None],
        },
        Row {
            name: "an_indexed_axis_does_not_pair_by_size_with_a_named_one",
            declared: vec![indexed("ia", 2), named("cities", &["boston", "seattle"])],
            source: vec!["ia"],
            target: vec!["cities"],
            projection: Projection::Full,
            expect: vec![None],
        },
        Row {
            name: "indexed_dimensions_of_different_lengths_do_not_pair",
            declared: vec![indexed("ia", 3), indexed("ib", 4)],
            source: vec!["ia"],
            target: vec!["ib"],
            projection: Projection::Full,
            expect: vec![None],
        },
        // ---- the allocation is one-to-one: A, C, E ----
        Row {
            name: "two_source_axes_cannot_both_take_one_target_axis",
            declared: vec![indexed("ix", 3), indexed("iy", 3), indexed("iz", 3)],
            source: vec!["ix", "iy"],
            target: vec!["iz"],
            projection: Projection::Full,
            expect: vec![by_size(0), None],
        },
        // ---- the allocation is positional: a repeated target dimension is
        // two distinct axes (A's documented property) ----
        Row {
            name: "a_repeated_target_dimension_is_two_axes_not_one",
            declared: vec![named("d", &["d1", "d2", "d3"])],
            source: vec!["d", "d"],
            target: vec!["d", "d"],
            projection: Projection::Full,
            expect: vec![exact(0), exact(1)],
        },
        Row {
            name: "one_source_axis_takes_the_first_of_two_repeated_target_axes",
            declared: vec![named("d", &["d1", "d2", "d3"])],
            source: vec!["d"],
            target: vec!["d", "d"],
            projection: Projection::Full,
            expect: vec![exact(0)],
        },
        // ---- flat staging: a stronger rule wins the target however the axes
        // are declared (A, GH #996) ----
        Row {
            name: "a_name_match_beats_an_earlier_axiss_mapping_match",
            declared: vec![
                named("cop", &["c1", "c2"]),
                maps_to(named("agg", &["r1", "r2"]), "cop"),
            ],
            source: vec!["cop", "agg"],
            target: vec!["agg"],
            projection: Projection::Full,
            expect: vec![None, exact(0)],
        },
        Row {
            name: "a_mapping_match_beats_an_earlier_axiss_size_match",
            declared: vec![
                indexed("ia", 2),
                indexed("ib", 2),
                maps_to(named("y", &["y1", "y2"]), "ia"),
            ],
            source: vec!["ib", "y"],
            target: vec!["ia"],
            projection: Projection::Full,
            expect: vec![None, mapped(0, "ia")],
        },
        Row {
            name: "a_subdimension_match_beats_an_earlier_axiss_size_match",
            declared: vec![
                indexed("ia", 2),
                indexed("ib", 2),
                named("parent", &["p1", "p2", "p3"]),
                named("sub", &["p1", "p3"]),
            ],
            source: vec!["ib", "sub"],
            target: vec!["parent"],
            projection: Projection::Subdimensions,
            expect: vec![None, subdim(0)],
        },
        // ---- ORDER WITHIN the mapping rung: the first TARGET that any of the
        // rung's four sub-rules relates the source axis to wins.
        //
        // DIVERGENCE for A, which staged its sub-rules across all targets --
        // forward and forward-to-a-parent for every target, then reverse for
        // every target -- so `s` took `t2` through its own `maps_to` instead of
        // `t1` through `t1`'s. Neither is more correct; the point of the phase
        // is that there is one order, and target-first is what C, F, H, L and M
        // already used. `simulate.rs`'s
        // `a_source_axis_takes_the_first_target_the_mapping_rung_relates_it_to`
        // pins what that means for a stock's flow wiring, the production
        // caller. ----
        Row {
            name: "the_mapping_rung_takes_the_first_target_any_sub_rule_relates",
            declared: vec![
                maps_to(named("s", &["s1", "s2", "s3"]), "t2"),
                maps_to(named("t1", &["t1e", "t2e", "t3e"]), "s"),
                named("t2", &["u1", "u2", "u3"]),
            ],
            source: vec!["s"],
            target: vec!["t1", "t2"],
            projection: Projection::Full,
            expect: vec![mapped(0, "s")],
        },
        // ---- precedence between the two rules F ordered the other way round.
        // DIVERGENCE: F tried size BEFORE mapping, so this axis took `ib`.
        //
        // The two rules can only compete across TARGETS, never within one
        // source axis: a mapping is declared on named dimensions only, so a
        // forward mapping needs a named source while the size rule needs an
        // indexed one. It is the REVERSE mapping arm that reaches an indexed
        // source -- `y` is named and maps onto `ia` -- and that is what makes
        // the two orderings observable. ----
        Row {
            name: "a_declared_mapping_outranks_a_size_match_on_another_axis",
            declared: vec![
                indexed("ia", 2),
                indexed("ib", 2),
                maps_to(named("y", &["y1", "y2"]), "ia"),
            ],
            source: vec!["ia"],
            target: vec!["ib", "y"],
            projection: Projection::Full,
            expect: vec![mapped(1, "ia")],
        },
        // ---- a source axis nothing supplies is None, not an error (A, C, E) ----
        Row {
            name: "an_unsupplied_source_axis_is_left_unmatched",
            declared: vec![
                named("a", &["a1"]),
                named("b", &["b1"]),
                named("c", &["c1"]),
            ],
            source: vec!["a", "c"],
            target: vec!["a", "b"],
            projection: Projection::Full,
            expect: vec![exact(0), None],
        },
        // ---- empty lists (D's empty case) ----
        Row {
            name: "no_source_axes_match_trivially",
            declared: vec![named("a", &["a1"])],
            source: vec![],
            target: vec!["a"],
            projection: Projection::Full,
            expect: vec![],
        },
    ]
}

/// Every row of the table, through the declared-dimension entry point.
#[test]
fn match_axes_follows_one_precedence_on_every_replaced_matchers_arms() {
    for row in rows() {
        let ctx = ctx_of(&row.declared);
        let source: Vec<Dimension> = row.source.iter().map(|n| dim(&ctx, n)).collect();
        let target: Vec<Dimension> = row.target.iter().map(|n| dim(&ctx, n)).collect();
        let direct_relations = DirectMappingsOnly(&ctx);
        let subdim_relations = SubdimensionRelations(&ctx);
        let relations: &dyn AxisRelations = match row.projection {
            Projection::Full => &ctx,
            Projection::DirectMappings => &direct_relations,
            Projection::Subdimensions => &subdim_relations,
        };
        let got = match_axes_partial(&axes_of(&source), &axes_of(&target), relations);
        assert_eq!(
            got, row.expect,
            "row '{}': source {:?} target {:?}",
            row.name, row.source, row.target
        );

        // `match_axes` is the same allocation over declared dimensions, with
        // totality required and the default (`DimensionsContext`) relations,
        // so only the rows that already run through those can cross-check it.
        if row.projection == Projection::Full {
            let total = match_axes(&source, &target, &ctx);
            let expected_total: Option<Vec<(usize, AxisMatch)>> =
                row.expect.clone().into_iter().collect();
            assert_eq!(
                total, expected_total,
                "row '{}': match_axes must be match_axes_partial collected",
                row.name
            );
        }
    }
}

/// More source axes than target axes can never be a total match: the
/// allocation is one-to-one, so pigeonhole leaves one unresolved. This is what
/// replaced `allocate_implicit_axes`'s explicit arity bail (B).
#[test]
fn more_source_axes_than_target_axes_has_no_total_match() {
    let ctx = ctx_of(&[named("a", &["a1"]), named("b", &["b1"])]);
    let source = vec![dim(&ctx, "a"), dim(&ctx, "b")];
    let target = vec![dim(&ctx, "a")];
    assert_eq!(match_axes(&source, &target, &ctx), None);
}

/// Without a [`DimensionsContext`] only the name and size rules can fire --
/// what a caller comparing two `ArrayView`s can answer (G). The mapping is
/// declared and would otherwise pair these two.
#[test]
fn no_relations_leaves_only_the_name_and_size_rules() {
    let ctx = ctx_of(&[
        named("dima", &["a1", "a2"]),
        maps_to(named("dimb", &["b1", "b2"]), "dima"),
    ]);
    let source = vec![dim(&ctx, "dimb")];
    let target = vec![dim(&ctx, "dima")];
    assert_eq!(
        match_axes_partial(&axes_of(&source), &axes_of(&target), &ctx),
        vec![mapped(0, "dima")]
    );
    assert_eq!(
        match_axes_partial(&axes_of(&source), &axes_of(&target), &NoAxisRelations),
        vec![None],
        "with no relations the declared mapping is invisible"
    );
}

/// An axis with no name never matches by name (G): a temp's axes are blank,
/// and two blanks are not thereby the same dimension.
#[test]
fn an_unnamed_axis_never_matches_by_name() {
    let source = [Axis::named("", 3)];
    let target = [Axis::named("", 3), Axis::named("d", 3)];
    assert_eq!(
        match_axes_partial(&source, &target, &NoAxisRelations),
        vec![None]
    );
}

/// `Axis::named` reports `indexed: false`, so a view's axes never reach the
/// size rule: a view records each axis's name and length but not which kind of
/// dimension produced it, and pairing two same-length axes on that alone is
/// the guess `named_dims` (G) refused to make.
#[test]
fn view_axes_do_not_match_by_size() {
    let source = [Axis::named("x", 3)];
    let target = [Axis::named("y", 3)];
    assert_eq!(
        match_axes_partial(&source, &target, &NoAxisRelations),
        vec![None]
    );
}

/// The `AxisRelations` projection a caller supplies is what decides which
/// rules can fire, so a projection that answers only `maps_to` -- which is all
/// `ast::Expr2`'s bounds unification can answer through `Expr2Context` --
/// reaches the mapping rule and not the subdimension one.
#[test]
fn a_relations_projection_that_answers_only_mapping_reaches_only_that_rule() {
    struct OnlyMapping;
    impl AxisRelations for OnlyMapping {
        fn maps_to(&self, from: &str, to: &str) -> bool {
            (from, to) == ("dimb", "dima")
        }
    }
    let source = [Axis::named("dimb", 3)];
    let target = [Axis::named("dima", 3)];
    assert_eq!(
        match_axes_partial(&source, &target, &OnlyMapping),
        vec![mapped(0, "dima")]
    );

    // The same projection cannot answer a subdimension relation, so an axis
    // pair that has only that relation is left unmatched.
    let sub_source = [Axis::named("sub", 2)];
    let sub_target = [Axis::named("parent", 4)];
    assert_eq!(
        match_axes_partial(&sub_source, &sub_target, &OnlyMapping),
        vec![None]
    );
}
