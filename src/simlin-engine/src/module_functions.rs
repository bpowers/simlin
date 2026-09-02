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
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ModuleFunctionDescriptor {
    /// The `datamodel::Model.name` of the target model -- `"stdlib⁚smth1"`
    /// for a stdlib function, or the macro's canonical model name.
    pub model_name: String,
    /// Ordered input-port variable names; call argument `i` wires to port `i`.
    /// A macro call must supply exactly this many arguments; a stdlib call may
    /// supply fewer, leaving the trailing ports unwired.
    pub parameter_ports: Vec<String>,
    /// The body variable whose value the call expression is replaced with.
    pub primary_output: String,
    /// `:`-list additional output ports (empty for stdlib and for
    /// single-output macros).
    pub additional_outputs: Vec<String>,
    /// A *genuine passthrough* macro: a single-parameter, single-output macro
    /// whose primary-output body is exactly `out = BUILTIN(param)` with
    /// `BUILTIN` canonicalizing to the macro's own renamed-builtin-collision
    /// name (`:MACRO: INIT(x) = INITIAL(x)` -> `init = init(x)`). A call to
    /// one lowers as the builtin it names ([`MacroCallResolution::Passthrough`])
    /// rather than instantiating the macro model, which the collapse loses
    /// nothing by since the body did no work beyond the bare call. Classified
    /// once by [`MacroRegistry::build`], the only place the body AST is
    /// available; always `false` for a stdlib descriptor.
    pub passthrough: bool,
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

/// Whether `canonical` names an opcode-backed engine intrinsic that the Vensim
/// MDL importer's builtin rename can make collide with a same-canonical-name
/// user macro: exactly `{init, previous}`.
///
/// `ast/expr1.rs` lowers exactly two opcode-backed intrinsics by name --
/// `init` (`LoadInitial`) and `previous` (`LoadPrev`) -- and recognizes only
/// those short names, so the MDL importer (`mdl/xmile_compat.rs`) must rename
/// Vensim's `INITIAL` / `ACTIVE INITIAL` / `REINITIAL` to `INIT` and desugar
/// `SAMPLE IF TRUE(...)` to `... PREVIOUS(SELF, init)`. That rename is what
/// makes a user macro canonically named `init` or `previous` whose body
/// invokes the Vensim builtin read as a self-call (C-LEARN's `:MACRO: INIT(x)
/// ... INIT = INITIAL(x)`, GH #554). Other importer renames (`INTEGER -> INT`,
/// `VMAX -> MAX`) target ordinary `is_builtin_fn` builtins with no dedicated
/// routing and are deliberately NOT in this set.
pub(crate) fn is_renamed_opcode_intrinsic(canonical: &str) -> bool {
    matches!(canonical, "init" | "previous")
}

/// Whether `canonical` names a stdlib-module-backed builtin that the Vensim MDL
/// importer's builtin rename can make collide with a same-canonical-name user
/// macro -- the stdlib companion of [`is_renamed_opcode_intrinsic`].
///
/// Delegates to [`crate::builtins::is_stdlib_module_function`], the one
/// predicate for "this canonical name expands to a `stdlib⁚...` model", so
/// this set cannot drift from the names that actually resolve to a stdlib
/// module. The importer rewrites `DELAY N(...)` to the single-token
/// `DELAYN(...)`, `SMOOTH N` to `SMTHN`, `DELAY FIXED` to `DELAY`, and the
/// `SMOOTH*`/`DELAY1`/`DELAY3`/`TREND`/`NPV` family to their stdlib names, so
/// `:MACRO: DELAYN(...) ... DELAYN = DELAY N(...)`
/// (test/metasd/thyroid-dynamics/thyroid-2008-d.mdl) stores the body
/// `delayn = delayn(...)`: the call is the renamed builtin, not recursion.
///
/// Routing such a self-call to the builtin terminates: it reaches
/// `rewrite_alias_module_call` then `stdlib_descriptor`, whose target
/// `stdlib⁚{name}` is necessarily distinct from the user macro's model (the
/// U+205A separator is not a legal Vensim identifier character) and whose fixed
/// body never references the macro. The `systems_*` members of the predicate
/// have no `stdlib_descriptor` entry and fall through to a terminating
/// `UnknownBuiltin`; the Vensim importer cannot produce them as a body call.
pub(crate) fn is_renamed_stdlib_module_builtin(canonical: &str) -> bool {
    crate::builtins::is_stdlib_module_function(canonical)
}

/// Whether `canonical` is a Vensim-MDL-importer-renamed builtin -- opcode-backed
/// or stdlib-module-backed -- that a same-canonical-name user macro's body can
/// legitimately call without it being recursion (Vensim macros cannot recurse;
/// the source wrote the distinct builtin name). Read only by
/// [`MacroRegistry::resolve_call`].
pub(crate) fn is_renamed_builtin_macro_collision(canonical: &str) -> bool {
    is_renamed_opcode_intrinsic(canonical) || is_renamed_stdlib_module_builtin(canonical)
}

/// Is a macro a *genuine passthrough* of a renamed builtin (see
/// [`ModuleFunctionDescriptor::passthrough`])? Pure and structural: `true` iff
/// ALL of
///
/// 1. the macro has exactly one parameter (`parameter_ports.len() == 1`);
/// 2. the macro has no additional outputs (a multi-output `:`-list macro
///    delivers more than the primary output, so it cannot collapse to one
///    builtin);
/// 3. the primary-output body AST is exactly `App(BUILTIN, [arg])` -- a single
///    call with a single argument;
/// 4. `arg` is exactly `Var(the sole parameter)` (the bare parameter, NOT an
///    expression like `param * 2`, which would do work the collapse drops);
/// 5. `canonicalize(call) == canonicalize(macro_name)` (a self-call -- the
///    form the importer's `INITIAL` -> `INIT` rename produces); and
/// 6. `is_renamed_builtin_macro_collision(canonicalize(call))`, so the
///    builtin lowering the call falls through to is a real opcode-backed
///    builtin (`init`/`previous`) or stdlib module rather than `UnknownBuiltin`.
///
/// The strictness of (3)-(6) guarantees the collapse cannot misfire on a
/// non-passthrough macro that merely shares a builtin name (`INIT = INIT(x) +
/// 1`, `INIT = INIT(x * 2)`): such a macro keeps expanding as a module.
pub(crate) fn classify_passthrough(
    macro_name: &str,
    parameter_ports: &[String],
    additional_outputs: &[String],
    primary_output_body_ast: &Expr0,
) -> bool {
    // (1) exactly one parameter; (2) no additional outputs.
    let [sole_param] = parameter_ports else {
        return false;
    };
    if !additional_outputs.is_empty() {
        return false;
    }

    // (3) the body is exactly a single one-argument call, (4) of the bare sole
    // parameter (canonical match, so a case/whitespace variant of the formal
    // parameter still counts).
    let Expr0::App(UntypedBuiltinFn(call, args), _) = primary_output_body_ast else {
        return false;
    };
    let [Expr0::Var(arg_ident, _)] = args.as_slice() else {
        return false;
    };
    if canonicalize(arg_ident.as_str()) != canonicalize(sole_param) {
        return false;
    }

    // (5) a self-call, (6) of a renamed-builtin collision.
    let call_canonical = canonicalize(call);
    call_canonical == canonicalize(macro_name)
        && is_renamed_builtin_macro_collision(call_canonical.as_ref())
}

/// Bridge from a datamodel macro `Model`/`MacroSpec` to the pure
/// [`classify_passthrough`]: locate the primary-output body variable, parse its
/// (single, scalar) equation, and classify. `false` when the primary output is
/// missing, has no equation, is an arrayed multi-formula body (a passthrough's
/// `out = BUILTIN(param)` is a single scalar formula), or fails to parse.
///
/// This is the only place each macro body equation is parsed for
/// classification, so the (transient) body AST never needs to escape registry
/// build.
fn classify_macro_passthrough(model: &datamodel::Model, spec: &datamodel::MacroSpec) -> bool {
    let primary_canonical = canonicalize(&spec.primary_output);
    let Some(primary_var) = model
        .variables
        .iter()
        .find(|v| canonicalize(v.get_ident()) == primary_canonical)
    else {
        return false;
    };
    let Some(equation) = primary_var.get_equation() else {
        return false;
    };
    // A genuine passthrough body is a single scalar formula. An arrayed body
    // yields multiple per-element formulas (which `Equation::source_text`
    // `\n`-joins into something that does not reparse as one expression), so it
    // can never be the bare `out = BUILTIN(param)` shape -- treat it as a
    // non-passthrough rather than guessing at one element.
    let formulas = equation_formulas(equation);
    let [formula] = formulas.as_slice() else {
        return false;
    };
    let Ok(Some(ast)) = Expr0::new(formula, LexerType::Equation) else {
        return false;
    };
    classify_passthrough(
        &model.name,
        &spec.parameters,
        &spec.additional_outputs,
        &ast,
    )
}

/// Build a [`ModuleFunctionDescriptor`] for a stdlib module-function.
///
/// Called *after* `rewrite_alias_module_call` has normalized aliases, so
/// `name` is already a canonical stdlib model name. Returns `None` for any
/// name that is not a stdlib module-function.
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
        passthrough: false,
    })
}

