// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What one variable's parse synthesizes -- the `PREVIOUS`/`INIT` arguments it
//! hoists out of their call, the module instances a stdlib or macro call
//! expands into and the call arguments hoisted for them -- and the identity
//! every one of them is filed under.
//!
//! Every helper is parsed DATA. A [`Capture`] and a [`HoistedArg`] carry an
//! `Expr0` subtree, an [`ImplicitModule`] carries its target model and its
//! input wiring, and [`ImplicitVar::parsed_variable`] is the one way a
//! consumer turns any of them back into the parse-stage variable it stands
//! for. Nothing prints a helper to equation text and parses it back, and
//! nothing rewrites a helper's body: a body written inside an apply-to-all
//! keeps the axes it was written for ([`CaptureShape::ApplyToAll`]) or the
//! element it was expanded for ([`crate::variable::ElementScope`]), and
//! lowering resolves it there.
//!
//! `PREVIOUS`/`INIT` compile to opcodes that read a fixed slot or a static
//! view of the snapshot regions. An argument that addresses neither
//! ([`crate::snapshot_arg::SnapshotAccess::Capture`]) has to be evaluated into
//! storage of its own first, and that storage is a *capture*: a hidden
//! per-callsite evaluation unit whose body is the argument. A module instance's
//! input ports likewise read variables by NAME, so a call argument that is not
//! already a bare identifier is hoisted into an aux of its own.
//!
//! A capture carries the argument as an AST subtree. Its identity is
//! positional -- the parent variable it was hoisted out of, plus the walk
//! counter `id` the visitor was at -- and every stage downstream of the parse
//! consumes the subtree directly.
//!
//! **Never route a helper's body through the printer and the lexer.** Whenever
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

use indexmap::IndexMap;

use crate::ast::{Ast, Expr0};
use crate::common::{Canonical, EquationError, ErrorCode, Ident};
use crate::datamodel;
use crate::dimensions::DimensionsContext;
use crate::model::ParsedVariable;
use crate::variable::{ElementScope, VarKind, Variable, get_dimensions};

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

/// The element suffix of a per-element helper's name: the active element's
/// coordinates joined by `,`, lowercased -- the one spelling of an element
/// tuple the helper names carry.
pub(crate) fn element_suffix(scope: &ElementScope) -> String {
    scope
        .element
        .iter()
        .map(|e| e.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// Which snapshot builtins read a capture's storage, and therefore which
/// phases have to evaluate it: a capture's kind is its phase demand.
///
/// `PREVIOUS` reads the prior step's committed value, so its capture is
/// refreshed in flows and needs no initial evaluation -- every read before the
/// first snapshot exists takes the intrinsic's fallback. `INIT` reads the
/// frozen initial-values snapshot, so its capture is populated once, in
/// initials, and has no flow fragment unless a per-step definition reads its
/// current value (`db::dep_graph::model_dependency_graph` decides both). The
/// dt and active-initial passes of one variable restart the walk counter, so
/// they can mint the same positional storage for different consumers; one
/// capture then serves both and its demand is the union
/// ([`Capture::merge_same_definition`]).
// `Debug` is unconditional: `db::ImplicitVarDeps` derives it in every build
// and carries a `CaptureKind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureKind {
    Previous,
    Init,
    PreviousAndInit,
}

impl CaptureKind {
    /// The consumer, spelled as a model writes it; a shared capture names
    /// both. No production caller: `Debug` is feature-gated crate-wide, so
    /// the tests that pin each capture shape's kind need a printable spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            CaptureKind::Previous => "PREVIOUS",
            CaptureKind::Init => "INIT",
            CaptureKind::PreviousAndInit => "PREVIOUS+INIT",
        }
    }

    /// The storage is refreshed every step, ahead of the snapshot a
    /// `PREVIOUS` reads.
    pub const fn needs_flows(self) -> bool {
        matches!(self, CaptureKind::Previous | CaptureKind::PreviousAndInit)
    }

    /// The storage is populated in initials, ahead of the snapshot an `INIT`
    /// reads.
    pub const fn needs_initials(self) -> bool {
        matches!(self, CaptureKind::Init | CaptureKind::PreviousAndInit)
    }

    const fn union(self, other: Self) -> Self {
        match (self, other) {
            (CaptureKind::Previous, CaptureKind::Previous) => CaptureKind::Previous,
            (CaptureKind::Init, CaptureKind::Init) => CaptureKind::Init,
            _ => CaptureKind::PreviousAndInit,
        }
    }
}

