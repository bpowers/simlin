// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Implicit WITH-LOOKUP support for LTM link scores (GH #910).
//!
//! A value-bearing variable that also carries graphical-function tables is
//! lowered by `compiler::apply_implicit_with_lookup` to `LOOKUP(self_gf, input)`,
//! but its *equation text* -- what every LTM partial is built from -- spells only
//! the gf's input. This module owns the rules that put the gf back: which
//! variables are implicit WITH-LOOKUPs, and what `LOOKUP` table reference each
//! emitter should wrap its partial in.
//!
//! Split out of `ltm_augment.rs` to keep that file under the project
//! line-count lint; included via `#[path]`, so `super::*` resolves the parent's
//! private items.

use std::collections::HashSet;

use crate::variable::{VarKind, Variable};

use super::quote_ident;

/// Whether `var` is an implicit WITH-LOOKUP variable: a value-bearing
/// (non table-only) variable carrying at least one non-degenerate
/// graphical-function table, so the compiler lowers it to
/// `LOOKUP(self_gf, input)` (`compiler::apply_implicit_with_lookup`).
///
/// # Implicit WITH LOOKUP coverage (GH #910)
///
/// Every LTM surface that reasons about such a target must account for the
/// gf that the equation text does not spell:
///
/// - **Structural polarity** composes the gf's monotonicity
///   (`ltm::polarity::compose_with_lookup_polarity`).
/// - **Link-score partials** must be commensurable with the target's
///   (gf-output-units) deltas. Two partial shapes reach the guard form:
///   a *full re-evaluation* of the target's equation (the ceteris-paribus
///   partial, and the reducer emitters' enumerated inner partial), which is
///   in gf-INPUT units and is therefore wrapped in `LOOKUP({table_ref}, .)`;
///   and the *delta-ratio stand-in* (`partial == target`), which is already
///   in gf-output units and must NOT be wrapped.
/// - The table reference itself comes from [`with_lookup_table_ref`]
///   (Scalar / ApplyToAll), [`WithLookupSlotRefs`]
///   (per-target-element slots), or [`with_lookup_reducer_owner_wrap`]
///   (the reducer emitters, whose partial is row-scoped rather than
///   element-scoped).
///
/// ## Coverage
///
/// The complete set of link-score numerator producers is every construction of
/// [`super::link_score_guard_form`] / [`super::link_score_guard_form_with_numerator`] (plus
/// [`super::build_element_reducer_link_score`], which inlines the same guard). Each,
/// with its class and wrap:
///
/// | producer | class | wrap |
/// |---|---|---|
/// | [`super::shaped_guard_form_text`] changed-first (the ceteris-paribus partial behind [`super::generate_link_score_equation_for_link`] and [`super::build_arrayed_link_score_equation`]) | 1 | yes |
/// | [`super::shaped_guard_form_text`] changed-last (its frozen re-evaluation fallback) | 1 | yes |
/// | [`super::generate_scalar_feeder_to_agg_equation`] (changed-last feeder half) | 1 | yes, owner wrap |
/// | [`super::generate_iterated_feeder_to_agg_equation`] (changed-last feeder half) | 1 | yes, owner wrap |
/// | [`super::generate_scalar_to_element_equation`] (scalar source or `$⁚ltm⁚agg⁚{n}` -> arrayed target, per element) | 1 | yes, per-slot |
/// | [`super::generate_per_element_link_equation`] (`RefShape::PerElement` source read, per (row, element)) | 1 | yes, per-slot |
/// | [`super::generate_agg_to_scalar_target_equation`] (`$⁚ltm⁚agg⁚{n}` -> scalar target) | 1 | yes |
/// | [`super::build_element_reducer_link_score`], `Constant` arm (SIZE) | -- | n/a: it returns `"0"` as the whole link-score equation, short-circuiting the guard form -- there is no partial |
/// | [`super::build_element_reducer_link_score`], `Linear` arm, gf path (the enumerated `linear_inner_partial` re-evaluation) | 1 | yes, owner wrap |
/// | [`super::build_element_reducer_link_score`], `Linear` arm, gf-LESS path (`PREVIOUS(target) + delta`) | -- | n/a: the incremental anchored form is neither a full re-evaluation nor a delta-ratio stand-in; it exists only where there is no gf to wrap |
/// | [`super::build_element_reducer_link_score`], MIN / MAX / STDDEV arms | 1 | yes, owner wrap |
/// | [`super::build_element_reducer_link_score`], `!is_bare` / RANK / un-pinnable-body arms | 2 | never |
///
/// Not numerator producers, and so nothing to wrap: `generate_flow_to_stock_equation`
/// (a fixed structural formula whose target is a `Variable::Stock`, which
/// `is_implicit_with_lookup` excludes) and the module composite / black-box
/// scores (`Δoutput`-shaped transfer formulas with no target-equation partial).
///
/// ## Residuals
///
/// 1. [`super::build_arrayed_link_score_equation`]'s `Ast::Arrayed` EXCEPT-**default**
///    slot stays unwrapped when the target carries PER-ELEMENT tables: that
///    emitter renders ONE `Equation::Arrayed` default text shared by every
///    default-covered element, and those elements apply DIFFERENT tables, so no
///    single `LOOKUP` argument is correct. (With a shared variable-level table
///    the default IS wrapped.) This is a real gap, not a fabricated-input case:
///    with `apply_default_to_missing`, an element listed with an EMPTY equation
///    still gets its table recorded by `variable::build_tables` while
///    `variable.rs`'s `.filter(|(_, ast)| ast.is_some())` drops it from
///    `per_elem` -- so it legitimately takes the default and applies its own gf.
///
///    The two sibling emitters that *enumerate* target elements --
///    `db::ltm::link_scores::emit_agg_to_target_link_scores` and
///    `emit_per_element_link_scores` -- resolve each default-covered element's
///    own table by name and DO wrap it. The asymmetry is structural: they emit
///    one scalar variable per element, this one emits a single defaulted
///    `Equation::Arrayed`. Closing the gap means enumerating the default-covered
///    elements into explicit slots here, which would change gf-less emitted text.
/// 2. A reducer owner with PER-ELEMENT tables has no expressible row-scoped
///    reference, so [`WithLookupWrap::PerElementUndecidable`] tells the emitters
///    to decline the edge loudly
///    (`db::ltm::link_scores::decline_with_lookup_reducer_edge`) rather than emit
///    a possibly sign-inverted score. Neither call site has a known live input;
///    see that function's doc for the per-call-site reachability argument.
pub(crate) fn is_implicit_with_lookup(var: &Variable) -> bool {
    matches!(
        var.kind,
        VarKind::Aux {
            is_table_only: false,
            ..
        }
    ) && var.tables().iter().any(|t| !t.x.is_empty())
}

