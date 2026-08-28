// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Parsed data synthesized while walking one variable: `PREVIOUS`/`INIT`
//! captures, module-call arguments hoisted into auxes, module instances, and
//! the lookup key every one of them is filed under.
//!
//! `PREVIOUS`/`INIT` compile to opcodes that read a fixed slot or a static
//! view of the snapshot regions. An argument that addresses neither
//! ([`crate::snapshot_arg::SnapshotAccess::Capture`]) has to be evaluated into
//! storage of its own first, and that storage is a *capture*: a hidden
//! per-callsite evaluation unit whose body is the argument.
//!
//! A capture carries the argument as an AST subtree. Its logical callsite is
//! positional -- the parent variable it was hoisted out of, the walk counter
//! `id`, the argument position, and any active-element suffix -- and those
//! inputs are the single source from which its name is constructed. Every
//! stage downstream of the parse consumes the subtree directly.
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
//! [`Capture::ident`] is the physical lookup key used today. Every stage that
//! files a unit of evaluation under a *name* -- including
//! `ImplicitVarMeta::{name,index_hint}`, the runlist (a lexicographic sort),
//! the layout's implicit section (name-sorted), the results offset map, and
//! symbolic bytecode's `VarRef` -- needs one for a capture too, and it must
//! sort exactly where it sorts today or the compiled artifact moves. The name
//! is derived once by [`synthetic_ident`] from the logical callsite inputs. No
//! downstream store physically addresses a capture by a `(parent, id)` tuple.
//!
//! A [`HoistedArg`] carries a module-call argument as an AST subtree for the
//! same reason as a capture. An [`ImplicitModule`] carries the target model and
//! input wiring directly. A bare identifier argument is not hoisted: it wires
//! straight to the module port by name, so an instance's references and
//! hoisted-argument list do not correspond one-to-one.

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

/// Which snapshot builtin uses a capture's storage.
///
/// This is also the capture's phase demand: `Previous` storage refreshes in
/// flows before the next snapshot commit, while `Init` storage is populated in
/// initials before the frozen snapshot is taken. The combined arm is reachable
/// when a variable's dt and active-initial equations mint the same positional
/// capture with the same body for different snapshot builtins. One storage
/// definition then serves both consumers and its phase requirements are their
/// union.
#[cfg_attr(any(test, feature = "debug-derive"), derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    Previous,
    Init,
    PreviousAndInit,
}

impl CaptureKind {
    /// The two source-level snapshot consumers. The combined arm is produced
    /// only by merging these across a variable's dt/initial parse passes.
    #[cfg(test)]
    pub(crate) const SOURCE_KINDS: [CaptureKind; 2] = [CaptureKind::Previous, CaptureKind::Init];

    /// The snapshot consumer spelled as a model writes it, with a distinct
    /// label for storage shared by both consumers.
    pub fn as_str(self) -> &'static str {
        match self {
            CaptureKind::Previous => "PREVIOUS",
            CaptureKind::Init => "INIT",
            CaptureKind::PreviousAndInit => "PREVIOUS+INIT",
        }
    }

    /// Whether this capture must be refreshed before each snapshot commit.
    pub(crate) const fn needs_flows(self) -> bool {
        matches!(self, CaptureKind::Previous | CaptureKind::PreviousAndInit)
    }

    /// Whether this capture must be populated before `initial_values` is frozen.
    pub(crate) const fn needs_initials(self) -> bool {
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
    /// [`synthetic_ident`] from the logical callsite inputs. Stored rather than
    /// recomputed because the parent's rewritten equation refers to the
    /// capture by it, and a capture outlives the visitor that knew its parent's
    /// name.
    ident: String,
    /// The walk counter this capture was minted at. It is retained as typed
    /// callsite data; current downstream lookup remains keyed by `ident`.
    id: usize,
    kind: CaptureKind,
    /// The argument, exactly as the walk left it. A scalar capture from an
    /// explicitly per-element walk is dimension-substituted; a shaped capture
    /// is not, so active axes, mappings and subdimensions remain available to
    /// ordinary apply-to-all lowering (including GH #541's bare arrayed name).
    arg: Expr0,
    /// The active apply-to-all element, when this capture is one element's own.
    suffix: Option<String>,
    /// The canonical dimension names a shaped capture applies over; empty for
    /// a scalar one. A shaped capture occupies one slot per element and is
    /// read back per element by the rewritten call, so every consumer that
    /// lays it out or subscripts it needs its declared shape.
    dims: Vec<String>,
}

impl Capture {
    /// Mint the capture for one `PREVIOUS`/`INIT` call of `parent`.
    ///
    /// `dims` empty makes a scalar capture; non-empty makes a shaped one whose
    /// body is held unsubstituted over those dimensions.
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

