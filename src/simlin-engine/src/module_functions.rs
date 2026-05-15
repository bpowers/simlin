// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
// This module is the pure functional core of the module-function resolver.
// It takes `datamodel` values in and returns a registry or an `Error` out,
// with no I/O and no compiler-pipeline plumbing (that wiring lives in
// `model.rs`/`db.rs`). Every function here is deterministic and side-effect
// free, so the unit tests below hand-build small `datamodel::Model` fixtures
// and assert directly.

//! The module-function resolver: a unified `ModuleFunctionDescriptor` for
//! both stdlib functions (`SMTH1`, `DELAY3`, `TREND`, `NPV`, ...) and project
//! macros, plus the per-project `MacroRegistry` and its build-time validation
//! (duplicate macro name, macro/model name collision, recursion cycle).
//!
//! This generalizes the engine's existing *stdlib-as-modules* mechanism:
//! `BuiltinVisitor` instantiates stdlib functions as `Variable::Module`
//! targets, and a macro (Phase 2 turns each `:MACRO:` into a macro-marked
//! `datamodel::Model`) is structurally just another module-target model. A
//! descriptor answers, for one call name, "what model does this expand into,
//! which input ports do the arguments wire to, and which body variable's
//! value replaces the call expression?".

use std::collections::HashMap;

use crate::ast::Expr0;
use crate::builtins::UntypedBuiltinFn;
use crate::common::{Error, canonicalize};
use crate::lexer::LexerType;
use crate::{datamodel, model_err};

/// The unified answer for "what does this module-function expand into,"
/// serving both stdlib functions and project macros.
//
// `salsa::Update` lets `MacroRegistry` (which holds these) be the return
// value of the `project_macro_registry` salsa-tracked query. This is a pure
// data marker (in-place update support), not a side effect -- it does not
// compromise this module's Functional-Core status, mirroring how
// `datamodel::MacroSpec`/`Compat` derive it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, salsa::Update)]
pub(crate) struct ModuleFunctionDescriptor {
    /// The `datamodel::Model.name` of the target model -- `"stdlib⁚smth1"`
    /// for a stdlib function, or the macro's canonical model name.
    pub model_name: String,
    /// Ordered input-port variable names; call argument `i` wires to port `i`.
    pub parameter_ports: Vec<String>,
    /// The body variable whose value the call expression is replaced with.
    pub primary_output: String,
    /// `:`-list additional output ports (empty for stdlib and for
    /// single-output macros; consumed in Phase 4).
    pub additional_outputs: Vec<String>,
    /// True for project macros (strict arity -- argument count must equal
    /// `parameter_ports.len()`); false for stdlib functions, which permit
    /// fewer arguments than ports (trailing ports are optional).
    pub is_macro: bool,
}

/// The single source of truth for stdlib input-port names and order. Each
/// entry is the ordered list of input-port variable names of the
/// correspondingly-named `stdlib⁚{name}` model; call argument `i` wires to
/// port `i`. `None` for any name that is not a stdlib module-function.
///
/// `name` is expected to already be a canonical stdlib model name (the
/// caller normalizes `delay`/`delayn`/`smthn` aliases via
/// `rewrite_alias_module_call` *before* consulting this).
pub(crate) fn stdlib_args(name: &str) -> Option<&'static [&'static str]> {
    let args: &'static [&'static str] = match name {
        "smth1" | "smth3" | "delay" | "delay1" | "delay3" | "trend" => {
            &["input", "delay_time", "initial_value"]
        }
        "npv" => &["stream", "discount_rate", "initial_value", "factor"],
        _ => {
            return None;
        }
    };
    Some(args)
}

/// Whether `canonical` names an opcode-backed engine *intrinsic* that the
/// Vensim MDL importer's builtin-rename can collide with a same-canonical-name
/// user macro.
///
/// This set is exactly `{init, previous}`, and it is the SINGLE SOURCE OF
/// TRUTH shared by the macro-recursion check here (`collect_called_macros`,
/// which must not record a false `self -> self` edge for such a wrap) and the
/// `builtins_visitor` macro-expansion precedence (which must resolve such a
/// call to the intrinsic, not recurse into the macro forever). Keeping one
/// predicate guarantees the two sites agree by construction.
///
/// Why these two names specifically (cross-ref #554):
/// - `ast/expr1.rs` lowers exactly two opcode-backed intrinsics by name:
///   `"init"` (`Init`, `LoadInitial`) and `"previous"` (`Previous`,
///   `LoadPrev`). They are the only builtins with the dedicated
///   per-call temp-arg routing in `builtins_visitor::BuiltinVisitor::walk`
///   (`init_needs_temp_arg` / `previous_needs_temp_arg`).
/// - The MDL importer (`mdl/xmile_compat.rs`) renames the Vensim builtins
///   `INITIAL` / `ACTIVE INITIAL` / `REINITIAL` to `INIT`, and desugars
///   `SAMPLE IF TRUE(...)` to `... PREVIOUS(SELF, init)`. Because the engine's
///   `Expr1` lowering recognizes only the short opcode names (`init`, not
///   `initial`), this rename is *necessary* -- and it manufactures a name
///   collision when a user macro is itself canonically named `init` or
///   `previous` and its body invokes that Vensim builtin (C-LEARN's
///   `:MACRO: INIT(x) ... INIT = INITIAL(x)`).
///
/// Other importer renames (e.g. `INTEGER -> INT`, `VMAX -> MAX`) target
/// ordinary `is_builtin_fn` builtins with no special walk() routing, so a
/// same-named-macro wrap of those is not a false *recursion* in the same way
/// and is intentionally NOT in this set. The renames that resolve to a
/// *stdlib module* (`DELAY N -> DELAYN`, `SMOOTH N -> SMTHN`, `DELAY FIXED
/// -> DELAY`, ...) are the same false-recursion class but with a different
/// termination argument; they live in the companion
/// [`is_renamed_stdlib_module_builtin`], and both feed the shared
/// [`is_renamed_builtin_macro_collision`] predicate (#554 follow-up).
pub(crate) fn is_renamed_opcode_intrinsic(canonical: &str) -> bool {
    matches!(canonical, "init" | "previous")
}