/// The implicit WITH-LOOKUP verdict for a variable that OWNS a reducer whose
/// per-`(row, slot)` link scores the reducer emitters build (GH #910).
///
/// Unlike the ceteris-paribus generators -- which build one partial per
/// target ELEMENT and can therefore name that element's own table -- a
/// reducer emitter's partial is a single scalar expression per source row,
/// so it can only carry ONE `LOOKUP` argument.
pub(crate) enum WithLookupWrap {
    /// The owner applies no gf: its partial needs no wrap.
    NoGf,
    /// The owner applies ONE table to every element; wrap with this
    /// reference (a bare ident for a scalar owner, `to[1,...]` for an
    /// arrayed one -- table index 0, the only table).
    Wrap(String),
    /// The owner carries PER-ELEMENT tables: different elements apply
    /// different gfs, so no single `LOOKUP` argument is correct for the
    /// row-scoped partial. The caller must decline the edge loudly rather
    /// than emit an unwrapped (possibly sign-inverted) score.
    PerElementUndecidable,
}

/// Classify a reducer owner for the implicit WITH-LOOKUP wrap. See
/// [`WithLookupWrap`] and [`is_implicit_with_lookup`].
pub(crate) fn with_lookup_reducer_owner_wrap(to_var: &Variable) -> WithLookupWrap {
    if !is_implicit_with_lookup(to_var) {
        return WithLookupWrap::NoGf;
    }
    let [table] = to_var.tables() else {
        return WithLookupWrap::PerElementUndecidable;
    };
    if table.x.is_empty() {
        // A zero-point table is treated as ABSENT by the compiler, so the
        // raw reducer evaluates unwrapped. (Unreachable: the
        // `is_implicit_with_lookup` guard above requires a non-empty table
        // and there is only one.)
        return WithLookupWrap::NoGf;
    }
    let quoted = quote_ident(to_var.ident());
    match to_var.get_dimensions() {
        Some(dims) if !dims.is_empty() => {
            let ones = vec!["1"; dims.len()].join(",");
            WithLookupWrap::Wrap(format!("{quoted}[{ones}]"))
        }
        _ => WithLookupWrap::Wrap(quoted),
    }
}

