// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Captures: the `PREVIOUS`/`INIT` arguments the parse hoists out of their
//! call, and the identity every synthesized helper is filed under.
//!
//! `PREVIOUS`/`INIT` compile to opcodes that read a fixed slot or a static
//! view of the snapshot regions. An argument that addresses neither
//! ([`crate::snapshot_arg::SnapshotAccess::Capture`]) has to be evaluated into
//! storage of its own first, and that storage is a *capture*: a hidden
//! per-callsite evaluation unit whose body is the argument.
//!
//! A capture carries the argument as an AST subtree. Its identity is
//! positional -- the parent variable it was hoisted out of, plus the walk
//! counter `id` the visitor was at -- and every stage downstream of the parse
//! consumes the subtree directly.
//!
//! **Never route a capture's body through the printer and the lexer.** Whenever
//! those two disagree about one spelling, a legal model stops compiling, and the
//! quiet half of that class is worse than the loud half: an `If` printed bare
//! under an operator re-parses with the operator moved inside its else branch,
//! which is a wrong number rather than a refusal. GH #913 is where the class was
//! found.
//!
//! # Why a name still exists
//!
//! [`Capture::ident`] is an external key, not the identity. Every stage that
//! files a unit of evaluation under a *name* -- the runlist (a lexicographic
//! sort), the layout's implicit section (name-sorted), the results offset map,
//! and symbolic bytecode's `VarRef` -- needs one for a capture too, and it must
//! sort exactly where it sorts today or the compiled artifact moves. So the
//! name is derived, once, by [`synthetic_ident`], from the positional identity
//! plus the active element. Internal code addresses a capture by `(parent,
//! id)`; only the stages listed above use the name.

use crate::ast::{Ast, Expr0, print_eqn};
use crate::builtins_visitor::{empty_macro_registry, instantiate_implicit_modules};
use crate::common::{Canonical, EquationError, Ident};
use crate::datamodel;
use crate::dimensions::DimensionsContext;
use crate::model::VariableStage0;
use crate::variable::{VarKind, Variable, get_dimensions};

/// The name a synthesized helper of `parent` is filed under.
///
/// The separator is U+205A (TWO DOT PUNCTUATION) and the prefix is `$`; neither
/// is an identifier character, so a helper name can never collide with a
/// user-authored one, and every one of them prints quoted.
///
/// `part` says what the helper holds -- `arg0` for a `PREVIOUS`/`INIT`
/// capture, `arg{i}` for a hoisted module-call argument, the function name for
/// a module instance -- and `n` is the walk counter, shared by a module
/// instance and its argument helpers and incremented once per call.
///
/// `suffix` is the active apply-to-all element, present exactly when the parent
/// is expanded per element and this helper is one element's own. It is what
/// keeps distinct slots of an `Ast::Arrayed` parent from minting one name for
/// different bodies (PR #668).
///
/// **This is the single statement of the spelling.** Every runlist is a
/// lexicographic sort tie-broken by DFS visit order, and the layout's implicit
/// section and the results offset map are both name-sorted, so a helper filed
/// under a different string sorts elsewhere and moves the compiled artifact.
/// One function means the parse, the dependency stage and the layout cannot
/// disagree about where a helper sorts.
pub(crate) fn synthetic_ident(parent: &str, n: usize, part: &str, suffix: Option<&str>) -> String {
    match suffix {
        Some(suffix) if !suffix.is_empty() => format!("$⁚{parent}⁚{n}⁚{part}⁚{suffix}"),
        _ => format!("$⁚{parent}⁚{n}⁚{part}"),
    }
}

/// Which builtin's argument a capture holds.
///
/// The two differ in how the dependency graph schedules them -- a `Previous`
/// capture carries no edge from its parent in either phase, an `Init` capture
/// keeps the parent's initial edge and is seeded into the initials runlist --
/// but that difference is derived from the parent's own classification rather
/// than read from here. This field records what the call was, so a later
/// change that wants to treat the two differently (taking `Init` captures out
/// of the flows runlist, which is a shape change with its own ledger row) has
/// the fact in hand rather than having to re-derive it from the parent's AST.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    Previous,
    Init,
}

