// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use indexmap::IndexMap;

use crate::ast::{Ast, BinaryOp, Expr0, IndexExpr0, Literal};
use crate::builtins::{UntypedBuiltinFn, is_builtin_fn};
use crate::capture::{
    Capture, CaptureKind, CaptureShape, HoistedArg, ImplicitModule, ImplicitVar, element_suffix,
    insert_implicit_var,
};
use crate::common::{
    Canonical, CanonicalDimensionName, CanonicalElementName, EquationError, Ident, RawIdent,
    canonicalize,
};
use crate::dimensions::{
    Axis, AxisIndexName, Dimension, DimensionsContext, DirectMappingsOnly, SubscriptIterator,
    axes_of, match_axes_partial, resolve_axis_index_name,
};
use crate::eqn_err;
use crate::module_functions::{
    MacroCallResolution, MacroRegistry, ModuleFunctionDescriptor, stdlib_descriptor,
};
use crate::snapshot_arg::{SnapshotAccess, SnapshotArg, SnapshotIndex};
use crate::variable::ElementScope;

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

/// What an apply-to-all body asks of the expansion, from the calls it
/// contains. Ordered, so a body's requirement is the maximum over its calls.
// `Debug` is unconditional, as the crate keeps it for every type an
// `assert_eq!` compares (the routing-arm table in `macro_expansion_tests`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum PerElement {
    /// Nothing that synthesizes a helper.
    None,
    /// Only `PREVIOUS`/`INIT`: the body stays one structural apply-to-all
    /// equation, and a capture it needs is one structural helper over the
    /// declared axes ([`crate::capture::CaptureShape::ApplyToAll`]), resolved
    /// per element by ordinary lowering.
    SnapshotOnly,
    /// A stdlib or macro module call: an instance carries state and wiring of
    /// its own, so the body is expanded into one equation per element, each
    /// with its own instance, and every helper minted along the way is one
    /// element's own ([`crate::variable::ElementScope`]).
    ModuleInstance,
}

