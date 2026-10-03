// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! An `Equation::Arrayed` written as MDL equations the reader reads back as
//! the same variable: over the same dimensions, with the same element
//! equations and the same `:EXCEPT:` default.
//!
//! An element equation (`x[a1] = 5`) names no dimension, so a variable written
//! one element at a time leaves the reader to infer its dimensions from the
//! element names, which says nothing about which of several dimensions
//! holding those elements the variable is over. So the writer names the
//! dimensions wherever the equations allow it:
//!
//! - an array of numbers over every element of its dimensions is the number
//!   list that names them (`x[DimA] = 1, 2, 3`), or for three dimensions or
//!   more, as Vensim writes one, a list of the last two for each element of
//!   the leading ones;
//! - otherwise the elements are grouped under a left-hand side that names a
//!   dimension wherever every element equation agrees across it
//!   (`x[a1, DimB] = 1`, `x[a2, DimB] = 2`);
//! - a variable with an `:EXCEPT:` default is written as an `:EXCEPT:`
//!   equation over its dimensions, or over a subrange of one that the
//!   default names, followed by the elements it excepts and those outside
//!   it, where that equation defines an element (`except_equations`);
//!   otherwise as the equations of its elements, with a warning that the
//!   default is not kept.
//!
//! The reader decides an axis that no equation names by the elements defined
//! on it (`convert::smallest_dimension_holding`).

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use crate::ast::{Expr0, IndexExpr0, UnaryOp};
use crate::builtins::UntypedBuiltinFn;
use crate::datamodel::{self, GraphicalFunction};
use crate::lexer::LexerType;

use super::{
    ExportWarning, WriterContext, equation_to_mdl, format_mdl_element_key, format_mdl_ident,
    is_external_data_placeholder, is_lookup_only_equation, normalized_stock_initial,
    wrap_active_initial, wrap_equation_with_continuations, wrap_initial, write_lookup,
    write_lookup_body, write_units_and_comment,
};

/// One element of an `Equation::Arrayed`: its key, equation, initial
/// equation and graphical function.
pub(super) type Slot = (String, String, Option<String>, Option<GraphicalFunction>);

/// What an arrayed variable's equations compute.
#[derive(Clone, Copy)]
pub(super) enum Rhs<'a> {
    /// An auxiliary or a flow: each element's equation, wrapped in ACTIVE
    /// INITIAL by its own initial or else the variable's.
    Value { compat: &'a datamodel::Compat },
    /// A stock: each element's equation is its initial value, written as
    /// `INTEG(net flow, initial)`.
    Stock {
        net_flow: &'a str,
        compat: &'a datamodel::Compat,
    },
}

/// An arrayed variable to write.
#[derive(Clone, Copy)]
pub(super) struct Arrayed<'a> {
    /// The display name the entries are written under.
    pub name: &'a str,
    pub dims: &'a [String],
    pub slots: &'a [Slot],
    pub default: &'a Option<String>,
    pub has_except_default: bool,
    pub rhs: Rhs<'a>,
    pub units: &'a Option<String>,
    pub doc: &'a str,
}

/// An element key's canonical parts, one per dimension.
fn key_parts(key: &str) -> Vec<String> {
    key.split(',')
        .map(|part| crate::common::canonicalize(part.trim()).into_owned())
        .collect()
}

/// The canonical element keys of `dim` as a slot spells them, in declared
/// order: a named dimension's element names, an indexed dimension's
/// positions. None for a dimension the project does not declare.
fn axis_keys(dim: &str, ctx: &WriterContext) -> Option<Vec<String>> {
    if let Some(elements) = ctx.dim_named_elements(dim) {
        return Some(
            elements
                .iter()
                .map(|e| crate::common::canonicalize(e).into_owned())
                .collect(),
        );
    }
    let (_, size) = ctx
        .indexed_dims
        .get(crate::common::canonicalize(dim).as_ref())?;
    Some((1..=*size).map(|position| position.to_string()).collect())
}

/// Put an arrayed variable's entries in order of their canonical element keys,
/// ties broken by the key as written.
///
/// The order is a function of the element set alone, never of the order the
/// entries are stored in, which is what makes a write a fixed point: the MDL
/// importer stores elements in canonical key order, and another producer of a
/// datamodel (the XMILE reader, an edit) in any order it likes. Canonical
/// parts are compared because a key part's spelling is not its order (`_`
/// and a space).
pub(super) fn order_arrayed_entries(entries: &mut [Slot]) {
    entries.sort_by_cached_key(|(key, _, _, _)| (key_parts(key), key.clone()));
}

