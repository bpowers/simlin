// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::Arc;

use crate::ast::{
    self, ArrayView, BinaryOp, Expr3, Expr3LowerContext, IndexExpr3, Loc, TempAllocator,
};
use crate::common::{
    Canonical, CanonicalDimensionName, CanonicalElementName, ErrorCode, ErrorKind, Ident, IdentMap,
    Result, canonicalize,
};
use crate::dimensions::{
    Axis, AxisMatch, Dimension, DimensionsContext, DirectMappingsOnly, SubdimensionRelations,
    axes_of, match_axes_partial,
};
use crate::variable::{ElementScope, VarKind, Variable};
use crate::{Error, sim_err};

use super::dimensions::{UnaryOp, allocate_implicit_axes, axis_reordering};
use super::expr::{BuiltinFn, Expr, SubscriptIndex, VarRef};
use super::fragment::{DepKind, DepShape, ModelShape};
use super::subscript::{
    IndexOp, Subscript3Config, ViewBuildConfig, ViewBuildResult, build_view_from_ops,
    normalize_subscripts3,
};
use crate::builtins::ArgKind;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub(crate) struct Context<'a> {
    pub(crate) core: ContextCore<'a>,
    pub(crate) active_dimension: Option<Arc<[Dimension]>>,
    pub(crate) active_subscript: Option<Vec<CanonicalElementName>>,
    pub(crate) is_initial: bool,
    /// When true, wildcards should always be preserved for iteration (inside SUM, etc.)
    /// rather than being collapsed based on active_dimension matching.
    pub(crate) preserve_wildcards_for_iteration: bool,
    /// When true, ActiveDimRef subscripts are promoted to Wildcard so the full
    /// dimension view is preserved.  This is needed for array-producing builtins
    /// (VectorSortOrder, VectorElmMap, etc.) but NOT for array reducers (SUM,
    /// MEAN, etc.) where ActiveDimRef should resolve to a concrete offset.
    pub(crate) promote_active_dim_ref: bool,
    /// The fragment's temp allocator. [`Context::new`] creates one per
    /// fragment (one `compiler::Var::new` call) and every context derived from
    /// it -- per-element, wildcard-preserving, transposed -- shares it, so each
    /// temp a lowering step materializes is drawn from one sequence. The ids
    /// are dense by construction, and distinct except across the elements of
    /// one plain apply-to-all or arrayed equation, which share a range
    /// (`TempAllocator::element_scopes`).
    pub(crate) temps: Rc<TempAllocator>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy)]
pub(crate) struct ContextCore<'a> {
    pub(crate) dimensions: &'a [Dimension],
    pub(crate) dimensions_ctx: &'a DimensionsContext,
    /// The shape of every name the fragment can reference, the variable being
    /// lowered included. A module dependency's shape carries the sub-model's
    /// fixed layout, which is how a cross-module name resolves.
    pub(crate) deps: &'a IdentMap<Ident<Canonical>, DepShape>,
    /// The extents [`super::fragment::reference_extents`] projects out of
    /// `deps`, so lowering and codegen answer "how big is the variable this
    /// reference addresses?" from one table. Derived, never authored.
    pub(crate) var_sizes: &'a super::VarSizes,
    pub(crate) inputs: &'a BTreeSet<Ident<Canonical>>,
}

/// What a name denotes from inside a fragment (see [`Context::resolve`]):
/// `'d` is the dependency shapes' lifetime, `'n` the resolved name's.
struct Resolved<'d, 'n> {
    shape: &'d DepShape,
    /// The last segment of the name -- the variable itself, as diagnostics
    /// spell it.
    leaf: &'n str,
    /// For a cross-module name, the module instance it relocates through and
    /// the slot its variable's block starts at inside that instance.
    instance: Option<(Ident<Canonical>, usize)>,
}

impl<'a> std::ops::Deref for Context<'a> {
    type Target = ContextCore<'a>;

    fn deref(&self) -> &Self::Target {
        &self.core
    }
}

