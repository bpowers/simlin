// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Concrete-form lowerings for LTM link-score equations.
//!
//! The `agg → target` and `PerElement` link-score generators in `ltm_augment`
//! were inverted by the "transform-first" restructuring: the ceteris-paribus
//! wrap (`wrap_changed_first_ast`) runs on the target's OWN equation -- hoisted
//! reducers still spelled `SUM(...)`, the live source's occurrence held at its
//! actual shape -- and the concrete-form rewrite happens around it. This module
//! owns those lowerings:
//!
//! - [`substitute_reducers_in_expr0`] rewrites each hoisted reducer subtree in
//!   the wrapped AST to its synthetic aggregate node's bare name (the held-live
//!   reducer to `agg`, each frozen `PREVIOUS(SUM(..))` to `PREVIOUS(agg)`);
//! - the `PerElement` ROW PINNING -- [`pin_source_subscript_indices`] for one
//!   occurrence and [`pin_only_source_refs`] for a subtree the wrap froze --
//!   supported by [`PerElementRefCtx`], [`per_element_row_for_target`] (the
//!   single row derivation the link-score NAME and equation both consume), and
//!   [`qualify_axis_element`]. These are called FROM the ceteris-paribus wrap as
//!   it descends, not as a pass over its output: the wrap is the only place that
//!   knows both the occurrence (by path) and whether it is about to freeze the
//!   reference, and that second fact is what selects the bare-row spelling for
//!   the live occurrence over the qualified-row spelling for every other one.
//!   Running them afterward forced the lowering to re-derive each occurrence's
//!   per-axis access with an Expr0 classifier, since a `SiteId` computed on the
//!   original AST cannot address a tree the wrap has inserted `PREVIOUS` nodes
//!   into; that classifier is now deleted.
//!
//! Running the wrap on the target's own equation is what makes it
//! occurrence-addressable: it is keyed on the target's OWN occurrence stream, so
//! every decision comes from `db::ltm_ir`'s one classification. NONE of the
//! lowerings here re-classifies -- the agg substitution is a pure text-keyed AST
//! rewrite, and the row pinning reads `OccurrenceSite::axes`. See the generators
//! in `ltm_augment` -- `generate_agg_to_scalar_target_equation`,
//! `generate_scalar_to_element_equation`, and `generate_per_element_link_equation`
//! -- for the compositions that call these.
//!
//! Split out of `ltm_augment.rs` to keep that file under the project
//! line-count lint; included via `#[path]`, so `super::*` resolves the parent's
//! private items.

use std::collections::HashMap;

use crate::ast::{Expr0, IndexExpr0, print_eqn};
use crate::builtins::UntypedBuiltinFn;
use crate::common::{Canonical, Ident, RawIdent};

use super::{DepElementPin, OccurrenceLookup, qualify_element_csv};
use crate::db::ltm_ir::{OccurrenceAxis, OccurrenceSite};

#[cfg(test)]
use super::PartialEquationError;
#[cfg(test)]
use crate::lexer::LexerType;

/// Read-only context for the `PerElement` row-pinning lowering: everything it
/// needs to substitute the live source's subscript indices for one
/// `(site, target element)` instantiation of a `PerElement` link score
/// (GH #525, T6 of the shape-expressiveness design).
///
/// It carries no per-axis element-name lists and no iterated-dim recognition
/// context: the per-axis truth comes off the occurrence IR
/// (`OccurrenceSite::axes`), so this lowering holds projection data only and
/// never classifies an axis itself.
pub(super) struct PerElementRefCtx<'a> {
    /// The live source variable (canonical).
    pub(super) from: &'a Ident<Canonical>,
    /// The emitting site's per-axis access vector
    /// ([`RefShape::PerElement`]'s `axes`).
    pub(super) site_axes: &'a [crate::ltm_agg::AxisRead],
    /// The row this `(site, e)` instantiation reads -- BARE element names,
    /// one per source axis (parallel to `site_axes`). The wrap holds the
    /// `site_axes`-shaped occurrence live (its shape equals `site_axes`); the
    /// lowering rewrites that live occurrence to exactly these bare indices so
    /// it prints as the historical `from[<row>]`.
    pub(super) row_parts_bare: &'a [String],
    /// The source's declared dimensions (for index qualification).
    pub(super) from_dims: &'a [crate::dimensions::Dimension],
    /// The target equation's dimensions, in axis order and WITH duplicates --
    /// the positional twin of `target_elem_by_dim`, which a repeated dimension
    /// makes unrepresentable. [`dep_row_for_target`] consumes these.
    pub(super) target_dims: &'a [crate::dimensions::Dimension],
    /// This instantiation's target element, one bare name per axis of
    /// `target_dims`.
    pub(super) target_elements: &'a [String],
    /// Target-iterated dim (canonical) -> (element of `e` for that dim,
    /// its index within the dim) -- the projection data `e` supplies.
    pub(super) target_elem_by_dim: &'a HashMap<String, (String, usize)>,
    pub(super) dim_ctx: &'a crate::dimensions::DimensionsContext,
}

/// Qualify one element of one axis (`"nyc"` over `Region` ->
/// `"region\u{B7}nyc"`), via the same defensive rules as
/// [`qualify_element_csv`].
pub(super) fn qualify_axis_element(elem: &str, dim: &crate::dimensions::Dimension) -> String {
    qualify_element_csv(elem, std::slice::from_ref(dim))
}

/// The source row a per-axis access vector reads for one full target
/// element: project the target element onto the `Iterated` / `MappedRead`
/// axes and fill `Pinned` axes with their literals. One bare element name
/// per axis, in source-axis order. `None` when a projected axis's dim is
/// missing from the target projection or its correspondence is unusable (a
/// mid-edit inconsistency; callers degrade conservatively) -- and for any
/// `Reduced` axis, which the `PerElement` invariant excludes.
///
/// Both projected axes read the one executed correspondence
/// (`DimensionsContext::executed_read_correspondence`, indexed by TARGET
/// element position and yielding the source element the executed simulation
/// reads for it): an `Iterated` index and a `MappedRead` index differ only in
/// which target dimension they are driven by, and every dimension-named
/// subscript survives to `IndexOp::ActiveDimRef` and is resolved name-first,
/// then through the declared element map (GH #997).
///
/// This is the SINGLE row derivation for the `PerElement` family's
/// emission: the link-score NAME's row (computed by
/// `emit_per_element_link_scores`) and the equation's live-reference row
/// (computed by [`pin_source_subscript_indices`]) both come from here,
/// so they cannot disagree.
pub(crate) fn per_element_row_for_target(
    axes: &[crate::ltm_agg::AxisRead],
    target_elem_by_dim: &HashMap<String, (String, usize)>,
    dim_ctx: &crate::dimensions::DimensionsContext,
) -> Option<Vec<String>> {
    use crate::common::CanonicalDimensionName;
    use crate::ltm_agg::AxisRead;
    axes.iter()
        .map(|ax| match ax {
            AxisRead::Pinned(e) => Some(e.clone()),
            AxisRead::Iterated { dim, source_dim } | AxisRead::MappedRead { dim, source_dim } => {
                let (elem, idx) = target_elem_by_dim.get(dim)?;
                if dim == source_dim {
                    return Some(elem.clone());
                }
                let corr = dim_ctx.executed_read_correspondence(
                    &CanonicalDimensionName::from_raw(dim),
                    &CanonicalDimensionName::from_raw(source_dim),
                )?;
                corr.get(*idx).map(|e| e.as_str().to_string())
            }
            AxisRead::Reduced { .. } => None,
        })
        .collect()
}

