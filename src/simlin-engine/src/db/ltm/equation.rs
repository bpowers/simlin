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

use std::sync::Arc;

use crate::ast::{Ast, Expr0};
use crate::common::{CanonicalElementName, EquationError};
use crate::lexer::LexerType;

/// One equation arm: the authoritative parsed AST plus its diagnostic text.
///
/// See the module docs for why both are carried. `expr` is `None` in two
/// materially DIFFERENT cases, which [`LtmArm::parse_error`] distinguishes:
///
/// - an **empty** equation (`Expr0::new("")` yields `Ok(None)`, e.g. a
///   discovery-only stub) -- legitimate, no errors, and dropped from an
///   `Arrayed` slot map exactly as `variable::parse_equation` drops it;
/// - a **failed parse** -- `parse_error` is `Some`, and [`LtmEquation::to_flow_ast`]
///   surfaces it so the fragment is REJECTED rather than silently
///   zero-filled.
///
/// Conflating the two is what made a bad arm silent: with siblings that parse,
/// an `Arrayed` equation still produced bytecode, so the "no bytecode ⇒
/// `model_ltm_fragment_diagnostics` warns" path never fired and that element's
/// score read a constant 0 with no diagnostic at all.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct LtmArm {
    /// The generator's exact source-form spelling. Retained ONLY for
    /// diagnostics; never re-parsed to compile.
    pub text: String,
    /// The authoritative compiled AST (`Expr0::new(text)`).
    ///
    /// Behind an `Arc` because every emitted link score is cloned out of the
    /// `link_score_equation_text_shaped` memo (`db/ltm/link_scores.rs`) into
    /// `model_ltm_variables`' own list, so the tree would otherwise be retained
    /// TWICE for the whole life of the database -- on C-LEARN, two copies of
    /// 12.78 MB of equations, whose ASTs dominate that query's ~273 MiB. Sharing
    /// makes that clone a refcount bump and retains one copy.
    ///
    /// `Arc<Expr0>` still compares BY VALUE, which is load-bearing: salsa
    /// backdates a re-executed query whose value compares equal, and that is
    /// what lets an unrelated edit reuse the expensive downstream fragment (GH
    /// #981). Pointer equality would be an optimization on top, never a
    /// substitute.
    pub expr: Option<Arc<Expr0>>,
    /// `Some` iff `text` FAILED to parse -- never merely because it was empty.
    /// Preserved (rather than discarded at construction) so the arm that failed
    /// can reject its whole equation; see the type docs.
    ///
    /// The FIRST error only, deliberately: this is a boolean-with-provenance,
    /// since the emitted diagnostic names the variable and its text rather than
    /// the parse position, and the whole equation is rejected regardless of how
    /// many arms or positions failed. Keeping a `Vec` here inflated
    /// `LtmSyntheticVar` (hence `ShapedLinkScore`) past clippy's
    /// `large_enum_variant` threshold for data no consumer reads.
    pub parse_error: Option<EquationError>,
}

