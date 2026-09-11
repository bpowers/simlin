// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Incremental layout: bring an existing diagram in line with a patched model
//! while leaving every element the patch did not touch exactly where it is --
//! place and settle the new elements, rebuild the flows whose stock faces
//! changed, and diff the connectors and clouds.

use super::*;

/// Compute initial positions for newly-added elements based on their
/// dependency connections to existing elements.
///
/// Three placement strategies:
/// - Connected aux/module: centroid of connected existing elements with
///   ring spreading when multiple new elements share the same connections
/// - Connected chain element: near connected existing elements with offset
/// - Disconnected element: at the diagram periphery beyond existing bounds
pub fn compute_new_element_positions(
    state: &LayoutState,
    metadata: &ComputedMetadata,
    new_elements: &NewElements,
) -> HashMap<String, Position> {
    let mut result: HashMap<String, Position> = HashMap::new();

    let new_set: HashSet<&str> = new_elements
        .new_stocks
        .iter()
        .chain(&new_elements.new_flows)
        .chain(&new_elements.new_auxes)
        .chain(&new_elements.new_modules)
        .map(|s| s.as_str())
        .collect();

    // Compute bounding box of all existing positioned elements for periphery placement
    let (bbox_min, bbox_max) = existing_bounding_box(state);

    // Place new auxes and modules near connected existing elements
    place_new_point_elements(
        state,
        metadata,
        &new_elements.new_auxes,
        &new_set,
        &bbox_min,
        &bbox_max,
        &mut result,
    );
    place_new_point_elements(
        state,
        metadata,
        &new_elements.new_modules,
        &new_set,
        &bbox_min,
        &bbox_max,
        &mut result,
    );

    // Place new stocks and flows (chain elements)
    place_new_chain_elements(
        state,
        metadata,
        new_elements,
        &new_set,
        &bbox_max,
        &mut result,
    );

    result
}

/// Bounding box of variable elements (stocks, flows, auxes, modules) only.
/// Excludes aliases, groups, and clouds so that outlier non-variable elements
/// don't push new variable placement far from the actual model graph.
/// Returns ((min_x, min_y), (max_x, max_y)).
/// When no variable elements exist, returns a default origin area.
pub(super) fn existing_bounding_box(state: &LayoutState) -> (Position, Position) {
    let variable_uids: HashSet<i32> = state
        .elements
        .iter()
        .filter(|e| {
            matches!(
                e,
                ViewElement::Stock(_)
                    | ViewElement::Flow(_)
                    | ViewElement::Aux(_)
                    | ViewElement::Module(_)
            )
        })
        .map(|e| e.get_uid())
        .collect();

    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut found = false;
    for (&uid, pos) in &state.positions {
        if !variable_uids.contains(&uid) {
            continue;
        }
        found = true;
        min_x = min_x.min(pos.x);
        min_y = min_y.min(pos.y);
        max_x = max_x.max(pos.x);
        max_y = max_y.max(pos.y);
    }
    if !found {
        return (
            Position::new(DIAGRAM_ORIGIN_MARGIN, DIAGRAM_ORIGIN_MARGIN),
            Position::new(DIAGRAM_ORIGIN_MARGIN, DIAGRAM_ORIGIN_MARGIN),
        );
    }
    (Position::new(min_x, min_y), Position::new(max_x, max_y))
}

/// Collect (uid, position) pairs for existing elements connected to a given
/// ident via dep_graph (things `ident` depends on) and reverse_dep_graph
/// (things that depend on `ident`), excluding other new elements.
///
/// Returning UIDs alongside positions lets callers build grouping keys
/// directly from stable identifiers rather than doing a position-based
/// reverse lookup.
fn connected_existing_positions(
    state: &LayoutState,
    metadata: &ComputedMetadata,
    ident: &str,
    new_set: &HashSet<&str>,
) -> Vec<(i32, Position)> {
    let mut pairs = Vec::new();
    let mut seen = HashSet::new();

    // Forward: things this element depends on
    if let Some(deps) = metadata.dep_graph.get(ident) {
        for dep in deps {
            if new_set.contains(dep.as_str()) || !seen.insert(dep.as_str()) {
                continue;
            }
            if let Some(uid) = state.uid_manager.get_uid(dep)
                && let Some(&pos) = state.positions.get(&uid)
            {
                pairs.push((uid, pos));
            }
        }
    }

    // Reverse: things that depend on this element
    if let Some(dependents) = metadata.reverse_dep_graph.get(ident) {
        for dep in dependents {
            if new_set.contains(dep.as_str()) || !seen.insert(dep.as_str()) {
                continue;
            }
            if let Some(uid) = state.uid_manager.get_uid(dep)
                && let Some(&pos) = state.positions.get(&uid)
            {
                pairs.push((uid, pos));
            }
        }
    }

    pairs
}

/// Centroid of a non-empty set of positions.
fn centroid(positions: &[Position]) -> Position {
    let n = positions.len() as f64;
    let sum_x: f64 = positions.iter().map(|p| p.x).sum();
    let sum_y: f64 = positions.iter().map(|p| p.y).sum();
    Position::new(sum_x / n, sum_y / n)
}

/// Place new aux or module elements near their connected existing elements,
/// spreading multiple elements that share the same connections into a ring.
fn place_new_point_elements(
    state: &LayoutState,
    metadata: &ComputedMetadata,
    new_idents: &[String],
    new_set: &HashSet<&str>,
    bbox_min: &Position,
    bbox_max: &Position,
    result: &mut HashMap<String, Position>,
) {
    if new_idents.is_empty() {
        return;
    }

    // Group new elements by their set of connected existing element UIDs
    // so we can spread apart those that share the same connection set.
    let mut connection_groups: HashMap<Vec<i32>, Vec<String>> = HashMap::new();
    let mut ident_centroids: HashMap<String, Position> = HashMap::new();
    let mut disconnected_index: usize = 0;

    for ident in new_idents {
        let connected = connected_existing_positions(state, metadata, ident, new_set);
        if connected.is_empty() {
            // No connections to existing elements: place at periphery,
            // staggering vertically so multiple disconnected inserts don't overlap.
            let periphery_x = bbox_max.x + 150.0;
            let center_y = (bbox_min.y + bbox_max.y) / 2.0;
            let offset_y = disconnected_index as f64 * 80.0;
            disconnected_index += 1;
            result.insert(
                ident.clone(),
                Position::new(periphery_x, center_y + offset_y),
            );
            continue;
        }

        let positions: Vec<Position> = connected.iter().map(|(_, p)| *p).collect();
        let center = centroid(&positions);
        ident_centroids.insert(ident.clone(), center);

        // Build a sorted UID key for grouping elements that share the same
        // connection set, so they can be spread into a ring rather than stacked.
        let mut uid_key: Vec<i32> = connected.iter().map(|(uid, _)| *uid).collect();
        uid_key.sort();
        uid_key.dedup();

        connection_groups
            .entry(uid_key)
            .or_default()
            .push(ident.clone());
    }

    // Place each group, spreading elements in a ring when multiple share
    // the same connection set (AC4.4).
    for group in connection_groups.values() {
        let group_count = group.len();
        for (i, ident) in group.iter().enumerate() {
            let base = ident_centroids
                .get(ident)
                .copied()
                .unwrap_or(Position::new(bbox_max.x + 150.0, bbox_min.y));

            if group_count == 1 {
                // Offset slightly from the centroid so SFDP has non-zero
                // initial displacement. Without this, a new element seeded
                // exactly on its only neighbor gets zero force and stays stacked.
                result.insert(ident.clone(), Position::new(base.x + 50.0, base.y + 30.0));
            } else {
                let angle = i as f64 * 2.0 * PI / group_count.max(8) as f64;
                let radius = 50.0;
                result.insert(
                    ident.clone(),
                    Position::new(base.x + radius * angle.cos(), base.y + radius * angle.sin()),
                );
            }
        }
    }
}

