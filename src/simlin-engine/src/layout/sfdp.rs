// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::BTreeMap;

use rand::Rng;
use rand::SeedableRng;
use rand::rngs::StdRng;

use super::graph::{ConstrainedGraph, Layout, NodeId, Position};

/// Configuration for the SFDP (Scalable Force-Directed Placement) algorithm.
#[derive(Clone, Debug)]
pub struct SfdpConfig {
    /// Ideal edge length. When negative, auto-calculated from the average
    /// edge length in the initial layout.
    pub k: f64,
    /// Maximum number of force-directed iterations.
    pub max_iterations: usize,
    /// The algorithm stops when the adaptive step size drops below
    /// `convergence_threshold / k`.
    pub convergence_threshold: f64,
    /// Starting step size for node displacement per iteration.
    pub initial_step_size: f64,
    /// Multiplicative cooling applied to the step size when total energy
    /// increases between iterations.
    pub cooling_factor: f64,
    /// Repulsive exponent controlling how repulsion scales with distance.
    pub p: f64,
    /// Attractive constant scaling the spring forces along edges.
    pub c: f64,
    /// Whether to rearrange degree-1 nodes evenly around their parent after
    /// the main loop finishes.
    pub beautify_leaves: bool,
}

impl Default for SfdpConfig {
    fn default() -> Self {
        Self {
            k: 1.0,
            max_iterations: 600,
            convergence_threshold: 1e-4,
            initial_step_size: 1.0,
            cooling_factor: 0.9,
            p: -1.0,
            c: 0.2,
            beautify_leaves: false,
        }
    }
}

/// SFDP layout engine. Positions graph nodes by iterating repulsive forces
/// between all node pairs and attractive spring forces along edges, with
/// support for rigid group constraints and pinned nodes.
struct Sfdp<'a, N: NodeId> {
    graph: &'a ConstrainedGraph<N>,
    config: &'a SfdpConfig,
    rng: StdRng,
}

impl<'a, N: NodeId> Sfdp<'a, N> {
    fn new(graph: &'a ConstrainedGraph<N>, config: &'a SfdpConfig, seed: u64) -> Self {
        Self {
            graph,
            config,
            rng: StdRng::seed_from_u64(seed),
        }
    }

    fn random_layout(&mut self) -> Layout<N> {
        let mut layout = BTreeMap::new();
        for node in self.graph.nodes() {
            layout.insert(
                node.clone(),
                Position::new(self.rng.random::<f64>(), self.rng.random::<f64>()),
            );
        }
        layout
    }

    fn calculate_average_edge_length(&self, layout: &Layout<N>) -> f64 {
        let edges = self.graph.edges();
        if edges.is_empty() {
            return 1.0;
        }

        let mut total_dist = 0.0;
        let mut count = 0usize;

        for edge in edges {
            if let (Some(p1), Some(p2)) = (layout.get(&edge.from), layout.get(&edge.to)) {
                let dx = p1.x - p2.x;
                let dy = p1.y - p2.y;
                total_dist += (dx * dx + dy * dy).sqrt();
                count += 1;
            }
        }

        if count == 0 {
            1.0
        } else {
            total_dist / count as f64
        }
    }

    /// Compute the offsets of each rigid group member relative to the first
    /// member (the "base"). These offsets are restored after every iteration
    /// to prevent floating-point drift from deforming the group.
    fn compute_rigid_group_offsets(&self, layout: &Layout<N>) -> Vec<BTreeMap<N, Position>> {
        self.graph
            .rigid_groups()
            .iter()
            .map(|group| {
                let mut offsets = BTreeMap::new();
                if group.members.is_empty() {
                    return offsets;
                }
                let base = &group.members[0];
                let base_pos = layout.get(base).copied().unwrap_or_default();
                for member in &group.members {
                    let pos = layout.get(member).copied().unwrap_or_default();
                    offsets.insert(
                        member.clone(),
                        Position::new(pos.x - base_pos.x, pos.y - base_pos.y),
                    );
                }
                offsets
            })
            .collect()
    }

