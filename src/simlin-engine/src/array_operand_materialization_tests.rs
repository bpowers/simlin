// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Codegen consumes an array-valued operand as a **view over storage**
//! (`compiler::codegen::walk_expr_as_view`), so a *computed* array -- `vals[d]
//! + bump[d]`, `NOT ...`, an `IF` over two arrays -- has to be evaluated into a
//! temp of its own before the builtin that reads it. GH #995: the shapes below
//! failed to compile at all, in ordinary hand-written apply-to-all models, with
//! LTM disabled. `compiler::array_operand` is the fix.
//!
//! # The enumeration these rows are derived from
//!
//! **Positions.** Every `walk_expr_as_view` call site in `codegen.rs` is one
//! row of the position axis; there is no other way for an operand to be
//! required to be a view. In source order:
//!
//! | call site | position | covered by |
//! |---|---|---|
//! | `emit_array_reduce` | `SUM`/`SIZE`/`STDDEV`/`MIN`(1-arg)/`MAX`(1-arg)/`MEAN`(1-arg) arg0 | [`reducer_positions`] |
//! | `VectorSelect` | arg0 (selection), arg1 (values) | [`vector_select_positions`] |
//! | `VectorElmMap` | arg0 (source), arg1 (offsets) | [`vector_elm_map_positions`] |
//! | `VectorSortOrder` | arg0 (array) | [`vector_sort_order_positions`] |
//! | `Rank` | arg0 (array) | [`rank_positions`] |
//! | `Lookup`/`LookupForward`/`LookupBackward` | arg0 (arrayed GF table) | [`deliberately_unmaterialized_positions`] |
//! | `AllocateAvailable` | arg0 (requests) | [`allocate_positions`] |
//! | `AllocateAvailable` | arg1 (priority profiles) | [`deliberately_unmaterialized_positions`] |
//! | `AllocateByPriority` | arg0 (requests), arg1 (priorities) | [`allocate_positions`] |
//!
//! Two positions are deliberately **not** materialized, and each is pinned as
//! still-failing rather than left unstated -- see
//! [`deliberately_unmaterialized_positions`] for the reasons, which live next
//! to the code in `compiler::array_operand`.
//!
//! **Shapes.** The materializer's decision is `is_view`, the negation of
//! `walk_expr_as_view`'s four accepting arms, plus "an array view can be
//! derived for it", minus "it contains an array-valued `PREVIOUS`/`INIT`". The
//! shape axis is therefore the set of *rejected* `compiler::Expr` variants that
//! can carry an array: `Op2`, `Op1`, `If`, and `App` (an elementwise builtin --
//! [`elementwise_builtin_operands_materialize`] covers the two families
//! `find_expr_array_view` recognises -- or a nested array-producing builtin).
//!
//! **The collapse.** Position and shape are decided by two separate, singly
//! implemented pieces of code -- one `match` over `BuiltinFn` naming the view
//! positions, and one shared `materialize_view_operand` that all of them call.
//! So the matrix is covered as a cross rather than as a full product: *every*
//! position is exercised with one computed shape (an `Op2`) plus its
//! already-compiling control, and *every* shape is exercised at one position
//! (`VECTOR SORT ORDER` arg0). The two spellings of an apply-to-all reference
//! -- `vals[d]`, which only means "the whole array" after `context.rs`'s
//! `with_vector_builtin_wildcards` promotion, and `vals[*]` -- are a third
//! axis, covered at `VECTOR SORT ORDER` arg0 and `RANK` arg0, the two arms the
//! issue reports separately.
//!
//! **Why some rows carry a `+ SUM(VECTOR SORT ORDER(vals[*], 1))` tail.** A
//! reducer or `VECTOR SELECT` argument only survives Pass 1 unmaterialized
//! when the equation *also* holds an array-producing builtin: that is what
//! makes `compiler::mod`'s apply-to-all hoister lower through
//! `lower_preserving_dimensions`, whose `Pass1Context` has no apply-to-all
//! context and so defers every operand carrying a dimension reference. The
//! tail is the smallest thing that forces that path; it contributes the
//! constant 1 + 2 + 0 = 3.
//!
//! **Values.** Every compiling row asserts VM numbers, chosen so that reading
//! the *wrong* array gives a different answer than reading the computed one:
//! `vals = [30, 10, 20]` and `vals + bump = [30, 110, 20]` have different sort
//! orders, different ranks and different element-map results, and each wrong
//! rule (read `vals` raw, read `bump` raw, collapse the operand to its first
//! element) lands on its own distinct answer. The value each wrong rule would
//! produce is written next to the assertion.
//!
//! `PREVIOUS`/`INIT` of an arrayed reference is Phase C3 and stays broken here
//! on purpose: a `BeginIter` body cannot emit a per-element `LoadPrev`. Those
//! rows assert the *attributed* failure (`NotSimulatable` naming the
//! variable), bare
//! ([`previous_and_init_operands_still_fail_attributed`]) and nested inside a
//! computed operand where a sibling could supply the shape
//! ([`nested_previous_and_init_operands_still_fail_attributed`]) -- so C3 has
//! red tests waiting rather than silence. The complement is a green row:
//! [`a_scalar_previous_beside_an_array_operand_still_materializes`], without
//! which the decline could be widened to "contains any `PREVIOUS`" unnoticed.

use crate::common::ErrorCode;
use crate::test_common::TestProject;

/// The shared fixture. Values are chosen so a computed operand's answer
/// differs from the answer produced by reading either raw input -- see the
/// module docs.
///
/// * `vals   = [30, 10, 20]`
/// * `bump   = [0, 100, 0]`   (so `vals + bump = [30, 110, 20]`)
/// * `offs   = [2, 0, 1]`
/// * `shift  = [-2, 1, 0]`    (so `offs + shift = [0, 1, 1]`)
/// * `sel    = [1, 1, 0]`
/// * `mask   = [1, 0, 0]`     (so `sel - mask = [0, 1, 0]`)
/// * `matrix = [[1, 2, 3], [10, 20, 30]]`
fn fixture(name: &str) -> TestProject {
    TestProject::new(name)
        .indexed_dimension("d", 3)
        .indexed_dimension("e", 2)
        .array_with_ranges("vals[d]", vec![("1", "30"), ("2", "10"), ("3", "20")])
        .array_with_ranges("bump[d]", vec![("1", "0"), ("2", "100"), ("3", "0")])
        .array_with_ranges("offs[d]", vec![("1", "2"), ("2", "0"), ("3", "1")])
        .array_with_ranges("shift[d]", vec![("1", "-2"), ("2", "1"), ("3", "0")])
        .array_with_ranges("sel[d]", vec![("1", "1"), ("2", "1"), ("3", "0")])
        .array_with_ranges("mask[d]", vec![("1", "1"), ("2", "0"), ("3", "0")])
        .array_with_ranges(
            "matrix[e,d]",
            vec![
                ("1,1", "1"),
                ("1,2", "2"),
                ("1,3", "3"),
                ("2,1", "10"),
                ("2,2", "20"),
                ("2,3", "30"),
            ],
        )
}