/// Place new stock and flow elements.  When connected to existing
/// structure, place near the connected elements; when disconnected,
/// place at the diagram periphery.
fn place_new_chain_elements(
    state: &LayoutState,
    metadata: &ComputedMetadata,
    new_elements: &NewElements,
    new_set: &HashSet<&str>,
    bbox_max: &Position,
    result: &mut HashMap<String, Position>,
) {
    let offset_x = 100.0;
    let offset_y = 50.0;

    for stock_ident in &new_elements.new_stocks {
        let connected = connected_existing_positions(state, metadata, stock_ident, new_set);
        if connected.is_empty() {
            // Periphery placement
            let pos = Position::new(bbox_max.x + 150.0, bbox_max.y + offset_y);
            result.insert(stock_ident.clone(), pos);
        } else {
            let positions: Vec<Position> = connected.iter().map(|(_, p)| *p).collect();
            let center = centroid(&positions);
            result.insert(
                stock_ident.clone(),
                Position::new(center.x + offset_x, center.y + offset_y),
            );
        }
    }

    for flow_ident in &new_elements.new_flows {
        // A flow between two EXISTING stocks belongs at their midpoint (the valve
        // sits on the pipe between them), not offset to the side -- and it is
        // pinned there during settle (see `settle_new_elements`), because as a
        // free SFDP node with rest length k it would be pushed far from the
        // midpoint whenever the two stocks are closer together than k.
        if let Some(mid) = stock_to_stock_flow_midpoint(state, metadata, flow_ident, new_set) {
            result.insert(flow_ident.clone(), mid);
            continue;
        }
        let connected = connected_existing_positions(state, metadata, flow_ident, new_set);
        if connected.is_empty() {
            let pos = Position::new(bbox_max.x + 200.0, bbox_max.y + offset_y);
            result.insert(flow_ident.clone(), pos);
        } else {
            let positions: Vec<Position> = connected.iter().map(|(_, p)| *p).collect();
            let center = centroid(&positions);
            result.insert(
                flow_ident.clone(),
                Position::new(center.x + offset_x, center.y),
            );
        }
    }
}

/// The midpoint of a flow's two stocks when BOTH already exist (are not new), or
/// `None` otherwise (a cloud flow, or a flow into/out of a new stock, which the
/// chain/rigid-group machinery positions instead). This is the canonical valve
/// position for an incrementally-added stock-to-stock flow.
fn stock_to_stock_flow_midpoint(
    state: &LayoutState,
    metadata: &ComputedMetadata,
    flow_ident: &str,
    new_set: &HashSet<&str>,
) -> Option<Position> {
    let (from_stock, to_stock) = metadata.connected_stocks(flow_ident);
    let from_stock = from_stock?;
    let to_stock = to_stock?;
    if new_set.contains(from_stock) || new_set.contains(to_stock) {
        return None;
    }
    let a = state
        .uid_manager
        .get_uid(from_stock)
        .and_then(|uid| state.positions.get(&uid).copied())?;
    let b = state
        .uid_manager
        .get_uid(to_stock)
        .and_then(|uid| state.positions.get(&uid).copied())?;
    Some(chain::stock_pair_valve_position(a, b, 0, 1))
}

/// Run SFDP + annealing with existing elements pinned and only new
/// elements free to move. This settles new elements into positions
/// that respect the force-directed layout while preserving all
/// existing element positions exactly.
pub fn settle_new_elements(
    state: &mut LayoutState,
    config: &LayoutConfig,
    model: &datamodel::Model,
    metadata: &ComputedMetadata,
    new_elements: &NewElements,
    chains_data: &[(Vec<String>, Vec<String>, Vec<String>)],
) -> Result<(), String> {
    if new_elements.is_empty() {
        return Ok(());
    }

    let new_ident_set: HashSet<&str> = new_elements
        .new_stocks
        .iter()
        .chain(&new_elements.new_flows)
        .chain(&new_elements.new_auxes)
        .chain(&new_elements.new_modules)
        .map(|s| s.as_str())
        .collect();

    // Isolated variables are excluded from the force graph (see
    // `build_full_graph`), which on this incremental path means they simply
    // stay where `compute_new_element_positions` placed them -- no parking
    // pass, since incremental layout's contract is minimal disturbance.
    let FullGraph {
        graph: full_graph,
        var_to_node,
        isolated_vars: _,
    } = build_full_graph(state, model, metadata)?;

    // Build constrained graph: pin existing elements, make new chains rigid groups
    let mut constrained_builder = ConstrainedGraphBuilder::new(full_graph);

    // Pin all existing (non-new) nodes, plus any NEW flow that connects two
    // existing stocks: its valve is fixed at the stock midpoint
    // (`stock_to_stock_flow_midpoint`), so letting it float as an SFDP node
    // (rest length k) would push it far off whenever the stocks are closer than
    // k. The rest of a genuinely new chain still settles normally.
    let mut pinned_node_ids: Vec<String> = var_to_node
        .iter()
        .filter(|(ident, _)| !new_ident_set.contains(ident.as_str()))
        .map(|(_, node_id)| node_id.clone())
        .collect();
    for flow_ident in &new_elements.new_flows {
        if stock_to_stock_flow_midpoint(state, metadata, flow_ident, &new_ident_set).is_some()
            && let Some(node_id) = var_to_node.get(flow_ident)
        {
            pinned_node_ids.push(node_id.clone());
        }
    }
    constrained_builder.pin(&pinned_node_ids);

    // Add rigid groups for new chain elements (same pattern as run_sfdp_with_rigid_chains)
    for (_stocks, _flows, all_vars) in chains_data {
        let mut group_members: Vec<String> = Vec::new();
        let mut added: HashSet<String> = HashSet::new();

        for var_ident in all_vars {
            if !new_ident_set.contains(var_ident.as_str()) {
                continue;
            }
            if let Some(node_id) = var_to_node.get(var_ident)
                && added.insert(node_id.clone())
            {
                group_members.push(node_id.clone());

                let canonical = canonicalize(var_ident);
                if let Some(cloud_idents) = state.flow_ident_to_clouds.get(canonical.as_ref()) {
                    for cloud_ident in cloud_idents {
                        if let Some(cloud_node) = var_to_node.get(cloud_ident)
                            && added.insert(cloud_node.clone())
                        {
                            group_members.push(cloud_node.clone());
                        }
                    }
                }
            }
        }

        if group_members.len() > 1 {
            constrained_builder.add_rigid_group(group_members);
        }
    }

    let constrained_graph = constrained_builder.build();

    // Seed initial positions: existing elements from state.positions,
    // new elements from state.positions (which were set by compute_new_element_positions)
    let mut initial_layout: Layout<String> = BTreeMap::new();
    for (var_ident, node_id) in &var_to_node {
        if let Some(uid) = state.uid_manager.get_uid(var_ident)
            && let Some(&pos) = state.positions.get(&uid)
        {
            initial_layout.insert(node_id.clone(), pos);
            continue;
        }
        if let Some(&cloud_uid) = state.cloud_ident_to_uid.get(var_ident)
            && let Some(&pos) = state.positions.get(&cloud_uid)
        {
            initial_layout.insert(node_id.clone(), pos);
        }
    }

    let sfdp_config = SfdpConfig::for_aux_placement();

    let node_to_ident: HashMap<String, String> = var_to_node
        .iter()
        .map(|(ident, node_id)| (node_id.clone(), ident.clone()))
        .collect();
    let stock_inflows: HashMap<String, HashSet<String>> = metadata
        .stock_to_inflows
        .iter()
        .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
        .collect();
    let stock_outflows: HashMap<String, HashSet<String>> = metadata
        .stock_to_outflows
        .iter()
        .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
        .collect();

    let new_node_ids: HashSet<String> = var_to_node
        .iter()
        .filter(|(ident, _)| new_ident_set.contains(ident.as_str()))
        .map(|(_, node_id)| node_id.clone())
        .collect();

    let build_segments = |candidate_layout: &Layout<String>| -> Vec<LineSegment> {
        let mut segments = Vec::new();

        for edge in constrained_graph.edges() {
            let (Some(&from_pos), Some(&to_pos)) = (
                candidate_layout.get(&edge.from),
                candidate_layout.get(&edge.to),
            ) else {
                continue;
            };

            if let (Some(from_ident), Some(to_ident)) =
                (node_to_ident.get(&edge.from), node_to_ident.get(&edge.to))
                && is_structural_stock_flow(from_ident, to_ident, &stock_inflows, &stock_outflows)
            {
                continue;
            }

            segments.push(LineSegment {
                start: from_pos,
                end: to_pos,
                from_node: edge.from.clone(),
                to_node: edge.to.clone(),
            });
        }

        for (flow_ident, tmpl) in &state.flow_templates {
            if tmpl.offsets.len() < 2 {
                continue;
            }
            let Some(node_id) = var_to_node.get(flow_ident) else {
                continue;
            };
            let Some(&center) = candidate_layout.get(node_id) else {
                continue;
            };

            let points: Vec<Position> = tmpl
                .offsets
                .iter()
                .map(|offset| Position::new(center.x + offset.x, center.y + offset.y))
                .collect();

            for i in 0..points.len() - 1 {
                segments.push(LineSegment {
                    start: points[i],
                    end: points[i + 1],
                    from_node: format!("{}#{}", flow_ident, i),
                    to_node: format!("{}#{}", flow_ident, i + 1),
                });
            }
        }

        segments
    };

    let mut adjacency: annealing::AdjacencyMap<String> = HashMap::new();
    for edge in constrained_graph.edges() {
        adjacency
            .entry(edge.from.clone())
            .or_default()
            .push((edge.to.clone(), edge.weight));
        adjacency
            .entry(edge.to.clone())
            .or_default()
            .push((edge.from.clone(), edge.weight));
    }

    let max_delta_aux = config.annealing_max_delta_aux;
    let annealing_config = config.clone();
    let annealing_seed = config.annealing_random_seed;

    let mut annealing_round: usize = 0;
    let mut last_annealing_iter: usize = 0;
    let mut best_cost: f64 = f64::INFINITY;
    let mut best_layout: Option<Layout<String>> = None;

    let final_layout = compute_layout_from_initial_with_callback(
        &constrained_graph,
        &sfdp_config,
        &initial_layout,
        annealing_seed,
        &mut |iter, layout| {
            if !should_trigger_annealing(
                iter,
                annealing_config.annealing_interval,
                last_annealing_iter,
                annealing_round,
                annealing_config.annealing_max_rounds,
            ) {
                return None;
            }

            let result = run_annealing_with_filter(
                layout,
                build_segments,
                // Incremental settling perturbs only the new elements around
                // pinned existing ones; a new element must still not land on
                // top of another node.
                |layout: &Layout<String>| point_node_pileup_count(layout, &new_node_ids) as f64,
                &annealing_config,
                annealing_seed.wrapping_add(annealing_round as u64),
                |node_id: &String| new_node_ids.contains(node_id),
                |node_id: &String| {
                    if new_node_ids.contains(node_id) {
                        max_delta_aux
                    } else {
                        0.0
                    }
                },
                &adjacency,
            );

            last_annealing_iter = iter;
            annealing_round += 1;

            if result.cost < best_cost {
                best_cost = result.cost;
                best_layout = Some(result.layout.clone());
                Some(result.layout)
            } else {
                None
            }
        },
    );

    let settled_layout = if let Some(saved) = best_layout {
        let final_crossings = annealing::count_crossings(&build_segments(&final_layout));
        if final_crossings as f64 > best_cost {
            saved
        } else {
            final_layout
        }
    } else {
        final_layout
    };

    // Only update positions for new elements; existing elements stay unchanged
    for (var_ident, node_id) in &var_to_node {
        if !new_ident_set.contains(var_ident.as_str()) {
            continue;
        }
        if let Some(&pos) = settled_layout.get(node_id)
            && let Some(uid) = state.uid_manager.get_uid(var_ident)
        {
            state.positions.insert(uid, pos);
        }
    }

    // Also update positions for clouds of new flows.  SFDP moves cloud nodes in a rigid
    // group together with their parent flow, but the loop above skips cloud idents since
    // they are not model variables and therefore not in new_ident_set.  Without recording
    // the settled cloud positions here, the coordinate update loop in incremental_layout
    // cannot apply the flow's displacement to the cloud element, leaving the cloud stranded
    // at its creation position while the flow endpoint shifts.
    for var_ident in var_to_node.keys() {
        if !new_ident_set.contains(var_ident.as_str()) {
            continue;
        }
        let canonical = canonicalize(var_ident);
        if let Some(cloud_idents) = state.flow_ident_to_clouds.get(canonical.as_ref()) {
            for cloud_ident in cloud_idents {
                if let Some(&cloud_uid) = state.cloud_ident_to_uid.get(cloud_ident)
                    && let Some(cloud_node) = var_to_node.get(cloud_ident)
                    && let Some(&pos) = settled_layout.get(cloud_node)
                {
                    state.positions.insert(cloud_uid, pos);
                }
            }
        }
    }

    Ok(())
}