/// The storage a capture occupies and the context its body is resolved in --
/// decided by where the `PREVIOUS`/`INIT` call was written.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub enum CaptureShape {
    /// A scalar equation's argument: one slot, a scalar body.
    Scalar,
    /// An apply-to-all body's argument, kept structural: one slot per element
    /// of the parent's declared axes (canonical dimension names), the body
    /// resolved per element by the same apply-to-all lowering the parent gets,
    /// and read back by the parent as `capture[Dim..]` -- the reference that
    /// leaves each axis standing for lowering to pin to the active element.
    ApplyToAll(Vec<String>),
    /// One element's argument, minted while the parent's body was walked for
    /// that element -- a module-bearing apply-to-all body, an explicit
    /// `Ast::Arrayed` slot, or a module-bearing EXCEPT default materialized
    /// for a slot no explicit element claims: one slot, a scalar body
    /// resolved as that element (`ElementScope`).
    Element(ElementScope),
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
    /// The argument, exactly as the walk left it: a subscript naming an axis
    /// of the parent stays that subscript, and lowering resolves it under the
    /// shape's axes or element.
    arg: Expr0,
    shape: CaptureShape,
}

impl Capture {
    /// Mint the capture for one `PREVIOUS`/`INIT` call of `parent`.
    pub(crate) fn new(
        parent: &str,
        id: usize,
        kind: CaptureKind,
        arg: Expr0,
        shape: CaptureShape,
    ) -> Self {
        let suffix = match &shape {
            CaptureShape::Element(scope) => Some(element_suffix(scope)),
            CaptureShape::Scalar | CaptureShape::ApplyToAll(_) => None,
        };
        Capture {
            ident: synthetic_ident(parent, id, "arg0", suffix.as_deref()),
            id,
            kind,
            arg,
            shape,
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

    pub fn shape(&self) -> &CaptureShape {
        &self.shape
    }

    /// The canonical dimension names this capture's STORAGE applies over;
    /// empty when it is one slot.
    pub fn dims(&self) -> &[String] {
        match &self.shape {
            CaptureShape::ApplyToAll(dims) => dims,
            CaptureShape::Scalar | CaptureShape::Element(_) => &[],
        }
    }

    /// Absorb another capture that defines the same value: `true` when it
    /// does, after which this capture's phase demand covers both consumers.
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
    ///
    /// The kind is excluded too, and unioned instead: the same storage read by
    /// a `PREVIOUS` in the dt equation and by an `INIT` in the active-initial
    /// equation is one value evaluated in both phases, not two helpers
    /// claiming one name.
    pub(crate) fn merge_same_definition(&mut self, other: &Self) -> bool {
        let same = self.ident == other.ident
            && self.shape == other.shape
            && self.arg.eq_ignoring_loc(&other.arg);
        if same {
            self.kind = self.kind.union(other.kind);
        }
        same
    }

    /// This capture as the parse-stage variable it stands for.
    ///
    /// A dimension name this capture cannot resolve is recorded as an equation
    /// error and discards the AST, exactly as the parse does: the caller's
    /// loud-safe `None` then keeps the helper out of the compile rather than
    /// laying it out at the wrong size. See [`subtree_parsed_variable`] for
    /// what the body is.
    fn parsed_variable(&self, dimensions: &DimensionsContext) -> ParsedVariable {
        let (ast, scope, errors) = match &self.shape {
            CaptureShape::Scalar => (Some(Ast::Scalar(self.arg.clone())), None, Vec::new()),
            CaptureShape::Element(scope) => (
                Some(Ast::Scalar(self.arg.clone())),
                Some(scope.clone()),
                Vec::new(),
            ),
            CaptureShape::ApplyToAll(dims) => match get_dimensions(dimensions, dims) {
                Ok(resolved) => (
                    Some(Ast::ApplyToAll(resolved, self.arg.clone())),
                    None,
                    vec![],
                ),
                Err(err) => (None, None, vec![err]),
            },
        };
        subtree_parsed_variable(&self.ident, ast, scope, errors)
    }
}

/// The parse-stage variable of a helper whose body is an `Expr0` subtree -- a
/// [`Capture`] or a [`HoistedArg`].
///
/// Equivalent to what `variable::parse_var` produces for the helper aux, minus
/// the lexing and parsing: a plain, non-negative, non-flow aux with no
/// graphical function, no initial-phase equation of its own (the parse produces
/// none for a `Scalar`/`ApplyToAll` equation without an `ACTIVE INITIAL`), and
/// no units. `db::capture_tests::a_captures_fragment_is_its_argument_compiled`
/// and `db::implicit_module_tests::a_hoisted_arguments_fragment_is_the_argument_compiled`
/// are the measurement: a helper and a sibling aux holding the same expression
/// compile to identical bytecode.
///
/// `Variable::eqn` is the one field of a parsed variable that is source text
/// rather than an AST, and a helper has none: its body is the subtree, and a
/// printed projection of it would be engine-generated text that some reader
/// would eventually parse again (the GH #965 boundary). The LTM generators
/// read a target's dimensions and body off `ast()` and keep an `eqn` arm for
/// SOURCE variables only, whose text is user-authored.
///
/// Any span an error here carries indexes the PARENT's equation text, since
/// that is where the subtree was written, and that is how it is rendered:
/// `db::fragment_compile::compile_implicit_var_fragment` reports the errors
/// against the parent, so the snippet underlines the argument inside the
/// parent's equation.
///
/// The body is not walked again. The parent's walk visited every node of it
/// and every decision that walk makes is final: a call's arguments are walked
/// before the call itself, so each `PREVIOUS`/`INIT` left in the body was
/// routed to a direct read under the parent's snapshot-index facts and its
/// declared axes, and a second walk here -- which has no owning model to ask
/// -- could only re-decide such a read against a different rule and mint a
/// helper the parent never filed.
fn subtree_parsed_variable(
    ident: &str,
    ast: Option<Ast<Expr0>>,
    element_scope: Option<ElementScope>,
    errors: Vec<EquationError>,
) -> ParsedVariable {
    Variable {
        ident: Ident::<Canonical>::new(ident),
        units: None,
        eqn: None,
        diagnostics: errors
            .into_iter()
            .map(crate::diagnostic::DiagnosticError::Equation)
            .collect(),
        kind: VarKind::Aux {
            ast,
            // A helper's body is one expression. It has no separate
            // initial-phase equation, so both phases run it.
            init_ast: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            element_scope,
        },
    }
}

/// One module-call argument that is not a bare identifier, hoisted into an aux
/// the instance's input port reads by name.
///
/// A bare identifier argument produces no `HoistedArg` at all -- it wires
/// straight to its port -- so the `arg{i}` in the name is the argument's
/// position in the CALL, not in any list of hoisted arguments, and
/// [`ImplicitModule::references`] does not correspond one-to-one with the
/// hoisted arguments of its call. The exceptions are the two bare names a
/// scalar port cannot read directly: an arrayed variable and a dimension name,
/// which under a per-element expansion mean one element's value, and are
/// hoisted into a helper scoped to that element so lowering reads it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct HoistedArg {
    /// The name every name-keyed stage files this argument under, derived by
    /// [`synthetic_ident`] from the call's `(parent, id)`, the argument's
    /// position and the active element; see [`Capture::ident`] for why one
    /// exists.
    ident: String,
    /// The argument, exactly as the walk left it.
    arg: Expr0,
    /// The element this argument is one element of, when the parent is
    /// expanded per element; `None` for a scalar parent's argument.
    scope: Option<ElementScope>,
}

impl HoistedArg {
    /// Mint the helper for argument `index` of the call `parent`'s walk is at
    /// counter `id`, for element `scope` when the parent is walked per
    /// element.
    pub(crate) fn new(
        parent: &str,
        id: usize,
        index: usize,
        arg: Expr0,
        scope: Option<ElementScope>,
    ) -> Self {
        let suffix = scope.as_ref().map(element_suffix);
        HoistedArg {
            ident: synthetic_ident(parent, id, &format!("arg{index}"), suffix.as_deref()),
            arg,
            scope,
        }
    }

