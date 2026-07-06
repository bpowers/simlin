// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Record-derived result extraction.
//!
//! Section 1 of a VDF stores one record per name-bound symbol, and a record's
//! `f[11]` doubles as either an OT-block start (owner) or a section-6
//! lookup-record index (graphical-function descriptor) -- an *untagged* union
//! whose discriminator is not stored on disk (see `docs/design/vdf.md`
//! "Appendix: the owner/descriptor discriminator"). The reader is expected to
//! already know the model. For the
//! model-free reader this module reconstructs which records are owners using
//! the decoded forward link: a descriptor's `f[11]` indexes the section-6
//! lookup-record array, and that array is in case-insensitive alphabetical
//! order of the lookup-definition names.
//!
//! `decoded_record_spans` produces one `DecodedRecordSpan` per section-1
//! record whose `(name key, OT-start, shape)` triple is structurally valid
//! and whose covered OT slots all carry an owner class code.
//! `identify_descriptor_records` then peels off the descriptor records that
//! collide with real owner spans, leaving a clean non-overlapping owner
//! partition (`Time` at OT[0] aside).

use std::collections::{HashMap, HashSet};

use super::{
    SYSTEM_NAMES, VDF_SECTION6_OT_CODE_STOCK, VdfFile, VdfRecord, VdfSection3Directory,
    VdfSection3DirectoryEntry, is_lookupish_name, is_owner_ot_class_code,
};
use crate::common::{Canonical, Ident};

/// One direct record -> name -> OT-span fact.
///
/// Built by `decoded_record_spans`. A span here means the record carries:
/// - an `f[2]` that resolves through the section-2 name-key formula;
/// - an `f[11]` interpretable as an OT block start in `[1, ot_count)`;
/// - a non-zero `f[6]` shape code whose flat span is structurally decoded;
/// - and (class-code guard) an OT slot at `f[11]` whose section-6 class
///   code marks real saved data (`is_owner_ot_class_code`).
///
/// Whether the record is the *emitted* series owner is a separate question
/// answered by `identify_descriptor_records`.
#[derive(Clone, Debug)]
pub(super) struct DecodedRecordSpan {
    pub(super) rec_idx: usize,
    pub(super) name: String,
    pub(super) start: usize,
    pub(super) end: usize,
    /// `f[10]`, used as the descriptor tie-break when the lexical
    /// lookup-def name test is ambiguous.
    pub(super) sort_key: u32,
}

impl DecodedRecordSpan {
    pub(super) fn length(&self) -> usize {
        self.end - self.start
    }
}

/// Compute the structural OT-flat span of a record, returning `None` when
/// the shape cannot be resolved.
///
/// `f[6] == 5` is the scalar marker (one slot). `f[6] == 32` is Vensim's
/// generic single-shape arrayed marker; it binds when exactly one
/// section-3 entry has a non-zero flat size. Otherwise the section-3
/// directory's per-shape-code entry is used.
fn decoded_record_shape_length(
    rec: &VdfRecord,
    section3_directory: Option<&VdfSection3Directory>,
    sec3_sole_flat_size: Option<usize>,
) -> Option<usize> {
    let shape_code = rec.fields[6];
    if shape_code == 0 {
        return None;
    }
    if shape_code == 5 {
        return Some(1);
    }
    if let Some(dir) = section3_directory
        && let Some(entry) = dir.entry_for_record_shape_code(shape_code)
    {
        let s = entry.flat_size();
        if s >= 1 {
            return Some(s);
        }
    }
    if shape_code == 32
        && let Some(s) = sec3_sole_flat_size
        && s >= 1
    {
        return Some(s);
    }
    None
}

/// Build the direct record -> name -> OT-span facts from a VDF.
///
/// This deliberately performs no descriptor pruning, no owner selection, no
/// name-category filtering, and no array-label guessing. Whether a span is
/// the user-facing series owner is decided downstream in
/// `identify_descriptor_records`, which is the only place that resolves the
/// `f[11]` owner/descriptor union.
pub(super) fn decoded_record_spans(
    vdf: &VdfFile,
    name_key_to_name_index: &HashMap<u32, usize>,
    section3_directory: Option<&VdfSection3Directory>,
) -> Vec<DecodedRecordSpan> {
    let codes = vdf.section6_ot_class_codes();
    let sec3_sole_flat_size = section3_directory.and_then(|d| {
        let sizes: HashSet<usize> = d
            .entries
            .iter()
            .map(|e| e.flat_size())
            .filter(|&s| s > 0)
            .collect();
        if sizes.len() == 1 {
            sizes.into_iter().next()
        } else {
            None
        }
    });

    let mut spans = Vec::new();
    for (rec_idx, rec) in vdf.records.iter().enumerate() {
        let Some(&name_idx) = name_key_to_name_index.get(&rec.fields[2]) else {
            continue;
        };
        let Some(name) = vdf.names.get(name_idx).cloned() else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let start = rec.fields[11] as usize;
        if start == 0 || start >= vdf.offset_table_count {
            continue;
        }
        let length = match decoded_record_shape_length(rec, section3_directory, sec3_sole_flat_size)
        {
            Some(l) => l,
            None => continue,
        };
        let end = start + length;
        if end > vdf.offset_table_count {
            continue;
        }
        // Class-code guard: every in-bounds OT slot in the span must carry
        // a real-data owner code. Time (0x0f) is excluded by `start >= 1`;
        // any non-owner code in-range indicates a descriptor
        // reinterpretation of `f[11]` or a stale ghost record, not a real
        // owner span. Slots past the end of `codes` are silently accepted
        // to match the Python xray implementation -- the upstream
        // `end > offset_table_count` gate already covers the realistic OOB
        // case, and a short class-code array would be a parser defect
        // rather than a span-level signal.
        if let Some(ref code_vec) = codes {
            let mut any_non_owner_in_bounds = false;
            for ot_idx in start..end {
                if let Some(&code) = code_vec.get(ot_idx)
                    && !is_owner_ot_class_code(code)
                {
                    any_non_owner_in_bounds = true;
                    break;
                }
            }
            if any_non_owner_in_bounds {
                continue;
            }
        }

        spans.push(DecodedRecordSpan {
            rec_idx,
            name,
            start,
            end,
            sort_key: rec.fields[10],
        });
    }
    spans
}

/// Result of identifying graphical-function descriptor records.
///
/// `descriptor_indices` are the records (by `rec_idx`) that are dropped -- NOT
/// emitted at their `f[11]`-as-OT-start slot, because they are graphical-function
/// definitions (tables), not saved time series. Two sub-cases:
/// - **Overlapping descriptors**: their consuming owner record exists separately
///   in the same OT component and carries the series.
/// - **Standalone descriptors** (a lookup-only variable Vensim saves *only* as a
///   descriptor, no separate consumer-owner record): a bare lookup is a table,
///   so it has no series of its own; the consumer variables that call it are
///   separately-emitted owners. Both sub-cases are simply dropped.
///
/// `used_f10_fallback` records when the descriptor peeling step had to
/// resort to the highest-`f[10]` tie-break because the lexical
/// lookup-def-name test was ambiguous (`Ref.vdf` is the canonical case).
/// `standalone_drop_veto_fired` / `standalone_drop_vetoed_candidates` record
/// when the standalone lookup-only drop's per-file coherence veto withheld
/// candidates (`SimService/Base.vdf` is the canonical case) -- a silent veto
/// would let a future file quietly resurrect ghost columns. The flags are
/// exposed for tests and future diagnostics; they have no effect on the
/// descriptor membership decision itself.
///
/// `residual_overlap_components` records the still-conflicted spans dropped by
/// the residual-overlap floor (see [`residual_overlap_components`]): every span
/// in each component is added to `descriptor_indices` and therefore NOT
/// emitted, so no OT slot is ever resolved by alphabetical emission order.
/// The components are retained as data (not just the drop set) so a later
/// re-resolution stage can narrow the drop before it falls back to this floor.
#[derive(Clone, Debug, Default)]
pub(super) struct DescriptorIdentification {
    pub(super) descriptor_indices: HashSet<usize>,
    #[allow(dead_code)]
    pub(super) used_f10_fallback: bool,
    #[allow(dead_code)]
    pub(super) standalone_drop_veto_fired: bool,
    #[allow(dead_code)]
    pub(super) standalone_drop_vetoed_candidates: usize,
    pub(super) residual_overlap_components: Vec<ResidualOverlapComponent>,
}

/// A residual OT-overlap component: two or more DIFFERENTLY-named decoded
/// spans that still claim a shared OT slot after descriptor peeling and the
/// standalone lookup-only drop. Stage 1 drops every span in the component
/// from emission (honest missing data over a silent alphabetical first-claim
/// win); the component is retained here so a later stage can re-resolve it
/// before falling back to the drop.
#[derive(Clone, Debug, Default)]
pub(super) struct ResidualOverlapComponent {
    /// Indices into the `spans` slice for every span in the component (all
    /// dropped in Stage 1).
    pub(super) span_indices: Vec<usize>,
    /// OT slots claimed by two or more differently-named spans of the
    /// component (the genuine conflicts). Sorted ascending.
    pub(super) contested_ots: Vec<usize>,
}

/// Outcome of the standalone lookup-only descriptor detection: the records
/// to drop, plus the per-file coherence veto diagnostics (`veto_fired` with
/// the number of physically-gated candidates the veto withheld).
#[derive(Clone, Debug, Default)]
struct StandaloneDropOutcome {
    dropped: HashSet<usize>,
    veto_fired: bool,
    vetoed_candidates: usize,
}

/// Iterative union-find without rank.
struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, mut x: usize) -> usize {
        while self.parent[x] != x {
            self.parent[x] = self.parent[self.parent[x]];
            x = self.parent[x];
        }
        x
    }

    fn union(&mut self, x: usize, y: usize) {
        let px = self.find(x);
        let py = self.find(y);
        if px != py {
            self.parent[px] = py;
        }
    }
}

