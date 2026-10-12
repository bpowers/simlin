// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Edits by field: the patch operations that change one part of one
//! variable -- its equation, one element's equation, its units, notes,
//! lookup or dimensions -- and leave the rest as it is. A host and the agent
//! tools both edit this way, so each operation means one thing to both.
//!
//! An arrayed variable is stored as the equation every element follows (an
//! apply-to-all equation, or an arrayed equation's default) and the elements
//! with equations of their own (its arms, in its dimensions' declared
//! order). Each operation keeps that form at its smallest: a variable whose
//! last own cell is cleared becomes apply-to-all again, and one with no
//! dimensions left becomes a scalar. A stock's equation is its initial value.

use crate::common::{CanonicalElementName, Error, ErrorCode, ErrorKind, Result, canonicalize};
use crate::datamodel::{self, Equation, GraphicalFunction, Variable};
use crate::patch::Site;

/// One element's arm as `Equation::Arrayed` stores it: its subscript, its
/// equation, its active initial, and its lookup.
pub(crate) type Arm = (String, String, Option<String>, Option<GraphicalFunction>);

pub(crate) fn refuse(code: ErrorCode, details: String) -> Error {
    Error::new(ErrorKind::Model, code, Some(details))
}

/// Replace the variable `ident` by what `edit` makes of it, leaving it in the
/// allocation it is in when the edit changes nothing.
fn edit_variable(
    model: &mut datamodel::Model,
    ident: &str,
    edit: impl FnOnce(&Variable) -> Result<Variable>,
) -> Result<()> {
    let canonical = canonicalize(ident);
    let index = model
        .variables
        .iter()
        .position(|var| canonicalize(var.get_ident()) == canonical)
        .ok_or_else(|| {
            refuse(
                ErrorCode::DoesNotExist,
                format!("there is no variable named '{ident}'"),
            )
        })?;
    let edited = edit(&model.variables[index])?;
    if edited != model.variables[index]
        && let Some(slot) = model.variables.get_mut(index)
    {
        *slot = edited;
    }
    Ok(())
}

fn equation_mut(var: &mut Variable) -> Result<&mut Equation> {
    match var {
        Variable::Stock(stock) => Ok(&mut stock.equation),
        Variable::Flow(flow) => Ok(&mut flow.equation),
        Variable::Aux(aux) => Ok(&mut aux.equation),
        Variable::Module(module) => Err(refuse(
            ErrorCode::Generic,
            format!("'{}' is a module, which has no equation", module.ident),
        )),
    }
}

fn gf_mut(var: &mut Variable) -> Result<&mut Option<GraphicalFunction>> {
    match var {
        Variable::Flow(flow) => Ok(&mut flow.gf),
        Variable::Aux(aux) => Ok(&mut aux.gf),
        Variable::Stock(_) | Variable::Module(_) => Err(refuse(
            ErrorCode::Generic,
            format!(
                "'{}' cannot have a lookup: only an auxiliary or a flow can",
                var.get_ident()
            ),
        )),
    }
}

/// The project's dimension `name`, by canonical name.
fn dimension<'a>(dims: &'a [datamodel::Dimension], name: &str) -> Result<&'a datamodel::Dimension> {
    let canonical = canonicalize(name);
    dims.iter()
        .find(|d| canonicalize(d.name()) == canonical)
        .ok_or_else(|| {
            refuse(
                ErrorCode::BadDimensionName,
                format!("'{name}' is not a declared dimension"),
            )
        })
}

/// A dimension's elements as the model spells them; an indexed one's are
/// `1` to its size.
pub(crate) fn elements_of(dim: &datamodel::Dimension) -> Vec<String> {
    match &dim.elements {
        datamodel::DimensionElements::Named(names) => names.clone(),
        datamodel::DimensionElements::Indexed(size) => (1..=*size).map(|n| n.to_string()).collect(),
    }
}

/// The element of `dim` that `written` names, as the model spells it.
fn element_in(dim: &datamodel::Dimension, written: &str) -> Result<String> {
    let canonical = canonicalize(written.trim());
    elements_of(dim)
        .into_iter()
        .find(|e| canonicalize(e) == canonical)
        .ok_or_else(|| {
            refuse(
                ErrorCode::BadDimensionName,
                format!("'{written}' is not an element of '{}'", dim.name()),
            )
        })
}

/// Every element of `dims`, row-major, as the model spells each part.
fn combinations(dims: &[&datamodel::Dimension]) -> Vec<Vec<String>> {
    dims.iter().fold(vec![Vec::new()], |acc, dim| {
        let elements = elements_of(dim);
        acc.iter()
            .flat_map(|prefix| {
                elements.iter().map(move |e| {
                    let mut next = prefix.clone();
                    next.push(e.clone());
                    next
                })
            })
            .collect()
    })
}

