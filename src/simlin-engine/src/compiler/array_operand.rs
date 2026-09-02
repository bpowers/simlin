// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The one pass that materializes array values into temps.
//!
//! Codegen consumes an array-valued operand as a **view over storage**
//! ([`super::codegen::Compiler::walk_expr_as_view`]): a `StaticSubscript`, a
//! `TempArray`, a whole `Var`, a dynamic `Subscript`, or an array-valued
//! `PREVIOUS`/`INIT` over a snapshot buffer, and nothing else. It also has no
//! opcode that evaluates an array-producing builtin (`VECTOR ELM MAP`,
//! `VECTOR SORT ORDER`, `RANK`, `ALLOCATE AVAILABLE`, `ALLOCATE BY PRIORITY`)
//! anywhere but as an `AssignTemp` body, nor one that applies a per-element
//! arrayed graphical function anywhere but as a `LookupArray` writing a temp.
//! Everything else that carries an array shape -- `vals[D] * 2`, `NOT ...`, an
//! `IF` selecting between two arrays, an elementwise `ABS(...)` -- has to be
//! evaluated into a temp of its own before whatever reads it. Codegen already
//! knows how to do that: an `AssignTemp` whose body is not one of the
//! array-producing opcodes lowers to a `BeginIter` loop that evaluates the body
//! element by element.
//!
//! This pass discharges all of it, and it is the only one: [`super::Var::new`]
//! runs it once over the fully lowered, constant-folded fragment, where every
//! subscript is resolved and every view is concrete, and it is the only caller
//! of [`crate::ast::TempAllocator::alloc`] in the crate. Positions are read off
//! [`crate::builtins::BuiltinFn::signature`] -- `ArgKind::Array` says which
//! operands codegen wants as views, `ResultKind::Array` says which calls write
//! a temp -- so a new builtin is classified in the table rather than in a
//! second hand-maintained list here.
//!
//! # Once per equation, or once per element
//!
//! An apply-to-all or arrayed equation is expanded per element before this pass
//! runs, so the same array value can appear in several elements' code. Which
//! ones are the same is decided HERE, on the lowered form, by structural
//! identity: a body that is equal in two elements is evaluated ONCE, hoisted
//! ahead of the element code into a temp each element reads back at its own
//! index; a body that differs -- because a scalar argument or a subscript
//! resolved to the active element -- is evaluated per element.
//!
//! Structural identity is exactly the right test and needs no separate analysis
//! pass: the elements of one equation are lowerings of one arm under different
//! active subscripts, so two elements' bodies are equal precisely when nothing
//! in the body depended on the element. Sharing is safe because a fragment
//! writes only its own variable's slots and its own temps, and a self-reference
//! is a dependency cycle the graph rejects before compilation -- so no
//! expression in the fragment can change what an earlier element's body read.
//!
//! The two regimes also differ in what they cost. A shared body takes one temp
//! id for the whole equation. A per-element body takes one id that every
//! element REUSES, because such a temp is written and read inside one element's
//! code and is dead before the next element runs
//! ([`crate::ast::TempAllocator::element_scopes`]) -- which is what keeps a
//! 300-element equation at one temp slot rather than 300, the bytecode `TempId`
//! being a `u8`. A temp read through a static VIEW carries the `u32` and is
//! rejected at resolution above 255 (`symbolic::resolve_static_view`); that
//! refusal stays loud (GH #583).
//!
//! # Why it is safe
//!
//! It rewrites **only** operands codegen would have rejected: [`is_view`] is
//! the negation of `walk_expr_as_view`'s accepting arms, so an operand position
//! that compiles today passes through untouched.
//!
//! # What still declines
//!
//! Four limits are worth knowing before reading a "this shape does not
//! compile" report as a bug in this pass:
//!
//! * An operand only materializes if [`super::find_expr_array_view`] can
//!   derive a shape for it. That function's `App` arm reads each builtin's
//!   `ResultKind` to decide which arguments propagate an array shape; the ones
//!   that propagate none (the reducers, `VECTOR SELECT`, the `Lookup` family)
//!   are explained there.
//! * That shape is the JOIN of every array in the operand -- the view they all
//!   broadcast into -- so an operand mixing INCOMPARABLE shapes (`row[e]` and
//!   `col[d]`, neither containing the other) has none, and declines. The union
//!   `[e,d]` would compile, but nothing in the operand says whether it is
//!   `[e,d]` or `[d,e]`, and the temp's axis order is the axis
//!   `VECTOR SORT ORDER` sorts along. Declining leaves the loud codegen
//!   rejection; guessing would leave a plausible array of wrong numbers.
//! * An operand carrying a REPEATED-dimension view (`matrix[d,d]`) declines --
//!   mixed with another shape, and as the operand's SOLE shape. The compile-time
//!   projection pairs a temp's axes to the variable's one to one
//!   (`super::project_var_index_to_temp`), so reading a `[d,d]` temp back
//!   element by element would be right; what cannot say WHICH `d` is meant is
//!   the RUNTIME broadcast: `codegen::array_view_to_static_temp` keys a temp
//!   view's `DimId`s by dimension NAME, and `dimensions::match_dimensions_two_pass`
//!   pairs a source axis with the FIRST iteration axis of that id, so a
//!   `BeginIter` body evaluating `matrix[d,d] * 2` into a `[d,d]` temp reads the
//!   diagonal. Declining keeps the loud codegen rejection instead.
//!
//!   The refusal lives HERE and in `codegen::snapshot_static_view` -- the two
//!   positions that can be loud about it -- and deliberately not inside
//!   `super::join_array_views`, even though that reads as the tidier home for
//!   it: an array-producing builtin sizes its own temp from
//!   `super::find_expr_array_view` and substitutes the VARIABLE's view for a
//!   `None`, silently and at a possibly different SIZE, so a refusal in the
//!   join would size the temp of `out[d] = SUM(VECTOR SORT ORDER(matrix[d,d],
//!   1))` at `out`'s three slots while the sort order still wrote nine, and the
//!   VM would index past the temp. Reading a repeated dimension DIRECTLY is a
//!   different matter and compiles: `out[d,d] = matrix[d,d]` copies the
//!   matrix and `VECTOR SORT ORDER(matrix[d,d], 1)` sorts each row, because
//!   the subscript lowering allocates active positions one to one
//!   (`compiler::subscript::normalize_subscripts3`). The shape is Simlin's to
//!   define: Vensim REJECTS the declaration (run in Vensim DSS 2026-08-04,
//!   `vensim-probes/repeated_dimension.mdl` refuses to simulate with "DimA
//!   appears more than once on LHS"), so no MDL-imported model carries it,
//!   while the XMILE v1.0 spec exemplifies it (`docs/reference/xmile-v1.0.html`,
//!   "A 2D non-apply-to-all array with dimensions X by X, where X is size 2",
//!   verified in-repo) and says nothing about what a REFERENCE such as
//!   `sq[X,X]` means. Pinned by
//!   `array_operand_materialization_tests::a_repeated_dimension_read_directly_reads_each_axis`
//!   and `a_repeated_dimension_operand_declines_rather_than_guessing_which_axis`.
//! * An array-producing builtin, or a per-element arrayed-GF apply, in a SCALAR
//!   position of a SCALAR equation has no element to read back -- `s = VECTOR
//!   SORT ORDER(vals[*], 1)` asks for a whole array in one slot -- so it is
//!   left for codegen to reject with a diagnostic.