    pub fn ident(&self) -> &str {
        &self.ident
    }

    pub fn arg(&self) -> &Expr0 {
        &self.arg
    }

    pub fn scope(&self) -> Option<&ElementScope> {
        self.scope.as_ref()
    }

    /// Do these two hoisted arguments define the same value? Source position
    /// is excluded for the reason [`Capture::merge_same_definition`] gives.
    pub(crate) fn same_definition(&self, other: &Self) -> bool {
        self.ident == other.ident
            && self.scope == other.scope
            && self.arg.eq_ignoring_loc(&other.arg)
    }

    fn parsed_variable(&self) -> ParsedVariable {
        subtree_parsed_variable(
            &self.ident,
            Some(Ast::Scalar(self.arg.clone())),
            self.scope.clone(),
            Vec::new(),
        )
    }
}

/// One stdlib or macro module-function call, expanded into a module instance.
///
/// A module instance has no equation: it IS its target model plus the wiring
/// that feeds the model's input ports, and that wiring is the whole of what
/// downstream stages read off it -- the dependency graph takes its `src`s as
/// dependencies, `build_module_inputs` turns the pairs into the instance's
/// inputs, and layout sizes the instance from the target model. The fields
/// keep the names of `datamodel::Module`'s, which is what those readers were
/// written against, so one reader serves an explicit instance and an implicit
/// one alike.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct ImplicitModule {
    /// The name every name-keyed stage files this instance under, derived by
    /// [`synthetic_ident`] from `(parent, id)`, the call name and the active
    /// element. It is also the prefix the instance's output is read through
    /// (`{ident}·{primary_output}`) and the prefix of every `dst` below.
    pub(crate) ident: String,
    /// The model this instance instantiates: `stdlib⁚{func}` for a stdlib
    /// call, the macro's own model name for a macro.
    pub(crate) model_name: String,
    /// The input wiring, in call order: `src` is the variable feeding the port
    /// (a user variable for a bare identifier argument, a [`HoistedArg`]'s
    /// ident otherwise) and `dst` is `{ident}.{port}`.
    ///
    /// One entry per WIRED argument. A stdlib call may pass fewer arguments
    /// than the target model has ports (`SMTH1` without an initial value), and
    /// then only the leading ports are wired.
    pub(crate) references: Vec<datamodel::ModuleReference>,
}

