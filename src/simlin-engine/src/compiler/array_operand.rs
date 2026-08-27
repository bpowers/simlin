// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The single materialization pass for lowered array expressions.
//!
//! Subscript resolution runs before this module. Consequently every source
//! view is concrete and every apply-to-all element has its final scalar
//! arguments. This pass then establishes codegen's array contract: an array
//! operand is a view over variable, snapshot, or temp storage, and every
//! array-producing builtin is the right-hand side of one `AssignTemp`.
//!
//! Each top-level assignment owns an allocator element scope. Sequential array
//! elements therefore reuse one dense id range. An exact final expression and
//! output view may reuse the first write to that recycled slot when recursive
//! purity proves it stable. Definitions that depend on the assignment target
//! or mutable evaluation state are never shared. Resolved SCC assembly rejects
//! any temp whose definition and uses cross reorderable element segments.

use crate::ast::{ArrayView, TempAllocator};
use crate::builtins::{ArgKind, BuiltinSig, Invariance, ResultKind};
use crate::common::{Canonical, CanonicalDimensionName, CanonicalElementName, Ident};
use crate::compiler::expr::{BuiltinFn, Expr, SubscriptIndex};
use crate::dimensions::{Axis, AxisMatch, Dimension, DimensionsContext, match_axes_partial};

