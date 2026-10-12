// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The one per-variable lowering entry point, [`lower_fragment`], and the
//! input it consumes, [`FragmentInput`].
//!
//! A fragment is one variable's one phase compiled to layout-independent
//! symbolic bytecode. Lowering it needs the variable itself and, for every
//! name the variable can reference, only that name's **shape** -- its
//! dimensions and whether it is a plain variable or a module instance
//! ([`DepShape`]). Explicit variables, parse-synthesized implicit helpers, LTM
//! synthetic variables and LTM implicit helpers each build a `FragmentInput`
//! through a constructor of their own in `db/`, and every one of them is
//! lowered here. The emission half shares the same input:
//! [`FragmentInput::emit_ctx`] is the codegen context the per-phase emitter
//! (`db::assemble::compile_phase_to_per_var_bytecodes`) reads.
//!
//! Addresses are assigned once, at assembly, so a `FragmentInput` carries no
//! offset of the model being compiled. The single exception is a module
//! dependency: a cross-module reference `m·x` lowers to `VarRef { name: m,
//! element_offset: x's slot inside the instance }`, because the sub-model's
//! layout is already fixed when the parent's fragments compile. That layout,
//! with each sub-model variable's shape, is the [`ModelShape`] a
//! [`DepKind::Module`] carries -- recursively, so a nested `m·n·x` resolves
//! through the chain of shapes.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::sync::Arc;

use crate::common::{Canonical, Ident, IdentMap, IdentSet, Result};
use crate::dimensions::{Dimension, DimensionsContext};
use crate::variable::Variable;

use super::codegen::ModuleCtx;
use super::context::{Context, ContextCore};
use super::{Table, Var, VarRef, VarSizes};

/// What lowering needs to know about one name a fragment can reference: its
/// dimensions and whether it is a plain variable or a module instance.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) struct DepShape {
    /// The declared dimensions; empty for a scalar and for a module instance.
    pub dims: Vec<Dimension>,
    pub kind: DepKind,
}

/// The storage a dependency's name denotes.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) enum DepKind {
    /// An aux, flow, stock, lookup table or synthesized helper: one slot per
    /// element of `dims` (one slot when scalar).
    Var,
    /// A module instance: one block of the instantiated model's slots, laid
    /// out as `shape` says.
    Module { shape: Arc<ModelShape> },
}

impl DepShape {
    /// A plain variable over `dims` (scalar when empty).
    pub(crate) fn var(dims: Vec<Dimension>) -> Self {
        DepShape {
            dims,
            kind: DepKind::Var,
        }
    }

    /// A module instance of a model laid out as `shape`.
    pub(crate) fn module(shape: Arc<ModelShape>) -> Self {
        DepShape {
            dims: Vec::new(),
            kind: DepKind::Module { shape },
        }
    }

    /// Whether the name is a module instance. This is the one shape whose
    /// value depends on the LTM overlay -- it carries the sub-model's layout,
    /// which holds that model's LTM section under `On` -- so it is also what
    /// decides whether a fragment's memo is keyed on the overlay
    /// (`db::var_fragment::fragment_reads_module`).
    pub(crate) fn is_module(&self) -> bool {
        matches!(self.kind, DepKind::Module { .. })
    }

    /// The dimensions, or `None` for a scalar -- the form every arrayed-lowering
    /// decision in `compiler::context` branches on.
    pub(crate) fn dimensions(&self) -> Option<&[Dimension]> {
        if self.dims.is_empty() {
            None
        } else {
            Some(&self.dims)
        }
    }

    /// The slots this name occupies: the element count of a variable, the
    /// instantiated model's slot count for a module.
    pub(crate) fn size(&self) -> usize {
        match &self.kind {
            DepKind::Var => self.dims.iter().map(|d| d.len()).product::<usize>().max(1),
            DepKind::Module { shape } => shape.n_slots,
        }
    }
}