/// Per DECLARED dimension of a dep, the element it reads at ONE target element
/// when referenced BARE -- `None` for a dimension the target element does not
/// supply. Bare element names, in the dep's own declaration order.
///
/// A bare arrayed reference inside an apply-to-all body reads, for each of its
/// OWN axes, the element the enclosing iteration selects on the target axis that
/// axis is allocated to. `growth[Region,Age] = ... w ...` over a `w[Age]` reads
/// `w[<the Age coordinate>]`, and over a `w[Age,Region]` reads
/// `w[<Age>,<Region>]` -- the transpose of the target's own tuple. That is the
/// executed behaviour (pinned by
/// `bare_arrayed_dep_is_pinned_over_its_own_declared_dims`' numeric oracle), and
/// it is why pinning such a reference with the target's FULL element tuple is
/// wrong twice over: over- or under-arity when the dep declares a strict subset
/// (a fragment that fails to compile, so the score reads a constant 0), and a
/// compilable, SILENTLY WRONG element when it declares the same dimensions in
/// another order (GH #974).
///
/// **The ALLOCATION is not decided here.** Which target axis supplies which dep
/// axis, and through which correspondence, is
/// [`crate::db::bare_axis_pairing`] -- the one pairing of two declared
/// dimension lists, the same answer behind the element graph's
/// `expand_same_element` and the arrayed score's admission -- so a pin cannot
/// spell a row the simulation does not read. A per-axis `.find` over the
/// target's dimension NAMES must not stand in for it, because a name is not an
/// axis identity: a target repeating a dimension (`target[D,D]`) has two axes
/// with one name, and a name-keyed map keeps only the last (the simulation
/// reads the FIRST, measured by `repeated_target_dimension_reads_the_first_axis`);
/// and two dep axes that can each map to the same target axis would both claim
/// it, because an independent search tracks no `used` set (the simulation
/// allocates one-to-one, measured by
/// `doubly_mapped_dep_axes_are_allocated_one_to_one`). A table keyed by
/// dimension name is what produced GH #986's wrong row, so the rule is asked
/// for rather than restated.
fn dep_axis_elements(
    dep_dims: &[crate::dimensions::Dimension],
    target_dims: &[crate::dimensions::Dimension],
    target_elements: &[String],
    dim_ctx: &crate::dimensions::DimensionsContext,
) -> Vec<Option<String>> {
    use crate::common::CanonicalElementName;
    use crate::db::BareAxis;
    if target_dims.len() != target_elements.len() {
        return vec![None; dep_dims.len()];
    }
    crate::db::bare_axis_pairing(
        dep_dims,
        target_dims,
        dim_ctx,
        crate::db::BareSpelling::Equation,
    )
    .iter()
    .map(|axis| match axis {
        BareAxis::Positional(pos) => target_elements.get(*pos).cloned(),
        BareAxis::Mapped { pos, reads } => {
            let target_dim = target_dims.get(*pos)?;
            let target_elem = CanonicalElementName::from_raw(target_elements.get(*pos)?);
            let idx = target_dim.get_offset(&target_elem)?;
            reads.get(idx).map(|e| e.as_str().to_string())
        }
        BareAxis::Collapsed => None,
    })
    .collect()
}

/// The element a variable declared over `dep_dims` reads at ONE target element
/// when referenced BARE, one bare element name per declared dimension --
/// [`dep_axis_elements`] with every axis resolved. `None` when some declared
/// dimension does not project, because a BARE reference has to be spelled at
/// the dep's full arity or not at all.
pub(crate) fn dep_row_for_target(
    dep_dims: &[crate::dimensions::Dimension],
    target_dims: &[crate::dimensions::Dimension],
    target_elements: &[String],
    dim_ctx: &crate::dimensions::DimensionsContext,
) -> Option<Vec<String>> {
    dep_axis_elements(dep_dims, target_dims, target_elements, dim_ctx)
        .into_iter()
        .collect()
}