/// Re-snap stock-attached flow endpoints to stock edges after SFDP settlement.
///
/// SFDP may move flow valves while stocks stay pinned, causing the
/// proportional point translation to detach endpoints from their stocks.
/// This function restores each attached endpoint to the correct stock
/// edge, using the flow valve position to determine which face of the
/// stock rectangle the flow approaches from.
pub fn resnap_flow_endpoints(state: &mut LayoutState, config: &LayoutConfig) {
    let stock_positions: HashMap<i32, Position> = state
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some((s.uid, Position::new(s.x, s.y))),
            _ => None,
        })
        .collect();

    let half_w = config.stock_width / 2.0;
    let half_h = config.stock_height / 2.0;

    for elem in &mut state.elements {
        if let ViewElement::Flow(f) = elem {
            let valve = Position::new(f.x, f.y);
            for pt in &mut f.points {
                if let Some(attached_uid) = pt.attached_to_uid
                    && let Some(stock_pos) = stock_positions.get(&attached_uid)
                {
                    let dx = valve.x - stock_pos.x;
                    let dy = valve.y - stock_pos.y;

                    // Determine which face the flow approaches from using
                    // aspect-ratio-normalized comparison of dx vs dy.
                    if half_h * dx.abs() >= half_w * dy.abs() {
                        // Horizontal approach: snap to left or right edge.
                        // Preserve the y position (may be off-center for
                        // multi-flow sides), clamped to stock bounds.
                        pt.x = stock_pos.x + dx.signum() * half_w;
                        pt.y = pt.y.clamp(stock_pos.y - half_h, stock_pos.y + half_h);
                    } else {
                        // Vertical approach: snap to top or bottom edge.
                        // Preserve the x position (may be off-center for
                        // multi-flow sides), clamped to stock bounds.
                        pt.x = pt.x.clamp(stock_pos.x - half_w, stock_pos.x + half_w);
                        pt.y = stock_pos.y + dy.signum() * half_h;
                    }
                }
            }
        }
    }
}