impl ImplicitModule {
    /// Mint the instance for the call of `call_name` that `parent`'s walk is at
    /// counter `id`, wiring each `(src, port)` pair in call order.
    pub(crate) fn new<'p>(
        parent: &str,
        id: usize,
        call_name: &str,
        suffix: Option<&str>,
        model_name: String,
        wiring: impl IntoIterator<Item = (String, &'p str)>,
    ) -> Self {
        let ident = synthetic_ident(parent, id, call_name, suffix);
        let references = wiring
            .into_iter()
            .map(|(src, port)| datamodel::ModuleReference {
                src,
                dst: format!("{ident}.{port}"),
            })
            .collect();
        ImplicitModule {
            ident,
            model_name,
            references,
        }
    }

    /// This instance as the parse-stage variable it stands for: the wiring as
    /// the kind's `inputs`, no equation, and nothing that can fail -- the
    /// sources are names, and whether they resolve is the dependency graph's
    /// question, not the parse's.
    fn parsed_variable(&self) -> ParsedVariable {
        Variable {
            ident: Ident::<Canonical>::new(&self.ident),
            units: None,
            eqn: None,
            diagnostics: vec![],
            kind: VarKind::Module {
                model_name: Ident::new(&self.model_name),
                inputs: self.references.clone(),
            },
        }
    }
}

/// One helper the parse synthesized while walking a variable's equation.
///
/// The list a parse produces is ordered by synthesis, and that order is
/// load-bearing: it rides two salsa-cached values with derived `PartialEq`
/// (`ParsedVariableResult::implicit_vars` and `VariableDeps::implicit_vars`),
/// so an unstable order defeats backdating and makes the compiled artifact
/// irreproducible (GH #1002).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub enum ImplicitVar {
    /// A `PREVIOUS`/`INIT` argument hoisted out of its call.
    Capture(Capture),
    /// A module-call argument that is not a bare identifier, hoisted so a port
    /// has a name to read.
    HoistedArg(HoistedArg),
    /// A stdlib or macro module-function call, expanded into a module instance.
    Module(ImplicitModule),
}

impl ImplicitVar {
    pub fn ident(&self) -> &str {
        match self {
            ImplicitVar::Capture(c) => c.ident(),
            ImplicitVar::HoistedArg(a) => a.ident(),
            ImplicitVar::Module(m) => &m.ident,
        }
    }

    pub fn capture(&self) -> Option<&Capture> {
        match self {
            ImplicitVar::Capture(c) => Some(c),
            ImplicitVar::HoistedArg(_) | ImplicitVar::Module(_) => None,
        }
    }

    /// The module instance this helper is, if it is one.
    pub fn module(&self) -> Option<&ImplicitModule> {
        match self {
            ImplicitVar::Module(m) => Some(m),
            ImplicitVar::Capture(_) | ImplicitVar::HoistedArg(_) => None,
        }
    }

    pub fn is_module(&self) -> bool {
        self.module().is_some()
    }

