// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A variable's equations as a host draws them: each text with its parse
//! tree, and an arrayed variable as the equation every element follows plus
//! one cell per element, in its dimensions' order.
//!
//! One shape serves every variable. A scalar has no dimensions and no cells,
//! and its equation is `every`; an apply-to-all variable has an `every` that
//! every cell follows; an arrayed variable has the cells that have equations
//! of their own, and an `every` when its default applies to the rest. A
//! stock's equation is its initial value.
//!
//! The tree is the parse alone (`Expr0::new`): a builtin is a call as the
//! modeler wrote it (`SMTH1(x, 3)`), never the module instance the compiler
//! makes of it, and nothing is resolved but what a host needs to draw a
//! reference: whether the model has the variable and how the model spells it,
//! and what each index inside brackets names on the axis it indexes (an
//! element first, then a dimension, the compiler's precedence,
//! `dimensions::resolve_axis_index_name`). Where the grouping rule every
//! printer applies (`ast::needs_parens`) puts parentheses, the tree has a
//! `group` node, so a host needs no precedence table of its own.
//!
//! Every span is a pair of UTF-8 byte offsets into the text it belongs to,
//! `[start, end)`; a host that measures text in UTF-16 code units converts.

use serde::Serialize;

use crate::ast::{self, Expr0, IndexExpr0, NodeShape};
use crate::builtins::Loc;
use crate::common::{CanonicalElementName, canonicalize};
use crate::datamodel::{self, Equation};
use crate::lexer::LexerType;

/// One variable's equations.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariableEquation {
    /// `stock`, `flow`, `aux` or `module`.
    pub kind: &'static str,
    /// The variable's dimensions, each with its elements in declared order;
    /// empty for a scalar.
    pub dimensions: Vec<DimensionView>,
    /// The equation for every element, or a scalar's equation. `None` for a
    /// module, and for an arrayed variable whose elements each have their own
    /// equation or none.
    pub every: Option<EquationText>,
    /// One cell per element, row-major over `dimensions`; empty for a scalar.
    pub cells: Vec<Cell>,
    /// Element equations that name no element of the variable's dimensions,
    /// which nothing reads: shown so that nothing the model holds is hidden.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unmatched: Vec<UnmatchedArm>,
    /// The variable's own lookup table, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lookup: Option<crate::json::GraphicalFunction>,
}

/// A dimension as a variable's cells are laid out over it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DimensionView {
    pub name: String,
    /// Named elements as the model spells them; an indexed dimension's are
    /// `1` to its size.
    pub elements: Vec<String>,
}

/// What a cell holds.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CellState {
    /// The element has an equation of its own.
    Own,
    /// The element reads the equation every element follows.
    Follows,
    /// The element has no equation and no equation to follow: it evaluates
    /// to 0, and the model warns of it.
    Missing,
}

/// One element of an arrayed variable.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cell {
    /// The element on each dimension, as the model spells it.
    pub element: Vec<String>,
    /// The element's canonical key (`young`, `nyc,young`): what a diagnostic's
    /// element names, and what an edit of the cell names.
    pub key: String,
    pub state: CellState,
    /// The element's own equation, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub own: Option<EquationText>,
    /// The element's own lookup table, when it has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lookup: Option<crate::json::GraphicalFunction>,
}

/// An element equation whose subscript names no element of the variable.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnmatchedArm {
    pub subscript: String,
    pub equation: EquationText,
}

/// One equation's text and its tree.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EquationText {
    pub text: String,
    /// `None` for an empty text and for one that does not parse.
    pub tree: Option<Node>,
    /// Why the text does not parse, at the first place it fails.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<TextError>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextError {
    pub span: [u16; 2],
    pub reason: String,
}

