// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! End-to-end pins for the per-builtin facts `BuiltinFn::signature()` states
//! (`builtins.rs`). The unit-level agreement between every variant and its
//! signature is pinned beside the table; these rows pin the places where the
//! table's statement of a fact decides how an equation compiles, on the shapes
//! where a single per-site rule is load-bearing:
//!
//! - n-ary `MEAN` is the scalar mean of its arguments (`ArgKind::Scalar` at
//!   every position), not an array reduction, at every stage;
//! - `RANK`'s array argument is a whole-array operand like
//!   `VECTOR SORT ORDER`'s, so the two accept the same operand shapes.

use crate::test_common::TestProject;

/// `MEAN(a, b)` over two array-valued operands is the scalar mean codegen
/// emits for the n-ary form, and the operands are lowered as scalars. An
/// n-ary `MEAN` operand with an array shape must therefore be REFUSED with a
/// diagnostic, never handed to the scalar-mean emitter as a temp it cannot
/// read (which is an `unwrap` on an empty stack, a process abort under
/// `panic = abort`).
#[test]
fn n_ary_mean_over_array_operands_is_refused_not_aborted() {
    let project = TestProject::new("mean_n_ary_arrays")
        .indexed_dimension("D", 3)
        .array_const("a[D]", 5.0)
        .array_const("b[D]", 7.0)
        .scalar_aux("x", "MEAN(a[*] + 1, b[*] + 1)");

    let result = project.compile_incremental();
    assert!(
        result.is_err(),
        "an n-ary MEAN of array operands must be refused, got {result:?}"
    );
}

/// n-ary `MEAN` with `@N`-subscripted scalar operands lowers its arguments
/// exactly as n-ary `MAX` does -- as scalars -- and the two agree on the value.
///
/// `MAX(x, y)` is XMILE v1.0's "larger of two numbers" (section 3.5). n-ary
/// `MEAN` is a Stella extension -- XMILE v1.0 itself defines only the
/// one-argument array mean (section 3.7.1.3) -- with in-repo ground truth:
/// `test/test-models/tests/builtin_mean/builtin_mean.stmx` (Stella Professional
/// 1.9.4) evaluates `MEAN(1, 2, ..., 9, TIME)` and its Stella-produced
/// `output.tab` gives 4.6 at `TIME = 1`; `test/modules2/modules2.xmile` (Stella
/// Architect 2.0) uses a two-argument `MEAN`. XMILE defines `@N` relative to
/// "the dimension in the entity that contains the equation" (section 3.7.1),
/// so `@N` in a scalar equation is the engine's extension
/// (`Context::lower_index_expr3` resolves it to element `N`). What this pins is
/// that the engine applies one rule to both builtins.
#[test]
fn n_ary_mean_lowers_its_operands_as_scalars_like_n_ary_max() {
    let project = TestProject::new("mean_n_ary_dim_positions")
        .indexed_dimension("D", 3)
        .array_with_ranges("a[D]", vec![("1", "10"), ("2", "20"), ("3", "30")])
        .array_with_ranges("b[D]", vec![("1", "1"), ("2", "2"), ("3", "3")])
        .scalar_aux("mean_of", "MEAN(a[@1], b[@2])")
        .scalar_aux("max_of", "MAX(a[@1], b[@2])");

    project.assert_compiles_incremental();
    project.assert_vm_scalar_result("mean_of", 6.0);
    project.assert_vm_scalar_result("max_of", 10.0);
}

