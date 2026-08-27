// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use indexmap::IndexMap;

use crate::ast::{Ast, BinaryOp, Expr0, IndexExpr0, Literal};
use crate::builtins::{UntypedBuiltinFn, is_builtin_fn};
use crate::capture::{Capture, CaptureKind, HoistedArg, ImplicitModule, ImplicitVar};
use crate::common::{
    Canonical, CanonicalDimensionName, CanonicalElementName, EquationError, Ident, RawIdent,
    canonicalize,
};
#[cfg(test)]
use crate::datamodel;
use crate::dimensions::{Dimension, DimensionsContext, SubscriptIterator};
use crate::eqn_err;
use crate::module_functions::{
    MacroRegistry, ModuleFunctionDescriptor, is_renamed_builtin_macro_collision, stdlib_descriptor,
};
use crate::snapshot_arg::{SnapshotAccess, SnapshotArg, SnapshotIndex};

/// An empty registry used when no project macros are in scope (e.g. the
/// `BuiltinVisitor::new` / `new_with_subscript_context` constructors before
/// `with_macro_registry` runs). Lets the `macro_registry` field be a plain
/// `&MacroRegistry` -- no `Option` handling at the `resolve_macro` call
/// sites -- while still defaulting to "no macros".
static EMPTY_MACRO_REGISTRY: LazyLock<MacroRegistry> = LazyLock::new(MacroRegistry::default);

/// The shared empty macro registry, for parse paths with no project macros
/// in scope (the `parse_var` convenience wrapper and the many test call
/// sites). Avoids allocating a fresh `MacroRegistry` per parse call.
pub(crate) fn empty_macro_registry() -> &'static MacroRegistry {
    &EMPTY_MACRO_REGISTRY
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
            if crate::builtins::is_stdlib_module_function(func.as_str())
                || macro_registry.resolve_macro(func).is_some()
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

/// Collapse entries that repeat an earlier entry's identifier, preserving
/// first-occurrence order -- but ONLY when the duplicates define the same
/// thing (`ImplicitVar::same_definition`).
///
/// The per-element apply-to-all expansion runs a fresh `BuiltinVisitor` per
/// element and unions every visitor's synthesized helpers. Lookup-key behavior
/// by path:
///
/// * A *scalar* per-element capture, and the arrayed capture synthesized in the
///   `Ast::Arrayed` per-element expansion, both carry the element in their name
///   (`...⁚arg0⁚north`), so the union already holds N distinct entries -- one
///   per slot. No collapsing happens for them.
/// * The arrayed `PREVIOUS`/`INIT` capture synthesized in the `Ast::ApplyToAll`
///   per-element expansion (GH #541) deliberately omits the element suffix:
///   every slot walks the *same cloned* body, so all N copies define the same
///   value. The union collapses them to one.
///
/// An ident collision whose two helpers define DIFFERENT things is a compiler
/// bug -- exactly the silent corruption a suffix-less helper caused for the
/// `Ast::Arrayed` path (PR #668), where two slots' different bodies shared one
/// name and a later slot read the earlier slot's helper. Such a collision is
/// returned as a loud `Generic` error (a clean compile failure) instead of
/// being silently kept-first, so any future regression of this class surfaces
/// rather than corrupting results.
fn dedup_vars_by_ident(
    vars: Vec<ImplicitVar>,
) -> std::result::Result<Vec<ImplicitVar>, EquationError> {
    let mut seen: HashMap<Ident<Canonical>, ImplicitVar> = HashMap::new();
    let mut deduped: Vec<ImplicitVar> = Vec::with_capacity(vars.len());
    for v in vars {
        let ident = Ident::new(v.ident());
        match seen.get(&ident) {
            Some(existing) if existing.same_definition(&v) => {
                // Same-definition duplicate (the `Ast::ApplyToAll` suffix-less
                // arrayed helper): drop it, keeping the first occurrence.
            }
            Some(_) => {
                // Same name, different content: a synthesized-helper id
                // collision the per-path suffix rules must prevent.
                return eqn_err!(
                    Generic,
                    0,
                    0,
                    format!("two different synthesized helpers both claim the name '{ident}'")
                );
            }
            None => {
                seen.insert(ident, v.clone());
                deduped.push(v);
            }
        }
    }
    Ok(deduped)
}

