// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Display;
use std::hash::Hash;
use std::ops::{Add, Sub};

/// 2D position/vector used throughout the layout pipeline.
#[derive(Clone, Copy, PartialEq, Default)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

impl std::fmt::Debug for Position {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({:.2}, {:.2})", self.x, self.y)
    }
}

impl Position {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// 2D cross product: z-component of the 3D cross product.
    pub fn cross_2d(self, other: Self) -> f64 {
        self.x * other.y - self.y * other.x
    }

    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y
    }

    pub fn length(self) -> f64 {
        (self.x * self.x + self.y * self.y).sqrt()
    }

    /// Angle from `self` to `other` in radians, in [-pi, pi].
    pub fn angle_to(self, other: Self) -> f64 {
        let delta = other - self;
        delta.y.atan2(delta.x)
    }
}

impl Add for Position {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
        }
    }
}

impl Sub for Position {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
        }
    }
}

/// Trait bound for graph node identifiers.
pub trait NodeId: Hash + Eq + Clone + Ord + Display {}
impl<T: Hash + Eq + Clone + Ord + Display> NodeId for T {}

/// A weighted edge connecting two nodes.
#[derive(Clone)]
pub struct Edge<N> {
    pub from: N,
    pub to: N,
    pub weight: f64,
}

/// Maps nodes to positions.
pub type Layout<N> = BTreeMap<N, Position>;

/// Immutable weighted graph. Use `GraphBuilder` to construct.
pub struct Graph<N: NodeId> {
    nodes: BTreeSet<N>,
    edges: Vec<Edge<N>>,
    is_directed: bool,
    adj: BTreeMap<N, BTreeMap<N, f64>>,
    adj_incoming: BTreeMap<N, BTreeMap<N, f64>>,
}

impl<N: NodeId> Graph<N> {
    pub fn nodes(&self) -> impl Iterator<Item = &N> {
        self.nodes.iter()
    }

    pub fn edges(&self) -> &[Edge<N>] {
        &self.edges
    }

    pub fn neighbors(&self, node: &N) -> Option<impl Iterator<Item = (&N, f64)>> {
        if !self.nodes.contains(node) {
            return None;
        }
        Some(
            self.adj
                .get(node)
                .into_iter()
                .flat_map(|m| m.iter().map(|(n, &w)| (n, w))),
        )
    }

    pub fn incoming_neighbors(&self, node: &N) -> Option<Vec<(&N, f64)>> {
        if !self.nodes.contains(node) {
            return None;
        }
        let source = if self.is_directed {
            &self.adj_incoming
        } else {
            &self.adj
        };
        Some(
            source
                .get(node)
                .into_iter()
                .flat_map(|m| m.iter().map(|(n, w)| (n, *w)))
                .collect(),
        )
    }

    pub fn degree(&self, node: &N) -> usize {
        self.adj.get(node).map_or(0, |m| m.len())
    }

