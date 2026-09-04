// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The ceteris-paribus wrap's read side of the occurrence IR:
//! [`SlotOccurrences`] (a target's occurrence stream grouped by slot) and
//! [`OccurrenceLookup`] (one slot's view, keyed by structural path).
//!
//! Split out of `ltm_augment.rs` only to keep that file under the project
//! line-count lint; included via `#[path]`, so `super::*` resolves the parent's
//! private items and every caller keeps naming these
//! `crate::ltm_augment::*`.

use std::collections::HashMap;

use crate::common::{Canonical, Ident};
use crate::db::RefShape;
use crate::db::ltm_ir::{OccurrenceRef, OccurrenceSite};

/// The ceteris-paribus wrap's view of the occurrence IR for ONE slot of a
/// target equation: the per-occurrence access shape and per-axis
/// classification `db::ltm_ir` already decided on the target's `Expr2` AST,
/// keyed by the structural child-index path with the slot prefix stripped.
///
/// This is the SINGLE classifier family the wrap consumes. The wrap runs on
/// the target's printed-and-reparsed `Expr0`; rather than re-deriving each
/// occurrence's shape on that `Expr0` (the retired Expr0 mirror classifiers),
/// it tracks the same left-to-right child-index path `db::ltm_ir::walk_all_in_expr`
/// builds and looks the occurrence up here. The print->reparse round trip is
/// child-index-isomorphic to the `Expr2` walk (proved corpus-wide by
/// `classifier_agreement_tests::assert_occurrence_stream_aligns`), so a path
/// hit returns exactly the shape/axes the edge emitter used -- the two
/// families cannot drift because there is only one.
///
/// A path MISS on a live-source subscript is the one residual production hazard
/// -- a novel shape the alignment gate cannot cover could make the reparse
/// non-isomorphic. That is NOT silently tolerated: the subscript arm flags it
/// (`WrapOutcome::missing_occurrence`) on a non-empty stream, so the partial is
/// abandoned with a loud skip-and-warn instead of freezing the live reference
/// into a constant-0 score.
///
/// Slot paths are rebased to slot-local form because the wrap walks a single
/// slot's expression from its root: an `Ast::Scalar`/`ApplyToAll` target is
/// slot `0`; an `Ast::Arrayed` target's per-element slots are numbered in
/// canonical element-key-sorted order (`build_arrayed_link_score_equation`
/// wraps each slot separately), matching the SiteId slot prefix. Obtain one from
/// [`SlotOccurrences::for_slot`].
pub(crate) struct OccurrenceLookup<'a> {
    /// `(slot-local path, occurrence)` for every occurrence in this slot, in
    /// document order. LTM equations are short, so a linear scan within a slot
    /// is cheaper than hashing.
    entries: &'a [(&'a [u16], &'a OccurrenceSite)],
}

/// A target's occurrence stream GROUPED BY SLOT, built in one pass.
///
/// This exists for the arrayed case: `build_arrayed_link_score_equation` wraps
/// each of N element equations separately and needs that slot's occurrences
/// each time. Filtering the whole stream per slot made generating ONE edge's
/// score Theta(N^2) (plus N temporary vectors) -- measured at 37.7ms / 141ms /
/// 564ms / 2.32s for N = 50 / 100 / 200 / 400, a clean 4x per doubling. Grouping
/// once and indexing per slot makes it linear.
///
/// It is also the ONLY way to obtain an [`OccurrenceLookup`], deliberately: the
/// lookup borrows from this index, so the type system requires callers to hoist
/// it out of their per-element loop rather than rebuild it inside. (A
/// convenience constructor that built a throwaway index per call would compile
/// only by re-introducing the quadratic.)
pub(crate) struct SlotOccurrences<'a> {
    by_slot: HashMap<u16, Vec<(&'a [u16], &'a OccurrenceSite)>>,
}

impl<'a> SlotOccurrences<'a> {
    /// Group `occs` (a target's whole occurrence stream) by slot, rebasing each
    /// occurrence's SiteId to its slot-local path. One pass over the stream.
    pub(crate) fn new(occs: &'a [OccurrenceSite]) -> Self {
        let mut by_slot: HashMap<u16, Vec<(&'a [u16], &'a OccurrenceSite)>> = HashMap::new();
        for o in occs {
            // An occurrence always carries a slot prefix (`walk_all_in_expr`
            // pushes it before descending), so an empty SiteId cannot occur;
            // skipping rather than indexing keeps this total either way.
            if let Some(&slot) = o.site_id.0.first() {
                by_slot
                    .entry(slot)
                    .or_default()
                    .push((&o.site_id.0[1..], o));
            }
        }
        SlotOccurrences { by_slot }
    }

    /// The lookup for `slot`. Empty for a slot with no recorded occurrences
    /// (which [`OccurrenceLookup::is_empty`] distinguishes from a desync).
    pub(crate) fn for_slot(&self, slot: u16) -> OccurrenceLookup<'_> {
        OccurrenceLookup {
            entries: self.by_slot.get(&slot).map(Vec::as_slice).unwrap_or(&[]),
        }
    }
}