/// Write `var`'s equations to `buf`, the last carrying its units and
/// documentation.
pub(super) fn write_arrayed(
    buf: &mut String,
    var: &Arrayed<'_>,
    ctx: &WriterContext,
    warnings: &mut Vec<ExportWarning>,
) {
    let mut slots = var.slots.to_vec();
    order_arrayed_entries(&mut slots);
    let read_back_dims;
    let var = &match dims_read_back(var, &slots, ctx) {
        Some(dims) => {
            warnings.push(ExportWarning::new(format!(
                "arrayed variable '{}' defines only some elements of {}; MDL has no \
                 spelling for the rest, and the save is read back over {}, the \
                 dimensions of exactly the elements it defines",
                var.name,
                display_dims(var.dims, ctx),
                display_dims(&dims, ctx)
            )));
            read_back_dims = dims;
            Arrayed {
                dims: &read_back_dims,
                ..*var
            }
        }
        None => *var,
    };
    let axes: Option<Vec<Vec<String>>> = var.dims.iter().map(|d| axis_keys(d, ctx)).collect();

    let default = var
        .default
        .as_deref()
        .filter(|d| var.has_except_default && !d.trim().is_empty());
    if let Some(default) = default {
        match except_equations(var, default, &slots, ctx, warnings) {
            Ok(text) => {
                buf.push_str(&text);
                return;
            }
            Err(why) => {
                let taken = take_the_default_as_input(&mut slots, default);
                let undefined = undefined_elements(&slots, axes.as_deref());
                warnings.push(except_fallback_warning(var.name, why, &taken, undefined));
            }
        }
    }

    write_elements(buf, var, &slots, axes.as_deref(), ctx, warnings);
}