impl Context<'_> {
    pub(crate) fn new<'a>(core: ContextCore<'a>, is_initial: bool) -> Context<'a> {
        Context {
            core,
            active_dimension: None,
            active_subscript: None,
            is_initial,
            preserve_wildcards_for_iteration: false,
            promote_active_dim_ref: false,
            temps: Rc::new(TempAllocator::default()),
        }
    }

    fn with_active_context(
        &self,
        active_dimension: Option<Arc<[Dimension]>>,
        active_subscript: Option<Vec<CanonicalElementName>>,
    ) -> Self {
        Context {
            core: self.core,
            active_dimension,
            active_subscript,
            is_initial: self.is_initial,
            preserve_wildcards_for_iteration: self.preserve_wildcards_for_iteration,
            promote_active_dim_ref: self.promote_active_dim_ref,
            temps: Rc::clone(&self.temps),
        }
    }

    pub(crate) fn with_active_subscripts<S: AsRef<str>>(
        &self,
        active_dimension: Arc<[Dimension]>,
        subscripts: &[S],
    ) -> Self {
        self.with_active_context(
            Some(active_dimension),
            Some(
                subscripts
                    .iter()
                    .map(|s| CanonicalElementName::from_raw(s.as_ref()))
                    .collect(),
            ),
        )
    }

    /// The context a per-element helper's scalar body is lowered under
    /// (`variable::ElementScope`): the scope's axes resolved against the
    /// project's dimensions, this context with that element active -- the
    /// same context `compiler::expand_per_element` gives the element's own
    /// slot of an apply-to-all equation -- and the element's row-major
    /// position among those axes.
    pub(crate) fn element_scope_context(
        &self,
        scope: &ElementScope,
    ) -> Result<(Vec<Dimension>, Self, usize)> {
        // Both refusals are defensive: the scope's names are the parent's
        // declared dimensions and one of their elements, which the parse has
        // already resolved.
        let dims: Vec<Dimension> = scope
            .dims
            .iter()
            .map(|name| match self.dimensions_ctx.get(name) {
                Some(dim) => Ok(dim.clone()),
                None => sim_err!(BadDimensionName, name.as_str().to_string()),
            })
            .collect::<Result<_>>()?;
        let elements: Vec<&str> = scope.element.iter().map(|e| e.as_str()).collect();
        let Some(index) = dims
            .iter()
            .zip(&scope.element)
            .try_fold(0usize, |acc, (dim, element)| {
                dim.get_offset(element).map(|off| acc * dim.len() + off)
            })
        else {
            return sim_err!(MismatchedDimensions, elements.join(","));
        };
        let elem_ctx =
            self.with_active_subscripts(Arc::<[Dimension]>::from(dims.clone()), &elements);
        Ok((dims, elem_ctx, index))
    }

    /// `expr` with every read the active element resolves to ONE element
    /// rewritten to that element's static index, for a describer that
    /// classifies reads by their spelling rather than lowering them (LTM's
    /// reference-site IR over a per-element helper's body).
    ///
    /// The resolution is this context's own, not a restatement of it: a bare
    /// arrayed name gets the subscripts [`Self::lower_pass0`] gives it, and
    /// each subscript then goes through [`Self::normalize_subscript_ops`] and
    /// [`build_view_from_ops`] exactly as [`Self::lower_subscript`] runs them,
    /// so the index written back is the element the compiled read addresses.
    /// Only an index the ACTIVE ELEMENT resolved (an `IndexOp::ActiveDimRef`)
    /// is rewritten: a literal is already static as written, and a wildcard,
    /// a range or a dimension position stays the axis it spells. Inside an
    /// array-producing builtin's whole-array operand (`ArgKind::Array { whole:
    /// true }`) an active-dimension subscript keeps its axis, so nothing there
    /// is touched. A subscript the compiler cannot resolve statically -- a
    /// dynamic index, an unknown name, a mismatched arity -- is left as
    /// written; the compile of the same body refuses it or lowers it
    /// dynamically, and the describer's conservative reading of the spelling
    /// is the right one for it.
    pub(crate) fn pin_element_reads(&self, expr: &ast::Expr2) -> ast::Expr2 {
        use ast::Expr2;
        match expr {
            Expr2::Var(id, _, loc) if self.dims_of(id).is_some() => {
                // What `lower_pass0` spells a bare arrayed reference as: the
                // reference's bounds are the variable's own axes. The axes come
                // from this fragment's dependency shapes rather than the `Var`'s
                // own bounds because a helper is lowered to `Expr2` without a
                // model (`db::analysis::reconstruct_implicit_variable`), so its
                // bare references carry no bounds; `get_ref` resolves them the
                // same way when the fragment compiles.
                let dims = self.dims_of(id).expect("checked above");
                let bounds = ast::ArrayBounds::Named {
                    name: id.as_str().to_string(),
                    dims: dims.iter().map(|d| d.len()).collect(),
                    dim_names: Some(dims.iter().map(|d| d.name().to_string()).collect()),
                };
                let subscripts = self.make_dimension_subscripts(id, &bounds, *loc);
                let sub_bounds = self.make_subscript_bounds(id, &bounds, &subscripts);
                self.pin_subscript(id, subscripts, sub_bounds, *loc)
            }
            Expr2::Var(..) | Expr2::Const(..) => expr.clone(),
            Expr2::Subscript(id, indices, bounds, loc) => {
                self.pin_subscript(id, indices.clone(), bounds.clone(), *loc)
            }
            Expr2::App(builtin, bounds, loc) => Expr2::App(
                builtin
                    .try_map_ref_with_kinds(|arg, kind| {
                        Ok::<_, std::convert::Infallible>(match kind {
                            ArgKind::Array { whole: true } => arg.clone(),
                            ArgKind::Array { whole: false }
                            | ArgKind::Scalar
                            | ArgKind::Table
                            | ArgKind::Ident => self.pin_element_reads(arg),
                        })
                    })
                    .unwrap(),
                bounds.clone(),
                *loc,
            ),
            Expr2::Op1(op, inner, bounds, loc) => Expr2::Op1(
                *op,
                Box::new(self.pin_element_reads(inner)),
                bounds.clone(),
                *loc,
            ),
            Expr2::Op2(op, l, r, bounds, loc) => Expr2::Op2(
                *op,
                Box::new(self.pin_element_reads(l)),
                Box::new(self.pin_element_reads(r)),
                bounds.clone(),
                *loc,
            ),
            Expr2::If(c, t, f, bounds, loc) => Expr2::If(
                Box::new(self.pin_element_reads(c)),
                Box::new(self.pin_element_reads(t)),
                Box::new(self.pin_element_reads(f)),
                bounds.clone(),
                *loc,
            ),
        }
    }

    /// [`Self::pin_element_reads`] for one subscript: the nested index
    /// expressions pinned first, then every index the active element resolves
    /// replaced by the 1-based position it resolved to.
    fn pin_subscript(
        &self,
        id: &Ident<Canonical>,
        indices: Vec<ast::IndexExpr2>,
        bounds: Option<ast::ArrayBounds>,
        loc: Loc,
    ) -> ast::Expr2 {
        use ast::{Expr2, IndexExpr2};
        let indices: Vec<IndexExpr2> = indices
            .into_iter()
            .map(|idx| match idx {
                IndexExpr2::Expr(e) => IndexExpr2::Expr(self.pin_element_reads(&e)),
                IndexExpr2::Range(l, r, range_loc) => IndexExpr2::Range(
                    self.pin_element_reads(&l),
                    self.pin_element_reads(&r),
                    range_loc,
                ),
                other => other,
            })
            .collect();
        let indices = match self.static_element_indices(id, &indices) {
            Some(resolved) => indices
                .into_iter()
                .zip(resolved)
                .map(|(idx, element)| match element {
                    Some(element) => IndexExpr2::Expr(Expr2::Const(
                        (element + 1).to_string(),
                        ast::Literal::new((element + 1) as f64),
                        loc,
                    )),
                    None => idx,
                })
                .collect(),
            None => indices,
        };
        Expr2::Subscript(id.clone(), indices, bounds, loc)
    }

    /// Per index of `id[indices]`, the 0-based element the active element
    /// resolves it to, or `None` for an index left as written; `None` overall
    /// when the subscript is not one the compiler resolves statically.
    fn static_element_indices(
        &self,
        id: &Ident<Canonical>,
        indices: &[ast::IndexExpr2],
    ) -> Option<Vec<Option<usize>>> {
        let dims = self.subscript_dims(id).ok()?;
        if indices.len() != dims.len() {
            return None;
        }
        let indices3: Vec<IndexExpr3> = indices
            .iter()
            .enumerate()
            .map(|(i, idx)| IndexExpr3::from_index_expr2(idx, dims.get(i), self).ok())
            .collect::<Option<_>>()?;
        let ops = self.normalize_subscript_ops(id, &indices3, dims).ok()??;
        let orig_dims: Vec<usize> = dims.iter().map(|d| d.len()).collect();
        let strides = Self::row_major_strides(&orig_dims);
        let built =
            build_view_from_ops(&ops, &orig_dims, &strides, &self.view_config(dims)).ok()?;
        Some(
            ops.iter()
                .zip(&built.single_indices)
                .map(|(op, single)| matches!(op, IndexOp::ActiveDimRef(_)).then_some(*single))
                .collect(),
        )
    }

    /// The reference `ident` denotes in the model being compiled, resolving an
    /// arrayed variable's *implicit* subscripts (the active A2A element) to an
    /// element offset.
    pub(super) fn get_ref(&self, ident: &Ident<Canonical>) -> Result<VarRef> {
        self.var_ref(ident, false)
    }

    /// `get_base_ref` ignores arrays -- it yields the variable's first slot --
    /// and should only be used from `Var::new` and `Expr::Subscript`, where the
    /// element is selected by an explicit view or index expression instead.
    pub(super) fn get_base_ref(&self, ident: &Ident<Canonical>) -> Result<VarRef> {
        self.var_ref(ident, true)
    }

    /// The shape `ident` denotes, following a `·`-qualified name into the
    /// module dependency's sub-model shape (and on through nested instances).
    pub(super) fn shape_of(&self, ident: &Ident<Canonical>) -> Result<&DepShape> {
        Ok(self.resolve(ident)?.shape)
    }

    /// The dimensions of the variable `ident` denotes, or `None` when it is a
    /// scalar, a module instance, or not a name this fragment can reference.
    fn dims_of(&self, ident: &Ident<Canonical>) -> Option<&[Dimension]> {
        self.shape_of(ident).ok().and_then(DepShape::dimensions)
    }

    /// The active subscript each of `dims` reads, for a subscript-less arrayed
    /// reference inside an apply-to-all body.
    ///
    /// The axis ALLOCATION -- which active axis supplies which of `dims` -- is
    /// `dimensions::allocate_implicit_axes`, shared with the LTM per-element
    /// projection so a link-score pin cannot spell a row this reference does not
    /// read. See that function for the two properties (positional, one-to-one)
    /// that a name-keyed re-derivation gets wrong.
    ///
    /// **Which references arrive here** -- worth stating, because the obvious
    /// guess is wrong and a GH #996 investigation lost time to it. A bare
    /// arrayed reference in an EQUATION BODY does NOT: [`Self::lower_pass0`]
    /// rewrites it into an explicit `Expr2::Subscript` before Expr3, so it
    /// resolves through the subscript path and never reaches `var_ref`'s
    /// arrayed branch. The invariant, measured by tagging each call with its
    /// caller and running the whole lib suite: **exactly two production
    /// callers, and both are wiring rather than expressions.** ZERO calls
    /// arrive via `lower_from_expr3`, which is the one that would mean an
    /// ordinary equation reference.
    ///
    /// - [`Self::fold_flows`] -- a stock's inflow/outflow references;
    /// - `compiler::Var::new` -- the stock self-reference inside
    ///   `build_stock_update_expr`, plus module input wiring.
    ///
    /// The counts, since they are only reproducible with the condition
    /// attached (`cargo test -p simlin-engine --lib -- --nocapture
    /// --test-threads=1`, which `--nocapture` is required for -- without it
    /// stderr is captured and the measurement reads zero): 477 + 438 = 915
    /// production calls on the current tree, or 441 + 402 = 843 when
    /// `crate::mapped_reference_semantics_tests` is skipped, since that module
    /// adds calls of its own. One further call in either condition comes from
    /// `test_get_implicit_subscript_off_translates_through_mapping_parent`,
    /// which invokes [`Self::get_implicit_subscript_off`] directly.
    ///
    /// That split is why the flow reference is the one subscript-less spelling
    /// that can follow an explicit element map (through the
    /// `translate_via_mapping` fallback in
    /// [`Self::get_implicit_subscript_off`], though only when the source
    /// dimension does not already contain an element of the same NAME) while
    /// the bare in-equation one is positional; both halves are pinned in
    /// `crate::mapped_reference_semantics_tests`. It is also why the GH #996
    /// hazard fixture there is built from a two-axis FLOW under a stock: no
    /// ordinary expression can reach this allocation at all.
    fn get_implicit_subscripts(&self, dims: &[Dimension], ident: &str) -> Result<Vec<&str>> {
        if self.active_dimension.is_none() {
            return sim_err!(ArrayReferenceNeedsExplicitSubscripts, ident.to_owned());
        }
        let active_dims = self.active_dimension.as_ref().unwrap();
        let active_subscripts = self.active_subscript.as_ref().unwrap();
        assert_eq!(active_dims.len(), active_subscripts.len());

        match allocate_implicit_axes(dims, active_dims, self.dimensions_ctx) {
            Some(alloc) => Ok(alloc
                .into_iter()
                .map(|i| active_subscripts[i].as_str())
                .collect()),
            None => sim_err!(MismatchedDimensions, ident.to_owned()),
        }
    }

    fn get_implicit_subscript_off(&self, dims: &[Dimension], ident: &str) -> Result<usize> {
        let subscripts = self.get_implicit_subscripts(dims, ident)?;
        let active_dims = self.active_dimension.as_ref().unwrap();

        let mut off = 0_usize;
        for (dim, subscript) in dims.iter().zip(subscripts) {
            let element = CanonicalElementName::from_raw(subscript);
            // The subscript comes from an active dimension; which element of
            // THIS source axis it selects is
            // `DimensionsContext::resolve_mapped_read` (GH #997) -- name
            // first, then the declared mapping, then a mapped parent of the
            // active subdimension.
            //
            // The `get_offset` guard is the SEARCH half and stays here: with
            // several active dimensions, only the one that owns this element
            // can supply it, and the shared rule is per (source axis, active
            // dimension) pair rather than a search over candidates. At least
            // one active dimension always passes it -- `get_implicit_subscripts`
            // returns active SUBSCRIPTS, each an element of its own active
            // dimension -- so the shared rule's name-first arm is reached for
            // every element, exactly as the un-guarded `dim.get_offset` that
            // used to precede this loop was.
            //
            // One behaviour widened when the loop became a `find_map`: a
            // translation that resolves to an element this axis does not
            // declare (a malformed element map) used to abort the whole
            // resolution, and now falls through to the remaining active
            // dimensions. That can only resolve a reference that previously
            // failed to compile.
            let element_off = active_dims.iter().find_map(|active_dim| {
                active_dim.get_offset(&element)?;
                let resolved = self
                    .dimensions_ctx
                    .resolve_mapped_read(dim, active_dim, &element)?;
                dim.get_offset(&resolved)
            });
            let element_off = element_off.ok_or_else(|| {
                crate::Error::new(
                    ErrorKind::Model,
                    ErrorCode::MismatchedDimensions,
                    Some(format!(
                        "cannot resolve subscript '{}' for dimension '{}' on variable '{}'",
                        subscript,
                        dim.name(),
                        ident
                    )),
                )
            })?;
            off = off * dim.len() + element_off;
        }

        Ok(off)
    }

    /// Convert a dimension + subscript to its 1-based index value.
    /// For indexed dimensions (Dim(5)), the subscript is a numeric string like "3".
    /// For named dimensions (Cities{A,B,C}), the subscript is an element name like "B",
    /// and we return its position + 1.
    fn subscript_to_index(dim: &Dimension, subscript: &CanonicalElementName) -> f64 {
        match dim {
            Dimension::Indexed(_, _) => {
                // For indexed dimensions, the subscript is already a 1-based index
                // stored as a string (e.g., "3" means the third element).
                subscript.as_str().parse::<f64>().unwrap_or(1.0)
            }
            Dimension::Named(_, named_dim) => {
                // For named dimensions, find the element's position using O(1) hash lookup
                // get_element_index returns 0-based, so add 1 for 1-based subscript offset
                named_dim
                    .get_element_index(subscript.as_str())
                    .map(|off| (off + 1) as f64)
                    .unwrap_or(1.0)
            }
        }
    }

    /// Resolve `ident` -- a plain name, or a `·`-qualified cross-module name
    /// `m·x` / `m·n·x` -- to the shape it denotes and, for a cross-module name,
    /// to the instance it relocates through.
    ///
    /// A plain name is looked up in the fragment's dependency shapes. A
    /// qualified name's first segment must be a module dependency; each
    /// further segment is looked up in the current sub-model shape, accumulating
    /// the slot offset at which that variable's block starts inside the
    /// instance, and a segment that is followed by another must itself be a
    /// nested module instance. A name that leaves that chain at any point --
    /// the head is not a module dependency (the module variable may have been
    /// deleted while dependent equations still reference `module.output`), a
    /// segment names no sub-model variable -- is a loud `DoesNotExist`, never a
    /// silent slot 0.
    fn resolve<'d, 'n>(&'d self, ident: &'n Ident<Canonical>) -> Result<Resolved<'d, 'n>> {
        let does_not_exist = || Error::new(ErrorKind::Simulation, ErrorCode::DoesNotExist, None);
        let ident_str = ident.as_str();
        let Some(pos) = ident_str.find('\u{00B7}') else {
            let shape = self.deps.get(ident).ok_or_else(does_not_exist)?;
            return Ok(Resolved {
                shape,
                leaf: ident_str,
                instance: None,
            });
        };
        let head = &ident_str[..pos];
        let mut rest = &ident_str[pos + '\u{00B7}'.len_utf8()..];
        let mut model: &'d ModelShape = match &self.deps.get(head).ok_or_else(does_not_exist)?.kind
        {
            DepKind::Module { shape } => shape,
            DepKind::Var => return Err(does_not_exist()),
        };
        let mut base = 0;
        loop {
            let (segment, tail) = match rest.find('\u{00B7}') {
                Some(p) => (&rest[..p], Some(&rest[p + '\u{00B7}'.len_utf8()..])),
                None => (rest, None),
            };
            let entry = model.vars.get(segment).ok_or_else(does_not_exist)?;
            base += entry.offset;
            match tail {
                None => {
                    return Ok(Resolved {
                        shape: &entry.shape,
                        leaf: segment,
                        instance: Some((Ident::from_str_unchecked(head), base)),
                    });
                }
                Some(tail) => {
                    let DepKind::Module { shape: nested } = &entry.shape.kind else {
                        return Err(does_not_exist());
                    };
                    model = nested.as_ref();
                    rest = tail;
                }
            }
        }
    }

    /// Resolve `ident` to a layout-independent [`VarRef`] in the model being
    /// compiled.
    ///
    /// A plain name resolves to itself, so the reference is just
    /// `(ident, implicit element offset)` -- the model's own layout is never
    /// consulted. A cross-module name `m·x` resolves to the *module* variable
    /// `m`, because the enclosing model's layout has one entry spanning the
    /// whole sub-model instance and none for `m·x`; the element offset is `x`'s
    /// position inside that block, which the sub-model's already-fixed layout
    /// (the module dependency's shape) supplies. That is the one place a
    /// concrete offset is computed during lowering, and it is sound because a
    /// sub-model's layout is fixed (`db::compute_layout`) before any fragment
    /// of the parent compiles: the parent relocates the whole block through
    /// the module variable's name.
    fn var_ref(&self, ident: &Ident<Canonical>, ignore_arrays: bool) -> Result<VarRef> {
        let resolved = self.resolve(ident)?;
        let (name, base) = match resolved.instance {
            Some((instance, base)) => (instance, base),
            None => (ident.clone(), 0),
        };
        if ignore_arrays {
            return Ok(VarRef::new(name, base));
        }
        match resolved.shape.dimensions() {
            Some(dims) => {
                let off = self.get_implicit_subscript_off(dims, resolved.leaf)?;
                Ok(VarRef::new(name, base + off))
            }
            None => Ok(VarRef::new(name, base)),
        }
    }

    /// Pass 0: Structural lowering - expands bare array variable references.
    ///
    /// Transforms `Expr2::Var` with ArrayBounds into `Expr2::Subscript` with
    /// dimension name subscripts. This ensures:
    /// 1. Subsequent phases can treat all Var nodes as scalars
    /// 2. Dimension bindings are explicit for A2A processing
    /// 3. Dimension reordering works correctly
    pub(super) fn lower_pass0(&self, expr: &ast::Expr2) -> ast::Expr2 {
        match expr {
            ast::Expr2::Var(id, Some(bounds), loc) => {
                // Expand bare array variable to Subscript with dimension name subscripts
                let subscripts = self.make_dimension_subscripts(id, bounds, *loc);
                let subscript_bounds = self.make_subscript_bounds(id, bounds, &subscripts);
                ast::Expr2::Subscript(id.clone(), subscripts, subscript_bounds, *loc)
            }
            ast::Expr2::Var(_, None, _) => expr.clone(), // Scalar - unchanged
            ast::Expr2::Const(_, _, _) => expr.clone(),
            ast::Expr2::Subscript(id, args, bounds, loc) => {
                // Recursively process expressions inside subscripts
                let new_args: Vec<ast::IndexExpr2> = args
                    .iter()
                    .map(|arg| self.lower_pass0_index_expr(arg))
                    .collect();
                ast::Expr2::Subscript(id.clone(), new_args, bounds.clone(), *loc)
            }
            ast::Expr2::Op1(op, inner, bounds, loc) => {
                ast::Expr2::Op1(*op, Box::new(self.lower_pass0(inner)), bounds.clone(), *loc)
            }
            ast::Expr2::Op2(op, left, right, bounds, loc) => ast::Expr2::Op2(
                *op,
                Box::new(self.lower_pass0(left)),
                Box::new(self.lower_pass0(right)),
                bounds.clone(),
                *loc,
            ),
            ast::Expr2::If(cond, then_branch, else_branch, bounds, loc) => ast::Expr2::If(
                Box::new(self.lower_pass0(cond)),
                Box::new(self.lower_pass0(then_branch)),
                Box::new(self.lower_pass0(else_branch)),
                bounds.clone(),
                *loc,
            ),
            ast::Expr2::App(builtin, bounds, loc) => ast::Expr2::App(
                builtin.map_ref(|arg| self.lower_pass0(arg)),
                bounds.clone(),
                *loc,
            ),
        }
    }

    /// Create dimension name subscripts from ArrayBounds.
    ///
    /// For each dimension in bounds:
    /// - If the dimension is in the active set, use a dimension name subscript
    ///   (creates proper A2A binding via ActiveDimRef)
    /// - If the dimension is NOT in the active set, use a wildcard
    ///   (needed for reductions like SUM where we iterate over non-active dims)
    ///
    /// This handles:
    /// - Full A2A: result[A,B] = source where source is [A,B] -> source[A,B]
    /// - Partial reduction: result[A] = SUM(source) where source is [A,B] -> SUM(source[A,*])
    /// - Full reduction: total = SUM(source) where source is [A,B] -> SUM(source[*,*])
    fn make_dimension_subscripts(
        &self,
        ident: &Ident<Canonical>,
        bounds: &ast::ArrayBounds,
        loc: Loc,
    ) -> Vec<ast::IndexExpr2> {
        // Get the source dimensions (from the dependency's shape or bounds)
        let source_dims: Option<Vec<Dimension>> = self.dims_of(ident).map(|dims| dims.to_vec());

        let Some(source_dims) = source_dims else {
            return bounds
                .dims()
                .iter()
                .map(|_| ast::IndexExpr2::Wildcard(loc))
                .collect();
        };

        // If we have active dimensions, use the unified dimension matching algorithm
        let Some(active_dims) = self.active_dimension.as_ref() else {
            // No active dimensions (not in A2A context) - use wildcards
            return source_dims
                .iter()
                .map(|_| ast::IndexExpr2::Wildcard(loc))
                .collect();
        };

        // `dimensions::match_axes_partial` is the one axis-matching precedence.
        // The PARTIAL answer is what this position wants: a reduction such as
        // `SUM(source[A,B])` under an `[A]` target leaves `B` unmatched on
        // purpose, and an unmatched axis becomes the wildcard the reducer
        // iterates.
        //
        // Only the DIRECT mappings: what this function can emit for a matched
        // axis is a dimension-name subscript, and that resolves to the active
        // dimension's ORDINAL (GH #527 / #997). The ordinal read is the
        // documented rule for a directly mapped pair; for a mapping onto a
        // PARENT of the target it is not, and the wildcard this leaves instead
        // keeps `resolve_iteration_element`'s element-name-and-mapping
        // resolution, which reads the mapped element.
        let source_to_target = match_axes_partial(
            &axes_of(&source_dims),
            &axes_of(active_dims),
            &DirectMappingsOnly(self.dimensions_ctx),
        );

        source_dims
            .iter()
            .enumerate()
            .map(|(source_idx, _source_dim)| {
                if let Some((target_idx, _)) = source_to_target[source_idx].as_ref() {
                    let active_dim = &active_dims[*target_idx];
                    // Create a dimension reference to the matched active dimension
                    ast::IndexExpr2::Expr(ast::Expr2::Var(Ident::new(active_dim.name()), None, loc))
                } else {
                    // Source dimension didn't match any active dimension - use wildcard
                    // (needed for reductions like SUM where we iterate over non-matched dims)
                    ast::IndexExpr2::Wildcard(loc)
                }
            })
            .collect()
    }

    fn make_subscript_bounds(
        &self,
        ident: &Ident<Canonical>,
        bounds: &ast::ArrayBounds,
        subscripts: &[ast::IndexExpr2],
    ) -> Option<ast::ArrayBounds> {
        let dims = self.dims_of(ident)?;

        let mut result_dims = Vec::new();
        let mut result_dim_names = Vec::new();

        for (i, subscript) in subscripts.iter().enumerate() {
            match subscript {
                ast::IndexExpr2::Wildcard(_) | ast::IndexExpr2::Range(_, _, _) => {
                    result_dims.push(dims[i].len());
                    result_dim_names.push(dims[i].name().to_string());
                }
                ast::IndexExpr2::StarRange(subdim_name, _) => {
                    let len = self
                        .dimensions_ctx
                        .get(subdim_name)
                        .map(|dim| dim.len())
                        .unwrap_or_else(|| dims[i].len());
                    result_dims.push(len);
                    result_dim_names.push(subdim_name.as_str().to_string());
                }
                ast::IndexExpr2::Expr(_) | ast::IndexExpr2::DimPosition(_, _) => {}
            }
        }

        if result_dims.is_empty() {
            return None;
        }

        let dim_names = Some(result_dim_names);
        match bounds {
            ast::ArrayBounds::Named { name, .. } => Some(ast::ArrayBounds::Named {
                name: name.clone(),
                dims: result_dims,
                dim_names,
            }),
            ast::ArrayBounds::Temp { id, .. } => Some(ast::ArrayBounds::Temp {
                id: *id,
                dims: result_dims,
                dim_names,
            }),
        }
    }

    /// Recursively process index expressions
    fn lower_pass0_index_expr(&self, expr: &ast::IndexExpr2) -> ast::IndexExpr2 {
        match expr {
            ast::IndexExpr2::Expr(inner) => ast::IndexExpr2::Expr(self.lower_pass0(inner)),
            ast::IndexExpr2::Range(start, end, loc) => {
                ast::IndexExpr2::Range(self.lower_pass0(start), self.lower_pass0(end), *loc)
            }
            // Wildcard, StarRange, DimPosition remain unchanged
            ast::IndexExpr2::Wildcard(_)
            | ast::IndexExpr2::StarRange(_, _)
            | ast::IndexExpr2::DimPosition(_, _) => expr.clone(),
        }
    }

    /// Lower an `Expr2` to the compiler's `Expr`: pass 0, then `Expr3`, then
    /// `lower_from_expr3`.
    ///
    /// The lowering is structural. It materializes nothing: an expression whose
    /// value codegen cannot express in place becomes a temp afterwards, in
    /// [`super::array_operand`], which is the fragment's one materialization
    /// pass and sees every element's lowered form at once. That is why a
    /// subscript naming an active apply-to-all dimension stays an
    /// `IndexExpr3::Dimension` all the way to
    /// [`super::subscript::normalize_subscripts3`], which allocates one active
    /// axis per occurrence: an array-producing builtin's wildcard-preserving
    /// context can then promote it back to a whole axis, and an ordinary
    /// reference resolves it to the active element.
    pub(super) fn lower(&self, expr: &ast::Expr2) -> Result<Expr> {
        // Pass 0: normalize bare arrays into explicit subscripts.
        let normalized = self.lower_pass0(expr);

        // Expr3: wildcard resolution and dimension detection.
        // Carry the reason across, never the span: an `Error` renders its
        // `details` to the user as prose, and a span rendered as prose ("Error
        // at 8:10") displaces the sentence the raising site wrote.
        let expr3 = Expr3::from_expr2(&normalized, self).map_err(|e| Error {
            kind: ErrorKind::Model,
            code: e.code,
            details: e.details,
        })?;

        self.lower_from_expr3(&expr3)
    }

    pub(super) fn fold_flows(&self, flows: &[Ident<Canonical>]) -> Result<Option<Expr>> {
        if flows.is_empty() {
            return Ok(None);
        }

        let loads: Result<Vec<Expr>> = flows
            .iter()
            .map(|flow| self.get_ref(flow).map(|var| Expr::Var(var, Loc::default())))
            .collect();
        let mut loads = loads?.into_iter();

        let first = loads.next().unwrap();
        Ok(Some(loads.fold(first, |acc, flow| {
            Expr::Op2(BinaryOp::Add, Box::new(acc), Box::new(flow), Loc::default())
        })))
    }

    /// Apply dimension reordering to an expression
    fn apply_dimension_reordering(
        &self,
        expr: Expr,
        reordering: Vec<usize>,
        loc: Loc,
    ) -> Result<Expr> {
        // The reordering vector contains 0-based indices indicating the new position of each dimension
        // For example, [1, 0] means swap dimensions (transpose for 2D)
        // [1, 2, 0] means the first output dim is the second input dim, etc.

        // Check if this is a simple variable or static subscript that we can reorder directly
        match &expr {
            Expr::Var(var, _) => {
                // This is a bare array variable - create a StaticSubscript with reordered view
                // First, get the variable's declared dimensions
                if let Some(dims) = self.whole_var_dims(var) {
                    let orig_dims: Vec<usize> = dims.iter().map(|d| d.len()).collect();
                    let orig_dim_names: Vec<String> =
                        dims.iter().map(|d| d.name().to_string()).collect();

                    // Create a contiguous view with names and apply reordering
                    let view = ArrayView::contiguous_with_names(orig_dims, orig_dim_names);
                    return Ok(Expr::StaticSubscript(
                        var.clone(),
                        view.reorder_dimensions(&reordering),
                        loc,
                    ));
                }
            }
            Expr::StaticSubscript(var, view, _) => {
                // Apply reordering to existing view
                return Ok(Expr::StaticSubscript(
                    var.clone(),
                    view.reorder_dimensions(&reordering),
                    loc,
                ));
            }
            _ => {}
        }

        // For other expressions, fall back to transpose for 2D
        if reordering.len() == 2 && reordering == vec![1, 0] {
            // This is a simple transpose
            Ok(Expr::Op1(UnaryOp::Transpose, Box::new(expr), loc))
        } else {
            // For more complex reordering, we'd need to create a view with reordered strides
            // For now, just return the expression unchanged
            // TODO: Implement general dimension reordering
            Ok(expr)
        }
    }

    /// The declared dimensions of the variable a reference addresses in
    /// *whole*.
    ///
    /// `None` for a reference into the middle of a variable: a mid-variable
    /// reference names an element, not the array, so neither the array's
    /// dimensions nor its extent apply to it.
    ///
    /// The sole reader is [`Self::apply_dimension_reordering`], which wants a
    /// bare array reference's declared DIMENSIONS. A CROSS-MODULE reference
    /// names the module instance here, whose shape declares no dimensions, so
    /// the reorder falls through to its generic path rather than specializing
    /// -- correct, if unspecialized. Do not reach for this to answer a question
    /// about a reference's EXTENT: [`super::VarSizes`] is where that lives,
    /// precisely because it resolves the cross-module case.
    fn whole_var_dims(&self, var: &VarRef) -> Option<&[Dimension]> {
        if !var.is_whole_var() {
            return None;
        }
        self.deps.get(&var.name)?.dimensions()
    }

    /// Full element count of the variable `base` addresses in whole. `None` for
    /// a reference that does not start at a variable's base. Used by the GH #578
    /// scalar-source VECTOR ELM MAP fold to bound the per-element static read.
    ///
    /// This reads the SAME table `codegen::full_source_len` reads, which is the
    /// point: the fold and the opcode must agree about where a source's storage
    /// ends, and they are on opposite sides of lowering. Answering from the
    /// variable's own declared dimensions -- what this did before -- silently
    /// reported 1 for a cross-module source, whose dependency shape here is the
    /// dimensionless module instance rather than the sub-model variable the
    /// reference actually names.
    pub(super) fn full_var_len_for_base(&self, base: &VarRef) -> Option<usize> {
        self.var_sizes.get(base).copied()
    }

    pub(super) fn build_stock_update_expr(&self, stock: &VarRef, var: &Variable) -> Result<Expr> {
        if let VarKind::Stock {
            inflows, outflows, ..
        } = &var.kind
        {
            let inflows = self
                .fold_flows(inflows)?
                .unwrap_or(Expr::Const(0.0, Loc::default()));
            let outflows = self
                .fold_flows(outflows)?
                .unwrap_or(Expr::Const(0.0, Loc::default()));

            let dt_update = Expr::Op2(
                BinaryOp::Mul,
                Box::new(Expr::Op2(
                    BinaryOp::Sub,
                    Box::new(inflows),
                    Box::new(outflows),
                    Loc::default(),
                )),
                Box::new(Expr::Dt(Loc::default())),
                Loc::default(),
            );

            Ok(Expr::Op2(
                BinaryOp::Add,
                Box::new(Expr::Var(stock.clone(), Loc::default())),
                Box::new(dt_update),
                Loc::default(),
            ))
        } else {
            unreachable!(
                "build_stock_update_expr called with non-stock {}",
                var.ident()
            );
        }
    }
}