    /// Main force-directed loop with rigid group constraints.
    fn force_directed_with_rigid(&self, layout: &mut Layout<N>, k: f64) {
        let config = self.config;
        let kp = k.powf(1.0 - config.p);
        let crk = config.c.powf((2.0 - config.p) / 3.0) / k;

        let mut step = config.initial_step_size;
        let mut prev_norm = f64::MAX;

        let nodes: Vec<N> = self.graph.nodes().cloned().collect();

        let group_offsets = self.compute_rigid_group_offsets(layout);

        for _iteration in 0..config.max_iterations {
            let mut forces: BTreeMap<N, Position> = BTreeMap::new();

            // O(n^2) repulsive forces between all node pairs
            for i in 0..nodes.len() {
                let n1 = &nodes[i];
                let pos1 = match layout.get(n1) {
                    Some(p) => *p,
                    None => continue,
                };
                for n2 in &nodes[(i + 1)..] {
                    let pos2 = match layout.get(n2) {
                        Some(p) => *p,
                        None => continue,
                    };

                    let dx = pos1.x - pos2.x;
                    let dy = pos1.y - pos2.y;
                    let dist = (dx * dx + dy * dy).sqrt().max(1e-9);

                    let f = kp / dist.powf(1.0 - config.p);
                    let fx = f * dx / dist;
                    let fy = f * dy / dist;

                    let f1 = forces.entry(n1.clone()).or_default();
                    f1.x += fx;
                    f1.y += fy;

                    let f2 = forces.entry(n2.clone()).or_default();
                    f2.x -= fx;
                    f2.y -= fy;
                }
            }

            // O(edges) attractive forces along edges
            for edge in self.graph.edges() {
                let pos1 = match layout.get(&edge.from) {
                    Some(p) => *p,
                    None => continue,
                };
                let pos2 = match layout.get(&edge.to) {
                    Some(p) => *p,
                    None => continue,
                };

                let dx = pos1.x - pos2.x;
                let dy = pos1.y - pos2.y;
                let dist = (dx * dx + dy * dy).sqrt();
                if dist < 1e-9 {
                    continue;
                }

                let f = crk * dist * edge.weight;
                let fx = f * dx / dist;
                let fy = f * dy / dist;

                let f1 = forces.entry(edge.from.clone()).or_default();
                f1.x -= fx;
                f1.y -= fy;

                let f2 = forces.entry(edge.to.clone()).or_default();
                f2.x += fx;
                f2.y += fy;
            }

            let norm =
                self.apply_forces_with_rigid_constraints(layout, &forces, step, &group_offsets);

            // Adaptive cooling: increase step when energy drops significantly,
            // cool when energy rises, hold steady for small improvements.
            if norm >= prev_norm {
                step *= config.cooling_factor;
            } else if norm <= 0.95 * prev_norm {
                step *= 0.99 / config.cooling_factor;
            }
            prev_norm = norm;

            if step < config.convergence_threshold / k {
                break;
            }
        }
    }

    /// Apply computed forces to the layout, respecting pinned nodes and rigid
    /// group constraints. Returns the total force magnitude (used for adaptive
    /// step sizing and convergence checking).
    fn apply_forces_with_rigid_constraints(
        &self,
        layout: &mut Layout<N>,
        forces: &BTreeMap<N, Position>,
        step: f64,
        group_offsets: &[BTreeMap<N, Position>],
    ) -> f64 {
        let mut group_forces: BTreeMap<usize, Position> = BTreeMap::new();
        let mut norm = 0.0;

        for (node, force) in forces {
            if let Some(group_idx) = self.graph.is_in_rigid_group(node) {
                let gf = group_forces.entry(group_idx).or_default();
                gf.x += force.x;
                gf.y += force.y;
            } else {
                if self.graph.is_pinned(node) {
                    continue;
                }

                let mag = (force.x * force.x + force.y * force.y).sqrt();
                norm += mag;

                if mag > 0.0
                    && let Some(pos) = layout.get_mut(node)
                {
                    pos.x += step * force.x / mag;
                    pos.y += step * force.y / mag;
                }
            }
        }

        // Move rigid groups as units (translation only)
        let rigid_groups = self.graph.rigid_groups();
        for (group_idx, total_force) in &group_forces {
            let group = &rigid_groups[*group_idx];

            let group_pinned = group
                .members
                .first()
                .is_some_and(|m| self.graph.is_pinned(m));
            if group_pinned {
                continue;
            }

            let mag = (total_force.x * total_force.x + total_force.y * total_force.y).sqrt();
            norm += mag;

            if mag > 0.0 {
                let dx = step * total_force.x / mag;
                let dy = step * total_force.y / mag;

                for member in &group.members {
                    if let Some(pos) = layout.get_mut(member) {
                        pos.x += dx;
                        pos.y += dy;
                    }
                }
            }
        }

        // Restore rigid group offsets relative to the base member to prevent
        // floating-point drift from accumulating across iterations.
        for (group_idx, group) in rigid_groups.iter().enumerate() {
            if group.members.len() < 2 {
                continue;
            }
            let base = &group.members[0];
            let base_pos = match layout.get(base) {
                Some(p) => *p,
                None => continue,
            };
            if let Some(offsets) = group_offsets.get(group_idx) {
                for member in &group.members[1..] {
                    if let Some(offset) = offsets.get(member)
                        && let Some(pos) = layout.get_mut(member)
                    {
                        pos.x = base_pos.x + offset.x;
                        pos.y = base_pos.y + offset.y;
                    }
                }
            }
        }

        norm
    }

