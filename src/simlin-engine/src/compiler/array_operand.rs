// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Materialization of computed array operands (GH #995).
//!
//! Codegen consumes an array-valued operand as a **view over storage**
//! ([`super::codegen::Compiler::walk_expr_as_view`]): a `StaticSubscript`, a
//! `TempArray`, a whole `Var`, or a dynamic `Subscript`, and nothing else. A
//! *computed* array -- `vals[D] * 2`, `NOT ...`, an `IF` selecting between two
//! arrays, an elementwise `ABS(...)`, a nested array-producing builtin -- is
//! none of those, so it has to be evaluated into a temp of its own before the
//! builtin that reads it. Codegen already knows how to do that: an
//! `AssignTemp` whose body is not one of the array-producing opcodes lowers to
//! a `BeginIter` loop that evaluates the body element by element.
//!
//! [`super::context::Context::lower`]'s Pass 1
//! ([`crate::ast::Pass1Context`]) materializes the operands it can see,
//! but it works on `Expr3`, *before* subscripts are resolved, so two shapes get
//! past it:
//!
//! * an operand still carrying an unresolved apply-to-all dimension reference.
//!   `vals[D]` inside a vector builtin only means "the whole array" after
//!   [`super::context::Context::with_vector_builtin_wildcards`] promotes its
//!   `ActiveDimRef` to a `Wildcard`, which happens during lowering, after Pass
//!   1; Pass 1 sees an unresolved reference and defers to pass 2.
//! * an operand the *type checker* bounded as a scalar for the same reason:
//!   `vals[D] * 2` carries `ArrayBounds: None`, so Pass 1's
//!   `needs_decomposition` declines it before the deferral even matters.
//!
//! This pass is the backstop, and it runs on the fully lowered fragment, where
//! the promotion has happened and every view is concrete.
//!
//! # Why it is safe
//!
//! It rewrites **only** operands codegen would have rejected: [`is_view`] is
//! the negation of `walk_expr_as_view`'s accepting arms, so a fragment that
//! compiles today passes through untouched, temp count included.
//!
//! Where it does fire it costs one temp per materialized operand, which on the
//! per-element hoisting path (one temp per array ELEMENT already) doubles temp
//! consumption. That runs into the `u8` `TempId` namespace above ~128 elements
//! -- and a materialized operand is read through a static VIEW, the one place a
//! temp id is not narrowed to `u8`. `symbolic::resolve_static_view` rejects
//! that combination loudly rather than letting the two narrowings disagree;
//! #583 is the real fix. Measured max temps per fragment across the checked-in
//! corpus and C-LEARN: 21, unchanged by this pass.
//!
//! # What still declines
//!
//! Four limits are worth knowing before reading a "this shape does not
//! compile" report as a bug in this pass:
//!
//! * An operand only materializes if [`super::find_expr_array_view`] can
//!   derive a shape for it. That function's `App` arm is an exhaustive match
//!   naming exactly which builtins propagate an array shape; the ones that do
//!   not (the reducers, `VECTOR SELECT`, the `Lookup` family) are listed there
//!   with the reason.
//! * That shape is the JOIN of every array in the operand -- the view they all
//!   broadcast into -- so an operand mixing INCOMPARABLE shapes (`row[e]` and
//!   `col[d]`, neither containing the other) has none, and declines. The union
//!   `[e,d]` would compile, but nothing in the operand says whether it is
//!   `[e,d]` or `[d,e]`, and the temp's axis order is the axis
//!   `VECTOR SORT ORDER` sorts along. Declining leaves the loud codegen
//!   rejection; guessing would leave a plausible array of wrong numbers.
//! * An operand carrying a REPEATED-dimension view (`matrix[d,d]`) declines --
//!   mixed with another shape, and as the operand's SOLE shape. `[d,d]` can say
//!   "contains `d` at size 3" but not WHICH `d`, and every layer that projects
//!   between an array and a temp matches by name and takes the first hit:
//!   `super::project_var_index_to_temp` gives both axes the same coordinate, so
//!   `out[i,j]` would read `temp[i,i]`, and `codegen::array_view_to_static_temp`
//!   keys `DimId`s the same way. There is no shape to give.
//!
//!   The refusal lives HERE and in `codegen::snapshot_static_view` -- the two
//!   positions that can be loud about it -- and deliberately not inside
//!   `super::join_array_views`, even though that reads as the tidier home for
//!   it. `super::find_expr_array_view` has four consumers and the other three
//!   substitute the VARIABLE's own view for a `None`, silently and at a
//!   possibly different SIZE: refusing in the join sized the temp of
//!   `out[d] = SUM(VECTOR SORT ORDER(matrix[d,d], 1))` at `out`'s three slots
//!   while the sort order still wrote nine, and the VM indexed past the temp.
//!   That equation returns numbers at the merge base, so the tidier home cost a
//!   process abort on a shape that worked.
//!
//!   This costs nothing that worked: measured at the MERGE BASE `ccf7ed34`,
//!   `VECTOR SORT ORDER(matrix[d,d] * 2, 1)` does not compile, and neither does
//!   the `PREVIOUS`/`INIT` spelling `codegen::snapshot_static_view` refuses on
//!   the same grounds. Both became compilable on this branch, and both compiled
//!   to first-axis-wins garbage until this refusal. Reading a repeated dimension
//!   DIRECTLY is a different matter and is untouched: `out[d,d] = matrix[d,d]`
//!   and `VECTOR SORT ORDER(matrix[d,d], 1)` compile at the merge base, to those
//!   same wrong numbers, and remain exactly as they were -- a disclosed residual
//!   whose blast radius is now MEASURED: Vensim REJECTS the declaration -- run in Vensim DSS 2026-08-04, `vensim-probes/repeated_dimension.mdl` refuses to simulate with "DimA appears more than once on LHS" -- so no MDL-imported model can contain this shape and the residual is confined to hand-authored XMILE/JSON/protobuf. It is NOT illegitimate, though: the XMILE v1.0 spec exemplifies the declaration (`docs/reference/xmile-v1.0.html`, "A 2D non-apply-to-all array with dimensions X by X, where X is size 2", verified in-repo), so a conformant file may carry it and Simlin must keep reading it. The spec exemplifies only the DECLARATION, with per-element equations; it says nothing about what a REFERENCE such as `sq[X,X]` means, which is the part that is wrong here. Pinned by
//!   `array_operand_materialization_tests::a_repeated_dimension_read_directly_is_a_pre_existing_residual`,
//!   whose fix belongs in the projection rather than here.

