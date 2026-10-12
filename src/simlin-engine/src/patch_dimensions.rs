// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Edits of a project's dimensions: the patch operations that add a
//! dimension or set its elements, rename a dimension or one of its elements,
//! and delete a dimension no variable uses. Each carries what the edit means
//! through the variables over the dimension -- their dimension lists, the
//! elements with equations of their own, and the equations that name the
//! dimension or its elements -- so a model reads after it as it read before,
//! under the new names.
//!
//! A name inside a subscript's brackets can be a dimension, an element of any
//! dimension, or a variable, and the compiler has one precedence for it
//! (`dimensions::resolve_axis_index_name`). A rename that would leave a
//! bracketed name meaning something else is refused, the posture a variable
//! rename takes (`patch::Reader`): a rename does not choose among readings.

use std::collections::BTreeSet;

use crate::common::{Canonical, ErrorCode, Ident, Result, canonicalize};
use crate::datamodel::{self, DimensionElements, Equation, Variable};
use crate::patch::{Site, respelled_text};
use crate::patch_fields::{Arm, arrayed, elements_of, in_dimension_order, refuse};

/// What joins a dimension's name to one of its elements in an equation
/// (`Age.young`, canonical `age·young`), the element's position.
const QUALIFIER: char = '\u{00B7}';

fn canonical(name: &str) -> String {
    canonicalize(name).into_owned()
}

/// The position of the dimension `name` in `project.dimensions`.
fn dimension_index(project: &datamodel::Project, name: &str) -> Result<usize> {
    let wanted = canonical(name);
    project
        .dimensions
        .iter()
        .position(|d| canonical(d.name()) == wanted)
        .ok_or_else(|| {
            refuse(
                ErrorCode::BadDimensionName,
                format!("'{name}' is not a declared dimension"),
            )
        })
}

/// A dimension's elements, canonical.
fn canonical_elements(dim: &datamodel::Dimension) -> Vec<String> {
    elements_of(dim).iter().map(|e| canonical(e)).collect()
}

/// The first variable of any model named `name` (canonical), as
/// `(model, variable)` display names.
fn variable_named(project: &datamodel::Project, name: &str) -> Option<String> {
    project.models.iter().find_map(|model| {
        model
            .variables
            .iter()
            .find(|var| canonical(var.get_ident()) == name)
            .map(|var| quoted_in(project, model, var.get_ident()))
    })
}

/// A variable as a refusal names it: quoted, with its model where the
/// project has more than one, since a host shows the refusal to a person
/// who knows the one model as the model.
fn quoted_in(project: &datamodel::Project, model: &datamodel::Model, ident: &str) -> String {
    if project.models.len() > 1 {
        format!("'{ident}' in model '{}'", model.name)
    } else {
        format!("'{ident}'")
    }
}

/// The first dimension declaring an element named `name` (canonical), other
/// than those `skip` says to pass over.
fn dimension_with_element<'p>(
    project: &'p datamodel::Project,
    name: &str,
    skip: impl Fn(&datamodel::Dimension) -> bool,
) -> Option<&'p datamodel::Dimension> {
    project.dimensions.iter().find(|dim| {
        !skip(dim)
            && matches!(&dim.elements, DimensionElements::Named(_))
            && canonical_elements(dim).iter().any(|e| e == name)
    })
}

/// A subscript key's parts as written, split at the commas outside quotes.
fn key_parts(subscript: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in subscript.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                current.push(ch);
            }
            ',' if !in_quotes => parts.push(std::mem::take(&mut current)),
            _ => current.push(ch),
        }
    }
    parts.push(current);
    parts
}

/// Each model's variables, by position, as `edit` makes them; a variable
/// `edit` leaves (`None`) keeps its allocation.
fn edit_variables(
    project: &mut datamodel::Project,
    mut edit: impl FnMut(&Variable) -> Result<Option<Variable>>,
) -> Result<()> {
    // Every edit is made before any is written, so a refusal leaves every
    // model as it was.
    let mut edits: Vec<(usize, usize, Variable)> = Vec::new();
    for (m, model) in project.models.iter().enumerate() {
        for (v, var) in model.variables.iter().enumerate() {
            if let Some(edited) = edit(var)? {
                edits.push((m, v, edited));
            }
        }
    }
    for (m, v, edited) in edits {
        project.models[m].variables.replace(v, edited);
    }
    Ok(())
}

