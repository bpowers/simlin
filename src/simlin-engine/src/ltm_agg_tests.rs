// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for `ltm_agg`: aggregate-node enumeration, the per-axis access
//! classifier, and the reducer decision table.
//!
//! Split out of `ltm_agg.rs` only to keep that file under the project
//! line-count lint, and mounted with `#[path]` as its `tests` child module, so
//! `use super::*` resolves the parent's private items exactly as an inline
//! `mod tests` did.

use super::*;
use crate::db::{SimlinDb, sync_from_datamodel};
use crate::test_common::TestProject;

/// The synthetic node whose identity -- its spelled reducer, printed -- is
/// `identity`; `None` for a reducer that only ever appears as a variable's
/// whole dt-equation (a variable-backed agg is found through `aggs_in_var`).
fn agg_for_key<'a>(result: &'a AggNodesResult, identity: &str) -> Option<&'a AggNode> {
    result
        .synthetic_by_key
        .get(identity)
        .map(|&i| &result.aggs[i])
}

/// Test helper: the source-variable names of an agg (sorted + deduped
/// by the [`AggNode::sources`] construction invariant).
fn source_names(a: &AggNode) -> Vec<&str> {
    a.sources.iter().map(|s| s.var.as_str()).collect()
}

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

/// Build a `TestProject` and return the GH #791 cartesian-decline verdict
/// for the `from -> to` edge.
fn source_read(project: &TestProject, from: &str, to: &str) -> UnhoistedSourceRead {
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let link = LtmLinkId::new(&db, from.to_string(), to.to_string());
    unhoisted_reducer_source_read(&db, link, sync.models["main"].source, sync.project).clone()
}

/// GH #791: a multi-source reducer whose source read is a STRICT slice
/// (`pop[nyc,*]`, with no full-extent read of `pop`) is the silent-cartesian
/// family -- `StrictSlice` (the caller loud-skips it), carrying the actual
/// slice so the diagnostic renders `pop[nyc,*]` rather than a canned
/// example.
#[test]
fn unhoisted_source_read_strict_slice_for_pinned_only_read() {
    let project = TestProject::new("strict_slice")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux("pop[Region,D2]", "1")
        .array_aux("w[D2]", "0.5")
        .array_aux("share[Region]", "SUM(pop[nyc,*] * w[*])");
    let UnhoistedSourceRead::StrictSlice(slice) = source_read(&project, "pop", "share") else {
        panic!("the pinned-only read must classify StrictSlice");
    };
    assert_eq!(render_read_slice_for_diagnostic(&slice), "nyc,*");
}

/// GH #793: a hoisted full-extent sibling reducer must not mask an
/// un-hoisted strict-slice sibling on the same `pop -> share` edge. The
/// full-extent read is already represented by synthetic agg halves, so the
/// residual un-hoisted verdict remains `StrictSlice`.
#[test]
fn unhoisted_source_read_ignores_hoisted_sibling_full_read() {
    let project = TestProject::new("strict_with_hoisted_sibling")
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
    let UnhoistedSourceRead::StrictSlice(slice) = source_read(&project, "pop", "share") else {
        panic!("the un-hoisted strict sibling must not be masked by the hoisted full read");
    };
    assert_eq!(render_read_slice_for_diagnostic(&slice), "nyc,*");
}

/// GH #791 boundary: the SAME variable read at full extent (`pop[*]`) AND
/// pinned (`pop[north]`) -- the GH #744 self-reference family -- leaves NO
/// row unread, so it is `FullExtent` (the caller keeps the conservative
/// delta-ratio cartesian, unchanged).
#[test]
fn unhoisted_source_read_full_extent_when_full_read_present() {
    let project = TestProject::new("self_ref")
        .named_dimension("region", &["north", "south"])
        .array_aux("pop[region]", "1")
        .scalar_aux("tp", "SUM(pop[*] * pop[north])");
    assert!(matches!(
        source_read(&project, "pop", "tp"),
        UnhoistedSourceRead::FullExtent
    ));
}

/// GH #791 boundary: a pure full-extent multi-source read (`matrix[D1,*]`,
/// `[Iterated, Reduced]`) is `FullExtent` -- the #779 bare-feeder fixture's
/// `matrix -> growth` edge keeps its correct cartesian diagonal.
#[test]
fn unhoisted_source_read_full_extent_for_iterated_reduced() {
    let project = TestProject::new("iter_reduced")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["c", "d"])
        .array_aux("matrix[D1,D2]", "1")
        .array_aux("frac", "0.5")
        .array_aux("growth[D1]", "SUM(matrix[D1,*] * frac)");
    assert!(matches!(
        source_read(&project, "matrix", "growth"),
        UnhoistedSourceRead::FullExtent
    ));
}

/// GH #791 boundary: a dynamic-index reducer (`SUM(pop[idx,*])`, `idx`
/// non-literal) is NOT statically describable -- `NotDescribable`, so the
/// caller keeps the DOCUMENTED conservative cartesian cross-product.
#[test]
fn unhoisted_source_read_not_describable_for_dynamic_index() {
    let project = TestProject::new("dyn_index")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux("pop[Region,D2]", "1")
        .scalar_aux("idx", "2")
        .array_aux("share[Region]", "SUM(pop[idx,*])");
    assert!(matches!(
        source_read(&project, "pop", "share"),
        UnhoistedSourceRead::NotDescribable
    ));
}

/// GH #792: a PER-ELEMENT-EQUATION (`Ast::Arrayed`) owner whose every slot
/// holds a strict-slice multi-source reducer (each `share` slot is
/// `SUM(pop[<region>,*] * w[*])`) classifies the `pop -> share` edge
/// `PerElementReducerRead` -- a decline. The first describable slice in
/// sorted-slot order (`boston`) rides along for the diagnostic.
#[test]
fn unhoisted_source_read_declines_per_element_strict_slots() {
    let project = TestProject::new("per_element_strict")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux("pop[Region,D2]", "1")
        .array_aux("w[D2]", "0.5")
        .array_with_ranges_direct(
            "share",
            vec!["Region".into()],
            vec![
                ("nyc", "SUM(pop[nyc,*] * w[*])"),
                ("boston", "SUM(pop[boston,*] * w[*])"),
            ],
            None,
        );
    let UnhoistedSourceRead::PerElementReducerRead(Some(slice)) =
        source_read(&project, "pop", "share")
    else {
        panic!("per-element strict-slice slots must classify PerElementReducerRead(Some)");
    };
    // Sorted-key walk visits `boston` before `nyc`.
    assert_eq!(render_read_slice_for_diagnostic(&slice), "boston,*");
}

/// GH #792 any-reducer-read rule: ONLY ONE slot reads `pop` (inside a
/// reducer); the other slot does not read `pop` at all. Any slot's reducer
/// read declines the WHOLE edge (the Bare stand-in conflates the slots).
#[test]
fn unhoisted_source_read_declines_when_only_some_slots_read() {
    let project = TestProject::new("per_element_some")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux("pop[Region,D2]", "1")
        .array_aux("w[D2]", "0.5")
        .array_with_ranges_direct(
            "share",
            vec!["Region".into()],
            vec![("nyc", "SUM(pop[nyc,*] * w[*])"), ("boston", "0")],
            None,
        );
    let UnhoistedSourceRead::PerElementReducerRead(Some(slice)) =
        source_read(&project, "pop", "share")
    else {
        panic!("a single reducer-reading slot must decline the whole edge");
    };
    assert_eq!(render_read_slice_for_diagnostic(&slice), "nyc,*");
}

/// GH #792 finding 2: a per-element owner whose every slot reads `pop` at
/// FULL EXTENT inside an I1-declined multi-source reducer
/// (`SUM(pop[*,*] * w[*])`) ALSO declines -- the full-extent verdict only
/// validates the cartesian projection, which needs a single dt-expression
/// a per-element owner does not have; the Bare stand-in is just as wrong
/// for full reads (verified ~-0.0 empirically).
#[test]
fn unhoisted_source_read_declines_per_element_full_extent_slots() {
    let project = TestProject::new("per_element_full")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux("pop[Region,D2]", "1")
        .array_aux("w[D2]", "0.5")
        .array_with_ranges_direct(
            "share",
            vec!["Region".into()],
            vec![
                ("nyc", "SUM(pop[*,*] * w[*])"),
                ("boston", "SUM(pop[*,*] * w[*])"),
            ],
            None,
        );
    let UnhoistedSourceRead::PerElementReducerRead(Some(slice)) =
        source_read(&project, "pop", "share")
    else {
        panic!("per-element full-extent multi-source slots must still decline");
    };
    assert_eq!(render_read_slice_for_diagnostic(&slice), "*,*");
}

/// GH #792 finding 1: the DIM-NAME spelling (`SUM(pop[Region,*] * w[*])`
/// per slot). In a per-element slot no iterated dimension is in scope
/// (mirroring `enumerate_agg_nodes`' Arrayed arm), so the dim-named index
/// is not statically describable -- but the read IS a reducer read, so the
/// edge still declines, with no representative slice for the diagnostic.
/// (Execution pins `Region` to the slot's element -- a strict read -- so
/// the previous `Iterated => full extent` classification was wrong; the
/// executed-value pin lives in the integration twin.)
#[test]
fn unhoisted_source_read_declines_per_element_dim_named_slots() {
    let project = TestProject::new("per_element_dim_named")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux("pop[Region,D2]", "1")
        .array_aux("w[D2]", "0.5")
        .array_with_ranges_direct(
            "share",
            vec!["Region".into()],
            vec![
                ("nyc", "SUM(pop[Region,*] * w[*])"),
                ("boston", "SUM(pop[Region,*] * w[*])"),
            ],
            None,
        );
    assert!(matches!(
        source_read(&project, "pop", "share"),
        UnhoistedSourceRead::PerElementReducerRead(None)
    ));
}

/// GH #792 explicit sub-case decision: a DYNAMIC-INDEX reducer read inside
/// a per-element slot (`SUM(pop[idx,*])`) also declines. Pre-fix it was
/// silently stand-in'd exactly like the strict spelling (the scalar/A2A
/// dynamic-index family keeps its documented conservative cartesian, but a
/// per-element owner has no cartesian arm to keep), so declining is both
/// sound and consistent; no existing test pinned the old silent behavior.
#[test]
fn unhoisted_source_read_declines_per_element_dynamic_index_slots() {
    let project = TestProject::new("per_element_dyn")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux("pop[Region,D2]", "1")
        .scalar_aux("idx", "2")
        .array_with_ranges_direct(
            "share",
            vec!["Region".into()],
            vec![("nyc", "SUM(pop[idx,*])"), ("boston", "SUM(pop[idx,*])")],
            None,
        );
    assert!(matches!(
        source_read(&project, "pop", "share"),
        UnhoistedSourceRead::PerElementReducerRead(None)
    ));
}

/// GH #792 non-decline boundary: a per-element owner that references `pop`
/// only OUTSIDE any reducer (the disjoint-dim FixedIndex family's shape)
/// classifies `NotDescribable` -- no reducer read, so the edge keeps its
/// existing emission path (`try_disjoint_dim_arrayed_link_scores` et al).
#[test]
fn unhoisted_source_read_not_describable_for_per_element_non_reducer_refs() {
    let project = TestProject::new("per_element_bare")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux("pop[Region,D2]", "1")
        .array_with_ranges_direct(
            "share",
            vec!["Region".into()],
            vec![
                ("nyc", "pop[nyc,p] * 0.5"),
                ("boston", "pop[boston,q] * 0.5"),
            ],
            None,
        );
    assert!(matches!(
        source_read(&project, "pop", "share"),
        UnhoistedSourceRead::NotDescribable
    ));
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
    assert_eq!(source_names(agg), vec!["population"]);
    assert!(agg.result_dims.is_empty());
    // `agg_for_key` resolves only synthetic aggs, so it must not find this one.
    assert!(agg_for_key(&result, "sum(population[*])").is_none());
}