/// Identify graphical-function descriptor records among the decoded spans.
///
/// Background. Vensim stores graphical-function definitions ("descriptor"
/// records) and their consuming variables ("owner" records) side-by-side in
/// section 1 with `f[11]` as an *untagged* union: for owners it is the
/// OT-block start, for descriptors it is the zero-based index into the
/// section-6 lookup-record array (case-insensitive alphabetical order of
/// the lookup-def names). The on-disk format does not store the
/// discriminator -- a field-by-field analysis (vdf.md "Appendix: the
/// owner/descriptor discriminator") confirms no byte, bit, or
/// `(f0, f1)` combination distinguishes the two.
///
/// Algorithm. Spans that overlap in OT space form a connected component
/// (descriptors sometimes have arrayed shapes that cross owner ranges, so
/// they need not literally share `f[11]` with their colliding owners).
/// Within each component, peel off descriptor records iteratively:
/// 1. **Lookup-def name test.** If exactly one candidate's name is
///    lexically lookupish (`is_lookupish_name`) it is the descriptor.
/// 2. **Highest-`f[10]` fallback.** When the lookup-def name test is
///    ambiguous (e.g. `Ref.vdf` where descriptors are domain
///    abbreviations), the candidate with the highest `f[10]` is treated as
///    the descriptor and `used_f10_fallback` is flagged.
///
/// Once a record is identified as a descriptor, its true binding is the
/// decoded forward link: `lookup_record[f[11]].word[10]` is the
/// evaluated-output OT, `word[5..6]` are the section-7 x/y array offsets,
/// `word[12]` is the optional dependency-chain root.
pub(super) fn identify_descriptor_records(
    vdf: &VdfFile,
    spans: &[DecodedRecordSpan],
) -> DescriptorIdentification {
    let n_lookups = vdf.section6_lookup_records().map(|v| v.len()).unwrap_or(0);
    if n_lookups == 0 || spans.is_empty() {
        return DescriptorIdentification::default();
    }

    // Build OT-slot -> spans-claiming-it. Spans that share any OT slot with
    // another span are descriptor-pair candidates.
    let mut by_slot: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, span) in spans.iter().enumerate() {
        for ot in span.start..span.end {
            by_slot.entry(ot).or_default().push(i);
        }
    }

    // Connected components of overlapping spans (union-find on span indices).
    let mut uf = UnionFind::new(spans.len());
    for slot_spans in by_slot.values() {
        if slot_spans.len() >= 2 {
            let base = slot_spans[0];
            for &other in &slot_spans[1..] {
                uf.union(base, other);
            }
        }
    }

    // A span participates in overlap iff some OT in its range has 2+
    // claimants.
    let mut overlapping: HashSet<usize> = HashSet::new();
    for slot_spans in by_slot.values() {
        if slot_spans.len() >= 2 {
            overlapping.extend(slot_spans.iter().copied());
        }
    }

    let mut components: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, _) in spans.iter().enumerate() {
        if overlapping.contains(&i) {
            let root = uf.find(i);
            components.entry(root).or_default().push(i);
        }
    }

    let mut descriptor_indices: HashSet<usize> = HashSet::new();
    let mut used_f10_fallback = false;

    for component in components.values() {
        // Iteratively peel off descriptor records until the component is
        // internally non-overlapping. Candidates are restricted to records
        // whose `f[11]` is in `[0, lookup_count)` -- the structural
        // pre-condition for the lookup-record forward link.
        let mut active: Vec<usize> = component.clone();
        loop {
            let mut comp_by_slot: HashMap<usize, Vec<usize>> = HashMap::new();
            for &i in &active {
                let span = &spans[i];
                for ot in span.start..span.end {
                    comp_by_slot.entry(ot).or_default().push(i);
                }
            }
            let mut still_overlapping: HashSet<usize> = HashSet::new();
            for slot_spans in comp_by_slot.values() {
                if slot_spans.len() >= 2 {
                    still_overlapping.extend(slot_spans.iter().copied());
                }
            }
            if still_overlapping.is_empty() {
                break;
            }
            let candidates: Vec<usize> = active
                .iter()
                .copied()
                .filter(|&i| {
                    if !still_overlapping.contains(&i) {
                        return false;
                    }
                    let f11 = vdf.records[spans[i].rec_idx].fields[11] as usize;
                    f11 < n_lookups
                })
                .collect();
            if candidates.is_empty() {
                // Owner-only overlap with no descriptor candidate: leave the
                // component alone. The caller (or precision report) is
                // expected to surface a residual `record-span-overlap`.
                break;
            }
            let lookupish: Vec<usize> = candidates
                .iter()
                .copied()
                .filter(|&i| is_lookupish_name(&spans[i].name))
                .collect();
            let descriptor_span_idx = if lookupish.len() == 1 {
                lookupish[0]
            } else {
                used_f10_fallback = true;
                *candidates
                    .iter()
                    .max_by_key(|&&i| (spans[i].sort_key, spans[i].rec_idx))
                    .expect("candidates non-empty")
            };
            descriptor_indices.insert(spans[descriptor_span_idx].rec_idx);
            active.retain(|&i| i != descriptor_span_idx);
        }
    }

    // Standalone (non-overlapping) descriptors: a lookup-only variable Vensim
    // saves only as a descriptor record (a graphical-function definition). The
    // overlap path above never sees it (it collides with nothing), so it would
    // otherwise decode at its spurious `f[11]`-as-OT-start ghost slot. A bare
    // lookup is a table, not a time series, so recognise it and DROP it -- its
    // values, where they matter, are carried by the consumer variables that
    // call it (separately-emitted owners).
    let lookup_records = vdf.section6_lookup_records();
    let lookup_word10: Vec<usize> = lookup_records
        .as_ref()
        .map(|recs| recs.iter().map(|r| r.ot_index()).collect())
        .unwrap_or_default();
    let lookup_word11: Vec<usize> = lookup_records
        .as_ref()
        .map(|recs| recs.iter().map(|r| r.output_width()).collect())
        .unwrap_or_default();
    let class_codes = vdf.section6_ot_class_codes().unwrap_or_default();
    let f11_by_span: Vec<u32> = spans
        .iter()
        .map(|s| vdf.records[s.rec_idx].fields[11])
        .collect();
    let standalone = standalone_lookup_only_descriptors(
        spans,
        &f11_by_span,
        &overlapping,
        &descriptor_indices,
        n_lookups,
        &lookup_word10,
        &lookup_word11,
        &class_codes,
        vdf.offset_table_count,
    );
    descriptor_indices.extend(standalone.dropped);

    // Residual-overlap re-resolution. After peeling overlap-path descriptors
    // and dropping standalone lookup-only tables, some differently-named spans
    // can STILL claim a shared OT slot -- an owner-vs-owner conflict no
    // structural signal resolved. Compute those components as data, then
    // re-resolve each from scratch (lexical peel of table names + the
    // alphabetical-ordering oracle), recovering the real owners and dropping
    // only the ghosts. Whatever the oracle cannot adjudicate is honest-dropped
    // and threaded out on `DescriptorIdentification` for the diagnostics.
    let phase1_descriptors = descriptor_indices.clone();
    let residual_components = residual_overlap_components(spans, &descriptor_indices);
    let resolution = resolve_residual_components(
        spans,
        &residual_components,
        &phase1_descriptors,
        RESIDUAL_ORDERING_GATE,
    );
    for rec_idx in &resolution.readmitted {
        descriptor_indices.remove(rec_idx);
    }
    descriptor_indices.extend(resolution.dropped.iter().copied());

    DescriptorIdentification {
        descriptor_indices,
        used_f10_fallback,
        standalone_drop_veto_fired: standalone.veto_fired,
        standalone_drop_vetoed_candidates: standalone.vetoed_candidates,
        residual_overlap_components: resolution.unresolved_components,
    }
}

/// Detect residual OT-overlap among the decoded spans that survive descriptor
/// peeling and the standalone lookup-only drop (functional core: takes the
/// spans plus the already-dropped `rec_idx` set, so it is unit-testable on
/// synthetic inputs with no fixture).
///
/// `identify_descriptor_records` resolves the owner/descriptor `f[11]` union
/// for the overlaps a structural signal can adjudicate (the lexical
/// lookup-def name test, the section-6 forward link). On the 2007-era
/// SimService writer that is not enough: it emits section-1 records for model
/// variables that were NOT saved in the run (data variables, lookup/table
/// definitions, supplementary vars), and those records carry a stale `f[11]`
/// (plausibly the variable's slot in the full runtime array, while the file's
/// OT contains only saved variables). The stale `f[11]`-as-OT-start spans land
/// on arbitrary saved slots; most fail the `f11 < n_lookups` candidacy gate, so
/// the overlap peel can neither remove them nor detect that it failed, and the
/// component exits still conflicted (see docs/design/vdf.md "Residual
/// OT-overlap" and the stale-`f[11]` hypothesis).
///
/// Emitting both spans would let alphabetical emission order silently pick the
/// OT owner and scatter one variable's data under another variable's name. This
/// returns the residual components; the caller drops every span in them.
///
/// Conflict is DIFFERENT-name only: two spans of the same name that overlap are
/// the ordinary same-variable duplicate the emitter's per-name dedup already
/// resolves (lowest start wins), not a cross-variable ownership conflict. Every
/// span sharing a genuinely-contested slot (one carrying two distinct names) is
/// pulled into the component and dropped, since the whole slot is ambiguous.
fn residual_overlap_components(
    spans: &[DecodedRecordSpan],
    dropped: &HashSet<usize>,
) -> Vec<ResidualOverlapComponent> {
    // slot -> surviving span indices claiming it.
    let mut by_slot: HashMap<usize, Vec<usize>> = HashMap::new();
    for (i, span) in spans.iter().enumerate() {
        if dropped.contains(&span.rec_idx) {
            continue;
        }
        for ot in span.start..span.end {
            by_slot.entry(ot).or_default().push(i);
        }
    }

    // A slot is genuinely contested iff two of its claimants carry distinct
    // names. Union every span on such a slot (the whole slot is ambiguous) and
    // record the slot as contested.
    let mut uf = UnionFind::new(spans.len());
    let mut conflicted: HashSet<usize> = HashSet::new();
    let mut contested_slot_reps: Vec<(usize, usize)> = Vec::new(); // (ot, rep span idx)
    for (&ot, slot_spans) in &by_slot {
        let mut distinct_names = false;
        'pairs: for a in 0..slot_spans.len() {
            for b in (a + 1)..slot_spans.len() {
                if spans[slot_spans[a]].name != spans[slot_spans[b]].name {
                    distinct_names = true;
                    break 'pairs;
                }
            }
        }
        if !distinct_names {
            continue;
        }
        let base = slot_spans[0];
        for &other in slot_spans {
            uf.union(base, other);
            conflicted.insert(other);
        }
        contested_slot_reps.push((ot, base));
    }

    if conflicted.is_empty() {
        return Vec::new();
    }

    // Group conflicted spans by union-find root; collect each component's
    // contested OTs by the same root.
    let mut spans_by_root: HashMap<usize, Vec<usize>> = HashMap::new();
    for &i in &conflicted {
        spans_by_root.entry(uf.find(i)).or_default().push(i);
    }
    let mut ots_by_root: HashMap<usize, Vec<usize>> = HashMap::new();
    for &(ot, rep) in &contested_slot_reps {
        ots_by_root.entry(uf.find(rep)).or_default().push(ot);
    }

    let mut components: Vec<ResidualOverlapComponent> = spans_by_root
        .into_iter()
        .map(|(root, mut span_indices)| {
            span_indices.sort_unstable();
            let mut contested_ots = ots_by_root.remove(&root).unwrap_or_default();
            contested_ots.sort_unstable();
            contested_ots.dedup();
            ResidualOverlapComponent {
                span_indices,
                contested_ots,
            }
        })
        .collect();
    // Deterministic order (HashMap iteration is not): by first contested OT,
    // then by first span index.
    components.sort_by_key(|c| {
        (
            c.contested_ots.first().copied().unwrap_or(usize::MAX),
            c.span_indices.first().copied().unwrap_or(usize::MAX),
        )
    });
    components
}

/// Minimum fraction of adjacent uncontested-owner pairs (sorted by OT) that
/// must be alphabetically ordered for the ordering oracle to run on a file.
///
/// Vensim allocates OT slots in case-insensitive alphabetical order within a
/// run, so a genuine run file's uncontested owners are overwhelmingly ordered
/// (the four probed corpus files measure 98.6-99.6%; the residual few percent
/// of breaks are run boundaries). A file below this bar does not exhibit the
/// invariant, so the oracle abstains and every residual span is honest-dropped
/// (Stage 1 semantics). The bar is a principled "overwhelming majority", not
/// tuned to make any one file pass.
const RESIDUAL_ORDERING_GATE: f64 = 0.95;

/// Outcome of re-resolving the residual-overlap components (Stage 2).
///
/// `dropped` are `rec_idx`es the re-resolution drops (ghosts + the honest-drop
/// fallback). `readmitted` are `rec_idx`es phase 1 peeled that the
/// re-resolution recovered as real owners (e.g. `c Identified Oil Reserve`).
/// `unresolved_components` are the honest-dropped remainder the oracle could
/// not adjudicate, surfaced on the diagnostics (empty when every component
/// fully resolves, as on `SimService/Base.vdf`).
#[derive(Clone, Debug, Default)]
pub(super) struct ResidualResolution {
    pub(super) dropped: HashSet<usize>,
    pub(super) readmitted: HashSet<usize>,
    pub(super) unresolved_components: Vec<ResidualOverlapComponent>,
}