// Implement Expr3LowerContext for Context to enable Expr2 -> Expr3 conversion
impl Expr3LowerContext for Context<'_> {
    /// Resolved through [`Self::shape_of`], so a CROSS-MODULE name (`m·arr`)
    /// reports the sub-model variable's dimensions rather than nothing: a
    /// `None` there would make `Expr3::from_expr2` treat every cross-module
    /// array as a scalar and reject a WILDCARD subscript on one
    /// (`SUM(m.arr[*])`) as `CantSubscriptScalar`. `ast::ArrayContext::
    /// get_dimensions`, the Expr2-stage twin of this trait method, follows the
    /// module variable into the sub-model the same way; this is the same rule,
    /// one stage later.
    fn get_dimensions(&self, ident: &str) -> Option<&[Dimension]> {
        let canonical = canonicalize(ident);
        // A plain name is its own key, so the direct lookup answers it without
        // interning an `Ident`; only a name the fragment does not hold -- every
        // dotted cross-module one among them -- pays for the module-following
        // resolution. The two agree on a plain name by construction: `resolve`
        // reduces to this same lookup when there is no module separator to
        // follow.
        match self.deps.get(&*canonical) {
            Some(shape) => shape.dimensions(),
            None => self.dims_of(&Ident::from_unchecked(canonical.into_owned())),
        }
    }

    /// Only `ident` needs canonicalizing: a `Dimension`'s name is a
    /// `CanonicalDimensionName`, canonical by construction at every site that
    /// builds one (`dimensions::dimension_name_is_canonical_for_every_constructor`
    /// pins that), so canonicalizing it again could not change it -- and
    /// `canonicalize` still scans the whole string to decide that.
    ///
    /// This runs once per bare-identifier subscript per reference
    /// (`ast::expr3::IndexExpr3::from_index_expr2`), so the redundant scan was
    /// paid once per DECLARED DIMENSION per subscript. On a model with 126
    /// dimensions it was half of every `canonicalize` call the compiler made
    /// and ~5% of a whole compile.
    fn is_dimension_name(&self, ident: &str) -> bool {
        let canonical = canonicalize(ident);
        self.dimensions.iter().any(|dim| dim.name() == &*canonical)
    }
}

