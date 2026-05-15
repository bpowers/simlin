// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Materialization of Vensim multi-output (`:`-list) macro invocations.
//!
//! A single-output macro invocation (`total = M(a, b)`) has a plain-text
//! equivalent and is left as ordinary equation text -- it is expanded later
//! by `BuiltinVisitor` (Phase 3). A *multi-output* invocation
//! (`total = add3(a, b, c : minv, maxv)`) cannot be expressed as plain text
//! (a call returns several named values at once), so it is materialized here,
//! at MDL import, **before** `build_equation` / the XMILE formatter ever sees
//! it (the formatter has a `debug_assert!(output_bindings.is_empty())` -- see
//! Phase 4).
//!
//! A multi-output call is valid only as the *entire* right-hand side of an
//! equation -- that is the sole position with a well-defined
//! materialization. The parser nonetheless syntactically accepts the
//! invalid *nested* form (`y = c + add3(p, q, r : lo, hi)`): the inner
//! `App` keeps its `output_bindings`. `find_nested_multi_output_call`
//! enforces the whole-RHS-only rule -- it rejects any `output_bindings`-
//! bearing `App` in a non-top-level position with a `ConvertError`,
//! *before* `detect_multi_output_call`. Without it the nested form would
//! reach the XMILE formatter and panic on the debug-build
//! `debug_assert!(output_bindings.is_empty())` (and, in a release build,
//! silently drop the `:`-list outputs).
//!
//! A multi-output invocation `total = add3(a, b, c : minv, maxv)` becomes:
//! - one input-only `Variable::Module` named `{lhs}_macro` (collision-safe,
//!   serialization-stable -- this is round-tripped, so deliberately NOT the
//!   `$⁚` compile-time-synthetic prefix), targeting the called macro's
//!   `Model.name`, with one input `ModuleReference` per call argument
//!   (`dst = "{module}.{param}"`, `src =` the argument);
//! - a primary-output binding `Aux` replacing the LHS aux: `total` reads
//!   `{module}.{primary_output}`;
//! - one additional-output binding `Aux` per `:`-list entry: the call-site
//!   name (`minv`) becomes the variable ident, reading
//!   `{module}.{macro_internal_output_name}` (e.g. `{module}.minval`).
//!
//! The module-output reference text uses an ASCII period at the datamodel
//! layer; `canonicalize()` converts it to U+00B7 only at compile-time parse
//! (the authoritative Separator convention). The reference then resolves
//! through the fully-general `get_submodel_offset` machinery exactly like any
//! other `module.output` reference.

use std::collections::{HashMap, HashSet};

use crate::datamodel::{self, Aux, Equation, Model, Module, ModuleReference, Variable};

use crate::mdl::ast::{CallKind, Equation as MdlEquation, Expr, FullEquation};
use crate::mdl::xmile_compat::quoted_space_to_underbar;

use super::ConversionContext;
use super::helpers::{canonical_name, extract_units, variable_ident};
use super::types::ConvertError;

/// The outcome of scanning the main model's symbols for multi-output macro
/// invocations: the brand-new datamodel variables to add, and the set of
/// (canonical) symbol names whose normal `build_variable` pass must be
/// **skipped** (their multi-output `Expr::App` must never reach the
/// formatter).
#[derive(Default)]
pub(super) struct MultiOutputMaterialization {
    /// Variables to append to the model (modules + binding auxes + any
    /// hoisted expression-argument auxes).
    pub variables: Vec<Variable>,
    /// Canonical symbol names that the normal per-symbol build loop must not
    /// process (the primary-output binding aux replaces the LHS symbol; the
    /// additional-output names are brand-new and not in `symbols` at all).
    pub skip_symbols: HashSet<String>,
}

/// A detected multi-output invocation, extracted from a symbol's selected
/// equation.
struct MultiOutputCall<'a, 'input> {
    /// Canonical name of the LHS symbol (the one being replaced).
    lhs_canonical: String,
    /// Raw call name (the macro being invoked); canonicalized for lookup.
    call_name: &'a str,
    /// Positional call arguments.
    args: &'a [Expr<'input>],
    /// Post-`:` output-binding expressions (each expected to be a `Var`).
    output_bindings: &'a [Expr<'input>],
}