/// Case-insensitive Vensim OT-allocation sort key (mirrors the Python reader's
/// `_vensim_sort_key`). Names in the corpus are ASCII, so this matches the
/// Python `str.lower()` byte-for-byte.
fn vensim_sort_key(name: &str) -> String {
    name.to_lowercase()
}

fn spans_overlap(a: &DecodedRecordSpan, b: &DecodedRecordSpan) -> bool {
    !(a.end <= b.start || b.end <= a.start)
}

/// Whether span `idx` still overlaps another span in `active`.
fn overlaps_any(idx: usize, active: &[usize], spans: &[DecodedRecordSpan]) -> bool {
    active
        .iter()
        .any(|&other| other != idx && spans_overlap(&spans[idx], &spans[other]))
}

/// Fraction of adjacent OT-sorted uncontested-owner pairs that are name-ordered
/// -- the measured strength of Vensim's alphabetical OT allocation on this
/// file. 1.0 when there are fewer than two owners.
fn alphabetical_consistency(uncontested_by_ot: &[usize], spans: &[DecodedRecordSpan]) -> f64 {
    if uncontested_by_ot.len() < 2 {
        return 1.0;
    }
    let mut ok = 0usize;
    for w in uncontested_by_ot.windows(2) {
        if vensim_sort_key(&spans[w[0]].name) <= vensim_sort_key(&spans[w[1]].name) {
            ok += 1;
        }
    }
    ok as f64 / (uncontested_by_ot.len() - 1) as f64
}

/// Ordering verdict for a span given its prev/next anchor bracket (all args are
/// case-insensitive sort keys). An INVERTED bracket (`prev > next`) means a run
/// boundary sits between the anchors, so the unreliable next side is dropped
/// and only the prev side is tested (the cluster-A case, where the next anchor
/// `agricultural land in use` sorts before the real `c *` owners). Otherwise
/// the name must fall within `[prev, next]`.
fn ordering_ok(prev: Option<&str>, next: Option<&str>, name: &str) -> bool {
    if let (Some(p), Some(n)) = (prev, next)
        && p > n
    {
        return p <= name;
    }
    prev.is_none_or(|p| p <= name) && next.is_none_or(|n| name <= n)
}

/// Nearest span in the OT-`start`-sorted `sorted` list with start strictly
/// before `start`.
fn anchor_prev(sorted: &[usize], spans: &[DecodedRecordSpan], start: usize) -> Option<usize> {
    let pos = sorted.partition_point(|&i| spans[i].start < start);
    (pos > 0).then(|| sorted[pos - 1])
}

/// Nearest span in the OT-`start`-sorted `sorted` list with start at or after
/// `end`.
fn anchor_next(sorted: &[usize], spans: &[DecodedRecordSpan], end: usize) -> Option<usize> {
    let pos = sorted.partition_point(|&i| spans[i].start < end);
    sorted.get(pos).copied()
}

/// Ordering-oracle verdict: does `span_idx` sit where Vensim's alphabetical OT
/// allocation would put a real owner?
///
/// Anchors come in two tiers. When a RECOVERED real owner (a residual-component
/// span already confirmed this pass) brackets the span on BOTH sides, that tier
/// wins: recovered reals share the span's interleaved run, so they are the
/// reliable same-run evidence (this is what adjudicates `indicated per capita
/// fish demand` vs `China future GDP growth rate` at OT 127 -- the recovered
/// `Indicated China GDP`@123 / `indicated row Coal demand`@128 bracket, not the
/// nearest uncontested owner `cafe history`@124, which is a different run).
/// Otherwise the file's uncontested owners are the anchors.
fn residual_span_is_owner(
    span_idx: usize,
    recovered_sorted: &[usize],
    uncontested_sorted: &[usize],
    spans: &[DecodedRecordSpan],
) -> bool {
    let name_key = vensim_sort_key(&spans[span_idx].name);
    let start = spans[span_idx].start;
    let end = spans[span_idx].end;
    let rp = anchor_prev(recovered_sorted, spans, start);
    let rn = anchor_next(recovered_sorted, spans, end);
    if let (Some(p), Some(n)) = (rp, rn) {
        return ordering_ok(
            Some(&vensim_sort_key(&spans[p].name)),
            Some(&vensim_sort_key(&spans[n].name)),
            &name_key,
        );
    }
    let up = anchor_prev(uncontested_sorted, spans, start).map(|i| vensim_sort_key(&spans[i].name));
    let un = anchor_next(uncontested_sorted, spans, end).map(|i| vensim_sort_key(&spans[i].name));
    ordering_ok(up.as_deref(), un.as_deref(), &name_key)
}

/// Re-resolve each residual-overlap component from scratch, recovering the real
/// owners and dropping only the ghosts (stale-`f[11]` unsaved-variable
/// records). Functional core: operates on the decoded spans plus the phase-1
/// descriptor set, so it is unit-testable on synthetic inputs.
///
/// Per component (see docs/design/vdf.md "Residual OT-overlap"):
/// (a) discard the component's phase-1 overlap peels that spatially belong to
///     it (un-peel: any phase-1 descriptor overlapping a component span, e.g.
///     the wrongly f10-peeled `c Identified Oil Reserve`), then
/// (b) LEXICALLY peel spans whose names are lookupish, WITHOUT the
///     `f11 < n_lookups` gate -- a lookup definition is a table, not a series,
///     so its stale `f[11]` cannot forward-link, then
/// (c) adjudicate the remaining conflicts with the alphabetical-ordering oracle
///     ([`residual_span_is_owner`]), iterating a fixpoint so a span confirmed as
///     an owner becomes an anchor for its neighbours, then
/// (d) honest-drop anything still in conflict (surfaced on the diagnostics).
///
/// Gated per file: if the uncontested owners do not exhibit the
/// alphabetical-allocation invariant (`gate_threshold`), the oracle abstains and
/// every residual span is honest-dropped (Stage 1 semantics), so a file that
/// does not follow the invariant is never mis-adjudicated.
fn resolve_residual_components(
    spans: &[DecodedRecordSpan],
    components: &[ResidualOverlapComponent],
    phase1_descriptors: &HashSet<usize>,
    gate_threshold: f64,
) -> ResidualResolution {
    let mut dropped: HashSet<usize> = HashSet::new();
    if components.is_empty() {
        return ResidualResolution::default();
    }

    let comp_span_idx: HashSet<usize> = components
        .iter()
        .flat_map(|c| c.span_indices.iter().copied())
        .collect();

    // Uncontested owners: the clean, non-conflicted owner partition -- the
    // alphabetical reference. Sorted by OT start.
    let mut uncontested: Vec<usize> = (0..spans.len())
        .filter(|&i| !phase1_descriptors.contains(&spans[i].rec_idx) && !comp_span_idx.contains(&i))
        .collect();
    uncontested.sort_by_key(|&i| (spans[i].start, spans[i].rec_idx));

    // Gate: abstain (honest-drop all) when the file does not exhibit the
    // alphabetical invariant.
    if alphabetical_consistency(&uncontested, spans) < gate_threshold {
        for c in components {
            for &i in &c.span_indices {
                dropped.insert(spans[i].rec_idx);
            }
        }
        return ResidualResolution {
            dropped,
            readmitted: HashSet::new(),
            unresolved_components: components.to_vec(),
        };
    }

    // Per-component active sets: component spans + un-peeled phase-1 descriptors
    // overlapping any component span (a phase-1 descriptor is given a second
    // chance exactly when it collides with a component's spans).
    let mut comp_active: Vec<Vec<usize>> = Vec::with_capacity(components.len());
    let mut unpeeled_recidx: HashSet<usize> = HashSet::new();
    for c in components {
        let mut active: Vec<usize> = c.span_indices.clone();
        let component_spans = active.clone();
        for (i, span) in spans.iter().enumerate() {
            if phase1_descriptors.contains(&span.rec_idx)
                && component_spans
                    .iter()
                    .any(|&cs| spans_overlap(span, &spans[cs]))
            {
                active.push(i);
                unpeeled_recidx.insert(span.rec_idx);
            }
        }
        active.sort_by_key(|&i| (spans[i].start, spans[i].rec_idx));
        comp_active.push(active);
    }

    // (a) lexical peel: drop lookupish table names (no `f11 < n_lookups` gate).
    for active in &mut comp_active {
        active.retain(|&i| {
            if is_lookupish_name(&spans[i].name) {
                dropped.insert(spans[i].rec_idx);
                false
            } else {
                true
            }
        });
    }

    // (b)+(c) fixpoint: confirm non-overlapping spans as recovered owners, then
    // drop the decisive ghosts, until no component changes.
    let mut recovered: Vec<usize> = Vec::new();
    let mut changed = true;
    while changed {
        changed = false;
        for active in &mut comp_active {
            let mut still = Vec::with_capacity(active.len());
            for &i in active.iter() {
                if overlaps_any(i, active, spans) {
                    still.push(i);
                } else {
                    recovered.push(i);
                    changed = true;
                }
            }
            *active = still;
        }
        recovered.sort_by_key(|&i| (spans[i].start, spans[i].rec_idx));
        for active in &mut comp_active {
            let overl: Vec<usize> = active
                .iter()
                .copied()
                .filter(|&i| overlaps_any(i, active, spans))
                .collect();
            if overl.is_empty() {
                continue;
            }
            let mut ghosts: Vec<usize> = Vec::new();
            let mut has_owner = false;
            for &i in &overl {
                if residual_span_is_owner(i, &recovered, &uncontested, spans) {
                    has_owner = true;
                } else {
                    ghosts.push(i);
                }
            }
            if has_owner && !ghosts.is_empty() {
                let ghost_set: HashSet<usize> = ghosts.iter().copied().collect();
                for &i in &ghosts {
                    dropped.insert(spans[i].rec_idx);
                }
                active.retain(|&i| !ghost_set.contains(&i));
                changed = true;
            }
        }
    }

    // (d) honest-drop the still-conflicted remainder; report it on diagnostics.
    let mut unresolved: Vec<ResidualOverlapComponent> = Vec::new();
    for (c, active) in components.iter().zip(comp_active.iter()) {
        let leftover: Vec<usize> = active
            .iter()
            .copied()
            .filter(|&i| overlaps_any(i, active, spans))
            .collect();
        if leftover.is_empty() {
            continue;
        }
        let leftover_set: HashSet<usize> = leftover.iter().copied().collect();
        let mut span_indices: Vec<usize> = c
            .span_indices
            .iter()
            .copied()
            .filter(|i| leftover_set.contains(i))
            .collect();
        span_indices.sort_unstable();
        for &i in &leftover {
            dropped.insert(spans[i].rec_idx);
        }
        let mut contested: Vec<usize> = Vec::new();
        for (ai, &a) in leftover.iter().enumerate() {
            for &b in &leftover[ai + 1..] {
                if spans_overlap(&spans[a], &spans[b]) {
                    let lo = spans[a].start.max(spans[b].start);
                    let hi = spans[a].end.min(spans[b].end);
                    contested.extend(lo..hi);
                }
            }
        }
        contested.sort_unstable();
        contested.dedup();
        unresolved.push(ResidualOverlapComponent {
            span_indices,
            contested_ots: contested,
        });
    }

    // Un-peeled phase-1 descriptors that were kept are readmitted as owners.
    let mut kept_recidx: HashSet<usize> = recovered.iter().map(|&i| spans[i].rec_idx).collect();
    for active in &comp_active {
        for &i in active {
            kept_recidx.insert(spans[i].rec_idx);
        }
    }
    let readmitted: HashSet<usize> = unpeeled_recidx
        .into_iter()
        .filter(|r| kept_recidx.contains(r) && !dropped.contains(r))
        .collect();

    ResidualResolution {
        dropped,
        readmitted,
        unresolved_components: unresolved,
    }
}