/// A node of an equation's tree.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Node {
    /// A number, as written.
    Number { text: String, span: [u16; 2] },
    /// A reference to a variable: `ident` is its canonical name, `name` how
    /// the model spells it (as written when the model has no such variable),
    /// `known` whether the name reads anything -- a variable, a module's
    /// output (`m.out`), or an element qualified by its dimension (`Age.old`)
    /// -- and `index` what each subscript names.
    Ref {
        ident: String,
        name: String,
        known: bool,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        index: Vec<Index>,
        span: [u16; 2],
    },
    /// A dimension's name used as a value: the position of the element the
    /// equation is for.
    Dimension { name: String, span: [u16; 2] },
    /// A binary operator: `+ - * / // ^ mod > < >= <= = <> and or`, `//`
    /// being division that gives 0 where the divisor is 0.
    Op {
        op: &'static str,
        left: Box<Node>,
        right: Box<Node>,
        span: [u16; 2],
    },
    /// A unary operator: `-`, `+`, `not`, or `'` (transpose).
    Unary {
        op: &'static str,
        operand: Box<Node>,
        span: [u16; 2],
    },
    If {
        cond: Box<Node>,
        then: Box<Node>,
        #[serde(rename = "else")]
        otherwise: Box<Node>,
        span: [u16; 2],
    },
    /// A builtin or macro call, named as written.
    Call {
        name: String,
        args: Vec<Node>,
        span: [u16; 2],
    },
    /// Parentheses the grouping rule requires around `inner`.
    Group { inner: Box<Node> },
}

/// What one index inside a reference's brackets names.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Index {
    /// One element of the axis, as the model spells it, and the dimension it
    /// is an element of.
    Element {
        name: String,
        dimension: String,
        span: [u16; 2],
    },
    /// A dimension's name: the element the equation is for.
    Dimension { name: String, span: [u16; 2] },
    /// `*`: every element of the axis.
    All { span: [u16; 2] },
    /// `*:Sub`: every element of a subrange.
    Subrange { name: String, span: [u16; 2] },
    /// `a:b`: the elements from one to the other.
    Range {
        from: Node,
        to: Node,
        span: [u16; 2],
    },
    /// `@n`: the element at the n-th dimension of the equation.
    Position { n: u32, span: [u16; 2] },
    /// Anything else: an expression computing the element's position, as
    /// `Age - 1` reads the element before.
    Expr { node: Node },
}

/// The equations of `ident` in `model_name`, or `None` when the project has
/// no such model or the model no such variable.
pub fn variable_equation(
    project: &datamodel::Project,
    model_name: &str,
    ident: &str,
) -> Option<VariableEquation> {
    let model = project.get_model(model_name)?;
    let variable = model.get_variable(&canonicalize(ident))?;
    let scope = Scope { project, model };
    let (kind, gf) = match variable {
        datamodel::Variable::Stock(_) => ("stock", None),
        datamodel::Variable::Flow(flow) => ("flow", flow.gf.as_ref()),
        datamodel::Variable::Aux(aux) => ("aux", aux.gf.as_ref()),
        datamodel::Variable::Module(_) => ("module", None),
    };
    let lookup = gf.cloned().map(crate::json::GraphicalFunction::from);
    let empty = || VariableEquation {
        kind,
        dimensions: Vec::new(),
        every: None,
        cells: Vec::new(),
        unmatched: Vec::new(),
        lookup: lookup.clone(),
    };
    let Some(equation) = variable.get_equation() else {
        return Some(empty());
    };
    let view = match equation {
        Equation::Scalar(text) => VariableEquation {
            every: Some(scope.text(text)),
            ..empty()
        },
        Equation::ApplyToAll(dims, text) => {
            let dimensions = scope.dimensions(dims);
            let cells = elements(&dimensions)
                .map(|(element, key)| Cell {
                    element,
                    key,
                    state: CellState::Follows,
                    own: None,
                    lookup: None,
                })
                .collect();
            VariableEquation {
                dimensions,
                every: Some(scope.text(text)),
                cells,
                ..empty()
            }
        }
        Equation::Arrayed(dims, arms, default, default_applies) => {
            let dimensions = scope.dimensions(dims);
            let every = default
                .as_ref()
                .filter(|_| *default_applies)
                .map(|text| scope.text(text));
            type Arm = (
                String,
                String,
                Option<String>,
                Option<datamodel::GraphicalFunction>,
            );
            let keyed: Vec<(CanonicalElementName, &Arm)> = arms
                .iter()
                .map(|arm| (CanonicalElementName::from_subscript(&arm.0), arm))
                .collect();
            let mut matched = vec![false; keyed.len()];
            let cells = elements(&dimensions)
                .map(|(element, key)| {
                    let arm = keyed.iter().position(|(k, _)| k.as_str() == key);
                    if let Some(at) = arm {
                        matched[at] = true;
                    }
                    let arm = arm.map(|at| keyed[at].1);
                    let own = arm
                        .filter(|(_, text, ..)| !text.trim().is_empty())
                        .map(|(_, text, ..)| scope.text(text));
                    let lookup = arm
                        .and_then(|(.., gf)| gf.clone())
                        .map(crate::json::GraphicalFunction::from);
                    let state = if own.is_some() || lookup.is_some() {
                        CellState::Own
                    } else if every.is_some() {
                        CellState::Follows
                    } else {
                        CellState::Missing
                    };
                    Cell {
                        element,
                        key,
                        state,
                        own,
                        lookup,
                    }
                })
                .collect();
            let unmatched = keyed
                .iter()
                .zip(&matched)
                .filter(|(_, matched)| !**matched)
                .map(|((_, (subscript, text, ..)), _)| UnmatchedArm {
                    subscript: subscript.clone(),
                    equation: scope.text(text),
                })
                .collect();
            VariableEquation {
                dimensions,
                every,
                cells,
                unmatched,
                ..empty()
            }
        }
    };
    Some(view)
}