impl<'input> ConversionContext<'input> {
    /// Scan the *main* model's symbols for multi-output macro invocations and
    /// materialize each one. `macro_models` maps a canonical macro name to
    /// the macro-marked [`Model`] it names (carrying both the target
    /// `Model.name` and the `MacroSpec`).
    ///
    /// Returns the variables to add plus the set of symbol names whose normal
    /// build must be skipped. Errors (unknown macro, arity mismatch) name the
    /// macro so the diagnostic is actionable.
    pub(super) fn materialize_multi_output_invocations(
        &self,
        macro_models: &HashMap<String, &Model>,
    ) -> Result<MultiOutputMaterialization, ConvertError> {
        let mut variables: Vec<Variable> = Vec::new();
        let mut skip_symbols: HashSet<String> = HashSet::new();

        // Track every ident that will exist so the synthetic module ident is
        // collision-safe and deterministic. Start from the existing symbol
        // keys (canonical) so `{lhs}_macro` never shadows a real variable.
        let mut taken_idents: HashSet<String> =
            self.symbols.keys().map(|k| canonical_name(k)).collect();

        // Deterministic iteration order: sort the symbol keys so the chosen
        // numeric disambiguators (and any hoisted-arg names) are stable
        // across runs regardless of HashMap ordering.
        let mut symbol_keys: Vec<&String> = self.symbols.keys().collect();
        symbol_keys.sort_unstable();

        for key in symbol_keys {
            let info = &self.symbols[key];

            // Enforce the invariant the whole module relies on: a
            // multi-output call may appear ONLY as the entire RHS of an
            // equation (the sole position with a well-defined
            // materialization). The parser, however, ACCEPTS the invalid
            // nested form (`y = c + ADD3(p, q, r : lo, hi)`) -- the inner
            // `Expr::App` keeps its `output_bindings`. Reject any
            // `output_bindings`-bearing `App` in a NON-top-level position
            // here, with an actionable diagnostic; otherwise it would slip
            // past `detect_multi_output_call` (which only matches a
            // whole-RHS `App`) and reach the XMILE formatter, where it
            // PANICS on the Phase-2 `debug_assert!(output_bindings
            // .is_empty())` in a debug build and SILENTLY drops the
            // `:`-list outputs in a release build.
            //
            // The scan must cover EVERY non-empty equation of the symbol,
            // not merely `select_equation`'s single representative pick: an
            // arrayed variable carries one `FullEquation` per element
            // override (`y[a1] = ...`, `y[a2] = ...`), and
            // `build_variable_with_elements` formats *all* valid per-element
            // equations. `select_equation` returns only `non_empty[0]`
            // (PurgeAFOEq), so a nested multi-output call in a *later*
            // per-element equation would otherwise bypass the guard and
            // reach the formatter. Emptiness uses the same `is_empty_rhs`
            // predicate `select_equation` / `build_variable_with_elements`
            // filter on, so the guard's coverage matches what the build
            // pass actually formats.
            for eq in info
                .equations
                .iter()
                .filter(|eq| !self.is_empty_rhs(&eq.equation))
            {
                if let Some(nested_macro) = find_nested_multi_output_call(&eq.equation) {
                    return Err(ConvertError::Other(format!(
                        "multi-output call to `{}` may only appear as the entire \
                         right-hand side of an equation",
                        nested_macro
                    )));
                }
            }

            // The legitimate whole-RHS multi-output detection still keys off
            // `select_equation`'s single representative pick: that preserves
            // the cycle-1 scalar materialization behavior exactly (a valid
            // whole-RHS `total = ADD3(p, q, r : lo, hi)` is the
            // representative equation and materializes; an arrayed variable
            // is not a valid multi-output materialization site, and the
            // all-equations guard above has already rejected any nested
            // multi-output call in any of its per-element equations).
            let Some(eq) = self.select_equation(&info.equations) else {
                continue;
            };
            let Some(call) = detect_multi_output_call(key, &eq.equation) else {
                continue;
            };

            let macro_model = macro_models
                .get(canonical_name(call.call_name).as_str())
                .copied()
                .ok_or_else(|| {
                    ConvertError::Other(format!(
                        "multi-output call to unknown macro `{}`",
                        call.call_name
                    ))
                })?;
            // A model in `macro_models` is macro-marked by construction.
            let spec = macro_model
                .macro_spec
                .as_ref()
                .expect("macro_models only contains macro-marked models");

            // Strict arity for both the input arguments and the `:`-outputs.
            if call.args.len() != spec.parameters.len() {
                return Err(ConvertError::Other(format!(
                    "multi-output call to macro `{}` has {} argument(s) but the \
                     macro declares {} parameter(s)",
                    call.call_name,
                    call.args.len(),
                    spec.parameters.len()
                )));
            }
            if call.output_bindings.len() != spec.additional_outputs.len() {
                return Err(ConvertError::Other(format!(
                    "multi-output call to macro `{}` binds {} `:`-output(s) but \
                     the macro declares {} additional output(s)",
                    call.call_name,
                    call.output_bindings.len(),
                    spec.additional_outputs.len()
                )));
            }

            // Reject a non-`Var` output binding: the `:`-list names must be
            // plain caller-side variable names (Vensim only allows that).
            let mut output_idents: Vec<String> = Vec::with_capacity(call.output_bindings.len());
            for binding in call.output_bindings {
                match binding {
                    Expr::Var(name, subs, _) if subs.is_empty() => {
                        output_idents.push(quoted_space_to_underbar(name));
                    }
                    _ => {
                        return Err(ConvertError::Other(format!(
                            "multi-output call to macro `{}` has a non-variable \
                             `:`-output binding (only plain variable names are allowed)",
                            call.call_name
                        )));
                    }
                }
            }

            // Mint the serialization-stable module ident. The LHS ident is
            // the canonical-name-to-underbar form (matching how every other
            // variable ident is produced).
            let lhs_ident = quoted_space_to_underbar(&call.lhs_canonical);
            let module_ident = mint_module_ident(&lhs_ident, &mut taken_idents);

            // One input ModuleReference per argument. A simple `Var` argument
            // wires directly by its canonical name; an expression-valued
            // argument is hoisted into a deterministic synthetic Aux.
            let mut references: Vec<ModuleReference> = Vec::with_capacity(call.args.len());
            for (i, arg) in call.args.iter().enumerate() {
                let src = match arg {
                    Expr::Var(name, subs, _) if subs.is_empty() => variable_ident(name),
                    _ => {
                        // Hoist the expression argument into its own aux with
                        // a deterministic, serialization-safe ident derived
                        // from the (already collision-safe) module ident.
                        let hoist_ident =
                            unique_ident(&format!("{}_arg{}", module_ident, i), &mut taken_idents);
                        let arg_eq = self.formatter.format_expr(arg);
                        variables.push(Variable::Aux(Aux {
                            ident: hoist_ident.clone(),
                            equation: Equation::Scalar(arg_eq),
                            documentation: String::new(),
                            units: None,
                            gf: None,
                            ai_state: None,
                            uid: None,
                            compat: datamodel::Compat::default(),
                        }));
                        hoist_ident
                    }
                };
                references.push(ModuleReference {
                    src,
                    dst: format!("{}.{}", module_ident, spec.parameters[i]),
                });
            }

            variables.push(Variable::Module(Module {
                ident: module_ident.clone(),
                model_name: macro_model.name.clone(),
                documentation: String::new(),
                units: None,
                references,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }));

            // Primary-output binding aux: replaces the LHS aux. `total` now
            // reads `{module}.{primary_output}`.
            variables.push(Variable::Aux(Aux {
                ident: lhs_ident,
                equation: Equation::Scalar(format!("{}.{}", module_ident, spec.primary_output)),
                documentation: invocation_doc(eq),
                units: extract_units(eq),
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }));

            // One additional-output binding aux per `:`-list entry. The
            // call-site name is the variable ident; the macro's internal
            // output name is what it reads from the module.
            for (i, out_ident) in output_idents.iter().enumerate() {
                variables.push(Variable::Aux(Aux {
                    ident: out_ident.clone(),
                    equation: Equation::Scalar(format!(
                        "{}.{}",
                        module_ident, spec.additional_outputs[i]
                    )),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }));
            }

            // The LHS symbol's normal build must be skipped: the
            // primary-output binding aux above replaces it.
            skip_symbols.insert(call.lhs_canonical);
            // Defense-in-depth: the additional-output binding auxes are
            // authoritative. Real Vensim multi-output models do not
            // separately declare the `:`-list names (they are created by the
            // call), but if a stray same-named declaration exists, skip it
            // so the normal loop does not also emit a duplicate (which would
            // shadow the binding reference with the stray equation).
            for out_ident in &output_idents {
                skip_symbols.insert(canonical_name(out_ident));
            }
        }

        Ok(MultiOutputMaterialization {
            variables,
            skip_symbols,
        })
    }
}