    /// Merge another use of this capture when it defines the same value.
    ///
    /// Source position is deliberately excluded. Definition equality is based
    /// on the BODY, not where the body was written: the apply-to-all
    /// expansion walks one cloned equation per element and collapses the
    /// identical results into one helper, and the dt and initial passes walk
    /// one equation twice. Comparing positions would refuse a pair that differs
    /// only in whitespace as two helpers claiming one name -- a compile error
    /// on a model that compiles today. `PartialEq` keeps positions, because
    /// salsa uses it to decide whether a re-parse changed anything and a moved
    /// span does change the diagnostics.
    pub(crate) fn merge_same_definition(&mut self, other: &Self) -> bool {
        let same_value = self.ident == other.ident
            && self.suffix == other.suffix
            && self.dims == other.dims
            && self.arg.eq_ignoring_loc(&other.arg);
        if same_value {
            self.kind = self.kind.union(other.kind);
        }
        same_value
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
    /// it does. A module call still requires one instance per active element;
    /// PREVIOUS/INIT alone preserve the capture's `Ast::ApplyToAll` storage
    /// shape and let ordinary lowering resolve its body per element. A second
    /// generation of helpers is impossible -- a capture body is already
    /// walked -- and is asserted rather than assumed.
    pub(crate) fn variable_stage0(&self, dimensions: &DimensionsContext) -> VariableStage0 {
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
        subtree_variable_stage0(
            &self.ident,
            ast,
            self.datamodel_equation(),
            errors,
            dimensions,
        )
    }
}

/// Build the parse-stage aux represented by an AST-carrying synthesized
/// helper.
///
/// Captures and hoisted module-call arguments are plain, non-negative,
/// non-flow auxes with no graphical function, units, or separate initial
/// equation. Their bodies have already been walked as part of the parent, but
/// `instantiate_implicit_modules` still owns module instantiation and snapshot
/// normalization. Snapshot-only shaped helpers remain `Ast::ApplyToAll`; a
/// nested helper generation would violate the argument-first walk order and is
/// therefore asserted.
fn subtree_variable_stage0(
    ident: &str,
    ast: Option<Ast<Expr0>>,
    eqn: datamodel::Equation,
    mut errors: Vec<EquationError>,
    dimensions: &DimensionsContext,
) -> VariableStage0 {
    let ident = Ident::<Canonical>::new(ident);
    let ast = ast.and_then(|ast| {
        match instantiate_implicit_modules(
            ident.as_str(),
            ast,
            Some(dimensions),
            None,
            empty_macro_registry(),
            None,
        ) {
            Ok((ast, nested)) => {
                debug_assert!(
                    nested.is_empty(),
                    "a synthesized helper's body must synthesize no further helpers"
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
        eqn: Some(eqn),
        errors,
        unit_errors: vec![],
        kind: VarKind::Aux {
            ast,
            init_ast: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
        },
    }
}

/// One module-call argument that is not a bare identifier, hoisted into its
/// own unit of evaluation so a module input port has a variable name to read.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct HoistedArg {
    ident: String,
    /// Shared with the module instance and the call's other hoisted arguments;
    /// the `arg{i}` name component identifies the argument within the call.
    id: usize,
    /// The argument exactly as the parent walk left it, including any active
    /// apply-to-all element substitution.
    arg: Expr0,
    suffix: Option<String>,
}

impl HoistedArg {
    pub(crate) fn new(
        parent: &str,
        id: usize,
        index: usize,
        arg: Expr0,
        suffix: Option<String>,
    ) -> Self {
        HoistedArg {
            ident: synthetic_ident(parent, id, &format!("arg{index}"), suffix.as_deref()),
            id,
            arg,
            suffix,
        }
    }

    pub fn ident(&self) -> &str {
        &self.ident
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn arg(&self) -> &Expr0 {
        &self.arg
    }

    pub fn suffix(&self) -> Option<&str> {
        self.suffix.as_deref()
    }

    pub(crate) fn same_definition(&self, other: &Self) -> bool {
        self.ident == other.ident
            && self.suffix == other.suffix
            && self.arg.eq_ignoring_loc(&other.arg)
    }

    pub(crate) fn variable_stage0(&self, dimensions: &DimensionsContext) -> VariableStage0 {
        subtree_variable_stage0(
            &self.ident,
            Some(Ast::Scalar(self.arg.clone())),
            datamodel::Equation::Scalar(print_eqn(&self.arg)),
            Vec::new(),
            dimensions,
        )
    }
}

/// One stdlib or macro module-function call expanded into a module instance.
///
/// The dependency graph reads the reference sources, fragment construction
/// reads the target model and wiring, and layout sizes the target model. Those
/// are therefore carried directly rather than wrapped in a datamodel variable.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct ImplicitModule {
    /// The external key derived from `(parent, id, call_name, suffix)` by the
    /// only constructor. It is stored because every name-keyed consumer needs
    /// a borrowed spelling, but callers cannot provide it independently of the
    /// logical callsite inputs.
    ident: String,
    /// Shared with every hoisted argument belonging to this call.
    id: usize,
    model_name: String,
    references: Vec<datamodel::ModuleReference>,
    suffix: Option<String>,
}

impl ImplicitModule {
    /// Mint one implicit module from its logical callsite and source-to-port
    /// wiring.
    ///
    /// Both the instance name and each `ModuleReference::dst` are derived here;
    /// accepting either from the caller would allow a typed `id`/`suffix` to
    /// disagree with the name consumed by layout and assembly.
    pub(crate) fn new(
        parent: &str,
        id: usize,
        call_name: &str,
        model_name: String,
        inputs: Vec<(String, String)>,
        suffix: Option<String>,
    ) -> Self {
        let ident = synthetic_ident(parent, id, call_name, suffix.as_deref());
        let references = inputs
            .into_iter()
            .map(|(src, port)| datamodel::ModuleReference {
                src,
                dst: format!("{ident}.{port}"),
            })
            .collect();
        ImplicitModule {
            ident,
            id,
            model_name,
            references,
            suffix,
        }
    }

    pub fn ident(&self) -> &str {
        &self.ident
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    pub fn references(&self) -> &[datamodel::ModuleReference] {
        &self.references
    }

    pub fn suffix(&self) -> Option<&str> {
        self.suffix.as_deref()
    }

    pub(crate) fn variable_stage0(&self) -> VariableStage0 {
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

    pub(crate) fn same_definition(&self, other: &Self) -> bool {
        self == other
    }
}

/// One helper the parse synthesized while walking a variable's equation.
///
/// Each arm is parsed data. The list is ordered by synthesis, and that order is
/// load-bearing because it rides salsa-cached values with derived `PartialEq`.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub enum ImplicitVar {
    /// A `PREVIOUS`/`INIT` argument hoisted out of its call.
    Capture(Capture),
    /// A non-identifier module-call argument hoisted into an aux.
    HoistedArg(HoistedArg),
    /// A module-function call expanded into an instance and its wiring.
    Module(ImplicitModule),
}

impl ImplicitVar {
    pub fn ident(&self) -> &str {
        match self {
            ImplicitVar::Capture(c) => c.ident(),
            ImplicitVar::HoistedArg(a) => a.ident(),
            ImplicitVar::Module(m) => m.ident(),
        }
    }

    /// Build the parse-stage variable represented by this typed helper.
    ///
    /// Every compile-stage consumer, including LTM, enters through this
    /// exhaustive dispatch so adding a helper form cannot leave one consumer
    /// reconstructing it through a different representation.
    pub(crate) fn variable_stage0(&self, dimensions: &DimensionsContext) -> VariableStage0 {
        match self {
            ImplicitVar::Capture(capture) => capture.variable_stage0(dimensions),
            ImplicitVar::HoistedArg(arg) => arg.variable_stage0(dimensions),
            ImplicitVar::Module(module) => module.variable_stage0(),
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
        match self {
            ImplicitVar::Capture(_) | ImplicitVar::HoistedArg(_) => false,
            ImplicitVar::Module(_) => true,
        }
    }

    /// The implicit-variable metadata has a stock bit for every evaluation
    /// unit, but the exhaustive arm list above contains no stock-producing
    /// form.
    pub fn is_stock(&self) -> bool {
        match self {
            ImplicitVar::Capture(_) | ImplicitVar::HoistedArg(_) | ImplicitVar::Module(_) => false,
        }
    }

    /// The dimension names this helper applies over, or `&[]` when it is
    /// scalar. An arrayed helper occupies one slot per element, so a consumer
    /// that lays it out or subscripts it reads this rather than assuming 1.
    pub fn equation_dims(&self) -> &[String] {
        match self {
            ImplicitVar::Capture(c) => c.dims(),
            ImplicitVar::HoistedArg(_) | ImplicitVar::Module(_) => &[],
        }
    }

    /// Merge another helper when it defines the same value. Capture consumers
    /// are unioned so one positional storage unit can serve both INIT and
    /// PREVIOUS; the other helper forms have no phase-specific use to merge.
    pub(crate) fn merge_same_definition(&mut self, other: &Self) -> bool {
        match (self, other) {
            (ImplicitVar::Capture(a), ImplicitVar::Capture(b)) => a.merge_same_definition(b),
            (ImplicitVar::HoistedArg(a), ImplicitVar::HoistedArg(b)) => a.same_definition(b),
            (ImplicitVar::Module(a), ImplicitVar::Module(b)) => a.same_definition(b),
            (ImplicitVar::Capture(_), ImplicitVar::HoistedArg(_) | ImplicitVar::Module(_))
            | (ImplicitVar::HoistedArg(_), ImplicitVar::Capture(_) | ImplicitVar::Module(_))
            | (ImplicitVar::Module(_), ImplicitVar::Capture(_) | ImplicitVar::HoistedArg(_)) => {
                false
            }
        }
    }
}