impl CaptureKind {
    /// The builtin this capture was hoisted out of, spelled as a model writes
    /// it. No production caller: `Debug` is feature-gated crate-wide, so the
    /// tests that pin each capture shape's kind need a printable spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            CaptureKind::Previous => "PREVIOUS",
            CaptureKind::Init => "INIT",
        }
    }
}

/// One `PREVIOUS`/`INIT` argument, hoisted into its own unit of evaluation.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct Capture {
    /// The name every name-keyed stage files this capture under, derived by
    /// [`synthetic_ident`] from the positional identity. Stored rather than
    /// recomputed because the parent's rewritten equation refers to the capture
    /// by it, and a capture outlives the visitor that knew its parent's name.
    ident: String,
    /// The walk counter this capture was minted at. `(parent, id)` is the
    /// identity; the name is derived from it.
    id: usize,
    kind: CaptureKind,
    /// The argument, exactly as the walk left it -- already dimension-
    /// substituted for a scalar capture in an apply-to-all body, and
    /// deliberately NOT substituted for an arrayed one, so a bare arrayed name
    /// inside it keeps its array shape (GH #541).
    arg: Expr0,
    /// The active apply-to-all element, when this capture is one element's own.
    suffix: Option<String>,
    /// The canonical dimension names an arrayed capture applies over; empty for
    /// a scalar one. An arrayed capture occupies one slot per element and is
    /// read back per element by the rewritten call, so every consumer that
    /// lays it out or subscripts it needs its declared shape.
    dims: Vec<String>,
}

impl Capture {
    /// Mint the capture for one `PREVIOUS`/`INIT` call of `parent`.
    ///
    /// `dims` empty makes a scalar capture; non-empty makes the arrayed one
    /// (GH #541), whose body is held unsubstituted over those dimensions.
    pub(crate) fn new(
        parent: &str,
        id: usize,
        kind: CaptureKind,
        arg: Expr0,
        suffix: Option<String>,
        dims: Vec<String>,
    ) -> Self {
        Capture {
            ident: synthetic_ident(parent, id, "arg0", suffix.as_deref()),
            id,
            kind,
            arg,
            suffix,
            dims,
        }
    }

