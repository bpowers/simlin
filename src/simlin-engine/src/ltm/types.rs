// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Public LTM data types: link/loop polarity, link, loop, and the
//! truncation marker. These are the user-visible vocabulary the rest of the
//! LTM submodules build on.

use crate::common::{Canonical, Ident};

/// Marker returned by circuit-enumeration helpers when the DFS bailed
/// out because it would have exceeded the caller-supplied `max_circuits`
/// budget.  Production callers pass `usize::MAX` (no truncation) so they
/// never see this value; stress tests and diagnostic harnesses use
/// smaller budgets and check for it explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TruncatedByBudget;

/// Polarity of a causal link
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkPolarity {
    Positive, // Increase in 'from' causes increase in 'to'
    Negative, // Increase in 'from' causes decrease in 'to'
    Unknown,  // Cannot determine polarity statically
}

impl LinkPolarity {
    /// Compose two consecutive link polarities (sign multiplication).
    ///
    /// Used when collapsing a chain `X -> M -> Y` into a single edge
    /// `X -> Y`: the resulting polarity is the product of the two.
    /// `Unknown` is absorbing -- any chain through an unknown-polarity
    /// link is itself unknown.
    pub fn compose(self, other: LinkPolarity) -> LinkPolarity {
        use LinkPolarity::*;
        match (self, other) {
            (Unknown, _) | (_, Unknown) => Unknown,
            (Positive, Positive) | (Negative, Negative) => Positive,
            (Positive, Negative) | (Negative, Positive) => Negative,
        }
    }
}

/// Represents a causal link between two variables.
///
/// The per-reference access shape distinction (Bare / FixedIndex / Wildcard
/// / DynamicIndex) is encoded in the link's `from` / `to` strings, not as
/// a separate field. Cross-dimensional edges in mixed/scalar loops carry
/// element-level `from` like `"pop[nyc]"` so loop-score equations resolve
/// to the per-element link score variable that
/// `try_cross_dimensional_link_scores` emits; a cross-element loop edge
/// that visits a single element of an A2A target carries the element on
/// `to` (`"mp[boston]"`); a loop running through an inlined reducer
/// traverses `from[d] → $⁚ltm⁚agg⁚{n} → to[e]` (the agg name has no
/// subscript). All other edges use variable-level names. See
/// `db::ltm::build_element_level_loops` for the normalization rule.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Link {
    pub from: Ident<Canonical>,
    pub to: Ident<Canonical>,
    pub polarity: LinkPolarity,
}