/// A sub-model's fixed layout together with each variable's shape: what a
/// cross-module reference resolves through (`db::model_shape` derives it from
/// `compute_layout` and the per-variable dimension queries).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Default)]
pub(crate) struct ModelShape {
    pub vars: IdentMap<Ident<Canonical>, ShapeEntry>,
    pub n_slots: usize,
}

/// One sub-model variable: where its block starts inside the instance, and its
/// shape (a nested module instance carries the nested model's shape).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) struct ShapeEntry {
    pub offset: usize,
    pub shape: DepShape,
}

/// Everything the lowering and emission of one variable's fragments read,
/// besides the phase. Built by one of the four constructors in `db/`.
pub(crate) struct FragmentInput<'a> {
    /// The variable being lowered, in its `Expr2` form: borrowed from its
    /// per-variable memo (`db::lowered_source_variable`,
    /// `db::lowered_implicit_variable`) by the two in-model constructors, so
    /// compiling a variable retains no second copy of its lowered tree; owned
    /// by the two LTM constructors, whose generated equations exist only for
    /// the fragment.
    pub target: Cow<'a, Variable>,
    /// The shape of every name the target can reference, its own included.
    pub deps: IdentMap<Ident<Canonical>, DepShape>,
    /// Graphical-function tables, keyed by the variable that declares them:
    /// the target's own and those of the lookup tables it calls.
    pub tables: IdentMap<Ident<Canonical>, Vec<Table>>,
    /// The module instance's input ports (empty for a root model): a name in
    /// this set lowers to a `ModuleInput` read instead of a slot read.
    pub module_inputs: BTreeSet<Ident<Canonical>>,
    pub model_name: Ident<Canonical>,
    /// The project's dimensions, in both forms the lowering consults.
    pub dimensions: &'a [Dimension],
    pub dimensions_ctx: &'a DimensionsContext,
    /// Derived from `deps` by [`reference_extents`], never authored: the one
    /// table both lowering (the ELM MAP fold) and codegen (`full_source_len`)
    /// read to answer how big the variable a reference addresses is.
    var_sizes: VarSizes,
}

impl<'a> FragmentInput<'a> {
    pub(crate) fn new(
        target: Cow<'a, Variable>,
        deps: IdentMap<Ident<Canonical>, DepShape>,
        tables: IdentMap<Ident<Canonical>, Vec<Table>>,
        module_inputs: BTreeSet<Ident<Canonical>>,
        model_name: Ident<Canonical>,
        dimensions: &'a [Dimension],
        dimensions_ctx: &'a DimensionsContext,
    ) -> Self {
        let var_sizes = reference_extents(&deps);
        FragmentInput {
            target,
            deps,
            tables,
            module_inputs,
            model_name,
            dimensions,
            dimensions_ctx,
            var_sizes,
        }
    }

    /// The phase-invariant codegen context for this fragment: everything
    /// `compile_phase_to_per_var_bytecodes` needs except the phase's lowered
    /// expressions and the temp sizes it derives from them (it fills both in).
    /// The empty runlist placeholders exist in exactly one place, here, so no
    /// emission site can forget which runlist a fragment's expressions belong
    /// in.
    pub(crate) fn emit_ctx(&self) -> ModuleCtx<'_> {
        ModuleCtx {
            ident: &self.model_name,
            inputs: &self.module_inputs,
            temp_sizes: &[],
            runlist_initials_by_var: &[],
            runlist_flows: &[],
            runlist_stocks: &[],
            var_sizes: &self.var_sizes,
            tables: &self.tables,
            dimensions: self.dimensions,
        }
    }
}

