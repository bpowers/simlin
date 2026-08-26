// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The one statement of what `PREVIOUS`/`INIT` can read directly out of the
//! snapshot regions, and therefore of when the parse must synthesize a capture
//! helper instead.
//!
//! Two stages ask the same question about the same call and must never answer
//! it differently:
//!
//! * the parse (`builtins_visitor`), over the source `Expr0` argument, decides
//!   whether to leave the argument in place or replace it with a reference to a
//!   synthesized `$⁚{parent}⁚{n}⁚arg0` capture helper;
//! * codegen (`compiler::codegen::static_slot` and
//!   `Compiler::snapshot_static_view`), over the lowered `Expr` argument,
//!   decides whether to emit a direct `LoadPrev`/`LoadInitial` against a fixed
//!   slot, a static view over the snapshot region, or a loud refusal.
//!
//! When those two drift, the dependency graph and the bytecode disagree about
//! what a variable reads -- the GH #568 failure class. Both call
//! [`SnapshotArg::access`], so the rule exists once; each stage only classifies
//! its own representation into [`SnapshotArg`], which is the part that cannot
//! be shared because the two representations are different languages.
//!
//! What this module does NOT decide: whether an addressable reference is
//! *admissible* in the position it appears. Codegen keeps three refusals of
//! its own over arguments this module calls addressable -- a view that names
//! one dimension twice (no usable projection), an array-valued `PREVIOUS` with
//! a non-default fallback (a view carries no per-call-site scalar state), and
//! an array-valued call in a scalar position outside an iteration. Those are
//! questions about what the reference *means* where it sits, not about whether
//! it addresses storage.

/// One subscript index of a `PREVIOUS`/`INIT` argument, by what the compiler
/// can resolve about it before the run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SnapshotIndex {
    /// Resolves to a fixed element at compile time: a numeric constant, a
    /// qualified `dimension·element` reference, or a bare element name that no
    /// variable shadows.
    Static,
    /// Leaves a whole dimension standing -- a wildcard, a star range, or a bare
    /// reference to one of the active apply-to-all dimensions -- so the
    /// reference is a view rather than a single slot.
    SpansDimension,
    /// Read at run time. Where it points is not known until the step runs, so
    /// no fixed slot and no fixed view addresses it.
    Dynamic,
}

/// A `PREVIOUS`/`INIT` argument reduced to the three facts that decide how, or
/// whether, the snapshot regions can be read through it directly.
///
/// Build one with [`SnapshotArg::whole`], [`SnapshotArg::subscripted`] or
/// [`SnapshotArg::not_storage`] rather than by hand, so the index-precedence
/// rule stays stated once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SnapshotArg {
    /// The argument is a reference to one variable's stored values. False for
    /// a computed expression, a temp, and a module-backed name (a module
    /// instance occupies a slot range whose sub-variable a fixed offset cannot
    /// name).
    base_names_storage: bool,
    /// At least one subscript index is read at run time.
    any_index_dynamic: bool,
    /// At least one subscript index leaves a whole dimension standing.
    any_dimension_standing: bool,
}

impl SnapshotArg {
    /// A bare, unsubscripted reference to one variable's stored values.
    pub(crate) const fn whole() -> Self {
        SnapshotArg {
            base_names_storage: true,
            any_index_dynamic: false,
            any_dimension_standing: false,
        }
    }

    /// Anything that does not reference one variable's stored values.
    pub(crate) const fn not_storage() -> Self {
        SnapshotArg {
            base_names_storage: false,
            any_index_dynamic: false,
            any_dimension_standing: false,
        }
    }

    /// A subscripted reference to one variable's stored values, folded from its
    /// classified indices.
    ///
    /// An index is counted as standing whenever it spans a dimension, even if
    /// it would also classify as `Static`. That precedence is load-bearing
    /// rather than arbitrary: a name can be both an active apply-to-all
    /// dimension and an element of some other dimension, and in that case the
    /// dimension it leaves standing is what the reference means. Classifying it
    /// as `Static` instead would collapse `x[Dim]` to one element before
    /// lowering can tell whether the position wants the element or the whole
    /// array (GH #995).
    pub(crate) fn subscripted(indices: impl IntoIterator<Item = SnapshotIndex>) -> Self {
        let mut arg = SnapshotArg {
            base_names_storage: true,
            any_index_dynamic: false,
            any_dimension_standing: false,
        };
        for index in indices {
            match index {
                SnapshotIndex::Static => {}
                SnapshotIndex::SpansDimension => arg.any_dimension_standing = true,
                SnapshotIndex::Dynamic => arg.any_index_dynamic = true,
            }
        }
        arg
    }