impl OccurrenceLookup<'static> {
    /// The lookup for a subtree the IR records NOTHING for, by design.
    ///
    /// There is exactly one such subtree: a `LOOKUP` TABLE argument, which
    /// `db::ltm_ir`'s walker skips whole ("a graphical-function table reference
    /// is static data, not a causal edge"). The wrap descends into that
    /// argument's subscript INDEX expressions anyway, because those ARE runtime
    /// value reads and have to be frozen for ceteris paribus (GH #984) -- and
    /// descending with the slot's real lookup would be unsound in two ways at
    /// once. A path hit under a table argument would be a COLLISION (the paths
    /// there belong to no recorded occurrence), and a MISS on a live-source
    /// subscript would trip the desync guard, whose premise -- an occurrence for
    /// every live-source subscript head -- is exactly what does not hold here.
    ///
    /// With an empty lookup the guard is inert (it keys on `is_empty`) and every
    /// shape lookup misses uniformly, so nothing under a table argument is
    /// treated as the live reference: a source read there is frozen, which is
    /// the conservative and ceteris-paribus-correct answer for a reference the
    /// IR attributes nothing to. The `PerElement` row pinning is unaffected --
    /// it discharges a table argument structurally, by name, needing no
    /// occurrence at all.
    pub(super) fn empty() -> Self {
        OccurrenceLookup { entries: &[] }
    }
}

impl<'a> OccurrenceLookup<'a> {
    /// The occurrence at exactly `path`, if any (a genuine causal reference at
    /// that node). `None` for a node that is not a recorded occurrence (a
    /// function name, a dimension name, a literal element selector, or a
    /// deeper index the walk skipped).
    pub(super) fn get(&self, path: &[u16]) -> Option<&'a OccurrenceSite> {
        self.entries
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, o)| *o)
    }

    /// Whether this slot recorded NO occurrences. `true` for a genuinely
    /// source-free slot -- an EXCEPT default that never references `from`, whose
    /// trivial-zero guard form is legitimate, or an AGG-source generator whose
    /// live source (the synthetic agg) is never a recorded occurrence. A miss on
    /// a NON-empty lookup, by contrast, is a walker desync -- the `missing_occurrence`
    /// guard in the subscript arm keys on this so it never false-fires on a
    /// legitimately source-free slot.
    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Does any occurrence of the live source (a model `Variable` or a
    /// `ModuleOutput` composite) STRICTLY under `prefix` carry access shape
    /// `shape`? This is the occurrence-IR form of the retired
    /// `expr0_contains_live_match` lookahead: an array-reducer `App` at
    /// `prefix` is frozen whole (GH #517) unless it genuinely holds the live
    /// reference, which is exactly "a live-shaped occurrence lives in its
    /// subtree".
    ///
    /// Index-nested occurrences (`other_arr[live]`) are EXCLUDED: the retired
    /// `expr0_contains_live_match` only inspected subscript *heads*, never
    /// recursing into a subscript's index expressions, so an index-nested
    /// `live` never made the enclosing reducer "hold the live ref". The
    /// occurrence IR marks those `index_nested`, so filtering on it reproduces
    /// that boundary exactly (the reducer freezes whole and the bare `live`
    /// stays live -- `char_reducer_index_nested_freeze`).
    ///
    /// A `live` source can be either a model `Variable` (the ordinary link
    /// score) or a `module·port` composite (`OccurrenceRef::ModuleOutput`, the
    /// `db::module_link_score_equation` live channel). The retired
    /// `expr0_contains_live_match` matched on the bare-`Var` ident text and so
    /// treated BOTH the same -- a composite read bare inside a reducer made the
    /// reducer hold the live ref. Matching on the occurrence's resolved name
    /// (whichever variant) reproduces that: a module-output live source inside a
    /// reducer recurses just like a variable one, instead of freezing whole.
    pub(super) fn subtree_has_live_shape(
        &self,
        prefix: &[u16],
        live: &Ident<Canonical>,
        shape: &RefShape,
    ) -> bool {
        self.entries.iter().any(|(p, o)| {
            p.len() > prefix.len()
                && p.starts_with(prefix)
                && !o.index_nested
                && occurrence_realizes_shape(o, shape)
                && occurrence_names_source(&o.reference, live)
        })
    }
}

/// Whether occurrence `occ` is a reading at the edge's access shape
/// `live_shape` -- the one predicate behind every by-shape live/frozen
/// decision the wrap makes (the subscript arm's live match, the whole-reducer
/// freeze's lookahead, the frozen-reference walker). Shapes match by
/// equality, with one deliberate widening: a `DynamicIndex` live shape is
/// also realized by a `Wildcard` occurrence inside a reducer. The
/// reference-site IR classifies the wildcard argument of a reducer it did
/// NOT hoist as `DynamicIndex` for the edge consumers (the conservative
/// cross-product), while the occurrence keeps the walker's `Wildcard`; read
/// by equality alone the wrap finds no live occurrence for such an edge,
/// freezes the reducer whole and scores it a silent 0: for
/// `x = other + SUM(pop * w[*])` with `w` live, `PREVIOUS(sum(pop * w[*]))`
/// moves with nothing. Held live, the argument stays and the co-sources
/// freeze around it (`sum(PREVIOUS(pop) * w[*])`).
pub(super) fn occurrence_realizes_shape(occ: &OccurrenceSite, live_shape: &RefShape) -> bool {
    &occ.shape == live_shape
        || (matches!(live_shape, RefShape::DynamicIndex)
            && matches!(occ.shape, RefShape::Wildcard)
            && occ.in_reducer)
}

/// Whether occurrence reference `reference` names the live source `live` --
/// either a model `Variable` or a `module·port` composite
/// (`OccurrenceRef::ModuleOutput`). The wrap treats both alike (a bare read of
/// either is a live match), so the occurrence-IR predicates must too.
pub(super) fn occurrence_names_source(reference: &OccurrenceRef, live: &Ident<Canonical>) -> bool {
    match reference {
        OccurrenceRef::Variable(v) => &Ident::<Canonical>::new(v) == live,
        OccurrenceRef::ModuleOutput { composite, .. } => {
            &Ident::<Canonical>::new(composite) == live
        }
    }
}
