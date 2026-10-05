// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! One equation where an arrayed variable's elements say one thing.
//!
//! An importer can write a variable out once per element, each element's
//! names substituted into its copy: Vensim's reader does this for a variable
//! defined by more than one equation (`POPULATION[INFANT]` and
//! `POPULATION[ALL BUT YOUNGEST]`) and for an `:EXCEPT:` equation. A host
//! then shows a row of near-identical element equations where the file
//! wrote one. [`compact_element_equations`] stores such a variable as the
//! equation every element follows, plus the elements whose equations say
//! something else, the form an edit by field stores (`patch_fields`).
//!
//! Each element's equation is read with its own element as "this element":
//! a subscript naming the element on a dimension the variable is over reads
//! that dimension's name (`x[CHILD]` at `CHILD` is `x[AGE]`), and one naming
//! the element a fixed number of places along the same dimension, on a
//! variable over that very dimension, reads an offset (`aging[INFANT]` at
//! `CHILD` is `aging[AGE - 1]`). Each rewrite reads at that element exactly
//! what the element read before:
//!
//! - a dimension's name in brackets, on an axis that declares every element
//!   of the dimension, reads the active element by name
//!   (`DimensionsContext::resolve_mapped_read`);
//! - an offset is a position, `AGE - 1` the position before the element's
//!   own, and it is read only where the axis it indexes is the dimension it
//!   counts along.
//!
//! So where every element's equation reads the same once rewritten, that
//! one equation computes every element as its own did, and the variable
//! simulates as before. Anything else in a subscript -- another element, a
//! range, a name of no element -- is left as written, and an element whose
//! equation then reads differently keeps it as its own.
//!
//! The compaction stores a dimension's name and not an offset: loop
//! analysis scores an element's link through its own equation, and its
//! per-element link scores cannot yet read a dimension inside a computed
//! index, so a chain stored as `aging[AGE - 1]` would go unscored where its
//! elements' equations are scored. An offset is read for comparing two
//! element equations ([`ElementReader`], which the save check asks), where
//! nothing is stored.

use std::collections::HashMap;

use crate::ast::{Expr0, IndexExpr0, print_eqn};
use crate::builtins::UntypedBuiltinFn;
use crate::common::{CanonicalElementName, RawIdent, canonicalize};
use crate::datamodel::{self, DimensionElements, Equation, Project, Variable};
use crate::lexer::LexerType;

fn canonical(name: &str) -> String {
    canonicalize(name).into_owned()
}

/// A dimension as an element's equation is read against it.
struct Axis {
    /// The name as the variable's equation lists it.
    written: String,
    canonical: String,
    /// Its elements, canonical, in declared order.
    elements: Vec<String>,
}

/// What the rewrite reads of the model: each dimension's elements, and each
/// variable's dimensions, all canonical.
struct Model {
    dimensions: HashMap<String, Vec<String>>,
    variables: HashMap<String, Vec<String>>,
}

impl Model {
    fn of(project: &Project, model: &datamodel::Model) -> Model {
        let dimensions = project
            .dimensions
            .iter()
            .map(|dim| {
                let elements = match &dim.elements {
                    DimensionElements::Named(names) => names.iter().map(|e| canonical(e)).collect(),
                    DimensionElements::Indexed(size) => {
                        (1..=*size).map(|n| n.to_string()).collect()
                    }
                };
                (canonical(dim.name()), elements)
            })
            .collect();
        let variables = model
            .variables
            .iter()
            .filter_map(|var| {
                let dims = match var.get_equation()? {
                    Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, ..) => dims,
                    Equation::Scalar(_) => return None,
                };
                Some((
                    canonical(var.get_ident()),
                    dims.iter().map(|d| canonical(d)).collect(),
                ))
            })
            .collect();
        Model {
            dimensions,
            variables,
        }
    }

    /// Whether every element of `axis` is an element of the dimension
    /// `source` (canonical).
    fn declares_every_element_of(&self, source: &str, axis: &Axis) -> bool {
        source == axis.canonical
            || self
                .dimensions
                .get(source)
                .is_some_and(|elements| axis.elements.iter().all(|e| elements.contains(e)))
    }
}