impl LtmArm {
    /// Parse `text` into the authoritative AST once, at the generation
    /// (source-format) boundary.
    pub fn new(text: String) -> Self {
        // Degradation is non-fatal but LOUD: the parse errors are RETAINED and
        // `to_flow_ast` returns them, so the fragment is rejected and
        // `model_ltm_fragment_diagnostics` warns. Retaining them (rather than
        // collapsing the failure into a bare `expr: None`) is what makes the
        // ARRAYED case loud too -- with siblings that parse, a dropped arm would
        // leave the fragment alive and that element's score reading a silent 0.
        //
        // An earlier revision did `debug_assert!(false, ..)`, on the theory that
        // a generated equation is always a `print_eqn` re-print and so a parse
        // failure could only be an augmentation-layer bug. That theory was
        // wrong, and the assert made a VALID model abort a debug build: a
        // canonical name the lexer cannot read bare (`1stock`) was emitted
        // unquoted by `ltm_augment::quote_ident`. The root cause is fixed there
        // (it now shares `ast::needs_quoting` with `print_ident`), but the
        // degradation stays non-fatal on principle -- this runs inside a salsa
        // query on the ordinary read path, where aborting on user input is
        // strictly worse than a diagnostic, and libsimlin release builds are
        // panic=abort.
        let (expr, parse_error) = match Expr0::new(&text, LexerType::Equation) {
            Ok(expr) => (expr.map(Arc::new), None),
            // `Expr0::new` reports every position it found; keep the first as
            // the failure's provenance (see the field docs).
            Err(errs) => (None, errs.into_iter().next()),
        };
        Self {
            text,
            expr,
            parse_error,
        }
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
#[derive(Clone, PartialEq)]
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

    /// Every arm's retained parse errors, in arm order (elements then default).
    /// Non-empty means at least one arm's generated text failed to parse -- an
    /// augmentation-layer bug -- as distinct from an arm that is legitimately
    /// EMPTY (which carries no error).
    fn arm_parse_errors(&self) -> Vec<EquationError> {
        let arms: Vec<&LtmArm> = match self {
            LtmEquation::Scalar(arm) | LtmEquation::ApplyToAll(_, arm) => vec![arm],
            LtmEquation::Arrayed {
                elements, default, ..
            } => elements
                .iter()
                .map(|(_, arm)| arm)
                .chain(default.as_ref())
                .collect(),
        };
        arms.iter()
            .filter_map(|arm| arm.parse_error.clone())
            .collect()
    }

    /// Build the flow-phase `Ast<Expr0>` for compilation and the layout
    /// implicit-var scan, resolving the dimension NAMES to `Dimension`s against
    /// the project datamodel dims. Mirrors `variable::parse_equation` MINUS the
    /// text parse -- the arm ASTs are already parsed -- and returns any
    /// dimension-resolution error alongside (so the compiler drops the fragment
    /// and the diagnostic pass warns, exactly as a text parse error did).
    ///
    /// A FAILED arm parse is returned as an error and yields NO ast, for every
    /// shape. That is load-bearing for `Arrayed`: dropping the bad arm and
    /// keeping its parsing siblings (which is what `variable::parse_equation`
    /// does for an EMPTY arm) would leave the fragment with bytecode, so the
    /// compiler would zero-fill the missing slot, `flow_bytecodes` would stay
    /// `Some`, and `model_ltm_fragment_diagnostics` would emit nothing -- a
    /// silent per-element zero. An arm that is legitimately EMPTY still just
    /// drops, exactly as `parse_equation` drops it.
    ///
    /// `None` ast with no errors is an empty equation: it compiles to no
    /// bytecode. LTM synthetic variables are flow-phase only, so there is no
    /// init-phase ast to build.
    pub fn to_flow_ast(
        &self,
        dimensions: &crate::dimensions::DimensionsContext,
    ) -> (Option<Ast<Expr0>>, Vec<EquationError>) {
        // A generated arm that failed to parse rejects the whole equation,
        // BEFORE any shape-specific assembly -- see the note above on why the
        // `Arrayed` slot map must not simply drop it.
        let parse_errors = self.arm_parse_errors();
        if !parse_errors.is_empty() {
            return (None, parse_errors);
        }
        // The arms' ASTs are shared (`Arc`), but `Ast<Expr0>` owns its tree, so
        // building one unshares. That is the right trade: the result is consumed
        // by the fragment compile and dropped, whereas the arm itself is retained
        // for the life of the database -- so the sharing is what bounds RETENTION,
        // not what avoids this transient copy.
        match self {
            LtmEquation::Scalar(arm) => (arm.expr.as_deref().cloned().map(Ast::Scalar), vec![]),
            LtmEquation::ApplyToAll(dims, arm) => {
                match crate::variable::get_dimensions(dimensions, dims) {
                    Ok(resolved) => (
                        arm.expr
                            .as_deref()
                            .cloned()
                            .map(|e| Ast::ApplyToAll(resolved, e)),
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
                // no expr. Reaching here means no arm FAILED to parse, so a
                // missing expr is an empty body only.
                let map: HashMap<CanonicalElementName, Expr0> = elements
                    .iter()
                    .filter_map(|(subscript, arm)| {
                        arm.expr
                            .as_deref()
                            .cloned()
                            .map(|e| (CanonicalElementName::from_raw(subscript), e))
                    })
                    .collect();
                let default_expr = default.as_ref().and_then(|a| a.expr.as_deref().cloned());
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

#[cfg(test)]
mod tests {
    use super::{LtmArm, LtmEquation};

    /// Salsa backdates a re-executed query's memo by `PartialEq`, and the
    /// literal on `Expr0::Const` is an `ast::Literal` compared by bit pattern,
    /// so a NaN-bearing LTM equation is equal to an identical rebuild of
    /// itself -- which is what lets `link_score_equation_text_shaped` backdate
    /// and the expensive `compile_ltm_var_fragment` be reused (GH #981).
    ///
    /// The controls (an ordinary equation, and a genuinely edited one) are what
    /// keep this from passing by making salsa blind.
    #[test]
    fn a_nan_bearing_ltm_equation_is_equal_to_itself() {
        assert!(
            LtmArm::new("NaN".to_string()).expr.is_some(),
            "the fixture text must parse, or nothing below measures the AST"
        );
        assert_eq!(
            LtmArm::new("NaN".to_string()),
            LtmArm::new("NaN".to_string())
        );
        assert_eq!(
            LtmEquation::scalar("1 + NaN".to_string()),
            LtmEquation::scalar("1 + NaN".to_string())
        );
        assert_eq!(
            LtmEquation::scalar("1 + 2".to_string()),
            LtmEquation::scalar("1 + 2".to_string()),
            "control: an ordinary equation was always equal to itself"
        );
        assert_ne!(
            LtmEquation::scalar("1 + NaN".to_string()),
            LtmEquation::scalar("2 + NaN".to_string()),
            "control: a genuine difference must still be visible"
        );
    }
}