fn key(parts: &[String]) -> CanonicalElementName {
    CanonicalElementName::from_parts(parts)
}

/// `arms` in the row-major order of `dims`, arms naming no element of them
/// after, in the order they were in.
pub(crate) fn in_dimension_order(dims: &[&datamodel::Dimension], arms: Vec<Arm>) -> Vec<Arm> {
    let order: Vec<CanonicalElementName> =
        combinations(dims).iter().map(|parts| key(parts)).collect();
    let mut arms: Vec<(usize, Arm)> = arms
        .into_iter()
        .map(|arm| {
            let at = order
                .iter()
                .position(|k| *k == CanonicalElementName::from_subscript(&arm.0))
                .unwrap_or(usize::MAX);
            (at, arm)
        })
        .collect();
    arms.sort_by_key(|(at, _)| *at);
    arms.into_iter().map(|(_, arm)| arm).collect()
}

/// An arrayed equation from its default and arms, in its smallest form: no
/// arms left and a default every element follows is apply-to-all.
pub(crate) fn arrayed(
    dims: Vec<String>,
    arms: Vec<Arm>,
    default: Option<String>,
    applies: bool,
) -> Equation {
    match default {
        Some(default) if applies && arms.is_empty() => Equation::ApplyToAll(dims, default),
        default => Equation::Arrayed(dims, arms, default, applies),
    }
}

/// `setEquation`: a scalar's equation (a stock's initial value), or the
/// equation every element of an arrayed variable follows. Elements with
/// equations of their own keep them.
pub(crate) fn set_equation(model: &mut datamodel::Model, ident: &str, text: String) -> Result<()> {
    edit_variable(model, ident, |var| {
        let mut var = var.clone();
        let equation = equation_mut(&mut var)?;
        *equation = match std::mem::replace(equation, Equation::Scalar(String::new())) {
            Equation::Scalar(_) => Equation::Scalar(text),
            Equation::ApplyToAll(dims, _) => Equation::ApplyToAll(dims, text),
            Equation::Arrayed(dims, arms, _, _) => arrayed(dims, arms, Some(text), true),
        };
        Ok(var)
    })
}

/// `setElementEquation`: one element's own equation, or with `None` -- or a
/// text with nothing in it -- none, so it follows the equation every element
/// follows again (its lookup goes with it). `element` names the element on
/// each of the variable's dimensions, in order.
pub(crate) fn set_element_equation(
    project_dims: &[datamodel::Dimension],
    model: &mut datamodel::Model,
    ident: &str,
    element: &[String],
    text: Option<String>,
) -> Result<()> {
    edit_variable(model, ident, |var| {
        let mut var = var.clone();
        let name = var.get_ident().to_string();
        let equation = equation_mut(&mut var)?;
        let (dims, mut arms, default, applies) =
            match std::mem::replace(equation, Equation::Scalar(String::new())) {
                Equation::Scalar(_) => {
                    return Err(refuse(
                        ErrorCode::Generic,
                        format!("'{name}' is not arrayed, so it has no elements"),
                    ));
                }
                Equation::ApplyToAll(dims, every) => (dims, Vec::new(), Some(every), true),
                Equation::Arrayed(dims, arms, default, applies) => (dims, arms, default, applies),
            };
        let resolved: Vec<&datamodel::Dimension> = dims
            .iter()
            .map(|d| dimension(project_dims, d))
            .collect::<Result<_>>()?;
        if element.len() != resolved.len() {
            return Err(refuse(
                ErrorCode::MismatchedDimensions,
                format!(
                    "'{name}' has {} dimensions, so an element names {} parts, not {}",
                    resolved.len(),
                    resolved.len(),
                    element.len()
                ),
            ));
        }
        let parts: Vec<String> = resolved
            .iter()
            .zip(element)
            .map(|(dim, written)| element_in(dim, written))
            .collect::<Result<_>>()?;
        let target = key(&parts);
        let at = arms
            .iter()
            .position(|arm| CanonicalElementName::from_subscript(&arm.0) == target);
        // An empty equation is no equation of the element's own: stored, it
        // would be one no format but the engine's own reads as following.
        let text = text.filter(|text| !text.trim().is_empty());
        match (at, text) {
            (Some(at), Some(text)) => arms[at].1 = text,
            (None, Some(text)) => arms.push((parts.join(","), text, None, None)),
            (Some(at), None) => {
                arms.remove(at);
            }
            (None, None) => {}
        }
        let arms = in_dimension_order(&resolved, arms);
        *equation = arrayed(dims, arms, default, applies);
        Ok(var)
    })
}