/// Every element of `dimensions`, row-major, as the model spells it and as
/// its canonical key.
fn elements(dimensions: &[DimensionView]) -> impl Iterator<Item = (Vec<String>, String)> + '_ {
    let total: usize = dimensions.iter().map(|d| d.elements.len()).product();
    (0..total).map(move |mut offset| {
        let mut element = vec![String::new(); dimensions.len()];
        for (axis, dim) in dimensions.iter().enumerate().rev() {
            let len = dim.elements.len();
            element[axis] = dim.elements[offset % len].clone();
            offset /= len;
        }
        let key = CanonicalElementName::from_parts(&element)
            .as_str()
            .to_string();
        (element, key)
    })
}

/// What a tree is resolved against: the project's dimensions and the model's
/// variables.
struct Scope<'a> {
    project: &'a datamodel::Project,
    model: &'a datamodel::Model,
}

impl Scope<'_> {
    fn dimension(&self, name: &str) -> Option<&datamodel::Dimension> {
        let canonical = canonicalize(name);
        self.project
            .dimensions
            .iter()
            .find(|d| canonicalize(d.name()) == canonical)
    }

    fn dimension_view(dim: &datamodel::Dimension) -> DimensionView {
        let elements = match &dim.elements {
            datamodel::DimensionElements::Named(names) => names.clone(),
            datamodel::DimensionElements::Indexed(size) => {
                (1..=*size).map(|n| n.to_string()).collect()
            }
        };
        DimensionView {
            name: dim.name().to_string(),
            elements,
        }
    }

    /// The dimensions a variable names, in its order; a name the project
    /// does not declare stands as a dimension with no elements.
    fn dimensions(&self, names: &[String]) -> Vec<DimensionView> {
        names
            .iter()
            .map(|name| match self.dimension(name) {
                Some(dim) => Self::dimension_view(dim),
                None => DimensionView {
                    name: name.clone(),
                    elements: Vec::new(),
                },
            })
            .collect()
    }

    fn text(&self, text: &str) -> EquationText {
        match Expr0::new(text, LexerType::Equation) {
            Ok(tree) => EquationText {
                text: text.to_string(),
                tree: tree.map(|expr| self.node(&expr)),
                error: None,
            },
            Err(errors) => EquationText {
                text: text.to_string(),
                tree: None,
                error: errors.first().map(|e| TextError {
                    span: [e.start, e.end],
                    reason: e
                        .details
                        .clone()
                        .unwrap_or_else(|| e.code.description().to_string()),
                }),
            },
        }
    }

    /// `child` as written under `parent`, grouped where the rule needs it.
    fn child(&self, parent: NodeShape, child: &Expr0, is_right: bool) -> Box<Node> {
        let node = self.node(child);
        if ast::needs_parens(parent, shape(child), is_right) {
            Box::new(Node::Group {
                inner: Box::new(node),
            })
        } else {
            Box::new(node)
        }
    }

    fn node(&self, expr: &Expr0) -> Node {
        match expr {
            Expr0::Const(text, _, loc) => Node::Number {
                text: text.clone(),
                span: span(loc),
            },
            Expr0::Var(ident, loc) => {
                let written = ident.as_str();
                if self.variable(written).is_none() && self.dimension(written).is_some() {
                    return Node::Dimension {
                        name: self.dimension(written).unwrap().name().to_string(),
                        span: span(loc),
                    };
                }
                self.reference(written, Vec::new(), loc)
            }
            Expr0::Subscript(ident, indices, loc) => {
                let axes = self.axes(ident.as_str());
                let index = indices
                    .iter()
                    .enumerate()
                    .map(|(at, index)| self.index(index, axes.get(at).copied().flatten()))
                    .collect();
                self.reference(ident.as_str(), index, loc)
            }
            Expr0::App(call, loc) if is_safe_division(expr) => Node::Op {
                op: "//",
                left: self.child(shape(expr), &call.1[0], false),
                right: self.child(shape(expr), &call.1[1], true),
                span: span(loc),
            },
            Expr0::App(call, loc) => Node::Call {
                name: call.0.clone(),
                args: call.1.iter().map(|arg| self.node(arg)).collect(),
                span: span(loc),
            },
            Expr0::Op1(op, operand, loc) => Node::Unary {
                op: unary_op(op),
                operand: self.child(expr.shape(), operand, false),
                span: span(loc),
            },
            Expr0::Op2(op, left, right, loc) => Node::Op {
                op: binary_op(op),
                left: self.child(expr.shape(), left, false),
                right: self.child(expr.shape(), right, true),
                span: span(loc),
            },
            Expr0::If(cond, then, otherwise, loc) => Node::If {
                cond: Box::new(self.node(cond)),
                then: Box::new(self.node(then)),
                otherwise: Box::new(self.node(otherwise)),
                span: span(loc),
            },
        }
    }

    fn variable(&self, written: &str) -> Option<&datamodel::Variable> {
        self.model.get_variable(&canonicalize(written))
    }

    fn reference(&self, written: &str, index: Vec<Index>, loc: &Loc) -> Node {
        let variable = self.variable(written);
        Node::Ref {
            ident: canonicalize(written).into_owned(),
            name: variable
                .map(|v| v.get_ident().to_string())
                .unwrap_or_else(|| written.to_string()),
            known: variable.is_some() || self.reads_through_a_qualifier(written),
            index,
            span: span(loc),
        }
    }

    /// Whether `written` qualifies a name that reads something: an element of
    /// a dimension (`Age.old`), or a variable of the model a module instance
    /// of this model runs (`m.out`, and on through nested instances).
    fn reads_through_a_qualifier(&self, written: &str) -> bool {
        let canonical = canonicalize(written);
        let Some((head, rest)) = canonical.split_once('\u{00B7}') else {
            return false;
        };
        if let Some(dim) = self.dimension(head)
            && element_of(dim, rest).is_some()
        {
            return true;
        }
        let mut model = self.model;
        let mut path = canonical.as_ref();
        while let Some((instance, rest)) = path.split_once('\u{00B7}') {
            let Some(datamodel::Variable::Module(module)) = model.get_variable(instance) else {
                return false;
            };
            let Some(next) = self.project.get_model(&module.model_name) else {
                return false;
            };
            model = next;
            path = rest;
        }
        model.get_variable(path).is_some()
    }

    /// The declared dimension of each axis of the variable `written`, as far
    /// as the model declares it.
    fn axes(&self, written: &str) -> Vec<Option<&datamodel::Dimension>> {
        let names: &[String] = match self.variable(written).and_then(|v| v.get_equation()) {
            Some(Equation::ApplyToAll(dims, _)) | Some(Equation::Arrayed(dims, ..)) => dims,
            _ => &[],
        };
        names.iter().map(|name| self.dimension(name)).collect()
    }

    fn index(&self, index: &IndexExpr0, axis: Option<&datamodel::Dimension>) -> Index {
        match index {
            IndexExpr0::Wildcard(loc) => Index::All { span: span(loc) },
            IndexExpr0::StarRange(name, loc) => Index::Subrange {
                name: self
                    .dimension(name.as_str())
                    .map(|d| d.name().to_string())
                    .unwrap_or_else(|| name.as_str().to_string()),
                span: span(loc),
            },
            IndexExpr0::Range(from, to, loc) => Index::Range {
                from: self.node(from),
                to: self.node(to),
                span: span(loc),
            },
            IndexExpr0::DimPosition(n, loc) => Index::Position {
                n: *n,
                span: span(loc),
            },
            IndexExpr0::Expr(Expr0::Var(name, loc)) => {
                // An element of the axis first, then a dimension: the
                // compiler's precedence (`dimensions::resolve_axis_index_name`).
                if let Some(axis) = axis
                    && let Some(element) = element_of(axis, name.as_str())
                {
                    return Index::Element {
                        name: element,
                        dimension: axis.name().to_string(),
                        span: span(loc),
                    };
                }
                if let Some(dim) = self.dimension(name.as_str()) {
                    return Index::Dimension {
                        name: dim.name().to_string(),
                        span: span(loc),
                    };
                }
                Index::Expr {
                    node: self.node(&Expr0::Var(name.clone(), *loc)),
                }
            }
            IndexExpr0::Expr(expr) => Index::Expr {
                node: self.node(expr),
            },
        }
    }
}