impl Context<'_> {
    /// Create a context with transposed active dimensions for transpose operations.
    /// Used when processing expressions under a Transpose operator in A2A context.
    fn with_transposed_active_context(&self) -> Self {
        let reversed_dims = self.active_dimension.as_ref().map(|active_dims| {
            let mut reversed: Vec<Dimension> = active_dims.iter().cloned().collect();
            reversed.reverse();
            Arc::<[Dimension]>::from(reversed)
        });
        let reversed_subscripts = self.active_subscript.as_ref().map(|active_subs| {
            let mut reversed = active_subs.clone();
            reversed.reverse();
            reversed
        });
        self.with_active_context(reversed_dims, reversed_subscripts)
    }

    /// Create a context that preserves wildcards for array iteration.
    /// Used for array reducer builtins (SUM, MAX, MIN, MEAN, STDDEV, SIZE).
    /// ActiveDimRef subscripts are NOT promoted -- they resolve to a concrete
    /// element offset, so `SUM(matrix[DimA, DimB])` sums over one dimension
    /// while the other iterates.
    fn with_preserved_wildcards(&self) -> Self {
        Context {
            core: self.core,
            active_dimension: self.active_dimension.clone(),
            active_subscript: self.active_subscript.clone(),
            is_initial: self.is_initial,
            preserve_wildcards_for_iteration: true,
            promote_active_dim_ref: false,
            temps: Rc::clone(&self.temps),
        }
    }

    /// Create a context for array-producing vector builtins (VectorSortOrder,
    /// VectorElmMap, VectorSelect, AllocateAvailable).  Like
    /// `with_preserved_wildcards`, but also promotes ActiveDimRef to Wildcard so
    /// references like `vals[DimA]` inside these builtins keep their full array
    /// view.
    fn with_vector_builtin_wildcards(&self) -> Self {
        Context {
            core: self.core,
            active_dimension: self.active_dimension.clone(),
            active_subscript: self.active_subscript.clone(),
            is_initial: self.is_initial,
            preserve_wildcards_for_iteration: true,
            promote_active_dim_ref: true,
            temps: Rc::clone(&self.temps),
        }
    }

    /// Lower an `Expr3` to the compiler's `Expr`, one arm per variant.
    ///
    /// Structural throughout: an expression whose value codegen cannot produce
    /// in place becomes a temp afterwards, in `super::array_operand`.
    pub(super) fn lower_from_expr3(&self, expr: &Expr3) -> Result<Expr> {
        match expr {
            Expr3::Const(_, n, loc) => Ok(Expr::Const(n.value(), *loc)),

            Expr3::Var(id, _, loc) => {
                // A `dimensions::Dimension`'s name is canonical by
                // construction and so is `id`, so neither side needs
                // re-canonicalizing to be compared.
                let is_dimension = self.dimensions.iter().any(|dim| dim.name() == id.as_str());

                if is_dimension {
                    // A dimension name USED AS A VALUE is the active element's
                    // 1-based position along that dimension.
                    if let Some(active_dims) = &self.active_dimension {
                        if let Some(active_subscripts) = &self.active_subscript {
                            // Which active axis the name stands for is
                            // `dimensions::match_axes_partial`, the one
                            // precedence, over the single named axis. Only the
                            // DIRECT mappings, because what this produces is an
                            // index into the NAMED dimension translated from the
                            // active element through that mapping, and the
                            // indirect correspondences have no such translation.
                            let matched = match_axes_partial(
                                &[Axis::named(id.as_str(), 0)],
                                &axes_of(active_dims),
                                &DirectMappingsOnly(self.dimensions_ctx),
                            )
                            .into_iter()
                            .next()
                            .flatten();

                            match matched {
                                Some((active_idx, AxisMatch::Exact)) => {
                                    let index = Self::subscript_to_index(
                                        &active_dims[active_idx],
                                        &active_subscripts[active_idx],
                                    );
                                    return Ok(Expr::Const(index, *loc));
                                }
                                // e.g. `s[DimA] = DimB` where DimB -> DimA.
                                // Translate to the position in the REFERENCED
                                // dimension rather than the active one, which is
                                // what a reordered element map makes different.
                                Some((active_idx, AxisMatch::Mapped { .. })) => {
                                    let id_dim_name = CanonicalDimensionName::from_raw(id.as_str());
                                    let active_dim = &active_dims[active_idx];
                                    let subscript = &active_subscripts[active_idx];
                                    if let Some(translated) =
                                        self.dimensions_ctx.translate_via_mapping(
                                            &id_dim_name,
                                            active_dim.canonical_name(),
                                            subscript,
                                        )
                                        && let Some(id_dim) = self.dimensions_ctx.get(&id_dim_name)
                                    {
                                        let index = Self::subscript_to_index(id_dim, &translated);
                                        return Ok(Expr::Const(index, *loc));
                                    }
                                    return Err(Error::new(
                                        ErrorKind::Model,
                                        ErrorCode::Generic,
                                        Some(format!(
                                            "dimension mapping between '{}' and '{}' exists but could not translate subscript '{}'",
                                            id_dim_name,
                                            active_dim.name(),
                                            subscript
                                        )),
                                    ));
                                }
                                // No active axis supplies it; fall through to
                                // the module-input and variable lookups below.
                                _ => {}
                            }
                        }
                    } else {
                        // We're in a scalar context but trying to use a dimension name
                        return Err(Error {
                            kind: ErrorKind::Model,
                            code: ErrorCode::DimensionInScalarContext,
                            details: Some(format!(
                                "Dimension '{id}' cannot be used in a scalar equation"
                            )),
                        });
                    }
                }

                // Not a dimension, check if it's a module input
                if let Some((off, _)) = self
                    .inputs
                    .iter()
                    .enumerate()
                    .find(|(_, input)| id.as_str() == input.as_str())
                {
                    return Ok(Expr::ModuleInput(off, *loc));
                }

                // Check if it's a regular variable
                match self.get_ref(id) {
                    Ok(var) => Ok(Expr::Var(var, *loc)),
                    Err(err) => {
                        // If get_offset fails because it's an array without implicit subscripts,
                        // try to create a full array view
                        if matches!(err.code, ErrorCode::ArrayReferenceNeedsExplicitSubscripts)
                            && let Some(source_dims) = self.dims_of(id)
                        {
                            // This is an array variable - check if we need dimension reordering
                            let base = self.get_base_ref(id)?;

                            // Check if we're in an A2A context and need to reorder dimensions
                            if let Some(target_dims) = &self.active_dimension {
                                // Get dimension names
                                let source_dim_names: Vec<String> =
                                    source_dims.iter().map(|d| d.name().to_string()).collect();
                                let target_dim_names: Vec<String> =
                                    target_dims.iter().map(|d| d.name().to_string()).collect();

                                // Check if dimensions can be reordered
                                if let Some(reordering) =
                                    axis_reordering(&source_dim_names, &target_dim_names)
                                {
                                    // Check if reordering is needed (not identity)
                                    let needs_reordering =
                                        reordering.iter().enumerate().any(|(i, &idx)| i != idx);

                                    if needs_reordering {
                                        // Create a transposed view
                                        let orig_dims: Vec<usize> =
                                            source_dims.iter().map(|d| d.len()).collect();

                                        // Reorder the dimensions
                                        let reordered_dims: Vec<usize> = target_dims
                                            .iter()
                                            .map(|target_dim| {
                                                source_dims
                                                    .iter()
                                                    .find(|source_dim| {
                                                        canonicalize(source_dim.name())
                                                            == canonicalize(target_dim.name())
                                                    })
                                                    .unwrap()
                                                    .len()
                                            })
                                            .collect();

                                        // Create strides for the reordered view
                                        let mut strides = vec![1isize; orig_dims.len()];
                                        for i in (0..orig_dims.len() - 1).rev() {
                                            strides[i] = strides[i + 1] * orig_dims[i + 1] as isize;
                                        }

                                        // Reorder the strides according to the dimension reordering
                                        let reordered_strides: Vec<isize> =
                                            reordering.iter().map(|&idx| strides[idx]).collect();

                                        let view = ArrayView {
                                            dims: reordered_dims,
                                            strides: reordered_strides,
                                            offset: 0,
                                            sparse: Vec::new(),
                                            dim_names: target_dim_names.clone(),
                                        };

                                        return Ok(Expr::StaticSubscript(base, view, *loc));
                                    }
                                }
                            }

                            // No reordering needed or not in A2A context
                            let orig_dims: Vec<usize> =
                                source_dims.iter().map(|d| d.len()).collect();
                            let dim_names: Vec<String> =
                                source_dims.iter().map(|d| d.name().to_string()).collect();
                            let view = ArrayView::contiguous_with_names(orig_dims, dim_names);
                            return Ok(Expr::StaticSubscript(base, view, *loc));
                        }
                        Err(err)
                    }
                }
            }

            Expr3::Subscript(id, indices, _bounds, loc) => self.lower_subscript(id, indices, *loc),

            Expr3::App(builtin, _bounds, loc) => {
                // Lower builtin directly without converting to Expr2
                let lowered_builtin = self.lower_builtin_expr3(builtin)?;
                Ok(Expr::App(lowered_builtin, *loc))
            }

            Expr3::Op1(op, inner, _bounds, loc) => {
                match op {
                    ast::UnaryOp::Transpose => {
                        // Special handling for transpose of bare array variables
                        if let Expr3::Var(id, _, var_loc) = &**inner {
                            // Check the variable's declared dimensions: is it an array?
                            if let Some(dims) = self.dims_of(id) {
                                if self.active_dimension.is_some() {
                                    // We're in an A2A context - need to handle bare array transpose specially
                                    // Process the variable with reversed active dimensions
                                    let result = self
                                        .with_transposed_active_context()
                                        .lower_from_expr3(inner)?;
                                    return Ok(result);
                                } else {
                                    // Not in A2A context - create a wildcard subscript to get the full array
                                    // then apply transpose
                                    let base = self.get_base_ref(id)?;
                                    let orig_dims: Vec<usize> =
                                        dims.iter().map(|d| d.len()).collect();
                                    let orig_dim_names: Vec<String> =
                                        dims.iter().map(|d| d.name().to_string()).collect();
                                    let orig_strides =
                                        ArrayView::contiguous(orig_dims.clone()).strides;

                                    // Create a view for the full array and transpose it
                                    let view = ArrayView {
                                        dims: orig_dims.clone(),
                                        strides: orig_strides,
                                        offset: 0,
                                        sparse: Vec::new(),
                                        dim_names: orig_dim_names,
                                    };

                                    return Ok(Expr::StaticSubscript(
                                        base,
                                        view.transpose(),
                                        *var_loc,
                                    ));
                                }
                            }
                        }

                        // Default transpose handling
                        if self.active_dimension.is_some() {
                            // In A2A context, transpose swaps the active indices
                            self.with_transposed_active_context()
                                .lower_from_expr3(inner)
                        } else {
                            let lowered = self.lower_from_expr3(inner)?;
                            // Transpose reverses the dimensions of an array
                            match lowered {
                                Expr::StaticSubscript(base, view, expr_loc) => {
                                    Ok(Expr::StaticSubscript(base, view.transpose(), expr_loc))
                                }
                                _ => {
                                    // For other expressions, wrap in a transpose operation
                                    Ok(Expr::Op1(UnaryOp::Transpose, Box::new(lowered), *loc))
                                }
                            }
                        }
                    }
                    _ => {
                        // Process the inner expression first for other operators
                        let lowered = self.lower_from_expr3(inner)?;
                        let result = match op {
                            ast::UnaryOp::Negative => Expr::Op2(
                                BinaryOp::Sub,
                                Box::new(Expr::Const(0.0, *loc)),
                                Box::new(lowered),
                                *loc,
                            ),
                            ast::UnaryOp::Positive => lowered,
                            ast::UnaryOp::Not => Expr::Op1(UnaryOp::Not, Box::new(lowered), *loc),
                            ast::UnaryOp::Transpose => unreachable!("Transpose handled above"),
                        };
                        Ok(result)
                    }
                }
            }

            Expr3::Op2(op, left, right, array_bounds, loc) => {
                // Lower both operands
                let mut l_expr = self.lower_from_expr3(left)?;
                let mut r_expr = self.lower_from_expr3(right)?;

                // Only apply dimension reordering if we're NOT in an A2A context.
                // In A2A context, the implicit subscripts already handle dimension reordering.
                if self.active_dimension.is_none() {
                    // If the result is an array, check if operand dimension reordering is needed.
                    if let Some(bounds) = array_bounds
                        && bounds.dim_names().is_some()
                    {
                        let l_dim_names: Option<Vec<String>> =
                            match left.get_array_bounds().and_then(|b| b.dim_names()) {
                                Some(names) => Some(names.iter().map(|s| s.to_string()).collect()),
                                None => self.get_expr3_dimension_names(left),
                            };
                        let r_dim_names: Option<Vec<String>> =
                            match right.get_array_bounds().and_then(|b| b.dim_names()) {
                                Some(names) => Some(names.iter().map(|s| s.to_string()).collect()),
                                None => self.get_expr3_dimension_names(right),
                            };

                        // Check if right needs reordering to match left's dimension order
                        if let (Some(l_names), Some(r_names)) = (&l_dim_names, &r_dim_names)
                            && l_names != r_names
                        {
                            // Check if r can be reordered to match l
                            if let Some(reordering) = axis_reordering(r_names, l_names) {
                                r_expr =
                                    self.apply_dimension_reordering(r_expr, reordering, *loc)?;
                            }
                            // Otherwise check if l can be reordered to match r
                            else if let Some(reordering) = axis_reordering(l_names, r_names) {
                                l_expr =
                                    self.apply_dimension_reordering(l_expr, reordering, *loc)?;
                            }
                        }
                    }
                }

                let bin_op = match op {
                    ast::BinaryOp::Add => BinaryOp::Add,
                    ast::BinaryOp::Sub => BinaryOp::Sub,
                    ast::BinaryOp::Exp => BinaryOp::Exp,
                    ast::BinaryOp::Mul => BinaryOp::Mul,
                    ast::BinaryOp::Div => BinaryOp::Div,
                    ast::BinaryOp::Mod => BinaryOp::Mod,
                    ast::BinaryOp::Gt => BinaryOp::Gt,
                    ast::BinaryOp::Gte => BinaryOp::Gte,
                    ast::BinaryOp::Lt => BinaryOp::Lt,
                    ast::BinaryOp::Lte => BinaryOp::Lte,
                    ast::BinaryOp::Eq => BinaryOp::Eq,
                    ast::BinaryOp::Neq => BinaryOp::Neq,
                    ast::BinaryOp::And => BinaryOp::And,
                    ast::BinaryOp::Or => BinaryOp::Or,
                };

                Ok(Expr::Op2(bin_op, Box::new(l_expr), Box::new(r_expr), *loc))
            }

            Expr3::If(cond, then_expr, else_expr, _bounds, loc) => {
                let cond = self.lower_from_expr3(cond)?;
                let t = self.lower_from_expr3(then_expr)?;
                let f = self.lower_from_expr3(else_expr)?;
                Ok(Expr::If(Box::new(cond), Box::new(t), Box::new(f), *loc))
            }
        }
    }

    /// Row-major strides for `dims`.
    fn row_major_strides(dims: &[usize]) -> Vec<isize> {
        let mut strides = vec![1isize; dims.len()];
        for i in (0..dims.len().saturating_sub(1)).rev() {
            strides[i] = strides[i + 1] * dims[i + 1] as isize;
        }
        strides
    }

    /// The declared dimensions of the variable a subscript expression reads.
    fn subscript_dims(&self, id: &Ident<Canonical>) -> Result<&[Dimension]> {
        self.shape_of(id)?.dimensions().ok_or_else(|| {
            Error::new(
                ErrorKind::Model,
                ErrorCode::Generic,
                Some(format!(
                    "expected array variable '{}' to have dimensions",
                    id.as_str()
                )),
            )
        })
    }

    /// `id[indices]` lowered.
    ///
    /// Four steps, each its own function: NORMALIZE the index expressions into
    /// static [`IndexOp`]s ([`Self::normalize_subscript_ops`]), BUILD the view
    /// they describe and decide what the enclosing context reads through it
    /// ([`Self::lower_static_subscript`]), RESOLVE the one slot an
    /// apply-to-all element reads ([`Self::resolve_iteration_element`]), and
    /// EMIT the runtime-evaluated form for an index no view can carry
    /// ([`Self::lower_dynamic_subscript`]).
    fn lower_subscript(
        &self,
        id: &Ident<Canonical>,
        indices: &[IndexExpr3],
        loc: Loc,
    ) -> Result<Expr> {
        let base = self.get_base_ref(id)?;
        let dims = self.subscript_dims(id)?;

        if indices.len() != dims.len() {
            return sim_err!(MismatchedDimensions, id.as_str().to_string());
        }

        // An ARRAY-valued index (`a[b[*]]`) has no meaning: an index selects
        // one element and nothing says which element of `b` supplies it.
        for idx in indices {
            if let IndexExpr3::Expr(expr) = idx
                && expr.get_array_bounds().is_some()
            {
                return sim_err!(
                    Generic,
                    format!("array-valued subscript expression for '{}'", id.as_str())
                );
            }
        }

        if let Some(operations) = self.normalize_subscript_ops(id, indices, dims)?
            && let Some(expr) = self.lower_static_subscript(id, &operations, dims, &base, loc)?
        {
            return Ok(expr);
        }

        self.lower_dynamic_subscript(id, indices, dims, base, loc)
    }

    /// The static [`IndexOp`]s `indices` normalize to, or `None` when one of
    /// them needs runtime evaluation.
    ///
    /// A dimension position (`@N`) normally SURVIVES normalization so the
    /// enclosing iteration can bind it to an axis. In a scalar context -- no
    /// active apply-to-all dimension, and not inside an array builtin whose
    /// wildcards are being preserved -- there is no iteration to bind it to
    /// and `@N` selects element N of the axis instead, so it is resolved here.
    fn normalize_subscript_ops(
        &self,
        id: &Ident<Canonical>,
        indices: &[IndexExpr3],
        dims: &[Dimension],
    ) -> Result<Option<Vec<IndexOp>>> {
        let config = Subscript3Config {
            dims,
            all_dimensions: self.dimensions,
            dimensions_ctx: self.dimensions_ctx,
            active_dimension: self.active_dimension.as_deref(),
        };
        let Some(mut operations) = normalize_subscripts3(indices, &config) else {
            return Ok(None);
        };

        if self.active_dimension.is_none() && !self.preserve_wildcards_for_iteration {
            for (i, op) in operations.iter_mut().enumerate() {
                if let IndexOp::DimPosition(pos) = op {
                    let pos_1based = *pos + 1;
                    // `normalize_subscripts3` already rejects `@0`; guarded
                    // here too because the subtraction below would wrap.
                    if pos_1based == 0 || pos_1based > dims[i].len() {
                        return sim_err!(MismatchedDimensions, id.as_str().to_string());
                    }
                    *op = IndexOp::Single(pos_1based - 1);
                }
            }
        }

        Ok(Some(operations))
    }

    /// The context [`build_view_from_ops`] resolves an `ActiveDimRef` against.
    fn view_config<'c>(&'c self, dims: &'c [Dimension]) -> ViewBuildConfig<'c> {
        ViewBuildConfig {
            active_subscript: self.active_subscript.as_deref(),
            dims,
            active_dimension: self.active_dimension.as_deref(),
            dimensions_ctx: Some(self.dimensions_ctx),
        }
    }

    /// The lowered form of a subscript whose indices are all static, or `None`
    /// when only the dynamic path can finish it.
    ///
    /// `None` is exactly one shape: a dimension position (`@N`) inside an
    /// apply-to-all body. `@N` names an axis of the ITERATION rather than an
    /// element of the source, so it is bound per element by
    /// [`Self::lower_index_expr3`] instead of being baked into a view here.
    fn lower_static_subscript(
        &self,
        id: &Ident<Canonical>,
        operations: &[IndexOp],
        dims: &[Dimension],
        base: &VarRef,
        loc: Loc,
    ) -> Result<Option<Expr>> {
        let orig_dims: Vec<usize> = dims.iter().map(|d| d.len()).collect();
        let orig_strides = Self::row_major_strides(&orig_dims);
        let config = self.view_config(dims);
        let built = build_view_from_ops(operations, &orig_dims, &orig_strides, &config)?;

        let (Some(active_dims), Some(active_subscripts)) = (
            self.active_dimension.as_deref(),
            self.active_subscript.as_deref(),
        ) else {
            // Outside an apply-to-all body the whole view IS the value.
            return Ok(Some(Expr::StaticSubscript(base.clone(), built.view, loc)));
        };

        if operations
            .iter()
            .any(|op| matches!(op, IndexOp::DimPosition(_)))
        {
            return Ok(None);
        }

        if self.preserves_axes_for_iteration(operations) {
            // Only an ARRAY-PRODUCING builtin promotes an `ActiveDimRef` back
            // to a whole-axis wildcard; a reducer keeps the concrete offset
            // `build_view_from_ops` resolved.
            let preserved_ops: Vec<IndexOp> = operations
                .iter()
                .map(|op| match op {
                    IndexOp::ActiveDimRef(_) if self.promote_active_dim_ref => IndexOp::Wildcard,
                    other => other.clone(),
                })
                .collect();
            let preserved =
                build_view_from_ops(&preserved_ops, &orig_dims, &orig_strides, &config)?;
            return Ok(Some(Expr::StaticSubscript(
                base.clone(),
                preserved.view,
                loc,
            )));
        }

        if built.view.dims.is_empty() {
            return Ok(Some(self.collapsed_element_read(
                operations,
                base,
                &built.view,
                loc,
            )));
        }

        self.resolve_iteration_element(
            id,
            operations,
            dims,
            &built,
            active_dims,
            active_subscripts,
            base,
            loc,
        )
        .map(Some)
    }

    /// Whether this reference's axes survive as a view for an enclosing array
    /// builtin's iteration rather than collapsing to the active element.
    ///
    /// A wildcard, a star range and a range always do, inside any array
    /// builtin. An `ActiveDimRef` does so only inside an ARRAY-PRODUCING one
    /// (`VECTOR SORT ORDER`, `VECTOR ELM MAP`, ...), where the source array may
    /// live in a different dimension space than the output; inside a reducer
    /// (`SUM`, `MEAN`, ...) it resolves to a concrete offset instead.
    fn preserves_axes_for_iteration(&self, operations: &[IndexOp]) -> bool {
        if !self.preserve_wildcards_for_iteration {
            return false;
        }
        let has_axis_ops = operations.iter().any(|op| {
            matches!(
                op,
                IndexOp::Wildcard | IndexOp::SparseRange { .. } | IndexOp::Range { .. }
            )
        });
        let has_active_dim_ref = operations
            .iter()
            .any(|op| matches!(op, IndexOp::ActiveDimRef(_)));
        has_axis_ops || (self.promote_active_dim_ref && has_active_dim_ref)
    }

    /// A source reference whose axes all collapsed to ONE element.
    ///
    /// Inside an array-producing builtin the element's BASE OFFSET has to
    /// survive, because the builtin maps over the source variable's full
    /// row-major storage starting from that base (genuine-Vensim
    /// `VECTOR ELM MAP`, GH #578): the collapsed view's `offset` is the
    /// element's flat index, and the variable base lets
    /// `codegen::full_source_len` recover the full source length. Promoting
    /// the `Single` ops back to a whole-array wildcard view discards that base
    /// and is correct only when the element happens to be index 0.
    fn collapsed_element_read(
        &self,
        operations: &[IndexOp],
        base: &VarRef,
        view: &ArrayView,
        loc: Loc,
    ) -> Expr {
        if self.promote_active_dim_ref
            && operations.iter().any(|op| matches!(op, IndexOp::Single(_)))
        {
            return Expr::StaticSubscript(base.clone(), view.clone(), loc);
        }
        Expr::Var(base.offset_by(view.offset), loc)
    }

    /// The dimension an [`AxisMatch::Mapped`] pairing was declared THROUGH
    /// when it is neither axis's own -- the common-mapping-target rung, where
    /// both axes map onto a third dimension.
    ///
    /// The forward and reverse rungs name the target and the source
    /// respectively, and the mapped-parent rung is answered by its own arm
    /// before this one is asked, so what is left is the two-mapping case.
    /// Reading the PAIRING rather than re-deriving the relation is what keeps
    /// the element step from resolving through a rung
    /// [`crate::dimensions::match_axes_partial`] did not use: an axis the
    /// matcher left unpaired is read positionally, and must not pick up a
    /// mapping the matcher declined to apply.
    fn common_mapping_target(
        pairing: Option<&AxisMatch>,
        source_dim: &Dimension,
        target_dim: &Dimension,
    ) -> Option<CanonicalDimensionName> {
        match pairing {
            Some(AxisMatch::Mapped { via })
                if via != source_dim.canonical_name() && via != target_dim.canonical_name() =>
            {
                Some(via.clone())
            }
            _ => None,
        }
    }

    /// The single slot the current apply-to-all element reads through
    /// `built.view`.
    ///
    /// Two questions, in this order.
    ///
    /// WHICH ACTIVE AXIS supplies each view axis is
    /// [`crate::dimensions::match_axes_partial`], the engine's one axis-matching
    /// precedence, run over the view's `(name, length)` axes. An axis it does
    /// not pair is read POSITIONALLY, and positional reading requires the
    /// lengths to agree -- except for a range-derived axis, which is allowed
    /// to be shorter and produces NaN past its end.
    ///
    /// WHICH ELEMENT of the source axis that active subscript selects is the
    /// source axis's own name first, then the declared mapping in either
    /// direction, then a mapped parent of the active subdimension, then the
    /// dimension both axes map onto ([`Self::common_mapping_target`]). Those
    /// last three arms answer the same three relations the pairing ran on, so
    /// a reference resolves its element through the rung that paired its axes
    /// rather than through a second, weaker guess. A mapping that EXISTS but
    /// cannot translate is an error rather than a fallback onto the target
    /// axis: falling back hides a misconfigured element map (a size mismatch,
    /// say) behind a plausible-looking index.
    #[allow(clippy::too_many_arguments)]
    fn resolve_iteration_element(
        &self,
        id: &Ident<Canonical>,
        operations: &[IndexOp],
        dims: &[Dimension],
        built: &ViewBuildResult,
        active_dims: &[Dimension],
        active_subscripts: &[CanonicalElementName],
        base: &VarRef,
        loc: Loc,
    ) -> Result<Expr> {
        let ViewBuildResult {
            view,
            dim_mapping,
            single_indices,
        } = built;

        let view_axes: Vec<Axis<'_>> = view
            .dim_names
            .iter()
            .zip(view.dims.iter())
            .map(|(name, &len)| Axis::named(name.as_str(), len))
            .collect();
        let supplied_by =
            match_axes_partial(&view_axes, &axes_of(active_dims), self.dimensions_ctx);
        let every_axis_paired = supplied_by.iter().all(Option::is_some);

        // Inside an array-producing builtin (`promote_active_dim_ref`) an arity
        // mismatch is expected: the source array lives in a different dimension
        // space than the output (`d[DimA,B1]` partially collapses to DimA only,
        // which differs from a DimA x DimB output).
        if !every_axis_paired
            && !self.promote_active_dim_ref
            && view.dims.len() != active_dims.len()
        {
            return sim_err!(MismatchedDimensions, id.as_str().to_string());
        }

        // Which view axes came from a Range, so a size mismatch is allowed and
        // an out-of-bounds element becomes NaN rather than an error.
        let range_view_dims: std::collections::HashSet<usize> = {
            let mut set = std::collections::HashSet::new();
            let mut view_dim_idx = 0;
            for op in operations {
                match op {
                    // These collapse their axis and contribute no view axis.
                    IndexOp::Single(_) | IndexOp::ActiveDimRef(_) => {}
                    IndexOp::Range { .. } => {
                        set.insert(view_dim_idx);
                        view_dim_idx += 1;
                    }
                    _ => {
                        view_dim_idx += 1;
                    }
                }
            }
            set
        };

        // A positionally-read axis must have the target's length. Skipped while
        // preserving wildcards for iteration (`SUM`, `MEAN`, ...): the view
        // describes what the REDUCTION iterates and is independent of the
        // output's dimensions, so `SUM(c[*])` in a DimB context is valid.
        if !self.preserve_wildcards_for_iteration {
            for (view_idx, &view_dim) in view.dims.iter().enumerate() {
                if supplied_by[view_idx].is_none()
                    && view_idx < active_dims.len()
                    && !range_view_dims.contains(&view_idx)
                    && view_dim != active_dims[view_idx].len()
                {
                    return sim_err!(MismatchedDimensions, id.as_str().to_string());
                }
            }
        }

        let mut result_index = 0;
        for (view_idx, stride) in view.strides.iter().enumerate() {
            let (active_idx, subscript, pairing) = match &supplied_by[view_idx] {
                Some((active_idx, how)) => {
                    (*active_idx, &active_subscripts[*active_idx], Some(how))
                }
                None if view_idx < active_subscripts.len() => {
                    (view_idx, &active_subscripts[view_idx], None)
                }
                None => return sim_err!(MismatchedDimensions, id.as_str().to_string()),
            };

            let Some(Some(dim_idx)) = dim_mapping.get(view_idx).copied() else {
                return sim_err!(MismatchedDimensions, id.as_str().to_string());
            };
            if dim_idx >= dims.len() {
                return sim_err!(MismatchedDimensions, id.as_str().to_string());
            }

            let source_dim = &dims[dim_idx];
            let target_dim = &active_dims[active_idx];
            let is_sparse = view.sparse.iter().any(|s| s.dim_index == view_idx);

            let prefer_source = source_dim.name() == target_dim.name()
                || matches!(source_dim, Dimension::Named(_, _));
            let mut source_offset = if prefer_source {
                source_dim.get_offset(subscript)
            } else {
                None
            };

            // A mapping that exists but cannot translate is authoritative:
            // `mapping_failed` suppresses the target-axis fallback below.
            let mut mapping_failed = false;
            if source_offset.is_none() {
                let source_dim_name = source_dim.canonical_name();
                let target_dim_name = target_dim.canonical_name();

                let has_direct_or_reverse_mapping = self
                    .dimensions_ctx
                    .has_mapping_to(source_dim_name, target_dim_name)
                    || self
                        .dimensions_ctx
                        .has_mapping_to(target_dim_name, source_dim_name);
                let has_parent_mapping = self
                    .dimensions_ctx
                    .has_mapping_to_parent_of(source_dim_name, target_dim_name);

                if let Some(translated) = self.dimensions_ctx.translate_via_mapping(
                    source_dim_name,
                    target_dim_name,
                    subscript,
                ) {
                    source_offset = source_dim.get_offset(&translated);
                } else if has_parent_mapping {
                    // The source maps onto a PARENT of the target: translate
                    // through that specific parent rather than the first
                    // mapping target.
                    let parent_target = self
                        .dimensions_ctx
                        .find_mapping_parent_of(source_dim_name, target_dim_name);
                    if let Some(parent) = parent_target
                        && let Some(translated) = self
                            .dimensions_ctx
                            .translate_to_source_via_mapping(source_dim_name, parent, subscript)
                    {
                        source_offset = source_dim.get_offset(&translated);
                    } else {
                        mapping_failed = true;
                    }
                } else if let Some(via) =
                    Self::common_mapping_target(pairing, source_dim, target_dim)
                {
                    // The COMMON-MAPPING-TARGET rung: neither axis maps onto
                    // the other, but both declare a mapping onto `via`, so the
                    // element runs target -> via -> source. That is two
                    // translations where every other rung needs one, and the
                    // `target_offset` fallback below -- reading the source axis
                    // at the target element's own ORDINAL -- is that element
                    // only while BOTH mappings are positional. With an explicit
                    // element map on either side the ordinal is a different
                    // row, so the chain is walked rather than assumed, and a
                    // chain that cannot be walked is authoritative like any
                    // other declared mapping.
                    //
                    // Which element a pair of dimensions mapping onto a third
                    // corresponds through is UNVERIFIED against Vensim and
                    // Stella: neither documents the two-mapping case, and no
                    // model under `test/` spells it. What is not a judgement
                    // call is that the reference resolves through `via` if it
                    // resolves at all -- `via` is the only reason the matcher
                    // paired these axes.
                    if let Some(via_element) =
                        self.dimensions_ctx
                            .translate_via_mapping(&via, target_dim_name, subscript)
                        && let Some(translated) = self.dimensions_ctx.translate_via_mapping(
                            source_dim_name,
                            &via,
                            &via_element,
                        )
                    {
                        source_offset = source_dim.get_offset(&translated);
                    } else {
                        mapping_failed = true;
                    }
                } else if has_direct_or_reverse_mapping {
                    mapping_failed = true;
                }
            }

            let target_offset = if source_offset.is_none() && !mapping_failed {
                target_dim.get_offset(subscript)
            } else {
                None
            };

            let (abs_offset, offset_from_source) = if let Some(abs_offset) = source_offset {
                (abs_offset, true)
            } else if let Some(abs_offset) = target_offset {
                (abs_offset, false)
            } else if mapping_failed {
                return sim_err!(
                    MismatchedDimensions,
                    format!(
                        "{}: dimension mapping from {} to {} failed for subscript '{}' \
                         (check that both dimensions have the same number of elements)",
                        id.as_str(),
                        source_dim.name(),
                        target_dim.name(),
                        subscript.as_str()
                    )
                );
            } else {
                return sim_err!(MismatchedDimensions, id.as_str().to_string());
            };

            let rel_offset = if is_sparse {
                if !offset_from_source {
                    return sim_err!(MismatchedDimensions, id.as_str().to_string());
                }
                abs_offset
            } else if offset_from_source {
                let start_offset = single_indices[dim_idx];
                match abs_offset.checked_sub(start_offset) {
                    Some(rel_offset) => rel_offset,
                    // Before a range's start is out of bounds: NaN for a
                    // range-derived axis, an error for anything else.
                    None if range_view_dims.contains(&view_idx) => {
                        return Ok(Expr::Const(f64::NAN, loc));
                    }
                    None => return sim_err!(MismatchedDimensions, id.as_str().to_string()),
                }
            } else {
                abs_offset
            };

            if range_view_dims.contains(&view_idx) && rel_offset >= view.dims[view_idx] {
                return Ok(Expr::Const(f64::NAN, loc));
            }

            result_index += rel_offset * (*stride as usize);
        }

        Ok(Expr::Var(base.offset_by(view.offset + result_index), loc))
    }

    /// A subscript at least one of whose indices needs runtime evaluation.
    ///
    /// The one special case is a dynamic RANGE (`data[start:end]` with
    /// variable bounds) read per element of an apply-to-all body: a range is
    /// not a scalar subscript, so it lowers to a per-element conditional --
    /// load `data[start + i]` while `start + i <= end`, NaN past the end.
    fn lower_dynamic_subscript(
        &self,
        id: &Ident<Canonical>,
        indices: &[IndexExpr3],
        dims: &[Dimension],
        base: VarRef,
        loc: Loc,
    ) -> Result<Expr> {
        let has_dynamic_range = indices
            .iter()
            .any(|idx| matches!(idx, IndexExpr3::Range(..)));

        if has_dynamic_range
            && indices.len() == 1
            && dims.len() == 1
            && let Some(active_subscripts) = &self.active_subscript
            && let Some(active_dims) = &self.active_dimension
            && let IndexExpr3::Range(start_expr, end_expr, _) = &indices[0]
        {
            // Which active axis is the source's own is
            // `dimensions::match_axes_partial` -- and this is the ONE caller
            // that admits the subdimension rung, because it only picks which
            // axis to compare POSITIONS against and never resolves an element
            // through the answer (an out-of-range position becomes NaN below).
            // In a multi-dimensional context (`target[DimA, DimB] =
            // data[start:end]` over a DimB-indexed `data`) the source axis is
            // not necessarily `active_dims[0]`.
            let source_axes = axes_of(&dims[..1]);
            let match_idx = match_axes_partial(
                &source_axes,
                &axes_of(active_dims),
                &SubdimensionRelations(self.dimensions_ctx),
            )[0]
            .as_ref()
            .map(|(active_idx, _)| *active_idx)
            // No axis of the target corresponds. Reading the first is a
            // guess, and it is the guess this arm has always made; the
            // range's own bounds check turns a wrong position into NaN
            // rather than into a neighbouring element.
            .unwrap_or(0);
            let target_dim = &active_dims[match_idx];
            let subscript = &active_subscripts[match_idx];
            let elem_pos_1based = Self::subscript_to_index(target_dim, subscript);

            let start_lowered = self.lower_from_expr3(start_expr)?;
            let end_lowered = self.lower_from_expr3(end_expr)?;

            // P is the 1-based position of the current target element and
            // i = P - 1 its 0-based index. `[start:end]` selects source
            // positions start..end (1-based), so range_view[i] is
            // data[start + i], valid while start + i <= end.
            let i_0based = elem_pos_1based - 1.0;
            let computed_index = Expr::Op2(
                BinaryOp::Add,
                Box::new(start_lowered),
                Box::new(Expr::Const(i_0based, loc)),
                loc,
            );
            let in_bounds = Expr::Op2(
                BinaryOp::Lte,
                Box::new(computed_index.clone()),
                Box::new(end_lowered),
                loc,
            );
            let load_elem = Expr::Subscript(
                base,
                vec![SubscriptIndex::Single(computed_index)],
                vec![dims[0].len()],
                loc,
            );

            return Ok(Expr::If(
                Box::new(in_bounds),
                Box::new(load_elem),
                Box::new(Expr::Const(f64::NAN, loc)),
                loc,
            ));
        }

        let orig_dims: Vec<usize> = dims.iter().map(|d| d.len()).collect();
        let args: Result<Vec<_>> = indices
            .iter()
            .enumerate()
            .map(|(i, arg)| self.lower_index_expr3(arg, id, i, dims, &orig_dims, loc))
            .collect();
        Ok(Expr::Subscript(base, args?, orig_dims, loc))
    }

    /// Get dimension names from an Expr3 if it's an array variable
    fn get_expr3_dimension_names(&self, expr: &Expr3) -> Option<Vec<String>> {
        match expr {
            Expr3::Var(ident, _, _) | Expr3::Subscript(ident, _, _, _) => {
                let dims = self.dims_of(ident)?;
                Some(dims.iter().map(|d| d.name().to_string()).collect())
            }
            _ => None,
        }
    }

    /// The snapshot storage a bare `PREVIOUS`/`INIT` argument `ident` reads
    /// when its dependency shape decides the answer differently from an
    /// ordinary read of the same name -- or a refusal, when the name has no
    /// snapshot storage at all -- and `Ok(None)` when the ordinary read is
    /// the right one.
    ///
    /// The parse leaves every bare reference in place -- it reads nothing of
    /// the owning model -- so this is where a name's KIND settles what a
    /// snapshot of it addresses:
    ///
    /// * a bare module instance (`PREVIOUS(sub)`) has no storage of its own:
    ///   the instance's block starts with whichever sub-model variable the
    ///   layout put first, and reading it would be a plausible wrong number,
    ///   so it is refused;
    /// * a bound module input port reads its OWN slot. An ordinary read of the
    ///   port lowers to `Expr::ModuleInput` (the value the parent pushed), but
    ///   the port's fragment assigns that value to the port's slot every phase
    ///   (`Var::new`), so its `prev_values`/`initial_values` entry is exactly
    ///   the lagged or frozen port value -- the direct read is what a capture
    ///   of the port would have computed, without the capture;
    /// * anything else -- a plain variable, a module-call aux, a qualified
    ///   `m·port` -- lowers as an ordinary reference to a fixed slot.
    fn snapshot_storage(&self, ident: &Ident<Canonical>) -> Result<Option<VarRef>> {
        if let DepKind::Module { .. } = self.shape_of(ident)?.kind {
            return Err(Error::new(
                ErrorKind::Simulation,
                ErrorCode::NotSimulatable,
                Some(format!(
                    "PREVIOUS/INIT cannot read the bare module instance '{ident}': \
                     name one of its output ports"
                )),
            ));
        }
        if self.inputs.contains(ident) {
            return Ok(Some(self.get_ref(ident)?));
        }
        Ok(None)
    }

    /// Lower a BuiltinFn<Expr3> to BuiltinFn (i.e., BuiltinFn<Expr>).
    ///
    /// Which context lowers each argument is the table's `ArgKind`. A
    /// reducer's array operand (`Array { whole: false }`) keeps wildcards for
    /// iteration while an active-dimension reference still pins its axis, so
    /// `SUM(matrix[DimA, *])` sums one row per element. A vector builtin's
    /// array operand (`Array { whole: true }`) also promotes active-dimension
    /// references back to wildcards, so `vals[DimA]` inside
    /// `VECTOR SORT ORDER` keeps its full array view. A scalar, and a lookup's
    /// table reference, lower in the enclosing context. A `PREVIOUS`/`INIT`
    /// argument that is a bare name additionally goes through
    /// [`Self::snapshot_storage`], which is where the name's kind decides what
    /// the snapshot addresses.
    fn lower_builtin_expr3(
        &self,
        builtin: &crate::builtins::BuiltinFn<Expr3>,
    ) -> Result<BuiltinFn> {
        use crate::builtins::BuiltinFn as B;

        let snapshot_slot = match builtin {
            B::Previous(arg, _) | B::Init(arg) => match arg.as_ref() {
                Expr3::Var(ident, _, loc) => self.snapshot_storage(ident)?.map(|r| (r, *loc)),
                _ => None,
            },
            _ => None,
        };

        let mut whole_ctx: Option<Context<'_>> = None;
        let mut lowered = builtin.try_map_ref_with_kinds(|arg, kind| match kind {
            ArgKind::Array { whole: false } => {
                self.with_preserved_wildcards().lower_from_expr3(arg)
            }
            ArgKind::Array { whole: true } => whole_ctx
                .get_or_insert_with(|| self.with_vector_builtin_wildcards())
                .lower_from_expr3(arg),
            ArgKind::Scalar => self.lower_from_expr3(arg),
            // Inside an apply-to-all body a lookup's table lowers like a
            // REDUCER's operand: the element pins the axes it names and every
            // other axis survives as a view. That is what a per-element
            // arrayed-GF apply is -- `out[COP] = LOOKUP(g, t)` over `g[COP]`
            // applies each element's own table, while over `g[COP, ROW]` the
            // free ROW axis makes the apply array-valued and
            // `compiler::array_operand` materializes it into the `LookupArray`
            // temp codegen emits. OUTSIDE one the enclosing context already
            // keeps the whole view, and switching to the wildcard-preserving
            // one would additionally stop `normalize_subscript_ops` resolving a
            // `@N` table index to its element -- there is no iteration for `@N`
            // to name in a table position, so it must resolve.
            ArgKind::Table if self.active_dimension.is_some() => {
                self.with_preserved_wildcards().lower_from_expr3(arg)
            }
            ArgKind::Table => self.lower_from_expr3(arg),
            ArgKind::Ident => unreachable!("an identifier payload is not an expression argument"),
        })?;
        if let Some((var, loc)) = snapshot_slot
            && let BuiltinFn::Previous(arg, _) | BuiltinFn::Init(arg) = &mut lowered
        {
            **arg = Expr::Var(var, loc);
        }
        // ALLOCATE AVAILABLE reads all four XPriority columns for each
        // requester, while the Vensim convention spells the argument collapsed
        // (`pp[region, ptype]` means "the priority vector starting from
        // ptype"), so the lowered profile is re-expanded to the variable's
        // full requester x XPriority array.
        if let (
            crate::builtins::BuiltinFn::AllocateAvailable(_, pp, _),
            BuiltinFn::AllocateAvailable(_, lowered_pp, _),
        ) = (builtin, &mut lowered)
        {
            self.expand_pp_view_for_allocate(pp, lowered_pp)?;
        }
        Ok(lowered)
    }

    /// For ALLOCATE AVAILABLE's pp argument, ensure the full variable array
    /// is accessible.  The Vensim convention pp[requester_dim, ptype] means
    /// "the priority vector starting at ptype", but ALLOCATE AVAILABLE reads
    /// all XPriority columns for each requester.  If lowering produced a
    /// StaticSubscript that collapsed some dimensions (e.g. only region but
    /// not XPriority), replace it with a full-variable view.
    fn expand_pp_view_for_allocate(&self, expr3: &Expr3, lowered: &mut Expr) -> Result<()> {
        // Only expand if the lowered expression is a subscripted variable
        // with fewer dimensions than the full variable.
        let (current_ndims, loc) = match &*lowered {
            Expr::StaticSubscript(_, view, loc) => (view.dims.len(), *loc),
            Expr::Var(_, loc) => (0, *loc),
            _ => return Ok(()),
        };

        // Find the variable identifier from the Expr3 to look up full dimensions
        let var_ident = match expr3 {
            Expr3::Subscript(id, _, _, _) => id,
            Expr3::Var(id, _, _) => id,
            _ => return Ok(()),
        };

        let Some(full_dims) = self.dims_of(var_ident) else {
            return Ok(());
        };

        if current_ndims >= full_dims.len() {
            return Ok(());
        }

        // The view has fewer dimensions than the full variable -- some were
        // collapsed by per-element subscript evaluation.  Rebuild with a
        // full contiguous view because ALLOCATE AVAILABLE needs the complete
        // priority profile array (all requesters' profiles) to perform
        // simultaneous allocation.  Any explicit subscripts that restricted
        // dimensions are intentionally overridden: the allocator requires
        // the full array regardless of the calling element's context.
        let base = self.get_base_ref(var_ident)?;
        let dim_sizes: Vec<usize> = full_dims.iter().map(|d| d.len()).collect();
        let dim_names: Vec<String> = full_dims.iter().map(|d| d.name().to_string()).collect();
        let view = ArrayView::contiguous_with_names(dim_sizes, dim_names);
        *lowered = Expr::StaticSubscript(base, view, loc);
        Ok(())
    }

    /// Lower an IndexExpr3 to SubscriptIndex for dynamic subscript handling.
    /// This is used when normalize_subscripts3 returns None.
    /// Returns SubscriptIndex::Single for single-element access or
    /// SubscriptIndex::Range for range access.
    #[allow(clippy::too_many_arguments)]
    fn lower_index_expr3(
        &self,
        idx: &IndexExpr3,
        id: &Ident<Canonical>,
        i: usize,
        dims: &[Dimension],
        _orig_dims: &[usize],
        _loc: Loc,
    ) -> Result<SubscriptIndex> {
        match idx {
            IndexExpr3::StarRange(subdim_name, star_loc) => {
                // StarRange in dynamic context - need to resolve the current element
                if self.active_dimension.is_none() {
                    return sim_err!(
                        ArrayReferenceNeedsExplicitSubscripts,
                        id.as_str().to_string()
                    );
                }
                let active_dims = self.active_dimension.as_ref().unwrap();
                let active_subscripts = self.active_subscript.as_ref().unwrap();
                let dim = &dims[i];

                // Check if this is the full dimension or a subdimension
                let parent_name = crate::common::CanonicalDimensionName::from_raw(dim.name());

                if subdim_name.as_str() == parent_name.as_str() {
                    // Full dimension - find matching active dimension
                    for (active_dim, active_subscript) in active_dims.iter().zip(active_subscripts)
                    {
                        if active_dim.name() == dim.name() {
                            if let Dimension::Named(_, _) = dim
                                && let Some(subscript_off) = dim.get_offset(active_subscript)
                            {
                                return Ok(SubscriptIndex::Single(Expr::Const(
                                    (subscript_off + 1) as f64,
                                    *star_loc,
                                )));
                            } else if let Dimension::Indexed(_, _) = dim
                                && let Ok(idx_val) = active_subscript.as_str().parse::<usize>()
                            {
                                return Ok(SubscriptIndex::Single(Expr::Const(
                                    idx_val as f64,
                                    *star_loc,
                                )));
                            }
                        }
                    }
                }

                // Subdimension case - not yet supported in dynamic context
                sim_err!(TodoStarRange, id.as_str().to_string())
            }

            // StaticRange - should have been handled by normalize_subscripts3,
            // but handle here as a fallback by creating a Range with constants
            IndexExpr3::StaticRange(start_0based, end_0based, loc) => {
                // Convert back to 1-based for the Expr (XMILE uses 1-based indices)
                let start_expr = Expr::Const((*start_0based + 1) as f64, *loc);
                let end_expr = Expr::Const(*end_0based as f64, *loc);
                Ok(SubscriptIndex::Range(start_expr, end_expr))
            }

            IndexExpr3::Range(start, end, _range_loc) => {
                // Dynamic range - lower both bound expressions
                let start_expr = self.lower_from_expr3(start)?;
                let end_expr = self.lower_from_expr3(end)?;
                Ok(SubscriptIndex::Range(start_expr, end_expr))
            }

            IndexExpr3::DimPosition(pos, dim_loc) => {
                let pos_val = *pos as usize;

                // Scalar context: no active A2A dimension, resolve @N directly
                // to a concrete 1-based element offset in the target dimension.
                if self.active_dimension.is_none() {
                    if pos_val == 0 || pos_val > dims[i].len() {
                        return sim_err!(MismatchedDimensions, id.as_str().to_string());
                    }
                    return Ok(SubscriptIndex::Single(Expr::Const(
                        pos_val as f64,
                        *dim_loc,
                    )));
                }

                // A2A context: try to resolve @N via the active subscript at
                // this position (dimension-reordering path, e.g. matrix[@2, @1]).
                // For named dimensions, element names are unique across dimensions,
                // so get_offset reliably distinguishes elements — this also handles
                // subdimension cases (e.g. selected[SubRegion] = data[@1]).
                // For indexed dimensions, numeric element names overlap across
                // unrelated dimensions (e.g. "2" is valid in both X and Y), so
                // get_offset alone can't discriminate the mixed-wildcard case
                // (row[Y] = matrix[@1, *]); we require an exact dimension match.
                let active_subscripts = self.active_subscript.as_ref().unwrap();
                let active_dims = self.active_dimension.as_ref().unwrap();
                let dim = &dims[i];
                let pos_0 = pos_val.saturating_sub(1);
                if pos_0 < active_subscripts.len() {
                    let subscript = &active_subscripts[pos_0];
                    let allow_binding = match dim {
                        Dimension::Named(..) => true,
                        Dimension::Indexed(..) => active_dims.iter().any(|ad| ad == dim),
                    };
                    if allow_binding && let Some(offset) = dim.get_offset(subscript) {
                        return Ok(SubscriptIndex::Single(Expr::Const(
                            (offset + 1) as f64,
                            *dim_loc,
                        )));
                    }
                }

                // A2A fallback for mixed cases (e.g. cube[@1, *, @3]) where
                // the active subscript doesn't match the target dimension.
                // Resolve to a concrete 1-based offset, same as scalar context.
                if pos_val == 0 || pos_val > dims[i].len() {
                    return sim_err!(MismatchedDimensions, id.as_str().to_string());
                }
                Ok(SubscriptIndex::Single(Expr::Const(
                    pos_val as f64,
                    *dim_loc,
                )))
            }

            IndexExpr3::Expr(e) => {
                // Handle Var expressions that might be dimension elements or DimName.Index syntax
                if let Expr3::Var(ident, _, var_loc) = e {
                    let dim = &dims[i];

                    // First check if it's a named dimension element
                    if let Some(offset) = dim.get_offset(
                        &crate::common::CanonicalElementName::from_raw(ident.as_str()),
                    ) {
                        return Ok(SubscriptIndex::Single(Expr::Const(
                            (offset + 1) as f64,
                            *var_loc,
                        )));
                    }

                    // Check for DimName.Index syntax (e.g., "Dim.3" for indexed dimensions)
                    if let Dimension::Indexed(dim_name, size) = dim {
                        let expected_prefix = format!("{}.", dim_name.as_str());
                        if ident.as_str().starts_with(&expected_prefix)
                            && let Ok(idx) =
                                ident.as_str()[expected_prefix.len()..].parse::<usize>()
                        {
                            // Validate the index is within bounds (1-based)
                            let size_usize = *size as usize;
                            if idx >= 1 && idx <= size_usize {
                                return Ok(SubscriptIndex::Single(Expr::Const(
                                    idx as f64, *var_loc,
                                )));
                            }
                        }
                    }

                    // Check if it's a dimension name (A2A reference)
                    let is_dim_name = self
                        .dimensions
                        .iter()
                        .any(|d| &*canonicalize(d.name()) == ident.as_str());

                    if is_dim_name {
                        if self.active_dimension.is_none() {
                            return sim_err!(
                                ArrayReferenceNeedsExplicitSubscripts,
                                id.as_str().to_string()
                            );
                        }
                        let active_dims = self.active_dimension.as_ref().unwrap();
                        let active_subscripts = self.active_subscript.as_ref().unwrap();

                        for (active_dim, active_subscript) in
                            active_dims.iter().zip(active_subscripts)
                        {
                            if &*canonicalize(active_dim.name()) == ident.as_str() {
                                if let Some(offset) = dim.get_offset(active_subscript) {
                                    return Ok(SubscriptIndex::Single(Expr::Const(
                                        (offset + 1) as f64,
                                        *var_loc,
                                    )));
                                } else if let Ok(idx_val) =
                                    active_subscript.as_str().parse::<usize>()
                                {
                                    return Ok(SubscriptIndex::Single(Expr::Const(
                                        idx_val as f64,
                                        *var_loc,
                                    )));
                                }
                            }
                        }
                    }
                }

                // Fall back to lowering the expression directly
                Ok(SubscriptIndex::Single(self.lower_from_expr3(e)?))
            }

            IndexExpr3::Dimension(name, dim_loc) => {
                let dim = &dims[i];

                // First check if the name matches an element of the parent dimension.
                // An element name that happens to match a dimension name should be
                // resolved as an element, not as an A2A dimension reference.
                if let Some(offset) = dim.get_offset(
                    &crate::common::CanonicalElementName::from_raw(name.as_str()),
                ) {
                    return Ok(SubscriptIndex::Single(Expr::Const(
                        (offset + 1) as f64,
                        *dim_loc,
                    )));
                }

                // A2A dimension reference in dynamic context
                if self.active_dimension.is_none() {
                    return sim_err!(
                        ArrayReferenceNeedsExplicitSubscripts,
                        id.as_str().to_string()
                    );
                }
                let active_dims = self.active_dimension.as_ref().unwrap();
                let active_subscripts = self.active_subscript.as_ref().unwrap();

                // Find the active dimension this index names, then resolve the
                // element it selects on THIS source axis. Which active
                // dimension: by name first, then through a declared mapping in
                // either direction -- the pairing
                // `compiler::subscript::normalize_subscripts3` makes on the
                // static path. Which element: the shared executed rule
                // (`DimensionsContext::resolve_mapped_read`, GH #997).
                //
                // Both halves used to be spelled out here as two separate
                // loops, and the second one consulted the mapping WITHOUT
                // trying the active element's own name against this axis
                // first -- a divergence from the two static sites that would
                // have read a different element for a mapped pair whose two
                // dimensions share element names. Instrumenting all three
                // arms found this one resolving nothing across the lib suite,
                // but it is reachable -- measured at 8 references in the
                // integration corpus -- so the divergence was latent rather
                // than absent. Routing it through the shared rule removes it.
                //
                // One behaviour changed for a reference that ALREADY compiled,
                // and it is a fix rather than a wash: where a source axis's
                // dimension maps to two active dimensions and the index names
                // one of them, the old second loop could pair it with the OTHER
                // (it tested only that a mapping existed, in either direction,
                // and took the first active dimension that had one). The
                // candidate order below names the one the index spells first,
                // matching what `normalize_subscripts3` picks on the static
                // path for the same reference.
                // Candidates in `normalize_subscripts3`'s order -- every active
                // dimension the index NAMES, then every one it reaches through
                // a declared mapping -- and the first that resolves wins.
                //
                // The two used to be separate passes distinguished by a
                // `Pairing` enum whose only reader was a numeric fallback: for
                // an INDEXED active dimension whose numeral the source axis did
                // not declare, the by-name pass emitted the raw 1-based index.
                // Both are gone. The fallback is measured DEAD -- zero
                // executions across the lib and integration corpora, where the
                // by-name candidate is reached 8 times and every one resolves
                // by name identity -- and its static twin `build_view_from_ops`
                // has no such fallback at all, so keeping it was the same class
                // of latent divergence GH #997 removed from the rest of this
                // arm. Both paths now REFUSE an unresolvable subscript rather
                // than one of them inventing an index -- the codes still
                // differ (`MismatchedDimensions` here, `Generic` there), which
                // is worth tidying but is not what the fallback was about.
                // (The gate was not structurally vacuous: a NAMED dimension may
                // declare a mapping toward an indexed one, which puts an
                // indexed active dimension in the mapping candidates. It is
                // empirically dead, which is the stronger reason to drop it.)
                //
                // The ORDER survives as a chained iterator rather than as a
                // documented property, because it costs nothing and mirrors
                // `normalize_subscripts3`. No reference in either corpus has
                // two candidates: this arm is entered 12 times, 8 with a single
                // by-name candidate (every one resolving by name identity) and
                // 4 with none at all (the `no_mapping_*` refusal cells of
                // `crate::mapped_reference_semantics_tests`). The two-candidate
                // shape -- a target iterating both a dimension and something
                // mapped to it -- is nevertheless REACHABLE: a subscript naming
                // an active dimension reaches the subscript path as an
                // `IndexExpr3::Dimension` rather than as a folded ordinal,
                // which is how all 8 corpus references (`LOOKUP` table
                // arguments with an `@N` sibling) arrive here naming an ACTIVE
                // dimension. A fixture of that shape reaches this loop with two
                // candidates.
                // What is unmeasured is whether the two ever resolve to
                // DIFFERENT elements in a model that compiles; the order is
                // chosen to match the static path either way.
                let sub_dim_name = CanonicalDimensionName::from_raw(name.as_str());
                let by_name = active_dims
                    .iter()
                    .zip(active_subscripts)
                    .filter(|(ad, _)| ad.canonical_name().as_str() == name.as_str());
                let by_mapping = active_dims
                    .iter()
                    .zip(active_subscripts)
                    .filter(|(ad, _)| ad.canonical_name().as_str() != name.as_str())
                    .filter(|(ad, _)| {
                        let adn = ad.canonical_name();
                        self.dimensions_ctx.has_mapping_to(&sub_dim_name, adn)
                            || self.dimensions_ctx.has_mapping_to(adn, &sub_dim_name)
                    });
                for (active_dim, active_subscript) in by_name.chain(by_mapping) {
                    if let Some(resolved) =
                        self.dimensions_ctx
                            .resolve_mapped_read(dim, active_dim, active_subscript)
                        && let Some(offset) = dim.get_offset(&resolved)
                    {
                        return Ok(SubscriptIndex::Single(Expr::Const(
                            (offset + 1) as f64,
                            *dim_loc,
                        )));
                    }
                }

                sim_err!(MismatchedDimensions, id.as_str().to_string())
            }
        }
    }
}

