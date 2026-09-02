// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use indexmap::IndexMap;

use crate::ast::{Ast, BinaryOp, Expr0, IndexExpr0, Literal};
use crate::builtins::{UntypedBuiltinFn, is_builtin_fn};
use crate::capture::{
    Capture, CaptureKind, HoistedArg, ImplicitModule, ImplicitVar, insert_implicit_var,
};
use crate::common::{
    Canonical, CanonicalDimensionName, CanonicalElementName, EquationError, Ident, RawIdent,
    canonicalize,
};
use crate::dimensions::{
    AxisIndexName, Dimension, DimensionsContext, SubscriptIterator, resolve_axis_index_name,
};
use crate::eqn_err;
use crate::module_functions::{
    MacroCallResolution, MacroRegistry, ModuleFunctionDescriptor, stdlib_descriptor,
};
use crate::snapshot_arg::{SnapshotAccess, SnapshotArg, SnapshotIndex};

/// An empty registry used when no project macros are in scope (the
/// `BuiltinVisitor::new` constructor before `with_macro_registry` runs). Lets
/// the `macro_registry` field be a plain `&MacroRegistry` -- no `Option`
/// handling at the `resolve_call` site -- while still defaulting to "no
/// macros".
static EMPTY_MACRO_REGISTRY: LazyLock<MacroRegistry> = LazyLock::new(MacroRegistry::default);

/// The shared empty macro registry, for parse paths with no project macros
/// in scope (the `parse_var` convenience wrapper and the many test call
/// sites). Avoids allocating a fresh `MacroRegistry` per parse call.
pub(crate) fn empty_macro_registry() -> &'static MacroRegistry {
    &EMPTY_MACRO_REGISTRY
}

/// The model-local facts behind the one question a `PREVIOUS`/`INIT`
/// subscript asks of the owning model: does this identifier index pin ONE
/// declared element of the referenced variable's axis, so the call reads that
/// slot directly, or must the argument be captured? The decision has to be
/// made here, in the parse: a capture cannot be un-minted at lowering, and
/// capturing every identifier index costs a hidden slot and a flow evaluation
/// per read.
///
/// Which facts are in scope depends on who is parsing, and each arm is a
/// different rule because it has different facts:
///
/// * [`Self::Axes`] is the source parse (`db::parse_source_variable`). A bare
///   identifier is static when it is an element of the referenced variable's
///   declared axis at that position -- XMILE 1.0 section 3.7.1, "subscript
///   index names MAY be used unambiguously as part of a subscript ... once the
///   dimensions assigned to the variable have been specified", with footnote
///   9's precedence, "if a variable name is the same as an element name, the
///   element name prevails" (`docs/reference/xmile-v1.0.html`), which is the
///   compiler's own resolution of the same index
///   (`dimensions::resolve_axis_index_name`). A qualified `dimension·element`
///   is static when the project declares that element: the qualifying
///   dimension need not be the referenced axis, since its 1-based position is
///   what the compiler applies, positionally, to whatever axis is referenced
///   (`dimensions::resolve_axis_index_position`). Both facts are closures
///   because this layer has no database; the query supplies them through
///   per-name projections, so a parse depends on exactly the variable it
///   subscripts and the qualified names it spells, never on the model's
///   variable set.
/// * [`Self::ModelNames`] is the generated LTM parse. A generated equation may
///   subscript an LTM synthetic variable or a helper, neither of which is a
///   `SourceVariable` with declared axes to ask, so a bare identifier is
///   static when it is an element of ANY dimension and no variable of the
///   model shadows it. Where the two rules disagree, one mints a capture the
///   other does not; a model that compiles under either computes the same
///   value, because a capture's body resolves the index exactly as the direct
///   read would.
/// * [`Self::NoModel`] has no model in scope: a bare identifier is never
///   static, and a qualified name is static when the dimension context the
///   parse was given declares it.
#[derive(Clone, Copy)]
pub enum SnapshotIndexFacts<'a> {
    Axes {
        /// The declared dimension at position `axis` of the variable `base`
        /// names -- `base` as written in the subscript, not canonicalized --
        /// or `None` when the owning model declares no such variable or the
        /// variable has no such axis.
        axis_of: &'a dyn Fn(&str, usize) -> Option<Dimension>,
        /// Whether the project declares the element a qualified
        /// `dimension·element` spelling names.
        is_qualified_element: &'a dyn Fn(&str) -> bool,
    },
    ModelNames(&'a HashSet<Ident<Canonical>>),
    NoModel,
}

/// Check if the expression contains any **module-function** call that needs
/// per-element expansion in A2A context: a stdlib function, a project macro
/// (consulted via `macro_registry`), or `init`/`previous` (which may need
/// per-element temp vars though they create no standalone module).
///
/// This is the recognition predicate that gates the `Ast::ApplyToAll` /
/// `Ast::Arrayed` per-element expansion paths in `instantiate_implicit_modules`.
/// Macro-awareness is what lets an *arrayed* macro invocation enter the
/// per-element path (a scalar macro call expands via `walk()`'s `App`-arm
/// change regardless); Phase 4's arrayed fixtures exercise this end-to-end.
pub(crate) fn contains_module_call(expr: &Expr0, macro_registry: &MacroRegistry) -> bool {
    use Expr0::*;
    match expr {
        Const(_, _, _) => false,
        Var(_, _) => false,
        App(UntypedBuiltinFn(func, args), _) => {
            // The gate over-approximates: a passthrough or a renamed self-call
            // that lowers as a builtin still enters the per-element path, which
            // only costs an `Ast::Arrayed` of identical slots.
            if crate::builtins::is_stdlib_module_function(func.as_str())
                || !matches!(
                    macro_registry.resolve_call(func, None),
                    MacroCallResolution::Unresolved
                )
                || matches!(func.as_str(), "init" | "previous")
            {
                return true;
            }
            args.iter().any(|a| contains_module_call(a, macro_registry))
        }
        Subscript(_, args, _) => args.iter().any(|idx| match idx {
            IndexExpr0::Expr(e) => contains_module_call(e, macro_registry),
            _ => false,
        }),
        Op1(_, r, _) => contains_module_call(r, macro_registry),
        Op2(_, l, r, _) => {
            contains_module_call(l, macro_registry) || contains_module_call(r, macro_registry)
        }
        If(cond, t, f, _) => {
            contains_module_call(cond, macro_registry)
                || contains_module_call(t, macro_registry)
                || contains_module_call(f, macro_registry)
        }
    }
}

fn parse_module_order_arg(expr: &Expr0) -> Option<u32> {
    if let Expr0::Const(_, n, _) = expr {
        let n = n.value();
        let rounded = n.round();
        if (n - rounded).abs() < 1e-9 && rounded >= 0.0 {
            return Some(rounded as u32);
        }
    }
    None
}

fn rewrite_alias_module_call(
    func: String,
    args: Vec<Expr0>,
    loc: crate::builtins::Loc,
) -> Result<(String, Vec<Expr0>), EquationError> {
    // xmutil maps DELAY FIXED to DELAY(...); semantically this is a
    // pipeline delay, not an exponential smooth like delay1.  The stdlib
    // framework cannot represent the ring-buffer state needed for a true
    // pipeline delay, so for now we map it to delay1 as a rough
    // approximation.  This is known-incorrect for models where the exact
    // delay matters (e.g. delay_time >> DT).
    if func == "delay" {
        return Ok(("delay1".to_string(), args));
    }
    if !matches!(func.as_str(), "delayn" | "smthn") {
        return Ok((func, args));
    }
    if args.len() < 3 || args.len() > 4 {
        return eqn_err!(
            BadBuiltinArgs,
            loc.start,
            loc.end,
            format!(
                "{func} takes 3 or 4 arguments, but {} were given",
                args.len()
            )
        );
    }

    let mut it = args.into_iter();
    let input = it.next().unwrap();
    let delay_time = it.next().unwrap();
    let order_expr = it.next().unwrap();
    let init = it.next();

    let Some(order) = parse_module_order_arg(&order_expr) else {
        return eqn_err!(
            UnknownBuiltin,
            loc.start,
            loc.end,
            format!("{func}'s order argument must be the literal 1 or 3")
        );
    };
    let rewritten_name = match (func.as_str(), order) {
        ("delayn", 1) => "delay1",
        ("delayn", 3) => "delay3",
        ("smthn", 1) => "smth1",
        ("smthn", 3) => "smth3",
        _ => {
            return eqn_err!(
                UnknownBuiltin,
                loc.start,
                loc.end,
                format!("{func} of order {order} is not supported; use order 1 or 3")
            );
        }
    };

    let init_expr = init.unwrap_or_else(|| input.clone());
    Ok((
        rewritten_name.to_string(),
        vec![input, delay_time, init_expr],
    ))
}

/// Get dimension names from a slice of Dimensions
fn get_dimension_names(dimensions: &[Dimension]) -> Vec<CanonicalDimensionName> {
    dimensions
        .iter()
        .map(|d| match d {
            Dimension::Named(name, _) => name.clone(),
            Dimension::Indexed(name, _) => name.clone(),
        })
        .collect()
}