#[derive(Clone, Copy)]
enum ValueUse<'a> {
    Scalar,
    Array { target: Option<&'a ArrayView> },
    Element { index: usize, view: &'a ArrayView },
}

/// Materialize every computed array operand and array-producing result after
/// subscript resolution.
pub(super) fn materialize_arrays(
    exprs: Vec<Expr>,
    target_view: Option<&ArrayView>,
    dimensions: &DimensionsContext,
    temps: &TempAllocator,
) -> Vec<Expr> {
    let scopes = temps.element_scopes();
    let assigned_targets = exprs
        .iter()
        .filter_map(|expr| match expr {
            Expr::AssignCurr(dst, _) | Expr::AssignNext(dst, _) => Some(dst.name.clone()),
            _ => None,
        })
        .collect();
    let mut definitions = DefinitionCache::new(assigned_targets);
    let mut out = Vec::with_capacity(exprs.len());
    for expr in exprs {
        scopes.begin_element();
        let mut before = Vec::new();
        let expr = match expr {
            Expr::AssignCurr(dst, rhs) => {
                let usage = target_view.map_or(ValueUse::Scalar, |view| ValueUse::Element {
                    index: dst.element_offset,
                    view,
                });
                Expr::AssignCurr(
                    dst,
                    Box::new(rewrite(
                        *rhs,
                        usage,
                        dimensions,
                        temps,
                        &mut definitions,
                        &mut before,
                    )),
                )
            }
            Expr::AssignNext(dst, rhs) => {
                let usage = target_view.map_or(ValueUse::Scalar, |view| ValueUse::Element {
                    index: dst.element_offset,
                    view,
                });
                Expr::AssignNext(
                    dst,
                    Box::new(rewrite(
                        *rhs,
                        usage,
                        dimensions,
                        temps,
                        &mut definitions,
                        &mut before,
                    )),
                )
            }
            other => rewrite(
                other,
                ValueUse::Scalar,
                dimensions,
                temps,
                &mut definitions,
                &mut before,
            ),
        };
        out.extend(before);
        out.push(expr);
    }
    out
}

fn rewrite(
    expr: Expr,
    usage: ValueUse<'_>,
    dimensions: &DimensionsContext,
    temps: &TempAllocator,
    definitions: &mut DefinitionCache,
    before: &mut Vec<Expr>,
) -> Expr {
    match expr {
        Expr::App(builtin, loc) => {
            // Capture the stable signature name before `map_with_kinds`
            // consumes the builtin.
            let name = builtin.name().to_ascii_lowercase().replace(' ', "_");
            let result_target = match usage {
                ValueUse::Array { target } => target,
                ValueUse::Element { view, .. } => Some(view),
                ValueUse::Scalar => None,
            };
            let shape_from = match builtin.result_kind() {
                ResultKind::Array { shape_from } => Some(shape_from as usize),
                ResultKind::Scalar | ResultKind::Elementwise => None,
            };
            let mut position = 0usize;
            let builtin = builtin.map_with_kinds(|arg, kind| {
                let child_usage = match kind {
                    ArgKind::Array { .. } => ValueUse::Array {
                        target: (shape_from == Some(position))
                            .then_some(result_target)
                            .flatten(),
                    },
                    ArgKind::Scalar | ArgKind::Table => usage,
                    ArgKind::Ident => {
                        unreachable!("an identifier payload is not an expression argument")
                    }
                };
                let arg = rewrite(arg, child_usage, dimensions, temps, definitions, before);
                let policy = operand_policy(&name, position, kind);
                position += 1;
                match policy {
                    OperandPolicy::Materialize => {
                        let target = match child_usage {
                            ValueUse::Array { target } => target,
                            ValueUse::Scalar | ValueUse::Element { .. } => None,
                        };
                        materialize_operand(arg, target, dimensions, temps, definitions, before)
                    }
                    OperandPolicy::Identity => arg,
                    OperandPolicy::NotExpression => {
                        unreachable!("BuiltinFn::map_with_kinds does not visit identifier payloads")
                    }
                }
            });
            materialize_result(
                Expr::App(builtin, loc),
                usage,
                dimensions,
                temps,
                definitions,
                before,
            )
        }
        Expr::Op1(op, inner, loc) => Expr::Op1(
            op,
            Box::new(rewrite(
                *inner,
                usage,
                dimensions,
                temps,
                definitions,
                before,
            )),
            loc,
        ),
        Expr::Op2(op, lhs, rhs, loc) => Expr::Op2(
            op,
            Box::new(rewrite(*lhs, usage, dimensions, temps, definitions, before)),
            Box::new(rewrite(*rhs, usage, dimensions, temps, definitions, before)),
            loc,
        ),
        Expr::If(cond, then_expr, else_expr, loc) => Expr::If(
            Box::new(rewrite(
                *cond,
                usage,
                dimensions,
                temps,
                definitions,
                before,
            )),
            Box::new(rewrite(
                *then_expr,
                usage,
                dimensions,
                temps,
                definitions,
                before,
            )),
            Box::new(rewrite(
                *else_expr,
                usage,
                dimensions,
                temps,
                definitions,
                before,
            )),
            loc,
        ),
        Expr::Subscript(base, indices, bounds, loc) => {
            let indices = indices
                .into_iter()
                .map(|idx| match idx {
                    SubscriptIndex::Single(e) => SubscriptIndex::Single(rewrite(
                        e,
                        ValueUse::Scalar,
                        dimensions,
                        temps,
                        definitions,
                        before,
                    )),
                    SubscriptIndex::Range(start, end) => SubscriptIndex::Range(
                        rewrite(
                            start,
                            ValueUse::Scalar,
                            dimensions,
                            temps,
                            definitions,
                            before,
                        ),
                        rewrite(
                            end,
                            ValueUse::Scalar,
                            dimensions,
                            temps,
                            definitions,
                            before,
                        ),
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
                .map(|arg| {
                    rewrite(
                        arg,
                        ValueUse::Scalar,
                        dimensions,
                        temps,
                        definitions,
                        before,
                    )
                })
                .collect(),
        ),
        Expr::AssignTemp(id, rhs, view) => Expr::AssignTemp(
            id,
            Box::new(rewrite(
                *rhs,
                ValueUse::Array {
                    target: Some(&view),
                },
                dimensions,
                temps,
                definitions,
                before,
            )),
            view,
        ),
        leaf @ (Expr::Const(_, _)
        | Expr::Var(_, _)
        | Expr::StaticSubscript(_, _, _)
        | Expr::TempArray(_, _, _)
        | Expr::TempArrayElement(_, _, _, _)
        | Expr::Dt(_)
        | Expr::ModuleInput(_, _)) => leaf,
        Expr::AssignCurr(_, _) | Expr::AssignNext(_, _) => {
            unreachable!("assignments only occur at fragment top level")
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OperandPolicy {
    Materialize,
    Identity,
    NotExpression,
}

fn operand_policy(name: &str, position: usize, kind: ArgKind) -> OperandPolicy {
    match kind {
        // ALLOCATE AVAILABLE's profile is expanded during lowering to the
        // complete requester x priority view. A computed profile has no
        // defined expansion, so preserve codegen's loud rejection.
        ArgKind::Array { .. } if name == "allocate_available" && position == 1 => {
            OperandPolicy::Identity
        }
        ArgKind::Array { .. } => OperandPolicy::Materialize,
        ArgKind::Scalar | ArgKind::Table => OperandPolicy::Identity,
        ArgKind::Ident => OperandPolicy::NotExpression,
    }
}

fn materialize_operand(
    mut operand: Expr,
    target: Option<&ArrayView>,
    dimensions: &DimensionsContext,
    temps: &TempAllocator,
    definitions: &mut DefinitionCache,
    before: &mut Vec<Expr>,
) -> Expr {
    if is_view(&operand) || is_snapshot_view(&operand) {
        return operand;
    }
    let Some(source_view) = super::find_array_operand_view(&mut operand, target, dimensions) else {
        // An incomparable or unknown shape remains for codegen's structured
        // rejection. Inventing an axis order would silently mis-broadcast.
        return operand;
    };
    if source_view.dims.is_empty() || super::view_repeats_a_dimension(&source_view) {
        return operand;
    }
    let view = compact_view(source_view);
    let loc = operand.get_loc();
    let id = definitions.materialize(operand, &view, temps, before);
    Expr::TempArray(id, view, loc)
}

fn materialize_result(
    expr: Expr,
    usage: ValueUse<'_>,
    dimensions: &DimensionsContext,
    temps: &TempAllocator,
    definitions: &mut DefinitionCache,
    before: &mut Vec<Expr>,
) -> Expr {
    let Expr::App(builtin, _) = &expr else {
        unreachable!()
    };
    let (source_view, requires_axis) = match builtin.result_kind() {
        // A collapsed source is still a one-element array result. Codegen's
        // array-producing opcode therefore still needs an `AssignTemp` even
        // when the view has zero surviving axes.
        ResultKind::Array { shape_from } => {
            let inferred = super::find_expr_array_view(&expr);
            if inferred.is_none() && has_array_shape(builtin.args()[shape_from as usize]) {
                // A runtime-sized range or incomparable collection of views
                // cannot be represented by one fixed temp view. Preserve the
                // raw call for codegen's structured rejection rather than
                // pretending it is the degenerate one-element result.
                return expr;
            }
            (
                Some(inferred.unwrap_or_else(|| ArrayView::contiguous(Vec::new()))),
                false,
            )
        }
        ResultKind::Scalar => (arrayed_lookup_view(builtin), true),
        ResultKind::Elementwise => (None, true),
    };
    let Some(source_view) = source_view else {
        return expr;
    };
    if requires_axis && source_view.dims.is_empty() {
        return expr;
    }
    let view = compact_view(source_view);
    let loc = expr.get_loc();
    let id = definitions.materialize(expr, &view, temps, before);
    match usage {
        ValueUse::Array { .. } => Expr::TempArray(id, view, loc),
        ValueUse::Element {
            index,
            view: target_view,
        } => match project_element(index, target_view, &view, dimensions) {
            Some(element) => Expr::TempArrayElement(id, view.clone(), element, loc),
            // Every result axis must correspond to a target axis. Keeping the
            // view makes codegen report its canonical "array where a single
            // value is required" diagnostic; choosing coordinate zero here
            // would silently accept `out[COP] = LOOKUP(g[COP,ROW], Time)`.
            None => Expr::TempArray(id, view, loc),
        },
        ValueUse::Scalar => Expr::TempArray(id, view, loc),
    }
}

#[derive(Clone, PartialEq)]
struct CachedDefinition {
    expr: Expr,
    view: ArrayView,
}

/// Definitions cached by recycled physical temp id for one phase's ordered
/// assignment sequence. A cache hit is dominated by the first write because
/// materialization emits that write before its assignment, and a slot's entry
/// is replaced whenever an intervening element gives the id another meaning.
struct DefinitionCache {
    assigned_targets: Vec<Ident<Canonical>>,
    by_temp: Vec<Option<CachedDefinition>>,
}

impl DefinitionCache {
    fn new(assigned_targets: Vec<Ident<Canonical>>) -> Self {
        Self {
            assigned_targets,
            by_temp: Vec::new(),
        }
    }

    fn materialize(
        &mut self,
        expr: Expr,
        view: &ArrayView,
        temps: &TempAllocator,
        before: &mut Vec<Expr>,
    ) -> u32 {
        let reusable = definition_is_reusable(&expr, &self.assigned_targets);
        let definition = CachedDefinition {
            expr: expr.clone(),
            view: view.clone(),
        };
        let id = temps.alloc();
        let slot = id as usize;
        if self.by_temp.len() <= slot {
            self.by_temp.resize_with(slot + 1, || None);
        }
        if reusable && self.by_temp[slot].as_ref() == Some(&definition) {
            return id;
        }
        self.by_temp[slot] = reusable.then_some(definition);
        before.push(Expr::AssignTemp(id, Box::new(expr), view.clone()));
        id
    }
}

/// Whether one resolved definition is stable for the rest of this phase's
/// assignment sequence. `BuiltinSig::invariance` is the semantic boundary:
/// only pure calls qualify. The lookup family qualifies: its table identity
/// selects immutable entries in `CompiledContext::graphical_functions`, and
/// both VM and wasm lookup opcodes only read those entries. Target reads,
/// module evaluation and nested temps can observe state that an earlier
/// assignment or materialization changed, so none can be reused.
fn definition_is_reusable(expr: &Expr, assigned_targets: &[Ident<Canonical>]) -> bool {
    let reads_assigned_target = |name: &Ident<Canonical>| assigned_targets.contains(name);
    match expr {
        Expr::Const(_, _) | Expr::Dt(_) | Expr::ModuleInput(_, _) => true,
        Expr::Var(var, _) | Expr::StaticSubscript(var, _, _) => !reads_assigned_target(&var.name),
        Expr::Subscript(var, indices, _, _) => {
            !reads_assigned_target(&var.name)
                && indices.iter().all(|index| match index {
                    SubscriptIndex::Single(expr) => definition_is_reusable(expr, assigned_targets),
                    SubscriptIndex::Range(start, end) => {
                        definition_is_reusable(start, assigned_targets)
                            && definition_is_reusable(end, assigned_targets)
                    }
                })
        }
        Expr::App(builtin, _) => {
            signature_definition_is_reusable(builtin.signature())
                && builtin
                    .args()
                    .iter()
                    .all(|arg| definition_is_reusable(arg, assigned_targets))
        }
        Expr::Op1(_, inner, _) => definition_is_reusable(inner, assigned_targets),
        Expr::Op2(_, lhs, rhs, _) => {
            definition_is_reusable(lhs, assigned_targets)
                && definition_is_reusable(rhs, assigned_targets)
        }
        Expr::If(cond, then_expr, else_expr, _) => {
            definition_is_reusable(cond, assigned_targets)
                && definition_is_reusable(then_expr, assigned_targets)
                && definition_is_reusable(else_expr, assigned_targets)
        }
        Expr::TempArray(_, _, _)
        | Expr::TempArrayElement(_, _, _, _)
        | Expr::EvalModule(_, _, _, _)
        | Expr::AssignCurr(_, _)
        | Expr::AssignNext(_, _)
        | Expr::AssignTemp(_, _, _) => false,
    }
}

fn signature_definition_is_reusable(signature: &BuiltinSig) -> bool {
    signature.invariance == Invariance::Pure
}

/// Whether an expression contains evidence of an array shape that the view
/// inference above failed to represent. Absence is meaningful: an array
/// builtin over scalar/fixed-element inputs has a real one-element result.
fn has_array_shape(expr: &Expr) -> bool {
    if super::find_expr_array_view(expr).is_some() {
        return true;
    }
    match expr {
        Expr::Subscript(_, indices, _, _) => indices
            .iter()
            .any(|index| matches!(index, SubscriptIndex::Range(..))),
        Expr::App(builtin, _) => builtin.args().into_iter().any(has_array_shape),
        Expr::Op1(_, inner, _) => has_array_shape(inner),
        Expr::Op2(_, lhs, rhs, _) => has_array_shape(lhs) || has_array_shape(rhs),
        Expr::If(cond, then_expr, else_expr, _) => {
            has_array_shape(cond) || has_array_shape(then_expr) || has_array_shape(else_expr)
        }
        Expr::AssignTemp(_, inner, _) | Expr::AssignCurr(_, inner) | Expr::AssignNext(_, inner) => {
            has_array_shape(inner)
        }
        Expr::EvalModule(_, _, _, args) => args.iter().any(has_array_shape),
        Expr::Const(_, _)
        | Expr::Var(_, _)
        | Expr::StaticSubscript(_, _, _)
        | Expr::TempArray(_, _, _)
        | Expr::TempArrayElement(_, _, _, _)
        | Expr::Dt(_)
        | Expr::ModuleInput(_, _) => false,
    }
}

/// Lookup over an array of graphical functions is array-valued despite the
/// lookup family's ordinary scalar signature. Codegen selects `LookupArray`
/// precisely when the table view retains an axis.
fn arrayed_lookup_view(builtin: &BuiltinFn) -> Option<ArrayView> {
    let table = match builtin {
        BuiltinFn::Lookup(table, _, _)
        | BuiltinFn::LookupForward(table, _, _)
        | BuiltinFn::LookupBackward(table, _, _) => table.as_ref(),
        _ => return None,
    };
    super::find_expr_array_view(table)
}

fn compact_view(view: ArrayView) -> ArrayView {
    if view.dim_names.len() == view.dims.len() {
        ArrayView::contiguous_with_names(view.dims, view.dim_names)
    } else {
        ArrayView::contiguous(view.dims)
    }
}

/// Project a target's row-major element onto a result temp. The shared matcher
/// allocates axes one-to-one, so repeated names pair by occurrence:
/// `[D,D] -> [D,D]` is `0->0, 1->1`, never `0->0, 1->0`.
fn project_element(
    index: usize,
    target: &ArrayView,
    result: &ArrayView,
    dimensions: &DimensionsContext,
) -> Option<usize> {
    let target_coords = coordinates(index, &target.dims);
    let mapping = if target.dim_names.len() == target.dims.len()
        && result.dim_names.len() == result.dims.len()
        && target.dim_names.iter().all(|name| !name.is_empty())
        && result.dim_names.iter().all(|name| !name.is_empty())
    {
        let result_axes: Vec<_> = result
            .dim_names
            .iter()
            .zip(&result.dims)
            .map(|(name, &len)| Axis::named(name, len))
            .collect();
        let target_axes: Vec<_> = target
            .dim_names
            .iter()
            .zip(&target.dims)
            .map(|(name, &len)| Axis::named(name, len))
            .collect();
        match_axes_partial(&result_axes, &target_axes, dimensions)
            .into_iter()
            .collect::<Vec<_>>()
    } else if result.dims == target.dims {
        (0..result.dims.len())
            .map(|axis| Some((axis, crate::dimensions::AxisMatch::Exact)))
            .collect()
    } else {
        vec![None; result.dims.len()]
    };

    let result_coords: Option<Vec<usize>> = mapping
        .into_iter()
        .enumerate()
        .map(|(result_axis, matched)| {
            let (target_axis, relation) = matched?;
            match relation {
                AxisMatch::Exact | AxisMatch::BySize => Some(target_coords[target_axis]),
                AxisMatch::Mapped { .. } | AxisMatch::Subdimension => {
                    let target_name = target.dim_names.get(target_axis)?;
                    let result_name = result.dim_names.get(result_axis)?;
                    mapped_coordinate(
                        target_coords[target_axis],
                        target_name,
                        result_name,
                        dimensions,
                    )
                }
            }
        })
        .collect();
    Some(linear_index(&result_coords?, &result.dims))
}

/// Translate a target coordinate through the same name-first, then declared
/// mapping rule used when lowering a source read. Axis pairing alone is not
/// enough for an explicit element map: the target's ordinal may select a
/// differently-positioned source element.
fn mapped_coordinate(
    target_coord: usize,
    target_name: &str,
    result_name: &str,
    dimensions: &DimensionsContext,
) -> Option<usize> {
    let target_dim = dimensions.get(&CanonicalDimensionName::from_raw(target_name))?;
    let result_dim = dimensions.get(&CanonicalDimensionName::from_raw(result_name))?;
    let target_element = match target_dim {
        Dimension::Named(_, named) => named.elements.get(target_coord)?.clone(),
        Dimension::Indexed(_, size) if target_coord < *size as usize => {
            CanonicalElementName::from_raw(&(target_coord + 1).to_string())
        }
        Dimension::Indexed(..) => return None,
    };
    let result_element = dimensions.resolve_mapped_read(result_dim, target_dim, &target_element)?;
    result_dim.get_offset(&result_element)
}

fn coordinates(mut index: usize, dims: &[usize]) -> Vec<usize> {
    let mut coords = vec![0; dims.len()];
    for axis in (0..dims.len()).rev() {
        coords[axis] = index % dims[axis];
        index /= dims[axis];
    }
    coords
}

fn linear_index(coords: &[usize], dims: &[usize]) -> usize {
    coords
        .iter()
        .zip(dims)
        .fold(0, |index, (&coord, &len)| index * len + coord)
}

fn is_snapshot_view(expr: &Expr) -> bool {
    matches!(expr, Expr::App(builtin, _) if super::snapshot_view_arg(builtin).is_some())
}

fn is_view(expr: &Expr) -> bool {
    matches!(
        expr,
        Expr::StaticSubscript(_, _, _)
            | Expr::TempArray(_, _, _)
            | Expr::Var(_, _)
            | Expr::Subscript(_, _, _, _)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Loc;
    use crate::compiler::VarRef;

    /// The signature table is the exhaustive row source. Result semantics are
    /// exercised end-to-end by `array_tests` and
    /// `array_operand_materialization_tests`; arrayed lookup's extra result
    /// shape is exercised by `per_element_gf_tests` because it depends on a
    /// production graphical-function registry rather than a signature row.
    #[test]
    fn every_signature_row_has_an_explicit_materialization_policy() {
        let mut array_results = Vec::new();
        for sig in BuiltinSig::ALL {
            if matches!(sig.result, ResultKind::Array { .. }) {
                array_results.push(sig.name);
            }
            for (position, &kind) in sig.arg_kinds.iter().enumerate() {
                let policy = operand_policy(sig.name, position, kind);
                match kind {
                    ArgKind::Array { .. } if sig.name == "allocate_available" && position == 1 => {
                        assert_eq!(policy, OperandPolicy::Identity);
                    }
                    ArgKind::Array { .. } => {
                        assert_eq!(policy, OperandPolicy::Materialize);
                    }
                    ArgKind::Scalar | ArgKind::Table => {
                        assert_eq!(policy, OperandPolicy::Identity);
                    }
                    ArgKind::Ident => assert_eq!(policy, OperandPolicy::NotExpression),
                }
            }
        }
        assert_eq!(
            array_results,
            [
                "rank",
                "vector_elm_map",
                "vector_sort_order",
                "allocate_available",
                "allocate_by_priority",
            ]
        );
    }

    /// `BuiltinSig::ALL` is also the exhaustive source for definition reuse.
    /// Pure functions are eligible, including the three immutable-table
    /// lookup rows. Each mutable-time class is ineligible even when its
    /// resolved argument tree is textually equal.
    #[test]
    fn every_signature_row_has_an_explicit_definition_reuse_class() {
        let mut reusable = Vec::new();
        let mut table = Vec::new();
        let mut time_dependent = Vec::new();
        let mut lagged = Vec::new();
        let mut snapshot = Vec::new();
        for sig in BuiltinSig::ALL {
            let eligible = signature_definition_is_reusable(sig);
            match (sig.invariance, sig.arg_kinds.contains(&ArgKind::Table)) {
                (Invariance::Pure, false) => {
                    assert!(eligible, "{} is a pure value function", sig.name);
                    reusable.push(sig.name);
                }
                (Invariance::Pure, true) => {
                    assert!(eligible, "{} only reads immutable table identity", sig.name);
                    reusable.push(sig.name);
                    table.push(sig.name);
                }
                (Invariance::TimeDependent, _) => {
                    assert!(!eligible, "{} reads mutable time", sig.name);
                    time_dependent.push(sig.name);
                }
                (Invariance::Lagged, _) => {
                    assert!(!eligible, "{} reads the previous snapshot", sig.name);
                    lagged.push(sig.name);
                }
                (Invariance::Snapshot, _) => {
                    assert!(!eligible, "{} reads the initial snapshot", sig.name);
                    snapshot.push(sig.name);
                }
            }
        }
        assert_eq!(
            reusable.len() + time_dependent.len() + lagged.len() + snapshot.len(),
            BuiltinSig::ALL.len()
        );
        assert_eq!(table, ["lookup", "lookup_forward", "lookup_backward"]);
        assert_eq!(time_dependent, ["pulse", "ramp", "step", "time"]);
        assert_eq!(lagged, ["previous"]);
        assert_eq!(snapshot, ["init"]);
        assert_eq!(reusable.len(), 38);
    }

    #[test]
    fn definition_reuse_rejects_mutable_and_temp_dependent_expression_classes() {
        let loc = Loc::default();
        let c = || Expr::Const(1.0, loc);
        let target = Ident::new("out");
        let assigned_targets = [target.clone()];

        assert!(definition_is_reusable(
            &Expr::App(BuiltinFn::Abs(Box::new(c())), loc),
            &assigned_targets
        ));
        assert!(!definition_is_reusable(
            &Expr::App(BuiltinFn::Time, loc),
            &assigned_targets
        ));
        assert!(!definition_is_reusable(
            &Expr::App(BuiltinFn::Previous(Box::new(c()), Box::new(c())), loc),
            &assigned_targets
        ));
        assert!(!definition_is_reusable(
            &Expr::App(BuiltinFn::Init(Box::new(c())), loc),
            &assigned_targets
        ));
        assert!(definition_is_reusable(
            &Expr::App(BuiltinFn::Lookup(Box::new(c()), Box::new(c()), loc), loc),
            &assigned_targets
        ));
        assert!(!definition_is_reusable(
            &Expr::TempArray(0, ArrayView::contiguous(vec![1]), loc),
            &assigned_targets
        ));
        assert!(!definition_is_reusable(
            &Expr::EvalModule(
                Ident::new("m"),
                Ident::new("submodel"),
                Default::default(),
                vec![c()],
            ),
            &assigned_targets
        ));
        assert!(!definition_is_reusable(
            &Expr::Var(VarRef::base(target), loc),
            &assigned_targets
        ));
    }

    #[test]
    fn repeated_axes_project_by_occurrence() {
        let dimensions = DimensionsContext::default();
        let target =
            ArrayView::contiguous_with_names(vec![2, 2], vec!["d".to_owned(), "d".to_owned()]);
        assert_eq!(project_element(0, &target, &target, &dimensions), Some(0));
        assert_eq!(project_element(1, &target, &target, &dimensions), Some(1));
        assert_eq!(project_element(2, &target, &target, &dimensions), Some(2));
        assert_eq!(project_element(3, &target, &target, &dimensions), Some(3));
    }

    #[test]
    fn unmatched_result_axis_cannot_be_projected_to_coordinate_zero() {
        let dimensions = DimensionsContext::default();
        let target = ArrayView::contiguous_with_names(vec![3], vec!["cop".to_owned()]);
        let result = ArrayView::contiguous_with_names(vec![2], vec!["row".to_owned()]);
        assert_eq!(project_element(0, &target, &result, &dimensions), None);
    }
}
