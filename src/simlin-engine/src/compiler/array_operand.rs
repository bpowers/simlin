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
//! Two limits are worth knowing before reading a "this shape does not compile"
//! report as a bug in this pass:
//!
//! * An operand only materializes if [`super::find_expr_array_view`] can
//!   derive a shape for it. That function's `App` arm is an exhaustive match
//!   naming exactly which builtins propagate an array shape; the ones that do
//!   not (the reducers, `VECTOR SELECT`, the `Lookup` family,
//!   `PREVIOUS`/`INIT`) are listed there with the reason.
//! * [`contains_element_collapsed_prev_or_init`] refuses an operand that
//!   *contains* an array-valued `PREVIOUS`/`INIT`, even when a sibling could
//!   supply the shape. That is GH #995's Phase C3 boundary, not an oversight.

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
        // the mapping ranges over, and the choice is this: the temp. Genuine
        // Vensim maps over the source VARIABLE's full row-major storage from
        // the base arg-1's element reference establishes, and
        // `vm_vector_elm_map.rs` implements that with a `source_is_full_array`
        // test -- a strict slice such as `matrix[E,*]` keeps a per-element
        // base and can read across rows, while a full contiguous source has
        // `base_i == 0`. A materialized operand is a fresh contiguous temp, so
        // it is full-array by construction: the mapping is confined to the
        // computed array. That is the only self-consistent reading (the temp
        // has no "rest of the variable" to run into) and it agrees with what
        // the practitioner would get by assigning the expression to a variable
        // of its own first -- the shape that already compiled. Pinned by
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
/// The "no derivable shape" case is what keeps a BARE `PREVIOUS(vals[D])` /
/// `INIT(vals[D])` operand failing loudly (GH #995's Phase C3): they lower to
/// an `App` that [`super::find_expr_array_view`] reports no view for, because a
/// `BeginIter` body cannot emit the per-element `LoadPrev`/`LoadInitial` those
/// need -- those opcodes address a static slot. Silently materializing them at
/// some guessed shape would trade a compile error for a wrong number. That is
/// NOT sufficient when the `PREVIOUS`/`INIT` is a SUBEXPRESSION of the operand
/// (a sibling supplies the shape), which is why
/// [`contains_element_collapsed_prev_or_init`] is checked first -- see its doc
/// for the wrong number it prevents.
fn materialize_view_operand(
    operand: Box<Expr>,
    next_temp_id: &mut u32,
    hoisted: &mut Vec<Expr>,
) -> Box<Expr> {
    if is_view(&operand) {
        return operand;
    }
    if contains_element_collapsed_prev_or_init(&operand) {
        return operand;
    }
    let Some(source_view) = super::find_expr_array_view(&operand) else {
        return operand;
    };
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

/// True when `expr` contains a `PREVIOUS`/`INIT` whose argument is not a plain
/// scalar variable reference.
///
/// A bare `PREVIOUS(vals[D])` operand declines on its own, because
/// [`super::find_expr_array_view`] reports no view for it. But that check is
/// not enough one level up: an operand's shape is derived from whichever leaf
/// HAS one (`Op2` takes `lhs.or_else(rhs)`), so
/// `VECTOR SORT ORDER(PREVIOUS(vals[D]) + bump[D], 1)` would take its shape
/// from the sibling `bump[D]` and materialize. It must not. `PREVIOUS`'s
/// argument is lowered element-collapsed -- a `StaticSubscript` with no
/// dimensions carrying the element's flat offset, `previous(vals@0 + view(dims:
/// [], offset: 1))` -- so the temp would compute ONE element's previous value
/// broadcast across the whole array. That is a plausible array of wrong
/// numbers where the fragment previously failed loudly, which is strictly
/// worse than not compiling.
///
/// A `PREVIOUS`/`INIT` of a genuinely scalar variable lowers to an
/// `Expr::Var`, carries no per-element identity, and broadcasts correctly, so
/// it stays materializable: `VECTOR SORT ORDER(vals[D] + PREVIOUS(s), 1)`
/// compiles and is right.
///
/// Declining a collapsed argument costs nothing beyond the status quo: an
/// operand containing an `App` at all was never one of `is_view`'s four
/// shapes, so it did not compile before this pass existed either. Phase C3
/// (GH #995) is where the array-valued forms get a snapshot-buffer view; this
/// predicate is the boundary it moves.
fn contains_element_collapsed_prev_or_init(expr: &Expr) -> bool {
    match expr {
        Expr::App(builtin, _) => {
            let collapsed_arg = match builtin {
                BuiltinFn::Previous(arg, _) | BuiltinFn::Init(arg) => {
                    !matches!(arg.as_ref(), Expr::Var(_, _))
                }
                _ => false,
            };
            if collapsed_arg {
                return true;
            }
            let mut found = false;
            builtin.for_each_expr_ref(|arg| {
                found = found || contains_element_collapsed_prev_or_init(arg);
            });
            found
        }
        Expr::Op1(_, inner, _) => contains_element_collapsed_prev_or_init(inner),
        Expr::Op2(_, lhs, rhs, _) => {
            contains_element_collapsed_prev_or_init(lhs)
                || contains_element_collapsed_prev_or_init(rhs)
        }
        Expr::If(cond, then_expr, else_expr, _) => {
            contains_element_collapsed_prev_or_init(cond)
                || contains_element_collapsed_prev_or_init(then_expr)
                || contains_element_collapsed_prev_or_init(else_expr)
        }
        Expr::Subscript(_, indices, _, _) => indices.iter().any(|idx| match idx {
            SubscriptIndex::Single(e) => contains_element_collapsed_prev_or_init(e),
            SubscriptIndex::Range(start, end) => {
                contains_element_collapsed_prev_or_init(start)
                    || contains_element_collapsed_prev_or_init(end)
            }
        }),
        Expr::EvalModule(_, _, _, args) => args.iter().any(contains_element_collapsed_prev_or_init),
        Expr::AssignCurr(_, rhs) | Expr::AssignNext(_, rhs) | Expr::AssignTemp(_, rhs, _) => {
            contains_element_collapsed_prev_or_init(rhs)
        }
        Expr::Const(_, _)
        | Expr::Var(_, _)
        | Expr::StaticSubscript(_, _, _)
        | Expr::TempArray(_, _, _)
        | Expr::TempArrayElement(_, _, _, _)
        | Expr::Dt(_)
        | Expr::ModuleInput(_, _) => false,
    }
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