impl FragmentInput<'_> {
    /// The context every lowering of this fragment runs under.
    fn context(&self, is_initial: bool) -> Context<'_> {
        Context::new(
            ContextCore {
                dimensions: self.dimensions,
                dimensions_ctx: self.dimensions_ctx,
                deps: &self.deps,
                var_sizes: &self.var_sizes,
                inputs: &self.module_inputs,
            },
            is_initial,
        )
    }

    /// An arrayed target as the equations of its elements, each with every
    /// read its element pins to one element spelled as that element's static
    /// index (`Context::pin_element_reads`), for a describer that classifies
    /// reads by their spelling (LTM's reference-site IR); `None` for a target
    /// that needs no such view.
    ///
    /// A target needs it where its equations read one variable in more than
    /// one spelling across its elements, or read an element computed from
    /// the element's position: an arrayed equation with elements of their
    /// own (one element's `c[young]` beside the others' `c[Age]`), and an
    /// equation that reads an offset (`c[Age - 1]`, which a describer would
    /// otherwise take as a read of any element). Pinned, every element reads
    /// what its slot of the compiled fragment reads, the form an equation
    /// written element by element has. An element with no equation is left
    /// out, as it is from the target.
    pub(crate) fn elementwise_target(&self) -> Option<Variable> {
        use crate::ast::Ast;
        if self.target.element_scope().is_some() {
            return None;
        }
        let ctx = self.context(false);
        let per_element = |ast: &Ast<crate::ast::Expr2>| -> Option<Ast<crate::ast::Expr2>> {
            let (dims, arms, default, applies) = match ast {
                Ast::Scalar(_) => return None,
                Ast::ApplyToAll(dims, expr) => {
                    if !reads_a_computed_position(expr, dims) {
                        return None;
                    }
                    (dims, None, Some(expr), false)
                }
                Ast::Arrayed(dims, arms, default, applies) => (
                    dims,
                    Some(arms),
                    default.as_ref().filter(|_| *applies),
                    *applies,
                ),
            };
            let bodies = || {
                arms.into_iter()
                    .flat_map(|arms| arms.values())
                    .chain(default)
            };
            let names: Vec<&str> = dims.iter().map(|d| d.canonical_name().as_str()).collect();
            let mut reads = ReadSpellings::default();
            for body in bodies() {
                reads.collect(body, &names, false);
            }
            let mut elements = IdentMap::default();
            for parts in crate::dimensions::SubscriptIterator::new(dims) {
                let key = crate::common::CanonicalElementName::from_parts(&parts);
                let Some(expr) = arms.and_then(|arms| arms.get(&key)).or(default) else {
                    continue;
                };
                let elem_ctx =
                    ctx.with_active_subscripts(std::sync::Arc::<[_]>::from(dims.clone()), &parts);
                elements.insert(key, describe_at_element(&elem_ctx, expr, &reads.pinned));
            }
            // Equations whose elements each read as written are their own
            // view.
            if arms.is_some_and(|arms| *arms == elements) {
                return None;
            }
            Some(Ast::Arrayed(dims.clone(), elements, None, applies))
        };
        let mut view = self.target.as_ref().clone();
        let changed = match &mut view.kind {
            crate::variable::VarKind::Aux { ast, init_ast, .. } => {
                let new_ast = ast.as_ref().and_then(per_element);
                let new_init = init_ast.as_ref().and_then(per_element);
                let changed = new_ast.is_some() || new_init.is_some();
                if let Some(new_ast) = new_ast {
                    *ast = Some(new_ast);
                }
                if let Some(new_init) = new_init {
                    *init_ast = Some(new_init);
                }
                changed
            }
            crate::variable::VarKind::Stock { init_ast, .. } => {
                match init_ast.as_ref().and_then(per_element) {
                    Some(new_ast) => {
                        *init_ast = Some(new_ast);
                        true
                    }
                    None => false,
                }
            }
            crate::variable::VarKind::Module { .. } => false,
        };
        changed.then_some(view)
    }

    /// The target with its element scope resolved into its body: every read
    /// the scope's element pins to one element spelled as that element's
    /// static index (`Context::pin_element_reads`), and no scope left, so a
    /// describer that classifies reads by their spelling (LTM's reference-site
    /// IR) sees the reads the compiled fragment makes. A target with no scope
    /// is returned as it is.
    pub(crate) fn element_pinned_target(&self) -> Variable {
        let Some(scope) = self.target.element_scope() else {
            return self.target.as_ref().clone();
        };
        let ctx = self.context(false);
        let Ok((_, elem_ctx, _)) = ctx.element_scope_context(scope) else {
            return self.target.as_ref().clone();
        };
        let pin = |ast: &Option<crate::ast::Ast<crate::ast::Expr2>>| {
            ast.as_ref().map(|ast| match ast {
                crate::ast::Ast::Scalar(expr) => {
                    crate::ast::Ast::Scalar(elem_ctx.pin_element_reads(expr))
                }
                other => other.clone(),
            })
        };
        let mut pinned = self.target.as_ref().clone();
        if let crate::variable::VarKind::Aux {
            ast,
            init_ast,
            element_scope,
            ..
        } = &mut pinned.kind
        {
            *ast = pin(ast);
            *init_ast = pin(init_ast);
            *element_scope = None;
        }
        pinned
    }
}