/// Identify *standalone* (non-overlapping) graphical-function descriptor
/// records to DROP.
///
/// A bare graphical function is a **table, not a time series**: Vensim saves no
/// series for it, only a descriptor record whose `f[11]` is a section-6
/// lookup-record index (not an OT start). `identify_descriptor_records` only
/// peels descriptors that sit in an *overlapping* OT component (a collision
/// with their consuming owner reveals them). A lookup-only variable saved
/// *only* as a descriptor collides with nothing, so it would otherwise decode
/// at its spurious `f[11]`-as-OT-start ghost slot (a stock slot holding
/// `0`/garbage; see `docs/design/vdf.md`, "Standalone graphical-function
/// descriptors"). This recognises such a record and returns its `rec_idx` so
/// the caller drops it -- exactly like an overlapping descriptor. The table's
/// values, where they matter, are carried by the consumer variables that call
/// it (e.g. `Historical GDP[COP] = IF Time<=cutoff THEN Historical GDP
/// LOOKUP(Time/One year) ELSE :NA:`), which the reader emits as ordinary
/// owners under their own names.
///
/// This pure function (functional core) detects the descriptor conservatively
/// to avoid dropping a real owner:
/// - the span is NOT in `overlapping` (the connected-component peeling path
///   owns the overlapping case);
/// - its `f[11]` (`f11_by_span[i]`) is a valid section-6 lookup-record index
///   (`< n_lookups`) -- the structural pre-condition for the forward link;
/// - **every** `f[11]`-as-OT-start ghost slot (`span.start .. span.end`)
///   carries the **stock** class code (`0x08`). A graphical function is never a
///   stock, so landing on stock slots is the spurious-owner telltale; a
///   legitimate non-stock owner whose `f[11]` is coincidentally `< n_lookups`
///   carries a `0x11` (dynamic) etc. code and is left untouched;
/// - the forward link `lookup_record[f[11]].word[10]` is a valid data OT
///   (`1 <= ot < ot_count`) with owner class codes across
///   `[word[10], word[10] + span_len)`, and for an **arrayed** descriptor
///   (`span_len > 1`) the forward width (`word[11]`) equals the element count.
///   These confirm `f[11]` really indexes this variable's graphical function
///   rather than coincidentally landing in the lookup-index range;
/// - **consumer corroboration**: the forward link must be the exact START of
///   a decoded span of exactly the descriptor's length that can actually be
///   an emitted owner: spans that are themselves standalone candidates and
///   spans already peeled as overlap-path descriptors (`peeled_descriptors`)
///   are excluded as corroborators -- a dropped ghost cannot vouch for
///   another drop, and two real stocks must not mutually corroborate each
///   other's (wrong) drop. A lookup's consumer is by definition a real saved
///   variable, so a genuine bare-lookup descriptor's forward link resolves
///   to a decoded consumer span (10/10 on `Ref.vdf`). A real stock whose own
///   OT start is coincidentally `< n_lookups` points, through the unrelated
///   lookup it accidentally indexes, at an arbitrary OT that is usually not
///   a span start (3 of the 4 `SimService/Base.vdf` false-positive stocks
///   fail here);
/// - **per-file coherence**: if ANY candidate that passes the physical gates
///   above fails consumer corroboration, NO standalone drop happens at all.
///   A writer that emits standalone bare-lookup descriptors does so
///   coherently (every one corroborates), while a file whose standalone
///   `f[11] < n_lookups` population is stocks-by-coincidence (`SimService`:
///   the leading alphabetical stock block collides with the lookup-index
///   range because both OT stock allocation and the lookup array are
///   alphabetical) will contain uncorroborated candidates, vetoing the set.
///   The veto is NOT silent: the outcome carries `veto_fired` and the count
///   of withheld candidates, surfaced on `DescriptorIdentification`, so a
///   future file with nine genuine bare lookups and one weird candidate
///   resurrecting nine ghost columns is observable rather than a repeat of
///   the silent-behavior class that shipped the original false positives.
///
/// Record-field discrimination is impossible here: the SimService
/// false-positive stocks carry `f[14] == 0xf6800000` just like descriptors
/// (they are lookup-associated stocks), while two genuine `Ref.vdf`
/// descriptors (`Ozone precursor forcings`, `"OC, BC, and bio aerosol
/// forcings"`) carry graph-metadata floats in `f[8]`/`f[9]`/`f[14]` instead
/// of sentinels -- no field separates the populations (see vdf.md
/// "Appendix: the owner/descriptor discriminator").
///
/// Not matched here (the model-free reader cannot safely distinguish them from
/// real owners, so they are left as-is): the `rs_hfc*` family, whose forward
/// link is a *wider* shared 2-D consumer (`word[11] != span_len`), and a
/// descriptor whose forward link is Time/`0`. The engine should not be
/// synthesising a `gf(Time)` series for any of these lookup tables in the first
/// place; see #597.
// Functional core: takes pre-extracted slices (the section-6 lookup forward
// links, OT class codes) rather than `&VdfFile`, so the detection is unit
// testable on synthetic inputs with no fixture. That decoupling is the reason
// for the parameter count.
#[allow(clippy::too_many_arguments)]
fn standalone_lookup_only_descriptors(
    spans: &[DecodedRecordSpan],
    f11_by_span: &[u32],
    overlapping: &HashSet<usize>,
    peeled_descriptors: &HashSet<usize>,
    n_lookups: usize,
    lookup_word10: &[usize],
    lookup_word11: &[usize],
    class_codes: &[u8],
    ot_count: usize,
) -> StandaloneDropOutcome {
    // Corroboration index: span start -> [(span index, length)].
    let mut span_starts: HashMap<usize, Vec<(usize, usize)>> = HashMap::new();
    for (i, span) in spans.iter().enumerate() {
        span_starts
            .entry(span.start)
            .or_default()
            .push((i, span.length()));
    }

    // Pass 1: candidates passing the physical gates, with their forward OT.
    let mut candidates: Vec<(usize, usize, usize)> = Vec::new(); // (span idx, rec_idx, fwd)
    for (i, span) in spans.iter().enumerate() {
        if overlapping.contains(&i) {
            continue;
        }
        let span_len = span.length();
        if span_len == 0 {
            continue;
        }
        let f11 = match f11_by_span.get(i) {
            Some(&v) => v as usize,
            None => continue,
        };
        // f[11] must be a valid section-6 lookup-record index.
        if f11 >= n_lookups {
            continue;
        }
        // Every f[11]-as-OT-start ghost slot must carry the STOCK class code --
        // the spurious-owner telltale (a graphical-function/lookup variable is
        // never a stock). For an arrayed descriptor all `span_len` ghost slots
        // are checked; for a scalar one this is the single `span.start` slot.
        let ghost_all_stock = (span.start..span.end)
            .all(|ot| class_codes.get(ot).copied() == Some(VDF_SECTION6_OT_CODE_STOCK));
        if !ghost_all_stock {
            continue;
        }
        // Resolve the forward link and require it be a valid data OT.
        let fwd = match lookup_word10.get(f11) {
            Some(&v) => v,
            None => continue,
        };
        if fwd == 0 || fwd >= ot_count {
            continue;
        }
        // Confirmation gate for arrayed descriptors: the forward link's output
        // width (`word[11]`) must equal the descriptor's element count, i.e. the
        // forward consumer has this variable's exact shape. This is what lets us
        // be confident `f[11]` indexes this variable's graphical function rather
        // than coincidentally landing in the lookup-index range -- so it is safe
        // to drop. A wider shared consumer (the `rs_hfc*` family forwarding to
        // one 63-wide block) has `width != span_len` and is NOT matched here
        // (the model-free reader can't safely tell it from an arrayed owner).
        // Scalar descriptors (no width gate) rely on the forward-link guards.
        if span_len > 1 {
            match lookup_word11.get(f11) {
                Some(&width) if width == span_len => {}
                _ => continue,
            }
        }
        // The whole forward block must carry real-data owner class codes.
        if fwd + span_len > ot_count {
            continue;
        }
        let fwd_block_all_owner = (fwd..fwd + span_len).all(|ot| {
            class_codes
                .get(ot)
                .copied()
                .map(is_owner_ot_class_code)
                .unwrap_or(false)
        });
        if !fwd_block_all_owner {
            continue;
        }
        candidates.push((i, span.rec_idx, fwd));
    }

    // Pass 2: consumer corroboration. Corroborators are restricted to spans
    // that can actually be emitted owners: a standalone candidate (which
    // might itself be dropped) and an overlap-peeled descriptor are excluded,
    // so a dropped ghost never vouches for a drop and two candidates never
    // mutually corroborate.
    let candidate_span_idxs: HashSet<usize> = candidates.iter().map(|&(i, _, _)| i).collect();
    let mut dropped = HashSet::new();
    let mut any_uncorroborated = false;
    for &(i, rec_idx, fwd) in &candidates {
        let span_len = spans[i].length();
        let corroborated = span_starts.get(&fwd).is_some_and(|entries| {
            entries.iter().any(|&(j, l)| {
                j != i
                    && l == span_len
                    && !candidate_span_idxs.contains(&j)
                    && !peeled_descriptors.contains(&spans[j].rec_idx)
            })
        });
        if corroborated {
            dropped.insert(rec_idx);
        } else {
            any_uncorroborated = true;
        }
    }

    // Per-file coherence: one uncorroborated candidate means the file's
    // standalone f[11] < n_lookups population is owners-by-coincidence,
    // so nothing is dropped. Refusing to drop is the safe failure mode --
    // and the veto is reported, not silent.
    if any_uncorroborated {
        return StandaloneDropOutcome {
            dropped: HashSet::new(),
            veto_fired: true,
            vetoed_candidates: candidates.len(),
        };
    }
    StandaloneDropOutcome {
        dropped,
        veto_fired: false,
        vetoed_candidates: 0,
    }
}

#[cfg(test)]
mod standalone_descriptor_tests {
    use super::*;
    // `VDF_SECTION6_OT_CODE_STOCK` arrives via `use super::*`; the dynamic and
    // Time codes are pulled in directly for the synthetic OT class arrays.
    use crate::vdf::{VDF_SECTION6_OT_CODE_DYNAMIC, VDF_SECTION6_OT_CODE_TIME};

    fn span(rec_idx: usize, name: &str, start: usize) -> DecodedRecordSpan {
        // Scalar span (length 1).
        span_with_len(rec_idx, name, start, 1)
    }

    fn span_with_len(rec_idx: usize, name: &str, start: usize, len: usize) -> DecodedRecordSpan {
        DecodedRecordSpan {
            rec_idx,
            name: name.to_string(),
            start,
            end: start + len,
            sort_key: 0,
        }
    }