    pub fn ident(&self) -> &str {
        &self.ident
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn kind(&self) -> CaptureKind {
        self.kind
    }

    pub fn arg(&self) -> &Expr0 {
        &self.arg
    }

    pub fn suffix(&self) -> Option<&str> {
        self.suffix.as_deref()
    }

    /// The canonical dimension names this capture applies over; empty when it
    /// is scalar.
    pub fn dims(&self) -> &[String] {
        &self.dims
    }

    /// Do these two captures define the same value?
    ///
    /// Source position is deliberately excluded. A helper's identity has always
    /// been its BODY, not where the body was written: the apply-to-all
    /// expansion walks one cloned equation per element and collapses the
    /// identical results into one helper, and the dt and initial passes walk
    /// one equation twice. Comparing positions would refuse a pair that differs
    /// only in whitespace as two helpers claiming one name -- a compile error
    /// on a model that compiles today. `PartialEq` keeps positions, because
    /// salsa uses it to decide whether a re-parse changed anything and a moved
    /// span does change the diagnostics.
    pub(crate) fn same_definition(&self, other: &Self) -> bool {
        self.ident == other.ident
            && self.kind == other.kind
            && self.suffix == other.suffix
            && self.dims == other.dims
            && self.arg.eq_ignoring_loc(&other.arg)
    }

    /// The `datamodel::Equation` this capture stands for.
    ///
    /// The only field of a parsed variable that is source text rather than an
    /// AST is `Variable::eqn`, and a capture has no source text of its own, so
    /// it prints its subtree here. Two consumers read it, both in LTM's
    /// link-score generator, and both are why the field is filled rather than
    /// left empty:
    ///
    /// * `ltm_augment::target_equation_dims` takes an ARRAYED target's
    ///   datamodel-cased dimension names off it, and a link score whose target
    ///   reports no dimensions is generated scalar;
    /// * `ltm_augment::scalar_or_a2a_target_expr` falls back to
    ///   `scalar_eqn_text_or_zero` and RE-PARSES this text whenever the target
    ///   has no lowered AST. A capture can reach it that way:
    ///   `db::analysis::reconstruct_implicit_variable` lowers every capture
    ///   through `model::lower_variable`, which is total and discards the AST
    ///   on a lowering error.
    ///
    /// So this text is parsed again on one path, and the deleted round trip is
    /// the one on the COMPILE path, not every round trip in the engine. LTM's
    /// ordinary link-score generation prints the target's LOWERED body
    /// (`patch::expr2_to_expr0` + `print_eqn`) and re-parses it at
    /// `db::ltm::equation::LtmArm::new` -- the GH #965 generated-text boundary,
    /// which applies to every variable, captures included, and which this
    /// module does not touch.
    fn datamodel_equation(&self) -> datamodel::Equation {
        let text = print_eqn(&self.arg);
        if self.dims.is_empty() {
            datamodel::Equation::Scalar(text)
        } else {
            datamodel::Equation::ApplyToAll(self.dims.clone(), text)
        }
    }

    /// This capture as the parse-stage variable it stands for.
    ///
    /// Equivalent to what `variable::parse_var` produces for the synthesized
    /// helper aux, minus the lexing and parsing: a plain, non-negative,
    /// non-flow aux with no graphical function, no initial-phase equation of
    /// its own, and no units. `db::capture_tests` pins the equivalence against
    /// `parse_var` over the printed equation for every capture shape, so the
    /// two cannot drift.
    ///
    /// A dimension name this capture cannot resolve is recorded as an equation
    /// error and discards the AST, exactly as the parse does: the caller's
    /// loud-safe `None` then keeps the helper out of the compile rather than
    /// laying it out at the wrong size.
    ///
    /// Any span such an error carries indexes the PARENT's equation text, since
    /// that is where the subtree was written -- where a re-parse of the printed
    /// helper indexed the printed text. Nothing observes the difference today:
    /// `db::fragment_compile::lower_implicit_var` returns `None` on any error
    /// here, and the helper surfaces through `assemble_module`'s batch
    /// "failed to compile fragments" message, which names the helper rather
    /// than rendering a snippet.
    ///
    /// The body goes through `instantiate_implicit_modules` because a parse of
    /// it does, and the visitor is not a no-op on it: its
    /// per-element gate fires on a bare `PREVIOUS`/`INIT` as well as on a
    /// module call, so an ARRAYED capture whose body holds one becomes an
    /// `Ast::Arrayed` of identical elements rather than staying an
    /// `Ast::ApplyToAll`. That expansion decides the fragment's shape, so
    /// skipping it here would change the compiled artifact. (Keeping the
    /// `ApplyToAll` is a deliberate shape change with its own ledger row, not
    /// a side effect of moving the body off text.) A second generation of
    /// helpers is impossible -- a capture body is already walked -- and is
    /// asserted rather than assumed.
    pub(crate) fn variable_stage0(&self, dimensions: &DimensionsContext) -> VariableStage0 {
        let ident = Ident::<Canonical>::new(&self.ident);
        let mut errors: Vec<EquationError> = Vec::new();
        let ast = if self.dims.is_empty() {
            Some(Ast::Scalar(self.arg.clone()))
        } else {
            match get_dimensions(dimensions, &self.dims) {
                Ok(dims) => Some(Ast::ApplyToAll(dims, self.arg.clone())),
                Err(err) => {
                    errors.push(err);
                    None
                }
            }
        };
        let ast = ast.and_then(|ast| {
            match instantiate_implicit_modules(
                ident.as_str(),
                ast,
                Some(dimensions),
                // The same four model-level facts a parse of a synthesized
                // helper has always been given: none of them. A capture body
                // names no module the parent's walk did not already resolve,
                // and it is not a macro body.
                None,
                None,
                empty_macro_registry(),
                None,
            ) {
                Ok((ast, nested)) => {
                    debug_assert!(
                        nested.is_empty(),
                        "a capture body must synthesize no further helpers"
                    );
                    Some(ast)
                }
                Err(err) => {
                    errors.push(err);
                    None
                }
            }
        });

        Variable {
            ident,
            units: None,
            eqn: Some(self.datamodel_equation()),
            errors,
            unit_errors: vec![],
            kind: VarKind::Aux {
                ast,
                // A capture's body is one expression: the argument. It has no
                // separate initial-phase equation, so both phases run it.
                init_ast: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
            },
        }
    }
}

/// One helper the parse synthesized while walking a variable's equation.
///
/// Two things ride this list, and they are at different stages of Phase 7:
/// a [`Capture`] is an AST subtree with positional identity, while a stdlib or
/// macro module instance and its hoisted call arguments are still
/// `datamodel::Variable`s carrying printed equation text.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub enum ImplicitVar {
    /// A `PREVIOUS`/`INIT` argument hoisted out of its call.
    Capture(Capture),
    /// A stdlib/macro module instance, or an argument aux hoisted out of such
    /// a call.
    ///
    /// Boxed because a `datamodel::Variable` is an order of magnitude larger
    /// than a capture, and this list is retained by salsa for every variable of
    /// every model -- and, under LTM, for every synthetic variable too, where
    /// the retention was measured at +82 MiB on C-LEARN
    /// (`db::ltm::model_ltm_implicit_var_info`).
    Synthesized(Box<datamodel::Variable>),
}