/// The element-pin table for ONE target element: each dep in `pinnable` mapped
/// to what IT reads there ([`DepElementPin`]), qualified in its own dimensions'
/// space so a frozen read compiles to a direct LoadPrev.
///
/// Per dep axis, one entry per DIMENSION NAME a subscript index on that axis
/// may spell, each resolved by the rule the compiler applies to that spelling:
///
/// - the axis's own dimension -- `dep[Region]` over a `Region` axis, or
///   `mapped[State]` over a `State` axis the target reads through a declared
///   map (`IndexOp::ActiveDimRef` resolves it name-first, then through the
///   map, GH #997) -- reads the element of the bare row, since a bare `dep` is
///   what pass 0 rewrites into exactly this spelling;
/// - every dimension the target iterates that reads this axis --
///   `energy[nonrenewable]` over a `source` axis, `w[Region]` over a `State`
///   axis mapped onto `Region` -- reads that dimension's coordinate of the
///   target element through `DimensionsContext::executed_read_correspondence`,
///   the derivation [`per_element_row_for_target`] runs for the live source's
///   `Iterated` axes, so a frozen other-dep and the live reference cannot
///   disagree about one spelling.
///
/// The entries are grouped by AXIS POSITION and looked up by an index's
/// position and spelled name, never by name alone: two dep axes can both be
/// readable through one target dimension (`m[State, County]` with both mapped
/// onto `Region`), and a name-keyed table hands the second axis the first's
/// element.
///
/// A dep no axis of which has an entry is ABSENT from the table entirely
/// (nothing to rewrite). A dep only SOME of whose axes project is present but
/// has no [`bare_row`](DepElementPin::bare_row): its dimension-name indices are
/// still substituted -- that is the GH #654 helper-aux fix, and it only needs
/// the axes the reference actually spells as dimension names -- while a BARE
/// reference to it is left alone, since no correct full-arity subscript
/// exists. Leaving it bare is the loud direction: a bare multi-slot reference
/// in a scalar fragment fails to compile and surfaces an `Assembly` warning,
/// where the pre-GH #974 full-target-tuple pin silently mis-read the dep
/// whenever the arity happened to match.
///
/// `pinnable` carries each dep's declared `Dimension`s, resolved ONCE per
/// target equation by the caller; only the projection is per element.
pub(crate) fn dep_element_pins(
    pinnable: &[(Ident<Canonical>, Vec<crate::dimensions::Dimension>)],
    target_dims: &[crate::dimensions::Dimension],
    target_elements: &[String],
    dim_ctx: &crate::dimensions::DimensionsContext,
) -> HashMap<Ident<Canonical>, DepElementPin> {
    use crate::common::CanonicalElementName;
    pinnable
        .iter()
        .filter_map(|(ident, dep_dims)| {
            let bare = dep_axis_elements(dep_dims, target_dims, target_elements, dim_ctx);
            let axes: Vec<Vec<(String, String)>> = dep_dims
                .iter()
                .zip(&bare)
                .map(|(dep_dim, bare_elem)| {
                    let mut spellings: Vec<(String, String)> = bare_elem
                        .iter()
                        .map(|e| (dep_dim.name().to_string(), qualify_axis_element(e, dep_dim)))
                        .collect();
                    if target_dims.len() != target_elements.len() {
                        return spellings;
                    }
                    for (target_dim, target_elem) in target_dims.iter().zip(target_elements) {
                        // The axis's own name is the bare row's entry above, and
                        // a repeated target dimension is the FIRST axis's read.
                        if spellings.iter().any(|(name, _)| name == target_dim.name()) {
                            continue;
                        }
                        let Some(reads) = dim_ctx.executed_read_correspondence(
                            target_dim.canonical_name(),
                            dep_dim.canonical_name(),
                        ) else {
                            continue;
                        };
                        let Some(idx) =
                            target_dim.get_offset(&CanonicalElementName::from_raw(target_elem))
                        else {
                            continue;
                        };
                        if let Some(e) = reads.get(idx) {
                            spellings.push((
                                target_dim.name().to_string(),
                                qualify_axis_element(e.as_str(), dep_dim),
                            ));
                        }
                    }
                    spellings
                })
                .collect();
            let bare_row = bare.iter().all(Option::is_some).then(|| {
                axes.iter()
                    .map(|spellings| spellings[0].1.clone())
                    .collect()
            });
            if axes.iter().all(Vec::is_empty) && bare_row.is_none() {
                return None;
            }
            Some((ident.clone(), DepElementPin { axes, bare_row }))
        })
        .collect()
}

/// The per-axis [`crate::ltm_agg::AxisRead`] slice of a live-source subscript
/// occurrence, or `None` when the occurrence is not fully describable per axis.
///
/// This is the IR-driven replacement for the retired Expr0 per-axis classifier
/// (`classify_expr0_per_element_axes`). The equivalence is exact rather than
/// approximate: that classifier accepted an index only when it was an
/// iterated-dimension name lined up with the source's axis at that position, or
/// a literal element of that axis, with the arity matching on all of
/// indices / source dims / source element lists. The IR reaches the SAME
/// per-axis verdict through [`crate::ltm_agg::classify_axis_access`] -- the
/// shared classifier the retired `expr0_iterated_axis_lines_up` mirrored gate
/// for gate -- so "every axis `Iterated` or `Pinned`, arity equal to the
/// source's declared arity" is the same predicate. A `Reduced` axis (a wildcard
/// or star-range index), a `MismatchedIterated` one, and a `Dynamic` one all
/// fall out here exactly as they produced `None` there. An over-arity index can
/// never be `Iterated` (the IR only consults `classify_axis_access` where the
/// source has an axis at that position), so the old `i < from_dims.len()` guard
/// is implied rather than dropped.
fn axes_as_read_slice(occ: &OccurrenceSite, arity: usize) -> Option<Vec<crate::ltm_agg::AxisRead>> {
    use crate::ltm_agg::AxisRead;
    if occ.axes.is_empty() || occ.axes.len() != arity {
        return None;
    }
    occ.axes
        .iter()
        .map(|a| match a {
            OccurrenceAxis::Pinned(e) => Some(AxisRead::Pinned(e.clone())),
            OccurrenceAxis::Iterated { dim, source_dim } => Some(AxisRead::Iterated {
                dim: dim.clone(),
                source_dim: source_dim.clone(),
            }),
            OccurrenceAxis::MappedRead { dim, source_dim } => Some(AxisRead::MappedRead {
                dim: dim.clone(),
                source_dim: source_dim.clone(),
            }),
            OccurrenceAxis::Reduced { .. }
            | OccurrenceAxis::MismatchedIterated { .. }
            | OccurrenceAxis::Dynamic => None,
        })
        .collect()
}

/// The row indices for one occurrence of the live source, QUALIFIED
/// (`region\u{B7}a`) so a frozen read compiles to a direct LoadPrev in the
/// scalar fragment.
fn qualified_row_indices(row: &[String], ctx: &PerElementRefCtx<'_>) -> Vec<IndexExpr0> {
    row.iter()
        .zip(ctx.from_dims)
        .map(|(part, dim)| {
            IndexExpr0::Expr(Expr0::Var(
                RawIdent::new_from_str(&qualify_axis_element(part, dim)),
                crate::ast::Loc::default(),
            ))
        })
        .collect()
}

