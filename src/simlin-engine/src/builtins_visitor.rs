// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use crate::ast::{Ast, BinaryOp, Expr0, IndexExpr0, print_eqn};
use crate::builtins::{UntypedBuiltinFn, is_builtin_fn};
use crate::common::{
    Canonical, CanonicalDimensionName, CanonicalElementName, EquationError, Ident, RawIdent,
    canonicalize,
};
use crate::dimensions::{Dimension, DimensionsContext, SubscriptIterator};
use crate::module_functions::{
    MacroRegistry, ModuleFunctionDescriptor, is_renamed_builtin_macro_collision, stdlib_descriptor,
};
use crate::{datamodel, eqn_err};

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
        let rounded = n.round();
        if (*n - rounded).abs() < 1e-9 && rounded >= 0.0 {
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
        return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
    }

    let mut it = args.into_iter();
    let input = it.next().unwrap();
    let delay_time = it.next().unwrap();
    let order_expr = it.next().unwrap();
    let init = it.next();

    let Some(order) = parse_module_order_arg(&order_expr) else {
        return eqn_err!(UnknownBuiltin, loc.start, loc.end);
    };
    let rewritten_name = match (func.as_str(), order) {
        ("delayn", 1) => "delay1",
        ("delayn", 3) => "delay3",
        ("smthn", 1) => "smth1",
        ("smthn", 3) => "smth3",
        _ => return eqn_err!(UnknownBuiltin, loc.start, loc.end),
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

/// Collapse entries that repeat an earlier entry's identifier, preserving
/// first-occurrence order -- but ONLY when the duplicates are byte-identical.
///
/// The per-element apply-to-all expansion runs a fresh `BuiltinVisitor` per
/// element and unions every visitor's synthesized helpers. Helper identity by
/// path:
///
/// * A *scalar* per-element helper, and the arrayed helper synthesized in the
///   `Ast::Arrayed` per-element expansion, both carry the element in their name
///   (`...⁚arg0⁚north`), so the union already holds N distinct entries -- one
///   per slot. No collapsing happens for them.
/// * The arrayed `PREVIOUS`/`INIT` helper synthesized in the `Ast::ApplyToAll`
///   per-element expansion (GH #541) deliberately omits the element suffix:
///   every slot walks the *same cloned* body, so all N copies are
///   byte-identical `Equation::ApplyToAll` variables. The union MUST collapse
///   them to one (the downstream layout indexes `implicit_vars` positionally,
///   so duplicate names would mint colliding slots).
///
/// An ident collision whose two variables are NOT byte-identical is a compiler
/// bug -- exactly the silent corruption a suffix-less helper caused for the
/// `Ast::Arrayed` path (PR #668), where two slots' different bodies shared one
/// name and a later slot read the earlier slot's helper. Such a collision is
/// returned as a loud `Generic` error (a clean compile failure) instead of
/// being silently kept-first, so any future regression of this class surfaces
/// rather than corrupting results.
fn dedup_vars_by_ident(
    vars: Vec<datamodel::Variable>,
) -> std::result::Result<Vec<datamodel::Variable>, EquationError> {
    let mut seen: HashMap<Ident<Canonical>, datamodel::Variable> = HashMap::new();
    let mut deduped: Vec<datamodel::Variable> = Vec::with_capacity(vars.len());
    for v in vars {
        let ident = Ident::new(v.get_ident());
        match seen.get(&ident) {
            Some(existing) if existing == &v => {
                // Byte-identical duplicate (the `Ast::ApplyToAll` suffix-less
                // arrayed helper): drop it, keeping the first occurrence.
            }
            Some(_) => {
                // Same name, different content: a synthesized-helper id
                // collision the per-path suffix rules must prevent.
                return eqn_err!(Generic, 0, 0);
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
    /// Modules synthesized during the current walk (e.g., SMOOTH, DELAY
    /// expansions). These are created using the same
    /// `is_stdlib_module_function` classification rule, extending the base
    /// set from `collect_module_idents()` at runtime so that nested references
    /// (like `PREVIOUS(SMOOTH(...))`) correctly synthesize scalar helper args.
    vars: HashMap<Ident<Canonical>, datamodel::Variable>,
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
    /// This selects whether the GH #541 arrayed `PREVIOUS`/`INIT` helper
    /// (`make_temp_arg`'s arrayed branch) carries the active element in its
    /// name. In the `Ast::ApplyToAll` per-element expansion every slot walks the
    /// SAME cloned body, so the suffix-less helper `$⁚{var}⁚{n}⁚arg0` is
    /// byte-identical across slots and `dedup_vars_by_ident` correctly collapses
    /// them to one. In the `Ast::Arrayed` per-element expansion the bodies
    /// differ per slot, so a suffix-less helper would mint the SAME id for
    /// DIFFERENT `Equation::ApplyToAll` bodies -- a silent collision that made a
    /// later slot read an earlier slot's helper (PR #668). When this flag is
    /// set, the arrayed helper appends the slot's element suffix (like the
    /// scalar helpers always have), so distinct slots never collide. Set ONLY by
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
            || self
                .vars
                .get(ident)
                .is_some_and(|var| matches!(var, datamodel::Variable::Module(_)))
    }

    /// PREVIOUS/INIT opcode routing only applies to direct scalar variables.
    /// Module variables and qualified module outputs (`module·output`) must
    /// be treated as module-backed so PREVIOUS/INIT can synthesize scalar
    /// helper args before compiling to intrinsic opcodes.
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

    /// Substitute dimension references in the expression with concrete element names.
    /// For example, if we're processing element "A2" of dimension "SubA",
    /// transform `input[SubA]` to `input[A2]`.
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
                                return Const(subscript[i].clone(), val, loc);
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
    /// expression arg routed through `make_temp_arg`).
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
            // the helper fragment fails (GH #742). The name is lowercased
            // because parsed `Expr0` builtin names keep their source casing
            // while `reducer_kind_from_name` matches lowercase.
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

    /// Synthesize the helper aux that captures an expression `PREVIOUS`/`INIT`
    /// argument and return the reference expression the caller substitutes for
    /// the argument.
    ///
    /// Outside A2A context (`active_subscript == None`), or when the captured
    /// argument carries no bare variable reference, the helper is a scalar aux
    /// holding the (dimension-substituted) argument and the reference is a bare
    /// `Var` -- unchanged from the original behavior.
    ///
    /// In A2A context, when the argument contains a bare variable reference
    /// (`arg_has_bare_var_ref`) AND no subscript at all (`arg_has_subscript`),
    /// the helper is instead an *arrayed* aux (`Equation::ApplyToAll` over the
    /// active dimensions) holding the argument *without* per-element
    /// substitution, so a bare arrayed name keeps its array shape (GH #541). The
    /// returned reference subscripts that helper by the active element
    /// (`helper[<element>]`), a static per-element access the caller's outer
    /// `PREVIOUS`/`INIT` then compiles to a fixed slot. The arrayed helper's
    /// name carries NO element suffix, so every element of the enclosing
    /// apply-to-all produces the identical `Equation::ApplyToAll` helper, which
    /// `instantiate_implicit_modules` deduplicates into one.
    ///
    /// Any subscripted arg takes the OLD scalar path instead (see
    /// `arg_has_subscript`): `substitute_dimension_refs` translates each
    /// subscript per element, which is the proven pre-#541 behavior and avoids
    /// the arrayed helper's subscript-interaction bugs (the C-LEARN regression).
    fn make_temp_arg(&mut self, arg: Expr0) -> Expr0 {
        let loc = crate::builtins::Loc::default();

        // The active per-element subscript, cloned up front so the helper-
        // insertion (`&mut self.vars`) below does not conflict with borrowing
        // it. `Some` exactly in A2A context; cheap (a few element-name Strings).
        let active_subscript = self.active_subscript.clone();
        if let Some(subscript) = active_subscript.as_ref()
            && self.arg_has_bare_var_ref(&arg)
            && !self.arg_has_subscript(&arg)
        {
            // Arrayed helper holding the *un-substituted* argument so bare
            // arrayed names stay arrayed and a subscripted reference (`arr[Dim]`)
            // broadcasts over the helper's own dimensions instead of being
            // frozen to one element.
            //
            // The name omits the element suffix in the `Ast::ApplyToAll`
            // per-element expansion (every slot walks the same cloned body, so
            // the suffix-less helper is byte-identical and `dedup_vars_by_ident`
            // collapses the N copies into one). But in the `Ast::Arrayed`
            // per-element expansion (`per_element_equation`) each slot has its
            // OWN body, so a suffix-less id would mint the same name for
            // different bodies -- a silent collision (PR #668). There the name
            // carries the slot's element suffix, exactly like the scalar
            // helpers, so distinct slots get distinct helpers.
            let subscript_suffix = self.subscript_suffix();
            let id = if self.per_element_equation && !subscript_suffix.is_empty() {
                format!(
                    "$⁚{}⁚{}⁚arg0⁚{}",
                    self.variable_name, self.n, subscript_suffix
                )
            } else {
                format!("$⁚{}⁚{}⁚arg0", self.variable_name, self.n)
            };
            // The helper aux's `Equation::ApplyToAll` dims carry the active
            // (canonical) dimension names; `variable::get_dimensions` resolves
            // them canonically against the project dimensions, so they match a
            // dimension declared with original casing/spacing.
            let dims: Vec<String> = self
                .dimension_names
                .iter()
                .map(|d| d.as_str().to_string())
                .collect();
            let eqn = print_eqn(&arg);
            let x_var = datamodel::Variable::Aux(datamodel::Aux {
                ident: id.clone(),
                equation: datamodel::Equation::ApplyToAll(dims, eqn),
                documentation: "".to_string(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            });
            self.vars.insert(Ident::new(&id), x_var);
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
            return Expr0::Subscript(RawIdent::new_from_str(&id), indices, loc);
        }

        let transformed_arg = if self.active_subscript.is_some() {
            self.substitute_dimension_refs(arg)
        } else {
            arg
        };
        let subscript_suffix = self.subscript_suffix();
        let id = if subscript_suffix.is_empty() {
            format!("$⁚{}⁚{}⁚arg0", self.variable_name, self.n)
        } else {
            format!(
                "$⁚{}⁚{}⁚arg0⁚{}",
                self.variable_name, self.n, subscript_suffix
            )
        };
        let eqn = print_eqn(&transformed_arg);
        let x_var = datamodel::Variable::Aux(datamodel::Aux {
            ident: id.clone(),
            equation: datamodel::Equation::Scalar(eqn),
            documentation: "".to_string(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        });
        self.vars.insert(Ident::new(&id), x_var);
        self.n += 1;
        Expr0::Var(RawIdent::new_from_str(&id), loc)
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

    /// Expand one module-function call (stdlib *or* macro) into a synthetic
    /// `Variable::Module` plus hoisted argument `Aux`es, returning the
    /// replacement expression `Var("<module>·<primary_output>")`.
    ///
    /// This is the generalized form of the previously stdlib-hardcoded
    /// rewrite. The descriptor supplies the three facts that used to be
    /// inlined: the target `model_name` (was `format!("stdlib⁚{func}")`),
    /// the ordered `dst` port names (was `stdlib_args`), and the output
    /// variable whose value replaces the call (was the hardcoded
    /// `·output`). The synthetic-instance name, A2A subscript-suffix logic,
    /// and per-argument hoisting are reused verbatim, so for a stdlib
    /// descriptor (`primary_output == "output"`) the expansion is
    /// byte-for-byte identical to before.
    ///
    /// Arity: a project macro is strict -- `args.len()` must equal
    /// `descriptor.parameter_ports.len()`, else `BadBuiltinArgs` over the
    /// call's span. Stdlib functions keep their lenient behavior (a trailing
    /// port like `SMTH1`'s `initial_value` may be unwired), so no arity
    /// check is applied when `!descriptor.is_macro`.
    ///
    /// `func` is only used to name the synthetic instance/arg vars (kept
    /// identical to the pre-extraction `$⁚{var}⁚{n}⁚{func}` form); routing
    /// is entirely descriptor-driven.
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
            return eqn_err!(BadBuiltinArgs, loc.start, loc.end);
        }

        // In A2A context, add subscript suffix to module name for uniqueness
        let subscript_suffix = self.subscript_suffix();
        let module_name = if subscript_suffix.is_empty() {
            format!("$⁚{}⁚{}⁚{}", self.variable_name, self.n, func)
        } else {
            format!(
                "$⁚{}⁚{}⁚{}⁚{}",
                self.variable_name, self.n, func, subscript_suffix
            )
        };

        let ident_args = args.into_iter().enumerate().map(|(i, arg)| {
            if let Var(id, _loc) = arg {
                // In A2A context, substitute dimension refs in simple var references too
                if self.active_subscript.is_some() {
                    let substituted = self.substitute_dimension_refs(Var(id.clone(), _loc));
                    if let Var(new_id, _) = substituted {
                        return new_id.as_str().to_string();
                    }
                }
                id.as_str().to_string()
            } else {
                // In A2A context, substitute dimension refs and add subscript suffix
                let transformed_arg = if self.active_subscript.is_some() {
                    self.substitute_dimension_refs(arg)
                } else {
                    arg
                };

                let id = if subscript_suffix.is_empty() {
                    format!("$⁚{}⁚{}⁚arg{}", self.variable_name, self.n, i)
                } else {
                    format!(
                        "$⁚{}⁚{}⁚arg{}⁚{}",
                        self.variable_name, self.n, i, subscript_suffix
                    )
                };
                let eqn = print_eqn(&transformed_arg);
                let x_var = datamodel::Variable::Aux(datamodel::Aux {
                    ident: id.clone(),
                    equation: datamodel::Equation::Scalar(eqn),
                    documentation: "".to_string(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                });
                self.vars.insert(Ident::new(&id), x_var);
                id
            }
        });

        let references: Vec<_> = ident_args
            .into_iter()
            .enumerate()
            .map(|(i, src)| datamodel::ModuleReference {
                src,
                // dst port names come from the descriptor (stdlib_args for a
                // stdlib func, MacroSpec.parameters for a macro). A macro
                // call has exactly `parameter_ports.len()` args (checked
                // above); a stdlib call may legitimately have fewer, wiring
                // only the leading ports.
                dst: format!("{}.{}", module_name, descriptor.parameter_ports[i]),
            })
            .collect();
        let x_module = datamodel::Variable::Module(datamodel::Module {
            ident: module_name.clone(),
            model_name: descriptor.model_name.clone(),
            documentation: "".to_string(),
            units: None,
            references,
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        });
        // The same U+00B7 (·) middle-dot the previously-hardcoded
        // `·output` used (the already-canonical compile-time AST separator);
        // `primary_output` is "output" for stdlib, so stdlib stays identical.
        let module_output_name = format!("{}\u{b7}{}", module_name, descriptor.primary_output);
        self.vars.insert(Ident::new(&module_name), x_module);

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
                // `make_temp_arg` hoisting for an expression argument
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
                    args.push(Const("0".to_string(), 0.0, loc));
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
                    // them (`make_temp_arg` substitutes internally, and the
                    // substitution is idempotent).
                    let arg0 = match arg0 {
                        Subscript(_, _, _) if self.active_subscript.is_some() => {
                            self.substitute_dimension_refs(arg0)
                        }
                        other => other,
                    };
                    let needs_temp_arg = match &arg0 {
                        Var(ident, _) => self.is_module_backed_ident(ident),
                        Subscript(id, indices, _) => {
                            self.is_module_backed_ident(id)
                                || !indices.iter().all(|idx| self.index_is_static(idx))
                        }
                        _ => true,
                    };
                    let arg0 = if needs_temp_arg {
                        // `make_temp_arg` returns the reference expression for
                        // the synthesized helper: a bare `Var` for a scalar
                        // helper, or a subscripted `helper[<element>]` access
                        // for the arrayed helper it synthesizes when the arg
                        // carries a bare arrayed reference (GH #541).
                        self.make_temp_arg(arg0)
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
                    return eqn_err!(UnknownBuiltin, loc.start, loc.end);
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
) -> std::result::Result<(Ast<Expr0>, Vec<datamodel::Variable>), EquationError> {
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
                for (subscript_key, equation) in elements {
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
                let elements: std::result::Result<HashMap<_, _>, EquationError> = elements
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

    /// Build a minimal `Variable::Aux` with the given ident and scalar equation.
    fn aux(ident: &str, eqn: &str) -> datamodel::Variable {
        datamodel::Variable::Aux(datamodel::Aux {
            ident: ident.to_string(),
            equation: datamodel::Equation::Scalar(eqn.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })
    }

    /// `dedup_vars_by_ident` collapses byte-identical duplicates (the
    /// `Ast::ApplyToAll` suffix-less arrayed helper) but keeps distinct idents.
    #[test]
    fn dedup_vars_collapses_identical_keeps_distinct() {
        let vars = vec![
            aux("h", "previous(a, 0)"),
            aux("h", "previous(a, 0)"), // byte-identical duplicate
            aux("g", "previous(b, 0)"),
        ];
        let out = dedup_vars_by_ident(vars).expect("identical duplicate must collapse");
        assert_eq!(out.len(), 2, "identical 'h' collapses; 'g' stays");
        assert_eq!(out[0].get_ident(), "h");
        assert_eq!(out[1].get_ident(), "g");
    }

    /// An ident collision whose two variables DIFFER (the PR #668 corruption:
    /// two `Ast::Arrayed` slots minting the same suffix-less helper id for
    /// different bodies) must be a LOUD error, never silently kept-first.
    #[test]
    fn dedup_vars_errors_on_conflicting_collision() {
        let vars = vec![
            aux("h", "previous(a, 0)"),
            aux("h", "previous(b, 0)"), // same ident, DIFFERENT body
        ];
        let err = dedup_vars_by_ident(vars)
            .expect_err("a conflicting same-ident collision must be a loud error");
        assert_eq!(
            err.code,
            crate::common::ErrorCode::Generic,
            "expected a Generic compiler-invariant error, got {err:?}"
        );
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