/// Represents a feedback loop.
///
/// For scalar models, `dimensions` is empty and links reference scalar
/// variable names.  For arrayed models, a pure-dimension A2A loop has
/// `dimensions` set to the shared dimension names (e.g., `["Region"]`)
/// and links reference variable-level names (the A2A expansion handles
/// per-element evaluation).  Mixed loops (scalar + arrayed nodes, or
/// cross-element feedback) have empty `dimensions` and use
/// element-specific link names.
///
/// # `stocks` granularity invariant
///
/// `stocks` is **always element-level** for an arrayed model:
///
/// - **`dimensions.is_empty()` -- scalar models, and mixed/cross-element
///   loops in an arrayed model**: stocks are element-level names (e.g.,
///   `"pop[nyc]"`; plain names like `"pop"` for a genuinely scalar model,
///   which is the degenerate element-level case with no subscript).  A
///   cross-element loop typically traverses the same stock variable at
///   several elements (e.g. both `population[nyc]` AND `population[boston]`),
///   and every one of those element-level nodes belongs in `stocks`.
///
/// - **`!dimensions.is_empty()` -- A2A loops**: stocks are element-level
///   names covering the loop's *entire* dimension element space -- one
///   `"{var}[{elem-tuple}]"` per element, for each variable in the cycle
///   that is a stock (so a 3-element `pop[Region]` A2A stock-flow loop has
///   `["pop[r0]", "pop[r1]", "pop[r2]"]`).  This is what lets
///   `CyclePartitions::partition_for_loop` resolve a partition *per slot*:
///   it groups these stocks by their element-tuple suffix and looks each
///   slot up in `model_element_cycle_partitions`'s element-keyed
///   `stock_partition` map, so two element-wise-uncoupled A2A feedback
///   subsystems over the same dimension get distinct per-slot partitions
///   instead of being pooled (GH #487).
///
/// In both cases the keys must be element-level because
/// `partition_for_loop` looks up stocks in `model_element_cycle_partitions`,
/// whose `stock_partition` map is element-keyed.  Variable-level names here
/// would cause `partition_for_loop` to return `None`, silently corrupting
/// per-loop / per-slot normalization in `ltm_post::compute_rel_loop_scores*`
/// (the loop would bucket into the catch-all `None` group instead of its
/// actual SCC).
///
/// `assign_loop_ids` derives loop IDs from `links` (sorted distinct
/// variable names), not `stocks`, so the element-level `stocks` granularity
/// does not perturb `r{n}`/`b{n}`/`u{n}` numbering.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct Loop {
    pub id: String,
    pub links: Vec<Link>,
    pub stocks: Vec<Ident<Canonical>>,
    pub polarity: LoopPolarity,
    /// Dimension names for A2A loop scores. Empty for scalar or mixed loops.
    pub dimensions: Vec<String>,
    /// Per-slot element-subscripted link cycles for a dimensioned loop whose
    /// underlying element circuits reference *per-element* link-score names
    /// (FixedIndex `from[e]→to`, per-target-element `from→to[e]`) rather than
    /// a single Bare A2A name per edge.
    ///
    /// Each entry is `(element_tuple, links)` where `element_tuple` is the
    /// comma-joined canonical element names of the slot (matching
    /// [`crate::ltm::loop_dimension_element_tuples`]'s tuple format) and
    /// `links` is that slot's element-subscripted link cycle (the same
    /// `Link.from` / `Link.to` subscript conventions cross-element loops use).
    /// The loop-score generator emits an `Equation::Arrayed` whose slot
    /// equations are built from these links; element tuples of the loop's
    /// dimension space with no entry here score a constant 0 (a structurally
    /// absent per-element instance).
    ///
    /// Empty when the loop's links resolve uniformly through Bare A2A
    /// link-score names (the compact `Equation::ApplyToAll` form is used) or
    /// when the loop is scalar (`dimensions` empty). Only dimensioned loops
    /// built from per-element circuits (the enumerator's A2A-collapse on
    /// per-element-equation models, and dimensioned pinned loops -- GH #653)
    /// populate it.
    pub slot_links: Vec<(String, Vec<Link>)>,
}

impl Loop {
    /// Format the loop as a string showing the variable path
    pub fn format_path(&self) -> String {
        if self.links.is_empty() {
            return String::new();
        }

        // Build the path by following links
        let mut path = Vec::new();
        let current = &self.links[0].from;
        path.push(current.as_str());

        for link in &self.links {
            path.push(link.to.as_str());
        }

        path.join(" -> ")
    }
}

/// Loop polarity classification
///
/// The structural polarity is determined by counting negative links:
/// - Even number of negative links → Reinforcing
/// - Odd number of negative links → Balancing
/// - ANY link with unknown polarity → Undetermined
///
/// At runtime, the loop score series is also classified according to the
/// signed-sum confidence ratio `|r - |b|| / (r + |b|)` (Schoenberg and
/// Eberlein, 2020; see `docs/reference/ltm--loops-that-matter.md`):
/// - r is the sum of positive scores, |b| the absolute sum of negative scores
/// - When all valid scores share a sign the ratio is exactly 1 and the loop
///   is classified Reinforcing or Balancing.
/// - When both signs occur but one polarity dominates with confidence at or
///   above [`POLARITY_CONFIDENCE_THRESHOLD`], the loop is classified
///   `MostlyReinforcing` or `MostlyBalancing` ("Rux"/"Bux" in the paper).
/// - Otherwise the loop is `Undetermined`.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum LoopPolarity {
    /// R loop - amplifies changes (positive loop score)
    /// Structurally: even number of negative links
    Reinforcing,
    /// B loop - counteracts changes (negative loop score)
    /// Structurally: odd number of negative links
    Balancing,
    /// Rux loop - "predominantly reinforcing" with mixed-sign loop scores.
    /// Confidence is at or above [`POLARITY_CONFIDENCE_THRESHOLD`] but the
    /// loop has expressed both polarities during simulation.
    MostlyReinforcing,
    /// Bux loop - "predominantly balancing" with mixed-sign loop scores.
    /// Confidence is at or above [`POLARITY_CONFIDENCE_THRESHOLD`] but the
    /// loop has expressed both polarities during simulation.
    MostlyBalancing,
    /// U loop - polarity cannot be determined or changes during simulation
    /// Structurally: any link has unknown polarity
    /// At runtime: loop score has both positive and negative values with
    /// confidence below [`POLARITY_CONFIDENCE_THRESHOLD`].
    Undetermined,
}