    /// A standalone graphical-function descriptor whose `f[11]` is a valid
    /// section-6 lookup-record index and whose `f[11]`-as-OT-start lands on a
    /// STOCK (0x08) ghost slot must be DROPPED (recognised as a lookup-only
    /// table), not emitted at the ghost slot. Reproduces the `Ref.vdf`
    /// standalone-lookup-only mis-decode on a minimal synthetic record set
    /// (NOT keyed on any C-LEARN name).
    #[test]
    fn standalone_lookup_descriptor_is_dropped() {
        // OT layout (class codes): 0=Time, 1=dynamic owner (the real GF output
        // the descriptor must resolve to), 2=stock-coded GHOST slot the
        // descriptor's f[11]-as-OT-start spuriously lands on.
        let class_codes = [
            VDF_SECTION6_OT_CODE_TIME,    // OT 0: Time
            VDF_SECTION6_OT_CODE_DYNAMIC, // OT 1: real evaluated-output (forward link)
            VDF_SECTION6_OT_CODE_STOCK,   // OT 2: ghost stock slot
        ];
        let ot_count = class_codes.len();

        // Two lookup records. The descriptor's f[11] == 1 indexes lookup
        // record[1], whose word[10] (evaluated-output OT) == 1.
        let lookup_word10 = [9usize, 1usize];
        let n_lookups = lookup_word10.len();
        let lookup_word11 = vec![1usize; n_lookups];

        // One standalone descriptor span: its f[11] == 1 (a valid lookup
        // index), but as an OT-start it lands on OT 2 (the stock ghost). It is
        // NOT in any overlap component. The consumer span at the forward link
        // (OT 1) corroborates the drop; it is never a candidate itself
        // because its slot is DYNAMIC, so the ghost-stock gate rejects it
        // first (its f[11] of 0 would fail the forward-link gate too).
        let spans = [
            span(0, "Some Forcing graph", 2),
            span(1, "Some Forcing consumer", 1),
        ];
        let f11_by_span = [1u32, 0u32];
        let overlapping: HashSet<usize> = HashSet::new();

        let outcome = standalone_lookup_only_descriptors(
            &spans,
            &f11_by_span,
            &overlapping,
            &HashSet::new(),
            n_lookups,
            &lookup_word10,
            &lookup_word11,
            &class_codes,
            ot_count,
        );
        let dropped = outcome.dropped;

        // The descriptor (rec_idx 0) must be dropped (recognised as a
        // lookup-only table), NOT emitted at its ghost f[11]-as-OT-start slot.
        assert!(
            dropped.contains(&0),
            "lookup-only descriptor must be dropped"
        );
    }

    /// A legitimate scalar owner whose data lives at its `f[11]`-as-OT-start
    /// slot (a DYNAMIC 0x11 slot) must NOT be dropped, even if `f[11]` happens
    /// to be a valid lookup index. This guards the two `Ref.vdf`
    /// `*_conc_change_at_impact_year` owners (class 0x11) the fix must preserve.
    #[test]
    fn legit_dynamic_owner_with_small_f11_is_not_dropped() {
        let class_codes = [
            VDF_SECTION6_OT_CODE_TIME,
            VDF_SECTION6_OT_CODE_DYNAMIC, // OT 1: the owner's real data
        ];
        let ot_count = class_codes.len();
        let lookup_word10 = [9usize, 9usize];
        let n_lookups = lookup_word10.len();
        let lookup_word11 = vec![1usize; n_lookups];
        // f[11] == 1 is both the owner's OT start (dynamic, holds its data) AND
        // coincidentally a valid lookup index. It must stay an owner.
        let spans = [span(0, "Some Concentration", 1)];
        let f11_by_span = [1u32];
        let overlapping: HashSet<usize> = HashSet::new();

        let outcome = standalone_lookup_only_descriptors(
            &spans,
            &f11_by_span,
            &overlapping,
            &HashSet::new(),
            n_lookups,
            &lookup_word10,
            &lookup_word11,
            &class_codes,
            ot_count,
        );
        let dropped = outcome.dropped;
        assert!(
            dropped.is_empty(),
            "a dynamic-coded owner must not be dropped: {dropped:?}"
        );
    }

    /// Pins the STOCK-slot guard as the *sole* load-bearing reason a legitimate
    /// dynamic owner is left untouched.
    ///
    /// `legit_dynamic_owner_with_small_f11_is_not_dropped` (above) also exercises
    /// a dynamic owner, but there the forward link `lookup_word10[f11] == 9` is
    /// out of OT range, so the "valid forward data OT" guard rejects the drop
    /// *first* and the STOCK-slot guard never decides. This case constructs a
    /// span where every *other* precondition passes -- it is non-overlapping,
    /// scalar, its `f[11]` is a valid lookup index, and its forward link resolves
    /// to a valid in-range owner OT -- so the ONLY condition standing between the
    /// owner and a (wrong) drop is the STOCK-slot requirement: its
    /// `f[11]`-as-OT-start lands on a DYNAMIC (0x11) slot, not a STOCK (0x08)
    /// ghost. Removing or broadening that guard would flip this test to a
    /// non-empty drop (verified by mutation), so it enforces the docstring
    /// promise to preserve legitimate 0x11 owners.
    #[test]
    fn legit_dynamic_owner_blocked_only_by_stock_slot_guard() {
        // OT layout: 0=Time, 1=the dynamic owner's own data slot (where its
        // f[11]-as-OT-start lands -- DYNAMIC, NOT stock), 2=a second dynamic
        // owner that the forward link resolves to (a valid in-range data OT).
        let class_codes = [
            VDF_SECTION6_OT_CODE_TIME,
            VDF_SECTION6_OT_CODE_DYNAMIC, // OT 1: owner's f[11]-as-OT-start slot
            VDF_SECTION6_OT_CODE_DYNAMIC, // OT 2: a valid forward-link data OT
        ];
        let ot_count = class_codes.len();
        // f[11] == 1 indexes lookup record[1], whose word[10] == 2 -- an
        // in-range owner OT, so the forward-data-OT guards (1 <= fwd < ot_count
        // and fwd is an owner class) BOTH pass. Only the STOCK-slot guard at
        // span.start (OT 1, DYNAMIC) can reject.
        let lookup_word10 = [9usize, 2usize];
        let n_lookups = lookup_word10.len();
        let lookup_word11 = vec![1usize; n_lookups];
        // A consumer span at the forward link (OT 2) satisfies the consumer
        // corroboration gate, so ONLY the STOCK-slot guard can reject.
        let spans = [
            span(0, "Some Dynamic Owner", 1),
            span(1, "Forward Consumer", 2),
        ];
        let f11_by_span = [1u32, 0u32];
        let overlapping: HashSet<usize> = HashSet::new();

        let outcome = standalone_lookup_only_descriptors(
            &spans,
            &f11_by_span,
            &overlapping,
            &HashSet::new(),
            n_lookups,
            &lookup_word10,
            &lookup_word11,
            &class_codes,
            ot_count,
        );
        let dropped = outcome.dropped;
        assert!(
            dropped.is_empty(),
            "a dynamic owner whose only disqualifier is the non-stock slot must \
             not be dropped (the STOCK-slot guard is the sole gate here): {dropped:?}"
        );
    }

    /// A standalone descriptor whose forward link `word[10]` points at Time
    /// (OT 0) has no valid evaluated-output OT; it must NOT be dropped (Time is
    /// never a data owner). Guards the `Ref Global Emissions ... LOOKUP` case.
    #[test]
    fn standalone_descriptor_with_time_forward_link_is_not_dropped() {
        let class_codes = [
            VDF_SECTION6_OT_CODE_TIME,
            VDF_SECTION6_OT_CODE_STOCK, // OT 1: ghost stock slot
        ];
        let ot_count = class_codes.len();
        // lookup record[1].word[10] == 0 -> Time, an invalid evaluated output.
        let lookup_word10 = [9usize, 0usize];
        let n_lookups = lookup_word10.len();
        let lookup_word11 = vec![1usize; n_lookups];
        let spans = [span(0, "Ref graph LOOKUP", 1)];
        let f11_by_span = [1u32];
        let overlapping: HashSet<usize> = HashSet::new();

        let outcome = standalone_lookup_only_descriptors(
            &spans,
            &f11_by_span,
            &overlapping,
            &HashSet::new(),
            n_lookups,
            &lookup_word10,
            &lookup_word11,
            &class_codes,
            ot_count,
        );
        let dropped = outcome.dropped;
        assert!(
            dropped.is_empty(),
            "a Time forward-link must not be dropped: {dropped:?}"
        );
    }

    /// An OVERLAPPING descriptor (already handled by the connected-component
    /// peeling path) must NOT be dropped by the standalone path -- it is the
    /// existing path's responsibility to drop it in favor of its colliding
    /// consumer owner.
    #[test]
    fn overlapping_descriptor_is_left_to_the_component_path() {
        let class_codes = [VDF_SECTION6_OT_CODE_TIME, VDF_SECTION6_OT_CODE_STOCK];
        let ot_count = class_codes.len();
        let lookup_word10 = [9usize, 1usize];
        let n_lookups = lookup_word10.len();
        let lookup_word11 = vec![1usize; n_lookups];
        let spans = [span(0, "Overlapping graph", 1)];
        let f11_by_span = [1u32];
        // Mark span 0 as overlapping.
        let mut overlapping: HashSet<usize> = HashSet::new();
        overlapping.insert(0);

        let outcome = standalone_lookup_only_descriptors(
            &spans,
            &f11_by_span,
            &overlapping,
            &HashSet::new(),
            n_lookups,
            &lookup_word10,
            &lookup_word11,
            &class_codes,
            ot_count,
        );
        let dropped = outcome.dropped;
        assert!(
            dropped.is_empty(),
            "an overlapping descriptor must be left to the component path: {dropped:?}"
        );
    }

    /// An ARRAYED standalone descriptor (span length > 1) IS dropped to its
    /// forward OT-block start when the forward link's output width
    /// (`word[11]`) equals the descriptor's element count -- the clean case
    /// where the forward block is this variable's own per-element series. The
    /// caller emits the block with section-3 element labels. Mirrors the
    /// `Ref.vdf` `historical_*_lookup` / `rs_ch4` (COP, 7) bases.
    #[test]
    fn arrayed_standalone_descriptor_is_dropped_when_width_matches() {
        // OT layout: 0=Time, [1,4) = 3 dynamic owners (the forward block the
        // descriptor should resolve to), [4,7) = 3 stock GHOST slots its
        // f[11]-as-OT-start spuriously covers.
        let class_codes = [
            VDF_SECTION6_OT_CODE_TIME,
            VDF_SECTION6_OT_CODE_DYNAMIC,
            VDF_SECTION6_OT_CODE_DYNAMIC,
            VDF_SECTION6_OT_CODE_DYNAMIC,
            VDF_SECTION6_OT_CODE_STOCK,
            VDF_SECTION6_OT_CODE_STOCK,
            VDF_SECTION6_OT_CODE_STOCK,
        ];
        let ot_count = class_codes.len();
        // lookup record[1]: word[10] == 1 (forward block start), word[11] == 3
        // (output width == the descriptor's 3 elements).
        let lookup_word10 = [9usize, 1usize];
        let lookup_word11 = [0usize, 3usize];
        let n_lookups = lookup_word10.len();
        // A 3-element arrayed descriptor over the stock ghost span [4,7);
        // the 3-wide consumer span at the forward link (OT 1) corroborates.
        let spans = [
            span_with_len(0, "RS arrayed graph", 4, 3),
            span_with_len(1, "RS arrayed consumer", 1, 3),
        ];
        let f11_by_span = [1u32, 0u32];
        let overlapping: HashSet<usize> = HashSet::new();

        let outcome = standalone_lookup_only_descriptors(
            &spans,
            &f11_by_span,
            &overlapping,
            &HashSet::new(),
            n_lookups,
            &lookup_word10,
            &lookup_word11,
            &class_codes,
            ot_count,
        );
        let dropped = outcome.dropped;
        // Recognised as a lookup-only table and dropped.
        assert!(
            dropped.contains(&0),
            "arrayed lookup-only descriptor must be dropped"
        );
    }