/// A `Context` over hand-built dependency shapes, for the unit tests of the
/// resolution mechanics below. Production contexts are built by
/// `super::fragment::lower_fragment` from a `FragmentInput`; these tests
/// exercise the resolver on inputs of their own choosing.
#[cfg(test)]
fn test_context<'a>(
    dimensions: &'a [Dimension],
    dimensions_ctx: &'a DimensionsContext,
    deps: &'a IdentMap<Ident<Canonical>, DepShape>,
    var_sizes: &'a super::VarSizes,
    inputs: &'a BTreeSet<Ident<Canonical>>,
) -> Context<'a> {
    Context::new(
        ContextCore {
            dimensions,
            dimensions_ctx,
            deps,
            var_sizes,
            inputs,
        },
        false,
    )
}

#[test]
fn test_lower() {
    use crate::common::{Canonical, Ident};
    let lower_if = |op: ast::BinaryOp| {
        use ast::Expr2::*;
        Box::new(If(
            Box::new(Op2(
                op,
                Box::new(Var(Ident::new("true_input"), None, Loc::default())),
                Box::new(Var(Ident::new("false_input"), None, Loc::default())),
                None,
                Loc::default(),
            )),
            Box::new(Const(
                "1".to_string(),
                crate::ast::Literal::new(1.0),
                Loc::default(),
            )),
            Box::new(Const(
                "0".to_string(),
                crate::ast::Literal::new(0.0),
                Loc::default(),
            )),
            None,
            Loc::default(),
        ))
    };
    let expected = |op: BinaryOp| {
        Expr::If(
            Box::new(Expr::Op2(
                op,
                Box::new(Expr::Var(
                    VarRef::base(Ident::new("true_input")),
                    Loc::default(),
                )),
                Box::new(Expr::Var(
                    VarRef::base(Ident::new("false_input")),
                    Loc::default(),
                )),
                Loc::default(),
            )),
            Box::new(Expr::Const(1.0, Loc::default())),
            Box::new(Expr::Const(0.0, Loc::default())),
            Loc::default(),
        )
    };

    let inputs = BTreeSet::new();
    let mut deps: IdentMap<Ident<Canonical>, DepShape> = Default::default();
    deps.insert(Ident::new("true_input"), DepShape::var(vec![]));
    deps.insert(Ident::new("false_input"), DepShape::var(vec![]));
    let dims_ctx = DimensionsContext::default();
    let var_sizes = super::fragment::reference_extents(&deps);
    let context = test_context(&[], &dims_ctx, &deps, &var_sizes, &inputs);

    for (op, lowered_op) in [
        (ast::BinaryOp::And, BinaryOp::And),
        (ast::BinaryOp::Or, BinaryOp::Or),
    ] {
        let output = context.lower(&lower_if(op)).expect("lowers");
        assert_eq!(expected(lowered_op), output);
    }
}