/// AC4.3 (arrayed variant): `agg[D1] = SUM(matrix[D1,*])` is whole-RHS, so
/// the variable is the agg; `result_dims` carries `D1` and `read_slice`
/// records the `Iterated(D1)` / `Reduced` axis split (the `D1` axis is
/// iterated over the A2A dimension space, the second axis is reduced).
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
    assert_eq!(source_names(agg), vec!["matrix"]);
    assert_eq!(agg.result_dims, vec!["D1".to_string()]);
    assert_eq!(
        agg.canonical_read_slice(),
        vec![
            AxisRead::Iterated {
                dim: "d1".to_string(),
                source_dim: "d1".to_string()
            },
            AxisRead::Reduced { subset: None }
        ]
    );
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
    assert_eq!(source_names(agg), vec!["pop"]);
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
    assert_eq!(synthetic[0].reducer_key, "sum(pop[*])");
    assert_eq!(source_names(synthetic[0]), vec!["pop"]);
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
    assert_eq!(denom_agg.reducer_key, "sum(pop[*])");

    // `share` must own a *synthetic* agg with the same reducer text.
    let share_agg = result
        .aggs_in_var("share")
        .find(|a| a.is_synthetic)
        .expect("expected a synthetic agg owned by `share`");
    assert_eq!(share_agg.name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
    assert_eq!(share_agg.reducer_key, "sum(pop[*])");
    assert_eq!(source_names(share_agg), vec!["pop"]);
    // `agg_for_key` resolves the reducer text to the *synthetic* agg.
    assert_eq!(
        agg_for_key(&result, "sum(pop[*])").map(|a| a.name.as_str()),
        Some("$\u{205A}ltm\u{205A}agg\u{205A}0")
    );

    // There must be exactly one synthetic agg and exactly one
    // variable-backed agg -- two distinct nodes despite identical text.
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    let var_backed_aggs: Vec<&AggNode> = result.aggs.iter().filter(|a| !a.is_synthetic).collect();
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
    assert_eq!(share_agg.reducer_key, "sum(pop[*])");

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
    assert_eq!(synthetic[0].reducer_key, "sum(a[*])");
    assert_eq!(source_names(synthetic[0]), vec!["a"]);
    assert_eq!(synthetic[1].name, "$\u{205A}ltm\u{205A}agg\u{205A}1");
    assert_eq!(synthetic[1].reducer_key, "sum(b[*])");
    assert_eq!(source_names(synthetic[1]), vec!["b"]);
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
    assert_eq!(synthetic[0].reducer_key, "sum(pop[*])");
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
        synthetic.iter().map(|a| a.reducer_key.as_str()).collect();
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
        agg_for_key(&r1, "sum(a[*])").map(|a| a.name.clone()),
        Some("$\u{205A}ltm\u{205A}agg\u{205A}0".to_string())
    );
    assert_eq!(
        agg_for_key(&r1, "sum(b[*])").map(|a| a.name.clone()),
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
        agg_for_key(&result, "sum(pop[*])").is_some(),
        "the arrayed reducer must be recognized; got: {:?}",
        result.aggs
    );
    assert!(
        agg_for_key(&result, "sum(s)").is_none(),
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

/// AC4.1: a reducer over an explicit *slice* used as a sub-expression
/// (`x[r] = ... + SUM(pop[NYC, *])`) IS hoisted into a synthetic agg --
/// the `read_slice` descriptor records which rows it reads
/// (`[Pinned(nyc), Reduced]` over `pop`'s `[Region, Age]` axes), so the
/// element-graph reroute and the per-element reducer link scores route
/// only those rows. `result_dims` is `[]` here: there is no `Iterated`
/// axis (the `Region` on the target `x` is broadcast; the read is a
/// single row). The `pop[NYC, Adult]` `Direct` reference is separate --
/// not part of the agg.
#[test]
fn slice_reducer_subexpression_is_hoisted() {
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
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(
        synthetic.len(),
        1,
        "a slice-reducer subexpression must mint one synthetic agg; got: {:?}",
        result.aggs
    );
    assert_eq!(synthetic[0].name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
    // `expr2_to_string` puts a space after the comma in a multi-index
    // subscript -- assert the canonical text it actually produces.
    assert_eq!(synthetic[0].reducer_key, "sum(pop[nyc, *])");
    assert_eq!(source_names(synthetic[0]), vec!["pop"]);
    assert_eq!(
        synthetic[0].canonical_read_slice(),
        vec![
            AxisRead::Pinned("nyc".to_string()),
            AxisRead::Reduced { subset: None }
        ]
    );
    assert!(
        synthetic[0].result_dims.is_empty(),
        "no Iterated axis -- result dims must be empty; got: {:?}",
        synthetic[0].result_dims
    );
    assert!(
        result
            .aggs_in_var("x")
            .any(|a| a.name == "$\u{205A}ltm\u{205A}agg\u{205A}0")
    );
}

/// AC4.2: a *partial*-reduce slice over an iterated dimension used as a
/// sub-expression (`x[D1] = ... + SUM(matrix[D1, *])`, `matrix[D1, D2]`,
/// `x` A2A over `D1`) mints an arrayed synthetic agg over `D1`:
/// `read_slice = [Iterated(d1), Reduced]`, `result_dims = [D1]`. The
/// element graph routes `matrix[d1, d2] → agg[d1]`.
#[test]
fn sliced_reducer_over_iterated_dim_mints_arrayed_agg() {
    let project = TestProject::new("iterated_slice_subexpr")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
        .array_aux_direct(
            "x",
            vec!["D1".into()],
            "matrix[a, x] + SUM(matrix[D1, *])",
            None,
        );

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(
        synthetic.len(),
        1,
        "an iterated-dim slice-reducer subexpression must mint one synthetic agg; got: {:?}",
        result.aggs
    );
    assert_eq!(
        synthetic[0].canonical_read_slice(),
        vec![
            AxisRead::Iterated {
                dim: "d1".to_string(),
                source_dim: "d1".to_string()
            },
            AxisRead::Reduced { subset: None }
        ]
    );
    assert_eq!(synthetic[0].result_dims, vec!["D1".to_string()]);
    assert_eq!(source_names(synthetic[0]), vec!["matrix"]);
    // `expr2_to_string` canonicalizes the iterated dim name lowercase.
    assert_eq!(synthetic[0].reducer_key, "sum(matrix[d1, *])");
}

/// #514: a *mixed* read slice -- `Iterated` + `Pinned` + `Reduced` axes
/// on one source. `matrix3d[D1, Region, Age]`, `x` A2A over `D1`,
/// `x[D1] = ... + SUM(matrix3d[D1, NYC, *])`: the first axis is iterated
/// over the target's `D1`, the second is pinned to the literal `NYC`,
/// the third (wildcard) is reduced ⇒ `read_slice = [Iterated(d1),
/// Pinned(nyc), Reduced]`, `result_dims = [D1]` (only the iterated axis
/// shapes the agg). Mints one arrayed synthetic agg over `D1`.
#[test]
fn mixed_pinned_iterated_reduced_slice_mints_arrayed_agg() {
    let project = TestProject::new("mixed_slice_subexpr")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("Region", &["NYC", "Boston"])
        .named_dimension("Age", &["Adult", "Child"])
        .array_aux_direct(
            "matrix3d",
            vec!["D1".into(), "Region".into(), "Age".into()],
            "1",
            None,
        )
        .array_aux_direct(
            "x",
            vec!["D1".into()],
            "matrix3d[a, NYC, Adult] + SUM(matrix3d[D1, NYC, *])",
            None,
        );

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(
        synthetic.len(),
        1,
        "a mixed pinned/iterated/reduced slice must mint one synthetic agg; got: {:?}",
        result.aggs
    );
    assert_eq!(
        synthetic[0].canonical_read_slice(),
        vec![
            AxisRead::Iterated {
                dim: "d1".to_string(),
                source_dim: "d1".to_string()
            },
            AxisRead::Pinned("nyc".to_string()),
            AxisRead::Reduced { subset: None }
        ]
    );
    assert_eq!(synthetic[0].result_dims, vec!["D1".to_string()]);
    assert_eq!(source_names(synthetic[0]), vec!["matrix3d"]);
    assert_eq!(synthetic[0].reducer_key, "sum(matrix3d[d1, nyc, *])");
}

/// #514: a multi-source reducer whose arrayed args agree on their read
/// slice -- `total = 1 + SUM(a[*] + b[*])`, `a`, `b` both over `D`. The
/// reducer's argument expression references two arrayed sources; each
/// reads its whole extent (`[Reduced]`), the slices agree, so one
/// synthetic agg is minted carrying that combined slice and *both* source
/// variables.
#[test]
fn multi_source_reducer_agreeing_slices_mints_one_agg() {
    let project = TestProject::new("multi_source_reducer")
        .named_dimension("D", &["p", "q"])
        .array_aux_direct("a", vec!["D".into()], "1", None)
        .array_aux_direct("b", vec!["D".into()], "2", None)
        .scalar_aux("total", "1 + SUM(a[*] + b[*])");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(
        synthetic.len(),
        1,
        "a multi-source reducer with agreeing slices must mint one synthetic agg; got: {:?}",
        result.aggs
    );
    assert_eq!(
        synthetic[0].canonical_read_slice(),
        vec![AxisRead::Reduced { subset: None }]
    );
    assert!(synthetic[0].result_dims.is_empty());
    // `sources` lists every arrayed model variable in the argument
    // (sorted by name), each carrying the IDENTICAL canonical slice --
    // invariant I1's identical-co-source form (T2 of the
    // shape-expressiveness design: acceptance is identical-only, so
    // per-source slices cannot yet differ).
    assert_eq!(source_names(synthetic[0]), vec!["a", "b"]);
    for s in &synthetic[0].sources {
        assert_eq!(
            s.read_slice,
            vec![AxisRead::Reduced { subset: None }],
            "every arrayed co-source must carry the canonical slice; got {:?} for {}",
            s.read_slice,
            s.var
        );
    }
}

/// #514 (negative guard): a multi-source reducer whose arrayed args read
/// *incompatible* slices is NOT hoisted -- `total = 1 + SUM(a[*] + b[*])`
/// where `a` is over `D1` and `b` is over `D2` (disjoint dims, so
/// `[Reduced]` for `a`'s one axis vs `[Reduced]` for `b`'s -- the slices
/// have the same shape but the *sources* differ in dimensionality; more
/// to the point, a 1-axis `a[*]` and a 2-axis `b[*, *]` disagree on
/// length). Use clearly-different ranks to force the disagreement: `a`
/// over `D1`, `b` over `D1 x D2`. `combined_read_slice` returns `None`,
/// so no agg is minted for this reducer.
#[test]
fn multi_source_reducer_disagreeing_slices_is_not_hoisted() {
    let project = TestProject::new("multi_source_disagree")
        .named_dimension("D1", &["p", "q"])
        .named_dimension("D2", &["x", "y"])
        .array_aux_direct("a", vec!["D1".into()], "1", None)
        .array_aux_direct("b", vec!["D1".into(), "D2".into()], "2", None)
        .scalar_aux("total", "1 + SUM(a[*] + b[*, *])");

    let result = agg_nodes(&project);
    assert!(
        result
            .aggs
            .iter()
            .all(|ag| !ag.reads_var("a") && !ag.reads_var("b")),
        "a multi-source reducer whose args read incompatible slices must not be hoisted; \
         got: {:?}",
        result.aggs
    );
    assert!(result.synthetic_by_key.is_empty());
}

/// GH #534: a sliced reducer whose iterated index lines up with the
/// source's row axis via a *positional dimension mapping*
/// (`matrix[Region, D2]`, `State` over `{s1, s2}` with a `State→Region`
/// mapping, target A2A over `State` with `... + SUM(matrix[State, *])`)
/// IS hoisted: the `Iterated` axis carries the (target, source) dim
/// pair, `result_dims` is the TARGET's iterated dim (`State` -- the
/// dimension the agg variable is arrayed over), and the emitters remap
/// each source row to its positionally-corresponding slot.
/// (`classify_iterated_dim_shape`'s own mapped branch -- a
/// whole-equation-iterated subscript, not a sliced reducer argument --
/// is a separate path and stays `Bare`; see
/// `db::ltm_ir_tests::ir_mapped_iterated_dim_subscript_is_bare`.)
#[test]
fn mapped_iterated_dim_sliced_reducer_is_hoisted_with_pair() {
    let project = TestProject::new("mapped_iterated_slice")
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .array_aux_direct("matrix", vec!["Region".into(), "D2".into()], "1", None)
        .array_aux_direct(
            "out",
            vec!["State".into()],
            "matrix[r1, x] + SUM(matrix[State, *])",
            None,
        );

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(
        synthetic.len(),
        1,
        "a positionally-mapped sliced reducer must mint one synthetic agg; got: {:?}",
        result.aggs
    );
    assert_eq!(
        synthetic[0].canonical_read_slice(),
        vec![
            AxisRead::Iterated {
                dim: "state".to_string(),
                source_dim: "region".to_string()
            },
            AxisRead::Reduced { subset: None }
        ]
    );
    assert_eq!(
        synthetic[0].result_dims,
        vec!["State".to_string()],
        "the agg's result axis is the TARGET equation's iterated dim"
    );
    assert_eq!(source_names(synthetic[0]), vec!["matrix"]);
    assert_eq!(synthetic[0].reducer_key, "sum(matrix[state, *])");
}

/// A sliced reducer over an EXPLICIT element-mapped pair IS hoisted; the
/// slice carries the `(State, Region)` pair and the slot remap follows the
/// map (`mapped_reference_semantics_tests`' `(Permuted, IteratedDim)` cell
/// measures that spelling following the map against the VM;
/// `element_graph_element_mapped_sliced_reducer_remaps_along_the_map` pins the
/// slots).
#[test]
fn element_mapped_sliced_reducer_is_hoisted() {
    let project = TestProject::new("element_mapped_slice")
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension_with_element_mapping(
            "State",
            &["s1", "s2"],
            "Region",
            &[("s1", "r2"), ("s2", "r1")],
        )
        .array_aux_direct("matrix", vec!["Region".into(), "D2".into()], "1", None)
        .array_aux_direct(
            "out",
            vec!["State".into()],
            "1 + SUM(matrix[State, *])",
            None,
        );

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
    assert_eq!(
        synthetic[0].source_read_slice("matrix"),
        &[
            AxisRead::Iterated {
                dim: "state".to_string(),
                source_dim: "region".to_string()
            },
            AxisRead::Reduced { subset: None }
        ]
    );
    assert_eq!(synthetic[0].result_dims, vec!["State".to_string()]);
}

/// GH #757: a sliced reducer whose POSITIONAL mapping is declared only in
/// the REVERSE direction (on the source's `Region` toward `State`) is hoisted
/// -- the executed correspondence accepts both declaration directions (the
/// compiler's `translate_via_mapping` resolves both). The slice and
/// `result_dims` are identical to the forward-declared twin.
#[test]
fn reverse_declared_mapped_sliced_reducer_is_hoisted() {
    let project = TestProject::new("reverse_mapped_slice")
        .named_dimension_with_mapping("Region", &["r1", "r2"], "State")
        .named_dimension("D2", &["x", "y"])
        .named_dimension("State", &["s1", "s2"])
        .array_aux_direct("matrix", vec!["Region".into(), "D2".into()], "1", None)
        .array_aux_direct(
            "out",
            vec!["State".into()],
            "1 + SUM(matrix[State, *])",
            None,
        );

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(
        synthetic.len(),
        1,
        "the reverse-declared positionally-mapped sliced reducer must be hoisted; got: {:?}",
        result.aggs
    );
    assert_eq!(
        synthetic[0].canonical_read_slice(),
        vec![
            AxisRead::Iterated {
                dim: "state".to_string(),
                source_dim: "region".to_string()
            },
            AxisRead::Reduced { subset: None }
        ]
    );
    assert_eq!(synthetic[0].result_dims, vec!["State".to_string()]);
}

/// GH #534: the whole-RHS twin -- `out[State] = SUM(matrix[State,*])`
/// over a positionally-mapped pair mints a SYNTHETIC agg, an exception
/// to the variable-is-the-agg rule for whole-RHS reducers: the
/// variable-backed link-score path (`try_cross_dimensional_link_scores`)
/// matches result axes against source axes by name, so a remapped pair
/// falls off it onto the `Wildcard` per-shape partial, whose
/// PREVIOUS-wrapping mangles the iterated index into a non-compiling
/// `matrix[PREVIOUS(state),*]` (silently stubbed to 0). The synthetic
/// agg gives the whole-RHS case the same remapped two-half scoring as
/// an inline mapped reducer.
#[test]
fn whole_rhs_mapped_partial_reduce_mints_synthetic_agg() {
    let project = TestProject::new("whole_rhs_mapped")
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .array_aux_direct("matrix", vec!["Region".into(), "D2".into()], "1", None)
        .array_aux_direct("out", vec!["State".into()], "SUM(matrix[State, *])", None);

    let result = agg_nodes(&project);
    assert!(
        result.aggs.iter().all(|a| a.is_synthetic),
        "a whole-RHS MAPPED reducer must mint a synthetic agg (not variable-backed); got: {:?}",
        result.aggs
    );
    let agg = result
        .aggs_in_var("out")
        .next()
        .expect("expected a synthetic agg owned by `out`");
    assert_eq!(agg.name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
    assert_eq!(
        agg.canonical_read_slice(),
        vec![
            AxisRead::Iterated {
                dim: "state".to_string(),
                source_dim: "region".to_string()
            },
            AxisRead::Reduced { subset: None }
        ]
    );
    assert_eq!(agg.result_dims, vec!["State".to_string()]);
}

/// GH #764 (T4): the whole-RHS BROADCAST twin -- `out[D1,D3] =
/// SUM(matrix[D1,*])`, `result_dims` (`[D1]`) a strict subset of the
/// owner's dims (`[D1,D3]`) -- mints a SYNTHETIC agg, generalizing the
/// GH #534 carve-out: the variable-backed per-`(row, slot)` machinery
/// requires each slot to name a complete `to` element, which a
/// broadcast slot does not. The synthetic agg is arrayed over
/// `result_dims` and rides the two-half emitters + the GH #528
/// agg-to-target projection.
#[test]
fn whole_rhs_broadcast_partial_reduce_mints_synthetic_agg() {
    let project = TestProject::new("whole_rhs_broadcast_764")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension("D3", &["p", "q"])
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
        .array_aux_direct(
            "out",
            vec!["D1".into(), "D3".into()],
            "SUM(matrix[D1, *])",
            None,
        );

    let result = agg_nodes(&project);
    assert!(
        result.aggs.iter().all(|a| a.is_synthetic),
        "a whole-RHS BROADCAST reducer must mint a synthetic agg (not variable-backed); \
         got: {:?}",
        result.aggs
    );
    let agg = result
        .aggs_in_var("out")
        .next()
        .expect("expected a synthetic agg owned by `out`");
    assert_eq!(agg.name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
    assert_eq!(
        agg.canonical_read_slice(),
        vec![
            AxisRead::Iterated {
                dim: "d1".to_string(),
                source_dim: "d1".to_string()
            },
            AxisRead::Reduced { subset: None }
        ]
    );
    assert_eq!(agg.result_dims, vec!["D1".to_string()]);
    assert_eq!(source_names(agg), vec!["matrix"]);
}

/// GH #764 (T4): the whole-RHS PERMUTED twin -- `out[D2,D1] =
/// SUM(cube[D1,D2,*])`, `result_dims` (`[D1,D2]`, slice order) in a
/// different order than the owner's dims (`[D2,D1]`) -- mints a
/// SYNTHETIC agg too: variable-backed slot coordinates are in
/// `Iterated`-axis order, which would mis-subscript `to`. Slots of the
/// synthetic agg are keyed by `result_dims` order, and the GH #528
/// projection reorders per target element.
#[test]
fn whole_rhs_permuted_partial_reduce_mints_synthetic_agg() {
    let project = TestProject::new("whole_rhs_permuted_764")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension("D3", &["p", "q"])
        .array_aux_direct(
            "cube",
            vec!["D1".into(), "D2".into(), "D3".into()],
            "1",
            None,
        )
        .array_aux_direct(
            "out",
            vec!["D2".into(), "D1".into()],
            "SUM(cube[D1, D2, *])",
            None,
        );

    let result = agg_nodes(&project);
    assert!(
        result.aggs.iter().all(|a| a.is_synthetic),
        "a whole-RHS PERMUTED reducer must mint a synthetic agg (not variable-backed); \
         got: {:?}",
        result.aggs
    );
    let agg = result
        .aggs_in_var("out")
        .next()
        .expect("expected a synthetic agg owned by `out`");
    assert_eq!(
        agg.result_dims,
        vec!["D1".to_string(), "D2".to_string()],
        "result_dims stay in Iterated-axis (slice) order, not the owner's declared order"
    );
    assert_eq!(
        agg.canonical_read_slice(),
        vec![
            AxisRead::Iterated {
                dim: "d1".to_string(),
                source_dim: "d1".to_string()
            },
            AxisRead::Iterated {
                dim: "d2".to_string(),
                source_dim: "d2".to_string()
            },
            AxisRead::Reduced { subset: None }
        ]
    );
}

/// GH #764 ∩ GH #765 (T4): a non-aligned whole-RHS reduce that ALSO
/// carries a `Pinned` axis (`out[D1,D2] = SUM(cube[D1,nyc,*])` over
/// `cube[D1,Region,D2]`) mints a synthetic agg whose slice keeps the
/// `Pinned` axis -- so the synthetic-half emitters (which are
/// Pinned-correct via `read_slice_rows`) score only the read rows.
/// Pre-T4 this shape rode the OLD full-cartesian link-score
/// derivation, scoring unread (`boston`) rows.
#[test]
fn whole_rhs_broadcast_pinned_mix_mints_synthetic_agg() {
    let project = TestProject::new("whole_rhs_broadcast_pinned_764")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["x", "y"])
        .array_aux_direct(
            "cube",
            vec!["D1".into(), "Region".into(), "D2".into()],
            "1",
            None,
        )
        .array_aux_direct(
            "out",
            vec!["D1".into(), "D2".into()],
            "SUM(cube[D1, nyc, *])",
            None,
        );

    let result = agg_nodes(&project);
    assert!(
        result.aggs.iter().all(|a| a.is_synthetic),
        "a Pinned-bearing non-aligned whole-RHS reducer must mint a synthetic agg; got: {:?}",
        result.aggs
    );
    let agg = result
        .aggs_in_var("out")
        .next()
        .expect("expected a synthetic agg owned by `out`");
    assert_eq!(agg.result_dims, vec!["D1".to_string()]);
    assert_eq!(
        agg.canonical_read_slice(),
        vec![
            AxisRead::Iterated {
                dim: "d1".to_string(),
                source_dim: "d1".to_string()
            },
            AxisRead::Pinned("nyc".to_string()),
            AxisRead::Reduced { subset: None }
        ]
    );
}

/// GH #534: `iterated_axis_slot_elements` -- identity for the literal case,
/// and the preimage of the executed correspondence for a foreign axis: the
/// diagonal under a positional mapping, the map's slots under an element
/// map, name identity where the two dimensions share element names, and
/// `None` for an undeclared pair with disjoint names or a many-to-one map (a
/// source element with several preimages has no single agg slot).
#[test]
fn iterated_axis_slot_elements_cases() {
    use crate::datamodel::{Dimension as DmDimension, DimensionMapping};
    use crate::dimensions::DimensionsContext;

    let named = |name: &str, elems: &[&str], mappings: Vec<DimensionMapping>| {
        let mut d = DmDimension::named(
            name.to_string(),
            elems.iter().map(|e| e.to_string()).collect(),
        );
        d.mappings = mappings;
        d
    };
    let positional = DimensionMapping {
        target: "Region".to_string(),
        element_map: vec![],
    };
    let element_mapped = DimensionMapping {
        target: "Region".to_string(),
        element_map: vec![
            ("s1".to_string(), "r2".to_string()),
            ("s2".to_string(), "r1".to_string()),
        ],
    };

    let region_elems = vec!["r1".to_string(), "r2".to_string()];

    let slots = |names: &[&str]| -> Option<Vec<Option<String>>> {
        Some(names.iter().map(|n| Some(n.to_string())).collect())
    };

    // Literal: identity (no dim_ctx lookups consulted).
    let ctx = DimensionsContext::from(&[
        named("Region", &["r1", "r2"], vec![]),
        named("State", &["s1", "s2"], vec![positional.clone()]),
    ]);
    assert_eq!(
        iterated_axis_slot_elements("region", "region", &region_elems, &ctx),
        slots(&["r1", "r2"])
    );

    // Positional mapping: source row r1 feeds slot s1, r2 feeds s2
    // (index-identity under the positional correspondence).
    assert_eq!(
        iterated_axis_slot_elements("state", "region", &region_elems, &ctx),
        slots(&["s1", "s2"])
    );

    // Explicit element map: the MAP's slots. The map here is the reverse
    // permutation (s1↦r2), so source row r1 feeds slot s2; an ordinal remap
    // would give ["s1", "s2"] and fail this row.
    let ctx_elem = DimensionsContext::from(&[
        named("Region", &["r1", "r2"], vec![]),
        named("State", &["s1", "s2"], vec![element_mapped]),
    ]);
    assert_eq!(
        iterated_axis_slot_elements("state", "region", &region_elems, &ctx_elem),
        slots(&["s2", "s1"])
    );

    // Shared element names under no mapping: name identity, whatever the
    // declared order.
    let ctx_shared = DimensionsContext::from(&[
        named("Region", &["r1", "r2"], vec![]),
        named("State", &["r2", "r1"], vec![]),
    ]);
    assert_eq!(
        iterated_axis_slot_elements("state", "region", &region_elems, &ctx_shared),
        slots(&["r1", "r2"])
    );

    // A superset source read through a subrange: the subrange's elements have
    // their slots (by name) and the others are not read -- `None` in their
    // position, not a decline.
    let ctx_subrange = DimensionsContext::from(&[
        named("Source", &["coal", "oilgas", "hn", "new"], vec![]),
        named("Nonrenewable", &["coal", "oilgas"], vec![]),
    ]);
    let source_elems: Vec<String> = ["coal", "oilgas", "hn", "new"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        iterated_axis_slot_elements("nonrenewable", "source", &source_elems, &ctx_subrange),
        Some(vec![
            Some("coal".to_string()),
            Some("oilgas".to_string()),
            None,
            None
        ])
    );

    // Unmapped pair with disjoint names: declined.
    let ctx_unmapped = DimensionsContext::from(&[
        named("Region", &["r1", "r2"], vec![]),
        named("State", &["s1", "s2"], vec![]),
    ]);
    assert_eq!(
        iterated_axis_slot_elements("state", "region", &region_elems, &ctx_unmapped),
        None
    );

    // A many-to-one map: `r1` is read by two target elements, so it has no
    // single slot; the direct-reference path describes the read, the
    // aggregate path declines the hoist.
    let ctx_many = DimensionsContext::from(&[
        named("Region", &["r1", "r2"], vec![]),
        named(
            "State",
            &["s1", "s2", "s3"],
            vec![DimensionMapping {
                target: "Region".to_string(),
                element_map: vec![
                    ("s1".to_string(), "r1".to_string()),
                    ("s2".to_string(), "r1".to_string()),
                    ("s3".to_string(), "r2".to_string()),
                ],
            }],
        ),
    ]);
    assert_eq!(
        iterated_axis_slot_elements("state", "region", &region_elems, &ctx_many),
        None
    );
}

/// AC4.4 (the carve-out): a reducer over a *dynamic* index
/// (`x[Region] = SUM(pop[idx, *])`, `idx` a scalar aux -- a non-literal
/// index) is NOT statically describable: `compute_read_slice` returns
/// `None` for the `idx` axis, so the reducer is not hoisted and its
/// reference stays on the conservative path. Pin this narrow case.
#[test]
fn dynamic_index_reducer_subexpression_is_not_hoisted() {
    let project = TestProject::new("dynamic_index_reducer")
        .named_dimension("Region", &["NYC", "Boston"])
        .named_dimension("Age", &["Adult", "Child"])
        .array_aux_direct("pop", vec!["Region".into(), "Age".into()], "10", None)
        .scalar_aux("idx", "1")
        .array_aux_direct("x", vec!["Region".into()], "SUM(pop[idx, *])", None);

    let result = agg_nodes(&project);
    assert!(
        result.aggs.iter().all(|a| !a.reads_var("pop")),
        "a dynamic-index reducer must not be hoisted; got: {:?}",
        result.aggs
    );
    assert!(result.synthetic_by_key.is_empty());
}

/// AC4.2 (positive guard): a whole-RHS slice/partial reduce
/// (`agg[D1] = SUM(matrix[D1, *])`) IS recognized -- but as a
/// variable-backed agg, not a synthetic one (covered by
/// `whole_rhs_arrayed_partial_reduce_is_its_own_agg`); and an all-
/// wildcard reducer subexpression (`SUM(matrix[*, *])`, no literal pin)
/// is still hoisted as a synthetic agg with an all-`Reduced` slice.
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
    assert_eq!(source_names(synthetic[0]), vec!["matrix"]);
    assert_eq!(
        synthetic[0].canonical_read_slice(),
        vec![
            AxisRead::Reduced { subset: None },
            AxisRead::Reduced { subset: None }
        ]
    );
    assert!(synthetic[0].result_dims.is_empty());
}

/// AC1.2 + GH #982: the consolidated reducer table classifies every array
/// reducer the LTM machinery cares about, and all THREE derived predicates
/// agree with the table row for row.
///
/// The rows come from [`REDUCER_DECISION_TABLE`], whose 11 entries are
/// derived from `reducer_kind_from_name`'s own arms (see its doc): every
/// arm, every arity guard in both directions, and the catch-all. Each row
/// asserts the name-keyed decider, the builtin-keyed decider, the arity
/// the builtin form reports, and the three consumers -- so the `SIZE` /
/// `RANK` inversion between `reducer_collapses_to_scalar` ("does this
/// collapse to a scalar?") and `builtin_routes_through_agg` ("did an agg
/// get minted for it?") is pinned in both directions on both rows, and an
/// edit to either predicate reds here rather than drifting silently
/// (GH #982).
#[test]
fn reducer_kind_classifies_every_array_reducer() {
    for row in REDUCER_DECISION_TABLE {
        let name = row.name;
        let arity = row.arity;
        let builtin = row.builtin();
        assert_eq!(
            builtin_reducer_arity(&builtin),
            arity,
            "{name}/{arity}: the builtin form must report the row's arity"
        );
        assert_eq!(
            reducer_kind_from_name(name, arity),
            row.kind,
            "{name}/{arity}: reducer_kind_from_name"
        );
        assert_eq!(
            reducer_kind(&builtin),
            row.kind,
            "{name}/{arity}: reducer_kind"
        );
        assert_eq!(
            reducer_collapses_to_scalar(name, arity),
            row.collapses_to_scalar,
            "{name}/{arity}: reducer_collapses_to_scalar"
        );
        assert_eq!(
            reducer_is_hoistable(&builtin),
            row.is_hoistable,
            "{name}/{arity}: reducer_is_hoistable"
        );
        assert_eq!(
            builtin_routes_through_agg(&builtin),
            row.routes_through_agg,
            "{name}/{arity}: builtin_routes_through_agg"
        );
    }

    // The table keys `stddev`/`rank`/`size`/`sum` at one arity each, but
    // `reducer_kind_from_name` ignores arity for them -- only
    // `mean`/`min`/`max` carry an `arity == 1` guard. Spot-check one, so a
    // stray arity guard added to an arity-insensitive arm is caught.
    assert_eq!(
        reducer_kind_from_name("stddev", 7),
        Some(ReducerKind::Nonlinear)
    );
    assert_eq!(
        reducer_kind_from_name("rank", 2),
        Some(ReducerKind::Nonlinear)
    );
}

/// GH #776: a RANK subexpression mints an ARRAY-valued synthetic agg,
/// not a scalar reducer agg. Its result dims are the ranked argument's
/// non-pinned axes, so a bare one-dimensional source ranks over Region.
#[test]
fn rank_subexpression_mints_array_valued_synthetic_agg() {
    let project = TestProject::new("rank_array_agg")
        .named_dimension("Region", &["north", "south"])
        .array_aux("pop[Region]", "100")
        .array_aux("scale[Region]", "pop[Region] * 0.01")
        .array_aux("grow[Region]", "scale[Region] * RANK(pop, 1)");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(
        synthetic.len(),
        1,
        "RANK must mint one synthetic aggregate node; got: {:?}",
        result.aggs
    );
    assert!(synthetic[0].array_valued_rank);
    assert_eq!(synthetic[0].result_dims, vec!["Region"]);
    assert_eq!(source_names(synthetic[0]), vec!["pop"]);
}

/// GH #796 review: multi-source RANK result dims must not depend on
/// `HashMap` iteration order. When several sources share the canonical
/// rank slice but carry differently named mapped axes, choose the first
/// source in canonical source-name order -- the same order `AggSource`
/// emission uses.
#[test]
fn rank_multi_source_result_dims_use_sorted_source_order() {
    let project = TestProject::new("rank_multi_source_dim_order")
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        // Declare `b` first to make the expected order source-name-based,
        // not model declaration order.
        .array_aux("b[State]", "1")
        .array_aux("a[Region]", "2")
        .array_aux("r[Region]", "RANK(a[*] + b[*], 1)");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(
        synthetic.len(),
        1,
        "multi-source RANK must mint one synthetic aggregate node; got: {:?}",
        result.aggs
    );
    assert!(synthetic[0].array_valued_rank);
    assert_eq!(source_names(synthetic[0]), vec!["a", "b"]);
    assert_eq!(synthetic[0].result_dims, vec!["Region"]);
}

/// GH #796 review: array-valued RANK over a proper StarRange returns the
/// ranked subdimension view. Its synthetic helper must be dimensioned over
/// `Core`, not the parent `Region`, or helper slots and source halves drift.
#[test]
fn rank_star_range_result_dims_preserve_subdimension() {
    let project = TestProject::new("rank_star_range_subdimension")
        .named_dimension("Region", &["a", "b", "c"])
        .named_dimension("Core", &["a", "b"])
        .array_aux("arr[Region]", "10")
        .array_aux("ranking[Core]", "RANK(arr[*:Core], 1)");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(
        synthetic.len(),
        1,
        "subdimension RANK must mint one synthetic aggregate node; got: {:?}",
        result.aggs
    );
    assert!(synthetic[0].array_valued_rank);
    assert_eq!(synthetic[0].result_dims, vec!["Core"]);
}

/// GH #796 review: the source-to-RANK slot helper is shared by element
/// graph edges and link-score names. It must fan active/iterated axes out
/// across every RANK output slot and use result-dimension element names,
/// not source-dimension names, for mapped sibling sources.
#[test]
fn rank_output_slots_use_result_dimension_elements() {
    let active_dim_read = vec![AxisRead::Iterated {
        dim: "region".to_string(),
        source_dim: "region".to_string(),
    }];
    assert_eq!(
        rank_output_slot_parts_for_row(
            &active_dim_read,
            &[vec!["north".to_string(), "south".to_string()]],
            &["north".to_string()],
        ),
        Some(vec![vec!["north".to_string()], vec!["south".to_string()]])
    );

    let mapped_source_read = vec![AxisRead::Reduced { subset: None }];
    assert_eq!(
        rank_output_slot_parts_for_row(
            &mapped_source_read,
            &[vec!["r1".to_string(), "r2".to_string()]],
            &[],
        ),
        Some(vec![vec!["r1".to_string()], vec!["r2".to_string()]])
    );

    let context_plus_ranked_axis = vec![
        AxisRead::Iterated {
            dim: "region".to_string(),
            source_dim: "region".to_string(),
        },
        AxisRead::Reduced { subset: None },
    ];
    assert_eq!(
        rank_output_slot_parts_for_row(
            &context_plus_ranked_axis,
            &[
                vec!["north".to_string(), "south".to_string()],
                vec!["x".to_string(), "y".to_string()],
            ],
            &["north".to_string()],
        ),
        Some(vec![
            vec!["north".to_string(), "x".to_string()],
            vec!["north".to_string(), "y".to_string()]
        ])
    );
}

/// GH #776 whole-RHS form: `r[Region] = RANK(pop, 1)` uses the same
/// synthetic array-valued helper as the inline spelling, rather than
/// becoming a variable-backed scalar reducer agg.
#[test]
fn rank_whole_rhs_mints_synthetic_not_variable_backed() {
    let project = TestProject::new("rank_whole_rhs")
        .named_dimension("Region", &["north", "south"])
        .array_aux("pop[Region]", "100")
        .array_aux("r[Region]", "RANK(pop, 1)");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(
        synthetic.len(),
        1,
        "whole-RHS RANK must mint one synthetic aggregate node; got: {:?}",
        result.aggs
    );
    assert!(synthetic[0].array_valued_rank);
    assert!(result.aggs.iter().all(|a| a.is_synthetic));
}

/// GH #766: a StarRange naming a dimension that is NEITHER the axis's
/// own dimension NOR a proper subdimension of it (at best a mid-edit
/// inconsistency) DECLINES the hoist -- it must not silently widen to
/// the full extent. The reducer stays on the conservative path.
#[test]
fn star_range_non_subdimension_declines_hoist() {
    let project = TestProject::new("star_range_decline")
        .named_dimension("Region", &["a", "b", "c"])
        .named_dimension("Other", &["p", "q"])
        .array_aux("arr[Region]", "10")
        .scalar_aux("x", "1 + MEAN(arr[*:Other])");

    let result = agg_nodes(&project);
    assert!(
        result.aggs.is_empty(),
        "a non-subdimension StarRange must decline the hoist; got: {:?}",
        result.aggs
    );
}

/// GH #766: a StarRange over the axis's OWN dimension (`SUM(arr[*:Region])`
/// where `arr` is declared over `Region`) is the full extent --
/// `Reduced{subset: None}`, byte-identical to a plain `*`.
#[test]
fn star_range_own_dimension_is_full_extent() {
    let project = TestProject::new("star_range_own_dim")
        .named_dimension("Region", &["a", "b", "c"])
        .array_aux("arr[Region]", "10")
        .scalar_aux("x", "1 + SUM(arr[*:Region])");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
    assert_eq!(
        synthetic[0].canonical_read_slice(),
        vec![AxisRead::Reduced { subset: None }]
    );
}

/// GH #766: a StarRange over a PROPER subdimension carries the
/// subdimension's elements as the `Reduced` subset (canonical names, in
/// subdimension-declared order, resolved via `SubdimensionRelation`).
#[test]
fn star_range_proper_subdimension_carries_subset() {
    let project = TestProject::new("star_range_subset")
        .named_dimension("Region", &["a", "b", "c"])
        .named_dimension("Core", &["a", "b"])
        .array_aux("arr[Region]", "10")
        .scalar_aux("x", "1 + MEAN(arr[*:Core])");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
    assert_eq!(
        synthetic[0].canonical_read_slice(),
        vec![AxisRead::Reduced {
            subset: Some(vec!["a".to_string(), "b".to_string()])
        }]
    );
    assert!(synthetic[0].result_dims.is_empty());
}

/// GH #766 (composition): a subset StarRange composes with an iterated
/// axis -- `out[D1] = 1 + SUM(matrix[D1, *:SubD2])` hoists a synthetic
/// agg whose slice is `[Iterated(d1), Reduced{subset}]` and whose
/// `result_dims` carry `D1`.
#[test]
fn star_range_subset_composes_with_iterated_axis() {
    let project = TestProject::new("star_range_iterated_subset")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y", "z"])
        .named_dimension("SubD2", &["x", "y"])
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
        .array_aux_direct(
            "out",
            vec!["D1".into()],
            "1 + SUM(matrix[D1, *:SubD2])",
            None,
        );

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
    assert_eq!(
        synthetic[0].canonical_read_slice(),
        vec![
            AxisRead::Iterated {
                dim: "d1".to_string(),
                source_dim: "d1".to_string()
            },
            AxisRead::Reduced {
                subset: Some(vec!["x".to_string(), "y".to_string()])
            }
        ]
    );
    assert_eq!(synthetic[0].result_dims, vec!["D1".to_string()]);
}

/// Test helper: resolve the named dimensions of a synced project into
/// `Dimension` objects (for the gate's `to_dims` argument).
fn resolve_dims(
    db: &SimlinDb,
    project: crate::db::SourceProject,
    names: &[&str],
) -> Vec<crate::dimensions::Dimension> {
    let dim_ctx = crate::db::project_dimensions_context(db, project);
    names
        .iter()
        .map(|n| {
            dim_ctx
                .get(&crate::common::CanonicalDimensionName::from_raw(n))
                .unwrap_or_else(|| panic!("dimension {n} resolves"))
                .clone()
        })
        .collect()
}

/// GH #766 x T3: a VARIABLE-BACKED partial reduce whose slice carries a
/// SUBSET (`out[D1] = SUM(matrix[D1,*:SubD2])` as the whole RHS) is
/// ACCEPTED by the reduce gate: `try_cross_dimensional_link_scores`
/// derives co-reduced rows from the same `read_slice_rows`, so the
/// subset edges pair with subset divisors. (Pre-T3 the slice was
/// excluded onto the loud conservative regime because the score
/// derivation enumerated the full cartesian.)
#[test]
fn variable_backed_subset_slice_is_accepted_by_reduce_gate() {
    let project = TestProject::new("vb_subset_accepted")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y", "z"])
        .named_dimension("SubD2", &["x", "y"])
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
        .array_aux_direct("out", vec!["D1".into()], "SUM(matrix[D1, *:SubD2])", None);

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let result = enumerate_agg_nodes(&db, sync.models["main"].source, sync.project);

    // The variable-backed agg exists and carries the subset...
    let agg = result
        .aggs_in_var("out")
        .find(|a| a.name == "out")
        .expect("expected a variable-backed agg owned by `out`");
    assert!(matches!(
        &agg.canonical_read_slice()[1],
        AxisRead::Reduced { subset: Some(s) } if s == &["x".to_string(), "y".to_string()]
    ));

    // ...and the gate admits it (T3 of the shape-expressiveness design).
    let to_dims = resolve_dims(&db, sync.project, &["d1"]);
    let accepted = variable_backed_reduce_agg(result, "matrix", "out", &to_dims)
        .expect("the subset-bearing aligned slice must be admitted by the reduce gate");
    assert_eq!(accepted.name, "out");
}

/// GH #765 x T3: a VARIABLE-BACKED Pinned-mixed aligned slice
/// (`outf[D1] = MEAN(cube[D1,x,*])`) is ACCEPTED by the reduce gate --
/// the T1-era Pinned exclusion is deleted atomically with the
/// `read_slice_rows` derivation swap.
#[test]
fn reduce_gate_accepts_pinned_mixed_aligned_slice() {
    let project = TestProject::new("gate_pinned_mixed")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension("D3", &["p", "q"])
        .array_aux_direct(
            "cube",
            vec!["D1".into(), "D2".into(), "D3".into()],
            "1",
            None,
        )
        .array_aux_direct("outf", vec!["D1".into()], "MEAN(cube[D1, x, *])", None);

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let result = enumerate_agg_nodes(&db, sync.models["main"].source, sync.project);

    let to_dims = resolve_dims(&db, sync.project, &["d1"]);
    let accepted = variable_backed_reduce_agg(result, "cube", "outf", &to_dims)
        .expect("the Pinned-mixed aligned slice must be admitted by the reduce gate");
    assert_eq!(accepted.name, "outf");
}

/// Section 6 (scalar owner): a scalar-result Pinned slice
/// (`total = SUM(pop[nyc,*])`, `to_dims` empty) is admitted -- the slot
/// is the bare `total` node, so `emit_agg_routed_edges` emits exactly
/// the read rows into `to`, matching the per-read-row scores.
#[test]
fn reduce_gate_accepts_scalar_owner_pinned_slice() {
    let project = TestProject::new("gate_scalar_owner_pinned")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux_direct("pop", vec!["Region".into(), "D2".into()], "1", None)
        .scalar_aux("total", "SUM(pop[nyc, *])");

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let result = enumerate_agg_nodes(&db, sync.models["main"].source, sync.project);

    let accepted = variable_backed_reduce_agg(result, "pop", "total", &[])
        .expect("the scalar-owner Pinned slice must be admitted by the reduce gate");
    assert_eq!(accepted.name, "total");
}

/// Section 6 (inert skip): a PURE full-extent scalar reduce
/// (`total = SUM(pop[*])`) stays OUT of the gate -- the reference
/// walker's reduction edges are already the true reads, so routing it
/// through the gate would change nothing and is skipped to keep the
/// diff inert (byte-identity).
#[test]
fn reduce_gate_declines_pure_full_extent_slice() {
    let project = TestProject::new("gate_full_extent")
        .named_dimension("Region", &["nyc", "boston"])
        .array_aux("pop[Region]", "1")
        .scalar_aux("total", "SUM(pop[*])");

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let result = enumerate_agg_nodes(&db, sync.models["main"].source, sync.project);

    assert!(
        variable_backed_reduce_agg(result, "pop", "total", &[]).is_none(),
        "a pure full-extent slice keeps the reference walker's edges (inert skip)"
    );
}

/// GH #777: an ARRAYED-owner scalar-result Pinned slice
/// (`share[Region] = SUM(pop[nyc,*])` -- no `Iterated` axis, arrayed
/// `to`) is ADMITTED: the single scalar reducer value broadcasts over
/// the owner's dims, and the per-(read-row, full-target-element)
/// machinery (`emit_agg_routed_edges`' broadcast fan-out +
/// `try_cross_dimensional_link_scores`' broadcast-reduce branch) names
/// every slot.
#[test]
fn reduce_gate_admits_arrayed_owner_scalar_result_slice() {
    let project = TestProject::new("gate_broadcast_pinned")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_aux_direct("pop", vec!["Region".into(), "D2".into()], "1", None)
        .array_aux_direct("share", vec!["Region".into()], "SUM(pop[nyc, *])", None);

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let result = enumerate_agg_nodes(&db, sync.models["main"].source, sync.project);

    let to_dims = resolve_dims(&db, sync.project, &["region"]);
    let accepted = variable_backed_reduce_agg(result, "pop", "share", &to_dims)
        .expect("the arrayed-owner broadcast slice must be admitted (GH #777)");
    assert_eq!(accepted.name, "share");
    assert!(
        accepted.result_dims.is_empty(),
        "the broadcast reducer's result is scalar (no Iterated axis); got: {:?}",
        accepted.result_dims
    );
}

/// GH #764 boundary (T4): a partial reduce BROADCAST over extra target
/// dims (`out[D1,D3] = SUM(matrix[D1,*])` -- `result_dims` a strict
/// subset of `to`'s dims) never reaches the variable-backed gate at all
/// anymore: T4's minting condition routes it to a SYNTHETIC agg, so
/// `variable_backed_reduce_agg` finds no variable-backed candidate (its
/// Iterated-arm alignment check stays as defense).
#[test]
fn reduce_gate_declines_broadcast_result_dims() {
    let project = TestProject::new("gate_broadcast_result")
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension("D3", &["p", "q"])
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
        .array_aux_direct(
            "out",
            vec!["D1".into(), "D3".into()],
            "SUM(matrix[D1, *])",
            None,
        );

    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let result = enumerate_agg_nodes(&db, sync.models["main"].source, sync.project);

    let to_dims = resolve_dims(&db, sync.project, &["d1", "d3"]);
    assert!(
        variable_backed_reduce_agg(result, "matrix", "out", &to_dims).is_none(),
        "a broadcast whole-RHS reduce must have no variable-backed agg (GH #764)"
    );
    assert!(
        result.aggs.iter().all(|a| a.is_synthetic),
        "T4 mints a synthetic agg for the broadcast shape; got: {:?}",
        result.aggs
    );
}

/// GH #766 / invariant I3 (uniqueness of the full-extent form): a
/// StarRange naming a SAME-CARDINALITY "subdimension" -- including a
/// permuted alias of the axis's element set (`Alias = [c, a, b]` over
/// `Region = [a, b, c]`: containment + equal size means the same element
/// SET) -- normalizes to `Reduced{subset: None}`, never a `Some` subset
/// covering the whole axis. Reduction order is irrelevant (the reduced
/// rows are a set), and keeping the full-extent representation unique
/// means downstream byte-identity does not depend on which spelling the
/// modeler used.
#[test]
fn star_range_same_cardinality_alias_normalizes_to_full_extent() {
    let project = TestProject::new("star_range_alias")
        .named_dimension("Region", &["a", "b", "c"])
        .named_dimension("Alias", &["c", "a", "b"])
        .array_aux("arr[Region]", "10")
        .scalar_aux("x", "1 + SUM(arr[*:Alias])");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
    assert_eq!(
        synthetic[0].canonical_read_slice(),
        vec![AxisRead::Reduced { subset: None }],
        "a whole-axis alias must normalize to the unique full-extent form"
    );
}

/// GH #766 (indexed dimensions): a StarRange over an INDEXED
/// subdimension (`SubIndex(3)` with declared `parent = Index(5)`, which
/// maps to the parent's first 3 elements) resolves the subset through
/// the same `SubdimensionRelation` path as named dimensions -- the
/// subset elements are the canonical indexed names `"1".."3"`, matching
/// `dimension_element_names`'s output.
#[test]
fn star_range_indexed_subdimension_carries_subset() {
    let project = TestProject::new("star_range_indexed_subdim")
        .indexed_dimension("Index", 5)
        .indexed_subdimension("SubIndex", 3, "Index")
        .array_aux("arr[Index]", "10")
        .scalar_aux("x", "1 + MEAN(arr[*:SubIndex])");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
    assert_eq!(
        synthetic[0].canonical_read_slice(),
        vec![AxisRead::Reduced {
            subset: Some(vec!["1".to_string(), "2".to_string(), "3".to_string()])
        }]
    );
}

// --- T2 (shape-expressiveness design): per-source `AggNode` invariant
// pins -- the per-source REPRESENTATION invariants (I2, I3b, sorted
// ordering) and the declines `accept_source_slices` enforces. I1's
// *feeder clause* pins (deferred from T2 to avoid the GH #739 vacuity
// trap) landed with T5's RED fixtures, in the section above.

/// T2 / I3b ordering: `sources` is sorted by canonical variable name
/// regardless of AST occurrence order -- `SUM(b[*] + a[*])` (with `b`
/// first in the argument) still yields `[a, b]`, so salsa cache
/// equality and downstream emission order never depend on how the
/// modeler spelled the argument.
#[test]
fn multi_source_sources_are_sorted_by_var_name() {
    let project = TestProject::new("sorted_sources")
        .named_dimension("D", &["p", "q"])
        .array_aux_direct("a", vec!["D".into()], "1", None)
        .array_aux_direct("b", vec!["D".into()], "2", None)
        .scalar_aux("total", "1 + SUM(b[*] + a[*])");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
    assert_eq!(
        source_names(synthetic[0]),
        vec!["a", "b"],
        "sources must be sorted by canonical name, not AST occurrence order"
    );
}

/// T2 / I3b dedup: the same variable referenced twice with the SAME
/// slice (`SUM(a[*] + a[*])`) collapses to ONE `AggSource` -- the
/// by-name downstream consumers (`aggs_in_var` routing, the
/// half-emitters) key `sources` on the variable name, so a duplicate
/// entry would make them ambiguous.
#[test]
fn duplicate_var_same_slice_collapses_to_one_source() {
    let project = TestProject::new("dup_var_same_slice")
        .named_dimension("D", &["p", "q"])
        .array_aux_direct("a", vec!["D".into()], "1", None)
        .scalar_aux("total", "1 + SUM(a[*] + a[*])");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
    assert_eq!(
        source_names(synthetic[0]),
        vec!["a"],
        "a variable read twice with the same slice is one AggSource"
    );
    assert_eq!(
        synthetic[0].sources[0].read_slice,
        vec![AxisRead::Reduced { subset: None }]
    );
}

/// T2 / I3b decline: the same variable referenced with two DIFFERENT
/// slices (`SUM(a[*] + a[p])` -- `[Reduced]` vs `[Pinned(p)]`) declines
/// the hoist -- since T5, `accept_source_slices`' per-variable
/// one-slice check (the I3b clause); the pin keeps I3b from regressing
/// under the widened per-source acceptance.
#[test]
fn duplicate_var_with_conflicting_slices_declines_hoist() {
    let project = TestProject::new("dup_var_conflicting")
        .named_dimension("D", &["p", "q"])
        .array_aux_direct("a", vec!["D".into()], "1", None)
        .scalar_aux("total", "1 + SUM(a[*] + a[p])");

    let result = agg_nodes(&project);
    assert!(
        result.aggs.iter().all(|ag| !ag.reads_var("a")),
        "one variable with two different slices must decline the hoist; got: {:?}",
        result.aggs
    );
    assert!(result.synthetic_by_key.is_empty());
}

/// T2 / I1 decline (GREEN characterization): two co-sources with
/// DIFFERING `Reduced` subsets (`SUM(a[*:Sub1] + b[*:Sub2])`) decline
/// the hoist -- their co-reduced rows per slot would disagree, so no
/// canonical slice exists. Enforced by `accept_source_slices`'
/// co-source-identity clause (subset is part of `AxisRead` equality).
#[test]
fn differing_reduced_subsets_decline_hoist() {
    let project = TestProject::new("differing_subsets")
        .named_dimension("Region", &["a", "b", "c"])
        .named_dimension("Sub1", &["a", "b"])
        .named_dimension("Sub2", &["b", "c"])
        .array_aux("p[Region]", "1")
        .array_aux("q[Region]", "2")
        .scalar_aux("total", "1 + SUM(p[*:Sub1] + q[*:Sub2])");

    let result = agg_nodes(&project);
    assert!(
        result
            .aggs
            .iter()
            .all(|ag| !ag.reads_var("p") && !ag.reads_var("q")),
        "co-sources with differing Reduced subsets must decline the hoist; got: {:?}",
        result.aggs
    );
    assert!(result.synthetic_by_key.is_empty());
}

/// T2 / I1 positive twin: two co-sources with the SAME `Reduced` subset
/// (`SUM(p[*:Sub] + q[*:Sub])`) hoist one agg whose every source
/// carries the identical subset-bearing canonical slice.
#[test]
fn agreeing_reduced_subsets_hoist_with_shared_subset() {
    let project = TestProject::new("agreeing_subsets")
        .named_dimension("Region", &["a", "b", "c"])
        .named_dimension("Sub", &["a", "b"])
        .array_aux("p[Region]", "1")
        .array_aux("q[Region]", "2")
        .scalar_aux("total", "1 + SUM(p[*:Sub] + q[*:Sub])");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
    assert_eq!(source_names(synthetic[0]), vec!["p", "q"]);
    let expected = vec![AxisRead::Reduced {
        subset: Some(vec!["a".to_string(), "b".to_string()]),
    }];
    for s in &synthetic[0].sources {
        assert_eq!(s.read_slice, expected, "source {} slice", s.var);
    }
}

/// T2 / I2 + the scalar-feeder representation: a scalar feeder of a
/// hoisted reducer (`scale` in `SUM(pop[*] * scale)`, GH #737) IS a
/// source -- the routing filter and the element graph's scalar-feeder
/// arm key on membership -- and carries an EMPTY read slice (one
/// `AxisRead` per axis, and a scalar has none), while the arrayed
/// co-source's slice has one entry per its declared axis.
#[test]
fn scalar_feeder_source_carries_empty_slice() {
    let project = TestProject::new("scalar_feeder")
        .named_dimension("Region", &["NYC", "Boston"])
        .array_aux("pop[Region]", "100")
        .scalar_aux("scale", "0.5")
        .scalar_aux("total", "1 + SUM(pop[*] * scale)");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
    assert_eq!(source_names(synthetic[0]), vec!["pop", "scale"]);
    // I2: one AxisRead per the source's OWN declared axes.
    assert_eq!(
        synthetic[0].source_read_slice("pop"),
        vec![AxisRead::Reduced { subset: None }],
        "the arrayed co-source's slice has one entry per its axis"
    );
    assert!(
        synthetic[0].source_read_slice("scale").is_empty(),
        "a scalar feeder has no axes, so its slice is empty"
    );
    // The canonical slice skips the feeder's empty slice.
    assert_eq!(
        synthetic[0].canonical_read_slice(),
        vec![AxisRead::Reduced { subset: None }]
    );
    // And the defensive non-source lookup is the empty slice too.
    assert!(synthetic[0].source_read_slice("absent").is_empty());
    assert!(synthetic[0].reads_var("scale"));
    assert!(!synthetic[0].reads_var("absent"));
}

// -- T5 / GH #767: the I1 FEEDER clause -------------------------------
//
// These pins were deliberately deferred from T2 (pinning them before the
// acceptance widened would have been vacuous -- the GH #739 trap). They
// land with T5's RED fixtures: an iterated-dim projection feeder is
// accepted as an `AggSource` with ITS OWN slice, the canonical slice is
// the co-source (Reduced-bearing) slice regardless of source sort order,
// and everything outside the projection rule still declines.

/// T5 / I1 feeder clause (GH #767): the iterated-dim-feeder reducer
/// `1 + SUM(matrix[D1,*] * frac[D1])` (inline => synthetic) IS hoisted:
/// `matrix` is the co-source carrying the canonical
/// `[Iterated, Reduced]` slice, `frac` is a projection feeder carrying
/// its OWN `[Iterated]` slice. `frac` sorts BEFORE `matrix`, so this
/// also pins the `canonical_read_slice` contract fix: the canonical
/// slice is the first slice WITH a `Reduced` axis, never an
/// alphabetically-first feeder slice.
#[test]
fn iterated_dim_feeder_projection_hoists_with_per_source_slices() {
    let project = TestProject::new("feeder_projection")
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
        .array_aux("frac[D1]", "0.5")
        .array_aux("growth[D1]", "1 + SUM(matrix[D1, *] * frac[D1])");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(
        synthetic.len(),
        1,
        "the projection-feeder reducer must be hoisted (GH #767); got: {:?}",
        result.aggs
    );
    let agg = synthetic[0];
    assert_eq!(source_names(agg), vec!["frac", "matrix"]);
    assert_eq!(
        agg.source_read_slice("frac"),
        vec![AxisRead::Iterated {
            dim: "d1".to_string(),
            source_dim: "d1".to_string()
        }],
        "the feeder carries its OWN projection slice"
    );
    assert_eq!(
        agg.source_read_slice("matrix"),
        vec![
            AxisRead::Iterated {
                dim: "d1".to_string(),
                source_dim: "d1".to_string()
            },
            AxisRead::Reduced { subset: None }
        ],
        "the co-source carries the canonical slice"
    );
    // The contract fix: even though `frac` sorts first, the canonical
    // slice is the Reduced-bearing co-source slice.
    assert_eq!(agg.canonical_read_slice(), agg.source_read_slice("matrix"));
    assert_eq!(agg.result_dims, vec!["D1".to_string()]);
}

/// T5 / I1: the WHOLE-RHS form of the feeder shape
/// (`growth[D1] = SUM(matrix[D1,*] * frac[D1])`, the GH #743/#767
/// fixture) is VARIABLE-BACKED -- the canonical (co-source) slice is
/// aligned with the owner's dims, so the variable IS the agg and no
/// synthetic is minted.
#[test]
fn iterated_dim_feeder_whole_rhs_is_variable_backed() {
    let project = TestProject::new("feeder_whole_rhs")
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
        .array_aux("frac[D1]", "0.5")
        .array_aux("growth[D1]", "SUM(matrix[D1, *] * frac[D1])");

    let result = agg_nodes(&project);
    assert!(result.synthetic_by_key.is_empty(), "got: {:?}", result.aggs);
    let vb: Vec<&AggNode> = result.aggs.iter().filter(|a| !a.is_synthetic).collect();
    assert_eq!(vb.len(), 1, "got: {:?}", result.aggs);
    assert_eq!(vb[0].name, "growth");
    assert_eq!(source_names(vb[0]), vec!["frac", "matrix"]);
    assert_eq!(vb[0].result_dims, vec!["D1".to_string()]);
    assert!(vb[0].source_is_projection_feeder("frac"));
    assert!(!vb[0].source_is_projection_feeder("matrix"));
}

/// T5 / I1: a feeder combined with a SCALAR feeder still hoists --
/// `SUM(matrix[D1,*] * frac[D1] * scale)` has three sources, the scalar
/// one with an empty slice (it is NOT a projection feeder: the
/// changed-last machinery for scalar feeders is `generate_scalar_feeder_
/// to_agg_equation`, not the per-row form).
#[test]
fn projection_feeder_and_scalar_feeder_combo_hoists() {
    let project = TestProject::new("feeder_combo")
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
        .array_aux("frac[D1]", "0.5")
        .scalar_aux("scale", "2")
        .array_aux("growth[D1]", "1 + SUM(matrix[D1, *] * frac[D1] * scale)");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
    let agg = synthetic[0];
    assert_eq!(source_names(agg), vec!["frac", "matrix", "scale"]);
    assert!(agg.source_read_slice("scale").is_empty());
    assert!(agg.source_is_projection_feeder("frac"));
    assert!(!agg.source_is_projection_feeder("scale"));
}

/// T5 / I1 (review MINOR-5): a PINNED-bearing CANONICAL slice is within
/// the feeder clause's scope -- the clause keys only on the canonical
/// slice's Iterated target dims, so `SUM(cube[D1, c1, *] * frac[D1])`
/// hoists with canonical `[Iterated, Pinned(c1), Reduced]` and the
/// feeder's own `[Iterated]` projection. (It is the FEEDER's slice that
/// must be Iterated-only, not the canonical one.)
#[test]
fn pinned_bearing_canonical_with_feeder_hoists() {
    let project = TestProject::new("pinned_canonical_feeder")
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .named_dimension("D3", &["k1", "k2"])
        .array_aux_direct(
            "cube",
            vec!["D1".into(), "D2".into(), "D3".into()],
            "5",
            None,
        )
        .array_aux("frac[D1]", "0.5")
        .array_aux("growth[D1]", "1 + SUM(cube[D1, c1, *] * frac[D1])");

    let result = agg_nodes(&project);
    let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
    let agg = synthetic[0];
    assert_eq!(
        agg.source_read_slice("cube"),
        vec![
            AxisRead::Iterated {
                dim: "d1".to_string(),
                source_dim: "d1".to_string()
            },
            AxisRead::Pinned("c1".to_string()),
            AxisRead::Reduced { subset: None }
        ]
    );
    assert!(agg.source_is_projection_feeder("frac"));
    assert_eq!(agg.result_dims, vec!["D1".to_string()]);
}

/// T5 / I1 decline: a no-`Reduced` source with a PINNED axis is NOT a
/// projection feeder (the design's clause: a feeder slice consists ONLY
/// of `Iterated` axes) -- the hoist declines.
#[test]
fn feeder_with_pinned_axis_declines_hoist() {
    let project = TestProject::new("feeder_pinned")
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
        .array_aux_direct("w", vec!["D1".into(), "D2".into()], "0.5", None)
        .array_aux("growth[D1]", "1 + SUM(matrix[D1, *] * w[D1, c1])");

    let result = agg_nodes(&project);
    assert!(
        result.aggs.iter().all(|a| !a.reads_var("matrix")),
        "a Pinned-axis no-Reduced source must decline the hoist; got: {:?}",
        result.aggs
    );
}

/// T5 / I1 decline: a feeder whose Iterated dims are a PROPER SUBSET of
/// the canonical slice's Iterated dims declines -- its rows are not 1:1
/// with the agg result slots (one feeder row would feed every slot it
/// projects from, a broadcast the per-`(row, slot)` machinery cannot
/// name). Documented residual: the design's I1 wording ("drawn from the
/// canonical Iterated target-dim set") is implemented as ordered
/// EQUALITY for exactly this reason.
#[test]
fn feeder_with_subset_iterated_dims_declines_hoist() {
    let project = TestProject::new("feeder_subset")
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .named_dimension("D3", &["x", "y"])
        .array_aux_direct(
            "cube",
            vec!["D1".into(), "D2".into(), "D3".into()],
            "5",
            None,
        )
        .array_aux("w[D1]", "0.5")
        .array_aux_direct(
            "growth",
            vec!["D1".into(), "D2".into()],
            "1 + SUM(cube[D1, D2, *] * w[D1])",
            None,
        );

    let result = agg_nodes(&project);
    assert!(
        result.aggs.iter().all(|a| !a.reads_var("cube")),
        "a proper-subset feeder must decline the hoist; got: {:?}",
        result.aggs
    );
}

/// T5 / I1 decline: a feeder whose Iterated dims are a PERMUTATION of
/// the canonical order declines -- `read_slice_rows` derives slot
/// coordinates in the source's axis order, so a permuted feeder's slots
/// would mis-name the agg's `result_dims`-ordered slots.
#[test]
fn feeder_with_permuted_iterated_dims_declines_hoist() {
    let project = TestProject::new("feeder_permuted")
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .named_dimension("D3", &["x", "y"])
        .array_aux_direct(
            "cube",
            vec!["D1".into(), "D2".into(), "D3".into()],
            "5",
            None,
        )
        .array_aux_direct("w", vec!["D2".into(), "D1".into()], "0.5", None)
        .array_aux_direct(
            "growth",
            vec!["D1".into(), "D2".into()],
            "1 + SUM(cube[D1, D2, *] * w[D2, D1])",
            None,
        );

    let result = agg_nodes(&project);
    assert!(
        result.aggs.iter().all(|a| !a.reads_var("cube")),
        "a permuted feeder must decline the hoist; got: {:?}",
        result.aggs
    );
}

/// T5 / I1 decline: a MAPPED Iterated axis (GH #534) anywhere in the
/// combination declines the feeder clause -- pinning the slot element
/// into the equation text reads the TARGET-dim element, which is not
/// the source row a mapped reference reads, so the changed-last feeder
/// equation would mis-pin. The mapped sliced reducer WITHOUT a feeder
/// stays hoisted (the GH #534 path is unchanged).
#[test]
fn mapped_iterated_axis_with_feeder_declines_hoist() {
    let project = TestProject::new("feeder_mapped")
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension("D2", &["x", "y"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .array_aux_direct("matrix", vec!["Region".into(), "D2".into()], "1", None)
        .array_aux_direct("frac", vec!["State".into()], "0.5", None)
        .array_aux_direct(
            "out",
            vec!["State".into()],
            "1 + SUM(matrix[State, *] * frac[State])",
            None,
        );

    let result = agg_nodes(&project);
    assert!(
        result.aggs.iter().all(|a| !a.reads_var("matrix")),
        "a mapped iterated axis with a feeder must decline the hoist; got: {:?}",
        result.aggs
    );
}

/// T5 / I3b decline: the same variable appearing as both a co-source
/// and a feeder-shaped reference (`SUM(matrix[D1,*] * matrix[D1,c1])`)
/// declines -- one variable, two different slices.
#[test]
fn duplicate_var_as_co_source_and_feeder_declines_hoist() {
    let project = TestProject::new("dup_co_source_feeder")
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
        .array_aux("growth[D1]", "1 + SUM(matrix[D1, *] * matrix[D1, c1])");

    let result = agg_nodes(&project);
    assert!(
        result.aggs.iter().all(|a| !a.reads_var("matrix")),
        "one variable with co-source AND feeder slices must decline; got: {:?}",
        result.aggs
    );
}

/// T5 / I1 decline: two CO-SOURCES (both Reduced-bearing) with
/// differing slices still decline, exactly as before the feeder clause
/// -- the clause widens acceptance only for no-`Reduced` projections.
#[test]
fn co_sources_with_differing_slices_still_decline() {
    let project = TestProject::new("co_source_differ")
        .named_dimension("D1", &["r1", "r2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_aux_direct("a", vec!["D1".into(), "D2".into()], "1", None)
        .array_aux_direct("b", vec!["D2".into(), "D1".into()], "2", None)
        .array_aux("growth[D1]", "1 + SUM(a[D1, *] + b[*, D1])");

    let result = agg_nodes(&project);
    assert!(
        result
            .aggs
            .iter()
            .all(|ag| !ag.reads_var("a") && !ag.reads_var("b")),
        "co-sources with differing slices must still decline; got: {:?}",
        result.aggs
    );
}

/// PR #784 review (P3), purely defensive: every arrayed reducer source
/// has a `per_var` slice by construction (`collect_var_refs` and
/// `collect_arrayed_source_slices` walk the identical reference
/// surface), but if that invariant ever broke, [`agg_sources`] must
/// DECLINE the hoist (`None` -- the reference stays on the conservative
/// Direct path) rather than silently substituting the CANONICAL slice:
/// for a projection feeder (whose slice differs from canonical by
/// design, GH #767) that substitution would mislabel the feeder as a
/// co-source and corrupt the per-`(row, slot)` link scores downstream.
#[test]
fn agg_sources_declines_when_arrayed_source_lacks_per_var_slice() {
    let project = TestProject::new("agg_sources_invariant")
        .named_dimension("D1", &["r1", "r2"])
        .array_aux("pop[D1]", "1")
        .scalar_aux("scale", "2")
        .scalar_aux("total", "1 + SUM(pop[*] * scale)");
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let variables = crate::db::model_lowered_variables(&db, model, sync.project);
    let dm_dims = crate::db::project_datamodel_dims(&db, sync.project);
    let dim_ctx = crate::db::project_dimensions_context(&db, sync.project);
    let ctx = AggWalkCtx {
        variables: &variables,
        target_iterated_dims: &[],
        target_dims: &[],
        dm_dims: dm_dims.as_slice(),
        dim_ctx,
    };
    let canonical = vec![AxisRead::Reduced { subset: None }];

    // The invariant broken by hand: `pop` (arrayed) absent from `per_var`.
    let broken = CombinedReadSlices {
        canonical: canonical.clone(),
        per_var: HashMap::new(),
    };
    assert_eq!(
        agg_sources(vec!["pop".to_string()], &broken, &ctx),
        None,
        "a missing per-var slice for an arrayed source must decline the \
         hoist, never substitute the canonical slice"
    );

    // The intact invariant: each source carries its own slice; a scalar
    // source still gets the empty slice.
    let intact = CombinedReadSlices {
        canonical: canonical.clone(),
        per_var: HashMap::from([("pop".to_string(), canonical.clone())]),
    };
    let sources = agg_sources(vec!["scale".to_string(), "pop".to_string()], &intact, &ctx)
        .expect("an intact per-var map must build the sources");
    assert_eq!(
        sources,
        vec![
            AggSource {
                var: "pop".to_string(),
                read_slice: canonical,
            },
            AggSource {
                var: "scale".to_string(),
                read_slice: vec![],
            },
        ]
    );
}

/// GH #983: every recognized reducer's classification is CARRIED on the
/// SYNTHETIC node it decided, so no emitter has to recover it by
/// parsing [`AggNode::reducer_key`].
///
/// Scope, stated because "every recognized reducer" is only one of the two
/// axes here: this covers all seven reducers on the SYNTHETIC producer arm,
/// which is the arm both readers actually reach. `register_agg`'s
/// variable-backed arm stores a `reducer` too and no test covers it, because
/// no reader consumes it -- see [`AggNode::reducer`]'s doc.
///
/// The rows are the reducer set itself, not a sample. `reducer_kind` is
/// the only admission test, and [`crate::ltm_augment`]'s
/// `classify_builtin_if_references_source` -- the function that reads the
/// kind, name and body back off `AggNode::reducer` -- destructures exactly
/// SEVEN `BuiltinFn` variants (`Sum`, `Mean`, `Min`, `Max`, `Stddev`,
/// `Rank`, `Size`) and calls `unreachable!()` on the rest, so those seven
/// are the whole space and this table has seven rows. Each row states the
/// three facts the emitters read: whether a node is minted at all, and (if
/// so) the `ReducerKind` and uppercase name the carried builtin classifies
/// to.
///
/// `SIZE` is the one row that mints nothing (`ReducerKind::Constant` is
/// never hoisted -- its link score is always 0), and `RANK` is the one
/// row that mints an ARRAY-valued node; both are properties of the
/// enumerator this table would notice changing.
#[test]
fn every_reducer_carries_its_classification_on_the_agg_node() {
    use crate::ltm_augment::{ReducerKind, classify_reducer_in_builtin};

    // (equation for `out`, out's dims, expected (kind, name, array_valued_rank))
    struct Row {
        equation: &'static str,
        out_dims: &'static [&'static str],
        expected: Option<(ReducerKind, &'static str, bool)>,
    }
    let rows = [
        Row {
            equation: "1 + SUM(pop[*])",
            out_dims: &[],
            expected: Some((ReducerKind::Linear, "SUM", false)),
        },
        Row {
            equation: "1 + MEAN(pop[*])",
            out_dims: &[],
            expected: Some((ReducerKind::Linear, "MEAN", false)),
        },
        Row {
            equation: "1 + MIN(pop[*])",
            out_dims: &[],
            expected: Some((ReducerKind::Nonlinear, "MIN", false)),
        },
        Row {
            equation: "1 + MAX(pop[*])",
            out_dims: &[],
            expected: Some((ReducerKind::Nonlinear, "MAX", false)),
        },
        Row {
            equation: "1 + STDDEV(pop[*])",
            out_dims: &[],
            expected: Some((ReducerKind::Nonlinear, "STDDEV", false)),
        },
        Row {
            // Array-valued: the node is arrayed over the ranked axis, so
            // its consumer must be arrayed too.
            equation: "1 + RANK(pop[*], 1)",
            out_dims: &["Region"],
            expected: Some((ReducerKind::Nonlinear, "RANK", true)),
        },
        Row {
            // `ReducerKind::Constant`: recognized, never hoisted.
            equation: "1 + SIZE(pop[*])",
            out_dims: &[],
            expected: None,
        },
    ];

    for Row {
        equation,
        out_dims,
        expected,
    } in rows
    {
        let mut project = TestProject::new("carried")
            .named_dimension("Region", &["nyc", "boston"])
            .array_aux("pop[Region]", "1");
        project = if out_dims.is_empty() {
            project.scalar_aux("out", equation)
        } else {
            project.array_aux_direct(
                "out",
                out_dims.iter().map(|d| (*d).to_string()).collect(),
                equation,
                None,
            )
        };
        let aggs = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = aggs.aggs.iter().filter(|a| a.is_synthetic).collect();

        let Some((kind, name, array_valued_rank)) = expected else {
            assert!(
                synthetic.is_empty(),
                "{equation}: a Constant reducer must mint no aggregate node"
            );
            continue;
        };
        assert_eq!(
            synthetic.len(),
            1,
            "{equation}: expected exactly one synthetic aggregate node"
        );
        let agg = synthetic[0];
        assert_eq!(
            agg.array_valued_rank, array_valued_rank,
            "{equation}: array_valued_rank"
        );

        let classified = classify_reducer_in_builtin(&agg.reducer, "pop", true)
            .unwrap_or_else(|| panic!("{equation}: the carried builtin must classify"));
        assert_eq!(classified.kind, kind, "{equation}: kind");
        assert_eq!(classified.name, name, "{equation}: name");
        assert!(
            classified.is_bare,
            "{equation}: an aggregate node's equation IS the reducer call"
        );
        // The body is the reducer's array argument, taken from the AST the
        // enumerator walked rather than parsed from `reducer_key`.
        assert_eq!(
            crate::ast::print_eqn(&classified.body),
            "pop[*]",
            "{equation}: body"
        );
    }
}

/// GH #983: the carried reducer is stored in
/// [`crate::ast::Expr2::strip_loc_and_bounds`] form, so a `Loc`-only edit
/// leaves the salsa-cached `AggNodesResult` equal.
///
/// **This is one of the two ways the backdating claim can fail, and it
/// measures only this one.** The other is float non-reflexivity on a NaN
/// literal, which normalization cannot touch and which is closed at the
/// root by [`crate::ast::Literal`]'s bit-pattern equality;
/// [`a_nan_literal_in_a_reducer_does_not_defeat_agg_backdating`] pins that
/// arm.
///
/// The two spellings differ ONLY in a leading term that mints no
/// aggregate node of its own -- a `SIZE` call, which is recognized as a
/// reducer but never hoisted -- so `SUM(pop[*])` moves to a different byte
/// offset and nothing else about the enumeration changes. The whole
/// `AggNodesResult` must therefore compare EQUAL, which is exactly
/// salsa's backdating criterion and so is the invalidation claim: an edit
/// that changes neither the reducer nor its sources must not invalidate
/// this query's consumers, as it did not before the node carried an AST
/// at all.
///
/// Both halves of the normalization are measured. `db::model_lowered_variables`
/// -- the source of the ASTs this query walks -- lowers each variable under
/// its dependencies' shapes, so `pop[*]`'s bound carries a temp id, and the
/// leading `SIZE(other[*])` is chosen over a bare constant because its arrayed
/// argument takes the first temp id and pushes `pop[*]` to the second: an
/// `ArrayBounds` left in the carried reducer would make the two spellings
/// unequal on the id alone.
///
/// The second assertion is a weaker independent check on the same
/// property (re-normalizing the stored builtin is a no-op); it catches
/// normalization being dropped entirely but, being idempotence, cannot
/// by itself catch the normalization being made too weak. That is what
/// the equality assertion above is for.
/// `AggNode::reducer_expr0` -- the typed reducer the agg's own equation and
/// the feeder link scores are generated from -- is the tree a parse of
/// `reducer_key` produces, up to `Loc`, and prints as that key. This is what
/// lets `ltm_augment_tests`' feeder-generator rows hand the generators a
/// parsed reducer: the parse IS the value production supplies. The rows are
/// the reducer shapes those generators see -- a scalar feeder beside a
/// wildcard slice, and an iterated-dim projection feeder beside a partial
/// slice -- plus a nested reducer, which `reducer_expr0` projects recursively.
#[test]
fn the_typed_reducer_is_the_parse_of_its_key() {
    use crate::ast::{Expr0, print_eqn};
    use crate::lexer::LexerType;

    let nodes = agg_nodes(
        &TestProject::new("typed_reducer")
            .named_dimension("d1", &["r1", "r2"])
            .named_dimension("d2", &["c1", "c2"])
            .array_aux("pop[d1]", "1")
            .array_aux("matrix[d1, d2]", "2")
            .array_aux("frac[d1]", "3")
            .scalar_aux("scale", "2")
            .scalar_aux("total", "1 + SUM(pop[*] * scale)")
            .array_aux("growth[d1]", "1 + SUM(matrix[d1, *] * frac[d1])")
            .scalar_aux("nested", "1 + SUM(pop[*] * SUM(matrix[*, *]))"),
    );
    let synthetic: Vec<&AggNode> = nodes.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(
        synthetic.len(),
        3,
        "three hoisted reducers: {:?}",
        synthetic.iter().map(|a| &a.reducer_key).collect::<Vec<_>>()
    );
    for agg in synthetic {
        let typed = agg.reducer_expr0();
        assert_eq!(
            print_eqn(&typed),
            agg.reducer_key,
            "the key is the typed reducer's print"
        );
        let parsed = Expr0::new(&agg.reducer_key, LexerType::Equation)
            .expect("a reducer key lexes")
            .expect("a reducer key is an expression");
        assert!(
            typed.eq_ignoring_loc(&parsed),
            "a parse of the key is the typed reducer: {} vs {}",
            print_eqn(&typed),
            print_eqn(&parsed)
        );
    }
}

#[test]
fn the_carried_reducer_is_normalized_so_offset_only_edits_backdate() {
    let build = |leading: &str| {
        TestProject::new("normalized")
            .named_dimension("Region", &["nyc", "boston"])
            .array_aux("pop[Region]", "1")
            .array_aux("other[Region]", "2")
            .scalar_aux("out", &format!("{leading} + SUM(pop[*])"))
    };
    let before = agg_nodes(&build("1"));
    let after = agg_nodes(&build("SIZE(other[*])"));

    let sum_agg = |r: &AggNodesResult| {
        r.aggs
            .iter()
            .find(|a| a.reducer_key == "sum(pop[*])")
            .cloned()
            .expect("the SUM subexpression must be hoisted")
    };
    assert_eq!(
        before, after,
        "an edit that only moves the reducer's byte offsets must leave the \
         enumerated aggregate nodes equal, so salsa backdates"
    );
    let agg = sum_agg(&before);
    assert_eq!(
        agg.reducer
            .clone()
            .map(crate::ast::Expr2::strip_loc_and_bounds)
            .strip_own_locs(),
        agg.reducer,
        "the stored reducer must already be normalized"
    );
}

// The third channel by which `AggNode::reducer` (GH #983) could make the
// salsa-cached, `PartialEq`-compared `AggNodesResult` unequal to an
// identical rebuild: the `f64` on `Expr2::Const`, which
// `strip_loc_and_bounds` cannot normalize away (there is no rewrite that
// removes a NaN literal without changing what the equation means).
//
// It is closed at the ROOT rather than here: `ast::Literal` compares float
// literals by BIT PATTERN, so `nan == nan` and a bit-identical AST is
// equal to itself (GH #987/#981). Salsa's backdating criterion is exactly
// that equality, so without the root fix every revision bump -- from any
// unrelated edit anywhere in the project -- re-executed
// `model_element_causal_edges`, `model_ltm_reference_sites` and
// `model_ltm_variables` for any model with a `nan` in a hoisted reducer.
//
// This test was written inverted (`assert_ne!`) as the characterization of
// that defect; flipping it is how the root fix demonstrates it reached a
// real consumer rather than only its own unit test.
#[test]
fn a_nan_literal_in_a_reducer_does_not_defeat_agg_backdating() {
    let build = || {
        TestProject::new("nan_backdating")
            .named_dimension("Region", &["nyc", "boston"])
            .array_aux("pop[Region]", "1")
            .scalar_aux("out", "1 + SUM(pop[*] * nan)")
    };

    // Guard against a vacuous pass: the reducer really is hoisted, so the
    // NaN really does ride on a stored `AggNode::reducer`.
    let enumerated = agg_nodes(&build());
    let synthetic: Vec<&AggNode> = enumerated.aggs.iter().filter(|a| a.is_synthetic).collect();
    assert_eq!(
        synthetic.len(),
        1,
        "the NaN-bearing reducer must still be hoisted for this to measure anything"
    );

    assert_eq!(
        agg_nodes(&build()),
        agg_nodes(&build()),
        "two enumerations of an identical NaN-bearing model must compare \
         equal, so `enumerate_agg_nodes` backdates for this model like any \
         other"
    );
}

/// The full ENUMERATION of [`classify_axis_access`]'s verdicts for the
/// element-vs-dimension-name collision, plus a per-`IndexExpr2`-variant
/// control row (GH #986).
///
/// XMILE permits a dimension to declare an element whose name is also a
/// dimension name (`Bucket = [region, old]` beside a `Region` dimension),
/// and this classifier used to break the tie the wrong way: it asked
/// "is the name one of the target's iterated dimensions?" FIRST, so
/// `effect[Region, region]` was read as an iteration over `Region` rather
/// than as `Bucket`'s `region` ELEMENT. `compiler::subscript`'s
/// `normalize_subscripts3` resolves it the other way round, and the
/// simulation is the authority: a classifier that disagrees describes rows
/// the simulation never reads.
///
/// The two collision rows are the whole point, and they fail differently --
/// which is why both are here rather than one standing in for the pair:
///
/// - UNMAPPED (`bucket` has no mapping to `region`): the old order found no
///   correspondence and returned `None`, so the reference fell to the
///   conservative cross-product. Over-broad, not wrong -- a precision gap;
/// - MAPPED (`bucket` positionally mapped to `region`): the old order took
///   the mapped branch and returned `Iterated` over a dimension the compiler
///   never iterates there, so read slices and element edges named rows the
///   simulation does not read. That one is a correctness hazard, and it is
///   the reason the fix is not just a precision improvement.
///
/// The remaining rows are the enumeration this is derived from rather than
/// sampled: the function's match has SIX arms (`Wildcard`, `StarRange`,
/// `Range | DimPosition`, `Expr(Var)`, `Expr(Const)`, `Expr(_)`) and every one
/// has a row, with `Expr(Var)` fanning out over all FIVE ways a name can
/// resolve -- an element of the axis, an iterated dimension matching the axis by
/// name, an iterated dimension matching it through a positional mapping, an
/// iterated dimension with no usable correspondence, and a name that is neither.
///
/// `Expr(Const)` resolves by a different rule than `Expr(Var)`, and its row
/// says so: a constant index is POSITIONAL (`compiler::subscript` lowers it to
/// `IndexOp::Single(value - 1)`, a raw offset into this axis), so `pop[2]`
/// selects the second element of a NAMED dimension rather than naming nothing.
/// The same rule is what lets a constified `dimension·element` reference resolve
/// at all.
///
/// Two companions, doing different jobs -- this row states the classifier's
/// answer and neither of them does:
/// `db::ltm_element_instance_tests::numeric_literal_index_is_positional_in_a_named_dimension`
/// pins the PREMISE against the VM (that `pop[2]` really reads `boston`), and
/// `db::ltm_element_instance_tests::a_constant_pinned_reducer_axis_narrows_the_read_slice`
/// pins the CONSEQUENCE end to end (the read slice this row's `Pinned` produces
/// narrows `SUM(matrix[2,*])`'s edges to the row it reads). Together they are
/// what makes this row's answer checkable rather than asserted; alone, this
/// assertion is the only thing holding `resolve_literal_axis_index`'s constant
/// arm.
///
/// One arm is covered elsewhere rather than here: `StarRange` has three
/// outcomes (the axis's own dimension, a PROPER subdimension, and a name that
/// is neither), and the subdimension pair is GH #766's, pinned by the
/// `*_star_range_*` tests in this file over real models. The row below states
/// the own-dimension outcome, which is the one a name-resolution change could
/// disturb.
#[test]
fn classify_axis_access_resolves_a_colliding_name_element_first() {
    use crate::ast::{Expr2, IndexExpr2, Loc};
    use crate::common::{CanonicalDimensionName, Ident};
    use crate::datamodel;

    // `bucket`'s FIRST element is named `region`, exactly like the `region`
    // DIMENSION the target iterates. `mapped` decides whether `bucket` also
    // carries a positional mapping to `region`, which is what separates the
    // old code's two failure modes.
    let ctx_with = |mapped: bool| {
        let mut bucket = datamodel::Dimension::named(
            "bucket".to_string(),
            vec!["region".to_string(), "old".to_string()],
        );
        if mapped {
            bucket.mappings = vec![datamodel::DimensionMapping {
                target: "region".to_string(),
                element_map: vec![],
            }];
        }
        // Iterated by the target and POSITIONALLY mapped to `region`, so it
        // lines up with a `region` axis by correspondence rather than by name.
        let mut mapped_dim = datamodel::Dimension::named(
            "mapped_dim".to_string(),
            vec!["m1".to_string(), "m2".to_string()],
        );
        mapped_dim.mappings = vec![datamodel::DimensionMapping {
            target: "region".to_string(),
            element_map: vec![],
        }];
        crate::dimensions::DimensionsContext::from(
            [
                datamodel::Dimension::named(
                    "region".to_string(),
                    vec!["nyc".to_string(), "boston".to_string()],
                ),
                mapped_dim,
                // Iterated by the target and mapped to nothing -- the row where
                // the correspondence declines.
                datamodel::Dimension::named(
                    "lonely".to_string(),
                    vec!["l1".to_string(), "l2".to_string()],
                ),
                bucket,
            ]
            .as_slice(),
        )
    };
    let bucket_axis = |ctx: &crate::dimensions::DimensionsContext| {
        ctx.get(&CanonicalDimensionName::from_raw("bucket"))
            .expect("the fixture declares `bucket`")
            .clone()
    };
    let var_idx = |name: &str| IndexExpr2::Expr(Expr2::Var(Ident::new(name), None, Loc::default()));
    // The target iterates `region` plus the two dimensions the mapped rows need:
    // `mapped_dim` (positionally mapped to `region`) and `lonely` (mapped to
    // nothing).
    let iterated = [
        "region".to_string(),
        "mapped_dim".to_string(),
        "lonely".to_string(),
    ];

    // --- the collision, in both mapping states ---------------------------
    for mapped in [false, true] {
        let ctx = ctx_with(mapped);
        assert_eq!(
            classify_axis_access(&var_idx("region"), &bucket_axis(&ctx), &iterated, &ctx),
            Some(AxisRead::Pinned("region".to_string())),
            "a name the axis declares as an ELEMENT must resolve to that \
             element even when it also names an iterated dimension \
             (mapped = {mapped}) -- that is what `normalize_subscripts3` does"
        );
        // The control: the same axis's other element, which collides with
        // nothing. A fix that special-cased the colliding name would not
        // change this row, so it cannot stand in for the row above.
        assert_eq!(
            classify_axis_access(&var_idx("old"), &bucket_axis(&ctx), &iterated, &ctx),
            Some(AxisRead::Pinned("old".to_string())),
            "the non-colliding control element (mapped = {mapped})"
        );
    }
    // Non-vacuity: the collision must be real in both directions, or the
    // rows above are testing an ordinary element lookup.
    let mapped_ctx = ctx_with(true);
    assert!(mapped_ctx.is_dimension_name("region"));
    assert!(
        mapped_ctx
            .executed_read_correspondence(
                &CanonicalDimensionName::from_raw("region"),
                &CanonicalDimensionName::from_raw("bucket"),
            )
            .is_some(),
        "the mapped row needs a usable `region`/`bucket` correspondence, or \
         it duplicates the unmapped row"
    );

    // --- the rest of the space -------------------------------------------
    let ctx = ctx_with(false);
    let region_axis = ctx
        .get(&CanonicalDimensionName::from_raw("region"))
        .expect("the fixture declares `region`")
        .clone();
    // A name the axis does NOT declare, matching the axis's own dimension:
    // the ordinary iterated form.
    assert_eq!(
        classify_axis_access(&var_idx("region"), &region_axis, &iterated, &ctx),
        Some(AxisRead::Iterated {
            dim: "region".to_string(),
            source_dim: "region".to_string(),
        }),
        "an iterated dimension matching the axis by name"
    );
    // Iterated, matching the axis through a POSITIONAL mapping rather than by
    // name: `mapped_dim` is not `region`, but the correspondence lines them up.
    assert_eq!(
        classify_axis_access(&var_idx("mapped_dim"), &region_axis, &iterated, &ctx),
        Some(AxisRead::Iterated {
            dim: "mapped_dim".to_string(),
            source_dim: "region".to_string(),
        }),
        "an iterated dimension matching the axis through a positional mapping"
    );
    // Iterated, with NO usable correspondence to this axis: declines.
    assert_eq!(
        classify_axis_access(&var_idx("lonely"), &region_axis, &iterated, &ctx),
        None,
        "an iterated dimension with no usable correspondence to this axis \
         declines, keeping the reference on the conservative path"
    );
    // A name that is neither an element of the axis nor iterated.
    assert_eq!(
        classify_axis_access(&var_idx("idx"), &region_axis, &iterated, &ctx),
        None,
        "a variable read selecting the element is not statically describable"
    );
    // The remaining `IndexExpr2` variants, none of which poses the
    // name-resolution question.
    assert_eq!(
        classify_axis_access(
            &IndexExpr2::Wildcard(Loc::default()),
            &region_axis,
            &iterated,
            &ctx
        ),
        Some(AxisRead::Reduced { subset: None }),
        "a wildcard reduces the whole axis"
    );
    assert_eq!(
        classify_axis_access(
            &IndexExpr2::StarRange(CanonicalDimensionName::from_raw("region"), Loc::default()),
            &region_axis,
            &iterated,
            &ctx
        ),
        Some(AxisRead::Reduced { subset: None }),
        "`*:D` over the axis's own dimension is the full extent"
    );
    assert_eq!(
        classify_axis_access(
            &IndexExpr2::Range(
                Expr2::Const(
                    "1".to_string(),
                    crate::ast::Literal::new(1.0),
                    Loc::default()
                ),
                Expr2::Const(
                    "2".to_string(),
                    crate::ast::Literal::new(2.0),
                    Loc::default()
                ),
                Loc::default()
            ),
            &region_axis,
            &iterated,
            &ctx
        ),
        None,
        "a range is not statically describable per axis"
    );
    assert_eq!(
        classify_axis_access(
            &IndexExpr2::DimPosition(2, Loc::default()),
            &region_axis,
            &iterated,
            &ctx
        ),
        None,
        "an `@N` position is declined for hoisting"
    );
    assert_eq!(
        classify_axis_access(
            &IndexExpr2::Expr(Expr2::Const(
                "2".to_string(),
                crate::ast::Literal::new(2.0),
                Loc::default()
            )),
            &region_axis,
            &iterated,
            &ctx
        ),
        Some(AxisRead::Pinned("boston".to_string())),
        "a constant index is POSITIONAL: `pop[2]` selects the axis's SECOND \
         element, whatever it is named"
    );
    assert_eq!(
        classify_axis_access(
            &IndexExpr2::Expr(Expr2::Op2(
                crate::ast::BinaryOp::Add,
                Box::new(Expr2::Var(Ident::new("idx"), None, Loc::default())),
                Box::new(Expr2::Const(
                    "1".to_string(),
                    crate::ast::Literal::new(1.0),
                    Loc::default()
                )),
                None,
                Loc::default()
            )),
            &region_axis,
            &iterated,
            &ctx
        ),
        None,
        "a compound index expression is not statically describable"
    );
}

/// The ORACLE behind [`classify_axis_access_resolves_a_colliding_name_element_first`]:
/// what the executed simulation actually reads for a colliding index name.
///
/// The classifier's job is to describe the reference the simulation
/// performs, so "element-first is right" is a claim about this engine and is
/// checked by running it rather than asserted. `Bucket = [old, region]`
/// declares its colliding element at position 1 while `Region`'s `nyc` sits
/// at position 0, which is what makes the two readings distinguishable:
/// element-first reads `effect[nyc, region]`, and the dimension-name reading
/// would resolve `Region`'s current element `nyc` against `Bucket` (which
/// declares no `nyc`) and read a different row -- or fail to resolve at all.
///
/// The second half asserts the IR now DESCRIBES that read: the `effect -> t`
/// site classifies `PerElement` with the `Bucket` axis `Pinned` to the
/// literal element. Before GH #986 the classifier took the dimension-name
/// branch, found no `Region`/`Bucket` correspondence and declined, so the
/// reference fell to the conservative cross-product instead.
#[test]
fn a_colliding_index_name_reads_the_axis_element_in_the_simulation() {
    use crate::db::{RefShape, compile_project_incremental, ltm_ir::model_ltm_reference_sites};

    let project = TestProject::new("colliding_element_name")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        // `region` is an ELEMENT of `Bucket` and also the name of a
        // DIMENSION -- the collision XMILE permits. It sits at position 1 so
        // the two readings pick different rows.
        .named_dimension("Bucket", &["old", "region"])
        .array_with_ranges_direct(
            "regw",
            vec!["Region".to_string()],
            vec![("nyc", "1"), ("boston", "2")],
            None,
        )
        .array_with_ranges_direct(
            "bucketw",
            vec!["Bucket".to_string()],
            vec![("old", "3"), ("region", "4")],
            None,
        )
        .array_aux_direct(
            "effect",
            vec!["Region".to_string(), "Bucket".to_string()],
            "regw[Region] * 10 + bucketw[Bucket]",
            None,
        )
        .array_aux_direct(
            "t",
            vec!["Region".to_string()],
            "effect[Region, region]",
            None,
        )
        .build_datamodel();

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("the colliding-name model compiles");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end().expect("simulation should run");
    let results = vm.into_results();
    let at = |name: &str| -> f64 {
        let off = compiled
            .offsets
            .iter()
            .find(|(k, _)| k.as_str() == name)
            .unwrap_or_else(|| panic!("no slot named {name}"))
            .1;
        results.iter().next_back().expect("a saved step")[*off]
    };
    // Non-vacuity: the two candidate reads must differ, or the oracle
    // cannot tell the orders apart.
    assert_ne!(at("effect[nyc,region]"), at("effect[nyc,old]"));
    assert_eq!(
        at("t[nyc]"),
        at("effect[nyc,region]"),
        "`effect[Region, region]` must read `Bucket`'s `region` ELEMENT -- \
         the axis's own element namespace wins inside square brackets"
    );
    assert_eq!(at("t[boston]"), at("effect[boston,region]"));

    // ...and the reference-site IR describes exactly that read.
    let sites = model_ltm_reference_sites(&db, sync.models["main"].source, sync.project);
    let shapes: Vec<&RefShape> = sites
        .sites
        .get(&("effect".to_string(), "t".to_string()))
        .map(|v| v.iter().map(|s| &s.shape).collect())
        .unwrap_or_default();
    assert_eq!(
        shapes,
        vec![&RefShape::PerElement {
            axes: vec![
                AxisRead::Iterated {
                    dim: "region".to_string(),
                    source_dim: "region".to_string(),
                },
                AxisRead::Pinned("region".to_string()),
            ],
        }],
        "the classified site must pin the `Bucket` axis to its literal \
         element, matching the read the VM just performed"
    );
}