/// If `eq` is a top-level multi-output macro invocation
/// (`Regular(lhs, App(name, _, args, Symbol, output_bindings, _))` with a
/// non-empty `output_bindings`), extract it.
///
/// Only the whole RHS is inspected here: valid Vensim only ever emits a
/// multi-output call as the entire right-hand side of an equation (the sole
/// position with a well-defined materialization). The *nested* form is
/// syntactically accepted by the parser but is invalid; it is rejected with
/// a `ConvertError` by [`find_nested_multi_output_call`] (run as a guard in
/// [`ConversionContext::materialize_multi_output_invocations`] *before* this
/// detection), so by the time a non-`None` result here is materialized the
/// invariant has already been enforced -- it is no longer merely asserted.
fn detect_multi_output_call<'a, 'input>(
    symbol_key: &str,
    eq: &'a MdlEquation<'input>,
) -> Option<MultiOutputCall<'a, 'input>> {
    let MdlEquation::Regular(_lhs, expr) = eq else {
        return None;
    };
    let Expr::App(name, _subscripts, args, CallKind::Symbol, output_bindings, _) = expr else {
        return None;
    };
    if output_bindings.is_empty() {
        return None;
    }
    Some(MultiOutputCall {
        lhs_canonical: canonical_name(symbol_key),
        call_name: name.as_ref(),
        args,
        output_bindings,
    })
}