/// A name as an equation writes it: as the model spells it where that reads
/// back bare, and otherwise as the printer spells it.
fn spelled(name: &str) -> String {
    let bare = name.split_whitespace().collect::<Vec<_>>().join("_");
    if !bare.is_empty() && !bare.contains('.') && crate::ast::print_ident(&bare) == canonical(&bare)
    {
        bare
    } else {
        crate::ast::print_ident(name)
    }
}

/// One element's equation, read with the element as "this element": the
/// text with each rewritten subscript spliced in, and the canonical form two
/// such texts compare by. `None` for a text that does not parse.
fn read_at(
    text: &str,
    model: &Model,
    axes: &[Axis],
    at: &[usize],
    offsets: bool,
) -> Option<(String, String)> {
    let expr = Expr0::new(text, LexerType::Equation).ok()??;
    let mut reading = Reading {
        model,
        axes,
        at,
        offsets,
        splices: Vec::new(),
    };
    reading.expr(&expr);
    let mut splices = reading.splices;
    if splices.is_empty() {
        return Some((text.to_string(), print_eqn(&expr)));
    }
    // A span is a pair of `u16`s; past its reach a splice would land
    // elsewhere in the text.
    if text.len() > u16::MAX as usize {
        return None;
    }
    splices.sort_by_key(|(start, ..)| std::cmp::Reverse(*start));
    let mut rewritten = text.to_string();
    for (start, len, new) in splices {
        rewritten.replace_range(start..start + len, &new);
    }
    let parsed = Expr0::new(&rewritten, LexerType::Equation).ok()??;
    Some((rewritten, print_eqn(&parsed)))
}

/// The subscripts of one element's equation that read the element, `at` (a
/// position on each of `axes`), as "this element", each recorded in
/// `splices` as `(start, length, new text)`.
struct Reading<'a> {
    model: &'a Model,
    axes: &'a [Axis],
    at: &'a [usize],
    /// Whether an element a fixed number of places along reads as an offset.
    offsets: bool,
    splices: Vec<(usize, usize, String)>,
}

impl Reading<'_> {
    fn expr(&mut self, expr: &Expr0) {
        match expr {
            Expr0::Const(..) | Expr0::Var(..) => {}
            Expr0::App(UntypedBuiltinFn(_, args), _) => {
                for arg in args {
                    self.expr(arg);
                }
            }
            Expr0::Subscript(ident, indexes, _) => {
                let source = self.model.variables.get(&canonical(ident.as_str()));
                for (position, index) in indexes.iter().enumerate() {
                    match index {
                        IndexExpr0::Expr(Expr0::Var(name, loc)) => {
                            let source_dim = source.and_then(|dims| dims.get(position));
                            if let Some(new) = self.this_element(name, source_dim) {
                                self.splices
                                    .push((loc.start as usize, name.as_str().len(), new));
                            }
                        }
                        IndexExpr0::Expr(e) => self.expr(e),
                        IndexExpr0::Range(lo, hi, _) => {
                            self.expr(lo);
                            self.expr(hi);
                        }
                        IndexExpr0::Wildcard(_)
                        | IndexExpr0::StarRange(..)
                        | IndexExpr0::DimPosition(..) => {}
                    }
                }
            }
            Expr0::Op1(_, inner, _) => self.expr(inner),
            Expr0::Op2(_, l, r, _) => {
                self.expr(l);
                self.expr(r);
            }
            Expr0::If(c, t, f, _) => {
                self.expr(c);
                self.expr(t);
                self.expr(f);
            }
        }
    }

    /// What the index `name`, on an axis over `source_dim`, reads as at the
    /// element: the dimension's name where it names the element on an axis
    /// that declares every element of the dimension, an offset where it
    /// names another element of the dimension the axis is over, and `None`
    /// -- left as written -- otherwise, or where it names an element of more
    /// than one of the variable's dimensions.
    fn this_element(&self, name: &RawIdent, source_dim: Option<&String>) -> Option<String> {
        let source_dim = source_dim?;
        let element = canonical(name.as_str());
        let mut found = None;
        for (k, axis) in self.axes.iter().enumerate() {
            let Some(place) = axis.elements.iter().position(|e| *e == element) else {
                continue;
            };
            if found.is_some() {
                return None;
            }
            found = Some((k, place));
        }
        let (k, place) = found?;
        let axis = &self.axes[k];
        let own = self.at[k];
        let dim = spelled(&axis.written);
        if place == own {
            return self
                .model
                .declares_every_element_of(source_dim, axis)
                .then_some(dim);
        }
        if !self.offsets || *source_dim != axis.canonical {
            return None;
        }
        Some(if place > own {
            format!("{dim} + {}", place - own)
        } else {
            format!("{dim} - {}", own - place)
        })
    }
}