    /// The argument subtree a subtree-bodied helper holds, whose spans index
    /// the PARENT's equation text. `None` for a module instance: it is its
    /// target model plus its port wiring, with no argument of its own, so a
    /// failure to lower it has no span in the parent's equation to point at.
    pub fn arg(&self) -> Option<&Expr0> {
        match self {
            ImplicitVar::Capture(c) => Some(c.arg()),
            ImplicitVar::HoistedArg(a) => Some(a.arg()),
            ImplicitVar::Module(_) => None,
        }
    }

    /// The dimension names this helper's STORAGE applies over, or `&[]` when
    /// it is one slot. An arrayed helper occupies one slot per element, so a
    /// consumer that lays it out or subscripts it reads this rather than
    /// assuming 1. Only a structural capture is ever arrayed: a per-element
    /// helper is one element's slot, and a module instance is sized by its
    /// target model.
    pub fn equation_dims(&self) -> &[String] {
        match self {
            ImplicitVar::Capture(c) => c.dims(),
            ImplicitVar::HoistedArg(_) | ImplicitVar::Module(_) => &[],
        }
    }

    /// The element this helper's scalar body is one element of, when it was
    /// minted for one element of its parent's apply-to-all body.
    pub fn element_scope(&self) -> Option<&ElementScope> {
        match self {
            ImplicitVar::Capture(c) => match c.shape() {
                CaptureShape::Element(scope) => Some(scope),
                CaptureShape::Scalar | CaptureShape::ApplyToAll(_) => None,
            },
            ImplicitVar::HoistedArg(a) => a.scope(),
            ImplicitVar::Module(_) => None,
        }
    }

    /// Absorb another helper that defines the same thing: the question
    /// [`insert_implicit_var`] asks when two of them claim one name. See
    /// [`Capture::merge_same_definition`] for why the subtree-bodied arms
    /// answer it without consulting source positions, and why a capture also
    /// unions the other's phase demand; a module instance has neither
    /// positions to ignore nor a demand to merge.
    pub(crate) fn merge_same_definition(&mut self, other: &Self) -> bool {
        match (self, other) {
            (ImplicitVar::Capture(a), ImplicitVar::Capture(b)) => a.merge_same_definition(b),
            (ImplicitVar::HoistedArg(a), ImplicitVar::HoistedArg(b)) => a.same_definition(b),
            (ImplicitVar::Module(a), ImplicitVar::Module(b)) => a == b,
            _ => false,
        }
    }

    /// This helper as the parse-stage variable it stands for -- the one
    /// conversion every consumer of a helper uses, so no consumer can build a
    /// helper's variable through a different representation than another.
    ///
    /// `dimensions` is the project's whole dimension context: a structural
    /// capture's declared axes are resolved against it.
    pub(crate) fn parsed_variable(&self, dimensions: &DimensionsContext) -> ParsedVariable {
        match self {
            ImplicitVar::Capture(c) => c.parsed_variable(dimensions),
            ImplicitVar::HoistedArg(a) => a.parsed_variable(),
            ImplicitVar::Module(m) => m.parsed_variable(),
        }
    }
}

/// File one synthesized helper under its name.
///
/// The one rule for two helpers claiming one name, applied wherever helpers
/// accumulate: inside one walk (`BuiltinVisitor`), across the per-element walks
/// of an apply-to-all or arrayed parent, and across the dt and initial passes
/// of one variable (`variable::parse_var`). A same-definition repeat is
/// absorbed into the first -- the dt and initial passes walk one equation
/// twice, and a capture the two passes mint for different snapshot consumers
/// is one helper serving both. A different helper claiming the name is refused
/// before it can overwrite the first: the silent last-wins alternative made a
/// later `Ast::Arrayed` slot read an earlier slot's capture (PR #668), and a
/// macro named `ARG1` invoked as `ARG1(k, k * 2)` mints its instance and its
/// second argument's helper under one name from ordinary source.
/// `DuplicateVariable` because two helpers really do claim one name.
pub(crate) fn insert_implicit_var(
    vars: &mut IndexMap<Ident<Canonical>, ImplicitVar>,
    var: ImplicitVar,
) -> Result<(), EquationError> {
    let ident = Ident::<Canonical>::new(var.ident());
    let Some(existing) = vars.get_mut(&ident) else {
        vars.insert(ident, var);
        return Ok(());
    };
    if existing.merge_same_definition(&var) {
        return Ok(());
    }
    Err(EquationError::detailed(
        ErrorCode::DuplicateVariable,
        0,
        0,
        format!("two different synthesized helpers both claim the name '{ident}'"),
    ))
}