/// Row-pin ONE subscript occurrence of the live source for a `PerElement` link
/// score, given the occurrence the IR recorded at its node
/// (GH #525, T6 of the shape-expressiveness design).
///
/// `live` is whether the ceteris-paribus wrap is leaving this occurrence LIVE --
/// its shape equals the emitting site's AND it is not inside a subtree the wrap
/// froze. Only the wrap can answer that, which is why the pinning runs inside
/// the wrap's own traversal rather than as a pass over the wrapped tree; see
/// the module docs.
///
/// - the LIVE occurrence is rewritten to the row's BARE element indices, so it
///   prints as the historical `from[<row>]`;
/// - any other fully-describable occurrence (a different `PerElement` shape, an
///   all-`Iterated` Bare-shaped subscript, an all-`Pinned` literal subscript) is
///   rewritten to ITS OWN row for this target element, QUALIFIED, so the freeze
///   the wrap puts around it compiles to a direct LoadPrev;
/// - a partially-describable subscript (a wildcard slice, a dynamic index) gets
///   only its `Iterated` indices substituted (qualified; meaning-preserving --
///   the iterated dim IS that element in this slot) and leaves the rest to
///   `recurse_index`, which is the wrap's own index pass, so a genuinely dynamic
///   index still gets its `PREVIOUS(idx)` lag.
///
/// A node with NO recorded occurrence is left untouched HERE. That is the loud
/// path, not a silent one: the wrap's own `missing_occurrence` guard fires on a
/// live-source subscript whose path misses on a non-empty stream, and the caller
/// abandons the partial with a warning. ([`pin_only_source_refs`] is the one
/// caller that first substitutes such a subscript's indices structurally, by
/// name, because a `LOOKUP` table argument legitimately has no occurrence and
/// still has to compile; see [`pin_dimension_name_indices`].)
pub(super) fn pin_source_subscript_indices(
    indices: Box<[IndexExpr0]>,
    node_occ: Option<&OccurrenceSite>,
    ctx: &PerElementRefCtx<'_>,
    live: bool,
    mut recurse_index: impl FnMut(usize, IndexExpr0) -> IndexExpr0,
) -> Box<[IndexExpr0]> {
    let describable = node_occ.and_then(|o| axes_as_read_slice(o, ctx.from_dims.len()));
    if let Some(occ_axes) = describable {
        if live && occ_axes == ctx.site_axes {
            return ctx
                .row_parts_bare
                .iter()
                .map(|p| {
                    IndexExpr0::Expr(Expr0::Var(
                        RawIdent::new_from_str(p),
                        crate::ast::Loc::default(),
                    ))
                })
                .collect();
        }
        if let Some(row) =
            per_element_row_for_target(&occ_axes, ctx.target_elem_by_dim, ctx.dim_ctx)
        {
            return qualified_row_indices(&row, ctx).into();
        }
        return indices;
    }
    // Partially describable: substitute only the axes the IR classified as
    // PROJECTED -- `Iterated` or (GH #997) `MappedRead`, each carrying its own
    // resolution rule into the shared row derivation -- and hand every other
    // index to the wrap's own index pass.
    let axes = node_occ.map(|o| o.axes.as_slice()).unwrap_or(&[]);
    indices
        .into_iter()
        .enumerate()
        .map(|(i, idx)| {
            let projected_axis = match axes.get(i) {
                Some(OccurrenceAxis::Iterated { dim, source_dim }) => {
                    Some(crate::ltm_agg::AxisRead::Iterated {
                        dim: dim.clone(),
                        source_dim: source_dim.clone(),
                    })
                }
                Some(OccurrenceAxis::MappedRead { dim, source_dim }) => {
                    Some(crate::ltm_agg::AxisRead::MappedRead {
                        dim: dim.clone(),
                        source_dim: source_dim.clone(),
                    })
                }
                _ => None,
            };
            let substituted = match (projected_axis, ctx.from_dims.get(i)) {
                (Some(ax), Some(from_dim)) => per_element_row_for_target(
                    std::slice::from_ref(&ax),
                    ctx.target_elem_by_dim,
                    ctx.dim_ctx,
                )
                .map(|row| qualify_axis_element(&row[0], from_dim)),
                _ => None,
            };
            match substituted {
                Some(part) => IndexExpr0::Expr(Expr0::Var(
                    RawIdent::new_from_str(&part),
                    crate::ast::Loc::default(),
                )),
                None => recurse_index(i, idx),
            }
        })
        .collect()
}

/// Row-pin a BARE `Var` reference to the live source (the mixed
/// `Bare`+`PerElement` edge's other site): each axis reads the target element's
/// coordinate for that axis's own dimension, qualified.
///
/// The projection is [`dep_row_for_target`]'s -- the same rule that pins a bare
/// arrayed OTHER-dep -- so a positionally-MAPPED axis resolves through the
/// correspondence rather than being declined (GH #974). Before that it matched
/// dimension names only, so `growth[State,Age] = pop[State,young] * pop` over a
/// `pop[Region,Age]` source found no `Region` coordinate, left the bare
/// reference alone, and the wrap froze it into a bare multi-slot
/// `PREVIOUS(pop)` that cannot compile in a scalar fragment -- every
/// per-element score on the edge silently read a constant 0.
///
/// `None` when some axis does not resolve -- the caller then leaves the bare
/// reference for the wrap's conservative freeze.
pub(super) fn pin_bare_source_ref(ctx: &PerElementRefCtx<'_>) -> Option<Vec<IndexExpr0>> {
    dep_row_for_target(
        ctx.from_dims,
        ctx.target_dims,
        ctx.target_elements,
        ctx.dim_ctx,
    )
    .map(|row| qualified_row_indices(&row, ctx))
}

/// What [`pin_dimension_name_indices`] can say about ONE index of a source
/// subscript the occurrence IR records nothing for.
enum IndexVerdict {
    /// A static selector this rule rewrites: an iterated coordinate, or a literal
    /// element qualified with its own axis.
    Pinned(String),
    /// Nothing to do -- the index is left exactly as written. Two kinds land
    /// here and the rule treats them alike because nothing downstream
    /// distinguishes them: a STATIC selector it would spell the same way (a
    /// numeric literal, an `@N` position, an already-`dim·elem` name), and a
    /// RUNTIME read (`pop[Region, idx]`) which this rule cannot spell and does
    /// not need to. Its ceteris-paribus obligation is discharged before this
    /// descent runs: by [`crate::ltm_augment::freeze_lookup_table_indices`] on a
    /// bare `LOOKUP` table argument (which is why an index reaching here as a
    /// bare `Var` at all means the wrap did not run over it), and by the
    /// enclosing freeze on the descents the wrap does not enter -- a pre-existing
    /// `PREVIOUS`/`INIT`, a whole-frozen reducer (GH #984).
    Keep,
    /// No pin can spell it, because NEITHER shared row derivation
    /// ([`per_element_row_for_target`] on an `Iterated` or a `MappedRead` axis)
    /// can resolve it: a dimension the target does not iterate and no iterated
    /// dimension is mapped to, an iterated dimension with no usable
    /// correspondence to this source axis (an undeclared pair with disjoint
    /// names, or a transposition), an ambiguous mapped pairing, an index no
    /// axis owns. Left alone it keeps a
    /// DIMENSION-name subscript, which cannot resolve in a scalar fragment, so
    /// this one is LOUD -- a compilability verdict.
    Unspellable,
}