/// Perform three-way connector diff: compare old links in LayoutState
/// against edges derived from the current dep_graph, then preserve
/// unchanged links, remove stale ones, and create new links with
/// default shapes.
pub fn diff_connectors(state: &mut LayoutState, metadata: &ComputedMetadata) {
    // Build HashMap<(from_uid, to_uid), ViewElement> for existing links
    let mut old_links: HashMap<(i32, i32), ViewElement> = HashMap::new();
    for elem in &state.elements {
        if let ViewElement::Link(l) = elem {
            old_links.insert((l.from_uid, l.to_uid), elem.clone());
        }
    }

    // Compute new dependency edges from dep_graph, skipping structural flow-stock edges
    let stock_inflows: HashMap<String, HashSet<String>> = metadata
        .stock_to_inflows
        .iter()
        .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
        .collect();
    let stock_outflows: HashMap<String, HashSet<String>> = metadata
        .stock_to_outflows
        .iter()
        .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
        .collect();

    let mut new_edges: HashSet<(i32, i32)> = HashSet::new();
    let mut new_edge_idents: HashMap<(i32, i32), (String, String)> = HashMap::new();

    for (var, deps) in &metadata.dep_graph {
        for dep in deps {
            let from_ident = dep.as_str();
            let to_ident = var.as_str();

            if is_structural_flow_stock(from_ident, to_ident, &stock_inflows, &stock_outflows) {
                continue;
            }

            let from_uid = match state.uid_manager.get_uid(from_ident) {
                Some(uid) => uid,
                None => continue,
            };
            let to_uid = match state.uid_manager.get_uid(to_ident) {
                Some(uid) => uid,
                None => continue,
            };

            if from_uid != 0 && to_uid != 0 {
                new_edges.insert((from_uid, to_uid));
                new_edge_idents.insert(
                    (from_uid, to_uid),
                    (from_ident.to_string(), to_ident.to_string()),
                );
            }
        }
    }

    // Build alias UID -> primary variable UID mapping so that old links
    // targeting aliases are recognized as semantically equivalent to the
    // primary variable link. Without this, imported views with causal links
    // terminating on aliases would lose those links after an incremental edit.
    let alias_to_primary: HashMap<i32, i32> = state
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Alias(a) => Some((a.uid, a.alias_of_uid)),
            _ => None,
        })
        .collect();

    // Remove all old links from elements
    state
        .elements
        .retain(|elem| !matches!(elem, ViewElement::Link(_)));

    // Track which old links have been consumed so each is used at most once.
    let mut consumed_old_links: HashSet<(i32, i32)> = HashSet::new();

    // Iterate edges in a deterministic order. `new_edges` is a HashSet, so its
    // iteration order is per-process random; since each newly-created link both
    // allocates a sequential `uid` and is appended to `state.elements` in this
    // loop, hash order would otherwise assign different uids / element ordering
    // to the same logical link run-to-run (the incremental analogue of #633).
    let mut sorted_new_edges: Vec<(i32, i32)> = new_edges.iter().copied().collect();
    sorted_new_edges.sort_unstable();

    // Add back preserved links (unchanged) and create new links
    for (from_uid, to_uid) in sorted_new_edges {
        if let Some(old_link) = old_links.get(&(from_uid, to_uid)) {
            // Preserved: keep the old link exactly as-is
            state.elements.push(old_link.clone());
            consumed_old_links.insert((from_uid, to_uid));
        } else if let Some(key) = old_links
            .keys()
            .copied()
            .filter(|&(of, ot)| {
                if consumed_old_links.contains(&(of, ot)) {
                    return false;
                }
                let rf = alias_to_primary.get(&of).copied().unwrap_or(of);
                let rt = alias_to_primary.get(&ot).copied().unwrap_or(ot);
                rf == from_uid && rt == to_uid
            })
            // Pick the lowest matching key so the alias-match selection is
            // deterministic; HashMap iteration order would otherwise vary.
            .min()
        {
            // Preserved via alias: the old link targets an alias whose primary
            // variable matches this dependency edge. Keep the alias link as-is.
            state.elements.push(old_links[&key].clone());
            consumed_old_links.insert(key);
        } else if let Some((from_ident, to_ident)) = new_edge_idents.get(&(from_uid, to_uid)) {
            // Added: create new link with default shape
            let link_uid = state.uid_manager.alloc("");
            let shape = if is_structural_stock_flow(
                from_ident,
                to_ident,
                &stock_inflows,
                &stock_outflows,
            ) {
                let arc_angle = if let (Some(&s_pos), Some(&f_pos)) =
                    (state.positions.get(&from_uid), state.positions.get(&to_uid))
                {
                    calc_stock_flow_arc_angle(s_pos, f_pos)
                } else {
                    -45.0
                };
                LinkShape::Arc(arc_angle)
            } else if metadata
                .dep_graph
                .get(from_ident)
                .is_some_and(|deps| deps.contains(to_ident))
            {
                let arc_angle = if let (Some(&from_pos), Some(&to_pos)) =
                    (state.positions.get(&from_uid), state.positions.get(&to_uid))
                {
                    calc_reciprocal_arc_angle(from_pos, to_pos)
                } else {
                    -45.0
                };
                LinkShape::Arc(arc_angle)
            } else {
                LinkShape::Straight
            };

            state.elements.push(ViewElement::Link(view_element::Link {
                uid: link_uid,
                from_uid,
                to_uid,
                shape,
                polarity: None,
            }));
        }
    }

    // Preserve remaining alias-backed links whose alias-resolved endpoints
    // match a valid dependency. Imported views may have multiple rendered
    // connectors for the same dependency (e.g., links to two different
    // aliases of the same variable).
    // Iterate in a deterministic order for the same reason as the new-edge loop:
    // the preserved links are appended to `state.elements`, so HashMap iteration
    // order would otherwise perturb element ordering run-to-run.
    let mut sorted_old_links: Vec<&(i32, i32)> = old_links.keys().collect();
    sorted_old_links.sort_unstable();
    for &(of, ot) in sorted_old_links {
        if consumed_old_links.contains(&(of, ot)) {
            continue;
        }
        let rf = alias_to_primary.get(&of).copied().unwrap_or(of);
        let rt = alias_to_primary.get(&ot).copied().unwrap_or(ot);
        if new_edges.contains(&(rf, rt)) {
            state.elements.push(old_links[&(of, ot)].clone());
        }
    }
}

/// Diff clouds for all flows: preserve existing clouds that are still
/// needed, remove clouds whose flow endpoint is now connected to a
/// stock, and create new clouds for newly-unconnected flow endpoints.
pub fn diff_clouds(state: &mut LayoutState, metadata: &ComputedMetadata) {
    // Index existing clouds by (flow_uid, is_source).
    // A source cloud is at the first flow point, a sink at the last.
    // We distinguish them by checking their position against the flow
    // element's points when possible, but we can also use a simpler
    // heuristic: group all clouds by flow_uid.
    let mut old_clouds_by_flow: HashMap<i32, Vec<ViewElement>> = HashMap::new();
    for elem in &state.elements {
        if let ViewElement::Cloud(c) = elem {
            old_clouds_by_flow
                .entry(c.flow_uid)
                .or_default()
                .push(elem.clone());
        }
    }

    // Determine which clouds should exist for each flow
    let mut needed_flow_uids: HashSet<i32> = HashSet::new();
    // Track which flows need source/sink clouds
    let mut need_source: HashSet<i32> = HashSet::new();
    let mut need_sink: HashSet<i32> = HashSet::new();

    for (flow_ident, (from_stock, to_stock)) in &metadata.flow_to_stocks {
        let flow_uid = match state.uid_manager.get_uid(flow_ident) {
            Some(uid) => uid,
            None => continue,
        };
        needed_flow_uids.insert(flow_uid);
        if from_stock.is_none() {
            need_source.insert(flow_uid);
        }
        if to_stock.is_none() {
            need_sink.insert(flow_uid);
        }
    }

    // Snapshot flow endpoint positions before mutating state.elements
    let flow_endpoints: HashMap<i32, (Position, Position)> = state
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Flow(f) if !f.points.is_empty() => {
                let first = Position::new(f.points[0].x, f.points[0].y);
                let last_idx = f.points.len() - 1;
                let last = Position::new(f.points[last_idx].x, f.points[last_idx].y);
                Some((f.uid, (first, last)))
            }
            _ => None,
        })
        .collect();

    // Remove all old clouds from elements
    state
        .elements
        .retain(|elem| !matches!(elem, ViewElement::Cloud(_)));

    // For each flow, determine what to keep vs create
    let all_flow_uids: HashSet<i32> = needed_flow_uids
        .iter()
        .chain(old_clouds_by_flow.keys())
        .copied()
        .collect();

    for flow_uid in all_flow_uids {
        let old_clouds = old_clouds_by_flow
            .get(&flow_uid)
            .cloned()
            .unwrap_or_default();
        let wants_source = need_source.contains(&flow_uid);
        let wants_sink = need_sink.contains(&flow_uid);

        let needed_count = wants_source as usize + wants_sink as usize;

        if needed_count == 0 {
            for c in &old_clouds {
                if let ViewElement::Cloud(cloud) = c {
                    state.positions.remove(&cloud.uid);
                }
            }
            continue;
        }

        // Preserve existing clouds by matching to needed roles (source/sink)
        // based on proximity to flow endpoints, rather than iteration order.
        let endpoints = flow_endpoints.get(&flow_uid);
        let mut preserved_source = false;
        let mut preserved_sink = false;
        let mut used_uids: HashSet<i32> = HashSet::new();

        let find_nearest =
            |clouds: &[ViewElement], target: &Position, exclude: &HashSet<i32>| -> Option<i32> {
                clouds
                    .iter()
                    .filter_map(|c| match c {
                        ViewElement::Cloud(cloud) if !exclude.contains(&cloud.uid) => {
                            let d = (cloud.x - target.x).powi(2) + (cloud.y - target.y).powi(2);
                            Some((cloud.uid, d))
                        }
                        _ => None,
                    })
                    .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(uid, _)| uid)
            };

        if let Some((src_pos, snk_pos)) = endpoints {
            if wants_source && let Some(uid) = find_nearest(&old_clouds, src_pos, &used_uids) {
                used_uids.insert(uid);
                preserved_source = true;
            }
            if wants_sink && let Some(uid) = find_nearest(&old_clouds, snk_pos, &used_uids) {
                used_uids.insert(uid);
                preserved_sink = true;
            }
        } else {
            // No endpoint info: preserve in order as a fallback
            for cloud in &old_clouds {
                if let ViewElement::Cloud(c) = cloud {
                    if wants_source && !preserved_source {
                        used_uids.insert(c.uid);
                        preserved_source = true;
                    } else if wants_sink && !preserved_sink {
                        used_uids.insert(c.uid);
                        preserved_sink = true;
                    }
                }
            }
        }

        // Push preserved clouds and remove positions of discarded ones
        for cloud in &old_clouds {
            if let ViewElement::Cloud(c) = cloud {
                if used_uids.contains(&c.uid) {
                    state.elements.push(cloud.clone());
                } else {
                    state.positions.remove(&c.uid);
                }
            }
        }

        // Create new clouds for roles that couldn't be filled from old clouds
        if wants_source && !preserved_source {
            let pos = endpoints.map(|(src, _)| *src);
            let (cx, cy) = pos.map_or((0.0, 0.0), |p| (p.x, p.y));
            let cloud_uid = state.uid_manager.alloc("");
            state.elements.push(ViewElement::Cloud(view_element::Cloud {
                uid: cloud_uid,
                flow_uid,
                x: cx,
                y: cy,
                compat: None,
            }));
            state.positions.insert(cloud_uid, Position::new(cx, cy));
        }
        if wants_sink && !preserved_sink {
            let pos = endpoints.map(|(_, sink)| *sink);
            let (cx, cy) = pos.map_or((0.0, 0.0), |p| (p.x, p.y));
            let cloud_uid = state.uid_manager.alloc("");
            state.elements.push(ViewElement::Cloud(view_element::Cloud {
                uid: cloud_uid,
                flow_uid,
                x: cx,
                y: cy,
                compat: None,
            }));
            state.positions.insert(cloud_uid, Position::new(cx, cy));
        }
    }

    // Repair pass: for XMILE-imported views a cloud element may exist but the
    // corresponding flow point's attached_to_uid may be None.  Wire up any
    // unattached flow endpoints to their matching cloud.
    //
    // Build a map from flow_uid to the clouds that now exist for it.
    let mut clouds_by_flow: HashMap<i32, Vec<(i32, f64, f64)>> = HashMap::new();
    for elem in &state.elements {
        if let ViewElement::Cloud(c) = elem {
            clouds_by_flow
                .entry(c.flow_uid)
                .or_default()
                .push((c.uid, c.x, c.y));
        }
    }

    for elem in &mut state.elements {
        let flow = match elem {
            ViewElement::Flow(f) => f,
            _ => continue,
        };
        let Some(clouds) = clouds_by_flow.get(&flow.uid) else {
            continue;
        };
        if flow.points.len() < 2 {
            continue;
        }

        // For each flow endpoint (source=0, sink=last) that is unattached,
        // assign the nearest cloud.  We use a simple squared-distance heuristic
        // which is correct for both single-cloud and two-cloud cases.
        let last = flow.points.len() - 1;
        for pt_idx in [0, last] {
            if flow.points[pt_idx].attached_to_uid.is_some() {
                continue;
            }
            let px = flow.points[pt_idx].x;
            let py = flow.points[pt_idx].y;
            let nearest = clouds.iter().min_by(|(_, ax, ay), (_, bx, by)| {
                let da = (ax - px).powi(2) + (ay - py).powi(2);
                let db = (bx - px).powi(2) + (by - py).powi(2);
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            });
            if let Some(&(cloud_uid, _, _)) = nearest {
                flow.points[pt_idx].attached_to_uid = Some(cloud_uid);
            }
        }
    }
}