/// Whether `canonical` names a *stdlib-module-backed* builtin that the Vensim
/// MDL importer's builtin-rename can collide with a same-canonical-name user
/// macro -- the #554-follow-up companion of [`is_renamed_opcode_intrinsic`].
///
/// Membership is delegated to [`crate::builtins::is_stdlib_module_function`],
/// the single authoritative predicate for "this canonical name expands to a
/// `stdlib⁚...` model" (already used by `builtins_visitor`'s
/// `contains_module_call` and the walk's stdlib path). Delegating -- rather
/// than hand-maintaining a parallel list -- guarantees this suppression set
/// cannot drift from the names that actually resolve to a stdlib module: if a
/// name is ever added to / removed from stdlib-module backing, both the
/// resolution and this carve-out move together.
///
/// Why a *stdlib-module*-specific predicate (cross-ref #554, GH thyroid):
/// - The MDL importer (`mdl/xmile_compat.rs`) rewrites Vensim `DELAY N(...)`
///   to the single-token XMILE `DELAYN(...)`, `SMOOTH N(...)` to `SMTHN(...)`,
///   `DELAY FIXED` to `DELAY`, the `SMOOTH*`/`DELAY1`/`DELAY3` family to
///   `SMTH*`/`DELAY1`/`DELAY3`, `TREND`/`NPV` upcased. Every one of those
///   canonical names is recognized by `is_stdlib_module_function` and, after
///   `rewrite_alias_module_call`, resolves through `stdlib_descriptor` to a
///   distinct `stdlib⁚{name}` model (`stdlib⁚delay1`, `stdlib⁚smth3`, ...).
/// - So `:MACRO: DELAYN(...) ... DELAYN = DELAY N(...)`
///   (test/metasd/thyroid-dynamics/thyroid-2008-d.mdl) stores the datamodel
///   body `delayn = delayn(...)`: the `delayn` call is the *renamed builtin*,
///   not genuine self-recursion (Vensim macros cannot recurse and the source
///   wrote the distinct name `DELAY N`).
///
/// TERMINATION (verified against `builtins_visitor::BuiltinVisitor::walk`):
/// when Part B skips the macro-shadows-everything `resolve_macro` for such a
/// self-call, the call falls through to `rewrite_alias_module_call` then
/// `stdlib_descriptor`, producing a `Variable::Module` whose `model_name` is
/// `"stdlib⁚{name}"`. The U+205A separator is not a legal Vensim identifier
/// character and the importer never mints that prefix for a user model, so
/// the stdlib model is necessarily DISTINCT from the user macro's model
/// (whose name is the macro's own name). The stdlib model body is fixed
/// stdlib content that never references the user macro, so compiling it does
/// not re-enter the macro: the expansion terminates. (For the
/// `systems_rate`/`systems_leak`/`systems_conversion` members of
/// `is_stdlib_module_function` -- not Vensim builtins, no `stdlib_descriptor`
/// entry -- the fall-through is a terminating `UnknownBuiltin`, not infinite
/// recursion, so they are harmless to include and the Vensim importer cannot
/// produce them as a body call anyway.)
pub(crate) fn is_renamed_stdlib_module_builtin(canonical: &str) -> bool {
    crate::builtins::is_stdlib_module_function(canonical)
}

/// The single shared predicate the #554 / #554-follow-up self-edge
/// suppression keys off: `canonical` is a Vensim-MDL-importer-renamed builtin
/// (opcode-backed *or* stdlib-module-backed) that a same-canonical-name user
/// macro's body can legitimately reference without it being recursion.
///
/// Used by BOTH halves so they cannot drift (the #554 design property): Part
/// A (`collect_called_macros`, here) must not record a false `self -> self`
/// recursion edge for such a wrap, and Part B
/// (`builtins_visitor::BuiltinVisitor::walk`) must resolve such a self-call
/// to the builtin/intrinsic (an opcode for `init`/`previous`, the stdlib
/// module for `delayn`/`smthn`/...) rather than re-entering the macro
/// forever. Both terminate (see each member predicate's doc).
pub(crate) fn is_renamed_builtin_macro_collision(canonical: &str) -> bool {
    is_renamed_opcode_intrinsic(canonical) || is_renamed_stdlib_module_builtin(canonical)
}