use crate::ast::{ArrayView, TempAllocator};
use crate::builtins::{ArgKind, ResultKind};
use crate::compiler::expr::{BuiltinFn, Expr, SubscriptIndex, VarRef};
use crate::dimensions::DimensionsContext;

/// The arrayed variable whose per-element slots an equation's assignments
/// write, for the one position that needs to know WHICH element it is: an
/// array value assigned to a single slot, which reads one element back out of
/// the temp it materializes into.
///
/// `None` for a scalar equation, which is what leaves an array value assigned
/// to its one slot as the loud codegen rejection it has to be.
pub(super) struct ElementTarget {
    /// The variable's first slot; an assignment's element index is its
    /// distance from here.
    pub(super) base: VarRef,
    /// The variable's own dimensions, contiguous -- the space
    /// [`super::project_var_index_to_temp`] projects an element index FROM.
    pub(super) view: ArrayView,
}

/// How the expression under rewrite is consumed.
///
/// Only two answers matter, because they are the two things codegen can emit
/// for an array-producing call: a whole-array read (`TempArray`, inside a view
/// operand) or one element of it (`TempArrayElement`, wherever a number is
/// wanted). The position propagates unchanged through `Op1`/`Op2`/`If`, whose
/// operands are consumed however the surrounding expression is, and is reset to
/// [`Position::Array`] by an `ArgKind::Array` operand and to
/// [`Position::Scalar`] by every other builtin argument.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Position {
    Array,
    Scalar,
}