    pub fn has_node(&self, node: &N) -> bool {
        self.nodes.contains(node)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_directed(&self) -> bool {
        self.is_directed
    }

    /// Detect all cycles in the graph.
    pub fn find_cycles(&self) -> Vec<Vec<N>> {
        if self.is_directed {
            self.find_directed_cycles()
        } else {
            self.find_undirected_cycles()
        }
    }

    fn find_directed_cycles(&self) -> Vec<Vec<N>> {
        let mut cycles = Vec::new();
        let mut visited = BTreeSet::new();
        let mut rec_stack = BTreeSet::new();

        for node in &self.nodes {
            if !visited.contains(node) {
                self.dfs_directed_cycles(
                    node,
                    &mut Vec::new(),
                    &mut visited,
                    &mut rec_stack,
                    &mut cycles,
                );
            }
        }
        cycles
    }

    fn dfs_directed_cycles(
        &self,
        node: &N,
        path: &mut Vec<N>,
        visited: &mut BTreeSet<N>,
        rec_stack: &mut BTreeSet<N>,
        cycles: &mut Vec<Vec<N>>,
    ) {
        visited.insert(node.clone());
        rec_stack.insert(node.clone());
        path.push(node.clone());

        if let Some(neighbors) = self.adj.get(node) {
            for neighbor in neighbors.keys() {
                if !visited.contains(neighbor) {
                    self.dfs_directed_cycles(neighbor, path, visited, rec_stack, cycles);
                } else if rec_stack.contains(neighbor) {
                    let mut cycle = Vec::new();
                    let mut found = false;
                    for n in path.iter() {
                        if n == neighbor {
                            found = true;
                        }
                        if found {
                            cycle.push(n.clone());
                        }
                    }
                    if !cycle.is_empty() {
                        cycles.push(cycle);
                    }
                }
            }
        }

        rec_stack.remove(node);
        path.pop();
    }

    fn find_undirected_cycles(&self) -> Vec<Vec<N>> {
        let mut cycles = Vec::new();
        let mut visited = BTreeSet::new();

        for node in &self.nodes {
            if !visited.contains(node) {
                self.dfs_undirected_cycles(node, None, &mut Vec::new(), &mut visited, &mut cycles);
            }
        }
        cycles
    }

    fn dfs_undirected_cycles(
        &self,
        node: &N,
        parent: Option<&N>,
        path: &mut Vec<N>,
        visited: &mut BTreeSet<N>,
        cycles: &mut Vec<Vec<N>>,
    ) {
        visited.insert(node.clone());
        path.push(node.clone());

        if let Some(neighbors) = self.adj.get(node) {
            for neighbor in neighbors.keys() {
                if !visited.contains(neighbor) {
                    self.dfs_undirected_cycles(neighbor, Some(node), path, visited, cycles);
                } else if parent.is_none_or(|p| p != neighbor)
                    && let Some(start) = path.iter().position(|n| n == neighbor)
                {
                    cycles.push(path[start..].to_vec());
                }
            }
        }

        path.pop();
    }

    pub fn has_cycle(&self) -> bool {
        if self.is_directed {
            self.has_directed_cycle()
        } else {
            self.has_undirected_cycle()
        }
    }

    fn has_directed_cycle(&self) -> bool {
        let mut visited = BTreeSet::new();
        let mut rec_stack = BTreeSet::new();

        for node in &self.nodes {
            if !visited.contains(node)
                && self.dfs_has_directed_cycle(node, &mut visited, &mut rec_stack)
            {
                return true;
            }
        }
        false
    }

    fn dfs_has_directed_cycle(
        &self,
        node: &N,
        visited: &mut BTreeSet<N>,
        rec_stack: &mut BTreeSet<N>,
    ) -> bool {
        visited.insert(node.clone());
        rec_stack.insert(node.clone());

        if let Some(neighbors) = self.adj.get(node) {
            for neighbor in neighbors.keys() {
                if !visited.contains(neighbor) {
                    if self.dfs_has_directed_cycle(neighbor, visited, rec_stack) {
                        return true;
                    }
                } else if rec_stack.contains(neighbor) {
                    return true;
                }
            }
        }

        rec_stack.remove(node);
        false
    }

    fn has_undirected_cycle(&self) -> bool {
        let mut visited = BTreeSet::new();

        for node in &self.nodes {
            if !visited.contains(node) && self.dfs_has_undirected_cycle(node, None, &mut visited) {
                return true;
            }
        }
        false
    }

    fn dfs_has_undirected_cycle(
        &self,
        node: &N,
        parent: Option<&N>,
        visited: &mut BTreeSet<N>,
    ) -> bool {
        visited.insert(node.clone());

        if let Some(neighbors) = self.adj.get(node) {
            for neighbor in neighbors.keys() {
                if !visited.contains(neighbor) {
                    if self.dfs_has_undirected_cycle(neighbor, Some(node), visited) {
                        return true;
                    }
                } else if parent.is_none_or(|p| p != neighbor) {
                    return true;
                }
            }
        }
        false
    }

    /// Topological sort for directed acyclic graphs. Returns `None` if the graph
    /// is undirected or contains cycles.
    pub fn topological_sort(&self) -> Option<Vec<N>> {
        if !self.is_directed || self.has_cycle() {
            return None;
        }

        let mut in_degree: BTreeMap<N, usize> = BTreeMap::new();
        for node in &self.nodes {
            let degree = self.adj_incoming.get(node).map_or(0, |m| m.len());
            in_degree.insert(node.clone(), degree);
        }

        let mut queue: Vec<N> = in_degree
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(n, _)| n.clone())
            .collect();

        let mut result = Vec::new();
        while let Some(node) = queue.first().cloned() {
            queue.remove(0);
            result.push(node.clone());

            if let Some(neighbors) = self.adj.get(&node) {
                let mut next: Vec<N> = Vec::new();
                for neighbor in neighbors.keys() {
                    if let Some(d) = in_degree.get_mut(neighbor) {
                        *d -= 1;
                        if *d == 0 {
                            next.push(neighbor.clone());
                        }
                    }
                }
                next.sort();
                queue.extend(next);
                queue.sort();
            }
        }

        if result.len() == self.nodes.len() {
            Some(result)
        } else {
            None
        }
    }
}