    /// Post-processing pass that rearranges degree-1 (leaf) nodes evenly
    /// around their single neighbor at distance `k`.
    fn beautify_leaves(&self, layout: &mut Layout<N>, k: f64) {
        let mut parents: BTreeMap<N, N> = BTreeMap::new();
        for node in self.graph.nodes() {
            if self.graph.degree(node) == 1
                && !self.graph.is_pinned(node)
                && self.graph.is_in_rigid_group(node).is_none()
                && let Some(mut neighbors) = self.graph.neighbors(node)
                && let Some((neighbor, _weight)) = neighbors.next()
            {
                parents.insert(node.clone(), neighbor.clone());
            }
        }

        // Group leaves by parent
        let mut children_of: BTreeMap<N, Vec<N>> = BTreeMap::new();
        for (leaf, parent) in &parents {
            children_of
                .entry(parent.clone())
                .or_default()
                .push(leaf.clone());
        }

        for (parent, leaves) in &children_of {
            if leaves.len() <= 1 {
                continue;
            }

            let parent_pos = match layout.get(parent) {
                Some(p) => *p,
                None => continue,
            };

            let angle_step = 2.0 * std::f64::consts::PI / leaves.len() as f64;

            for (i, leaf) in leaves.iter().enumerate() {
                let angle = i as f64 * angle_step;
                if let Some(pos) = layout.get_mut(leaf) {
                    pos.x = parent_pos.x + k * angle.cos();
                    pos.y = parent_pos.y + k * angle.sin();
                }
            }
        }
    }
}

/// Compute a force-directed layout for the given constrained graph, starting
/// from random initial positions seeded by `seed`.
pub fn compute_layout<N: NodeId>(
    graph: &ConstrainedGraph<N>,
    config: &SfdpConfig,
    seed: u64,
) -> Layout<N> {
    if graph.node_count() == 0 {
        return BTreeMap::new();
    }

    let mut sfdp = Sfdp::new(graph, config, seed);
    let mut layout = sfdp.random_layout();

    let k = if config.k < 0.0 {
        sfdp.calculate_average_edge_length(&layout)
    } else {
        config.k
    };

    sfdp.force_directed_with_rigid(&mut layout, k);

    if config.beautify_leaves {
        sfdp.beautify_leaves(&mut layout, k);
    }

    layout
}