/// The routing decision for one parsed call, before any builtin lowering.
///
/// `Expand` instantiates the macro model. The other three keep the call's
/// function and arguments and continue into builtin lowering, for three
/// different reasons that two consumers -- `BuiltinVisitor::walk` and
/// [`MacroRegistry`]'s recursion analysis -- have to agree on: `Passthrough` is
/// a genuine passthrough macro at an external call site, which keeps the
/// macro's declared arity (the descriptor is retained for the check);
/// `RenamedBuiltinSelfCall` is the enclosing macro's own importer-renamed
/// builtin (GH #554), which is the builtin and takes the builtin's arity; and
/// `Unresolved` is a name no project macro claims.
pub(crate) enum MacroCallResolution<'a> {
    Expand(&'a ModuleFunctionDescriptor),
    Passthrough(&'a ModuleFunctionDescriptor),
    RenamedBuiltinSelfCall,
    Unresolved,
}

/// A per-project macro registry, built once per compile from all of the
/// project's models. Answers "is this call name a project macro, and if so
/// what is its [`ModuleFunctionDescriptor`]?".
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Default, PartialEq, Eq)]
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
    ///   (`CircularDependency`, message names the cycle path);
    /// - **macros.AC5.7** a macro body that instantiates a module
    ///   (`MacroContainsModule`, message names the macro and its module
    ///   variables). See Pass 4 for why this is a CYCLE-SAFETY rule.
    ///
    /// **On failure the returned registry is EMPTY, and that is load-bearing for
    /// cycle safety** -- see the note on the `Err` return path in
    /// `db::macro_registry::project_macro_registry`, which is where the empty
    /// registry is actually materialized for the pipeline.
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
            // Classify a genuine passthrough macro (single-param `out =
            // BUILTIN(param)` self-call of a renamed builtin) once here, where
            // the body is parseable; the call site reads this off the
            // descriptor to collapse it to the opcode rather than expanding the
            // buggy per-element synthetic module (#591-c1). A non-passthrough
            // macro -- including Phase 2's RAMP FROM TO, which is NOT a
            // passthrough -- gets `None` and still expands as a module.
            let passthrough = classify_macro_passthrough(model, spec);
            macros.insert(
                canonical.clone(),
                ModuleFunctionDescriptor {
                    model_name: model.name.clone(),
                    parameter_ports: spec.parameters.clone(),
                    primary_output: spec.primary_output.clone(),
                    additional_outputs: spec.additional_outputs.clone(),
                    passthrough,
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

        // Pass 4: reject a macro body that instantiates a module. This is a
        // CYCLE-SAFETY rule, not a taste rule, and it is NOT redundant with
        // Pass 3.
        //
        // `db::project_module_graph` is the gate every production compile /
        // diagnostic / analysis entry point consults so that a module cycle
        // surfaces as a clean `CircularDependency` instead of driving the
        // recursive `compute_layout` / `model_shape` queries into salsa's
        // dependency-graph cycle panic (an abort under `panic = "abort"`). That
        // graph records only EXPLICIT `Variable::Module` edges, because it reads
        // variable KINDS off the salsa inputs and must never depend on a parse
        // result. A macro CALL is an implicit module edge, so the cycle
        //
        //     mac ->(explicit module in its body)-> u ->(macro call)-> mac
        //
        // has one edge the graph cannot see. `cycle_error_from` reports no cycle
        // and every entry point then aborts.
        //
        // Pass 3 cannot catch it: the macro set here is VALID, and has to be for
        // the bug to fire at all. `check_for_recursion` collects macro-to-macro
        // edges only; this cycle runs through `u`, a NON-macro model, so Pass 3's
        // graph cannot express the edge.
        //
        // Rejecting the shape (rather than widening the graph to parse-derived
        // edges, which would put every variable's parse on every compile's
        // dependency list) CLOSES the hole rather than narrowing it:
        //
        //   - The invisible edges are exactly the implicit module edges. They are
        //     synthesized at one site -- `builtins_visitor::expand_module_function`
        //     -- from a `ModuleFunctionDescriptor`, which has exactly two
        //     producers: `stdlib_descriptor` (target `stdlib⁚{name}`) and Pass 1
        //     above (target the macro's own model).
        //   - A stdlib model is a SINK: it holds no module variable, explicit or
        //     implicit. Asserted over synced Stage0s by
        //     `db::stages_tests::omitting_stdlib_models_from_the_lowering_scope_is_inert_today`.
        //   - So a macro model's outgoing edges are three, and all three are
        //     handled: an explicit module in its body (this pass), an implicit
        //     macro-to-macro edge (Pass 3 rejects a cycle among them, and on ANY
        //     build failure the registry the pipeline gets is empty, so no call
        //     is classified module-backed and the edge does not exist), or an
        //     implicit stdlib edge (to a sink).
        //   - Therefore every remaining cycle lies entirely in explicit edges,
        //     which is exactly what `project_module_graph` records.
        //
        // RESIDUAL: that first step -- builtin expansion and macro expansion
        // being the only synthesisers of implicit module vars -- was ENUMERATED
        // from the code paths above, not proved exhaustively. Nothing
        // structurally prevents a future implicit-var synthesizer from minting a
        // `Variable::Module` with a different target, and such a target would be
        // invisible to the gate again. There are THREE recorders of implicit
        // module vars today, all fed by the same `expand_module_function`:
        // `db::query::model_implicit_var_info` and
        // `db::ltm::model_ltm_implicit_var_info` (both carrying `is_module` +
        // `model_name`), plus `db::stages::model_scope_models`, which walks the
        // Stage0 `variables`. Only the first opens a cycle path: `compute_layout`
        // recurses on `model_implicit_var_info`'s module entries (Section 2) but
        // takes `meta.size` verbatim for the LTM ones (Section 3b), and
        // `model_shape` recurses only through `compute_layout`. So the LTM
        // recorder is inert for cycle safety -- but it is a place a future edit
        // could make recursive, which is why it is named here rather than left
        // out of the enumeration.
        //
        // The rejection is deliberately BROADER than the cycle, and the cost is
        // REAL, not zero. Measured against the pre-pass code, an explicit module
        // inside a macro targeting a model that does not call back is acyclic and
        // works end to end: it compiles, analyses, diagnoses cleanly, and
        // SIMULATES correctly. This pass rejects it anyway. Two things make that
        // shape reachable rather than hypothetical:
        //
        //   - The XMILE reader PASSES IT THROUGH (it does not synthesize it):
        //     `xmile::Macro.variables` reuses the `<model>` content model, and
        //     `macro_to_datamodel` filters only `Var::Unhandled`, so a `<module>`
        //     written inside a `<macro>` becomes a `Variable::Module` in a
        //     macro-marked model. Protobuf/JSON round-trip it the same way, so a
        //     project already stored with this shape is affected too.
        //   - Simlin's OWN XMILE writer round-trips it.
        //
        // The MDL importer cannot produce it, but not for the reason one might
        // assume: `mdl/convert/multi_output.rs` DOES mint a `Variable::Module`
        // (for a multi-output `:`-list invocation). It is unreachable from a macro
        // body only because the scoped body sub-context hard-codes an empty
        // materialization -- see `mdl/convert/macros.rs`'s `build_model(...,
        // &Default::default())`.
        //
        // We reject it regardless, for two reasons on record. (1) Narrowing to
        // only-when-cyclic needs a second reachability analysis here that must
        // agree with `project_module_graph`'s, and the back edge it would have to
        // see is the macro CALL -- discoverable only by parsing every model's
        // equations, which is exactly the dependency-list cost that widening the
        // graph was rejected for, wearing a different hat. Two reachability
        // implementations disagreeing is the failure mode commit 61d467d2
        // documents for the two diagnostic collectors. (2) A macro is a TEMPLATE;
        // instantiating a sub-model inside one is dubious on its own terms, so
        // this reads as a language rule with a cycle-safety motivation rather than
        // as a workaround. A loud rejection naming the remedy beats an abort.
        //
        // How many real models this affects is JUDGEMENT, not measurement: XMILE
        // §4.8 makes `<module>` syntactically legal inside `<macro>` because the
        // content model is shared, but Stella emits no `<macro>` at all and xmutil
        // emits them only from Vensim `:MACRO:` blocks, which cannot contain
        // modules. So: hand-written XMILE or a third-party writer. Small, but not
        // empty -- and nobody should lean on that estimate the way they can lean
        // on the MDL argument above.
        //
        // NOT affected, and the first thing a reader worries this breaks: a module
        // TARGETING a macro model. That is how a multi-output macro invocation
        // works at all -- the `Variable::Module` lives in the CALLING model. This
        // pass tests `macro_spec.is_some()` on the model that HOLDS the variable,
        // so those are untouched (`ac5_7_module_in_a_non_macro_model_is_not_rejected`).
        //
        // TWO CONSEQUENCES OF REJECTING, both intended, both surprising in
        // isolation:
        //
        //   - The reported macro name is CANONICAL (`my_macro` for a macro the
        //     file spells `My Macro`), because the salsa reconstruction path
        //     (`db::macro_registry::reconstruct_project_models`) has only the
        //     canonical name available. A modeller grepping their source for the
        //     reported name will not find it verbatim.
        //   - A rejected macro ALSO produces one `UnknownBuiltin` per call site,
        //     naming a real, correctly-declared macro as an unknown builtin. That
        //     is the pre-existing behaviour for ANY invalid macro set (a failed
        //     build yields an empty registry, so no call resolves as a macro), and
        //     it must NOT be suppressed: it is the load-bearing observable that no
        //     implicit module edge was synthesized, which is what keeps the two
        //     entry points that run their passes anyway from walking the cycle.
        //     Pinned by `db::module_cycle_tests::
        //     rejecting_a_macro_empties_the_registry_so_no_implicit_edge_is_synthesized`.
        for model in models {
            if model.macro_spec.is_none() {
                continue;
            }
            // Canonicalized and sorted so the message cannot depend on
            // body-variable iteration order: the salsa reconstruction path
            // (`db::macro_registry::reconstruct_project_models`) walks a
            // `HashMap<String, SourceVariable>`.
            let mut module_idents: Vec<String> = model
                .variables
                .iter()
                .filter_map(|var| match var {
                    datamodel::Variable::Module(m) => Some(canonicalize(&m.ident).into_owned()),
                    _ => None,
                })
                .collect();
            if module_idents.is_empty() {
                continue;
            }
            module_idents.sort_unstable();
            return model_err!(
                MacroContainsModule,
                format!(
                    "macro '{}' instantiates a module ({}); a macro body cannot \
                     contain a module -- move the sub-model instantiation to a \
                     caller of the macro",
                    canonicalize(&model.name),
                    module_idents.join(", ")
                )
            );
        }

        Ok(registry)
    }

    /// Look up a call name (canonicalized) in the macro registry.
    pub(crate) fn resolve_macro(&self, call_name: &str) -> Option<&ModuleFunctionDescriptor> {
        let canonical = canonicalize(call_name);
        self.macros.get(canonical.as_ref())
    }

    /// Route one parsed call: macro-shadows-everything precedence -- a project
    /// macro is resolved before any builtin, so a macro named `SSHAPE` or
    /// `RAMP FROM TO` expands as the macro -- with its two exceptions stated
    /// here and nowhere else.
    ///
    /// That precedence is the engine's rule, unverified against Vensim: its
    /// macro documentation (vensim.com/documentation/macros.html and 22145,
    /// "Defining Macros") says only that a macro name is any valid unquoted
    /// name and says nothing about a macro named after a builtin. XMILE 1.0
    /// section 3.2.2.5 (`docs/reference/xmile-v1.0.html`) goes the other way:
    /// builtin names "are reserved identifiers. They cannot be used as vendor-
    /// or user-defined namespaces, macros, or functions. Any conflict with
    /// these names ... SHOULD be flagged as an error". The collision the rule
    /// exists for is the MDL importer's own (`INITIAL -> INIT`, see
    /// `is_renamed_opcode_intrinsic`), which is why a same-named call inside
    /// the macro's body is the builtin.
    ///
    /// `enclosing_model` is the macro whose body the call sits in, if any. A
    /// call there whose canonical name is the enclosing macro's own AND a
    /// renamed builtin (`is_renamed_builtin_macro_collision`) is the importer's
    /// renamed builtin, not recursion (GH #554): `:MACRO: INIT(x) ... INIT =
    /// INITIAL(x)` stores the body `init = init(x)`, and resolving that call
    /// back to the macro would recurse forever, while a false `init -> init`
    /// edge in recursion analysis fails the whole registry and un-shadows every
    /// other macro of the project. The suppression is strictly the
    /// same-name-and-renamed-builtin case: a *different* macro that happens to
    /// be named after a builtin still resolves (so `init -> previous -> init`
    /// is still a rejected cycle), and a self-recursive macro whose name is not
    /// a renamed builtin (`foo = foo(x)`) still resolves to itself.
    pub(crate) fn resolve_call(
        &self,
        call_name: &str,
        enclosing_model: Option<&str>,
    ) -> MacroCallResolution<'_> {
        let call = canonicalize(call_name);
        if enclosing_model.is_some_and(|enclosing| call == canonicalize(enclosing))
            && is_renamed_builtin_macro_collision(call.as_ref())
        {
            return MacroCallResolution::RenamedBuiltinSelfCall;
        }
        match self.macros.get(call.as_ref()) {
            Some(descriptor) if descriptor.passthrough => {
                MacroCallResolution::Passthrough(descriptor)
            }
            Some(descriptor) => MacroCallResolution::Expand(descriptor),
            None => MacroCallResolution::Unresolved,
        }
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
                // Scan each source formula INDIVIDUALLY. An
                // `Equation::Arrayed` body variable carries one formula per
                // element (plus an optional EXCEPT default);
                // `Equation::source_text()` `\n`-joins them, which does NOT
                // reparse as a single expression -- a single
                // `Expr0::new(source_text())` would fail and silently drop
                // EVERY macro-call edge of the variable, so a recursion
                // cycle whose closing call sits in one per-element arrayed
                // body equation slipped past this guard and the
                // depth-limit-free expander then hung / overflowed instead
                // of reporting `CircularDependency`. Each element formula
                // parses fine on its own.
                for formula in equation_formulas(equation) {
                    let Ok(Some(ast)) = Expr0::new(formula, LexerType::Equation) else {
                        // A body equation that does not parse is a
                        // per-variable diagnostic surfaced later by the
                        // normal compile path; it is not the registry's job
                        // to report it, and it cannot introduce a
                        // (resolvable) macro call edge.
                        continue;
                    };
                    let mut called: std::collections::BTreeSet<String> = Default::default();
                    collect_called_macros(&ast, &from, self, &mut called);
                    if let Some(set) = edges.get_mut(&from) {
                        set.extend(called);
                    }
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

/// The individually-parseable source formulas of an equation.
///
/// A `Scalar`/`ApplyToAll` equation is one formula. An `Arrayed` equation
/// is one formula per explicitly-listed element plus any EXCEPT default --
/// the same pieces `Equation::source_text()` produces, but returned
/// separately rather than `\n`-joined: the joined form does NOT reparse as
/// a single expression, so callers that need to parse a body equation (the
/// macro recursion scan) must walk the formulas one at a time.
fn equation_formulas(eq: &datamodel::Equation) -> Vec<&str> {
    match eq {
        datamodel::Equation::Scalar(s) | datamodel::Equation::ApplyToAll(_, s) => {
            vec![s.as_str()]
        }
        datamodel::Equation::Arrayed(_, elements, default, _) => {
            let mut formulas: Vec<&str> =
                elements.iter().map(|(_, eqn, _, _)| eqn.as_str()).collect();
            if let Some(default_eqn) = default {
                formulas.push(default_eqn.as_str());
            }
            formulas
        }
    }
}

/// Walk an `Expr0` AST and record every call that [`MacroRegistry::resolve_call`]
/// expands, producing the macro-call edges out of `enclosing` (the canonical
/// name of the macro whose body this AST is).
///
/// Reading the same routing decision the expansion visitor reads is what keeps
/// this graph and the expansion agreeing about which calls are macro edges: a
/// passthrough and the enclosing macro's renamed-builtin self-call both lower
/// as builtins and so have no edge (a passthrough's own body is exactly such a
/// self-call, so it has no outgoing edge and cannot lie on a cycle either).
fn collect_called_macros(
    expr: &Expr0,
    enclosing: &str,
    registry: &MacroRegistry,
    out: &mut std::collections::BTreeSet<String>,
) {
    use crate::ast::IndexExpr0;
    use Expr0::*;
    match expr {
        Const(_, _, _) => {}
        Var(_, _) => {}
        App(UntypedBuiltinFn(func, args), _) => {
            if let MacroCallResolution::Expand(_) = registry.resolve_call(func, Some(enclosing)) {
                out.insert(canonicalize(func).into_owned());
            }
            for arg in args {
                collect_called_macros(arg, enclosing, registry, out);
            }
        }
        Subscript(_, args, _) => {
            for idx in args {
                match idx {
                    IndexExpr0::Range(l, r, _) => {
                        collect_called_macros(l, enclosing, registry, out);
                        collect_called_macros(r, enclosing, registry, out);
                    }
                    IndexExpr0::Expr(e) => collect_called_macros(e, enclosing, registry, out),
                    IndexExpr0::Wildcard(_)
                    | IndexExpr0::StarRange(_, _)
                    | IndexExpr0::DimPosition(_, _) => {}
                }
            }
        }
        Op1(_, r, _) => collect_called_macros(r, enclosing, registry, out),
        Op2(_, l, r, _) => {
            collect_called_macros(l, enclosing, registry, out);
            collect_called_macros(r, enclosing, registry, out);
        }
        If(cond, t, f, _) => {
            collect_called_macros(cond, enclosing, registry, out);
            collect_called_macros(t, enclosing, registry, out);
            collect_called_macros(f, enclosing, registry, out);
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

    /// A macro-marked model whose single body variable (the primary
    /// output) is an `Equation::Arrayed` with the given per-element
    /// formulas. `Equation::source_text()` `\n`-joins these element
    /// formulas, which is NOT a single parseable expression -- the regression
    /// the arrayed-body recursion tests exercise.
    fn macro_model_arrayed_body(name: &str, params: &[&str], elements: &[(&str, &str)]) -> Model {
        let arrayed = Equation::Arrayed(
            vec!["d".to_string()],
            elements
                .iter()
                .map(|(el, eqn)| (el.to_string(), eqn.to_string(), None, None))
                .collect(),
            None,
            false,
        );
        let mut variables = vec![Variable::Aux(Aux {
            ident: name.to_string(),
            equation: arrayed,
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })];
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

    /// Every arm of [`MacroCallResolution`], from the registry production
    /// builds, and the precedence between the two exceptions: the same `init`
    /// call is a passthrough at an external call site and the renamed builtin
    /// inside its own macro's body.
    #[test]
    fn resolve_call_covers_every_arm_and_the_self_call_takes_precedence() {
        let models = vec![
            plain_model("main"),
            macro_model("ordinary", &["x"], "x + 1"),
            macro_model("init", &["x"], "init(x)"),
            macro_model("previous", &["x", "fallback"], "previous(x, fallback) + 1"),
        ];
        let registry = MacroRegistry::build(&models).expect("the fixture builds");
        // `(call, enclosing macro, expected arm, expected descriptor model)`.
        let rows = [
            ("ordinary", None, "expand", Some("ordinary")),
            ("init", None, "passthrough", Some("init")),
            ("INIT", Some("init"), "renamed builtin self-call", None),
            (
                "previous",
                Some("PREVIOUS"),
                "renamed builtin self-call",
                None,
            ),
            ("ordinary", Some("ordinary"), "expand", Some("ordinary")),
            ("missing", None, "unresolved", None),
        ];
        for (call, enclosing, want, want_model) in rows {
            let (got, got_model) = match registry.resolve_call(call, enclosing) {
                MacroCallResolution::Expand(d) => ("expand", Some(d.model_name.as_str())),
                MacroCallResolution::Passthrough(d) => ("passthrough", Some(d.model_name.as_str())),
                MacroCallResolution::RenamedBuiltinSelfCall => ("renamed builtin self-call", None),
                MacroCallResolution::Unresolved => ("unresolved", None),
            };
            assert_eq!(
                (got, got_model),
                (want, want_model),
                "{call} inside {enclosing:?}"
            );
        }
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

    // --- macros.AC5.7: a macro body may not instantiate a module -----------

    /// A `Variable::Module` body variable, for the AC5.7 fixtures.
    fn module_var(ident: &str, target_model: &str) -> Variable {
        Variable::Module(datamodel::Module {
            ident: ident.to_string(),
            model_name: target_model.to_string(),
            documentation: String::new(),
            units: None,
            references: vec![],
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        })
    }

    /// A macro-marked model whose body holds a module is rejected, naming both
    /// the macro and the offending module variable.
    #[test]
    fn ac5_7_macro_holding_a_module_is_a_build_error_naming_both() {
        let mut mac = macro_model("mac", &["p1"], "p1 * 2");
        mac.variables.push(module_var("u_hop", "u"));
        let models = vec![plain_model("u"), mac];

        let err = MacroRegistry::build(&models)
            .expect_err("a macro holding a module must fail registry build");
        assert_eq!(err.code, crate::common::ErrorCode::MacroContainsModule);
        let details = err.get_details().unwrap_or_default();
        assert!(
            details.contains("mac") && details.contains("u_hop"),
            "the rejection must name the macro and its module variable: {details:?}",
        );
    }

    /// A module in an ORDINARY model is untouched -- the rule is scoped to
    /// macro-marked models. Without this the pass could quietly become a
    /// project-wide ban on sub-models.
    #[test]
    fn ac5_7_module_in_a_non_macro_model_is_not_rejected() {
        let mut host = plain_model("host");
        host.variables.push(module_var("sub", "u"));
        let models = vec![
            host,
            plain_model("u"),
            macro_model("mac", &["p1"], "p1 * 2"),
        ];
        MacroRegistry::build(&models)
            .expect("a module in a non-macro model must not trip the macro rule");
    }

    /// The message is independent of body-variable iteration order. The salsa
    /// reconstruction path (`db::macro_registry::reconstruct_project_models`)
    /// walks a `HashMap<String, SourceVariable>`, so a macro with two module
    /// variables would otherwise report them in an arbitrary order and the
    /// message would differ run to run -- the same hazard `find_cycle`'s sorted
    /// roots / `BTreeSet` successors exist for.
    #[test]
    fn ac5_7_two_modules_in_one_macro_report_deterministically() {
        let build_message = |idents: [&str; 2]| {
            let mut mac = macro_model("mac", &["p1"], "p1 * 2");
            for ident in idents {
                mac.variables.push(module_var(ident, "u"));
            }
            MacroRegistry::build(&[plain_model("u"), mac])
                .expect_err("a macro holding modules must fail")
                .get_details()
                .unwrap_or_default()
        };
        assert_eq!(
            build_message(["zeta_hop", "alpha_hop"]),
            build_message(["alpha_hop", "zeta_hop"]),
            "the rejection message must not depend on body-variable order",
        );
        assert!(
            build_message(["zeta_hop", "alpha_hop"]).contains("alpha_hop, zeta_hop"),
            "both offending modules must be listed, sorted",
        );
    }

    /// Pass 4 runs LAST -- after ALL THREE pre-existing passes -- so adding it
    /// shifts no existing error message or ordering, and every fixture that
    /// asserted an AC5.2/AC5.3 code keeps getting it. Cycle safety does not
    /// depend on which pass fires (any failure empties the registry); the
    /// ordering is purely about not perturbing the pre-existing surface.
    ///
    /// Both halves are needed. A fixture that is merely duplicated pins the
    /// ordering against Passes 1-2 only: moving Pass 4 to sit between Pass 2 and
    /// Pass 3 would leave it green, which is how a first version of this test
    /// gave a mutation probe a false pass.
    #[test]
    fn ac5_7_pass_runs_after_the_pre_existing_passes() {
        // vs Passes 1-2 (duplicate macro name).
        let mut dup = macro_model("mac", &["p1"], "p1 * 2");
        dup.variables.push(module_var("u_hop", "u"));
        let err = MacroRegistry::build(&[plain_model("u"), dup.clone(), dup])
            .expect_err("a duplicate macro must fail");
        assert_eq!(
            err.code,
            crate::common::ErrorCode::DuplicateMacroName,
            "the pre-existing duplicate-name pass must still win",
        );

        // vs Pass 3 (macro recursion). `mac` both self-recurses and holds a
        // module, so only the pass ORDER decides which code is reported.
        let mut recursive = macro_model("mac", &["p1"], "mac(p1) + 1");
        recursive.variables.push(module_var("u_hop", "u"));
        let err = MacroRegistry::build(&[plain_model("u"), recursive])
            .expect_err("a self-recursive macro must fail");
        assert_eq!(
            err.code,
            crate::common::ErrorCode::CircularDependency,
            "the pre-existing recursion pass must still win",
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

    // --- macros.AC5.2: recursion hidden in an arrayed multi-element body ---
    //
    // `Equation::source_text()` joins an `Equation::Arrayed` body
    // variable's per-element formulas (and any EXCEPT default) with `\n`.
    // That concatenation is NOT a single parseable expression, so a single
    // `Expr0::new(source_text())` parse of it failed and the recursion scan
    // silently dropped EVERY macro-call edge of the variable -- including a
    // self/mutual call sitting in one element that closes a cycle. The
    // (depth-limit-free) expander would then hang / overflow instead of the
    // design's promised `CircularDependency`. Each element formula parses
    // fine on its own, so the scan must consider them individually.

    #[test]
    fn ac5_2_self_recursion_in_arrayed_multi_element_body_is_circular_dependency() {
        // `a`'s body is arrayed with two element equations; the first
        // element calls `a` -> a self-edge that only a per-element scan
        // sees (the `\n`-joined `a(x)\nx` does not parse as one expr).
        let models = vec![macro_model_arrayed_body(
            "a",
            &["x"],
            &[("e1", "a(x)"), ("e2", "x")],
        )];
        let err = MacroRegistry::build(&models).expect_err(
            "a self-call inside one element of an arrayed multi-element \
             macro body must still be detected as recursion",
        );
        assert_eq!(
            err.code,
            crate::common::ErrorCode::CircularDependency,
            "recursion hidden in a per-element arrayed body equation must \
             be reported as CircularDependency, not silently admitted"
        );
    }

    #[test]
    fn ac5_2_recursion_closed_by_a_later_arrayed_element_is_detected() {
        // The cycle-closing call sits in the SECOND element equation, so a
        // fix that only inspected the first element would still miss it:
        // a -> b, and b's arrayed body calls `a` only in its 2nd element.
        let models = vec![
            macro_model("a", &["x"], "b(x)"),
            macro_model_arrayed_body("b", &["y"], &[("e1", "y + 1"), ("e2", "a(y)")]),
        ];
        let err = MacroRegistry::build(&models)
            .expect_err("a -> b -> a closed by b's 2nd arrayed element must be a cycle");
        assert_eq!(err.code, crate::common::ErrorCode::CircularDependency);
    }

    #[test]
    fn arrayed_multi_element_body_without_recursion_builds_ok() {
        // The fix must not over-reject: a perfectly valid arrayed
        // multi-element body with no macro self/mutual call must still
        // build and resolve.
        let models = vec![
            plain_model("main"),
            macro_model_arrayed_body("a", &["x"], &[("e1", "x + 1"), ("e2", "x * 2")]),
        ];
        let registry = MacroRegistry::build(&models)
            .expect("a non-recursive arrayed multi-element macro body must build");
        assert!(registry.resolve_macro("a").is_some());
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

    // --- MacroRegistry::build threads the passthrough classification ----------
    //
    // The pure `classify_passthrough` is computed once at registry-build time
    // (the only place each macro body is parsed) and stored on the descriptor,
    // so the call site can read it without re-parsing the (discarded) body AST.

    #[test]
    fn build_classifies_init_passthrough_macro_as_some() {
        // The #591-c1 shape: `:MACRO: INIT(x) = INITIAL(x)` stored as the
        // datamodel macro body `init = init(x)` after the importer rename.
        let models = vec![plain_model("main"), macro_model("init", &["x"], "init(x)")];
        let registry = MacroRegistry::build(&models).expect("the INIT passthrough macro builds");
        let d = registry
            .resolve_macro("init")
            .expect("the INIT macro resolves");
        assert!(
            d.passthrough,
            "a genuine `INIT = INIT(x)` passthrough must be classified at build time"
        );
    }

    #[test]
    fn build_does_not_classify_near_miss_init_macro() {
        // `:MACRO: INIT(x) = INITIAL(x) + 1` is NOT a bare passthrough -- the
        // `+ 1` is real work the opcode collapse would drop -- so the descriptor
        // must record `passthrough == false` and the macro keeps expanding.
        let models = vec![
            plain_model("main"),
            macro_model("init", &["x"], "init(x) + 1"),
        ];
        let registry =
            MacroRegistry::build(&models).expect("the near-miss INIT macro still builds");
        let d = registry
            .resolve_macro("init")
            .expect("the INIT macro resolves");
        assert!(
            !d.passthrough,
            "INIT = INIT(x) + 1 is a near-miss and must NOT be classified as a passthrough"
        );
    }

    #[test]
    fn build_leaves_non_passthrough_macro_descriptor_passthrough_none() {
        // An ordinary macro (not a renamed-builtin self-call at all) must carry
        // `passthrough == false`.
        let models = vec![
            plain_model("main"),
            macro_model("mymacro", &["a", "b"], "a * b"),
        ];
        let registry = MacroRegistry::build(&models).expect("ordinary macro project builds");
        let d = registry.resolve_macro("mymacro").expect("mymacro resolves");
        assert!(
            !d.passthrough,
            "a non-passthrough macro must have passthrough == false"
        );
    }

    #[test]
    fn stdlib_descriptor_passthrough_is_none() {
        // Stdlib (non-macro) descriptors are never passthroughs.
        let d = stdlib_descriptor("smth1").expect("smth1 is a stdlib module-function");
        assert!(
            !d.passthrough,
            "a stdlib descriptor is not a passthrough macro"
        );
    }

    // --- classify_passthrough: the pure structural passthrough classifier ---
    //
    // A `:MACRO: INIT(x) = INITIAL(x)` collides (after the MDL importer renames
    // `INITIAL` -> `INIT`) with the opcode-backed `init` intrinsic, so its
    // datamodel body is `init = init(x)`. Such a *genuine passthrough* macro
    // (single param; body exactly `out = BUILTIN(param)` where `BUILTIN`
    // canonicalizes to the same renamed-builtin collision name) is collapsed at
    // the call site directly to the opcode (LoadInitial), bypassing the buggy
    // per-element synthetic module. `classify_passthrough` is the pure
    // structural rule that decides this; it must NOT misfire on a non-passthrough
    // macro that merely shares a builtin name.

    /// Parse a macro body equation into the `Expr0` AST the classifier expects.
    fn body_ast(equation: &str) -> Expr0 {
        Expr0::new(equation, LexerType::Equation)
            .expect("body equation must parse")
            .expect("body equation must not be empty")
    }

    #[test]
    fn classify_passthrough_init_self_call_is_some_init() {
        // The exact #591-c1 shape: `INIT = INIT(x)` (single param `x`), the
        // datamodel form of `:MACRO: INIT(x) = INITIAL(x)` after the importer
        // rename. `init` is an opcode-backed renamed-builtin collision, so the
        // call-site fall-through to the `init`->LoadInitial intrinsic routing is
        // valid -- classify as a passthrough targeting `init`.
        let result = classify_passthrough("init", &["x".to_string()], &[], &body_ast("init(x)"));
        assert!(
            result,
            "INIT = INIT(x) is a genuine passthrough to the `init` opcode"
        );
    }

    #[test]
    fn classify_passthrough_op2_body_is_none() {
        // `INIT = INIT(x) + 1` is NOT a bare passthrough: the body is an Op2,
        // not a single call, so collapsing it to the opcode would drop the
        // `+ 1` (AC3.4 negative).
        let result =
            classify_passthrough("init", &["x".to_string()], &[], &body_ast("init(x) + 1"));
        assert!(
            !result,
            "INIT = INIT(x) + 1 is an Op2 body, not a bare passthrough"
        );
    }

    #[test]
    fn classify_passthrough_arg_not_bare_param_is_none() {
        // `INIT = INIT(x * 2)`: the call's argument is an expression, not the
        // bare parameter, so the macro does real work the opcode collapse would
        // discard (AC3.4 negative).
        let result =
            classify_passthrough("init", &["x".to_string()], &[], &body_ast("init(x * 2)"));
        assert!(
            !result,
            "INIT = INIT(x * 2) has an expression argument, not the bare param"
        );
    }

    #[test]
    fn classify_passthrough_two_param_body_is_none() {
        // A two-parameter macro fails the single-parameter arity gate even when
        // its body is a single one-argument call.
        let result = classify_passthrough(
            "f",
            &["a".to_string(), "b".to_string()],
            &[],
            &body_ast("f(a)"),
        );
        assert!(
            !result,
            "a two-parameter macro cannot be a single-arg passthrough"
        );
    }

    #[test]
    fn classify_passthrough_non_collision_builtin_is_none() {
        // `ABS = ABS(x)`: a single-param, bare-arg self-call, but `abs` is NOT a
        // renamed-builtin collision (no dedicated opcode/stdlib-module routing
        // reachable by the call-site fall-through), so the passthrough collapse
        // would have nowhere valid to land -- must be None.
        let result = classify_passthrough("abs", &["x".to_string()], &[], &body_ast("abs(x)"));
        assert!(
            !result,
            "`abs` is not a renamed-builtin collision, so it is not opcode-backed \
             via the call-site fall-through"
        );
    }

    #[test]
    fn classify_passthrough_multi_output_macro_is_none() {
        // A multi-output macro (additional outputs present from Vensim's
        // `:`-list syntax) is NOT collapsible: its call site receives more than
        // the primary output, so it must keep expanding as a module even if its
        // primary-output body looks like a bare self-call.
        let result = classify_passthrough(
            "init",
            &["x".to_string()],
            &["secondary".to_string()],
            &body_ast("init(x)"),
        );
        assert!(
            !result,
            "a multi-output macro must not collapse to a single opcode"
        );
    }

    #[test]
    fn classify_passthrough_different_call_name_is_none() {
        // The call must be a *self*-call (canonicalize(call) ==
        // canonicalize(macro_name)) -- the form the importer's rename produces.
        // `INIT = PREVIOUS(x, 0)` is not a self-call (the macro is `init`, the
        // call is `previous`), so it is not the renamed-builtin self-collapse
        // case. NOTE: `previous(x, 0)` is a TWO-arg call, so this case is
        // actually rejected at the single-argument gate (3) before gate (5) is
        // reached; `classify_passthrough_single_arg_non_self_call_name_is_none`
        // below exercises gate (5) in isolation.
        let result =
            classify_passthrough("init", &["x".to_string()], &[], &body_ast("previous(x, 0)"));
        assert!(
            !result,
            "a call to a different builtin name than the macro is not a self-call \
             passthrough"
        );
    }

    #[test]
    fn classify_passthrough_single_arg_non_self_call_name_is_none() {
        // Gate (5) -- the self-call-name check -- in isolation. A single-arg
        // `previous(x)` body inside an `init`-named macro passes the arity gate
        // (1/2), the single-call/single-arg gate (3), and the bare-arg gate (4),
        // so the ONLY thing that can reject it is gate (5):
        // canonicalize("previous") != canonicalize("init"). (`previous(x)` is one
        // arg at the `Expr0` level; the unary->`previous(x, 0)` desugar happens
        // later in `builtins_visitor`, not at parse time.) Without gate (5) this
        // would mis-collapse a non-self-call macro onto the wrong opcode.
        let result =
            classify_passthrough("init", &["x".to_string()], &[], &body_ast("previous(x)"));
        assert!(
            !result,
            "a single-arg call to a different builtin name than the macro must be \
             rejected at the self-call-name gate, not collapsed"
        );
    }
}