/// A variable's equation, for a variable that has one.
fn equation_of(var: &mut Variable) -> Option<&mut Equation> {
    match var {
        Variable::Stock(stock) => Some(&mut stock.equation),
        Variable::Flow(flow) => Some(&mut flow.equation),
        Variable::Aux(aux) => Some(&mut aux.equation),
        Variable::Module(_) => None,
    }
}

/// The positions of `dimension` (canonical) in an equation's dimensions.
fn positions_of(dims: &[String], dimension: &str) -> Vec<usize> {
    dims.iter()
        .enumerate()
        .filter(|(_, d)| canonical(d) == dimension)
        .map(|(at, _)| at)
        .collect()
}

/// `upsertDimension`: adds `dimension`, or replaces the declared dimension
/// of its name, taking its elements and their order, its mappings and its
/// parent.
///
/// An element the dimension no longer has takes with it the elements of
/// every variable over the dimension that have equations of their own there;
/// such a variable whose last own element goes follows its one equation
/// again. An element added follows the equation every element follows, where
/// the variable has one, and is otherwise an element with no equation, which
/// the model's diagnostics name (`MissingElementEquation`). The order of the
/// elements is the order every variable over the dimension is laid out in,
/// and each variable's own elements are kept in it.
///
/// Refused when the dimension would share its name with a variable (XMILE
/// 1.0 section 3.7.1: dimension names "must be distinct from model variables
/// names") or a new dimension with an element, when an element it adds is
/// named like a dimension, when it names an element twice, or when an element it no longer
/// has is still named by another dimension -- a subrange that declares it
/// and no other dimension does, or an element map -- which the edit does not
/// change. Renaming an element
/// is `renameElement`: replacing a dimension cannot tell a renamed element
/// from one removed and another added.
pub(crate) fn upsert_dimension(
    project: &mut datamodel::Project,
    dimension: datamodel::Dimension,
) -> Result<()> {
    let name = canonical(dimension.name());
    if name.is_empty() {
        return Err(refuse(
            ErrorCode::BadDimensionName,
            "a dimension needs a name".to_string(),
        ));
    }
    if let Some(var) = variable_named(project, &name) {
        return Err(refuse(
            ErrorCode::DuplicateVariable,
            format!(
                "'{}' is the name of the variable {var}; a dimension's name must differ from \
                 every variable's",
                dimension.name()
            ),
        ));
    }
    if dimension_index(project, &name).is_err()
        && let Some(dim) = dimension_with_element(project, &name, |_| false)
    {
        return Err(refuse(
            ErrorCode::DuplicateVariable,
            format!(
                "'{}' is also an element of '{}', so a subscript that writes it may mean the \
                 element or the dimension",
                dimension.name(),
                dim.name()
            ),
        ));
    }
    let new_elements = canonical_elements(&dimension);
    // The elements the dimension declares already, which this edit does not
    // add: a clash one of them has with a variable is the model's as it was.
    let declared: BTreeSet<String> = dimension_index(project, &name)
        .map(|at| {
            canonical_elements(&project.dimensions[at])
                .into_iter()
                .collect()
        })
        .unwrap_or_default();
    let mut seen = BTreeSet::new();
    for (element, written) in new_elements.iter().zip(elements_of(&dimension)) {
        if element.is_empty() {
            return Err(refuse(
                ErrorCode::BadDimensionName,
                format!("'{}' has an element with no name", dimension.name()),
            ));
        }
        if !seen.insert(element.clone()) {
            return Err(refuse(
                ErrorCode::BadDimensionName,
                format!("'{}' names the element '{written}' twice", dimension.name()),
            ));
        }
        if !declared.contains(element)
            && (*element == name
                || project
                    .dimensions
                    .iter()
                    .any(|d| canonical(d.name()) == *element))
        {
            return Err(refuse(
                ErrorCode::DuplicateVariable,
                format!(
                    "cannot add the element '{written}' to '{}': it is a dimension's name, so a \
                     subscript that writes it may mean the dimension or the element",
                    dimension.name()
                ),
            ));
        }
        if !declared.contains(element)
            && let Some(var) = variable_named(project, element)
        {
            return Err(refuse(
                ErrorCode::DuplicateVariable,
                format!(
                    "cannot add the element '{written}' to '{}': {var} is a variable of that \
                     name, so a subscript that writes it may mean the variable or the element",
                    dimension.name()
                ),
            ));
        }
    }

    let Ok(at) = dimension_index(project, &name) else {
        project.dimensions.push(dimension);
        return Ok(());
    };
    let old = &project.dimensions[at];
    let old_elements = canonical_elements(old);
    let removed: BTreeSet<&String> = old_elements
        .iter()
        .filter(|e| !new_elements.contains(e))
        .collect();
    if !removed.is_empty() {
        let old_set: BTreeSet<&String> = old_elements.iter().collect();
        let is_subrange = |other: &datamodel::Dimension| {
            matches!(&other.elements, DimensionElements::Named(_))
                && canonical_elements(other)
                    .iter()
                    .all(|e| old_set.contains(e))
        };
        // An element removed here that a dimension outside this one still
        // declares (the parent of a subrange edited here) still exists, and
        // a subrange declaring it is that dimension's subrange; one that only
        // subranges of this dimension would still declare is gone.
        let declared_elsewhere = |element: &String| {
            project.dimensions.iter().enumerate().any(|(i, dim)| {
                i != at
                    && !is_subrange(dim)
                    && matches!(&dim.elements, DimensionElements::Named(_))
                    && canonical_elements(dim).contains(element)
            })
        };
        for (other_at, other) in project.dimensions.iter().enumerate() {
            if other_at == at {
                continue;
            }
            let other_elements = canonical_elements(other);
            if let Some(element) = other_elements
                .iter()
                .find(|e| is_subrange(other) && removed.contains(e) && !declared_elsewhere(e))
            {
                return Err(refuse(
                    ErrorCode::BadDimensionName,
                    format!(
                        "'{element}' is also an element of '{}', a subrange of '{}'; change \
                         that dimension first",
                        other.name(),
                        old.name()
                    ),
                ));
            }
            let mapped = other.mappings.iter().any(|mapping| {
                canonical(&mapping.target) == name
                    && mapping
                        .element_map
                        .iter()
                        .any(|(_, target)| removed.contains(&canonical(target)))
            });
            let child_past_the_end = other.parent.as_deref().map(canonical).as_deref()
                == Some(name.as_str())
                && other.len() > new_elements.len();
            if mapped || child_past_the_end {
                return Err(refuse(
                    ErrorCode::BadDimensionName,
                    format!(
                        "the dimension '{}' maps onto an element '{}' would no longer have; \
                         change that dimension first",
                        other.name(),
                        old.name()
                    ),
                ));
            }
        }
    }

    let mut dims_after = project.dimensions.clone();
    dims_after[at] = dimension.clone();
    edit_variables(project, |var| {
        let mut edited = var.clone();
        let Some(equation) = equation_of(&mut edited) else {
            return Ok(None);
        };
        let Equation::Arrayed(dims, arms, default, applies) = equation else {
            return Ok(None);
        };
        let positions = positions_of(dims, &name);
        if positions.is_empty() {
            return Ok(None);
        }
        let kept: Vec<Arm> = arms
            .iter()
            .filter(|arm| {
                let parts = key_parts(&arm.0);
                !positions.iter().any(|&p| {
                    parts
                        .get(p)
                        .is_some_and(|part| removed.contains(&canonical(part.trim())))
                })
            })
            .cloned()
            .collect();
        let laid_out: Option<Vec<&datamodel::Dimension>> = dims
            .iter()
            .map(|d| {
                dims_after
                    .iter()
                    .find(|x| canonical(x.name()) == canonical(d))
            })
            .collect();
        let kept = match laid_out {
            Some(laid_out) => in_dimension_order(&laid_out, kept),
            None => kept,
        };
        if kept == *arms {
            return Ok(None);
        }
        *equation = arrayed(dims.clone(), kept, default.clone(), *applies);
        Ok(Some(edited))
    })?;
    project.dimensions[at] = dimension;
    Ok(())
}