    /// An arrayed descriptor whose forward link points at a WIDER shared
    /// consumer (`word[11] != span_len`) must NOT be dropped: the forward
    /// block is not this variable's own series. Pins the width gate as the
    /// sole disqualifier here -- every other precondition passes. Mirrors the
    /// `Ref.vdf` `rs_hfc*` family, where eight 7-element descriptors all
    /// forward-link to one 63-wide consumer block.
    #[test]
    fn arrayed_standalone_descriptor_with_wider_shared_consumer_is_not_dropped() {
        let class_codes = [
            VDF_SECTION6_OT_CODE_TIME,
            VDF_SECTION6_OT_CODE_DYNAMIC,
            VDF_SECTION6_OT_CODE_DYNAMIC,
            VDF_SECTION6_OT_CODE_DYNAMIC,
            VDF_SECTION6_OT_CODE_STOCK,
            VDF_SECTION6_OT_CODE_STOCK,
            VDF_SECTION6_OT_CODE_STOCK,
        ];
        let ot_count = class_codes.len();
        // Forward block start (OT 1) and an in-range owner block [1,4) are both
        // fine; ONLY the width mismatch (5 != 3) rejects the drop.
        let lookup_word10 = [9usize, 1usize];
        let lookup_word11 = [0usize, 5usize];
        let n_lookups = lookup_word10.len();
        // A 3-wide span at the forward link keeps consumer corroboration
        // satisfiable; ONLY the width mismatch (5 != 3) rejects the drop.
        let spans = [
            span_with_len(0, "RS HFC arrayed graph", 4, 3),
            span_with_len(1, "RS HFC shared consumer", 1, 3),
        ];
        let f11_by_span = [1u32, 0u32];
        let overlapping: HashSet<usize> = HashSet::new();

        let outcome = standalone_lookup_only_descriptors(
            &spans,
            &f11_by_span,
            &overlapping,
            &HashSet::new(),
            n_lookups,
            &lookup_word10,
            &lookup_word11,
            &class_codes,
            ot_count,
        );
        let dropped = outcome.dropped;
        assert!(
            dropped.is_empty(),
            "an arrayed descriptor with a wider shared consumer must not be \
             dropped (the width gate is the sole disqualifier here): {dropped:?}"
        );
    }

    /// A real stock whose own OT start is coincidentally a valid lookup
    /// index passes every physical gate (it IS a stock, so the ghost-stock
    /// telltale is trivially satisfied; scalar spans have no width gate;
    /// the unrelated lookup it indexes has a valid owner-coded forward
    /// link) -- consumer corroboration is the SOLE veto: no decoded span
    /// starts at the forward OT. Mirrors `SimService/Base.vdf`'s
    /// `Agriculture Employment` (first value 3.4e6), which the pre-gate
    /// code silently dropped.
    #[test]
    fn plain_stock_owner_without_consumer_span_is_not_dropped() {
        // OT layout: 0=Time, 1=the real stock (its own f[11]-as-OT-start
        // slot), 2=a dynamic owner slot the unrelated lookup's forward link
        // points at -- but NO decoded span starts there.
        let class_codes = [
            VDF_SECTION6_OT_CODE_TIME,
            VDF_SECTION6_OT_CODE_STOCK,   // OT 1: the stock's own data
            VDF_SECTION6_OT_CODE_DYNAMIC, // OT 2: forward link target, span-less
        ];
        let ot_count = class_codes.len();
        let lookup_word10 = [9usize, 2usize];
        let n_lookups = lookup_word10.len();
        let lookup_word11 = vec![1usize; n_lookups];
        let spans = [span(0, "Agriculture Employment", 1)];
        let f11_by_span = [1u32];
        let overlapping: HashSet<usize> = HashSet::new();

        let outcome = standalone_lookup_only_descriptors(
            &spans,
            &f11_by_span,
            &overlapping,
            &HashSet::new(),
            n_lookups,
            &lookup_word10,
            &lookup_word11,
            &class_codes,
            ot_count,
        );
        assert!(
            outcome.dropped.is_empty(),
            "a stock owner with an uncorroborated forward link must not be \
             dropped (consumer corroboration is the sole gate here): {:?}",
            outcome.dropped
        );
        // The veto is observable, never silent.
        assert!(outcome.veto_fired);
        assert_eq!(outcome.vetoed_candidates, 1);
    }

    /// Per-file coherence: when TWO candidates pass the physical gates and
    /// one of them fails consumer corroboration, NEITHER is dropped -- the
    /// uncorroborated candidate is evidence the file's standalone
    /// `f[11] < n_lookups` population is stocks-by-coincidence, not
    /// bare-lookup descriptors. Mirrors `SimService/Base.vdf`, where
    /// `Alaska Oil Discovered Reserves` happened to corroborate (its
    /// unrelated lookup's consumer exists as a span) but its three sibling
    /// stocks did not.
    #[test]
    fn one_uncorroborated_candidate_vetoes_all_standalone_drops() {
        // OT layout: 0=Time, 1..3=two stock slots (the two candidates'
        // f[11]-as-OT-start slots), 3=a dynamic slot where a decoded span
        // DOES start (corroborates candidate 0), 4=a dynamic slot where no
        // span starts (candidate 1 fails corroboration).
        let class_codes = [
            VDF_SECTION6_OT_CODE_TIME,
            VDF_SECTION6_OT_CODE_STOCK,   // OT 1: candidate 0's slot
            VDF_SECTION6_OT_CODE_STOCK,   // OT 2: candidate 1's slot
            VDF_SECTION6_OT_CODE_DYNAMIC, // OT 3: corroborated forward link
            VDF_SECTION6_OT_CODE_DYNAMIC, // OT 4: uncorroborated forward link
        ];
        let ot_count = class_codes.len();
        // lookup[1].word[10] == 3 (a span starts there), lookup[2].word[10]
        // == 4 (no span starts there).
        let lookup_word10 = [9usize, 3usize, 4usize];
        let n_lookups = lookup_word10.len();
        let lookup_word11 = vec![1usize; n_lookups];
        let spans = [
            span(0, "Alaska Oil Discovered Reserves", 1),
            span(1, "Atmos UOcean Temp", 2),
            span(2, "land allocated for ethanol", 3),
        ];
        let f11_by_span = [1u32, 2u32, 0u32];
        let overlapping: HashSet<usize> = HashSet::new();

        let outcome = standalone_lookup_only_descriptors(
            &spans,
            &f11_by_span,
            &overlapping,
            &HashSet::new(),
            n_lookups,
            &lookup_word10,
            &lookup_word11,
            &class_codes,
            ot_count,
        );
        assert!(
            outcome.dropped.is_empty(),
            "one uncorroborated candidate must veto every standalone drop \
             in the file: {:?}",
            outcome.dropped
        );
        // Both physically-gated candidates were withheld, observably.
        assert!(outcome.veto_fired);
        assert_eq!(outcome.vetoed_candidates, 2);
    }

    /// Two candidates must not mutually corroborate each other's drop: a
    /// corroborator has to be a span that can actually be an emitted owner,
    /// and a standalone candidate might itself be dropped. Here candidate 0's
    /// forward link lands exactly on candidate 1's span and vice versa --
    /// without the corroborator restriction both real stocks would be
    /// dropped and the veto would never fire.
    #[test]
    fn candidates_do_not_mutually_corroborate() {
        let class_codes = [
            VDF_SECTION6_OT_CODE_TIME,
            VDF_SECTION6_OT_CODE_STOCK, // OT 1: candidate 0's slot
            VDF_SECTION6_OT_CODE_STOCK, // OT 2: candidate 1's slot
        ];
        let ot_count = class_codes.len();
        // lookup[1].word[10] == 2 (candidate 1's slot), lookup[2].word[10]
        // == 1 (candidate 0's slot): a mutual-corroboration trap.
        let lookup_word10 = [9usize, 2usize, 1usize];
        let n_lookups = lookup_word10.len();
        let lookup_word11 = vec![1usize; n_lookups];
        let spans = [span(0, "Stock A", 1), span(1, "Stock B", 2)];
        let f11_by_span = [1u32, 2u32];
        let overlapping: HashSet<usize> = HashSet::new();

        let outcome = standalone_lookup_only_descriptors(
            &spans,
            &f11_by_span,
            &overlapping,
            &HashSet::new(),
            n_lookups,
            &lookup_word10,
            &lookup_word11,
            &class_codes,
            ot_count,
        );
        assert!(
            outcome.dropped.is_empty(),
            "two candidates must not mutually corroborate: {:?}",
            outcome.dropped
        );
        assert!(outcome.veto_fired);
        assert_eq!(outcome.vetoed_candidates, 2);
    }

    /// An overlap-peeled descriptor cannot corroborate a standalone drop: a
    /// peeled ghost is not an emitted owner. The forward link here lands on a
    /// span that is in `peeled_descriptors`, so the candidate is
    /// uncorroborated and the veto fires.
    #[test]
    fn peeled_descriptor_does_not_corroborate() {
        let class_codes = [
            VDF_SECTION6_OT_CODE_TIME,
            VDF_SECTION6_OT_CODE_STOCK,   // OT 1: candidate's slot
            VDF_SECTION6_OT_CODE_DYNAMIC, // OT 2: forward link, peeled span
        ];
        let ot_count = class_codes.len();
        let lookup_word10 = [9usize, 2usize];
        let n_lookups = lookup_word10.len();
        let lookup_word11 = vec![1usize; n_lookups];
        let spans = [span(0, "Some Stock", 1), span(7, "Peeled ghost", 2)];
        let f11_by_span = [1u32, 0u32];
        let overlapping: HashSet<usize> = HashSet::new();
        // Span 1 (rec_idx 7) was peeled as an overlap-path descriptor.
        let peeled: HashSet<usize> = [7usize].into_iter().collect();

        let outcome = standalone_lookup_only_descriptors(
            &spans,
            &f11_by_span,
            &overlapping,
            &peeled,
            n_lookups,
            &lookup_word10,
            &lookup_word11,
            &class_codes,
            ot_count,
        );
        assert!(
            outcome.dropped.is_empty(),
            "a peeled descriptor must not corroborate a drop: {:?}",
            outcome.dropped
        );
        assert!(outcome.veto_fired);
        assert_eq!(outcome.vetoed_candidates, 1);
    }
}

#[cfg(test)]
mod residual_overlap_tests {
    use super::*;

    fn span_with_len(rec_idx: usize, name: &str, start: usize, len: usize) -> DecodedRecordSpan {
        DecodedRecordSpan {
            rec_idx,
            name: name.to_string(),
            start,
            end: start + len,
            sort_key: 0,
        }
    }

    fn scalar(rec_idx: usize, name: &str, start: usize) -> DecodedRecordSpan {
        span_with_len(rec_idx, name, start, 1)
    }

    /// A clean, non-overlapping owner partition (the corpus norm once
    /// descriptors are peeled) yields NO residual components, so the floor is
    /// a provable no-op on every file the peel already resolves.
    #[test]
    fn clean_partition_has_no_residual_components() {
        let spans = [
            scalar(0, "alpha", 1),
            scalar(1, "beta", 2),
            span_with_len(2, "gamma", 3, 3),
        ];
        let components = residual_overlap_components(&spans, &HashSet::new());
        assert!(
            components.is_empty(),
            "no overlap must produce no residual components: {components:?}"
        );
    }

