// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

pub mod annealing;
pub mod chain;
pub mod config;
pub mod connector;
pub mod graph;
pub mod metadata;
pub mod placement;
pub mod sfdp;
pub mod text;
pub mod uid;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::f64::consts::PI;

use self::annealing::{FlowTemplate, LineSegment, run_annealing_with_filter};
use self::chain::{DIAGRAM_ORIGIN_MARGIN, compute_chain_positions, make_cloud_node_ident};
use self::config::LayoutConfig;
use self::connector::{
    FlowOrientation, calc_stock_flow_arc_angle, calculate_loop_arc_angle, compute_flow_orientation,
};
use self::graph::{ConstrainedGraphBuilder, Graph, GraphBuilder, Layout, Position};
use self::metadata::{ComputedMetadata, LoopPolarity, StockFlowChain};
use self::placement::{
    calculate_optimal_label_side, calculate_restricted_label_side, normalize_coordinates,
};
use self::sfdp::{SfdpConfig, compute_layout_from_initial_with_callback, should_trigger_annealing};
use self::text::{estimate_label_bounds, format_label_with_line_breaks};
use self::uid::UidManager;
use crate::common::{Ident, canonicalize};
use crate::datamodel;
use crate::datamodel::view_element::{self, FlowPoint, LabelSide, LinkShape};
use crate::datamodel::{Rect, ViewElement};

/// A queued element during chain layout BFS traversal.
struct WorkItem {
    id: String,
    item_type: WorkItemType,
    position: Position,
    connected_to: String,
}

#[derive(Clone, Copy, PartialEq)]
enum WorkItemType {
    Stock,
    Flow,
}

/// Result of a single layout generation, used to select the best among parallel attempts.
struct LayoutResult {
    view: datamodel::StockFlow,
    crossings: usize,
    seed: u64,
}

/// Shared mutable state for layout generation, separated from immutable
/// context so it can be passed to standalone layout functions independently.
pub struct LayoutState {
    pub uid_manager: UidManager,

    /// Canonical ident -> original display name (pre-built for O(1) lookup).
    pub display_names: HashMap<String, String>,

    pub elements: Vec<ViewElement>,
    pub positions: HashMap<i32, Position>,

    pub flow_templates: HashMap<String, FlowTemplate>,
    pub cloud_ident_to_uid: HashMap<String, i32>,
    pub cloud_ident_to_flow_ident: HashMap<String, String>,
    pub flow_ident_to_clouds: HashMap<String, Vec<String>>,
}

impl LayoutState {
    pub fn new(model: &datamodel::Model) -> Self {
        let mut uid_manager = UidManager::new();
        let mut display_names = HashMap::new();

        for var in &model.variables {
            let ident = var.get_ident();
            let canonical = canonicalize(ident).into_owned();
            display_names.insert(canonical, ident.to_string());

            // Seed the UID manager from existing model variable UIDs
            if let Some(uid) = match var {
                datamodel::Variable::Stock(s) => s.uid,
                datamodel::Variable::Flow(f) => f.uid,
                datamodel::Variable::Aux(a) => a.uid,
                datamodel::Variable::Module(m) => m.uid,
            } {
                uid_manager.add(uid, ident);
            }
        }

        Self {
            uid_manager,
            display_names,
            elements: Vec::new(),
            positions: HashMap::new(),
            flow_templates: HashMap::new(),
            cloud_ident_to_uid: HashMap::new(),
            cloud_ident_to_flow_ident: HashMap::new(),
            flow_ident_to_clouds: HashMap::new(),
        }
    }

    /// Seed all layout state from an existing diagram view, enabling
    /// incremental layout to preserve existing element positions.
    pub fn from_existing_view(old_view: &datamodel::StockFlow, model: &datamodel::Model) -> Self {
        let mut uid_manager = UidManager::new();
        let mut display_names = HashMap::new();
        let mut positions = HashMap::new();
        let mut flow_templates = HashMap::new();
        let mut cloud_ident_to_uid = HashMap::new();
        let mut cloud_ident_to_flow_ident = HashMap::new();
        let mut flow_ident_to_clouds: HashMap<String, Vec<String>> = HashMap::new();

        // Seed display names and UID manager from model variables
        for var in &model.variables {
            let ident = var.get_ident();
            let canonical = canonicalize(ident).into_owned();
            display_names.insert(canonical, ident.to_string());

            if let Some(uid) = match var {
                datamodel::Variable::Stock(s) => s.uid,
                datamodel::Variable::Flow(f) => f.uid,
                datamodel::Variable::Aux(a) => a.uid,
                datamodel::Variable::Module(m) => m.uid,
            } {
                uid_manager.add(uid, ident);
            }
        }

        // Seed UID manager from view elements and extract positions
        for elem in &old_view.elements {
            match elem {
                ViewElement::Aux(a) => {
                    uid_manager.add(a.uid, &canonicalize(&a.name));
                    positions.insert(a.uid, Position::new(a.x, a.y));
                }
                ViewElement::Stock(s) => {
                    uid_manager.add(s.uid, &canonicalize(&s.name));
                    positions.insert(s.uid, Position::new(s.x, s.y));
                }
                ViewElement::Flow(f) => {
                    uid_manager.add(f.uid, &canonicalize(&f.name));
                    positions.insert(f.uid, Position::new(f.x, f.y));
                }
                ViewElement::Module(m) => {
                    uid_manager.add(m.uid, &canonicalize(&m.name));
                    positions.insert(m.uid, Position::new(m.x, m.y));
                }
                ViewElement::Group(g) => {
                    uid_manager.add(g.uid, &canonicalize(&g.name));
                    positions.insert(g.uid, Position::new(g.x, g.y));
                }
                ViewElement::Cloud(c) => {
                    uid_manager.add(c.uid, "");
                    positions.insert(c.uid, Position::new(c.x, c.y));
                }
                ViewElement::Link(l) => {
                    uid_manager.add(l.uid, "");
                }
                ViewElement::Alias(a) => {
                    uid_manager.add(a.uid, "");
                    positions.insert(a.uid, Position::new(a.x, a.y));
                }
            }
        }

        // Build uid-to-ident map using the uid_manager (which was seeded from
        // view elements above), not from model variable UIDs which may be None
        // for XMILE-parsed models.
        let uid_to_ident: HashMap<i32, String> = model
            .variables
            .iter()
            .filter_map(|var| {
                let ident = canonicalize(var.get_ident()).into_owned();
                let uid = uid_manager.get_uid(&ident)?;
                Some((uid, ident))
            })
            .collect();

        // Populate flow_templates from existing flow elements
        for elem in &old_view.elements {
            if let ViewElement::Flow(f) = elem
                && let Some(ident) = uid_to_ident.get(&f.uid)
                && f.points.len() >= 2
            {
                let offsets: Vec<Position> = f
                    .points
                    .iter()
                    .map(|pt| Position::new(pt.x - f.x, pt.y - f.y))
                    .collect();
                flow_templates.insert(ident.clone(), FlowTemplate { offsets });
            }
        }

        // Populate cloud maps from existing cloud elements
        for elem in &old_view.elements {
            if let ViewElement::Cloud(c) = elem {
                let cloud_ident = make_cloud_node_ident(c.uid);
                if let Some(flow_ident) = uid_to_ident.get(&c.flow_uid) {
                    cloud_ident_to_uid.insert(cloud_ident.clone(), c.uid);
                    cloud_ident_to_flow_ident.insert(cloud_ident.clone(), flow_ident.clone());
                    flow_ident_to_clouds
                        .entry(flow_ident.clone())
                        .or_default()
                        .push(cloud_ident);
                }
            }
        }

        Self {
            uid_manager,
            display_names,
            elements: old_view.elements.clone(),
            positions,
            flow_templates,
            cloud_ident_to_uid,
            cloud_ident_to_flow_ident,
            flow_ident_to_clouds,
        }
    }

    /// Get or allocate a UID for a variable by its canonical ident.
    pub fn get_or_alloc_uid(&mut self, ident: &str) -> i32 {
        self.uid_manager.alloc(ident)
    }

    /// Get the display name for a variable, preferring the original case.
    pub fn display_name(&self, canonical_ident: &str) -> String {
        self.display_names
            .get(canonical_ident)
            .cloned()
            .unwrap_or_else(|| canonical_ident.to_string())
    }

    /// Remove all view elements associated with a deleted variable:
    /// the primary element, any links referencing it, any clouds for
    /// the flow, and internal cloud bookkeeping maps.
    pub fn apply_deletion(&mut self, deleted_ident: &str) {
        let canonical = canonicalize(deleted_ident);
        let deleted_uid = match self.uid_manager.get_uid(&canonical) {
            Some(uid) => uid,
            None => return,
        };

        self.positions.remove(&deleted_uid);

        // Collect UIDs of clouds being removed so we can clean up their positions
        let removed_cloud_uids: Vec<i32> = self
            .elements
            .iter()
            .filter_map(|elem| match elem {
                ViewElement::Cloud(c) if c.flow_uid == deleted_uid => Some(c.uid),
                _ => None,
            })
            .collect();

        self.elements.retain(|elem| match elem {
            ViewElement::Aux(a) if a.uid == deleted_uid => false,
            ViewElement::Stock(s) if s.uid == deleted_uid => false,
            ViewElement::Flow(f) if f.uid == deleted_uid => false,
            ViewElement::Module(m) if m.uid == deleted_uid => false,
            ViewElement::Link(l) if l.from_uid == deleted_uid || l.to_uid == deleted_uid => false,
            ViewElement::Cloud(c) if c.flow_uid == deleted_uid => false,
            _ => true,
        });

        for cloud_uid in &removed_cloud_uids {
            self.positions.remove(cloud_uid);
        }

        // Clean up cloud bookkeeping for the deleted flow
        let canonical_str = canonical.into_owned();
        if let Some(cloud_idents) = self.flow_ident_to_clouds.remove(&canonical_str) {
            for ci in &cloud_idents {
                self.cloud_ident_to_uid.remove(ci);
                self.cloud_ident_to_flow_ident.remove(ci);
            }
        }
        self.display_names.remove(&canonical_str);
    }
}

/// Pick a starting stock for chain layout. Returns the stock with the
/// highest flow connectivity (inflows + outflows), breaking ties
/// alphabetically for determinism.
fn pick_starting_stock<'b>(metadata: &ComputedMetadata, stocks: &'b [String]) -> Option<&'b str> {
    stocks
        .iter()
        .max_by(|a, b| {
            let a_count = metadata
                .stock_to_inflows
                .get(a.as_str())
                .map_or(0, |v| v.len())
                + metadata
                    .stock_to_outflows
                    .get(a.as_str())
                    .map_or(0, |v| v.len());
            let b_count = metadata
                .stock_to_inflows
                .get(b.as_str())
                .map_or(0, |v| v.len())
                + metadata
                    .stock_to_outflows
                    .get(b.as_str())
                    .map_or(0, |v| v.len());
            a_count.cmp(&b_count).then_with(|| b.cmp(a))
        })
        .map(|s| s.as_str())
}

/// Add cloud elements for flow endpoints that don't connect to a stock.
fn build_clouds_for_flow(
    state: &mut LayoutState,
    metadata: &ComputedMetadata,
    flow_ident: &str,
    flow_elem: &mut view_element::Flow,
) {
    let (from_stock, to_stock) = metadata.connected_stocks(flow_ident);
    let has_from = from_stock.is_some();
    let has_to = to_stock.is_some();

    // Source cloud (no from stock)
    if !has_from && !flow_elem.points.is_empty() {
        let cx = flow_elem.points[0].x;
        let cy = flow_elem.points[0].y;
        let cloud_uid = state.uid_manager.alloc("");
        let cloud = ViewElement::Cloud(view_element::Cloud {
            uid: cloud_uid,
            flow_uid: flow_elem.uid,
            x: cx,
            y: cy,
            compat: None,
        });
        state.elements.push(cloud);
        flow_elem.points[0].attached_to_uid = Some(cloud_uid);
    }

    // Sink cloud (no to stock)
    if !has_to && !flow_elem.points.is_empty() {
        let last_idx = flow_elem.points.len() - 1;
        let cx = flow_elem.points[last_idx].x;
        let cy = flow_elem.points[last_idx].y;
        let cloud_uid = state.uid_manager.alloc("");
        let cloud = ViewElement::Cloud(view_element::Cloud {
            uid: cloud_uid,
            flow_uid: flow_elem.uid,
            x: cx,
            y: cy,
            compat: None,
        });
        state.elements.push(cloud);
        flow_elem.points[last_idx].attached_to_uid = Some(cloud_uid);
    }
}

/// Cache a flow's polyline offsets (relative to valve center) for crossing detection.
fn record_flow_template(state: &mut LayoutState, flow_ident: &str, flow_elem: &view_element::Flow) {
    if flow_elem.points.len() < 2 {
        return;
    }
    let offsets: Vec<Position> = flow_elem
        .points
        .iter()
        .map(|pt| Position::new(pt.x - flow_elem.x, pt.y - flow_elem.y))
        .collect();
    state
        .flow_templates
        .insert(flow_ident.to_string(), FlowTemplate { offsets });
}

/// Create a single flow view element with its flow points and clouds.
fn create_flow_view_element(
    state: &mut LayoutState,
    config: &LayoutConfig,
    metadata: &ComputedMetadata,
    flow_ident: &str,
    uid: i32,
    pos: Position,
) -> Result<(), String> {
    let (from_stock, to_stock) = metadata.connected_stocks(flow_ident);
    let from_stock = from_stock.map(|s| s.to_string());
    let to_stock = to_stock.map(|s| s.to_string());
    let name = state.display_name(flow_ident);
    let formatted = format_label_with_line_breaks(&name);

    let flow_points = match (from_stock.as_deref(), to_stock.as_deref()) {
        (Some(from), Some(to)) => {
            let from_uid = state.get_or_alloc_uid(from);
            let to_uid = state.get_or_alloc_uid(to);
            let from_pos = state
                .positions
                .get(&from_uid)
                .copied()
                .unwrap_or(Position::new(pos.x - 50.0, pos.y));
            let to_pos = state
                .positions
                .get(&to_uid)
                .copied()
                .unwrap_or(Position::new(pos.x + 50.0, pos.y));
            vec![
                FlowPoint {
                    x: from_pos.x + config.stock_width / 2.0,
                    y: pos.y,
                    attached_to_uid: Some(from_uid),
                },
                FlowPoint {
                    x: to_pos.x - config.stock_width / 2.0,
                    y: pos.y,
                    attached_to_uid: Some(to_uid),
                },
            ]
        }
        (Some(from), None) => {
            let from_uid = state.get_or_alloc_uid(from);
            let from_pos = state
                .positions
                .get(&from_uid)
                .copied()
                .unwrap_or(Position::new(pos.x - 50.0, pos.y));
            vec![
                FlowPoint {
                    x: from_pos.x + config.stock_width / 2.0,
                    y: pos.y,
                    attached_to_uid: Some(from_uid),
                },
                FlowPoint {
                    x: pos.x + 50.0,
                    y: pos.y,
                    attached_to_uid: None,
                },
            ]
        }
        (None, Some(to)) => {
            let to_uid = state.get_or_alloc_uid(to);
            let to_pos = state
                .positions
                .get(&to_uid)
                .copied()
                .unwrap_or(Position::new(pos.x + 50.0, pos.y));
            vec![
                FlowPoint {
                    x: pos.x - 50.0,
                    y: pos.y,
                    attached_to_uid: None,
                },
                FlowPoint {
                    x: to_pos.x - config.stock_width / 2.0,
                    y: pos.y,
                    attached_to_uid: Some(to_uid),
                },
            ]
        }
        (None, None) => {
            vec![
                FlowPoint {
                    x: pos.x - 50.0,
                    y: pos.y,
                    attached_to_uid: None,
                },
                FlowPoint {
                    x: pos.x + 50.0,
                    y: pos.y,
                    attached_to_uid: None,
                },
            ]
        }
    };

    let orientation = compute_flow_orientation(&flow_points);
    let label_side = match orientation {
        FlowOrientation::Horizontal => LabelSide::Top,
        FlowOrientation::Vertical => LabelSide::Left,
    };

    let mut flow_elem = view_element::Flow {
        name: formatted,
        uid,
        x: pos.x,
        y: pos.y,
        label_side,
        points: flow_points,
        compat: None,
        label_compat: None,
    };

    // Add clouds for missing stock endpoints
    build_clouds_for_flow(state, metadata, flow_ident, &mut flow_elem);

    // Record flow template for crossing detection
    record_flow_template(state, flow_ident, &flow_elem);

    state.elements.push(ViewElement::Flow(flow_elem));
    state.positions.insert(uid, pos);

    Ok(())
}

/// Convert positioned stock/flow identifiers into ViewElements.
fn create_view_elements(
    state: &mut LayoutState,
    config: &LayoutConfig,
    metadata: &ComputedMetadata,
    positioned: &HashMap<String, Position>,
    stocks: &[String],
    flows: &[String],
) -> Result<(), String> {
    // Create stock view elements
    for stock_ident in stocks {
        if let Some(&pos) = positioned.get(stock_ident) {
            let uid = state.get_or_alloc_uid(stock_ident);
            let name = state.display_name(stock_ident);
            let formatted = format_label_with_line_breaks(&name);
            let elem = ViewElement::Stock(view_element::Stock {
                name: formatted,
                uid,
                x: pos.x,
                y: pos.y,
                label_side: LabelSide::Bottom,
                compat: None,
            });
            state.elements.push(elem);
            state.positions.insert(uid, pos);
        }
    }

    // Create flow view elements
    for flow_ident in flows {
        if let Some(&pos) = positioned.get(flow_ident) {
            let uid = state.get_or_alloc_uid(flow_ident);
            create_flow_view_element(state, config, metadata, flow_ident, uid, pos)?;
        }
    }

    Ok(())
}

