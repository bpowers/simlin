// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the LTM reference-site classification IR.
//!
//! Two layers:
//! 1. `collect_reference_sites_tests` -- the `(shape, in_reducer)` contract
//!    per AST site (the Phase-1 regression guards, ported from `db/analysis.rs`).
//!    These exercise the production all-sources walker `collect_all_reference_sites`
//!    (the IR builds on it) and pin the per-AST-site shape + `in_reducer`
//!    primitive that feeds the IR's routing decision.
//! 2. `model_ltm_reference_sites_tests` -- the *public* IR contract: the
//!    `(shape, target_element, routing)` of each `ClassifiedSite`, the AC1.4
//!    `StarRange` consistency, and the AC1.5 SIZE / scalar-source-reducer
//!    `Direct` routing. Each asserts the routing annotation lines up with
//!    `enumerate_agg_nodes` (the sole hoisting decider).

use super::*;
use crate::common::{Canonical, Ident};
use crate::db::{SimlinDb, sync_from_datamodel};
use crate::test_common::TestProject;

// ── Layer 1: the per-AST-site (shape, in_reducer) contract ─────────────────

mod collect_reference_sites_tests {
    use super::*;

    /// Helper: build a project, sync into salsa, walk `target_name`'s AST via
    /// the production `collect_all_reference_sites`, and return the reference
    /// sites bucketed under `source_name`. `lookup_dims` resolves a
    /// referenced variable's dimensions from the reconstructed `Variable` map
    /// -- the same way `model_ltm_reference_sites` does.
    fn collect(project: &TestProject, target_name: &str, source_name: &str) -> Vec<ReferenceSite> {
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let source_model = sync.models["main"].source;
        let source_project = sync.project;

        let variables = crate::db::reconstruct_model_variables(&db, source_model, source_project);
        let target_var = variables
            .get(&Ident::<Canonical>::new(target_name))
            .cloned()
            .unwrap_or_else(|| panic!("variable '{target_name}' not found"));

        let dm_dims = crate::db::project_datamodel_dims(&db, source_project);
        let dim_ctx = crate::dimensions::DimensionsContext::from(dm_dims.as_slice());
        let mut lookup_dims = |name: &str| -> Vec<crate::dimensions::Dimension> {
            variables
                .get(&Ident::<Canonical>::new(name))
                .and_then(|v| v.get_dimensions())
                .map(|d| d.to_vec())
                .unwrap_or_default()
        };
        super::collect_all_reference_sites(&target_var, &variables, &dim_ctx, &mut lookup_dims)
            .remove(source_name)
            .unwrap_or_default()
    }

    #[test]
    fn ref_site_bare_a2a() {
        // A2A equation: births[Region] = population * 0.1
        // The bare `population` reference is one occurrence with shape Bare.
        let project = TestProject::new("bare_a2a")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("population[Region]", "100")
            .array_aux("births[Region]", "population * 0.1");

        let sites = collect(&project, "births", "population");
        assert_eq!(sites.len(), 1, "sites: {sites:?}");
        assert_eq!(sites[0].shape, RefShape::Bare);
    }

    #[test]
    fn ref_site_fixed_index() {
        // relative_pop[Region] = population / population[NYC]
        // Two occurrences: a bare `population` (numerator) and a
        // FixedIndex `population[NYC]` (denominator).
        let project = TestProject::new("fixed")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("population[Region]", "100")
            .array_aux("relative_pop[Region]", "population / population[NYC]");

        let sites = collect(&project, "relative_pop", "population");
        assert_eq!(sites.len(), 2, "sites: {sites:?}");
        // AST-walk order: numerator first (bare), denominator second (FixedIndex).
        assert_eq!(sites[0].shape, RefShape::Bare);
        assert_eq!(
            sites[1].shape,
            RefShape::FixedIndex(vec!["nyc".to_string()])
        );
    }

    #[test]
    fn ref_site_wildcard_reducer() {
        // total = SUM(population[*])
        // The wildcard subscript inside the reducer produces one Wildcard
        // site, and it must be flagged `in_reducer`.
        let project = TestProject::new("wild")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("population[Region]", "100")
            .scalar_aux("total", "SUM(population[*])");

        let sites = collect(&project, "total", "population");
        assert_eq!(sites.len(), 1, "sites: {sites:?}");
        assert_eq!(sites[0].shape, RefShape::Wildcard);
        assert!(sites[0].in_reducer, "SUM's wildcard arg is in a reducer");
    }

    #[test]
    fn ref_site_bare_arrayed_arg_is_in_reducer() {
        // total = SUM(pop)   (pop is arrayed)
        // A bare arrayed argument to a reducer is the whole-array full
        // reduce that `enumerate_agg_nodes` hoists. The AST reference is a
        // bare `Var`, so its site shape is `Bare` -- but it must still be
        // flagged `in_reducer` so the element-graph reroute treats it as
        // the reducer's input (consistent with `SUM(pop[*])`, which differs
        // only in the explicit wildcard subscript).
        let project = TestProject::new("bare_arrayed_arg")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            .scalar_aux("total", "SUM(pop)");

        let sites = collect(&project, "total", "pop");
        assert_eq!(sites.len(), 1, "sites: {sites:?}");
        assert_eq!(sites[0].shape, RefShape::Bare);
        assert!(
            sites[0].in_reducer,
            "SUM's bare arrayed arg is the reducer's input"
        );
    }

    #[test]
    fn ref_site_mixed_bare_and_wildcard() {
        // share[Region] = population / SUM(population[*])
        // Two occurrences: a bare numerator (not in a reducer) and a wildcard
        // reducer denominator (in a reducer).
        let project = TestProject::new("mixed")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("population[Region]", "100")
            .array_aux("share[Region]", "population / SUM(population[*])");

        let sites = collect(&project, "share", "population");
        assert_eq!(sites.len(), 2, "sites: {sites:?}");
        let bare = sites
            .iter()
            .find(|s| s.shape == RefShape::Bare)
            .expect("expected a Bare site");
        assert!(!bare.in_reducer, "the bare numerator is not in a reducer");
        let wildcard = sites
            .iter()
            .find(|s| s.shape == RefShape::Wildcard)
            .expect("expected a Wildcard site");
        assert!(
            wildcard.in_reducer,
            "the SUM's wildcard arg is in a reducer"
        );
    }

    /// The Fix 1 case: `x = SUM(pop[*]) + pop[idx]`. Two occurrences of
    /// `pop`: the `SUM`'s wildcard arg (Wildcard, `in_reducer`) and the
    /// direct dynamic-index reference `pop[idx]` (DynamicIndex, *not*
    /// `in_reducer` -- it's not syntactically inside any reducer). The
    /// element-graph reroute keys on `in_reducer`, so the direct `pop[idx]`
    /// must keep its own conservative edge / Bare link score even though it
    /// shares the `DynamicIndex` shape that the old (shape-only) predicate
    /// would have collapsed into the hoisted agg.
    #[test]
    fn ref_site_reducer_and_direct_dynamic_index() {
        let project = TestProject::new("mixed_dyn")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            .scalar_aux("idx", "1")
            .scalar_aux("x", "SUM(pop[*]) + pop[idx]");

        let sites = collect(&project, "x", "pop");
        assert_eq!(sites.len(), 2, "sites: {sites:?}");
        let wildcard = sites
            .iter()
            .find(|s| s.shape == RefShape::Wildcard)
            .expect("expected a Wildcard site for SUM(pop[*])");
        assert!(wildcard.in_reducer, "SUM's wildcard arg is in a reducer");
        let dynamic = sites
            .iter()
            .find(|s| s.shape == RefShape::DynamicIndex)
            .expect("expected a DynamicIndex site for pop[idx]");
        assert!(
            !dynamic.in_reducer,
            "the direct pop[idx] reference is not inside any reducer"
        );
    }

    /// `SIZE(pop[*])` is *not* a reducer for hoisting purposes (its result
    /// doesn't depend on element values), so its wildcard arg is not
    /// `in_reducer`. (`enumerate_agg_nodes` excludes SIZE for the same
    /// reason; the two must agree.)
    #[test]
    fn ref_site_size_arg_is_not_in_reducer() {
        let project = TestProject::new("size_arg")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            .scalar_aux("n", "SIZE(pop[*])");

        let sites = collect(&project, "n", "pop");
        assert_eq!(sites.len(), 1, "sites: {sites:?}");
        assert_eq!(sites[0].shape, RefShape::Wildcard);
        assert!(
            !sites[0].in_reducer,
            "SIZE is not an element-value reducer, so its arg is not in_reducer"
        );
    }

    /// The 2-argument `MIN(a, b)` / `MAX(a, b)` are scalar pairwise ops, not
    /// array reducers, so their arguments are not `in_reducer`. The 1-arg
    /// `MIN(pop[*])` *is* a reducer. This guards the `Min(_, None)` vs
    /// `Min(_, Some(_))` distinction against drifting from
    /// `ltm_agg::reducer_kind`.
    #[test]
    fn ref_site_two_arg_min_is_not_a_reducer() {
        let project = TestProject::new("two_arg_min")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            // floor[Region] uses pop both as a 2-arg MIN operand (not a
            // reducer) and inside a 1-arg MIN reducer.
            .array_aux("floor[Region]", "MIN(pop, 50) + MIN(pop[*])");

        let sites = collect(&project, "floor", "pop");
        // `MIN(pop, 50)` -> one Bare site (not in_reducer);
        // `MIN(pop[*])` -> one Wildcard site (in_reducer).
        assert_eq!(sites.len(), 2, "sites: {sites:?}");
        let bare = sites
            .iter()
            .find(|s| s.shape == RefShape::Bare)
            .expect("expected a Bare site for the 2-arg MIN operand");
        assert!(
            !bare.in_reducer,
            "2-arg MIN(pop, 50) is a scalar pairwise op, not a reducer"
        );
        let wildcard = sites
            .iter()
            .find(|s| s.shape == RefShape::Wildcard)
            .expect("expected a Wildcard site for the 1-arg MIN reducer");
        assert!(wildcard.in_reducer, "1-arg MIN(pop[*]) is an array reducer");
    }

    /// A reducer nested inside another reducer's argument: every reference
    /// below the outer reducer stays `in_reducer` (the flag is sticky).
    #[test]
    fn ref_site_nested_reducer_arg_stays_in_reducer() {
        let project = TestProject::new("nested_red")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
            // SUM over D1 of (per-D1 partial SUM over D2) -- the inner
            // matrix[D1,*] reference sits two reducers deep.
            .scalar_aux("grand_total", "SUM(SUM(matrix[*, *]))");

        let sites = collect(&project, "grand_total", "matrix");
        assert_eq!(sites.len(), 1, "sites: {sites:?}");
        assert!(
            sites[0].in_reducer,
            "a reference nested in two reducers is still in a reducer"
        );
    }
}

// ── Layer 2: the public ClassifiedSite IR contract ─────────────────────────

mod model_ltm_reference_sites_tests {
    use super::*;

    /// Sync `project`, run `model_ltm_reference_sites` and `enumerate_agg_nodes`,
    /// and hand both (plus the db) to `body`. The IR doesn't depend on
    /// `ltm_enabled` -- it is a structural classification -- so callers don't
    /// need to flip the LTM flag.
    fn with_ir<R>(
        project: &TestProject,
        body: impl FnOnce(&SimlinDb, &LtmReferenceSitesResult, &crate::ltm_agg::AggNodesResult) -> R,
    ) -> R {
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let model = sync.models["main"].source;
        let proj = sync.project;
        let ir = model_ltm_reference_sites(&db, model, proj);
        let aggs = crate::ltm_agg::enumerate_agg_nodes(&db, model, proj);
        body(&db, ir, aggs)
    }