/// Classify every call in `expr` once, before any visitor is chosen: the
/// expansion's shape is decided per body, and a walk cannot change shape
/// halfway through.
///
/// The stdlib predicate is [`crate::builtins::is_stdlib_module_function`],
/// which is defined over the same descriptor table the expansion reads
/// (`stdlib_descriptor`), so the two cannot drift. A macro call that the
/// registry routes to the builtin rather than expanding -- a passthrough at an
/// external call site, the enclosing macro's own renamed builtin (GH #554) --
/// is classified as that builtin.
pub(crate) fn per_element_requirements(
    expr: &Expr0,
    macro_registry: &MacroRegistry,
    enclosing_model: Option<&str>,
) -> PerElement {
    use Expr0::*;
    let of = |e: &Expr0| per_element_requirements(e, macro_registry, enclosing_model);
    match expr {
        Const(_, _, _) | Var(_, _) => PerElement::None,
        App(UntypedBuiltinFn(func, args), _) => {
            let own = match macro_registry.resolve_call(func, enclosing_model) {
                MacroCallResolution::Expand(_) => PerElement::ModuleInstance,
                MacroCallResolution::Passthrough(_)
                | MacroCallResolution::RenamedBuiltinSelfCall
                | MacroCallResolution::Unresolved => {
                    if crate::builtins::is_stdlib_module_function(func.as_str()) {
                        PerElement::ModuleInstance
                    } else if matches!(func.as_str(), "init" | "previous") {
                        PerElement::SnapshotOnly
                    } else {
                        PerElement::None
                    }
                }
            };
            args.iter().map(of).fold(own, PerElement::max)
        }
        Subscript(_, args, _) => args
            .iter()
            .map(|idx| match idx {
                IndexExpr0::Expr(e) => of(e),
                IndexExpr0::Range(start, end, _) => of(start).max(of(end)),
                IndexExpr0::Wildcard(_)
                | IndexExpr0::StarRange(_, _)
                | IndexExpr0::DimPosition(_, _) => PerElement::None,
            })
            .max()
            .unwrap_or(PerElement::None),
        Op1(_, r, _) => of(r),
        Op2(_, l, r, _) => of(l).max(of(r)),
        If(cond, t, f, _) => of(cond).max(of(t)).max(of(f)),
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

/// Normalize the stdlib aliases to the model each call instantiates:
/// `DELAY` to `DELAY1`, and `DELAYN`/`SMTHN` with a literal order 1 or 3 to
/// `DELAY1`/`DELAY3`/`SMTH1`/`SMTH3` with the order argument consumed.
///
/// An omitted initial value stays omitted: the call wires only `[input,
/// delay_time]`, and the canonical model's `isModuleInput(initial_value)`
/// guard falls back to the input, which is what XMILE 1.0 section 3.5.3
/// (Delay Functions, `docs/reference/xmile-v1.0.html#_Toc439926074`)
/// specifies for `DELAYN` and `SMTHN`: "If initial value is not provided, the
/// initial value of input will be used". An explicit fourth argument is an
/// independent port, as for every other stdlib call.
///
/// `DELAY(x, t)` is XMILE 1.0 section 3.5.3's infinite-order material delay
/// -- a pipeline delay, Vensim's `DELAY FIXED` as xmutil maps it -- which the
/// stdlib framework cannot represent (it has no ring-buffer state), so it is
/// a first-order delay here: known-incorrect where the exact delay matters
/// (`delay_time >> DT`).
///
/// Only the first- and third-order forms have a stdlib model; every other
/// literal order is refused loudly.
fn rewrite_alias_module_call(
    func: String,
    args: Vec<Expr0>,
    loc: crate::builtins::Loc,
) -> Result<(String, Vec<Expr0>), EquationError> {
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

    let mut args = vec![input, delay_time];
    args.extend(init);
    Ok((rewritten_name.to_string(), args))
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
/// The per-element path unions each slot's synthesized helpers into one vector,
/// whose ORDER rides two salsa-cached values with derived `PartialEq`
/// (`ParsedVariableResult::implicit_vars` and `VariableDeps::implicit_vars`),
/// so an unstable order defeats backdating and makes the compiled artifact
/// irreproducible. That path is the reachable one, and
/// `db::fragment_determinism_tests::per_element_implicit_helper_order_is_stable_across_fresh_databases`
/// gates it.
///
/// The other path walks every slot with ONE visitor, so the `n` counter that
/// names each synthesized helper (`$⁚v⁚{n}⁚arg0`) is handed out in iteration
/// order. It is taken when `dimensions.is_empty()`, which is LIVE and not a
/// degenerate path: an `<aux>` carrying `<element subscript=…>` children with
/// no `<dimensions>` sibling is an ordinary XMILE document, `xmile::variables`'
/// `convert_equation!` maps the missing element to an empty dimension list,
/// and the model compiles. Reverting the ordering here leaves the helper NAMES
/// identical -- the counter is monotonic, so it emits `⁚0⁚`, `⁚1⁚`, `⁚2⁚`
/// however the slots are walked -- and re-binds which SLOT'S EQUATION each name
/// carries, which then moves the compiled artifact. Gated by
/// `db::fragment_determinism_tests::undimensioned_arrayed_helper_bindings_are_stable_across_fresh_databases`,
/// whose fixture is read through the real XMILE reader rather than
/// hand-built. (A failed dimension LOOKUP is not this case: it returns
/// `Err(BadDimensionName)` and yields no AST at all.)
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
    /// The parent's declared axes, when the body being walked is an
    /// apply-to-all body or one slot of one: what a subscript naming one of
    /// them spans, what a structural capture applies over, and the axes a
    /// per-element helper's scope names. Empty for a scalar equation.
    dimensions: Vec<Dimension>,
    /// `dimensions` by canonical name.
    dimension_names: Vec<CanonicalDimensionName>,
    /// The active element on each of `dimensions` when the body is walked for
    /// ONE element -- a per-element expansion, or an explicit `Ast::Arrayed`
    /// slot -- and every helper minted is that element's own. `None` for a
    /// scalar equation and for a structural apply-to-all walk.
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
        }
    }

    /// Walk an apply-to-all body structurally: `dimensions` are the parent's
    /// declared axes, and no element is active.
    fn with_declared_dimensions(mut self, dimensions: &[Dimension]) -> Self {
        self.dimension_names = get_dimension_names(dimensions);
        self.dimensions = dimensions.to_vec();
        self
    }

    /// Walk one element of an apply-to-all or arrayed parent: `dimensions` are
    /// the parent's declared axes and `element` the active element on each,
    /// which every helper minted is scoped to and named for.
    fn with_active_element(mut self, dimensions: &[Dimension], element: &[String]) -> Self {
        self.active_subscript = Some(element.to_vec());
        self.with_declared_dimensions(dimensions)
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

    /// Set the model-local facts `index_is_static` decides an identifier
    /// index with (see [`SnapshotIndexFacts`]).
    fn with_snapshot_index(mut self, snapshot_index: SnapshotIndexFacts<'a>) -> Self {
        self.snapshot_index = snapshot_index;
        self
    }

    /// Set the dimensions context so PREVIOUS/INIT can recognize statically
    /// resolvable subscript indices (qualified `dimension·element` references)
    /// and a bare dimension-name argument is known to be one.
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
    /// True for a wildcard or star-range, and for a dimension name the parent's
    /// declared axes answer for -- one of them by name, or a dimension one of
    /// them relates to through a declared mapping or subdimension. That is the
    /// spelling `context.rs` resolves per element in scalar position and
    /// promotes to the whole array inside a vector builtin's array-operand
    /// position (`with_vector_builtin_wildcards`), and the question is put to
    /// the same matcher under the same projection the compiler's
    /// `Subscript3Config::active_dim_ref` uses to resolve it
    /// (`match_axes_partial` under `DirectMappingsOnly`), so a snapshot reads
    /// its slot directly exactly where lowering resolves one. A name no
    /// declared axis answers for is a runtime index: a variable, or a
    /// dimension the element cannot resolve.
    fn index_spans_a_dimension(&self, idx: &IndexExpr0) -> bool {
        match idx {
            IndexExpr0::Wildcard(_) | IndexExpr0::StarRange(_, _) => true,
            IndexExpr0::Expr(Expr0::Var(ident, _)) => {
                let canonical = CanonicalDimensionName::from_raw(ident.as_str());
                if self.dimension_names.iter().any(|d| d == &canonical) {
                    return true;
                }
                self.dimensions_ctx.is_some_and(|ctx| {
                    ctx.get(&canonical).is_some()
                        && match_axes_partial(
                            &[Axis::named(canonical.as_str(), 0)],
                            &axes_of(&self.dimensions),
                            &DirectMappingsOnly(ctx),
                        )
                        .into_iter()
                        .next()
                        .flatten()
                        .is_some()
                })
            }
            _ => false,
        }
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

    /// Get the subscript suffix for module names (e.g., "a2" or "a1,b2")
    fn subscript_suffix(&self) -> String {
        self.element_scope()
            .map(|scope| element_suffix(&scope))
            .unwrap_or_default()
    }

    /// The element this walk is scoped to, when it walks one element of an
    /// apply-to-all body.
    fn element_scope(&self) -> Option<ElementScope> {
        self.active_subscript.as_ref().map(|element| ElementScope {
            dims: self.dimension_names.clone(),
            element: element
                .iter()
                .map(|e| CanonicalElementName::from_raw(e))
                .collect(),
        })
    }

    /// Can a bare identifier argument of a module call be wired to the scalar
    /// input port by name, or does it mean one element's value under this
    /// walk's active element -- an arrayed variable, whose port wiring would
    /// hand a whole array to a scalar port, or a dimension name, whose value
    /// is the active element's position?
    ///
    /// Only the source parse can tell an arrayed name from a scalar one
    /// ([`SnapshotIndexFacts::Axes`], the same per-name projection the
    /// snapshot-index question uses); the other two rules wire every bare
    /// name, as a scalar equation always does.
    fn arg_needs_element_scope(&self, name: &RawIdent) -> bool {
        if self.active_subscript.is_none() {
            return false;
        }
        let canonical = canonicalize(name.as_str());
        let is_dimension = self.dimensions_ctx.is_some_and(|ctx| {
            ctx.get(&CanonicalDimensionName::from_raw(&canonical))
                .is_some()
        });
        let is_arrayed = match self.snapshot_index {
            SnapshotIndexFacts::Axes { axis_of, .. } => axis_of(name.as_str(), 0).is_some(),
            SnapshotIndexFacts::ModelNames(_) | SnapshotIndexFacts::NoModel => false,
        };
        is_dimension || is_arrayed
    }

    /// Hoist a `PREVIOUS`/`INIT` argument into a [`Capture`] and return the
    /// reference expression the caller substitutes for the argument.
    ///
    /// The capture's shape is where the call was written. In a scalar
    /// equation it is a scalar. In an apply-to-all body walked structurally it
    /// is one helper over the parent's declared axes, holding the argument
    /// untouched, and the reference is `capture[Dim, ..]` -- one index per
    /// axis naming that axis, which leaves every axis standing for lowering to
    /// pin to the active element, exactly as the parent's own `x[Dim]` reads
    /// resolve. In a body walked for one element it is that element's own,
    /// scalar, scoped to the element so lowering resolves its body there.
    fn hoist_capture(&mut self, kind: CaptureKind, arg: Expr0) -> Result<Expr0, EquationError> {
        let loc = crate::builtins::Loc::default();
        let shape = match self.element_scope() {
            Some(scope) => CaptureShape::Element(scope),
            None if self.dimension_names.is_empty() => CaptureShape::Scalar,
            None => CaptureShape::ApplyToAll(
                self.dimension_names
                    .iter()
                    .map(|d| d.as_str().to_string())
                    .collect(),
            ),
        };
        let capture = Capture::new(self.variable_name, self.n, kind, arg, shape);
        let ident = RawIdent::new_from_str(capture.ident());
        let reference = match capture.shape() {
            CaptureShape::ApplyToAll(dims) => Expr0::Subscript(
                ident,
                dims.iter()
                    .map(|dim| IndexExpr0::Expr(Expr0::Var(RawIdent::new_from_str(dim), loc)))
                    .collect(),
                loc,
            ),
            CaptureShape::Scalar | CaptureShape::Element(_) => Expr0::Var(ident, loc),
        };
        self.insert_implicit_var(ImplicitVar::Capture(capture))?;
        self.n += 1;
        Ok(reference)
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
            let src = match arg {
                // A bare identifier wires straight to the port, unless it
                // means one element's value under the active element.
                Var(name, _) if !self.arg_needs_element_scope(&name) => name.as_str().to_string(),
                arg => {
                    let hoisted =
                        HoistedArg::new(self.variable_name, self.n, i, arg, self.element_scope());
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
                // that read a fixed slot or a static view, so arg0 must
                // address storage:
                //   * a direct variable reference, or
                //   * a subscripted reference whose every index is statically
                //     resolvable -- a numeric constant, a qualified
                //     `dimension·element` reference, or a bare element of the
                //     referenced axis (see `index_is_static`) -- or leaves a
                //     declared axis of the parent standing (`x[Dim]` in an
                //     apply-to-all body), which lowering pins to the active
                //     element or keeps as a view, whichever the position wants.
                //
                // Anything else (nested PREVIOUS, PREVIOUS(expr), the output
                // of a module instance synthesized in this walk, dynamic
                // subscript indices) is rewritten through a synthesized
                // capture that holds the value each timestep -- which also
                // gives dynamic indices the correct lagged semantics (the
                // index itself is read at the *previous* step). Which STORAGE
                // a direct reference addresses -- a plain slot, a module output
                // port, a bound input port's own slot, or a bare module
                // instance that has none -- is resolved at lowering from the
                // dependency's shape.
                let is_prev_routing = func == "previous" && args.len() == 2;
                let is_init_routing = func == "init" && args.len() == 1;
                if is_prev_routing || is_init_routing {
                    let mut args = args.into_iter();
                    let arg0 = args.next().expect("previous/init arity checked");
                    // `snapshot_arg` reduces the argument to the form
                    // `SnapshotArg::access` decides over -- the same rule
                    // codegen's `static_slot`/`snapshot_static_view` apply to
                    // the LOWERED argument, stated once so the two cannot
                    // drift.
                    let needs_temp_arg =
                        self.snapshot_arg(&arg0).access() == SnapshotAccess::Capture;
                    let arg0 = if needs_temp_arg {
                        let kind = if is_prev_routing {
                            CaptureKind::Previous
                        } else {
                            CaptureKind::Init
                        };
                        self.hoist_capture(kind, arg0)?
                    } else {
                        arg0
                    };
                    let new_args: Box<[Expr0]> = if is_prev_routing {
                        let fallback = args.next().expect("previous arity checked");
                        Box::new([arg0, fallback])
                    } else {
                        Box::new([arg0])
                    };
                    return Ok(App(UntypedBuiltinFn(func, new_args), loc));
                }
                if is_builtin_fn(&func) {
                    // Builtins that survive routing stay as builtins (e.g.
                    // PREVIOUS(var, init) and INIT(var)) and compile to opcodes.
                    return Ok(App(UntypedBuiltinFn(func, args.into()), loc));
                }

                // `stdlib_descriptor` is the authoritative per-name lookup:
                // it both rejects unknown names (UnknownBuiltin still fires
                // for a name that is neither a macro -- handled above -- nor
                // an `is_builtin_fn` builtin, nor a stdlib module, satisfying
                // macros.AC5.6) and supplies the descriptor that drives the
                // shared module rewrite.
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
                let args: Result<Box<[IndexExpr0]>, EquationError> =
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
/// The shape of the expansion is decided per body by
/// [`per_element_requirements`]: a scalar equation is walked once; an
/// apply-to-all body containing a module call is expanded into an
/// `Ast::Arrayed` of per-element equations, each with its own instance and
/// its own element-scoped helpers; an apply-to-all body containing at most
/// `PREVIOUS`/`INIT` keeps its structural shape and mints structural captures
/// over its axes. An explicit `Ast::Arrayed` walks each slot for its own
/// element; its EXCEPT default is materialized into the missing slots when it
/// needs an instance per element, and otherwise transformed once, structurally,
/// and kept as the default.
///
/// `macro_registry` carries the per-project macros: a call name resolving
/// there expands as a macro (shadowing an identically named builtin/stdlib
/// func) and an *arrayed* macro invocation rides the per-element path.
///
/// `enclosing_model` is the owning model's name when `variable_name` is a
/// macro-marked model's body variable (`None` otherwise): the registry needs
/// it to tell a macro body's renamed-builtin self-call (GH #554) from
/// recursion.
///
/// `snapshot_index` is what the walk may ask the owning model about an
/// identifier index of a `PREVIOUS`/`INIT` subscript, and about a bare
/// module-call argument -- the model-level questions the parse answers itself,
/// because a capture cannot be un-minted at lowering (see
/// [`SnapshotIndexFacts`] for the three rules).
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
    let requirements =
        |expr: &Expr0| per_element_requirements(expr, macro_registry, enclosing_model);
    // The helpers of one variable, across every walk this expansion runs:
    // `insert_implicit_var` is what lets two walks minting one helper collapse
    // it, and what refuses two walks minting different helpers under one name.
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
            if requirements(&ast) == PerElement::ModuleInstance && !dimensions.is_empty() {
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
                let mut walker = visitor().with_declared_dimensions(&dimensions);
                let transformed = walker.walk(ast)?;
                collect(walker)?;
                Ast::ApplyToAll(dimensions, transformed)
            }
        }
        Ast::Arrayed(dimensions, elements, default_expr, apply_default_to_missing) => {
            if dimensions.is_empty() {
                // One visitor across every slot, so the `n` counter that names
                // each synthesized helper is handed out in this iteration
                // order -- see `elements_in_stable_order`.
                let mut walker = visitor();
                let mut new_elements = HashMap::new();
                for (subscript_key, equation) in elements_in_stable_order(elements) {
                    new_elements.insert(subscript_key, walker.walk(equation)?);
                }
                let transformed_default = match default_expr {
                    Some(default_expr) => Some(walker.walk(default_expr)?),
                    None => None,
                };
                collect(walker)?;
                return Ok((
                    Ast::Arrayed(
                        dimensions,
                        new_elements,
                        transformed_default,
                        apply_default_to_missing,
                    ),
                    all_vars.into_values().collect(),
                ));
            }
            // An explicit slot is one element's own equation, walked for that
            // element: every helper it mints carries the slot's element, so
            // distinct slots never claim one name (PR #668).
            let mut new_elements = HashMap::new();
            for (subscript_key, equation) in elements_in_stable_order(elements) {
                let subscript_parts: Vec<String> = subscript_key
                    .as_str()
                    .split(',')
                    .map(|s| s.to_string())
                    .collect();
                let mut walker = visitor().with_active_element(&dimensions, &subscript_parts);
                let transformed = walker.walk(equation)?;
                collect(walker)?;
                new_elements.insert(subscript_key, transformed);
            }
            let mut transformed_default = None;
            let mut apply_default = apply_default_to_missing;
            if let Some(default_expr) = default_expr {
                let missing: Vec<Vec<String>> = SubscriptIterator::new(&dimensions)
                    .filter(|subscript| {
                        !new_elements
                            .contains_key(&CanonicalElementName::from_raw(&subscript.join(",")))
                    })
                    .collect();
                if apply_default_to_missing
                    && requirements(&default_expr) == PerElement::ModuleInstance
                {
                    // An instance carries state and wiring of its own, so one
                    // default equation cannot serve several elements: the
                    // default is materialized into exactly the slots it
                    // applies to, each with its own instance.
                    for subscript in missing {
                        let mut walker = visitor().with_active_element(&dimensions, &subscript);
                        let transformed = walker.walk(default_expr.clone())?;
                        collect(walker)?;
                        new_elements.insert(
                            CanonicalElementName::from_raw(&subscript.join(",")),
                            transformed,
                        );
                    }
                    apply_default = false;
                } else {
                    // A snapshot-only default stays one structural equation
                    // over the declared axes, exactly as an apply-to-all body
                    // does, and the compiler selects it for the missing slots.
                    let mut walker = visitor().with_declared_dimensions(&dimensions);
                    let transformed = walker.walk(default_expr)?;
                    collect(walker)?;
                    transformed_default = Some(transformed);
                }
            }
            Ast::Arrayed(dimensions, new_elements, transformed_default, apply_default)
        }
    };
    Ok((ast, all_vars.into_values().collect()))
}

#[cfg(test)]
mod tests {
    use crate::test_common::TestProject;

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