/// `setUnits`: the variable's units; `None` or an empty string clears them.
pub(crate) fn set_units(
    model: &mut datamodel::Model,
    ident: &str,
    units: Option<String>,
) -> Result<()> {
    let units = units
        .map(|u| u.trim().to_string())
        .filter(|u| !u.is_empty());
    edit_variable(model, ident, |var| {
        let mut var = var.clone();
        match &mut var {
            Variable::Stock(stock) => stock.units = units,
            Variable::Flow(flow) => flow.units = units,
            Variable::Aux(aux) => aux.units = units,
            Variable::Module(module) => module.units = units,
        }
        Ok(var)
    })
}

/// `setNotes`: the variable's documentation; empty clears it.
pub(crate) fn set_notes(model: &mut datamodel::Model, ident: &str, notes: String) -> Result<()> {
    edit_variable(model, ident, |var| {
        let mut var = var.clone();
        match &mut var {
            Variable::Stock(stock) => stock.documentation = notes,
            Variable::Flow(flow) => flow.documentation = notes,
            Variable::Aux(aux) => aux.documentation = notes,
            Variable::Module(module) => module.documentation = notes,
        }
        Ok(var)
    })
}

/// `setLookup`: the variable's lookup table, or with `element` one
/// element's; `None` removes it. An element given a lookup and no equation
/// of its own takes the equation every element follows as its own, the
/// lookup's input -- written out, so every format keeps it (an MDL element
/// lookup is a `WITH LOOKUP` of its own input) -- and an element whose
/// lookup is removed and whose equation is that one follows it again.
pub(crate) fn set_lookup(
    project_dims: &[datamodel::Dimension],
    model: &mut datamodel::Model,
    ident: &str,
    element: Option<&[String]>,
    lookup: Option<GraphicalFunction>,
) -> Result<()> {
    edit_variable(model, ident, |var| {
        let mut var = var.clone();
        let name = var.get_ident().to_string();
        let Some(element) = element else {
            *gf_mut(&mut var)? = lookup;
            return Ok(var);
        };
        gf_mut(&mut var)?;
        let equation = equation_mut(&mut var)?;
        let (dims, mut arms, default, applies) =
            match std::mem::replace(equation, Equation::Scalar(String::new())) {
                Equation::Scalar(_) => {
                    return Err(refuse(
                        ErrorCode::Generic,
                        format!("'{name}' is not arrayed, so it has no elements"),
                    ));
                }
                Equation::ApplyToAll(dims, every) => (dims, Vec::new(), Some(every), true),
                Equation::Arrayed(dims, arms, default, applies) => (dims, arms, default, applies),
            };
        let resolved: Vec<&datamodel::Dimension> = dims
            .iter()
            .map(|d| dimension(project_dims, d))
            .collect::<Result<_>>()?;
        if element.len() != resolved.len() {
            return Err(refuse(
                ErrorCode::MismatchedDimensions,
                format!(
                    "'{name}' has {} dimensions, so an element names {} parts, not {}",
                    resolved.len(),
                    resolved.len(),
                    element.len()
                ),
            ));
        }
        let parts: Vec<String> = resolved
            .iter()
            .zip(element)
            .map(|(dim, written)| element_in(dim, written))
            .collect::<Result<_>>()?;
        let target = key(&parts);
        let at = arms
            .iter()
            .position(|arm| CanonicalElementName::from_subscript(&arm.0) == target);
        let every = default.clone().filter(|_| applies);
        match (at, lookup) {
            (Some(at), lookup) => {
                arms[at].3 = lookup;
                // An element left with no lookup, no initial and no equation
                // but the one every element follows has nothing of its own.
                let text = arms[at].1.trim();
                if arms[at].3.is_none()
                    && arms[at].2.is_none()
                    && (text.is_empty() || every.as_deref().is_some_and(|e| e.trim() == text))
                {
                    arms.remove(at);
                }
            }
            (None, Some(lookup)) => {
                let input = every.clone().unwrap_or_default();
                arms.push((parts.join(","), input, None, Some(lookup)));
            }
            (None, None) => {}
        }
        let arms = in_dimension_order(&resolved, arms);
        *equation = arrayed(dims, arms, default, applies);
        Ok(var)
    })
}