    fn sites_for<'a>(
        ir: &'a LtmReferenceSitesResult,
        from: &str,
        to: &str,
    ) -> &'a [ClassifiedSite] {
        ir.sites
            .get(&(from.to_string(), to.to_string()))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// `share[R] = population / SUM(population[*])`: the `(population, share)`
    /// edge has two sites -- the bare numerator (`Direct`, shape `Bare`) and
    /// the SUM's wildcard arg, which is routed through the synthetic agg
    /// `enumerate_agg_nodes` minted for `sum(population[*])`. There is *no*
    /// `Direct` Wildcard site for `(population, share)`.
    #[test]
    fn ir_routes_share_with_sum_through_synthetic_agg() {
        let project = TestProject::new("share_sum_ir")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("population[Region]", "100")
            .array_aux("share[Region]", "population / SUM(population[*])");

        with_ir(&project, |_db, ir, aggs| {
            // There's exactly one synthetic agg, for the SUM subexpression.
            let synthetic: Vec<&crate::ltm_agg::AggNode> =
                aggs.aggs.iter().filter(|a| a.is_synthetic).collect();
            assert_eq!(
                synthetic.len(),
                1,
                "expected one synthetic agg for SUM(population[*]); got {:?}",
                aggs.aggs
            );
            let agg_idx = aggs.synthetic_by_key["sum(population[*])"];

            let sites = sites_for(ir, "population", "share");
            assert_eq!(sites.len(), 2, "sites: {sites:?}");
            // AST-walk order: numerator first.
            assert_eq!(sites[0].shape, RefShape::Bare);
            assert_eq!(sites[0].routing, SiteRouting::Direct);
            assert_eq!(sites[1].shape, RefShape::Wildcard);
            assert_eq!(
                sites[1].routing,
                SiteRouting::ThroughAgg {
                    agg: AggRef(agg_idx)
                }
            );
            // No additional Direct-Wildcard site.
            assert!(
                !sites
                    .iter()
                    .any(|s| s.shape == RefShape::Wildcard && s.routing == SiteRouting::Direct),
                "the SUM's reducer arg must not also produce a Direct Wildcard site: {sites:?}"
            );
        });
    }

    /// `relative_pop[R] = population / population[NYC]`: both sites are
    /// `Direct` (no reducer, no agg).
    #[test]
    fn ir_bare_and_fixed_index_are_direct() {
        let project = TestProject::new("fixed_ir")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("population[Region]", "100")
            .array_aux("relative_pop[Region]", "population / population[NYC]");

        with_ir(&project, |_db, ir, aggs| {
            assert!(
                aggs.aggs.is_empty(),
                "no reducer here, so no aggs; got {:?}",
                aggs.aggs
            );
            let sites = sites_for(ir, "population", "relative_pop");
            assert_eq!(sites.len(), 2, "sites: {sites:?}");
            assert_eq!(sites[0].shape, RefShape::Bare);
            assert_eq!(sites[0].routing, SiteRouting::Direct);
            assert_eq!(
                sites[1].shape,
                RefShape::FixedIndex(vec!["nyc".to_string()])
            );
            assert_eq!(sites[1].routing, SiteRouting::Direct);
        });
    }

    /// `x = SUM(pop[*]) + pop[idx]`: the SUM arg routes through the agg; the
    /// direct `pop[idx]` keeps its own `Direct` `DynamicIndex` site.
    #[test]
    fn ir_reducer_arg_routed_direct_dynamic_index_not() {
        let project = TestProject::new("mixed_dyn_ir")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            .scalar_aux("idx", "1")
            .scalar_aux("x", "SUM(pop[*]) + pop[idx]");

        with_ir(&project, |_db, ir, aggs| {
            let agg_idx = aggs.synthetic_by_key["sum(pop[*])"];
            let sites = sites_for(ir, "pop", "x");
            assert_eq!(sites.len(), 2, "sites: {sites:?}");
            let routed = sites
                .iter()
                .find(|s| s.shape == RefShape::Wildcard)
                .expect("expected a Wildcard site for SUM(pop[*])");
            assert_eq!(
                routed.routing,
                SiteRouting::ThroughAgg {
                    agg: AggRef(agg_idx)
                }
            );
            let direct = sites
                .iter()
                .find(|s| s.shape == RefShape::DynamicIndex)
                .expect("expected a DynamicIndex site for pop[idx]");
            assert_eq!(direct.routing, SiteRouting::Direct);
        });
    }

    /// GH #793: routing is per reducer site, not per `(from, to)` edge. The
    /// full-extent sibling reducer mints a synthetic agg, but the strict-slice
    /// sibling is I1-declined and must stay Direct so the link-score layer can
    /// loudly drop the incomplete edge instead of treating the sibling agg
    /// halves as complete attribution.
    #[test]
    fn ir_hoisted_sibling_does_not_claim_declined_strict_slice_site() {
        let project = TestProject::new("gh793_ir")
            .named_dimension("Region", &["nyc", "boston"])
            .named_dimension("D2", &["p", "q"])
            .array_aux("pop[Region,D2]", "1")
            .array_aux("w[D2]", "0.5")
            .array_aux_direct(
                "share",
                vec!["Region".into()],
                "SUM(pop[nyc, *] * w[*]) + SUM(pop[*, *])",
                None,
            );

        with_ir(&project, |_db, ir, aggs| {
            let agg_idx = aggs.synthetic_by_key["sum(pop[*, *])"];
            let sites = sites_for(ir, "pop", "share");
            assert_eq!(sites.len(), 2, "sites: {sites:?}");
            assert!(
                sites.iter().any(|s| {
                    s.shape == RefShape::Wildcard
                        && matches!(
                            s.routing,
                            SiteRouting::ThroughAgg { agg } if agg == AggRef(agg_idx)
                        )
                }),
                "the full-extent sibling must route through its own synthetic agg; \
                 sites: {sites:?}"
            );
            assert!(
                sites
                    .iter()
                    .any(|s| s.shape == RefShape::DynamicIndex && s.routing == SiteRouting::Direct),
                "the declined strict-slice sibling must stay Direct, not route \
                 through the full-extent sibling agg; sites: {sites:?}"
            );
        });
    }

    /// `total = SUM(population[*])` is the *whole* RHS of a scalar var, so
    /// `enumerate_agg_nodes` makes `total` itself a *variable-backed* agg
    /// (no synthetic minted). `routed_aggs` for `(population, total)` filters
    /// to synthetic aggs only, so it's empty -- the reducer reference stays
    /// `Direct` with shape `Wildcard`, matching what the old element-graph
    /// walker did (the `Wildcard` shape then drives `emit_edges_for_reference`'s
    /// reduction edge set / `try_cross_dimensional_link_scores`).
    #[test]
    fn ir_whole_rhs_reducer_is_direct_wildcard() {
        let project = TestProject::new("whole_rhs_ir")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("population[Region]", "100")
            .scalar_aux("total", "SUM(population[*])");

        with_ir(&project, |_db, ir, aggs| {
            // One variable-backed agg (the var `total`), no synthetic.
            assert_eq!(aggs.aggs.len(), 1, "{:?}", aggs.aggs);
            assert!(!aggs.aggs[0].is_synthetic);
            assert_eq!(aggs.aggs[0].name, "total");
            assert!(aggs.synthetic_by_key.is_empty());

            let sites = sites_for(ir, "population", "total");
            assert_eq!(sites.len(), 1, "sites: {sites:?}");
            assert_eq!(sites[0].shape, RefShape::Wildcard);
            assert_eq!(sites[0].routing, SiteRouting::Direct);
        });
    }

    /// AC1.4: an all-`StarRange` reducer reference (`SUM(x[*:SubDim])`) is
    /// classified `Wildcard` and routed through the synthetic agg
    /// `enumerate_agg_nodes` minted (because `compute_read_slice` maps `*:Dim`
    /// to `AxisRead::Reduced`, so the reducer is hoisted) -- with *no*
    /// additional `DynamicIndex`/`Direct` site for `(x, total)`. Before the
    /// fix the same reference classified as `DynamicIndex`; the
    /// `route_through_agg` reroute papered over it but left a latent
    /// disagreement.
    #[test]
    fn ir_starrange_reducer_routes_through_agg_no_stray_direct_edge() {
        let project = TestProject::new("starrange_ir")
            .indexed_dimension("Dim", 4)
            .indexed_subdimension("SubDim", 2, "Dim")
            .array_aux_direct("x", vec!["Dim".into()], "1", None)
            // A subexpression (not the whole RHS) so a *synthetic* agg is minted.
            .scalar_aux("total", "SUM(x[*:SubDim]) + 1");

        with_ir(&project, |_db, ir, aggs| {
            let synthetic: Vec<&crate::ltm_agg::AggNode> =
                aggs.aggs.iter().filter(|a| a.is_synthetic).collect();
            assert_eq!(
                synthetic.len(),
                1,
                "expected one synthetic agg for SUM(x[*:SubDim]); got {:?}",
                aggs.aggs
            );
            let agg_idx = aggs.synthetic_by_key.values().next().copied().unwrap();

            let sites = sites_for(ir, "x", "total");
            assert_eq!(sites.len(), 1, "sites: {sites:?}");
            assert_eq!(
                sites[0].shape,
                RefShape::Wildcard,
                "an all-`*:Dim` reducer subscript must classify as Wildcard, not DynamicIndex"
            );
            assert_eq!(
                sites[0].routing,
                SiteRouting::ThroughAgg {
                    agg: AggRef(agg_idx)
                }
            );
        });
    }

    /// AC1.5: a `SIZE(pop[*])` reference is `Direct` with shape `Wildcard`.
    /// `SIZE` is not `reducer_is_hoistable`, so `enumerate_agg_nodes` mints no
    /// agg, the reference is not `in_reducer`, and the IR records `Direct`.
    #[test]
    fn ir_size_reducer_is_direct() {
        let project = TestProject::new("size_ir")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            .scalar_aux("n", "SIZE(pop[*])");

        with_ir(&project, |_db, ir, aggs| {
            assert!(
                aggs.aggs.is_empty(),
                "SIZE is never hoisted; got {:?}",
                aggs.aggs
            );
            let sites = sites_for(ir, "pop", "n");
            assert_eq!(sites.len(), 1, "sites: {sites:?}");
            assert_eq!(sites[0].shape, RefShape::Wildcard);
            assert_eq!(sites[0].routing, SiteRouting::Direct);
        });
    }

    /// AC1.5: a reducer over a *scalar* source (`total = SUM(s)` with `s`
    /// scalar) is `Direct` with shape `Bare`. `enumerate_agg_nodes` mints no
    /// agg (a reducer needs ≥1 arrayed source), so `routed_aggs` is empty and
    /// the reference -- even though it's syntactically inside `SUM` -- routes
    /// `Direct`.
    #[test]
    fn ir_scalar_source_reducer_is_direct_bare() {
        let project = TestProject::new("scalar_red_ir")
            .scalar_aux("s", "3")
            .scalar_aux("total", "SUM(s)");

        with_ir(&project, |_db, ir, aggs| {
            assert!(
                aggs.aggs.is_empty(),
                "a reducer over only scalar sources is never hoisted; got {:?}",
                aggs.aggs
            );
            let sites = sites_for(ir, "s", "total");
            assert_eq!(sites.len(), 1, "sites: {sites:?}");
            assert_eq!(sites[0].shape, RefShape::Bare);
            assert_eq!(sites[0].routing, SiteRouting::Direct);
        });
    }

    /// An arrayed per-element target carries `target_element` on each site.
    /// `births[Region]` with per-element equations referencing `pop`:
    /// `births[NYC] = pop[NYC] * 0.1`, `births[Boston] = pop[Boston] * 0.2`.
    #[test]
    fn ir_arrayed_per_element_target_carries_target_element() {
        let project = TestProject::new("per_elem_ir")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            .array_with_ranges_direct(
                "births",
                vec!["Region".into()],
                vec![("NYC", "pop[NYC] * 0.1"), ("Boston", "pop[Boston] * 0.2")],
                None,
            );

        with_ir(&project, |_db, ir, _aggs| {
            let sites = sites_for(ir, "pop", "births");
            assert_eq!(sites.len(), 2, "sites: {sites:?}");
            let nyc = sites
                .iter()
                .find(|s| s.target_element.as_deref() == Some("nyc"))
                .expect("expected a site pinned to the nyc target element");
            assert_eq!(nyc.shape, RefShape::FixedIndex(vec!["nyc".to_string()]));
            assert_eq!(nyc.routing, SiteRouting::Direct);
            let boston = sites
                .iter()
                .find(|s| s.target_element.as_deref() == Some("boston"))
                .expect("expected a site pinned to the boston target element");
            assert_eq!(
                boston.shape,
                RefShape::FixedIndex(vec!["boston".to_string()])
            );
        });
    }

    // ── #511: iterated-dimension subscripts classify as Bare ─────────────

    /// AC3.1 (classification side): `growth[Region,Age] = row_sum[Region] * c`
    /// with `row_sum` over `Region` and `growth` over `Region x Age`. The
    /// `row_sum[Region]` subscript iterates over `growth`'s own `Region`
    /// dimension and reads the same `Region` element of `row_sum`, so it is a
    /// same-element-on-shared-dims reference (`RefShape::Bare`) rather than a
    /// genuine cross-element one. Before the fix `resolve_literal_index`
    /// rejected the dimension name `Region` and the site fell to
    /// `DynamicIndex` (which then drove the conservative cross-product and a
    /// `PREVIOUS(Subscript(...))` link-score partial).
    #[test]
    fn ir_iterated_dim_subscript_is_bare() {
        let project = TestProject::new("iterated_dim_ir")
            .named_dimension("Region", &["a", "b"])
            .named_dimension("Age", &["young", "old"])
            .array_aux("row_sum[Region]", "100")
            .array_aux_direct(
                "growth",
                vec!["Region".into(), "Age".into()],
                "row_sum[Region] * 0.5",
                None,
            );

        with_ir(&project, |_db, ir, aggs| {
            assert!(
                aggs.aggs.is_empty(),
                "no reducer here, so no aggs; got {:?}",
                aggs.aggs
            );
            let sites = sites_for(ir, "row_sum", "growth");
            assert_eq!(sites.len(), 1, "sites: {sites:?}");
            assert_eq!(
                sites[0].shape,
                RefShape::Bare,
                "an iterated-dimension subscript over the target's own dimension \
                 reads the same source element -- it is Bare, not DynamicIndex"
            );
            assert_eq!(sites[0].routing, SiteRouting::Direct);
            assert_eq!(sites[0].target_element, None);
        });
    }

    /// AC3.5: a *mapped*-dimension iterated subscript is handled the same way
    /// -- `Region` over `{a,b}`, `State` over `{s1,s2}` with a `State→Region`
    /// mapping, `x` over `Region`, `target[State] = x[State] * c`: `x[State]`
    /// is `Bare`. Downstream, `expand_same_element` projects the `Bare` edge
    /// along the mapping's element correspondence (the GH #527 diagonal --
    /// see `element_graph_tests::element_graph_mapped_iterated_dim_matches_bare_baseline`).
    #[test]
    fn ir_mapped_iterated_dim_subscript_is_bare() {
        let project = TestProject::new("mapped_iterated_dim_ir")
            .named_dimension("Region", &["a", "b"])
            .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
            .array_aux_direct("x", vec!["Region".into()], "100", None)
            .array_aux_direct("target", vec!["State".into()], "x[State] * 0.5", None);

        with_ir(&project, |_db, ir, _aggs| {
            let sites = sites_for(ir, "x", "target");
            assert_eq!(sites.len(), 1, "sites: {sites:?}");
            assert_eq!(
                sites[0].shape,
                RefShape::Bare,
                "a mapped-dimension iterated subscript (State maps to Region) is \
                 still a same-element reference -- Bare"
            );
            assert_eq!(sites[0].routing, SiteRouting::Direct);
        });
    }

    /// GH #757 (T6 flip): a mapped iterated-dim subscript whose POSITIONAL
    /// mapping is declared only in the REVERSE direction (on the source's
    /// `Region` toward `State`) now classifies `Bare` too -- the mapped arm
    /// gates on `positional_correspondence` (both declaration
    /// directions, via `classify_axis_access`'s
    /// `iterated_axis_slot_elements`), matching the compiler's
    /// `translate_via_mapping`.
    #[test]
    fn ir_reverse_declared_mapped_iterated_dim_subscript_is_bare() {
        let project = TestProject::new("reverse_mapped_iterated_dim_ir")
            .named_dimension_with_mapping("Region", &["a", "b"], "State")
            .named_dimension("State", &["s1", "s2"])
            .array_aux_direct("x", vec!["Region".into()], "100", None)
            .array_aux_direct("target", vec!["State".into()], "x[State] * 0.5", None);

        with_ir(&project, |_db, ir, _aggs| {
            let sites = sites_for(ir, "x", "target");
            assert_eq!(sites.len(), 1, "sites: {sites:?}");
            assert_eq!(
                sites[0].shape,
                RefShape::Bare,
                "a reverse-declared positionally-mapped iterated subscript is \
                 a same-element reference -- Bare (GH #757)"
            );
            assert_eq!(sites[0].routing, SiteRouting::Direct);
        });
    }

    /// GH #525 (T6): a MIXED iterated+literal subscript (`pop[Region, young]`
    /// inside an A2A-over-`Region` equation) classifies
    /// `RefShape::PerElement` with one `AxisRead` per source axis --
    /// `Iterated` for the position-matched dimension index, `Pinned` for the
    /// literal element -- in declared-axis order.
    #[test]
    fn ir_mixed_iterated_literal_subscript_is_per_element() {
        use crate::ltm_agg::AxisRead;
        let project = TestProject::new("per_element_ir")
            .named_dimension("Region", &["a", "b"])
            .named_dimension("Age", &["young", "old"])
            .array_aux_direct("pop", vec!["Region".into(), "Age".into()], "100", None)
            .array_aux_direct(
                "row_sum",
                vec!["Region".into()],
                "pop[Region, young] + pop[Region, old]",
                None,
            );

        with_ir(&project, |_db, ir, _aggs| {
            let sites = sites_for(ir, "pop", "row_sum");
            assert_eq!(sites.len(), 2, "sites: {sites:?}");
            assert_eq!(
                sites[0].shape,
                RefShape::PerElement {
                    axes: vec![
                        AxisRead::Iterated {
                            dim: "region".to_string(),
                            source_dim: "region".to_string(),
                        },
                        AxisRead::Pinned("young".to_string()),
                    ],
                },
            );
            assert_eq!(
                sites[1].shape,
                RefShape::PerElement {
                    axes: vec![
                        AxisRead::Iterated {
                            dim: "region".to_string(),
                            source_dim: "region".to_string(),
                        },
                        AxisRead::Pinned("old".to_string()),
                    ],
                },
            );
            assert_eq!(sites[0].routing, SiteRouting::Direct);
            assert_eq!(sites[1].routing, SiteRouting::Direct);
        });
    }

    /// The `PerElement` canonicalization boundary: an all-`Pinned` subscript
    /// stays `FixedIndex` (so every existing per-element link-score NAME is
    /// untouched), and a direct wildcard-bearing mix stays on its coarse
    /// classification (a non-reducer reference never collapses an axis).
    #[test]
    fn ir_per_element_canonicalization_boundaries() {
        let project = TestProject::new("per_element_bounds_ir")
            .named_dimension("Region", &["a", "b"])
            .named_dimension("Age", &["young", "old"])
            .array_aux_direct("pop", vec!["Region".into(), "Age".into()], "100", None)
            .array_aux_direct(
                "out",
                vec!["Region".into()],
                "pop[a, young] + SIZE(pop[Region, *])",
                None,
            );

        with_ir(&project, |_db, ir, _aggs| {
            let sites = sites_for(ir, "pop", "out");
            assert_eq!(sites.len(), 2, "sites: {sites:?}");
            assert_eq!(
                sites[0].shape,
                RefShape::FixedIndex(vec!["a".to_string(), "young".to_string()]),
                "all-Pinned canonicalizes to FixedIndex"
            );
            assert_eq!(
                sites[1].shape,
                RefShape::Wildcard,
                "an iterated+wildcard mix keeps its coarse classification -- \
                 the Reduced post-filter rejects it from the per-axis family \
                 and `classify_subscript_shape`'s any-wildcard rule says \
                 Wildcard (SIZE never sets in_reducer, so the #514 \
                 reclassification doesn't fire either)"
            );
        });
    }

    /// A *position-mismatched* iterated subscript is NOT Bare: `row_sum` over
    /// `D1`, `growth` over `D1 x D2`, `growth[D1,D2] = row_sum[D2] * c`. Index
    /// `D2` doesn't match `row_sum`'s declared dimension `D1` (and `D2`
    /// doesn't map to `D1`), so it's a genuine cross-element reference and
    /// stays `DynamicIndex` (Phase 4 territory, not Phase 3).
    #[test]
    fn ir_position_mismatched_iterated_dim_stays_dynamic() {
        let project = TestProject::new("position_mismatch_ir")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .array_aux_direct("row_sum", vec!["D1".into()], "100", None)
            .array_aux_direct(
                "growth",
                vec!["D1".into(), "D2".into()],
                "row_sum[D2] * 0.5",
                None,
            );

        with_ir(&project, |_db, ir, _aggs| {
            let sites = sites_for(ir, "row_sum", "growth");
            assert_eq!(sites.len(), 1, "sites: {sites:?}");
            assert_eq!(
                sites[0].shape,
                RefShape::DynamicIndex,
                "row_sum[D2] inside growth[D1,D2] is a position-mismatched \
                 cross-element reference -- not Bare"
            );
        });
    }

    /// A *partially*-iterated subscript (one index iterated, one literal) is
    /// out of scope for Phase 3 -- it keeps its current `FixedIndex`-or-
    /// `DynamicIndex` classification (Phase 4 handles sliced reducers).
    /// `matrix` over `D1 x D2`, `growth` over `D1 x D2`,
    /// `growth[D1,D2] = matrix[D1, x] * c` (literal `x` in the second slot):
    /// not all-iterated, so not Bare; the literal element makes it
    /// `DynamicIndex` (a partial-fixed subscript classifies conservatively).
    #[test]
    fn ir_partially_iterated_dim_subscript_not_bare() {
        let project = TestProject::new("partial_iterated_ir")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "100", None)
            .array_aux_direct(
                "growth",
                vec!["D1".into(), "D2".into()],
                "matrix[D1, x] * 0.5",
                None,
            );

        with_ir(&project, |_db, ir, _aggs| {
            let sites = sites_for(ir, "matrix", "growth");
            assert_eq!(sites.len(), 1, "sites: {sites:?}");
            assert_ne!(
                sites[0].shape,
                RefShape::Bare,
                "a partially-iterated subscript (`matrix[D1, x]`) is not the \
                 all-iterated same-element case Phase 3 recognizes"
            );
        });
    }

    /// #514: the not-hoistable dynamic-index reducer carve-out, observed at
    /// the IR level. `x[Region] = SUM(pop[idx, *])` with `idx` a scalar aux
    /// (a non-literal index): `enumerate_agg_nodes` declines to hoist (the
    /// `idx` axis isn't statically describable -- see
    /// `ltm_agg::tests::dynamic_index_reducer_subexpression_is_not_hoisted`),
    /// so `model_ltm_reference_sites` reclassifies the `(pop, x)` reducer-arg
    /// site from `Wildcard` to `DynamicIndex` and leaves it `Direct` (not
    /// `ThroughAgg`) -- the conservative cross-product, never the agg path.
    #[test]
    fn ir_dynamic_index_reducer_site_is_direct_dynamic_index() {
        let project = TestProject::new("dynamic_index_reducer_ir")
            .named_dimension("Region", &["NYC", "Boston"])
            .named_dimension("Age", &["Adult", "Child"])
            .array_aux_direct("pop", vec!["Region".into(), "Age".into()], "10", None)
            .scalar_aux("idx", "1")
            .array_aux_direct("x", vec!["Region".into()], "SUM(pop[idx, *])", None);

        with_ir(&project, |_db, ir, aggs| {
            assert!(
                aggs.aggs.iter().all(|a| !a.reads_var("pop")),
                "the dynamic-index reducer must not be hoisted; got: {:?}",
                aggs.aggs
            );
            let sites = sites_for(ir, "pop", "x");
            assert_eq!(sites.len(), 1, "sites: {sites:?}");
            assert_eq!(
                sites[0].shape,
                RefShape::DynamicIndex,
                "a not-hoistable dynamic-index reducer arg is reclassified \
                 from Wildcard to DynamicIndex"
            );
            assert_eq!(
                sites[0].routing,
                SiteRouting::Direct,
                "an unhoisted reducer arg stays on the conservative direct \
                 path, never routed through an agg"
            );
        });
    }
}

