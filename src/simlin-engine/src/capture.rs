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
//! for. Nothing prints a helper to equation text and parses it back.
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

use crate::ast::{Ast, Expr0, print_eqn};
use crate::builtins_visitor::{
    SnapshotIndexFacts, empty_macro_registry, instantiate_implicit_modules,
};
use crate::common::{Canonical, EquationError, ErrorCode, Ident};
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
            && self.suffix == other.suffix
            && self.dims == other.dims
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
    /// what the body goes through and why.
    fn parsed_variable(&self, dimensions: &DimensionsContext) -> VariableStage0 {
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
        let text = print_eqn(&self.arg);
        let eqn = if self.dims.is_empty() {
            datamodel::Equation::Scalar(text)
        } else {
            datamodel::Equation::ApplyToAll(self.dims.clone(), text)
        };
        subtree_parsed_variable(&self.ident, ast, eqn, errors, dimensions)
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
/// rather than an AST, and a helper has no source text of its own, so it prints
/// its subtree there. Two readers need it, both in LTM's link-score generator:
/// `ltm_augment::target_equation_dims` takes an ARRAYED target's
/// datamodel-cased dimension names off it (a target reporting no dimensions
/// gets a scalar link score), and `ltm_augment::scalar_or_a2a_target_expr`
/// falls back to that text whenever the target has no lowered AST, which
/// `db::analysis::reconstruct_implicit_variable` produces for any helper whose
/// lowering fails (`model::lower_variable` is total and discards the AST). That
/// fallback is the GH #965 generated-text boundary, which applies to every
/// variable and is not on the compile path.
///
/// Any span an error here carries indexes the PARENT's equation text, since
/// that is where the subtree was written, and that is how it is rendered:
/// `db::fragment_compile::compile_implicit_var_fragment` reports the errors
/// against the parent, so the snippet underlines the argument inside the
/// parent's equation.
///
/// A scalar body is not walked again. The parent's walk visited every node of
/// it and every decision that walk makes is final: a call's arguments are
/// walked before the call itself, so each `PREVIOUS`/`INIT` left in the body
/// was routed to a direct read under the parent's snapshot-index facts, and a
/// second walk here -- which has no owning model to ask -- could only
/// re-decide such a read against a different rule and mint a helper the parent
/// never filed. An ARRAYED capture's body is walked, because the visitor is
/// not a no-op on an apply-to-all: its per-element gate fires on a bare
/// `PREVIOUS`/`INIT` as well as on a module call, so the body becomes an
/// `Ast::Arrayed` of identical elements rather than staying an
/// `Ast::ApplyToAll`, and that expansion decides the fragment's shape. Such a
/// body holds no subscript at all (`BuiltinVisitor::hoist_capture`), so that
/// walk classifies no index; a helper it nonetheless minted is refused loudly
/// rather than dropped.
fn subtree_parsed_variable(
    ident: &str,
    ast: Option<Ast<Expr0>>,
    eqn: datamodel::Equation,
    mut errors: Vec<EquationError>,
    dimensions: &DimensionsContext,
) -> VariableStage0 {
    let ident = Ident::<Canonical>::new(ident);
    // Where the body was written in the parent's equation, for the one error
    // this function raises itself.
    let loc = match &ast {
        Some(Ast::Scalar(body) | Ast::ApplyToAll(_, body)) => body.get_loc(),
        Some(Ast::Arrayed(..)) | None => crate::ast::Loc::default(),
    };
    let ast = ast.and_then(|ast| {
        if let Ast::Scalar(_) = ast {
            return Some(ast);
        }
        match instantiate_implicit_modules(
            ident.as_str(),
            ast,
            Some(dimensions),
            // A helper body is given none of the model-level facts a parse
            // can carry: it names no module the parent's walk did not already
            // resolve, its indices were classified by that walk, and it is not
            // a macro body.
            SnapshotIndexFacts::NoModel,
            empty_macro_registry(),
            None,
        ) {
            Ok((ast, nested)) if nested.is_empty() => Some(ast),
            Ok(_) => {
                errors.push(EquationError::detailed(
                    ErrorCode::Generic,
                    loc.start,
                    loc.end,
                    format!("the body of synthesized helper '{ident}' synthesized further helpers"),
                ));
                None
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
        eqn: Some(eqn),
        errors,
        unit_errors: vec![],
        kind: VarKind::Aux {
            ast,
            // A helper's body is one expression. It has no separate
            // initial-phase equation, so both phases run it.
            init_ast: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
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
/// hoisted arguments of its call.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct HoistedArg {
    /// The name every name-keyed stage files this argument under, derived by
    /// [`synthetic_ident`] from the call's `(parent, id)`, the argument's
    /// position and the active element; see [`Capture::ident`] for why one
    /// exists.
    ident: String,
    /// The argument, exactly as the walk left it -- already dimension-
    /// substituted when the parent is expanded per element, because the helper
    /// is a scalar aux with no dimension context of its own to resolve a bare
    /// dimension name against.
    arg: Expr0,
}

impl HoistedArg {
    /// Mint the helper for argument `index` of the call `parent`'s walk is at
    /// counter `id`, expanding element `suffix` when the parent is walked per
    /// element.
    pub(crate) fn new(
        parent: &str,
        id: usize,
        index: usize,
        arg: Expr0,
        suffix: Option<&str>,
    ) -> Self {
        HoistedArg {
            ident: synthetic_ident(parent, id, &format!("arg{index}"), suffix),
            arg,
        }
    }

    pub fn ident(&self) -> &str {
        &self.ident
    }

    pub fn arg(&self) -> &Expr0 {
        &self.arg
    }

    /// Do these two hoisted arguments define the same value? Source position
    /// is excluded for the reason [`Capture::same_definition`] gives.
    pub(crate) fn same_definition(&self, other: &Self) -> bool {
        self.ident == other.ident && self.arg.eq_ignoring_loc(&other.arg)
    }

    fn parsed_variable(&self, dimensions: &DimensionsContext) -> VariableStage0 {
        subtree_parsed_variable(
            &self.ident,
            Some(Ast::Scalar(self.arg.clone())),
            datamodel::Equation::Scalar(print_eqn(&self.arg)),
            Vec::new(),
            dimensions,
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
    fn parsed_variable(&self) -> VariableStage0 {
        Variable {
            ident: Ident::<Canonical>::new(&self.ident),
            units: None,
            eqn: None,
            errors: vec![],
            unit_errors: vec![],
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

    /// The dimension names this helper applies over, or `&[]` when it is
    /// scalar. An arrayed helper occupies one slot per element, so a consumer
    /// that lays it out or subscripts it reads this rather than assuming 1.
    /// Only a capture is ever arrayed (GH #541): a hoisted argument is a scalar
    /// aux, and a module instance is sized by its target model.
    pub fn equation_dims(&self) -> &[String] {
        match self {
            ImplicitVar::Capture(c) => c.dims(),
            ImplicitVar::HoistedArg(_) | ImplicitVar::Module(_) => &[],
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
    /// `dimensions` is the project's whole dimension context: a helper body was
    /// written inside its parent's equation, and the parent's parse resolved
    /// every dimension name in it against the dimensions that parent reads.
    pub(crate) fn parsed_variable(&self, dimensions: &DimensionsContext) -> VariableStage0 {
        match self {
            ImplicitVar::Capture(c) => c.parsed_variable(dimensions),
            ImplicitVar::HoistedArg(a) => a.parsed_variable(dimensions),
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
/// absorbed into the first -- the apply-to-all expansion walks one cloned body
/// per element and the GH #541 arrayed capture it mints is deliberately
/// suffix-less, so every element's copy is the same helper, and a capture the
/// two passes mint for different snapshot consumers is one helper serving
/// both. A different helper claiming the name is refused before it can
/// overwrite the first: the silent last-wins alternative made a later
/// `Ast::Arrayed` slot read an earlier slot's capture (PR #668), and a macro
/// named `ARG1` invoked as `ARG1(k, k * 2)` mints its instance and its second
/// argument's helper under one name from ordinary source. `DuplicateVariable`
/// because two helpers really do claim one name.
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