/// Lower one phase of a fragment: the target's initial-value form when
/// `is_initial`, its flow or stock-update form otherwise. The `Err` is the
/// phase's lowering failure, reported by the caller as a per-variable
/// diagnostic; the other phase may still lower.
pub(crate) fn lower_fragment(input: &FragmentInput<'_>, is_initial: bool) -> Result<Var> {
    Var::new(&input.context(is_initial), input.target.as_ref())
}

/// The extent of every variable a reference over `deps` can address **in
/// whole**, keyed by the reference that addresses it.
///
/// This is the single statement of "which reference addresses which variable's
/// full storage", read by lowering's scalar-source ELM MAP fold
/// (`Context::full_var_len_for_base`, GH #578) and by codegen
/// (`full_source_len`), so the two cannot disagree about where a source's
/// storage ends.
///
/// An ordinary variable contributes one entry, at its base. A module instance
/// contributes none of its own -- its slot count is the whole sub-model block,
/// which is the extent of nothing a reference can name -- and instead one entry
/// per sub-model variable at that variable's slot within the instance,
/// recursively through nested instances so every entry names a leaf variable.
/// A reference landing mid-array is absent, which is the answer it has always
/// had: the extent of one element of a bigger array is not the array's extent,
/// so the reader falls back to what the lowered view says.
pub(crate) fn reference_extents(deps: &IdentMap<Ident<Canonical>, DepShape>) -> VarSizes {
    let mut extents = VarSizes::default();
    for (name, shape) in deps {
        match &shape.kind {
            DepKind::Module { shape } => collect_instance_extents(shape, name, 0, &mut extents),
            DepKind::Var => {
                extents.insert(VarRef::base(name.clone()), shape.size());
            }
        }
    }
    extents
}

/// One module instance's contribution to [`reference_extents`]. `base` is the
/// slot the instance's copy of `shape` starts at, measured from the instance's
/// own base, so a nested instance's variables are reached by accumulating the
/// offsets on the way down -- the same arithmetic `Context::resolve` performs
/// when it lowers `m·n·x`. Shapes are finite trees (a cyclic module graph
/// cannot construct one: `db::project_module_graph` rejects it before any
/// shape is derived), so the walk needs no cycle guard.
fn collect_instance_extents(
    shape: &ModelShape,
    instance: &Ident<Canonical>,
    base: usize,
    extents: &mut VarSizes,
) {
    for entry in shape.vars.values() {
        match &entry.shape.kind {
            DepKind::Module { shape: nested } => {
                collect_instance_extents(nested, instance, base + entry.offset, extents)
            }
            DepKind::Var => {
                extents.insert(
                    VarRef::new(instance.clone(), base + entry.offset),
                    entry.shape.size(),
                );
            }
        }
    }
}