/// `dims` as the project declares them, for a warning.
fn display_dims(dims: &[String], ctx: &WriterContext) -> String {
    dims.iter()
        .map(|d| ctx.dimension(d).map_or(d.as_str(), |dim| dim.name.as_str()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Give each slot with a table and no equation of its own the `:EXCEPT:`
/// `default` as its table's input, as the variable computes it while the
/// default applies (an XMILE graphical function arrayed with one shared
/// `<eqn>` reads this way); written without the default, such an element
/// would be a bare lookup, a static table with no value. Returns the keys of
/// the slots changed.
fn take_the_default_as_input(slots: &mut [Slot], default: &str) -> Vec<String> {
    let mut taken = Vec::new();
    for (key, eqn, _, gf) in slots.iter_mut() {
        if gf.is_some() && is_lookup_only_equation(eqn) {
            *eqn = default.to_owned();
            taken.push(key.clone());
        }
    }
    taken
}

/// The keys of the elements of `axes` that no slot holds, which only the
/// `:EXCEPT:` default defines; none when an axis's elements are unknown.
fn undefined_elements(slots: &[Slot], axes: Option<&[Vec<String>]>) -> (Vec<String>, usize) {
    let Some(axes) = axes else {
        return (Vec::new(), 0);
    };
    let held: HashSet<Vec<String>> = slots.iter().map(|slot| key_parts(&slot.0)).collect();
    let total = axes
        .iter()
        .try_fold(1usize, |n, axis| n.checked_mul(axis.len()))
        .unwrap_or(usize::MAX);
    let in_axes = held
        .iter()
        .filter(|key| key.len() == axes.len() && key.iter().zip(axes).all(|(e, a)| a.contains(e)))
        .count();
    // The first few, found by walking the product lazily; the rest counted.
    let mut named = Vec::new();
    let mut at = vec![0usize; axes.len()];
    while named.len() < 12 && axes.iter().all(|a| !a.is_empty()) {
        let key: Vec<String> = at.iter().zip(axes).map(|(&i, a)| a[i].clone()).collect();
        if !held.contains(&key) {
            named.push(key.join(","));
        }
        let Some(axis) = (0..axes.len()).rev().find(|&i| at[i] + 1 < axes[i].len()) else {
            break;
        };
        at[axis] += 1;
        at[axis + 1..].iter_mut().for_each(|i| *i = 0);
    }
    (named, total.saturating_sub(in_axes))
}

/// The warning for an `:EXCEPT:` default written as its elements' equations,
/// saying what happened to each element it touched.
fn except_fallback_warning(
    name: &str,
    why: &str,
    taken: &[String],
    (undefined, count): (Vec<String>, usize),
) -> ExportWarning {
    let mut message = format!(
        "arrayed variable '{name}' has an :EXCEPT: default that {why}; it is written as \
         the equations of its elements and the default is not kept"
    );
    if !taken.is_empty() {
        message.push_str(&format!(
            "; the elements with a table and no equation of their own ({}) take the \
             default as their table's input",
            taken.join("; ")
        ));
    }
    if count > 0 {
        let more = count - undefined.len();
        let more = if more > 0 {
            format!("; and {more} more")
        } else {
            String::new()
        };
        message.push_str(&format!(
            "; the {count} elements only the default defines ({}{more}) are not written",
            undefined.join("; ")
        ));
    }
    ExportWarning::new(message)
}

/// Write `var`'s elements, `slots`, without an `:EXCEPT:` default: as number
/// lists, or under left-hand sides that name a dimension wherever the slots
/// agree across it, or as one apply-to-all equation.
fn write_elements(
    buf: &mut String,
    var: &Arrayed<'_>,
    slots: &[Slot],
    axes: Option<&[Vec<String>]>,
    ctx: &WriterContext,
    warnings: &mut Vec<ExportWarning>,
) {
    if let (Rhs::Value { compat }, Some(axes)) = (&var.rhs, axes)
        && compat.active_initial.is_none()
        && let Some(lists) = number_lists(var.dims, slots, axes, ctx)
    {
        for (at, (lhs, body)) in lists.iter().enumerate() {
            write!(buf, "{}[{lhs}]=\n\t{body}", var.name).unwrap();
            if at + 1 == lists.len() {
                write_units_and_comment(buf, var.units, var.doc);
            } else {
                buf.push_str("\n\t~~|\n");
            }
        }
        return;
    }

    let groups = lhs_groups(var.dims, slots, axes, ctx);
    // One equation over every element is what the reader reads as an
    // apply-to-all, so it is written as one.
    if let [(lhs, (_, eqn, None, None))] = groups.as_slice()
        && *lhs
            == var
                .dims
                .iter()
                .map(|d| format_mdl_ident(d))
                .collect::<Vec<_>>()
                .join(",")
    {
        let dims: Vec<&str> = var.dims.iter().map(String::as_str).collect();
        match var.rhs {
            Rhs::Value { compat } => super::write_single_entry(
                buf,
                var.name,
                &wrap_active_initial(eqn, compat),
                &dims,
                var.units,
                var.doc,
                None,
                ctx,
                warnings,
            ),
            Rhs::Stock { net_flow, compat } => {
                let initial =
                    equation_to_mdl(&wrap_active_initial(eqn, compat), var.name, ctx, warnings);
                super::write_stock_entry(
                    buf, var.name, net_flow, &initial, &dims, var.units, var.doc,
                );
            }
        }
        return;
    }
    write_groups(buf, var, &groups, true, ctx, warnings);
}

/// The dimensions the reader reads `var`'s elements back over, where they
/// are not `var`'s own: on an axis where the slots define only some of the
/// dimension's elements and no `:EXCEPT:` default defines the rest, the
/// reader takes the smallest declared dimension holding the elements defined
/// (`convert::smallest_dimension_holding`, the reader's own rule). None when
/// every axis reads back as it is, or a dimension's elements are unknown.
fn dims_read_back(var: &Arrayed<'_>, slots: &[Slot], ctx: &WriterContext) -> Option<Vec<String>> {
    if var.has_except_default || slots.is_empty() {
        return None;
    }
    let keys: Vec<Vec<String>> = slots.iter().map(|(key, _, _, _)| key_parts(key)).collect();
    let mut read_back = Vec::with_capacity(var.dims.len());
    let mut changed = false;
    for (axis, dim) in var.dims.iter().enumerate() {
        let declared: std::collections::BTreeSet<String> = ctx
            .dim_named_elements(dim)?
            .iter()
            .map(|e| crate::common::canonicalize(e).into_owned())
            .collect();
        let defined: std::collections::BTreeSet<String> = keys
            .iter()
            .filter_map(|key| key.get(axis).cloned())
            .collect();
        if defined == declared {
            read_back.push(dim.clone());
            continue;
        }
        let reader_names: std::collections::BTreeSet<String> = defined
            .iter()
            .map(|e| crate::mdl::builtins::to_lower_space(e))
            .collect();
        let holding =
            super::super::convert::smallest_dimension_holding(&ctx.dimensions, &reader_names)?;
        changed |= crate::common::canonicalize(&holding.name) != crate::common::canonicalize(dim);
        read_back.push(holding.name.clone());
    }
    changed.then_some(read_back)
}

/// A left-hand side's subscripts as written, and the slot whose equation it
/// carries.
type Group<'a> = (String, &'a Slot);

/// Write each group as one equation; the last carries the units and
/// documentation when `last_is_last`.
fn write_groups(
    buf: &mut String,
    var: &Arrayed<'_>,
    groups: &[Group<'_>],
    last_is_last: bool,
    ctx: &WriterContext,
    warnings: &mut Vec<ExportWarning>,
) {
    for (at, (lhs, (_, eqn, initial, gf))) in groups.iter().enumerate() {
        let target = format!("{}[{lhs}]", var.name);
        write_rhs(
            buf,
            var,
            &target,
            eqn,
            initial.as_deref(),
            gf.as_ref(),
            ctx,
            warnings,
        );
        if last_is_last && at + 1 == groups.len() {
            write_units_and_comment(buf, var.units, var.doc);
        } else {
            buf.push_str("\n\t~~|\n");
        }
    }
}

/// Write `target` (the name and subscripts) and its right-hand side: the
/// `=` or `:=`, and the equation.
// Every argument is one piece of the entry being written.
#[allow(clippy::too_many_arguments)]
fn write_rhs(
    buf: &mut String,
    var: &Arrayed<'_>,
    target: &str,
    eqn: &str,
    initial: Option<&str>,
    gf: Option<&GraphicalFunction>,
    ctx: &WriterContext,
    warnings: &mut Vec<ExportWarning>,
) {
    match var.rhs {
        Rhs::Stock { net_flow, compat } => {
            let initial =
                equation_to_mdl(&wrap_active_initial(eqn, compat), var.name, ctx, warnings);
            write!(
                buf,
                "{target}=\n\tINTEG({net_flow}, {})",
                normalized_stock_initial(&initial)
            )
            .unwrap();
        }
        Rhs::Value { compat } => {
            // An element's own initial is its ACTIVE INITIAL: the importer
            // stores an arrayed ACTIVE INITIAL on each element (`X[COP] =
            // ACTIVE INITIAL(expr, init)` as every element of COP with initial
            // `init`).
            let eqn = match initial {
                Some(initial) => wrap_initial(eqn, initial),
                None => wrap_active_initial(eqn, compat),
            };
            let assign = if is_external_data_placeholder(&eqn) {
                ":="
            } else {
                "="
            };
            match gf {
                Some(gf) if is_lookup_only_equation(&eqn) => {
                    write!(buf, "{target}(\n\t").unwrap();
                    write_lookup_body(buf, gf);
                    buf.push(')');
                }
                Some(gf) => {
                    let mdl = equation_to_mdl(&eqn, var.name, ctx, warnings);
                    write!(buf, "{target}{assign}\n\tWITH LOOKUP({mdl}, ").unwrap();
                    write_lookup(buf, gf);
                    buf.push(')');
                }
                None => {
                    let mdl = equation_to_mdl(&eqn, var.name, ctx, warnings);
                    write!(
                        buf,
                        "{target}{assign}\n\t{}",
                        wrap_equation_with_continuations(&mdl, 80)
                    )
                    .unwrap();
                }
            }
        }
    }
}

// ---- :EXCEPT: ----

/// How a variable's `:EXCEPT:` default is written: its `:EXCEPT:` equation
/// over `lhs`, the slots that equation excepts, each written after it as its
/// own equation, and the slots outside `lhs`, written after those.
struct ExceptPlan {
    /// The left-hand side's subscripts, canonical: each the variable's
    /// dimension on its axis, or a subrange of it the default names.
    lhs: Vec<String>,
    excepted: Vec<Slot>,
    outside: Vec<Slot>,
}

/// The equations that write `var`'s `:EXCEPT:` default, or why none do. The
/// left-hand side is the variable's dimensions, with an axis narrowed to a
/// subrange the default names, as the file the variable was read from wrote
/// it. An equation that would define no element is not written: a default
/// naming a range the left-hand side does not hold (a mapped one, say) is
/// one no element is (`defined_by_default`), so such a default is written as
/// its elements' equations instead.
fn except_equations(
    var: &Arrayed<'_>,
    default: &str,
    slots: &[Slot],
    ctx: &WriterContext,
    warnings: &mut Vec<ExportWarning>,
) -> Result<String, &'static str> {
    if is_external_data_placeholder(default) {
        return Err("reads external data");
    }
    let Ok(Some(default_ast)) = Expr0::new(default, LexerType::Equation) else {
        return Err("does not parse");
    };
    let dims: Vec<String> = var
        .dims
        .iter()
        .map(|d| crate::common::canonicalize(d).into_owned())
        .collect();
    let lhs = narrowed_to_subranges(&dims, &ranges_named(&default_ast, ctx), ctx);
    let plan = except_plan(&default_ast, default, slots, lhs, ctx)?;
    let mut text = String::new();
    write_except(&mut text, var, default, &plan, ctx, warnings);
    Ok(text)
}

/// The subscript ranges `expr` names without a `!`, canonical: the names in
/// its subscripts that are dimensions of the project.
fn ranges_named(expr: &Expr0, ctx: &WriterContext) -> Vec<String> {
    fn walk(expr: &Expr0, ctx: &WriterContext, out: &mut Vec<String>) {
        match expr {
            Expr0::Const(..) | Expr0::Var(..) => {}
            Expr0::App(UntypedBuiltinFn(_, args), _) => {
                args.iter().for_each(|a| walk(a, ctx, out));
            }
            Expr0::Subscript(_, indices, _) => {
                for index in indices {
                    match index {
                        IndexExpr0::Expr(Expr0::Var(name, _)) => {
                            let name = crate::common::canonicalize(name.as_str()).into_owned();
                            if ctx.dimension(&name).is_some() {
                                out.push(name);
                            }
                        }
                        IndexExpr0::Expr(e) => walk(e, ctx, out),
                        IndexExpr0::Range(l, r, _) => {
                            walk(l, ctx, out);
                            walk(r, ctx, out);
                        }
                        IndexExpr0::Wildcard(_)
                        | IndexExpr0::StarRange(_, _)
                        | IndexExpr0::DimPosition(_, _) => {}
                    }
                }
            }
            Expr0::Op1(_, e, _) => walk(e, ctx, out),
            Expr0::Op2(_, l, r, _) => {
                walk(l, ctx, out);
                walk(r, ctx, out);
            }
            Expr0::If(c, t, f, _) => {
                walk(c, ctx, out);
                walk(t, ctx, out);
                walk(f, ctx, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(expr, ctx, &mut out);
    out
}

/// `dims` with each axis the default does not name replaced by the first of
/// `named` that is a proper subrange of it (a dimension holding some, not
/// all, of its elements) and that no other axis took.
fn narrowed_to_subranges(dims: &[String], named: &[String], ctx: &WriterContext) -> Vec<String> {
    let mut taken: Vec<&String> = Vec::new();
    dims.iter()
        .map(|dim| {
            if named.contains(dim) {
                return dim.clone();
            }
            let Some(whole) = axis_keys(dim, ctx) else {
                return dim.clone();
            };
            let subrange = named.iter().find(|range| {
                !dims.contains(range)
                    && !taken.contains(range)
                    && axis_keys(range, ctx).is_some_and(|part| {
                        part.len() < whole.len() && part.iter().all(|e| whole.contains(e))
                    })
            });
            match subrange {
                Some(range) => {
                    taken.push(range);
                    range.clone()
                }
                None => dim.clone(),
            }
        })
        .collect()
}

/// The plan for an `:EXCEPT:` equation over `lhs`: the slots it covers that
/// the default does not define (those whose equation is not the default for
/// their element, or that carry an initial or a table), and the slots it does
/// not cover. Err when it would define no element.
///
/// A default that defines every element it covers is written with one
/// element excepted all the same: an `:EXCEPT:` equation with nothing
/// excepted is not one, and the reader keeps a default only beside the
/// equations of the elements it excepts. Which element is excepted is
/// arbitrary; it is a function of the covered product alone (its last
/// element), so every save of the variable excepts the same one. Without a
/// slot of its own its equation is the default, which can be written for it
/// only where the default names none of the left-hand side's ranges.
fn except_plan(
    default_ast: &Expr0,
    default: &str,
    slots: &[Slot],
    lhs: Vec<String>,
    ctx: &WriterContext,
) -> Result<ExceptPlan, &'static str> {
    let axes: Vec<Vec<String>> = lhs
        .iter()
        .map(|range| axis_keys(range, ctx))
        .collect::<Option<_>>()
        .ok_or("is over a dimension whose elements the writer does not know")?;
    let covers = |key: &[String]| {
        key.len() == axes.len() && key.iter().zip(&axes).all(|(e, axis)| axis.contains(e))
    };
    let (covered, outside): (Vec<Slot>, Vec<Slot>) = slots
        .iter()
        .cloned()
        .partition(|slot| covers(&key_parts(&slot.0)));
    let mut excepted: Vec<Slot> = covered
        .iter()
        .filter(|slot| !defined_by_default(default_ast, slot, &lhs))
        .cloned()
        .collect();
    let covered_count: usize = axes.iter().map(Vec::len).product();
    if excepted.is_empty() {
        let last: Vec<String> = axes
            .iter()
            .map(|elements| elements.last().cloned())
            .collect::<Option<_>>()
            .ok_or("is over a dimension with no elements")?;
        match covered.iter().find(|(key, _, _, _)| key_parts(key) == last) {
            Some(slot) => excepted.push(slot.clone()),
            None if !names_any_of(default_ast, &lhs) => {
                let key: Option<Vec<String>> = lhs
                    .iter()
                    .zip(&axes)
                    .map(|(range, elements)| ctx.element_at(range, elements.len()))
                    .collect();
                let key = key.ok_or("is over a dimension with no elements")?;
                excepted.push((key.join(","), default.to_owned(), None, None));
            }
            None => return Err("defines every element and names its own dimensions"),
        }
    }
    if excepted.len() >= covered_count {
        return Err("defines no element its :EXCEPT: equation names");
    }
    Ok(ExceptPlan {
        lhs,
        excepted,
        outside,
    })
}

/// Whether `expr` names any of `dims` (canonical), as a subscript or a
/// value.
fn names_any_of(expr: &Expr0, dims: &[String]) -> bool {
    let named = |name: &str| dims.contains(&crate::common::canonicalize(name).into_owned());
    match expr {
        Expr0::Const(..) => false,
        Expr0::Var(name, _) => named(name.as_str()),
        Expr0::App(UntypedBuiltinFn(_, args), _) => args.iter().any(|a| names_any_of(a, dims)),
        Expr0::Subscript(_, indices, _) => indices.iter().any(|index| match index {
            IndexExpr0::Expr(e) => names_any_of(e, dims),
            IndexExpr0::Range(l, r, _) => names_any_of(l, dims) || names_any_of(r, dims),
            IndexExpr0::StarRange(name, _) => named(name.as_str()),
            IndexExpr0::Wildcard(_) | IndexExpr0::DimPosition(_, _) => false,
        }),
        Expr0::Op1(_, e, _) => names_any_of(e, dims),
        Expr0::Op2(_, l, r, _) => names_any_of(l, dims) || names_any_of(r, dims),
        Expr0::If(c, t, f, _) => {
            names_any_of(c, dims) || names_any_of(t, dims) || names_any_of(f, dims)
        }
    }
}

/// Whether `slot` is what the `:EXCEPT:` equation of `default` over `lhs`
/// (canonical ranges) makes of its element: no initial or table, and the
/// default with each of `lhs` replaced by the slot's element.
fn defined_by_default(default: &Expr0, slot: &Slot, lhs: &[String]) -> bool {
    let (key, eqn, initial, gf) = slot;
    initial.is_none()
        && gf.is_none()
        && matches!(Expr0::new(eqn, LexerType::Equation), Ok(Some(slot))
            if same_with_elements(default, &slot, lhs, &key_parts(key)))
}

/// Write the `:EXCEPT:` equation, the equations of the elements it excepts,
/// and those of the elements outside its left-hand side.
fn write_except(
    buf: &mut String,
    var: &Arrayed<'_>,
    default: &str,
    plan: &ExceptPlan,
    ctx: &WriterContext,
    warnings: &mut Vec<ExportWarning>,
) {
    let lhs: Vec<String> = plan
        .lhs
        .iter()
        .map(|range| format_mdl_ident(ctx.dimension(range).map_or(range.as_str(), |d| &d.name)))
        .collect();
    let excepted: Vec<String> = plan
        .excepted
        .iter()
        .map(|(key, _, _, _)| format!("[{}]", format_mdl_element_key(key, &plan.lhs, ctx)))
        .collect();
    let target = format!(
        "{}[{}] :EXCEPT: {}",
        var.name,
        lhs.join(","),
        excepted.join(", ")
    );
    write_rhs(buf, var, &target, default, None, None, ctx, warnings);
    buf.push_str("\n\t~~|\n");
    let mut groups: Vec<Group<'_>> = plan
        .excepted
        .iter()
        .map(|slot| (format_mdl_element_key(&slot.0, var.dims, ctx), slot))
        .collect();
    let axes: Option<Vec<Vec<String>>> = var.dims.iter().map(|d| axis_keys(d, ctx)).collect();
    groups.extend(lhs_groups(var.dims, &plan.outside, axes.as_deref(), ctx));
    write_groups(buf, var, &groups, true, ctx, warnings);
}

/// Whether `a` and `b` are two spellings of one equation: names compared
/// canonically and numbers by value, as the reader respells both. Text that
/// does not parse compares as written, trimmed.
#[cfg(test)]
pub(in crate::mdl) fn same_equation(a: &str, b: &str) -> bool {
    match (
        Expr0::new(a, LexerType::Equation),
        Expr0::new(b, LexerType::Equation),
    ) {
        (Ok(Some(a)), Ok(Some(b))) => same_with_elements(&a, &b, &[], &[]),
        _ => a.trim() == b.trim(),
    }
}

/// Whether `slot` is `default` with each of the variable's dimensions named
/// in a subscript replaced by the slot's element of it, as the reader makes
/// an element of an `:EXCEPT:` equation. Names compare canonically and
/// numbers by value.
fn same_with_elements(default: &Expr0, slot: &Expr0, dims: &[String], key: &[String]) -> bool {
    let same = |a: &Expr0, b: &Expr0| same_with_elements(a, b, dims, key);
    let canon = |name: &str| crate::common::canonicalize(name).into_owned();
    // PI is written as its value: Vensim has no PI function.
    let is_pi = |e: &Expr0| matches!(e, Expr0::App(UntypedBuiltinFn(f, args), _) if f == "pi" && args.is_empty());
    match (default, slot) {
        (Expr0::Const(_, a, _), Expr0::Const(_, b, _)) => {
            a.value().to_bits() == b.value().to_bits()
        }
        (pi, Expr0::Const(_, n, _)) | (Expr0::Const(_, n, _), pi) if is_pi(pi) => {
            n.value().to_bits() == std::f64::consts::PI.to_bits()
        }
        (Expr0::Var(a, _), Expr0::Var(b, _)) => canon(a.as_str()) == canon(b.as_str()),
        (Expr0::App(UntypedBuiltinFn(f, xs), _), Expr0::App(UntypedBuiltinFn(g, ys), _)) => {
            f == g && xs.len() == ys.len() && xs.iter().zip(ys.iter()).all(|(x, y)| same(x, y))
        }
        (Expr0::Subscript(a, xs, _), Expr0::Subscript(b, ys, _)) => {
            canon(a.as_str()) == canon(b.as_str())
                && xs.len() == ys.len()
                && xs.iter().zip(ys.iter()).all(|(x, y)| match (x, y) {
                    (IndexExpr0::Expr(Expr0::Var(d, _)), IndexExpr0::Expr(Expr0::Var(e, _)))
                        if dims.contains(&canon(d.as_str())) =>
                    {
                        let at = dims.iter().position(|dim| *dim == canon(d.as_str()));
                        at.and_then(|at| key.get(at)) == Some(&canon(e.as_str()))
                    }
                    (IndexExpr0::Expr(x), IndexExpr0::Expr(y)) => same(x, y),
                    (IndexExpr0::Wildcard(_), IndexExpr0::Wildcard(_)) => true,
                    (IndexExpr0::StarRange(x, _), IndexExpr0::StarRange(y, _)) => {
                        canon(x.as_str()) == canon(y.as_str())
                    }
                    (IndexExpr0::Range(xl, xr, _), IndexExpr0::Range(yl, yr, _)) => {
                        same(xl, yl) && same(xr, yr)
                    }
                    (IndexExpr0::DimPosition(x, _), IndexExpr0::DimPosition(y, _)) => x == y,
                    _ => false,
                })
        }
        (Expr0::Op1(o, x, _), Expr0::Op1(p, y, _)) => o == p && same(x, y),
        (Expr0::Op2(o, xl, xr, _), Expr0::Op2(p, yl, yr, _)) => {
            o == p && same(xl, yl) && same(xr, yr)
        }
        (Expr0::If(xc, xt, xf, _), Expr0::If(yc, yt, yf, _)) => {
            same(xc, yc) && same(xt, yt) && same(xf, yf)
        }
        _ => false,
    }
}

// ---- number lists ----

/// The number `eqn` is, a literal or a negated one, as the reader stores it
/// (`format_number`), so the list reads back as it is written. None for a
/// negative zero, which the reader stores from a list as zero: written as an
/// element's own equation, `-0`, it reads back as itself.
fn number_text(eqn: &str) -> Option<String> {
    let value = match Expr0::new(eqn, LexerType::Equation) {
        Ok(Some(Expr0::Const(_, n, _))) => n.value(),
        Ok(Some(Expr0::Op1(UnaryOp::Negative, inner, _))) => match *inner {
            Expr0::Const(_, n, _) => -n.value(),
            _ => return None,
        },
        _ => return None,
    };
    (value.is_finite() && !(value == 0.0 && value.is_sign_negative()))
        .then(|| crate::mdl::xmile_compat::format_number(value))
}

/// The number lists that define `slots` over `dims`, whose element keys are
/// `axes`, each with its left-hand side's subscripts: over one dimension one
/// list naming it (`1, 2, 3`); over two, one list of rows (`1, 2; 3, 4;`);
/// over more, as Vensim writes such an array, one list of the last two
/// dimensions for each element of the leading ones (`x[a1, DimB, DimC] = 1,
/// 2; 3, 4;`). Elements are in declared order. None unless every element of
/// the dimensions' product has a slot that is a number and nothing else.
fn number_lists(
    dims: &[String],
    slots: &[Slot],
    axes: &[Vec<String>],
    ctx: &WriterContext,
) -> Option<Vec<(String, String)>> {
    let expected: usize = axes.iter().map(Vec::len).product();
    if axes.is_empty() || expected == 0 || slots.len() != expected {
        return None;
    }
    let mut by_key: HashMap<Vec<String>, String> = HashMap::new();
    for (key, eqn, initial, gf) in slots {
        if initial.is_some() || gf.is_some() {
            return None;
        }
        if by_key.insert(key_parts(key), number_text(eqn)?).is_some() {
            return None;
        }
    }
    let named: Vec<String> = dims.iter().map(|d| format_mdl_ident(d)).collect();
    if let [only] = axes {
        let cells: Option<Vec<String>> = only
            .iter()
            .map(|e| by_key.get(&vec![e.clone()]).cloned())
            .collect();
        return Some(vec![(named[0].clone(), cells?.join(", "))]);
    }
    let lead = axes.len() - 2;
    // Every combination of the leading axes' positions, in row-major order.
    let mut prefixes: Vec<Vec<usize>> = vec![Vec::new()];
    for axis in &axes[..lead] {
        prefixes = prefixes
            .into_iter()
            .flat_map(|prefix| {
                (0..axis.len()).map(move |at| {
                    let mut next = prefix.clone();
                    next.push(at);
                    next
                })
            })
            .collect();
    }
    let mut lists = Vec::with_capacity(prefixes.len());
    for prefix in prefixes {
        let mut lhs: Vec<String> = Vec::with_capacity(axes.len());
        for (axis, &at) in prefix.iter().enumerate() {
            lhs.push(ctx.element_at(&dims[axis], at + 1)?);
        }
        lhs.extend(named[lead..].iter().cloned());
        let mut body = String::new();
        for row in &axes[lead] {
            let cells: Option<Vec<String>> = axes[lead + 1]
                .iter()
                .map(|col| {
                    let mut key: Vec<String> = prefix
                        .iter()
                        .enumerate()
                        .map(|(axis, &at)| axes[axis][at].clone())
                        .collect();
                    key.push(row.clone());
                    key.push(col.clone());
                    by_key.get(&key).cloned()
                })
                .collect();
            body.push_str(&cells?.join(", "));
            body.push(';');
        }
        lists.push((lhs.join(","), body));
    }
    Some(lists)
}

// ---- grouping ----

/// The equations `slots` are written as: a left-hand side's subscripts and
/// the slot whose equation it carries, in the slots' order. A dimension every
/// slot agrees across (every element of it has a slot with the same
/// equation, initial and table, the other subscripts held) is named in the
/// left-hand side; any other position holds the slot's element.
fn lhs_groups<'a>(
    dims: &[String],
    slots: &'a [Slot],
    axes: Option<&[Vec<String>]>,
    ctx: &WriterContext,
) -> Vec<Group<'a>> {
    let elementwise = || {
        slots
            .iter()
            .map(|slot| (format_mdl_element_key(&slot.0, dims, ctx), slot))
            .collect()
    };
    let Some(axes) = axes else {
        return elementwise();
    };
    let keys: Vec<Vec<String>> = slots.iter().map(|(key, _, _, _)| key_parts(key)).collect();
    if keys.iter().any(|key| key.len() != axes.len()) {
        return elementwise();
    }
    let index: HashMap<&[String], usize> = keys
        .iter()
        .enumerate()
        .map(|(at, key)| (key.as_slice(), at))
        .collect();
    let same = |a: &Slot, b: &Slot| a.1 == b.1 && a.2 == b.2 && a.3 == b.3;
    let free: Vec<bool> = axes
        .iter()
        .enumerate()
        .map(|(axis, elements)| {
            !elements.is_empty()
                && keys.iter().enumerate().all(|(at, key)| {
                    elements.iter().all(|element| {
                        let mut other = key.clone();
                        other[axis] = element.clone();
                        index
                            .get(other.as_slice())
                            .is_some_and(|&j| same(&slots[at], &slots[j]))
                    })
                })
        })
        .collect();
    if !free.iter().any(|f| *f) {
        return elementwise();
    }
    let mut seen: HashSet<Vec<&str>> = HashSet::new();
    let mut groups = Vec::new();
    for (key, slot) in keys.iter().zip(slots) {
        let pinned: Vec<&str> = key
            .iter()
            .zip(&free)
            .map(|(part, free)| if *free { "" } else { part.as_str() })
            .collect();
        if !seen.insert(pinned) {
            continue;
        }
        let written: Vec<String> = slot
            .0
            .split(',')
            .zip(dims)
            .zip(&free)
            .map(|((part, dim), free)| {
                if *free {
                    format_mdl_ident(dim)
                } else {
                    format_mdl_element_key(part, std::slice::from_ref(dim), ctx)
                }
            })
            .collect();
        groups.push((written.join(","), slot));
    }
    groups
}

#[cfg(test)]
#[path = "writer_arrayed_tests.rs"]
mod tests;