/// Find an `Expr::App` with a non-empty `output_bindings` (a multi-output
/// macro call) anywhere OTHER THAN the top-level RHS position of `eq`,
/// returning the offending macro's raw name.
///
/// A multi-output call is only valid as the *entire* right-hand side of an
/// equation -- that is the sole position [`detect_multi_output_call`]
/// matches and the sole one with a well-defined materialization (a Module
/// plus the primary/additional binding auxes). The parser, however,
/// syntactically accepts the invalid nested form (`y = c + ADD3(p, q, r :
/// lo, hi)`): the inner `App` retains its `output_bindings`. This guard
/// enforces the whole-RHS-only rule that `detect_multi_output_call`'s doc
/// describes, turning what would otherwise be a debug-build panic (the
/// Phase-2 `debug_assert!(output_bindings.is_empty())` in the XMILE
/// formatter) / release-build silent `:`-output loss into a clean
/// `ConvertError`.
///
/// The top-level whole-RHS `App` that legitimately IS a multi-output call is
/// deliberately NOT flagged: when the RHS root is such an `App`, only its
/// `args` and `output_bindings` sub-expressions are scanned (a multi-output
/// call nested *inside an argument* of another multi-output call is still
/// invalid). For any other RHS root, the entire expression tree is scanned.
/// A non-`Regular` equation has no expression RHS, so nothing to scan.
///
/// The variant coverage mirrors `helpers::rewrite_dollar_time` (the Phase-2
/// recursion precedent): `Op1`/`Paren` recurse into the inner expr,
/// `Op2` into both sides, `App` into every arg *and* every output binding;
/// the leaf variants (`Var`/`Const`/`Literal`/`Na`) terminate.
fn find_nested_multi_output_call<'a>(eq: &'a MdlEquation<'_>) -> Option<&'a str> {
    let MdlEquation::Regular(_lhs, expr) = eq else {
        return None;
    };
    match expr {
        // The whole RHS *is* a multi-output call: this is the legitimate
        // form `detect_multi_output_call` will materialize. The root App
        // itself is not an error, but a multi-output call nested inside one
        // of its arguments or `:`-output expressions still is.
        Expr::App(_, _, args, CallKind::Symbol, output_bindings, _)
            if !output_bindings.is_empty() =>
        {
            args.iter()
                .chain(output_bindings.iter())
                .find_map(find_nested_multi_output_call_in_expr)
        }
        // Any other RHS root: a multi-output call anywhere in the tree is a
        // nested (illegal) occurrence.
        other => find_nested_multi_output_call_in_expr(other),
    }
}