use crate::ast::ArrayView;
use crate::compiler::expr::{BuiltinFn, Expr, SubscriptIndex};

/// Rewrite `exprs` so every array operand codegen reads as a view actually is
/// one, splicing an `AssignTemp` in front of the expression that needs it.
///
/// Temp ids continue past the highest one the fragment already uses, so the
/// new temps cannot collide with the per-element ids the apply-to-all hoister
/// assigns (which restart at 0 for each `lower()` call and are remapped there).
pub(super) fn materialize_computed_array_operands(exprs: Vec<Expr>) -> Vec<Expr> {
    let mut next_temp_id = super::next_available_temp_id(&exprs);
    let mut out: Vec<Expr> = Vec::with_capacity(exprs.len());
    for expr in exprs {
        // Hoisted assignments are emitted immediately before the expression
        // that reads them, and in the order they were allocated, so a nested
        // materialization's temp is always written before the outer one that
        // consumes it.
        let mut hoisted = Vec::new();
        let expr = rewrite(expr, &mut next_temp_id, &mut hoisted);
        out.extend(hoisted);
        out.push(expr);
    }
    out
}

fn rewrite(expr: Expr, next_temp_id: &mut u32, hoisted: &mut Vec<Expr>) -> Expr {
    match expr {
        Expr::App(builtin, loc) => {
            // Bottom-up: an operand that is itself a builtin call is rewritten
            // (and, if it needs it, materialized) before this level looks at
            // it, so a nested array-producing builtin arrives here as a
            // `TempArray` rather than as an un-viewable `App`.
            let builtin = builtin.map(|arg| rewrite(arg, next_temp_id, hoisted));
            Expr::App(
                materialize_view_operands(builtin, next_temp_id, hoisted),
                loc,
            )
        }
        Expr::Op1(op, inner, loc) => {
            Expr::Op1(op, Box::new(rewrite(*inner, next_temp_id, hoisted)), loc)
        }
        Expr::Op2(op, lhs, rhs, loc) => Expr::Op2(
            op,
            Box::new(rewrite(*lhs, next_temp_id, hoisted)),
            Box::new(rewrite(*rhs, next_temp_id, hoisted)),
            loc,
        ),
        Expr::If(cond, then_expr, else_expr, loc) => Expr::If(
            Box::new(rewrite(*cond, next_temp_id, hoisted)),
            Box::new(rewrite(*then_expr, next_temp_id, hoisted)),
            Box::new(rewrite(*else_expr, next_temp_id, hoisted)),
            loc,
        ),
        Expr::Subscript(base, indices, bounds, loc) => {
            let indices = indices
                .into_iter()
                .map(|idx| match idx {
                    SubscriptIndex::Single(e) => {
                        SubscriptIndex::Single(rewrite(e, next_temp_id, hoisted))
                    }
                    SubscriptIndex::Range(start, end) => SubscriptIndex::Range(
                        rewrite(start, next_temp_id, hoisted),
                        rewrite(end, next_temp_id, hoisted),
                    ),
                })
                .collect();
            Expr::Subscript(base, indices, bounds, loc)
        }
        Expr::EvalModule(ident, model_name, input_set, args) => Expr::EvalModule(
            ident,
            model_name,
            input_set,
            args.into_iter()
                .map(|arg| rewrite(arg, next_temp_id, hoisted))
                .collect(),
        ),
        Expr::AssignCurr(dst, rhs) => {
            Expr::AssignCurr(dst, Box::new(rewrite(*rhs, next_temp_id, hoisted)))
        }
        Expr::AssignNext(dst, rhs) => {
            Expr::AssignNext(dst, Box::new(rewrite(*rhs, next_temp_id, hoisted)))
        }
        Expr::AssignTemp(id, rhs, view) => {
            Expr::AssignTemp(id, Box::new(rewrite(*rhs, next_temp_id, hoisted)), view)
        }
        leaf @ (Expr::Const(_, _)
        | Expr::Var(_, _)
        | Expr::StaticSubscript(_, _, _)
        | Expr::TempArray(_, _, _)
        | Expr::TempArrayElement(_, _, _, _)
        | Expr::Dt(_)
        | Expr::ModuleInput(_, _)) => leaf,
    }
}