/// One `Ast::Arrayed` variable's per-element equations, in an order that is a
/// function of the model rather than of the process's hash seed (GH #1002).
///
/// `Ast::Arrayed` stores its slots in a `HashMap`, and both expansion paths in
/// `instantiate_implicit_modules` read that order into something observable.
/// The module-call path unions each slot's synthesized helpers into one vector,
/// whose ORDER rides two salsa-cached values with derived `PartialEq`
/// (`ParsedVariableResult::implicit_vars` and `VariableDeps::implicit_vars`),
/// so an unstable order defeats backdating and makes the compiled artifact
/// irreproducible. That path is the reachable one, and
/// `db::fragment_determinism_tests::per_element_implicit_helper_order_is_stable_across_fresh_databases`
/// gates it.
///
/// The other path walks every slot with ONE visitor, so the `n` counter that
/// names each synthesized helper (`$⁚v⁚{n}⁚arg0`) is handed out in iteration
/// order. It is taken when `!any_module_call || dimensions.is_empty()`, and the
/// two disjuncts are NOT alike:
///
/// * `!any_module_call` is inert. `contains_module_call` is true for every
///   construct that can synthesize a helper at all -- a stdlib call, a macro,
///   `init`, `previous` -- since the sole `hoist_capture` call site sits inside
///   the PREVIOUS/INIT routing branch, and the only other producer is
///   `expand_module_function`, whose macro and stdlib call sites are gated by
///   the same predicates `contains_module_call` consults. With no module call
///   there is nothing to name.
/// * `dimensions.is_empty()` is LIVE, and not a degenerate path: an `<aux>`
///   carrying `<element subscript=…>` children with no `<dimensions>` sibling
///   is an ordinary XMILE document, `xmile::variables`' `convert_equation!`
///   maps the missing element to an empty dimension list, and the model
///   compiles. Reverting the ordering here leaves the helper NAMES identical --
///   the counter is monotonic, so it emits `⁚0⁚`, `⁚1⁚`, `⁚2⁚` however the
///   slots are walked -- and re-binds which SLOT'S EQUATION each name carries,
///   which then moves the compiled artifact. Gated by
///   `db::fragment_determinism_tests::undimensioned_arrayed_helper_bindings_are_stable_across_fresh_databases`,
///   whose fixture is read through the real XMILE reader rather than
///   hand-built. (A failed dimension LOOKUP is not this case: it returns
///   `Err(BadDimensionName)` and yields no AST at all.)
///
/// Sorted by canonical element name rather than walked in declared row-major
/// order: the map is allowed to be sparse (an `EXCEPT` default covers the
/// slots it omits), so a `SubscriptIterator` walk would need a rule for a
/// position the map has no entry for, while a total order over the keys that
/// are actually present needs none.
fn elements_in_stable_order(
    elements: HashMap<CanonicalElementName, Expr0>,
) -> Vec<(CanonicalElementName, Expr0)> {
    let mut ordered: Vec<(CanonicalElementName, Expr0)> = elements.into_iter().collect();
    ordered.sort_by(|(a, _), (b, _)| a.cmp(b));
    ordered
}

pub struct BuiltinVisitor<'a> {
    variable_name: &'a str,
    /// Every helper synthesized during the current walk -- `PREVIOUS`/`INIT`
    /// captures, the module instances a stdlib or macro call expands into and
    /// the auxes their non-identifier arguments are hoisted into -- filed by
    /// name through [`Self::insert_implicit_var`], the only writer. A module
    /// minted here is the one name this walk knows to be module-backed for
    /// the rest of the walk, so a nested reference (`PREVIOUS(SMOOTH(...))`)
    /// captures; every other name's kind is lowering's to decide.
    ///
    /// Insertion-ordered, and that is load-bearing: every producer of
    /// `ParsedVariableResult::implicit_vars` emits this map with `.values()`,
    /// and the order must be a function of the equation alone. Insertion
    /// order is the walk's synthesis order, which is. A `HashMap` here would
    /// draw a fresh `RandomState` per parse, so two parses of one variable in
    /// one process could report the helpers in two orders (GH #1002); the two
    /// salsa values carrying this order -- `ParsedVariableResult::implicit_vars`
    /// and `VariableDeps::implicit_vars` -- have derived `PartialEq`, so an
    /// unstable order defeats backdating and makes the compiled artifact
    /// irreproducible (the GH #595 class). A helper's identity is its name
    /// (`ImplicitVarMeta::name`), never its position.
    vars: IndexMap<Ident<Canonical>, ImplicitVar>,
    n: usize,
    self_allowed: bool,
    /// Full dimension info for A2A context (used to identify indexed vs named dimensions)
    dimensions: Vec<Dimension>,
    /// Dimension names for A2A context (derived from dimensions)
    dimension_names: Vec<CanonicalDimensionName>,
    /// Current subscript element names being processed in A2A context
    active_subscript: Option<Vec<String>>,
    /// Reference to DimensionsContext for dimension mapping lookups
    dimensions_ctx: Option<&'a DimensionsContext>,
    /// What `index_is_static` may ask the owning model about an identifier
    /// index of a `PREVIOUS`/`INIT` subscript.
    snapshot_index: SnapshotIndexFacts<'a>,
    /// The per-project macro registry, consulted through
    /// `MacroRegistry::resolve_call` for every call before any builtin
    /// routing, so a project macro shadows an identically named builtin or
    /// stdlib function (the engine's rule; see `resolve_call`).
    macro_registry: &'a MacroRegistry,
    /// The canonical name of the macro model whose body this visitor is
    /// expanding, if any (i.e. the variable being parsed belongs to a
    /// macro-marked model). `None` for ordinary (non-macro-body) variables.
    /// `MacroRegistry::resolve_call` reads it to tell the importer-renamed
    /// builtin a macro body calls under its own name (GH #554) from a
    /// recursive call.
    enclosing_model: Option<&'a str>,
    /// `true` only when this visitor is walking ONE slot's equation of a
    /// per-element (`Ast::Arrayed`) variable -- distinct slots have DISTINCT
    /// equations, even though they share `variable_name` and each fresh visitor
    /// restarts `n` at 0.
    ///
    /// This selects whether the GH #541 arrayed `PREVIOUS`/`INIT` capture
    /// (`hoist_capture`'s arrayed branch) carries the active element in its
    /// name. In the `Ast::ApplyToAll` per-element expansion every slot walks the
    /// SAME cloned body, so the suffix-less capture `$⁚{var}⁚{n}⁚arg0` defines
    /// the same value in every slot and the union of the slots' helpers
    /// collapses them to one. In the `Ast::Arrayed` per-element expansion the
    /// bodies differ per slot, so a suffix-less capture would mint the SAME id
    /// for DIFFERENT bodies -- a silent collision that made a later slot read an
    /// earlier slot's capture (PR #668). When this flag is set, the arrayed
    /// capture appends the slot's element suffix (like the scalar captures
    /// always have), so distinct slots never collide. Set ONLY by
    /// the `Ast::Arrayed` branch of `instantiate_implicit_modules`; NOT by its
    /// `default_expr` visitor (which has no `active_subscript`, and so never
    /// reaches the arrayed-helper branch).
    per_element_equation: bool,
}

impl<'a> BuiltinVisitor<'a> {
    pub fn new(variable_name: &'a str) -> Self {
        Self {
            variable_name,
            vars: Default::default(),
            n: 0,
            self_allowed: false,
            dimensions: Vec::new(),
            dimension_names: Vec::new(),
            active_subscript: None,
            dimensions_ctx: None,
            snapshot_index: SnapshotIndexFacts::NoModel,
            macro_registry: &EMPTY_MACRO_REGISTRY,
            enclosing_model: None,
            per_element_equation: false,
        }
    }

    /// Walk one element of an apply-to-all or arrayed parent: `dimensions` are
    /// the parent's declared dimensions and `element` the active element on
    /// each of them, which is what per-element substitution and helper suffixes
    /// are derived from.
    fn with_active_element(mut self, dimensions: &[Dimension], element: &[String]) -> Self {
        self.dimension_names = get_dimension_names(dimensions);
        self.dimensions = dimensions.to_vec();
        self.active_subscript = Some(element.to_vec());
        self
    }

    /// Set the per-project macro registry so macro calls expand (and a
    /// project macro shadows an identically named builtin / stdlib func).
    fn with_macro_registry(mut self, macro_registry: &'a MacroRegistry) -> Self {
        self.macro_registry = macro_registry;
        self
    }

    /// Set the enclosing macro model name (#554). Pass the owning model's
    /// name when parsing a macro-marked model's body variable; the
    /// same-named-opcode-intrinsic exception in `walk()` keys off its
    /// canonicalization. A no-op (stays `None`) for non-macro-body callers.
    fn with_enclosing_model(mut self, enclosing_model: Option<&'a str>) -> Self {
        self.enclosing_model = enclosing_model;
        self
    }

    /// Mark this visitor as walking a per-element (`Ast::Arrayed`) slot
    /// equation, so the GH #541 arrayed `PREVIOUS`/`INIT` helper carries the
    /// element suffix and distinct slots never collide on a suffix-less id
    /// (PR #668). Set only by the `Ast::Arrayed` per-element expansion.
    fn with_per_element_equation(mut self, per_element_equation: bool) -> Self {
        self.per_element_equation = per_element_equation;
        self
    }

    /// Set the model-local facts `index_is_static` decides an identifier
    /// index with (see [`SnapshotIndexFacts`]).
    fn with_snapshot_index(mut self, snapshot_index: SnapshotIndexFacts<'a>) -> Self {
        self.snapshot_index = snapshot_index;
        self
    }

    /// Set the dimensions context so PREVIOUS/INIT can recognize statically
    /// resolvable subscript indices (qualified `dimension·element` references)
    /// and per-element substitution can follow declared mappings.
    fn with_dimensions_ctx(mut self, dimensions_ctx: Option<&'a DimensionsContext>) -> Self {
        self.dimensions_ctx = dimensions_ctx;
        self
    }

    /// Returns true when the identifier names a module instance synthesized
    /// earlier in this walk -- the one kind of module-backed name the parse
    /// knows without reading the owning model.
    fn is_known_module_ident(&self, ident: &Ident<Canonical>) -> bool {
        self.vars.get(ident).is_some_and(ImplicitVar::is_module)
    }

    /// Is `ident`, or the base of a `module·output` reference, a module
    /// instance this walk synthesized (`PREVIOUS(SMTH1(x, 3))`)? Such a call is
    /// captured: the instance's output is a value the parent's own equation
    /// only reads through the instance.
    ///
    /// Every other module-backed name -- an explicit module instance, a
    /// module-call aux, a bound input port -- is an ordinary reference here;
    /// lowering resolves it against the dependency's shape
    /// (`compiler::context::Context::snapshot_storage`): a scalar module-call
    /// aux and a qualified output port are fixed slots, a bound input port
    /// reads its own slot, and a bare module instance is refused. The `·`
    /// split runs on the RAW ident, so a fully-quoted composite such as the
    /// `"module·port"` `ltm_augment::quote_ident` emits keeps its quotes and
    /// misses -- correctly, since only a synthesized instance's OWN output
    /// reference is spelled unquoted.
    fn is_module_backed_ident(&self, ident: &RawIdent) -> bool {
        let canonical = Ident::new(&canonicalize(ident.as_str()));
        if self.is_known_module_ident(&canonical) {
            return true;
        }

        ident
            .as_str()
            .split_once('·')
            .is_some_and(|(base, _)| self.is_known_module_ident(&Ident::new(&canonicalize(base))))
    }