pub struct BuiltinVisitor<'a> {
    variable_name: &'a str,
    /// Every helper synthesized during the current walk: `PREVIOUS`/`INIT`
    /// captures, plus the module instances a stdlib or macro call expands into
    /// and the auxes their non-`Var` arguments are hoisted into. The modules
    /// are created using the same `is_stdlib_module_function` classification
    /// rule, extending the base set from `collect_module_idents()` at runtime
    /// so that nested references (like `PREVIOUS(SMOOTH(...))`) correctly
    /// capture.
    ///
    /// Every producer files through `insert_implicit_var`. Calling
    /// `IndexMap::insert` directly would silently replace an earlier helper:
    /// a macro named `ARG1` with a computed second argument can make its module
    /// and that argument both derive `$⁚{parent}⁚{n}⁚arg1`. Same-definition
    /// repeats are idempotent; any other collision is a loud parse error.
    ///
    /// Insertion-ordered, and that is load-bearing rather than incidental
    /// (GH #1002). Every producer of `ParsedVariableResult::implicit_vars`
    /// emits this map with `.values()`, and `ImplicitVarMeta` used to identify
    /// a helper by its POSITION in the resulting vector. Rust draws a fresh
    /// `RandomState` key per `HashMap`, so with a `HashMap` here two parses of
    /// the same variable -- in ONE process, differing only in their
    /// `ModuleIdentContext` -- reported the helpers in two different orders,
    /// and a position recorded against one parse resolved to a different
    /// helper against the other. Insertion order is the walk's synthesis
    /// order, so it is a function of the equation alone. Helper lookup no
    /// longer rides on position (see `ImplicitVarMeta::name`), but the two salsa
    /// values carrying this order -- `ParsedVariableResult::implicit_vars` and
    /// `VariableDeps::implicit_vars` -- have derived `PartialEq`, so an
    /// unstable order still defeats backdating and makes the compiled artifact
    /// irreproducible (the GH #595 class).
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
    /// Identifiers of Module variables in the parent model.
    /// PREVIOUS(module_var) must synthesize a scalar temp arg rather than
    /// reading a flat slot directly, because modules occupy multiple slots.
    module_idents: Option<&'a HashSet<Ident<Canonical>>>,
    /// Identifiers of *all* variables in the parent model, when known.
    ///
    /// Used by `index_is_static` to accept a *bare* element name as a static
    /// subscript index: a name that is a dimension element AND not any
    /// variable's name cannot be a dynamic-index reference, so the compiler
    /// is guaranteed to resolve it against the subscripted variable's
    /// declared dimensions (the element interpretation always wins -- see
    /// `compiler::context`'s subscript lowering). `None` (the user-equation
    /// parse path, which must stay incremental under variable renames)
    /// disables the check, keeping bare element indices on the conservative
    /// helper path.
    model_var_names: Option<&'a HashSet<Ident<Canonical>>>,
    /// The per-project macro registry. A call name that resolves here is
    /// expanded as a macro -- *before* alias-normalization, `is_builtin_fn`,
    /// or the stdlib lookup -- so a project macro shadows an identically
    /// named builtin or stdlib function (Vensim's rule). Defaults to an
    /// empty registry (no project macros) until `with_macro_registry`.
    macro_registry: &'a MacroRegistry,
    /// The canonical name of the macro model whose body this visitor is
    /// expanding, if any (i.e. the variable being parsed belongs to a
    /// macro-marked model). `None` for ordinary (non-macro-body) variables.
    ///
    /// #554 (+ follow-up): when expanding a macro body, a call whose
    /// canonical name equals this enclosing macro's own canonical name AND is
    /// a Vensim-MDL-importer-renamed builtin -- opcode-backed
    /// (`init`/`previous`) *or* stdlib-module-backed (`delayn`/`smthn`/...),
    /// per the shared `is_renamed_builtin_macro_collision` -- must resolve to
    /// the BUILTIN, not recurse into the macro. The importer's necessary
    /// `INITIAL -> INIT` / `SAMPLE IF TRUE -> PREVIOUS` / `DELAY N -> DELAYN`
    /// / `SMOOTH N -> SMTHN` rename makes such a body literally read
    /// `init = init(x)` or `delayn = delayn(...)`; without this exception the
    /// macro-shadows-everything precedence (`resolve_macro` below) would
    /// re-resolve the call to the macro forever (a salsa module-map cycle).
    /// `module_functions`' `collect_called_macros` suppresses the matching
    /// false recursion edge using the *same* predicate, so the two halves
    /// agree by construction.
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
    /// the same value in every slot and `dedup_vars_by_ident` correctly
    /// collapses them to one. In the `Ast::Arrayed` per-element expansion the
    /// bodies differ per slot, so a suffix-less capture would mint the SAME id
    /// for DIFFERENT bodies -- a silent collision that made a later slot read an
    /// earlier slot's capture (PR #668). When this flag is set, the arrayed
    /// capture appends the slot's element suffix (like the scalar captures
    /// always have), so distinct slots never collide. Set ONLY by
    /// the `Ast::Arrayed` branch of `instantiate_implicit_modules`; NOT by its
    /// `default_expr` visitor (which uses `::new`, has no `active_subscript`, and
    /// so never reaches the arrayed-helper branch).
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
            module_idents: None,
            model_var_names: None,
            macro_registry: &EMPTY_MACRO_REGISTRY,
            enclosing_model: None,
            per_element_equation: false,
        }
    }

    /// File one helper under its derived name without allowing `IndexMap`'s
    /// replacement behavior to hide a collision.
    ///
    /// A repeated same-definition helper is idempotent (the apply-to-all walk
    /// can encounter one), while different definitions claiming one name are
    /// a source-reachable compile error. The first definition remains in the
    /// map so an error can never change which helper later checks observe.
    fn insert_implicit_var(&mut self, implicit_var: ImplicitVar) -> Result<(), EquationError> {
        let ident = Ident::<Canonical>::new(implicit_var.ident());
        match self.vars.get(&ident) {
            Some(existing) if existing.same_definition(&implicit_var) => Ok(()),
            Some(_) => eqn_err!(
                DuplicateVariable,
                0,
                0,
                format!("two different synthesized helpers both claim the name '{ident}'")
            ),
            None => {
                self.vars.insert(ident, implicit_var);
                Ok(())
            }
        }
    }

    /// Create a visitor with A2A subscript context for per-element module creation
    pub fn new_with_subscript_context(
        variable_name: &'a str,
        dimensions: &[Dimension],
        subscript: &[String],
        dimensions_ctx: Option<&'a DimensionsContext>,
    ) -> Self {
        Self {
            variable_name,
            vars: Default::default(),
            n: 0,
            self_allowed: false,
            dimensions: dimensions.to_vec(),
            dimension_names: get_dimension_names(dimensions),
            active_subscript: Some(subscript.to_vec()),
            dimensions_ctx,
            module_idents: None,
            model_var_names: None,
            macro_registry: &EMPTY_MACRO_REGISTRY,
            enclosing_model: None,
            per_element_equation: false,
        }
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

    /// #554 (+ follow-up): is `func` (a raw call name) the enclosing macro's
    /// own same-canonical-name renamed builtin -- i.e. the MDL importer's
    /// renamed `INITIAL`/`SAMPLE IF TRUE` (opcode-backed) or
    /// `DELAY N`/`SMOOTH N`/... (stdlib-module-backed) builtin appearing
    /// inside the like-named macro's body? Such a call must resolve to the
    /// builtin (the opcode for `init`/`previous`, the distinct `stdlib⁚...`
    /// module for `delayn`/...), NOT (recursively) to the macro. Shares
    /// `is_renamed_builtin_macro_collision` with
    /// `module_functions::collect_called_macros` so the recursion-edge
    /// suppression and this expansion exception cannot drift apart.
    fn is_enclosing_macro_renamed_builtin_self_call(&self, func: &str) -> bool {
        let Some(enclosing) = self.enclosing_model else {
            return false;
        };
        let call = canonicalize(func);
        let enclosing = canonicalize(enclosing);
        call == enclosing && is_renamed_builtin_macro_collision(call.as_ref())
    }

    /// Set the module identifiers for PREVIOUS routing.
    fn with_module_idents(mut self, module_idents: Option<&'a HashSet<Ident<Canonical>>>) -> Self {
        self.module_idents = module_idents;
        self
    }

    /// Set the model's full variable-name set so `index_is_static` can accept
    /// non-shadowed bare element names (see the `model_var_names` field doc).
    fn with_model_var_names(
        mut self,
        model_var_names: Option<&'a HashSet<Ident<Canonical>>>,
    ) -> Self {
        self.model_var_names = model_var_names;
        self
    }

    /// Set the dimensions context so PREVIOUS/INIT can recognize statically
    /// resolvable subscript indices (qualified `dimension·element` references)
    /// outside of A2A per-element walks. The A2A constructor
    /// (`new_with_subscript_context`) already receives it.
    fn with_dimensions_ctx(mut self, dimensions_ctx: Option<&'a DimensionsContext>) -> Self {
        // Keep an existing context (set by `new_with_subscript_context`) if the
        // caller passes None.
        if dimensions_ctx.is_some() {
            self.dimensions_ctx = dimensions_ctx;
        }
        self
    }

    /// Returns true when the identifier names a module variable in either
    /// the parent model (`module_idents`) or modules synthesized in this pass.
    fn is_known_module_ident(&self, ident: &Ident<Canonical>) -> bool {
        self.module_idents.is_some_and(|ids| ids.contains(ident))
            || self.vars.get(ident).is_some_and(ImplicitVar::is_module)
    }

    /// PREVIOUS/INIT opcode routing only applies to direct scalar variables.
    /// Module variables and qualified module outputs (`module·output`) must
    /// be treated as module-backed so PREVIOUS/INIT can synthesize scalar
    /// helper args before compiling to intrinsic opcodes.
    ///
    /// The `·` split runs on the RAW ident, so a fully-quoted composite --
    /// `"module·port"`, which is what `ltm_augment::quote_ident` emits for every
    /// module-output reference in a generated LTM equation -- misses: the raw text
    /// keeps its quotes, the base is `"module` (an unclosed quote that
    /// `canonicalize` passes through verbatim), and the module lookup fails. That
    /// miss is INERT, not a latent bug: `canonicalize` strips a balanced quoted
    /// part, so the whole ident still resolves through `Context::var_ref` to the
    /// module instance's slot, codegen's `static_slot` accepts the resulting
    /// `Expr::Var`, and the emitted `LoadPrev` reads exactly the slot a capture
    /// helper would have. The helper path is needed only for a reference codegen
    /// cannot take a fixed slot for -- an ARRAYED module output port, which no
    /// model in the corpus produces. Splitting the canonical form instead would be
    /// a one-token change; it is not made because it buys no observable behavior
    /// and the quoted spelling is itself pinned by
    /// `ltm_augment_tests::quote_ident_needs_both_of_its_conjuncts`.
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

    /// Is this subscript index expression *certainly* statically resolvable
    /// at compile time?
    ///
    /// Returns true for:
    ///   * a numeric constant;
    ///   * a qualified `dimension·element` reference (which
    ///     `constify_dimensions` folds to a constant during Expr1 lowering,
    ///     regardless of context);
    ///   * when the model's variable-name set is known (`model_var_names`),
    ///     a bare identifier that is a dimension element and is NOT shadowed
    ///     by any variable (model variable, module, or implicit var
    ///     synthesized during this walk). Such a name cannot be a
    ///     dynamic-index reference, so the compiler is guaranteed to resolve
    ///     it against the subscripted variable's declared dimensions -- the
    ///     element interpretation always wins in subscript lowering.
    ///
    /// Without `model_var_names`, bare identifiers are NOT considered static
    /// even when they name a dimension element: XMILE explicitly allows
    /// element names to shadow variable names ("the Element names can be the
    /// same as Variable names"), and only the compiler -- which knows the
    /// subscripted variable's declared dimensions -- can disambiguate
    /// element-vs-variable for them. A bare identifier index therefore stays
    /// on the conservative helper-aux path for PREVIOUS/INIT.
    fn index_is_static(&self, idx: &IndexExpr0) -> bool {
        match idx {
            IndexExpr0::Expr(Expr0::Const(_, _, _)) => true,
            IndexExpr0::Expr(Expr0::Var(ident, _)) => {
                let canonical = canonicalize(ident.as_str());
                let Some(ctx) = self.dimensions_ctx else {
                    return false;
                };
                if ctx.lookup(&canonical).is_some() {
                    return true;
                }
                let Some(var_names) = self.model_var_names else {
                    return false;
                };
                let elem = crate::common::CanonicalElementName::from_raw(&canonical);
                let canonical_ident = Ident::new(&canonical);
                ctx.is_element_of_any_dimension(&elem)
                    && !var_names.contains(&canonical_ident)
                    && !self.vars.contains_key(&canonical_ident)
            }
            _ => false,
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

    /// Classify one subscript index for the shared `PREVIOUS`/`INIT` predicate.
    ///
    /// Spanning is asked BEFORE staticness because a name can satisfy both --
    /// an active apply-to-all dimension that some *other* dimension also
    /// declares as an element -- and what such an index leaves standing is what
    /// the reference means. [`SnapshotArg::subscripted`] carries the same
    /// precedence for the fold; the two must agree.
    fn classify_snapshot_index(&self, idx: &IndexExpr0) -> SnapshotIndex {
        if self.index_spans_a_dimension(idx) {
            SnapshotIndex::SpansDimension
        } else if self.index_is_static(idx) {
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
    /// A module-backed base classifies as `not_storage`, which is stricter than
    /// codegen: codegen's `static_slot` accepts an `m·port` reference like any
    /// other variable, so the capture this synthesizes for one is redundant.
    /// The redundancy is preserved deliberately -- dropping it removes slots and
    /// fragments, which is an artifact-shape change with its own ledger row.
    fn snapshot_arg(&self, arg: &Expr0) -> SnapshotArg {
        match arg {
            Expr0::Var(ident, _) if !self.is_module_backed_ident(ident) => SnapshotArg::whole(),
            Expr0::Subscript(id, indices, _) if !self.is_module_backed_ident(id) => {
                SnapshotArg::subscripted(
                    indices.iter().map(|idx| self.classify_snapshot_index(idx)),
                )
            }
            _ => SnapshotArg::not_storage(),
        }
    }

    /// Rewrite dimension references in `expr` to the active element: processing
    /// element `A2` of dimension `SubA`, `input[SubA]` becomes `input[A2]`.
    ///
    /// This decision belongs before the hoist. A capture, hoisted call
    /// argument, or module-input source is a scalar unit with no active
    /// apply-to-all element of its own, so lowering cannot resolve the bare
    /// dimension later. The GH #541 arrayed-capture arm is deliberately exempt:
    /// its declared dimensions give lowering the context it needs.
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
                // Check dimension mappings: if this dimension maps to one of our parent dimensions,
                // translate the subscript using positional correspondence.
                // For example, if DimA maps to DimB and we're processing subscript "b1" of DimB,
                // translate the reference to DimA to its equivalent element "a1".
                if let Some(ctx) = self.dimensions_ctx {
                    for (i, dim_name) in self.dimension_names.iter().enumerate() {
                        let target_element = CanonicalElementName::from_raw(&subscript[i]);

                        // Try direct/reverse mapping first, including secondary targets.
                        if let Some(source_element) =
                            ctx.translate_via_mapping(&canonical_name, dim_name, &target_element)
                        {
                            let qualified_name =
                                format!("{}·{}", canonical_name.as_str(), source_element.as_str());
                            return Var(RawIdent::new_from_str(&qualified_name), loc);
                        }

                        // If the active dimension is a subdimension of a mapped target,
                        // resolve through that mapped parent.
                        if let Some(parent_dim) =
                            ctx.find_mapping_parent_of(&canonical_name, dim_name)
                            && let Some(source_element) = ctx.translate_to_source_via_mapping(
                                &canonical_name,
                                parent_dim,
                                &target_element,
                            )
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
    /// `substitute_dimension_refs`) nor module-backed (those get their own
    /// per-element helper)? Such a bare reference is the one that breaks the
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
        // checked insertion below does not conflict with borrowing
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
            // every slot's capture defines the same value and
            // `dedup_vars_by_ident` collapses the N copies into one). But in the
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

        let transformed_arg = if self.active_subscript.is_some() {
            self.substitute_dimension_refs(arg)
        } else {
            arg
        };
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

    /// Expand one module-function call into an [`ImplicitModule`] plus a
    /// [`HoistedArg`] for each computed argument, returning a reference to the
    /// module's primary output. The descriptor owns the target model, ordered
    /// input ports, primary output, and macro-vs-stdlib arity rule.
    ///
    /// Arity: a project macro is strict -- `args.len()` must equal
    /// `descriptor.parameter_ports.len()`, else `BadBuiltinArgs` over the
    /// call's span. Stdlib functions keep their lenient behavior (a trailing
    /// port like `SMTH1`'s `initial_value` may be unwired), so no arity
    /// check is applied when `!descriptor.is_macro`.
    ///
    /// `func` is used only to derive the synthetic name; routing is entirely
    /// descriptor-driven.
    fn expand_module_function(
        &mut self,
        descriptor: &ModuleFunctionDescriptor,
        func: &str,
        args: Vec<Expr0>,
        loc: crate::builtins::Loc,
    ) -> Result<Expr0, EquationError> {
        use Expr0::*;

        if descriptor.is_macro && args.len() != descriptor.parameter_ports.len() {
            // Macro arity is strict; the span covers the whole call so the
            // diagnostic identifies the macro in context (macros.AC5.1).
            return eqn_err!(
                BadBuiltinArgs,
                loc.start,
                loc.end,
                format!(
                    "macro {func} takes exactly {} argument(s), but {} were given",
                    descriptor.parameter_ports.len(),
                    args.len()
                )
            );
        }

        let subscript_suffix = self.subscript_suffix();
        let suffix = (!subscript_suffix.is_empty()).then(|| subscript_suffix.clone());

        // Arguments are inserted before their instance. This ordering and the
        // shared counter are logical callsite facts, not presentation.
        let mut references = Vec::with_capacity(args.len());
        for (i, arg) in args.into_iter().enumerate() {
            let src = if let Var(id, loc) = arg {
                // A bare identifier wires directly. In apply-to-all
                // context a dimension name is first resolved to the active
                // qualified element name; indexed dimensions substitute to
                // a constant and retain the identifier spelling because a
                // module reference can only name storage.
                if self.active_subscript.is_some()
                    && let Var(new_id, _) = self.substitute_dimension_refs(Var(id.clone(), loc))
                {
                    new_id.as_str().to_string()
                } else {
                    id.as_str().to_string()
                }
            } else {
                // A port can only read a named variable, so a computed
                // argument becomes a scalar aux carrying the walked AST.
                let transformed_arg = if self.active_subscript.is_some() {
                    self.substitute_dimension_refs(arg)
                } else {
                    arg
                };
                let hoisted = HoistedArg::new(
                    self.variable_name,
                    self.n,
                    i,
                    transformed_arg,
                    suffix.clone(),
                );
                let id = hoisted.ident().to_string();
                self.insert_implicit_var(ImplicitVar::HoistedArg(hoisted))?;
                id
            };

            // The descriptor owns the ordered input-port names. A macro has
            // strict arity; stdlib calls may omit trailing optional ports.
            references.push((src, descriptor.parameter_ports[i].clone()));
        }

        let module = ImplicitModule::new(
            self.variable_name,
            self.n,
            func,
            descriptor.model_name.clone(),
            references,
            suffix,
        );
        let module_name = module.ident().to_string();
        // The same U+00B7 (·) middle-dot the previously-hardcoded
        // `·output` used (the already-canonical compile-time AST separator);
        // `primary_output` is "output" for stdlib, so stdlib stays identical.
        let module_output_name = format!("{}\u{b7}{}", module_name, descriptor.primary_output);
        self.insert_implicit_var(ImplicitVar::Module(module))?;

        self.n += 1;
        Ok(Var(RawIdent::new_from_str(&module_output_name), loc))
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

                // #554 (+ follow-up) exception to the macro-shadows-everything
                // precedence below: when expanding a macro body, a call whose
                // canonical name equals the *enclosing* macro's own canonical
                // name AND is a Vensim-MDL-importer-renamed builtin --
                // opcode-backed (`init`/`previous`) or stdlib-module-backed
                // (`delayn`/`smthn`/...) -- is the importer's renamed builtin
                // (`INITIAL` -> `INIT`, `SAMPLE IF TRUE` -> `PREVIOUS`,
                // `DELAY N` -> `DELAYN`, `SMOOTH N` -> `SMTHN`), NOT a
                // recursive macro call (Vensim macros cannot recurse; the
                // source wrote the distinct builtin name). It must resolve to
                // the builtin, so we skip `resolve_macro` and fall through:
                // for `init`/`previous` to the PREVIOUS/INIT intrinsic routing
                // (-> the LoadInitial/LoadPrev opcode), for `delayn`/... to
                // `rewrite_alias_module_call` + `stdlib_descriptor` (-> a
                // DISTINCT `stdlib⁚delay1`/... module whose fixed body never
                // references the user macro). Without this an INVOKED
                // such-macro would infinite-loop / form a salsa module-map
                // cycle: the body's `init(x)` / `delayn(...)` would re-resolve
                // to the macro forever. `module_functions::collect_called_macros`
                // suppresses the mirror false recursion edge with the same
                // shared predicate, so the registry build *and* this expansion
                // stay consistent (#554 + follow-up).
                let is_renamed_builtin_self_call =
                    self.is_enclosing_macro_renamed_builtin_self_call(&func);

                // Macro-shadows-everything precedence (Vensim's rule): a
                // project macro is resolved here, BEFORE alias
                // normalization / modulo / previous / init / is_builtin_fn
                // / the stdlib lookup. A macro named `SSHAPE` or
                // `RAMP FROM TO` therefore expands as the macro even though
                // it parsed as `CallKind::Builtin`. `func` is the raw call
                // name (resolve_macro canonicalizes internally).
                //
                // The #554 self-call exception (`is_renamed_builtin_self_call`)
                // suppresses resolution for a renamed-builtin call inside the
                // like-named macro's own body, so it routes to the intrinsic
                // rather than recursing into the macro.
                let descriptor = if is_renamed_builtin_self_call {
                    None
                } else {
                    self.macro_registry.resolve_macro(&func)
                };

                // #591-c1: a *genuine passthrough* macro
                // (`:MACRO: INIT(x) = INITIAL(x)`, stored after the importer's
                // INITIAL -> INIT rename as `init = init(x)`) is NOT expanded
                // into a per-element synthetic module (which mis-orders /
                // mis-propagates its value). Only a NON-passthrough resolved
                // descriptor expands here; a passthrough descriptor leaves
                // `func`/`args` untouched and falls through to the
                // renamed-builtin intrinsic routing below -- exactly as the
                // #554 self-call exception does inside a macro body, here
                // generalized from the macro body to the call site.
                //
                // The fall-through is sound because of the self-call invariant
                // the classifier guarantees: `passthrough.is_some()` implies
                // `canonicalize(call) == canonicalize(macro_name)` AND
                // `is_renamed_builtin_macro_collision(canonicalize(call))`
                // (`classify_passthrough`). So `func` here canonicalizes to the
                // opcode-backed builtin (e.g. `init`) and routes to the right
                // intrinsic below -- `init` -> `LoadInitial`, with the existing
                // `hoist_capture` hoisting for an expression argument
                // (`init_needs_temp_arg`). The macro body did no work beyond the
                // bare call, so collapsing to the opcode loses nothing.
                if let Some(descriptor) = descriptor
                    && descriptor.passthrough.is_none()
                {
                    let descriptor = descriptor.clone();
                    return self.expand_module_function(&descriptor, &func, args, loc);
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
                //   * a direct (non-module-backed) scalar variable reference, or
                //   * a subscripted reference whose base is not module-backed
                //     and whose every index is statically resolvable -- a
                //     numeric constant or a qualified `dimension·element`
                //     reference (see `index_is_static`).
                //
                // Anything else (nested PREVIOUS, PREVIOUS(expr),
                // PREVIOUS(module_var), dynamic subscript indices) is rewritten
                // through a synthesized scalar temp variable that captures the
                // value each timestep -- which also gives dynamic indices the
                // correct lagged semantics (the index itself is read at the
                // *previous* step).
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

/// Expand module-function calls -- stdlib (SMTH1, DELAY, ...) *and* project
/// macros -- plus PREVIOUS/INIT builtins into implicit module instances and
/// opcode-backed builtins.
///
/// `macro_registry` carries the per-project macros: a call name resolving
/// there expands as a macro (shadowing an identically named builtin/stdlib
/// func) and an *arrayed* macro invocation rides the per-element path via
/// `contains_module_call`. When `module_idents` is provided,
/// `PREVIOUS(module_var)` synthesizes a scalar temp arg instead of reading a
/// flat slot directly.
///
/// `enclosing_model` is the owning model's name when `variable_name` is a
/// macro-marked model's body variable (`None` otherwise). It drives the #554
/// same-named-opcode-intrinsic exception in `BuiltinVisitor::walk` so a
/// macro body's renamed-builtin call (`init` inside macro `INIT`) resolves to
/// the intrinsic instead of recursing into the macro forever.
///
/// `model_var_names`, when provided, is the model's full variable-name set;
/// it lets `PREVIOUS`/`INIT` accept a non-shadowed bare element name as a
/// static subscript index instead of synthesizing a helper aux (see
/// `BuiltinVisitor::index_is_static`).
pub fn instantiate_implicit_modules(
    variable_name: &str,
    ast: Ast<Expr0>,
    dimensions_ctx: Option<&DimensionsContext>,
    module_idents: Option<&HashSet<Ident<Canonical>>>,
    model_var_names: Option<&HashSet<Ident<Canonical>>>,
    macro_registry: &MacroRegistry,
    enclosing_model: Option<&str>,
) -> std::result::Result<(Ast<Expr0>, Vec<ImplicitVar>), EquationError> {
    match ast {
        Ast::Scalar(ast) => {
            let mut builtin_visitor = BuiltinVisitor::new(variable_name)
                .with_dimensions_ctx(dimensions_ctx)
                .with_module_idents(module_idents)
                .with_model_var_names(model_var_names)
                .with_macro_registry(macro_registry)
                .with_enclosing_model(enclosing_model);
            let transformed = builtin_visitor.walk(ast)?;
            let vars: Vec<_> = builtin_visitor.vars.values().cloned().collect();
            Ok((Ast::Scalar(transformed), vars))
        }
        Ast::ApplyToAll(dimensions, ast) => {
            // Check if expression contains a module-function call (stdlib or
            // macro) - if so, expand to per-element modules.
            if contains_module_call(&ast, macro_registry) && !dimensions.is_empty() {
                let mut all_vars = Vec::new();
                let mut elements = HashMap::new();

                for subscript in SubscriptIterator::new(&dimensions) {
                    let subscript_key = CanonicalElementName::from_raw(&subscript.join(","));
                    let ast_clone = ast.clone();

                    let mut visitor = BuiltinVisitor::new_with_subscript_context(
                        variable_name,
                        &dimensions,
                        &subscript,
                        dimensions_ctx,
                    )
                    .with_module_idents(module_idents)
                    .with_model_var_names(model_var_names)
                    .with_macro_registry(macro_registry)
                    .with_enclosing_model(enclosing_model);
                    let transformed_ast = visitor.walk(ast_clone)?;

                    elements.insert(subscript_key, transformed_ast);
                    all_vars.extend(visitor.vars.values().cloned());
                }

                Ok((
                    Ast::Arrayed(dimensions, elements, None, false),
                    dedup_vars_by_ident(all_vars)?,
                ))
            } else {
                // No module-function calls - original behavior
                let mut builtin_visitor = BuiltinVisitor::new(variable_name)
                    .with_dimensions_ctx(dimensions_ctx)
                    .with_module_idents(module_idents)
                    .with_model_var_names(model_var_names)
                    .with_macro_registry(macro_registry)
                    .with_enclosing_model(enclosing_model);
                let transformed = builtin_visitor.walk(ast)?;
                let vars: Vec<_> = builtin_visitor.vars.values().cloned().collect();
                Ok((Ast::ApplyToAll(dimensions, transformed), vars))
            }
        }
        Ast::Arrayed(dimensions, elements, default_expr, apply_default_to_missing) => {
            let any_module_call = elements
                .values()
                .any(|e| contains_module_call(e, macro_registry))
                || default_expr
                    .as_ref()
                    .is_some_and(|e| contains_module_call(e, macro_registry));
            if any_module_call && !dimensions.is_empty() {
                let mut all_vars = Vec::new();
                let mut new_elements = HashMap::new();
                for (subscript_key, equation) in elements_in_stable_order(elements) {
                    let subscript_parts: Vec<String> = subscript_key
                        .as_str()
                        .split(',')
                        .map(|s| s.to_string())
                        .collect();
                    let mut visitor = BuiltinVisitor::new_with_subscript_context(
                        variable_name,
                        &dimensions,
                        &subscript_parts,
                        dimensions_ctx,
                    )
                    .with_module_idents(module_idents)
                    .with_model_var_names(model_var_names)
                    .with_macro_registry(macro_registry)
                    .with_enclosing_model(enclosing_model)
                    // Per-element slots have distinct equations, so any arrayed
                    // PREVIOUS/INIT helper must carry the element suffix to avoid
                    // colliding across slots (PR #668).
                    .with_per_element_equation(true);
                    let transformed = visitor.walk(equation)?;
                    new_elements.insert(subscript_key, transformed);
                    all_vars.extend(visitor.vars.values().cloned());
                }
                let transformed_default = if let Some(default_expr) = default_expr {
                    let mut default_visitor = BuiltinVisitor::new(variable_name)
                        .with_dimensions_ctx(dimensions_ctx)
                        .with_module_idents(module_idents)
                        .with_macro_registry(macro_registry)
                        .with_enclosing_model(enclosing_model);
                    let transformed = default_visitor.walk(default_expr)?;
                    all_vars.extend(default_visitor.vars.values().cloned());
                    Some(transformed)
                } else {
                    None
                };
                Ok((
                    Ast::Arrayed(
                        dimensions,
                        new_elements,
                        transformed_default,
                        apply_default_to_missing,
                    ),
                    dedup_vars_by_ident(all_vars)?,
                ))
            } else {
                let mut builtin_visitor = BuiltinVisitor::new(variable_name)
                    .with_dimensions_ctx(dimensions_ctx)
                    .with_module_idents(module_idents)
                    .with_model_var_names(model_var_names)
                    .with_macro_registry(macro_registry)
                    .with_enclosing_model(enclosing_model);
                // One visitor across every slot, so the `n` counter that names
                // each synthesized helper is handed out in this iteration
                // order -- see `elements_in_stable_order`.
                let elements: std::result::Result<HashMap<_, _>, EquationError> =
                    elements_in_stable_order(elements)
                        .into_iter()
                        .map(|(subscript, equation)| {
                            builtin_visitor.walk(equation).map(|ast| (subscript, ast))
                        })
                        .collect();
                let transformed_default = if let Some(default_expr) = default_expr {
                    Some(builtin_visitor.walk(default_expr)?)
                } else {
                    None
                };
                let vars: Vec<_> = builtin_visitor.vars.values().cloned().collect();
                Ok((
                    Ast::Arrayed(
                        dimensions,
                        elements?,
                        transformed_default,
                        apply_default_to_missing,
                    ),
                    vars,
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins::Loc;
    use crate::test_common::TestProject;

    fn body(eqn: &str) -> Expr0 {
        Expr0::new(eqn, crate::lexer::LexerType::Equation)
            .expect("test argument must lex")
            .expect("test argument must parse")
    }

    /// A capture of `parent`, minted the way the visitor mints one.
    fn capture(parent: &str, id: usize, eqn: &str) -> ImplicitVar {
        ImplicitVar::Capture(Capture::new(
            parent,
            id,
            CaptureKind::Previous,
            body(eqn),
            None,
            Vec::new(),
        ))
    }

    fn hoisted(parent: &str, id: usize, index: usize, eqn: &str) -> ImplicitVar {
        ImplicitVar::HoistedArg(HoistedArg::new(parent, id, index, body(eqn), None))
    }

    fn module(parent: &str, id: usize, func: &str, src: &str) -> ImplicitVar {
        ImplicitVar::Module(ImplicitModule::new(
            parent,
            id,
            func,
            format!("stdlib\u{205A}{func}"),
            vec![(src.to_string(), "input".to_string())],
            None,
        ))
    }

    /// One representative of every current [`ImplicitVar`] arm, derived from
    /// the production visitor rather than assembled as a test-only variant
    /// list. The equation emits a capture while walking the first argument,
    /// then hoisted arguments and their module instance.
    fn every_implicit_var_variant() -> Vec<ImplicitVar> {
        let ast = Ast::Scalar(body("SMTH1(PREVIOUS(a * 2, 0) + 1, 2)"));
        let (_, emitted) =
            instantiate_implicit_modules("h", ast, None, None, None, empty_macro_registry(), None)
                .expect("the representative production expansion must succeed");

        let mut capture = None;
        let mut hoisted = None;
        let mut module = None;
        for helper in emitted {
            // Exhaustive by construction: a new enum arm must state how it is
            // represented in the N-way dedup tests below.
            match helper {
                ImplicitVar::Capture(_) if capture.is_none() => capture = Some(helper),
                ImplicitVar::HoistedArg(_) if hoisted.is_none() => hoisted = Some(helper),
                ImplicitVar::Module(_) if module.is_none() => module = Some(helper),
                ImplicitVar::Capture(_) | ImplicitVar::HoistedArg(_) | ImplicitVar::Module(_) => {}
            }
        }
        vec![
            capture.expect("the production fixture must emit a capture"),
            hoisted.expect("the production fixture must emit a hoisted argument"),
            module.expect("the production fixture must emit a module"),
        ]
    }

    /// A different-ident helper of the same arm as `helper`.
    fn distinct_helper(helper: &ImplicitVar) -> ImplicitVar {
        match helper {
            ImplicitVar::Capture(_) => capture("g", 0, "b"),
            ImplicitVar::HoistedArg(_) => hoisted("g", 0, 0, "b"),
            ImplicitVar::Module(_) => module("g", 0, "smth1", "b"),
        }
    }

    /// The same ident as `helper`, but a different definition.
    fn conflicting_helper(helper: &ImplicitVar) -> ImplicitVar {
        match helper {
            ImplicitVar::Capture(c) => ImplicitVar::Capture(Capture::new(
                "h",
                c.id(),
                c.kind(),
                body("different"),
                c.suffix().map(str::to_string),
                c.dims().to_vec(),
            )),
            // The production representative is the first call argument.
            ImplicitVar::HoistedArg(a) => ImplicitVar::HoistedArg(HoistedArg::new(
                "h",
                a.id(),
                0,
                body("different"),
                a.suffix().map(str::to_string),
            )),
            ImplicitVar::Module(m) => ImplicitVar::Module(ImplicitModule::new(
                "h",
                m.id(),
                "smth1",
                m.model_name().to_string(),
                vec![("different".to_string(), "input".to_string())],
                m.suffix().map(str::to_string),
            )),
        }
    }

    /// Re-key a production-emitted representative so every arm claims the
    /// same physical lookup key. The bodies, model, and wiring sources come from the
    /// visitor output; only the private constructor inputs that produce the
    /// common `$⁚matrix⁚0⁚arg0` key are changed. That is the state the
    /// name-keyed dedup must adjudicate.
    fn pair_matrix_helper(helper: &ImplicitVar) -> ImplicitVar {
        match helper {
            ImplicitVar::Capture(c) => ImplicitVar::Capture(Capture::new(
                "matrix",
                0,
                c.kind(),
                c.arg().clone(),
                None,
                c.dims().to_vec(),
            )),
            ImplicitVar::HoistedArg(a) => {
                ImplicitVar::HoistedArg(HoistedArg::new("matrix", 0, 0, a.arg().clone(), None))
            }
            ImplicitVar::Module(m) => ImplicitVar::Module(ImplicitModule::new(
                "matrix",
                0,
                "arg0",
                m.model_name().to_string(),
                m.references()
                    .iter()
                    .map(|reference| {
                        let (_, port) = reference
                            .dst
                            .rsplit_once('.')
                            .expect("production module destinations carry a port");
                        (reference.src.clone(), port.to_string())
                    })
                    .collect(),
                None,
            )),
        }
    }

    fn implicit_var_arm(helper: &ImplicitVar) -> &'static str {
        match helper {
            ImplicitVar::Capture(_) => "Capture",
            ImplicitVar::HoistedArg(_) => "HoistedArg",
            ImplicitVar::Module(_) => "Module",
        }
    }

    /// `dedup_vars_by_ident` collapses same-definition duplicates (the
    /// `Ast::ApplyToAll` suffix-less arrayed capture) but keeps distinct
    /// idents.
    ///
    /// Rows come from [`every_implicit_var_variant`], and
    /// [`distinct_helper`]'s exhaustive match makes a new arm a compile error
    /// until its semantics are pinned here.
    #[test]
    fn dedup_vars_collapses_identical_keeps_distinct() {
        for helper in every_implicit_var_variant() {
            assert!(!helper.is_stock(), "no current implicit arm is a stock");
            let distinct = distinct_helper(&helper);
            let expected = [helper.ident().to_string(), distinct.ident().to_string()];
            let out = dedup_vars_by_ident(vec![helper.clone(), helper, distinct])
                .unwrap_or_else(|e| panic!("same-definition duplicate must collapse: {e:?}"));
            assert_eq!(out.len(), 2, "the first collapses; the second stays");
            assert_eq!(out[0].ident(), expected[0]);
            assert_eq!(out[1].ident(), expected[1]);
        }
    }

    /// An ident collision whose two helpers differ (the PR #668 corruption:
    /// two `Ast::Arrayed` slots minting one suffix-less helper id for different
    /// bodies) is a loud error, never silently kept-first.
    ///
    /// The representatives are production-emitted and
    /// [`conflicting_helper`]'s exhaustive match pins every current enum arm.
    #[test]
    fn dedup_vars_errors_on_conflicting_collision() {
        for helper in every_implicit_var_variant() {
            let conflict = conflicting_helper(&helper);
            assert_eq!(
                helper.ident(),
                conflict.ident(),
                "the fixture must collide on ident"
            );
            let err = dedup_vars_by_ident(vec![helper, conflict])
                .expect_err("a conflicting same-ident collision must be a loud error");
            assert_eq!(
                err.code,
                crate::common::ErrorCode::Generic,
                "expected a Generic compiler-invariant error, got {err:?}"
            );
        }
    }

    /// The visitor's checked map insertion is idempotent for an exact repeat,
    /// rejects a different definition under the same key, and leaves the first
    /// definition intact. Representatives come from the production expansion
    /// in [`every_implicit_var_variant`]; [`conflicting_helper`] is exhaustive
    /// over the enum, so every producer arm is constrained.
    #[test]
    fn checked_insertion_keeps_first_and_refuses_conflicting_definition() {
        for helper in every_implicit_var_variant() {
            let mut visitor = BuiltinVisitor::new("h");
            visitor
                .insert_implicit_var(helper.clone())
                .expect("the first definition must be accepted");
            visitor
                .insert_implicit_var(helper.clone())
                .expect("an exact repeat must be idempotent");
            assert_eq!(visitor.vars.len(), 1);

            let conflict = conflicting_helper(&helper);
            let error = visitor
                .insert_implicit_var(conflict)
                .expect_err("a different definition under the same name must be loud");
            assert_eq!(error.code, crate::common::ErrorCode::DuplicateVariable);
            assert_eq!(visitor.vars.len(), 1);
            assert!(
                visitor
                    .vars
                    .values()
                    .next()
                    .is_some_and(|kept| kept == &helper),
                "the rejected insertion must not replace the first definition"
            );
        }
    }

    /// Every ordered pair in the current 3x3 [`ImplicitVar`] matrix reaches
    /// name-keyed dedup. Same-arm clones collapse; each of the six cross-arm
    /// pairs is a loud collision in both directions.
    ///
    /// Representatives come from [`every_implicit_var_variant`], not test-only
    /// enum construction. [`pair_matrix_helper`] is exhaustive, so a new arm
    /// cannot silently escape this matrix even though Rust enums have no
    /// runtime variant iterator.
    #[test]
    fn dedup_vars_covers_the_complete_variant_pair_matrix() {
        let helpers: Vec<_> = every_implicit_var_variant()
            .iter()
            .map(pair_matrix_helper)
            .collect();
        assert_eq!(helpers.len(), 3, "the current enum has three variants");

        let mut pair_count = 0;
        let mut cross_variant_count = 0;
        for (left_index, left) in helpers.iter().enumerate() {
            for (right_index, right) in helpers.iter().enumerate() {
                pair_count += 1;
                assert_eq!(
                    left.ident(),
                    right.ident(),
                    "the matrix only constrains same-name pairs"
                );
                let result = dedup_vars_by_ident(vec![left.clone(), right.clone()]);
                if left_index == right_index {
                    let deduped = result.unwrap_or_else(|error| {
                        panic!(
                            "{} + {} clones must collapse: {error:?}",
                            implicit_var_arm(left),
                            implicit_var_arm(right)
                        )
                    });
                    assert_eq!(deduped.len(), 1);
                } else {
                    cross_variant_count += 1;
                    let error = result.expect_err("cross-variant same-name pair must be loud");
                    assert_eq!(
                        error.code,
                        crate::common::ErrorCode::Generic,
                        "{} + {} must refuse as a compiler-invariant collision",
                        implicit_var_arm(left),
                        implicit_var_arm(right)
                    );
                }
            }
        }
        assert_eq!(pair_count, 9, "3 variants produce 3x3 ordered pairs");
        assert_eq!(
            cross_variant_count, 6,
            "the off-diagonal matrix has six ordered pairs"
        );
    }

    /// A capture whose two copies differ only in where they were written is
    /// ONE capture. The apply-to-all expansion walks a clone of one body per
    /// element and the dt and initial passes walk one equation twice, so this
    /// is what lets those copies collapse instead of colliding; positions
    /// would also make a whitespace-only difference between an element's
    /// equation and its initial equation a compile error.
    #[test]
    fn capture_definition_equality_ignores_source_position() {
        let spaced = capture("h", 0, "a + b");
        let tight = capture("h", 0, "a+b");
        assert_ne!(spaced, tight, "PartialEq keeps positions, for salsa");
        assert!(
            spaced.same_definition(&tight),
            "the dedup question ignores them"
        );
        let out = dedup_vars_by_ident(vec![spaced, tight])
            .expect("two spellings of one body are one capture");
        assert_eq!(out.len(), 1);
    }

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
        let visitor = BuiltinVisitor::new_with_subscript_context(
            "test_var",
            &active_dims,
            &active_subscript,
            Some(&dims_ctx),
        );

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
    /// The argument is carried as an `Expr0` subtree, so the printer and lexer
    /// need not agree on spellings such as `<>`, `not`, and chained `^` for the
    /// model to compile.
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

    /// The subtree keeps an `If` under an operator grouped as written (GH #913).
    ///
    /// `If` is not an atom in the equation grammar. Printing the argument AST
    /// `Div(If(1>0, 10, 20), 2)` bare under its operator can produce
    ///
    /// ```text
    /// if (1 > 0) then (10) else (20) / 2
    /// ```
    ///
    /// which parses as `If(1>0, 10, 20/2)` and silently reports `10` instead of
    /// `5`. Carrying the subtree makes that regrouping impossible.
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