/// The face of `stock` each drawn side flow (one attached to the stock at a
/// single end) currently sits on: the face its stock-attached endpoint lies
/// on, by the aspect-normalized rule `resnap_flow_endpoints` uses.
fn existing_side_flow_faces(
    state: &LayoutState,
    config: &LayoutConfig,
    metadata: &ComputedMetadata,
    stock_ident: &str,
) -> HashMap<String, StockAttachSide> {
    let mut faces = HashMap::new();
    let Some(stock_uid) = state.uid_manager.get_uid(stock_ident) else {
        return faces;
    };
    let Some(&stock_pos) = state.positions.get(&stock_uid) else {
        return faces;
    };
    let side_flows = metadata
        .stock_to_outflows
        .get(stock_ident)
        .into_iter()
        .chain(metadata.stock_to_inflows.get(stock_ident))
        .flatten()
        .filter(|flow| {
            let (from, to) = metadata.connected_stocks(flow);
            from.is_none() || to.is_none()
        });
    for flow_ident in side_flows {
        let Some(uid) = state.uid_manager.get_uid(flow_ident) else {
            continue;
        };
        let attached = state.elements.iter().find_map(|e| match e {
            ViewElement::Flow(f) if f.uid == uid => f
                .points
                .iter()
                .find(|pt| pt.attached_to_uid == Some(stock_uid))
                .map(|pt| (pt.x, pt.y)),
            _ => None,
        });
        let Some((x, y)) = attached else { continue };
        let (dx, dy) = (x - stock_pos.x, y - stock_pos.y);
        let half_w = config.stock_width / 2.0;
        let half_h = config.stock_height / 2.0;
        let side = if half_h * dx.abs() >= half_w * dy.abs() {
            if dx >= 0.0 {
                StockAttachSide::Right
            } else {
                StockAttachSide::Left
            }
        } else if dy >= 0.0 {
            StockAttachSide::Bottom
        } else {
            StockAttachSide::Top
        };
        faces.insert(flow_ident.clone(), side);
    }
    faces
}

/// Re-sort flows on each affected stock's sides by their existing
/// attachment position rather than alphabetical ident.  This preserves
/// the visual left-to-right (or top-to-bottom) ordering of imported or
/// manually-edited flows when a sibling is added or removed.
///
/// Only affects flows that already have view elements in `state`;
/// new flows without positions are placed last (sorted by ident among
/// themselves).
fn reorder_attachments_by_position(
    attachments: &mut HashMap<String, FlowAttachment>,
    state: &LayoutState,
    affected_stocks: &HashSet<String>,
    metadata: &ComputedMetadata,
) {
    for stock_ident in affected_stocks {
        let stock_uid = match state.uid_manager.get_uid(stock_ident) {
            Some(uid) => uid,
            None => continue,
        };

        // Group flows on this stock by side, recording each flow's
        // existing attachment position (x for Top/Bottom, y for Left/Right).
        let mut by_side: HashMap<StockAttachSide, Vec<(String, f64)>> = HashMap::new();

        for (flow_ident, att) in attachments.iter() {
            let (from, to) = metadata.connected_stocks(flow_ident);
            // Skip stock-to-stock flows: their attachment side depends on
            // which stock classified them last, so including them would
            // count them on the wrong side of one stock.
            if from.is_some() && to.is_some() {
                continue;
            }
            let connected =
                from.is_some_and(|s| s == stock_ident) || to.is_some_and(|s| s == stock_ident);
            if !connected {
                continue;
            }

            let pos_key = state
                .uid_manager
                .get_uid(flow_ident)
                .and_then(|uid| {
                    state.elements.iter().find_map(|e| match e {
                        ViewElement::Flow(f) if f.uid == uid => f
                            .points
                            .iter()
                            .find(|pt| pt.attached_to_uid == Some(stock_uid))
                            .map(|pt| match att.side {
                                StockAttachSide::Bottom | StockAttachSide::Top => pt.x,
                                StockAttachSide::Left | StockAttachSide::Right => pt.y,
                            }),
                        _ => None,
                    })
                })
                .unwrap_or(f64::MAX); // new flows sort last

            by_side
                .entry(att.side)
                .or_default()
                .push((flow_ident.clone(), pos_key));
        }

        // Re-sort each side group by position and reassign offsets
        for flows in by_side.values_mut() {
            if flows.len() <= 1 {
                continue;
            }
            flows.sort_by(|a, b| {
                a.1.partial_cmp(&b.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.0.cmp(&b.0))
            });
            let n = flows.len();
            for (i, (flow_ident, _)) in flows.iter().enumerate() {
                let offset = if n == 1 {
                    0.5
                } else {
                    (i as f64 + 1.0) / (n as f64 + 1.0)
                };
                if let Some(att) = attachments.get_mut(flow_ident) {
                    att.offset = offset;
                }
            }
        }
    }
}

/// Compute the valve position for a flow based on its attachment info and
/// connected stock position.  Returns `None` if the flow has no attachment
/// or the stock position is unknown, in which case the caller should fall
/// back to `initial_positions`.
fn attachment_based_flow_position(
    state: &LayoutState,
    config: &LayoutConfig,
    metadata: &ComputedMetadata,
    flow_ident: &str,
    flow_attachments: &HashMap<String, FlowAttachment>,
) -> Option<Position> {
    let attachment = flow_attachments.get(flow_ident)?;
    let (from_stock, to_stock) = metadata.connected_stocks(flow_ident);
    let stock_name = from_stock.or(to_stock)?;
    let stock_uid = state.uid_manager.get_uid(stock_name)?;
    let stock_pos = state.positions.get(&stock_uid)?;
    Some(side_flow_valve_position(*stock_pos, *attachment, config))
}

/// Write `sides` back onto the named elements that carry those UIDs. Used by
/// incremental layout after a flow is rebuilt with unchanged orientation
/// (`create_flow_view_element` picks a default side) to reinstate the side
/// the element had before the rebuild.
fn restore_label_sides(state: &mut LayoutState, sides: &HashMap<i32, LabelSide>) {
    for elem in &mut state.elements {
        let Some(&side) = sides.get(&elem.get_uid()) else {
            continue;
        };
        match elem {
            ViewElement::Stock(s) => s.label_side = side,
            ViewElement::Flow(f) => f.label_side = side,
            ViewElement::Aux(a) => a.label_side = side,
            ViewElement::Module(m) => m.label_side = side,
            _ => {}
        }
    }
}