/// The single enumeration of view-requiring operand positions, derived from
/// codegen's `walk_expr_as_view` call sites. Written as an exhaustive match
/// with no `_` arm so a new `BuiltinFn` variant is a compile error here rather
/// than a silently unconsidered position.
fn materialize_view_operands(
    builtin: BuiltinFn,
    next_temp_id: &mut u32,
    hoisted: &mut Vec<Expr>,
) -> BuiltinFn {
    use crate::builtins::BuiltinFn::*;
    let mut mat = |arg: Box<Expr>| materialize_view_operand(arg, next_temp_id, hoisted);
    match builtin {
        // `emit_array_reduce`: pushes the argument as a view unconditionally.
        Sum(a) => Sum(mat(a)),
        Size(a) => Size(mat(a)),
        Stddev(a) => Stddev(mat(a)),
        // One-argument MIN/MAX are the array reductions; the two-argument
        // forms are scalar and take no view.
        Min(a, b) => match b {
            None => Min(mat(a), None),
            Some(b) => Min(a, Some(b)),
        },
        Max(a, b) => match b {
            None => Max(mat(a), None),
            Some(b) => Max(a, Some(b)),
        },

        // The scalar-reducing selector and the five array-producing opcodes.
        VectorSelect(sel, values, max_val, action, err) => {
            VectorSelect(mat(sel), mat(values), max_val, action, err)
        }
        // Materializing an ELM MAP *source* deliberately changes which storage
        // the mapping ranges over, and the choice is this: the temp.
        //
        // The rule for a VARIABLE source is DOCUMENTED and ground-truthed. The
        // Vensim reference page for VECTOR ELM MAP (retrieved 2026-08-02) says
        // the function "returns the value of the variable that is offset from
        // vec by the specified amount", and that an offset "outside the range of
        // the variable" yields `:NA:`; its multi-subscript example spells the
        // offset as a flat index over the whole variable
        // (`(sub-1)*ELMCOUNT(tub)*ELMCOUNT(gub) + ...`). Real Vensim output
        // agrees: in `test/sdeverywhere/models/vector/`,
        // `f[DimA,DimB] = VECTOR ELM MAP(d[DimA,B1], a[DimA])` prints
        // `1,1,5,5,6,6`, and `f[A2,B1] = 5 = d[A2,B2]` -- the mapping read past
        // its own `B1` slice into the next row. `vm_vector_elm_map.rs`
        // implements exactly that with a `source_is_full_array` test: a strict
        // slice keeps a per-element base and can read across rows, a full
        // contiguous source has `base_i == 0`.
        //
        // A COMPUTED source is a Simlin EXTENSION, and it is now settled that
        // it is one. Vensim rejects the shape outright -- run in Vensim DSS on
        // 2026-08-04, `vensim-probes/elm_map_computed_source.mdl` refuses to
        // simulate with "Argument 1 to function VECTOR ELM MAP must be a normal
        // variable". So there is no Vensim behaviour to match here, and the
        // question is not "which rule does Vensim use" but "what shall this mean
        // in Simlin".
        //
        // It means the HELPER-EQUIVALENT thing: an inline expression behaves
        // exactly as the same values pre-assigned to a named variable, which is
        // the spelling that IS legal Vensim. A materialized operand is a fresh
        // contiguous temp and so is full-array by construction, which confines
        // the mapping to the computed array -- exactly what
        // `VECTOR ELM MAP(helper[A1], offs)` does when `helper` holds those
        // values. That definition is deliberate and no longer provisional; the
        // temp has no "rest of the variable" to run into, so nothing else is
        // even expressible. Pinned by
        // `array_operand_materialization_tests::materializing_an_elm_map_source_confines_the_mapping_to_the_temp`.
        VectorElmMap(source, offsets) => VectorElmMap(mat(source), mat(offsets)),
        VectorSortOrder(array, direction) => VectorSortOrder(mat(array), direction),
        Rank(array, direction) => Rank(mat(array), direction),
        // ALLOCATE AVAILABLE's priority-profile argument is deliberately NOT
        // materialized. Its view is rewritten during lowering by
        // `context::Context::expand_pp_view_for_allocate`, which re-expands a
        // collapsed reference such as `pp[D,1]` back to the variable's full
        // requester x XPriority array because the allocator always reads all
        // four profile columns. That helper only understands a direct variable
        // reference, so a computed profile array has no defined shape here:
        // materializing `pp[D,1] + adj[D,1]` would silently hand the VM a
        // one-column-per-requester temp. Leaving it alone keeps the loud
        // codegen rejection instead.
        AllocateAvailable(requests, profiles, available) => {
            AllocateAvailable(mat(requests), profiles, available)
        }
        AllocateByPriority(requests, priorities, size, width, supply) => {
            AllocateByPriority(mat(requests), mat(priorities), size, width, supply)
        }

        // An arrayed graphical-function apply reads its table as a view, but
        // the table must name a whole *variable*: codegen resolves it to a
        // `base_gf` by ident (`arrayed_lookup_table_info`), and a temp has no
        // graphical functions attached to it.
        Lookup(_, _, _) | LookupForward(_, _, _) | LookupBackward(_, _, _) => builtin,
        // MEAN is the one reduce that is variadic. Only its single-argument
        // form is an array reduction and only that form reaches
        // `emit_array_reduce`; the multi-argument form averages scalars and
        // has no view position at all. Codegen's `Mean` arm matches the four
        // view shapes and emits a plain scalar `walk_expr` otherwise -- which
        // is right for `MEAN(a * b)` over two scalars, and is why a
        // scalar-shaped argument must keep passing through untouched. That
        // fallback is NOT a licence to leave an array-shaped argument alone:
        // `MEAN(matrix[E,*] * 2)` reaches the fallback, emits a scalar walk
        // over an array expression, and fails to compile. `mat` declines
        // anything with no derivable array view, so the scalar form is
        // unaffected and the array form now agrees with every other reducer.
        Mean(args) => {
            if args.len() == 1 {
                Mean(args.into_iter().map(|a| *mat(Box::new(a))).collect())
            } else {
                Mean(args)
            }
        }

        // No view-requiring operand.
        other @ (Abs(_)
        | Arccos(_)
        | Arcsin(_)
        | Arctan(_)
        | Cos(_)
        | Exp(_)
        | Round(_)
        | Inf
        | Int(_)
        | IsModuleInput(_, _)
        | Ln(_)
        | Log10(_)
        | Pi
        | Pulse(_, _, _)
        | Quantum(_, _)
        | Ramp(_, _, _)
        | SafeDiv(_, _, _)
        | Sign(_)
        | Sshape(_, _, _)
        | Sin(_)
        | Sqrt(_)
        | Step(_, _)
        | Tan(_)
        | Time
        | TimeStep
        | StartTime
        | FinalTime
        | Previous(_, _)
        | Init(_)) => other,
    }
}

