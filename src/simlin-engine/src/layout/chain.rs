// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::config::LayoutConfig;
use super::graph::{ConstrainedGraphBuilder, GraphBuilder, Position};
use super::metadata::{ComputedMetadata, StockFlowChain};
use super::sfdp::{SfdpConfig, compute_layout_from_initial};

/// Prefix for synthetic cloud nodes representing missing flow endpoints.
pub const CLOUD_NODE_PREFIX: &str = "__cloud__";
/// Prefix for chain-level synthetic cloud nodes in the dependency graph.
pub const CHAIN_CLOUD_NODE_PREFIX: &str = "__chain_cloud__";
/// Margin from the diagram origin for element placement.
pub const DIAGRAM_ORIGIN_MARGIN: f64 = 50.0;

/// Create a cloud node identifier from a unique integer ID.
pub fn make_cloud_node_ident(uid: i32) -> String {
    format!("{CLOUD_NODE_PREFIX}{uid}")
}

/// Parse a cloud node identifier back to its integer ID.
/// Returns `None` if the string does not have the expected prefix or suffix.
pub fn parse_cloud_node_ident(ident: &str) -> Option<i32> {
    ident.strip_prefix(CLOUD_NODE_PREFIX)?.parse().ok()
}

/// Create a chain-level cloud identifier from a chain index and sequence number.
pub fn make_chain_cloud_ident(chain_index: usize, seq: usize) -> String {
    format!("{CHAIN_CLOUD_NODE_PREFIX}{chain_index}_{seq}")
}

/// Parse a chain cloud identifier back to (chain_index, sequence).
/// Returns `None` if the string does not match the expected format.
pub fn parse_chain_cloud_ident(ident: &str) -> Option<(usize, usize)> {
    let suffix = ident.strip_prefix(CHAIN_CLOUD_NODE_PREFIX)?;
    let (chain_part, seq_part) = suffix.split_once('_')?;
    let chain_index = chain_part.parse().ok()?;
    let seq = seq_part.parse().ok()?;
    Some((chain_index, seq))
}