/// A name as an equation writes it: as the model spells it, its spaces
/// written `_`, where that reads back bare, and otherwise in quotes.
pub(crate) fn spelled(name: &str) -> String {
    let bare = name.split_whitespace().collect::<Vec<_>>().join("_");
    if !bare.is_empty() && !bare.contains('.') && crate::ast::print_ident(&bare) == canonical(&bare)
    {
        bare
    } else if !name.contains(['"', '\\']) {
        // In quotes as written, which reads back as the name -- its case,
        // spaces and periods included -- where the printer would spell its
        // canonical form.
        format!("\"{}\"", name.trim())
    } else {
        crate::ast::print_ident(name)
    }
}

/// An element qualified by its dimension (`Age.young`), its position.
pub(crate) fn qualified(dimension: &str, element: &str) -> String {
    format!("{}.{}", spelled(dimension), spelled(element))
}

/// A text with each reference `respell` gives a new spelling respelled, or
/// `None` when it gives none. `respell` is asked of each reference's
/// canonical name and returns the text written in its place.
pub(crate) fn respelled(
    text: &str,
    respell: &dyn Fn(Site, &str) -> Option<String>,
) -> Option<String> {
    let respelling = |site: Site, reference: &Ident<Canonical>| {
        Ok(respell(site, reference.as_str()).map(Ident::from_unchecked))
    };
    // `respell` refuses nothing, so the walk finds no ambiguity.
    respelled_text(text, &respelling, &|written| written.as_str().to_string())
        .ok()
        .flatten()
}

