// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Ghost (alias) generation: replace connectors that span the diagram with
//! short local links from an alias of the source variable.
//!
//! The remaining crossings on large generated layouts are STRUCTURAL: a
//! widely-used parameter sits in one place and its connectors cross everything
//! between it and its far-flung consumers. No local move can fix that --
//! ghosting the parameter next to each distant consumer cluster can. This
//! mirrors hand-drawn practice exactly: in the multi-view Vensim corpus,
//! 14-26% of variables are ghosts, every one of them a pure input (in-degree
//! 0), and each ghost copy serves 1-2 consumers.

use std::collections::{HashMap, HashSet};

use crate::datamodel::ViewElement;
use crate::datamodel::view_element::{self, LabelSide};

use super::config::LayoutConfig;
use super::graph::Position;
use super::metadata::ComputedMetadata;
use super::{LayoutState, format_label_with_line_breaks};

/// A connector longer than this many `horizontal_spacing`s is "long": the
/// distance at which a ghost copy reads better than a cross-diagram line.
const GHOST_DISTANCE_FACTOR: f64 = 2.5;

/// At most this fraction of the model's variables become ghosts (the
/// hand-drawn corpus shows 14-26%; the cap keeps a pathological model from
/// drowning in copies).
pub(super) const GHOST_BUDGET_FRACTION: f64 = 0.25;

/// Consumers within this distance of each other share one ghost copy.
const GHOST_CLUSTER_RADIUS: f64 = 200.0;

/// How far from its consumer (cluster centroid) a ghost is placed. Far enough
/// that the ghost's own label has room, near enough to read as "local".
const GHOST_OFFSET: f64 = 70.0;

/// The edges re-routed through ghosts: maps `(source_ident, consumer_ident)`
/// to the ghost element's uid. `build_connectors` draws those connectors from
/// the ghost instead of the original source.
pub(super) type ReroutedEdges = HashMap<(String, String), i32>;