/// Layout a single chain at the given base position using BFS.
fn layout_chain(
    state: &mut LayoutState,
    config: &LayoutConfig,
    metadata: &ComputedMetadata,
    stocks: &[String],
    flows: &[String],
    base_position: Position,
) -> Result<(), String> {
    if stocks.is_empty() && flows.is_empty() {
        return Ok(());
    }

    let start_stock = match pick_starting_stock(metadata, stocks) {
        Some(s) => s.to_string(),
        None => {
            // Flow-only chain (no stocks). Place flows at base_position.
            for flow_ident in flows {
                let uid = state.get_or_alloc_uid(flow_ident);
                create_flow_view_element(state, config, metadata, flow_ident, uid, base_position)?;
            }
            return Ok(());
        }
    };

    let mut positioned: HashMap<String, Position> = HashMap::new();
    positioned.insert(start_stock.clone(), base_position);

    let mut queue = VecDeque::from([WorkItem {
        id: start_stock.clone(),
        item_type: WorkItemType::Stock,
        position: base_position,
        connected_to: String::new(),
    }]);

    while let Some(item) = queue.pop_front() {
        match item.item_type {
            WorkItemType::Stock => {
                // First-positioned-wins: if this stock was already placed
                // (via a different BFS path), keep its existing position
                // to preserve the order in which chains are laid out.
                if !positioned.contains_key(&item.id) {
                    positioned.insert(item.id.clone(), item.position);
                }

                let stock_pos = positioned[&item.id];

                // Find inflows for this stock
                let inflows = metadata
                    .stock_to_inflows
                    .get(&item.id)
                    .cloned()
                    .unwrap_or_default();
                for inflow_id in &inflows {
                    if !positioned.contains_key(inflow_id) {
                        queue.push_back(WorkItem {
                            id: inflow_id.clone(),
                            item_type: WorkItemType::Flow,
                            position: stock_pos,
                            connected_to: item.id.clone(),
                        });
                    }
                }

                // Find outflows for this stock
                let outflows = metadata
                    .stock_to_outflows
                    .get(&item.id)
                    .cloned()
                    .unwrap_or_default();
                for outflow_id in &outflows {
                    if !positioned.contains_key(outflow_id) {
                        queue.push_back(WorkItem {
                            id: outflow_id.clone(),
                            item_type: WorkItemType::Flow,
                            position: stock_pos,
                            connected_to: item.id.clone(),
                        });
                    }
                }
            }
            WorkItemType::Flow => {
                if positioned.contains_key(&item.id) {
                    continue;
                }

                let (from_stock, to_stock) = metadata.connected_stocks(&item.id);

                let flow_pos = match (from_stock, to_stock) {
                    (Some(from), Some(to)) => {
                        let from = from.to_string();
                        let to = to.to_string();
                        if item.connected_to == from {
                            // Position sink stock to the right
                            if !positioned.contains_key(&to) {
                                let other_pos = Position::new(
                                    item.position.x
                                        + config.stock_width
                                        + config.horizontal_spacing,
                                    item.position.y,
                                );
                                positioned.insert(to.clone(), other_pos);
                                queue.push_back(WorkItem {
                                    id: to.clone(),
                                    item_type: WorkItemType::Stock,
                                    position: other_pos,
                                    connected_to: String::new(),
                                });
                            }
                            Position::new(
                                (item.position.x + positioned[&to].x) / 2.0,
                                item.position.y,
                            )
                        } else {
                            // Position source stock to the left
                            if !positioned.contains_key(&from) {
                                let other_pos = Position::new(
                                    item.position.x
                                        - config.stock_width
                                        - config.horizontal_spacing,
                                    item.position.y,
                                );
                                positioned.insert(from.clone(), other_pos);
                                queue.push_back(WorkItem {
                                    id: from.clone(),
                                    item_type: WorkItemType::Stock,
                                    position: other_pos,
                                    connected_to: String::new(),
                                });
                            }
                            Position::new(
                                (positioned[&from].x + item.position.x) / 2.0,
                                item.position.y,
                            )
                        }
                    }
                    (Some(_from), None) => {
                        // Outflow to cloud
                        Position::new(
                            item.position.x
                                + config.stock_width / 2.0
                                + config.horizontal_spacing / 2.0,
                            item.position.y,
                        )
                    }
                    (None, Some(_to)) => {
                        // Inflow from cloud
                        Position::new(
                            item.position.x
                                - config.stock_width / 2.0
                                - config.horizontal_spacing / 2.0,
                            item.position.y,
                        )
                    }
                    (None, None) => {
                        // Cloud-to-cloud
                        item.position
                    }
                };

                positioned.insert(item.id.clone(), flow_pos);
            }
        }
    }

    // Convert positioned elements to view elements
    create_view_elements(state, config, metadata, &positioned, stocks, flows)
}

/// Rebuild flow templates from current view elements.
fn refresh_flow_templates(state: &mut LayoutState, model: &datamodel::Model) {
    state.flow_templates.clear();

    let uid_to_ident: HashMap<i32, String> = model
        .variables
        .iter()
        .filter_map(|var| match var {
            datamodel::Variable::Flow(f) => {
                let ident = canonicalize(&f.ident).into_owned();
                state.uid_manager.get_uid(&ident).map(|uid| (uid, ident))
            }
            _ => None,
        })
        .collect();

    for elem in &state.elements {
        if let ViewElement::Flow(flow_elem) = elem
            && let Some(ident) = uid_to_ident.get(&flow_elem.uid)
            && flow_elem.points.len() >= 2
        {
            let offsets: Vec<Position> = flow_elem
                .points
                .iter()
                .map(|pt| Position::new(pt.x - flow_elem.x, pt.y - flow_elem.y))
                .collect();
            state
                .flow_templates
                .insert(ident.clone(), FlowTemplate { offsets });
        }
    }
}

/// Build an undirected graph with all model variables and cloud nodes for SFDP.
fn build_full_graph(
    state: &mut LayoutState,
    model: &datamodel::Model,
    metadata: &ComputedMetadata,
) -> Result<(Graph<String>, HashMap<String, String>), String> {
    state.cloud_ident_to_uid.clear();
    state.cloud_ident_to_flow_ident.clear();
    state.flow_ident_to_clouds.clear();

    let flow_uid_to_ident: HashMap<i32, String> = model
        .variables
        .iter()
        .filter_map(|var| match var {
            datamodel::Variable::Flow(f) => {
                let ident = canonicalize(&f.ident).into_owned();
                state.uid_manager.get_uid(&ident).map(|uid| (uid, ident))
            }
            _ => None,
        })
        .collect();

    let mut var_to_node: HashMap<String, String> = HashMap::new();
    let mut node_to_var: HashMap<String, String> = HashMap::new();
    let mut builder = GraphBuilder::<String>::new_undirected();
    let mut node_index = 0;

    let all_vars: BTreeSet<String> = metadata
        .dep_graph
        .keys()
        .chain(metadata.dep_graph.values().flat_map(|deps| deps.iter()))
        .cloned()
        .collect();

    for var_ident in &all_vars {
        let node_id = format!("node_{}", node_index);
        var_to_node.insert(var_ident.clone(), node_id.clone());
        node_to_var.insert(node_id.clone(), var_ident.clone());
        builder.add_node(node_id);
        node_index += 1;
    }

    for (from_ident, deps) in &metadata.dep_graph {
        if let Some(from_node) = var_to_node.get(from_ident) {
            for to_ident in deps {
                if let Some(to_node) = var_to_node.get(to_ident) {
                    builder.add_edge(from_node.clone(), to_node.clone(), 1.0);
                }
            }
        }
    }

    for elem in &state.elements {
        if let ViewElement::Cloud(cloud) = elem {
            let flow_ident = match flow_uid_to_ident.get(&cloud.flow_uid) {
                Some(ident) => ident.clone(),
                None => {
                    return Err(format!(
                        "build_full_graph: cloud {} references unknown flow UID {}",
                        cloud.uid, cloud.flow_uid
                    ));
                }
            };

            let cloud_ident = make_cloud_node_ident(cloud.uid);
            if !var_to_node.contains_key(&cloud_ident) {
                let node_id = format!("node_{}", node_index);
                builder.add_node(node_id.clone());
                var_to_node.insert(cloud_ident.clone(), node_id.clone());
                node_to_var.insert(node_id, cloud_ident.clone());
                node_index += 1;
            }

            let flow_node = var_to_node.get(&flow_ident).ok_or_else(|| {
                format!("build_full_graph: missing node for flow '{}'", flow_ident)
            })?;
            let cloud_node = var_to_node[&cloud_ident].clone();
            builder.add_edge(flow_node.clone(), cloud_node, 1.0);

            state
                .cloud_ident_to_uid
                .insert(cloud_ident.clone(), cloud.uid);
            state
                .cloud_ident_to_flow_ident
                .insert(cloud_ident.clone(), flow_ident.clone());
            state
                .flow_ident_to_clouds
                .entry(flow_ident)
                .or_default()
                .push(cloud_ident);
        }
    }

    Ok((builder.build(), var_to_node))
}

/// Run SFDP with chain elements locked into rigid groups.
fn run_sfdp_with_rigid_chains(
    state: &LayoutState,
    config: &LayoutConfig,
    model: &datamodel::Model,
    metadata: &ComputedMetadata,
    full_graph: Graph<String>,
    chains_data: &[(Vec<String>, Vec<String>, Vec<String>)],
    var_to_node: &HashMap<String, String>,
) -> Result<Layout<String>, String> {
    let mut constrained_builder = ConstrainedGraphBuilder::new(full_graph);

    for (_stocks, _flows, all_vars) in chains_data {
        let mut group_members: Vec<String> = Vec::new();
        let mut added: HashSet<String> = HashSet::new();

        for var_ident in all_vars {
            if let Some(node_id) = var_to_node.get(var_ident) {
                let uid = state.uid_manager.get_uid(var_ident);
                let is_positioned = uid.is_some_and(|u| state.positions.contains_key(&u));
                if is_positioned && added.insert(node_id.clone()) {
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
        }

        if group_members.len() > 1 {
            constrained_builder.add_rigid_group(group_members);
        }
    }

    let constrained_graph = constrained_builder.build();

    let mut initial_layout: Layout<String> = BTreeMap::new();
    let cloud_uid_to_pos: HashMap<i32, Position> = state
        .elements
        .iter()
        .filter_map(|elem| {
            if let ViewElement::Cloud(cloud) = elem {
                Some((cloud.uid, Position::new(cloud.x, cloud.y)))
            } else {
                None
            }
        })
        .collect();

    let mut center_x = config.start_x;
    let mut center_y = config.start_y;
    let mut count = 0;

    for (var_ident, node_id) in var_to_node {
        if let Some(uid) = state.uid_manager.get_uid(var_ident)
            && let Some(&pos) = state.positions.get(&uid)
        {
            initial_layout.insert(node_id.clone(), pos);
            center_x += pos.x;
            center_y += pos.y;
            count += 1;
            continue;
        }

        if let Some(&cloud_uid) = state.cloud_ident_to_uid.get(var_ident) {
            if let Some(&pos) = cloud_uid_to_pos.get(&cloud_uid) {
                initial_layout.insert(node_id.clone(), pos);
                continue;
            }
            if let Some(flow_ident) = state.cloud_ident_to_flow_ident.get(var_ident)
                && let Some(flow_uid) = state.uid_manager.get_uid(flow_ident)
                && let Some(&pos) = state.positions.get(&flow_uid)
            {
                initial_layout.insert(node_id.clone(), pos);
                continue;
            }
        }
    }

    // Average the accumulated positions with the start position (which was
    // used as the initial accumulator value) to bias the center toward the
    // configured origin when few nodes are positioned.
    if count > 0 {
        center_x /= (count + 1) as f64;
        center_y /= (count + 1) as f64;
    }

    let mut aux_index = 0;
    for node_id in var_to_node.values() {
        if initial_layout.contains_key(node_id) {
            continue;
        }
        let angle = aux_index as f64 * 2.0 * PI / 8.0;
        let radius = 100.0;
        initial_layout.insert(
            node_id.clone(),
            Position::new(
                center_x + radius * angle.cos(),
                center_y + radius * angle.sin(),
            ),
        );
        aux_index += 1;
    }

    // Match Praxis `runSFDPWithAnnealing`: only K/C/MaxIter/CoolFactor
    // are overridden for auxiliary layout. Other SFDP parameters stay at
    // their defaults (notably p=-1.0 and step size=0.1) to avoid runaway
    // repulsion that can fling disconnected chains far apart.
    let sfdp_config = SfdpConfig {
        k: 75.0,
        max_iterations: 5000,
        convergence_threshold: 0.001,
        initial_step_size: 0.1,
        cooling_factor: 0.9995,
        c: 3.0,
        ..SfdpConfig::default()
    };

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
    let aux_node_ids: HashSet<String> = model
        .variables
        .iter()
        .filter_map(|var| {
            if let datamodel::Variable::Aux(aux) = var {
                let canonical = canonicalize(&aux.ident);
                var_to_node.get(canonical.as_ref()).cloned()
            } else {
                None
            }
        })
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
    let max_delta_chain = config.annealing_max_delta_chain;
    let annealing_config = config.clone();
    let annealing_seed = config.annealing_random_seed;

    let mut annealing_round: usize = 0;
    let mut last_annealing_iter: usize = 0;
    let mut best_crossings: usize = usize::MAX;
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
                &annealing_config,
                annealing_seed.wrapping_add(annealing_round as u64),
                |node_id: &String| aux_node_ids.contains(node_id),
                |node_id: &String| {
                    if aux_node_ids.contains(node_id) {
                        max_delta_aux
                    } else {
                        max_delta_chain
                    }
                },
                &adjacency,
            );

            last_annealing_iter = iter;
            annealing_round += 1;

            if result.crossings < best_crossings {
                best_crossings = result.crossings;
                best_layout = Some(result.layout.clone());
                Some(result.layout)
            } else {
                None
            }
        },
    );

    // If SFDP drifted after a good annealing round, the final layout
    // may be worse than the best we found. Compare and keep the better one.
    if let Some(saved) = best_layout {
        let final_crossings = annealing::count_crossings(&build_segments(&final_layout));
        if final_crossings > best_crossings {
            return Ok(saved);
        }
    }

    Ok(final_layout)
}

/// Update all element coordinates from SFDP results.
fn apply_layout_positions(
    state: &mut LayoutState,
    model: &datamodel::Model,
    layout: &Layout<String>,
    var_to_node: &HashMap<String, String>,
) -> Result<(), String> {
    let layout_by_ident: HashMap<String, Position> = var_to_node
        .iter()
        .filter_map(|(ident, node_id)| layout.get(node_id).map(|&pos| (ident.clone(), pos)))
        .collect();

    let uid_to_ident: HashMap<i32, String> = model
        .variables
        .iter()
        .filter_map(|var| {
            let ident = canonicalize(var.get_ident()).into_owned();
            state.uid_manager.get_uid(&ident).map(|uid| (uid, ident))
        })
        .collect();

    let mut flow_deltas: HashMap<i32, Position> = HashMap::new();

    for elem in &mut state.elements {
        match elem {
            ViewElement::Stock(stock) => {
                if let Some(ident) = uid_to_ident.get(&stock.uid)
                    && let Some(&pos) = layout_by_ident.get(ident)
                {
                    stock.x = pos.x;
                    stock.y = pos.y;
                    state.positions.insert(stock.uid, pos);
                }
            }
            ViewElement::Flow(flow) => {
                if let Some(ident) = uid_to_ident.get(&flow.uid)
                    && let Some(&pos) = layout_by_ident.get(ident)
                {
                    let dx = pos.x - flow.x;
                    let dy = pos.y - flow.y;
                    if dx != 0.0 || dy != 0.0 {
                        for pt in &mut flow.points {
                            pt.x += dx;
                            pt.y += dy;
                        }
                    }
                    flow.x = pos.x;
                    flow.y = pos.y;
                    state.positions.insert(flow.uid, pos);
                    flow_deltas.insert(flow.uid, Position::new(dx, dy));
                }
            }
            ViewElement::Aux(aux) => {
                if let Some(ident) = uid_to_ident.get(&aux.uid)
                    && let Some(&pos) = layout_by_ident.get(ident)
                {
                    aux.x = pos.x;
                    aux.y = pos.y;
                    state.positions.insert(aux.uid, pos);
                }
            }
            ViewElement::Module(module) => {
                if let Some(ident) = uid_to_ident.get(&module.uid)
                    && let Some(&pos) = layout_by_ident.get(ident)
                {
                    module.x = pos.x;
                    module.y = pos.y;
                    state.positions.insert(module.uid, pos);
                }
            }
            ViewElement::Cloud(cloud) => {
                let cloud_ident = make_cloud_node_ident(cloud.uid);
                if let Some(&pos) = layout_by_ident.get(&cloud_ident) {
                    cloud.x = pos.x;
                    cloud.y = pos.y;
                } else if let Some(&delta) = flow_deltas.get(&cloud.flow_uid) {
                    cloud.x += delta.x;
                    cloud.y += delta.y;
                }
            }
            _ => {}
        }
    }

    Ok(())
}

/// Create auxiliary view elements for variables not yet in the elements list.
fn create_missing_auxiliary_elements(
    state: &mut LayoutState,
    model: &datamodel::Model,
    layout: &Layout<String>,
    var_to_node: &HashMap<String, String>,
) -> Result<(), String> {
    let existing_uids: HashSet<i32> = state.elements.iter().map(|e| e.get_uid()).collect();

    for var in &model.variables {
        if let datamodel::Variable::Aux(aux) = var {
            let canonical = canonicalize(&aux.ident);
            let uid = state.uid_manager.alloc(&canonical);
            if existing_uids.contains(&uid) {
                continue;
            }

            let pos = var_to_node
                .get(canonical.as_ref())
                .and_then(|node_id| layout.get(node_id))
                .copied()
                .ok_or_else(|| {
                    format!(
                        "create_missing_auxiliary_elements: no layout position for aux '{}'",
                        canonical.as_ref()
                    )
                })?;

            let name = state.display_name(&canonical);
            let formatted = format_label_with_line_breaks(&name);
            let elem = ViewElement::Aux(view_element::Aux {
                name: formatted,
                uid,
                x: pos.x,
                y: pos.y,
                label_side: LabelSide::Bottom,
                compat: None,
            });
            state.elements.push(elem);
            state.positions.insert(uid, pos);
        }
    }
    Ok(())
}

/// Create module view elements for variables not yet in the elements list.
fn create_missing_module_elements(
    state: &mut LayoutState,
    model: &datamodel::Model,
    layout: &Layout<String>,
    var_to_node: &HashMap<String, String>,
) -> Result<(), String> {
    let existing_uids: HashSet<i32> = state.elements.iter().map(|e| e.get_uid()).collect();

    for var in &model.variables {
        if let datamodel::Variable::Module(m) = var {
            let canonical = canonicalize(&m.ident);
            let uid = state.uid_manager.alloc(&canonical);
            if existing_uids.contains(&uid) {
                continue;
            }

            let pos = var_to_node
                .get(canonical.as_ref())
                .and_then(|node_id| layout.get(node_id))
                .copied()
                .ok_or_else(|| {
                    format!(
                        "create_missing_module_elements: no layout position for module '{}'",
                        canonical.as_ref()
                    )
                })?;

            let name = state.display_name(&canonical);
            let formatted = format_label_with_line_breaks(&name);
            let elem = ViewElement::Module(view_element::Module {
                name: formatted,
                uid,
                x: pos.x,
                y: pos.y,
                label_side: LabelSide::Bottom,
            });
            state.elements.push(elem);
            state.positions.insert(uid, pos);
        }
    }
    Ok(())
}

/// Position auxiliaries using SFDP with rigid chain groups (steps 1-6 of
/// the auxiliary placement pipeline).
fn place_auxiliaries(
    state: &mut LayoutState,
    config: &LayoutConfig,
    model: &datamodel::Model,
    metadata: &ComputedMetadata,
    chains_data: &[(Vec<String>, Vec<String>, Vec<String>)],
) -> Result<(), String> {
    refresh_flow_templates(state, model);

    let (full_graph, var_to_node) = build_full_graph(state, model, metadata)?;

    if full_graph.node_count() == 0 {
        return Ok(());
    }

    let layout = run_sfdp_with_rigid_chains(
        state,
        config,
        model,
        metadata,
        full_graph,
        chains_data,
        &var_to_node,
    )?;

    apply_layout_positions(state, model, &layout, &var_to_node)?;

    create_missing_auxiliary_elements(state, model, &layout, &var_to_node)?;

    create_missing_module_elements(state, model, &layout, &var_to_node)?;

    Ok(())
}