/// Every expression text of every variable respelled by `respell`.
///
/// Refused, with `rename` saying what was asked, where a respelled text would
/// not read back: a name that has to be written in quotes cannot qualify an
/// element or a star range (`Age.old`, `Age.*`), and a rename to one leaves
/// such a text an error rather than the reference it was.
fn respell_texts(
    project: &mut datamodel::Project,
    rename: &str,
    respell: &dyn Fn(Site, &str) -> Option<String>,
) -> Result<()> {
    edit_variables(project, |var| {
        let mut unreadable: Option<String> = None;
        let edited = var.map_expression_texts(|_, text| {
            let new = respelled(text, respell)?;
            if unreadable.is_none()
                && !matches!(
                    crate::ast::Expr0::new(&new, crate::lexer::LexerType::Equation),
                    Ok(Some(_))
                )
            {
                unreadable = Some(text.to_string());
            }
            Some(new)
        });
        match unreadable {
            Some(text) => Err(refuse(
                ErrorCode::BadDimensionName,
                format!(
                    "{rename}: the equation of '{}' writes it qualified (`{text}`), and a name \
                     written in quotes cannot qualify an element or a star range. Choose a name \
                     that needs no quotes (letters, digits and underscores, beginning with a \
                     letter), or first write the equation without the qualified name: an element \
                     by its own name in brackets (`c[old]`), a position as a number, a star \
                     range as `*:Dimension`",
                    var.get_ident()
                ),
            )),
            None => Ok(edited),
        }
    })
}