/// The variables an equation reads in a spelling loop analysis would take
/// for other than the read the run makes, gathered over every element's
/// body: those [`describe_at_element`] pins at every read.
///
/// A variable read only by the dimensions the equation iterates (`gdp[COP]`,
/// or `gdp` alone) is read at the same element everywhere, which is how loop
/// analysis reads that spelling, and it scores such a read over the
/// variable's dimensions at once. Any other read -- an index computed at the
/// element (`c[Age - 1]`), a literal element (`c[l1]`), a wildcard, a read in
/// a reducer's argument -- makes the variable pinned at every read, so that
/// loop analysis never sees one variable read in two spellings it would
/// reconcile wrongly (`c[Age]` beside `c[l1]`).
#[derive(Default)]
struct ReadSpellings {
    pinned: IdentSet<Ident<Canonical>>,
}

impl ReadSpellings {
    fn collect(&mut self, expr: &crate::ast::Expr2, dims: &[&str], in_reducer: bool) {
        use crate::ast::{Expr2, IndexExpr2};
        use crate::builtins::ArgKind;
        match expr {
            Expr2::Var(id, ..) => {
                if in_reducer {
                    self.pinned.insert(id.clone());
                }
            }
            Expr2::Const(..) => {}
            Expr2::Subscript(id, indices, ..) => {
                let iterated = indices.iter().all(|index| {
                    matches!(index, IndexExpr2::Expr(Expr2::Var(name, ..)) if dims.contains(&name.as_str()))
                });
                if in_reducer || !iterated {
                    self.pinned.insert(id.clone());
                }
                for index in indices {
                    match index {
                        IndexExpr2::Expr(e) => self.collect(e, dims, in_reducer),
                        IndexExpr2::Range(l, r, _) => {
                            self.collect(l, dims, in_reducer);
                            self.collect(r, dims, in_reducer);
                        }
                        IndexExpr2::Wildcard(_)
                        | IndexExpr2::StarRange(..)
                        | IndexExpr2::DimPosition(..) => {}
                    }
                }
            }
            Expr2::App(builtin, ..) => {
                for (arg, kind) in builtin.args_with_kinds() {
                    let reduces = matches!(kind, ArgKind::Array { whole: false });
                    self.collect(arg, dims, in_reducer || reduces);
                }
            }
            Expr2::Op1(_, inner, ..) => self.collect(inner, dims, in_reducer),
            Expr2::Op2(_, l, r, ..) => {
                self.collect(l, dims, in_reducer);
                self.collect(r, dims, in_reducer);
            }
            Expr2::If(c, t, f, ..) => {
                self.collect(c, dims, in_reducer);
                self.collect(t, dims, in_reducer);
                self.collect(f, dims, in_reducer);
            }
        }
    }
}