/// Move `operand` into a temp of its own and return the `TempArray` reference
/// that replaces it, or return it unchanged when it is already a view or when
/// no array shape can be derived for it.
///
/// A bare array-valued `PREVIOUS`/`INIT` is left alone for the same reason a
/// `StaticSubscript` is: it already IS a view, over a snapshot buffer rather
/// than over `curr` (GH #995, [`is_snapshot_view`]). Materializing it would
/// spend a temp to copy an array that codegen can address directly.
fn materialize_view_operand(
    operand: Box<Expr>,
    next_temp_id: &mut u32,
    hoisted: &mut Vec<Expr>,
) -> Box<Expr> {
    if is_view(&operand) {
        return operand;
    }
    if is_snapshot_view(&operand) {
        return operand;
    }
    // The operand's shape is the JOIN of its subexpressions' shapes, which is
    // what makes `small[d] + wide[e,d]` and `wide[e,d] + small[d]` the same
    // array (`super::find_expr_array_view`). `None` covers both "no shape" and
    // "two shapes neither of which contains the other"; declining leaves the
    // operand for codegen to reject, exactly as the two deliberately
    // unmaterialized positions above do, rather than guessing an axis order.
    let Some(source_view) = super::find_expr_array_view(&operand) else {
        return operand;
    };
    // A repeated dimension name is refused as a TEMP's shape even when it is the
    // operand's sole shape (`super::view_repeats_a_dimension`). The refusal lives
    // here rather than inside the join because `find_expr_array_view` has four
    // consumers and only this one is loud: the three hoisters in `super` fall
    // back to the VARIABLE's own view, so a `None` there silently reshapes a
    // temp instead of declining -- measured, `out[d] = SUM(VECTOR SORT
    // ORDER(matrix[d,d], 1))` sized a 9-element sort order's temp at 3 and the
    // VM indexed past it. Refusing at the one site that produces a diagnostic
    // keeps the direct spellings byte-identical to the merge base, which is all
    // this refusal ever claimed.
    if super::view_repeats_a_dimension(&source_view) {
        return operand;
    }
    if source_view.dims.is_empty() {
        return operand;
    }

    // The temp is fresh compact storage; only the SHAPE of the producing
    // expression carries over. (Codegen normalizes a temp's view to compact
    // row-major strides anyway -- `array_view_to_static_temp` -- so passing a
    // sliced source view would merely be misleading, not wrong.)
    let view = if source_view.dim_names.len() == source_view.dims.len() {
        ArrayView::contiguous_with_names(source_view.dims, source_view.dim_names)
    } else {
        ArrayView::contiguous(source_view.dims)
    };

    let loc = operand.get_loc();
    let temp_id = *next_temp_id;
    *next_temp_id += 1;
    hoisted.push(Expr::AssignTemp(temp_id, operand, view.clone()));
    Box::new(Expr::TempArray(temp_id, view, loc))
}