/// Reads one element's equation of a variable of one model with the element
/// as "this element" (the module docs): what two element equations are
/// compared by, so `a[A1] * 2` at `A1` and `a[DimA] * 2` are one equation.
pub(crate) struct ElementReader {
    model: Model,
}

impl ElementReader {
    pub(crate) fn new(project: &Project, model: &datamodel::Model) -> ElementReader {
        ElementReader {
            model: Model::of(project, model),
        }
    }

    /// `text`, the equation of the element `key` of a variable over `dims`,
    /// with each subscript that reads the element as "this element"
    /// rewritten; `None` where nothing is, or the text cannot be read so.
    pub(crate) fn read(&self, text: &str, dims: &[String], key: &str) -> Option<String> {
        let axes = axes_of(&self.model, dims)?;
        let at = element_at(&axes, key)?;
        let (rewritten, _) = read_at(text, &self.model, &axes, &at, true)?;
        (rewritten != text).then_some(rewritten)
    }
}

/// The axes of a variable over `dims`: `None` when a dimension is not
/// declared, or one is listed twice, which gives an element's name two
/// places.
fn axes_of(model: &Model, dims: &[String]) -> Option<Vec<Axis>> {
    let axes: Vec<Axis> = dims
        .iter()
        .map(|d| {
            let canonical = canonical(d);
            let elements = model.dimensions.get(&canonical)?.clone();
            Some(Axis {
                written: d.clone(),
                canonical,
                elements,
            })
        })
        .collect::<Option<_>>()?;
    let mut names: Vec<&str> = axes.iter().map(|a| a.canonical.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    (names.len() == axes.len()).then_some(axes)
}

/// The position on each of `axes` of the element `key` names.
fn element_at(axes: &[Axis], key: &str) -> Option<Vec<usize>> {
    let key = CanonicalElementName::from_subscript(key);
    let parts: Vec<&str> = key.parts().collect();
    if parts.len() != axes.len() {
        return None;
    }
    parts
        .iter()
        .zip(axes)
        .map(|(part, axis)| axis.elements.iter().position(|e| e == part))
        .collect()
}

/// Store each arrayed variable of `project`'s models whose elements say one
/// thing as that one equation, and the rest as exceptions to it (see the
/// module docs).
///
/// A variable is left as it is when an element has an initial or a lookup
/// of its own, when every element is a number (a list of numbers is its own
/// idiom), when an element's equation does not parse, or when an element it
/// lists is none of its dimensions'. An applying default keeps the elements
/// that read as it does as followers; otherwise a variable is collapsed only
/// where every element has an equation of its own -- an element left out
/// has no equation, and a default would give it one -- and a default that
/// so fills no element is replaced by the equation most elements read.
pub(crate) fn compact_element_equations(project: &mut Project) {
    for m in 0..project.models.len() {
        let model = Model::of(project, &project.models[m]);
        let mut edits = Vec::new();
        for (v, var) in project.models[m].variables.iter().enumerate() {
            if let Some(compacted) = compacted(var, &model) {
                edits.push((v, compacted));
            }
        }
        for (v, compacted) in edits {
            project.models[m].variables.replace(v, compacted);
        }
    }
}

/// `var` with its element equations compacted, or `None` to leave it.
fn compacted(var: &Variable, model: &Model) -> Option<Variable> {
    let Some(Equation::Arrayed(dims, arms, default, applies)) = var.get_equation() else {
        return None;
    };
    if arms.is_empty()
        || arms
            .iter()
            .any(|(_, _, initial, gf)| initial.is_some() || gf.is_some())
        || arms.iter().all(|(_, text, ..)| is_number(text))
    {
        return None;
    }
    let axes = axes_of(model, dims)?;

    // Each element's equation read at the element, by its key.
    let mut read: Vec<(Vec<usize>, String, String)> = Vec::with_capacity(arms.len());
    for (key, text, ..) in arms {
        let at = element_at(&axes, key)?;
        let (rewritten, form) = read_at(text, model, &axes, &at, false)?;
        read.push((at, rewritten, form));
    }

    let total: usize = axes.iter().map(|a| a.elements.len()).product();
    // The elements that read as an applying default follow it.
    if let Some(default_text) = default.as_deref().filter(|_| *applies) {
        let default_form = canonical_form(default_text)?;
        let kept: Vec<_> = arms
            .iter()
            .zip(&read)
            .filter(|(_, (_, _, form))| *form != default_form)
            .map(|(arm, _)| arm.clone())
            .collect();
        if kept.len() != arms.len() {
            let compact = crate::patch_fields::arrayed(dims.clone(), kept, default.clone(), true);
            return with_equation(var, compact);
        }
    }
    // Otherwise, where every element has an equation of its own (so no
    // default fills one), the equation most elements read, the first in the
    // elements' order of those most read, with the rest as exceptions.
    let mut seen = read.iter().map(|(at, ..)| at).collect::<Vec<_>>();
    seen.sort_unstable();
    seen.dedup();
    if seen.len() != total || read.len() != total {
        return None;
    }
    let mut ordered: Vec<usize> = (0..read.len()).collect();
    ordered.sort_by_key(|&i| read[i].0.clone());
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for (_, _, form) in &read {
        *counts.entry(form.as_str()).or_default() += 1;
    }
    let top = ordered
        .iter()
        .enumerate()
        .max_by_key(|&(place, &i)| (counts[read[i].2.as_str()], std::cmp::Reverse(place)))
        .map(|(_, &i)| i)?;
    let top_form = read[top].2.clone();
    let followed = counts[top_form.as_str()];
    if followed < 2 || followed * 2 <= total {
        return None;
    }
    let kept: Vec<_> = ordered
        .iter()
        .filter(|&&i| read[i].2 != top_form)
        .map(|&i| arms[i].clone())
        .collect();
    let compact = crate::patch_fields::arrayed(dims.clone(), kept, Some(read[top].1.clone()), true);
    with_equation(var, compact)
}

/// `var` with `equation` for its own.
fn with_equation(var: &Variable, equation: Equation) -> Option<Variable> {
    let mut var = var.clone();
    match &mut var {
        Variable::Stock(stock) => stock.equation = equation,
        Variable::Flow(flow) => flow.equation = equation,
        Variable::Aux(aux) => aux.equation = equation,
        Variable::Module(_) => return None,
    }
    Some(var)
}

/// Whether `text` is a number, negative ones included.
fn is_number(text: &str) -> bool {
    fn number(expr: &Expr0) -> bool {
        match expr {
            Expr0::Const(..) => true,
            Expr0::Op1(crate::ast::UnaryOp::Negative | crate::ast::UnaryOp::Positive, inner, _) => {
                number(inner)
            }
            _ => false,
        }
    }
    matches!(Expr0::new(text, LexerType::Equation), Ok(Some(expr)) if number(&expr))
}

fn canonical_form(text: &str) -> Option<String> {
    let expr = Expr0::new(text, LexerType::Equation).ok()??;
    Some(print_eqn(&expr))
}

#[cfg(test)]
#[path = "element_equations_tests.rs"]
mod element_equations_tests;
