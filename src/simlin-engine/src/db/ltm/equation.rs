// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The typed, authoritative representation of an LTM synthetic variable's
//! equation ([`LtmEquation`]).
//!
//! GH #965 (storage/compile boundary): LTM generated equations are internal
//! compiler artifacts, not user-authored source. This type carries them as a
//! parsed [`Expr0`] AST -- what the fragment compiler and the layout implicit-
//! var scan actually consume -- so normal operation never prints an equation
//! to `datamodel::Equation` text and re-parses it back to compile it (the
//! former `db/ltm/parse.rs::parse_ltm_equation` transient-`Aux` round trip,
//! run 2-3 times per equation; GH #655 finding 3).
//!
//! Each equation arm ([`LtmArm`]) keeps the generator's exact source-form text
//! ALONGSIDE the parsed AST. The text is the DIAGNOSTIC serialization only --
//! the characterization dump (`db/ltm_char_tests.rs`) and the partial-equation
//! warning read it, and it preserves the source spelling byte-for-byte -- while
//! the AST is the sole compiled representation. They are created together from
//! one generator output (`expr = Expr0::new(text)`) and only ever moved as a
//! unit (`scalarize`/`retarget_dims` re-tag dimensions, never rewrite an arm),
//! so they cannot drift.

use std::collections::HashMap;

use crate::ast::{Ast, Expr0};
use crate::common::{CanonicalElementName, EquationError};
use crate::datamodel;
use crate::lexer::LexerType;

/// One equation arm: the authoritative parsed AST plus its diagnostic text.
///
/// See the module docs for why both are carried. `expr` is `None` for an
/// empty equation (`Expr0::new("")` yields `Ok(None)`, e.g. a discovery-only
/// stub) or the effectively-unreachable case of a generated equation that
/// fails to parse -- both compile to no bytecode, mirroring how the old text
/// path handled an empty/bad `datamodel::Equation`.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, salsa::Update)]
pub struct LtmArm {
    /// The generator's exact source-form spelling. Retained ONLY for
    /// diagnostics; never re-parsed to compile.
    pub text: String,
    /// The authoritative compiled AST (`Expr0::new(text)`).
    pub expr: Option<Expr0>,
}

impl LtmArm {
    /// Parse `text` into the authoritative AST once, at the generation
    /// (source-format) boundary.
    pub fn new(text: String) -> Self {
        let expr = match Expr0::new(&text, LexerType::Equation) {
            Ok(expr) => expr,
            Err(_) => {
                // A generated LTM equation is always a `print_eqn` re-print or
                // a guard-form assembly of already-parsed sub-expressions, so a
                // parse failure here means a bug in the augmentation layer, not
                // bad user input. Degrade exactly as the old text path did (an
                // unparseable equation carried no AST and compiled to no
                // bytecode -> the fragment is dropped and
                // `model_ltm_fragment_diagnostics` warns) rather than panicking
                // -- libsimlin release builds are panic=abort.
                debug_assert!(false, "LTM generated equation failed to parse: {text}");
                None
            }
        };
        Self { text, expr }
    }
}

/// A synthetic LTM variable's equation, mirroring the three
/// `datamodel::Equation` shapes but carrying a parsed [`Expr0`] per arm
/// instead of source text (see the module docs).
///
/// Dimension NAMES (datamodel casing) are kept as `String`s so the shaping
/// helpers ([`LtmEquation::dimensions`], [`LtmEquation::scalarize`],
/// [`LtmEquation::retarget_dims`]) and layout sizing read them without a
/// `DimensionsContext`; they are resolved to `Dimension`s only when lowering
/// to an `Ast<Expr0>` for compilation ([`LtmEquation::to_flow_ast`]).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, salsa::Update)]
pub enum LtmEquation {
    Scalar(LtmArm),
    ApplyToAll(Vec<String>, LtmArm),
    Arrayed {
        dims: Vec<String>,
        /// `(raw element subscript, arm)`, order-preserving so
        /// [`LtmEquation::scalarize`] can pick the first slot exactly as the
        /// old text path did over a `datamodel::Equation::Arrayed`'s `Vec`.
        elements: Vec<(String, LtmArm)>,
        default: Option<LtmArm>,
        has_except_default: bool,
    },
}

impl LtmEquation {
    /// A scalar synthetic equation, parsed once from `text`.
    pub fn scalar(text: String) -> Self {
        LtmEquation::Scalar(LtmArm::new(text))
    }

    /// An apply-to-all synthetic equation over `dims` (datamodel-cased names),
    /// parsed once from the shared body `text`.
    pub fn apply_to_all(dims: Vec<String>, text: String) -> Self {
        LtmEquation::ApplyToAll(dims, LtmArm::new(text))
    }

    /// A per-element (arrayed) synthetic equation. `elements` is
    /// `(raw subscript, body text)` in slot order; each body is parsed once.
    pub fn arrayed(
        dims: Vec<String>,
        elements: Vec<(String, String)>,
        default: Option<String>,
        has_except_default: bool,
    ) -> Self {
        LtmEquation::Arrayed {
            dims,
            elements: elements
                .into_iter()
                .map(|(subscript, text)| (subscript, LtmArm::new(text)))
                .collect(),
            default: default.map(LtmArm::new),
            has_except_default,
        }
    }

    /// The equation's diagnostic source text concatenated into a single string
    /// (the generator's exact spelling): the scalar / apply-to-all formula
    /// verbatim, or -- for the per-element (`Arrayed`) variant -- every element
    /// formula plus any default joined by newlines. Mirrors
    /// [`datamodel::Equation::source_text`]; a convenience for diagnostics and
    /// tests that inspect an equation as text without matching its variant.
    pub fn source_text(&self) -> String {
        match self {
            LtmEquation::Scalar(arm) | LtmEquation::ApplyToAll(_, arm) => arm.text.clone(),
            LtmEquation::Arrayed {
                elements, default, ..
            } => {
                let mut parts: Vec<&str> =
                    elements.iter().map(|(_, arm)| arm.text.as_str()).collect();
                if let Some(default_arm) = default {
                    parts.push(default_arm.text.as_str());
                }
                parts.join("\n")
            }
        }
    }