/// One materialized array value: the body a temp is written from, and who
/// reads it.
struct Site {
    /// The rewritten body. Doubles as the identity two elements are compared
    /// on: bodies are built bottom-up and a repeat is never rebuilt, so an
    /// equal body is an equal computation. The comparison is `Expr`'s derived
    /// `PartialEq`, which includes every `Loc`. That is not load-bearing for
    /// soundness -- two arms spelling one text at different offsets compute
    /// the same value -- but it is what confines sharing to the elements of
    /// one arm (an apply-to-all body or an EXCEPT default, lowered once per
    /// element from one text) plus explicit arms that spell the body at the
    /// same offset; the temp counts and the "explicit arms never share"
    /// statements in the docs rest on it.
    body: Expr,
    view: ArrayView,
    /// The first top-level expression -- one per element of the equation --
    /// whose code reads this temp.
    first_group: usize,
    /// Read by a second element's code too: the fact the numbering keys on.
    shared: bool,
    /// The temps this body itself reads. All have smaller ids (a nested value
    /// is materialized before the one that reads it), which is what lets one
    /// reverse sweep propagate sharedness.
    reads: Vec<usize>,
}

/// Rewrite `exprs` so every array value codegen cannot express in place is a
/// temp it can, and return the fragment's temp ids to `temps`.
///
/// `exprs` is one variable's one phase, one top-level expression per element of
/// the equation (or the single expression of a scalar one). `target` names the
/// variable those expressions assign to; see [`ElementTarget`].
pub(super) fn materialize_computed_array_operands(
    exprs: Vec<Expr>,
    temps: &TempAllocator,
    target: Option<&ElementTarget>,
    dimensions_ctx: &DimensionsContext,
) -> Vec<Expr> {
    let mut pass = Materializer {
        sites: Vec::new(),
        target,
        dimensions_ctx,
    };

    // Rewrite each element's code, recording every array value it materializes.
    // Ids issued here are provisional indices into `sites`; the final numbering
    // needs to know which sites turned out to be shared, which is only settled
    // once every element has been seen.
    let mut groups: Vec<(std::ops::Range<usize>, Expr)> = Vec::with_capacity(exprs.len());
    for (group, expr) in exprs.into_iter().enumerate() {
        let element = pass.element_of(&expr);
        let first_site = pass.sites.len();
        let expr = pass.rewrite(expr, Position::Scalar, element, group);
        groups.push((first_site..pass.sites.len(), expr));
    }
    if pass.sites.is_empty() {
        return groups.into_iter().map(|(_, expr)| expr).collect();
    }

    // A site is shared when two elements read it, and transitively when a
    // shared body reads it: that body is emitted once, ahead of the element
    // code, so everything it reads has to outlive one element too. `reads`
    // always names smaller ids, so one descending sweep closes the relation.
    let mut shared: Vec<bool> = pass.sites.iter().map(|s| s.shared).collect();
    for i in (0..pass.sites.len()).rev() {
        if shared[i] {
            for &j in &pass.sites[i].reads {
                shared[j] = true;
            }
        }
    }

    // The final numbering. Shared ids come first, so they sit below the range
    // the elements recycle and no element's temp can clobber one. Each element
    // then reissues the same ids for its own temps
    // (`TempAllocator::element_scopes`), which is what keeps the count at one
    // per simultaneously-live temp rather than one per element.
    let mut final_id: Vec<u32> = vec![0; pass.sites.len()];
    for (i, is_shared) in shared.iter().enumerate() {
        if *is_shared {
            final_id[i] = temps.alloc();
        }
    }
    {
        let scopes = temps.element_scopes();
        for (home_sites, _) in &groups {
            scopes.begin_element();
            for i in home_sites.clone() {
                if !shared[i] {
                    final_id[i] = temps.alloc();
                }
            }
        }
    }

    // Emit: the shared bodies once, in id order (a body's own reads have
    // smaller ids, so each is written before the one that reads it), then each
    // element preceded by the temps only it uses.
    let mut sites = pass.sites;
    let mut out: Vec<Expr> = Vec::new();
    for (i, is_shared) in shared.iter().enumerate() {
        if *is_shared {
            out.push(take_assignment(&mut sites[i], i));
        }
    }
    for (home_sites, expr) in groups {
        for i in home_sites {
            if !shared[i] {
                out.push(take_assignment(&mut sites[i], i));
            }
        }
        out.push(expr);
    }
    // Every temp id in `out` -- the writes emitted just above and the reads
    // inside the expressions -- is still the site's index; one sweep maps them
    // all to the numbering above. Emitting a final id directly would put it
    // through this map a second time.
    //
    // The precondition that makes the sweep unconditional: NO pre-existing temp
    // id reaches it. Nothing upstream of this pass allocates a temp -- `Expr3`
    // is a structural rewrite and `compiler::context` lowers it structurally --
    // so every id in `out` was issued as a site index by the loop above. An id
    // from any other source would be silently reinterpreted as a site index and
    // land on an unrelated slot, so it is checked rather than assumed.
    debug_assert!(
        {
            let mut in_range = true;
            for expr in &mut out {
                visit_temp_ids(expr, &mut |id| in_range &= (*id as usize) < final_id.len());
            }
            in_range
        },
        "a temp id outside this pass's {} sites reached the final remap",
        final_id.len()
    );
    for expr in &mut out {
        visit_temp_ids(expr, &mut |id| *id = final_id[*id as usize]);
    }
    out
}