/// `renameDimension`: renames the dimension `from` to `to`, and with it every
/// variable's list of dimensions, every other dimension's mapping onto it or
/// its parent, and every equation that names it: in a subscript's brackets
/// (`x[Age]`), as a star range (`*:Age`), as the position of the element the
/// equation is at (`Age > 1`), and qualifying an element (`Age.young`).
///
/// Refused when `to` is another dimension's name, a variable's, or an
/// element's -- in brackets it would then mean one of two things -- and when
/// `from` is a variable's or an element's name too, so that an equation
/// writing it may mean either.
pub(crate) fn rename_dimension(
    project: &mut datamodel::Project,
    from: &str,
    to: &str,
) -> Result<()> {
    let at = dimension_index(project, from)?;
    let old = canonical(from);
    let new = canonical(to);
    if new.is_empty() || to.contains('"') {
        return Err(refuse(
            ErrorCode::BadDimensionName,
            format!("'{to}' cannot name a dimension"),
        ));
    }
    if old == new {
        project.dimensions[at].name = to.to_string();
        return Ok(());
    }
    if project
        .dimensions
        .iter()
        .any(|d| canonical(d.name()) == new)
    {
        return Err(refuse(
            ErrorCode::DuplicateVariable,
            format!("cannot rename '{from}' to '{to}': a dimension named '{to}' already exists"),
        ));
    }
    for name in [&old, &new] {
        if let Some(var) = variable_named(project, name) {
            return Err(refuse(
                ErrorCode::DuplicateVariable,
                format!(
                    "cannot rename '{from}' to '{to}': {var} is a variable of that name, so an \
                     equation that writes it may mean the variable or the \
                     dimension"
                ),
            ));
        }
        if let Some(dim) = dimension_with_element(project, name, |_| false) {
            return Err(refuse(
                ErrorCode::DuplicateVariable,
                format!(
                    "cannot rename '{from}' to '{to}': '{name}' is also an element of '{}', so a \
                     subscript that writes it may mean the element or the dimension",
                    dim.name()
                ),
            ));
        }
    }

    let qualified_prefix = format!("{old}{QUALIFIER}");
    let elements = elements_of(&project.dimensions[at]);
    let rename = format!("cannot rename '{from}' to '{to}'");
    respell_texts(project, &rename, &|site, reference| match site {
        Site::Index | Site::StarRange | Site::Value if reference == old => Some(spelled(to)),
        Site::Index | Site::Value => {
            let element = reference.strip_prefix(&qualified_prefix)?;
            let written = elements.iter().find(|e| canonical(e) == element)?;
            Some(qualified(to, written))
        }
        Site::Subscripted | Site::StarRange => None,
    })?;
    edit_variables(project, |var| {
        let mut edited = var.clone();
        let Some(equation) = equation_of(&mut edited) else {
            return Ok(None);
        };
        let dims = match equation {
            Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, ..) => dims,
            Equation::Scalar(_) => return Ok(None),
        };
        let positions = positions_of(dims, &old);
        if positions.is_empty() {
            return Ok(None);
        }
        for p in positions {
            dims[p] = to.to_string();
        }
        Ok(Some(edited))
    })?;
    for dim in project.dimensions.iter_mut() {
        for mapping in dim.mappings.iter_mut() {
            if canonical(&mapping.target) == old {
                mapping.target = to.to_string();
            }
        }
        if dim.parent.as_deref().map(canonical).as_deref() == Some(old.as_str()) {
            dim.parent = Some(to.to_string());
        }
    }
    project.dimensions[at].name = to.to_string();
    Ok(())
}

/// The named dimensions that declare the element `element` (canonical) as
/// one element with `dimension`'s: `dimension` itself, its subranges and the
/// dimensions it is a subrange of, and theirs in turn. Two overlapping
/// subranges of one dimension share its element by name, and renaming it
/// renames it in each.
fn family(project: &datamodel::Project, at: usize, element: &str) -> Vec<usize> {
    let sets: Vec<BTreeSet<String>> = project
        .dimensions
        .iter()
        .map(|d| match &d.elements {
            DimensionElements::Named(_) => canonical_elements(d).into_iter().collect(),
            DimensionElements::Indexed(_) => BTreeSet::new(),
        })
        .collect();
    let mut members = vec![at];
    loop {
        let grew: Vec<usize> = (0..sets.len())
            .filter(|i| !members.contains(i) && sets[*i].contains(element))
            .filter(|i| {
                members
                    .iter()
                    .any(|m| sets[*i].is_subset(&sets[*m]) || sets[*m].is_subset(&sets[*i]))
            })
            .collect();
        if grew.is_empty() {
            return members;
        }
        members.extend(grew);
    }
}