/// The `LOOKUP` table reference for a Scalar/ApplyToAll implicit
/// WITH-LOOKUP target's link-score partial (GH #910), or `None` when the
/// target's compiled value applies no gf.
///
/// A value-bearing tables-carrying target lowers to `LOOKUP(self_gf,
/// input)` (`compiler::apply_implicit_with_lookup`); the partial must be
/// fed through the same table so the guard form's numerator lives in
/// gf-output units like the target deltas it is ratioed against:
///
/// - Scalar target: the bare (quoted) target name -- exactly the
///   table-by-ident resolution an explicit `LOOKUP(var, x)` call gets.
/// - ApplyToAll target: only a variable-level gf is possible (per-element
///   gfs require `Equation::Arrayed`, routed to
///   [`super::build_arrayed_link_score_equation`]), and the compiler applies its
///   single table (index 0) to EVERY element -- so the reference pins the
///   first element (`to[1,...]`, 1-based numeric subscripts). A bare
///   arrayed reference would instead resolve each iterated element's own
///   table offset, which is out of range for every element but the first.
///
/// `None` for: no tables, a table-only variable (a static table -- no
/// implicit wrap), a zero-point table (treated as ABSENT by the compiler:
/// the raw input evaluates unwrapped), and defensively for a multi-table
/// list reaching this Scalar/A2A path.
///
/// Coverage boundary: see the `Implicit WITH LOOKUP coverage` note on
/// [`is_implicit_with_lookup`].
pub(crate) fn with_lookup_table_ref(to_var: &Variable) -> Option<String> {
    use crate::ast::Ast;
    let VarKind::Aux {
        tables,
        is_table_only: false,
        ..
    } = &to_var.kind
    else {
        return None;
    };
    let [table] = tables.as_slice() else {
        return None;
    };
    if table.x.is_empty() {
        return None;
    }
    let quoted = quote_ident(to_var.ident());
    match to_var.ast() {
        Some(Ast::ApplyToAll(dims, _)) if !dims.is_empty() => {
            let ones = vec!["1"; dims.len()].join(",");
            Some(format!("{quoted}[{ones}]"))
        }
        // An `Ast::Arrayed` target is routed to
        // `build_arrayed_link_score_equation` before this is reached; if
        // one ever leaks here (the degraded eqn-text fallback), a bare
        // arrayed table reference would be wrong, so decline the wrap.
        Some(Ast::Arrayed(..)) => None,
        _ => Some(quoted),
    }
}

/// Per-target-element `LOOKUP` table references for an implicit WITH-LOOKUP
/// target, resolved once per target (GH #910).
///
/// Mirrors `compiler::apply_implicit_with_lookup`'s per-element rule: with a
/// single variable-level table every element applies table 0 (pin the first
/// element, `to[1,...]`); with per-element tables the slot's element applies
/// its OWN table at the element's row-major flat offset (`to[<elem>]` -- the
/// same static-subscript resolution an explicit `LOOKUP(var[elem], x)` gets),
/// and an element whose table is an empty placeholder (no gf) keeps its raw
/// input equation, so no wrap.
///
/// The per-element element-name -> table mapping is materialized ONCE in
/// [`WithLookupSlotRefs::new`]. Callers ask per target element, so resolving an
/// element's flat offset by scanning `SubscriptIterator` on each call would make
/// emission quadratic in the element count (with a string join per candidate).
/// The gf-less and single-table cases -- every ordinary model -- build nothing.
pub(crate) struct WithLookupSlotRefs {
    kind: SlotRefKind,
}