/// Threshold at or above which a loop with mixed-sign runtime scores is
/// classified as `MostlyReinforcing`/`MostlyBalancing` rather than
/// `Undetermined`.
///
/// The value 0.99 follows Schoenberg & Eberlein (2020) -- "Seamlessly
/// Integrating LTM into the Modeling Process", section on simplified-CLD
/// polarity confidence -- which uses the same ratio `|r - |b|| / (r + |b|)`
/// and the same 0.99 cutoff to distinguish "predominantly R/B" loops
/// (Rux/Bux) from genuinely mixed (Ux) loops.  See
/// `docs/reference/ltm--loops-that-matter.md` section 13.7 for the formula
/// and `docs/reference/papers/schoenberg2020.2-thesis--summary.md` section
/// 7.6 for the labelling table.
///
/// The cited papers describe the cutoff verbally as "above 0.99" (`>`)
/// while section 13.7 describes gray rendering as "0.99 or lower" (`<=`),
/// which leaves the boundary ambiguous in prose.  This implementation
/// uses `confidence >= POLARITY_CONFIDENCE_THRESHOLD` to match the
/// formula in section 13.7 -- a confidence value of exactly 0.99 still
/// signals strong dominance, so falling on the inclusive side avoids
/// surfacing those loops as Undetermined.
pub const POLARITY_CONFIDENCE_THRESHOLD: f64 = 0.99;

impl LoopPolarity {
    /// Classify loop polarity based on actual runtime loop score values
    /// and return the polarity-confidence ratio in `[0.0, 1.0]`.
    ///
    /// The confidence is `|r - |b|| / (r + |b|)` over the valid (finite,
    /// non-zero) entries, where `r` and `|b|` are the sum of positive and
    /// the absolute sum of negative scores respectively.  An empty valid
    /// set returns `None`; otherwise `r + |b|` is strictly positive and
    /// finite (every valid entry is finite and non-zero), so the ratio is
    /// well-defined and lies in `[0.0, 1.0]`.  Filtering on `!is_finite()`
    /// rather than just `is_nan()` keeps `Inf`/`-Inf` -- which a
    /// numerically unstable simulation could surface -- from poisoning
    /// `denom` and producing `Inf / Inf = NaN` confidence (which `clamp`
    /// would not repair).
    ///
    /// Classification rules (matching the Rux/Bux convention from
    /// Schoenberg & Eberlein, 2020):
    /// - All valid scores share a sign → `Reinforcing` / `Balancing`,
    ///   confidence 1.0.
    /// - Mixed signs and confidence ≥ [`POLARITY_CONFIDENCE_THRESHOLD`]
    ///   → `MostlyReinforcing` / `MostlyBalancing` depending on which
    ///   polarity dominates the magnitude tally.
    /// - Mixed signs and confidence < [`POLARITY_CONFIDENCE_THRESHOLD`]
    ///   → `Undetermined`.
    /// - No valid scores → `None`.
    pub fn from_runtime_scores(scores: &[f64]) -> Option<(Self, f64)> {
        let mut positive_sum = 0.0_f64;
        let mut negative_sum_abs = 0.0_f64;
        let mut has_valid = false;

        for &v in scores {
            // `!is_finite()` rejects NaN, Inf, and -Inf in one check; combined
            // with the zero filter this guarantees `denom` below is finite and
            // strictly positive whenever `has_valid` is true.
            if !v.is_finite() || v == 0.0 {
                continue;
            }
            has_valid = true;
            if v > 0.0 {
                positive_sum += v;
            } else {
                negative_sum_abs += -v;
            }
        }

        if !has_valid {
            return None;
        }

        let denom = positive_sum + negative_sum_abs;
        // `has_valid` guarantees at least one non-zero finite score, so
        // `denom` is strictly positive here.
        let confidence = ((positive_sum - negative_sum_abs).abs() / denom).clamp(0.0, 1.0);

        // `has_valid` guarantees at least one strictly-positive or
        // strictly-negative score, so the (false, false) arm cannot occur
        // here -- it is matched with `unreachable!()` to satisfy the
        // exhaustiveness check while documenting the invariant.
        let polarity = match (positive_sum > 0.0, negative_sum_abs > 0.0) {
            (true, false) => LoopPolarity::Reinforcing,
            (false, true) => LoopPolarity::Balancing,
            (true, true) => {
                // Make every mixed-sign sub-case explicit rather than nesting a
                // `positive_sum > negative_sum_abs` test inside the threshold
                // gate (#506).  The previous nesting made the inner `else`
                // serve two roles at once -- the strictly-dominant balancing
                // case AND the (unreachable) exact-tie case -- so a reader had
                // to reconstruct the threshold arithmetic to see that "balancing
                // wins ties" was never actually in effect.
                if confidence < POLARITY_CONFIDENCE_THRESHOLD {
                    LoopPolarity::Undetermined
                } else if positive_sum > negative_sum_abs {
                    LoopPolarity::MostlyReinforcing
                } else if negative_sum_abs > positive_sum {
                    LoopPolarity::MostlyBalancing
                } else {
                    // confidence >= threshold AND positive_sum ==
                    // negative_sum_abs implies the ratio numerator is 0, hence
                    // confidence == 0 -- which contradicts the threshold check
                    // above for any threshold > 0.  So this arm is unreachable
                    // with the production threshold.  Enforce that invariant at
                    // compile time (the threshold is a const, so the assertion
                    // is const-evaluable): if someone sets the threshold to 0
                    // the build fails here rather than silently letting an exact
                    // magnitude tie classify as Balancing -- a "balancing wins
                    // ties" rule the LTM literature does not justify.  The
                    // `Undetermined` fallback is the safe classification if the
                    // arm ever does become reachable.
                    const {
                        assert!(
                            POLARITY_CONFIDENCE_THRESHOLD > 0.0,
                            "tie-case (positive_sum == negative_sum_abs) with threshold == 0 \
                             would silently classify as Balancing; update the classification \
                             rules if you set the threshold to 0"
                        );
                    }
                    LoopPolarity::Undetermined
                }
            }
            (false, false) => unreachable!(),
        };

        Some((polarity, confidence))
    }