    /// Is this index of `base`'s axis `axis` *certainly* statically resolvable
    /// at compile time -- a numeric constant, or an identifier the walk's
    /// [`SnapshotIndexFacts`] resolve to one declared element (each arm's rule
    /// is stated there)?
    ///
    /// A helper minted by this walk is never an element name, so `vars` is
    /// consulted only by the generated-LTM rule, whose name set holds the
    /// model's explicit variables and cannot see them.
    fn index_is_static(&self, base: &str, axis: usize, idx: &IndexExpr0) -> bool {
        let IndexExpr0::Expr(expr) = idx else {
            return false;
        };
        let ident = match expr {
            Expr0::Const(_, _, _) => return true,
            Expr0::Var(ident, _) => ident,
            _ => return false,
        };
        let canonical = canonicalize(ident.as_str());
        match self.snapshot_index {
            SnapshotIndexFacts::Axes {
                axis_of,
                is_qualified_element,
            } => {
                if canonical.contains('·') {
                    is_qualified_element(&canonical)
                } else {
                    axis_of(base, axis).is_some_and(|axis_dim| {
                        matches!(
                            resolve_axis_index_name(&canonical, &axis_dim, |_| false),
                            AxisIndexName::Element(_)
                        )
                    })
                }
            }
            SnapshotIndexFacts::ModelNames(var_names) => {
                let Some(ctx) = self.dimensions_ctx else {
                    return false;
                };
                if ctx.lookup(&canonical).is_some() {
                    return true;
                }
                let elem = CanonicalElementName::from_raw(&canonical);
                let canonical_ident = Ident::new(&canonical);
                ctx.is_element_of_any_dimension(&elem)
                    && !var_names.contains(&canonical_ident)
                    && !self.vars.contains_key(&canonical_ident)
            }
            SnapshotIndexFacts::NoModel => self
                .dimensions_ctx
                .is_some_and(|ctx| ctx.lookup(&canonical).is_some()),
        }
    }

    /// Does this subscript index leave a whole dimension standing, rather than
    /// selecting one element of it?
    ///
    /// True for a wildcard or star-range, and for a bare reference to one of the
    /// ACTIVE apply-to-all dimensions -- the spelling `context.rs` resolves per
    /// element in scalar position and promotes to the whole array inside a
    /// vector builtin's array-operand position
    /// (`with_vector_builtin_wildcards`). A mapped or otherwise foreign
    /// dimension name is deliberately NOT included: those need
    /// `substitute_dimension_refs`' positional translation, which is only
    /// available here.
    fn index_spans_a_dimension(&self, idx: &IndexExpr0) -> bool {
        match idx {
            IndexExpr0::Wildcard(_) | IndexExpr0::StarRange(_, _) => true,
            IndexExpr0::Expr(Expr0::Var(ident, _)) => {
                let canonical = CanonicalDimensionName::from_raw(ident.as_str());
                self.dimension_names.iter().any(|d| d == &canonical)
            }
            _ => false,
        }
    }

    /// Is `arg` a subscripted reference that is ARRAY-shaped -- one whose
    /// indices leave at least one dimension standing, and where every index is
    /// either that or statically resolvable (GH #995)?
    ///
    /// This is the `View` arm of [`BuiltinVisitor::snapshot_arg`], named
    /// separately because the substitution site reads it as a question about
    /// the argument's shape rather than about the capture decision.
    ///
    /// Such an argument is passed through to lowering untouched: no per-element
    /// substitution, and no synthesized capture helper. That is what lets an
    /// array-valued `PREVIOUS`/`INIT` exist at all, and it puts the decision in
    /// the one place that can make it. `PREVIOUS(vals[d])` means the element in
    /// `y[d] = PREVIOUS(vals[d])` and the whole array in
    /// `y[d] = VECTOR SORT ORDER(PREVIOUS(vals[d]), 1)`, exactly as bare
    /// `vals[d]` does -- and only `compiler::context` knows which position it
    /// is in. Substituting here would pin it to one element before that context
    /// exists; the helper path cannot hold it either, since a scalar
    /// `Equation::Scalar` helper holding `vals[*]` does not compile.
    ///
    /// Every other shape takes the scalar routing: an all-static subscript
    /// substitutes and compiles to `LoadPrev`/`LoadInitial` against a fixed
    /// slot, and anything with a dynamic index gets a capture helper (which is
    /// also what gives a dynamic index the correct lagged semantics).
    fn arg_is_array_shaped(&self, arg: &Expr0) -> bool {
        self.snapshot_arg(arg).access() == SnapshotAccess::View
    }

    /// Classify index `axis` of a subscript on `base` for the shared
    /// `PREVIOUS`/`INIT` predicate.
    ///
    /// Spanning is asked BEFORE staticness because a name can satisfy both --
    /// an active apply-to-all dimension that some *other* dimension also
    /// declares as an element -- and what such an index leaves standing is what
    /// the reference means. [`SnapshotArg::subscripted`] carries the same
    /// precedence for the fold; the two must agree.
    fn classify_snapshot_index(&self, base: &str, axis: usize, idx: &IndexExpr0) -> SnapshotIndex {
        if self.index_spans_a_dimension(idx) {
            SnapshotIndex::SpansDimension
        } else if self.index_is_static(base, axis, idx) {
            SnapshotIndex::Static
        } else {
            SnapshotIndex::Dynamic
        }
    }

    /// Reduce a source `PREVIOUS`/`INIT` argument to the form
    /// [`SnapshotArg::access`] decides over, so this parse-time capture
    /// decision and codegen's direct-read decision cannot drift apart -- the
    /// GH #568 failure class, where the dependency graph and the bytecode
    /// disagree about what a variable reads.
    ///
    /// The only base this classifies as `not_storage` on its own account is a
    /// module instance synthesized in this walk. What every other name denotes
    /// is a fact about the owning model, which the parse does not read; the
    /// reference passes through and lowering resolves it against the
    /// dependency's shape.
    fn snapshot_arg(&self, arg: &Expr0) -> SnapshotArg {
        match arg {
            Expr0::Var(ident, _) if !self.is_module_backed_ident(ident) => SnapshotArg::whole(),
            Expr0::Subscript(id, indices, _) if !self.is_module_backed_ident(id) => {
                SnapshotArg::subscripted(
                    indices
                        .iter()
                        .enumerate()
                        .map(|(axis, idx)| self.classify_snapshot_index(id.as_str(), axis, idx)),
                )
            }
            _ => SnapshotArg::not_storage(),
        }
    }

    /// Rewrite dimension references in `expr` to the active element: processing
    /// element `A2` of dimension `SubA`, `input[SubA]` becomes `input[A2]`, and
    /// a foreign dimension name resolves through `resolve_mapped_read`.
    ///
    /// An `Expr0` -> `Expr0` rewrite, and it is where the per-element decision
    /// has to be made: everything it rewrites is about to be hoisted OUT of the
    /// apply-to-all body into a helper of its own -- a [`Capture`], a
    /// [`HoistedArg`], or a module-input `src` name -- and a helper is a SCALAR
    /// variable with no dimensions of its own. Lowering cannot make the decision
    /// later, because by then the helper's fragment is an `Ast::Scalar` with no
    /// active element for a bare `SubA` to resolve against; it would lower as a
    /// dimension in scalar context and the fragment would be refused. So the
    /// rewrite is the parse-time stand-in for the compiler's resolution of the
    /// same spelling and must give the same answer, which
    /// `mapped_reference_semantics_tests`' hoisted-argument column measures.
    ///
    /// The one shape deliberately NOT rewritten is the GH #541 arrayed capture,
    /// which keeps its body unsubstituted precisely because it is arrayed and
    /// therefore does have dimensions to resolve against ([`Self::hoist_capture`]).
    fn substitute_dimension_refs(&self, expr: Expr0) -> Expr0 {
        use Expr0::*;
        use std::mem;

        let subscript = match &self.active_subscript {
            Some(s) => s,
            None => return expr,
        };

        match expr {
            Const(_, _, _) => expr,
            Var(ref ident, loc) => {
                // Check if this var is a dimension name that should be substituted
                let canonical_name = CanonicalDimensionName::from_raw(ident.as_str());
                for (i, dim_name) in self.dimension_names.iter().enumerate() {
                    if &canonical_name == dim_name {
                        // Check if this is an indexed or named dimension
                        match &self.dimensions[i] {
                            Dimension::Indexed(_, _) => {
                                // For indexed dimensions, the subscript element is a number
                                // Use it directly as a Const
                                let val: f64 = subscript[i].parse().unwrap_or(0.0);
                                return Const(subscript[i].clone(), Literal::new(val), loc);
                            }
                            Dimension::Named(_, _) => {
                                // For named dimensions, use qualified element (dimension·element).
                                // During constify_dimensions, this gets looked up via
                                // DimensionsContext::lookup which returns a 1-based index
                                // (from indexed_elements). The compiler then converts this
                                // 1-based value to 0-based when processing subscript indices.
                                let qualified_name =
                                    format!("{}·{}", dim_name.as_str(), subscript[i]);
                                return Var(RawIdent::new_from_str(&qualified_name), loc);
                            }
                        }
                    }
                }
                // A FOREIGN dimension name -- one the parent does not iterate --
                // resolves against each active element through
                // `resolve_mapped_read`, the same rule the compiler applies to
                // this spelling (`compiler::subscript::build_view_from_ops`):
                // the element's own name on the source axis first, then the
                // declared mapping, then a mapped parent. The context is the
                // parent's NARROWED one, so a dimension with no declared
                // relation to the active ones is not found and the name is
                // left for lowering to refuse.
                if let Some(ctx) = self.dimensions_ctx
                    && let Some(source_axis) = ctx.get(&canonical_name)
                {
                    for (i, active) in self.dimensions.iter().enumerate() {
                        let target_element = CanonicalElementName::from_raw(&subscript[i]);
                        if let Some(source_element) =
                            ctx.resolve_mapped_read(source_axis, active, &target_element)
                        {
                            let qualified_name =
                                format!("{}·{}", canonical_name.as_str(), source_element.as_str());
                            return Var(RawIdent::new_from_str(&qualified_name), loc);
                        }
                    }
                }
                expr
            }
            App(UntypedBuiltinFn(func, args), loc) => {
                let args = args
                    .into_iter()
                    .map(|a| self.substitute_dimension_refs(a))
                    .collect();
                App(UntypedBuiltinFn(func, args), loc)
            }
            Subscript(id, args, loc) => {
                let args = args
                    .into_iter()
                    .map(|idx| match idx {
                        IndexExpr0::Expr(e) => IndexExpr0::Expr(self.substitute_dimension_refs(e)),
                        other => other,
                    })
                    .collect();
                Subscript(id, args, loc)
            }
            Op1(op, mut r, loc) => {
                *r = self.substitute_dimension_refs(mem::take(&mut *r));
                Op1(op, r, loc)
            }
            Op2(op, mut l, mut r, loc) => {
                *l = self.substitute_dimension_refs(mem::take(&mut *l));
                *r = self.substitute_dimension_refs(mem::take(&mut *r));
                Op2(op, l, r, loc)
            }
            If(mut cond, mut t, mut f, loc) => {
                *cond = self.substitute_dimension_refs(mem::take(&mut *cond));
                *t = self.substitute_dimension_refs(mem::take(&mut *t));
                *f = self.substitute_dimension_refs(mem::take(&mut *f));
                If(cond, t, f, loc)
            }
        }
    }