/// The [`crate::ltm_agg::AxisRead::MappedRead`] axis for a subscript index that
/// names the non-active dimension `index_dim` against source axis `axis_dim`, or
/// `None` when execution pairs it with no single iterated dimension of this
/// target (GH #997).
///
/// The pairing and its usability gate are `DimensionsContext`'s, so this asks
/// the same two questions `ltm_agg::classify_axis_access`'s `Unresolved` arm
/// asks and cannot accept a spelling the classifier rejects. It builds an
/// `AxisRead` only to hand to the shared row derivation; it decides no shape.
fn mapped_read_axis(
    index_dim: &str,
    axis_dim: &crate::dimensions::Dimension,
    ctx: &PerElementRefCtx<'_>,
) -> Option<crate::ltm_agg::AxisRead> {
    use crate::common::CanonicalDimensionName;
    let target_iterated: Vec<String> = ctx.target_elem_by_dim.keys().cloned().collect();
    let index_canon = CanonicalDimensionName::from_raw(index_dim);
    let partner = ctx
        .dim_ctx
        .mapped_read_partner_dim(&index_canon, &target_iterated)?;
    ctx.dim_ctx
        .executed_read_correspondence(&partner, axis_dim.canonical_name())?;
    Some(crate::ltm_agg::AxisRead::MappedRead {
        dim: partner.as_str().to_string(),
        source_dim: axis_dim.name().to_string(),
    })
}