/// Builder for constructing an immutable `Graph`.
pub struct GraphBuilder<N: NodeId> {
    nodes: BTreeSet<N>,
    edges: Vec<Edge<N>>,
    adj: BTreeMap<N, BTreeMap<N, f64>>,
    adj_incoming: BTreeMap<N, BTreeMap<N, f64>>,
    is_directed: bool,
}

impl<N: NodeId> GraphBuilder<N> {
    pub fn new_undirected() -> Self {
        Self {
            nodes: BTreeSet::new(),
            edges: Vec::new(),
            adj: BTreeMap::new(),
            adj_incoming: BTreeMap::new(),
            is_directed: false,
        }
    }

    pub fn new_directed() -> Self {
        Self {
            nodes: BTreeSet::new(),
            edges: Vec::new(),
            adj: BTreeMap::new(),
            adj_incoming: BTreeMap::new(),
            is_directed: true,
        }
    }

    pub fn add_node(&mut self, node: N) -> &mut Self {
        self.nodes.insert(node);
        self
    }

    pub fn add_edge(&mut self, from: N, to: N, weight: f64) -> &mut Self {
        self.nodes.insert(from.clone());
        self.nodes.insert(to.clone());
        self.edges.push(Edge {
            from: from.clone(),
            to: to.clone(),
            weight,
        });

        if self.is_directed {
            self.adj
                .entry(from.clone())
                .or_default()
                .insert(to.clone(), weight);
            self.adj_incoming
                .entry(to)
                .or_default()
                .insert(from, weight);
        } else {
            self.adj
                .entry(from.clone())
                .or_default()
                .insert(to.clone(), weight);
            self.adj.entry(to).or_default().insert(from, weight);
        }
        self
    }

    pub fn build(self) -> Graph<N> {
        Graph {
            nodes: self.nodes,
            edges: self.edges,
            is_directed: self.is_directed,
            adj: self.adj,
            adj_incoming: self.adj_incoming,
        }
    }
}

/// A group of nodes that move together as a rigid unit during layout.
pub struct RigidGroup<N: NodeId> {
    pub members: Vec<N>,
    pub offsets: BTreeMap<N, Position>,
}

/// A graph augmented with layout constraints (pinned nodes, rigid groups).
pub struct ConstrainedGraph<N: NodeId> {
    pub(crate) graph: Graph<N>,
    pinned: BTreeSet<N>,
    pub(crate) rigid_groups: Vec<RigidGroup<N>>,
    node_to_group: HashMap<N, usize>,
}

impl<N: NodeId> std::ops::Deref for ConstrainedGraph<N> {
    type Target = Graph<N>;
    fn deref(&self) -> &Graph<N> {
        &self.graph
    }
}

impl<N: NodeId> ConstrainedGraph<N> {
    pub fn is_pinned(&self, node: &N) -> bool {
        self.pinned.contains(node)
    }

    pub fn get_rigid_group(&self, node: &N) -> Option<&RigidGroup<N>> {
        self.node_to_group
            .get(node)
            .map(|&idx| &self.rigid_groups[idx])
    }

    pub fn rigid_groups(&self) -> &[RigidGroup<N>] {
        &self.rigid_groups
    }

    pub fn is_in_rigid_group(&self, node: &N) -> Option<usize> {
        self.node_to_group.get(node).copied()
    }
}

/// Builder for `ConstrainedGraph`.
pub struct ConstrainedGraphBuilder<N: NodeId> {
    graph: Graph<N>,
    pinned: BTreeSet<N>,
    rigid_groups: Vec<Vec<N>>,
}

impl<N: NodeId> ConstrainedGraphBuilder<N> {
    pub fn new(graph: Graph<N>) -> Self {
        Self {
            graph,
            pinned: BTreeSet::new(),
            rigid_groups: Vec::new(),
        }
    }

    pub fn pin(&mut self, nodes: &[N]) -> &mut Self {
        for node in nodes {
            if self.graph.has_node(node) {
                self.pinned.insert(node.clone());
            }
        }
        self
    }

    pub fn add_rigid_group(&mut self, members: Vec<N>) -> &mut Self {
        let valid: Vec<N> = members
            .into_iter()
            .filter(|m| self.graph.has_node(m))
            .collect();
        if valid.len() >= 2 {
            let any_pinned = valid.iter().any(|m| self.pinned.contains(m));
            if any_pinned {
                for m in &valid {
                    self.pinned.insert(m.clone());
                }
            }
            self.rigid_groups.push(valid);
        }
        self
    }