/// Compile `out[d] = <eqn>` against the shared fixture and return `out`.
fn out_of(name: &str, eqn: &str) -> Vec<f64> {
    let project = fixture(name).array_aux("out[d]", eqn);
    project.assert_compiles_incremental();
    project.vm_result_incremental("out")
}

/// Compile `out[e] = <eqn>` against the shared fixture and return `out`. Used
/// by the reducer rows, whose argument is a row slice `matrix[e,*]`.
fn row_out_of(name: &str, eqn: &str) -> Vec<f64> {
    let project = fixture(name).array_aux("out[e]", eqn);
    project.assert_compiles_incremental();
    project.vm_result_incremental("out")
}

fn assert_close(actual: &[f64], expected: &[f64], what: &str) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{what}: length mismatch, got {actual:?}"
    );
    for (i, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
        assert!(
            (a - e).abs() < 1e-9,
            "{what}: element {i} expected {e}, got {a} (whole array {actual:?})"
        );
    }
}

/// A row that is still expected to fail: it must fail, and it must still name
/// the variable it failed for (GH #994's attribution), not fail anonymously.
fn assert_fails_attributed(project: TestProject, what: &str) {
    let err = project
        .compile_incremental()
        .err()
        .unwrap_or_else(|| panic!("{what}: compiled, but this shape is not supposed to"));
    assert_eq!(
        err.code,
        ErrorCode::NotSimulatable,
        "{what}: expected a NotSimulatable rejection, got {err:?}"
    );
    let details = err.get_details().unwrap_or_default();
    assert!(
        details.contains("out"),
        "{what}: the rejection must name the variable it belongs to, got {details:?}"
    );
}

// ===========================================================================
// Position axis: one computed operand (an `Op2`) per `walk_expr_as_view` call
// site, plus the already-compiling control.
// ===========================================================================

#[test]
fn vector_sort_order_positions() {
    // Control: a plain reference is already a view, and already compiled.
    // `vals = [30, 10, 20]` ascending: 10@1, 20@2, 30@0.
    assert_close(
        &out_of("vso_ctl", "VECTOR SORT ORDER(vals[d], 1)"),
        &[1.0, 2.0, 0.0],
        "control: VECTOR SORT ORDER over a direct reference",
    );

    // Computed: `vals + bump = [30, 110, 20]` ascending: 20@2, 30@0, 110@1.
    // Reading `vals` raw would give [1, 2, 0]; reading `bump` raw would give
    // [0, 2, 1]; collapsing to a 1-element view would give [0, 0, 0].
    assert_close(
        &out_of("vso_arg0", "VECTOR SORT ORDER(vals[d] + bump[d], 1)"),
        &[2.0, 0.0, 1.0],
        "VECTOR SORT ORDER arg0, computed",
    );
}

#[test]
fn rank_positions() {
    // Control. RANK is 1-based: `vals = [30, 10, 20]` ascending ranks are
    // 30 -> 3, 10 -> 1, 20 -> 2.
    assert_close(
        &out_of("rank_ctl", "RANK(vals[d], 1)"),
        &[3.0, 1.0, 2.0],
        "control: RANK over a direct reference",
    );

    // Computed: `[30, 110, 20]` ascending ranks are 30 -> 2, 110 -> 3,
    // 20 -> 1. Reading `vals` raw would give [3, 1, 2]; reading `bump` raw
    // would give [1, 3, 2]; a collapsed view would give [1, 1, 1].
    assert_close(
        &out_of("rank_arg0", "RANK(vals[d] + bump[d], 1)"),
        &[2.0, 3.0, 1.0],
        "RANK arg0, computed",
    );
}

#[test]
fn vector_elm_map_positions() {
    // Control: `offs = [2, 0, 1]` maps `vals = [30, 10, 20]` to
    // [vals[2], vals[0], vals[1]] = [20, 30, 10].
    assert_close(
        &out_of("elm_ctl", "VECTOR ELM MAP(vals[d], offs[d])"),
        &[20.0, 30.0, 10.0],
        "control: VECTOR ELM MAP over direct references",
    );

    // arg0 computed: the source becomes [30, 110, 20], mapped by
    // offs = [2, 0, 1] to [20, 30, 110]. Reading `vals` raw would give
    // [20, 30, 10]; reading `bump` raw would give [0, 0, 100].
    assert_close(
        &out_of("elm_arg0", "VECTOR ELM MAP(vals[d] + bump[d], offs[d])"),
        &[20.0, 30.0, 110.0],
        "VECTOR ELM MAP arg0 (source), computed",
    );

    // arg1 computed: the offsets become `offs + shift = [0, 1, 1]`, so the
    // result is [vals[0], vals[1], vals[1]] = [30, 10, 10]. Reading `offs` raw
    // would give [20, 30, 10]; reading `shift` raw would put element 0 at
    // offset -2, which is out of range and yields NaN.
    assert_close(
        &out_of("elm_arg1", "VECTOR ELM MAP(vals[d], offs[d] + shift[d])"),
        &[30.0, 10.0, 10.0],
        "VECTOR ELM MAP arg1 (offsets), computed",
    );
}