/// Compute a force-directed layout starting from the given initial positions.
/// Nodes present in the graph but missing from `initial` receive random
/// positions.
pub fn compute_layout_from_initial<N: NodeId>(
    graph: &ConstrainedGraph<N>,
    config: &SfdpConfig,
    initial: &Layout<N>,
    seed: u64,
) -> Layout<N> {
    if graph.node_count() == 0 {
        return BTreeMap::new();
    }

    let mut sfdp = Sfdp::new(graph, config, seed);

    let mut layout = initial.clone();
    for node in graph.nodes() {
        layout
            .entry(node.clone())
            .or_insert_with(|| Position::new(sfdp.rng.random::<f64>(), sfdp.rng.random::<f64>()));
    }

    let k = if config.k < 0.0 {
        sfdp.calculate_average_edge_length(&layout)
    } else {
        config.k
    };

    sfdp.force_directed_with_rigid(&mut layout, k);

    if config.beautify_leaves {
        sfdp.beautify_leaves(&mut layout, k);
    }

    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::graph::{ConstrainedGraphBuilder, GraphBuilder};

    fn triangle_graph() -> ConstrainedGraph<String> {
        let mut gb = GraphBuilder::new_undirected();
        gb.add_edge("a".into(), "b".into(), 1.0);
        gb.add_edge("b".into(), "c".into(), 1.0);
        gb.add_edge("c".into(), "a".into(), 1.0);
        ConstrainedGraphBuilder::new(gb.build()).build()
    }

    #[test]
    fn test_sfdp_convergence() {
        let cg = triangle_graph();
        let config = SfdpConfig::default();
        let layout = compute_layout(&cg, &config, 42);

        assert_eq!(layout.len(), 3);

        let positions: Vec<Position> = layout.values().copied().collect();
        let all_same = positions
            .windows(2)
            .all(|w| (w[0].x - w[1].x).abs() < 1e-12 && (w[0].y - w[1].y).abs() < 1e-12);
        assert!(
            !all_same,
            "nodes should not all occupy the same position after layout"
        );

        for pos in &positions {
            assert!(pos.x.is_finite());
            assert!(pos.y.is_finite());
        }
    }

    #[test]
    fn test_sfdp_rigid_group_coherence() {
        let mut gb = GraphBuilder::new_undirected();
        gb.add_edge("a".into(), "b".into(), 1.0);
        gb.add_edge("b".into(), "c".into(), 1.0);
        gb.add_edge("c".into(), "d".into(), 1.0);

        let mut cb = ConstrainedGraphBuilder::new(gb.build());
        cb.add_rigid_group(vec!["a".into(), "b".into()]);
        let cg = cb.build();

        let mut initial = BTreeMap::new();
        initial.insert("a".to_string(), Position::new(0.0, 0.0));
        initial.insert("b".to_string(), Position::new(1.0, 0.0));
        initial.insert("c".to_string(), Position::new(2.0, 0.0));
        initial.insert("d".to_string(), Position::new(3.0, 0.0));

        let config = SfdpConfig::default();
        let layout = compute_layout_from_initial(&cg, &config, &initial, 42);

        let pos_a = layout.get("a").unwrap();
        let pos_b = layout.get("b").unwrap();

        let dx = pos_b.x - pos_a.x;
        let dy = pos_b.y - pos_a.y;
        assert!(
            (dx - 1.0).abs() < 1e-9,
            "rigid group x-offset not preserved: expected 1.0, got {dx}"
        );
        assert!(
            dy.abs() < 1e-9,
            "rigid group y-offset not preserved: expected 0.0, got {dy}"
        );
    }

    #[test]
    fn test_sfdp_deterministic() {
        let cg = triangle_graph();
        let config = SfdpConfig::default();

        let layout1 = compute_layout(&cg, &config, 123);
        let layout2 = compute_layout(&cg, &config, 123);

        assert_eq!(layout1.len(), layout2.len());
        for (node, pos1) in &layout1 {
            let pos2 = layout2.get(node).expect("node missing in second layout");
            assert!(
                (pos1.x - pos2.x).abs() < 1e-15,
                "x mismatch for node {node}"
            );
            assert!(
                (pos1.y - pos2.y).abs() < 1e-15,
                "y mismatch for node {node}"
            );
        }
    }

    #[test]
    fn test_sfdp_pinned_nodes() {
        let mut gb = GraphBuilder::new_undirected();
        gb.add_edge("a".into(), "b".into(), 1.0);
        gb.add_edge("b".into(), "c".into(), 1.0);

        let mut cb = ConstrainedGraphBuilder::new(gb.build());
        cb.pin(&["a".into()]);
        let cg = cb.build();

        let pinned_pos = Position::new(5.0, 5.0);
        let mut initial = BTreeMap::new();
        initial.insert("a".to_string(), pinned_pos);
        initial.insert("b".to_string(), Position::new(6.0, 5.0));
        initial.insert("c".to_string(), Position::new(7.0, 5.0));

        let config = SfdpConfig::default();
        let layout = compute_layout_from_initial(&cg, &config, &initial, 42);

        let pos_a = layout.get("a").unwrap();
        assert!(
            (pos_a.x - pinned_pos.x).abs() < 1e-15,
            "pinned node x changed"
        );
        assert!(
            (pos_a.y - pinned_pos.y).abs() < 1e-15,
            "pinned node y changed"
        );
    }

    #[test]
    fn test_sfdp_empty_graph() {
        let gb = GraphBuilder::<String>::new_undirected();
        let cg = ConstrainedGraphBuilder::new(gb.build()).build();

        let config = SfdpConfig::default();
        let layout = compute_layout(&cg, &config, 0);
        assert!(layout.is_empty());
    }

    #[test]
    fn test_beautify_leaves_skips_pinned() {
        // Star graph: hub with 3 leaves, one leaf is pinned
        let mut gb = GraphBuilder::new_undirected();
        gb.add_edge("hub".into(), "leaf1".into(), 1.0);
        gb.add_edge("hub".into(), "leaf2".into(), 1.0);
        gb.add_edge("hub".into(), "pinned_leaf".into(), 1.0);

        let mut cb = ConstrainedGraphBuilder::new(gb.build());
        cb.pin(&["pinned_leaf".into()]);
        let cg = cb.build();

        let pinned_pos = Position::new(99.0, 99.0);
        let mut initial = BTreeMap::new();
        initial.insert("hub".to_string(), Position::new(0.0, 0.0));
        initial.insert("leaf1".to_string(), Position::new(1.0, 0.0));
        initial.insert("leaf2".to_string(), Position::new(0.0, 1.0));
        initial.insert("pinned_leaf".to_string(), pinned_pos);

        let config = SfdpConfig {
            beautify_leaves: true,
            ..SfdpConfig::default()
        };
        let layout = compute_layout_from_initial(&cg, &config, &initial, 42);

        let pos = layout.get("pinned_leaf").unwrap();
        assert!(
            (pos.x - pinned_pos.x).abs() < 1e-15,
            "pinned leaf x should not change: expected {}, got {}",
            pinned_pos.x,
            pos.x
        );
        assert!(
            (pos.y - pinned_pos.y).abs() < 1e-15,
            "pinned leaf y should not change: expected {}, got {}",
            pinned_pos.y,
            pos.y
        );
    }
}