/// Generate ghost (alias) elements for distant consumers of pure-input
/// variables, mutating `state` (new `ViewElement::Alias` elements + positions)
/// and returning the re-routed edge map for `build_connectors`.
///
/// Only PURE INPUTS (variables with no dependencies of their own) are ghosted:
/// a mid-graph variable is part of causal chains -- and possibly feedback
/// loops -- that ghosting would visually sever. Pure inputs are exactly what
/// human modelers ghost (every ghost in the observed corpus is one).
///
/// Each qualifying source keeps its NEAREST consumer connector (the original
/// element stays meaningfully placed); consumers beyond the distance threshold
/// are clustered greedily (nearest-first, max 2 per cluster within
/// `GHOST_CLUSTER_RADIUS`) and each cluster gets one ghost. Generation stops at
/// the budget. Deterministic: iteration follows the BTreeMap dep-graph order
/// and positions, never HashMap order.
pub(super) fn generate_ghosts(
    state: &mut LayoutState,
    config: &LayoutConfig,
    metadata: &ComputedMetadata,
) -> ReroutedEdges {
    let threshold = GHOST_DISTANCE_FACTOR * config.horizontal_spacing;

    let pos_of = |state: &LayoutState, ident: &str| -> Option<Position> {
        let uid = state.uid_manager.get_uid(ident)?;
        state.positions.get(&uid).copied()
    };

    // Pure inputs: variables that appear as a dependency of something but have
    // no dependencies of their own. reverse_dep_graph is a BTreeMap, so source
    // iteration order is deterministic.
    let is_pure_input = |ident: &str| -> bool {
        metadata
            .dep_graph
            .get(ident)
            .is_none_or(|deps| deps.is_empty())
    };

    // The variable count for the budget: every ident that participates in the
    // dependency graph (either side).
    let var_count = metadata
        .dep_graph
        .keys()
        .chain(metadata.reverse_dep_graph.keys())
        .collect::<HashSet<_>>()
        .len();
    let budget = ((var_count as f64) * GHOST_BUDGET_FRACTION).ceil() as usize;

    let mut rerouted: ReroutedEdges = HashMap::new();
    let mut ghost_count = 0usize;

    for (source, consumers) in metadata.reverse_dep_graph.iter() {
        if ghost_count >= budget {
            break;
        }
        if !is_pure_input(source) {
            continue;
        }
        let Some(source_pos) = pos_of(state, source) else {
            continue;
        };

        // The source's consumers, with positions and distances, deterministic
        // (BTreeSet iteration), nearest first.
        let mut placed: Vec<(&str, Position, f64)> = consumers
            .iter()
            .filter_map(|c| {
                let p = pos_of(state, c)?;
                let d = ((p.x - source_pos.x).powi(2) + (p.y - source_pos.y).powi(2)).sqrt();
                Some((c.as_str(), p, d))
            })
            .collect();
        placed.sort_by(|a, b| a.2.total_cmp(&b.2).then(a.0.cmp(b.0)));

        // The nearest consumer keeps the original connector; only FAR consumers
        // beyond it are candidates for ghosting.
        let far: Vec<(&str, Position, f64)> = placed
            .iter()
            .skip(1)
            .filter(|(_, _, d)| *d > threshold)
            .cloned()
            .collect();
        if far.is_empty() {
            continue;
        }

        // Greedy clustering: walk far consumers nearest-source-first; each
        // unclaimed consumer starts a cluster and claims at most one other
        // consumer within GHOST_CLUSTER_RADIUS of it.
        let mut claimed: Vec<bool> = vec![false; far.len()];
        for i in 0..far.len() {
            if claimed[i] || ghost_count >= budget {
                continue;
            }
            claimed[i] = true;
            let mut members = vec![i];
            for (j, claimed_j) in claimed.iter_mut().enumerate().skip(i + 1) {
                if *claimed_j || members.len() >= 2 {
                    continue;
                }
                let d =
                    ((far[i].1.x - far[j].1.x).powi(2) + (far[i].1.y - far[j].1.y).powi(2)).sqrt();
                if d <= GHOST_CLUSTER_RADIUS {
                    *claimed_j = true;
                    members.push(j);
                }
            }

            // Place the ghost beside the cluster: at the member centroid,
            // offset toward the original source (so the connector reads as
            // "coming from that direction") but only GHOST_OFFSET away.
            let cx = members.iter().map(|&m| far[m].1.x).sum::<f64>() / members.len() as f64;
            let cy = members.iter().map(|&m| far[m].1.y).sum::<f64>() / members.len() as f64;
            let dx = source_pos.x - cx;
            let dy = source_pos.y - cy;
            let len = (dx * dx + dy * dy).sqrt().max(1e-9);
            let ghost_pos =
                Position::new(cx + dx / len * GHOST_OFFSET, cy + dy / len * GHOST_OFFSET);

            // The ghost element. Its alias_of_uid is the source's uid; its
            // label renders the source's name (resolved by the renderer and
            // the metric through alias_of_uid).
            let Some(source_uid) = state.uid_manager.get_uid(source) else {
                continue;
            };
            let ghost_uid = state.uid_manager.alloc("");
            state.elements.push(ViewElement::Alias(view_element::Alias {
                uid: ghost_uid,
                alias_of_uid: source_uid,
                x: ghost_pos.x,
                y: ghost_pos.y,
                label_side: LabelSide::Bottom,
                compat: None,
            }));
            state.positions.insert(ghost_uid, ghost_pos);
            ghost_count += 1;

            for &m in &members {
                rerouted.insert((source.to_string(), far[m].0.to_string()), ghost_uid);
            }
        }
    }

    // The display-name helper is unused here today but the import documents the
    // relationship: a ghost renders its SOURCE's formatted name, which is why
    // no name is stored on the Alias element itself.
    let _ = format_label_with_line_breaks;

    rerouted
}