/// The `AssignTemp` that writes site `id`, taking its body. `id` is the site's
/// index, like every other temp id in the pass's output.
fn take_assignment(site: &mut Site, id: usize) -> Expr {
    let body = std::mem::replace(&mut site.body, Expr::Const(0.0, crate::ast::Loc::default()));
    Expr::AssignTemp(id as u32, Box::new(body), site.view.clone())
}

/// Apply `f` to every temp id `expr` names, written or read.
///
/// Exhaustive on purpose: a variant that carried a temp id and was missed here
/// would keep a provisional id and index the wrong slot.
fn visit_temp_ids(expr: &mut Expr, f: &mut impl FnMut(&mut u32)) {
    match expr {
        Expr::AssignTemp(id, rhs, _) => {
            f(id);
            visit_temp_ids(rhs, f);
        }
        Expr::TempArray(id, _, _) | Expr::TempArrayElement(id, _, _, _) => f(id),
        Expr::App(builtin, _) => {
            for arg in builtin.args_mut() {
                visit_temp_ids(arg, f);
            }
        }
        Expr::Op1(_, inner, _) => visit_temp_ids(inner, f),
        Expr::Op2(_, lhs, rhs, _) => {
            visit_temp_ids(lhs, f);
            visit_temp_ids(rhs, f);
        }
        Expr::If(cond, t, e, _) => {
            visit_temp_ids(cond, f);
            visit_temp_ids(t, f);
            visit_temp_ids(e, f);
        }
        Expr::Subscript(_, indices, _, _) => {
            for index in indices {
                match index {
                    SubscriptIndex::Single(e) => visit_temp_ids(e, f),
                    SubscriptIndex::Range(lo, hi) => {
                        visit_temp_ids(lo, f);
                        visit_temp_ids(hi, f);
                    }
                }
            }
        }
        Expr::EvalModule(_, _, _, args) => {
            for arg in args {
                visit_temp_ids(arg, f);
            }
        }
        Expr::AssignCurr(_, rhs) | Expr::AssignNext(_, rhs) => visit_temp_ids(rhs, f),
        Expr::Const(_, _) | Expr::Var(_, _) | Expr::StaticSubscript(_, _, _) | Expr::Dt(_) => {}
        Expr::ModuleInput(_, _) => {}
    }
}

struct Materializer<'a> {
    sites: Vec<Site>,
    target: Option<&'a ElementTarget>,
    dimensions_ctx: &'a DimensionsContext,
}