// ── Layer 3: the per-occurrence enumeration (Track A2a) ────────────────────
//
// The `occurrences` view is the finer, per-reference-occurrence enumeration
// over a whole target equation that the ceteris-paribus transform will consume
// in A2b. Each test drives a hard shape and pins the one field it adds; every
// pin would fail without the corresponding addition.
mod occurrence_ir_tests {
    use super::*;
    use crate::datamodel;
    use crate::db::ltm_ir::{OccurrenceAxis, OccurrenceRef, OccurrenceSite};
    use crate::testutils::{x_aux, x_flow, x_model, x_module, x_stock};

    /// Sync `datamodel`, run `model_ltm_reference_sites`, and return the
    /// occurrence stream for `target` in `model_name` (empty if none).
    fn occ_from_datamodel(
        datamodel: &datamodel::Project,
        model_name: &str,
        target: &str,
    ) -> Vec<OccurrenceSite> {
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, datamodel);
        let model = sync.models[model_name].source;
        let proj = sync.project;
        let ir = model_ltm_reference_sites(&db, model, proj);
        ir.occurrences.get(target).cloned().unwrap_or_default()
    }

    /// `occ_from_datamodel` for the `main` model of a `TestProject`.
    fn occ_of(project: &TestProject, target: &str) -> Vec<OccurrenceSite> {
        occ_from_datamodel(&project.build_datamodel(), "main", target)
    }

    /// Run `model_ltm_reference_sites` for `project`'s `main` model and hand the
    /// full IR plus `target`'s occurrence stream to `body` (for tests that need
    /// to cross-check the occurrence view against the per-edge `sites` view).
    fn with_ir_and_occ(
        project: &TestProject,
        target: &str,
        body: impl FnOnce(&LtmReferenceSitesResult, &[OccurrenceSite]),
    ) {
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let model = sync.models["main"].source;
        let proj = sync.project;
        let ir = model_ltm_reference_sites(&db, model, proj);
        let occs = ir.occurrences.get(target).cloned().unwrap_or_default();
        body(ir, &occs);
    }

    fn var_occs<'a>(occs: &'a [OccurrenceSite], name: &str) -> Vec<&'a OccurrenceSite> {
        occs.iter()
            .filter(|o| matches!(&o.reference, OccurrenceRef::Variable(v) if v == name))
            .collect()
    }

    #[test]
    fn enumerates_every_occurrence_with_stable_distinct_site_ids() {
        // `relative_pop[Region] = population / population[nyc]`: two occurrences
        // of `population` -- the bare numerator (document-first) then the
        // FixedIndex denominator -- enumerated per occurrence over the whole
        // equation, with distinct, deterministic `SiteId`s.
        let project = TestProject::new("main")
            .named_dimension("Region", &["nyc", "boston"])
            .array_aux("population[Region]", "100")
            .array_aux("relative_pop[Region]", "population / population[nyc]");
        let occs = occ_of(&project, "relative_pop");
        let pop = var_occs(&occs, "population");
        assert_eq!(pop.len(), 2, "occs: {occs:?}");
        assert_eq!(pop[0].shape, RefShape::Bare, "numerator is document-first");
        assert_eq!(pop[1].shape, RefShape::FixedIndex(vec!["nyc".to_string()]));
        assert_ne!(
            pop[0].site_id, pop[1].site_id,
            "two same-source occurrences must have distinct SiteIds"
        );
        // Determinism (a salsa requirement): a fresh build yields the identical
        // stream, SiteIds included.
        let occs2 = occ_of(&project, "relative_pop");
        assert_eq!(
            occs, occs2,
            "the occurrence stream (including SiteIds) must be deterministic"
        );
    }

    #[test]
    fn reducer_enclosure_surfaced_on_an_unhoisted_reducer() {
        // `z = SUM(pop[idx, *])` with `idx` a scalar: the reducer is NOT hoisted
        // (dynamic index), yet the occurrence must still surface `in_reducer`
        // (the #517/#779 "inside a scalar reducer" fact the per-edge
        // `ClassifiedSite` folds away by reclassifying Wildcard->DynamicIndex and
        // dropping the bit).
        let project = TestProject::new("main")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .array_aux("pop[D1, D2]", "1")
            .scalar_aux("idx", "1")
            .scalar_aux("z", "SUM(pop[idx, *])");
        with_ir_and_occ(&project, "z", |ir, occs| {
            let pop = var_occs(occs, "pop");
            assert_eq!(pop.len(), 1, "occs: {occs:?}");
            let o = pop[0];
            assert_eq!(
                o.shape,
                RefShape::Wildcard,
                "the occurrence keeps the RAW walker shape, unlike ClassifiedSite"
            );
            assert!(o.in_reducer, "the SUM argument sits inside a reducer");
            // The per-edge view folds the enclosure away: it reclassifies to
            // DynamicIndex, discarding the reducer-enclosure bit.
            let sites = ir.sites.get(&("pop".to_string(), "z".to_string())).unwrap();
            assert!(
                sites.iter().any(|s| s.shape == RefShape::DynamicIndex),
                "ClassifiedSite reclassifies the unhoisted Wildcard to DynamicIndex"
            );
        });
    }

    #[test]
    fn hoisted_reducer_occurrence_keeps_the_raw_wildcard_shape() {
        // `share[Region] = pop / SUM(pop[*])`: the bare numerator is Bare; the
        // SUM's wildcard arg is hoisted, and its occurrence KEEPS the raw
        // `Wildcard` shape where the per-edge view reclassifies. Routing lives
        // exclusively on the per-EDGE `ClassifiedSite` now (the occurrence copy
        // duplicated the same GH #793 narrowing), so `ThroughAgg` is asserted
        // there.
        let project = TestProject::new("main")
            .named_dimension("Region", &["nyc", "boston"])
            .array_aux("pop[Region]", "100")
            .array_aux("share[Region]", "pop / SUM(pop[*])");
        with_ir_and_occ(&project, "share", |ir, occs| {
            let pop = var_occs(occs, "pop");
            assert_eq!(pop.len(), 2, "occs: {occs:?}");
            let bare = pop.iter().find(|o| !o.in_reducer).expect("bare numerator");
            assert_eq!(bare.shape, RefShape::Bare);
            let reduced = pop.iter().find(|o| o.in_reducer).expect("SUM argument");
            assert_eq!(
                reduced.shape,
                RefShape::Wildcard,
                "the raw walker shape is preserved on the occurrence"
            );
            let sites = ir
                .sites
                .get(&("pop".to_string(), "share".to_string()))
                .expect("the pop -> share edge");
            assert!(
                sites
                    .iter()
                    .any(|s| matches!(s.routing, SiteRouting::ThroughAgg { .. })),
                "the hoisted reducer read routes through its synthetic agg: {sites:?}"
            );
        });
    }

    #[test]
    fn index_nested_marks_subscript_index_occurrence() {
        // `to = SUM(w[from]) + from`: the `from` inside `w[from]` is reachable
        // ONLY through a subscript index (`index_nested`), while the bare `from`
        // outside is not -- the distinction the transform needs to name the
        // normalizer and to whole-freeze a reducer whose only live occurrence is
        // index-nested (Q4).
        let project = TestProject::new("main")
            .named_dimension("Region", &["nyc", "boston"])
            .array_aux("w[Region]", "1")
            .scalar_aux("from", "1")
            .scalar_aux("to", "SUM(w[from]) + from");
        with_ir_and_occ(&project, "to", |_ir, occs| {
            let froms = var_occs(occs, "from");
            assert_eq!(froms.len(), 2, "occs: {occs:?}");
            let nested = froms
                .iter()
                .find(|o| o.index_nested)
                .expect("the from inside w[from]");
            assert!(nested.in_reducer, "w[from] sits inside SUM");
            let bare = froms
                .iter()
                .find(|o| !o.index_nested)
                .expect("the bare `+ from`");
            assert!(!bare.in_reducer, "the bare `from` is outside the reducer");
        });
    }

    #[test]
    fn mismatched_iterated_axis_is_distinct_from_dynamic() {
        // A transposed subscript `arr[D2, D1]` (arr declared `[D1, D2]`) inside
        // an A2A-over-`[D1, D2]` target: the coarse shape collapses to
        // DynamicIndex (unchanged), but the per-axis record marks each axis
        // `MismatchedIterated`, so `derive_other_dep_verdict`'s
        // `Mismatch` is derivable from the IR (distinct from a genuine
        // dynamic index below).
        let project = TestProject::new("main")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .array_aux("arr[D1, D2]", "1")
            .array_aux_direct(
                "t",
                vec!["D1".to_string(), "D2".to_string()],
                "arr[D2, D1] * 2",
                None,
            );
        with_ir_and_occ(&project, "t", |_ir, occs| {
            let arr = var_occs(occs, "arr");
            assert_eq!(arr.len(), 1, "occs: {occs:?}");
            assert_eq!(
                arr[0].shape,
                RefShape::DynamicIndex,
                "coarse shape unchanged"
            );
            assert_eq!(
                arr[0].axes,
                vec![
                    OccurrenceAxis::MismatchedIterated {
                        dim: "d2".to_string()
                    },
                    OccurrenceAxis::MismatchedIterated {
                        dim: "d1".to_string()
                    },
                ],
                "a transposed iterated subscript is per-axis MismatchedIterated"
            );
        });

        // A genuinely dynamic index (`pp[k + 1]`) is `Dynamic`, not
        // MismatchedIterated -- the distinction the transform must act on.
        let dyn_project = TestProject::new("main")
            .indexed_dimension("Idx", 3)
            .array_aux("pp[Idx]", "1")
            .scalar_aux("k", "1")
            .array_aux_direct("dref", vec!["Idx".to_string()], "pp[k + 1]", None);
        with_ir_and_occ(&dyn_project, "dref", |_ir, occs| {
            let pp = var_occs(occs, "pp");
            assert_eq!(pp.len(), 1, "occs: {occs:?}");
            assert_eq!(pp[0].shape, RefShape::DynamicIndex);
            assert_eq!(
                pp[0].axes,
                vec![OccurrenceAxis::Dynamic],
                "an arithmetic index is Dynamic, never MismatchedIterated"
            );
        });
    }

    /// A multi-output module whose parent aux reads TWO of its output ports:
    /// the module-qualified live channel `db::module_link_score_equation`
    /// selects today via a per-process-random HashSet `.find()` (GH #971). The
    /// occurrence stream enumerates both composites in document order so A2b has
    /// a deterministic IR source of truth.
    fn two_output_module_project() -> datamodel::Project {
        datamodel::Project {
            name: "two_output".to_string(),
            sim_specs: datamodel::SimSpecs {
                start: 0.0,
                stop: 1.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: datamodel::SimMethod::Euler,
                time_units: None,
            },
            dimensions: vec![],
            units: vec![],
            models: vec![
                x_model(
                    "main",
                    vec![
                        x_stock("level", "50", &[], &["adjustment"], None),
                        // `combined` reads out_a THEN out_b (document order).
                        x_aux("combined", "multi_out.out_a + multi_out.out_b", None),
                        x_flow("adjustment", "combined / 5", None),
                        x_module("multi_out", &[("level", "multi_out.input")], None),
                    ],
                ),
                x_model(
                    "multi_out",
                    vec![
                        datamodel::Variable::Aux(datamodel::Aux {
                            ident: "input".to_string(),
                            equation: datamodel::Equation::Scalar("0".to_string()),
                            documentation: String::new(),
                            units: None,
                            gf: None,
                            ai_state: None,
                            uid: None,
                            compat: datamodel::Compat {
                                can_be_module_input: true,
                                ..datamodel::Compat::default()
                            },
                        }),
                        x_aux("out_a", "input * 2", None),
                        x_aux("out_b", "input * 3", None),
                    ],
                ),
            ],
            source: None,
            ai_information: None,
        }
    }

    #[test]
    fn module_output_occurrences_recorded_in_document_order() {
        let project = two_output_module_project();
        let occs = occ_from_datamodel(&project, "main", "combined");
        let module_occs: Vec<(String, String, String)> = occs
            .iter()
            .filter_map(|o| match &o.reference {
                OccurrenceRef::ModuleOutput {
                    module,
                    port,
                    composite,
                } => Some((module.clone(), port.clone(), composite.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            module_occs,
            vec![
                (
                    "multi_out".to_string(),
                    "out_a".to_string(),
                    "multi_out\u{00B7}out_a".to_string()
                ),
                (
                    "multi_out".to_string(),
                    "out_b".to_string(),
                    "multi_out\u{00B7}out_b".to_string()
                ),
            ],
            "both module-output composites are enumerated in document order \
             (out_a before out_b); no ClassifiedSite exists for these"
        );
    }

    /// An ARRAYED user-module output referenced by an iterated-dim subscript
    /// as a NON-live dep of an A2A target (finding 3 / byte-parity restore).
    /// The walker classifies a subscripted `module·port` composite's axes
    /// EXACTLY like a model-variable subscript's: `module·port` is never a
    /// variable key, so `from_dims` is empty and a bare iterated-dim index
    /// (`Region`) lands `MismatchedIterated`. With a non-variable head the dep
    /// arity is `None`, so `derive_other_dep_verdict` permissively COLLAPSES
    /// the subscript -- the ceteris-paribus wrap then rewrites
    /// `mod·out[Region]` to a bare `PREVIOUS(mod·out)`, matching the retired
    /// Expr0 `classify_other_dep_iterated_dim_subscript`. The pre-fix stage-2
    /// state pushed EMPTY `axes` for a module-output subscript, which derived
    /// `NotIterated` and froze the uncompilable dim-name subscript verbatim --
    /// a silent divergence no corpus fixture covered (both suites stayed
    /// byte-green either way).
    #[test]
    fn subscripted_arrayed_module_output_axes_derive_collapse() {
        use datamodel::{Aux, Equation, Variable};
        let arrayed_aux = |ident: &str, eqn: &str, can_input: bool| -> Variable {
            Variable::Aux(Aux {
                ident: ident.to_string(),
                equation: Equation::ApplyToAll(vec!["Region".to_string()], eqn.to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat {
                    can_be_module_input: can_input,
                    ..datamodel::Compat::default()
                },
            })
        };
        let project = datamodel::Project {
            name: "arrayed_mod".to_string(),
            sim_specs: datamodel::SimSpecs {
                start: 0.0,
                stop: 1.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: datamodel::SimMethod::Euler,
                time_units: None,
            },
            dimensions: vec![datamodel::Dimension::named(
                "Region".to_string(),
                vec!["nyc".to_string(), "boston".to_string()],
            )],
            units: vec![],
            models: vec![
                x_model(
                    "main",
                    vec![
                        arrayed_aux("live", "1", false),
                        // A2A over `Region`, referencing the arrayed module
                        // output by an iterated-dim subscript as a non-live dep.
                        arrayed_aux("combined", "live[Region] + sub.out[Region]", false),
                        x_module("sub", &[("live", "sub.input")], None),
                    ],
                ),
                x_model(
                    "sub",
                    vec![
                        arrayed_aux("input", "0", true),
                        arrayed_aux("out", "input[Region] * 2", false),
                    ],
                ),
            ],
            source: None,
            ai_information: None,
        };
        let occs = occ_from_datamodel(&project, "main", "combined");
        let mod_occ = occs
            .iter()
            .find(|o| matches!(&o.reference, OccurrenceRef::ModuleOutput { .. }))
            .unwrap_or_else(|| {
                panic!("expected a ModuleOutput occurrence for sub·out; occs: {occs:?}")
            });
        assert!(
            !mod_occ.axes.is_empty(),
            "a subscripted module output must carry classified axes (not empty), \
             else the verdict silently derives NotIterated: {mod_occ:?}"
        );
        // Dep arity is `None` (a `module·port` composite is not a variable key),
        // so the verdict permissively collapses -- byte-parity with HEAD.
        assert_eq!(
            derive_other_dep_verdict(&mod_occ.axes, None, 1),
            OtherDepVerdict::Collapse,
            "an iterated-dim subscript on an unthreadable module output collapses"
        );
    }

    /// The dominant production shape: an IMPLICIT stdlib expansion. `smoothed =
    /// SMTH1(input, 5) * 2` desugars (in the builtins visitor) to an implicit
    /// `Variable::Module` named `$⁚smoothed⁚0⁚smth1` whose `·output` composite
    /// the rewritten parent equation reads. Where
    /// `module_output_occurrences_recorded_in_document_order` pins an EXPLICIT
    /// author-written multi-output module, this pins the implicit path -- and it
    /// needs its own pin because it ADDITIONALLY depends on
    /// `reconstruct_model_variables`' implicit-var loop reconstructing that
    /// SMOOTH-expanded `Variable::Module`. `module_output_parts` only enumerates
    /// a `·`-composite whose head resolves to a module-kind variable in the
    /// reconstructed map; if a future change to that loop stopped rebuilding the
    /// implicit modules, the composite head would go unresolved and this channel
    /// would empty silently for the case that dominates real models -- with the
    /// explicit-module pin still green. Direct routing: the composite read sits
    /// outside any reducer.
    #[test]
    fn implicit_stdlib_module_output_occurrence_recorded_in_document_order() {
        let project = TestProject::new("main")
            .scalar_aux("input", "3")
            .scalar_aux("smoothed", "SMTH1(input, 5) * 2");
        let occs = occ_of(&project, "smoothed");
        let module_occs: Vec<(String, String, String)> = occs
            .iter()
            .filter_map(|o| match &o.reference {
                OccurrenceRef::ModuleOutput {
                    module,
                    port,
                    composite,
                } => Some((module.clone(), port.clone(), composite.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            module_occs,
            vec![(
                "$\u{205A}smoothed\u{205A}0\u{205A}smth1".to_string(),
                "output".to_string(),
                "$\u{205A}smoothed\u{205A}0\u{205A}smth1\u{00B7}output".to_string(),
            )],
            "the SMTH1 expansion's `·output` composite is enumerated exactly once \
             as a ModuleOutput occurrence; no ClassifiedSite exists for it: {occs:?}"
        );
        let module_occ = occs
            .iter()
            .find(|o| matches!(&o.reference, OccurrenceRef::ModuleOutput { .. }))
            .expect("the module-output occurrence");
        assert!(
            !module_occ.in_reducer,
            "the composite read is not inside a reducer"
        );
    }

    #[test]
    fn element_selector_index_is_not_a_causal_occurrence() {
        // Runtime pin: `pick = population[nyc]` reads the ELEMENT even though a
        // variable `nyc = 2` exists. Element interpretation -> population's nyc
        // slot (100); a variable-index interpretation would read population[2]
        // (boston, 200). Simulation confirms 100.
        let project = TestProject::new("main")
            .with_sim_time(0.0, 1.0, 1.0)
            .named_dimension("Region", &["nyc", "boston"])
            .array_with_ranges_direct(
                "population",
                vec!["Region".to_string()],
                vec![("nyc", "100"), ("boston", "200")],
                None,
            )
            .scalar_aux("nyc", "2")
            .scalar_aux("pick", "population[nyc]");
        assert_eq!(
            project.vm_result_incremental("pick")[0],
            100.0,
            "execution reads the element population[nyc]=100, not variable nyc"
        );
        // Edge-set pin: the walker and the ceteris-paribus transform now AGREE
        // the element selector is not a site, keeping the occurrence stream A2b
        // live-selects from faithful to execution. This is NOT the removal of a
        // consumer-visible causal edge: variable-level dep extraction
        // (`variable.rs` `ClassifyVisitor::is_dimension_or_element`, over all
        // project dims) already filtered the element-colliding index ident, so
        // the pre-fix walker's `nyc -> pick` site was an orphan no keyed
        // consumer ever read -- no `nyc -> pick` edge existed before either.
        // Only `population -> pick` FixedIndex remains.
        with_ir_and_occ(&project, "pick", |ir, occs| {
            assert!(
                var_occs(occs, "nyc").is_empty(),
                "the element selector `nyc` in population[nyc] is not a causal \
                 occurrence: {occs:?}"
            );
            assert!(
                !ir.sites
                    .contains_key(&("nyc".to_string(), "pick".to_string())),
                "no spurious nyc -> pick causal edge"
            );
            assert!(
                ir.sites
                    .contains_key(&("population".to_string(), "pick".to_string())),
                "the real population -> pick edge remains"
            );
        });
    }

    #[test]
    fn colliding_element_index_skipped_but_bare_variable_kept() {
        // `collide[Region] = population[nyc] * nyc`: the index `nyc` is an
        // element selector (skipped), but the bare `* nyc` is a genuine variable
        // reference (kept). Exactly one `nyc` occurrence, and it is not
        // index-nested.
        let project = TestProject::new("main")
            .named_dimension("Region", &["nyc", "boston"])
            .scalar_aux("nyc", "3")
            .array_aux("population[Region]", "100")
            .array_aux("collide[Region]", "population[nyc] * nyc");
        with_ir_and_occ(&project, "collide", |ir, occs| {
            let nyc = var_occs(occs, "nyc");
            assert_eq!(
                nyc.len(),
                1,
                "only the bare `* nyc` is causal, not the element selector: {occs:?}"
            );
            assert!(
                !nyc[0].index_nested,
                "the surviving nyc occurrence is the bare multiplication"
            );
            let sites = ir
                .sites
                .get(&("nyc".to_string(), "collide".to_string()))
                .expect("the bare nyc -> collide edge");
            assert_eq!(sites.len(), 1, "one Bare nyc site, not the pre-fix two");
            assert_eq!(sites[0].shape, RefShape::Bare);
        });
    }

    // ── Other-dep verdict derivation (the contract on `axes`) ──────────────
    //
    // `OccurrenceAxis`'s rustdoc states the rule by which the transform derives
    // `derive_other_dep_verdict`'s
    // `Collapse`/`Mismatch`/`NotIterated` from an occurrence's `axes`.
    // Track A3 stage 2 promoted the reference derivation to the production
    // `db::ltm_ir::derive_other_dep_verdict` -- the Expr0-side classifier now
    // builds `axes` from the parsed subscript and DELEGATES to it, so the wrap
    // and the IR cannot drift. The tests below exercise that promoted helper
    // directly (via `use super::*`), and the two corner tests pin the `axes` the
    // walker actually produces for the arity shapes where a rule keyed on the
    // per-axis arms ALONE (all-`Iterated` ⇒ Collapse, any-`MismatchedIterated`
    // ⇒ Mismatch) diverges from the transform. Deriving the verdict requires the
    // two arity facts the occurrence does not itself carry -- the dep's declared
    // arity and the target's iterated-dim count -- both of which the transform
    // holds.

    fn iterated(dim: &str) -> OccurrenceAxis {
        OccurrenceAxis::Iterated {
            dim: dim.to_string(),
            source_dim: dim.to_string(),
        }
    }

    fn mismatched(dim: &str) -> OccurrenceAxis {
        OccurrenceAxis::MismatchedIterated {
            dim: dim.to_string(),
        }
    }

    #[test]
    fn other_dep_verdict_rule_covers_every_branch() {
        // Natural equal-arity iterated subscript (`arr[D1,D2]` for arr [D1,D2],
        // target [D1,D2]): all `Iterated`, arity matches ⇒ Collapse.
        assert_eq!(
            derive_other_dep_verdict(&[iterated("d1"), iterated("d2")], Some(2), 2),
            OtherDepVerdict::Collapse,
        );
        // Transposed equal-arity (`arr[D2,D1]`): a `MismatchedIterated` axis
        // with matching arity ⇒ Mismatch (the GH #526 wrong-element freeze).
        assert_eq!(
            derive_other_dep_verdict(&[mismatched("d2"), mismatched("d1")], Some(2), 2),
            OtherDepVerdict::Mismatch,
        );
        // Under-arity (corner a): all `Iterated` but fewer indices than the
        // dep's declared arity ⇒ Mismatch, NOT the Collapse the all-`Iterated`
        // arms alone would give.
        assert_eq!(
            derive_other_dep_verdict(&[iterated("d1")], Some(2), 2),
            OtherDepVerdict::Mismatch,
        );
        // Over-target-arity (corner b): more indices than the target has
        // iterated dims ⇒ NotIterated, NOT the Mismatch the `MismatchedIterated`
        // arm alone would give.
        assert_eq!(
            derive_other_dep_verdict(&[iterated("d1"), mismatched("d1")], Some(2), 1),
            OtherDepVerdict::NotIterated,
        );
        // A non-iterated axis (a `Pinned` literal / `Dynamic` index) anywhere ⇒
        // NotIterated: not an iterated-dim subscript at all.
        assert_eq!(
            derive_other_dep_verdict(
                &[iterated("d1"), OccurrenceAxis::Pinned("young".to_string())],
                Some(2),
                2,
            ),
            OtherDepVerdict::NotIterated,
        );
        assert_eq!(
            derive_other_dep_verdict(&[OccurrenceAxis::Dynamic], Some(1), 1),
            OtherDepVerdict::NotIterated,
        );
        // Un-threadable dep (declared dims unknown) keeps the permissive
        // collapse regardless of the per-axis arms.
        assert_eq!(
            derive_other_dep_verdict(&[iterated("d1"), mismatched("d2")], None, 2),
            OtherDepVerdict::Collapse,
        );
    }

    #[test]
    fn under_arity_iterated_subscript_is_mismatch_not_collapse() {
        // Corner (a): `arr[D1]` for arr declared [D1,D2], inside an
        // A2A-over-[D1,D2] target. The single index lines up with arr's first
        // axis, so the occurrence's axes are all-`Iterated` -- yet the dep's
        // declared arity is 2, so
        // `derive_other_dep_verdict` returns `Mismatch`
        // (`axes.len() != dep_arity`, checked before the per-axis arms), NOT the
        // Collapse a rule keyed on the per-axis arms alone would derive.
        // Freezing the wrong element here is the GH #526 silent magnitude error.
        let project = TestProject::new("main")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .array_aux("arr[D1, D2]", "1")
            .array_aux_direct(
                "t",
                vec!["D1".to_string(), "D2".to_string()],
                "arr[D1] * 2",
                None,
            );
        with_ir_and_occ(&project, "t", |_ir, occs| {
            let arr = var_occs(occs, "arr");
            assert_eq!(arr.len(), 1, "occs: {occs:?}");
            assert_eq!(
                arr[0].shape,
                RefShape::DynamicIndex,
                "coarse shape unchanged"
            );
            assert_eq!(
                arr[0].axes,
                vec![iterated("d1")],
                "the single lined-up index is `Iterated`"
            );
            // dep arity 2, target iterated-dim count 2.
            assert_eq!(
                derive_other_dep_verdict(&arr[0].axes, Some(2), 2),
                OtherDepVerdict::Mismatch,
                "under-arity must derive Mismatch, not Collapse"
            );
        });
    }

    #[test]
    fn over_target_arity_iterated_subscript_is_not_iterated() {
        // Corner (b): `arr[D1,D1]` for arr declared [D1,D2], inside an
        // A2A-over-[D1] target. Position 0 lines up (`Iterated`); position 1's
        // `D1` names the source's `D2` axis, so it is `MismatchedIterated`.
        // But the subscript has MORE indices (2) than the target has iterated
        // dims (1), so `derive_other_dep_verdict` short-circuits
        // to `NotIterated` (`axes.len() > target_iterated_count`), NOT the Mismatch
        // the `MismatchedIterated` arm alone would derive.
        let project = TestProject::new("main")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .array_aux("arr[D1, D2]", "1")
            .array_aux_direct("t", vec!["D1".to_string()], "arr[D1, D1] * 2", None);
        with_ir_and_occ(&project, "t", |_ir, occs| {
            let arr = var_occs(occs, "arr");
            assert_eq!(arr.len(), 1, "occs: {occs:?}");
            assert_eq!(
                arr[0].shape,
                RefShape::DynamicIndex,
                "coarse shape unchanged"
            );
            assert_eq!(
                arr[0].axes,
                vec![iterated("d1"), mismatched("d1")],
                "position 1's D1 names the source's D2 axis: MismatchedIterated"
            );
            // dep arity 2, target iterated-dim count 1.
            assert_eq!(
                derive_other_dep_verdict(&arr[0].axes, Some(2), 1),
                OtherDepVerdict::NotIterated,
                "an over-target-arity subscript must derive NotIterated, not Mismatch"
            );
        });
    }

    #[test]
    fn verdict_ignores_over_arity_axis_labeling() {
        // Track A3 stage 2 / finding-3 verdict-equivalence. The Expr0-side
        // `#[cfg(test)]` axis builder (`ltm_augment::other_dep_occurrence_axes`)
        // and the IR walker (`classify_occurrence_axes`) can LABEL a per-axis index
        // differently at exactly ONE kind of position: an index that overflows
        // the dep's declared arity. The mirror's `dep_dims.get(i) == None` arm
        // marks it `Iterated{d,d}`; the IR's `source_dims.get(i) == None` arm
        // marks the SAME over-arity index `MismatchedIterated{d}`. Every
        // in-arity position agrees (both derivations gate the lineup on the
        // identical `ltm_agg::iterated_axis_slot_elements`; see
        // `classify_axis_access` vs `other_dep_axis_lines_up`).
        //
        // But an over-declared-arity position exists only when `axes.len() >
        // dep_arity`, and the arity check in `derive_other_dep_verdict` returns
        // `Mismatch` for that whole case BEFORE inspecting the per-axis arms. So
        // the sole labeling difference is dominated: the verdict cannot differ
        // between the two derivations. Production now reads `axes` straight off
        // the occurrence IR (one classifier family), so this only guards the
        // `#[cfg(test)]` Expr0 axis builder (`other_dep_occurrence_axes`) that
        // reconstructs occurrences for the text-level wrap unit tests: it stays
        // VERDICT-equivalent to `classify_occurrence_axes` even where the two
        // label an over-arity axis differently, so the reconstructed occurrence
        // is a faithful stand-in.
        //
        // 3 indices, dep declared arity 2 (index 2 overflows), target
        // iterated-dim count 3.
        let mirror = [iterated("d1"), iterated("d2"), iterated("d3")];
        let ir = [iterated("d1"), iterated("d2"), mismatched("d3")];
        assert_ne!(
            mirror[2], ir[2],
            "the two families label the over-arity index differently"
        );
        assert_eq!(
            derive_other_dep_verdict(&mirror, Some(2), 3),
            derive_other_dep_verdict(&ir, Some(2), 3),
            "the arity guard dominates the labeling difference: same verdict either way"
        );
        assert_eq!(
            derive_other_dep_verdict(&ir, Some(2), 3),
            OtherDepVerdict::Mismatch,
            "axes.len() (3) != dep arity (2) => Mismatch"
        );
    }
}

// ── Layer 4: the LTM front door (GH #978 / #979) ───────────────────────────
//
// `SiteId` occurrence identity is a path of `u16` child indices. Three of the
// walk's `push` sites take a count that the AST node's own shape does NOT bound
// -- and they are exactly the three arms of `SiteWidthAxis`:
//
//   | axis                | what varies                        | reachable via         |
//   |---------------------|------------------------------------|-----------------------|
//   | `ArrayedSlots`      | `Ast::Arrayed` per-element slots   | any `<element>` list  |
//   | `BuiltinContents`   | one builtin call's contents        | `MEAN` (the only      |
//   |                     |                                    | variadic `BuiltinFn`) |
//   | `SubscriptIndices`  | one subscript's index list         | any `x[a, b, ...]`    |
//
// Every OTHER push is a literal fixed by the node (`Ast::Scalar`/`ApplyToAll`'s
// single slot `0`, `Op1`'s one operand, `Op2`'s two, `Expr2::If`'s three, an
// `IndexExpr2::Range`'s two halves), which is why they have no arm and no test
// row: `expr_site_width_rejection`'s `match` is exhaustive with no catch-all, so
// a new variant with a variable child count is a compile error there.
//
// Each axis gets ONE fixture asserted in BOTH directions -- admitted at the
// production limit, refused when the limit is lowered below its width -- so the
// accept half cannot pass vacuously.
mod front_door_tests {
    use super::*;
    use crate::db::ltm_ir::{
        MAX_SITE_CHILDREN, SiteChildrenLimitGuard, SiteWidthAxis, model_ltm_reference_sites,
    };

    /// Sync `project` onto a FRESH db and hand `model_ltm_reference_sites`' full
    /// result for `main` to `body`. A fresh db per call is required: the query is
    /// salsa-memoized, so re-running it on one db under a different
    /// `SiteChildrenLimitGuard` would return the first limit's answer.
    fn with_ir(project: &TestProject, body: impl FnOnce(&LtmReferenceSitesResult)) {
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        body(model_ltm_reference_sites(
            &db,
            sync.models["main"].source,
            sync.project,
        ));
    }

    /// An `Ast::Arrayed` target with three per-element equations, so the walk
    /// numbers four slots (three elements plus the trailing default slot
    /// `ArrayedSlotMap` addresses unconditionally).
    fn arrayed_slots_fixture() -> TestProject {
        TestProject::new("main")
            .named_dimension("Region", &["a", "b", "c"])
            .array_aux("population[Region]", "100")
            .array_flow_with_ranges(
                "births[Region]",
                vec![
                    ("a", "population[a] * 0.1"),
                    ("b", "population[b] * 0.2"),
                    ("c", "population[c] * 0.3"),
                ],
            )
    }

    /// A three-argument `MEAN`, the only variadic `BuiltinFn`.
    fn builtin_contents_fixture() -> TestProject {
        TestProject::new("main")
            .scalar_aux("a", "1")
            .scalar_aux("b", "2")
            .scalar_aux("c", "3")
            .scalar_aux("avg", "MEAN(a, b, c)")
    }

    /// A two-index subscript. The index count is bounded by the AST alone:
    /// `Expr2::from` does not narrow it to the subscripted variable's declared
    /// arity (it simply ignores indices past `dims.len()`), which is why this
    /// axis needs the front door despite GH #979 claiming it was bounded.
    fn subscript_indices_fixture() -> TestProject {
        TestProject::new("main")
            .named_dimension("D1", &["x", "y"])
            .named_dimension("D2", &["p", "q"])
            .array_aux("matrix[D1,D2]", "1")
            .array_aux("total[D1,D2]", "matrix[D1, D2] * 2")
    }

    /// Every axis is admitted at the production limit: an ordinary model records
    /// its occurrences and carries no rejection. This is the control for the
    /// three refusal tests below -- each uses the SAME fixture with the limit
    /// lowered, so the refusal cannot be an artifact of the fixture.
    #[test]
    fn production_limit_admits_every_axis() {
        for (name, project, target) in [
            ("arrayed slots", arrayed_slots_fixture(), "births"),
            ("builtin contents", builtin_contents_fixture(), "avg"),
            ("subscript indices", subscript_indices_fixture(), "total"),
        ] {
            with_ir(&project, |ir| {
                assert_eq!(
                    ir.site_width_rejection, None,
                    "{name}: an ordinary model must pass the front door"
                );
                assert!(
                    ir.occurrences.contains_key(target),
                    "{name}: `{target}` must record its occurrences; got {:?}",
                    ir.occurrences.keys().collect::<Vec<_>>()
                );
            });
        }
    }

    /// The boundary itself, on all three axes: an equation needing EXACTLY
    /// `limit` children is fully addressable (indices `0 ..= limit - 1`) and must
    /// be ADMITTED. The comparison is `>`, and off-by-one to `>=` is a real
    /// hazard in the direction this session says to measure rather than reason
    /// about: it would refuse a legal equation and cost the whole model every
    /// link, loop and pathway score, silently.
    ///
    /// These rows reuse the refusal tests' own fixtures, one limit higher, so
    /// each axis is asserted at `n == limit` (accept) and `n == limit + 1`
    /// (refuse) against the same equation. Without them, `>` -> `>=` on the
    /// builtin and subscript axes left the entire suite green.
    #[test]
    fn a_width_exactly_at_the_limit_is_admitted_on_every_axis() {
        // ArrayedSlots: 3 element equations + the reserved default slot = 4.
        {
            let _guard = SiteChildrenLimitGuard::new(4);
            with_ir(&arrayed_slots_fixture(), |ir| {
                assert_eq!(
                    ir.site_width_rejection, None,
                    "4 slots at limit 4 are all addressable (0..=3)"
                );
                assert!(ir.occurrences.contains_key("births"));
            });
        }
        // BuiltinContents: `MEAN(a, b, c)` yields exactly 3 contents.
        {
            let _guard = SiteChildrenLimitGuard::new(3);
            with_ir(&builtin_contents_fixture(), |ir| {
                assert_eq!(
                    ir.site_width_rejection, None,
                    "3 builtin contents at limit 3 are all addressable (0..=2)"
                );
                assert!(ir.occurrences.contains_key("avg"));
            });
        }
        // SubscriptIndices: `matrix[D1, D2]` carries exactly 2 indices.
        {
            let _guard = SiteChildrenLimitGuard::new(2);
            with_ir(&subscript_indices_fixture(), |ir| {
                assert_eq!(
                    ir.site_width_rejection, None,
                    "2 subscript indices at limit 2 are all addressable (0..=1)"
                );
                assert!(ir.occurrences.contains_key("total"));
            });
        }
    }

    #[test]
    fn lowered_limit_refuses_an_over_wide_arrayed_slot_count() {
        // Three element equations need four slots; a limit of three cannot tell
        // the fourth apart from the first.
        let _guard = SiteChildrenLimitGuard::new(3);
        with_ir(&arrayed_slots_fixture(), |ir| {
            let rejection = ir
                .site_width_rejection
                .as_ref()
                .expect("a 4-slot target must be refused at limit 3");
            assert_eq!(rejection.variable, "births");
            assert_eq!(rejection.axis, SiteWidthAxis::ArrayedSlots);
            assert_eq!(rejection.count, 4, "3 element equations plus the default");
            assert_eq!(rejection.limit, 3);

            // The occurrence view records NOTHING for the refused equation, so
            // no two of its references can share a `SiteId`...
            assert!(
                !ir.occurrences.contains_key("births"),
                "a refused equation must mint no SiteId; got {:?}",
                ir.occurrences.get("births")
            );
            // ...while the per-edge view stays complete AND keeps its real
            // shapes. A missing entry there is read as a single `Bare` site by
            // `model_element_causal_edges`, which would MISCLASSIFY these
            // literal-element reads and emit wrong element edges.
            let edge = ir
                .sites
                .get(&("population".to_string(), "births".to_string()))
                .expect("the per-edge view is path-free and must survive refusal");
            assert_eq!(edge.len(), 3, "one site per element equation: {edge:?}");
            for site in edge {
                assert!(
                    matches!(site.shape, RefShape::FixedIndex(_)),
                    "the edge must keep its real shape, not degrade to Bare: {site:?}"
                );
            }
        });
    }

    #[test]
    fn lowered_limit_refuses_an_over_arity_builtin() {
        let _guard = SiteChildrenLimitGuard::new(2);
        with_ir(&builtin_contents_fixture(), |ir| {
            let rejection = ir
                .site_width_rejection
                .as_ref()
                .expect("a 3-argument MEAN must be refused at limit 2");
            assert_eq!(rejection.variable, "avg");
            assert_eq!(rejection.axis, SiteWidthAxis::BuiltinContents);
            assert_eq!(rejection.count, 3);
            assert!(!ir.occurrences.contains_key("avg"));
            // The edges into `avg` survive with their real shapes.
            for from in ["a", "b", "c"] {
                let edge = ir
                    .sites
                    .get(&(from.to_string(), "avg".to_string()))
                    .unwrap_or_else(|| panic!("edge {from}->avg must survive refusal"));
                assert_eq!(edge.len(), 1, "{from}: {edge:?}");
            }
        });
    }

    #[test]
    fn lowered_limit_refuses_an_over_wide_subscript() {
        let _guard = SiteChildrenLimitGuard::new(1);
        with_ir(&subscript_indices_fixture(), |ir| {
            let rejection = ir
                .site_width_rejection
                .as_ref()
                .expect("a 2-index subscript must be refused at limit 1");
            assert_eq!(rejection.variable, "total");
            assert_eq!(rejection.axis, SiteWidthAxis::SubscriptIndices);
            assert_eq!(rejection.count, 2);
            assert!(!ir.occurrences.contains_key("total"));
            let edge = ir
                .sites
                .get(&("matrix".to_string(), "total".to_string()))
                .expect("edge matrix->total must survive refusal");
            assert_eq!(edge.len(), 1, "{edge:?}");
            assert_eq!(
                edge[0].shape,
                RefShape::Bare,
                "`matrix[D1,D2]` in an A2A-over-[D1,D2] body is the same-element read"
            );
        });
    }

    /// The premise that makes `SubscriptIndices` a genuinely unbounded axis --
    /// verified by running the lowering, not by reading it.
    ///
    /// GH #979 asserts this axis is "bounded by the reference's declared
    /// subscript arity". It is not: `Expr2::from`'s `Expr1::Subscript` arm
    /// collects EVERY index and only consults `dims[i]` while `i < dims.len()`,
    /// so a subscript with more indices than the variable declares reaches the
    /// IR with all of them. (`classify_occurrence_axes` already accounts for
    /// that shape, calling it an arity mismatch.) If a future change narrows the
    /// index list during lowering, this reds -- and the axis, with its front-door
    /// arm, can be retired.
    #[test]
    fn a_subscript_keeps_more_indices_than_the_variable_declares() {
        let _guard = SiteChildrenLimitGuard::new(1);
        let project = TestProject::new("main")
            .indexed_dimension("D", 3)
            .array_aux("arr[D]", "1")
            // `arr` declares ONE dimension; this reference carries two indices.
            .scalar_aux("target", "arr[1, 2]");
        with_ir(&project, |ir| {
            let rejection = ir
                .site_width_rejection
                .as_ref()
                .expect("an over-arity subscript must reach the IR with both indices");
            assert_eq!(rejection.variable, "target");
            assert_eq!(rejection.axis, SiteWidthAxis::SubscriptIndices);
            assert_eq!(
                rejection.count, 2,
                "both indices survive lowering, though `arr` declares one dimension"
            );
        });
    }

    /// The check must find an over-wide node wherever it sits, so it has to
    /// descend every child edge it can descend. The rows below are ELEVEN, one
    /// per recursive call in `expr_site_width_rejection` -- `Op1`'s operand,
    /// `Op2`'s two, `If`'s three, an `App`'s `Expr` content and its
    /// `LookupTable` content, a `Subscript`'s `IndexExpr2::Expr` and its
    /// `Range`'s two halves. (`Const`/`Var` are leaves; `Wildcard`, `StarRange`
    /// and `DimPosition` carry no `Expr2`. Neither has a row because neither has
    /// a call.) Deleting any one descent reds exactly its row.
    ///
    /// The `LookupTable` and `Range` rows exist because a first version of this
    /// test had 8 rows while the checker had 11 calls, and deleting either
    /// descent left the whole suite green. The `LookupTable` row is the one that
    /// matters most: that descent is not conservatism, it is the only coverage of
    /// a path the CONSUMER builds and the producer walk never does (see
    /// `ast_site_width_rejection`'s rustdoc).
    #[test]
    fn the_check_descends_every_expression_edge() {
        // A 3-argument `MEAN` is over-wide at limit 2; every other node in these
        // equations has at most 2 children, so a row can only fail by failing to
        // reach the `MEAN`. (`LOOKUP`'s two contents and a `Range`'s two halves
        // are each 2, exactly at the limit.)
        let rows: &[(&str, &str)] = &[
            ("Op1 operand", "-MEAN(a, b, c)"),
            ("Op2 left", "MEAN(a, b, c) + 1"),
            ("Op2 right", "1 + MEAN(a, b, c)"),
            ("If cond", "IF MEAN(a, b, c) > 0 THEN 1 ELSE 2"),
            ("If then", "IF a > 0 THEN MEAN(a, b, c) ELSE 2"),
            ("If else", "IF a > 0 THEN 1 ELSE MEAN(a, b, c)"),
            ("App Expr content", "ABS(MEAN(a, b, c))"),
            ("App LookupTable content", "LOOKUP(MEAN(a, b, c), 1)"),
            ("Subscript IndexExpr2::Expr", "arr[MEAN(a, b, c)]"),
            ("Subscript Range low", "SUM(arr[MEAN(a, b, c):3])"),
            ("Subscript Range high", "SUM(arr[1:MEAN(a, b, c)])"),
        ];
        for (edge, equation) in rows {
            let _guard = SiteChildrenLimitGuard::new(2);
            let project = TestProject::new("main")
                .indexed_dimension("D", 3)
                .array_aux("arr[D]", "1")
                .scalar_aux("a", "1")
                .scalar_aux("b", "2")
                .scalar_aux("c", "3")
                .scalar_aux("target", equation);
            with_ir(&project, |ir| {
                let rejection = ir
                    .site_width_rejection
                    .as_ref()
                    .unwrap_or_else(|| panic!("{edge}: `{equation}` must be refused at limit 2"));
                assert_eq!(rejection.variable, "target", "{edge}");
                assert_eq!(rejection.axis, SiteWidthAxis::BuiltinContents, "{edge}");
                assert_eq!(rejection.count, 3, "{edge}");
            });
        }
    }

    /// The same, one row per `Ast` shape: the check runs on whichever equation
    /// form the target carries, including an `Ast::Arrayed`'s per-element AND
    /// default expressions (which `ast_site_width_rejection` walks separately
    /// from the slot count itself).
    #[test]
    fn the_check_descends_every_equation_shape() {
        // Limit 3 with a FOUR-argument `MEAN`, so the arrayed rows' own slot
        // counts (at most 2 elements plus the default = 3) stay inside the limit
        // and the rejection can only come from descending into the equation.
        const OVER_WIDE: &str = "MEAN(a, b, c, d)";
        let leaves = |p: TestProject| {
            p.scalar_aux("a", "1")
                .scalar_aux("b", "2")
                .scalar_aux("c", "3")
                .scalar_aux("d", "4")
        };
        let scalar = leaves(TestProject::new("main")).scalar_aux("target", OVER_WIDE);
        let apply_to_all = leaves(TestProject::new("main").named_dimension("Region", &["x", "y"]))
            .array_aux("target[Region]", OVER_WIDE);
        // A per-element equation, in the slot the walk numbers first.
        let arrayed_element =
            leaves(TestProject::new("main").named_dimension("Region", &["x", "y"]))
                .array_flow_with_ranges("target[Region]", vec![("x", OVER_WIDE), ("y", "1")]);
        // The default (EXCEPT) equation, the slot AFTER the last element.
        let arrayed_default =
            leaves(TestProject::new("main").named_dimension("Region", &["x", "y"]))
                .array_with_default_and_overrides("target[Region]", OVER_WIDE, vec![("x", "1")]);

        for (shape, project) in [
            ("Ast::Scalar", scalar),
            ("Ast::ApplyToAll", apply_to_all),
            ("Ast::Arrayed element", arrayed_element),
            ("Ast::Arrayed default", arrayed_default),
        ] {
            let _guard = SiteChildrenLimitGuard::new(3);
            with_ir(&project, |ir| {
                let rejection = ir
                    .site_width_rejection
                    .as_ref()
                    .unwrap_or_else(|| panic!("{shape}: must be refused at limit 3"));
                assert_eq!(rejection.variable, "target", "{shape}");
                assert_eq!(rejection.axis, SiteWidthAxis::BuiltinContents, "{shape}");
                assert_eq!(rejection.count, 4, "{shape}");
            });
        }
    }

    /// An over-wide equation that references NO model variable must still be
    /// refused, and the refusal must name the canonically-FIRST offender.
    ///
    /// Both halves guard the driver rather than the checker. The walk skips a
    /// target with neither reference sites nor occurrences, so capturing the
    /// verdict after that skip would silently admit a model whose widest
    /// equation happens to be constant-only; and the first-offender pick is what
    /// makes the emitted diagnostic reproducible across processes, which a
    /// `HashMap`-ordered scan would not be.
    #[test]
    fn a_reference_free_over_wide_equation_is_refused_and_names_the_first_offender() {
        let _guard = SiteChildrenLimitGuard::new(2);
        let project = TestProject::new("main")
            // Canonically after `avg_early`, and also over-wide.
            .scalar_aux("zz_avg_late", "MEAN(1, 2, 3)")
            .scalar_aux("avg_early", "MEAN(4, 5, 6)");
        with_ir(&project, |ir| {
            let rejection = ir
                .site_width_rejection
                .as_ref()
                .expect("a constant-only MEAN(1, 2, 3) is still an over-wide equation");
            assert_eq!(
                rejection.variable, "avg_early",
                "the canonically-first offender must be reported"
            );
            assert_eq!(rejection.axis, SiteWidthAxis::BuiltinContents);
        });
    }

    /// The production limit is the whole `u16` range: with no value reserved as
    /// a sentinel, child `u16::MAX` is an ordinary, addressable child.
    #[test]
    fn the_production_limit_is_the_whole_u16_range() {
        assert_eq!(MAX_SITE_CHILDREN, u16::MAX as usize + 1);
        assert_eq!(u16::try_from(MAX_SITE_CHILDREN - 1), Ok(u16::MAX));
    }

    /// A refusal must reach the user AND stop LTM generation for the model: the
    /// two halves of "refuse LTM for this model with a diagnostic". Asserted in
    /// both directions on one fixture, so neither half can pass vacuously.
    #[test]
    fn refusal_emits_no_ltm_vars_and_warns_through_the_diagnostic_surface() {
        use crate::db::{
            DiagnosticError, DiagnosticSeverity, collect_model_diagnostics, model_ltm_variables,
        };
        use salsa::Setter;

        // A stock/flow feedback loop whose flow equation is a 3-argument `MEAN`:
        // scoreable at the production limit, over-wide at limit 2.
        let fixture = || {
            TestProject::new("main")
                .stock("level", "100", &["inflow"], &[], None)
                .flow("inflow", "MEAN(level, drain, boost)", None)
                .scalar_aux("drain", "1")
                .scalar_aux("boost", "2")
        };

        let width_warnings = |db: &SimlinDb, model, project| -> Vec<String> {
            collect_model_diagnostics(db, model, project)
                .into_iter()
                .filter(|d| d.severity == DiagnosticSeverity::Warning)
                .filter_map(|d| match d.error {
                    DiagnosticError::Assembly(msg)
                        if msg.contains("LTM analysis was skipped for this model") =>
                    {
                        Some(msg)
                    }
                    _ => None,
                })
                .collect()
        };

        // Control: at the production limit the model IS scored and no width
        // warning is emitted.
        {
            let mut db = SimlinDb::default();
            let (project, model) = {
                let sync = sync_from_datamodel(&db, &fixture().build_datamodel());
                (sync.project, sync.models["main"].source)
            };
            project.set_ltm_enabled(&mut db).to(true);
            assert!(
                !model_ltm_variables(&db, model, project).vars.is_empty(),
                "the control fixture must be scoreable, or the refusal below proves nothing"
            );
            assert!(
                width_warnings(&db, model, project).is_empty(),
                "no width warning at the production limit"
            );
        }

        // Refusal: no LTM variable at all, plus a Warning naming the variable.
        {
            let _guard = SiteChildrenLimitGuard::new(2);
            let mut db = SimlinDb::default();
            let (project, model) = {
                let sync = sync_from_datamodel(&db, &fixture().build_datamodel());
                (sync.project, sync.models["main"].source)
            };
            project.set_ltm_enabled(&mut db).to(true);
            assert!(
                model_ltm_variables(&db, model, project).vars.is_empty(),
                "a refused model must emit no link, loop, or pathway score"
            );
            let warnings = width_warnings(&db, model, project);
            assert_eq!(
                warnings.len(),
                1,
                "exactly one width refusal must reach collect_model_diagnostics; got {warnings:?}"
            );
            assert!(
                warnings[0].contains("inflow"),
                "the warning must name the offending variable: {}",
                warnings[0]
            );
            // ...and the noun phrase must be the FIRED axis's, not another
            // arm's. Spelled literally rather than as `describe()` so a swap of
            // two arms' strings cannot move both sides of the comparison.
            assert!(
                warnings[0].contains("arguments to one builtin call"),
                "the warning must describe the axis that actually fired: {}",
                warnings[0]
            );
        }
    }

    /// `SiteWidthAxis::describe()` is a three-arm decision whose strings are the
    /// user-facing noun phrase in the refusal `Warning`, so it gets one row per
    /// arm with the text spelled literally. Nothing else in the suite would
    /// notice two arms being swapped: the end-to-end assertion above pins the
    /// arm-to-message wiring, and these pin the strings themselves.
    #[test]
    fn every_axis_describes_itself() {
        let rows = [
            (
                SiteWidthAxis::ArrayedSlots,
                "equation slots (per-element equations plus a default)",
            ),
            (
                SiteWidthAxis::BuiltinContents,
                "arguments to one builtin call",
            ),
            (SiteWidthAxis::SubscriptIndices, "indices in one subscript"),
        ];
        for (axis, expected) in rows {
            assert_eq!(axis.describe(), expected, "{axis:?}");
        }
        // Distinct, so a message cannot describe two axes the same way.
        let described: std::collections::HashSet<&str> =
            rows.iter().map(|(a, _)| a.describe()).collect();
        assert_eq!(described.len(), rows.len());
    }

    /// A refusal costs the SCORES, not the STRUCTURE.
    ///
    /// This is the batch's central claim about blast radius, and it is what makes
    /// a whole-model refusal an acceptable trade: the per-edge `sites` view is
    /// name-keyed and path-free, so it is untouched by a width refusal, and the
    /// two surfaces that read it -- `model_edge_shapes` and the loop enumeration
    /// behind `model_detected_loops` -- keep answering exactly as before. If a
    /// later change routes either surface through the occurrence stream, or makes
    /// the refusal empty `sites`, this reds and the trade has to be re-argued.
    ///
    /// The fixture is ARRAYED with literal-element reads on purpose. Both
    /// consumers default a MISSING per-edge entry to a single `Bare` site, so on
    /// a scalar model "the view survived" and "the view was emptied and
    /// defaulted" are indistinguishable and the test would pass under a
    /// `sites.clear()` -- verified: an earlier scalar version of this test did.
    /// `FixedIndex` shapes are what make the difference observable.
    #[test]
    fn a_refusal_leaves_edge_shapes_and_detected_loops_unchanged() {
        use crate::db::{model_detected_loops, model_edge_shapes};

        // `level[Region]` -> `growth[Region]` -> `level[Region]`: a feedback loop
        // whose per-element flow equations read the stock by LITERAL element, so
        // the edge carries `FixedIndex` sites rather than `Bare`. Three element
        // equations need four slots, so it is refused at limit 3.
        let fixture = || {
            TestProject::new("main")
                .named_dimension("Region", &["a", "b", "c"])
                .array_stock("level[Region]", "100", &["growth"], &[], None)
                .array_flow_with_ranges(
                    "growth[Region]",
                    vec![
                        ("a", "level[a] * 0.1"),
                        ("b", "level[b] * 0.1"),
                        ("c", "level[c] * 0.1"),
                    ],
                )
        };
        let structure = || {
            let db = SimlinDb::default();
            let sync = sync_from_datamodel(&db, &fixture().build_datamodel());
            let (model, project) = (sync.models["main"].source, sync.project);
            let loops: Vec<(String, Vec<String>)> = model_detected_loops(&db, model, project)
                .loops
                .iter()
                .map(|l| (l.id.clone(), l.variables.clone()))
                .collect();
            (loops, model_edge_shapes(&db, model, project).clone())
        };

        let (loops_ok, shapes_ok) = structure();
        assert!(
            !loops_ok.is_empty(),
            "the fixture must detect a loop, or the comparison below is vacuous"
        );
        // The guard that makes the comparison able to fail: at least one edge
        // must carry a NON-`Bare` shape, since `Bare` is exactly what a cleared
        // view degrades to.
        assert!(
            shapes_ok
                .edge_shapes
                .values()
                .any(|s| s.iter().any(|shape| !matches!(shape, RefShape::Bare))),
            "the fixture must carry a non-Bare edge shape: {:?}",
            shapes_ok.edge_shapes
        );

        let (loops_refused, shapes_refused) = {
            let _guard = SiteChildrenLimitGuard::new(3);
            // Confirm the refusal really fires for this fixture at this limit.
            with_ir(&fixture(), |ir| {
                assert!(ir.site_width_rejection.is_some(), "refusal must fire");
            });
            structure()
        };

        assert_eq!(
            loops_refused, loops_ok,
            "loop DETECTION reads the path-free per-edge view and must be unaffected"
        );
        assert_eq!(
            shapes_refused, shapes_ok,
            "`model_edge_shapes` reads the same view and must be byte-equal"
        );
    }
}