/// `RANK` consumes its first argument as a whole array, exactly as
/// `VECTOR SORT ORDER` does, so an operand that unions two named dimensions
/// (`a[*] + b[*]` over `a[X]` and `b[Y]`, a 2 x 3 cross product the
/// materializer evaluates into a temp) is accepted by both when the builtin
/// ITSELF is what
/// opens the `Expr2` dimension-union gate: the apply-to-all
/// `out[X,Y] = RANK(a[*] + b[*], 1)`. (Under an outer reducer --
/// `SUM(RANK(...))` -- the reducer opens the gate, so that spelling tells the
/// two builtins apart only by value, not by admission.) What this pins is
/// ADMISSION parity: the gate reads the table's array positions, so `RANK` is
/// admitted exactly where `VECTOR SORT ORDER` is.
///
/// The values are pinned so a change in either opcode is noticed, not as a
/// claim that both are right. `a + b` is `[[31, 11, 21], [32, 12, 22]]`.
/// `VECTOR SORT ORDER` orders within each innermost row (`[1, 2, 0]` twice),
/// the rule `vm_vector_sort_order.rs` implements from the Vensim DSS 7.3.4
/// ground truth in `test/test-models/tests/vector_order/output.tab`.
/// `[5, 1, 3, 6, 2, 4]` is Simlin's CURRENT `Opcode::Rank` (`vm.rs`): the
/// 1-based rank of every element over the whole view. That diverges from
/// Vensim's `VECTOR RANK`, which ranks within the innermost row as well: in
/// the same `output.tab`, `revenue RANK2A[company,Region] =
/// VECTOR RANK(revenue2A[company,Region], 1)` gives each `company` row the
/// ranks 1..3 over its three `Region` values (at `Time = 0`, `company1`:
/// `1, 2, 3`; `company2`: `2, 1, 3`), never 1..15 over the 5 x 3 operand.
/// Per-row ranking of this test's operand would give `[3, 1, 2, 3, 1, 2]`.
/// The divergence is tracked in GH #1026; the VM and wasm backends are out of
/// this test's scope.
#[test]
fn rank_accepts_the_cross_dimension_operand_vector_sort_order_accepts() {
    let fixture = |name: &str| {
        TestProject::new(name)
            .with_sim_time(0.0, 1.0, 1.0)
            .named_dimension("X", &["x1", "x2"])
            .named_dimension("Y", &["y1", "y2", "y3"])
            .array_with_ranges("a[X]", vec![("x1", "1"), ("x2", "2")])
            .array_with_ranges("b[Y]", vec![("y1", "30"), ("y2", "10"), ("y3", "20")])
    };

    let sorted =
        fixture("rank_parity_vso").array_aux("out[X,Y]", "VECTOR SORT ORDER(a[*] + b[*], 1)");
    sorted.assert_compiles_incremental();
    sorted.assert_vm_result("out", &[1.0, 2.0, 0.0, 1.0, 2.0, 0.0]);

    let ranked = fixture("rank_parity_rank").array_aux("out[X,Y]", "RANK(a[*] + b[*], 1)");
    ranked.assert_compiles_incremental();
    ranked.assert_vm_result("out", &[5.0, 1.0, 3.0, 6.0, 2.0, 4.0]);
}

/// The apply-to-all spelling of the same operand, `a[X] + b[Y]` inside an
/// equation over `[X, Y]`, is the SAME operand and gives the same array: a
/// vector builtin promotes an apply-to-all element reference back to the whole
/// axis, so both spellings arrive at `[X]` beside `[Y]`, and two shapes neither
/// of which contains the other broadcast into their cross product.
///
/// The two spellings used to disagree -- the wildcard one compiled and the
/// apply-to-all one was refused -- because two passes decided the operand's
/// shape and only one of them knew the union. One pass decides it now, so this
/// row is what says the spelling does not change the answer.
#[test]
fn rank_and_vector_sort_order_read_the_same_operand_in_both_spellings() {
    let fixture = |name: &str| {
        TestProject::new(name)
            .with_sim_time(0.0, 1.0, 1.0)
            .named_dimension("X", &["x1", "x2"])
            .named_dimension("Y", &["y1", "y2", "y3"])
            .array_with_ranges("a[X]", vec![("x1", "1"), ("x2", "2")])
            .array_with_ranges("b[Y]", vec![("y1", "30"), ("y2", "10"), ("y3", "20")])
    };

    let sorted = fixture("rank_a2a_vso").array_aux("out[X,Y]", "VECTOR SORT ORDER(a[X] + b[Y], 1)");
    sorted.assert_compiles_incremental();
    sorted.assert_vm_result("out", &[1.0, 2.0, 0.0, 1.0, 2.0, 0.0]);

    let ranked = fixture("rank_a2a_rank").array_aux("out[X,Y]", "RANK(a[X] + b[Y], 1)");
    ranked.assert_compiles_incremental();
    ranked.assert_vm_result("out", &[5.0, 1.0, 3.0, 6.0, 2.0, 4.0]);
}