/// Recursively follow incoming edges in the dependency graph until we
/// reach a variable that belongs to a chain.  Returns the set of chain
/// indices that are (transitively) upstream of `var`.
fn find_chain_sources(
    var: &str,
    visited: &mut BTreeSet<String>,
    var_to_chain: &HashMap<String, usize>,
    dep_graph: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<usize> {
    if !visited.insert(var.to_string()) {
        return BTreeSet::new();
    }

    if let Some(&chain_idx) = var_to_chain.get(var) {
        let mut result = BTreeSet::new();
        result.insert(chain_idx);
        return result;
    }

    let mut sources = BTreeSet::new();
    if let Some(deps) = dep_graph.get(var) {
        for dep in deps {
            sources.extend(find_chain_sources(dep, visited, var_to_chain, dep_graph));
        }
    }
    sources
}

/// Build an undirected graph where each stock-flow chain is a node,
/// cross-chain dependencies become edges, and synthetic cloud nodes
/// are added for flows with missing from/to stocks.
///
/// Returns the constrained graph and a map from variable identifiers
/// to graph node IDs.
pub fn build_chain_dependency_graph(
    chains: &[StockFlowChain],
    metadata: &ComputedMetadata,
) -> (
    super::graph::ConstrainedGraph<String>,
    HashMap<String, String>,
) {
    let mut var_to_chain: HashMap<String, usize> = HashMap::new();
    for (i, chain) in chains.iter().enumerate() {
        for var in &chain.all_vars {
            var_to_chain.insert(var.clone(), i);
        }
    }

    let mut builder = GraphBuilder::<String>::new_undirected();
    let mut var_to_node: HashMap<String, String> = HashMap::new();

    // Each chain becomes a node
    for (i, chain) in chains.iter().enumerate() {
        let node_id = format!("chain_{i}");
        builder.add_node(node_id.clone());
        for var in &chain.all_vars {
            var_to_node.insert(var.clone(), node_id.clone());
        }
    }

    // Track edges we've already added to avoid duplicates
    let mut added_edges: BTreeSet<(usize, usize)> = BTreeSet::new();

    // For each chain variable with incoming dependencies, trace through
    // auxiliaries to find source chains and add cross-chain edges.
    for (i, chain) in chains.iter().enumerate() {
        for var in &chain.all_vars {
            if let Some(deps) = metadata.dep_graph.get(var) {
                for dep in deps {
                    let mut visited = BTreeSet::new();
                    let source_chains =
                        find_chain_sources(dep, &mut visited, &var_to_chain, &metadata.dep_graph);
                    for &source_chain in &source_chains {
                        if source_chain == i {
                            continue;
                        }
                        let edge_key = if source_chain < i {
                            (source_chain, i)
                        } else {
                            (i, source_chain)
                        };
                        if added_edges.insert(edge_key) {
                            builder.add_edge(
                                format!("chain_{}", edge_key.0),
                                format!("chain_{}", edge_key.1),
                                1.0,
                            );
                        }
                    }
                }
            }
        }
    }

    // Add synthetic cloud nodes for flows missing a from or to stock
    let mut cloud_seq: BTreeMap<usize, usize> = BTreeMap::new();
    for (i, chain) in chains.iter().enumerate() {
        for flow in &chain.flows {
            let (from_stock, to_stock) = metadata.connected_stocks(flow);
            if from_stock.is_none() || to_stock.is_none() {
                let seq = cloud_seq.entry(i).or_insert(0);
                let cloud_id = make_chain_cloud_ident(i, *seq);
                *seq += 1;
                builder.add_node(cloud_id.clone());
                builder.add_edge(format!("chain_{i}"), cloud_id, 1.0);
            }
        }
    }

    let constrained = ConstrainedGraphBuilder::new(builder.build()).build();
    (constrained, var_to_node)
}

/// Use SFDP to compute positions for chains, then normalize to target dimensions.
pub fn compute_chain_positions(
    chains: &[StockFlowChain],
    metadata: &ComputedMetadata,
    config: &LayoutConfig,
) -> BTreeMap<usize, Position> {
    if chains.is_empty() {
        return BTreeMap::new();
    }

    let n = chains.len();

    if n == 1 {
        let mut result = BTreeMap::new();
        result.insert(0, Position::new(config.start_x, config.start_y));
        return result;
    }

    let (graph, _var_to_node) = build_chain_dependency_graph(chains, metadata);

    // Create initial layout with distinct positions per chain
    let mut initial = BTreeMap::new();
    for i in 0..n {
        initial.insert(
            format!("chain_{i}"),
            Position::new(
                config.start_x + i as f64 * config.horizontal_spacing,
                config.start_y + i as f64 * config.vertical_spacing,
            ),
        );
    }

    // Position synthetic cloud nodes at their parent chain's position
    for node in graph.nodes() {
        if let Some((chain_idx, _seq)) = parse_chain_cloud_ident(node)
            && let Some(&chain_pos) = initial.get(&format!("chain_{chain_idx}"))
        {
            initial.insert(node.clone(), chain_pos);
        }
    }

    let sfdp_config = SfdpConfig {
        k: 150.0,
        c: 0.5,
        ..SfdpConfig::default()
    };

    let layout = compute_layout_from_initial(&graph, &sfdp_config, &initial, 42);

    // Extract only the chain node positions
    let chain_positions: BTreeMap<usize, Position> = (0..n)
        .filter_map(|i| layout.get(&format!("chain_{i}")).map(|pos| (i, *pos)))
        .collect();

    // Find bounding box of chain positions
    let mut min_x = f64::MAX;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::MAX;
    let mut max_y = f64::NEG_INFINITY;
    for pos in chain_positions.values() {
        min_x = min_x.min(pos.x);
        max_x = max_x.max(pos.x);
        min_y = min_y.min(pos.y);
        max_y = max_y.max(pos.y);
    }

    let layout_width = max_x - min_x;
    let layout_height = max_y - min_y;

    // Fallback to deterministic lanes if layout is degenerate
    if layout_width <= 1e-6 {
        let mut result = BTreeMap::new();
        for i in 0..n {
            result.insert(
                i,
                Position::new(
                    config.start_x + i as f64 * config.horizontal_spacing,
                    config.start_y + i as f64 * config.vertical_spacing,
                ),
            );
        }
        return result;
    }

    let target_width =
        (config.horizontal_spacing * (n as f64 - 1.0)).max(config.horizontal_spacing);
    let target_height = (config.vertical_spacing * (n as f64 - 1.0)).max(config.vertical_spacing);

    let mut result = BTreeMap::new();
    for (i, pos) in &chain_positions {
        let nx = config.start_x + (pos.x - min_x) / layout_width * target_width;
        let ny = if layout_height <= 1e-6 {
            config.start_y + *i as f64 * config.vertical_spacing
        } else {
            config.start_y + (pos.y - min_y) / layout_height * target_height
        };
        result.insert(*i, Position::new(nx, ny));
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::metadata::ComputedMetadata;

    #[test]
    fn test_cloud_node_ident_roundtrip() {
        let ident = make_cloud_node_ident(42);
        assert_eq!(ident, "__cloud__42");
        assert_eq!(parse_cloud_node_ident(&ident), Some(42));

        let ident = make_cloud_node_ident(0);
        assert_eq!(ident, "__cloud__0");
        assert_eq!(parse_cloud_node_ident(&ident), Some(0));

        let ident = make_cloud_node_ident(-7);
        assert_eq!(ident, "__cloud__-7");
        assert_eq!(parse_cloud_node_ident(&ident), Some(-7));
    }

    #[test]
    fn test_chain_cloud_ident_roundtrip() {
        let ident = make_chain_cloud_ident(1, 3);
        assert_eq!(ident, "__chain_cloud__1_3");
        assert_eq!(parse_chain_cloud_ident(&ident), Some((1, 3)));

        let ident = make_chain_cloud_ident(0, 0);
        assert_eq!(ident, "__chain_cloud__0_0");
        assert_eq!(parse_chain_cloud_ident(&ident), Some((0, 0)));

        let ident = make_chain_cloud_ident(99, 42);
        assert_eq!(ident, "__chain_cloud__99_42");
        assert_eq!(parse_chain_cloud_ident(&ident), Some((99, 42)));
    }

    #[test]
    fn test_parse_cloud_node_ident_invalid() {
        assert_eq!(parse_cloud_node_ident(""), None);
        assert_eq!(parse_cloud_node_ident("__cloud__"), None);
        assert_eq!(parse_cloud_node_ident("__cloud__abc"), None);
        assert_eq!(parse_cloud_node_ident("not_a_cloud"), None);
        assert_eq!(parse_cloud_node_ident("__chain_cloud__1_2"), None);
    }

    #[test]
    fn test_parse_chain_cloud_ident_invalid() {
        assert_eq!(parse_chain_cloud_ident(""), None);
        assert_eq!(parse_chain_cloud_ident("__chain_cloud__"), None);
        assert_eq!(parse_chain_cloud_ident("__chain_cloud__1"), None);
        assert_eq!(parse_chain_cloud_ident("__chain_cloud__a_b"), None);
        assert_eq!(parse_chain_cloud_ident("__chain_cloud__1_"), None);
        assert_eq!(parse_chain_cloud_ident("__cloud__42"), None);
        assert_eq!(parse_chain_cloud_ident("not_a_chain_cloud"), None);
    }

    #[test]
    fn test_compute_chain_positions_empty() {
        let metadata = ComputedMetadata::new_empty();
        let config = LayoutConfig::default();
        let result = compute_chain_positions(&[], &metadata, &config);
        assert!(result.is_empty());
    }

    #[test]
    fn test_compute_chain_positions_single() {
        let chains = vec![StockFlowChain {
            stocks: vec!["population".to_string()],
            flows: vec!["births".to_string()],
            all_vars: vec!["population".to_string(), "births".to_string()],
            importance: 1.0,
        }];
        let mut metadata = ComputedMetadata::new_empty();
        metadata
            .flow_to_stocks
            .insert("births".to_string(), (None, Some("population".to_string())));
        let config = LayoutConfig::default();
        let result = compute_chain_positions(&chains, &metadata, &config);

        assert_eq!(result.len(), 1);
        let pos = result.get(&0).expect("chain 0 should have a position");
        assert!(
            (pos.x - config.start_x).abs() < f64::EPSILON,
            "single chain x should be start_x"
        );
        assert!(
            (pos.y - config.start_y).abs() < f64::EPSILON,
            "single chain y should be start_y"
        );
    }

    #[test]
    fn test_compute_chain_positions_multiple() {
        let chains = vec![
            StockFlowChain {
                stocks: vec!["stock_a".to_string()],
                flows: vec!["flow_a".to_string()],
                all_vars: vec!["stock_a".to_string(), "flow_a".to_string()],
                importance: 1.0,
            },
            StockFlowChain {
                stocks: vec!["stock_b".to_string()],
                flows: vec!["flow_b".to_string()],
                all_vars: vec!["stock_b".to_string(), "flow_b".to_string()],
                importance: 0.5,
            },
            StockFlowChain {
                stocks: vec!["stock_c".to_string()],
                flows: vec!["flow_c".to_string()],
                all_vars: vec!["stock_c".to_string(), "flow_c".to_string()],
                importance: 0.3,
            },
        ];
        let mut metadata = ComputedMetadata::new_empty();
        metadata.flow_to_stocks.insert(
            "flow_a".to_string(),
            (Some("stock_a".to_string()), Some("stock_a".to_string())),
        );
        metadata.flow_to_stocks.insert(
            "flow_b".to_string(),
            (Some("stock_b".to_string()), Some("stock_b".to_string())),
        );
        metadata.flow_to_stocks.insert(
            "flow_c".to_string(),
            (Some("stock_c".to_string()), Some("stock_c".to_string())),
        );
        let config = LayoutConfig::default();
        let result = compute_chain_positions(&chains, &metadata, &config);

        assert_eq!(result.len(), 3);

        // All positions should be finite
        for pos in result.values() {
            assert!(pos.x.is_finite(), "x should be finite");
            assert!(pos.y.is_finite(), "y should be finite");
        }

        // Positions should not all be identical
        let positions: Vec<&Position> = result.values().collect();
        let all_same = positions
            .windows(2)
            .all(|w| (w[0].x - w[1].x).abs() < 1e-6 && (w[0].y - w[1].y).abs() < 1e-6);
        assert!(!all_same, "multiple chains should have distinct positions");
    }

    #[test]
    fn test_build_chain_dependency_graph_no_cross_deps() {
        let chains = vec![
            StockFlowChain {
                stocks: vec!["stock_a".to_string()],
                flows: vec!["flow_a".to_string()],
                all_vars: vec!["stock_a".to_string(), "flow_a".to_string()],
                importance: 1.0,
            },
            StockFlowChain {
                stocks: vec!["stock_b".to_string()],
                flows: vec!["flow_b".to_string()],
                all_vars: vec!["stock_b".to_string(), "flow_b".to_string()],
                importance: 0.5,
            },
        ];
        let mut metadata = ComputedMetadata::new_empty();
        // Both flows have complete stock connections (no clouds needed)
        metadata.flow_to_stocks.insert(
            "flow_a".to_string(),
            (Some("stock_a".to_string()), Some("stock_a".to_string())),
        );
        metadata.flow_to_stocks.insert(
            "flow_b".to_string(),
            (Some("stock_b".to_string()), Some("stock_b".to_string())),
        );

        let (graph, var_to_node) = build_chain_dependency_graph(&chains, &metadata);

        // Should have exactly 2 chain nodes, no cloud nodes
        assert_eq!(graph.node_count(), 2);
        assert!(graph.has_node(&"chain_0".to_string()));
        assert!(graph.has_node(&"chain_1".to_string()));

        // No edges between independent chains
        assert!(graph.edges().is_empty());

        // Variable mapping should be present
        assert_eq!(var_to_node.get("stock_a"), Some(&"chain_0".to_string()));
        assert_eq!(var_to_node.get("flow_a"), Some(&"chain_0".to_string()));
        assert_eq!(var_to_node.get("stock_b"), Some(&"chain_1".to_string()));
        assert_eq!(var_to_node.get("flow_b"), Some(&"chain_1".to_string()));
    }

    #[test]
    fn test_find_chain_sources_direct() {
        let mut var_to_chain = HashMap::new();
        var_to_chain.insert("stock_a".to_string(), 0usize);
        var_to_chain.insert("flow_a".to_string(), 0);
        var_to_chain.insert("stock_b".to_string(), 1);
        var_to_chain.insert("flow_b".to_string(), 1);

        let mut dep_graph = BTreeMap::new();
        dep_graph.insert(
            "flow_b".to_string(),
            BTreeSet::from(["stock_a".to_string()]),
        );

        // flow_b depends on stock_a, which is in chain 0
        let mut visited = BTreeSet::new();
        let sources = find_chain_sources("stock_a", &mut visited, &var_to_chain, &dep_graph);
        assert_eq!(sources.len(), 1);
        assert!(sources.contains(&0));
    }

    #[test]
    fn test_find_chain_sources_through_auxiliary() {
        let mut var_to_chain = HashMap::new();
        var_to_chain.insert("stock_a".to_string(), 0usize);
        var_to_chain.insert("stock_b".to_string(), 1);

        let mut dep_graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        // aux depends on stock_a (chain 0)
        dep_graph.insert("aux".to_string(), BTreeSet::from(["stock_a".to_string()]));

        // Starting from aux (not in any chain), should find chain 0
        let mut visited = BTreeSet::new();
        let sources = find_chain_sources("aux", &mut visited, &var_to_chain, &dep_graph);
        assert_eq!(sources.len(), 1);
        assert!(sources.contains(&0));
    }

    #[test]
    fn test_build_chain_dependency_graph_with_clouds() {
        let chains = vec![StockFlowChain {
            stocks: vec!["population".to_string()],
            flows: vec!["births".to_string()],
            all_vars: vec!["population".to_string(), "births".to_string()],
            importance: 1.0,
        }];
        let mut metadata = ComputedMetadata::new_empty();
        // births has no from_stock -- should generate a cloud node
        metadata
            .flow_to_stocks
            .insert("births".to_string(), (None, Some("population".to_string())));

        let (graph, _var_to_node) = build_chain_dependency_graph(&chains, &metadata);

        // chain_0 + one cloud node
        assert_eq!(graph.node_count(), 2);
        assert!(graph.has_node(&"chain_0".to_string()));
        assert!(graph.has_node(&make_chain_cloud_ident(0, 0)));
    }

    #[test]
    fn test_build_chain_dependency_graph_cross_deps() {
        let chains = vec![
            StockFlowChain {
                stocks: vec!["stock_a".to_string()],
                flows: vec!["flow_a".to_string()],
                all_vars: vec!["stock_a".to_string(), "flow_a".to_string()],
                importance: 1.0,
            },
            StockFlowChain {
                stocks: vec!["stock_b".to_string()],
                flows: vec!["flow_b".to_string()],
                all_vars: vec!["stock_b".to_string(), "flow_b".to_string()],
                importance: 0.5,
            },
        ];
        let mut metadata = ComputedMetadata::new_empty();
        metadata.flow_to_stocks.insert(
            "flow_a".to_string(),
            (Some("stock_a".to_string()), Some("stock_a".to_string())),
        );
        metadata.flow_to_stocks.insert(
            "flow_b".to_string(),
            (Some("stock_b".to_string()), Some("stock_b".to_string())),
        );
        // flow_b depends on stock_a (cross-chain dependency)
        metadata.dep_graph.insert(
            "flow_b".to_string(),
            BTreeSet::from(["stock_a".to_string()]),
        );

        let (graph, _var_to_node) = build_chain_dependency_graph(&chains, &metadata);

        assert_eq!(graph.node_count(), 2);
        // Should have an edge between chain_0 and chain_1
        assert_eq!(graph.edges().len(), 1);
    }
}