/// True when `expr` is an array-valued `PREVIOUS`/`INIT` -- a fifth view shape
/// `codegen::walk_expr_as_view` accepts, over one of the VM's snapshot buffers
/// rather than over `curr` (GH #995).
///
/// Kept separate from [`is_view`] because the two are decided differently:
/// `is_view` is a pure shape test on the `Expr` variant, while this one depends
/// on the ARGUMENT's shape -- `PREVIOUS(vals[D])` is a view and
/// `PREVIOUS(matrix[E,1])` is a scalar. Both are decided by
/// [`super::snapshot_view_arg`], shared with codegen, so the pass and the
/// emitter cannot disagree about which calls take the array route. An argument
/// codegen cannot express as a snapshot view is a loud rejection THERE, and
/// declining to materialize here is what keeps it loud: a temp built around a
/// `PREVIOUS` its `BeginIter` body cannot emit would be a wrong number in place
/// of a diagnostic.
fn is_snapshot_view(expr: &Expr) -> bool {
    matches!(expr, Expr::App(builtin, _) if super::snapshot_view_arg(builtin).is_some())
}

/// The four expression shapes `codegen::walk_expr_as_view` accepts. Anything
/// else is a codegen error, which is exactly the set this pass rewrites.
fn is_view(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::StaticSubscript(_, _, _)
            | Expr::TempArray(_, _, _)
            | Expr::Var(_, _)
            | Expr::Subscript(_, _, _, _)
    )
}