    /// Get the subscript suffix for module/helper names (e.g., "a2" or "a1,b2")
    fn subscript_suffix(&self) -> String {
        match &self.active_subscript {
            Some(s) => s.join(",").to_lowercase(),
            None => String::new(),
        }
    }

    /// Does `arg` contain a *bare* (unsubscripted) variable reference that is
    /// neither a dimension name (those get rewritten to qualified elements by
    /// `substitute_dimension_refs`) nor the output of a module instance
    /// synthesized in this walk (the one module-backed name the parse knows)?
    /// Such a bare reference is the one that breaks the
    /// scalar-helper path: if it names an *arrayed* variable, a bare arrayed
    /// name has no meaning inside a scalar `Equation::Scalar` helper, so the
    /// helper fragment fails to compile (GH #541 -- the canonical trigger is a
    /// nested `PREVIOUS(PREVIOUS(arr))`, whose inner `PREVIOUS(arr)` is an
    /// expression arg routed through `hoist_capture`).
    ///
    /// We cannot tell here whether the bare name is arrayed or scalar (the
    /// visitor has no variable->dimensions map -- the per-variable parse path
    /// deliberately withholds the model's name set for salsa incrementality),
    /// so the conservative answer is "treat any surviving bare reference as
    /// possibly-arrayed and route it through an arrayed helper". An arrayed
    /// (`Equation::ApplyToAll`) helper broadcasts a *scalar* reference cleanly
    /// too, so a false positive (a bare scalar reference) stays correct -- it
    /// is merely held in a broadcast array rather than a scalar slot. A
    /// `Subscript` base (`arr[Dim]`) is NOT a bare reference: after
    /// substitution it is a per-element scalar access the scalar helper holds
    /// fine, which is why the explicitly-subscripted form already compiles.
    fn arg_has_bare_var_ref(&self, arg: &Expr0) -> bool {
        use Expr0::*;
        match arg {
            Const(_, _, _) => false,
            Var(ident, _) => {
                let canonical = CanonicalDimensionName::from_raw(ident.as_str());
                let is_active_dim = self.dimension_names.iter().any(|d| d == &canonical);
                !is_active_dim && !self.is_module_backed_ident(ident)
            }
            // A subscripted reference is already a per-element scalar access; a
            // wildcard/range index lives inside an array-reducer (handled by
            // its own array-view path), so we do not descend into indices here.
            Subscript(_, _, _) => false,
            // A scalar-collapsing array reducer (`SUM`/`MEAN`/`MIN`/`MAX`/
            // `STDDEV`/`SIZE`) collapses its arrayed argument to a SCALAR, so
            // a bare arrayed name inside it (`SUM(hfc_emissions)`) is
            // well-typed in a *scalar* helper -- it does NOT need the
            // arrayed-helper path, and wrapping `SUM(arr)` in an `ApplyToAll`
            // would broadcast a scalar reduce across the active dims and
            // corrupt the result (LTM link-score numerators are exactly this
            // shape). Do not descend into such a reducer. `RANK` is in the
            // reducer table but is ARRAY-valued (Vensim's VECTOR RANK), so a
            // bare arrayed name inside it MUST take the arrayed-helper path:
            // captured into a scalar helper, `rank(pop, 1)` is ill-typed and
            // the helper fragment fails (GH #742). The lowercasing is
            // defensive belt-and-suspenders: parsed `Expr0` function names
            // are already lowercase by construction (the parser lowercases
            // function-call identifiers).
            App(UntypedBuiltinFn(func, args), _) => {
                !crate::ltm_agg::reducer_collapses_to_scalar(&func.to_ascii_lowercase(), args.len())
                    && args.iter().any(|a| self.arg_has_bare_var_ref(a))
            }
            Op1(_, r, _) => self.arg_has_bare_var_ref(r),
            Op2(_, l, r, _) => self.arg_has_bare_var_ref(l) || self.arg_has_bare_var_ref(r),
            If(cond, t, f, _) => {
                self.arg_has_bare_var_ref(cond)
                    || self.arg_has_bare_var_ref(t)
                    || self.arg_has_bare_var_ref(f)
            }
        }
    }

    /// Does `arg` contain ANY `Subscript` expression?
    ///
    /// The GH #541 arrayed-helper path is restricted to args with NO subscript:
    /// the ONLY shape that genuinely needs it is a *bare* arrayed name
    /// (`PREVIOUS(PREVIOUS(pop))`), which carries no subscript. The moment a
    /// subscript is present -- whether by an active dimension (`reg[region]`),
    /// a mapped/foreign dimension (`agg[Aggregated Regions]` inside A2A-over-COP,
    /// the C-LEARN idiom), or a literal element -- the OLD per-element scalar
    /// helper path handles it correctly: `substitute_dimension_refs` translates
    /// each subscript to a concrete per-element reference (active dims to
    /// `dim·elem`, mapped dims through `translate_via_mapping`), which compiles
    /// in the scalar helper exactly as it did pre-#541. Wrapping a subscripted
    /// body in an `Equation::ApplyToAll` helper instead is both unnecessary and
    /// the source of subtle bugs (mapped-subscript ill-typing, per-element
    /// layout/value divergence under LTM), so we keep the proven scalar path for
    /// every subscripted arg. A bare arrayed name *alongside* a subscript in the
    /// same arg therefore also takes the scalar path; if that bare name is
    /// genuinely arrayed it fails cleanly there, as it did pre-#541 -- a known
    /// limitation no corpus model hits.
    fn arg_has_subscript(&self, arg: &Expr0) -> bool {
        use Expr0::*;
        match arg {
            Const(_, _, _) | Var(_, _) => false,
            Subscript(_, _, _) => true,
            App(UntypedBuiltinFn(_, args), _) => args.iter().any(|a| self.arg_has_subscript(a)),
            Op1(_, r, _) => self.arg_has_subscript(r),
            Op2(_, l, r, _) => self.arg_has_subscript(l) || self.arg_has_subscript(r),
            If(cond, t, f, _) => {
                self.arg_has_subscript(cond)
                    || self.arg_has_subscript(t)
                    || self.arg_has_subscript(f)
            }
        }
    }

