// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeMap, HashMap};

use rand::Rng;
use rand::SeedableRng;
use rand::rngs::StdRng;

use super::graph::{ConstrainedGraph, Layout, NodeId, Position};

/// Callback invoked after each SFDP iteration. Returns `Some(layout)` to
/// replace the current layout (e.g. after annealing), or `None` to continue.
type IterationCallback<'a, N> = &'a mut dyn FnMut(usize, &Layout<N>) -> Option<Layout<N>>;

/// Configuration for the SFDP (Scalable Force-Directed Placement) algorithm.
#[derive(Clone, Debug)]
pub struct SfdpConfig {
    /// Ideal edge length. When negative, auto-calculated from the average
    /// edge length in the initial layout.
    pub k: f64,
    /// Maximum number of force-directed iterations. A safety bound only: the
    /// loop normally exits earlier, when the adaptive step decays below the
    /// convergence threshold.
    pub max_iterations: usize,
    /// The algorithm CONVERGES when the adaptive step size drops below
    /// `convergence_threshold * k`: at that point every node moves less than
    /// this fraction of an ideal edge length per iteration, i.e. the layout is
    /// quiescent.
    pub convergence_threshold: f64,
    /// Starting step size as a fraction of the ideal edge length `k`. A node
    /// can therefore cross a neighborhood in a handful of early iterations.
    pub initial_step_size: f64,
    /// Hu's adaptive cooling factor `t`, in (0, 1): the step shrinks by `t`
    /// whenever total energy rises (we overshot) and grows by `1/t` after
    /// `STEP_PROGRESS_THRESHOLD` consecutive energy decreases (we are
    /// descending steadily -- take bigger steps).
    pub cooling_factor: f64,
    /// Repulsive exponent controlling how repulsion scales with distance.
    pub p: f64,
    /// Attractive constant scaling the spring forces along edges.
    pub c: f64,
    /// Whether to rearrange degree-1 nodes evenly around their parent after
    /// the main loop finishes.
    pub beautify_leaves: bool,
}

/// Number of consecutive energy decreases before the adaptive step grows
/// (Hu 2005, "Efficient, High-Quality Force-Directed Graph Drawing", section
/// 2.2). Growing only after sustained progress keeps the step from
/// oscillating.
const STEP_PROGRESS_THRESHOLD: usize = 5;

/// Strength of the centroid gravity applied when -- and only when -- the graph
/// has multiple connected components, as a fraction of the spring (attraction)
/// constant. Mutually-disconnected components feel only repulsion from each
/// other and would drift apart without bound under a converging step scheme;
/// this weak pull gives them a stable equilibrium a few ideal edge lengths
/// apart. A connected graph gets NO gravity, so the common case is undistorted.
const DISCONNECTED_GRAVITY: f64 = 0.5;