/// Row-pin a source subscript the occurrence IR deliberately records NOTHING
/// for, by NAME alone. Returns the rewritten indices plus whether the rule
/// DISCHARGED the subscript.
///
/// This is a lowering-COMPLETENESS rule, not a classifier, and that distinction
/// is the whole reason it exists. `db::ltm_ir` records no occurrence under a
/// `LOOKUP` TABLE argument (`BuiltinContents::LookupTable(_) => {}` -- "a
/// graphical-function table reference is static data, not a causal edge"), and it
/// is RIGHT: such a reference carries no causal edge, so it earns no attribution,
/// no element edge and no score. But the lowering is still obliged to emit a
/// COMPILABLE scalar fragment, and a dimension-name subscript
/// (`effect[region, old]`) cannot resolve in one. Compilability and attribution
/// are two separate obligations, and discharging the second does not require the
/// first -- which is why this asks the IR for nothing.
///
/// It consults NO occurrence and infers NO shape. Per index, by name:
///
/// - an index the source's axis at that position DECLARES as an element (or an
///   already-`dim·elem`-qualified one) is a literal selector, qualified with that
///   axis (`old` -> `age·old`). Elements are tried FIRST, and that precedence is
///   not this rule's to choose: it is the shared
///   [`crate::dimensions::resolve_axis_index_name`], which
///   `ltm_agg::classify_axis_access` reads too (GH #986 closed the divergence),
///   and which follows the compiler's own `normalize_subscripts3`. See that
///   function for why element-first is the right order and what the XMILE spec
///   does and does not settle about it.
///   Qualifying rather than leaving the element bare matters even though it would very
///   likely resolve bare (see `wrap_non_matching_in_previous`'s
///   `skip_index_qualification`): the wrap's generic `qualify_element_index` cannot
///   qualify an element name several dimensions declare, so a half-qualified subscript
///   is the one spelling whose compilability depends on the model's element names.
///   Qualifying here also makes this rule's output byte-identical to the
///   pre-`391bc3c1` pass's, which is the conservative thing for a regression fix to be;
/// - otherwise, an index spelling one of the TARGET's ITERATED dimensions is replaced by the
///   source element this target element reads on that axis -- derived by handing
///   an [`crate::ltm_agg::AxisRead::Iterated`] for the `(index dim, source axis
///   dim)` pair to [`per_element_row_for_target`], the SAME single row derivation
///   the occurrence-driven pin uses. That is what makes the identity axis
///   (`pop[Region, ..]` over a `Region` axis, the structural substitution
///   [`pin_bare_source_ref`] performs for a bare `Var`) and a MAPPED axis
///   (`effect[State, ..]` over a `Region` axis with a `State`/`Region` mapping,
///   either declaration direction -- GH #527 / #757) ONE arm rather than two: the
///   derivation resolves both through `executed_read_correspondence`, the rule
///   execution applies to every dimension-named index;
/// - otherwise, an index spelling a NON-ITERATED dimension that execution pairs
///   with one of the target's iterated dims (`effect[Aggregated Regions, ..]`
///   under a `COP`-iterating target -- C-LEARN's shape, GH #997) is replaced the
///   same way, through an [`crate::ltm_agg::AxisRead::MappedRead`] so the row
///   derivation applies the name-first-then-element-map rule THIS spelling gets.
///   The pairing and its gate are `DimensionsContext::mapped_read_partner_dim` /
///   `executed_read_correspondence`, the same two questions
///   `ltm_agg::classify_axis_access` asks, so this rule accepts exactly the
///   spellings the classifier accepts;
/// - an axis BOTH derivations decline -- no mapping either way, a transposition,
///   an ambiguous pairing, a dimension this target does not iterate -- is
///   `IndexVerdict::Unspellable`, and it is unspellable because the SHARED
///   derivations say so, not because the name differs;
/// - anything that is not a bare identifier is kept verbatim, because this rule has
///   nothing to say about it. A numeric literal, arithmetic over literals, and an
///   `@N` POSITION index (which `compiler::context`'s subscript lowering resolves
///   to a concrete element offset in scalar context) all select a FIXED element and
///   need no pin -- spelling `@N` out as an element name here would be a SECOND
///   implementation of position syntax living outside the compiler that owns it. A
///   compound expression selecting the element at RUNTIME (`idx + 1`, a nested
///   source read) needs no pin either: it already compiles, and its
///   ceteris-paribus obligation is discharged before this descent runs. So all of
///   them are `IndexVerdict::Keep` and the rule does not have to tell them apart.
///
/// The one LOUD verdict is `IndexVerdict::Unspellable`, and it is a COMPILABILITY
/// verdict: a dimension-name subscript that survives into a scalar fragment does
/// not resolve, so the partial is abandoned rather than emitted.
///
/// A RUNTIME index is not a loud verdict here, and the reason is upstream: a
/// bare `LOOKUP` table argument is the one place the wrap would otherwise wrap
/// nothing, leaving the index live so that `codegen::extract_table_info`
/// evaluated it at the current step and the partial isolating one source
/// varied with another's movement (GH #984). That is discharged at the source:
/// [`crate::ltm_augment::freeze_lookup_table_indices`] puts the table
/// argument's indices through the wrap's own index pass, having first WIDENED
/// that descent's dep set with the indices' own idents -- without which the
/// freeze would not fire at all, since `classify_dependencies` does not walk a
/// table expression and so reports no dependency for an index variable
/// referenced only there. A runtime index therefore arrives here already frozen
/// (or, inside an enclosing freeze, already lagged) for ANY ident, not just one
/// that happens to be a dependency elsewhere. One rule discharges it on every
/// path, and this rule keeps it.
///
/// There is no `RefShape` here, no live-vs-frozen decision, and this can never
/// make the reference live-selectable (the pin-only descent records no
/// `live_ref`). It builds an `AxisRead` only to ASK the shared row derivation a
/// question; it never decides an access shape. Do NOT grow it into a per-axis
/// classifier: the second classifier family was deleted on purpose (`391bc3c1`),
/// and every per-axis question it needs answered is already answered by
/// [`per_element_row_for_target`] and the `DimensionsContext` beneath it.
///
/// (`ltm_agg::classify_axis_access` -- the per-axis classifier the IR itself uses
/// -- is deliberately NOT the helper consulted here, for two reasons. It consumes
/// `IndexExpr2`, and this descent walks the `Expr0` the wrap lowered. More
/// importantly it answers a DIFFERENT question: "is this axis access statically
/// describable enough to hoist a reducer / emit an element edge?" -- which is why
/// it declines `@N`, correctly for hoisting and wrongly for compilability, and why
/// it has a `Reduced` verdict that means nothing to a pin. The shared answer this
/// rule needs is one level down, at the row derivation and the correspondence, and
/// both classifiers bottom out there.)
///
/// The caller turns `discharged == false` into `WrapOutcome::missing_occurrence`,
/// i.e. a warned skip.
fn pin_dimension_name_indices(
    indices: Box<[IndexExpr0]>,
    ctx: &PerElementRefCtx<'_>,
) -> (Box<[IndexExpr0]>, bool) {
    let mut discharged = true;
    let indices = indices
        .into_iter()
        .enumerate()
        .map(|(i, idx)| {
            // Only a bare identifier can name an element or a dimension, which is
            // the only question this rule answers. Everything else -- an `@N`
            // position, a numeric literal, a compound expression, a range or
            // wildcard (which cannot be a table index at all:
            // `codegen::extract_table_info` needs a subscript selecting exactly ONE
            // element and rejects anything wider as `BadTable`) -- is `Keep`.
            let IndexExpr0::Expr(Expr0::Var(name, loc)) = &idx else {
                return idx;
            };
            let (name, loc) = (crate::common::canonicalize(name.as_str()).to_string(), *loc);
            let Some(dim) = ctx.from_dims.get(i) else {
                // Over-arity: no axis owns this index, so nothing names its owner
                // and there is nothing to resolve it against.
                discharged = false;
                return idx;
            };
            // An already-`dim·elem`-qualified index carries its own dimension, so
            // it poses no element-vs-dimension-name question at all: it is static
            // and already spelled the way this rule would spell it.
            let verdict = if ctx.dim_ctx.lookup(&name).is_some() {
                IndexVerdict::Keep
            } else {
                match crate::dimensions::resolve_axis_index_name(&name, dim, |n| {
                    ctx.target_elem_by_dim.contains_key(n)
                }) {
                    // A literal element selector of THIS axis, qualified with that
                    // axis (`old` -> `age·old`).
                    crate::dimensions::AxisIndexName::Element(elem) => {
                        IndexVerdict::Pinned(qualify_axis_element(&elem, dim))
                    }
                    // The index names one of the TARGET's ITERATED dimensions, so this
                    // axis reads whatever element this target element projects onto it.
                    // WHICH element that is -- the identity for a same-named axis, the
                    // executed correspondence for a foreign one -- is the shared row
                    // derivation's answer, not this rule's: it declines an undeclared
                    // disjoint-name pair, and a name-directed pin therefore accepts
                    // exactly the pairs the occurrence-driven one does.
                    crate::dimensions::AxisIndexName::IteratedDim => {
                        let axis = crate::ltm_agg::AxisRead::Iterated {
                            dim: name.clone(),
                            source_dim: dim.name().to_string(),
                        };
                        match per_element_row_for_target(
                            std::slice::from_ref(&axis),
                            ctx.target_elem_by_dim,
                            ctx.dim_ctx,
                        ) {
                            Some(row) => IndexVerdict::Pinned(qualify_axis_element(&row[0], dim)),
                            None => IndexVerdict::Unspellable,
                        }
                    }
                    crate::dimensions::AxisIndexName::Unresolved => {
                        if !ctx.dim_ctx.is_dimension_name(&name) {
                            // Neither an element of this axis nor a dimension: a
                            // variable read selecting the element at runtime,
                            // already frozen by the wrap.
                            IndexVerdict::Keep
                        } else if let Some(row) =
                            mapped_read_axis(&name, dim, ctx).and_then(|axis| {
                                per_element_row_for_target(
                                    std::slice::from_ref(&axis),
                                    ctx.target_elem_by_dim,
                                    ctx.dim_ctx,
                                )
                            })
                        {
                            // GH #997: the index names a NON-ITERATED dimension
                            // -- typically the source's own -- that execution
                            // pairs with one of the target's iterated dims and
                            // resolves through the declared element map. The
                            // element is again the SHARED row derivation's
                            // answer, reached with an `AxisRead::MappedRead`
                            // rather than an `Iterated` so it takes the
                            // map-following rule this spelling gets. This is
                            // where C-LEARN's `x[Aggregated Regions]` deps land.
                            IndexVerdict::Pinned(qualify_axis_element(&row[0], dim))
                        } else {
                            // A dimension name no target coordinate projects onto
                            // this axis, on either rule.
                            IndexVerdict::Unspellable
                        }
                    }
                }
            };
            match verdict {
                // A pin that changes nothing keeps its own node.
                IndexVerdict::Pinned(part) if part == name => idx,
                IndexVerdict::Pinned(part) => {
                    IndexExpr0::Expr(Expr0::Var(RawIdent::new_from_str(&part), loc))
                }
                IndexVerdict::Keep => idx,
                IndexVerdict::Unspellable => {
                    discharged = false;
                    idx
                }
            }
        })
        .collect();
    (indices, discharged)
}