    /// Hoist a `PREVIOUS`/`INIT` argument into a [`Capture`] and return the
    /// reference expression the caller substitutes for the argument.
    ///
    /// Outside A2A context (`active_subscript == None`), or when the captured
    /// argument carries no bare variable reference, the capture is scalar: it
    /// holds the (dimension-substituted) argument and the reference is a bare
    /// `Var`.
    ///
    /// In A2A context, when the argument contains a bare variable reference
    /// (`arg_has_bare_var_ref`) AND no subscript at all (`arg_has_subscript`),
    /// the capture is instead *arrayed* -- it applies over the active
    /// dimensions and holds the argument *without* per-element substitution, so
    /// a bare arrayed name keeps its array shape (GH #541). The returned
    /// reference subscripts it by the active element (`capture[<element>]`), a
    /// static per-element access the caller's outer `PREVIOUS`/`INIT` then
    /// compiles to a fixed slot. Its name carries NO element suffix, so every
    /// element of the enclosing apply-to-all produces the same capture, which
    /// `instantiate_implicit_modules` deduplicates into one.
    ///
    /// Any subscripted arg takes the scalar path instead (see
    /// `arg_has_subscript`): `substitute_dimension_refs` translates each
    /// subscript per element, which avoids the arrayed capture's
    /// subscript-interaction bugs (the C-LEARN regression).
    fn hoist_capture(&mut self, kind: CaptureKind, arg: Expr0) -> Result<Expr0, EquationError> {
        let loc = crate::builtins::Loc::default();

        // The active per-element subscript, cloned up front so the helper-
        // insertion (`&mut self.vars`) below does not conflict with borrowing
        // it. `Some` exactly in A2A context; cheap (a few element-name Strings).
        let active_subscript = self.active_subscript.clone();
        if let Some(subscript) = active_subscript.as_ref()
            && self.arg_has_bare_var_ref(&arg)
            && !self.arg_has_subscript(&arg)
        {
            // Arrayed capture holding the *un-substituted* argument so bare
            // arrayed names stay arrayed and a subscripted reference (`arr[Dim]`)
            // broadcasts over the capture's own dimensions instead of being
            // frozen to one element.
            //
            // The name omits the element suffix in the `Ast::ApplyToAll`
            // per-element expansion (every slot walks the same cloned body, so
            // every slot's capture defines the same value and the union of the
            // slots' helpers collapses the N copies into one). But in the
            // `Ast::Arrayed` per-element expansion (`per_element_equation`) each
            // slot has its OWN body, so a suffix-less id would mint the same
            // name for different bodies -- a silent collision (PR #668). There
            // the name carries the slot's element suffix, exactly like the
            // scalar captures, so distinct slots get distinct captures.
            let subscript_suffix = self.subscript_suffix();
            let suffix = (self.per_element_equation && !subscript_suffix.is_empty())
                .then_some(subscript_suffix);
            // The capture's dims carry the active (canonical) dimension names;
            // `variable::get_dimensions` resolves them canonically against the
            // project dimensions, so they match a dimension declared with
            // original casing/spacing.
            let dims: Vec<String> = self
                .dimension_names
                .iter()
                .map(|d| d.as_str().to_string())
                .collect();
            let capture = Capture::new(self.variable_name, self.n, kind, arg, suffix, dims);
            let id = capture.ident().to_string();
            self.insert_implicit_var(ImplicitVar::Capture(capture))?;
            self.n += 1;

            // Reference the helper at the active element: one qualified
            // `dimension·element` index per active dimension. These are
            // statically resolvable, so the outer PREVIOUS/INIT compiles to a
            // fixed slot rather than synthesizing yet another helper.
            let indices: Vec<IndexExpr0> = self
                .dimension_names
                .iter()
                .zip(subscript.iter())
                .map(|(dim_name, elem)| {
                    let qualified = format!("{}·{}", dim_name.as_str(), elem);
                    IndexExpr0::Expr(Expr0::Var(RawIdent::new_from_str(&qualified), loc))
                })
                .collect();
            return Ok(Expr0::Subscript(RawIdent::new_from_str(&id), indices, loc));
        }

        let transformed_arg = self.substitute_dimension_refs(arg);
        let subscript_suffix = self.subscript_suffix();
        let suffix = (!subscript_suffix.is_empty()).then_some(subscript_suffix);
        let capture = Capture::new(
            self.variable_name,
            self.n,
            kind,
            transformed_arg,
            suffix,
            Vec::new(),
        );
        let id = capture.ident().to_string();
        self.insert_implicit_var(ImplicitVar::Capture(capture))?;
        self.n += 1;
        Ok(Expr0::Var(RawIdent::new_from_str(&id), loc))
    }

    /// File one helper this walk synthesized -- the only writer of `vars`.
    /// `capture::insert_implicit_var` owns the rule for two helpers claiming
    /// one name: a repeat is idempotent, a different helper is refused before
    /// it can overwrite the first.
    fn insert_implicit_var(&mut self, var: ImplicitVar) -> Result<(), EquationError> {
        insert_implicit_var(&mut self.vars, var)
    }

    fn walk_index(&mut self, expr: IndexExpr0) -> Result<IndexExpr0, EquationError> {
        use IndexExpr0::*;
        let result: IndexExpr0 = match expr {
            Wildcard(_) => expr,
            StarRange(_, _) => expr,
            Range(_, _, _) => expr,
            DimPosition(_, _) => expr,
            Expr(expr) => Expr(self.walk(expr)?),
        };

        Ok(result)
    }

    /// Expand one module-function call (stdlib or macro) into an
    /// [`ImplicitModule`] plus a [`HoistedArg`] for each argument that is not a
    /// bare identifier, returning the reference the call is replaced by: the
    /// instance's primary output, `{instance}·{primary_output}`.
    ///
    /// The descriptor supplies the target model, the ordered input ports and
    /// the primary output; `func` names the instance. The instance and its
    /// hoisted arguments share one walk counter, which advances once per call.
    ///
    /// A call can wire at most one argument per port: more is refused here,
    /// before any argument is hoisted, so no orphan helper is filed. A stdlib
    /// call may pass fewer (`SMTH1` without an initial value), leaving the
    /// trailing ports unwired; a macro's exact arity was checked at the routing
    /// decision.
    fn expand_module_function(
        &mut self,
        descriptor: &ModuleFunctionDescriptor,
        func: &str,
        args: Vec<Expr0>,
        loc: crate::builtins::Loc,
    ) -> Result<Expr0, EquationError> {
        use Expr0::*;

        // Only the over-arity half is refused here: a call with FEWER
        // arguments than ports leaves the trailing ports unwired, which is
        // right for an optional initial value and wrong for a missing
        // averaging time (GH #1031 owns the per-port required/optional fact).
        let ports = descriptor.parameter_ports.len();
        if args.len() > ports {
            return eqn_err!(
                BadBuiltinArgs,
                loc.start,
                loc.end,
                format!(
                    "{func} takes at most {ports} argument(s), but {} were given",
                    args.len()
                )
            );
        }

        let subscript_suffix = self.subscript_suffix();
        let suffix = (!subscript_suffix.is_empty()).then_some(subscript_suffix.as_str());

        // Argument-first: each argument that needs a helper is filed before
        // the instance, which is the order the implicit-var list keeps and the
        // order the salsa-cached vectors compare in.
        let mut sources: Vec<String> = Vec::with_capacity(args.len());
        for (i, arg) in args.into_iter().enumerate() {
            // Per element, a bare active dimension name becomes the qualified
            // element it stands for, and a subscript naming one is resolved to
            // that element BEFORE the hoist: the helper is a scalar aux with no
            // dimension context of its own to resolve it against later.
            let src = match arg {
                Var(name, var_loc) => {
                    match self.substitute_dimension_refs(Var(name.clone(), var_loc)) {
                        // A bare identifier wires straight to the port.
                        Var(substituted, _) => substituted.as_str().to_string(),
                        // An INDEXED active dimension name substitutes to a number,
                        // which has no name to wire; the port reads the dimension
                        // name as a variable, which the dependency stage refuses as
                        // unknown. Hoisting the number instead would make
                        // `SMTH1(Idx, t)` compile, a shape change with its own
                        // ledger row.
                        _ => name.as_str().to_string(),
                    }
                }
                arg => {
                    let arg = self.substitute_dimension_refs(arg);
                    let hoisted = HoistedArg::new(self.variable_name, self.n, i, arg, suffix);
                    let src = hoisted.ident().to_string();
                    self.insert_implicit_var(ImplicitVar::HoistedArg(hoisted))?;
                    src
                }
            };
            sources.push(src);
        }

        let module = ImplicitModule::new(
            self.variable_name,
            self.n,
            func,
            suffix,
            descriptor.model_name.clone(),
            sources
                .into_iter()
                .zip(descriptor.parameter_ports.iter().map(String::as_str)),
        );
        // U+00B7 (·) is the compile-time separator for a module's variable;
        // `primary_output` is `output` for every stdlib model.
        let output = format!("{}\u{b7}{}", module.ident, descriptor.primary_output);
        self.insert_implicit_var(ImplicitVar::Module(module))?;
        self.n += 1;
        Ok(Var(RawIdent::new_from_str(&output), loc))
    }