/// One step of Hu's adaptive cooling schedule: the next `(step, progress)`
/// given this iteration's total force magnitude ("energy") and the previous
/// one. `t` is the cooling factor in (0, 1). PURE.
fn adapt_step(step: f64, energy: f64, prev_energy: f64, progress: usize, t: f64) -> (f64, usize) {
    if energy < prev_energy {
        // Descending. After enough consecutive progress, take bigger steps.
        let progress = progress + 1;
        if progress >= STEP_PROGRESS_THRESHOLD {
            (step / t, 0)
        } else {
            (step, progress)
        }
    } else {
        // Overshot (energy rose): cool down and start counting afresh.
        (step * t, 0)
    }
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

impl SfdpConfig {
    /// The configuration production uses to place auxiliaries around the rigid
    /// stock-flow chains (`run_sfdp_with_rigid_chains` and the incremental
    /// `settle_new_elements`). Defined here, next to the algorithm, so the
    /// convergence behavior of the production configuration is directly
    /// testable.
    ///
    /// `initial_step_size`/`convergence_threshold` are fractions of `k`, and
    /// `cooling_factor` is Hu's `t` (see the field docs). The previous values
    /// (absolute step 0.1, cooling 0.9995) could not converge: reaching the
    /// threshold needed ~18k cooling steps with only 5000 iterations available,
    /// so every layout was cut off mid-flight rather than settled.
    pub fn for_aux_placement() -> Self {
        Self {
            // The ideal edge length must leave room for LABELS, not just node
            // shapes: a converged layout puts connected nodes ~k apart, and a
            // typical two-line variable label is 100-150 units wide. At the
            // pre-quiescence k of 75 the equilibrium piled labels on top of
            // each other (the non-converging schedule masked this by cutting
            // layouts off mid-expansion).
            k: 150.0,
            max_iterations: 5000,
            convergence_threshold: 0.001,
            initial_step_size: 1.0,
            cooling_factor: 0.9,
            c: 3.0,
            ..Self::default()
        }
    }

    /// The configuration production uses to position whole stock-flow chains
    /// relative to each other (`chain::compute_chain_positions`): larger ideal
    /// edge length and weaker attraction than aux placement.
    pub fn for_chain_positioning() -> Self {
        Self {
            k: 150.0,
            max_iterations: 5000,
            convergence_threshold: 0.001,
            initial_step_size: 1.0,
            cooling_factor: 0.9,
            c: 0.5,
            ..Self::default()
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

/// Map each node to its index in `nodes`, so the per-iteration force loop can
/// accumulate into a flat `Vec` instead of a String-keyed `BTreeMap`.
fn build_node_index<N: NodeId>(nodes: &[N]) -> HashMap<N, usize> {
    nodes
        .iter()
        .cloned()
        .enumerate()
        .map(|(i, node)| (node, i))
        .collect()
}

/// Resolve each graph edge to a `(from_idx, to_idx, weight)` triple once, so the
/// hot force loop never re-hashes or re-compares String node ids. Edges whose
/// endpoints are absent from `node_index` are dropped (they cannot contribute a
/// force), matching the prior `layout.get(..) => None => continue` behavior.
fn build_edge_indices<N: NodeId>(
    node_index: &HashMap<N, usize>,
    graph: &ConstrainedGraph<N>,
) -> Vec<(usize, usize, f64)> {
    graph
        .edges()
        .iter()
        .filter_map(|edge| {
            Some((
                *node_index.get(&edge.from)?,
                *node_index.get(&edge.to)?,
                edge.weight,
            ))
        })
        .collect()
}

/// Compute one SFDP iteration's forces using integer node indices, then return
/// them in the `BTreeMap<N, Position>` form `apply_forces_with_rigid_constraints`
/// consumes.
///
/// Repulsive forces accumulate in `i < j` pair order followed by edge order,
/// using flat `Vec` indexing rather than String-keyed maps (the hot loop;
/// String comparison alone was ~16% of total layout runtime before the index
/// form). `gravity` adds a weak linear pull toward the centroid of all
/// positioned nodes; it is non-zero only for graphs with multiple connected
/// components (see `force_directed_with_rigid_cb`), so connected graphs see
/// byte-identical forces to a gravity-free computation.
fn compute_iteration_forces<N: NodeId>(
    layout: &Layout<N>,
    nodes: &[N],
    edges_idx: &[(usize, usize, f64)],
    kp: f64,
    crk: f64,
    p: f64,
    gravity: f64,
) -> BTreeMap<N, Position> {
    let n = nodes.len();
    let positions: Vec<Option<Position>> =
        nodes.iter().map(|node| layout.get(node).copied()).collect();
    let mut forces = vec![Position::default(); n];

    // O(n^2) repulsive forces between all node pairs (same i < j order as before).
    for i in 0..n {
        let Some(pos1) = positions[i] else { continue };
        for j in (i + 1)..n {
            let Some(pos2) = positions[j] else { continue };

            let dx = pos1.x - pos2.x;
            let dy = pos1.y - pos2.y;
            let dist = (dx * dx + dy * dy).sqrt().max(1e-9);

            let f = kp / dist.powf(1.0 - p);
            let fx = f * dx / dist;
            let fy = f * dy / dist;

            forces[i].x += fx;
            forces[i].y += fy;
            forces[j].x -= fx;
            forces[j].y -= fy;
        }
    }

    // O(edges) attractive forces along edges (same edge order as before).
    for &(a, b, weight) in edges_idx {
        let (Some(pos1), Some(pos2)) = (positions[a], positions[b]) else {
            continue;
        };

        let dx = pos1.x - pos2.x;
        let dy = pos1.y - pos2.y;
        let dist = (dx * dx + dy * dy).sqrt();
        if dist < 1e-9 {
            continue;
        }

        let f = crk * dist * weight;
        let fx = f * dx / dist;
        let fy = f * dy / dist;

        forces[a].x -= fx;
        forces[a].y -= fy;
        forces[b].x += fx;
        forces[b].y += fy;
    }

    // Weak centroid gravity for disconnected graphs: a linear spring toward
    // the centroid of all positioned nodes. Guarded so a connected graph
    // (gravity == 0) skips the loop entirely.
    if gravity > 0.0 {
        let mut cx = 0.0;
        let mut cy = 0.0;
        let mut count = 0usize;
        for pos in positions.iter().flatten() {
            cx += pos.x;
            cy += pos.y;
            count += 1;
        }
        if count > 0 {
            let cx = cx / count as f64;
            let cy = cy / count as f64;
            for i in 0..n {
                let Some(pos) = positions[i] else { continue };
                forces[i].x += gravity * (cx - pos.x);
                forces[i].y += gravity * (cy - pos.y);
            }
        }
    }

    // Rebuild the sorted map apply_forces expects, one entry per node with a
    // position. A node that ends with zero net force is a no-op there (zero
    // magnitude => no move, no norm contribution, no group force), so this
    // matches the prior lazily-populated map's effect.
    let mut force_map = BTreeMap::new();
    for (i, node) in nodes.iter().enumerate() {
        if positions[i].is_some() {
            force_map.insert(node.clone(), forces[i]);
        }
    }
    force_map
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
        self.force_directed_with_rigid_cb(layout, k, &mut |_, _| None)
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

    /// Force-directed loop with iteration callback for interleaved annealing.
    ///
    /// The step schedule is Hu's adaptive cooling (see `adapt_step`), with both
    /// the initial step and the convergence threshold expressed relative to the
    /// ideal edge length `k`. The loop EXITS VIA CONVERGENCE in the normal case
    /// (the step decays below `convergence_threshold * k`, i.e. the layout is
    /// quiescent); `max_iterations` is only a safety bound.
    fn force_directed_with_rigid_cb(
        &self,
        layout: &mut Layout<N>,
        k: f64,
        callback: IterationCallback<'_, N>,
    ) {
        let config = self.config;
        let kp = k.powf(1.0 - config.p);
        let crk = config.c.powf((2.0 - config.p) / 3.0) / k;

        let mut step = config.initial_step_size * k;
        let convergence_step = config.convergence_threshold * k;
        let mut progress = 0usize;
        let mut prev_norm = f64::MAX;

        let nodes: Vec<N> = self.graph.nodes().cloned().collect();
        let node_index = build_node_index(&nodes);
        let edges_idx = build_edge_indices(&node_index, self.graph);

        // Mutually-disconnected components feel only repulsion from each other
        // and would drift apart forever under a converging step scheme; weak
        // centroid gravity gives them an equilibrium. Connected graphs (the
        // common case) get NO gravity.
        let gravity = if self.graph.connected_component_count() > 1 {
            DISCONNECTED_GRAVITY * crk
        } else {
            0.0
        };

        // Computed once from the initial layout. These offsets become stale if
        // the callback replaces the layout, but the constraints still converge
        // because SFDP re-applies offsets every iteration -- the worst case is
        // slightly slower convergence, not incorrect results.
        let group_offsets = self.compute_rigid_group_offsets(layout);

        for iteration in 0..config.max_iterations {
            let forces =
                compute_iteration_forces(layout, &nodes, &edges_idx, kp, crk, config.p, gravity);

            let norm =
                self.apply_forces_with_rigid_constraints(layout, &forces, step, &group_offsets);

            (step, progress) = adapt_step(step, norm, prev_norm, progress, config.cooling_factor);
            prev_norm = norm;

            // Invoke callback; if it returns an updated layout, use it
            if let Some(new_layout) = callback(iteration, layout) {
                *layout = new_layout;
                // The replaced (annealed) layout's force profile is unrelated
                // to the pre-replacement one; compare energies afresh so the
                // jump is not misread as an energy spike.
                prev_norm = f64::MAX;
            }

            if step < convergence_step {
                break;
            }
        }
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
    compute_layout_from_initial_with_callback(graph, config, initial, seed, &mut |_, _| None)
}

/// Like `compute_layout_from_initial` but calls `callback` after each SFDP
/// iteration. If the callback returns `Some(layout)`, SFDP continues from
/// that (e.g. annealed) layout. This enables interleaved annealing.
pub fn compute_layout_from_initial_with_callback<N: NodeId>(
    graph: &ConstrainedGraph<N>,
    config: &SfdpConfig,
    initial: &Layout<N>,
    seed: u64,
    callback: IterationCallback<'_, N>,
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

    sfdp.force_directed_with_rigid_cb(&mut layout, k, callback);

    if config.beautify_leaves {
        sfdp.beautify_leaves(&mut layout, k);
    }

    layout
}

/// Check if annealing should trigger at the current SFDP iteration.
pub fn should_trigger_annealing(
    iter: usize,
    interval: usize,
    last_annealing_iter: usize,
    round: usize,
    max_rounds: usize,
) -> bool {
    round < max_rounds && iter > 0 && (iter - last_annealing_iter) >= interval
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

    #[test]
    fn test_should_trigger_annealing_logic() {
        // Not enough iterations elapsed
        assert!(!should_trigger_annealing(50, 100, 0, 0, 3));
        // Enough iterations elapsed
        assert!(should_trigger_annealing(100, 100, 0, 0, 3));
        // Already at max rounds
        assert!(!should_trigger_annealing(200, 100, 0, 3, 3));
        // Iteration 0 never triggers
        assert!(!should_trigger_annealing(0, 1, 0, 0, 3));
        // Gap from last is less than interval
        assert!(!should_trigger_annealing(150, 100, 100, 1, 3));
        // Gap from last is exactly interval
        assert!(should_trigger_annealing(200, 100, 100, 1, 3));
    }

    #[test]
    fn test_sfdp_calls_callback() {
        let cg = triangle_graph();
        let config = SfdpConfig {
            max_iterations: 10,
            ..SfdpConfig::default()
        };
        let initial = BTreeMap::new();
        let mut call_count = 0usize;

        compute_layout_from_initial_with_callback(
            &cg,
            &config,
            &initial,
            42,
            &mut |_iter, _layout| {
                call_count += 1;
                None
            },
        );

        assert!(
            call_count > 0,
            "callback should have been invoked at least once"
        );
        assert!(
            call_count <= 10,
            "callback should not exceed max_iterations"
        );
    }

    // ---- adapt_step (Hu's schedule) ----

    #[test]
    fn test_adapt_step_grows_after_consecutive_decreases() {
        let t = 0.9;
        let mut step = 10.0;
        let mut progress = 0;
        // Four decreases: step unchanged, progress accumulates.
        for i in 1..STEP_PROGRESS_THRESHOLD {
            (step, progress) = adapt_step(step, 1.0, 2.0, progress, t);
            assert_eq!(step, 10.0, "step must hold until sustained progress");
            assert_eq!(progress, i);
        }
        // The fifth consecutive decrease grows the step and resets progress.
        (step, progress) = adapt_step(step, 1.0, 2.0, progress, t);
        assert!(
            (step - 10.0 / t).abs() < 1e-12,
            "step should grow by 1/t, got {step}"
        );
        assert_eq!(progress, 0);
    }

    #[test]
    fn test_adapt_step_cools_on_energy_increase() {
        let t = 0.9;
        // Progress is wiped and the step shrinks when energy rises.
        let (step, progress) = adapt_step(10.0, 3.0, 2.0, 4, t);
        assert!((step - 9.0).abs() < 1e-12, "step should shrink by t");
        assert_eq!(progress, 0);
        // Equal energy counts as "not decreasing" (cool) so plateaus also decay.
        let (step, progress) = adapt_step(10.0, 2.0, 2.0, 4, t);
        assert!((step - 9.0).abs() < 1e-12);
        assert_eq!(progress, 0);
    }

    /// A 6x6 grid mesh: 36 nodes, 60 edges, fully connected. Big enough that a
    /// broken step schedule cannot converge, small enough to run fast in tests.
    fn grid_graph(n: usize) -> ConstrainedGraph<String> {
        let node = |r: usize, c: usize| format!("n_{r}_{c}");
        let mut gb = GraphBuilder::new_undirected();
        for r in 0..n {
            for c in 0..n {
                if c + 1 < n {
                    gb.add_edge(node(r, c), node(r, c + 1), 1.0);
                }
                if r + 1 < n {
                    gb.add_edge(node(r, c), node(r + 1, c), 1.0);
                }
            }
        }
        ConstrainedGraphBuilder::new(gb.build()).build()
    }

    /// Run a config on a graph and report (last iteration index, final layout).
    fn run_to_completion(
        cg: &ConstrainedGraph<String>,
        config: &SfdpConfig,
    ) -> (usize, Layout<String>) {
        let mut last_iteration = 0;
        let layout = compute_layout_from_initial_with_callback(
            cg,
            config,
            &BTreeMap::new(),
            42,
            &mut |iter, _layout| {
                last_iteration = iter;
                None
            },
        );
        (last_iteration, layout)
    }

    /// QUIESCENCE: the production aux-placement configuration must CONVERGE (the
    /// adaptive step decays below the convergence threshold and the loop exits)
    /// on a moderately-sized connected graph -- not run until max_iterations and
    /// stop wherever it happens to be. Layouts cut off mid-flight are exactly the
    /// "this node is obviously stuck in the wrong place" tangles users see.
    #[test]
    fn test_aux_placement_config_converges_on_moderate_graph() {
        let cg = grid_graph(6);
        let config = SfdpConfig::for_aux_placement();

        let (last_iteration, layout) = run_to_completion(&cg, &config);

        assert_eq!(layout.len(), 36);
        assert!(
            last_iteration < config.max_iterations - 1,
            "production SFDP config should converge before max_iterations ({}); \
             it ran the full budget (last iteration {})",
            config.max_iterations,
            last_iteration
        );
    }

    /// Same property for the chain-positioning configuration.
    #[test]
    fn test_chain_positioning_config_converges_on_moderate_graph() {
        let cg = grid_graph(4);
        let config = SfdpConfig::for_chain_positioning();

        let (last_iteration, layout) = run_to_completion(&cg, &config);

        assert_eq!(layout.len(), 16);
        assert!(
            last_iteration < config.max_iterations - 1,
            "production chain SFDP config should converge before max_iterations ({}); \
             it ran the full budget (last iteration {})",
            config.max_iterations,
            last_iteration
        );
    }

    /// DISCONNECTED COMPONENTS: two clusters with no edge between them must end
    /// a bounded distance apart -- mutual repulsion alone would push them apart
    /// without limit under a properly-converging step scheme, so the algorithm
    /// applies a weak centroid gravity when (and only when) the graph has more
    /// than one component.
    #[test]
    fn test_disconnected_components_stay_bounded() {
        // Two 4-cycles, no edges between them.
        let mut gb = GraphBuilder::new_undirected();
        for (prefix, _) in [("a", 0), ("b", 1)] {
            for i in 0..4 {
                gb.add_edge(
                    format!("{prefix}{i}"),
                    format!("{prefix}{}", (i + 1) % 4),
                    1.0,
                );
            }
        }
        let cg = ConstrainedGraphBuilder::new(gb.build()).build();
        let config = SfdpConfig::for_aux_placement();

        let (_, layout) = run_to_completion(&cg, &config);

        // Centroid distance between the two clusters stays within a couple of
        // dozen ideal edge lengths.
        let centroid = |prefix: &str| -> Position {
            let pts: Vec<Position> = layout
                .iter()
                .filter(|(n, _)| n.starts_with(prefix))
                .map(|(_, &p)| p)
                .collect();
            Position::new(
                pts.iter().map(|p| p.x).sum::<f64>() / pts.len() as f64,
                pts.iter().map(|p| p.y).sum::<f64>() / pts.len() as f64,
            )
        };
        let dist = (centroid("a") - centroid("b")).length();
        assert!(
            dist < 25.0 * config.k,
            "disconnected components should stay bounded; centroids are {dist} apart \
             (k = {})",
            config.k
        );
        // ...but they must not be squeezed on top of each other either.
        assert!(
            dist > config.k / 2.0,
            "disconnected components should not collapse onto each other; \
             centroids are {dist} apart"
        );
    }
}