/// Create link view elements for all non-structural dependency edges.
fn build_connectors(
    state: &mut LayoutState,
    model: &datamodel::Model,
    metadata: &ComputedMetadata,
) -> Result<(), String> {
    let mut link_set: HashSet<String> = HashSet::new();

    let model_var_idents: HashSet<String> = model
        .variables
        .iter()
        .map(|v| canonicalize(v.get_ident()).into_owned())
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

    let dep_entries: Vec<(String, Vec<String>)> = metadata
        .dep_graph
        .iter()
        .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
        .collect();

    for (dependent_ident, dependencies) in &dep_entries {
        for dependency_ident in dependencies {
            // Metadata stores var -> dependencies, but view connectors are
            // dependency -> dependent.
            let from_ident = dependency_ident.as_str();
            let to_ident = dependent_ident.as_str();

            if is_structural_flow_stock(from_ident, to_ident, &stock_inflows, &stock_outflows) {
                continue;
            }

            let link_key = format!("{}->{}", from_ident, to_ident);
            if !link_set.insert(link_key) {
                continue;
            }

            let from_uid = match state.uid_manager.get_uid(from_ident) {
                Some(uid) => uid,
                None => {
                    if model_var_idents.contains(from_ident) {
                        return Err(format!(
                            "create_connectors: missing UID for model variable '{}'",
                            from_ident
                        ));
                    }
                    continue;
                }
            };
            let to_uid = match state.uid_manager.get_uid(to_ident) {
                Some(uid) => uid,
                None => {
                    if model_var_idents.contains(to_ident) {
                        return Err(format!(
                            "create_connectors: missing UID for model variable '{}'",
                            to_ident
                        ));
                    }
                    continue;
                }
            };

            if from_uid == 0 || to_uid == 0 {
                return Err(format!(
                    "create_connectors: invalid UID 0 in edge {} -> {}",
                    from_ident, to_ident
                ));
            }

            let link_uid = state.uid_manager.alloc("");
            let mut shape = LinkShape::Straight;

            if is_structural_stock_flow(from_ident, to_ident, &stock_inflows, &stock_outflows) {
                let arc_angle = if let (Some(&s_pos), Some(&f_pos)) =
                    (state.positions.get(&from_uid), state.positions.get(&to_uid))
                {
                    calc_stock_flow_arc_angle(s_pos, f_pos)
                } else {
                    -45.0
                };
                shape = LinkShape::Arc(arc_angle);
            }

            let link = ViewElement::Link(view_element::Link {
                uid: link_uid,
                from_uid,
                to_uid,
                shape,
                polarity: None,
            });
            state.elements.push(link);
        }
    }

    Ok(())
}

/// Determine which sides are available for label placement on a stock,
/// excluding sides where flows are attached.
fn calculate_allowed_label_sides_for_stock(
    state: &LayoutState,
    metadata: &ComputedMetadata,
    stock_ident: &str,
) -> Vec<LabelSide> {
    let stock_uid = match state.uid_manager.get_uid(stock_ident) {
        Some(uid) => uid,
        None => {
            return vec![
                LabelSide::Top,
                LabelSide::Bottom,
                LabelSide::Left,
                LabelSide::Right,
            ];
        }
    };
    let stock_pos = match state.positions.get(&stock_uid) {
        Some(&pos) => pos,
        None => {
            return vec![
                LabelSide::Top,
                LabelSide::Bottom,
                LabelSide::Left,
                LabelSide::Right,
            ];
        }
    };

    let mut blocked = [false; 4]; // top, bottom, left, right

    let all_flows: Vec<String> = metadata
        .stock_to_inflows
        .get(stock_ident)
        .into_iter()
        .chain(metadata.stock_to_outflows.get(stock_ident))
        .flat_map(|v| v.iter())
        .cloned()
        .collect();

    for flow_ident in &all_flows {
        if let Some(flow_uid) = state.uid_manager.get_uid(flow_ident)
            && let Some(&flow_pos) = state.positions.get(&flow_uid)
        {
            let dx = flow_pos.x - stock_pos.x;
            let dy = flow_pos.y - stock_pos.y;
            if dx.abs() >= dy.abs() {
                if dx >= 0.0 {
                    blocked[3] = true; // right
                } else {
                    blocked[2] = true; // left
                }
            } else if dy >= 0.0 {
                blocked[1] = true; // bottom
            } else {
                blocked[0] = true; // top
            }
        }
    }

    let sides = [
        LabelSide::Top,
        LabelSide::Bottom,
        LabelSide::Left,
        LabelSide::Right,
    ];
    let allowed: Vec<LabelSide> = sides
        .iter()
        .enumerate()
        .filter(|(i, _)| !blocked[*i])
        .map(|(_, &s)| s)
        .collect();

    if allowed.is_empty() {
        vec![
            LabelSide::Top,
            LabelSide::Bottom,
            LabelSide::Left,
            LabelSide::Right,
        ]
    } else {
        allowed
    }
}

/// Apply optimal label placement based on connector angles.
fn optimize_labels(state: &mut LayoutState, model: &datamodel::Model, metadata: &ComputedMetadata) {
    let uid_to_ident: HashMap<i32, String> = model
        .variables
        .iter()
        .filter_map(|var| {
            let ident = canonicalize(var.get_ident()).into_owned();
            state.uid_manager.get_uid(&ident).map(|uid| (uid, ident))
        })
        .collect();

    let ident_positions: HashMap<String, Position> = state
        .positions
        .iter()
        .filter_map(|(uid, pos)| uid_to_ident.get(uid).map(|ident| (ident.clone(), *pos)))
        .collect();

    let uses = &metadata.dep_graph;
    let used_by = &metadata.reverse_dep_graph;

    let updates: Vec<(usize, LabelSide)> = state
        .elements
        .iter()
        .enumerate()
        .filter_map(|(i, elem)| match elem {
            ViewElement::Stock(stock) => {
                let ident = uid_to_ident.get(&stock.uid)?;
                let allowed = calculate_allowed_label_sides_for_stock(state, metadata, ident);
                let side = calculate_restricted_label_side(
                    ident,
                    &ident_positions,
                    uses,
                    used_by,
                    &allowed,
                );
                Some((i, side))
            }
            ViewElement::Flow(flow) => {
                let ident = uid_to_ident.get(&flow.uid)?;
                if flow.points.len() >= 2 {
                    let orientation = compute_flow_orientation(&flow.points);
                    let allowed = match orientation {
                        FlowOrientation::Horizontal => {
                            vec![LabelSide::Top, LabelSide::Bottom]
                        }
                        FlowOrientation::Vertical => {
                            vec![LabelSide::Left, LabelSide::Right]
                        }
                    };
                    let side = calculate_restricted_label_side(
                        ident,
                        &ident_positions,
                        uses,
                        used_by,
                        &allowed,
                    );
                    Some((i, side))
                } else {
                    None
                }
            }
            ViewElement::Aux(aux) => {
                let ident = uid_to_ident.get(&aux.uid)?;
                let side = calculate_optimal_label_side(ident, &ident_positions, uses, used_by);
                Some((i, side))
            }
            ViewElement::Module(module) => {
                let ident = uid_to_ident.get(&module.uid)?;
                let side = calculate_optimal_label_side(ident, &ident_positions, uses, used_by);
                Some((i, side))
            }
            _ => None,
        })
        .collect();

    for (i, side) in updates {
        match &mut state.elements[i] {
            ViewElement::Stock(s) => s.label_side = side,
            ViewElement::Flow(f) => f.label_side = side,
            ViewElement::Aux(a) => a.label_side = side,
            ViewElement::Module(m) => m.label_side = side,
            _ => {}
        }
    }
}

/// Apply arc curvature to connectors involved in feedback loops.
fn apply_loop_curvature(
    state: &mut LayoutState,
    config: &LayoutConfig,
    model: &datamodel::Model,
    metadata: &ComputedMetadata,
) {
    if metadata.feedback_loops.is_empty() {
        return;
    }

    let uid_to_ident: HashMap<i32, String> = model
        .variables
        .iter()
        .filter_map(|var| {
            let ident = canonicalize(var.get_ident()).into_owned();
            state.uid_manager.get_uid(&ident).map(|uid| (uid, ident))
        })
        .collect();

    let mut link_map: HashMap<(String, String), usize> = HashMap::new();
    for (i, elem) in state.elements.iter().enumerate() {
        if let ViewElement::Link(link) = elem {
            let from_ident = uid_to_ident.get(&link.from_uid).cloned();
            let to_ident = uid_to_ident.get(&link.to_uid).cloned();
            if let (Some(from), Some(to)) = (from_ident, to_ident) {
                link_map.insert((from, to), i);
            }
        }
    }

    let loops = &metadata.feedback_loops;
    for i in (0..loops.len()).rev() {
        let loop_info = &loops[i];
        let chain = loop_info.causal_chain();
        if chain.len() < 2 {
            continue;
        }

        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        let mut count = 0;
        for var in chain {
            if let Some(uid) = state.uid_manager.get_uid(var)
                && let Some(&pos) = state.positions.get(&uid)
            {
                sum_x += pos.x;
                sum_y += pos.y;
                count += 1;
            }
        }
        if count == 0 {
            continue;
        }
        let loop_center = Position::new(sum_x / count as f64, sum_y / count as f64);

        for j in 0..chain.len() - 1 {
            let from = &chain[j];
            let to = &chain[j + 1];

            let Some(&elem_idx) = link_map.get(&(from.clone(), to.clone())) else {
                continue;
            };

            if let ViewElement::Link(link) = &state.elements[elem_idx]
                && matches!(link.shape, LinkShape::Arc(_))
            {
                continue;
            }

            let from_uid = state.uid_manager.get_uid(from);
            let to_uid = state.uid_manager.get_uid(to);
            if let (Some(f_uid), Some(t_uid)) = (from_uid, to_uid)
                && let (Some(&from_pos), Some(&to_pos)) =
                    (state.positions.get(&f_uid), state.positions.get(&t_uid))
            {
                let arc_angle = calculate_loop_arc_angle(
                    from_pos,
                    to_pos,
                    loop_center,
                    config.loop_curvature_factor,
                );
                if let ViewElement::Link(link) = &mut state.elements[elem_idx] {
                    link.shape = LinkShape::Arc(arc_angle);
                }
            }
        }
    }
}

/// Ensure every stock/flow/aux/module variable in the model has a
/// corresponding rendered view element.
fn validate_view_completeness(state: &LayoutState, model: &datamodel::Model) -> Result<(), String> {
    let mut expected_stocks = BTreeSet::new();
    let mut expected_flows = BTreeSet::new();
    let mut expected_auxes = BTreeSet::new();
    let mut expected_modules = BTreeSet::new();

    for var in &model.variables {
        match var {
            datamodel::Variable::Stock(s) => {
                expected_stocks.insert(canonicalize(&s.ident).into_owned());
            }
            datamodel::Variable::Flow(f) => {
                expected_flows.insert(canonicalize(&f.ident).into_owned());
            }
            datamodel::Variable::Aux(a) => {
                expected_auxes.insert(canonicalize(&a.ident).into_owned());
            }
            datamodel::Variable::Module(m) => {
                expected_modules.insert(canonicalize(&m.ident).into_owned());
            }
        }
    }

    let mut found_stocks = BTreeSet::new();
    let mut found_flows = BTreeSet::new();
    let mut found_auxes = BTreeSet::new();
    let mut found_modules = BTreeSet::new();

    for elem in &state.elements {
        match elem {
            ViewElement::Stock(s) => {
                found_stocks.insert(canonicalize(&s.name).into_owned());
            }
            ViewElement::Flow(f) => {
                found_flows.insert(canonicalize(&f.name).into_owned());
            }
            ViewElement::Aux(a) => {
                found_auxes.insert(canonicalize(&a.name).into_owned());
            }
            ViewElement::Module(m) => {
                found_modules.insert(canonicalize(&m.name).into_owned());
            }
            _ => {}
        }
    }

    let mut missing = Vec::new();
    for ident in expected_stocks.difference(&found_stocks) {
        missing.push(format!("stock '{}'", ident));
    }
    for ident in expected_flows.difference(&found_flows) {
        missing.push(format!("flow '{}'", ident));
    }
    for ident in expected_auxes.difference(&found_auxes) {
        missing.push(format!("aux '{}'", ident));
    }
    for ident in expected_modules.difference(&found_modules) {
        missing.push(format!("module '{}'", ident));
    }

    if missing.is_empty() {
        return Ok(());
    }
    missing.sort();
    Err(format!(
        "layout incomplete: missing view elements for {}",
        missing.join(", ")
    ))
}

/// Compute the bounding box of all layout elements, including label extents.
fn compute_bounds(elements: &[ViewElement], config: &LayoutConfig) -> (f64, f64, f64, f64) {
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    let update = |min_x: &mut f64,
                  min_y: &mut f64,
                  max_x: &mut f64,
                  max_y: &mut f64,
                  cx: f64,
                  cy: f64,
                  w: f64,
                  h: f64| {
        let hw = w / 2.0;
        let hh = h / 2.0;
        if cx - hw < *min_x {
            *min_x = cx - hw;
        }
        if cy - hh < *min_y {
            *min_y = cy - hh;
        }
        if cx + hw > *max_x {
            *max_x = cx + hw;
        }
        if cy + hh > *max_y {
            *max_y = cy + hh;
        }
    };

    let update_rect = |min_x: &mut f64,
                       min_y: &mut f64,
                       max_x: &mut f64,
                       max_y: &mut f64,
                       x0: f64,
                       y0: f64,
                       x1: f64,
                       y1: f64| {
        if x0 < *min_x {
            *min_x = x0;
        }
        if y0 < *min_y {
            *min_y = y0;
        }
        if x1 > *max_x {
            *max_x = x1;
        }
        if y1 > *max_y {
            *max_y = y1;
        }
    };

    for elem in elements {
        match elem {
            ViewElement::Stock(s) => {
                update(
                    &mut min_x,
                    &mut min_y,
                    &mut max_x,
                    &mut max_y,
                    s.x,
                    s.y,
                    config.stock_width,
                    config.stock_height,
                );
                let (lx0, ly0, lx1, ly1) = estimate_label_bounds(
                    &s.name,
                    s.x,
                    s.y,
                    s.label_side,
                    config.stock_width,
                    config.stock_height,
                );
                update_rect(
                    &mut min_x, &mut min_y, &mut max_x, &mut max_y, lx0, ly0, lx1, ly1,
                );
            }
            ViewElement::Flow(f) => {
                update(
                    &mut min_x,
                    &mut min_y,
                    &mut max_x,
                    &mut max_y,
                    f.x,
                    f.y,
                    config.flow_width,
                    config.flow_height,
                );
                for pt in &f.points {
                    update_rect(
                        &mut min_x, &mut min_y, &mut max_x, &mut max_y, pt.x, pt.y, pt.x, pt.y,
                    );
                }
                let (lx0, ly0, lx1, ly1) = estimate_label_bounds(
                    &f.name,
                    f.x,
                    f.y,
                    f.label_side,
                    config.flow_width,
                    config.flow_height,
                );
                update_rect(
                    &mut min_x, &mut min_y, &mut max_x, &mut max_y, lx0, ly0, lx1, ly1,
                );
            }
            ViewElement::Aux(a) => {
                update(
                    &mut min_x,
                    &mut min_y,
                    &mut max_x,
                    &mut max_y,
                    a.x,
                    a.y,
                    config.aux_width,
                    config.aux_height,
                );
                let (lx0, ly0, lx1, ly1) = estimate_label_bounds(
                    &a.name,
                    a.x,
                    a.y,
                    a.label_side,
                    config.aux_width,
                    config.aux_height,
                );
                update_rect(
                    &mut min_x, &mut min_y, &mut max_x, &mut max_y, lx0, ly0, lx1, ly1,
                );
            }
            ViewElement::Module(m) => {
                update(
                    &mut min_x,
                    &mut min_y,
                    &mut max_x,
                    &mut max_y,
                    m.x,
                    m.y,
                    config.module_width,
                    config.module_height,
                );
                let (lx0, ly0, lx1, ly1) = estimate_label_bounds(
                    &m.name,
                    m.x,
                    m.y,
                    m.label_side,
                    config.module_width,
                    config.module_height,
                );
                update_rect(
                    &mut min_x, &mut min_y, &mut max_x, &mut max_y, lx0, ly0, lx1, ly1,
                );
            }
            ViewElement::Cloud(c) => {
                update(
                    &mut min_x,
                    &mut min_y,
                    &mut max_x,
                    &mut max_y,
                    c.x,
                    c.y,
                    config.cloud_width,
                    config.cloud_height,
                );
            }
            _ => {}
        }
    }

    (min_x, min_y, max_x, max_y)
}

/// Compose all layout blocks into a complete stock-flow diagram.
pub fn fresh_layout(
    model: &datamodel::Model,
    metadata: &ComputedMetadata,
    config: &LayoutConfig,
) -> Result<datamodel::StockFlow, String> {
    let chains = &metadata.chains;
    if chains.is_empty() && model.variables.is_empty() {
        return Ok(datamodel::StockFlow {
            name: None,
            elements: Vec::new(),
            view_box: Rect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            },
            zoom: 1.0,
            use_lettered_polarity: false,
            font: None,
            sketch_compat: None,
        });
    }

    let mut state = LayoutState::new(model);

    // Phase 1: Compute chain positions using SFDP
    let chain_positions = compute_chain_positions(chains, metadata, config);

    // Phase 2: Layout each chain at its position
    let chains_data: Vec<_> = chains
        .iter()
        .map(|c| (c.stocks.clone(), c.flows.clone(), c.all_vars.clone()))
        .collect();
    for (i, (stocks, flows, _all_vars)) in chains_data.iter().enumerate() {
        let position = chain_positions
            .get(&i)
            .copied()
            .unwrap_or(Position::new(config.start_x, config.start_y));
        layout_chain(&mut state, config, metadata, stocks, flows, position)?;
    }

    // Phase 3: Position auxiliaries and create connectors
    place_auxiliaries(&mut state, config, model, metadata, &chains_data)?;
    build_connectors(&mut state, model, metadata)?;

    // Phase 4: Apply optimal label placement
    optimize_labels(&mut state, model, metadata);

    // Phase 5: Normalize coordinates
    normalize_coordinates(&mut state.elements, DIAGRAM_ORIGIN_MARGIN);

    // Phase 6: Apply feedback loop curvature
    apply_loop_curvature(&mut state, config, model, metadata);

    validate_view_completeness(&state, model)?;

    // Phase 7: Compute ViewBox from final element positions
    let (bmin_x, _bmin_y, bmax_x, bmax_y) = compute_bounds(&state.elements, config);
    let view_box = if !state.elements.is_empty() && bmin_x != f64::MAX {
        Rect {
            x: 0.0,
            y: 0.0,
            width: bmax_x + DIAGRAM_ORIGIN_MARGIN,
            height: bmax_y + DIAGRAM_ORIGIN_MARGIN,
        }
    } else {
        Rect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        }
    };

    Ok(datamodel::StockFlow {
        name: None,
        elements: state.elements,
        view_box,
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    })
}