    pub fn build(self) -> ConstrainedGraph<N> {
        let mut rigid_groups = Vec::with_capacity(self.rigid_groups.len());
        let mut node_to_group = HashMap::new();

        for (i, members) in self.rigid_groups.into_iter().enumerate() {
            for member in &members {
                node_to_group.insert(member.clone(), i);
            }
            rigid_groups.push(RigidGroup {
                members,
                offsets: BTreeMap::new(),
            });
        }

        ConstrainedGraph {
            graph: self.graph,
            pinned: self.pinned,
            rigid_groups,
            node_to_group,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_add_sub() {
        let a = Position::new(1.0, 2.0);
        let b = Position::new(3.0, 4.0);
        let sum = a + b;
        assert!((sum.x - 4.0).abs() < f64::EPSILON);
        assert!((sum.y - 6.0).abs() < f64::EPSILON);
        let diff = b - a;
        assert!((diff.x - 2.0).abs() < f64::EPSILON);
        assert!((diff.y - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_position_cross_2d_dot_length() {
        let a = Position::new(1.0, 0.0);
        let b = Position::new(0.0, 1.0);
        assert!((a.cross_2d(b) - 1.0).abs() < f64::EPSILON);
        assert!((b.cross_2d(a) - (-1.0)).abs() < f64::EPSILON);
        assert!(a.dot(b).abs() < f64::EPSILON);
        assert!((a.length() - 1.0).abs() < f64::EPSILON);
        let c = Position::new(3.0, 4.0);
        assert!((c.length() - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_position_angle_to() {
        let origin = Position::new(0.0, 0.0);
        let right = Position::new(1.0, 0.0);
        let up = Position::new(0.0, 1.0);
        assert!((origin.angle_to(right)).abs() < 1e-10);
        assert!((origin.angle_to(up) - std::f64::consts::FRAC_PI_2).abs() < 1e-10);
    }

    #[test]
    fn test_graph_builder_undirected() {
        let mut builder = GraphBuilder::<String>::new_undirected();
        builder.add_edge("a".into(), "b".into(), 1.0);
        builder.add_edge("b".into(), "c".into(), 2.0);
        let graph = builder.build();

        assert_eq!(graph.node_count(), 3);
        assert!(!graph.is_directed());

        let a_neighbors: Vec<_> = graph
            .neighbors(&"a".into())
            .unwrap()
            .map(|(n, _)| n.clone())
            .collect();
        assert!(a_neighbors.contains(&"b".to_string()));

        let b_neighbors: Vec<_> = graph
            .neighbors(&"b".into())
            .unwrap()
            .map(|(n, _)| n.clone())
            .collect();
        assert!(b_neighbors.contains(&"a".to_string()));
        assert!(b_neighbors.contains(&"c".to_string()));
    }

    #[test]
    fn test_graph_builder_directed() {
        let mut builder = GraphBuilder::<String>::new_directed();
        builder.add_edge("a".into(), "b".into(), 1.0);
        builder.add_edge("b".into(), "c".into(), 1.0);
        let graph = builder.build();

        assert!(graph.is_directed());

        let a_neighbors: Vec<_> = graph
            .neighbors(&"a".into())
            .unwrap()
            .map(|(n, _)| n.clone())
            .collect();
        assert_eq!(a_neighbors, vec!["b".to_string()]);

        let a_incoming = graph.incoming_neighbors(&"a".into()).unwrap();
        assert!(a_incoming.is_empty());

        let b_incoming: Vec<String> = graph
            .incoming_neighbors(&"b".into())
            .unwrap()
            .into_iter()
            .map(|(n, _)| n.clone())
            .collect();
        assert_eq!(b_incoming, vec!["a".to_string()]);
    }

    #[test]
    fn test_graph_degree_has_node() {
        let mut builder = GraphBuilder::<String>::new_undirected();
        builder.add_edge("a".into(), "b".into(), 1.0);
        builder.add_edge("a".into(), "c".into(), 1.0);
        builder.add_node("d".into());
        let graph = builder.build();

        assert_eq!(graph.degree(&"a".into()), 2);
        assert_eq!(graph.degree(&"b".into()), 1);
        assert_eq!(graph.degree(&"d".into()), 0);
        assert!(graph.has_node(&"a".into()));
        assert!(!graph.has_node(&"z".into()));
    }

    #[test]
    fn test_find_cycles_directed() {
        let mut builder = GraphBuilder::<String>::new_directed();
        builder.add_edge("a".into(), "b".into(), 1.0);
        builder.add_edge("b".into(), "c".into(), 1.0);
        builder.add_edge("c".into(), "a".into(), 1.0);
        let graph = builder.build();

        let cycles = graph.find_cycles();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].len(), 3);
    }

    #[test]
    fn test_find_cycles_undirected() {
        let mut builder = GraphBuilder::<String>::new_undirected();
        builder.add_edge("a".into(), "b".into(), 1.0);
        builder.add_edge("b".into(), "c".into(), 1.0);
        builder.add_edge("c".into(), "a".into(), 1.0);
        let graph = builder.build();

        let cycles = graph.find_cycles();
        assert!(!cycles.is_empty());
    }

    #[test]
    fn test_has_cycle_true_false() {
        // Directed cycle
        let mut builder = GraphBuilder::<String>::new_directed();
        builder.add_edge("a".into(), "b".into(), 1.0);
        builder.add_edge("b".into(), "a".into(), 1.0);
        assert!(builder.build().has_cycle());

        // Directed DAG
        let mut builder = GraphBuilder::<String>::new_directed();
        builder.add_edge("a".into(), "b".into(), 1.0);
        builder.add_edge("b".into(), "c".into(), 1.0);
        assert!(!builder.build().has_cycle());

        // Undirected cycle
        let mut builder = GraphBuilder::<String>::new_undirected();
        builder.add_edge("a".into(), "b".into(), 1.0);
        builder.add_edge("b".into(), "c".into(), 1.0);
        builder.add_edge("c".into(), "a".into(), 1.0);
        assert!(builder.build().has_cycle());

        // Undirected tree (no cycle)
        let mut builder = GraphBuilder::<String>::new_undirected();
        builder.add_edge("a".into(), "b".into(), 1.0);
        builder.add_edge("b".into(), "c".into(), 1.0);
        assert!(!builder.build().has_cycle());
    }

    #[test]
    fn test_topological_sort_dag() {
        let mut builder = GraphBuilder::<String>::new_directed();
        builder.add_edge("a".into(), "b".into(), 1.0);
        builder.add_edge("a".into(), "c".into(), 1.0);
        builder.add_edge("b".into(), "d".into(), 1.0);
        builder.add_edge("c".into(), "d".into(), 1.0);
        let graph = builder.build();

        let order = graph.topological_sort().unwrap();
        assert_eq!(order.len(), 4);

        // "a" must come before "b" and "c", both before "d"
        let pos = |n: &str| order.iter().position(|x| x == n).unwrap();
        assert!(pos("a") < pos("b"));
        assert!(pos("a") < pos("c"));
        assert!(pos("b") < pos("d"));
        assert!(pos("c") < pos("d"));
    }

    #[test]
    fn test_topological_sort_cyclic() {
        let mut builder = GraphBuilder::<String>::new_directed();
        builder.add_edge("a".into(), "b".into(), 1.0);
        builder.add_edge("b".into(), "a".into(), 1.0);
        let graph = builder.build();
        assert!(graph.topological_sort().is_none());
    }

    #[test]
    fn test_constrained_graph_pinning() {
        let mut builder = GraphBuilder::<String>::new_undirected();
        builder.add_edge("a".into(), "b".into(), 1.0);
        builder.add_node("c".into());
        let graph = builder.build();

        let mut cb = ConstrainedGraphBuilder::new(graph);
        cb.pin(&["a".into()]);
        let cg = cb.build();

        assert!(cg.is_pinned(&"a".into()));
        assert!(!cg.is_pinned(&"b".into()));
        assert!(!cg.is_pinned(&"c".into()));
    }

    #[test]
    fn test_constrained_graph_rigid_groups() {
        let mut builder = GraphBuilder::<String>::new_undirected();
        builder.add_edge("a".into(), "b".into(), 1.0);
        builder.add_edge("b".into(), "c".into(), 1.0);
        builder.add_node("d".into());
        let graph = builder.build();

        let mut cb = ConstrainedGraphBuilder::new(graph);
        cb.add_rigid_group(vec!["a".into(), "b".into()]);
        let cg = cb.build();

        assert!(cg.is_in_rigid_group(&"a".into()).is_some());
        assert!(cg.is_in_rigid_group(&"b".into()).is_some());
        assert!(cg.is_in_rigid_group(&"c".into()).is_none());
        assert!(cg.is_in_rigid_group(&"d".into()).is_none());

        assert_eq!(cg.rigid_groups().len(), 1);
        assert_eq!(cg.rigid_groups()[0].members.len(), 2);
    }
}
