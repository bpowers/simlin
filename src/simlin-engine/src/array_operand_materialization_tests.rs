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
//! `walk_expr_as_view`'s four storage-view arms, plus "an array view can be
//! derived for it", minus "it already IS a view over a snapshot buffer"
//! (`is_snapshot_view`, the C3 shape). The shape axis is therefore the set of
//! *rejected* `compiler::Expr` variants that can carry an array: `Op2`, `Op1`,
//! `If`, and `App` (an elementwise builtin --
//! [`elementwise_builtin_operands_materialize`] covers the two families
//! `find_expr_array_view` recognises -- or a nested array-producing builtin).
//!
//! **The collapse.** Position and shape are decided by two separate, singly
//! implemented pieces of code -- the signature table's `ArgKind::Array`
//! positions (`BuiltinFn::arg_kinds`, which `materialize_view_operands`
//! reads), and one shared `materialize_view_operand` that every such position
//! goes through.
//! So the matrix is covered as a cross rather than as a full product: *every*
//! position is exercised with one computed shape (an `Op2`) plus its
//! already-compiling control, and *every* shape is exercised at one position
//! (`VECTOR SORT ORDER` arg0). The two spellings of an apply-to-all reference
//! -- `vals[d]`, which only means "the whole array" after `context.rs`'s
//! `with_vector_builtin_wildcards` promotion, and `vals[*]` -- are a third
//! axis, covered at `VECTOR SORT ORDER` arg0 and `RANK` arg0, the two arms the
//! issue reports separately.
//!
//! **Why some rows carry a `+ SUM(VECTOR SORT ORDER(vals[*], 1))` tail.** One
//! pass materializes every operand whether or not the equation holds an
//! array-producing builtin, so the tail selects no path; it is a second
//! materialized value beside the row's own, and it contributes the constant
//! `1 + 2 + 0 = 3` that the values below are derived with.
//!
//! **Values.** Every compiling row asserts VM numbers, chosen so that reading
//! the *wrong* array gives a different answer than reading the computed one:
//! `vals = [30, 10, 20]` and `vals + bump = [30, 110, 20]` have different sort
//! orders, different ranks and different element-map results, and each wrong
//! rule (read `vals` raw, read `bump` raw, collapse the operand to its first
//! element) lands on its own distinct answer. The value each wrong rule would
//! produce is written next to the assertion.
//!
//! `PREVIOUS`/`INIT` of an arrayed reference is Phase C3 (GH #995's option D),
//! and it is a FIFTH view shape rather than a computed array: the call reads its
//! argument's view out of one of the VM's snapshot buffers. Its rows live in
//! their own section below, over a TIME-VARYING fixture -- a constant fixture
//! cannot tell a previous value from a current one, so every row there asserts
//! series rather than single arrays. The complement stays a green row:
//! [`a_scalar_previous_beside_an_array_operand_still_materializes`], without
//! which "array-valued" could widen to "any `PREVIOUS`" unnoticed and a scalar
//! `PREVIOUS(s)` would stop broadcasting.

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
    project.vm_result("out")
}