#[test]
fn test_with_active_subscripts_reuses_dimension_storage() {
    use crate::common::CanonicalDimensionName;

    let dims_ctx = DimensionsContext::default();
    let dims = vec![Dimension::Indexed(
        CanonicalDimensionName::from_raw("letters"),
        3,
    )];
    let deps: IdentMap<Ident<Canonical>, DepShape> = Default::default();
    let inputs = BTreeSet::new();
    let var_sizes = super::fragment::reference_extents(&deps);
    let base = test_context(&dims, &dims_ctx, &deps, &var_sizes, &inputs);

    let active_dims = Arc::<[Dimension]>::from(dims.clone());
    let ctx_a = base.with_active_subscripts(active_dims.clone(), &["1"]);
    let ctx_b = base.with_active_subscripts(active_dims.clone(), &["2"]);

    assert!(Arc::ptr_eq(
        ctx_a.active_dimension.as_ref().unwrap(),
        ctx_b.active_dimension.as_ref().unwrap()
    ));
    assert_eq!(ctx_a.active_subscript.as_ref().unwrap()[0].as_str(), "1");
    assert_eq!(ctx_b.active_subscript.as_ref().unwrap()[0].as_str(), "2");
}

#[test]
fn test_get_implicit_subscript_off_translates_through_mapping_parent() {
    let dim_a = crate::datamodel::Dimension::named(
        "dima".to_string(),
        vec!["a1".to_string(), "a2".to_string(), "a3".to_string()],
    );
    let sub_a = crate::datamodel::Dimension::named(
        "suba".to_string(),
        vec!["a2".to_string(), "a3".to_string()],
    );
    let mut dim_b = crate::datamodel::Dimension::named(
        "dimb".to_string(),
        vec!["b1".to_string(), "b2".to_string(), "b3".to_string()],
    );
    dim_b.set_maps_to("dima".to_string());

    let dims_ctx = DimensionsContext::from(&[dim_a.clone(), sub_a.clone(), dim_b.clone()]);
    let all_dims = vec![
        Dimension::from(&dim_a),
        Dimension::from(&sub_a),
        Dimension::from(&dim_b),
    ];
    let deps: IdentMap<Ident<Canonical>, DepShape> = Default::default();
    let inputs = BTreeSet::new();
    let var_sizes = super::fragment::reference_extents(&deps);
    let base = test_context(&all_dims, &dims_ctx, &deps, &var_sizes, &inputs);

    let active_dims = Arc::<[Dimension]>::from(vec![Dimension::from(&sub_a)]);
    let ctx = base.with_active_subscripts(active_dims, &["a2"]);
    let source_dims = vec![Dimension::from(&dim_b)];

    let off = ctx
        .get_implicit_subscript_off(&source_dims, "src")
        .expect("implicit offset should map suba[a2] -> dimb[b2]");
    assert_eq!(off, 1);
}