    fn walk(&mut self, expr: Expr0) -> Result<Expr0, EquationError> {
        use Expr0::*;
        use std::mem;
        let result: Expr0 = match expr {
            Const(_, _, _) => expr,
            Var(ref ident, loc) => {
                if ident.as_str().eq_ignore_ascii_case("self") && self.self_allowed {
                    Var(RawIdent::new_from_str(self.variable_name), loc)
                } else {
                    expr
                }
            }
            App(UntypedBuiltinFn(func, args), loc) => {
                let orig_self_allowed = self.self_allowed;
                self.self_allowed |= func == "previous" || func == "size";
                let args: Result<Vec<Expr0>, EquationError> =
                    args.into_iter().map(|e| self.walk(e)).collect();
                self.self_allowed = orig_self_allowed;
                let args = args?;

                // Macro-shadows-everything precedence (the engine's rule; see
                // `resolve_call` for what the specs say): a project macro is
                // resolved here, BEFORE alias normalization, MODULO,
                // PREVIOUS/INIT, `is_builtin_fn` and the stdlib lookup, so a
                // macro named `SSHAPE` or `RAMP FROM TO` expands as the macro
                // even though it parsed as `CallKind::Builtin`.
                // `resolve_call` is the one statement of that precedence and
                // of its two exceptions, and recursion analysis reads the same
                // decision, so the two cannot disagree about which calls
                // expand. A call that does not expand keeps `func`/`args` and
                // falls through to the builtin routing below: a passthrough
                // macro at an external call site, its declared arity checked;
                // and the enclosing macro's own renamed builtin, under the
                // builtin's arity.
                let registry = self.macro_registry;
                match registry.resolve_call(&func, self.enclosing_model) {
                    MacroCallResolution::Expand(descriptor) => {
                        macro_arity(descriptor, &func, args.len(), loc)?;
                        return self.expand_module_function(descriptor, &func, args, loc);
                    }
                    MacroCallResolution::Passthrough(descriptor) => {
                        macro_arity(descriptor, &func, args.len(), loc)?;
                    }
                    MacroCallResolution::RenamedBuiltinSelfCall
                    | MacroCallResolution::Unresolved => {}
                }

                let (func, args) = rewrite_alias_module_call(func, args, loc)?;
                // MODULO(x, y) is the function-call form of the MOD binary operator
                if func == "modulo" && args.len() == 2 {
                    let mut it = args.into_iter();
                    let lhs = it.next().unwrap();
                    let rhs = it.next().unwrap();
                    return Ok(Op2(BinaryOp::Mod, Box::new(lhs), Box::new(rhs), loc));
                }
                let args = if func == "previous" && args.len() == 1 {
                    let mut args = args;
                    args.push(Const("0".to_string(), Literal::new(0.0), loc));
                    args
                } else {
                    args
                };
                // PREVIOUS and INIT opcode routing:
                //
                // Both compile to intrinsic opcodes (LoadPrev / LoadInitial)
                // that read a fixed slot, so arg0 must resolve to a static
                // location:
                //   * a direct variable reference, or
                //   * a subscripted reference whose every index is statically
                //     resolvable -- a numeric constant, a qualified
                //     `dimension·element` reference, or a bare element of the
                //     referenced axis (see `index_is_static`).
                //
                // Anything else (nested PREVIOUS, PREVIOUS(expr), the output
                // of a module instance synthesized in this walk, dynamic
                // subscript indices) is rewritten through a synthesized scalar
                // temp variable that captures the value each timestep -- which
                // also gives dynamic indices the correct lagged semantics (the
                // index itself is read at the *previous* step). Which STORAGE
                // a direct reference addresses -- a plain slot, a module output
                // port, a bound input port's own slot, or a bare module
                // instance that has none -- is resolved at lowering from the
                // dependency's shape.
                //
                // In A2A per-element context, dimension references inside a
                // subscripted arg0 are substituted to qualified element
                // references FIRST, so `PREVIOUS(x[Dim], ...)` in an
                // apply-to-all equation resolves to a per-element static slot
                // instead of synthesizing one helper aux per element.
                let is_prev_routing = func == "previous" && args.len() == 2;
                let is_init_routing = func == "init" && args.len() == 1;
                if is_prev_routing || is_init_routing {
                    let mut args = args.into_iter();
                    let arg0 = args.next().expect("previous/init arity checked");
                    // Only subscripted args benefit from the substitution (it
                    // makes their indices statically resolvable); other shapes
                    // keep their original form so behavior is unchanged for
                    // them (`hoist_capture` substitutes internally, and the
                    // substitution is idempotent). An ARRAY-shaped subscript is
                    // the exception: substituting would pin it to one element
                    // before lowering can tell whether the position wants the
                    // element or the whole array (`arg_is_array_shaped`).
                    let arg0 = match arg0 {
                        Subscript(_, _, _)
                            if self.active_subscript.is_some()
                                && !self.arg_is_array_shaped(&arg0) =>
                        {
                            self.substitute_dimension_refs(arg0)
                        }
                        other => other,
                    };
                    // An index that leaves a dimension standing needs no
                    // helper either: it resolves statically too, just to a VIEW
                    // over the argument's storage rather than to a single slot
                    // (codegen's `snapshot_static_view`).
                    //
                    // `snapshot_arg` reduces the argument to the form
                    // `SnapshotArg::access` decides over -- the same rule
                    // codegen's `static_slot`/`snapshot_static_view` apply to
                    // the LOWERED argument, stated once so the two cannot
                    // drift.
                    let needs_temp_arg =
                        self.snapshot_arg(&arg0).access() == SnapshotAccess::Capture;
                    let arg0 = if needs_temp_arg {
                        // `hoist_capture` returns the reference expression for
                        // the synthesized capture: a bare `Var` for a scalar
                        // capture, or a subscripted `capture[<element>]` access
                        // for the arrayed capture it synthesizes when the arg
                        // carries a bare arrayed reference (GH #541).
                        let kind = if is_prev_routing {
                            CaptureKind::Previous
                        } else {
                            CaptureKind::Init
                        };
                        self.hoist_capture(kind, arg0)?
                    } else {
                        arg0
                    };
                    let new_args = if is_prev_routing {
                        let fallback = args.next().expect("previous arity checked");
                        vec![arg0, fallback]
                    } else {
                        vec![arg0]
                    };
                    return Ok(App(UntypedBuiltinFn(func, new_args), loc));
                }
                if is_builtin_fn(&func) {
                    // Builtins that survive routing stay as builtins (e.g.
                    // PREVIOUS(var, init) and INIT(var)) and compile to opcodes.
                    return Ok(App(UntypedBuiltinFn(func, args), loc));
                }

                // `stdlib_descriptor` is the authoritative per-name lookup:
                // it both rejects unknown names (UnknownBuiltin still fires
                // for a name that is neither a macro -- handled above -- nor
                // an `is_builtin_fn` builtin, nor a stdlib module, satisfying
                // macros.AC5.6) and supplies the descriptor that drives the
                // shared module rewrite. Folding the two into one lookup also
                // avoids a panic path for MODEL_NAMES entries without a
                // stdlib spec (e.g. `systems_*`) if a user equation ever
                // references them.
                let Some(descriptor) = stdlib_descriptor(&func) else {
                    return eqn_err!(
                        UnknownBuiltin,
                        loc.start,
                        loc.end,
                        format!("'{func}' is not a known function")
                    );
                };
                return self.expand_module_function(&descriptor, &func, args, loc);
            }
            Subscript(id, args, loc) => {
                let args: Result<Vec<IndexExpr0>, EquationError> =
                    args.into_iter().map(|e| self.walk_index(e)).collect();
                let args = args?;
                Subscript(id, args, loc)
            }
            Op1(op, mut r, loc) => {
                *r = self.walk(mem::take(&mut *r))?;
                Op1(op, r, loc)
            }
            Op2(op, mut l, mut r, loc) => {
                *l = self.walk(mem::take(&mut *l))?;
                *r = self.walk(mem::take(&mut *r))?;
                Op2(op, l, r, loc)
            }
            If(mut cond, mut t, mut f, loc) => {
                *cond = self.walk(mem::take(&mut *cond))?;
                *t = self.walk(mem::take(&mut *t))?;
                *f = self.walk(mem::take(&mut *f))?;
                If(cond, t, f, loc)
            }
        };

        Ok(result)
    }
}

/// A macro call's arity is strict: `args` must equal the macro's declared
/// parameters, else `BadBuiltinArgs` over the whole call so the diagnostic
/// identifies the macro in context (macros.AC5.1). Checked for every call the
/// registry claims, including a passthrough that goes on to lower as the
/// builtin, because the macro is what the model declared.
fn macro_arity(
    descriptor: &ModuleFunctionDescriptor,
    func: &str,
    args: usize,
    loc: crate::builtins::Loc,
) -> Result<(), EquationError> {
    let declared = descriptor.parameter_ports.len();
    if args == declared {
        return Ok(());
    }
    eqn_err!(
        BadBuiltinArgs,
        loc.start,
        loc.end,
        format!("macro {func} takes exactly {declared} argument(s), but {args} were given")
    )
}