/// Compile `out[e] = <eqn>` against the shared fixture and return `out`. Used
/// by the reducer rows, whose argument is a row slice `matrix[e,*]`.
fn row_out_of(name: &str, eqn: &str) -> Vec<f64> {
    let project = fixture(name).array_aux("out[e]", eqn);
    project.assert_compiles_incremental();
    project.vm_result("out")
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

/// A row that must fail for a STATED reason, checked against the per-variable
/// diagnostic (the surface a user reads) rather than against the aggregate
/// assembly error, which names only the variable.
fn assert_declines_because(project: TestProject, variable: &str, reason: &str) {
    use crate::db::{DiagnosticError, SimlinDb, collect_all_diagnostics, sync_from_datamodel};
    let datamodel = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let diags = collect_all_diagnostics(&db, sync.project);
    let matched = diags.iter().any(|d| {
        d.variable.as_deref() == Some(variable)
            && matches!(&d.error, DiagnosticError::Assembly(msg) if msg.contains(reason))
    });
    assert!(
        matched,
        "expected a diagnostic for '{variable}' containing {reason:?}; got: {diags:?}"
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

/// `VECTOR SELECT` in the ELEMENT spelling reads the element whether or not an
/// array-producing sibling sits beside it.
///
/// `sel[d]` and `vals[d]` inside an equation over `d` are that element
/// (`ArgKind::Array { whole: false }`, Vensim's `!` convention: an unmarked
/// subscript is the equation's own element), so each element selects from a
/// one-element range: `vals[d]` where `sel[d]` is 1, the missing value 0
/// otherwise -- `[30, 10, 0]`. A sibling `+ SUM(VECTOR SORT ORDER(vals[*], 1))`
/// adds its constant 3 and changes nothing else; one materialization pass
/// means the sibling cannot select a different lowering of the operand, which
/// is what makes the two rows agree.
#[test]
fn vector_select_element_spelling_reads_the_element_beside_any_sibling() {
    assert_close(
        &out_of("vsel_elem", "VECTOR SELECT(sel[d], vals[d], 0, 0, 0)"),
        &[30.0, 10.0, 0.0],
        "element spelling alone",
    );
    assert_close(
        &out_of(
            "vsel_elem_tail",
            "VECTOR SELECT(sel[d], vals[d], 0, 0, 0) + SUM(VECTOR SORT ORDER(vals[*], 1))",
        ),
        &[33.0, 13.0, 3.0],
        "element spelling beside an array-producing sibling",
    );
}

#[test]
fn vector_select_positions() {
    // VECTOR SELECT reduces to a scalar, so every element of `out` holds the
    // same value; the `+ SUM(VECTOR SORT ORDER(vals[*], 1))` tail adds 3.
    //
    // The operands are spelled `[*]`: VECTOR SELECT reduces the axis its
    // operands mark, so a `[d]` subscript inside an equation over `d` is that
    // ELEMENT (`ArgKind::Array { whole: false }`, Vensim's `!` convention) and
    // would make every row a degenerate one-element selection.
    //
    // Control: `sel = [1, 1, 0]` selects vals[0] + vals[1] = 40, plus 3.
    assert_close(
        &out_of(
            "vsel_ctl",
            "VECTOR SELECT(sel[*], vals[*], 0, 0, 0) + SUM(VECTOR SORT ORDER(vals[*], 1))",
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
            "VECTOR SELECT(sel[*] - mask[*], vals[*], 0, 0, 0) + SUM(VECTOR SORT ORDER(vals[*], 1))",
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
            "VECTOR SELECT(sel[*], vals[*] + bump[*], 0, 0, 0) + SUM(VECTOR SORT ORDER(vals[*], 1))",
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
    // makes MEAN agree with its four sibling reducers: an array-shaped MEAN
    // argument used to fall through codegen's scalar fallback and fail to
    // compile.
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
        let computed = computed.vm_result("out");

        let reference = allocate_fixture(&format!("alloc_r{i}"))
            .array_aux(row.helper.0, row.helper.1)
            .array_aux("out[d]", row.reference);
        reference.assert_compiles_incremental();
        let reference = reference.vm_result("out");

        let raw = allocate_fixture(&format!("alloc_w{i}")).array_aux("out[d]", row.raw);
        raw.assert_compiles_incremental();
        let raw = raw.vm_result("out");

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
/// shapes that decline for a reason other than the position. The reasons live
/// in `compiler::array_operand::materialize_view_operands`; what is pinned
/// here is that each still fails loudly, or keeps its existing meaning, rather
/// than compiling to something wrong.
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

    // The SAME position also refuses a `PREVIOUS`/`INIT` (GH #995 phase C3),
    // and for the same reason -- see
    // `a_snapshot_priority_profile_declines_rather_than_allocating_over_one_column`
    // for the wrong allocation it prevents and the workaround that compiles.
    for (name, eqn) in [
        (
            "unmat_pp_prev",
            "allocate_available(request[d], PREVIOUS(pp[d,1]), supply)",
        ),
        (
            "unmat_pp_init",
            "allocate_available(request[d], INIT(pp[d,1]), supply)",
        ),
    ] {
        assert_fails_attributed(
            TestProject::new(name)
                .indexed_dimension("d", 3)
                .indexed_dimension("xp", 4)
                .array_with_ranges("request[d]", vec![("1", "10"), ("2", "20"), ("3", "30")])
                .array_const("pp[d,xp]", 1.0)
                .scalar_const("supply", 35.0)
                .array_aux("out[d]", eqn),
            eqn,
        );
    }
}

/// The pp-position decline, with the fixture that shows what it prevents.
///
/// `pp` is CONSTANT here, so `PREVIOUS(pp[d,1])` and `pp[d,1]` hold the same
/// numbers at every step after the first: any difference in the allocation is
/// therefore a SHAPE defect and nothing else. Without the guard the frozen form
/// compiled and allocated over a one-column-per-requester profile -- a silently
/// wrong allocation where HEAD failed loudly, which is the regression this row
/// exists to keep out.
///
/// The workaround is asserted too, so the decline is a redirection rather than a
/// dead end: capturing the profile into a variable of its own gives the expander
/// the direct reference it needs, and the allocation then matches the unfrozen
/// model exactly (`pp` being constant is what makes that the RIGHT answer to
/// compare against).
#[test]
fn a_snapshot_priority_profile_declines_rather_than_allocating_over_one_column() {
    let fixture = |name: &str| {
        TestProject::new(name)
            .with_sim_time(0.0, 2.0, 1.0)
            .indexed_dimension("d", 3)
            .indexed_dimension("xp", 4)
            .array_with_ranges("request[d]", vec![("1", "10"), ("2", "20"), ("3", "30")])
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
    };

    assert_declines_because(
        fixture("pp_prev_reject").array_aux(
            "out[d]",
            "allocate_available(request[d], PREVIOUS(pp[d,1]), supply)",
        ),
        "out",
        "would allocate over one column",
    );

    let series = |project: TestProject| -> Vec<Vec<f64>> {
        project.assert_compiles_incremental();
        let all = project.run_vm_expecting_success();
        (1..=3)
            .map(|k| all.get(&format!("out[{k}]")).unwrap().clone())
            .collect()
    };
    let unfrozen = series(
        fixture("pp_unfrozen")
            .array_aux("out[d]", "allocate_available(request[d], pp[d,1], supply)"),
    );
    let captured = series(
        fixture("pp_captured")
            .array_aux("frozen[d,xp]", "PREVIOUS(pp[d,xp])")
            .array_aux(
                "out[d]",
                "allocate_available(request[d], frozen[d,1], supply)",
            ),
    );
    // Step 0 differs: the capture reads the PREVIOUS fallback (an all-zero
    // profile), which is a legitimate allocation over a degenerate profile
    // rather than a shape defect. From step 1 on the frozen profile IS `pp`,
    // so the two models must agree element for element.
    for (k, (a, b)) in captured.iter().zip(unfrozen.iter()).enumerate() {
        assert_close(
            &a[1..],
            &b[1..],
            &format!(
                "the per-element capture workaround must allocate exactly as the \
                 unfrozen model does once the snapshot exists (element {})",
                k + 1
            ),
        );
    }
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
/// accidental. It pins it against OURSELVES: the variable-source half is
/// documented and ground-truthed, the computed-source half is not.
///
/// **Documented.** The Vensim reference page for `VECTOR ELM MAP` (retrieved
/// 2026-08-02) says the function "returns the value of the variable that is
/// offset from vec by the specified amount", and that an offset "outside the
/// range of the variable" yields `:NA:`. Real Vensim output agrees: in
/// `test/sdeverywhere/models/vector/`,
/// `f[DimA,DimB] = VECTOR ELM MAP(d[DimA,B1], a[DimA])` prints `1,1,5,5,6,6`,
/// and `f[A2,B1] = 5 = d[A2,B2]` -- the mapping read past its own `B1` slice
/// into the next row of `d`'s storage. `vm_vector_elm_map.rs` implements that
/// with its `source_is_full_array` test: a strict slice such as `matrix[1,*]`
/// keeps a per-element base and CAN read across rows.
///
/// **A DEFINED EXTENSION, not a match.** Vensim rejects a computed source
/// outright: run in Vensim DSS on 2026-08-04,
/// `vensim-probes/elm_map_computed_source.mdl` refuses to simulate with
/// "Argument 1 to function VECTOR ELM MAP must be a normal variable". There is
/// therefore no Vensim behaviour for these numbers to agree or disagree with,
/// and the shape is one Simlin accepts and Vensim does not.
///
/// What it MEANS is defined by helper-equivalence: an inline expression behaves
/// exactly as the same values pre-assigned to a named variable -- the spelling
/// that IS legal Vensim. A materialized operand is a fresh contiguous temp, so
/// it is full-array by construction and the mapping is confined to the computed
/// array, which is exactly `VECTOR ELM MAP(helper[A1], offs)` for a `helper`
/// holding those values. The rows below pin that definition.
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
    let slice = slice.vm_result("out");
    assert_eq!(slice.len(), 3);
    assert!(
        (slice[0] - 10.0).abs() < 1e-9 && (slice[1] - 30.0).abs() < 1e-9 && slice[2].is_nan(),
        "a direct strict-slice source maps over the whole variable, got {slice:?}"
    );

    // The computed source is a temp of its own, so every offset is out of its
    // range.
    let computed = far().array_aux("out[d]", "VECTOR ELM MAP(matrix[1,*] * 1, far[d])");
    computed.assert_compiles_incremental();
    let computed = computed.vm_result("out");
    assert!(
        computed.len() == 3 && computed.iter().all(|v| v.is_nan()),
        "a materialized source confines the mapping to the computed array, got {computed:?}"
    );
}

/// C1: `RANK`'s array argument is materialized like all five of its siblings'.
///
/// This is not observable from VM values -- the operand is materialized either
/// way and the numbers agree -- so it is pinned at the lowered-fragment level,
/// through the production per-variable lowering (`TestProject::flow_exprs`).
/// What it constrains is the SHAPE of the fragment: an operand temp written
/// before the builtin's own, both read where the equation reads them.
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
            .flow_exprs("out")
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
        "the operand is materialized first (temp 0), then the builtin's own \
         result (temp 1)"
    );
    assert_eq!(
        rank, vso,
        "RANK must materialize its array argument like VECTOR SORT ORDER \
         does; a fragment numbered the other way means the two arms disagree"
    );
}

// ===========================================================================
// Spelling axis: `vals[d]` (needs the ActiveDimRef -> Wildcard promotion) and
// `vals[*]`, at the two arms the issue reports separately.
// ===========================================================================

#[test]
fn both_apply_to_all_spellings_materialize() {
    // The two spellings of one operand: the star one carries no
    // active-dimension reference, and the active-dimension one has its
    // references promoted back to whole axes by the vector builtin's own
    // operand rule. Both must agree.
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

    // `RANK` is the arm whose array argument used to be classified as a scalar
    // position, so BOTH spellings failed. Ranks of [30, 110, 20] ascending:
    // 2, 3, 1.
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
// Phase C3: `PREVIOUS`/`INIT` of an arrayed reference (GH #995, option D).
//
// An array-valued `PREVIOUS`/`INIT` is a VIEW over one of the VM's snapshot
// buffers -- the same `prev_values` / `initial_values` the scalar `LoadPrev` /
// `LoadInitial` read, addressed with the argument's own geometry. So it is a
// view position like any other, not a computed array that has to be
// materialized first.
//
// These rows were the red half of Phase C1+C2's decline. Each now asserts VM
// NUMBERS over a TIME-VARYING fixture, because a constant fixture cannot tell a
// previous value from a current one.
// ===========================================================================

/// The time-varying fixture. Everything the C3 rows read moves, so reading
/// `curr` where `prev` was meant is a different answer at every step but the
/// first.
///
/// Three saved steps (t = 0, 1, 2):
///
/// | variable   | t=0            | t=1            | t=2             |
/// |------------|----------------|----------------|-----------------|
/// | `vals[d]`  | `[30, 10, 20]` | `[5, 20, 20]`  | `[-20, 30, 20]` |
/// | `offs[d]`  | `[2, 0, 1]`    | `[1, 0, 1]`    | `[0, 0, 1]`     |
/// | `sel[d]`   | `[1, 1, 0]`    | `[0, 1, 0]`    | `[0, 1, 0]`     |
/// | `matrix[1,*]` | `[1, 2, 3]` | `[2, 2, 3]`    | `[3, 2, 3]`     |
/// | `matrix[2,*]` | `[10,20,30]`| `[10, 20, 40]` | `[10, 20, 50]`  |
///
/// `fixed[d] = [30, 10, 20]` is deliberately CONSTANT: it is the second operand
/// of the nested rows and the source of the `+ SUM(VECTOR SORT ORDER(fixed[*],
/// 1))` tail, which must contribute the same 3 at every step so the tail does
/// not smear the value being asserted. (That tail is what forces the lowering
/// path the `VECTOR SELECT` rows are about -- see the module docs.)
fn moving_fixture(name: &str) -> TestProject {
    TestProject::new(name)
        .with_sim_time(0.0, 2.0, 1.0)
        .indexed_dimension("d", 3)
        .indexed_dimension("e", 2)
        .array_with_ranges(
            "vals[d]",
            vec![
                ("1", "30 - 25 * TIME"),
                ("2", "10 + 10 * TIME"),
                ("3", "20"),
            ],
        )
        .array_with_ranges("offs[d]", vec![("1", "2 - TIME"), ("2", "0"), ("3", "1")])
        .array_with_ranges(
            "sel[d]",
            vec![("1", "IF TIME > 0.5 THEN 0 ELSE 1"), ("2", "1"), ("3", "0")],
        )
        .array_with_ranges("fixed[d]", vec![("1", "30"), ("2", "10"), ("3", "20")])
        .array_with_ranges(
            "matrix[e,d]",
            vec![
                ("1,1", "1 + TIME"),
                ("1,2", "2"),
                ("1,3", "3"),
                ("2,1", "10"),
                ("2,2", "20"),
                ("2,3", "30 + 10 * TIME"),
            ],
        )
        // A NAMED row dimension, so the qualified `row·r1` spelling exists, and
        // rows two orders of magnitude apart so reading the WRONG row cannot be
        // mistaken for reading the wrong step. Row sums: r1 = 6, 7, 8;
        // r2 = 600, 610, 620.
        .named_dimension("row", &["r1", "r2"])
        .array_with_ranges(
            "wide[row,d]",
            vec![
                ("r1,1", "1 + TIME"),
                ("r1,2", "2"),
                ("r1,3", "3"),
                ("r2,1", "100"),
                ("r2,2", "200"),
                ("r2,3", "300 + 10 * TIME"),
            ],
        )
}

/// Run `<lhs> = <eqn>` against the moving fixture and return each element's
/// series, element-major: `series[k]` is `out[k+1]` over t = 0, 1, 2.
fn moving_series(name: &str, lhs: &str, eqn: &str, n_elements: usize) -> Vec<Vec<f64>> {
    let project = moving_fixture(name).array_aux(lhs, eqn);
    project.assert_compiles_incremental();
    let all = project.run_vm_expecting_success();
    (1..=n_elements)
        .map(|k| {
            all.get(&format!("out[{k}]"))
                .unwrap_or_else(|| panic!("out[{k}] missing from {:?}", all.keys()))
                .clone()
        })
        .collect()
}

fn assert_series(actual: &[Vec<f64>], expected: &[[f64; 3]], what: &str) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{what}: element-count mismatch, got {actual:?}"
    );
    for (k, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
        assert_close(a, e, &format!("{what}: out[{}] over time", k + 1));
    }
}

/// The position axis, bare operand: every `walk_expr_as_view` call site from
/// the module-doc table, with `PREVIOUS(<arrayed reference>)` in it.
///
/// The whole point of the fixture moving is that each row's expected series
/// distinguishes four readings: the correct previous array, the CURRENT array,
/// one element's previous value broadcast, and the all-zero stub a failed
/// fragment leaves behind. Each row says which.
#[test]
fn previous_operands_are_views_over_the_prev_snapshot() {
    // VECTOR SORT ORDER arg0. prev(vals) is [0,0,0], [30,10,20], [5,20,20];
    // ascending sort orders are [0,1,2], [1,2,0], [0,1,2]. Reading `vals`
    // CURRENT would give [1,2,0], [0,1,2], [0,2,1]; broadcasting element 0's
    // previous value gives [0,1,2] at every step, which differs at t=1.
    assert_series(
        &moving_series(
            "c3_vso",
            "out[d]",
            "VECTOR SORT ORDER(PREVIOUS(vals[d]), 1)",
            3,
        ),
        &[[0.0, 1.0, 0.0], [1.0, 2.0, 1.0], [2.0, 0.0, 2.0]],
        "VECTOR SORT ORDER arg0",
    );

    // RANK arg0, 1-based. [0,0,0] ties to [1,2,3] under the stable sort;
    // [30,10,20] ranks [3,1,2]; [5,20,20] ranks [1,2,3]. Reading CURRENT would
    // give [3,1,2], [1,2,3], [1,3,2].
    assert_series(
        &moving_series("c3_rank", "out[d]", "RANK(PREVIOUS(vals[d]), 1)", 3),
        &[[1.0, 3.0, 1.0], [2.0, 1.0, 2.0], [3.0, 2.0, 3.0]],
        "RANK arg0",
    );

    // VECTOR ELM MAP arg0 (source). The prev view spans the whole variable, so
    // it is a full-array source and `result[i] = prev_vals[offs[i]]` with the
    // CURRENT offsets: [0,0,0] mapped by [2,0,1]; [30,10,20] by [1,0,1] ->
    // [10,30,10]; [5,20,20] by [0,0,1] -> [5,5,20]. This row is also the one
    // that pins `full_source_len` looking THROUGH the `PREVIOUS`: bounding the
    // source at 1 element instead of 3 turns every mapped offset but 0 into
    // `:NA:` (measured: `[NaN, NaN, 5]` for out[1]).
    assert_series(
        &moving_series(
            "c3_elm_src",
            "out[d]",
            "VECTOR ELM MAP(PREVIOUS(vals[d]), offs[d])",
            3,
        ),
        &[[0.0, 10.0, 5.0], [0.0, 30.0, 5.0], [0.0, 10.0, 20.0]],
        "VECTOR ELM MAP arg0 (source)",
    );

    // VECTOR ELM MAP arg1 (offsets): `result[i] = vals[prev_offs[i]]` over the
    // CURRENT vals. prev_offs [0,0,0] over [30,10,20] -> [30,30,30];
    // [2,0,1] over [5,20,20] -> [20,5,20]; [1,0,1] over [-20,30,20] ->
    // [30,-20,30]. Reading `offs` current would give [20,30,10], [20,5,20],
    // [-20,-20,30].
    assert_series(
        &moving_series(
            "c3_elm_off",
            "out[d]",
            "VECTOR ELM MAP(vals[d], PREVIOUS(offs[d]))",
            3,
        ),
        &[[30.0, 20.0, 30.0], [30.0, 5.0, -20.0], [30.0, 20.0, 30.0]],
        "VECTOR ELM MAP arg1 (offsets)",
    );

    // VECTOR SELECT reduces to a scalar, so every element of `out` holds the
    // same value; the constant tail adds 3.
    //
    // arg0 (selection): prev(sel) [0,0,0] selects nothing -> the max_value
    // argument 0; [1,1,0] selects vals[0]+vals[1] = 5+20 = 25; [0,1,0] selects
    // vals[1] = 30. Reading `sel` current would give 43, 23, 33.
    assert_series(
        &moving_series(
            "c3_sel_sel",
            "out[d]",
            "VECTOR SELECT(PREVIOUS(sel[*]), vals[*], 0, 0, 0) \
             + SUM(VECTOR SORT ORDER(fixed[*], 1))",
            3,
        ),
        &[[3.0, 28.0, 33.0], [3.0, 28.0, 33.0], [3.0, 28.0, 33.0]],
        "VECTOR SELECT arg0 (selection array)",
    );

    // arg1 (values): the CURRENT sel over prev(vals). [1,1,0] over [0,0,0] -> 0;
    // [0,1,0] over [30,10,20] -> 10; [0,1,0] over [5,20,20] -> 20. Reading
    // `vals` current would give 43, 23, 33.
    assert_series(
        &moving_series(
            "c3_sel_val",
            "out[d]",
            "VECTOR SELECT(sel[*], PREVIOUS(vals[*]), 0, 0, 0) \
             + SUM(VECTOR SORT ORDER(fixed[*], 1))",
            3,
        ),
        &[[3.0, 13.0, 23.0], [3.0, 13.0, 23.0], [3.0, 13.0, 23.0]],
        "VECTOR SELECT arg1 (value array)",
    );
}

/// The six `emit_array_reduce` arms plus `MEAN`, over the row slice
/// `PREVIOUS(matrix[e,*])`.
///
/// A reducer's argument is lowered with `with_preserved_wildcards`, which does
/// NOT promote an active-dimension reference -- so `matrix[e,*]` stays a ROW
/// slice and the prev view is that row of the snapshot, not the whole matrix.
/// That is the ELM MAP coherence rule stated positively: a prev view of a strict
/// slice behaves exactly like the curr view of the same slice.
///
/// prev rows are `[0,0,0]`/`[0,0,0]`, then `[1,2,3]`/`[10,20,30]`, then
/// `[2,2,3]`/`[10,20,40]`. Reading the CURRENT rows would shift each series one
/// step earlier, which every row below distinguishes except `SIZE` -- whose
/// point is the count, and whose wrong answer (a collapsed 1-element view) is 1.
#[test]
fn previous_reducer_operands_read_the_previous_row() {
    assert_series(
        &moving_series("c3_sum", "out[e]", "SUM(PREVIOUS(matrix[e,*]))", 2),
        &[[0.0, 6.0, 7.0], [0.0, 60.0, 70.0]],
        "SUM",
    );
    assert_series(
        &moving_series("c3_max", "out[e]", "MAX(PREVIOUS(matrix[e,*]))", 2),
        &[[0.0, 3.0, 3.0], [0.0, 30.0, 40.0]],
        "MAX (1-arg)",
    );
    assert_series(
        &moving_series("c3_min", "out[e]", "MIN(PREVIOUS(matrix[e,*]))", 2),
        &[[0.0, 1.0, 2.0], [0.0, 10.0, 10.0]],
        "MIN (1-arg)",
    );
    // SIZE counts elements of the prev view: 3 always. A collapsed operand
    // would give 1, which is the failure this row rules out.
    assert_series(
        &moving_series("c3_size", "out[e]", "SIZE(PREVIOUS(matrix[e,*]))", 2),
        &[[3.0, 3.0, 3.0], [3.0, 3.0, 3.0]],
        "SIZE",
    );
    // MEAN's single-argument form is an array reduction, and its codegen arm
    // enumerates the view shapes rather than pushing a view unconditionally --
    // so the snapshot view had to be added there too, or an array-valued
    // PREVIOUS would have fallen through to the scalar walk and failed to
    // compile (measured before the arm was extended).
    assert_series(
        &moving_series("c3_mean", "out[e]", "MEAN(PREVIOUS(matrix[e,*]))", 2),
        &[[0.0, 2.0, 7.0 / 3.0], [0.0, 20.0, 70.0 / 3.0]],
        "MEAN (1-arg)",
    );
    // STDDEV is the POPULATION deviation (`ArrayStddev` divides by n).
    let pop_stddev = |xs: [f64; 3]| -> f64 {
        let mean = (xs[0] + xs[1] + xs[2]) / 3.0;
        (xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / 3.0).sqrt()
    };
    assert_series(
        &moving_series("c3_stddev", "out[e]", "STDDEV(PREVIOUS(matrix[e,*]))", 2),
        &[
            [
                0.0,
                pop_stddev([1.0, 2.0, 3.0]),
                pop_stddev([2.0, 2.0, 3.0]),
            ],
            [
                0.0,
                pop_stddev([10.0, 20.0, 30.0]),
                pop_stddev([10.0, 20.0, 40.0]),
            ],
        ],
        "STDDEV",
    );
}

/// A prev view of a strict row slice must read THAT ROW of the snapshot.
///
/// The reducer rows above pin the LAG (they would catch reading `curr`) but are
/// weak on the ROW, because their two matrix rows are only an order of magnitude
/// apart and both are read by the same apply-to-all iteration. `wide`'s rows are
/// two orders of magnitude apart, so the four readings are unmistakable:
/// previous r1 is `[0, 6, 7]` and previous r2 is `[0, 600, 610]`, against the
/// `curr` controls `[6, 7, 8]` and `[600, 610, 620]`. Reading the wrong row
/// lands on the other row's series; reading the wrong step lands on its own
/// control.
///
/// SPELLING, disclosed rather than assumed. This uses the ACTIVE-DIMENSION
/// spelling (`wide[row,*]` under `out[row]`), which resolves per element. The
/// QUALIFIED spelling the LTM wrap generates --
/// `PREVIOUS(matrix[region·nyc,*])` -- pins the row by NAME instead, and it
/// reaches this SAME view route from an ordinary APPLY-TO-ALL user equation:
/// `arg_is_array_shaped` accepts it (the qualified index is static, the `*`
/// spans), so no capture helper is synthesized and the argument passes through
/// to lowering. Measured: `out[row] = SUM(PREVIOUS(wide[row·r1,*]))` compiles
/// with ZERO synthesized helpers and reads r1's previous row at every step,
/// while with `arg_is_array_shaped` reverted (HEAD's visitor) the same equation
/// declines through a capture helper. So the qualified spelling is neither
/// LTM-only nor pre-existing -- C3 is what routes it here.
///
/// What IS split is `Ast::Scalar` vs `Ast::ApplyToAll` inside
/// `builtins_visitor::instantiate_implicit_modules`, not user-vs-LTM: both
/// `variable.rs` and `db::ltm::parse` pass `Some(dimensions)`. In a SCALAR
/// equation the qualified index is not accepted as static and
/// `SUM(PREVIOUS(wide[row·r1,*]))` still declines through a helper; that half is
/// unchanged by C3 and is a front-end residual, not a view question.
///
/// The row property asserted below is the VIEW ARITHMETIC's, which both
/// spellings share, and the loop pins BOTH: the active-dimension rows read
/// each iteration's own row, while the qualified rows pin one row by NAME for
/// every element. The LTM-side witness that the qualified spelling compiles
/// there too is `db::ltm_char_tests::char_agg_nested_reducer`, whose partial
/// embeds `previous(matrix[region·boston,*])` -- but that fixture cannot
/// discriminate a ROW (every element of its `matrix` and `other` is the
/// constant 1), which is why the numeric row property lives here.
#[test]
fn a_prev_view_of_a_row_slice_reads_that_row_of_the_snapshot() {
    for (name, eqn, expected) in [
        (
            "c3_row_prev",
            "SUM(PREVIOUS(wide[row,*]))",
            [[0.0, 6.0, 7.0], [0.0, 600.0, 610.0]],
        ),
        // The `curr` controls, which are what a lost lag would return.
        (
            "c3_row_now",
            "SUM(wide[row,*])",
            [[6.0, 7.0, 8.0], [600.0, 610.0, 620.0]],
        ),
        // The QUALIFIED spelling: one row pinned by name, read for EVERY
        // element of the iteration. Reading the wrong row lands on r2's
        // unmistakable series; losing the lag lands on the curr control above.
        (
            "c3_row_prev_qual",
            "SUM(PREVIOUS(wide[row\u{B7}r1,*]))",
            [[0.0, 6.0, 7.0], [0.0, 6.0, 7.0]],
        ),
        (
            "c3_row_prev_qual2",
            "SUM(PREVIOUS(wide[row\u{B7}r2,*]))",
            [[0.0, 600.0, 610.0], [0.0, 600.0, 610.0]],
        ),
    ] {
        let project = moving_fixture(name).array_aux("out[row]", eqn);
        project.assert_compiles_incremental();
        let all = project.run_vm_expecting_success();
        for (elem, want) in ["r1", "r2"].into_iter().zip(expected.iter()) {
            assert_close(
                all.get(&format!("out[{elem}]"))
                    .unwrap_or_else(|| panic!("out[{elem}] missing")),
                want,
                &format!("{eqn} at row {elem}"),
            );
        }
    }
}

/// The `INIT` twins. `initial_values` is the post-initials snapshot, so an
/// `INIT` view is the t=0 array at EVERY step -- including t=0 itself, where
/// `PREVIOUS` reads its fallback instead.
///
/// `INIT(vals)` is `[30, 10, 20]` throughout, so the sort order is `[1, 2, 0]`
/// at every step. That is distinct from the `PREVIOUS` series above at t=0 and
/// t=2, and from reading `vals` current at t=1 and t=2.
#[test]
fn init_operands_are_views_over_the_initial_snapshot() {
    assert_series(
        &moving_series(
            "c3_init_vso",
            "out[d]",
            "VECTOR SORT ORDER(INIT(vals[d]), 1)",
            3,
        ),
        &[[1.0, 1.0, 1.0], [2.0, 2.0, 2.0], [0.0, 0.0, 0.0]],
        "VECTOR SORT ORDER arg0, INIT",
    );
    // A reducer over an INIT row slice: `matrix` row sums at t=0 are 6 and 60,
    // held for the whole run. The PREVIOUS twin above reads 0, then 6/60, then
    // 7/70.
    assert_series(
        &moving_series("c3_init_sum", "out[e]", "SUM(INIT(matrix[e,*]))", 2),
        &[[6.0, 6.0, 6.0], [60.0, 60.0, 60.0]],
        "SUM over an INIT row slice",
    );
    // An INIT view in the initials phase reads `curr` rather than the snapshot
    // (the snapshot does not exist yet), exactly as `Opcode::LoadInitial` does.
    // An arrayed stock whose INITIAL equation reduces an INIT view is the shape
    // that exercises it: `SUM(INIT(matrix[e,*]))` at t=0 is 6 and 60, and the
    // stock never changes, so a broken initials branch (reading an all-zero
    // snapshot) would leave it at 0.
    let init_phase = moving_fixture("c3_init_phase").array_stock(
        "lvl[e]",
        "SUM(INIT(matrix[e,*]))",
        &[],
        &[],
        None,
    );
    init_phase.assert_compiles_incremental();
    let all = init_phase.run_vm_expecting_success();
    assert_close(all.get("lvl[1]").unwrap(), &[6.0, 6.0, 6.0], "lvl[1]");
    assert_close(all.get("lvl[2]").unwrap(), &[60.0, 60.0, 60.0], "lvl[2]");
}

/// The nested rows Phase C1+C2 declined: an array-valued `PREVIOUS`/`INIT`
/// under arithmetic that is itself the operand.
///
/// C1+C2 refused these because the argument was lowered element-collapsed, so
/// materializing would have produced ONE element's previous value broadcast
/// across the array -- measured `[0, 2, 1]` where the answer was `[2, 0, 1]`.
/// The argument now keeps its array shape, `find_expr_array_view` gives the call
/// its argument's shape, and the operand materializes like any other computed
/// array: the `BeginIter` body reads the snapshot view per element.
///
/// `fixed = [30, 10, 20]`, so `prev(vals) + fixed` is `[30,10,20]`, `[60,20,40]`,
/// `[35,30,40]` and the ascending orders are `[1,2,0]`, `[1,2,0]`, `[1,0,2]`.
/// Reading `vals` CURRENT would give `[1,2,0]`, `[0,1,2]`, `[0,1,2]`; a
/// broadcast of element 0's previous value gives `[1,2,0]` at every step.
#[test]
fn nested_previous_and_init_operands_materialize() {
    assert_series(
        &moving_series(
            "c3n_vso",
            "out[d]",
            "VECTOR SORT ORDER(PREVIOUS(vals[d]) + fixed[d], 1)",
            3,
        ),
        &[[1.0, 1.0, 1.0], [2.0, 2.0, 0.0], [0.0, 0.0, 2.0]],
        "VECTOR SORT ORDER arg0, nested PREVIOUS",
    );
    // Operand order must not matter: `find_expr_array_view` on an `Op2` takes
    // `lhs.or_else(rhs)`, and both sides now carry the same shape.
    assert_series(
        &moving_series(
            "c3n_vso_rhs",
            "out[d]",
            "VECTOR SORT ORDER(fixed[d] + PREVIOUS(vals[d]), 1)",
            3,
        ),
        &[[1.0, 1.0, 1.0], [2.0, 2.0, 0.0], [0.0, 0.0, 2.0]],
        "VECTOR SORT ORDER arg0, nested PREVIOUS on the right",
    );
    // INIT nested: `[30,10,20] + [30,10,20] = [60,20,40]` at every step, so the
    // order is `[1,2,0]` throughout -- which the PREVIOUS row above differs from
    // at t=2.
    assert_series(
        &moving_series(
            "c3n_vso_init",
            "out[d]",
            "VECTOR SORT ORDER(INIT(vals[d]) + fixed[d], 1)",
            3,
        ),
        &[[1.0, 1.0, 1.0], [2.0, 2.0, 2.0], [0.0, 0.0, 0.0]],
        "VECTOR SORT ORDER arg0, nested INIT",
    );
    // RANK, the sibling arm Phase C1 fixed: ranks of the same three arrays are
    // [3,1,2], [3,1,2], [2,1,3].
    assert_series(
        &moving_series(
            "c3n_rank",
            "out[d]",
            "RANK(PREVIOUS(vals[d]) + fixed[d], 1)",
            3,
        ),
        &[[3.0, 3.0, 2.0], [1.0, 1.0, 1.0], [2.0, 2.0, 3.0]],
        "RANK arg0, nested PREVIOUS",
    );
    // VECTOR ELM MAP, both positions. Source: `prev(vals) + fixed` mapped by the
    // current `offs` -- and the materialized source is a fresh contiguous temp,
    // so the mapping is confined to it (`materializing_an_elm_map_source_...`).
    // [30,10,20] by [2,0,1] -> [20,30,10]; [60,20,40] by [1,0,1] -> [20,60,20];
    // [35,30,40] by [0,0,1] -> [35,35,30].
    assert_series(
        &moving_series(
            "c3n_elm_src",
            "out[d]",
            "VECTOR ELM MAP(PREVIOUS(vals[d]) + fixed[d], offs[d])",
            3,
        ),
        &[[20.0, 20.0, 35.0], [30.0, 60.0, 35.0], [10.0, 20.0, 30.0]],
        "VECTOR ELM MAP arg0, nested PREVIOUS",
    );
    // Offsets: `prev(offs) + 0` -- `fixed` would swamp the index range, so the
    // nested arithmetic here is `PREVIOUS(offs[d]) * 1`, an `Op2` all the same.
    // Same values as the bare arg1 row.
    assert_series(
        &moving_series(
            "c3n_elm_off",
            "out[d]",
            "VECTOR ELM MAP(vals[d], PREVIOUS(offs[d]) * 1)",
            3,
        ),
        &[[30.0, 20.0, 30.0], [30.0, 5.0, -20.0], [30.0, 20.0, 30.0]],
        "VECTOR ELM MAP arg1, nested PREVIOUS",
    );
    // VECTOR SELECT, both positions. Selection: `prev(sel) * 1` is the same
    // array, so the values match the bare arg0 row.
    assert_series(
        &moving_series(
            "c3n_sel_sel",
            "out[d]",
            "VECTOR SELECT(PREVIOUS(sel[*]) * 1, vals[*], 0, 0, 0) \
             + SUM(VECTOR SORT ORDER(fixed[*], 1))",
            3,
        ),
        &[[3.0, 28.0, 33.0], [3.0, 28.0, 33.0], [3.0, 28.0, 33.0]],
        "VECTOR SELECT arg0, nested PREVIOUS",
    );
    // Values: current `sel` over `prev(vals) + fixed`. [1,1,0] over [30,10,20]
    // -> 40; [0,1,0] over [60,20,40] -> 20; [0,1,0] over [35,30,40] -> 30.
    assert_series(
        &moving_series(
            "c3n_sel_val",
            "out[d]",
            "VECTOR SELECT(sel[*], PREVIOUS(vals[*]) + fixed[*], 0, 0, 0) \
             + SUM(VECTOR SORT ORDER(fixed[*], 1))",
            3,
        ),
        &[[43.0, 23.0, 33.0], [43.0, 23.0, 33.0], [43.0, 23.0, 33.0]],
        "VECTOR SELECT arg1, nested PREVIOUS",
    );
    // The reducer positions, which C1+C2 could not even reach: the argument
    // died in `builtins_visitor`'s capture-helper synthesis before the
    // materializer saw it. It now passes through, so the operand materializes.
    // `SUM(prev(matrix[e,*]) + matrix[e,*])` is the previous row's sum plus this
    // row's. Row 1: 0+6, 6+7, 7+8 -> 6, 13, 15. Row 2: 0+60, 60+70, 70+80 ->
    // 60, 130, 150. Reading `matrix` current on BOTH sides would double the
    // current row (12, 14, 16 and 120, 140, 160).
    assert_series(
        &moving_series(
            "c3n_sum",
            "out[e]",
            "SUM(PREVIOUS(matrix[e,*]) + matrix[e,*])",
            2,
        ),
        &[[6.0, 13.0, 15.0], [60.0, 130.0, 150.0]],
        "SUM over a nested PREVIOUS",
    );
    assert_series(
        &moving_series(
            "c3n_mean",
            "out[e]",
            "MEAN(PREVIOUS(matrix[e,*]) + matrix[e,*])",
            2,
        ),
        &[[2.0, 13.0 / 3.0, 5.0], [20.0, 130.0 / 3.0, 50.0]],
        "MEAN over a nested PREVIOUS",
    );
}

/// `ALLOCATE AVAILABLE` / `ALLOCATE BY PRIORITY`, the two positions the
/// materializer does hoist.
///
/// A bisection over allocation curves is not hand-computable the way a sort
/// order is, so -- exactly as [`allocate_positions`] does for the computed rows
/// -- each row is pinned against the model that captures the same array into a
/// variable of its own first. That reference is the PER-ELEMENT `LoadPrev`
/// route, which is the oracle this whole phase has to agree with, and the row
/// separately asserts it differs from the unfrozen model so "the previous values
/// were actually read" is asserted rather than assumed.
#[test]
fn allocate_previous_operands_agree_with_the_per_element_capture() {
    struct Row {
        what: &'static str,
        inline: &'static str,
        helper: (&'static str, &'static str),
        reference: &'static str,
        raw: &'static str,
    }
    let rows = [
        Row {
            what: "allocate_available arg0 (requests)",
            inline: "allocate_available(PREVIOUS(request[d]), pp[d,1], supply)",
            helper: ("prev_req[d]", "PREVIOUS(request[d])"),
            reference: "allocate_available(prev_req[d], pp[d,1], supply)",
            raw: "allocate_available(request[d], pp[d,1], supply)",
        },
        Row {
            what: "allocate_by_priority arg0 (requests)",
            inline: "allocate_by_priority(PREVIOUS(request[d]), priority[d], 0, width, supply)",
            helper: ("prev_req[d]", "PREVIOUS(request[d])"),
            reference: "allocate_by_priority(prev_req[d], priority[d], 0, width, supply)",
            raw: "allocate_by_priority(request[d], priority[d], 0, width, supply)",
        },
        Row {
            what: "allocate_by_priority arg1 (priorities)",
            inline: "allocate_by_priority(request[d], PREVIOUS(priority[d]), 0, width, supply)",
            helper: ("prev_pri[d]", "PREVIOUS(priority[d])"),
            reference: "allocate_by_priority(request[d], prev_pri[d], 0, width, supply)",
            raw: "allocate_by_priority(request[d], priority[d], 0, width, supply)",
        },
    ];

    // A moving ALLOCATE fixture: both the requests and the priorities change
    // every step, so freezing either one changes the allocation.
    let fixture = |name: &str| {
        TestProject::new(name)
            .with_sim_time(0.0, 2.0, 1.0)
            .indexed_dimension("d", 3)
            .indexed_dimension("xp", 4)
            .array_with_ranges(
                "request[d]",
                vec![("1", "10 + 10 * TIME"), ("2", "20"), ("3", "30 - 5 * TIME")],
            )
            .array_with_ranges(
                "priority[d]",
                vec![("1", "3"), ("2", "1 + TIME"), ("3", "2")],
            )
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
    };

    let series = |project: TestProject| -> Vec<Vec<f64>> {
        project.assert_compiles_incremental();
        let all = project.run_vm_expecting_success();
        (1..=3)
            .map(|k| all.get(&format!("out[{k}]")).unwrap().clone())
            .collect()
    };

    for (i, row) in rows.iter().enumerate() {
        let inline = series(fixture(&format!("c3_alloc_i{i}")).array_aux("out[d]", row.inline));
        let reference = series(
            fixture(&format!("c3_alloc_r{i}"))
                .array_aux(row.helper.0, row.helper.1)
                .array_aux("out[d]", row.reference),
        );
        let raw = series(fixture(&format!("c3_alloc_w{i}")).array_aux("out[d]", row.raw));

        for (k, (a, e)) in inline.iter().zip(reference.iter()).enumerate() {
            assert_close(
                a,
                e,
                &format!(
                    "{}: the inline array PREVIOUS must allocate exactly as the \
                     per-element capture helper does (element {})",
                    row.what,
                    k + 1
                ),
            );
        }
        assert_ne!(
            inline, raw,
            "{}: the fixture must make freezing the operand change the answer, \
             otherwise this row proves nothing (frozen {inline:?}, raw {raw:?})",
            row.what
        );
    }
}

/// The equivalence the whole design rests on, asserted directly: an array-valued
/// `PREVIOUS` reads, element for element and step for step, exactly what a
/// per-element `LoadPrev` with the same fallback reads.
///
/// The reference model captures `PREVIOUS(vals[d])` into an ordinary arrayed aux
/// -- which compiles to one `LoadPrev` per element, the route that has always
/// worked -- and then feeds THAT array to the same builtin. The two must agree
/// at every step, including the first, where the view route reads no buffer at
/// all and the scalar route returns its fallback.
///
/// Rows cover the two snapshot regions and both a whole-array and a strict-slice
/// argument, since those reach different parts of the view arithmetic.
#[test]
fn an_array_snapshot_view_agrees_with_the_per_element_capture() {
    struct Row {
        what: &'static str,
        lhs: &'static str,
        inline: &'static str,
        helper: (&'static str, &'static str),
        reference: &'static str,
        n: usize,
    }
    let rows = [
        Row {
            what: "PREVIOUS of a whole array, VECTOR SORT ORDER",
            lhs: "out[d]",
            inline: "VECTOR SORT ORDER(PREVIOUS(vals[d]), 1)",
            helper: ("cap[d]", "PREVIOUS(vals[d])"),
            reference: "VECTOR SORT ORDER(cap[d], 1)",
            n: 3,
        },
        Row {
            what: "INIT of a whole array, VECTOR SORT ORDER",
            lhs: "out[d]",
            inline: "VECTOR SORT ORDER(INIT(vals[d]), 1)",
            helper: ("cap[d]", "INIT(vals[d])"),
            reference: "VECTOR SORT ORDER(cap[d], 1)",
            n: 3,
        },
        Row {
            what: "PREVIOUS of a row slice, SUM",
            lhs: "out[e]",
            inline: "SUM(PREVIOUS(matrix[e,*]))",
            helper: ("cap[e,d]", "PREVIOUS(matrix[e,d])"),
            reference: "SUM(cap[e,*])",
            n: 2,
        },
        Row {
            what: "INIT of a row slice, SUM",
            lhs: "out[e]",
            inline: "SUM(INIT(matrix[e,*]))",
            helper: ("cap[e,d]", "INIT(matrix[e,d])"),
            reference: "SUM(cap[e,*])",
            n: 2,
        },
    ];

    for (i, row) in rows.iter().enumerate() {
        let inline = moving_series(&format!("c3_eq_i{i}"), row.lhs, row.inline, row.n);
        let reference_project = moving_fixture(&format!("c3_eq_r{i}"))
            .array_aux(row.helper.0, row.helper.1)
            .array_aux(row.lhs, row.reference);
        reference_project.assert_compiles_incremental();
        let all = reference_project.run_vm_expecting_success();
        let reference: Vec<Vec<f64>> = (1..=row.n)
            .map(|k| all.get(&format!("out[{k}]")).unwrap().clone())
            .collect();
        for (k, (a, e)) in inline.iter().zip(reference.iter()).enumerate() {
            assert_close(a, e, &format!("{}: element {}", row.what, k + 1));
        }
    }
}

/// FIRST-STEP SEMANTICS, stated as its own row rather than left implicit in the
/// series above.
///
/// `Opcode::LoadPrev` returns its caller-supplied fallback while
/// `use_prev_fallback` is set -- i.e. until the first snapshot is taken at the
/// end of step 0 -- and unary `PREVIOUS(x)` desugars to `PREVIOUS(x, 0)`. The
/// view route reproduces that by reading the fallback 0 for every element
/// (`vm::ChunkRegions::backing`'s `None` arm, and the wasm backend's `select` on
/// the same flag), which is why an array-valued `PREVIOUS` may carry no other
/// fallback.
///
/// `SUM(PREVIOUS(vals[d]))` at t=0 must therefore be 0, not `SUM(vals)` = 60 and
/// not a NaN from an unwritten buffer. `MIN` and `MAX` are included because they
/// would surface a stale or uninitialized buffer as an out-of-range extremum
/// rather than as a plausible zero.
#[test]
fn the_first_step_of_an_array_previous_is_the_scalar_fallback() {
    assert_series(
        &moving_series("c3_first_sum", "out[d]", "SUM(PREVIOUS(vals[*]))", 3),
        &[[0.0, 60.0, 45.0], [0.0, 60.0, 45.0], [0.0, 60.0, 45.0]],
        "SUM of a PREVIOUS view",
    );
    assert_series(
        &moving_series("c3_first_min", "out[d]", "MIN(PREVIOUS(vals[*]))", 3),
        &[[0.0, 10.0, 5.0], [0.0, 10.0, 5.0], [0.0, 10.0, 5.0]],
        "MIN of a PREVIOUS view",
    );
    assert_series(
        &moving_series("c3_first_max", "out[d]", "MAX(PREVIOUS(vals[*]))", 3),
        &[[0.0, 30.0, 20.0], [0.0, 30.0, 20.0], [0.0, 30.0, 20.0]],
        "MAX of a PREVIOUS view",
    );
    // The explicit spelling of the default fallback is the same value and must
    // stay accepted: `PREVIOUS(x)` desugars to exactly this.
    assert_series(
        &moving_series(
            "c3_first_explicit",
            "out[d]",
            "SUM(PREVIOUS(vals[*], 0))",
            3,
        ),
        &[[0.0, 60.0, 45.0], [0.0, 60.0, 45.0], [0.0, 60.0, 45.0]],
        "SUM of a PREVIOUS view with an explicit 0 fallback",
    );
}

/// The VM's half of the first-step semantics across a RESET, which one run
/// cannot reach: `Vm::reset` clears `prev_values_valid`, and a snapshot view
/// must go back to reading the fallback rather than the finished run's last
/// snapshot.
///
/// The VM is doubly protected here (it also zero-fills `prev_values` on reset),
/// which is exactly why this is asserted rather than assumed: the wasm backend
/// deliberately does NOT clear its snapshot regions and reproduces the semantics
/// with a `select` instead
/// (`wasmgen::module_tests::compile_simulation_repeated_run_resets_previous_fallback_for_an_array_view`,
/// which fails without it). The two backends must agree, so both sides of the
/// axis carry a row.
#[test]
fn a_reset_run_reads_the_fallback_again() {
    let project = moving_fixture("c3_reset").array_aux("out[d]", "SUM(PREVIOUS(vals[*]))");
    let compiled = project
        .compile_incremental()
        .expect("the fixture should compile");
    let mut vm = crate::vm::Vm::new(compiled).expect("VM creation should succeed");
    vm.run_to_end().expect("first run");
    let first = vm
        .get_series(&crate::common::Ident::new("out[1]"))
        .expect("out[1] series");
    vm.reset();
    vm.run_to_end().expect("second run");
    let second = vm
        .get_series(&crate::common::Ident::new("out[1]"))
        .expect("out[1] series");
    assert_close(&first, &[0.0, 60.0, 45.0], "first run");
    assert_close(&second, &first, "a reset run must reproduce the first run");
}

/// The pinned decline: a NON-default fallback on an array-valued `PREVIOUS`.
///
/// A view carries no per-call-site scalar, so the array route can only reproduce
/// the fallback the snapshot buffer already reads as before its first snapshot,
/// which is 0. Approximating -- silently reading 0 where the model asked for 5
/// -- would be a wrong number on the first step of every run, so the shape is
/// refused instead. The scalar spelling is unaffected, and the row below shows
/// the workaround: capture the array into a variable of its own, where each
/// element's `LoadPrev` carries the fallback.
#[test]
fn a_non_default_array_previous_fallback_declines_loudly() {
    assert_fails_attributed(
        moving_fixture("c3_fb_reject")
            .array_aux("out[d]", "VECTOR SORT ORDER(PREVIOUS(vals[d], 5), 1)"),
        "array PREVIOUS with a non-zero fallback",
    );
    // The rejection must name the FALLBACK, not merely fail: this construct is
    // one the practitioner can fix, and the message says how. Asserted through
    // the per-variable diagnostic, which is the surface a user reads.
    assert_declines_because(
        moving_fixture("c3_fb_reason")
            .array_aux("out[d]", "VECTOR SORT ORDER(PREVIOUS(vals[d], 5), 1)"),
        "out",
        "nowhere to carry a fallback",
    );
    // `-0.0` is not the default either, and the check compares BIT PATTERNS so
    // that it is not. The spelling matters: `-0` is a negation of the literal
    // `0`, which constant folding turns into `+0.0`, so it is accepted and IS
    // the default. `0 * -1` folds to a genuine `-0.0` (the shape
    // `compiler::fold` is documented to produce), and `1 / PREVIOUS(x, 0 * -1)`
    // is negative infinity where `1 / PREVIOUS(x, 0)` is positive -- a value
    // comparison would silently accept it and read the wrong sign of infinity
    // on the first step.
    assert_fails_attributed(
        moving_fixture("c3_fb_negzero")
            .array_aux("out[d]", "VECTOR SORT ORDER(PREVIOUS(vals[d], 0 * -1), 1)"),
        "array PREVIOUS with a -0.0 fallback",
    );
    // `-0` IS accepted, and that is the other half of the bit-pattern claim:
    // the literal is a negation of `0`, which constant folding turns back into
    // `+0.0`, so it IS the default and must not be refused. Only the folded
    // `0 * -1` above produces a genuine `-0.0`. Same series as the bare
    // `PREVIOUS(vals[d])` row, since the fallback is the default either way.
    assert_series(
        &moving_series(
            "c3_fb_negzero_ok",
            "out[d]",
            "VECTOR SORT ORDER(PREVIOUS(vals[d], -0), 1)",
            3,
        ),
        &[[0.0, 1.0, 0.0], [1.0, 2.0, 1.0], [2.0, 0.0, 2.0]],
        "a `-0` fallback folds to the default and is accepted",
    );

    // The workaround compiles and is per-element correct: at t=0 every element
    // reads 5, so the sort order is the identity under stable ties; afterwards
    // it is the previous array's order, matching the bare row above.
    let workaround = moving_fixture("c3_fb_helper")
        .array_aux("cap[d]", "PREVIOUS(vals[d], 5)")
        .array_aux("out[d]", "VECTOR SORT ORDER(cap[d], 1)");
    workaround.assert_compiles_incremental();
    let all = workaround.run_vm_expecting_success();
    assert_close(all.get("out[1]").unwrap(), &[0.0, 1.0, 0.0], "out[1]");
    assert_close(all.get("out[2]").unwrap(), &[1.0, 2.0, 1.0], "out[2]");
    assert_close(all.get("out[3]").unwrap(), &[2.0, 0.0, 2.0], "out[3]");
}

/// The three spellings of an arrayed reference, at `VECTOR SORT ORDER` arg0.
///
/// They arrive from three different directions and only one of them ever
/// reached lowering intact before: `vals` (a bare name) already lowered to a
/// whole-array view; `vals[*]` and `vals[d]` were claimed by
/// `builtins_visitor`'s capture-helper synthesis, which cannot hold an array
/// (`vals[*]` in a scalar `Equation::Scalar` helper does not compile) or pinned
/// them to one element (`substitute_dimension_refs` rewriting `d` to `d·elem`).
/// All three must now mean the same array.
#[test]
fn all_three_arrayed_previous_spellings_agree() {
    let expected = [[0.0, 1.0, 0.0], [1.0, 2.0, 1.0], [2.0, 0.0, 2.0]];
    for (name, eqn) in [
        ("c3_sp_dim", "VECTOR SORT ORDER(PREVIOUS(vals[d]), 1)"),
        ("c3_sp_star", "VECTOR SORT ORDER(PREVIOUS(vals[*]), 1)"),
        ("c3_sp_bare", "VECTOR SORT ORDER(PREVIOUS(vals), 1)"),
    ] {
        assert_series(
            &moving_series(name, "out[d]", eqn, 3),
            &expected,
            &format!("spelling: {eqn}"),
        );
    }

    // The other side of the boundary, unchanged: `PREVIOUS` of a SINGLE element
    // is still a scalar that broadcasts, not an array. `matrix[e,1]` pins the
    // trailing index, so the reference collapses to one slot and the operand is
    // `prev(matrix[e,0]) + matrix[e,*]` per element.
    //
    // The scalar is BROADCAST across the three reduced elements, so the sum is
    // `3 * prev_element + row_sum`. Row 1: prev(matrix[1,1]) is 0, 1, 2 and the
    // row sums are 6, 7, 8 -> 6, 10, 14. Row 2: prev(matrix[2,1]) is 0, 10, 10
    // over rows summing 60, 70, 80 -> 60, 100, 110. (Treating the element as an
    // ARRAY instead would give the row-slice sums 0, 6, 7 added to 6, 7, 8.)
    assert_series(
        &moving_series(
            "c3_sp_element",
            "out[e]",
            "SUM(PREVIOUS(matrix[e,1]) + matrix[e,*])",
            2,
        ),
        &[[6.0, 10.0, 14.0], [60.0, 100.0, 110.0]],
        "PREVIOUS of a fixed element still broadcasts",
    );
}

/// The DEGENERATE half of the view-operand rule, pinned so GH #995's "do NOT
/// simply make everything compile" section can be checked against it.
///
/// An element-collapsed `PREVIOUS`/`INIT` in a rank-like position now compiles,
/// to a one-element view -- a constant `0` sort order, a constant `1` rank. That
/// is the trap the issue names, and what makes it acceptable here is that it is
/// EXACTLY what the non-`PREVIOUS` twin already produced: `VECTOR SORT
/// ORDER(vals[1], 1)` is the same constant 0 at HEAD, and has been. C3 did not
/// create a degenerate answer; it stopped `PREVIOUS` from being the one operand
/// that behaved differently from its own argument in the same position.
///
/// The half the issue actually warns about -- the LTM ceteris-paribus wrap
/// pinning a rank-like builtin's ARGUMENT down to one element, turning a loud
/// drop into a plausible constant-0 score -- is unaffected: `ltm_agg`'s
/// rank-like decline is independent of compilability, and C-LEARN's five
/// `rank-like-partial` declines are byte-identical before and after C3.
#[test]
fn an_element_collapsed_snapshot_in_a_rank_like_position_matches_its_curr_twin() {
    for (name, eqn, expected) in [
        ("c3_degen_vso", "VECTOR SORT ORDER(vals[1], 1)", 0.0),
        (
            "c3_degen_vso_prev",
            "VECTOR SORT ORDER(PREVIOUS(vals[1]), 1)",
            0.0,
        ),
        ("c3_degen_rank", "RANK(vals[1], 1)", 1.0),
        ("c3_degen_rank_prev", "RANK(PREVIOUS(vals[1]), 1)", 1.0),
    ] {
        let series = moving_series(name, "out[d]", eqn, 3);
        for (k, s) in series.iter().enumerate() {
            assert_close(
                s,
                &[expected; 3],
                &format!(
                    "{eqn}: a one-element view is degenerate at element {}",
                    k + 1
                ),
            );
        }
    }
}

/// The shape PR #1001 was written against, verbatim: a per-row `VECTOR SELECT`
/// over the previous step's matrix rows.
///
/// `sel2` selects columns 1 and 3 of row 1 and column 2 of row 2, over
/// `PREVIOUS(matrix[Row,*])`. The previous rows are `[0,0,0]`/`[0,0,0]`, then
/// `[1,2,3]`/`[10,20,30]`, then `[2,2,3]`/`[10,20,40]`, so the selected sums are
/// 0/0, 1+3=4 / 20, and 2+3=5 / 20.
#[test]
fn the_gh_1001_user_shape_compiles_and_reads_the_previous_row() {
    let project = moving_fixture("c3_user_shape")
        .array_with_ranges(
            "sel2[e,d]",
            vec![
                ("1,1", "1"),
                ("1,2", "0"),
                ("1,3", "1"),
                ("2,1", "0"),
                ("2,2", "1"),
                ("2,3", "0"),
            ],
        )
        .array_aux(
            "picked[e]",
            "VECTOR SELECT(sel2[e,*], PREVIOUS(matrix[e,*]), 0, 0, 0)",
        );
    project.assert_compiles_incremental();
    let all = project.run_vm_expecting_success();
    assert_close(all.get("picked[1]").unwrap(), &[0.0, 4.0, 5.0], "picked[1]");
    assert_close(
        all.get("picked[2]").unwrap(),
        &[0.0, 20.0, 20.0],
        "picked[2]",
    );
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
        &project.vm_result("out"),
        &[1.0, 2.0, 0.0],
        "a scalar PREVIOUS beside an array operand",
    );

    let init = fixture("scalar_init")
        .scalar_const("s", 5.0)
        .array_aux("out[d]", "VECTOR SORT ORDER(vals[d] + INIT(s), 1)");
    init.assert_compiles_incremental();
    assert_close(
        &init.vm_result("out"),
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
        &fixed_element.vm_result("out"),
        &[12.0, 93.0],
        "a PREVIOUS of a fixed element broadcasts correctly and must not decline",
    );

    // A SCALAR `PREVIOUS`/`INIT` directly in a view position. `SUM(s)` for a
    // scalar `s` has always compiled -- `walk_expr_as_view`'s `Expr::Var` arm
    // pushes a one-element view -- while `SUM(PREVIOUS(s))` did not, which was
    // the same incoherence as the array rows. Both now take the same route, so
    // the reduce reads one element of the snapshot: `s = 10 + 5 * TIME`, so
    // `SUM(PREVIOUS(s))` is 0 (the fallback), 10, 15 and `SUM(INIT(s))` is 10
    // throughout.
    for (name, eqn, expected) in [
        ("scalar_view_prev", "SUM(PREVIOUS(s))", [0.0, 10.0, 15.0]),
        ("scalar_view_init", "SUM(INIT(s))", [10.0, 10.0, 10.0]),
    ] {
        let project = moving_fixture(name)
            .aux("s", "10 + 5 * TIME", None)
            .array_aux("out[d]", eqn);
        project.assert_compiles_incremental();
        let all = project.run_vm_expecting_success();
        assert_close(all.get("out[1]").unwrap(), &expected, eqn);
    }
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
/// The temp count is derived from the lowered expressions the same way
/// assembly derives a fragment's `temp_sizes` (`extract_temp_sizes`), so it is
/// the direct readout of how many temps the fragment allocates.
#[test]
fn a_computed_operand_costs_exactly_one_temp_and_a_view_costs_none() {
    let temps = |name: &str, eqn: &str| -> usize {
        let mut sizes = std::collections::HashMap::new();
        for expr in fixture(name).array_aux("out[d]", eqn).flow_exprs("out") {
            crate::compiler::extract_temp_sizes_pub(&expr, &mut sizes);
        }
        sizes.len()
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
    let all = project.run_vm_expecting_success();
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
// A body only one element reads is evaluated per element on an id the elements
// REISSUE, so an equation costs one id per simultaneously-live temp rather than
// one per element and a few hundred elements is nowhere near the `u8`
// namespace. Both rows below sit where the old one-id-per-element numbering ran
// out, and check that the arithmetic is right rather than that it is large.
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
        &project.vm_result("out"),
        &expected,
        "300-element per-element hoist, no temp read as a view",
    );
}

/// The same hoist WITH a materialized operand, which is where the `u8` `TempId`
/// namespace used to be spent one id per element (GH #583): the operand is a
/// temp read as a static VIEW, the one place an id is carried as a `u32` while
/// every writer narrows it to `u8`, so above 255 the view read storage no
/// opcode wrote.
///
/// It costs two ids now, whatever the element count: the operand
/// `301 - vals[*]` is the same body in every element and is materialized ONCE,
/// and the sort that reads it varies with `dir[d]` and RECYCLES a second id
/// across the elements. A 300-element equation therefore stays as far inside
/// the namespace as a 3-element one, which is what makes the arithmetic below
/// checkable rather than merely large.
///
/// The refusal itself is not deleted -- `symbolic::resolve_static_view` still
/// rejects a view over a temp above 255, pinned directly by
/// `symbolic::tests::test_resolve_static_view_temp_past_the_id_namespace` --
/// but no equation this shape can reach it any more.
///
/// Both spellings are rowed because they arrive from different directions: the
/// `vals[*]` operand was always element-invariant, while `vals[d]` is promoted
/// back to the whole axis by the vector builtin's own operand rule and so
/// becomes the same body.
#[test]
fn a_per_element_hoist_over_a_shared_operand_costs_two_ids_at_any_size() {
    const N: usize = 300;
    for (name, eqn) in [
        ("temp_view_dim", "VECTOR SORT ORDER(301 - vals[d], dir[d])"),
        ("temp_view_star", "VECTOR SORT ORDER(301 - vals[*], dir[d])"),
    ] {
        let project = sort_project(name, N, eqn);
        project.assert_compiles_incremental();
        // `vals` DEcreases and the operand `301 - vals` INcreases, so the two
        // sort orders are exact swaps of the sibling row's: element `k` sorts
        // ascending when `k` is odd (`sort_project`'s `dir`), and over an
        // increasing operand ascending is the identity `k` while descending is
        // the reversal `N - 1 - k`.
        let expected: Vec<f64> = (0..N)
            .map(|k| if k % 2 == 1 { k } else { N - 1 - k } as f64)
            .collect();
        assert_close(&project.vm_result("out"), &expected, eqn);
    }
}

/// An array-producing builtin nested inside *arithmetic* that is itself an
/// array operand: two temps, the inner one written before the outer body reads
/// it.
///
/// This is where "materialize every array value" earns the word EVERY. The
/// enclosing `Op2` is an operand and has to become a temp; codegen evaluates
/// such a temp with a `BeginIter` loop, which has no way to run an
/// array-producing opcode inside its body. So the inner call is materialized
/// FIRST, in its own array-valued position, and the `Op2` that reads it is an
/// ordinary elementwise body.
///
/// Values: `VECTOR ELM MAP(vals, offs)` over `vals = [30, 10, 20]` and
/// `offs = [2, 0, 1]` is `[vals[2], vals[0], vals[1]] = [20, 30, 10]`; plus
/// `bump = [0, 100, 0]` that is `[20, 130, 10]`; sorting ascending gives the
/// source indices in value order, `[2, 0, 1]`. Both spellings of the operand --
/// the wildcard one and the apply-to-all one, which promotes its element
/// references back to whole axes -- are the same operand and give the same
/// array.
#[test]
fn a_nested_array_producing_builtin_inside_arithmetic_materializes_first() {
    for (name, eqn) in [
        (
            "nested_dim",
            "VECTOR SORT ORDER(VECTOR ELM MAP(vals[d], offs[d]) + bump[d], 1)",
        ),
        (
            "nested_star",
            "VECTOR SORT ORDER(VECTOR ELM MAP(vals[*], offs[*]) + bump[*], 1)",
        ),
    ] {
        let project = fixture(name).array_aux("out[d]", eqn);
        project.assert_compiles_incremental();
        assert_close(&project.vm_result("out"), &[2.0, 0.0, 1.0], eqn);
    }
}

/// GH #995's own table, re-run. Every row the issue reported as failing now
/// compiles, and each is checked against the reading it is supposed to have.
///
/// Two rows resolve by COHERENCE rather than by gaining an array meaning, and
/// they are the ones worth stating: a single element stays a single element
/// under `PREVIOUS`, so in an array-operand position it pushes a ONE-ELEMENT
/// view -- a legitimate `VECTOR ELM MAP` base (the mapping ranges over the whole
/// source variable) and a degenerate one-element `VECTOR SELECT`.
///
/// The SPELLING decides which route the element takes, and the two are
/// different: a NUMERIC index (`vals[1]`) reaches the view over the snapshot,
/// while the bare element NAME the issue's table uses (`vals[e1]`) is not
/// accepted as a static index on the user-equation parse path, so it is read
/// through a scalar capture helper of extent one instead. Both are asserted
/// below, each against its own oracle, rather than one standing in for the
/// other -- the comments on each `compare` call carry the difference.
#[test]
fn every_row_of_the_issue_995_table_compiles() {
    // The issue's own dimension: element names, so `vals[e1]` is a literal
    // element rather than an index.
    let base = |name: &str| {
        TestProject::new(name)
            .with_sim_time(0.0, 2.0, 1.0)
            .named_dimension("d", &["e1", "e2", "e3"])
            .array_with_ranges(
                "vals[d]",
                vec![
                    ("e1", "30 - 25 * TIME"),
                    ("e2", "10 + 10 * TIME"),
                    ("e3", "20"),
                ],
            )
            .array_with_ranges(
                "offs[d]",
                vec![("e1", "2 - TIME"), ("e2", "0"), ("e3", "1")],
            )
    };

    // Every row of the table, in the issue's order. The first three were
    // reported as compiling and are the controls; the rest were reported as
    // failing.
    let rows = [
        "VECTOR SORT ORDER(vals[d], 1)",
        "VECTOR ELM MAP(vals[e1], offs[d])",
        "RANK(vals[*], 1)",
        "VECTOR SORT ORDER(PREVIOUS(vals[d]), 1)",
        "VECTOR ELM MAP(PREVIOUS(vals[e1]), offs[d])",
        "VECTOR ELM MAP(vals[e1], PREVIOUS(offs[d]))",
        "VECTOR SORT ORDER(INIT(vals[d]), 1)",
        "VECTOR SORT ORDER(vals[d] * 2, 1)",
        "VECTOR SELECT(PREVIOUS(offs[d]), vals[d], 0, 1, 0)",
        "RANK(vals[*] * 2, 1)",
    ];
    for (i, eqn) in rows.iter().enumerate() {
        base(&format!("t995_{i}"))
            .array_aux("out[d]", eqn)
            .assert_compiles_incremental();
    }

    // The two coherence rows, against the per-element capture. `cap` holds
    // `PREVIOUS(vals[d])` / `PREVIOUS(offs[d])` element by element -- one
    // `LoadPrev` per slot -- so substituting it for the inline `PREVIOUS` must
    // not change a number.
    let compare = |what: &str, capture: (&str, &str), inline: &str, reference: &str| {
        let run = |name: &str, project: TestProject| -> Vec<Vec<f64>> {
            project.assert_compiles_incremental();
            let all = project.run_vm_expecting_success();
            ["e1", "e2", "e3"]
                .into_iter()
                .map(|k| {
                    all.get(&format!("out[{k}]"))
                        .unwrap_or_else(|| panic!("{name}: out[{k}] missing"))
                        .clone()
                })
                .collect()
        };
        let a = run(what, base("t995_i").array_aux("out[d]", inline));
        let b = run(what, {
            // The captures are a mix of arrayed and scalar helpers, so pick
            // the constructor from the name rather than hard-coding one.
            let p = base("t995_r");
            let p = if capture.0.contains('[') {
                p.array_aux(capture.0, capture.1)
            } else {
                p.aux(capture.0, capture.1, None)
            };
            p.array_aux("out[d]", reference)
        });
        for (k, (x, y)) in a.iter().zip(b.iter()).enumerate() {
            // NaN-tolerant: a mapped offset outside the source's extent is a
            // genuine `:NA:`, and two runs agreeing on WHERE the NaNs fall is
            // part of what is being checked.
            assert_eq!(x.len(), y.len(), "{what}: element {} length", k + 1);
            for (step, (p, q)) in x.iter().zip(y.iter()).enumerate() {
                assert!(
                    (p.is_nan() && q.is_nan()) || (p - q).abs() < 1e-9,
                    "{what}: element {} step {step} -- inline {p}, per-element capture {q} \
                     (inline {x:?}, reference {y:?})",
                    k + 1
                );
            }
        }
    };
    // A single-element PREVIOUS base for VECTOR ELM MAP, spelled with a NUMERIC
    // index: the argument reaches lowering as the same collapsed
    // `StaticSubscript` its `curr` twin does, so the source keeps the whole
    // variable's extent and the mapping ranges over the previous array.
    compare(
        "VECTOR ELM MAP with a single-element PREVIOUS base",
        ("cap[d]", "PREVIOUS(vals[d])"),
        "VECTOR ELM MAP(PREVIOUS(vals[1]), offs[d])",
        "VECTOR ELM MAP(cap[1], offs[d])",
    );
    // The SAME element spelled with its bare NAME takes a different route, and
    // that is the spelling the issue's table uses. `index_is_static` will not
    // accept an unqualified element name on the user-equation parse path (such a
    // name can be shadowed by a variable, and the disambiguating check is
    // deliberately disabled there to stay incremental under renames), so
    // `builtins_visitor` synthesizes a scalar capture helper and `PREVIOUS`
    // reads THAT. The source is then the helper -- one slot -- and ELM MAP's
    // "range over the source variable's full storage" rule applies to it. That
    // is the same rule a materialized operand follows
    // (`materializing_an_elm_map_source_confines_the_mapping_to_the_temp`) and
    // the same answer a practitioner gets by writing the capture out, so it is
    // self-consistent rather than a second semantics -- but the two spellings DO
    // mean different things, and only the front end decides which, so both are
    // pinned rather than one standing in for the other.
    compare(
        "VECTOR ELM MAP with a bare-element-name PREVIOUS base",
        ("h", "PREVIOUS(vals[e1])"),
        "VECTOR ELM MAP(PREVIOUS(vals[e1]), offs[d])",
        "VECTOR ELM MAP(h, offs[d])",
    );
    compare(
        "VECTOR SELECT over a single-element PREVIOUS selection",
        ("cap[d]", "PREVIOUS(offs[d])"),
        "VECTOR SELECT(PREVIOUS(offs[d]), vals[d], 0, 1, 0)",
        "VECTOR SELECT(cap[d], vals[d], 0, 1, 0)",
    );
}

// ===========================================================================
// Shape axis, second dimension: an operand mixing arrays of DIFFERENT shapes.
// ===========================================================================

/// A wider companion for the shared fixture: `matrix[e,d]` is already there,
/// and `rowv[e]` is the shape that is incomparable with `vals[d]`.
///
/// * `rowv = [5, 50]`
/// * `matrixt[d,e]` is `matrix` transposed, the shape that ties with
///   `matrix[e,d]` on containment while disagreeing on axis order.
fn wide_fixture(name: &str) -> TestProject {
    fixture(name)
        .array_with_ranges("rowv[e]", vec![("1", "5"), ("2", "50")])
        .array_with_ranges(
            "matrixt[d,e]",
            vec![
                ("1,1", "1"),
                ("1,2", "10"),
                ("2,1", "2"),
                ("2,2", "20"),
                ("3,1", "3"),
                ("3,2", "30"),
            ],
        )
}

/// Compile `out[e,d] = <eqn>` against the wide fixture and return `out`,
/// row-major (`[e1d1, e1d2, e1d3, e2d1, e2d2, e2d3]`).
fn wide_out_of(name: &str, eqn: &str) -> Vec<f64> {
    let project = wide_fixture(name).array_aux("out[e,d]", eqn);
    project.assert_compiles_incremental();
    project.vm_result("out")
}

/// Both spellings of a commutative mixed-shape operand must produce the same
/// array -- the property the first-wins shape rule broke.
///
/// A computed operand is evaluated by codegen's `AssignTemp` -> `BeginIter`
/// loop, which broadcasts each source view onto the ITERATION by dimension id
/// (`vm`'s `LoadIterViewAt` -> `dimensions::match_dimensions_two_pass`), and a
/// source dimension the iteration does not have reads NaN. Shaping the temp by
/// the first array in the operand therefore made
/// `VECTOR SORT ORDER(vals[d] + matrix[e,d], 1)` iterate over `vals`'s three
/// elements, read `matrix` as three NaNs and return the sort order of NaNs
/// (measured `[0,1,2, 0,1,2]`), while the commuted `matrix[e,d] + vals[d]` --
/// the same array -- returned the right answer. `compiler::join_array_views`
/// picks the shape by CONTAINMENT instead, which has no left-to-right in it.
///
/// The values: `vals = [30,10,20]` and `matrix = [[1,2,3],[10,20,30]]`, so the
/// sum is `[[31,12,23],[40,30,50]]` and the in-row ascending orders are
/// `[1,2,0]` and `[1,0,2]`.
#[test]
fn a_mixed_shape_operand_agrees_with_its_commuted_spelling() {
    let expected = [1.0, 2.0, 0.0, 1.0, 0.0, 2.0];
    // Narrow first -- the spelling that read NaNs.
    assert_close(
        &wide_out_of("mix_narrow", "VECTOR SORT ORDER(vals[d] + matrix[e,d], 1)"),
        &expected,
        "mixed-shape operand, narrow array first",
    );
    // Wide first -- the spelling that happened to work.
    assert_close(
        &wide_out_of("mix_wide", "VECTOR SORT ORDER(matrix[e,d] + vals[d], 1)"),
        &expected,
        "mixed-shape operand, wide array first",
    );

    // A DIMENSIONLESS subexpression is the degenerate case of the same rule: a
    // subscript collapsed to one element carries no dimensions, so it
    // broadcasts and constrains nothing. Reading the first view blind made the
    // two orders disagree about whether the equation compiles AT ALL --
    // `vals[1] + bump[d]` was rejected while `bump[d] + vals[1]` compiled.
    // `vals[1] + bump = [30, 130, 30]`, ascending with stable ties `[0, 2, 1]`.
    assert_close(
        &out_of("mix_elem_lhs", "VECTOR SORT ORDER(vals[1] + bump[d], 1)"),
        &[0.0, 2.0, 1.0],
        "collapsed element first",
    );
    assert_close(
        &out_of("mix_elem_rhs", "VECTOR SORT ORDER(bump[d] + vals[1], 1)"),
        &[0.0, 2.0, 1.0],
        "collapsed element second",
    );
}

/// The join is over ALL the shapes, not just two, and it is order-independent
/// in the strong sense: no permutation of a three-array operand may change the
/// answer.
///
/// This is what makes the choice a maximum rather than a left-to-right fold. A
/// fold over `[d], [d], [e,d]` is fine, but a fold over `[e], [d], [e,d]` --
/// the shape of an operand mixing a row vector, a column vector and the matrix
/// they broadcast into -- would call the first two incomparable and decline
/// before ever seeing the third.
///
/// `vals + bump = [30,110,20]`, plus `matrix` rows gives `[[31,112,23],
/// [40,130,50]]`, whose in-row ascending orders are `[2,0,1]` and `[0,2,1]`.
#[test]
fn a_three_array_operand_joins_regardless_of_order() {
    let expected = [2.0, 0.0, 1.0, 0.0, 2.0, 1.0];
    for (name, eqn) in [
        (
            "mix3_a",
            "VECTOR SORT ORDER(vals[d] + bump[d] + matrix[e,d], 1)",
        ),
        (
            "mix3_b",
            "VECTOR SORT ORDER(matrix[e,d] + vals[d] + bump[d], 1)",
        ),
        (
            "mix3_c",
            "VECTOR SORT ORDER(vals[d] + matrix[e,d] + bump[d], 1)",
        ),
    ] {
        assert_close(&wide_out_of(name, eqn), &expected, eqn);
    }
    // The row/column/matrix mix a fold would decline on its second step. Only
    // `matrix` is maximal, so the join is `[e,d]`.
    // `rowv = [5,50]` broadcast down the rows plus `vals = [30,10,20]` across
    // them plus `matrix` gives `[[36,17,28],[90,80,100]]`; in-row ascending
    // orders `[1,2,0]` and `[1,0,2]`.
    assert_close(
        &wide_out_of(
            "mix3_rcm",
            "VECTOR SORT ORDER(rowv[e] + vals[d] + matrix[e,d], 1)",
        ),
        &[1.0, 2.0, 0.0, 1.0, 0.0, 2.0],
        "row + column + matrix",
    );
}

/// Every shape-carrying `Expr` variant reaches the same join, checked at the
/// one position (`VECTOR SORT ORDER` arg0) the shape axis is exercised at --
/// the mixed-shape twin of [`computed_operand_shapes`].
///
/// The `If` row is the one that is not merely a repeat of the `Op2` rule: the
/// CONDITION is a fourth operand, and it is read by the `BeginIter` body
/// (`codegen::collect_iter_source_views_impl` pushes its view) even though it
/// contributes nothing to an `IF` whose arms already agree. A shape derivation
/// that skipped it sized the temp from the arms alone and the condition read
/// NaN, which compares false, so the `IF` silently collapsed to its ELSE arm
/// for every element (measured `[0,2,1, 0,2,1]`).
#[test]
fn every_operand_shape_reaches_the_mixed_shape_join() {
    // Op2: covered by `a_mixed_shape_operand_agrees_with_its_commuted_spelling`.

    // Op1 (`NOT`, the only one a fragment can carry -- see
    // `computed_operand_shapes`). `vals < matrix` is [F,F,F] on row 1 and
    // [F,T,T] on row 2, so `NOT` gives [[1,1,1],[1,0,0]] and the in-row
    // ascending orders are [0,1,2] and [1,2,0].
    assert_close(
        &wide_out_of(
            "mixs_op1",
            "VECTOR SORT ORDER(NOT (vals[d] < matrix[e,d]), 1)",
        ),
        &[0.0, 1.0, 2.0, 1.0, 2.0, 0.0],
        "Op1 over a mixed-shape comparison",
    );

    // If, with the wide array in the CONDITION and both arms narrow.
    // `matrix > 5` is false across row 1 and true across row 2, so the result
    // is `bump = [0,100,0]` then `vals = [30,10,20]`; in-row ascending orders
    // [0,2,1] and [1,2,0].
    assert_close(
        &wide_out_of(
            "mixs_if_cond",
            "VECTOR SORT ORDER(IF matrix[e,d] > 5 THEN vals[d] ELSE bump[d], 1)",
        ),
        &[0.0, 2.0, 1.0, 1.0, 2.0, 0.0],
        "If with a wider condition than its arms",
    );

    // App, a multi-argument elementwise builtin: `MAX(vals[d], matrix[e,d])` is
    // [[30,10,20],[30,20,30]], in-row ascending orders [1,2,0] and [1,0,2].
    // Both argument orders, since this arm has its own first-wins rule.
    for (name, eqn) in [
        (
            "mixs_max_a",
            "VECTOR SORT ORDER(MAX(vals[d], matrix[e,d]), 1)",
        ),
        (
            "mixs_max_b",
            "VECTOR SORT ORDER(MAX(matrix[e,d], vals[d]), 1)",
        ),
    ] {
        assert_close(
            &wide_out_of(name, eqn),
            &[1.0, 2.0, 0.0, 1.0, 0.0, 2.0],
            eqn,
        );
    }

    // App, a single-argument elementwise builtin wrapping the mix.
    // `ABS(vals[d] - matrix[e,d])` is [[29,8,17],[20,10,10]]; in-row ascending
    // orders [1,2,0] and [1,2,0] (stable tie between the two 10s).
    assert_close(
        &wide_out_of(
            "mixs_abs",
            "VECTOR SORT ORDER(ABS(vals[d] - matrix[e,d]), 1)",
        ),
        &[1.0, 2.0, 0.0, 1.0, 2.0, 0.0],
        "elementwise builtin over a mixed-shape difference",
    );
}

/// Two shapes neither of which CONTAINS the other broadcast into their CROSS
/// PRODUCT, and the axis order is the order the operand names its axes.
///
/// That order is not a guess: it is the same left-to-right union `ast::Expr2`
/// already assigns to the expression's bounds, so the temp has the shape the
/// type checker gave the operand. It IS observable, because the axis order is
/// the axis `VECTOR SORT ORDER` sorts along -- and the two operand orders are
/// rowed here for exactly that reason, so the property is pinned rather than
/// discovered later.
///
/// Values, over `rowv = [5, 50]` and `vals = [30, 10, 20]`:
///
/// * `rowv[e] + vals[d]` is `[e,d]` = `[[35,15,25],[80,60,70]]`; sorting within
///   each innermost (`d`) row ascending gives `[1,2,0]` twice.
/// * `vals[d] + rowv[e]` is `[d,e]` = `[[35,80],[15,60],[25,70]]`; the
///   innermost row is now `e`, two wide, and every one of them is already
///   ascending, so the sort order is `[0,1]` three times. `out[e,d]` projects
///   that temp by NAME, so element `(e_i, d_j)` reads `temp[d_j][e_i] = i`.
/// * `matrix[e,d] + matrixt[d,e]` CONTAIN each other, so containment leaves two
///   maximal candidates and the union takes the first: `[e,d]`, holding
///   `2 * matrix`, whose innermost rows are ascending.
#[test]
fn incomparable_operand_shapes_broadcast_into_their_cross_product() {
    for (name, eqn, expected) in [
        (
            "incomp_rc",
            "VECTOR SORT ORDER(rowv[e] + vals[d], 1)",
            [1.0, 2.0, 0.0, 1.0, 2.0, 0.0],
        ),
        (
            "incomp_cr",
            "VECTOR SORT ORDER(vals[d] + rowv[e], 1)",
            [0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        ),
        (
            "incomp_transpose",
            "VECTOR SORT ORDER(matrix[e,d] + matrixt[d,e], 1)",
            [0.0, 1.0, 2.0, 0.0, 1.0, 2.0],
        ),
    ] {
        let project = wide_fixture(name).array_aux("out[e,d]", eqn);
        project.assert_compiles_incremental();
        assert_close(&project.vm_result("out"), &expected, eqn);
    }
}

// ===========================================================================
// Module instances: a static view is addressed at the executing INSTANCE's
// slot base, not the root's.
// ===========================================================================

/// An array view inside a sub-model instance reads that instance's own slots.
///
/// A static view's `base_off` is resolved out of the FRAGMENT'S OWN model
/// layout (`symbolic::resolve_static_view`), so it is module-relative -- exactly
/// like the offset `Opcode::LoadVar` reads as `curr[module_off + off]`. The
/// executing instance's `module_off` has to be added at push time, and it was
/// not: `StaticArrayView::to_runtime_view` copied `base_off` verbatim, so every
/// array reduction inside a sub-model read the ROOT's slots. Over this fixture
/// both instances returned `[1, 2, 3, 4]` -- the sum of the first three global
/// slots, `time + dt + initial_time` -- instead of their own arrays.
///
/// This predates the array-valued `PREVIOUS`/`INIT` route (`ViewStorage::Curr`
/// has always gone through the same push), so the `out_curr` rows are the
/// control and the `out_prev`/`out_init` rows are what GH #995 added on top of
/// it. All three regions are `n_slots` copies of `curr` and share its slot
/// numbering, which is why one addend serves all three -- and why a fix that
/// covered only the two new ones would have left the oldest one broken.
#[test]
fn an_array_view_inside_a_module_instance_reads_that_instance() {
    use crate::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
    use crate::vm::Vm;

    let project = crate::test_common::two_instance_arrayed_submodel_project();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let compiled =
        compile_project_incremental(&db, sync.project, "main").expect("two-instance compile");
    let mut vm = Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("run");
    let results = crate::test_common::collect_results(&vm.into_results());

    for (name, expected) in crate::test_common::two_instance_arrayed_submodel_expected() {
        let actual = results
            .get(name)
            .unwrap_or_else(|| panic!("no series for {name}"));
        assert_close(actual, &expected, name);
    }
}

/// Every operand carrying a REPEATED-dimension view declines -- mixed with
/// another shape or as the operand's SOLE shape.
///
/// `[d,d]` names one dimension twice. The compile-time projection
/// (`compiler::project_var_index_to_temp`) pairs a temp's axes one to one and
/// would read such a temp back correctly; the RUNTIME broadcast does not:
/// `codegen::array_view_to_static_temp` keys a temp view's `DimId`s by name and
/// `dimensions::match_dimensions_two_pass` pairs a source axis with the FIRST
/// iteration axis of that id, so a `BeginIter` body evaluating
/// `matrix[d,d] * 2` into a `[d,d]` temp would read the diagonal. There is no
/// correct temp to give, so the pass gives none and codegen refuses loudly.
#[test]
fn a_repeated_dimension_operand_declines_rather_than_guessing_which_axis() {
    let square = |name: &str| {
        fixture(name).array_with_ranges(
            "square[d,d]",
            vec![
                ("1,1", "11"),
                ("1,2", "12"),
                ("1,3", "13"),
                ("2,1", "21"),
                ("2,2", "22"),
                ("2,3", "23"),
                ("3,1", "31"),
                ("3,2", "32"),
                ("3,3", "33"),
            ],
        )
    };
    for (name, eqn) in [
        // Mixed with a different shape, both operand orders -- the join has no
        // containment relation to work with.
        ("sqmix_lhs", "VECTOR SORT ORDER(square[d,d] + vals[d], 1)"),
        ("sqmix_rhs", "VECTOR SORT ORDER(vals[d] + square[d,d], 1)"),
        // The SOLE shape. A single view needs no join, so nothing about
        // containment refuses this one; it is refused because the shape itself
        // cannot be projected into a temp
        // (`compiler::view_repeats_a_dimension`, checked by the materializer
        // after the join rather than inside it). Without this row that check
        // can be deleted with every other row still green.
        ("sqmix_alone", "VECTOR SORT ORDER(square[d,d] * 2, 1)"),
        (
            "sqmix_two",
            "VECTOR SORT ORDER(square[d,d] + square[d,d], 1)",
        ),
    ] {
        assert_fails_attributed(square(name).array_aux("out[d,d]", eqn), eqn);
        assert_declines_because(
            square(name).array_aux("out[d,d]", eqn),
            "out",
            "Cannot push view for expression type",
        );
    }

    // The array-valued `PREVIOUS`/`INIT` route reaches the same shape by a
    // different door -- it pushes a view over a snapshot region rather than over
    // a temp -- and it is equally new here: this did not compile at the merge
    // base either. `codegen::snapshot_static_view` refuses it, with its own
    // message rather than the generic view rejection.
    for (name, eqn) in [
        ("sqprev", "VECTOR SORT ORDER(PREVIOUS(square[d,d]), 1)"),
        ("sqinit", "VECTOR SORT ORDER(INIT(square[d,d]), 1)"),
    ] {
        assert_fails_attributed(square(name).array_aux("out[d,d]", eqn), eqn);
        assert_declines_because(
            square(name).array_aux("out[d,d]", eqn),
            "out",
            "names one dimension twice",
        );
    }
}

/// The complement, and the boundary of the refusal above: reading a repeated
/// dimension DIRECTLY compiles, and reads the cell rather than the diagonal.
///
/// | equation | result |
/// |---|---|
/// | `out[d,d] = square[d,d]` | the matrix |
/// | `out[d,d] = VECTOR SORT ORDER(square[d,d], 1)` | `[0,1,2]` per row |
///
/// Both follow from one rule stated in the two places that pair axes by
/// name: `compiler::subscript::normalize_subscripts3` allocates the
/// active positions one to one across a reference's subscripts, and
/// `compiler::project_var_index_to_temp` pairs a temp's axes to the variable's
/// the same way. `db::analysis::expand_same_element`'s repeated-target residual
/// is the same root cause on the LTM side and is NOT fixed here
/// (`mapped_reference_semantics_tests::a_repeated_target_dimension_reads_each_axis_on_the_executed_path`).
///
/// **Blast radius, measured.** Vensim REJECTS the declaration: run in Vensim DSS
/// 2026-08-04, `vensim-probes/repeated_dimension.mdl` refuses to simulate with
/// "DimA appears more than once on LHS". No MDL-imported model can carry the
/// shape, so this residual is confined to hand-authored XMILE/JSON/protobuf.
/// It is not illegitimate, though -- the XMILE v1.0 spec exemplifies the
/// declaration ("A 2D non-apply-to-all array with dimensions X by X, where X is
/// size 2", verified in `docs/reference/xmile-v1.0.html`) -- so the shape must
/// keep working and this test pins OUR reading of it. What the spec
/// exemplifies is only the DECLARATION; what a REFERENCE like `sq[X,X]` means
/// is Simlin's to define, and
/// `vensim-probes/stella_repeated_dimension.stmx` asks Stella. Note the defect
/// is narrower than "repeated dimensions are broken": on that probe Simlin's
/// STORAGE is a correct 2-D array (`SUM(sq[X,*])` gives the true row sums
/// 36/66/96 and `SUM(sq[*,*])` gives 198, both measured); only the subscripted
/// reference collapses.
#[test]
fn a_repeated_dimension_read_directly_reads_each_axis() {
    let square = |name: &str| {
        fixture(name).array_with_ranges(
            "square[d,d]",
            vec![
                ("1,1", "11"),
                ("1,2", "12"),
                ("1,3", "13"),
                ("2,1", "21"),
                ("2,2", "22"),
                ("2,3", "23"),
                ("3,1", "31"),
                ("3,2", "32"),
                ("3,3", "33"),
            ],
        )
    };
    let copy = square("sqdirect_copy").array_aux("out[d,d]", "square[d,d]");
    copy.assert_compiles_incremental();
    assert_close(
        &copy.vm_result("out"),
        &[11.0, 12.0, 13.0, 21.0, 22.0, 23.0, 31.0, 32.0, 33.0],
        "a direct repeated-dimension read copies the matrix",
    );

    let sorted = square("sqdirect_sort").array_aux("out[d,d]", "VECTOR SORT ORDER(square[d,d], 1)");
    sorted.assert_compiles_incremental();
    // Each row of `square` ascends, so the sort order of every row is `[0,1,2]`
    // and the temp's second `d` axis has to be read as the second one for that
    // to come back out.
    assert_close(
        &sorted.vm_result("out"),
        &[0.0, 1.0, 2.0, 0.0, 1.0, 2.0, 0.0, 1.0, 2.0],
        "the per-row orders are [0,1,2] throughout",
    );

    // The direct read NESTED IN A REDUCER, which is the row that says WHERE the
    // refusal above may live. An array-producing builtin sizes its temp from
    // `find_expr_array_view` and SUBSTITUTES the variable's own view when that
    // is `None` -- no diagnostic, and at a different size. Putting the
    // repeated-dimension refusal inside `compiler::join_array_views` therefore
    // sized these temps at `out3`'s three slots while the builtin still wrote
    // nine elements, and the VM indexed past the temp: a panic, and under
    // `panic = abort` a dead host process. (The values are reductions over the
    // whole nine-cell square, so the one-to-one axis pairing above does not
    // move them.)
    for (name, eqn, expected) in [
        ("sqred_sum", "SUM(VECTOR SORT ORDER(square[d,d], 1))", 9.0),
        ("sqred_mean", "MEAN(RANK(square[d,d], 1))", 5.0),
        ("sqred_max", "MAX(VECTOR SORT ORDER(square[d,d], 1))", 2.0),
        ("sqred_size", "SIZE(VECTOR SORT ORDER(square[d,d], 1))", 9.0),
    ] {
        let p = square(name).array_aux("out3[d]", eqn);
        p.assert_compiles_incremental();
        assert_close(
            &p.vm_result("out3"),
            &[expected; 3],
            "a repeated-dimension read inside a reducer must keep its merge-base value",
        );
    }
}

/// The two-HOP twin: `main` -> `mid` (twice) -> `inner`.
///
/// A one-hop fixture cannot distinguish the rule the VM actually uses --
/// ACCUMULATE `module_off + decl.off` at each `EvalModule` -- from "apply the
/// last hop only" or "re-base from the root at each hop", because at one hop all
/// three agree. `mid` carries a scalar ahead of its module declaration so
/// `inner`'s block does not start at its parent's base, which makes the two
/// hops' offsets distinct non-zero numbers that have to sum.
#[test]
fn an_array_view_inside_a_nested_module_instance_reads_that_instance() {
    use crate::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
    use crate::vm::Vm;

    let project = crate::test_common::nested_instance_arrayed_submodel_project();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main").expect("nested compile");
    let mut vm = Vm::new(compiled).expect("vm");
    vm.run_to_end().expect("run");
    let results = crate::test_common::collect_results(&vm.into_results());

    for (name, expected) in crate::test_common::nested_instance_arrayed_submodel_expected() {
        let actual = results
            .get(name)
            .unwrap_or_else(|| panic!("no series for {name}"));
        assert_close(actual, &expected, name);
    }
}

// ===========================================================================
// Once per equation. An array-producing builtin whose operands do not depend
// on the active element is evaluated ONCE and every element reads the temp.
// ===========================================================================

/// The number of `AssignTemp`s a variable's non-initial lowering emits, through
/// the production per-variable lowering: one per materialized array value.
fn materialized_temps(project: &TestProject, var: &str) -> usize {
    project
        .flow_exprs(var)
        .iter()
        .filter(|expr| matches!(expr, crate::compiler::Expr::AssignTemp(..)))
        .count()
}

/// C-LEARN's `sorted target X[COP,Target] = VECTOR ELM MAP(Src[COP,t1],
/// Target Order[COP,Target])`: the source names the iterated axis beside a
/// FIXED column. A vector builtin's operand is read whole, so the promoted
/// source is the `[cop]`-shaped column slice whichever element is active, and
/// the equation materializes ONE map that every element reads back.
///
/// Both spellings of the source are rowed because they must be the same
/// operand: the explicit `src[*,t1]` and the promoted `src[cop,t1]` give one
/// temp and identical numbers. Values follow Vensim's rule
/// (`vm_vector_elm_map.rs`): `out[c,t] = src_flat[base(c,t1) + off[c,t]]`, the
/// base being the column cell's flat index.
#[test]
fn an_elm_map_over_a_fixed_column_slice_is_materialized_once_per_equation() {
    let project = |name: &str, source: &str| {
        TestProject::new(name)
            .with_sim_time(0.0, 1.0, 1.0)
            .named_dimension("cop", &["c1", "c2", "c3"])
            .named_dimension("tgt", &["t1", "t2", "t3"])
            .array_with_ranges(
                "src[cop,tgt]",
                vec![
                    ("c1,t1", "11"),
                    ("c1,t2", "12"),
                    ("c1,t3", "13"),
                    ("c2,t1", "21"),
                    ("c2,t2", "22"),
                    ("c2,t3", "23"),
                    ("c3,t1", "31"),
                    ("c3,t2", "32"),
                    ("c3,t3", "33"),
                ],
            )
            .array_with_ranges(
                "off[cop,tgt]",
                vec![
                    ("c1,t1", "0"),
                    ("c1,t2", "1"),
                    ("c1,t3", "2"),
                    ("c2,t1", "2"),
                    ("c2,t2", "0"),
                    ("c2,t3", "1"),
                    ("c3,t1", "1"),
                    ("c3,t2", "2"),
                    ("c3,t3", "0"),
                ],
            )
            .array_aux(
                "out[cop,tgt]",
                &format!("VECTOR ELM MAP({source}, off[cop,tgt])"),
            )
    };
    let expected = [11.0, 12.0, 13.0, 23.0, 21.0, 22.0, 32.0, 33.0, 31.0];
    for (name, source) in [
        ("elm_once_col", "src[cop,t1]"),
        ("elm_once_star", "src[*,t1]"),
    ] {
        let project = project(name, source);
        assert_eq!(
            materialized_temps(&project, "out"),
            1,
            "{source}: one VECTOR ELM MAP for the whole equation"
        );
        assert_close(&project.vm_result("out"), &expected, source);
    }
}

/// `VECTOR SORT ORDER(vals[d], 1)` inside an equation over `d` reads the whole
/// of `vals` whichever element is active, so it is sorted once; the
/// element-varying `vals[d]` beside it materializes nothing.
///
/// Values: the ascending order of `[30, 10, 20]` is `[1, 2, 0]`, summing to 3,
/// so `out = vals + 3`.
#[test]
fn a_sort_order_over_the_iterated_axis_is_materialized_once_per_equation() {
    let project =
        fixture("sort_once").array_aux("out[d]", "SUM(VECTOR SORT ORDER(vals[d], 1)) + vals[d]");
    assert_eq!(
        materialized_temps(&project, "out"),
        1,
        "one VECTOR SORT ORDER for the whole equation"
    );
    assert_close(
        &project.vm_result("out"),
        &[33.0, 13.0, 23.0],
        "vals + the sum of one sort order",
    );
}