/// Assemble a [`datamodel::StockFlow`] from finalized layout state, copying
/// metadata (name, view box, zoom, font, sketch_compat) from `template`.
///
/// The view box is copied verbatim, never recomputed from the elements: the
/// editor stores its viewport there (the pan offset and the canvas size it
/// was last shown at, `Canvas.tsx` `getCanvasOffset`), and an incremental
/// pass runs after every kernel/MCP edit of a diagram someone is looking
/// at -- re-boxing it to the content bounds snaps their pan back and, when
/// the size no longer matches the canvas, triggers the editor's
/// proportional refit, so the diagram jumps on every "Updated from Python".
/// Content that ends up outside the viewport is the editor's business (it
/// re-centres an offscreen diagram on mount). Only a from-scratch layout
/// synthesises a box.
pub(super) fn build_stock_flow_from_state(
    state: LayoutState,
    template: &datamodel::StockFlow,
) -> datamodel::StockFlow {
    datamodel::StockFlow {
        name: template.name.clone(),
        elements: state.elements,
        view_box: template.view_box.clone(),
        zoom: if template.zoom > 0.0 {
            template.zoom
        } else {
            1.0
        },
        use_lettered_polarity: template.use_lettered_polarity,
        font: template.font.clone(),
        sketch_compat: template.sketch_compat.clone(),
    }
}