/// Build a [`ModuleFunctionDescriptor`] for a stdlib module-function.
///
/// Called *after* `rewrite_alias_module_call` has normalized aliases, so
/// `name` is already a canonical stdlib model name. Returns `None` for any
/// name that is not a stdlib module-function. This preserves the existing
/// stdlib behavior exactly -- it just bundles the previously-scattered facts
/// (model name = `"stdlib⁚{name}"`, ports = [`stdlib_args`], output =
/// `"output"`, not a macro) into one struct.
pub(crate) fn stdlib_descriptor(name: &str) -> Option<ModuleFunctionDescriptor> {
    let ports = stdlib_args(name)?;
    Some(ModuleFunctionDescriptor {
        // U+205A (TWO DOT PUNCTUATION) is the engine-canonical model-name
        // separator used everywhere stdlib models are named (see
        // `stdlib.gen.rs`, `db.rs`, `builtins_visitor.rs`).
        model_name: format!("stdlib\u{205A}{name}"),
        parameter_ports: ports.iter().map(|s| s.to_string()).collect(),
        primary_output: "output".to_string(),
        additional_outputs: vec![],
        is_macro: false,
    })
}

/// A per-project macro registry, built once per compile from all of the
/// project's models. Answers "is this call name a project macro, and if so
/// what is its [`ModuleFunctionDescriptor`]?".
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Default, PartialEq, Eq, salsa::Update)]
pub(crate) struct MacroRegistry {
    /// canonical macro name -> descriptor
    macros: HashMap<String, ModuleFunctionDescriptor>,
}

impl MacroRegistry {
    /// Build the registry from all of a project's models, validating it.
    ///
    /// A model is a macro iff `model.macro_spec.is_some()`. Each macro model
    /// becomes a [`ModuleFunctionDescriptor`] keyed by its canonical name.
    ///
    /// Returns `Err` (a model-level [`Error`]) when the macro set is invalid:
    /// - **macros.AC5.3** two macro-marked models with the same canonical
    ///   name (`DuplicateMacroName`, message names the macro);
    /// - **macros.AC5.3** a macro's canonical name equals a non-macro
    ///   model's canonical name (`DuplicateMacroName`, message names the
    ///   collision);
    /// - **macros.AC5.2** a directly- or mutually-recursive macro
    ///   (`CircularDependency`, message names the cycle path).
    pub(crate) fn build(models: &[datamodel::Model]) -> Result<MacroRegistry, Error> {
        let mut macros: HashMap<String, ModuleFunctionDescriptor> = HashMap::new();

        // Pass 1: collect macro descriptors, rejecting duplicate macro names.
        for model in models {
            let Some(spec) = model.macro_spec.as_ref() else {
                continue;
            };
            let canonical = canonicalize(&model.name).into_owned();
            if macros.contains_key(&canonical) {
                return model_err!(
                    DuplicateMacroName,
                    format!("duplicate macro definition: {}", canonical)
                );
            }
            macros.insert(
                canonical.clone(),
                ModuleFunctionDescriptor {
                    model_name: model.name.clone(),
                    parameter_ports: spec.parameters.clone(),
                    primary_output: spec.primary_output.clone(),
                    additional_outputs: spec.additional_outputs.clone(),
                    is_macro: true,
                },
            );
        }

        // Pass 2: reject a macro whose canonical name collides with a
        // non-macro model's canonical name. (A macro model is registered as
        // an ordinary sub-model; a same-named user model would make the
        // `model_name` lookup ambiguous.)
        for model in models {
            if model.macro_spec.is_some() {
                continue;
            }
            let canonical = canonicalize(&model.name).into_owned();
            if macros.contains_key(&canonical) {
                return model_err!(
                    DuplicateMacroName,
                    format!("macro name collides with model name: {}", canonical)
                );
            }
        }

        // Pass 3: reject direct/mutual recursion. Build the macro call graph
        // (an edge `this_macro -> called_macro` for every macro the body
        // invokes) and run cycle detection over it.
        let registry = MacroRegistry { macros };
        registry.check_for_recursion(models)?;
        Ok(registry)
    }

    /// Look up a call name (canonicalized) in the macro registry.
    pub(crate) fn resolve_macro(&self, call_name: &str) -> Option<&ModuleFunctionDescriptor> {
        let canonical = canonicalize(call_name);
        self.macros.get(canonical.as_ref())
    }

    /// Detect a recursion cycle among the registered macros.
    ///
    /// For each macro model, every body variable's equation text is parsed
    /// (`Expr0::new(text, LexerType::Equation)`) and walked for `App(name,
    /// ...)` nodes whose canonicalized `name` is another registered macro;
    /// each such reference is an edge `this_macro -> called_macro`. A cycle
    /// in that graph (including a self-edge) is a `CircularDependency` whose
    /// message names the cycle path.
    fn check_for_recursion(&self, models: &[datamodel::Model]) -> Result<(), Error> {
        // adjacency: canonical macro name -> set of canonical macro names it
        // calls. A BTreeSet keeps edge iteration order deterministic so a
        // reported cycle path is stable across runs.
        let mut edges: HashMap<String, std::collections::BTreeSet<String>> = HashMap::new();
        for name in self.macros.keys() {
            edges.entry(name.clone()).or_default();
        }

        for model in models {
            if model.macro_spec.is_none() {
                continue;
            }
            let from = canonicalize(&model.name).into_owned();
            // A macro could have been dropped from `self.macros` only if it
            // were a duplicate, which `build` already rejected; defensively
            // skip any model not in the registry rather than panicking. This
            // `continue` fires BEFORE `collect_called_macros` is reached, so
            // the (impossible) dropped model contributes no edges at all --
            // no self-edge, and `from` is never used as the #554
            // `enclosing` self-edge carve-out for it -- which is correct: a
            // model absent from the registry must not appear in the call
            // graph.
            if !self.macros.contains_key(&from) {
                continue;
            }
            for var in &model.variables {
                let Some(equation) = var.get_equation() else {
                    continue;
                };
                let text = equation.source_text();
                let Ok(Some(ast)) = Expr0::new(&text, LexerType::Equation) else {
                    // A body equation that does not parse is a per-variable
                    // diagnostic surfaced later by the normal compile path;
                    // it is not the registry's job to report it, and it
                    // cannot introduce a (resolvable) macro call edge.
                    continue;
                };
                let mut called: std::collections::BTreeSet<String> = Default::default();
                collect_called_macros(&ast, &from, &self.macros, &mut called);
                if let Some(set) = edges.get_mut(&from) {
                    set.extend(called);
                }
            }
        }

        if let Some(cycle) = find_cycle(&edges) {
            return model_err!(
                CircularDependency,
                format!("recursive macro: {}", cycle.join(" -> "))
            );
        }
        Ok(())
    }
}