/// `renameElement`: renames the element `from` of `dimension` to `to`, in
/// the dimension, in the subranges that share it (and the dimensions it is a
/// subrange of), in every element map that names it, in the keys of every
/// variable's own elements along those dimensions, and in every subscript
/// that reads it (`x[young]`, a range's end, `Age.young`).
///
/// Refused for an indexed dimension, whose elements are positions; when
/// another dimension declares an element of the same name, so that a
/// subscript writing it may mean either; when `from` is also a variable's or
/// a dimension's name, for the same reason; and when `to` is already an
/// element's, a dimension's or a variable's name.
pub(crate) fn rename_element(
    project: &mut datamodel::Project,
    dimension: &str,
    from: &str,
    to: &str,
) -> Result<()> {
    let at = dimension_index(project, dimension)?;
    let dim_name = project.dimensions[at].name().to_string();
    let DimensionElements::Named(elements) = &project.dimensions[at].elements else {
        return Err(refuse(
            ErrorCode::BadDimensionName,
            format!("'{dim_name}' is indexed: its elements are positions, which have no names"),
        ));
    };
    let old = canonical(from);
    let new = canonical(to);
    if !elements.iter().any(|e| canonical(e) == old) {
        return Err(refuse(
            ErrorCode::BadDimensionName,
            format!("'{from}' is not an element of '{dim_name}'"),
        ));
    }
    if new.is_empty() || to.contains('"') || to.contains(',') {
        return Err(refuse(
            ErrorCode::BadDimensionName,
            format!("'{to}' cannot name an element"),
        ));
    }
    let members = family(project, at, &old);
    let respell_list = |project: &mut datamodel::Project| {
        for &m in &members {
            if let DimensionElements::Named(elements) = &mut project.dimensions[m].elements {
                for e in elements.iter_mut() {
                    if canonical(e) == old {
                        *e = to.to_string();
                    }
                }
            }
        }
    };
    if old == new {
        respell_list(project);
        return Ok(());
    }
    if let Some(other) = dimension_with_element(project, &old, |d| {
        members
            .iter()
            .any(|&m| canonical(project.dimensions[m].name()) == canonical(d.name()))
    }) {
        return Err(refuse(
            ErrorCode::DuplicateVariable,
            format!(
                "cannot rename '{from}': '{}' declares an element of that name too, so a \
                 subscript that writes it may mean either",
                other.name()
            ),
        ));
    }
    if let Some(other) = dimension_with_element(project, &new, |_| false) {
        return Err(refuse(
            ErrorCode::DuplicateVariable,
            format!(
                "cannot rename '{from}' to '{to}': '{}' already has an element named '{to}'",
                other.name()
            ),
        ));
    }
    for name in [&old, &new] {
        if let Some(var) = variable_named(project, name) {
            return Err(refuse(
                ErrorCode::DuplicateVariable,
                format!(
                    "cannot rename '{from}' to '{to}': {var} is a variable of that name, so a \
                     subscript that writes it may mean the variable or the \
                     element"
                ),
            ));
        }
        if project
            .dimensions
            .iter()
            .any(|d| canonical(d.name()) == *name)
        {
            return Err(refuse(
                ErrorCode::DuplicateVariable,
                format!(
                    "cannot rename '{from}' to '{to}': '{name}' is a dimension's name, so a \
                     subscript that writes it may mean the dimension or the element"
                ),
            ));
        }
    }

    let member_names: Vec<String> = members
        .iter()
        .map(|&m| canonical(project.dimensions[m].name()))
        .collect();
    let member_display: Vec<String> = members
        .iter()
        .map(|&m| project.dimensions[m].name().to_string())
        .collect();
    let rename = format!("cannot rename '{from}' to '{to}'");
    respell_texts(project, &rename, &|site, reference| match site {
        Site::Index if reference == old => Some(spelled(to)),
        Site::Index | Site::Value => {
            let (qualifier, element) = reference.split_once(QUALIFIER)?;
            if element != old {
                return None;
            }
            let at = member_names.iter().position(|m| m == qualifier)?;
            Some(qualified(&member_display[at], to))
        }
        Site::Subscripted | Site::StarRange => None,
    })?;
    edit_variables(project, |var| {
        let mut edited = var.clone();
        let Some(Equation::Arrayed(dims, arms, ..)) = equation_of(&mut edited) else {
            return Ok(None);
        };
        let positions: Vec<usize> = dims
            .iter()
            .enumerate()
            .filter(|(_, d)| member_names.contains(&canonical(d)))
            .map(|(p, _)| p)
            .collect();
        let mut changed = false;
        for arm in arms.iter_mut() {
            let mut parts = key_parts(&arm.0);
            for &p in &positions {
                if parts
                    .get(p)
                    .is_some_and(|part| canonical(part.trim()) == old)
                {
                    parts[p] = to.to_string();
                    changed = true;
                }
            }
            arm.0 = parts.join(",");
        }
        Ok(changed.then_some(edited))
    })?;
    for dim in project.dimensions.iter_mut() {
        let source_is_member = member_names.contains(&canonical(dim.name()));
        for mapping in dim.mappings.iter_mut() {
            let target_is_member = member_names.contains(&canonical(&mapping.target));
            for (source, target) in mapping.element_map.iter_mut() {
                if source_is_member && canonical(source) == old {
                    *source = to.to_string();
                }
                if target_is_member && canonical(target) == old {
                    *target = to.to_string();
                }
            }
        }
    }
    respell_list(project);
    Ok(())
}