#[test]
fn vector_select_positions() {
    // VECTOR SELECT reduces to a scalar, so every element of `out` holds the
    // same value; the `+ SUM(VECTOR SORT ORDER(vals[*], 1))` tail adds 3 and
    // is what forces the lowering path these rows are about (module docs).
    //
    // Control: `sel = [1, 1, 0]` selects vals[0] + vals[1] = 40, plus 3.
    assert_close(
        &out_of(
            "vsel_ctl",
            "VECTOR SELECT(sel[d], vals[d], 0, 0, 0) + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        &[43.0, 43.0, 43.0],
        "control: VECTOR SELECT over direct references",
    );

    // arg0 computed: `sel - mask = [0, 1, 0]` selects vals[1] = 10, plus 3.
    // Reading `sel` raw would give 43; reading `mask` raw would give 33; a
    // collapsed 1-element view would select nothing and fall back to the
    // max_value argument, giving 3.
    assert_close(
        &out_of(
            "vsel_arg0",
            "VECTOR SELECT(sel[d] - mask[d], vals[d], 0, 0, 0) + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        &[13.0, 13.0, 13.0],
        "VECTOR SELECT arg0 (selection array), computed",
    );

    // arg1 computed: `sel = [1, 1, 0]` over `vals + bump = [30, 110, 20]`
    // selects 30 + 110 = 140, plus 3. Reading `vals` raw would give 43;
    // reading `bump` raw would give 103.
    assert_close(
        &out_of(
            "vsel_arg1",
            "VECTOR SELECT(sel[d], vals[d] + bump[d], 0, 0, 0) + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        &[143.0, 143.0, 143.0],
        "VECTOR SELECT arg1 (value array), computed",
    );
}

#[test]
fn reducer_positions() {
    // The five `emit_array_reduce` arms, over the row slice `matrix[e,*]`
    // (rows [1, 2, 3] and [10, 20, 30]). A reducer keeps its argument as a row
    // slice -- `with_preserved_wildcards` does NOT promote an active-dimension
    // reference -- so `matrix[e,*] * 2` is [2, 4, 6] and [20, 40, 60]. Each
    // row carries the `+ SUM(VECTOR SORT ORDER(vals[*], 1))` tail, worth 3.

    // Control: SUM of the raw rows is 6 and 60.
    assert_close(
        &row_out_of(
            "red_ctl",
            "SUM(matrix[e,*]) + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        &[9.0, 63.0],
        "control: SUM over a direct row slice",
    );

    // SUM: 12 and 120. Reading `matrix` raw would give 9 and 63.
    assert_close(
        &row_out_of(
            "red_sum",
            "SUM(matrix[e,*] * 2) + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        &[15.0, 123.0],
        "SUM over a computed array",
    );

    // MAX: 6 and 60. Reading `matrix` raw would give 6 and 33.
    assert_close(
        &row_out_of(
            "red_max",
            "MAX(matrix[e,*] * 2) + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        &[9.0, 63.0],
        "MAX over a computed array",
    );

    // MIN: 2 and 20. Reading `matrix` raw would give 4 and 13.
    assert_close(
        &row_out_of(
            "red_min",
            "MIN(matrix[e,*] * 2) + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        &[5.0, 23.0],
        "MIN over a computed array",
    );

    // SIZE counts elements: 3 either way. A collapsed operand would give 1,
    // which is the failure mode this row rules out.
    assert_close(
        &row_out_of(
            "red_size",
            "SIZE(matrix[e,*] * 2) + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        &[6.0, 6.0],
        "SIZE over a computed array",
    );

    // STDDEV is the POPULATION deviation (`ArrayStddev` divides by n, not
    // n - 1): sqrt(8/3) for [2, 4, 6] and sqrt(800/3) for [20, 40, 60].
    // Reading `matrix` raw would halve both.
    let pop_stddev = |xs: [f64; 3]| -> f64 {
        let mean = (xs[0] + xs[1] + xs[2]) / 3.0;
        (xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / 3.0).sqrt()
    };
    assert_close(
        &row_out_of(
            "red_stddev",
            "STDDEV(matrix[e,*] * 2) + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        &[
            pop_stddev([2.0, 4.0, 6.0]) + 3.0,
            pop_stddev([20.0, 40.0, 60.0]) + 3.0,
        ],
        "STDDEV over a computed array",
    );

    // Single-argument MEAN: means of [2,4,6] and [20,40,60] are 4 and 40.
    // Reading `matrix` raw would give 2 and 20. This row is the one that
    // makes MEAN agree with its four sibling reducers: before, an array-shaped
    // MEAN argument fell through codegen's scalar fallback and failed to
    // compile, so the `[*]` spelling array-meaned through Pass 1 while the
    // `[e,*]` spelling did not compile at all.
    assert_close(
        &row_out_of(
            "red_mean",
            "MEAN(matrix[e,*] * 2) + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        &[7.0, 43.0],
        "MEAN over a computed array",
    );
    // The variadic form has no view position and must be untouched: MEAN of
    // three scalars is their average, 2, plus the tail.
    assert_close(
        &row_out_of(
            "red_mean_variadic",
            "MEAN(matrix[e,1], matrix[e,2], matrix[e,3]) + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        &[5.0, 23.0],
        "variadic MEAN is a scalar average and takes no view",
    );
}

/// The ALLOCATE fixture: three requesters, a rectangular priority-profile
/// array, and the two "bump" arrays the computed-operand rows add in.
fn allocate_fixture(name: &str) -> TestProject {
    TestProject::new(name)
        .indexed_dimension("d", 3)
        .indexed_dimension("xp", 4)
        .array_with_ranges("request[d]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_with_ranges("extra[d]", vec![("1", "20"), ("2", "0"), ("3", "0")])
        .array_with_ranges("priority[d]", vec![("1", "3"), ("2", "1"), ("3", "2")])
        .array_with_ranges("prio_bump[d]", vec![("1", "0"), ("2", "5"), ("3", "0")])
        .array_with_ranges(
            "pp[d,xp]",
            vec![
                ("1,1", "1"),
                ("1,2", "3"),
                ("1,3", "1"),
                ("1,4", "0"),
                ("2,1", "1"),
                ("2,2", "1"),
                ("2,3", "1"),
                ("2,4", "0"),
                ("3,1", "1"),
                ("3,2", "2"),
                ("3,3", "1"),
                ("3,4", "0"),
            ],
        )
        .scalar_const("supply", 35.0)
        .scalar_const("width", 1.0)
}

/// `ALLOCATE AVAILABLE` and `ALLOCATE BY PRIORITY` run a bisection over
/// per-requester allocation curves, so their element values are not
/// hand-computable the way a sort order is. Each row instead pins the
/// computed-operand model against the model that materializes the same array
/// into a named variable first -- the path that already compiled -- and
/// separately asserts it differs from the raw-operand model, so "the
/// computation was actually read" is asserted rather than assumed.
#[test]
fn allocate_positions() {
    struct Row {
        what: &'static str,
        computed: &'static str,
        helper: (&'static str, &'static str),
        reference: &'static str,
        raw: &'static str,
    }

    let rows = [
        Row {
            what: "allocate_available arg0 (requests)",
            computed: "allocate_available(request[d] + extra[d], pp[d,1], supply)",
            helper: ("req2[d]", "request[d] + extra[d]"),
            reference: "allocate_available(req2[d], pp[d,1], supply)",
            raw: "allocate_available(request[d], pp[d,1], supply)",
        },
        Row {
            what: "allocate_by_priority arg0 (requests)",
            computed: "allocate_by_priority(request[d] + extra[d], priority[d], 0, width, supply)",
            helper: ("req2[d]", "request[d] + extra[d]"),
            reference: "allocate_by_priority(req2[d], priority[d], 0, width, supply)",
            raw: "allocate_by_priority(request[d], priority[d], 0, width, supply)",
        },
        Row {
            what: "allocate_by_priority arg1 (priorities)",
            computed: "allocate_by_priority(request[d], priority[d] + prio_bump[d], 0, width, supply)",
            helper: ("prio2[d]", "priority[d] + prio_bump[d]"),
            reference: "allocate_by_priority(request[d], prio2[d], 0, width, supply)",
            raw: "allocate_by_priority(request[d], priority[d], 0, width, supply)",
        },
    ];

    for (i, row) in rows.iter().enumerate() {
        let computed = allocate_fixture(&format!("alloc_c{i}")).array_aux("out[d]", row.computed);
        computed.assert_compiles_incremental();
        let computed = computed.vm_result_incremental("out");

        let reference = allocate_fixture(&format!("alloc_r{i}"))
            .array_aux(row.helper.0, row.helper.1)
            .array_aux("out[d]", row.reference);
        reference.assert_compiles_incremental();
        let reference = reference.vm_result_incremental("out");

        let raw = allocate_fixture(&format!("alloc_w{i}")).array_aux("out[d]", row.raw);
        raw.assert_compiles_incremental();
        let raw = raw.vm_result_incremental("out");

        assert_close(
            &computed,
            &reference,
            &format!(
                "{}: the inline computed operand must allocate exactly as the \
                 pre-materialized helper does",
                row.what
            ),
        );
        assert_ne!(
            computed, raw,
            "{}: the fixture must make the computed operand change the answer, \
             otherwise this row proves nothing (computed {computed:?}, raw {raw:?})",
            row.what
        );
    }
}

/// The one view position the materializer deliberately declines, plus the two
/// arms that decline for a reason other than the position. The reasons live on
/// the arms in `compiler::array_operand::materialize_view_operands`; what is
/// pinned here is that each still fails loudly, or keeps its existing meaning,
/// rather than compiling to something wrong.
///
/// The arrayed graphical-function table has no row: it is not constructible as
/// a computed expression from the equation language. `Lookup`'s table argument
/// is synthesized -- by `apply_implicit_with_lookup` for WITH LOOKUP and by the
/// table-holder resolution for `g[D!](x)` -- and is always a bare reference, so
/// the declining arm guards against a future producer rather than a shape
/// reachable today. (`SUM(LOOKUP(vals[d] * 2, 1))` is rejected earlier, at
/// table resolution: `vals` is not a graphical function at all.)
#[test]
fn deliberately_unmaterialized_positions() {
    // ALLOCATE AVAILABLE's priority-profile argument: its view is rewritten
    // by `context::expand_pp_view_for_allocate`, which re-expands a collapsed
    // `pp[d,1]` to the variable's full requester x XPriority array. That
    // helper only understands a direct variable reference, so materializing a
    // computed profile would silently hand the VM a one-column-per-requester
    // temp.
    let pp_computed = TestProject::new("unmat_pp")
        .indexed_dimension("d", 3)
        .indexed_dimension("xp", 4)
        .array_with_ranges("request[d]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_const("pp[d,xp]", 1.0)
        .array_const("pp_bump[d,xp]", 0.0)
        .scalar_const("supply", 35.0)
        .array_aux(
            "out[d]",
            "allocate_available(request[d], pp[d,1] + pp_bump[d,1], supply)",
        );
    assert_fails_attributed(
        pp_computed,
        "ALLOCATE AVAILABLE priority profiles, computed",
    );

    // A GENUINELY scalar argument is left alone, because no array view can be
    // derived for it -- `matrix[e,1] * 2` is two scalars. MEAN is the only
    // reduce arm this is observable through: codegen's `Mean` arm emits a
    // plain scalar walk for anything that is not one of the four view shapes,
    // where `emit_array_reduce` (SUM/SIZE/STDDEV/MIN/MAX) pushes a view
    // unconditionally and rejects a scalar expression with or without this
    // pass. So the row that matters is MEAN's, and it must keep its value.
    assert_close(
        &row_out_of(
            "unmat_mean_scalar",
            "MEAN(matrix[e,1] * 2) + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        &[5.0, 23.0],
        "MEAN of a computed SCALAR stays a scalar mean",
    );
}

// ===========================================================================
// Shape axis: every rejected `Expr` variant that can carry an array, at one
// position (VECTOR SORT ORDER arg0).
// ===========================================================================

#[test]
fn computed_operand_shapes() {
    // Op2 -- also covered by `vector_sort_order_positions`, restated here so
    // the shape enumeration is complete in one place.
    assert_close(
        &out_of("shape_op2", "VECTOR SORT ORDER(vals[d] + bump[d], 1)"),
        &[2.0, 0.0, 1.0],
        "shape Op2",
    );

    // Op1. Unary minus lowers to `Op2(Sub, 0, x)` and `Transpose` is folded
    // during lowering, so `NOT` is the only `Expr::Op1` a fragment can carry.
    // `NOT (vals[d] > 15)` = [0, 1, 0]; ascending with stable ties that is
    // 0@0, 0@2, 1@1 -> [0, 2, 1]. Reading `vals` raw would give [1, 2, 0].
    assert_close(
        &out_of("shape_op1", "VECTOR SORT ORDER(NOT (vals[d] > 15), 1)"),
        &[0.0, 2.0, 1.0],
        "shape Op1 (NOT)",
    );

    // If, selecting between two arrays. `IF sel[d] > 0 THEN bump[d] ELSE
    // vals[d]` = [0, 100, 20]; ascending that is 0@0, 20@2, 100@1 ->
    // [0, 2, 1]. Reading `vals` alone would give [1, 2, 0].
    assert_close(
        &out_of(
            "shape_if",
            "VECTOR SORT ORDER(IF sel[d] > 0 THEN bump[d] ELSE vals[d], 1)",
        ),
        &[0.0, 2.0, 1.0],
        "shape If",
    );

    // App, elementwise: `ABS(vals[d] - 25)` = [5, 15, 5]; ascending with
    // stable ties that is 5@0, 5@2, 15@1 -> [0, 2, 1]. Reading `vals` raw
    // would give [1, 2, 0].
    assert_close(
        &out_of("shape_app", "VECTOR SORT ORDER(ABS(vals[d] - 25), 1)"),
        &[0.0, 2.0, 1.0],
        "shape App (elementwise builtin)",
    );

    // App, a nested array-producing builtin. The inner ELM MAP yields
    // [vals[2], vals[0], vals[1]] = [20, 30, 10]; ascending that is
    // 10@2, 20@0, 30@1 -> [2, 0, 1].
    assert_close(
        &out_of(
            "shape_nested",
            "VECTOR SORT ORDER(VECTOR ELM MAP(vals[d], offs[d]), 1)",
        ),
        &[2.0, 0.0, 1.0],
        "shape App (nested array-producing builtin)",
    );
}

/// The elementwise scalar builtins `find_expr_array_view` recognises: an
/// operand whose outermost node is one of them takes the shape of whichever
/// argument has one, so it materializes like any other computed array. Two
/// representatives of the two families it grew for this work -- a
/// single-argument one (`SIGN`) and a multi-argument one (two-argument `MAX`).
///
/// `SIGN(vals[d] - 15)` = [1, -1, 1]; ascending with stable ties that is
/// -1@1, 1@0, 1@2 -> [1, 0, 2]. Reading `vals` raw would give [1, 2, 0].
/// `MAX(bump[d], vals[d])` = [30, 100, 20]; ascending -> [2, 0, 1]. Reading
/// `vals` raw gives [1, 2, 0], reading `bump` raw gives [0, 2, 1].
#[test]
fn elementwise_builtin_operands_materialize() {
    assert_close(
        &out_of("elemwise_sign", "VECTOR SORT ORDER(SIGN(vals[d] - 15), 1)"),
        &[1.0, 0.0, 2.0],
        "SIGN operand (single-argument elementwise)",
    );
    assert_close(
        &out_of(
            "elemwise_max2",
            "VECTOR SORT ORDER(MAX(bump[d], vals[d]), 1)",
        ),
        &[2.0, 0.0, 1.0],
        "two-argument MAX operand (multi-argument elementwise)",
    );
}

/// Materializing a `VECTOR ELM MAP` **source** changes which storage the
/// mapping ranges over, and this pins the choice rather than leaving it
/// accidental.
///
/// Genuine Vensim maps over the source VARIABLE's full row-major storage from
/// the base arg-1's element reference establishes, and `vm_vector_elm_map.rs`
/// implements that with a `source_is_full_array` test: a strict slice such as
/// `matrix[1,*]` keeps a per-element base and CAN read past the end of its own
/// row into the next one. A materialized operand is a fresh contiguous temp,
/// so it is full-array by construction and the mapping is confined to the
/// computed array.
///
/// `matrix` is [[1,2,3],[10,20,30]] (flat storage of 6) and `far` is [3,4,5].
/// Over the row-1 slice those offsets run off the end of row 1 and into row 2;
/// over a 3-element temp they are all out of range and yield `:NA:`.
#[test]
fn materializing_an_elm_map_source_confines_the_mapping_to_the_temp() {
    let far = || {
        fixture("elm_base").array_with_ranges("far[d]", vec![("1", "3"), ("2", "4"), ("3", "5")])
    };

    // The direct slice keeps genuine Vensim's full-variable rule. Recorded as
    // the contrast, and as a tripwire if that rule ever moves.
    let slice = far().array_aux("out[d]", "VECTOR ELM MAP(matrix[1,*], far[d])");
    slice.assert_compiles_incremental();
    let slice = slice.vm_result_incremental("out");
    assert_eq!(slice.len(), 3);
    assert!(
        (slice[0] - 10.0).abs() < 1e-9 && (slice[1] - 30.0).abs() < 1e-9 && slice[2].is_nan(),
        "a direct strict-slice source maps over the whole variable, got {slice:?}"
    );

    // The computed source is a temp of its own, so every offset is out of its
    // range.
    let computed = far().array_aux("out[d]", "VECTOR ELM MAP(matrix[1,*] * 1, far[d])");
    computed.assert_compiles_incremental();
    let computed = computed.vm_result_incremental("out");
    assert!(
        computed.len() == 3 && computed.iter().all(|v| v.is_nan()),
        "a materialized source confines the mapping to the computed array, got {computed:?}"
    );
}

/// C1: `Pass1Context`'s `Rank` arm decomposes its array argument like all five
/// of its siblings.
///
/// This is not observable from VM values -- the post-lowering pass materializes
/// the same operand either way, and the numbers agree -- so it is pinned at the
/// lowered-fragment level, through the production `Var::new` lowering that
/// `build_module` drives. What differs is WHERE the temp is allocated: Pass 1
/// numbers the operand's temp before the apply-to-all hoister numbers the
/// builtin's result, while the post-lowering pass continues past the highest id
/// the fragment already uses, so the two temps come out in the opposite order.
///
/// Stating it as "RANK's fragment has the same temp structure as VECTOR SORT
/// ORDER's" is the actual C1 claim (arm consistency) and reds if the arm
/// reverts to `transform_inner`.
#[test]
fn the_rank_arm_decomposes_its_array_argument_like_its_siblings() {
    fn assign_temp_ids(name: &str, eqn: &str) -> Vec<u32> {
        use crate::compiler::expr::Expr;
        fixture(name)
            .array_aux("out[d]", eqn)
            .build_module()
            .unwrap_or_else(|e| panic!("{name} should build: {e}"))
            .runlist_flows
            .iter()
            .filter_map(|e| match e {
                Expr::AssignTemp(id, _, _) => Some(*id),
                _ => None,
            })
            .collect()
    }

    let vso = assign_temp_ids("c1_vso", "VECTOR SORT ORDER(vals[*] + bump[*], 1)");
    let rank = assign_temp_ids("c1_rank", "RANK(vals[*] + bump[*], 1)");
    assert_eq!(
        vso,
        vec![0, 1],
        "the sibling arm decomposes in Pass 1: operand temp 0, then the \
         builtin's own result temp 1"
    );
    assert_eq!(
        rank, vso,
        "RANK must decompose its array argument in Pass 1 like VECTOR SORT \
         ORDER does; a fragment numbered the other way means the arm fell \
         through to the post-lowering pass instead"
    );
}

// ===========================================================================
// Spelling axis: `vals[d]` (needs the ActiveDimRef -> Wildcard promotion) and
// `vals[*]`, at the two arms the issue reports separately.
// ===========================================================================

#[test]
fn both_apply_to_all_spellings_materialize() {
    // `VECTOR SORT ORDER`'s star spelling reaches Pass 1 with no unresolved
    // dimension reference, so it already compiled; the active-dimension
    // spelling did not. Both must now agree.
    assert_close(
        &out_of("spell_vso_star", "VECTOR SORT ORDER(vals[*] + bump[*], 1)"),
        &[2.0, 0.0, 1.0],
        "VECTOR SORT ORDER, star spelling",
    );
    assert_close(
        &out_of("spell_vso_dim", "VECTOR SORT ORDER(vals[d] + bump[d], 1)"),
        &[2.0, 0.0, 1.0],
        "VECTOR SORT ORDER, active-dimension spelling",
    );

    // `RANK` is the arm whose Pass-1 recursion never called
    // `maybe_decompose_array_arg_inner`, unlike all five of its siblings, so
    // BOTH spellings failed. Ranks of [30, 110, 20] ascending: 2, 3, 1.
    assert_close(
        &out_of("spell_rank_star", "RANK(vals[*] + bump[*], 1)"),
        &[2.0, 3.0, 1.0],
        "RANK, star spelling",
    );
    assert_close(
        &out_of("spell_rank_dim", "RANK(vals[d] + bump[d], 1)"),
        &[2.0, 3.0, 1.0],
        "RANK, active-dimension spelling",
    );
}

// ===========================================================================
// Phase C3's red rows: `PREVIOUS`/`INIT` of an arrayed reference.
// ===========================================================================

/// A `BeginIter` body cannot emit a per-element `LoadPrev` -- that opcode
/// needs a static slot -- so an array-valued `PREVIOUS`/`INIT` operand is not
/// materializable and must keep failing loudly until Phase C3 gives it a
/// snapshot-buffer view. These rows are the issue's own table, and they turn
/// red the moment C3 lands.
#[test]
fn previous_and_init_operands_still_fail_attributed() {
    let rows = [
        ("c3_vso_prev", "VECTOR SORT ORDER(PREVIOUS(vals[d]), 1)"),
        ("c3_vso_init", "VECTOR SORT ORDER(INIT(vals[d]), 1)"),
        ("c3_rank_prev", "RANK(PREVIOUS(vals[d]), 1)"),
        (
            "c3_elm_src_prev",
            "VECTOR ELM MAP(PREVIOUS(vals[d]), offs[d])",
        ),
        (
            "c3_elm_off_prev",
            "VECTOR ELM MAP(vals[d], PREVIOUS(offs[d]))",
        ),
        (
            "c3_select_prev",
            "VECTOR SELECT(PREVIOUS(sel[d]), vals[d], 0, 0, 0) \
             + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
    ];
    for (name, eqn) in rows {
        assert_fails_attributed(fixture(name).array_aux("out[d]", eqn), eqn);
    }
}

/// The same boundary one level down, and the row this work nearly got wrong:
/// a `PREVIOUS`/`INIT` of an arrayed reference *nested inside* a computed
/// operand.
///
/// A bare `PREVIOUS(vals[d])` operand declines because no array view can be
/// derived for it. Nested, it can borrow a shape from its sibling --
/// `find_expr_array_view` on an `Op2` takes `lhs.or_else(rhs)` -- and would
/// materialize into a temp whose body reads `previous(vals@0 + view(dims: [],
/// offset: k))`, ONE element's previous value broadcast across the whole
/// array. That is a plausible array of wrong numbers replacing a loud failure
/// (measured: `[0, 2, 1]` where the correct answer is `[2, 0, 1]`), so
/// `contains_element_collapsed_prev_or_init` declines the whole operand.
///
/// Rows are derived from the same position enumeration as the compiling ones,
/// so every view position is covered nested as well as bare. Phase C3 flips
/// these to values; it should not delete them.
#[test]
fn nested_previous_and_init_operands_still_fail_attributed() {
    // Every view position from the module-doc table, with the PREVIOUS/INIT
    // buried under arithmetic that supplies the shape.
    let rows = [
        (
            "c3n_vso",
            "VECTOR SORT ORDER(PREVIOUS(vals[d]) + bump[d], 1)",
        ),
        // Operand order must not matter: `lhs.or_else(rhs)` finds the shape on
        // whichever side has one.
        (
            "c3n_vso_rhs",
            "VECTOR SORT ORDER(bump[d] + PREVIOUS(vals[d]), 1)",
        ),
        (
            "c3n_vso_init",
            "VECTOR SORT ORDER(INIT(vals[d]) + bump[d], 1)",
        ),
        ("c3n_rank", "RANK(PREVIOUS(vals[d]) + bump[d], 1)"),
        (
            "c3n_elm_src",
            "VECTOR ELM MAP(PREVIOUS(vals[d]) + bump[d], offs[d])",
        ),
        (
            "c3n_elm_off",
            "VECTOR ELM MAP(vals[d], PREVIOUS(offs[d]) + shift[d])",
        ),
        (
            "c3n_select_sel",
            "VECTOR SELECT(PREVIOUS(sel[d]) - mask[d], vals[d], 0, 0, 0) \
             + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        (
            "c3n_select_val",
            "VECTOR SELECT(sel[d], PREVIOUS(vals[d]) + bump[d], 0, 0, 0) \
             + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        // The reducer positions reach the SAME wall, but earlier and for a
        // different reason, so they are listed here to keep the position
        // enumeration complete rather than to exercise the decline: a reducer
        // argument is lowered with `with_preserved_wildcards`, which does NOT
        // promote, so `PREVIOUS(matrix[e,*])` never reaches the materializer
        // -- `builtins_visitor`'s capture-helper synthesis rejects the array
        // slice first. The failure names the synthesized helpers, which carry
        // the consuming variable's name.
        (
            "c3n_sum",
            "SUM(PREVIOUS(matrix[e,*]) + matrix[e,*]) + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        (
            "c3n_mean",
            "MEAN(PREVIOUS(matrix[e,*]) + matrix[e,*]) + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
    ];
    for (name, eqn) in rows {
        // The reducer rows are shaped over `e`; the vector-builtin rows over
        // `d`. Pick by which dimension the equation actually ranges on.
        let project = if eqn.contains("matrix[e,") {
            fixture(name).array_aux("out[e]", eqn)
        } else {
            fixture(name).array_aux("out[d]", eqn)
        };
        assert_fails_attributed(project, eqn);
    }

    // ALLOCATE's two materialized positions, on the allocate fixture.
    for (name, eqn) in [
        (
            "c3n_alloc_req",
            "allocate_available(PREVIOUS(request[d]) + extra[d], pp[d,1], supply)",
        ),
        (
            "c3n_alloc_pri",
            "allocate_by_priority(request[d], PREVIOUS(priority[d]) + prio_bump[d], 0, width, supply)",
        ),
    ] {
        assert_fails_attributed(allocate_fixture(name).array_aux("out[d]", eqn), eqn);
    }
}

/// The other side of that boundary: a `PREVIOUS`/`INIT` of a genuinely SCALAR
/// variable beside an array operand lowers to an `Expr::Var`, carries no
/// per-element identity, and broadcasts correctly -- so it must keep
/// materializing. Without this row the decline could be widened to "contains
/// any PREVIOUS" and nothing would notice.
///
/// `PREVIOUS(s)` is the constant 5 at every step, so the operand keeps `vals`'
/// ascending order, `[1, 2, 0]`.
#[test]
fn a_scalar_previous_beside_an_array_operand_still_materializes() {
    let project = fixture("scalar_prev")
        .scalar_const("s", 5.0)
        .array_aux("out[d]", "VECTOR SORT ORDER(vals[d] + PREVIOUS(s), 1)");
    project.assert_compiles_incremental();
    assert_close(
        &project.vm_result_incremental("out"),
        &[1.0, 2.0, 0.0],
        "a scalar PREVIOUS beside an array operand",
    );

    let init = fixture("scalar_init")
        .scalar_const("s", 5.0)
        .array_aux("out[d]", "VECTOR SORT ORDER(vals[d] + INIT(s), 1)");
    init.assert_compiles_incremental();
    assert_close(
        &init.vm_result_incremental("out"),
        &[1.0, 2.0, 0.0],
        "a scalar INIT beside an array operand",
    );

    // The boundary case the predicate has to get right: a PREVIOUS of a fixed
    // element spelled `matrix[e,1]` (2-D, literal trailing index) lowers to an
    // `Expr::Var` at that element's slot, and broadcasting it across the
    // reduced row is exactly what the equation says. Declining on "contains
    // any PREVIOUS" would break this. Measured for THIS spelling only: the
    // 1-D literal-index spelling `PREVIOUS(vals[2])` lowers to a
    // `StaticSubscript` instead, so the predicate declines it -- loud, and no
    // regression, since an operand containing an App was never a view shape
    // and did not compile before this pass either.
    //
    // Row 0: prev(matrix[0,0]) = 1, so SUM over [1,2,3] of (1 + x) = 9;
    // row 1: prev(matrix[1,0]) = 10, so SUM over [10,20,30] of (10 + x) = 90.
    // Plus the array-producing tail, worth 3. NOTE the value assertion here is
    // a compile-boundary pin, not a discriminator: matrix is constant, so the
    // same equation without the PREVIOUS returns the identical [12, 93] -- what
    // this row defends against is the decline being widened until the shape
    // stops compiling, and the values merely confirm the broadcast reading.
    let fixed_element = fixture("scalar_prev_elem").array_aux(
        "out[e]",
        "SUM(PREVIOUS(matrix[e,1]) + matrix[e,*]) + SUM(VECTOR SORT ORDER(vals[*], 1))",
    );
    fixed_element.assert_compiles_incremental();
    assert_close(
        &fixed_element.vm_result_incremental("out"),
        &[12.0, 93.0],
        "a PREVIOUS of a fixed element broadcasts correctly and must not decline",
    );
}

// ===========================================================================
// The safety property the whole pass rests on.
// ===========================================================================

/// The materializer must fire *only* where codegen would have rejected the
/// operand, so a fragment that compiled before is unchanged after. The
/// observable version of that claim: an operand that is already one of
/// `walk_expr_as_view`'s four accepted shapes consumes no extra temp, and a
/// computed one costs exactly one.
///
/// `temp_sizes` is derived from the lowered expressions, so it is the direct
/// readout of how many temps a fragment allocates.
#[test]
fn a_computed_operand_costs_exactly_one_temp_and_a_view_costs_none() {
    let temps = |name: &str, eqn: &str| -> usize {
        fixture(name)
            .array_aux("out[d]", eqn)
            .build_module()
            .unwrap_or_else(|e| panic!("{name} should build: {e}"))
            .temp_sizes
            .len()
    };

    let control = temps("temp_ctl", "VECTOR SORT ORDER(vals[d], 1)");
    assert_eq!(
        control, 1,
        "a direct-reference VECTOR SORT ORDER needs exactly the one temp its \
         own result lives in"
    );
    assert_eq!(
        temps("temp_computed", "VECTOR SORT ORDER(vals[d] + bump[d], 1)"),
        control + 1,
        "materializing a computed operand costs exactly one temp beyond the \
         builtin's own result"
    );
    assert_eq!(
        temps(
            "temp_shape_nested",
            "VECTOR SORT ORDER(ABS(vals[d] - 25), 1)"
        ),
        control + 1,
        "an elementwise builtin operand costs the same one temp -- the \
         materializer allocates per operand, not per node"
    );
}

/// The hoisted `AssignTemp` must be spliced in FRONT of the expression that
/// reads it. Nothing about a constant model can tell: a temp written after its
/// reader still holds the right value from the previous step, and at step 0 a
/// zeroed temp can coincide with the answer. So this row makes the operand
/// vary with time and reads the per-element series, where a stale temp is a
/// visibly different array at every step but the last.
///
/// `vals + bump * TIME` is [30,10,20] at t=0, [30,110,20] at t=1 and
/// [30,210,20] at t=2, so the ascending order moves from [1,2,0] to [2,0,1]
/// and stays. Read one step late it would be [0,1,2] (a zeroed temp sorts to
/// the identity under stable ties), then [1,2,0], then [2,0,1].
#[test]
fn the_hoisted_assignment_is_emitted_before_its_reader() {
    let project = fixture("hoist_order")
        .with_sim_time(0.0, 2.0, 1.0)
        .array_aux("out[d]", "VECTOR SORT ORDER(vals[d] + bump[d] * TIME, 1)");
    project.assert_compiles_incremental();
    let all = project.run_vm_incremental();
    let series = |elem: usize| -> Vec<f64> {
        all.get(&format!("out[{elem}]"))
            .unwrap_or_else(|| panic!("out[{elem}] missing from {:?}", all.keys()))
            .clone()
    };
    // Element-major: out[1] over t = 0, 1, 2 and so on. Written out as the
    // three per-step arrays for readability: [1,2,0], [2,0,1], [2,0,1].
    assert_close(&series(1), &[1.0, 2.0, 2.0], "out[1] over time");
    assert_close(&series(2), &[2.0, 0.0, 0.0], "out[2] over time");
    assert_close(&series(3), &[0.0, 1.0, 1.0], "out[3] over time");
}

// ===========================================================================
// The `TempId` namespace (GH #583).
//
// The per-element hoisting path allocates one temp per array ELEMENT -- each
// element re-evaluates the builtin with its own scalar argument -- and
// materializing a computed operand doubles that. `TempId` is a `u8`, so a few
// hundred elements is past the namespace, and BOTH tests below live at that
// boundary.
// ===========================================================================

/// A per-element hoist over `sort_project`'s dimension: `vals` descends and the
/// operand `301 - vals[d]` ascends, so the two readings are exact swaps at
/// every element. Element `k` sorts ascending when `k` is odd.
fn sort_project(name: &str, n: usize, eqn: &str) -> TestProject {
    fn refs(v: &[(String, String)]) -> Vec<(&str, &str)> {
        v.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect()
    }
    // vals[j] = n - 1 - j, i.e. [n-1, ..., 0]; `301 - vals[d]` is increasing.
    let vals: Vec<(String, String)> = (0..n)
        .map(|j| ((j + 1).to_string(), (n - 1 - j).to_string()))
        .collect();
    let dir: Vec<(String, String)> = (0..n)
        .map(|k| {
            (
                (k + 1).to_string(),
                if k % 2 == 1 { "1" } else { "-1" }.to_string(),
            )
        })
        .collect();
    TestProject::new(name)
        .indexed_dimension("d", n as u32)
        .array_with_ranges("vals[d]", refs(&vals))
        .array_with_ranges("dir[d]", refs(&dir))
        .array_aux("out[d]", eqn)
}

/// A per-element hoist that consumes MORE than 256 temp ids but never uses a
/// temp as a view SOURCE still produces the right numbers, and must keep doing
/// so: every writer and every reader of such a temp narrows the same id the
/// same way (`write_temp_id: id as TempId` against `LoadTempConst`'s
/// `temp_id: id as TempId`), and each element's temp is written immediately
/// before it is read, so the aliasing is unobservable.
///
/// That reasoning is bounded, and the bound is the fixture: the aliased temps
/// here are all the SAME SIZE, because a per-element hoist over one array
/// repeats one shape. Truncation is NOT safe in general -- temps of different
/// sizes sharing a truncated id let the larger write run past the smaller slot
/// into its neighbour's storage, in-bounds for the flat temp region and
/// therefore silent. No lowering path emits that today; #583 is the fix for
/// both halves.
///
/// So this is a property of the emission pattern, not of the namespace, and it
/// is pinned here rather than assumed: the moment a change makes those two
/// narrowings disagree, this returns a different array instead of failing.
#[test]
fn a_per_element_hoist_past_the_temp_namespace_without_a_temp_view_is_correct() {
    const N: usize = 300;
    let project = sort_project("temp_namespace_ok", N, "VECTOR SORT ORDER(vals[d], dir[d])");
    project.assert_compiles_incremental();

    // Over the DEcreasing `vals`, ascending is the reversal and descending is
    // the identity.
    let expected: Vec<f64> = (0..N)
        .map(|k| if k % 2 == 1 { N - 1 - k } else { k } as f64)
        .collect();
    assert_close(
        &project.vm_result_incremental("out"),
        &expected,
        "300-element per-element hoist, no temp read as a view",
    );
}

/// The same hoist WITH a materialized operand puts a temp in a view position,
/// and a view base is the one place a temp id is carried as a `u32` while
/// every writer narrows it to `u8` -- so above 255 the view reads storage no
/// opcode wrote. `symbolic::resolve_static_view` rejects that rather than
/// emitting a well-formed program with wrong numbers.
///
/// Both spellings are covered because they arrive from different directions,
/// and only one of them is new: the `vals[*]` spelling ALREADY put a Pass-1
/// temp in a view position, so at HEAD it returned a silently wrong array from
/// element 128 on (a pre-existing #583 instance, not caused by this work); the
/// `vals[d]` spelling did not compile at all before this module's fix, and
/// would have joined it. Both are now loud.
///
/// 130 elements is the smallest round size past the boundary (two temps per
/// element, so ids cross 255 at element 128).
#[test]
fn a_temp_read_as_a_view_past_the_temp_namespace_is_rejected() {
    for (name, eqn) in [
        ("temp_view_dim", "VECTOR SORT ORDER(301 - vals[d], dir[d])"),
        ("temp_view_star", "VECTOR SORT ORDER(301 - vals[*], dir[d])"),
    ] {
        let err = sort_project(name, 130, eqn)
            .compile_incremental()
            .err()
            .unwrap_or_else(|| panic!("{eqn}: a view over a temp above 255 must be rejected"));
        let details = err.get_details().unwrap_or_default();
        assert!(
            details.contains("TempId capacity"),
            "{eqn}: expected the temp-namespace rejection, got {err:?}"
        );
    }
}

/// A residual this work does NOT fix, pinned so it is loud rather than
/// silently rediscovered: an array-producing builtin nested inside
/// *arithmetic* that is itself an array operand.
///
/// `VECTOR SORT ORDER(VECTOR ELM MAP(a, b) + c, 1)` materializes the `Op2` into
/// a temp correctly -- for the star spelling that already happened in Pass 1,
/// before this work -- but the resulting `AssignTemp` body still holds the
/// inner `App(VectorElmMap)`, and codegen's `AssignTemp` arm only routes an
/// array-producing builtin to its opcode when the builtin is the body's *root*.
/// Anywhere else in the body it reaches the `BeginIter` loop and is rejected
/// with "array-producing builtin outside AssignTemp context".
///
/// That is a different contract from the one this module is about -- where an
/// array-producing `App` may APPEAR, not whether an operand is a view -- and
/// fixing it needs a notion of "this subexpression is array-valued" that the
/// lowered `Expr` tree does not carry locally. The bare nested form
/// (`VECTOR SORT ORDER(VECTOR ELM MAP(a, b), 1)`, no arithmetic) does work; see
/// [`computed_operand_shapes`].
#[test]
fn a_nested_array_producing_builtin_inside_arithmetic_is_a_separate_residual() {
    for (name, eqn) in [
        (
            "residual_dim",
            "VECTOR SORT ORDER(VECTOR ELM MAP(vals[d], offs[d]) + bump[d], 1)",
        ),
        (
            "residual_star",
            "VECTOR SORT ORDER(VECTOR ELM MAP(vals[*], offs[*]) + bump[*], 1)",
        ),
    ] {
        assert_fails_attributed(fixture(name).array_aux("out[d]", eqn), eqn);
    }
}