#[test]
fn test_positional_fallback_ignores_unrelated_mapping() {
    let mut source = crate::datamodel::Dimension::named(
        "source".to_string(),
        vec!["s1".to_string(), "s2".to_string()],
    );
    let target = crate::datamodel::Dimension::named(
        "target".to_string(),
        vec!["t1".to_string(), "t2".to_string()],
    );
    let other = crate::datamodel::Dimension::named(
        "other".to_string(),
        vec!["o1".to_string(), "o2".to_string()],
    );
    // Mapping to an unrelated dimension should not block positional fallback.
    source.set_maps_to("other".to_string());

    let dims_ctx = DimensionsContext::from(&[source.clone(), target.clone(), other.clone()]);
    let all_dims = vec![
        Dimension::from(&source),
        Dimension::from(&target),
        Dimension::from(&other),
    ];

    let mut deps: IdentMap<Ident<Canonical>, DepShape> = Default::default();
    deps.insert(
        Ident::new("source_var"),
        DepShape::var(vec![Dimension::from(&source)]),
    );
    let inputs = BTreeSet::new();
    let var_sizes = super::fragment::reference_extents(&deps);
    let base = test_context(&all_dims, &dims_ctx, &deps, &var_sizes, &inputs);

    let active_dims = Arc::<[Dimension]>::from(vec![Dimension::from(&target)]);
    let ctx = base.with_active_subscripts(active_dims, &["t2"]);
    let expr = Expr3::Subscript(
        Ident::new("source_var"),
        vec![IndexExpr3::StarRange(
            CanonicalDimensionName::from_raw("source"),
            Loc::default(),
        )],
        None,
        Loc::default(),
    );

    let lowered = ctx
        .lower_from_expr3(&expr)
        .expect("positional fallback should resolve source[*] in target context");
    assert_eq!(
        lowered,
        Expr::Var(VarRef::new(Ident::new("source_var"), 1), Loc::default()),
        "target element t2 should select the second source element"
    );
}