    /// Returns the conventional abbreviation for this polarity.
    ///
    /// Codes follow the LTM literature: "R", "B", "Rux", "Bux", "U".
    /// "Rux" / "Bux" denote unknown-but-predominantly-R/B loops -- the
    /// terminology comes from Schoenberg & Eberlein (2020).
    pub fn abbreviation(&self) -> &'static str {
        match self {
            LoopPolarity::Reinforcing => "R",
            LoopPolarity::Balancing => "B",
            LoopPolarity::MostlyReinforcing => "Rux",
            LoopPolarity::MostlyBalancing => "Bux",
            LoopPolarity::Undetermined => "U",
        }
    }
}

/// Normalize a module·output reference to just the module node.
/// E.g., "$⁚s⁚0⁚smth1·output" becomes "$⁚s⁚0⁚smth1".
/// Non-module references are returned unchanged.
pub(crate) fn normalize_module_ref(ident: &Ident<Canonical>) -> Ident<Canonical> {
    let s = ident.as_str();
    if let Some(pos) = s.find('\u{00B7}') {
        Ident::new(&s[..pos])
    } else {
        ident.clone()
    }
}

/// The reserved synthetic-name prefix: `$` followed by U+205A (TWO DOT
/// PUNCTUATION). Every macro-instantiation internal (`$⁚{var}⁚{n}⁚{func}`)
/// and every LTM-internal node (`$⁚ltm⁚agg⁚{n}`, `$⁚ltm⁚link_score⁚…`, etc.)
/// begins with it. Real model variables never start with `$`, so this is an
/// exact membership test for "is this node a synthetic/macro/module internal
/// rather than a user variable".
pub(crate) const SYNTHETIC_NODE_PREFIX: &str = "$\u{205A}";

/// `true` when `name` is a synthetic/internal node -- a macro-instantiation
/// internal (`$⁚{var}⁚{n}⁚{func}`, optionally `·`-suffixed for a module
/// output path), an LTM-internal aggregate/link-score/loop-score node, or any
/// other name carrying the reserved [`SYNTHETIC_NODE_PREFIX`].
///
/// This is the broad generalization of [`crate::ltm_agg::is_synthetic_agg_name`]
/// (which only matches `$⁚ltm⁚agg⁚{n}`): every node we must hide from the
/// user-facing causal view -- not just LTM aggregate nodes -- carries the `$⁚`
/// prefix. A node name may be a module-output path (`$⁚var⁚0⁚func·output`);
/// the leading-prefix test still classifies it as synthetic.
pub(crate) fn is_synthetic_node_name(name: &str) -> bool {
    name.starts_with(SYNTHETIC_NODE_PREFIX)
}