/// `expr` as loop analysis describes it at the active element of `ctx`: each
/// read of a variable in `pinned` ([`ReadSpellings`]) as the read the run
/// makes (`Context::pin_element_reads`) -- a computed index the element it
/// comes to, or not a number where it comes to none; a reducer's argument
/// with the axes the element names fixed -- and every other read as written.
/// A condition the element decides (`IF Age = 1`) takes its branch, and a
/// dimension read as a value is the element's position.
fn describe_at_element(
    ctx: &Context,
    expr: &crate::ast::Expr2,
    pinned: &IdentSet<Ident<Canonical>>,
) -> crate::ast::Expr2 {
    use crate::ast::Expr2;
    use crate::builtins::ArgKind;
    let recur = |e: &Expr2| describe_at_element(ctx, e, pinned);
    match expr {
        Expr2::Var(id, ..) if ctx.is_active_dimension(id) || pinned.contains(id) => {
            ctx.pin_element_reads(expr)
        }
        Expr2::Subscript(id, ..) if pinned.contains(id) => ctx.pin_element_reads(expr),
        Expr2::Var(..) | Expr2::Const(..) | Expr2::Subscript(..) => expr.clone(),
        Expr2::App(builtin, bounds, loc) => {
            if builtin
                .arg_kinds()
                .iter()
                .any(|kind| matches!(kind, ArgKind::Array { whole: false }))
            {
                return ctx.pin_element_reads(expr);
            }
            Expr2::App(
                builtin
                    .try_map_ref_with_kinds(|arg, kind| {
                        Ok::<_, std::convert::Infallible>(match kind {
                            ArgKind::Array { whole: true } => arg.clone(),
                            _ => recur(arg),
                        })
                    })
                    .unwrap(),
                bounds.clone(),
                *loc,
            )
        }
        Expr2::Op1(op, inner, bounds, loc) => {
            Expr2::Op1(*op, Box::new(recur(inner)), bounds.clone(), *loc)
        }
        Expr2::Op2(op, l, r, bounds, loc) => Expr2::Op2(
            *op,
            Box::new(recur(l)),
            Box::new(recur(r)),
            bounds.clone(),
            *loc,
        ),
        Expr2::If(c, t, f, bounds, loc) => match ctx.decided_branch(c) {
            Some(true) => recur(t),
            Some(false) => recur(f),
            None => Expr2::If(
                Box::new(recur(c)),
                Box::new(recur(t)),
                Box::new(recur(f)),
                bounds.clone(),
                *loc,
            ),
        },
    }
}

/// Whether a subscript of `expr` computes its index from one of `dims`: an
/// offset (`c[Age - 1]`), whose element is a constant at each element.
fn reads_a_computed_position(
    expr: &crate::ast::Expr2,
    dims: &[crate::dimensions::Dimension],
) -> bool {
    use crate::ast::{Expr2, IndexExpr2};
    fn names_one(expr: &Expr2, dims: &[crate::dimensions::Dimension]) -> bool {
        match expr {
            Expr2::Var(id, ..) => dims
                .iter()
                .any(|d| d.canonical_name().as_str() == id.as_str()),
            Expr2::Op1(_, inner, ..) => names_one(inner, dims),
            Expr2::Op2(_, l, r, ..) => names_one(l, dims) || names_one(r, dims),
            Expr2::If(c, t, f, ..) => {
                names_one(c, dims) || names_one(t, dims) || names_one(f, dims)
            }
            Expr2::App(builtin, ..) => {
                let mut found = false;
                crate::builtins::walk_builtin_expr(builtin, |contents| {
                    if let crate::builtins::BuiltinContents::Expr(e)
                    | crate::builtins::BuiltinContents::LookupTable(e) = contents
                    {
                        found |= names_one(e, dims);
                    }
                });
                found
            }
            Expr2::Const(..) | Expr2::Subscript(..) => false,
        }
    }
    let computes = |e: &Expr2| reads_a_computed_position(e, dims);
    match expr {
        Expr2::Const(..) | Expr2::Var(..) => false,
        Expr2::App(builtin, ..) => {
            let mut found = false;
            crate::builtins::walk_builtin_expr(builtin, |contents| {
                if let crate::builtins::BuiltinContents::Expr(e)
                | crate::builtins::BuiltinContents::LookupTable(e) = contents
                {
                    found |= computes(e);
                }
            });
            found
        }
        Expr2::Subscript(_, indexes, ..) => indexes.iter().any(|index| match index {
            IndexExpr2::Expr(Expr2::Var(..)) => false,
            IndexExpr2::Expr(e) => names_one(e, dims) || computes(e),
            IndexExpr2::Range(lo, hi, _) => computes(lo) || computes(hi),
            IndexExpr2::Wildcard(_) | IndexExpr2::StarRange(..) | IndexExpr2::DimPosition(..) => {
                false
            }
        }),
        Expr2::Op1(_, inner, ..) => computes(inner),
        Expr2::Op2(_, l, r, ..) => computes(l) || computes(r),
        Expr2::If(c, t, f, ..) => computes(c) || computes(t) || computes(f),
    }
}
