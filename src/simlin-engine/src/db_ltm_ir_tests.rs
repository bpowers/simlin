// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the LTM reference-site classification IR.
//!
//! Two layers:
//! 1. `collect_reference_sites_tests` -- the `(shape, in_reducer)` contract
//!    per AST site (the Phase-1 regression guards, ported from `db_analysis.rs`).
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
    /// is `Bare` (no new dimension-mapping code -- just don't exclude the
    /// mapped case from the iterated-dim recognition).
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
                aggs.aggs
                    .iter()
                    .all(|a| !a.source_vars.contains(&"pop".to_string())),
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