/// Recursively scan an expression subtree for an `Expr::App` with a
/// non-empty `output_bindings`, returning the offending macro's raw name.
/// Every position reached here is, by construction, NOT the top-level RHS,
/// so any such `App` is an illegal nested multi-output call.
fn find_nested_multi_output_call_in_expr<'a>(expr: &'a Expr<'_>) -> Option<&'a str> {
    match expr {
        Expr::App(name, _subscripts, args, kind, output_bindings, _) => {
            if matches!(kind, CallKind::Symbol) && !output_bindings.is_empty() {
                return Some(name.as_ref());
            }
            // Even an ordinary call's arguments / a non-multi-output App's
            // children may contain a nested multi-output call.
            args.iter()
                .chain(output_bindings.iter())
                .find_map(find_nested_multi_output_call_in_expr)
        }
        Expr::Op1(_, inner, _) | Expr::Paren(inner, _) => {
            find_nested_multi_output_call_in_expr(inner)
        }
        Expr::Op2(_, left, right, _) => find_nested_multi_output_call_in_expr(left)
            .or_else(|| find_nested_multi_output_call_in_expr(right)),
        Expr::Var(_, _, _) | Expr::Const(_, _) | Expr::Literal(_, _) | Expr::Na(_) => None,
    }
}

/// Documentation string for the materialized primary-output binding aux,
/// taken from the invocation equation's comment (preserves the modeler's
/// note, e.g. THEIL's "Note the output variables following the :").
fn invocation_doc(eq: &FullEquation<'_>) -> String {
    eq.comment
        .as_ref()
        .map(|c| c.to_string())
        .unwrap_or_default()
}

/// Mint the serialization-stable module ident: `{lhs}_macro`, with the
/// lowest numeric disambiguator (`{lhs}_macro_2`, `{lhs}_macro_3`, ...) that
/// is unique. Reserves the chosen ident in `taken`.
fn mint_module_ident(lhs_ident: &str, taken: &mut HashSet<String>) -> String {
    unique_ident(&format!("{}_macro", lhs_ident), taken)
}

/// Return `base` if its canonical form is free, else `base_2`, `base_3`, ...
/// (lowest free numeric suffix). Reserves the canonical form of the chosen
/// ident in `taken` so subsequent calls cannot collide.
fn unique_ident(base: &str, taken: &mut HashSet<String>) -> String {
    let base_canonical = canonical_name(base);
    if !taken.contains(&base_canonical) {
        taken.insert(base_canonical);
        return base.to_string();
    }
    let mut n = 2;
    loop {
        let candidate = format!("{}_{}", base, n);
        let candidate_canonical = canonical_name(&candidate);
        if !taken.contains(&candidate_canonical) {
            taken.insert(candidate_canonical);
            return candidate;
        }
        n += 1;
    }
}
