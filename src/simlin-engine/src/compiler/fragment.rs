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

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::common::{Canonical, Ident, IdentMap, Result};
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
    /// The variable being lowered, in its `Expr2` form.
    pub target: Variable,
    /// The shape of every name the target can reference, its own included.
    pub deps: IdentMap<Ident<Canonical>, DepShape>,
    /// Graphical-function tables, keyed by the variable that declares them:
    /// the target's own and those of the lookup tables it calls.
    pub tables: HashMap<Ident<Canonical>, Vec<Table>>,
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
        target: Variable,
        deps: IdentMap<Ident<Canonical>, DepShape>,
        tables: HashMap<Ident<Canonical>, Vec<Table>>,
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

    /// The target with its element scope resolved into its body: every read
    /// the scope's element pins to one element spelled as that element's
    /// static index (`Context::pin_element_reads`), and no scope left, so a
    /// describer that classifies reads by their spelling (LTM's reference-site
    /// IR) sees the reads the compiled fragment makes. A target with no scope
    /// is returned as it is.
    pub(crate) fn element_pinned_target(&self) -> Variable {
        let Some(scope) = self.target.element_scope() else {
            return self.target.clone();
        };
        let ctx = self.context(false);
        let Ok((_, elem_ctx, _)) = ctx.element_scope_context(scope) else {
            return self.target.clone();
        };
        let pin = |ast: &Option<crate::ast::Ast<crate::ast::Expr2>>| {
            ast.as_ref().map(|ast| match ast {
                crate::ast::Ast::Scalar(expr) => {
                    crate::ast::Ast::Scalar(elem_ctx.pin_element_reads(expr))
                }
                other => other.clone(),
            })
        };
        let mut pinned = self.target.clone();
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
    Var::new(&input.context(is_initial), &input.target)
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
    let mut extents = VarSizes::new();
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