impl Materializer<'_> {
    /// Which element of the target variable `expr` assigns, when it assigns one
    /// at all.
    ///
    /// The guard mirrors `super::apply_implicit_with_lookup`'s: every
    /// assignment a fragment emits targets its own variable at or after its
    /// base, and an assignment that does not is not an element of this
    /// equation.
    fn element_of(&self, expr: &Expr) -> Option<usize> {
        let target = self.target?;
        let dst = match expr {
            Expr::AssignCurr(dst, _) | Expr::AssignNext(dst, _) => dst,
            _ => return None,
        };
        (dst.name == target.base.name && dst.element_offset >= target.base.element_offset)
            .then(|| dst.element_offset - target.base.element_offset)
    }

    /// The shape a temp for an array value would take when the value's own
    /// subexpressions do not say -- the enclosing variable's, or a single slot
    /// in a scalar equation.
    fn fallback_view(&self) -> ArrayView {
        match self.target {
            Some(target) => target.view.clone(),
            None => ArrayView::contiguous(vec![1]),
        }
    }

    fn rewrite(&mut self, expr: Expr, pos: Position, element: Option<usize>, group: usize) -> Expr {
        match expr {
            Expr::App(builtin, loc) => {
                // Bottom-up, each argument in the position its `ArgKind` gives
                // it: an array operand is consumed whole, everything else --
                // the scalar arguments beside it, and a lookup's table
                // identity -- one number at a time.
                let builtin = builtin.map_with_kinds(|arg, kind| match kind {
                    ArgKind::Array { .. } => self.rewrite(arg, Position::Array, element, group),
                    ArgKind::Scalar | ArgKind::Table => {
                        self.rewrite(arg, Position::Scalar, element, group)
                    }
                    ArgKind::Ident => {
                        unreachable!("an identifier payload is not an expression argument")
                    }
                });
                let builtin = self.materialize_view_operands(builtin, group);
                let app = Expr::App(builtin, loc);
                match self.array_result_view(&app) {
                    Some(view) => self.materialize_value(app, view, pos, element, group),
                    None => app,
                }
            }
            Expr::Op1(op, inner, loc) => {
                Expr::Op1(op, Box::new(self.rewrite(*inner, pos, element, group)), loc)
            }
            Expr::Op2(op, lhs, rhs, loc) => Expr::Op2(
                op,
                Box::new(self.rewrite(*lhs, pos, element, group)),
                Box::new(self.rewrite(*rhs, pos, element, group)),
                loc,
            ),
            Expr::If(cond, then_expr, else_expr, loc) => Expr::If(
                Box::new(self.rewrite(*cond, pos, element, group)),
                Box::new(self.rewrite(*then_expr, pos, element, group)),
                Box::new(self.rewrite(*else_expr, pos, element, group)),
                loc,
            ),
            Expr::Subscript(base, indices, bounds, loc) => {
                let indices = indices
                    .into_iter()
                    .map(|idx| match idx {
                        SubscriptIndex::Single(e) => SubscriptIndex::Single(self.rewrite(
                            e,
                            Position::Scalar,
                            element,
                            group,
                        )),
                        SubscriptIndex::Range(start, end) => SubscriptIndex::Range(
                            self.rewrite(start, Position::Scalar, element, group),
                            self.rewrite(end, Position::Scalar, element, group),
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
                    .map(|arg| self.rewrite(arg, Position::Scalar, element, group))
                    .collect(),
            ),
            Expr::AssignCurr(dst, rhs) => Expr::AssignCurr(
                dst,
                Box::new(self.rewrite(*rhs, Position::Scalar, element, group)),
            ),
            Expr::AssignNext(dst, rhs) => Expr::AssignNext(
                dst,
                Box::new(self.rewrite(*rhs, Position::Scalar, element, group)),
            ),
            Expr::AssignTemp(id, rhs, view) => Expr::AssignTemp(
                id,
                Box::new(self.rewrite(*rhs, Position::Array, element, group)),
                view,
            ),
            leaf @ (Expr::Const(_, _)
            | Expr::Var(_, _)
            | Expr::StaticSubscript(_, _, _)
            | Expr::TempArray(_, _, _)
            | Expr::TempArrayElement(_, _, _, _)
            | Expr::Dt(_)
            | Expr::ModuleInput(_, _)) => leaf,
        }
    }

    /// The shape of `expr`'s value when codegen can only produce it by writing
    /// a temp, or `None` when the expression is not one of those.
    ///
    /// Two families, and both are read off the signature table rather than off
    /// a list here:
    ///
    /// * `ResultKind::Array` -- an array-producing builtin, whose dedicated
    ///   opcode writes a temp and has no in-place form;
    /// * a `LOOKUP` family call whose TABLE operand is a multi-element array,
    ///   the per-element arrayed-GF apply (GH #580 Bug B), which codegen emits
    ///   as `LookupArray` writing one result per table. A single-element table
    ///   base is the ordinary scalar lookup and yields `None`.
    fn array_result_view(&self, expr: &Expr) -> Option<ArrayView> {
        let Expr::App(builtin, _) = expr else {
            return None;
        };
        if let Some(view) = arrayed_lookup_apply_view(builtin) {
            return Some(view);
        }
        if !matches!(builtin.result_kind(), ResultKind::Array { .. }) {
            return None;
        }
        // The shape comes from the builtin's shaping argument
        // (`ResultKind::Array::shape_from`). When that argument carries no view
        // of its own -- a constant ELM MAP offset, say -- the enclosing
        // variable's shape is the only one available.
        Some(super::find_expr_array_view(expr).unwrap_or_else(|| self.fallback_view()))
    }

    /// Move an array VALUE into a temp and return the read that replaces it.
    ///
    /// A whole-array read where an array is wanted; the element this assignment
    /// writes where a number is. A scalar position with no element -- a scalar
    /// equation, whose one slot cannot hold an array at all -- still
    /// materializes, and reads the temp WHOLE: that is the shape
    /// `codegen::walk_expr`'s `TempArray` arm refuses with the one
    /// array-in-a-one-value-position diagnostic, which is the refusal this
    /// belongs in rather than each builtin having its own.
    fn materialize_value(
        &mut self,
        body: Expr,
        view: ArrayView,
        pos: Position,
        element: Option<usize>,
        group: usize,
    ) -> Expr {
        // Reading one element back out of the temp needs a coordinate for every
        // axis the temp has. An axis with no correspondence to the enclosing
        // variable's -- the free ROW of `out[COP] = LOOKUP(g, Time)` over a
        // `g[COP, ROW]` table -- has none, so the temp is read WHOLE instead,
        // which leaves the array in a one-value position and the refusal that
        // shape has to get.
        let index = match pos {
            Position::Array => None,
            Position::Scalar => element.and_then(|element| self.temp_index_of(element, &view)),
        };
        self.materialize_at(body, view, index, group)
    }

    /// The temp index the enclosing variable's element reads, when the temp's
    /// axes correspond to the variable's at all.
    fn temp_index_of(&self, element: usize, view: &ArrayView) -> Option<usize> {
        let target = self.target?;
        super::project_var_index_to_temp(element, &target.view, view, self.dimensions_ctx)
    }

    /// Record `body` as the fragment's temp for that value -- reusing the one
    /// an earlier element materialized when the body is the same -- and return
    /// the read.
    fn materialize_at(
        &mut self,
        body: Expr,
        view: ArrayView,
        index: Option<usize>,
        group: usize,
    ) -> Expr {
        let loc = body.get_loc();
        let id = match self.sites.iter().position(|site| site.body == body) {
            Some(id) => id,
            None => {
                let mut reads = Vec::new();
                let mut body = body;
                visit_temp_ids(&mut body, &mut |id| reads.push(*id as usize));
                self.sites.push(Site {
                    body,
                    view,
                    first_group: group,
                    shared: false,
                    reads,
                });
                self.sites.len() - 1
            }
        };
        if self.sites[id].first_group != group {
            self.sites[id].shared = true;
        }
        let view = self.sites[id].view.clone();
        match index {
            None => Expr::TempArray(id as u32, view, loc),
            Some(index) => Expr::TempArrayElement(id as u32, view, index, loc),
        }
    }

    /// Materialize every view-requiring operand position: the table's
    /// `ArgKind::Array` positions, which are exactly the positions codegen reads
    /// through `walk_expr_as_view` (the reducers' argument, `VECTOR SELECT`'s two
    /// arrays, the array-producing builtins' array arguments), minus the one
    /// position below that is deliberately left alone. A lookup's table
    /// (`ArgKind::Table`) is read as a view too, but it must name a whole
    /// *variable*: codegen resolves it to a `base_gf` by ident
    /// (`arrayed_lookup_table_info`), and a temp has no graphical functions
    /// attached to it. n-ary `MEAN` has no array position -- it averages scalars --
    /// so `MEAN(a * b)` over two scalars passes through untouched while the
    /// single-argument `MEAN(matrix[E,*] * 2)` materializes like every other
    /// reducer.
    fn materialize_view_operands(&mut self, builtin: BuiltinFn, group: usize) -> BuiltinFn {
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
        let exempt_profile = matches!(builtin, BuiltinFn::AllocateAvailable(_, _, _));
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
        // A COMPUTED source is a Simlin EXTENSION. Vensim rejects the shape
        // outright -- run in Vensim DSS on 2026-08-04,
        // `vensim-probes/elm_map_computed_source.mdl` refuses to simulate with
        // "Argument 1 to function VECTOR ELM MAP must be a normal variable". So
        // there is no Vensim behaviour to match here, and the question is not
        // "which rule does Vensim use" but "what shall this mean in Simlin".
        //
        // It means the HELPER-EQUIVALENT thing: an inline expression behaves
        // exactly as the same values pre-assigned to a named variable, which is
        // the spelling that IS legal Vensim. A materialized operand is a fresh
        // contiguous temp and so is full-array by construction, which confines
        // the mapping to the computed array -- exactly what
        // `VECTOR ELM MAP(helper[A1], offs)` does when `helper` holds those
        // values. That definition is deliberate; the temp has no "rest of the
        // variable" to run into, so nothing else is even expressible. Pinned by
        // `array_operand_materialization_tests::materializing_an_elm_map_source_confines_the_mapping_to_the_temp`.
        let mut position = 0usize;
        builtin.map_with_kinds(|arg, kind| {
            let is_profile = exempt_profile && position == 1;
            position += 1;
            match kind {
                ArgKind::Array { .. } if !is_profile => self.materialize_view_operand(arg, group),
                ArgKind::Array { .. } | ArgKind::Scalar | ArgKind::Table => arg,
                ArgKind::Ident => {
                    unreachable!("an identifier payload is not an expression argument")
                }
            }
        })
    }

    /// Move `operand` into a temp of its own and return the `TempArray`
    /// reference that replaces it, or return it unchanged when it is already a
    /// view or when no array shape can be derived for it.
    ///
    /// A bare array-valued `PREVIOUS`/`INIT` is left alone for the same reason a
    /// `StaticSubscript` is: it already IS a view, over a snapshot buffer rather
    /// than over `curr` (GH #995, [`is_snapshot_view`]). Materializing it would
    /// spend a temp to copy an array that codegen can address directly.
    fn materialize_view_operand(&mut self, operand: Expr, group: usize) -> Expr {
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
        // operand for codegen to reject, exactly as the deliberately
        // unmaterialized profile position above does, rather than guessing an
        // axis order.
        let Some(source_view) = super::find_expr_array_view(&operand) else {
            return operand;
        };
        // A repeated dimension name is refused as a TEMP's shape even when it is the
        // operand's sole shape (`super::view_repeats_a_dimension`: the runtime
        // broadcast pairs by dimension id and would read the diagonal). The refusal
        // lives here rather than inside the join because `find_expr_array_view` has
        // other consumers and only this one is loud: an array-producing builtin's
        // own shaping falls back to the VARIABLE's view for a `None`, so a `None`
        // there silently reshapes a temp instead of declining -- measured, `out[d]
        // = SUM(VECTOR SORT ORDER(matrix[d,d], 1))` sized a 9-element sort order's
        // temp at 3 and the VM indexed past it. Refusing at the one site that
        // produces a diagnostic costs no shape that reads the array directly.
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

        self.materialize_at(operand, view, None, group)
    }
}

/// The view of the table array a per-element arrayed-GF apply
/// (`LOOKUP(g[D!], idx)`, GH #580 Bug B) writes, or `None` for the ordinary
/// scalar lookup.
///
/// It has to be decomposed wherever it appears -- bare in a reducer argument,
/// or nested inside an `Op2`/`If` that is itself materialized -- because
/// otherwise an enclosing operand is materialized whole and buries an
/// un-emittable multi-element `Lookup` inside a `BeginIter` body that codegen
/// rejects with `BadTable`. The `AssignTemp`'s bare `App(Lookup(...))` body is
/// what codegen's dedicated `LookupArray` opcode consumes.
fn arrayed_lookup_apply_view(builtin: &BuiltinFn) -> Option<ArrayView> {
    let table = match builtin {
        BuiltinFn::Lookup(table, _, _)
        | BuiltinFn::LookupForward(table, _, _)
        | BuiltinFn::LookupBackward(table, _, _) => table,
        _ => return None,
    };
    let view = super::find_expr_array_view(table)?;
    (view.dims.iter().product::<usize>() > 1).then_some(view)
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