    /// The dimension names the equation carries (datamodel casing), or `&[]`
    /// for a scalar one. These are the names whose product gives the
    /// variable's layout slot count, kept in lockstep with
    /// `LtmSyntheticVar::dimensions` at every construction site.
    pub fn dimensions(&self) -> &[String] {
        match self {
            LtmEquation::Scalar(_) => &[],
            LtmEquation::ApplyToAll(dims, _) => dims,
            LtmEquation::Arrayed { dims, .. } => dims,
        }
    }

    /// Reduce the equation to a scalar one, keeping the first slot's arm.
    ///
    /// Used by the module-involved link-score path
    /// ([`crate::db::module_link_score_equation`]), which always emits a scalar
    /// variable regardless of the target's dimensionality, and by
    /// [`LtmEquation::retarget_dims`] to collapse a degenerate zero-dimension
    /// `Arrayed` (the empty-`dims` case) -- a per-element link score with no
    /// dimension to index is meaningless, so it falls back to scalar. For an
    /// `Arrayed`, the first per-element slot's arm is used.
    pub fn scalarize(self) -> Self {
        match self {
            LtmEquation::Scalar(_) => self,
            LtmEquation::ApplyToAll(_, arm) => LtmEquation::Scalar(arm),
            LtmEquation::Arrayed {
                elements, default, ..
            } => {
                let arm = elements
                    .into_iter()
                    .next()
                    .map(|(_, arm)| arm)
                    .or(default)
                    .unwrap_or_else(|| LtmArm::new("0".to_string()));
                LtmEquation::Scalar(arm)
            }
        }
    }

    /// Re-tag the equation so its dimension names match `dims` (the
    /// link-score-dimensions policy result the emission loop assigned to
    /// `LtmSyntheticVar::dimensions`). Empty `dims` collapses to `Scalar`;
    /// non-empty widens a scalar to `ApplyToAll` or re-targets the dimension
    /// names of an existing `ApplyToAll`/`Arrayed`, preserving the arm AST(s)
    /// verbatim.
    pub fn retarget_dims(self, dims: &[String]) -> Self {
        match self {
            LtmEquation::Scalar(arm) | LtmEquation::ApplyToAll(_, arm) => {
                if dims.is_empty() {
                    LtmEquation::Scalar(arm)
                } else {
                    LtmEquation::ApplyToAll(dims.to_vec(), arm)
                }
            }
            LtmEquation::Arrayed {
                dims: orig,
                elements,
                default,
                has_except_default,
            } => {
                if dims.is_empty() {
                    // The link-score-dimensions policy assigned no dimensions:
                    // a zero-dimension `Arrayed` is degenerate (its per-element
                    // partials have no dimension to index), so collapse to a
                    // scalar link score -- the pre-existing behavior for such
                    // edges.
                    LtmEquation::Arrayed {
                        dims: orig,
                        elements,
                        default,
                        has_except_default,
                    }
                    .scalarize()
                } else {
                    LtmEquation::Arrayed {
                        dims: dims.to_vec(),
                        elements,
                        default,
                        has_except_default,
                    }
                }
            }
        }
    }

    /// Build the flow-phase `Ast<Expr0>` for compilation and the layout
    /// implicit-var scan, resolving the dimension NAMES to `Dimension`s against
    /// the project datamodel dims. Mirrors `variable::parse_equation` MINUS the
    /// text parse -- the arm ASTs are already parsed -- and returns any
    /// dimension-resolution error alongside (so the compiler drops the fragment
    /// and the diagnostic pass warns, exactly as a text parse error did).
    ///
    /// `None` ast is an empty/unparseable equation (no arm expr): it compiles
    /// to no bytecode. LTM synthetic variables are flow-phase only, so there is
    /// no init-phase ast to build.
    pub fn to_flow_ast(
        &self,
        dimensions: &[datamodel::Dimension],
    ) -> (Option<Ast<Expr0>>, Vec<EquationError>) {
        match self {
            LtmEquation::Scalar(arm) => (arm.expr.clone().map(Ast::Scalar), vec![]),
            LtmEquation::ApplyToAll(dims, arm) => {
                match crate::variable::get_dimensions(dimensions, dims) {
                    Ok(resolved) => (
                        arm.expr.clone().map(|e| Ast::ApplyToAll(resolved, e)),
                        vec![],
                    ),
                    Err(err) => (None, vec![err]),
                }
            }
            LtmEquation::Arrayed {
                dims,
                elements,
                default,
                has_except_default,
            } => {
                // Mirror `parse_equation`'s Arrayed arm: drop element slots with
                // no expr (an empty/unparseable body) rather than error.
                let map: HashMap<CanonicalElementName, Expr0> = elements
                    .iter()
                    .filter_map(|(subscript, arm)| {
                        arm.expr
                            .clone()
                            .map(|e| (CanonicalElementName::from_raw(subscript), e))
                    })
                    .collect();
                let default_expr = default.as_ref().and_then(|a| a.expr.clone());
                match crate::variable::get_dimensions(dimensions, dims) {
                    Ok(resolved) => (
                        Some(Ast::Arrayed(
                            resolved,
                            map,
                            default_expr,
                            *has_except_default,
                        )),
                        vec![],
                    ),
                    Err(err) => (None, vec![err]),
                }
            }
        }
    }
}