/// Apply a model patch incrementally to an existing diagram view,
/// preserving existing element positions and only placing new or
/// modified elements.
///
/// The `project` must already reflect the post-patch model state
/// (i.e., `apply_patch` has been called). The `patch` is taken by
/// reference so callers can inspect the operations.
///
/// Contract for elements the patch did not touch: position AND
/// `label_side` are returned byte-for-byte. A label side is chosen only
/// for elements created in this pass -- new variables, kind-changed or
/// endpoint-changed rebuilds, and flows whose pipe orientation flipped.
/// A flow rebuilt merely to slide along the same stock face keeps its
/// side. The optimizer never revisits an existing side, even when a
/// connector added by this patch now crosses the label: hand placement
/// wins, and the human (or a full relayout) can move it.
///
/// Composition:
/// 1. Compute metadata for the post-patch model
/// 2. Seed LayoutState from old view
/// 3. Process deletions and renames from the patch
/// 4. Identify new elements, compute initial positions
/// 5. Create view elements and settle via pinned SFDP
/// 6. Diff connectors/clouds, place labels for this pass's elements,
///    apply loop curvature
/// 7. Build StockFlow from final state
pub fn incremental_layout(
    old_view: &datamodel::StockFlow,
    project: &datamodel::Project,
    model_name: &str,
    patch: &crate::patch::ModelPatch,
    db_state: Option<(&crate::db::SimlinDb, crate::db::SourceProject)>,
) -> Result<datamodel::StockFlow, String> {
    if old_view.elements.is_empty() {
        return generate_best_layout(project, model_name, db_state);
    }

    // View-only patches (UpsertView/DeleteView) don't affect model variables,
    // so the diagram should be returned unchanged. Without this guard, the
    // diff_connectors and optimize_labels passes would rewrite connectors and
    // labels even though nothing structurally changed.
    let has_variable_ops = patch.ops.iter().any(|op| {
        !matches!(
            op,
            crate::patch::ModelOperation::UpsertView { .. }
                | crate::patch::ModelOperation::DeleteView { .. }
        )
    });
    if !has_variable_ops {
        return Ok(old_view.clone());
    }

    let config = LayoutConfig::default();

    let not_found = || format!("model '{}' not found in project", model_name);
    let model = project.get_model(model_name).ok_or_else(not_found)?;
    let metadata = compute_metadata(project, model_name, db_state).ok_or_else(not_found)?;

    // Step 2: Seed state from old view
    let mut state = LayoutState::from_existing_view(old_view, model);

    // Step 3: Process deletions and renames
    for op in &patch.ops {
        match op {
            crate::patch::ModelOperation::DeleteVariable { ident } => {
                state.apply_deletion(ident);
            }
            crate::patch::ModelOperation::RenameVariable { from, to } => {
                let new_display = state
                    .display_names
                    .get(&canonicalize(to).into_owned())
                    .cloned()
                    .unwrap_or_else(|| to.clone());
                state.apply_rename(from, to, &new_display);
            }
            _ => {}
        }
    }

    // Between steps 3 and 4a: detect variables whose type changed (e.g., Aux -> Stock).
    // When a caller issues UpsertStock for a variable that was previously an Aux, there
    // is no DeleteVariable in the patch and the old Aux element is still in state.
    // identify_new_elements only checks for UID presence, not element type, so the
    // stale element would survive.  We detect type mismatches here and remove the
    // old element so it is rebuilt with the correct type.
    {
        let kind_changed: Vec<String> = model
            .variables
            .iter()
            .filter_map(|var| {
                let canonical = canonicalize(var.get_ident()).into_owned();
                let uid = state.uid_manager.get_uid(&canonical)?;
                // Find the view element for this UID
                let elem = state.elements.iter().find(|e| e.get_uid() == uid)?;
                // Check for a type mismatch
                let mismatch = !matches!(
                    (var, elem),
                    (datamodel::Variable::Stock(_), ViewElement::Stock(_))
                        | (datamodel::Variable::Flow(_), ViewElement::Flow(_))
                        | (datamodel::Variable::Aux(_), ViewElement::Aux(_))
                        | (datamodel::Variable::Module(_), ViewElement::Module(_))
                );
                if mismatch { Some(canonical) } else { None }
            })
            .collect();
        for ident in kind_changed {
            // Save the display name before apply_deletion removes it from display_names,
            // so the rebuilt element can recover the original casing (e.g. "Growth Rate"
            // instead of "growth_rate").
            let saved_display = state.display_names.get(&ident).cloned();
            state.apply_deletion(&ident);
            // Restore: use the saved original display name when available, otherwise
            // fall back to the canonical ident so the entry is always present.
            let display = saved_display.unwrap_or_else(|| ident.clone());
            state.display_names.insert(ident, display);
        }
    }

    // Between steps 3 and 4: detect flows whose stock connections changed.
    // A flow element keeps its old attached_to_uid values when preserved in state,
    // so a flow that moved from one stock to another would keep stale endpoints.
    // Remove such flows (and their clouds) so identify_new_elements picks them
    // up as new and they get rebuilt with correct endpoints.
    //
    // This also handles transitions between stock and cloud endpoints: if the
    // model now expects a cloud source (from_stock == None) but the preserved
    // flow's source point is still attached to a stock UID, the flow is stale.
    {
        let uid_to_ident: HashMap<i32, String> = model
            .variables
            .iter()
            .filter_map(|var| {
                let ident = canonicalize(var.get_ident()).into_owned();
                state.uid_manager.get_uid(&ident).map(|uid| (uid, ident))
            })
            .collect();

        // Build the set of cloud UIDs so we can validate cloud-endpoint assignments.
        // When a cloud is expected (expected_from/to == None), the flow endpoint must
        // be either unattached or attached to a cloud.  Checking against cloud_uids
        // (rather than just "not in stock_uids") catches the case where a stock was
        // kind-changed to an aux: the old UID is reused by the new non-stock element,
        // so the flow must be rebuilt with a proper cloud endpoint.
        let cloud_uids: HashSet<i32> = state
            .elements
            .iter()
            .filter_map(|elem| match elem {
                ViewElement::Cloud(c) => Some(c.uid),
                _ => None,
            })
            .collect();

        let flows_to_reset: Vec<String> = state
            .elements
            .iter()
            .filter_map(|elem| {
                let flow = match elem {
                    ViewElement::Flow(f) => f,
                    _ => return None,
                };
                if flow.points.len() < 2 {
                    return None;
                }
                let flow_ident = uid_to_ident.get(&flow.uid)?;
                let (expected_from, expected_to) = metadata.flow_to_stocks.get(flow_ident)?;

                let expected_from_uid = expected_from
                    .as_deref()
                    .and_then(|s| state.uid_manager.get_uid(s));
                let expected_to_uid = expected_to
                    .as_deref()
                    .and_then(|s| state.uid_manager.get_uid(s));

                // Check the source endpoint (points[0]):
                //   - None expected (cloud): endpoint must be unattached or attached to a cloud
                //   - Some(uid) expected: the source must be attached to exactly that stock
                let source_uid = flow.points[0].attached_to_uid;
                let from_matches = match expected_from_uid {
                    None => {
                        source_uid.is_none() || source_uid.is_some_and(|u| cloud_uids.contains(&u))
                    }
                    Some(uid) => source_uid == Some(uid),
                };

                // Check the sink endpoint (points[last]):
                //   - None expected (cloud): endpoint must be unattached or attached to a cloud
                //   - Some(uid) expected: the sink must be attached to exactly that stock
                let last = flow.points.len() - 1;
                let sink_uid = flow.points[last].attached_to_uid;
                let to_matches = match expected_to_uid {
                    None => sink_uid.is_none() || sink_uid.is_some_and(|u| cloud_uids.contains(&u)),
                    Some(uid) => sink_uid == Some(uid),
                };

                if from_matches && to_matches {
                    None
                } else {
                    Some(flow_ident.clone())
                }
            })
            .collect();

        for flow_ident in flows_to_reset {
            // apply_deletion removes the element from state.elements but leaves
            // the UID in uid_manager. identify_new_elements will see a UID with
            // no corresponding element and classify the flow as new, causing
            // create_flow_view_element to rebuild it with correct endpoints.
            let canonical = canonicalize(&flow_ident).into_owned();
            // Save the display name before apply_deletion removes it so the
            // rebuilt element recovers the original casing.
            let saved_display = state.display_names.get(&canonical).cloned();
            state.apply_deletion(&flow_ident);
            let display = saved_display.unwrap_or_else(|| flow_ident.clone());
            state.display_names.insert(canonical, display);
        }
    }

    // Step 4: Identify new elements and compute initial positions
    let new_elements = state.identify_new_elements(model);

    // Compute flow attachments for flows on stocks that are affected by
    // flow additions, deletions, or connection changes.  This ensures
    // preserved flows get reclassified when a sibling chain flow is
    // added or removed.
    let mut incr_flow_attachments: HashMap<String, FlowAttachment> = HashMap::new();
    let mut affected_stocks: HashSet<String> = HashSet::new();

    for flow_ident in &new_elements.new_flows {
        let (from_stock, to_stock) = metadata.connected_stocks(flow_ident);
        if let Some(stock) = from_stock {
            affected_stocks.insert(stock.to_string());
        }
        if let Some(stock) = to_stock {
            affected_stocks.insert(stock.to_string());
        }
    }

    // Also mark stocks whose flow connections changed via the patch
    // (e.g. when a chain flow is deleted, the stock loses a flow and
    // remaining cloud flows may need reclassification from Bottom/Top
    // back to Right/Left).
    for op in &patch.ops {
        if let crate::patch::ModelOperation::UpdateStockFlows { ident, .. } = op {
            let canonical = canonicalize(ident).into_owned();
            affected_stocks.insert(canonical);
        }
    }

    // For deleted flows, find which stocks they were connected to in the
    // old view. This handles patches that only emit DeleteVariable without
    // UpdateStockFlows -- the remaining sibling flows still need to be
    // reclassified.
    // Build UID-to-ident map from the model's stock variables rather than
    // from view element labels, since labels go through
    // format_label_with_line_breaks and may not round-trip through
    // canonicalize for quoted names like "a.b".
    let stock_uid_to_ident: HashMap<i32, String> = model
        .variables
        .iter()
        .filter_map(|v| {
            if !matches!(v, datamodel::Variable::Stock(_)) {
                return None;
            }
            let canonical = canonicalize(v.get_ident()).into_owned();
            state
                .uid_manager
                .get_uid(&canonical)
                .map(|uid| (uid, canonical))
        })
        .collect();
    for op in &patch.ops {
        if let crate::patch::ModelOperation::DeleteVariable { ident } = op {
            let canonical = canonicalize(ident).into_owned();
            // Match by UID rather than display name: labels go through
            // format_label_with_line_breaks which strips quoting, so
            // canonicalizing the label back can produce a different ident
            // for names like "a.b".
            let deleted_uid = match state.uid_manager.get_uid(&canonical) {
                Some(uid) => uid,
                None => continue,
            };
            for elem in &old_view.elements {
                if let ViewElement::Flow(f) = elem
                    && f.uid == deleted_uid
                {
                    for pt in &f.points {
                        if let Some(uid) = pt.attached_to_uid
                            && let Some(stock_ident) = stock_uid_to_ident.get(&uid)
                        {
                            affected_stocks.insert(stock_ident.clone());
                        }
                    }
                }
            }
        }
    }

    // The faces the affected stocks' side flows are drawn on before the patch.
    let mut existing_faces: HashMap<String, StockAttachSide> = HashMap::new();
    for stock in &affected_stocks {
        let existing = existing_side_flow_faces(&state, &config, &metadata, stock);
        let sides = classify_flow_sides(stock, &metadata, &existing);
        incr_flow_attachments.extend(sides);
        existing_faces.extend(existing);
    }

    // Re-sort flows within each side group by existing position rather
    // than alphabetical ident, so imported or manually-edited ordering
    // is preserved when a sibling is added or removed.
    reorder_attachments_by_position(
        &mut incr_flow_attachments,
        &state,
        &affected_stocks,
        &metadata,
    );

    // Check if any existing (preserved) flows need to change sides.
    // If classify_flow_sides assigns Bottom/Top to a flow that is
    // currently horizontal (or Right/Left to one that is vertical),
    // delete and rebuild it so its geometry matches.
    let mut flows_to_rebuild: Vec<String> = Vec::new();
    // Label sides of flows rebuilt only to move along the same stock face:
    // their pipe keeps its orientation, so the existing (possibly hand-placed)
    // side stays valid and is restored after the rebuild. Flows whose
    // orientation flips are rebuilt with a freshly chosen side instead.
    let mut offset_rebuilt_label_sides: HashMap<String, LabelSide> = HashMap::new();
    for (flow_ident, attachment) in &incr_flow_attachments {
        // Skip flows that are new (they'll be created below)
        if new_elements.new_flows.contains(flow_ident) {
            continue;
        }
        // Skip stock-to-stock (chain) flows entirely: their pipe geometry
        // is determined by both stock positions and ignores the attachment
        // offset.  Rebuilding them via attachment_based_flow_position (which
        // only knows one stock) would place the valve beside one stock
        // instead of between the pair.
        let (from_stock, to_stock) = metadata.connected_stocks(flow_ident);
        if from_stock.is_some() && to_stock.is_some() {
            continue;
        }
        // Check if this flow exists and has mismatched orientation or offset
        if let Some(uid) = state.uid_manager.get_uid(flow_ident) {
            let existing = state.elements.iter().find(|e| {
                if let ViewElement::Flow(f) = e {
                    f.uid == uid
                } else {
                    false
                }
            });
            if let Some(ViewElement::Flow(f)) = existing {
                let orientation = compute_flow_orientation(&f.points);
                let needs_vertical = matches!(
                    attachment.side,
                    StockAttachSide::Bottom | StockAttachSide::Top
                );
                let is_vertical = matches!(orientation, FlowOrientation::Vertical);
                if needs_vertical != is_vertical {
                    flows_to_rebuild.push(flow_ident.clone());
                } else if existing_faces
                    .get(flow_ident)
                    .is_some_and(|&side| side != attachment.side)
                {
                    // Moved to the opposite face (top <-> bottom, left <->
                    // right): the pipe keeps its orientation, so the label
                    // side stays valid.
                    flows_to_rebuild.push(flow_ident.clone());
                    offset_rebuilt_label_sides.insert(flow_ident.clone(), f.label_side);
                } else {
                    // Orientation matches but the offset may have changed
                    // (e.g. a sibling was added/removed on the same face).
                    let stock_name = from_stock.or(to_stock);
                    if let Some(sn) = stock_name
                        && let Some(stock_uid) = state.uid_manager.get_uid(sn)
                        && let Some(&stock_pos) = state.positions.get(&stock_uid)
                    {
                        let (expected, current) = if needs_vertical {
                            let exp = stock_pos.x - config.stock_width / 2.0
                                + config.stock_width * attachment.offset;
                            let cur = f
                                .points
                                .iter()
                                .find(|pt| pt.attached_to_uid == Some(stock_uid))
                                .map(|pt| pt.x);
                            (exp, cur)
                        } else {
                            let exp = stock_pos.y - config.stock_height / 2.0
                                + config.stock_height * attachment.offset;
                            let cur = f
                                .points
                                .iter()
                                .find(|pt| pt.attached_to_uid == Some(stock_uid))
                                .map(|pt| pt.y);
                            (exp, cur)
                        };
                        if let Some(c) = current
                            && (c - expected).abs() > 0.5
                        {
                            flows_to_rebuild.push(flow_ident.clone());
                            offset_rebuilt_label_sides.insert(flow_ident.clone(), f.label_side);
                        }
                    }
                }
            }
        }
    }

    // Save old positions before deletion so we have a fallback if
    // attachment_based_flow_position can't resolve the stock UID
    // (e.g. imported views with quoted identifiers).
    let old_flow_positions: HashMap<String, Position> = flows_to_rebuild
        .iter()
        .filter_map(|ident| {
            let uid = state.uid_manager.get_uid(ident)?;
            state.positions.get(&uid).map(|&pos| (ident.clone(), pos))
        })
        .collect();

    // Delete and rebuild flows that need to change orientation or offset
    for flow_ident in &flows_to_rebuild {
        let saved_display = state
            .display_names
            .get(&canonicalize(flow_ident).into_owned())
            .cloned();
        state.apply_deletion(flow_ident);
        if let Some(display) = saved_display {
            state
                .display_names
                .insert(canonicalize(flow_ident).into_owned(), display);
        }
    }

    // Every named element still standing at this point survived the patch
    // untouched (or was merely renamed): its label side is pinned for the rest
    // of the pass. Whatever gets created from here on -- new variables,
    // kind-changed or endpoint-changed rebuilds, orientation-flipped flows --
    // is absent from this snapshot and has its side chosen by
    // `optimize_labels_for` below. Offset-only rebuilt flows are added back
    // explicitly because they were just deleted but keep their orientation.
    let mut pinned_label_sides: HashMap<i32, LabelSide> = state
        .elements
        .iter()
        .filter_map(|elem| match elem {
            ViewElement::Stock(s) => Some((s.uid, s.label_side)),
            ViewElement::Flow(f) => Some((f.uid, f.label_side)),
            ViewElement::Aux(a) => Some((a.uid, a.label_side)),
            ViewElement::Module(m) => Some((m.uid, m.label_side)),
            _ => None,
        })
        .collect();
    for (flow_ident, side) in &offset_rebuilt_label_sides {
        if let Some(uid) = state.uid_manager.get_uid(flow_ident) {
            pinned_label_sides.insert(uid, *side);
        }
    }

    // Compute positions for rebuilt flows based on their attachment info,
    // falling back to the old position if the stock UID lookup fails.
    for flow_ident in &flows_to_rebuild {
        let pos = attachment_based_flow_position(
            &state,
            &config,
            &metadata,
            flow_ident,
            &incr_flow_attachments,
        )
        .or_else(|| old_flow_positions.get(flow_ident).copied());
        if let Some(pos) = pos {
            let uid = state.get_or_alloc_uid(flow_ident);
            create_flow_view_element(
                &mut state,
                &config,
                &metadata,
                flow_ident,
                uid,
                pos,
                &incr_flow_attachments,
            )?;
        }
    }

    restore_label_sides(&mut state, &pinned_label_sides);
    let needs_label_placement = |uid: i32| !pinned_label_sides.contains_key(&uid);

    if new_elements.is_empty() {
        // No new elements and no settlement step, so rebuilt flows
        // already have correct geometry from create_flow_view_element.
        // Skip resnap entirely to avoid rewriting unrelated manual or
        // imported flow endpoints elsewhere in the diagram.
        diff_connectors(&mut state, &metadata);
        diff_clouds(&mut state, &metadata);
        optimize_labels_for(&mut state, model, &metadata, needs_label_placement);
        apply_loop_curvature(&mut state, &config, model, &metadata);
        validate_view_completeness(&state, model)?;
        return Ok(build_stock_flow_from_state(state, old_view));
    }

    let initial_positions = compute_new_element_positions(&state, &metadata, &new_elements);

    // Step 5: Create view elements for new variables and insert their
    // initial positions into state so settlement can find them.
    for stock_ident in &new_elements.new_stocks {
        if let Some(&pos) = initial_positions.get(stock_ident) {
            let uid = state.get_or_alloc_uid(stock_ident);
            let name = state.display_name(stock_ident);
            let formatted = format_label_with_line_breaks(&name);
            state.elements.push(ViewElement::Stock(view_element::Stock {
                name: formatted,
                uid,
                x: pos.x,
                y: pos.y,
                label_side: LabelSide::Bottom,
                compat: None,
            }));
            state.positions.insert(uid, pos);
        }
    }

    for flow_ident in &new_elements.new_flows {
        // For stock-to-stock (chain) flows, use the generic seed position
        // which places the valve between the two stocks. For cloud flows
        // (one unattached end), use attachment-based position so top/bottom
        // flows get their valve on the correct vertical pipe.
        let (from_stock, to_stock) = metadata.connected_stocks(flow_ident);
        let is_stock_to_stock = from_stock.is_some() && to_stock.is_some();
        let pos = if is_stock_to_stock {
            initial_positions.get(flow_ident).copied()
        } else {
            attachment_based_flow_position(
                &state,
                &config,
                &metadata,
                flow_ident,
                &incr_flow_attachments,
            )
            .or_else(|| initial_positions.get(flow_ident).copied())
        };
        if let Some(pos) = pos {
            let uid = state.get_or_alloc_uid(flow_ident);
            create_flow_view_element(
                &mut state,
                &config,
                &metadata,
                flow_ident,
                uid,
                pos,
                &incr_flow_attachments,
            )?;
        }
    }

    // create_flow_view_element calls build_clouds_for_flow which pushes Cloud elements into
    // state.elements but does not add their positions to state.positions.  Record those
    // positions now so that settle_new_elements can seed proper initial positions for cloud
    // nodes in SFDP and later update them after settling.
    for elem in &state.elements {
        if let ViewElement::Cloud(c) = elem {
            state
                .positions
                .entry(c.uid)
                .or_insert_with(|| Position::new(c.x, c.y));
        }
    }

    for aux_ident in &new_elements.new_auxes {
        if let Some(&pos) = initial_positions.get(aux_ident) {
            let uid = state.get_or_alloc_uid(aux_ident);
            let name = state.display_name(aux_ident);
            let formatted = format_label_with_line_breaks(&name);
            state.elements.push(ViewElement::Aux(view_element::Aux {
                name: formatted,
                uid,
                x: pos.x,
                y: pos.y,
                label_side: LabelSide::Bottom,
                compat: None,
            }));
            state.positions.insert(uid, pos);
        }
    }

    for module_ident in &new_elements.new_modules {
        if let Some(&pos) = initial_positions.get(module_ident) {
            let uid = state.get_or_alloc_uid(module_ident);
            let name = state.display_name(module_ident);
            let formatted = format_label_with_line_breaks(&name);
            state
                .elements
                .push(ViewElement::Module(view_element::Module {
                    name: formatted,
                    uid,
                    x: pos.x,
                    y: pos.y,
                    label_side: LabelSide::Bottom,
                }));
            state.positions.insert(uid, pos);
        }
    }

    // Step 6: Settle new elements with existing elements pinned
    let chains_data: Vec<_> = metadata
        .chains
        .iter()
        .map(|c| (c.stocks.clone(), c.flows.clone(), c.all_vars.clone()))
        .collect();
    settle_new_elements(
        &mut state,
        &config,
        model,
        &metadata,
        &new_elements,
        &chains_data,
    )?;

    // Update view element coordinates from settled positions
    for elem in &mut state.elements {
        let uid = elem.get_uid();
        if let Some(&pos) = state.positions.get(&uid) {
            match elem {
                ViewElement::Stock(s) => {
                    s.x = pos.x;
                    s.y = pos.y;
                }
                ViewElement::Flow(f) => {
                    let dx = pos.x - f.x;
                    let dy = pos.y - f.y;
                    f.x = pos.x;
                    f.y = pos.y;
                    for pt in &mut f.points {
                        pt.x += dx;
                        pt.y += dy;
                    }
                }
                ViewElement::Aux(a) => {
                    a.x = pos.x;
                    a.y = pos.y;
                }
                ViewElement::Module(m) => {
                    m.x = pos.x;
                    m.y = pos.y;
                }
                ViewElement::Cloud(c) => {
                    c.x = pos.x;
                    c.y = pos.y;
                }
                _ => {}
            }
        }
    }

    resnap_flow_endpoints(&mut state, &config);

    // Step 7: Diff connectors and clouds
    diff_connectors(&mut state, &metadata);
    diff_clouds(&mut state, &metadata);

    // Step 8: Polish. Only elements created in this pass get a label side
    // chosen; pinned elements keep theirs even if a new connector now runs
    // through the label (hand placement wins; the human can move it).
    optimize_labels_for(&mut state, model, &metadata, needs_label_placement);
    apply_loop_curvature(&mut state, &config, model, &metadata);
    // Guarantee flows stay orthogonal after re-snapping endpoints to moved
    // stocks (only rewrites pipes that actually went diagonal; hand-routed
    // orthogonal flows are left untouched).
    orthogonal::orthogonalize_flow_pipes(&mut state.elements);

    validate_view_completeness(&state, model)?;

    // Step 9: Build StockFlow
    Ok(build_stock_flow_from_state(state, old_view))
}