enum SlotRefKind {
    /// The target applies no gf (no tables, table-only, or a zero-point table
    /// the compiler treats as ABSENT), so no slot is ever wrapped.
    NoGf,
    /// ONE variable-level table, applied to every element: this reference
    /// (`to[1,...]`) pins table index 0 for every slot, including the
    /// EXCEPT-default one. A bare arrayed reference would instead resolve each
    /// iterated element's own table offset -- out of range for every element
    /// but the first.
    Shared(String),
    /// PER-ELEMENT tables: each listed element with a non-empty table wraps with
    /// its own `to[<elem>]`. The EXCEPT-default slot is NOT wrappable -- one text
    /// is shared by elements carrying different tables (a documented residual).
    PerElement {
        quoted: String,
        with_table: HashSet<crate::common::CanonicalElementName>,
    },
}

impl WithLookupSlotRefs {
    /// Resolve `to_var`'s slot-wrap rule. `target_ast_dims` are the target's AST
    /// dimensions, whose row-major enumeration is the layout
    /// `variable::reorder_arrayed_element_tables` gives per-element tables.
    pub(crate) fn new(to_var: &Variable, target_ast_dims: &[crate::dimensions::Dimension]) -> Self {
        let kind = Self::classify(to_var, target_ast_dims);
        WithLookupSlotRefs { kind }
    }

    fn classify(
        to_var: &Variable,
        target_ast_dims: &[crate::dimensions::Dimension],
    ) -> SlotRefKind {
        let VarKind::Aux {
            tables,
            is_table_only: false,
            ..
        } = &to_var.kind
        else {
            return SlotRefKind::NoGf;
        };
        if tables.is_empty() {
            return SlotRefKind::NoGf;
        }
        let quoted = quote_ident(to_var.ident());
        if let [table] = tables.as_slice() {
            if table.x.is_empty() {
                return SlotRefKind::NoGf;
            }
            // Only arrayed (ApplyToAll / Arrayed) targets have slots; a scalar
            // target's reference is the bare ident `with_lookup_table_ref`
            // returns. Emitting a bare arrayed ident here would be actively
            // wrong (it resolves each element's own table offset), so an
            // invariant break declines the wrap rather than guessing.
            debug_assert!(
                !target_ast_dims.is_empty(),
                "WithLookupSlotRefs::new called on a dimensionless target"
            );
            if target_ast_dims.is_empty() {
                return SlotRefKind::NoGf;
            }
            let ones = vec!["1"; target_ast_dims.len()].join(",");
            return SlotRefKind::Shared(format!("{quoted}[{ones}]"));
        }
        // Per-element tables: walk the row-major element enumeration once,
        // keeping the elements whose table slot actually carries a gf.
        let with_table: HashSet<crate::common::CanonicalElementName> =
            crate::dimensions::SubscriptIterator::new(target_ast_dims)
                .enumerate()
                .filter(|(offset, _)| tables.get(*offset).is_some_and(|t| !t.x.is_empty()))
                .map(|(_, subscripts)| {
                    crate::common::CanonicalElementName::from_raw(&subscripts.join(","))
                })
                .collect();
        SlotRefKind::PerElement { quoted, with_table }
    }

    /// The `LOOKUP` table reference for target element `elem`, or `None` when
    /// that element's compiled value applies no gf.
    pub(crate) fn for_element(&self, elem: &crate::common::CanonicalElementName) -> Option<String> {
        match &self.kind {
            SlotRefKind::NoGf => None,
            SlotRefKind::Shared(reference) => Some(reference.clone()),
            SlotRefKind::PerElement { quoted, with_table } => with_table
                .contains(elem)
                .then(|| format!("{quoted}[{}]", elem.as_str())),
        }
    }

    /// The reference for an `Ast::Arrayed` target's EXCEPT-default slot, whose
    /// single text is shared by every default-covered element. A shared
    /// variable-level table applies uniformly and is pinned like any other slot;
    /// per-element tables differ across those elements, so no single argument is
    /// correct and the default stays unwrapped (a documented GH #910 residual).
    pub(crate) fn for_default(&self) -> Option<String> {
        match &self.kind {
            SlotRefKind::Shared(reference) => Some(reference.clone()),
            SlotRefKind::NoGf | SlotRefKind::PerElement { .. } => None,
        }
    }
}