impl ImplicitVar {
    pub fn ident(&self) -> &str {
        match self {
            ImplicitVar::Capture(c) => c.ident(),
            ImplicitVar::Synthesized(v) => v.get_ident(),
        }
    }

    pub fn capture(&self) -> Option<&Capture> {
        match self {
            ImplicitVar::Capture(c) => Some(c),
            ImplicitVar::Synthesized(_) => None,
        }
    }

    /// The datamodel variable this helper is, when it is not a capture.
    pub fn synthesized(&self) -> Option<&datamodel::Variable> {
        match self {
            ImplicitVar::Capture(_) => None,
            ImplicitVar::Synthesized(v) => Some(v),
        }
    }

    /// The module instance this helper is, if it is one.
    pub fn module(&self) -> Option<&datamodel::Module> {
        match self.synthesized() {
            Some(datamodel::Variable::Module(m)) => Some(m),
            _ => None,
        }
    }

    pub fn is_module(&self) -> bool {
        self.module().is_some()
    }

    pub fn is_stock(&self) -> bool {
        matches!(self.synthesized(), Some(datamodel::Variable::Stock(_)))
    }

    /// The dimension names this helper applies over, or `&[]` when it is
    /// scalar. An arrayed helper occupies one slot per element, so a consumer
    /// that lays it out or subscripts it reads this rather than assuming 1.
    pub fn equation_dims(&self) -> &[String] {
        match self {
            ImplicitVar::Capture(c) => c.dims(),
            ImplicitVar::Synthesized(v) => match v.get_equation() {
                Some(
                    datamodel::Equation::ApplyToAll(dims, _)
                    | datamodel::Equation::Arrayed(dims, _, _, _),
                ) => dims,
                _ => &[],
            },
        }
    }

    /// Do these two helpers define the same thing? The question the
    /// synthesized-helper dedup asks when two of them claim one name; see
    /// [`Capture::same_definition`] for why a capture answers it without
    /// consulting source positions.
    pub(crate) fn same_definition(&self, other: &Self) -> bool {
        match (self, other) {
            (ImplicitVar::Capture(a), ImplicitVar::Capture(b)) => a.same_definition(b),
            (ImplicitVar::Synthesized(a), ImplicitVar::Synthesized(b)) => a == b,
            _ => false,
        }
    }
}
