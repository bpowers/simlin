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
    /// gates on `mapped_element_correspondence` (both declaration
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
    use crate::db::ltm_ir::{OccurrenceAxis, OccurrenceRef, OccurrenceRouting, OccurrenceSite};
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
    fn reducer_enclosure_surfaced_even_when_routing_is_direct() {
        // `z = SUM(pop[idx, *])` with `idx` a scalar: the reducer is NOT hoisted
        // (dynamic index), so the occurrence routes `Direct` -- yet it must
        // still surface `in_reducer` + `reducer_keys` (the #517/#779
        // "inside a scalar reducer" fact the per-edge `ClassifiedSite` folds
        // away by reclassifying Wildcard->DynamicIndex and dropping the bit).
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
            assert_eq!(o.reducer_keys.len(), 1, "one enclosing reducer");
            assert_eq!(
                o.routing,
                OccurrenceRouting::Direct,
                "the dynamic-index reducer is not hoisted"
            );
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
    fn hoisted_reducer_occurrence_routes_through_agg() {
        // `share[Region] = pop / SUM(pop[*])`: the bare numerator is Direct/Bare;
        // the SUM's wildcard arg is hoisted, so its occurrence routes ThroughAgg
        // while KEEPING the raw Wildcard shape.
        let project = TestProject::new("main")
            .named_dimension("Region", &["nyc", "boston"])
            .array_aux("pop[Region]", "100")
            .array_aux("share[Region]", "pop / SUM(pop[*])");
        with_ir_and_occ(&project, "share", |_ir, occs| {
            let pop = var_occs(occs, "pop");
            assert_eq!(pop.len(), 2, "occs: {occs:?}");
            let bare = pop.iter().find(|o| !o.in_reducer).expect("bare numerator");
            assert_eq!(bare.routing, OccurrenceRouting::Direct);
            assert_eq!(bare.shape, RefShape::Bare);
            let reduced = pop.iter().find(|o| o.in_reducer).expect("SUM argument");
            assert!(
                matches!(reduced.routing, OccurrenceRouting::ThroughAgg { .. }),
                "a hoisted reducer occurrence routes through its synthetic agg"
            );
            assert_eq!(
                reduced.shape,
                RefShape::Wildcard,
                "the raw walker shape is preserved on the occurrence"
            );
        });
    }

    #[test]
    fn already_lagged_marks_previous_and_init_contents() {
        // `to = from + PREVIOUS(g) + INIT(h)`: `from` is live/unlagged; the
        // occurrences inside PREVIOUS and INIT are `already_lagged` (so the
        // transform will not re-wrap and double-lag them).
        let project = TestProject::new("main")
            .scalar_aux("from", "1")
            .scalar_aux("g", "2")
            .scalar_aux("h", "3")
            .scalar_aux("to", "from + PREVIOUS(g) + INIT(h)");
        with_ir_and_occ(&project, "to", |_ir, occs| {
            let get = |name: &str| {
                var_occs(occs, name)
                    .first()
                    .copied()
                    .unwrap_or_else(|| panic!("no occurrence for {name}: {occs:?}"))
            };
            assert!(!get("from").already_lagged, "the bare `from` is not lagged");
            assert!(get("g").already_lagged, "g sits inside PREVIOUS");
            assert!(get("h").already_lagged, "h sits inside INIT");
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
        assert_eq!(
            module_occ.routing,
            OccurrenceRouting::Direct,
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

/// F3: a `SiteId` child index must never WRAP. The addressable range is pinned
/// at its exact boundary, so a variadic builtin's 65,536th content declines to
/// be addressed rather than re-using child 0's path.
///
/// Pinned on the pure boundary function rather than by building a 65,536-argument
/// `MEAN` call: the fixture would be enormous and slow (the 3-minute suite cap),
/// while the boundary is the entire property. The consequence of getting this
/// wrong is silent -- a wrapped index makes the wrap's lookup return a different
/// occurrence, so it holds or freezes the wrong reference and emits a plausible
/// wrong score -- which is why the guard returns `None` instead of a wrapped
/// value, and why the walker records no occurrence for such a child.
#[test]
fn site_child_index_declines_rather_than_wrapping() {
    use super::site_child_index;

    assert_eq!(site_child_index(0), Some(0));
    assert_eq!(site_child_index(1), Some(1));
    // The last addressable child.
    assert_eq!(site_child_index(65_535), Some(u16::MAX));
    // One past it must DECLINE, not wrap to 0 (which would collide with the
    // first child's SiteId and silently mis-address the occurrence).
    assert_eq!(site_child_index(65_536), None);
    assert_eq!(site_child_index(65_537), None);
    assert_eq!(site_child_index(usize::MAX), None);
}