    /// What a direct read of this argument can address.
    ///
    /// This is the single statement of the rule. A reference into one
    /// variable's storage whose every index resolves before the run is
    /// addressable: a [`SnapshotAccess::Slot`] when the indices pin one
    /// element, a [`SnapshotAccess::View`] when at least one dimension is left
    /// standing. Everything else is a [`SnapshotAccess::Capture`] -- the parse
    /// must hoist it into a capture helper, and codegen must refuse a direct
    /// read of it rather than address the wrong storage.
    pub(crate) fn access(self) -> SnapshotAccess {
        if !self.base_names_storage || self.any_index_dynamic {
            SnapshotAccess::Capture
        } else if self.any_dimension_standing {
            SnapshotAccess::View
        } else {
            SnapshotAccess::Slot
        }
    }
}

/// What `PREVIOUS`/`INIT` can address directly for a given argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SnapshotAccess {
    /// One fixed slot. `LoadPrev`/`LoadInitial` reads it directly
    /// (`codegen::static_slot`). A view position wraps it as a one-element
    /// view.
    Slot,
    /// A static view over the snapshot region, with at least one dimension
    /// standing (`Compiler::snapshot_static_view`).
    View,
    /// Nothing addressable. The parse synthesizes a capture helper, whose own
    /// slot is then a `Slot` for the rewritten call; codegen refuses a direct
    /// read.
    Capture,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every combination of the [`SnapshotIndex`] alphabet, up to two indices,
    /// against the verdict.
    ///
    /// The rows are hand-written and that is legitimate here in a way it would
    /// not be for a fixture standing in for a model: the classified index list
    /// IS this function's domain, not a stand-in for something production
    /// derives. What production derives -- how an `Expr0` index or a lowered
    /// `ArrayView` becomes one of these three -- is checked against real
    /// equations by
    /// `db::prev_init_tests::every_prev_init_argument_shape_agrees_between_the_parse_and_codegen`.
    ///
    /// The mixed `SpansDimension`/`Static` rows are what pin the precedence,
    /// and they are the reason the fold asks about spanning first: an index can
    /// satisfy both classifications, and collapsing such a reference to one
    /// element would lose the dimension the position may still want.
    #[test]
    fn the_index_fold_covers_every_combination() {
        use SnapshotIndex::{Dynamic, SpansDimension, Static};

        let rows: &[(&[SnapshotIndex], SnapshotAccess)] = &[
            (&[], SnapshotAccess::Slot),
            (&[Static], SnapshotAccess::Slot),
            (&[SpansDimension], SnapshotAccess::View),
            (&[Dynamic], SnapshotAccess::Capture),
            (&[Static, Static], SnapshotAccess::Slot),
            (&[Static, SpansDimension], SnapshotAccess::View),
            (&[SpansDimension, Static], SnapshotAccess::View),
            (&[SpansDimension, SpansDimension], SnapshotAccess::View),
            (&[Static, Dynamic], SnapshotAccess::Capture),
            (&[Dynamic, Static], SnapshotAccess::Capture),
            (&[SpansDimension, Dynamic], SnapshotAccess::Capture),
            (&[Dynamic, SpansDimension], SnapshotAccess::Capture),
            (&[Dynamic, Dynamic], SnapshotAccess::Capture),
        ];
        for (indices, expected) in rows {
            assert_eq!(
                SnapshotArg::subscripted(indices.iter().copied()).access(),
                *expected,
                "indices {indices:?}"
            );
        }
    }

    /// The two non-subscripted constructors, which the fold does not reach.
    #[test]
    fn a_bare_reference_is_a_slot_and_a_non_reference_is_a_capture() {
        assert_eq!(SnapshotArg::whole().access(), SnapshotAccess::Slot);
        assert_eq!(SnapshotArg::not_storage().access(), SnapshotAccess::Capture);
    }
}