/// The resolver over every shape of name a fragment can spell: a plain name,
/// a module output, an arrayed sub-model variable read per element, a nested
/// `m·n·x`, and the three ways a qualified name leaves the chain (no such
/// module dependency, a non-module head, a missing sub-model variable). Each
/// row states the `VarRef` the standing invariant promises: the instance's
/// name and the variable's slot inside it.
#[test]
fn resolve_walks_module_shapes_and_refuses_loudly_off_the_chain() {
    use super::fragment::{ModelShape, ShapeEntry};
    use crate::common::CanonicalDimensionName;

    let d = Dimension::Indexed(CanonicalDimensionName::from_raw("d"), 3);
    // `inner`: `x` at 0 (scalar), `arr[d]` at 1.
    let inner = Arc::new(ModelShape {
        vars: [
            (
                Ident::new("x"),
                ShapeEntry {
                    offset: 0,
                    shape: DepShape::var(vec![]),
                },
            ),
            (
                Ident::new("arr"),
                ShapeEntry {
                    offset: 1,
                    shape: DepShape::var(vec![d.clone()]),
                },
            ),
        ]
        .into_iter()
        .collect(),
        n_slots: 4,
    });
    // `outer`: `out` at 0, a nested `inner` instance `n` at 1.
    let outer = Arc::new(ModelShape {
        vars: [
            (
                Ident::new("out"),
                ShapeEntry {
                    offset: 0,
                    shape: DepShape::var(vec![]),
                },
            ),
            (
                Ident::new("n"),
                ShapeEntry {
                    offset: 1,
                    shape: DepShape::module(inner.clone()),
                },
            ),
        ]
        .into_iter()
        .collect(),
        n_slots: 5,
    });
    let mut deps: IdentMap<Ident<Canonical>, DepShape> = Default::default();
    deps.insert(Ident::new("plain"), DepShape::var(vec![]));
    deps.insert(Ident::new("m"), DepShape::module(outer));
    let dims = [d.clone()];
    let dims_ctx = DimensionsContext::default();
    let var_sizes = super::fragment::reference_extents(&deps);
    let inputs = BTreeSet::new();
    let ctx = test_context(&dims, &dims_ctx, &deps, &var_sizes, &inputs);

    let var_ref = |name: &str| ctx.get_base_ref(&Ident::new(name));
    assert_eq!(var_ref("plain").unwrap(), VarRef::base(Ident::new("plain")));
    assert_eq!(
        var_ref("m\u{00B7}out").unwrap(),
        VarRef::new(Ident::new("m"), 0)
    );
    assert_eq!(
        var_ref("m\u{00B7}n\u{00B7}x").unwrap(),
        VarRef::new(Ident::new("m"), 1),
        "a nested instance accumulates the offsets on the way down"
    );
    assert_eq!(
        var_ref("m\u{00B7}n\u{00B7}arr").unwrap(),
        VarRef::new(Ident::new("m"), 2)
    );
    // The per-element read of the nested arrayed variable: element `2` of `d`
    // lands one slot past the array's base inside the instance.
    let elem_ctx = ctx.with_active_subscripts(Arc::<[Dimension]>::from(vec![d]), &["2"]);
    assert_eq!(
        elem_ctx
            .get_ref(&Ident::new("m\u{00B7}n\u{00B7}arr"))
            .unwrap(),
        VarRef::new(Ident::new("m"), 3)
    );
    // The dimensions of a qualified name come from the sub-model's shape.
    assert_eq!(
        ctx.get_dimensions("m\u{00B7}n\u{00B7}arr").map(|d| d.len()),
        Some(1)
    );
    assert_eq!(ctx.get_dimensions("m\u{00B7}out"), None);

    for off_the_chain in [
        "ghost\u{00B7}x",         // no such dependency
        "plain\u{00B7}x",         // the head is not a module
        "m\u{00B7}missing",       // no such sub-model variable
        "m\u{00B7}out\u{00B7}x",  // a non-module segment followed by another
        "m\u{00B7}n\u{00B7}nope", // missing in the nested model
    ] {
        let err = var_ref(off_the_chain).expect_err(off_the_chain);
        assert_eq!(err.code, ErrorCode::DoesNotExist, "{off_the_chain}");
    }

    // Every leaf of a module instance, and nothing for the instance itself,
    // is in the extents table, at its slot inside the instance.
    assert_eq!(var_sizes[&VarRef::base(Ident::new("plain"))], 1);
    assert_eq!(var_sizes[&VarRef::new(Ident::new("m"), 0)], 1);
    assert_eq!(var_sizes[&VarRef::new(Ident::new("m"), 1)], 1);
    assert_eq!(var_sizes[&VarRef::new(Ident::new("m"), 2)], 3);
    assert_eq!(var_sizes.len(), 4);
}

#[test]
fn reducer_bare_cosource_uses_active_slot_once() {
    use crate::test_common::TestProject;

    let project = TestProject::new("gh789_reducer_bare_cosource")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .named_dimension("D2", &["x", "y"])
        .array_aux("matrix[D1,D2]", "D1 * D2")
        .array_aux("frac[D1]", "10 * D1")
        .array_aux("solo[D1]", "SUM(frac)")
        .array_aux("growth[D1]", "SUM(matrix[D1,*] * frac)");

    project.assert_vm_result("solo", &[10.0, 20.0]);
    project.assert_vm_result("growth", &[30.0, 120.0]);
}

#[test]
fn rank_sliced_view_inside_reducer_in_arrayed_equation_runs() {
    use crate::test_common::TestProject;

    let project = TestProject::new("gh794_rank_sliced_view")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("D2", &["p", "q"])
        .array_stock("stock[Region]", "100", &[], &[], None)
        .array_aux("pop[Region,D2]", "stock[Region] * D2 * 0.1")
        .array_with_ranges_direct(
            "share",
            vec!["Region".into()],
            vec![
                ("nyc", "SUM(RANK(pop[nyc, *], 1))"),
                ("boston", "SUM(RANK(pop[boston, *], 1))"),
            ],
            None,
        );

    project.assert_vm_result("share", &[3.0, 3.0]);
}