/// Walk an `Expr0` AST and record every `App` whose canonicalized function
/// name is a key in `macros` (i.e. resolves to a registered macro), producing
/// the macro-call edges out of `enclosing` (the canonical name of the macro
/// whose body this AST is).
///
/// #554 (+ follow-up) exception (precisely scoped): a call is NOT recorded as
/// a macro edge when the called name canonicalizes to `enclosing`'s OWN
/// canonical name AND that name is a Vensim-MDL-importer-renamed builtin --
/// opcode-backed (`init`/`previous`) *or* stdlib-module-backed
/// (`delayn`/`smthn`/`delay`/...) -- per the shared
/// [`is_renamed_builtin_macro_collision`]. Such a call is the importer's
/// *renamed builtin* (`INITIAL` -> `INIT`, `SAMPLE IF TRUE` -> `PREVIOUS`,
/// `DELAY N` -> `DELAYN`, `SMOOTH N` -> `SMTHN`, ...), not genuine
/// self-recursion: Vensim macros cannot recurse, and the source wrote the
/// distinct builtin name. Resolving it to the builtin terminates -- the
/// `builtins_visitor` half, sharing the SAME predicate, makes the call
/// resolve to the opcode (`init`/`previous`) or the distinct `stdlib⁚...`
/// module (`delayn`/...) rather than re-entering the macro -- so recording an
/// `enclosing -> enclosing` self-edge here would be the #554-class false
/// positive that fails the *whole* `MacroRegistry::build` and un-shadows the
/// project's other macros (the cascade).
///
/// The suppression is strictly `called-canonical == enclosing AND
/// is_renamed_builtin_macro_collision(called-canonical)`: a *different* macro
/// that merely happens to be named after a builtin still produces a real edge
/// (so `init -> previous -> init` and `delayn -> smthn -> delayn`, A->B->A by
/// builtin names, are still rejected cycles), and a genuinely self-recursive
/// macro whose name is *not* a renamed builtin (`foo = foo(x,y)`) still
/// records its self-edge (macros.AC5.2 unweakened).
fn collect_called_macros(
    expr: &Expr0,
    enclosing: &str,
    macros: &HashMap<String, ModuleFunctionDescriptor>,
    out: &mut std::collections::BTreeSet<String>,
) {
    use crate::ast::IndexExpr0;
    use Expr0::*;
    match expr {
        Const(_, _, _) => {}
        Var(_, _) => {}
        App(UntypedBuiltinFn(func, args), _) => {
            let canonical = canonicalize(func);
            // #554 (+ follow-up): suppress ONLY the same-named-renamed-builtin
            // self-edge. Any other macro-resolving call (including a self-call
            // of a macro whose name is NOT a renamed builtin, preserving
            // macros.AC5.2) records its edge.
            let is_renamed_builtin_self_wrap = canonical.as_ref() == enclosing
                && is_renamed_builtin_macro_collision(canonical.as_ref());
            if !is_renamed_builtin_self_wrap && macros.contains_key(canonical.as_ref()) {
                out.insert(canonical.into_owned());
            }
            for arg in args {
                collect_called_macros(arg, enclosing, macros, out);
            }
        }
        Subscript(_, args, _) => {
            for idx in args {
                match idx {
                    IndexExpr0::Range(l, r, _) => {
                        collect_called_macros(l, enclosing, macros, out);
                        collect_called_macros(r, enclosing, macros, out);
                    }
                    IndexExpr0::Expr(e) => collect_called_macros(e, enclosing, macros, out),
                    IndexExpr0::Wildcard(_)
                    | IndexExpr0::StarRange(_, _)
                    | IndexExpr0::DimPosition(_, _) => {}
                }
            }
        }
        Op1(_, r, _) => collect_called_macros(r, enclosing, macros, out),
        Op2(_, l, r, _) => {
            collect_called_macros(l, enclosing, macros, out);
            collect_called_macros(r, enclosing, macros, out);
        }
        If(cond, t, f, _) => {
            collect_called_macros(cond, enclosing, macros, out);
            collect_called_macros(t, enclosing, macros, out);
            collect_called_macros(f, enclosing, macros, out);
        }
    }
}

