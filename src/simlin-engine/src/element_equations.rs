// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! One equation where an MDL file's copies of it say one thing.
//!
//! The MDL reader writes a variable out once per element, each element's
//! names substituted into its copy, when more than one equation defines it
//! (`POPULATION[INFANT]` and `POPULATION[ALL BUT YOUNGEST]`) or an `:EXCEPT:`
//! equation does. A host then shows a row of near-identical element
//! equations where the file wrote one. [`compact_element_equations`] takes
//! the copies back in the two cases where they say nothing the one equation
//! does not:
//!
//! - every element has a copy and the copies all read the same, so the
//!   variable is that one equation over its dimensions;
//! - the file wrote an `:EXCEPT:` equation, so an element whose copy reads
//!   as that equation follows it, and only the others keep one of their own.
//!
//! A copy is read with its own element as "this element": a subscript naming
//! the element, on a dimension the variable is over, reads that dimension's
//! name (`x[CHILD]` at `CHILD` is `x[AGE]`). That is what the element read
//! before, because a dimension's name in brackets, on an axis that declares
//! every element of the dimension, reads the active element by name
//! (`DimensionsContext::resolve_mapped_read`). Anything else in a subscript
//! -- another element, a range, a name of no element -- is left as written.
//!
//! Nothing else is read into the copies. Elements most of which agree, or
//! that each read the element before their own, stay as the file wrote them:
//! an equation for every element also defines the elements the dimension
//! gains later, and only the file can say that of a variable.

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
fn read_at(text: &str, model: &Model, axes: &[Axis], at: &[usize]) -> Option<(String, String)> {
    let expr = Expr0::new(text, LexerType::Equation).ok()??;
    let mut reading = Reading {
        model,
        axes,
        at,
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
    /// element: the dimension's name where it names the element itself on an
    /// axis that declares every element of the dimension, and `None` -- left
    /// as written -- otherwise, or where it names an element of more than one
    /// of the variable's dimensions.
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
        (place == self.at[k] && self.model.declares_every_element_of(source_dim, axis))
            .then(|| spelled(&axis.written))
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
        let (rewritten, _) = read_at(text, &self.model, &axes, &at)?;
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
/// thing as that one equation, and one whose elements follow the `:EXCEPT:`
/// default the file wrote as that default and its exceptions (see the module
/// docs).
///
/// A variable is left as it is when an element has an initial or a lookup
/// of its own, when an element's equation does not parse, or when an element
/// it lists is none of its dimensions'. A list of numbers is its own idiom
/// and stays one, unless a default applies that some of them differ from:
/// `d[Age] :EXCEPT: [old] = 0.01` is one number for every age but the old.
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
    {
        return None;
    }
    let numbers = arms.iter().all(|(_, text, ..)| is_number(text));
    let axes = axes_of(model, dims)?;

    // Each element's place, and its equation read at the element: the text
    // read so, and the form two readings compare by.
    let mut read: Vec<(Vec<usize>, String, String)> = Vec::with_capacity(arms.len());
    for (key, text, ..) in arms {
        let at = element_at(&axes, key)?;
        let (rewritten, form) = read_at(text, model, &axes, &at)?;
        read.push((at, rewritten, form));
    }

    // The file wrote a default: the elements that read as it follow it.
    if let Some(default_text) = default.as_deref().filter(|_| *applies) {
        let default_form = canonical_form(default_text)?;
        let kept: Vec<_> = arms
            .iter()
            .zip(&read)
            .filter(|(_, (_, _, form))| *form != default_form)
            .map(|(arm, _)| arm.clone())
            .collect();
        // Numbers that all say the default are a list, as written.
        if kept.len() == arms.len() || (numbers && kept.is_empty()) {
            return None;
        }
        let compact = crate::patch_fields::arrayed(dims.clone(), kept, default.clone(), true);
        return with_equation(var, compact);
    }
    if numbers {
        return None;
    }
    // Otherwise the variable is one equation only where every element has a
    // copy -- an element left out has no equation, and one equation over the
    // dimensions would give it one -- and the copies all read the same. The
    // text kept is the first element's, in the dimensions' order.
    let total: usize = axes.iter().map(|a| a.elements.len()).product();
    let mut places: Vec<&Vec<usize>> = read.iter().map(|(at, ..)| at).collect();
    places.sort_unstable();
    places.dedup();
    if places.len() != total || read.len() != total {
        return None;
    }
    let (_, text, form) = read.iter().min_by_key(|(at, ..)| at)?;
    if read.iter().any(|(_, _, other)| other != form) {
        return None;
    }
    with_equation(var, Equation::ApplyToAll(dims.clone(), text.clone()))
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