/// `deleteDimension`: removes the dimension `name`. Refused while a variable
/// is arrayed over it, an equation names it or one of its elements by it
/// (`*:Sub`, `Age.young`), or another dimension maps onto it or names it as
/// its parent, naming the first of each.
pub(crate) fn delete_dimension(project: &mut datamodel::Project, name: &str) -> Result<()> {
    let at = dimension_index(project, name)?;
    let dim = canonical(name);
    let qualified_prefix = format!("{dim}{QUALIFIER}");
    let mut users: Vec<String> = Vec::new();
    for model in &project.models {
        for var in model.variables.iter() {
            let over = var.get_equation().is_some_and(|equation| match equation {
                Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, ..) => {
                    !positions_of(dims, &dim).is_empty()
                }
                Equation::Scalar(_) => false,
            });
            let named = var.expression_texts().into_iter().any(|(_, text)| {
                crate::patch::text_references(text).is_some_and(|references| {
                    references.iter().any(|(site, reference)| {
                        *site != Site::Subscripted
                            && (reference.as_str() == dim
                                || reference.as_str().starts_with(&qualified_prefix))
                    })
                })
            });
            if over || named {
                users.push(quoted_in(project, model, var.get_ident()));
            }
        }
    }
    for other in &project.dimensions {
        let maps_onto = other
            .mappings
            .iter()
            .any(|mapping| canonical(&mapping.target) == dim);
        let child = other.parent.as_deref().map(canonical).as_deref() == Some(dim.as_str());
        if maps_onto || child {
            users.push(format!("the dimension '{}'", other.name()));
        }
    }
    if !users.is_empty() {
        let more = users.len().saturating_sub(3);
        let verb = if users.len() == 1 { "uses" } else { "use" };
        let mut named = users.into_iter().take(3).collect::<Vec<_>>().join(", ");
        if more > 0 {
            named.push_str(&format!(" and {more} more"));
        }
        return Err(refuse(
            ErrorCode::BadDimensionName,
            format!("cannot delete '{name}': {named} still {verb} it"),
        ));
    }
    project.dimensions.remove(at);
    Ok(())
}

#[cfg(test)]
#[path = "patch_dimensions_tests.rs"]
mod patch_dimensions_tests;