/// Check if a dependency edge is a structural flow->stock connection (already
/// visually represented by the flow pipe).
fn is_structural_flow_stock(
    from: &str,
    to: &str,
    stock_inflows: &HashMap<String, HashSet<String>>,
    stock_outflows: &HashMap<String, HashSet<String>>,
) -> bool {
    // from is flow, to is stock: check if from is an inflow or outflow of to
    if let Some(inflows) = stock_inflows.get(to)
        && inflows.contains(from)
    {
        return true;
    }
    if let Some(outflows) = stock_outflows.get(to)
        && outflows.contains(from)
    {
        return true;
    }
    false
}

/// Check if a dependency edge is a structural stock->flow connection (needs arc).
fn is_structural_stock_flow(
    from: &str,
    to: &str,
    stock_inflows: &HashMap<String, HashSet<String>>,
    stock_outflows: &HashMap<String, HashSet<String>>,
) -> bool {
    // from is stock, to is flow: check if to is an inflow or outflow of from
    if let Some(inflows) = stock_inflows.get(from)
        && inflows.contains(to)
    {
        return true;
    }
    if let Some(outflows) = stock_outflows.get(from)
        && outflows.contains(to)
    {
        return true;
    }
    false
}

/// Try to compile the project and return the compiled model for AST-based
/// dependency extraction. Returns `None` if compilation fails or the model
/// isn't found, in which case callers should fall back to string heuristics.
/// Resolve the canonical model name, handling the "main" alias for
/// unnamed models (empty name).
fn resolve_model_name<'a>(project: &'a datamodel::Project, model_name: &'a str) -> &'a str {
    project
        .get_model(model_name)
        .map(|m| m.name.as_str())
        .unwrap_or(model_name)
}

/// Ensure we have a mutable salsa db + source project, creating a
/// temporary one if the caller didn't provide one.
fn ensure_db_state_mut<'a>(
    project: &datamodel::Project,
    db_state: Option<(&'a mut crate::db::SimlinDb, crate::db::SourceProject)>,
    local_db: &'a mut Option<crate::db::SimlinDb>,
) -> (&'a mut crate::db::SimlinDb, crate::db::SourceProject) {
    match db_state {
        Some((db, sp)) => (db, sp),
        None => {
            let db = local_db.insert(crate::db::SimlinDb::default());
            // Extract the Copy SourceProject handle before reborrowing db.
            let source_project = crate::db::sync_from_datamodel(db, project).project;
            (db, source_project)
        }
    }
}

/// Extract variable dependencies from an equation using word-boundary
/// matching on the equation text. This is a heuristic that avoids requiring
/// full model compilation; it can produce false positives for identifiers
/// that collide with builtin function names and doesn't handle subscripted
/// references. Used as a fallback when compilation fails or a variable has
/// a parse error and no AST is available.
fn extract_equation_deps(var: &datamodel::Variable, all_idents: &HashSet<String>) -> Vec<String> {
    let equation = match var.get_equation() {
        Some(eq) => eq,
        None => return Vec::new(),
    };

    let equation_texts: Vec<&str> = match equation {
        datamodel::Equation::Scalar(text) => vec![text.as_str()],
        datamodel::Equation::ApplyToAll(_, text) => vec![text.as_str()],
        datamodel::Equation::Arrayed(_, entries, _, _) => entries
            .iter()
            .map(|(_, text, _, _)| text.as_str())
            .collect(),
    };
    if equation_texts.is_empty() {
        return Vec::new();
    }

    let mut deps = Vec::new();
    let lowered_texts: Vec<String> = equation_texts
        .iter()
        .map(|text| text.to_lowercase())
        .collect();
    let var_ident = canonicalize(var.get_ident());

    for ident in all_idents {
        if ident == var_ident.as_ref() {
            continue;
        }
        if lowered_texts
            .iter()
            .any(|equation_lower| contains_ident(equation_lower, ident))
        {
            deps.push(ident.clone());
        }
    }

    deps
}

/// Check if an identifier appears as a word boundary match in equation text.
fn contains_ident(equation_lower: &str, ident: &str) -> bool {
    let ident_lower = ident.to_lowercase();
    let mut search_from = 0;
    while let Some(pos) = equation_lower[search_from..].find(&ident_lower) {
        let abs_pos = search_from + pos;
        let end_pos = abs_pos + ident_lower.len();

        let before_ok = abs_pos == 0
            || !equation_lower.as_bytes()[abs_pos - 1].is_ascii_alphanumeric()
                && equation_lower.as_bytes()[abs_pos - 1] != b'_';
        let after_ok = end_pos >= equation_lower.len()
            || !equation_lower.as_bytes()[end_pos].is_ascii_alphanumeric()
                && equation_lower.as_bytes()[end_pos] != b'_';

        if before_ok && after_ok {
            return true;
        }
        search_from = abs_pos + 1;
    }
    false
}

/// Build feedback loops from the persisted model loop_metadata (UIDs only,
/// no LTM simulation). Used as a fallback when LTM detection fails.
fn build_feedback_loops_from_metadata(
    model: &datamodel::Model,
    uid_to_ident: &HashMap<i32, String>,
) -> Vec<metadata::FeedbackLoop> {
    let mut feedback_loops = Vec::new();
    for (idx, loop_md) in model.loop_metadata.iter().enumerate() {
        if loop_md.deleted {
            continue;
        }
        let mut variables: Vec<String> = loop_md
            .uids
            .iter()
            .filter_map(|uid| uid_to_ident.get(uid).cloned())
            .collect();
        if variables.len() < 2 {
            continue;
        }
        if variables.first() != variables.last()
            && let Some(first) = variables.first().cloned()
        {
            variables.push(first);
        }

        feedback_loops.push(metadata::FeedbackLoop {
            name: if loop_md.name.is_empty() {
                format!("loop_{}", idx + 1)
            } else {
                loop_md.name.clone()
            },
            polarity: LoopPolarity::Undetermined,
            variables,
            importance_series: Vec::new(),
            dominant_period: None,
        });
    }
    feedback_loops
}

/// Try to detect feedback loops using LTM analysis via the incremental
/// salsa compilation path. Compiles the project, detects loops, augments
/// with synthetic LTM variables, simulates, and extracts importance time
/// series. Returns `None` if any step fails, signaling the caller to fall
/// back to persisted loop_metadata.
fn try_detect_ltm_loops(
    db: &mut crate::db::SimlinDb,
    source_project: crate::db::SourceProject,
    model_name: &str,
) -> Option<Vec<metadata::FeedbackLoop>> {
    try_detect_ltm_loops_incremental(db, source_project, model_name)
}

/// Incremental salsa path for LTM loop detection.
fn try_detect_ltm_loops_incremental(
    db: &mut crate::db::SimlinDb,
    source_project: crate::db::SourceProject,
    actual_name: &str,
) -> Option<Vec<metadata::FeedbackLoop>> {
    use salsa::Setter;

    let actual_name_owned = actual_name.to_string();

    // Phase 1: Model lookup and loop detection.
    let detected = {
        let canonical_name = crate::canonicalize(&actual_name_owned);
        let source_model = *source_project.models(db).get(canonical_name.as_ref())?;
        crate::db::model_detected_loops(db, source_model, source_project)
    };

    if detected.loops.is_empty() {
        return Some(Vec::new());
    }

    // Phase 2: LTM compile and simulate.
    source_project.set_ltm_enabled(db).to(true);
    let vm_result = crate::db::compile_project_incremental(db, source_project, &actual_name_owned)
        .ok()
        .and_then(|compiled_sim| crate::vm::Vm::new(compiled_sim).ok())
        .and_then(|mut vm| {
            vm.run_to_end().ok()?;
            Some(vm)
        });
    source_project.set_ltm_enabled(db).to(false);

    let vm = vm_result?;

    // Phase 3: Build feedback loop structs from VM results.
    let mut feedback_loops = Vec::new();
    for dl in &detected.loops {
        let polarity = match dl.polarity {
            crate::db::DetectedLoopPolarity::Reinforcing => LoopPolarity::Reinforcing,
            crate::db::DetectedLoopPolarity::Balancing => LoopPolarity::Balancing,
            crate::db::DetectedLoopPolarity::Undetermined => LoopPolarity::Undetermined,
        };

        let variables: Vec<String> = {
            let mut vars = dl.variables.clone();
            if let Some(first) = vars.first().cloned() {
                vars.push(first);
            }
            vars
        };

        // Extract the relative loop score time series.
        let var_name = format!("$\u{205A}ltm\u{205A}rel_loop_score\u{205A}{}", dl.id);
        let var_ident = Ident::new(&var_name);
        let importance_series = vm
            .get_series(&var_ident)
            .unwrap_or_default()
            .into_iter()
            .map(|v| if v.is_finite() { v } else { 0.0 })
            .collect();

        feedback_loops.push(metadata::FeedbackLoop {
            name: dl.id.clone(),
            polarity,
            variables,
            importance_series,
            dominant_period: None,
        });
    }

    Some(feedback_loops)
}