/// Row-pin the live source's references inside a subtree the ceteris-paribus
/// wrap declines to descend into -- a pre-existing `PREVIOUS`/`INIT` call, the
/// GH #517 whole-frozen reducer, or a `LOOKUP` table argument.
///
/// This is the pin-only descent: it performs the lowering the wrap would have
/// performed had it recursed, driven by the SAME structural path cursor and the
/// SAME occurrence IR, and it wraps nothing. Everything it reaches is inside a
/// frozen subtree (or is static table data), so no occurrence here can be the
/// live reference -- every pin is qualified.
///
/// Why this rather than tagging the tree during the wrap (the alternative
/// `86df68bf`'s rustdoc weighed): a tag needs either a descent whose only
/// purpose is to write tags, or a `Loc`-keyed side map from node position to
/// occurrence -- a second representation of the classification, keyed on a
/// coordinate the wrap rewrites, with its own drift surface. This carries no map
/// and writes no tag; it reads the one IR by path and does the actual work.
///
/// A SUBSCRIPTED reference to the source with NO recorded occurrence has no
/// per-axis classification to pin it with, and must never be emitted un-pinned:
/// its DIMENSION-name subscript (`pop[region, young]`) cannot resolve in a scalar
/// link-score fragment -- the fragment is dropped, the variable keeps a layout
/// slot with no bytecode, and the score reads a constant 0. Two mechanisms cover
/// it, in order:
///
/// - [`pin_dimension_name_indices`] discharges it STRUCTURALLY when every
///   dimension-name index spells one of the source's own declared dims. That is
///   the reachable case -- a `LOOKUP` TABLE argument, which the IR skips as
///   static data. Compilability, not attribution: the reference is still not a
///   causal edge and still earns no score, it just has to compile.
/// - anything the rule cannot discharge sets `unlowerable`, which the caller
///   turns into `WrapOutcome::missing_occurrence`, i.e. a warned skip. The known
///   shape is fixed; the unknown class stays LOUD.
///
/// The two other pin-only descents (a pre-existing `PREVIOUS`/`INIT` call, a
/// whole-frozen reducer) are walked by the IR, so they reach neither mechanism. A
/// BARE `Var` reaches neither either: [`pin_bare_source_ref`] pins it
/// structurally from the source's own declared dims, needing no occurrence.
pub(super) fn pin_only_source_refs(
    expr: Expr0,
    ctx: &PerElementRefCtx<'_>,
    occ: &OccurrenceLookup<'_>,
    path: &[u16],
    unlowerable: &mut bool,
) -> Expr0 {
    match expr {
        Expr0::Const(..) => expr,
        Expr0::Var(ref ident, loc) => {
            if &Ident::<Canonical>::new(ident.as_str()) != ctx.from {
                return expr;
            }
            match pin_bare_source_ref(ctx) {
                Some(indices) => Expr0::Subscript(ident.clone(), indices.into(), loc),
                None => expr,
            }
        }
        Expr0::Subscript(ident, indices, loc) => {
            if &Ident::<Canonical>::new(ident.as_str()) != ctx.from {
                // Another variable's subscript: descend into expression indices
                // and range endpoints, where a nested source reference can hide
                // (`other[from[D, young]]`, `other[from[a]:3]`). The IR records
                // occurrences under both, so skipping them would leave a
                // recorded source occurrence un-pinned -- its dimension-name
                // subscript would survive into the scalar equation, which either
                // fails to compile or reads the wrong element.
                let indices = indices
                    .into_iter()
                    .enumerate()
                    .map(|(i, idx)| {
                        let idx_path = super::child_path(path, i);
                        match idx {
                            IndexExpr0::Expr(e) => IndexExpr0::Expr(pin_only_source_refs(
                                e,
                                ctx,
                                occ,
                                &idx_path,
                                unlowerable,
                            )),
                            IndexExpr0::Range(l, r, rloc) => IndexExpr0::Range(
                                Box::new(pin_only_source_refs(
                                    *l,
                                    ctx,
                                    occ,
                                    &super::child_path(&idx_path, 0),
                                    unlowerable,
                                )),
                                Box::new(pin_only_source_refs(
                                    *r,
                                    ctx,
                                    occ,
                                    &super::child_path(&idx_path, 1),
                                    unlowerable,
                                )),
                                rloc,
                            ),
                            // Wildcard / star-range / `@N` carry no `Expr0`.
                            other => other,
                        }
                    })
                    .collect();
                return Expr0::Subscript(ident, indices, loc);
            }
            let node_occ = occ.get(path);
            // With no recorded occurrence there is no per-axis classification to
            // pin with, but the fragment still has to COMPILE, so the structural
            // rule substitutes the source's own dimension-name indices by name.
            // What it cannot discharge stays loud -- see the fn docs and
            // [`pin_dimension_name_indices`]. `Some` occurrences bypass it
            // entirely, so nothing the IR classifies changes spelling.
            let (indices, discharged) = match node_occ {
                Some(_) => (indices, true),
                None => pin_dimension_name_indices(indices, ctx),
            };
            if !discharged {
                *unlowerable = true;
            }
            let indices = pin_source_subscript_indices(
                indices,
                node_occ,
                ctx,
                // Inside a frozen subtree nothing can be the live reference.
                false,
                |i, idx| {
                    // An index this occurrence's axes did not resolve: descend
                    // for a nested source reference, exactly as the
                    // other-variable arm above does -- BOTH an expression index
                    // and a range bound. A source reference nested in an
                    // expression index is reachable (a dynamic table element,
                    // `LOOKUP(pop[Region, pop[Region, young]], x)`, which
                    // `codegen::extract_table_info`'s `Expr::Subscript` arm
                    // supports), and skipping it left its dimension-name
                    // subscript in the scalar fragment with nothing set loud.
                    let idx_path = super::child_path(path, i);
                    match idx {
                        IndexExpr0::Expr(e) => IndexExpr0::Expr(pin_only_source_refs(
                            e,
                            ctx,
                            occ,
                            &idx_path,
                            unlowerable,
                        )),
                        IndexExpr0::Range(l, r, rloc) => IndexExpr0::Range(
                            Box::new(pin_only_source_refs(
                                *l,
                                ctx,
                                occ,
                                &super::child_path(&idx_path, 0),
                                unlowerable,
                            )),
                            Box::new(pin_only_source_refs(
                                *r,
                                ctx,
                                occ,
                                &super::child_path(&idx_path, 1),
                                unlowerable,
                            )),
                            rloc,
                        ),
                        // Wildcard / star-range / `@N` carry no `Expr0`.
                        other => other,
                    }
                },
            );
            Expr0::Subscript(ident, indices, loc)
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            let args = args
                .into_iter()
                .enumerate()
                .map(|(i, a)| {
                    pin_only_source_refs(a, ctx, occ, &super::child_path(path, i), unlowerable)
                })
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, inner, loc) => Expr0::Op1(
            op,
            Box::new(pin_only_source_refs(
                *inner,
                ctx,
                occ,
                &super::child_path(path, 0),
                unlowerable,
            )),
            loc,
        ),
        Expr0::Op2(op, l, r, loc) => Expr0::Op2(
            op,
            Box::new(pin_only_source_refs(
                *l,
                ctx,
                occ,
                &super::child_path(path, 0),
                unlowerable,
            )),
            Box::new(pin_only_source_refs(
                *r,
                ctx,
                occ,
                &super::child_path(path, 1),
                unlowerable,
            )),
            loc,
        ),
        Expr0::If(c, t, f, loc) => Expr0::If(
            Box::new(pin_only_source_refs(
                *c,
                ctx,
                occ,
                &super::child_path(path, 0),
                unlowerable,
            )),
            Box::new(pin_only_source_refs(
                *t,
                ctx,
                occ,
                &super::child_path(path, 1),
                unlowerable,
            )),
            Box::new(pin_only_source_refs(
                *f,
                ctx,
                occ,
                &super::child_path(path, 2),
                unlowerable,
            )),
            loc,
        ),
    }
}