    /// Two differently-named spans that share an OT slot after peeling are a
    /// genuine owner-vs-owner conflict: both are pulled into one component so
    /// the caller can drop them (honest missing data over a silent
    /// alphabetical first-claim win).
    #[test]
    fn distinct_name_overlap_is_a_residual_component() {
        let spans = [scalar(0, "china gdp", 5), scalar(1, "row coal demand", 5)];
        let components = residual_overlap_components(&spans, &HashSet::new());
        assert_eq!(components.len(), 1, "one contested slot, one component");
        assert_eq!(components[0].span_indices, vec![0, 1]);
        assert_eq!(components[0].contested_ots, vec![5]);
    }

    /// Two overlapping spans of the SAME name are the ordinary
    /// same-variable duplicate the per-name emitter dedup already resolves;
    /// they must NOT be flagged as a cross-variable conflict (dropping both
    /// would lose the variable entirely).
    #[test]
    fn same_name_overlap_is_not_residual() {
        let spans = [scalar(0, "population", 3), scalar(1, "population", 3)];
        let components = residual_overlap_components(&spans, &HashSet::new());
        assert!(
            components.is_empty(),
            "same-name overlap is not a residual conflict: {components:?}"
        );
    }

    /// A wide ghost span (a stale-`f[11]` unsaved-variable record) covering
    /// several narrow real owners forms ONE component containing the ghost
    /// and every owner it overlaps -- the `SimService/Base.vdf` cluster-A
    /// shape. Stage 1 drops the whole component; a later stage re-resolves it.
    #[test]
    fn wide_ghost_over_narrow_owners_forms_one_component() {
        let spans = [
            span_with_len(0, "age specific fertility distribution function", 2, 6),
            scalar(1, "c real gdp", 2),
            scalar(2, "capital agriculture", 4),
            span_with_len(3, "co2 in deep ocean", 5, 3),
            // An unrelated, non-overlapping owner outside the ghost span.
            scalar(4, "agricultural land in use", 20),
        ];
        let components = residual_overlap_components(&spans, &HashSet::new());
        assert_eq!(components.len(), 1, "one wide ghost, one component");
        assert_eq!(
            components[0].span_indices,
            vec![0, 1, 2, 3],
            "the ghost and every owner it covers are in the component; the \
             disjoint owner is not"
        );
        assert_eq!(components[0].contested_ots, vec![2, 4, 5, 6, 7]);
    }

    /// A span already dropped (a peeled descriptor or standalone lookup-only
    /// table) is invisible to residual detection: it cannot re-introduce a
    /// conflict it is no longer part of.
    #[test]
    fn dropped_span_is_excluded() {
        let spans = [scalar(0, "china gdp", 5), scalar(1, "row coal demand", 5)];
        // Span 1 (rec_idx 1) was already dropped upstream, so span 0 no longer
        // conflicts with anything.
        let dropped: HashSet<usize> = [1usize].into_iter().collect();
        let components = residual_overlap_components(&spans, &dropped);
        assert!(
            components.is_empty(),
            "a dropped span cannot form a residual conflict: {components:?}"
        );
    }

    /// Two disjoint conflicts produce two separate components, ordered
    /// deterministically by first contested OT.
    #[test]
    fn disjoint_conflicts_are_separate_components() {
        let spans = [
            scalar(0, "b var", 30),
            scalar(1, "a var", 30),
            scalar(2, "d var", 10),
            scalar(3, "c var", 10),
        ];
        let components = residual_overlap_components(&spans, &HashSet::new());
        assert_eq!(components.len(), 2);
        // Deterministic order: the OT-10 conflict comes before the OT-30 one.
        assert_eq!(components[0].contested_ots, vec![10]);
        assert_eq!(components[0].span_indices, vec![2, 3]);
        assert_eq!(components[1].contested_ots, vec![30]);
        assert_eq!(components[1].span_indices, vec![0, 1]);
    }
}

/// Stage 2 re-resolution (`resolve_residual_components`): the ordering oracle,
/// exercised on synthetic spans that reproduce each Base.vdf trap in miniature.
#[cfg(test)]
mod resolve_residual_tests {
    use super::*;

    fn span(rec_idx: usize, name: &str, start: usize, len: usize) -> DecodedRecordSpan {
        DecodedRecordSpan {
            rec_idx,
            name: name.to_string(),
            start,
            end: start + len,
            sort_key: 0,
        }
    }

    fn dropped_names<'a>(res: &ResidualResolution, spans: &'a [DecodedRecordSpan]) -> Vec<&'a str> {
        let mut names: Vec<&str> = spans
            .iter()
            .filter(|s| res.dropped.contains(&s.rec_idx))
            .map(|s| s.name.as_str())
            .collect();
        names.sort_unstable();
        names
    }

    /// Empty components -> the whole re-resolution is a no-op (the corpus norm:
    /// 139/141 files have no residual overlap at all).
    #[test]
    fn empty_components_is_noop() {
        let spans = [span(0, "a", 1, 1), span(1, "b", 2, 1)];
        let res = resolve_residual_components(&spans, &[], &HashSet::new(), RESIDUAL_ORDERING_GATE);
        assert!(res.dropped.is_empty() && res.readmitted.is_empty());
        assert!(res.unresolved_components.is_empty());
    }

    /// Run-boundary trap (cluster A). A wide ghost sorts BEFORE the real owners
    /// it covers; the next uncontested anchor sorts before the prev anchor (a
    /// run boundary), so the oracle falls back to the reliable prev side alone.
    /// The ghost fails prev and is dropped; every narrow owner passes.
    #[test]
    fn run_boundary_prev_only_drops_wide_ghost() {
        // Uncontested anchors: `c gas`@1 (prev) and `agri`@30 (next, sorts
        // before `c gas` -> inverted bracket -> prev-only).
        let spans = [
            span(0, "c gas", 1, 1),
            span(1, "agri", 30, 1),
            span(2, "age ghost", 3, 4), // wide ghost [3,7)
            span(3, "c oil", 3, 1),
            span(4, "c pig", 4, 1),
            span(5, "c rat", 5, 1),
            span(6, "c sun", 6, 1),
        ];
        let components = residual_overlap_components(&spans, &HashSet::new());
        let res = resolve_residual_components(&spans, &components, &HashSet::new(), 0.0);
        assert_eq!(dropped_names(&res, &spans), vec!["age ghost"]);
        assert!(res.readmitted.is_empty());
        assert!(res.unresolved_components.is_empty());
    }

    /// Recovered-anchor discrimination (comp2). Two non-lookupish spans share a
    /// slot; the nearest UNCONTESTED owners bracket BOTH (neither can be told
    /// apart that way). Only the recovered reals from the adjacent
    /// lexical-peel-resolved pairs form the tight same-run bracket that drops
    /// the ghost -- proving the recovered-anchor tier is load-bearing.
    #[test]
    fn recovered_anchor_bracket_resolves_scalar_pair() {
        let spans = [
            span(0, "tbl x lookup", 9, 2),  // comp1 lookupish ghost [9,11)
            span(1, "ind a", 10, 1),        // comp1 real
            span(2, "ind b", 12, 1),        // comp2 real (the hard one)
            span(3, "cat food", 12, 1),     // comp2 ghost (non-lookupish, run X)
            span(4, "tbl y lookup", 13, 2), // comp3 lookupish ghost [13,15)
            span(5, "ind c", 14, 1),        // comp3 real
            // Uncontested owners that loosely bracket comp2 on BOTH sides but
            // cannot discriminate (both candidates fall within [a1, z1]).
            span(6, "a1", 11, 1),
            span(7, "z1", 16, 1),
        ];
        let components = residual_overlap_components(&spans, &HashSet::new());
        let res = resolve_residual_components(&spans, &components, &HashSet::new(), 0.0);
        assert_eq!(
            dropped_names(&res, &spans),
            vec!["cat food", "tbl x lookup", "tbl y lookup"]
        );
        assert!(res.unresolved_components.is_empty());
    }

    /// Inconclusive -> honest-drop fallback. When no candidate for a shared slot
    /// is ordering-consistent (no owner to keep), the oracle abstains on that
    /// conflict and both spans are honest-dropped and surfaced on the
    /// diagnostics -- never silently first-claimed.
    #[test]
    fn inconclusive_conflict_is_honest_dropped() {
        let spans = [
            span(0, "a", 1, 1),
            span(1, "b", 9, 1),
            // Both sort AFTER the next anchor `b`@9 -> neither is a valid owner.
            span(2, "yyy", 5, 1),
            span(3, "zzz", 5, 1),
        ];
        let components = residual_overlap_components(&spans, &HashSet::new());
        let res = resolve_residual_components(&spans, &components, &HashSet::new(), 0.0);
        assert_eq!(dropped_names(&res, &spans), vec!["yyy", "zzz"]);
        assert_eq!(res.unresolved_components.len(), 1);
        assert_eq!(res.unresolved_components[0].contested_ots, vec![5]);
    }

    /// Un-peel + readmit (the `c Identified Oil Reserve` case). A phase-1
    /// overlap-peeled descriptor that spatially belongs to a component is given
    /// a second chance; when the oracle keeps it, it is readmitted as an owner.
    #[test]
    fn unpeeled_descriptor_is_readmitted_when_kept() {
        let spans = [
            span(0, "c gas", 1, 1),
            span(1, "agri", 20, 1),
            span(2, "age ghost", 8, 4), // wide ghost [8,12)
            span(3, "c pig", 9, 1),
            span(4, "c rat", 10, 1),
            span(5, "c sun", 11, 1),
            span(6, "c oil", 7, 2), // phase-1 descriptor [7,9), overlaps ghost @8
        ];
        let phase1: HashSet<usize> = [6usize].into_iter().collect();
        let components = residual_overlap_components(&spans, &phase1);
        let res = resolve_residual_components(&spans, &components, &phase1, 0.0);
        assert_eq!(dropped_names(&res, &spans), vec!["age ghost"]);
        assert_eq!(res.readmitted, [6usize].into_iter().collect());
        assert!(res.unresolved_components.is_empty());
    }

    /// Lexically lookupish names in a component are peeled WITHOUT the
    /// `f11 < n_lookups` gate: a lookup definition is a table, not a series.
    #[test]
    fn lexical_peel_drops_lookupish_names() {
        let spans = [
            span(0, "real var", 1, 1),
            span(1, "real var2", 2, 1),
            span(2, "foo lookup", 1, 2), // lookupish ghost [1,3)
        ];
        let components = residual_overlap_components(&spans, &HashSet::new());
        let res = resolve_residual_components(&spans, &components, &HashSet::new(), 0.0);
        assert_eq!(dropped_names(&res, &spans), vec!["foo lookup"]);
        assert!(res.unresolved_components.is_empty());
    }

    /// Chained-descriptor un-peel boundary. A phase-1 descriptor is un-peeled
    /// iff it overlaps an ORIGINAL component span -- never merely a
    /// previously-un-peeled descriptor. `d oil`@[9,12) overlaps the ghost and
    /// is un-peeled (and kept -> readmitted); `d tar`@[11,13) overlaps only
    /// `d oil`, so it stays a descriptor and is neither dropped nor readmitted.
    /// Pins the Rust/Python bit-parity contract on this constructible case.
    #[test]
    fn chained_descriptor_is_not_transitively_unpeeled() {
        let spans = [
            span(0, "c gas", 1, 1),     // uncontested prev anchor
            span(1, "zz", 30, 1),       // uncontested next anchor
            span(2, "age ghost", 5, 5), // wide ghost [5,10)
            span(3, "d1", 5, 1),
            span(4, "d2", 6, 1),
            span(5, "d3", 7, 1),
            span(6, "d4", 8, 1),
            span(7, "d oil", 9, 3), // phase-1 descriptor [9,12), overlaps ghost @9
            span(8, "d tar", 11, 2), // phase-1 descriptor [11,13), overlaps ONLY d oil
        ];
        let phase1: HashSet<usize> = [7usize, 8].into_iter().collect();
        let components = residual_overlap_components(&spans, &phase1);
        let res = resolve_residual_components(&spans, &components, &phase1, 0.0);
        assert_eq!(dropped_names(&res, &spans), vec!["age ghost"]);
        assert_eq!(res.readmitted, [7usize].into_iter().collect());
        assert!(
            !res.dropped.contains(&8) && !res.readmitted.contains(&8),
            "d tar overlaps only an un-peeled descriptor, so it must stay untouched"
        );
        assert!(res.unresolved_components.is_empty());
    }

    /// Gate abstention. A file whose uncontested owners do NOT exhibit the
    /// alphabetical-allocation invariant fails the gate, so the oracle abstains
    /// and every residual span is honest-dropped (Stage 1 semantics).
    #[test]
    fn gate_abstains_when_owners_not_alphabetical() {
        // Badly-shuffled uncontested owners -> low alphabetical consistency.
        let spans = [
            span(0, "z", 1, 1),
            span(1, "a", 2, 1),
            span(2, "m", 3, 1),
            span(3, "b", 4, 1),
            span(4, "ghost", 10, 1),
            span(5, "owner", 10, 1),
        ];
        let components = residual_overlap_components(&spans, &HashSet::new());
        let res = resolve_residual_components(&spans, &components, &HashSet::new(), 0.95);
        // Both contested spans dropped; the component is reported unresolved.
        assert_eq!(dropped_names(&res, &spans), vec!["ghost", "owner"]);
        assert_eq!(res.unresolved_components.len(), 1);
    }
}