/// Expand module-function calls -- stdlib (SMTH1, DELAY, ...) *and* project
/// macros -- plus PREVIOUS/INIT builtins into implicit module instances and
/// opcode-backed builtins.
///
/// `macro_registry` carries the per-project macros: a call name resolving
/// there expands as a macro (shadowing an identically named builtin/stdlib
/// func) and an *arrayed* macro invocation rides the per-element path via
/// `contains_module_call`.
///
/// `enclosing_model` is the owning model's name when `variable_name` is a
/// macro-marked model's body variable (`None` otherwise): the registry needs
/// it to tell a macro body's renamed-builtin self-call (GH #554) from
/// recursion.
///
/// `snapshot_index` is what the walk may ask the owning model about an
/// identifier index of a `PREVIOUS`/`INIT` subscript -- the one model-level
/// question the parse answers itself, because a capture cannot be un-minted at
/// lowering (see [`SnapshotIndexFacts`] for the three rules).
pub fn instantiate_implicit_modules(
    variable_name: &str,
    ast: Ast<Expr0>,
    dimensions_ctx: Option<&DimensionsContext>,
    snapshot_index: SnapshotIndexFacts<'_>,
    macro_registry: &MacroRegistry,
    enclosing_model: Option<&str>,
) -> std::result::Result<(Ast<Expr0>, Vec<ImplicitVar>), EquationError> {
    let visitor = || {
        BuiltinVisitor::new(variable_name)
            .with_dimensions_ctx(dimensions_ctx)
            .with_snapshot_index(snapshot_index)
            .with_macro_registry(macro_registry)
            .with_enclosing_model(enclosing_model)
    };
    // The helpers of one variable, across every walk this expansion runs:
    // `insert_implicit_var` is what lets the per-element walks of one cloned
    // body collapse their identical GH #541 captures into one, and what refuses
    // two walks minting different helpers under one name.
    let mut all_vars: IndexMap<Ident<Canonical>, ImplicitVar> = IndexMap::new();
    let mut collect = |visitor: BuiltinVisitor| -> Result<(), EquationError> {
        for var in visitor.vars.into_values() {
            insert_implicit_var(&mut all_vars, var)?;
        }
        Ok(())
    };

    let ast = match ast {
        Ast::Scalar(ast) => {
            let mut walker = visitor();
            let transformed = walker.walk(ast)?;
            collect(walker)?;
            Ast::Scalar(transformed)
        }
        Ast::ApplyToAll(dimensions, ast) => {
            // A body with a module-function call (stdlib or macro) or a
            // PREVIOUS/INIT is expanded once per element, into an arrayed
            // equation of per-element instances.
            if contains_module_call(&ast, macro_registry) && !dimensions.is_empty() {
                let mut elements = HashMap::new();
                for subscript in SubscriptIterator::new(&dimensions) {
                    let subscript_key = CanonicalElementName::from_raw(&subscript.join(","));
                    let mut walker = visitor().with_active_element(&dimensions, &subscript);
                    let transformed = walker.walk(ast.clone())?;
                    collect(walker)?;
                    elements.insert(subscript_key, transformed);
                }
                Ast::Arrayed(dimensions, elements, None, false)
            } else {
                let mut walker = visitor();
                let transformed = walker.walk(ast)?;
                collect(walker)?;
                Ast::ApplyToAll(dimensions, transformed)
            }
        }
        Ast::Arrayed(dimensions, elements, default_expr, apply_default_to_missing) => {
            let any_module_call = elements
                .values()
                .any(|e| contains_module_call(e, macro_registry))
                || default_expr
                    .as_ref()
                    .is_some_and(|e| contains_module_call(e, macro_registry));
            let mut new_elements = HashMap::new();
            let transformed_default;
            if any_module_call && !dimensions.is_empty() {
                for (subscript_key, equation) in elements_in_stable_order(elements) {
                    let subscript_parts: Vec<String> = subscript_key
                        .as_str()
                        .split(',')
                        .map(|s| s.to_string())
                        .collect();
                    let mut walker = visitor()
                        .with_active_element(&dimensions, &subscript_parts)
                        // Per-element slots have distinct equations, so any
                        // arrayed PREVIOUS/INIT helper must carry the element
                        // suffix to avoid colliding across slots (PR #668).
                        .with_per_element_equation(true);
                    let transformed = walker.walk(equation)?;
                    collect(walker)?;
                    new_elements.insert(subscript_key, transformed);
                }
                transformed_default = match default_expr {
                    Some(default_expr) => {
                        let mut walker = visitor();
                        let transformed = walker.walk(default_expr)?;
                        collect(walker)?;
                        Some(transformed)
                    }
                    None => None,
                };
            } else {
                // One visitor across every slot, so the `n` counter that names
                // each synthesized helper is handed out in this iteration
                // order -- see `elements_in_stable_order`.
                let mut walker = visitor();
                for (subscript_key, equation) in elements_in_stable_order(elements) {
                    new_elements.insert(subscript_key, walker.walk(equation)?);
                }
                transformed_default = match default_expr {
                    Some(default_expr) => Some(walker.walk(default_expr)?),
                    None => None,
                };
                collect(walker)?;
            }
            Ast::Arrayed(
                dimensions,
                new_elements,
                transformed_default,
                apply_default_to_missing,
            )
        }
    };
    Ok((ast, all_vars.into_values().collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::Loc;
    use crate::datamodel;
    use crate::test_common::TestProject;

    #[test]
    fn test_substitute_dimension_refs_uses_secondary_mapping_target() {
        let dim_a = datamodel::Dimension::named(
            "dima".to_string(),
            vec!["a1".to_string(), "a2".to_string()],
        );
        let dim_x = datamodel::Dimension::named(
            "dimx".to_string(),
            vec!["x1".to_string(), "x2".to_string()],
        );
        let mut dim_b = datamodel::Dimension::named(
            "dimb".to_string(),
            vec!["b1".to_string(), "b2".to_string()],
        );
        dim_b.mappings = vec![
            datamodel::DimensionMapping {
                target: "dimx".to_string(),
                element_map: vec![],
            },
            datamodel::DimensionMapping {
                target: "dima".to_string(),
                element_map: vec![],
            },
        ];

        let dims_ctx = DimensionsContext::from(&[dim_a.clone(), dim_x, dim_b.clone()]);
        let active_dims = vec![Dimension::from(&dim_a)];
        let active_subscript = vec!["a1".to_string()];
        let visitor = BuiltinVisitor::new("test_var")
            .with_dimensions_ctx(Some(&dims_ctx))
            .with_active_element(&active_dims, &active_subscript);

        let expr = Expr0::Var(RawIdent::new_from_str("dimb"), Loc::default());
        let rewritten = visitor.substitute_dimension_refs(expr);
        match rewritten {
            Expr0::Var(id, _) => {
                assert_eq!(id.as_str(), "dimb·b1");
            }
            other => panic!("expected Var, got {other:?}"),
        }
    }

    /// Every operator is usable inside a module-function argument (GH #913).
    ///
    /// A hoisted argument rides as its `Expr0` SUBTREE, so the printer and the
    /// lexer do not have to agree on a spelling for a model to compile. This
    /// test is the end-to-end statement of that: `<>`, `not` and a chained `^`
    /// are exactly the spellings a printer and a lexer are most likely to
    /// disagree on (`<>` as `!=`, `not` as `!`, neither of which the lexer
    /// accepts), so a reintroduced print-and-reparse of an argument turns these
    /// legal models into a hard compile failure and reds this test.
    #[test]
    fn every_operator_is_usable_inside_a_module_function_argument() {
        let project = TestProject::new("printer_reparse")
            .aux("a", "1", None)
            .aux("b", "2", None)
            .aux("neq", "SMTH1(IF (a <> b) THEN 1 ELSE 0, 3)", None)
            .aux("negated", "SMTH1(IF (not (a > b)) THEN 1 ELSE 0, 3)", None)
            .aux("exponent", "SMTH1(a ^ b ^ 2, 3)", None);

        project.assert_compiles_incremental();
    }

    /// The same guard as its sibling, for the shape that fails **SILENTLY** --
    /// and therefore the more important of the two.
    ///
    /// `If` is not an atom in the equation grammar: it is legal only at the top
    /// of an expression, inside parentheses, or as a call argument. An argument
    /// AST of `Div(If(1>0, 10, 20), 2)` printed bare under the operator reads
    ///
    /// ```text
    /// if (1 > 0) then (10) else (20) / 2
    /// ```
    ///
    /// which parses as `If(1>0, 10, 20/2)` -- the division migrated INTO the
    /// else branch. Unlike `<>` / `not` / chained `^`, which produce text the
    /// lexer rejects and so fail loudly, this text parses fine. It just means
    /// something different: a plain user model, no arrays and no LTM, that
    /// compiles clean, runs clean, and reports `10` instead of `5`, which
    /// nothing else in the engine would catch. That is why the argument is
    /// carried as a subtree and never as text (GH #913).
    #[test]
    fn module_backed_builtin_if_argument_is_not_regrouped() {
        // SMTH1's input is a constant 5, so the smooth sits at its initial value
        // (= the input) for the whole run. A regrouped `if` yields 10.
        let project = TestProject::new("printer_reparse_if").aux(
            "x",
            "SMTH1((IF (1 > 0) THEN 10 ELSE 20) / 2, 1)",
            None,
        );

        let x = project.vm_result("x");
        assert!(
            x.iter().all(|v| (*v - 5.0).abs() < 1e-12),
            "`SMTH1((IF (1 > 0) THEN 10 ELSE 20) / 2, 1)` must be 5: the `/ 2` \
             applies to the whole `if`, not just its else branch. Got {:?}",
            &x[..x.len().min(4)]
        );
    }

    /// Test that arrayed DELAY1 compiles and simulates
    /// d[SubA] = DELAY1(input[SubA], delay_time, init)
    #[test]
    fn test_arrayed_delay1_basic() {
        let project = TestProject::new("arrayed_delay")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .named_dimension("SubA", &["A2", "A3"])
            .array_const("input[SubA]", 10.0)
            .aux("delay_time", "1", None)
            .aux("init", "0", None)
            .array_aux("d[SubA]", "DELAY1(input[SubA], delay_time, init)");

        project.assert_compiles_incremental();
    }

    /// Test arrayed DELAY1 with mixed scalar and arrayed arguments
    /// d[DimA] = DELAY1(input_a[DimA], delay, init_scalar)
    #[test]
    fn test_arrayed_delay1_mixed_args() {
        let project = TestProject::new("arrayed_delay_mixed")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .array_const("input_a[DimA]", 10.0)
            .aux("delay", "5", None)
            .aux("init_scalar", "0", None)
            .array_aux("d[DimA]", "DELAY1(input_a[DimA], delay, init_scalar)");

        project.assert_compiles_incremental();
    }

    /// Test that arrayed DELAY1 produces correct numerical output
    /// With input=10, delay_time=5, init=0:
    /// - At t=0: stock=0, output=0
    /// - At t=1: stock=10, output=10/5=2
    #[test]
    fn test_arrayed_delay1_numerical_values() {
        // Using dt=1, which gives us time steps at 0, 1, 2, ...
        // DELAY1 with input=10, delay=5, init=0:
        // stock(0) = 0 (init * delay)
        // output(0) = 0 (stock/delay)
        // stock(1) = 0 + 1*(10 - 0) = 10
        // output(1) = 10/5 = 2
        let project = TestProject::new("delay_numerical")
            .named_dimension("DimA", &["A1", "A2"])
            .array_const("input_a[DimA]", 10.0)
            .aux("delay", "5", None)
            .aux("init", "0", None)
            .array_aux("d[DimA]", "DELAY1(input_a[DimA], delay, init)");

        project.assert_compiles_incremental();

        // Get results for 2 timesteps (0 and 1)
        // Each element should have independent delay state
        // At step 1, output should be input/delay = 10/5 = 2
        project.assert_vm_result("d", &[2.0, 2.0]);
    }

    /// Test arrayed DELAY1 with all arrayed arguments
    /// d[DimA] = DELAY1(input_a[DimA], delay_a[DimA], init_a[DimA])
    #[test]
    fn test_arrayed_delay1_all_arrayed() {
        let project = TestProject::new("arrayed_delay_all")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .array_const("input_a[DimA]", 10.0)
            // delay_a needs time units matching simulation time (Month)
            .array_const_with_units("delay_a[DimA]", 1.0, "Month")
            .array_const("init_a[DimA]", 0.0)
            .array_aux(
                "d[DimA]",
                "DELAY1(input_a[DimA], delay_a[DimA], init_a[DimA])",
            );

        project.assert_compiles_incremental();
    }

    /// Test arrayed DELAY1 with per-element different values (like d5 model)
    /// Verifies that each element gets its own module with correct inputs
    #[test]
    fn test_arrayed_delay1_different_element_values() {
        // Mirrors d5 in the delay model:
        // input_a[A1]=10, input_a[A2]=20
        // delay_a[A1]=2, delay_a[A2]=2
        // For DELAY1 with init=0:
        // At step 1: output = stock/delay = input/delay = 10/2=5, 20/2=10
        let project = TestProject::new("arrayed_delay_diff_values")
            .named_dimension("DimA", &["A1", "A2"])
            .array_with_ranges("input_a[DimA]", vec![("A1", "10"), ("A2", "20")])
            // delay_a needs time units matching simulation time (Month)
            .array_const_with_units("delay_a[DimA]", 2.0, "Month")
            .array_const("init_a[DimA]", 0.0)
            .array_aux(
                "d[DimA]",
                "DELAY1(input_a[DimA], delay_a[DimA], init_a[DimA])",
            );

        project.assert_compiles_incremental();

        // At step 1: output = stock/delay
        // For A1: input=10, delay=2, init=0 -> stock(1)=10, output(1)=10/2=5
        // For A2: input=20, delay=2, init=0 -> stock(1)=20, output(1)=20/2=10
        project.assert_vm_result("d", &[5.0, 10.0]);
    }

    /// Test arrayed DELAY3 with arrayed delay time
    /// d[DimA] = DELAY3(input, delay_a[DimA])
    #[test]
    fn test_arrayed_delay3() {
        let project = TestProject::new("arrayed_delay3")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .aux("input", "10", None)
            .array_const("delay_a[DimA]", 1.0)
            .array_aux("d[DimA]", "DELAY3(input, delay_a[DimA])");

        project.assert_compiles_incremental();
    }

    /// Test that DELAYN with order=1 is rewritten to DELAY1 and works in A2A.
    #[test]
    fn test_arrayed_delayn_order1() {
        let project = TestProject::new("arrayed_delayn1")
            .named_dimension("DimA", &["A1", "A2"])
            .array_const("input_a[DimA]", 10.0)
            .aux("delay_time", "1", None)
            .aux("init", "0", None)
            .array_aux("d[DimA]", "DELAYN(input_a[DimA], delay_time, 1, init)");

        project.assert_compiles_incremental();
    }

    /// Test that DELAYN with order=3 is rewritten to DELAY3.
    #[test]
    fn test_arrayed_delayn_order3() {
        let project = TestProject::new("arrayed_delayn3")
            .named_dimension("DimA", &["A1", "A2"])
            .array_const("input_a[DimA]", 10.0)
            .aux("delay_time", "1", None)
            .aux("init", "0", None)
            .array_aux("d[DimA]", "DELAYN(input_a[DimA], delay_time, 3, init)");

        project.assert_compiles_incremental();
    }

    /// Test arrayed SMOOTH1/SMTH1
    #[test]
    fn test_arrayed_smooth1() {
        let project = TestProject::new("arrayed_smooth1")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .array_const("input_a[DimA]", 10.0)
            .aux("smooth_time", "1", None)
            .array_aux("s[DimA]", "SMTH1(input_a[DimA], smooth_time)");

        project.assert_compiles_incremental();
    }

    /// Test that SMTHN with order=1 is rewritten to SMTH1.
    #[test]
    fn test_arrayed_smthn_order1() {
        let project = TestProject::new("arrayed_smthn1")
            .named_dimension("DimA", &["A1", "A2"])
            .array_const("input_a[DimA]", 10.0)
            .aux("smooth_time", "1", None)
            .aux("init", "0", None)
            .array_aux("s[DimA]", "SMTHN(input_a[DimA], smooth_time, 1, init)");

        project.assert_compiles_incremental();
    }

    /// Test that unsupported DELAYN order is rejected.
    #[test]
    fn test_arrayed_delayn_unsupported_order() {
        let project = TestProject::new("arrayed_delayn_bad_order")
            .named_dimension("DimA", &["A1", "A2"])
            .array_const("input_a[DimA]", 10.0)
            .aux("delay_time", "1", None)
            .aux("init", "0", None)
            .array_aux("d[DimA]", "DELAYN(input_a[DimA], delay_time, 2, init)");

        project.assert_compile_error_vm(crate::ErrorCode::UnknownBuiltin);
    }

    /// Test with indexed dimensions (numeric 1,2,3...)
    #[test]
    fn test_arrayed_delay1_indexed_dimension() {
        let project = TestProject::new("arrayed_delay_indexed")
            .indexed_dimension("Idx", 3)
            .array_const("input[Idx]", 10.0)
            .aux("delay_time", "1", None)
            .aux("init", "0", None)
            .array_aux("d[Idx]", "DELAY1(input[Idx], delay_time, init)");

        project.assert_compiles_incremental();
    }

    /// Test DELAY in expression context (k * DELAY3(...))
    #[test]
    fn test_arrayed_delay_in_expression() {
        let project = TestProject::new("arrayed_delay_expr")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .aux("k", "42", None)
            .aux("input", "10", None)
            .array_const("delay_a[DimA]", 1.0)
            .array_aux("d[DimA]", "k * DELAY3(input, delay_a[DimA])");

        project.assert_compiles_incremental();
    }

    /// Test that per-element (Arrayed) equations with stdlib calls get unique module names.
    /// When each element has its own equation containing DELAY1, each element
    /// must produce a uniquely-named module to avoid collisions.
    #[test]
    fn test_arrayed_per_element_delay1() {
        let project = TestProject::new("arrayed_per_element_delay")
            .named_dimension("DimA", &["A1", "A2"])
            .aux("input1", "10", None)
            .aux("input2", "20", None)
            .aux("delay_time", "5", None)
            .aux("init", "0", None)
            .array_with_ranges(
                "d[DimA]",
                vec![
                    ("A1", "DELAY1(input1, delay_time, init)"),
                    ("A2", "DELAY1(input2, delay_time, init)"),
                ],
            );

        project.assert_compiles_incremental();
    }

    /// Test per-element Arrayed equations mixing stdlib and non-stdlib expressions
    #[test]
    fn test_arrayed_per_element_mixed_stdlib() {
        let project = TestProject::new("arrayed_per_element_mixed")
            .named_dimension("DimA", &["A1", "A2"])
            .aux("input1", "10", None)
            .aux("delay_time", "5", None)
            .aux("init", "0", None)
            .array_with_ranges(
                "d[DimA]",
                vec![("A1", "DELAY1(input1, delay_time, init)"), ("A2", "42")],
            );

        project.assert_compiles_incremental();
    }

    /// Test that per-element (Arrayed) equations with stdlib calls using
    /// subscripted inputs produce correctly-suffixed module names.
    /// This verifies dimension reference substitution works in the Arrayed path.
    #[test]
    fn test_arrayed_per_element_delay1_with_subscripted_inputs() {
        let project = TestProject::new("arrayed_per_element_subscripted")
            .named_dimension("DimA", &["A1", "A2"])
            .array_with_ranges("input_a[DimA]", vec![("A1", "10"), ("A2", "20")])
            .aux("delay_time", "5", None)
            .aux("init", "0", None)
            .array_with_ranges(
                "d[DimA]",
                vec![
                    ("A1", "DELAY1(input_a[A1], delay_time, init)"),
                    ("A2", "DELAY1(input_a[A2], delay_time, init)"),
                ],
            );

        project.assert_compiles_incremental();
    }

    /// Test that NPV stdlib model compiles and produces accumulation.
    /// NPV output at time t includes the current step's discounted stream
    /// (unlike a normal stock which reflects the state before the current step).
    #[test]
    fn test_npv_basic() {
        // NPV with constant stream=10, discount_rate=0, init=0, factor=1
        // With zero discount rate, NPV just accumulates stream*factor each step
        let project = TestProject::new("npv_test")
            .with_sim_time(0.0, 2.0, 1.0)
            .aux("stream", "10", None)
            .aux("discount_rate", "0", None)
            .aux("init_val", "0", None)
            .aux("factor", "1", None)
            .aux(
                "result",
                "NPV(stream, discount_rate, init_val, factor)",
                None,
            );

        project.assert_compiles_incremental();
        // output = stock + inflow * DT
        // t=0: stock=0, inflow=10*1*(1+0)^0=10, output = 0 + 10*1 = 10
        // t=1: stock=10, inflow=10, output = 10 + 10 = 20
        // t=2: stock=20, inflow=10, output = 20 + 10 = 30
        project.assert_vm_result("result", &[10.0, 20.0, 30.0]);
    }

    /// Test NPV with non-zero discount rate
    #[test]
    fn test_npv_with_discount() {
        // NPV with stream=100, discount_rate=0.1, init=0, factor=1
        // discount_factor(t) = (1 + 0.1 * 1)^(-t/1) = 1.1^(-t)
        let project = TestProject::new("npv_discount_test")
            .with_sim_time(0.0, 2.0, 1.0)
            .aux("stream", "100", None)
            .aux("discount_rate", "0.1", None)
            .aux("init_val", "0", None)
            .aux("factor", "1", None)
            .aux(
                "result",
                "NPV(stream, discount_rate, init_val, factor)",
                None,
            );

        project.assert_compiles_incremental();
        let results = project.run_vm().unwrap();
        let vals = results.get("result").unwrap();
        // output = stock + inflow * DT
        // t=0: stock=0, inflow=100*1.1^0=100, output = 0 + 100 = 100
        // t=1: stock=100, inflow=100*1.1^(-1)=90.909, output = 100 + 90.909 = 190.909
        // t=2: stock=190.909, inflow=100*1.1^(-2)=82.645, output = 190.909 + 82.645 = 273.554
        assert!((vals[0] - 100.0).abs() < 1e-6);
        assert!((vals[1] - 190.909).abs() < 0.01);
        assert!((vals[2] - 273.554).abs() < 0.01);
    }

    /// Test that MODULO function call is converted to MOD binary op
    #[test]
    fn test_modulo_function() {
        let project = TestProject::new("modulo_test")
            .aux("a", "7", None)
            .aux("b", "3", None)
            .aux("result", "MODULO(a, b)", None);

        project.assert_compiles_incremental();
        project.assert_vm_result("result", &[1.0, 1.0]);
    }

    /// Regression test: nested INIT must not repeatedly wrap generated arg helpers.
    #[test]
    fn test_nested_init_does_not_rewrite_generated_arg_helpers() {
        let project = TestProject::new("nested_init_regression")
            .aux("x", "1", None)
            .aux("result", "INIT(INIT(x + 1))", None);

        project.assert_compiles_incremental();
        project.assert_vm_result("result", &[2.0, 2.0]);
    }

    /// Test that DELAY (from DELAY FIXED mapping) works as delay1
    #[test]
    fn test_delay_alias() {
        let project = TestProject::new("delay_alias_test")
            .aux("input", "10", None)
            .aux("delay_time", "1", None)
            .aux("init", "0", None)
            .aux("result", "DELAY(input, delay_time, init)", None);

        project.assert_compiles_incremental();
    }
}