/// `setDimensions`: the dimensions a variable is arrayed over, in order;
/// none makes it a scalar.
///
/// A dimension added repeats the variable along it: a scalar's equation
/// becomes the equation every element follows, and each element's own
/// equation is its equation at every element of the new dimension. A
/// dimension removed keeps the elements of `keep` -- one element per removed
/// dimension, in the order the variable had them -- or, without `keep`, needs
/// the variable to say the same thing at every element of the removed
/// dimensions, and is refused, naming two elements that differ, when it does
/// not. A removed dimension the equations name (`c[Age]`, `Age > 1`) is
/// read at the element `keep` names (`c[old]`, `Age.old > 1`), and without
/// `keep` the removal is refused, since the elements then differ in what they
/// compute. An element kept for its lookup alone reads the equation every
/// element follows through it.
pub(crate) fn set_dimensions(
    project_dims: &[datamodel::Dimension],
    model: &mut datamodel::Model,
    ident: &str,
    new_dims: &[String],
    keep: Option<&[String]>,
) -> Result<()> {
    edit_variable(model, ident, |var| {
        let mut var = var.clone();
        let name = var.get_ident().to_string();
        let new: Vec<&datamodel::Dimension> = new_dims
            .iter()
            .map(|d| dimension(project_dims, d))
            .collect::<Result<_>>()?;
        for (at, dim) in new.iter().enumerate() {
            if new[..at].iter().any(|d| d.name() == dim.name()) {
                return Err(refuse(
                    ErrorCode::MismatchedDimensions,
                    format!("'{}' is named twice", dim.name()),
                ));
            }
        }
        let equation = equation_mut(&mut var)?;
        let (old_names, arms, default, applies) = match equation.clone() {
            Equation::Scalar(text) => (Vec::new(), Vec::new(), Some(text), true),
            Equation::ApplyToAll(dims, every) => (dims, Vec::new(), Some(every), true),
            Equation::Arrayed(dims, arms, default, applies) => (dims, arms, default, applies),
        };
        let old: Vec<&datamodel::Dimension> = old_names
            .iter()
            .map(|d| dimension(project_dims, d))
            .collect::<Result<_>>()?;
        if old
            .iter()
            .map(|d| d.name())
            .eq(new.iter().map(|d| d.name()))
        {
            return Ok(var);
        }
        let removed: Vec<usize> = (0..old.len())
            .filter(|&at| !new.iter().any(|d| d.name() == old[at].name()))
            .collect();
        let kept: Option<Vec<String>> = match keep {
            None => None,
            Some(keep) => {
                if keep.len() != removed.len() {
                    return Err(refuse(
                        ErrorCode::MismatchedDimensions,
                        format!(
                            "{} dimensions are removed, so `keep` names {} elements, not {}",
                            removed.len(),
                            removed.len(),
                            keep.len()
                        ),
                    ));
                }
                Some(
                    removed
                        .iter()
                        .zip(keep)
                        .map(|(&at, written)| element_in(old[at], written))
                        .collect::<Result<_>>()?,
                )
            }
        };
        let arm_at = |parts: &[String]| -> Option<&Arm> {
            let target = key(parts);
            arms.iter()
                .find(|arm| CanonicalElementName::from_subscript(&arm.0) == target)
        };
        // The old element each new element reads, along the removed
        // dimensions, at each choice of them `keep` leaves open.
        let removed_choices: Vec<Vec<String>> = match &kept {
            Some(kept) => vec![kept.clone()],
            None => combinations(&removed.iter().map(|&at| old[at]).collect::<Vec<_>>()),
        };
        let old_parts = |new_parts: &[String], choice: &[String]| -> Vec<String> {
            old.iter()
                .enumerate()
                .map(
                    |(at, dim)| match new.iter().position(|d| d.name() == dim.name()) {
                        Some(new_at) => new_parts[new_at].clone(),
                        None => {
                            let removed_at = removed.iter().position(|&r| r == at).unwrap();
                            choice[removed_at].clone()
                        }
                    },
                )
                .collect()
        };
        let mut new_arms: Vec<Arm> = Vec::new();
        for parts in combinations(&new) {
            let readings: Vec<(Vec<String>, Option<&Arm>)> = removed_choices
                .iter()
                .map(|choice| {
                    let from = old_parts(&parts, choice);
                    let arm = arm_at(&from);
                    (from, arm)
                })
                .collect();
            let first = readings[0].1;
            let same = |a: Option<&Arm>, b: Option<&Arm>| match (a, b) {
                (None, None) => true,
                (Some(a), Some(b)) => a.1 == b.1 && a.2 == b.2 && a.3 == b.3,
                _ => false,
            };
            if let Some((from, _)) = readings.iter().find(|(_, arm)| !same(first, *arm)) {
                return Err(refuse(
                    ErrorCode::MismatchedDimensions,
                    format!(
                        "'{name}' differs at '{}' and '{}', so removing a dimension needs \
                         the element whose equation is kept",
                        readings[0].0.join(","),
                        from.join(",")
                    ),
                ));
            }
            if let Some(arm) = first {
                new_arms.push((parts.join(","), arm.1.clone(), arm.2.clone(), arm.3.clone()));
            }
        }
        let default = default.filter(|_| applies);
        // The removed dimensions the equations name, read at the kept
        // element; refused without one.
        let removed_dims: Vec<&datamodel::Dimension> = removed.iter().map(|&at| old[at]).collect();
        let at_kept = |text: &str| -> Result<String> {
            let names_one = crate::patch::text_references(text).is_some_and(|refs| {
                refs.iter().any(|(site, reference)| {
                    !matches!(site, Site::StarRange | Site::Subscripted)
                        && removed_dims
                            .iter()
                            .any(|d| canonicalize(d.name()).as_ref() == reference.as_str())
                })
            });
            if !names_one {
                return Ok(text.to_string());
            }
            let Some(kept) = &kept else {
                return Err(refuse(
                    ErrorCode::MismatchedDimensions,
                    format!(
                        "'{name}' names a dimension it would no longer have (`{text}`), so its \
                         elements differ, and removing the dimension needs the element whose \
                         equation is kept"
                    ),
                ));
            };
            let at = |reference: &str| {
                removed_dims
                    .iter()
                    .position(|d| canonicalize(d.name()).as_ref() == reference)
            };
            // A position is written qualified (`Age.old`), which a name in
            // quotes cannot be.
            let unwritable = crate::patch::text_references(text).and_then(|refs| {
                refs.iter().find_map(|(site, reference)| {
                    let k = at(reference.as_str()).filter(|_| *site == Site::Value)?;
                    let written =
                        crate::patch_dimensions::qualified(removed_dims[k].name(), &kept[k]);
                    written.contains('"').then_some((k, written))
                })
            });
            if let Some((k, written)) = unwritable {
                return Err(refuse(
                    ErrorCode::BadDimensionName,
                    format!(
                        "'{name}' reads '{dim}' as a position (`{text}`), which at '{element}' would \
                         be written `{written}`, and a name written in quotes cannot qualify an \
                         element: keep an element whose name needs no quotes, rename '{element}' \
                         to one first, or write the position as a number in the equation",
                        dim = removed_dims[k].name(),
                        element = kept[k],
                    ),
                ));
            }
            Ok(
                crate::patch_dimensions::respelled(text, &|site, reference| {
                    let k = at(reference)?;
                    match site {
                        Site::Index => Some(crate::patch_dimensions::spelled(&kept[k])),
                        Site::Value => Some(crate::patch_dimensions::qualified(
                            removed_dims[k].name(),
                            &kept[k],
                        )),
                        Site::StarRange | Site::Subscripted => None,
                    }
                })
                .unwrap_or_else(|| text.to_string()),
            )
        };
        let default = default.map(|text| at_kept(&text)).transpose()?;
        let mut new_arms: Vec<Arm> = new_arms
            .into_iter()
            .map(|(key, text, gf_text, gf)| Ok((key, at_kept(&text)?, gf_text, gf)))
            .collect::<Result<_>>()?;
        if new.is_empty() {
            let (text, initial, gf) = match new_arms.pop() {
                // An element with a lookup and no equation of its own reads
                // the equation every element follows through it.
                Some((_, text, initial, gf)) if text.trim().is_empty() && gf.is_some() => {
                    (default.unwrap_or_default(), initial, gf)
                }
                Some((_, text, initial, gf)) => (text, initial, gf),
                None => (default.unwrap_or_default(), None, None),
            };
            *equation = Equation::Scalar(text);
            if let Some(gf) = gf {
                *gf_mut(&mut var)? = Some(gf);
            }
            // The kept element's initial is the scalar's.
            if let Some(initial) = initial {
                match &mut var {
                    Variable::Stock(v) => v.compat.active_initial = Some(initial),
                    Variable::Flow(v) => v.compat.active_initial = Some(initial),
                    Variable::Aux(v) => v.compat.active_initial = Some(initial),
                    Variable::Module(_) => {}
                }
            }
            return Ok(var);
        }
        let names: Vec<String> = new.iter().map(|d| d.name().to_string()).collect();
        let applies = default.is_some();
        *equation = arrayed(names, new_arms, default, applies);
        Ok(var)
    })
}

#[cfg(test)]
#[path = "patch_fields_tests.rs"]
mod patch_fields_tests;