/// Substitute each recognized reducer subexpression in `equation_text` with a
/// (quoted) reference to its aggregate node.
///
/// `reducers` maps the canonical reducer-subexpression text (exactly as
/// `crate::patch::expr2_to_string` / `print_eqn` renders it -- lowercased,
/// whitespace-normalized) to the agg node's name. `equation_text` is parsed
/// to `Expr0`, and any subexpression of it whose `print_eqn` equals one of
/// those keys is replaced by a `Var(agg_name)` node, then the whole tree is
/// re-printed. The match is on the parsed AST subtree, not a substring of the
/// text, so a reducer text that is a textual prefix of a *different* reducer
/// subexpression (`sum(p[*])` vs `sum(p[*] + 1)`) is never falsely matched.
///
/// Returns `Err([`PartialEquationError`])` when `equation_text` does not parse
/// (a genuine parse error, or an empty/whitespace equation that yields no AST)
/// *and there are reducers to substitute*: with no AST there is no reducer
/// subexpression to replace, so returning the input unchanged would let the
/// inline reducer survive into the `agg → target` partial -- a partial that
/// references the live reducer instead of the hoisted aggregate node, a
/// wrong-but-clean-compiling link score (the agg-substitution-omission sibling
/// of the GH #311 PREVIOUS-omission hazard; GH #661). The db-bearing caller
/// converts the error into a `Warning` (via `emit_ltm_partial_equation_warning`)
/// and skips the variable. The failure is effectively unreachable in production
/// (the input is a `print_eqn` re-print of an already-parsed AST), so this is
/// defense-in-depth.
///
/// The empty-`reducers` case is a pure pass-through that never parses (there
/// is nothing to substitute), so it returns `Ok` with the text unchanged even
/// for otherwise-unparseable input.
///
/// Test-only since Track A stage 1: the agg-substitution is now a POST-transform
/// lowering that runs on the wrapped AST via [`substitute_reducers_in_expr0`]
/// (the generators call that directly), so the text-level wrapper survives only
/// as the unit-tested entry point for the substitution behavior.
#[cfg(test)]
pub(crate) fn substitute_reducers_in_equation(
    equation_text: &str,
    reducers: &HashMap<String, String>,
) -> Result<String, PartialEquationError> {
    if reducers.is_empty() {
        return Ok(equation_text.to_string());
    }
    let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) else {
        return Err(PartialEquationError::new(equation_text));
    };
    Ok(print_eqn(&substitute_reducers_in_expr0(ast, reducers)))
}

/// The agg-substitution POST-transform lowering (Track A stage 1): rewrite each
/// hoisted reducer subexpression in an already-wrapped `Expr0` to its synthetic
/// agg name in place, keyed by canonical `print_eqn` text (`reducers` maps
/// reducer text -> agg name). It runs on the WRAPPED AST -- AFTER the
/// ceteris-paribus wrap held one hoisted reducer live and froze the rest whole
/// -- so the held-live reducer becomes a bare agg name and each frozen
/// `PREVIOUS(SUM(..))` becomes `PREVIOUS(agg)`, byte-for-byte matching the old
/// wrap-of-agg-substituted-text composition. It deliberately does NOT
/// re-classify (a pure text match), so stage 2 can retarget the wrap onto the
/// occurrence IR without touching this lowering. The `agg → target` generators
/// call it directly; the text-level `substitute_reducers_in_equation` wrapper
/// is now test-only.
pub(super) fn substitute_reducers_in_expr0(
    expr: Expr0,
    reducers: &HashMap<String, String>,
) -> Expr0 {
    // A whole-subtree match wins before descending: a reducer App is opaque
    // -- once it matches an agg, we don't recurse into its (now-irrelevant)
    // argument.
    if let Some(agg_name) = reducers.get(&print_eqn(&expr)) {
        return Expr0::Var(
            crate::common::RawIdent::new_from_str(agg_name),
            crate::ast::Loc::default(),
        );
    }
    match expr {
        Expr0::Const(..) | Expr0::Var(..) => expr,
        Expr0::Subscript(ident, indices, loc) => {
            // A reducer can appear as (or inside) a subscript index expression
            // -- `stock[SUM(idx[*])]` -- and `walk_subexpr_for_aggs` hoists it
            // into a synthetic agg by descending into `IndexExpr2::Expr` /
            // `IndexExpr2::Range`, so the substituter must mirror that descent.
            // Wildcard / star-range / `@N` indices carry no `Expr0`, so they
            // pass through unchanged.
            let indices = indices
                .into_iter()
                .map(|idx| match idx {
                    IndexExpr0::Expr(e) => {
                        IndexExpr0::Expr(substitute_reducers_in_expr0(e, reducers))
                    }
                    IndexExpr0::Range(l, r, loc) => IndexExpr0::Range(
                        Box::new(substitute_reducers_in_expr0(*l, reducers)),
                        Box::new(substitute_reducers_in_expr0(*r, reducers)),
                        loc,
                    ),
                    IndexExpr0::Wildcard(_)
                    | IndexExpr0::StarRange(_, _)
                    | IndexExpr0::DimPosition(_, _) => idx,
                })
                .collect();
            Expr0::Subscript(ident, indices, loc)
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            let args = args
                .into_iter()
                .map(|a| substitute_reducers_in_expr0(a, reducers))
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, inner, loc) => Expr0::Op1(
            op,
            Box::new(substitute_reducers_in_expr0(*inner, reducers)),
            loc,
        ),
        Expr0::Op2(op, lhs, rhs, loc) => Expr0::Op2(
            op,
            Box::new(substitute_reducers_in_expr0(*lhs, reducers)),
            Box::new(substitute_reducers_in_expr0(*rhs, reducers)),
            loc,
        ),
        Expr0::If(cond, then_e, else_e, loc) => Expr0::If(
            Box::new(substitute_reducers_in_expr0(*cond, reducers)),
            Box::new(substitute_reducers_in_expr0(*then_e, reducers)),
            Box::new(substitute_reducers_in_expr0(*else_e, reducers)),
            loc,
        ),
    }
}