/// Compute metadata for a model from its variable definitions and dependency structure.
///
/// Uses the salsa incremental compilation path for dependency extraction and
/// LTM loop detection. When `db_state` is `None`, creates a temporary salsa
/// db internally.
pub fn compute_metadata(
    project: &datamodel::Project,
    model_name: &str,
    db_state: Option<(&mut crate::db::SimlinDb, crate::db::SourceProject)>,
) -> Option<ComputedMetadata> {
    let model = project.get_model(model_name)?;
    let mut dep_graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut reverse_dep_graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut constants: BTreeSet<String> = BTreeSet::new();
    let mut stock_to_inflows: HashMap<String, Vec<String>> = HashMap::new();
    let mut stock_to_outflows: HashMap<String, Vec<String>> = HashMap::new();
    let mut flow_to_stocks: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    let mut all_flows: BTreeSet<String> = BTreeSet::new();
    let mut uid_to_ident: HashMap<i32, String> = HashMap::new();

    // Collect stock inflows/outflows and known flow identities.
    for var in &model.variables {
        match var {
            datamodel::Variable::Stock(stock) => {
                let stock_ident = canonicalize(&stock.ident).into_owned();
                if let Some(uid) = stock.uid {
                    uid_to_ident.insert(uid, stock_ident.clone());
                }

                let inflows: Vec<String> = stock
                    .inflows
                    .iter()
                    .map(|f| canonicalize(f).into_owned())
                    .collect();
                let outflows: Vec<String> = stock
                    .outflows
                    .iter()
                    .map(|f| canonicalize(f).into_owned())
                    .collect();

                for f in &inflows {
                    let entry = flow_to_stocks.entry(f.clone()).or_insert((None, None));
                    entry.1 = Some(stock_ident.clone());
                    all_flows.insert(f.clone());
                }
                for f in &outflows {
                    let entry = flow_to_stocks.entry(f.clone()).or_insert((None, None));
                    entry.0 = Some(stock_ident.clone());
                    all_flows.insert(f.clone());
                }

                stock_to_inflows.insert(stock_ident.clone(), inflows);
                stock_to_outflows.insert(stock_ident, outflows);
            }
            datamodel::Variable::Flow(flow) => {
                let flow_ident = canonicalize(&flow.ident).into_owned();
                all_flows.insert(flow_ident.clone());
                flow_to_stocks
                    .entry(flow_ident.clone())
                    .or_insert((None, None));
                if let Some(uid) = flow.uid {
                    uid_to_ident.insert(uid, flow_ident);
                }
            }
            datamodel::Variable::Aux(aux) => {
                if let Some(uid) = aux.uid {
                    uid_to_ident.insert(uid, canonicalize(&aux.ident).into_owned());
                }
            }
            datamodel::Variable::Module(module) => {
                if let Some(uid) = module.uid {
                    uid_to_ident.insert(uid, canonicalize(&module.ident).into_owned());
                }
            }
        }
    }

    // Use a salsa db for dependency extraction and LTM detection, creating
    // a temporary one if the caller didn't provide one.
    let mut local_db: Option<crate::db::SimlinDb> = None;
    let (db, source_project) = ensure_db_state_mut(project, db_state, &mut local_db);

    let actual_model_name = resolve_model_name(project, model_name);
    let canonical_model_name = canonicalize(actual_model_name);
    let source_model = source_project
        .models(db)
        .get(canonical_model_name.as_ref())
        .copied();

    let all_idents: HashSet<String> = model
        .variables
        .iter()
        .map(|v| canonicalize(v.get_ident()).into_owned())
        .collect();

    for var in &model.variables {
        let var_ident = canonicalize(var.get_ident()).into_owned();
        dep_graph.entry(var_ident.clone()).or_default();

        // Modules derive dependencies from their reference bindings
        // rather than from equation parsing.
        if let datamodel::Variable::Module(module) = var {
            for reference in &module.references {
                let src_ident = canonicalize(&reference.src).into_owned();
                if src_ident != var_ident && all_idents.contains(&src_ident) {
                    dep_graph
                        .entry(var_ident.clone())
                        .or_default()
                        .insert(src_ident.clone());
                    reverse_dep_graph
                        .entry(src_ident)
                        .or_default()
                        .insert(var_ident.clone());
                }
            }
            continue;
        }

        // Use salsa dependency extraction when the source model and
        // variable are available. Only fall back to string heuristics
        // when the variable is not in the source model, or when the
        // equation failed to parse (no AST). If parsing succeeded but
        // deps are empty, the variable is a genuine constant.
        let deps: Vec<String> = source_model
            .and_then(|sm| {
                let sv = sm.variables(db).get(&var_ident)?.to_owned();
                let var_deps = crate::db::variable_direct_dependencies(db, sv, source_project);
                let mut combined: Vec<String> = var_deps
                    .dt_deps
                    .iter()
                    .chain(var_deps.initial_deps.iter())
                    .cloned()
                    .collect();
                combined.sort();
                combined.dedup();
                if combined.is_empty() {
                    // Check whether the equation actually parsed. If the AST
                    // is None, the equation has syntax errors and we fall back
                    // to string heuristics for approximate layout deps.
                    let empty_ctx = crate::db::ModuleIdentContext::new(db, vec![]);
                    let parsed = crate::db::parse_source_variable_with_module_context(
                        db,
                        sv,
                        source_project,
                        empty_ctx,
                    );
                    if parsed.variable.ast().is_none() {
                        return Some(extract_equation_deps(var, &all_idents));
                    }
                }
                Some(combined)
            })
            .unwrap_or_else(|| extract_equation_deps(var, &all_idents));

        // Filter to only include deps that are actual rendered model
        // variables. AST extraction can yield module-internal identifiers
        // (e.g. "m·out") that don't correspond to any rendered element.
        // Also exclude self-references: the string heuristic skips them,
        // but AST extraction doesn't (stocks reference themselves through
        // init expressions, SMOOTH/DELAY patterns, etc.).
        let deps: Vec<String> = deps
            .into_iter()
            .filter(|d| d != &var_ident && all_idents.contains(d))
            .collect();

        if deps.is_empty() && !matches!(var, datamodel::Variable::Stock(_)) {
            constants.insert(var_ident.clone());
        }

        for dep in &deps {
            dep_graph
                .entry(var_ident.clone())
                .or_default()
                .insert(dep.clone());
            reverse_dep_graph
                .entry(dep.clone())
                .or_default()
                .insert(var_ident.clone());
        }

        // Add structural stock-flow dependencies. In this metadata shape,
        // dep_graph stores var -> dependencies, so stocks depend on their
        // inflow/outflow variables.
        if let datamodel::Variable::Stock(stock) = var {
            for inflow in &stock.inflows {
                let flow_ident = canonicalize(inflow).into_owned();
                dep_graph
                    .entry(var_ident.clone())
                    .or_default()
                    .insert(flow_ident.clone());
                reverse_dep_graph
                    .entry(flow_ident)
                    .or_default()
                    .insert(var_ident.clone());
            }
            for outflow in &stock.outflows {
                let flow_ident = canonicalize(outflow).into_owned();
                dep_graph
                    .entry(var_ident.clone())
                    .or_default()
                    .insert(flow_ident.clone());
                reverse_dep_graph
                    .entry(flow_ident)
                    .or_default()
                    .insert(var_ident.clone());
            }
        }
    }

    // Detect stock-flow chains
    let chains = detect_chains(
        &stock_to_inflows,
        &stock_to_outflows,
        &flow_to_stocks,
        &all_flows,
    );

    // Try LTM-based loop detection. Falls back to persisted loop_metadata
    // if LTM detection or simulation fails.
    let mut feedback_loops = try_detect_ltm_loops(db, source_project, actual_model_name)
        .unwrap_or_else(|| build_feedback_loops_from_metadata(model, &uid_to_ident));
    feedback_loops.sort_by(|a, b| {
        b.average_importance()
            .partial_cmp(&a.average_importance())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let dominant_periods = {
        let specs = model.sim_specs.as_ref().unwrap_or(&project.sim_specs);
        let dt_to_f64 = |dt: &datamodel::Dt| match dt {
            datamodel::Dt::Dt(v) => *v,
            datamodel::Dt::Reciprocal(v) => 1.0 / v,
        };
        let dt = dt_to_f64(&specs.dt);
        let raw_save_step = specs.save_step.as_ref().map(dt_to_f64).unwrap_or(dt);
        // The VM/interpreter saves at most once per dt step
        // (save_every = max(1, round(save_step/dt))), so the effective
        // cadence is never faster than dt.
        let effective_save_step = raw_save_step.max(dt);
        metadata::calculate_dominant_periods(&feedback_loops, specs.start, effective_save_step)
    };

    Some(ComputedMetadata {
        chains,
        feedback_loops,
        dominant_periods,
        dep_graph,
        reverse_dep_graph,
        constants,
        stock_to_inflows,
        stock_to_outflows,
        flow_to_stocks,
    })
}

/// Detect stock-flow chains using union-find over stock/flow connections.
fn detect_chains(
    stock_to_inflows: &HashMap<String, Vec<String>>,
    stock_to_outflows: &HashMap<String, Vec<String>>,
    flow_to_stocks: &HashMap<String, (Option<String>, Option<String>)>,
    all_flows: &BTreeSet<String>,
) -> Vec<StockFlowChain> {
    // Collect all stocks
    let all_stocks: BTreeSet<String> = stock_to_inflows
        .keys()
        .chain(stock_to_outflows.keys())
        .cloned()
        .collect();

    if all_stocks.is_empty() {
        let mut chains = Vec::new();
        for flow_ident in all_flows {
            chains.push(StockFlowChain {
                stocks: Vec::new(),
                flows: vec![flow_ident.clone()],
                all_vars: vec![flow_ident.clone()],
                importance: 5.0,
            });
        }
        return chains;
    }

    // Build connected components via BFS
    let mut visited: HashSet<String> = HashSet::new();
    let mut flows_in_chains: HashSet<String> = HashSet::new();
    let mut chains = Vec::new();

    for start_stock in &all_stocks {
        if visited.contains(start_stock) {
            continue;
        }

        let mut chain_stocks = Vec::new();
        let mut chain_flows = Vec::new();
        let mut seen_flows: HashSet<String> = HashSet::new();
        let mut queue = VecDeque::from([start_stock.clone()]);

        while let Some(stock) = queue.pop_front() {
            if !visited.insert(stock.clone()) {
                continue;
            }
            chain_stocks.push(stock.clone());

            // Follow inflows to connected stocks
            if let Some(inflows) = stock_to_inflows.get(&stock) {
                for flow in inflows {
                    if seen_flows.insert(flow.clone()) {
                        chain_flows.push(flow.clone());
                        flows_in_chains.insert(flow.clone());
                    }
                    if let Some((Some(from_stock), _)) = flow_to_stocks.get(flow)
                        && !visited.contains(from_stock)
                    {
                        queue.push_back(from_stock.clone());
                    }
                }
            }
            // Follow outflows to connected stocks
            if let Some(outflows) = stock_to_outflows.get(&stock) {
                for flow in outflows {
                    if seen_flows.insert(flow.clone()) {
                        chain_flows.push(flow.clone());
                        flows_in_chains.insert(flow.clone());
                    }
                    if let Some((_, Some(to_stock))) = flow_to_stocks.get(flow)
                        && !visited.contains(to_stock)
                    {
                        queue.push_back(to_stock.clone());
                    }
                }
            }
        }

        let mut all_vars: Vec<String> = chain_stocks
            .iter()
            .chain(chain_flows.iter())
            .cloned()
            .collect();
        all_vars.sort();

        chains.push(StockFlowChain {
            stocks: chain_stocks.clone(),
            flows: chain_flows.clone(),
            all_vars,
            importance: (chain_stocks.len() * 10 + chain_flows.len() * 5) as f64,
        });
    }

    // Append isolated flows that are not connected to any stock chain.
    for flow_ident in all_flows {
        if flows_in_chains.contains(flow_ident) {
            continue;
        }
        chains.push(StockFlowChain {
            stocks: Vec::new(),
            flows: vec![flow_ident.clone()],
            all_vars: vec![flow_ident.clone()],
            importance: 5.0,
        });
    }

    chains.sort_by(|a, b| {
        b.importance
            .partial_cmp(&a.importance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    chains
}

/// Count edge crossings in a completed StockFlow view.
///
/// Arc and multi-point link shapes are approximated as straight segments
/// from source to target position, so counts for diagrams with curved
/// connectors are approximate.
pub fn count_view_crossings(view: &datamodel::StockFlow) -> usize {
    if view.elements.is_empty() {
        return 0;
    }

    let mut uid_positions: HashMap<i32, Position> = HashMap::new();
    for elem in &view.elements {
        match elem {
            ViewElement::Stock(s) => {
                uid_positions.insert(s.uid, Position::new(s.x, s.y));
            }
            ViewElement::Flow(f) => {
                uid_positions.insert(f.uid, Position::new(f.x, f.y));
            }
            ViewElement::Aux(a) => {
                uid_positions.insert(a.uid, Position::new(a.x, a.y));
            }
            ViewElement::Cloud(c) => {
                uid_positions.insert(c.uid, Position::new(c.x, c.y));
            }
            _ => {}
        }
    }

    let mut segments: Vec<LineSegment> = Vec::new();

    for elem in &view.elements {
        match elem {
            ViewElement::Link(link) => {
                if let (Some(&from_pos), Some(&to_pos)) = (
                    uid_positions.get(&link.from_uid),
                    uid_positions.get(&link.to_uid),
                ) {
                    segments.push(LineSegment {
                        start: from_pos,
                        end: to_pos,
                        from_node: format!("elem_{}", link.from_uid),
                        to_node: format!("elem_{}", link.to_uid),
                    });
                }
            }
            ViewElement::Flow(flow) => {
                for i in 0..flow.points.len().saturating_sub(1) {
                    segments.push(LineSegment {
                        start: Position::new(flow.points[i].x, flow.points[i].y),
                        end: Position::new(flow.points[i + 1].x, flow.points[i + 1].y),
                        from_node: format!("flow_{}#{}", flow.uid, i),
                        to_node: format!("flow_{}#{}", flow.uid, i + 1),
                    });
                }
            }
            _ => {}
        }
    }

    annealing::count_crossings(&segments)
}

/// Seeds for parallel layout generation. Each seed produces a different SFDP
/// layout; the one with fewest connector crossings is selected.
const LAYOUT_SEEDS: [u64; 4] = [42, 123, 456, 789];

/// Generate a complete stock-flow diagram layout for a model using a single
/// seed. This is the fast path; for higher-quality results use
/// [`generate_best_layout`] which tries multiple seeds in parallel.
///
/// Computes metadata (dependency graph, chains) from the model variables,
/// then runs the multi-phase layout pipeline: chain positioning via SFDP,
/// auxiliary placement, connector routing, label optimization, and
/// coordinate normalization.
pub fn generate_layout(
    project: &datamodel::Project,
    model_name: &str,
    db_state: Option<(&mut crate::db::SimlinDb, crate::db::SourceProject)>,
) -> Result<datamodel::StockFlow, String> {
    let config = LayoutConfig::default();
    let not_found = || format!("model '{}' not found in project", model_name);
    let model = project.get_model(model_name).ok_or_else(not_found)?;
    let metadata = compute_metadata(project, model_name, db_state).ok_or_else(not_found)?;
    fresh_layout(model, &metadata, &config)
}

/// Generate layout with a specific configuration.
pub fn generate_layout_with_config(
    project: &datamodel::Project,
    model_name: &str,
    mut config: LayoutConfig,
    db_state: Option<(&mut crate::db::SimlinDb, crate::db::SourceProject)>,
) -> Result<datamodel::StockFlow, String> {
    config.validate();
    let not_found = || format!("model '{}' not found in project", model_name);
    let model = project.get_model(model_name).ok_or_else(not_found)?;
    let metadata = compute_metadata(project, model_name, db_state).ok_or_else(not_found)?;
    fresh_layout(model, &metadata, &config)
}

/// Generate multiple layouts with different seeds in parallel and pick the
/// one with fewest crossings. On tie, the lowest seed wins.
pub fn generate_best_layout(
    project: &datamodel::Project,
    model_name: &str,
    db_state: Option<(&mut crate::db::SimlinDb, crate::db::SourceProject)>,
) -> Result<datamodel::StockFlow, String> {
    let config = LayoutConfig::default();
    let seeds = LAYOUT_SEEDS;
    let not_found = || format!("model '{}' not found in project", model_name);
    let model = project.get_model(model_name).ok_or_else(not_found)?;
    let metadata = compute_metadata(project, model_name, db_state).ok_or_else(not_found)?;

    let generate = |&seed: &u64| {
        let mut cfg = config.clone();
        cfg.annealing_random_seed = seed;
        let view = fresh_layout(model, &metadata, &cfg)?;
        let crossings = count_view_crossings(&view);
        Ok(LayoutResult {
            view,
            crossings,
            seed,
        })
    };

    #[cfg(not(target_arch = "wasm32"))]
    let results: Vec<Result<LayoutResult, String>> = {
        use rayon::prelude::*;
        seeds.par_iter().map(generate).collect()
    };

    #[cfg(target_arch = "wasm32")]
    let results: Vec<Result<LayoutResult, String>> = seeds.iter().map(generate).collect();

    select_best_layout(results)
}

/// Compute and return layout metadata (dependency graph, chains, feedback loops,
/// dominant periods) for a model without generating a full diagram layout.
///
/// This is useful for analysis tools that want loop importance, dominant periods,
/// or dependency information without paying the cost of SFDP + annealing.
pub fn compute_layout_metadata(
    project: &datamodel::Project,
    model_name: &str,
    db_state: Option<(&mut crate::db::SimlinDb, crate::db::SourceProject)>,
) -> Option<ComputedMetadata> {
    compute_metadata(project, model_name, db_state)
}

/// Pick the layout with fewest crossings; on tie, the one from the lowest seed.
fn select_best_layout(
    results: Vec<Result<LayoutResult, String>>,
) -> Result<datamodel::StockFlow, String> {
    let mut best: Option<LayoutResult> = None;

    for result in results {
        let lr = result?;
        best = Some(match best {
            None => lr,
            Some(prev) => {
                if lr.crossings < prev.crossings
                    || (lr.crossings == prev.crossings && lr.seed < prev.seed)
                {
                    lr
                } else {
                    prev
                }
            }
        });
    }

    best.map(|lr| lr.view)
        .ok_or_else(|| "no layout results".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel;

    fn test_project(model: datamodel::Model) -> datamodel::Project {
        let name = model.name.clone();
        datamodel::Project {
            name: name.clone(),
            sim_specs: datamodel::SimSpecs::default(),
            dimensions: Vec::new(),
            units: Vec::new(),
            models: vec![model],
            source: None,
            ai_information: None,
        }
    }

    /// Name used for test models -- matches the name in `simple_model()` and
    /// inline test models so that `project.get_model(TEST_MODEL)` finds them.
    const TEST_MODEL: &str = "test";

    fn simple_model() -> datamodel::Model {
        datamodel::Model {
            name: "test".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births".to_string()],
                    outflows: vec!["deaths".to_string()],
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "births".to_string(),
                    equation: datamodel::Equation::Scalar("population * birth_rate".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(2),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "deaths".to_string(),
                    equation: datamodel::Equation::Scalar("population * death_rate".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(3),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "birth_rate".to_string(),
                    equation: datamodel::Equation::Scalar("0.03".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(4),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "death_rate".to_string(),
                    equation: datamodel::Equation::Scalar("0.01".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(5),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        }
    }

    #[test]
    fn test_generate_layout_empty() {
        let project = test_project(datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: Vec::new(),
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        });
        let result = generate_layout(&project, TEST_MODEL, None).unwrap();
        assert!(result.elements.is_empty());
        assert_eq!(result.zoom, 1.0);
    }

    #[test]
    fn test_generate_layout_single_chain() {
        let project = test_project(simple_model());
        let result = generate_layout(&project, TEST_MODEL, None).unwrap();

        assert!(!result.elements.is_empty());
        assert_eq!(result.zoom, 1.0);

        // Should have stocks, flows, auxes, clouds, and links
        let stock_count = result
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Stock(_)))
            .count();
        let flow_count = result
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Flow(_)))
            .count();
        let aux_count = result
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Aux(_)))
            .count();

        assert_eq!(stock_count, 1); // population
        assert_eq!(flow_count, 2); // births, deaths
        assert_eq!(aux_count, 2); // birth_rate, death_rate
    }

    #[test]
    fn test_generate_layout_completeness() {
        let project = test_project(simple_model());
        let model = project.get_model(TEST_MODEL).unwrap();
        let result = generate_layout(&project, TEST_MODEL, None).unwrap();

        // Every model variable should have a view element
        let element_names: HashSet<String> = result
            .elements
            .iter()
            .filter_map(|e| e.get_name().map(|n| canonicalize(n).into_owned()))
            .collect();

        for var in &model.variables {
            let ident = canonicalize(var.get_ident()).into_owned();
            assert!(
                element_names.contains(&ident),
                "missing view element for {}",
                ident
            );
        }
    }

    #[test]
    fn test_coordinates_positive() {
        let project = test_project(simple_model());
        let result = generate_layout(&project, TEST_MODEL, None).unwrap();

        for elem in &result.elements {
            match elem {
                ViewElement::Stock(s) => {
                    assert!(s.x >= 0.0, "stock {} has negative x: {}", s.name, s.x);
                    assert!(s.y >= 0.0, "stock {} has negative y: {}", s.name, s.y);
                }
                ViewElement::Flow(f) => {
                    assert!(f.x >= 0.0, "flow {} has negative x: {}", f.name, f.x);
                    assert!(f.y >= 0.0, "flow {} has negative y: {}", f.name, f.y);
                }
                ViewElement::Aux(a) => {
                    assert!(a.x >= 0.0, "aux {} has negative x: {}", a.name, a.x);
                    assert!(a.y >= 0.0, "aux {} has negative y: {}", a.name, a.y);
                }
                ViewElement::Cloud(c) => {
                    assert!(c.x >= 0.0, "cloud {} has negative x: {}", c.uid, c.x);
                    assert!(c.y >= 0.0, "cloud {} has negative y: {}", c.uid, c.y);
                }
                _ => {}
            }
        }
    }

    #[test]
    fn test_no_duplicate_uids() {
        let project = test_project(simple_model());
        let result = generate_layout(&project, TEST_MODEL, None).unwrap();

        let mut uids: HashSet<i32> = HashSet::new();
        for elem in &result.elements {
            let uid = elem.get_uid();
            assert!(uids.insert(uid), "duplicate UID: {}", uid);
        }
    }

    #[test]
    fn test_viewbox_encompasses_elements() {
        let project = test_project(simple_model());
        let result = generate_layout(&project, TEST_MODEL, None).unwrap();

        let vb = &result.view_box;
        assert!(vb.width > 0.0);
        assert!(vb.height > 0.0);

        for elem in &result.elements {
            match elem {
                ViewElement::Stock(s) => {
                    assert!(
                        s.x <= vb.x + vb.width,
                        "stock x {} exceeds viewbox width {}",
                        s.x,
                        vb.width
                    );
                    assert!(
                        s.y <= vb.y + vb.height,
                        "stock y {} exceeds viewbox height {}",
                        s.y,
                        vb.height
                    );
                }
                ViewElement::Flow(f) => {
                    assert!(f.x <= vb.x + vb.width);
                    assert!(f.y <= vb.y + vb.height);
                }
                ViewElement::Aux(a) => {
                    assert!(a.x <= vb.x + vb.width);
                    assert!(a.y <= vb.y + vb.height);
                }
                _ => {}
            }
        }
    }

    #[test]
    fn test_zoom_default() {
        let project = test_project(simple_model());
        let result = generate_layout(&project, TEST_MODEL, None).unwrap();
        assert_eq!(result.zoom, 1.0);
    }

    #[test]
    fn test_flow_points_attached() {
        let project = test_project(simple_model());
        let result = generate_layout(&project, TEST_MODEL, None).unwrap();

        for elem in &result.elements {
            if let ViewElement::Flow(flow) = elem {
                assert!(
                    flow.points.len() >= 2,
                    "flow {} has too few points",
                    flow.name
                );
                // At least one endpoint should be attached
                let has_attachment = flow.points.iter().any(|p| p.attached_to_uid.is_some());
                assert!(
                    has_attachment,
                    "flow {} has no attached endpoints",
                    flow.name
                );
            }
        }
    }

    #[test]
    fn test_compute_metadata_chains() {
        let project = test_project(simple_model());
        let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

        // Should detect one chain: population + births + deaths
        assert_eq!(metadata.chains.len(), 1);
        assert_eq!(metadata.chains[0].stocks.len(), 1);
        assert!(
            metadata.chains[0]
                .stocks
                .contains(&"population".to_string())
        );
        assert_eq!(metadata.chains[0].flows.len(), 2);
    }

    #[test]
    fn test_compute_metadata_dep_graph() {
        let project = test_project(simple_model());
        let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

        // births depends on population and birth_rate
        let births_deps = metadata.dep_graph.get("births").unwrap();
        assert!(births_deps.contains("population"));
        assert!(births_deps.contains("birth_rate"));
    }

    #[test]
    fn test_compute_metadata_constants() {
        let project = test_project(simple_model());
        let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

        // birth_rate and death_rate are constants (scalar equations with no variable references)
        assert!(metadata.is_constant("birth_rate"));
        assert!(metadata.is_constant("death_rate"));
    }

    #[test]
    fn test_detect_chains_multiple() {
        let mut stock_to_inflows: HashMap<String, Vec<String>> = HashMap::new();
        let mut stock_to_outflows: HashMap<String, Vec<String>> = HashMap::new();
        let mut flow_to_stocks: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
        let mut all_flows: BTreeSet<String> = BTreeSet::new();

        // Chain 1: A -> f1 -> B
        stock_to_inflows.insert("b".into(), vec!["f1".into()]);
        stock_to_outflows.insert("a".into(), vec!["f1".into()]);
        flow_to_stocks.insert("f1".into(), (Some("a".into()), Some("b".into())));
        all_flows.insert("f1".into());

        // Chain 2: C (isolated stock)
        stock_to_inflows.insert("c".into(), vec![]);
        stock_to_outflows.insert("c".into(), vec![]);

        let chains = detect_chains(
            &stock_to_inflows,
            &stock_to_outflows,
            &flow_to_stocks,
            &all_flows,
        );
        assert_eq!(chains.len(), 2);
    }

    #[test]
    fn test_is_structural_stock_flow_matches_dep_graph_direction() {
        // dep_graph stores stock -> flow (stock depends on its inflows/outflows).
        // is_structural_stock_flow(from=stock, to=flow) should return true.
        let stock_inflows: HashMap<String, HashSet<String>> =
            HashMap::from([("population".into(), HashSet::from(["births".into()]))]);
        let stock_outflows: HashMap<String, HashSet<String>> =
            HashMap::from([("population".into(), HashSet::from(["deaths".into()]))]);

        assert!(is_structural_stock_flow(
            "population",
            "births",
            &stock_inflows,
            &stock_outflows,
        ));
        assert!(is_structural_stock_flow(
            "population",
            "deaths",
            &stock_inflows,
            &stock_outflows,
        ));
        // Reversed direction should NOT match
        assert!(!is_structural_stock_flow(
            "births",
            "population",
            &stock_inflows,
            &stock_outflows,
        ));
        // Unrelated pair should not match
        assert!(!is_structural_stock_flow(
            "birth_rate",
            "births",
            &stock_inflows,
            &stock_outflows,
        ));
    }

    #[test]
    fn test_is_structural_flow_stock_matches_connector_direction() {
        // Connectors render as dependency -> dependent. For structural
        // stock-flow deps, the connector goes from flow -> stock.
        // is_structural_flow_stock(from=flow, to=stock) should return true.
        let stock_inflows: HashMap<String, HashSet<String>> =
            HashMap::from([("population".into(), HashSet::from(["births".into()]))]);
        let stock_outflows: HashMap<String, HashSet<String>> =
            HashMap::from([("population".into(), HashSet::from(["deaths".into()]))]);

        assert!(is_structural_flow_stock(
            "births",
            "population",
            &stock_inflows,
            &stock_outflows,
        ));
        assert!(is_structural_flow_stock(
            "deaths",
            "population",
            &stock_inflows,
            &stock_outflows,
        ));
        // Reversed direction should NOT match
        assert!(!is_structural_flow_stock(
            "population",
            "births",
            &stock_inflows,
            &stock_outflows,
        ));
    }

    #[test]
    fn test_contains_ident_word_boundary() {
        assert!(contains_ident("a + b * c", "b"));
        assert!(!contains_ident("abc", "b"));
        assert!(contains_ident("birth_rate * population", "birth_rate"));
        assert!(!contains_ident("high_birth_rate * x", "birth_rate"));
    }

    fn make_aux(ident: &str, equation: &str) -> datamodel::Variable {
        datamodel::Variable::Aux(datamodel::Aux {
            ident: ident.to_string(),
            equation: datamodel::Equation::Scalar(equation.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            compat: datamodel::Compat {
                visibility: datamodel::Visibility::Public,
                ..datamodel::Compat::default()
            },
            ai_state: None,
            uid: None,
        })
    }

    #[test]
    fn test_extract_equation_deps_simple() {
        let var = make_aux("births", "population * birth_rate");
        let idents: HashSet<String> = ["population", "birth_rate", "births", "death_rate"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut deps = extract_equation_deps(&var, &idents);
        deps.sort();
        assert_eq!(deps, vec!["birth_rate", "population"]);
    }

    #[test]
    fn test_extract_equation_deps_excludes_self() {
        let var = make_aux("x", "x + y");
        let idents: HashSet<String> = ["x", "y"].iter().map(|s| s.to_string()).collect();
        let deps = extract_equation_deps(&var, &idents);
        assert_eq!(deps, vec!["y"]);
    }

    #[test]
    fn test_extract_equation_deps_builtin_function() {
        let var = make_aux("result", "MAX(a, b)");
        let idents: HashSet<String> = ["a", "b", "result"].iter().map(|s| s.to_string()).collect();
        let mut deps = extract_equation_deps(&var, &idents);
        deps.sort();
        assert_eq!(deps, vec!["a", "b"]);
    }

    #[test]
    fn test_extract_equation_deps_if_then_else() {
        let var = make_aux("output", "IF THEN ELSE(flag > 0, alpha, beta)");
        let idents: HashSet<String> = ["flag", "alpha", "beta", "output"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut deps = extract_equation_deps(&var, &idents);
        deps.sort();
        assert_eq!(deps, vec!["alpha", "beta", "flag"]);
    }

    #[test]
    fn test_extract_equation_deps_no_equation() {
        let var = datamodel::Variable::Stock(datamodel::Stock {
            ident: "stock".to_string(),
            equation: datamodel::Equation::Scalar(String::new()),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            compat: datamodel::Compat {
                visibility: datamodel::Visibility::Public,
                ..datamodel::Compat::default()
            },
            ai_state: None,
            uid: None,
        });
        let idents: HashSet<String> = ["stock", "x"].iter().map(|s| s.to_string()).collect();
        let deps = extract_equation_deps(&var, &idents);
        assert!(deps.is_empty());
    }

    #[test]
    fn test_extract_equation_deps_arrayed_uses_all_entries() {
        let var = datamodel::Variable::Aux(datamodel::Aux {
            ident: "arr".to_string(),
            equation: datamodel::Equation::Arrayed(
                vec!["dim".to_string()],
                vec![
                    ("a".to_string(), "foo".to_string(), None, None),
                    ("b".to_string(), "bar".to_string(), None, None),
                ],
                None,
                false,
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            compat: datamodel::Compat {
                visibility: datamodel::Visibility::Public,
                ..datamodel::Compat::default()
            },
            ai_state: None,
            uid: None,
        });
        let idents: HashSet<String> = ["arr", "foo", "bar"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let mut deps = extract_equation_deps(&var, &idents);
        deps.sort();
        assert_eq!(deps, vec!["bar", "foo"]);
    }

    #[test]
    fn test_select_best_layout_fewest_crossings() {
        let results = vec![
            Ok(LayoutResult {
                view: datamodel::StockFlow {
                    name: None,
                    elements: Vec::new(),
                    view_box: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 100.0,
                    },
                    zoom: 1.0,
                    use_lettered_polarity: false,
                    font: None,
                    sketch_compat: None,
                },
                crossings: 5,
                seed: 42,
            }),
            Ok(LayoutResult {
                view: datamodel::StockFlow {
                    name: None,
                    elements: Vec::new(),
                    view_box: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 100.0,
                    },
                    zoom: 1.0,
                    use_lettered_polarity: false,
                    font: None,
                    sketch_compat: None,
                },
                crossings: 2,
                seed: 123,
            }),
        ];
        let best = select_best_layout(results).unwrap();
        // Should pick the one with 2 crossings
        assert!(best.elements.is_empty());
    }

    #[test]
    fn test_select_best_layout_lowest_seed_on_tie() {
        let results = vec![
            Ok(LayoutResult {
                view: datamodel::StockFlow {
                    name: None,
                    elements: vec![ViewElement::Aux(view_element::Aux {
                        name: "from_seed_123".to_string(),
                        uid: 1,
                        x: 0.0,
                        y: 0.0,
                        label_side: LabelSide::Bottom,
                        compat: None,
                    })],
                    view_box: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 100.0,
                    },
                    zoom: 1.0,
                    use_lettered_polarity: false,
                    font: None,
                    sketch_compat: None,
                },
                crossings: 3,
                seed: 123,
            }),
            Ok(LayoutResult {
                view: datamodel::StockFlow {
                    name: None,
                    elements: vec![ViewElement::Aux(view_element::Aux {
                        name: "from_seed_42".to_string(),
                        uid: 2,
                        x: 0.0,
                        y: 0.0,
                        label_side: LabelSide::Bottom,
                        compat: None,
                    })],
                    view_box: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 100.0,
                    },
                    zoom: 1.0,
                    use_lettered_polarity: false,
                    font: None,
                    sketch_compat: None,
                },
                crossings: 3,
                seed: 42,
            }),
        ];
        let best = select_best_layout(results).unwrap();
        // Should pick seed 42 (lower seed wins on tie)
        assert_eq!(best.elements.len(), 1);
        if let ViewElement::Aux(aux) = &best.elements[0] {
            assert_eq!(aux.name, "from_seed_42");
        } else {
            unreachable!("expected Aux element");
        }
    }

    #[test]
    fn test_generate_layout_aux_only() {
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "rate".to_string(),
                    equation: datamodel::Equation::Scalar("0.5".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "factor".to_string(),
                    equation: datamodel::Equation::Scalar("rate * 2".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(2),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };
        let project = test_project(model);
        let result = generate_layout(&project, TEST_MODEL, None).unwrap();
        assert_eq!(
            result
                .elements
                .iter()
                .filter(|e| matches!(e, ViewElement::Aux(_)))
                .count(),
            2
        );
    }

    #[test]
    fn test_generate_layout_single_aux() {
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "x".to_string(),
                equation: datamodel::Equation::Scalar("42".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat {
                    visibility: datamodel::Visibility::Public,
                    ..datamodel::Compat::default()
                },
                ai_state: None,
                uid: Some(1),
            })],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };
        let project = test_project(model);
        let result = generate_layout(&project, TEST_MODEL, None).unwrap();
        assert_eq!(result.elements.len(), 1);
    }

    #[test]
    fn test_generate_layout_disconnected_stocks() {
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "stock_a".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec![],
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "stock_b".to_string(),
                    equation: datamodel::Equation::Scalar("200".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec![],
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(2),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };
        let project = test_project(model);
        let result = generate_layout(&project, TEST_MODEL, None).unwrap();
        let stocks: Vec<_> = result
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Stock(_)))
            .collect();
        assert_eq!(stocks.len(), 2);
    }

    #[test]
    fn test_generate_layout_disconnected_chains_do_not_explode_apart() {
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "stock_a".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec![],
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "stock_b".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec![],
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(2),
                }),
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "stock_c".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec![],
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(3),
                }),
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "stock_d".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec![],
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(4),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };
        let project = test_project(model);
        let result = generate_layout(&project, TEST_MODEL, None).unwrap();

        let stock_positions: Vec<(f64, f64)> = result
            .elements
            .iter()
            .filter_map(|e| {
                if let ViewElement::Stock(s) = e {
                    Some((s.x, s.y))
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(stock_positions.len(), 4);

        let min_x = stock_positions
            .iter()
            .map(|(x, _)| *x)
            .fold(f64::INFINITY, f64::min);
        let max_x = stock_positions
            .iter()
            .map(|(x, _)| *x)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_y = stock_positions
            .iter()
            .map(|(_, y)| *y)
            .fold(f64::INFINITY, f64::min);
        let max_y = stock_positions
            .iter()
            .map(|(_, y)| *y)
            .fold(f64::NEG_INFINITY, f64::max);

        // Disconnected chains should remain in a reasonable neighborhood, not
        // be flung thousands of units apart by force configuration.
        assert!(
            max_x - min_x < 10_000.0,
            "x span too large: {}",
            max_x - min_x
        );
        assert!(
            max_y - min_y < 10_000.0,
            "y span too large: {}",
            max_y - min_y
        );
    }

    #[test]
    fn test_connector_direction_dependency_to_dependent() {
        let project = test_project(simple_model());
        let model = project.get_model(TEST_MODEL).unwrap();
        let result = generate_layout(&project, TEST_MODEL, None).unwrap();

        let uid_to_ident: HashMap<i32, String> = model
            .variables
            .iter()
            .filter_map(|var| match var {
                datamodel::Variable::Stock(s) => {
                    s.uid.map(|uid| (uid, canonicalize(&s.ident).into_owned()))
                }
                datamodel::Variable::Flow(f) => {
                    f.uid.map(|uid| (uid, canonicalize(&f.ident).into_owned()))
                }
                datamodel::Variable::Aux(a) => {
                    a.uid.map(|uid| (uid, canonicalize(&a.ident).into_owned()))
                }
                datamodel::Variable::Module(_) => None,
            })
            .collect();

        let link_pairs: HashSet<(String, String)> = result
            .elements
            .iter()
            .filter_map(|elem| {
                if let ViewElement::Link(link) = elem {
                    let from = uid_to_ident.get(&link.from_uid)?.clone();
                    let to = uid_to_ident.get(&link.to_uid)?.clone();
                    Some((from, to))
                } else {
                    None
                }
            })
            .collect();

        assert!(
            link_pairs.contains(&("birth_rate".to_string(), "births".to_string())),
            "expected dependency link birth_rate -> births"
        );
        assert!(
            !link_pairs.contains(&("births".to_string(), "birth_rate".to_string())),
            "did not expect reversed dependency link births -> birth_rate"
        );
    }

    #[test]
    fn test_compute_metadata_includes_isolated_flows_when_stocks_exist() {
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "stock".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec!["connected_flow".to_string()],
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "connected_flow".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(2),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "isolated_flow".to_string(),
                    equation: datamodel::Equation::Scalar("5".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(3),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };

        let project = test_project(model);
        let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

        assert!(
            metadata
                .chains
                .iter()
                .any(|chain| chain.flows.contains(&"isolated_flow".to_string())),
            "expected isolated_flow to be represented in some chain"
        );
    }

    #[test]
    fn test_generate_layout_includes_isolated_flows_when_stocks_exist() {
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "stock".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec!["connected_flow".to_string()],
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "connected_flow".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(2),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "isolated_flow".to_string(),
                    equation: datamodel::Equation::Scalar("5".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(3),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };

        let project = test_project(model);
        let result = generate_layout(&project, TEST_MODEL, None).unwrap();
        let flow_count = result
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Flow(_)))
            .count();
        assert_eq!(flow_count, 2, "expected both flows to be laid out");
    }

    #[test]
    fn test_generate_layout_includes_module_elements_and_connectors() {
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Module(datamodel::Module {
                    ident: "m".to_string(),
                    model_name: "submodel".to_string(),
                    documentation: String::new(),
                    units: None,
                    references: Vec::new(),
                    ai_state: None,
                    uid: Some(2),
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..Default::default()
                    },
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "y".to_string(),
                    equation: datamodel::Equation::Scalar("x + m".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(3),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };

        let project = test_project(model);
        let result = generate_layout(&project, TEST_MODEL, None).unwrap();
        let element_uids: HashSet<i32> = result.elements.iter().map(|e| e.get_uid()).collect();

        // All link endpoints should reference rendered elements
        for elem in &result.elements {
            if let ViewElement::Link(link) = elem {
                assert!(
                    element_uids.contains(&link.from_uid),
                    "link from_uid {} should reference a rendered element",
                    link.from_uid
                );
                assert!(
                    element_uids.contains(&link.to_uid),
                    "link to_uid {} should reference a rendered element",
                    link.to_uid
                );
            }
        }

        // Module should produce a ViewElement::Module
        let module_count = result
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Module(_)))
            .count();
        assert_eq!(module_count, 1, "module 'm' should be rendered");

        // Auxiliaries should still be present
        let aux_count = result
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Aux(_)))
            .count();
        assert_eq!(aux_count, 2, "both auxiliaries should be rendered");

        // Module should have finite coordinates from SFDP
        for elem in &result.elements {
            if let ViewElement::Module(m) = elem {
                assert!(
                    m.x.is_finite() && m.y.is_finite(),
                    "module '{}' should have finite coordinates",
                    m.name
                );
            }
        }
    }

    #[test]
    fn test_count_view_crossings_shared_endpoint_bidirectional_links() {
        let view = datamodel::StockFlow {
            name: None,
            elements: vec![
                ViewElement::Aux(view_element::Aux {
                    name: "a".to_string(),
                    uid: 1,
                    x: 0.0,
                    y: 0.0,
                    label_side: LabelSide::Bottom,
                    compat: None,
                }),
                ViewElement::Aux(view_element::Aux {
                    name: "b".to_string(),
                    uid: 2,
                    x: 10.0,
                    y: 10.0,
                    label_side: LabelSide::Bottom,
                    compat: None,
                }),
                ViewElement::Aux(view_element::Aux {
                    name: "c".to_string(),
                    uid: 3,
                    x: 10.0,
                    y: -10.0,
                    label_side: LabelSide::Bottom,
                    compat: None,
                }),
                ViewElement::Link(view_element::Link {
                    uid: 4,
                    from_uid: 1,
                    to_uid: 2,
                    shape: LinkShape::Straight,
                    polarity: None,
                }),
                ViewElement::Link(view_element::Link {
                    uid: 5,
                    from_uid: 3,
                    to_uid: 1,
                    shape: LinkShape::Straight,
                    polarity: None,
                }),
            ],
            view_box: Rect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            zoom: 1.0,
            use_lettered_polarity: false,
            font: None,
            sketch_compat: None,
        };

        assert_eq!(count_view_crossings(&view), 0);
    }

    #[test]
    fn test_compute_metadata_populates_feedback_loops_from_model_metadata() {
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("y".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "y".to_string(),
                    equation: datamodel::Equation::Scalar("x".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..datamodel::Compat::default()
                    },
                    ai_state: None,
                    uid: Some(2),
                }),
            ],
            views: Vec::new(),
            loop_metadata: vec![datamodel::LoopMetadata {
                uids: vec![1, 2],
                deleted: false,
                name: "R1".to_string(),
                description: String::new(),
            }],
            groups: Vec::new(),
        };

        let project = test_project(model);
        let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();
        assert_eq!(metadata.feedback_loops.len(), 1);
        assert_eq!(metadata.feedback_loops[0].name, "R1");
        assert_eq!(
            metadata.feedback_loops[0].causal_chain(),
            &["x".to_string(), "y".to_string(), "x".to_string()]
        );
    }

    #[test]
    fn test_chain_importance_formula() {
        let mut stock_to_inflows: HashMap<String, Vec<String>> = HashMap::new();
        let mut stock_to_outflows: HashMap<String, Vec<String>> = HashMap::new();
        let mut flow_to_stocks: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
        let mut all_flows: BTreeSet<String> = BTreeSet::new();

        // Chain: s1 -> f1 -> s2 -> f2 -> (none), f3 -> s2
        // 2 stocks + 3 flows = 2*10 + 3*5 = 35
        stock_to_outflows.insert("s1".into(), vec!["f1".into()]);
        stock_to_inflows.insert("s2".into(), vec!["f1".into(), "f3".into()]);
        stock_to_outflows.insert("s2".into(), vec!["f2".into()]);
        stock_to_inflows.insert("s1".into(), vec![]);
        flow_to_stocks.insert("f1".into(), (Some("s1".into()), Some("s2".into())));
        flow_to_stocks.insert("f2".into(), (Some("s2".into()), None));
        flow_to_stocks.insert("f3".into(), (None, Some("s2".into())));
        all_flows.extend(["f1".into(), "f2".into(), "f3".into()]);

        let chains = detect_chains(
            &stock_to_inflows,
            &stock_to_outflows,
            &flow_to_stocks,
            &all_flows,
        );
        assert_eq!(chains.len(), 1);
        assert!((chains[0].importance - 35.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_chains_sorted_descending() {
        let mut stock_to_inflows: HashMap<String, Vec<String>> = HashMap::new();
        let mut stock_to_outflows: HashMap<String, Vec<String>> = HashMap::new();
        let mut flow_to_stocks: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
        let mut all_flows: BTreeSet<String> = BTreeSet::new();

        // Chain 1: 1 stock + 1 flow = 15
        stock_to_outflows.insert("a".into(), vec!["fa".into()]);
        stock_to_inflows.insert("a".into(), vec![]);
        flow_to_stocks.insert("fa".into(), (Some("a".into()), None));
        all_flows.insert("fa".into());

        // Chain 2: 2 stocks + 1 flow = 25
        stock_to_outflows.insert("b".into(), vec!["fb".into()]);
        stock_to_inflows.insert("b".into(), vec![]);
        stock_to_inflows.insert("c".into(), vec!["fb".into()]);
        stock_to_outflows.insert("c".into(), vec![]);
        flow_to_stocks.insert("fb".into(), (Some("b".into()), Some("c".into())));
        all_flows.insert("fb".into());

        let chains = detect_chains(
            &stock_to_inflows,
            &stock_to_outflows,
            &flow_to_stocks,
            &all_flows,
        );
        assert_eq!(chains.len(), 2);
        assert!(
            chains[0].importance >= chains[1].importance,
            "chains should be sorted descending by importance: {} vs {}",
            chains[0].importance,
            chains[1].importance
        );
        assert!((chains[0].importance - 25.0).abs() < f64::EPSILON);
        assert!((chains[1].importance - 15.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_isolated_flow_importance_is_5() {
        let stock_to_inflows: HashMap<String, Vec<String>> = HashMap::new();
        let stock_to_outflows: HashMap<String, Vec<String>> = HashMap::new();
        let flow_to_stocks: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
        let mut all_flows: BTreeSet<String> = BTreeSet::new();
        all_flows.insert("lonely_flow".into());

        let chains = detect_chains(
            &stock_to_inflows,
            &stock_to_outflows,
            &flow_to_stocks,
            &all_flows,
        );
        assert_eq!(chains.len(), 1);
        assert!((chains[0].importance - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_ast_deps_exclude_builtins() {
        // A variable referencing TIME (a builtin) should not produce a connector
        // to TIME since TIME is not a model variable.
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "rate".to_string(),
                    equation: datamodel::Equation::Scalar("0.1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: Some(1),
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..Default::default()
                    },
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "output".to_string(),
                    equation: datamodel::Equation::Scalar("rate * TIME".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: Some(2),
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..Default::default()
                    },
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };
        let project = test_project(model);
        let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

        let output_deps = metadata.dep_graph.get("output").unwrap();
        assert!(output_deps.contains("rate"), "output should depend on rate");
        assert!(
            !output_deps.contains("time"),
            "output should NOT depend on builtin TIME"
        );
    }

    #[test]
    fn test_ast_deps_no_false_positives() {
        // String heuristic would falsely match "birth" inside "birthday".
        // AST-based extraction should not.
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "birth".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: Some(1),
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..Default::default()
                    },
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "birthday".to_string(),
                    equation: datamodel::Equation::Scalar("365".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: Some(2),
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..Default::default()
                    },
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "output".to_string(),
                    equation: datamodel::Equation::Scalar("birthday + 1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: Some(3),
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..Default::default()
                    },
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };
        let project = test_project(model);
        let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

        let output_deps = metadata.dep_graph.get("output").unwrap();
        assert!(
            output_deps.contains("birthday"),
            "output should depend on birthday"
        );
        assert!(
            !output_deps.contains("birth"),
            "output should NOT falsely depend on birth (substring match)"
        );
    }

    #[test]
    fn test_deps_fallback_on_compile_error() {
        // A model with a module referencing a nonexistent submodel should
        // gracefully fall back to string heuristic for dep extraction.
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: Some(1),
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..Default::default()
                    },
                }),
                datamodel::Variable::Module(datamodel::Module {
                    ident: "m".to_string(),
                    model_name: "nonexistent_model".to_string(),
                    documentation: String::new(),
                    units: None,
                    references: Vec::new(),
                    ai_state: None,
                    uid: Some(2),
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..Default::default()
                    },
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "y".to_string(),
                    equation: datamodel::Equation::Scalar("x + 1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: Some(3),
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..Default::default()
                    },
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };
        let project = test_project(model);
        let result = generate_layout(&project, TEST_MODEL, None);
        assert!(
            result.is_ok(),
            "layout should succeed despite compile error, via fallback"
        );
    }

    #[test]
    fn test_ltm_fallback_on_sim_error() {
        // A model with loop_metadata but that can't be simulated should
        // fall back to persisted loop_metadata UIDs for feedback loops.
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("y".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: Some(1),
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..Default::default()
                    },
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "y".to_string(),
                    equation: datamodel::Equation::Scalar("x".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: Some(2),
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..Default::default()
                    },
                }),
            ],
            views: Vec::new(),
            loop_metadata: vec![datamodel::LoopMetadata {
                uids: vec![1, 2],
                deleted: false,
                name: "R1".to_string(),
                description: String::new(),
            }],
            groups: Vec::new(),
        };
        let project = test_project(model);
        let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

        // Should have fallen back to persisted metadata since this minimal
        // model doesn't have sim_specs and can't simulate.
        assert_eq!(metadata.feedback_loops.len(), 1);
        assert_eq!(metadata.feedback_loops[0].name, "R1");
    }

    #[test]
    fn test_compute_metadata_returns_none_for_unknown_model() {
        let project = test_project(simple_model());
        assert!(compute_metadata(&project, "nonexistent", None).is_none());
    }

    #[test]
    fn test_compute_metadata_falls_back_for_invalid_equation() {
        // A variable with an unparseable equation should still get
        // string-heuristic dependencies rather than being treated as a
        // constant.
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: Some(1),
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..Default::default()
                    },
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "births".to_string(),
                    equation: datamodel::Equation::Scalar(
                        "population *** totally_broken_syntax".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: Some(2),
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..Default::default()
                    },
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };
        let project = test_project(model);
        let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

        let births_deps = metadata.dep_graph.get("births").unwrap();
        assert!(
            births_deps.contains("population"),
            "births should depend on population via string-heuristic fallback, got: {:?}",
            births_deps,
        );
        assert!(
            !metadata.constants.contains("births"),
            "births should not be classified as a constant",
        );

        // population should also depend on births via structural inflows
        let pop_deps = metadata.dep_graph.get("population").unwrap();
        assert!(
            pop_deps.contains("births"),
            "population should depend on births via structural inflows, got: {:?}",
            pop_deps,
        );
    }

    #[test]
    fn test_compute_metadata_excludes_non_model_deps() {
        // Equation text that mentions an identifier not in the model
        // (simulating a module output reference like "m·out" that
        // resolve_non_private_dependencies might pass through).
        // The dep graph should only contain identifiers for actual
        // rendered model variables.
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("phantom + 1".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: Some(1),
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..Default::default()
                    },
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "y".to_string(),
                    equation: datamodel::Equation::Scalar("x * 2".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: Some(2),
                    compat: datamodel::Compat {
                        visibility: datamodel::Visibility::Public,
                        ..Default::default()
                    },
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };
        let project = test_project(model);
        let metadata = compute_metadata(&project, TEST_MODEL, None).unwrap();

        // "phantom" is not a model variable so should not appear in the dep graph
        let all_graph_nodes: BTreeSet<&String> = metadata
            .dep_graph
            .keys()
            .chain(metadata.dep_graph.values().flat_map(|deps| deps.iter()))
            .collect();
        assert!(
            !all_graph_nodes.contains(&"phantom".to_string()),
            "dep graph should not contain non-model identifiers, got: {:?}",
            all_graph_nodes,
        );

        // y should still depend on x (a real model variable)
        let y_deps = metadata.dep_graph.get("y").unwrap();
        assert!(
            y_deps.contains("x"),
            "y should depend on x, got: {:?}",
            y_deps,
        );

        // x should be a constant since its only dep (phantom) was filtered
        assert!(
            metadata.constants.contains("x"),
            "x should be a constant after filtering non-model deps",
        );
    }

    #[test]
    fn test_resolve_model_name_returns_actual_name_for_main_alias() {
        let model = datamodel::Model {
            name: String::new(),
            sim_specs: None,
            variables: Vec::new(),
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };
        let project = test_project(model);
        // "main" should resolve to the empty string (the actual model name)
        assert_eq!(resolve_model_name(&project, "main"), "");
    }

    #[test]
    fn test_resolve_model_name_passthrough_for_named_model() {
        let project = test_project(simple_model());
        assert_eq!(resolve_model_name(&project, TEST_MODEL), TEST_MODEL);
    }

    #[test]
    fn test_resolve_model_name_passthrough_for_unknown_model() {
        let project = test_project(simple_model());
        assert_eq!(resolve_model_name(&project, "nonexistent"), "nonexistent");
    }

    #[test]
    fn test_layout_chain() {
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births".to_string()],
                    outflows: vec![],
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "births".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(2),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };

        let config = LayoutConfig::default();

        let mut metadata = ComputedMetadata::new_empty();
        metadata
            .flow_to_stocks
            .insert("births".to_string(), (None, Some("population".to_string())));
        metadata
            .stock_to_inflows
            .insert("population".to_string(), vec!["births".to_string()]);

        let mut state = LayoutState::new(&model);
        let stocks = vec!["population".to_string()];
        let flows = vec!["births".to_string()];

        layout_chain(
            &mut state,
            &config,
            &metadata,
            &stocks,
            &flows,
            Position::new(100.0, 100.0),
        )
        .unwrap();

        // 1 stock + 1 flow + 1 cloud (births has no from_stock, so a source cloud is created)
        let stock_count = state
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Stock(_)))
            .count();
        let flow_count = state
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Flow(_)))
            .count();
        let cloud_count = state
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Cloud(_)))
            .count();
        assert_eq!(stock_count, 1);
        assert_eq!(flow_count, 1);
        assert_eq!(
            cloud_count, 1,
            "births has no from_stock, so one cloud expected"
        );

        // Stock and flow are present by name
        let element_names: HashSet<String> = state
            .elements
            .iter()
            .filter_map(|e| e.get_name().map(|n| canonicalize(n).into_owned()))
            .collect();
        assert!(
            element_names.contains("population"),
            "stock 'population' missing"
        );
        assert!(element_names.contains("births"), "flow 'births' missing");

        // All elements have finite coordinates (normalization happens
        // later in the pipeline, so pre-normalization clouds at unconnected
        // endpoints may have negative coordinates).
        for elem in &state.elements {
            match elem {
                ViewElement::Stock(s) => {
                    assert!(s.x.is_finite(), "stock {} has non-finite x", s.name);
                    assert!(s.y.is_finite(), "stock {} has non-finite y", s.name);
                }
                ViewElement::Flow(f) => {
                    assert!(f.x.is_finite(), "flow {} has non-finite x", f.name);
                    assert!(f.y.is_finite(), "flow {} has non-finite y", f.name);
                }
                ViewElement::Cloud(c) => {
                    assert!(c.x.is_finite(), "cloud {} has non-finite x", c.uid);
                    assert!(c.y.is_finite(), "cloud {} has non-finite y", c.uid);
                }
                _ => {}
            }
        }

        // UIDs are unique
        let mut uids: HashSet<i32> = HashSet::new();
        for elem in &state.elements {
            let uid = elem.get_uid();
            assert!(uids.insert(uid), "duplicate UID: {}", uid);
        }

        // Flow is positioned between the stock and the cloud endpoint.
        // With (None, Some("population")), the flow should be offset left
        // of the stock position (inflow from cloud).
        let stock_elem = state.elements.iter().find_map(|e| {
            if let ViewElement::Stock(s) = e {
                Some(s)
            } else {
                None
            }
        });
        let flow_elem = state.elements.iter().find_map(|e| {
            if let ViewElement::Flow(f) = e {
                Some(f)
            } else {
                None
            }
        });
        let stock_elem = stock_elem.expect("stock element should exist");
        let flow_elem = flow_elem.expect("flow element should exist");

        // The flow's x should differ from the stock's x (not stacked on top)
        assert!(
            (flow_elem.x - stock_elem.x).abs() > 1.0,
            "flow should be offset from stock, got flow.x={} stock.x={}",
            flow_elem.x,
            stock_elem.x
        );

        // Flow points should reference the stock via attached_to_uid
        let attached_uids: Vec<Option<i32>> = flow_elem
            .points
            .iter()
            .map(|pt| pt.attached_to_uid)
            .collect();
        assert!(
            attached_uids.contains(&Some(stock_elem.uid)),
            "at least one flow point should be attached to the stock uid, got {:?}",
            attached_uids
        );
    }

    #[test]
    fn test_build_clouds() {
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births".to_string()],
                    outflows: vec![],
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "births".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(2),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };

        let mut metadata = ComputedMetadata::new_empty();
        // births flows into population; no source stock, so a source cloud is needed
        metadata
            .flow_to_stocks
            .insert("births".to_string(), (None, Some("population".to_string())));
        metadata
            .stock_to_inflows
            .insert("population".to_string(), vec!["births".to_string()]);

        let mut state = LayoutState::new(&model);

        let flow_uid = state.get_or_alloc_uid("births");
        let stock_uid = state.get_or_alloc_uid("population");

        // Simulate post-layout_chain state: a flow element already exists
        // with points at known positions.  The first point (source end) has
        // no attached stock; the second point (sink end) is attached to the
        // population stock.
        let mut flow_elem = view_element::Flow {
            name: "births".to_string(),
            uid: flow_uid,
            x: 75.0,
            y: 100.0,
            label_side: view_element::LabelSide::Top,
            points: vec![
                view_element::FlowPoint {
                    x: 50.0,
                    y: 100.0,
                    attached_to_uid: None,
                },
                view_element::FlowPoint {
                    x: 100.0,
                    y: 100.0,
                    attached_to_uid: Some(stock_uid),
                },
            ],
            compat: None,
            label_compat: None,
        };

        let clouds_before = state
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Cloud(_)))
            .count();
        assert_eq!(
            clouds_before, 0,
            "no clouds before calling build_clouds_for_flow"
        );

        build_clouds_for_flow(&mut state, &metadata, "births", &mut flow_elem);

        // Exactly one cloud should be created (source end only; sink is connected)
        let clouds: Vec<&view_element::Cloud> = state
            .elements
            .iter()
            .filter_map(|e| {
                if let ViewElement::Cloud(c) = e {
                    Some(c)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            clouds.len(),
            1,
            "expected 1 cloud for the missing source endpoint"
        );

        let cloud = clouds[0];
        // The cloud's flow_uid must reference the flow
        assert_eq!(
            cloud.flow_uid, flow_uid,
            "cloud's flow_uid should match the flow"
        );

        // The cloud should be positioned at the first flow point (source end)
        assert!(
            (cloud.x - 50.0).abs() < f64::EPSILON,
            "cloud x should match source flow point"
        );
        assert!(
            (cloud.y - 100.0).abs() < f64::EPSILON,
            "cloud y should match source flow point"
        );

        // The first flow point (source) should now be attached to the cloud
        assert_eq!(
            flow_elem.points[0].attached_to_uid,
            Some(cloud.uid),
            "source flow point should be attached to the new cloud"
        );

        // The second flow point (sink) should remain attached to the stock
        assert_eq!(
            flow_elem.points[1].attached_to_uid,
            Some(stock_uid),
            "sink flow point should still be attached to the stock"
        );

        // Cloud UID must be unique (different from flow and stock UIDs)
        let mut uids: HashSet<i32> = HashSet::new();
        uids.insert(flow_uid);
        uids.insert(stock_uid);
        assert!(
            uids.insert(cloud.uid),
            "cloud UID {} should be unique",
            cloud.uid
        );
    }

    #[test]
    fn test_build_clouds_no_cloud_for_connected_endpoint() {
        // When both endpoints are connected to stocks, no clouds should be
        // created at all.
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "source".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec!["transfer".to_string()],
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "sink".to_string(),
                    equation: datamodel::Equation::Scalar("0".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["transfer".to_string()],
                    outflows: vec![],
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(2),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "transfer".to_string(),
                    equation: datamodel::Equation::Scalar("5".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(3),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };

        let mut metadata = ComputedMetadata::new_empty();
        metadata.flow_to_stocks.insert(
            "transfer".to_string(),
            (Some("source".to_string()), Some("sink".to_string())),
        );

        let mut state = LayoutState::new(&model);
        let flow_uid = state.get_or_alloc_uid("transfer");
        let source_uid = state.get_or_alloc_uid("source");
        let sink_uid = state.get_or_alloc_uid("sink");

        let mut flow_elem = view_element::Flow {
            name: "transfer".to_string(),
            uid: flow_uid,
            x: 75.0,
            y: 100.0,
            label_side: view_element::LabelSide::Top,
            points: vec![
                view_element::FlowPoint {
                    x: 50.0,
                    y: 100.0,
                    attached_to_uid: Some(source_uid),
                },
                view_element::FlowPoint {
                    x: 100.0,
                    y: 100.0,
                    attached_to_uid: Some(sink_uid),
                },
            ],
            compat: None,
            label_compat: None,
        };

        build_clouds_for_flow(&mut state, &metadata, "transfer", &mut flow_elem);

        let cloud_count = state
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Cloud(_)))
            .count();
        assert_eq!(
            cloud_count, 0,
            "no clouds when both endpoints are connected to stocks"
        );
    }

    #[test]
    fn test_build_clouds_sink_cloud() {
        // When the flow has a source stock but no sink stock, a sink cloud
        // should be created at the last flow point.
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec!["deaths".to_string()],
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "deaths".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(2),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };

        let mut metadata = ComputedMetadata::new_empty();
        // deaths flows out of population; no sink stock
        metadata
            .flow_to_stocks
            .insert("deaths".to_string(), (Some("population".to_string()), None));
        metadata
            .stock_to_outflows
            .insert("population".to_string(), vec!["deaths".to_string()]);

        let mut state = LayoutState::new(&model);
        let flow_uid = state.get_or_alloc_uid("deaths");
        let stock_uid = state.get_or_alloc_uid("population");

        let mut flow_elem = view_element::Flow {
            name: "deaths".to_string(),
            uid: flow_uid,
            x: 125.0,
            y: 100.0,
            label_side: view_element::LabelSide::Top,
            points: vec![
                view_element::FlowPoint {
                    x: 100.0,
                    y: 100.0,
                    attached_to_uid: Some(stock_uid),
                },
                view_element::FlowPoint {
                    x: 150.0,
                    y: 100.0,
                    attached_to_uid: None,
                },
            ],
            compat: None,
            label_compat: None,
        };

        build_clouds_for_flow(&mut state, &metadata, "deaths", &mut flow_elem);

        let clouds: Vec<&view_element::Cloud> = state
            .elements
            .iter()
            .filter_map(|e| {
                if let ViewElement::Cloud(c) = e {
                    Some(c)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(clouds.len(), 1, "expected 1 cloud for the missing sink");

        let cloud = clouds[0];
        assert_eq!(cloud.flow_uid, flow_uid);

        // Cloud should be at the last (sink) flow point
        assert!(
            (cloud.x - 150.0).abs() < f64::EPSILON,
            "cloud x should match sink flow point"
        );
        assert!(
            (cloud.y - 100.0).abs() < f64::EPSILON,
            "cloud y should match sink flow point"
        );

        // Source flow point should remain attached to the stock
        assert_eq!(flow_elem.points[0].attached_to_uid, Some(stock_uid));

        // Sink flow point should now be attached to the cloud
        assert_eq!(flow_elem.points[1].attached_to_uid, Some(cloud.uid));
    }

    #[test]
    fn test_build_connectors() {
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births".to_string()],
                    outflows: vec![],
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "births".to_string(),
                    equation: datamodel::Equation::Scalar("population * birth_rate".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(2),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "birth_rate".to_string(),
                    equation: datamodel::Equation::Scalar("0.03".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(3),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };

        let mut metadata = ComputedMetadata::new_empty();
        // births depends on birth_rate and population
        metadata.dep_graph.insert(
            "births".to_string(),
            vec!["birth_rate".to_string(), "population".to_string()]
                .into_iter()
                .collect(),
        );
        // population depends on births (structural inflow)
        metadata.dep_graph.insert(
            "population".to_string(),
            vec!["births".to_string()].into_iter().collect(),
        );
        metadata
            .stock_to_inflows
            .insert("population".to_string(), vec!["births".to_string()]);
        metadata
            .flow_to_stocks
            .insert("births".to_string(), (None, Some("population".to_string())));

        let mut state = LayoutState::new(&model);

        // Pre-populate positioned view elements (simulating post-layout state)
        let pop_uid = state.get_or_alloc_uid("population");
        let births_uid = state.get_or_alloc_uid("births");
        let br_uid = state.get_or_alloc_uid("birth_rate");

        state.elements.push(ViewElement::Stock(view_element::Stock {
            name: "population".to_string(),
            uid: pop_uid,
            x: 200.0,
            y: 100.0,
            label_side: view_element::LabelSide::Bottom,
            compat: None,
        }));
        state.positions.insert(pop_uid, Position::new(200.0, 100.0));

        state.elements.push(ViewElement::Flow(view_element::Flow {
            name: "births".to_string(),
            uid: births_uid,
            x: 150.0,
            y: 100.0,
            label_side: view_element::LabelSide::Top,
            points: vec![
                FlowPoint {
                    x: 100.0,
                    y: 100.0,
                    attached_to_uid: None,
                },
                FlowPoint {
                    x: 200.0,
                    y: 100.0,
                    attached_to_uid: Some(pop_uid),
                },
            ],
            compat: None,
            label_compat: None,
        }));
        state
            .positions
            .insert(births_uid, Position::new(150.0, 100.0));

        state.elements.push(ViewElement::Aux(view_element::Aux {
            name: "birth_rate".to_string(),
            uid: br_uid,
            x: 150.0,
            y: 50.0,
            label_side: view_element::LabelSide::Top,
            compat: None,
        }));
        state.positions.insert(br_uid, Position::new(150.0, 50.0));

        build_connectors(&mut state, &model, &metadata).unwrap();

        // Collect all Link elements
        let links: Vec<&view_element::Link> = state
            .elements
            .iter()
            .filter_map(|e| {
                if let ViewElement::Link(l) = e {
                    Some(l)
                } else {
                    None
                }
            })
            .collect();

        // Non-structural edge: birth_rate -> births should produce a link
        let br_to_births = links
            .iter()
            .find(|l| l.from_uid == br_uid && l.to_uid == births_uid);
        assert!(
            br_to_births.is_some(),
            "expected a link from birth_rate -> births"
        );

        // Structural stock-flow edge: births -> population (inflow
        // relationship) should NOT produce a link (already represented by
        // the flow pipe).
        let births_to_pop = links
            .iter()
            .find(|l| l.from_uid == births_uid && l.to_uid == pop_uid);
        assert!(
            births_to_pop.is_none(),
            "structural flow->stock edge should not produce a link"
        );

        // The stock->flow structural edge (population -> births) should
        // create an arc link, not be skipped.  Verify it exists with an
        // arc shape.
        let pop_to_births = links
            .iter()
            .find(|l| l.from_uid == pop_uid && l.to_uid == births_uid);
        assert!(
            pop_to_births.is_some(),
            "stock->flow structural edge should produce an arc link"
        );
        assert!(
            matches!(pop_to_births.unwrap().shape, LinkShape::Arc(_)),
            "stock->flow link should have arc shape"
        );

        // Every link's from_uid and to_uid should reference existing
        // element UIDs in the state.
        let all_uids: HashSet<i32> = state.elements.iter().map(|e| e.get_uid()).collect();
        for link in &links {
            assert!(
                all_uids.contains(&link.from_uid),
                "link from_uid {} not found in elements",
                link.from_uid
            );
            assert!(
                all_uids.contains(&link.to_uid),
                "link to_uid {} not found in elements",
                link.to_uid
            );
        }

        // No duplicate links (each (from_uid, to_uid) pair appears at most once)
        let link_pairs: HashSet<(i32, i32)> =
            links.iter().map(|l| (l.from_uid, l.to_uid)).collect();
        assert_eq!(link_pairs.len(), links.len(), "duplicate links detected");
    }

    #[test]
    fn test_optimize_labels() {
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "aux_a".to_string(),
                    equation: datamodel::Equation::Scalar("aux_b".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "aux_b".to_string(),
                    equation: datamodel::Equation::Scalar("10".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(2),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };

        let mut metadata = ComputedMetadata::new_empty();
        // aux_a depends on aux_b
        metadata.dep_graph.insert(
            "aux_a".to_string(),
            vec!["aux_b".to_string()].into_iter().collect(),
        );
        // reverse: aux_b is used by aux_a
        metadata.reverse_dep_graph.insert(
            "aux_b".to_string(),
            vec!["aux_a".to_string()].into_iter().collect(),
        );

        let mut state = LayoutState::new(&model);
        let a_uid = state.get_or_alloc_uid("aux_a");
        let b_uid = state.get_or_alloc_uid("aux_b");

        // aux_a below aux_b: the connection runs downward from aux_b and
        // upward into aux_a.  From each variable's perspective the connector
        // leaves toward the bottom, making Bottom a poor label position and
        // causing the optimizer to prefer Top.
        state.elements.push(ViewElement::Aux(view_element::Aux {
            name: "aux_a".to_string(),
            uid: a_uid,
            x: 100.0,
            y: 200.0,
            label_side: view_element::LabelSide::Bottom,
            compat: None,
        }));
        state.positions.insert(a_uid, Position::new(100.0, 200.0));

        state.elements.push(ViewElement::Aux(view_element::Aux {
            name: "aux_b".to_string(),
            uid: b_uid,
            x: 100.0,
            y: 100.0,
            label_side: view_element::LabelSide::Bottom,
            compat: None,
        }));
        state.positions.insert(b_uid, Position::new(100.0, 100.0));

        let orig_a = (100.0_f64, 200.0_f64);
        let orig_b = (100.0_f64, 100.0_f64);

        optimize_labels(&mut state, &model, &metadata);

        // At least one element's label_side should have moved away from
        // Bottom, because each aux has a connection in the downward or
        // upward direction that makes Bottom suboptimal.
        let sides: Vec<view_element::LabelSide> = state
            .elements
            .iter()
            .filter_map(|e| {
                if let ViewElement::Aux(a) = e {
                    Some(a.label_side)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(sides.len(), 2, "should still have 2 aux elements");
        let any_changed = sides.iter().any(|&s| s != view_element::LabelSide::Bottom);
        assert!(
            any_changed,
            "optimize_labels should adjust at least one label_side from the default; got {:?}",
            sides
        );

        // Element positions must not change -- optimize_labels moves
        // labels, not the elements themselves.
        for elem in &state.elements {
            if let ViewElement::Aux(a) = elem {
                if a.name == "aux_a" {
                    assert_eq!(a.x, orig_a.0, "aux_a x should be unchanged");
                    assert_eq!(a.y, orig_a.1, "aux_a y should be unchanged");
                } else if a.name == "aux_b" {
                    assert_eq!(a.x, orig_b.0, "aux_b x should be unchanged");
                    assert_eq!(a.y, orig_b.1, "aux_b y should be unchanged");
                }
            }
        }
    }

    #[test]
    fn test_place_auxiliaries() {
        let model = datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births".to_string()],
                    outflows: vec![],
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "births".to_string(),
                    equation: datamodel::Equation::Scalar("population * birth_rate".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(2),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "birth_rate".to_string(),
                    equation: datamodel::Equation::Scalar("0.03".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    compat: datamodel::Compat::default(),
                    ai_state: None,
                    uid: Some(3),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };

        let config = LayoutConfig::default();

        let mut metadata = ComputedMetadata::new_empty();
        // births depends on birth_rate and population
        metadata.dep_graph.insert(
            "births".to_string(),
            vec!["birth_rate".to_string(), "population".to_string()]
                .into_iter()
                .collect(),
        );
        // population depends on births (structural inflow)
        metadata.dep_graph.insert(
            "population".to_string(),
            vec!["births".to_string()].into_iter().collect(),
        );
        metadata
            .stock_to_inflows
            .insert("population".to_string(), vec!["births".to_string()]);
        metadata
            .flow_to_stocks
            .insert("births".to_string(), (None, Some("population".to_string())));

        let mut state = LayoutState::new(&model);

        // Pre-populate stock and flow view elements (simulating
        // post-layout_chain state).
        let pop_uid = state.get_or_alloc_uid("population");
        let births_uid = state.get_or_alloc_uid("births");

        let stock_x = 200.0_f64;
        let stock_y = 100.0_f64;
        let flow_x = 150.0_f64;
        let flow_y = 100.0_f64;

        state.elements.push(ViewElement::Stock(view_element::Stock {
            name: "population".to_string(),
            uid: pop_uid,
            x: stock_x,
            y: stock_y,
            label_side: view_element::LabelSide::Bottom,
            compat: None,
        }));
        state
            .positions
            .insert(pop_uid, Position::new(stock_x, stock_y));

        // A cloud at the source end of the flow (births has no from_stock)
        let cloud_uid = state.uid_manager.alloc("");
        state.elements.push(ViewElement::Cloud(view_element::Cloud {
            uid: cloud_uid,
            flow_uid: births_uid,
            x: 100.0,
            y: 100.0,
            compat: None,
        }));
        state
            .positions
            .insert(cloud_uid, Position::new(100.0, 100.0));

        state.elements.push(ViewElement::Flow(view_element::Flow {
            name: "births".to_string(),
            uid: births_uid,
            x: flow_x,
            y: flow_y,
            label_side: view_element::LabelSide::Top,
            points: vec![
                FlowPoint {
                    x: 100.0,
                    y: 100.0,
                    attached_to_uid: Some(cloud_uid),
                },
                FlowPoint {
                    x: 200.0,
                    y: 100.0,
                    attached_to_uid: Some(pop_uid),
                },
            ],
            compat: None,
            label_compat: None,
        }));
        state
            .positions
            .insert(births_uid, Position::new(flow_x, flow_y));

        let chains_data = vec![(
            vec!["population".to_string()],
            vec!["births".to_string()],
            vec!["population".to_string(), "births".to_string()],
        )];

        place_auxiliaries(&mut state, &config, &model, &metadata, &chains_data).unwrap();

        // An aux view element should be created for birth_rate
        let aux_elems: Vec<&view_element::Aux> = state
            .elements
            .iter()
            .filter_map(|e| {
                if let ViewElement::Aux(a) = e {
                    Some(a)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            aux_elems.len(),
            1,
            "expected exactly one aux element for birth_rate"
        );
        let aux = aux_elems[0];
        assert!(
            canonicalize(&aux.name).contains("birth_rate"),
            "aux element should be named birth_rate, got '{}'",
            aux.name
        );

        // The rigid group constraint means chain elements move together
        // as a unit.  Verify their relative spacing is preserved even if
        // SFDP shifts the group slightly.
        let post_stock = state
            .elements
            .iter()
            .find_map(|e| {
                if let ViewElement::Stock(s) = e {
                    (s.uid == pop_uid).then_some((s.x, s.y))
                } else {
                    None
                }
            })
            .expect("stock should exist");
        let post_flow = state
            .elements
            .iter()
            .find_map(|e| {
                if let ViewElement::Flow(f) = e {
                    (f.uid == births_uid).then_some((f.x, f.y))
                } else {
                    None
                }
            })
            .expect("flow should exist");

        let orig_dx = stock_x - flow_x;
        let orig_dy = stock_y - flow_y;
        let post_dx = post_stock.0 - post_flow.0;
        let post_dy = post_stock.1 - post_flow.1;
        assert!(
            (orig_dx - post_dx).abs() < 1.0 && (orig_dy - post_dy).abs() < 1.0,
            "chain relative spacing should be preserved: \
             orig delta ({}, {}), post delta ({}, {})",
            orig_dx,
            orig_dy,
            post_dx,
            post_dy
        );

        // The aux element should have positive coordinates
        assert!(aux.x > 0.0, "aux x should be positive, got {}", aux.x);
        assert!(aux.y > 0.0, "aux y should be positive, got {}", aux.y);

        // The aux element's UID should be unique
        let all_uids: Vec<i32> = state.elements.iter().map(|e| e.get_uid()).collect();
        let unique_uids: HashSet<i32> = all_uids.iter().copied().collect();
        assert_eq!(
            all_uids.len(),
            unique_uids.len(),
            "all UIDs should be unique: {:?}",
            all_uids
        );
        assert!(
            unique_uids.contains(&aux.uid),
            "aux UID {} should be in the element set",
            aux.uid
        );
        assert_ne!(aux.uid, pop_uid, "aux UID should differ from stock UID");
        assert_ne!(aux.uid, births_uid, "aux UID should differ from flow UID");
    }

    #[test]
    fn test_compute_metadata_with_main_alias() {
        let mut model = simple_model();
        model.name = String::new();
        let project = test_project(model);
        let metadata = compute_metadata(&project, "main", None);
        assert!(
            metadata.is_some(),
            "compute_metadata should work with 'main' alias for unnamed models"
        );
        let metadata = metadata.unwrap();
        assert!(!metadata.dep_graph.is_empty());
    }

    /// Build a LayoutState with known elements for deletion tests.
    /// Layout: stock(uid=1) --flow(uid=2)--> with aux(uid=3) feeding
    /// into the flow via a link, plus clouds on the flow endpoints,
    /// and a link from aux to flow.
    fn make_deletion_state() -> LayoutState {
        let mut state = LayoutState {
            uid_manager: UidManager::new(),
            display_names: HashMap::new(),
            elements: Vec::new(),
            positions: HashMap::new(),
            flow_templates: HashMap::new(),
            cloud_ident_to_uid: HashMap::new(),
            cloud_ident_to_flow_ident: HashMap::new(),
            flow_ident_to_clouds: HashMap::new(),
        };

        // Register named elements
        state.uid_manager.add(1, "population");
        state.uid_manager.add(2, "births");
        state.uid_manager.add(3, "birth_rate");
        // Unnamed elements (clouds and links) get sequential UIDs
        state.uid_manager.add(10, "");
        state.uid_manager.add(11, "");
        state.uid_manager.add(20, "");

        state
            .display_names
            .insert("population".into(), "population".into());
        state.display_names.insert("births".into(), "births".into());
        state
            .display_names
            .insert("birth_rate".into(), "birth_rate".into());

        // Stock
        state.elements.push(ViewElement::Stock(view_element::Stock {
            name: "population".into(),
            uid: 1,
            x: 100.0,
            y: 100.0,
            label_side: LabelSide::Bottom,
            compat: None,
        }));
        state.positions.insert(1, Position::new(100.0, 100.0));

        // Flow
        state.elements.push(ViewElement::Flow(view_element::Flow {
            name: "births".into(),
            uid: 2,
            x: 50.0,
            y: 100.0,
            label_side: LabelSide::Bottom,
            points: vec![
                FlowPoint {
                    x: 0.0,
                    y: 100.0,
                    attached_to_uid: Some(10),
                },
                FlowPoint {
                    x: 100.0,
                    y: 100.0,
                    attached_to_uid: Some(1),
                },
            ],
            compat: None,
            label_compat: None,
        }));
        state.positions.insert(2, Position::new(50.0, 100.0));

        // Aux
        state.elements.push(ViewElement::Aux(view_element::Aux {
            name: "birth_rate".into(),
            uid: 3,
            x: 50.0,
            y: 50.0,
            label_side: LabelSide::Bottom,
            compat: None,
        }));
        state.positions.insert(3, Position::new(50.0, 50.0));

        // Source cloud for the flow
        state.elements.push(ViewElement::Cloud(view_element::Cloud {
            uid: 10,
            flow_uid: 2,
            x: 0.0,
            y: 100.0,
            compat: None,
        }));
        state.positions.insert(10, Position::new(0.0, 100.0));
        state.cloud_ident_to_uid.insert("__cloud_10".into(), 10);
        state
            .cloud_ident_to_flow_ident
            .insert("__cloud_10".into(), "births".into());
        state
            .flow_ident_to_clouds
            .entry("births".into())
            .or_default()
            .push("__cloud_10".into());

        // Sink cloud for the flow
        state.elements.push(ViewElement::Cloud(view_element::Cloud {
            uid: 11,
            flow_uid: 2,
            x: 100.0,
            y: 100.0,
            compat: None,
        }));
        state.positions.insert(11, Position::new(100.0, 100.0));
        state.cloud_ident_to_uid.insert("__cloud_11".into(), 11);
        state
            .cloud_ident_to_flow_ident
            .insert("__cloud_11".into(), "births".into());
        state
            .flow_ident_to_clouds
            .entry("births".into())
            .or_default()
            .push("__cloud_11".into());

        // Link from birth_rate(3) to births(2)
        state.elements.push(ViewElement::Link(view_element::Link {
            uid: 20,
            from_uid: 3,
            to_uid: 2,
            shape: LinkShape::Straight,
            polarity: None,
        }));

        state
    }

    #[test]
    fn test_apply_deletion_removes_aux_element() {
        let mut state = make_deletion_state();

        let had_aux = state
            .elements
            .iter()
            .any(|e| matches!(e, ViewElement::Aux(a) if a.uid == 3));
        assert!(had_aux, "precondition: aux should exist before deletion");

        state.apply_deletion("birth_rate");

        let has_aux = state
            .elements
            .iter()
            .any(|e| matches!(e, ViewElement::Aux(a) if a.uid == 3));
        assert!(!has_aux, "aux element should be removed after deletion");
        assert!(
            !state.positions.contains_key(&3),
            "position should be removed"
        );
    }

    #[test]
    fn test_apply_deletion_removes_links_referencing_deleted_uid() {
        let mut state = make_deletion_state();

        let link_count_before = state
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Link(_)))
            .count();
        assert_eq!(link_count_before, 1, "precondition: one link");

        // Delete the aux that is the source of the link
        state.apply_deletion("birth_rate");

        let link_count_after = state
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Link(_)))
            .count();
        assert_eq!(
            link_count_after, 0,
            "link from deleted element should be removed"
        );
    }

    #[test]
    fn test_apply_deletion_removes_links_to_deleted_uid() {
        let mut state = make_deletion_state();

        // Delete the flow that is the target of the link
        state.apply_deletion("births");

        let link_count = state
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Link(_)))
            .count();
        assert_eq!(link_count, 0, "link to deleted element should be removed");
    }

    #[test]
    fn test_apply_deletion_removes_clouds_for_deleted_flow() {
        let mut state = make_deletion_state();

        let cloud_count_before = state
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Cloud(_)))
            .count();
        assert_eq!(cloud_count_before, 2, "precondition: two clouds");

        state.apply_deletion("births");

        let cloud_count_after = state
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Cloud(_)))
            .count();
        assert_eq!(
            cloud_count_after, 0,
            "clouds for deleted flow should be removed"
        );
        assert!(
            state.flow_ident_to_clouds.is_empty(),
            "flow_ident_to_clouds should be cleaned up"
        );
        assert!(
            state.cloud_ident_to_uid.is_empty(),
            "cloud_ident_to_uid should be cleaned up"
        );
        assert!(
            state.cloud_ident_to_flow_ident.is_empty(),
            "cloud_ident_to_flow_ident should be cleaned up"
        );
    }

    #[test]
    fn test_apply_deletion_noop_for_unknown_ident() {
        let mut state = make_deletion_state();
        let elem_count_before = state.elements.len();

        state.apply_deletion("nonexistent_var");

        assert_eq!(
            state.elements.len(),
            elem_count_before,
            "deleting unknown ident should be a no-op"
        );
    }

    #[test]
    fn test_apply_deletion_all_chain_elements() {
        let mut state = make_deletion_state();

        // Delete everything: aux, flow, stock -- order matters for
        // verifying cascading cleanup
        state.apply_deletion("birth_rate");
        state.apply_deletion("births");
        state.apply_deletion("population");

        assert!(
            state.elements.is_empty(),
            "all elements should be removed after deleting entire chain; remaining: {:?}",
            state
                .elements
                .iter()
                .map(|e| e.get_uid())
                .collect::<Vec<_>>()
        );
        assert!(
            state.positions.is_empty(),
            "all positions should be removed"
        );
        assert!(
            state.cloud_ident_to_uid.is_empty(),
            "cloud maps should be empty"
        );
        assert!(
            state.cloud_ident_to_flow_ident.is_empty(),
            "cloud flow maps should be empty"
        );
        assert!(
            state.flow_ident_to_clouds.is_empty(),
            "flow cloud maps should be empty"
        );
    }
}