/// Whether `expr` is a division written `a // b`, which the parser reads as a
/// call of `safediv` spanning both operands, where a call written by name
/// starts at its name, before its first argument.
fn is_safe_division(expr: &Expr0) -> bool {
    match expr {
        Expr0::App(call, loc) => {
            call.0.eq_ignore_ascii_case("safediv")
                && call.1.len() == 2
                && call.1[0].get_loc().start == loc.start
        }
        _ => false,
    }
}

/// How `expr` binds as written: `a // b` as the division it is.
fn shape(expr: &Expr0) -> NodeShape {
    if is_safe_division(expr) {
        NodeShape::Op2(ast::BinaryOp::Div)
    } else {
        expr.shape()
    }
}

/// The element of `axis` that `written` names, as the model spells it.
fn element_of(axis: &datamodel::Dimension, written: &str) -> Option<String> {
    match &axis.elements {
        datamodel::DimensionElements::Named(names) => {
            let canonical = canonicalize(written);
            names
                .iter()
                .find(|name| canonicalize(name) == canonical)
                .cloned()
        }
        datamodel::DimensionElements::Indexed(_) => None,
    }
}

fn span(loc: &Loc) -> [u16; 2] {
    [loc.start, loc.end]
}

fn binary_op(op: &ast::BinaryOp) -> &'static str {
    match op {
        ast::BinaryOp::Add => "+",
        ast::BinaryOp::Sub => "-",
        ast::BinaryOp::Exp => "^",
        ast::BinaryOp::Mul => "*",
        ast::BinaryOp::Div => "/",
        ast::BinaryOp::Mod => "mod",
        ast::BinaryOp::Gt => ">",
        ast::BinaryOp::Lt => "<",
        ast::BinaryOp::Gte => ">=",
        ast::BinaryOp::Lte => "<=",
        ast::BinaryOp::Eq => "=",
        ast::BinaryOp::Neq => "<>",
        ast::BinaryOp::And => "and",
        ast::BinaryOp::Or => "or",
    }
}

fn unary_op(op: &ast::UnaryOp) -> &'static str {
    match op {
        ast::UnaryOp::Positive => "+",
        ast::UnaryOp::Negative => "-",
        ast::UnaryOp::Not => "not",
        ast::UnaryOp::Transpose => "'",
    }
}

#[cfg(test)]
#[path = "equation_view_tests.rs"]
mod equation_view_tests;