/// Detect a cycle in the macro call graph via depth-first search with an
/// explicit recursion stack (the standard back-edge algorithm). Returns the
/// cycle as a path `[a, b, ..., a]` (the repeated node closes the cycle), or
/// `None` if the graph is acyclic. A self-edge `a -> a` yields `[a, a]`.
///
/// Node visitation and edge iteration are over sorted keys / `BTreeSet`s so
/// the reported path is deterministic regardless of `HashMap` ordering.
fn find_cycle(edges: &HashMap<String, std::collections::BTreeSet<String>>) -> Option<Vec<String>> {
    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }

    let mut color: HashMap<&str, Color> = HashMap::new();
    for k in edges.keys() {
        color.insert(k.as_str(), Color::White);
    }

    // Iterative DFS so a deep macro graph cannot overflow the stack. Each
    // stack frame tracks the node and an iterator position over its sorted
    // successors; `path` mirrors the current Gray chain for cycle reporting.
    let mut roots: Vec<&str> = edges.keys().map(|s| s.as_str()).collect();
    roots.sort_unstable();

    for root in roots {
        if color.get(root).copied() != Some(Color::White) {
            continue;
        }
        // (node, successors-as-sorted-vec, next-index-into-successors)
        let succs: Vec<&str> = edges
            .get(root)
            .map(|s| s.iter().map(|x| x.as_str()).collect())
            .unwrap_or_default();
        let mut stack: Vec<(&str, Vec<&str>, usize)> = vec![(root, succs, 0)];
        let mut path: Vec<&str> = vec![root];
        color.insert(root, Color::Gray);

        while let Some(&mut (node, ref succs, ref mut idx)) = stack.last_mut() {
            if *idx < succs.len() {
                let next = succs[*idx];
                *idx += 1;
                match color.get(next).copied() {
                    Some(Color::Gray) => {
                        // Back-edge: close the cycle at `next`.
                        let start = path.iter().position(|&n| n == next).unwrap_or(0);
                        let mut cycle: Vec<String> =
                            path[start..].iter().map(|s| s.to_string()).collect();
                        cycle.push(next.to_string());
                        return Some(cycle);
                    }
                    Some(Color::White) | None => {
                        let next_succs: Vec<&str> = edges
                            .get(next)
                            .map(|s| s.iter().map(|x| x.as_str()).collect())
                            .unwrap_or_default();
                        color.insert(next, Color::Gray);
                        path.push(next);
                        stack.push((next, next_succs, 0));
                    }
                    Some(Color::Black) => {}
                }
            } else {
                color.insert(node, Color::Black);
                path.pop();
                stack.pop();
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel::{Aux, Equation, MacroSpec, Model, Variable};

    /// A non-macro scalar aux body variable.
    fn aux(ident: &str, equation: &str) -> Variable {
        Variable::Aux(Aux {
            ident: ident.to_string(),
            equation: Equation::Scalar(equation.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })
    }

    /// An ordinary (non-macro) model with the given name.
    fn plain_model(name: &str) -> Model {
        Model {
            name: name.to_string(),
            sim_specs: None,
            variables: vec![aux("x", "1")],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }
    }

    /// A macro-marked model: `name(params...)` whose single body variable is
    /// `name = <body_equation>` (the primary output).
    fn macro_model(name: &str, params: &[&str], body_equation: &str) -> Model {
        let mut variables = vec![aux(name, body_equation)];
        // Synthesize a trivial port aux per parameter, mirroring
        // `Model::new_macro` (the registry only reads `macro_spec`, but a
        // realistic fixture keeps the port variables present).
        for p in params {
            variables.push(aux(p, "0"));
        }
        Model {
            name: name.to_string(),
            sim_specs: None,
            variables,
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: Some(MacroSpec {
                parameters: params.iter().map(|s| s.to_string()).collect(),
                primary_output: name.to_string(),
                additional_outputs: vec![],
            }),
        }
    }

    // --- stdlib_descriptor ------------------------------------------------

    #[test]
    fn stdlib_descriptor_hit_returns_ports_and_output() {
        let d = stdlib_descriptor("smth1").expect("smth1 is a stdlib module-function");
        assert_eq!(d.model_name, "stdlib\u{205A}smth1");
        assert_eq!(
            d.parameter_ports,
            vec![
                "input".to_string(),
                "delay_time".to_string(),
                "initial_value".to_string()
            ]
        );
        assert_eq!(d.primary_output, "output");
        assert_eq!(d.additional_outputs, Vec::<String>::new());
        assert!(!d.is_macro, "stdlib descriptors are not macros");
    }

    #[test]
    fn stdlib_descriptor_npv_has_four_ports() {
        let d = stdlib_descriptor("npv").expect("npv is a stdlib module-function");
        assert_eq!(d.model_name, "stdlib\u{205A}npv");
        assert_eq!(
            d.parameter_ports,
            vec![
                "stream".to_string(),
                "discount_rate".to_string(),
                "initial_value".to_string(),
                "factor".to_string()
            ]
        );
        assert_eq!(d.primary_output, "output");
    }

    #[test]
    fn stdlib_descriptor_miss_returns_none() {
        assert!(stdlib_descriptor("not_a_thing").is_none());
    }

    // --- MacroRegistry::build + resolve_macro -----------------------------

    #[test]
    fn build_then_resolve_returns_macro_descriptor() {
        let models = vec![
            plain_model("main"),
            macro_model("mymacro", &["a", "b"], "a * b"),
        ];
        let registry = MacroRegistry::build(&models).expect("valid macro project builds");

        let d = registry
            .resolve_macro("mymacro")
            .expect("mymacro resolves to its descriptor");
        assert!(d.is_macro);
        assert_eq!(d.model_name, "mymacro");
        assert_eq!(d.parameter_ports, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(d.primary_output, "mymacro");
        assert_eq!(d.additional_outputs, Vec::<String>::new());
    }

    #[test]
    fn resolve_macro_canonicalizes_the_lookup_key() {
        let models = vec![macro_model("my_macro", &["a"], "a")];
        let registry = MacroRegistry::build(&models).expect("builds");
        // Spaces canonicalize to underscores and uppercase to lowercase, so
        // a call written `MY MACRO` must resolve to `my_macro`.
        assert!(registry.resolve_macro("MY MACRO").is_some());
        assert!(registry.resolve_macro("my_macro").is_some());
    }

    #[test]
    fn resolve_macro_of_non_macro_name_is_none() {
        let models = vec![plain_model("main"), macro_model("mymacro", &["a"], "a")];
        let registry = MacroRegistry::build(&models).expect("builds");
        assert!(registry.resolve_macro("not_a_macro").is_none());
    }

    #[test]
    fn macro_named_like_a_stdlib_function_still_resolves_to_the_macro() {
        // The *precedence* (macro shadows stdlib) is enforced in the
        // BuiltinVisitor walk ordering; here we only confirm the registry
        // itself stores and returns the macro descriptor for `smth1`.
        let models = vec![macro_model("smth1", &["x"], "x + 1")];
        let registry = MacroRegistry::build(&models).expect("builds");
        let d = registry
            .resolve_macro("smth1")
            .expect("a macro named smth1 must resolve to the macro");
        assert!(d.is_macro);
        assert_eq!(d.model_name, "smth1");
        assert_eq!(d.parameter_ports, vec!["x".to_string()]);
    }

    // --- macros.AC5.3: duplicate macro name / model collision -------------

    #[test]
    fn ac5_3_two_macros_named_foo_is_a_build_error_naming_foo() {
        let models = vec![
            macro_model("foo", &["a"], "a"),
            macro_model("foo", &["b"], "b + 1"),
        ];
        let err = MacroRegistry::build(&models)
            .expect_err("two macros named foo must fail registry build");
        let details = err.get_details().unwrap_or_default();
        assert!(
            details.contains("foo"),
            "the duplicate-macro error must name the macro: {:?}",
            details
        );
    }

    #[test]
    fn ac5_3_macro_named_main_colliding_with_main_model_is_a_build_error() {
        let models = vec![plain_model("main"), macro_model("main", &["a"], "a")];
        let err = MacroRegistry::build(&models)
            .expect_err("a macro named `main` colliding with the main model must fail");
        let details = err.get_details().unwrap_or_default();
        assert!(
            details.contains("main"),
            "the collision error must name the collision: {:?}",
            details
        );
    }

    // --- macros.AC5.2: recursion cycle ------------------------------------

    #[test]
    fn ac5_2_self_recursive_macro_is_circular_dependency() {
        // `a`'s body calls `a` -> a self-edge in the call graph.
        let models = vec![macro_model("a", &["x"], "a(x) + 1")];
        let err = MacroRegistry::build(&models)
            .expect_err("a self-recursive macro must fail registry build");
        assert_eq!(
            err.code,
            crate::common::ErrorCode::CircularDependency,
            "a recursion cycle must be reported as CircularDependency"
        );
        let details = err.get_details().unwrap_or_default();
        assert!(
            details.contains('a'),
            "the cycle error must name the macro path: {:?}",
            details
        );
    }

    #[test]
    fn ac5_2_mutually_recursive_a_b_a_is_circular_dependency() {
        // a -> b -> a
        let models = vec![
            macro_model("a", &["x"], "b(x)"),
            macro_model("b", &["y"], "a(y)"),
        ];
        let err = MacroRegistry::build(&models)
            .expect_err("a mutually-recursive A/B pair must fail registry build");
        assert_eq!(err.code, crate::common::ErrorCode::CircularDependency);
    }

    #[test]
    fn ac5_2_a_calls_b_no_cycle_builds_ok() {
        // The `macro_cross_reference` shape: a -> b, no back-edge.
        let models = vec![
            plain_model("main"),
            macro_model("a", &["x"], "b(x) * 2"),
            macro_model("b", &["y"], "y + 1"),
        ];
        let registry = MacroRegistry::build(&models)
            .expect("a non-recursive macro-calls-macro project must build");
        assert!(registry.resolve_macro("a").is_some());
        assert!(registry.resolve_macro("b").is_some());
    }

    // --- #554: a macro that wraps a same-canonical-name opcode intrinsic ---
    //
    // The MDL importer must rename the Vensim `INITIAL` builtin to `INIT`
    // (`xmile_compat.rs::format_function_name`; the engine's `Expr1` lowering
    // recognizes only the opcode name `init`, not `initial`). So C-LEARN's
    // uninvoked `:MACRO: INIT(x) ... INIT = INITIAL(x)` is stored as the
    // datamodel macro body `init = init(x)`. The `init` call there is the
    // renamed intrinsic, NOT a recursive call -- Vensim macros cannot recurse
    // and the source wrote the distinct name `INITIAL`. Recording an
    // `init -> init` self-edge for it is the #554 false positive; it failed
    // the whole `MacroRegistry::build` (and the empty registry then un-shadowed
    // every other macro -- the cascade).

    #[test]
    fn issue_554_macro_wrapping_same_named_init_intrinsic_builds_ok() {
        // Exactly the #554 shape: a macro whose canonical name (`init`) equals
        // an opcode-backed engine intrinsic, whose body is `init = init(x)`
        // (the importer-renamed `INITIAL(x)`), PLUS another macro. The
        // registry must build (no false `init -> init` CircularDependency) and
        // BOTH macros must resolve, proving the cascade that blocked C-LEARN's
        // other macros (SSHAPE/SAMPLE UNTIL/RAMP FROM TO) is gone.
        let models = vec![
            plain_model("main"),
            macro_model("init", &["x"], "init(x)"),
            macro_model("sshape", &["xin", "profile"], "xin + profile"),
        ];
        let registry = MacroRegistry::build(&models).expect(
            "a macro wrapping the same-named `init` opcode intrinsic is NOT \
             recursive (#554): the body's `init(x)` is the importer-renamed \
             `INITIAL(x)` builtin, which resolves to the intrinsic and \
             terminates -- the registry must build",
        );
        assert!(
            registry.resolve_macro("init").is_some(),
            "the `init` macro itself must still be registered"
        );
        assert!(
            registry.resolve_macro("sshape").is_some(),
            "the OTHER macro must resolve -- the #554 false self-edge must \
             not fail the whole registry and un-shadow sibling macros"
        );
    }

    #[test]
    fn issue_554_macro_wrapping_same_named_previous_intrinsic_builds_ok() {
        // The `previous` analogue: Vensim `SAMPLE IF TRUE(cond,input,init)`
        // desugars to `... PREVIOUS(SELF, init) ...` (`xmile_compat.rs`), so a
        // macro named `PREVIOUS` whose body uses it stores a same-named
        // `previous(...)` call. `previous` is the other opcode-backed
        // intrinsic with dedicated walk() routing, so it is in the same
        // suppression set as `init`.
        let models = vec![
            plain_model("main"),
            macro_model("previous", &["x"], "previous(x, 0)"),
        ];
        let registry = MacroRegistry::build(&models).expect(
            "a macro wrapping the same-named `previous` opcode intrinsic is \
             NOT recursive (#554)",
        );
        assert!(registry.resolve_macro("previous").is_some());
    }

    #[test]
    fn issue_554_exception_does_not_weaken_ac5_2_genuine_self_recursion() {
        // CRITICAL guard (macros.AC5.2 must stay unweakened): a macro `foo`
        // whose body is `foo = foo(x)` where `foo` is NOT an opcode intrinsic
        // is GENUINE self-recursion (Vensim wrote the macro name itself, not a
        // renamed builtin) and MUST still be a CircularDependency. The #554
        // exception is scoped to the opcode-intrinsic-same-name case only.
        let models = vec![macro_model("foo", &["x"], "foo(x)")];
        let err = MacroRegistry::build(&models).expect_err(
            "a genuinely self-recursive non-intrinsic macro must STILL fail \
             registry build -- the #554 exception must not weaken AC5.2",
        );
        assert_eq!(
            err.code,
            crate::common::ErrorCode::CircularDependency,
            "genuine self-recursion must remain CircularDependency"
        );
        let details = err.get_details().unwrap_or_default();
        assert!(
            details.contains("foo"),
            "the cycle error must still name the recursive macro: {:?}",
            details
        );
    }

    #[test]
    fn issue_554_exception_does_not_weaken_ac5_2_mutual_recursion() {
        // The mutual-recursion guard: A -> B -> A by non-intrinsic names must
        // still be rejected. (A separate guard from the inline `ac5_2_*`
        // tests, kept adjacent to the #554 exception so a future loosening of
        // the exception that also breaks mutual recursion is caught here.)
        let models = vec![
            macro_model("alpha", &["x"], "beta(x)"),
            macro_model("beta", &["y"], "alpha(y)"),
        ];
        let err = MacroRegistry::build(&models)
            .expect_err("non-intrinsic mutual recursion must STILL fail");
        assert_eq!(err.code, crate::common::ErrorCode::CircularDependency);
    }

    #[test]
    fn issue_554_macro_calling_a_different_intrinsic_named_macro_is_recursion() {
        // Scope guard: the exception is `call-canonical == enclosing-canonical
        // AND in the intrinsic set`. A macro `init` that calls a DIFFERENT
        // macro which is also named after an intrinsic (`previous`) is a real
        // macro-to-macro edge (`init -> previous`), and if `previous` calls
        // `init` back, that A->B->A cycle MUST still be rejected. Only the
        // *self*-edge to the *same-named* intrinsic is suppressed.
        let models = vec![
            macro_model("init", &["x"], "previous(x, 0)"),
            macro_model("previous", &["y"], "init(y)"),
        ];
        let err = MacroRegistry::build(&models).expect_err(
            "init -> previous -> init is a genuine macro cycle and must fail \
             even though both names are intrinsic names (the suppression is \
             self-edge-only)",
        );
        assert_eq!(err.code, crate::common::ErrorCode::CircularDependency);
    }

    // --- #554 follow-up: a macro wrapping a same-canonical-name
    //     STDLIB-MODULE-backed renamed builtin (the `DELAY N` / thyroid case) -
    //
    // The MDL importer rewrites Vensim `DELAY N(input,dt,init,n)` to the XMILE
    // `DELAYN(input,dt,n,init)` (`mdl/xmile_compat.rs`). So
    // thyroid-2008-d.mdl's `:MACRO: DELAYN(...) ... DELAYN = DELAY N(...)` is
    // stored as the datamodel macro body `delayn = delayn(...)`. The `delayn`
    // call there is the renamed builtin, NOT a recursive call (Vensim macros
    // cannot recurse and the source wrote the distinct name `DELAY N`).
    // Recording a `delayn -> delayn` self-edge for it is a #554-class false
    // positive; it failed the whole `MacroRegistry::build` (the empty registry
    // then un-shadowed every other macro -- the same cascade as #554).
    //
    // UNLIKE #554's `init`/`previous` (opcode-backed, falls through to a
    // terminal LoadInitial/LoadPrev opcode), `delayn` is stdlib-module-backed:
    // skipping the macro resolve makes the call fall through to
    // `rewrite_alias_module_call`/`stdlib_descriptor`, resolving to a
    // `stdlib⁚delay1`/`stdlib⁚delay3` MODULE -- a DISTINCT fixed model whose
    // body never references the user `delayn` macro, so it terminates.
    //
    // NB: the importer ALREADY collapses Vensim `DELAY N` to the single-token
    // XMILE `DELAYN` before the datamodel macro body is formed (verified: the
    // thyroid macro body datamodel `source_text()` is
    // `DELAYN(Input, DelayTime, Order, Init)`), so the fixture body is the
    // single token `delayn(a, b)` (canonical `delayn`), NOT `delay n(...)`.

    #[test]
    fn issue_554_followup_macro_wrapping_same_named_delayn_builtin_builds_ok() {
        // Exactly the thyroid shape: a macro whose canonical name (`delayn`)
        // equals a stdlib-module-backed renamed builtin, whose body is
        // `delayn = delayn(a, b)` (the importer-renamed `DELAY N(...)`; >=2
        // params per GH#553's 1-arg-call->LOOKUP heuristic), PLUS a sibling
        // macro. The registry must build (no false `delayn -> delayn`
        // CircularDependency) and BOTH macros must resolve, proving the
        // #554-class cascade that blocked thyroid's other macros is gone.
        let models = vec![
            plain_model("main"),
            macro_model("delayn", &["a", "b"], "delayn(a, b)"),
            macro_model("pipeline", &["input", "delay_time"], "input + delay_time"),
        ];
        let registry = MacroRegistry::build(&models).expect(
            "a macro wrapping the same-named stdlib-module-backed `DELAY N` \
             builtin is NOT recursive (#554 follow-up): the body's \
             `delayn(...)` is the importer-renamed `DELAY N(...)` builtin, \
             which resolves to the stdlib delay module and terminates -- the \
             registry must build",
        );
        assert!(
            registry.resolve_macro("delayn").is_some(),
            "the `delayn` macro itself must still be registered"
        );
        assert!(
            registry.resolve_macro("pipeline").is_some(),
            "the OTHER macro must resolve -- the #554-class false self-edge \
             must not fail the whole registry and un-shadow sibling macros"
        );
    }

    #[test]
    fn issue_554_followup_macro_wrapping_same_named_smthn_builtin_builds_ok() {
        // The `smthn` analogue: Vensim `SMOOTH N` -> XMILE `SMTHN`
        // (`mdl/xmile_compat.rs`), also stdlib-module-backed
        // (`is_stdlib_module_function` matches `smthn`; resolves to
        // `stdlib⁚smth1`/`smth3`). A macro named `SMTHN` whose body uses it
        // is the same renamed-stdlib-module collision class.
        let models = vec![
            plain_model("main"),
            macro_model("smthn", &["a", "b"], "smthn(a, b)"),
        ];
        let registry = MacroRegistry::build(&models).expect(
            "a macro wrapping the same-named stdlib-module-backed `smth n` \
             builtin is NOT recursive (#554 follow-up)",
        );
        assert!(registry.resolve_macro("smthn").is_some());
    }

    #[test]
    fn issue_554_followup_does_not_weaken_ac5_2_genuine_self_recursion() {
        // CRITICAL guard (macros.AC5.2 must stay unweakened): a macro `foo`
        // whose body is `foo = foo(x, y)` where `foo` is NEITHER an opcode
        // intrinsic NOR a stdlib-module-backed renamed builtin is GENUINE
        // self-recursion (Vensim wrote the macro name itself, not a renamed
        // builtin) and MUST still be a CircularDependency. The #554-follow-up
        // exception is scoped to the renamed-builtin same-name case only.
        let models = vec![macro_model("foo", &["x", "y"], "foo(x, y)")];
        let err = MacroRegistry::build(&models).expect_err(
            "a genuinely self-recursive non-builtin macro must STILL fail \
             registry build -- the #554-follow-up exception must not weaken \
             AC5.2",
        );
        assert_eq!(
            err.code,
            crate::common::ErrorCode::CircularDependency,
            "genuine self-recursion must remain CircularDependency"
        );
        let details = err.get_details().unwrap_or_default();
        assert!(
            details.contains("foo"),
            "the cycle error must still name the recursive macro: {:?}",
            details
        );
    }

    #[test]
    fn issue_554_followup_macro_calling_a_different_stdlib_named_macro_is_recursion() {
        // Scope guard mirroring the opcode-intrinsic one: the suppression is
        // `call-canonical == enclosing-canonical AND in the renamed-builtin
        // set`. A macro `delayn` that calls a DIFFERENT macro also named after
        // a stdlib builtin (`smthn`) is a real macro-to-macro edge
        // (`delayn -> smthn`); if `smthn` calls `delayn` back, that A->B->A
        // cycle MUST still be rejected. Only the *self*-edge to the
        // *same-named* renamed builtin is suppressed.
        let models = vec![
            macro_model("delayn", &["x", "y"], "smthn(x, y)"),
            macro_model("smthn", &["p", "q"], "delayn(p, q)"),
        ];
        let err = MacroRegistry::build(&models).expect_err(
            "delayn -> smthn -> delayn is a genuine macro cycle and must fail \
             even though both names are stdlib-builtin names (the suppression \
             is self-edge-only)",
        );
        assert_eq!(err.code, crate::common::ErrorCode::CircularDependency);
    }
}
