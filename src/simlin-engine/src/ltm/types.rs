// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Public LTM data types: link/loop polarity, link, loop, module role,
//! and the truncation marker. These are the user-visible vocabulary the
//! rest of the LTM submodules build on.

use crate::common::{Canonical, Ident};
use crate::model::ModelStage1;
use crate::variable::Variable;

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

/// Represents a causal link between two variables.
///
/// The per-reference access shape distinction (Bare / FixedIndex / Wildcard
/// / DynamicIndex) is encoded in the link's `from` / `to` strings, not as
/// a separate field. Cross-dimensional edges in mixed/scalar loops carry
/// element-level `from` like `"pop[nyc]"` so loop-score equations resolve
/// to the per-element link score variable that
/// `try_cross_dimensional_link_scores` emits. All other edges use
/// variable-level names. See `db_ltm::build_element_level_loops` for the
/// normalization rule.
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
/// The granularity of `stocks` is keyed off `dimensions`:
///
/// - **`dimensions.is_empty()` -- mixed/scalar AND cross-element
///   approximation loops**: stocks are **element-level** names (e.g.,
///   `"pop[nyc]"`). This is required because `partition_for_loop` looks
///   up stocks in `model_element_cycle_partitions`, whose
///   `stock_partition` map is keyed on element-level names.  Using
///   variable-level names here would cause `partition_for_loop` to
///   return `None`, silently corrupting per-loop normalization in
///   `compute_rel_loop_scores` (the loop would bucket into the
///   catch-all `None` group instead of its actual SCC).  Cross-element
///   approximation loops (built by the wildcard-reducer branch in
///   `db_ltm::build_element_level_loops`) follow the same rule and
///   include every element-level stock node that appears in the
///   original circuit -- a single cross-element loop typically
///   traverses the same stock variable at multiple elements (e.g.,
///   both `population[nyc]` AND `population[boston]`), and all belong
///   in `stocks` so the partition lookup hits the SCC containing them.
///
/// - **`!dimensions.is_empty()` -- A2A loops**: stocks are
///   **variable-level** names (e.g., `"pop"`).  A2A loops are expanded
///   per-element during simulation so variable-level names are correct;
///   the element-level partition lookup is not used for A2A loops, and
///   `partition_for_loop` legitimately returns `None` for them.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct Loop {
    pub id: String,
    pub links: Vec<Link>,
    pub stocks: Vec<Ident<Canonical>>,
    pub polarity: LoopPolarity,
    /// Dimension names for A2A loop scores. Empty for scalar or mixed loops.
    pub dimensions: Vec<String>,
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
/// At runtime, if the loop score changes sign during simulation, the polarity
/// is also classified as Undetermined.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum LoopPolarity {
    /// R loop - amplifies changes (positive loop score)
    /// Structurally: even number of negative links
    Reinforcing,
    /// B loop - counteracts changes (negative loop score)
    /// Structurally: odd number of negative links
    Balancing,
    /// U loop - polarity cannot be determined or changes during simulation
    /// Structurally: any link has unknown polarity
    /// At runtime: loop score has both positive and negative values
    Undetermined,
}

impl LoopPolarity {
    /// Classify loop polarity based on actual runtime loop score values.
    ///
    /// This function examines the loop score values from a simulation run
    /// and determines the appropriate polarity:
    /// - All valid (non-NaN, non-zero) scores positive → Reinforcing
    /// - All valid scores negative → Balancing
    /// - Mix of positive and negative → Undetermined
    /// - No valid scores → returns None
    pub fn from_runtime_scores(scores: &[f64]) -> Option<Self> {
        let valid_scores: Vec<f64> = scores
            .iter()
            .copied()
            .filter(|v| !v.is_nan() && *v != 0.0)
            .collect();

        if valid_scores.is_empty() {
            return None;
        }

        let has_positive = valid_scores.iter().any(|v| *v > 0.0);
        let has_negative = valid_scores.iter().any(|v| *v < 0.0);

        match (has_positive, has_negative) {
            (true, false) => Some(LoopPolarity::Reinforcing),
            (false, true) => Some(LoopPolarity::Balancing),
            (true, true) => Some(LoopPolarity::Undetermined),
            (false, false) => None, // All zeros after filtering
        }
    }

    /// Returns the conventional single-letter abbreviation for this polarity
    pub fn abbreviation(&self) -> &'static str {
        match self {
            LoopPolarity::Reinforcing => "R",
            LoopPolarity::Balancing => "B",
            LoopPolarity::Undetermined => "U",
        }
    }
}

/// Classification of a module's role in LTM analysis.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModuleLtmRole {
    /// Has internal stocks (SMOOTH, DELAY, TREND, user-defined modules with stocks)
    DynamicModule,
    /// No internal stocks -- pure passthrough
    Passthrough,
}

/// Classify a module model for LTM analysis.
///
/// Dynamic modules contain stocks and need composite link scores.
/// Stateless modules are passthroughs.
pub(crate) fn classify_module_for_ltm(module_model: &ModelStage1) -> ModuleLtmRole {
    if module_model
        .variables
        .values()
        .any(|v| matches!(v, Variable::Stock { .. }))
    {
        ModuleLtmRole::DynamicModule
    } else {
        ModuleLtmRole::Passthrough
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