/// A reconstructed result-emission candidate. A `RecordResultCandidate`
/// covers exactly one OT-aligned span and binds it to one or more section-1
/// records. Multiple records can collapse onto the same span when several
/// alias names share a slot (e.g. SMOOTH/DELAY internal helpers).
#[derive(Clone, Debug)]
pub(super) struct RecordResultCandidate {
    pub(super) start: usize,
    pub(super) span: usize,
    pub(super) record_indices: Vec<usize>,
}

fn shape_template_entry_for_record_candidate<'a>(
    vdf: &VdfFile,
    candidate: &RecordResultCandidate,
    section3_directory: Option<&'a VdfSection3Directory>,
) -> Option<&'a VdfSection3DirectoryEntry> {
    let directory = section3_directory?;
    let mut by_offset: HashMap<usize, &VdfSection3DirectoryEntry> = HashMap::new();
    let mut saw_generic_array_marker = false;

    for &record_index in &candidate.record_indices {
        let Some(record) = vdf.records.get(record_index) else {
            continue;
        };
        let shape_code = record.fields[6];
        saw_generic_array_marker |= shape_code == 32;
        if shape_code == 0 || shape_code == 5 {
            continue;
        }
        if let Some(entry) = directory.entry_for_record_shape_code(shape_code)
            && entry.flat_size() == candidate.span
        {
            by_offset.insert(entry.file_offset, entry);
        }
    }

    // The generic 32 marker is only safe when the candidate's flat size
    // identifies exactly one active section-3 template.
    if by_offset.is_empty() && saw_generic_array_marker {
        let active: Vec<&VdfSection3DirectoryEntry> = directory
            .entries
            .iter()
            .filter(|entry| entry.flat_size() == candidate.span && entry.flat_size() > 0)
            .collect();
        if active.len() == 1 {
            by_offset.insert(active[0].file_offset, active[0]);
        }
    }

    if by_offset.len() == 1 {
        by_offset.into_values().next()
    } else {
        None
    }
}

/// Label an array owner span from the section-3 axis-ref bridge.
///
/// The candidate has already established the base variable and OT span.
/// This step is deliberately narrower: it only emits element labels when
/// the span's section-3 shape resolves to axis refs that point at decoded
/// dimension anchors with matching cardinalities. Otherwise callers keep
/// the old numeric fallback rather than guessing from same-size dimensions.
pub(super) fn array_element_labels_for_record_candidate(
    vdf: &VdfFile,
    candidate: &RecordResultCandidate,
    section3_directory: Option<&VdfSection3Directory>,
    dimension_elements_by_name: &HashMap<String, Vec<String>>,
    axis_ref_to_dim_name: &HashMap<u32, String>,
) -> Option<Vec<String>> {
    if candidate.span <= 1 {
        return None;
    }
    let entry = shape_template_entry_for_record_candidate(vdf, candidate, section3_directory)?;
    if entry.flat_size() != candidate.span {
        return None;
    }

    let axis_sizes = entry.axis_sizes();
    let axis_refs: Vec<u32> = entry
        .axis_slot_refs()
        .into_iter()
        .filter(|&axis_ref| axis_ref > 0)
        .collect();
    if axis_sizes.is_empty() || axis_sizes.len() != axis_refs.len() {
        return None;
    }
    let flat_size = axis_sizes
        .iter()
        .try_fold(1usize, |acc, size| acc.checked_mul(*size))?;
    if flat_size != candidate.span {
        return None;
    }

    let mut axes = Vec::with_capacity(axis_sizes.len());
    for (axis_size, axis_ref) in axis_sizes.into_iter().zip(axis_refs) {
        let dim_name = axis_ref_to_dim_name.get(&axis_ref)?;
        let elements = dimension_elements_by_name.get(&dim_name.to_lowercase())?;
        if elements.len() != axis_size {
            return None;
        }
        axes.push(elements.clone());
    }

    Some(cartesian_axis_labels(&axes))
}

fn cartesian_axis_labels(axes: &[Vec<String>]) -> Vec<String> {
    match axes {
        [] => Vec::new(),
        [single] => single.clone(),
        _ => {
            let mut labels = vec![String::new()];
            for axis in axes {
                let mut next = Vec::with_capacity(labels.len() * axis.len());
                for prefix in &labels {
                    for element in axis {
                        if prefix.is_empty() {
                            next.push(element.clone());
                        } else {
                            next.push(format!("{prefix},{element}"));
                        }
                    }
                }
                labels = next;
            }
            labels
        }
    }
}

/// Build the ordered `(Ident, OT)` column list for `to_results_via_records`.
///
/// Pipeline:
///   1. `decoded_record_spans` produces structurally-valid record-to-OT
///      spans (post class-code guard).
///   2. `identify_descriptor_records` removes graphical-function descriptor
///      records via the decoded forward link into the section-6 lookup
///      array.
///   3. The remaining owner spans are partitioned into model vs system
///      names (Vensim's case-insensitive sort decides emission order
///      within each partition); `Time` always heads the list at OT[0].
pub(super) fn build_record_result_columns(
    vdf: &VdfFile,
    name_key_to_name_index: &HashMap<u32, usize>,
    section3_directory: Option<&VdfSection3Directory>,
    dimension_elements_by_name: &HashMap<String, Vec<String>>,
    axis_ref_to_dim_name: &HashMap<u32, String>,
) -> Vec<(Ident<Canonical>, usize)> {
    let spans = decoded_record_spans(vdf, name_key_to_name_index, section3_directory);
    let desc_id = identify_descriptor_records(vdf, &spans);

    let mut model_spans: HashMap<&str, &DecodedRecordSpan> = HashMap::new();
    let mut system_spans: HashMap<&str, &DecodedRecordSpan> = HashMap::new();
    for span in spans
        .iter()
        .filter(|s| !desc_id.descriptor_indices.contains(&s.rec_idx))
    {
        if span.name == "Time" {
            continue;
        }
        let target = if SYSTEM_NAMES.contains(&span.name.as_str()) {
            &mut system_spans
        } else {
            &mut model_spans
        };
        match target.get(span.name.as_str()) {
            Some(prev) if prev.start <= span.start => {}
            _ => {
                target.insert(span.name.as_str(), span);
            }
        }
    }

    let mut ordered: Vec<(Ident<Canonical>, usize)> =
        vec![(Ident::<Canonical>::from_str_unchecked("time"), 0)];
    let mut claimed_ot: HashSet<usize> = HashSet::new();
    claimed_ot.insert(0);

    let mut model_names: Vec<&str> = model_spans.keys().copied().collect();
    model_names.sort_by_key(|name| name.to_lowercase());
    for name in model_names {
        emit_owner_span(
            vdf,
            model_spans[name],
            section3_directory,
            dimension_elements_by_name,
            axis_ref_to_dim_name,
            &mut ordered,
            &mut claimed_ot,
        );
    }

    let mut system_names_sorted: Vec<&str> = SYSTEM_NAMES
        .iter()
        .copied()
        .filter(|n| *n != "Time")
        .collect();
    system_names_sorted.sort_by_key(|name| name.to_lowercase());
    for name in system_names_sorted {
        if let Some(span) = system_spans.get(name) {
            emit_owner_span(
                vdf,
                span,
                section3_directory,
                dimension_elements_by_name,
                axis_ref_to_dim_name,
                &mut ordered,
                &mut claimed_ot,
            );
        }
    }

    // Standalone lookup-only descriptors are graphical-function tables, not
    // saved series, so `identify_descriptor_records` puts them in
    // `descriptor_indices` and they are dropped above (never emitted). Their
    // values, where they matter, are carried by the consumer variables that
    // call them, which appear here as ordinary owners under their own names.

    ordered
}

/// Append one owner span's columns to `ordered`, marking the OT slots in
/// `claimed_ot`. The span has already been validated as an owner record
/// (post descriptor identification). Element labels are resolved via
/// `array_element_labels_for_record_candidate`, which drives the
/// shape-template -> axis-ref -> dimension-elements bridge for arrayed
/// spans.
fn emit_owner_span(
    vdf: &VdfFile,
    span: &DecodedRecordSpan,
    section3_directory: Option<&VdfSection3Directory>,
    dimension_elements_by_name: &HashMap<String, Vec<String>>,
    axis_ref_to_dim_name: &HashMap<u32, String>,
    ordered: &mut Vec<(Ident<Canonical>, usize)>,
    claimed_ot: &mut HashSet<usize>,
) {
    let candidate = RecordResultCandidate {
        start: span.start,
        span: span.length(),
        record_indices: vec![span.rec_idx],
    };
    let element_labels = array_element_labels_for_record_candidate(
        vdf,
        &candidate,
        section3_directory,
        dimension_elements_by_name,
        axis_ref_to_dim_name,
    );
    for elem in 0..candidate.span {
        let ot = candidate.start + elem;
        if !claimed_ot.insert(ot) {
            continue;
        }
        let display = if candidate.span > 1 {
            match element_labels.as_ref().and_then(|labels| labels.get(elem)) {
                Some(label) => format!("{}[{}]", span.name, label),
                None => format!("{}[{elem}]", span.name),
            }
        } else {
            span.name.clone()
        };
        // System and user names flow through `Ident::new`, which lowercases
        // and strips spaces/underscores. `#`-prefixed internal signatures
        // (and other names with non-canonicalisable characters) use
        // `from_str_unchecked` so the raw name survives as the result
        // column key; otherwise they would collapse into an empty Ident.
        let key = if display.starts_with('#') {
            Ident::<Canonical>::from_str_unchecked(&display)
        } else {
            Ident::<Canonical>::new(&display)
        };
        ordered.push((key, ot));
    }
}
