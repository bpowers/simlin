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

use crate::common::canonicalize;
use crate::datamodel;
use crate::datamodel::view_element::{self, FlowPoint, LabelSide, LinkShape};
use crate::datamodel::{Rect, ViewElement};

use self::annealing::{FlowTemplate, LineSegment};
use self::chain::{DIAGRAM_ORIGIN_MARGIN, compute_chain_positions, make_cloud_node_ident};
use self::config::LayoutConfig;
use self::connector::{
    FlowOrientation, calc_stock_flow_arc_angle, calculate_loop_arc_angle, compute_flow_orientation,
};
use self::graph::{ConstrainedGraphBuilder, Graph, GraphBuilder, Layout, Position};
use self::metadata::{ComputedMetadata, StockFlowChain};
use self::placement::{
    calculate_optimal_label_side, calculate_restricted_label_side, normalize_coordinates,
};
use self::sfdp::{SfdpConfig, compute_layout_from_initial};
use self::text::format_label_with_line_breaks;
use self::uid::UidManager;

/// Tracks the bounding box of all layout elements.
struct Bounds {
    min_x: f64,
    min_y: f64,
    max_x: f64,
    max_y: f64,
}

impl Bounds {
    fn new() -> Self {
        Self {
            min_x: f64::MAX,
            min_y: f64::MAX,
            max_x: f64::NEG_INFINITY,
            max_y: f64::NEG_INFINITY,
        }
    }

    fn update(&mut self, min_x: f64, min_y: f64, max_x: f64, max_y: f64) {
        if min_x < self.min_x {
            self.min_x = min_x;
        }
        if min_y < self.min_y {
            self.min_y = min_y;
        }
        if max_x > self.max_x {
            self.max_x = max_x;
        }
        if max_y > self.max_y {
            self.max_y = max_y;
        }
    }
}

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

/// The main layout engine holding all intermediate state.
struct LayoutEngine<'a> {
    config: LayoutConfig,
    model: &'a datamodel::Model,
    metadata: ComputedMetadata,
    uid_manager: UidManager,

    /// Canonical ident -> original display name (pre-built for O(1) lookup).
    display_names: HashMap<String, String>,

    elements: Vec<ViewElement>,
    positions: HashMap<i32, Position>,

    flow_templates: HashMap<String, FlowTemplate>,
    cloud_ident_to_uid: HashMap<String, i32>,
    cloud_ident_to_flow_ident: HashMap<String, String>,
    flow_ident_to_clouds: HashMap<String, Vec<String>>,

    bounds: Bounds,
}

impl<'a> LayoutEngine<'a> {
    fn new(config: LayoutConfig, model: &'a datamodel::Model, metadata: ComputedMetadata) -> Self {
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
            config,
            model,
            metadata,
            uid_manager,
            display_names,
            elements: Vec::new(),
            positions: HashMap::new(),
            flow_templates: HashMap::new(),
            cloud_ident_to_uid: HashMap::new(),
            cloud_ident_to_flow_ident: HashMap::new(),
            flow_ident_to_clouds: HashMap::new(),
            bounds: Bounds::new(),
        }
    }

    /// Main pipeline: produce a complete stock-flow diagram layout.
    fn generate_layout(mut self) -> Result<datamodel::StockFlow, String> {
        let chains = &self.metadata.chains;
        if chains.is_empty() && self.model.variables.is_empty() {
            return Ok(datamodel::StockFlow {
                elements: Vec::new(),
                view_box: Rect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                },
                zoom: 1.0,
                use_lettered_polarity: false,
            });
        }

        // Phase 1: Compute chain positions using SFDP
        let chain_positions = compute_chain_positions(chains, &self.metadata, &self.config);

        // Phase 2: Layout each chain at its position
        // Need to clone chain data since self is borrowed mutably below
        let chains_data: Vec<_> = chains
            .iter()
            .map(|c| (c.stocks.clone(), c.flows.clone(), c.all_vars.clone()))
            .collect();
        for (i, (stocks, flows, _all_vars)) in chains_data.iter().enumerate() {
            let position = chain_positions
                .get(&i)
                .copied()
                .unwrap_or(Position::new(self.config.start_x, self.config.start_y));
            self.layout_chain_at_position(stocks, flows, position)?;
        }

        // Phase 3: Position auxiliaries and create connectors
        self.layout_auxiliaries_and_connectors(&chains_data)?;

        // Phase 4: Apply optimal label placement
        self.apply_optimal_label_placement();

        // Phase 5: Normalize coordinates
        normalize_coordinates(&mut self.elements, DIAGRAM_ORIGIN_MARGIN);
        self.recalculate_bounds();

        // Phase 6: Apply feedback loop curvature
        self.apply_feedback_loop_curvature();

        // Phase 7: Compute ViewBox
        let view_box = if !self.elements.is_empty() && self.bounds.min_x != f64::MAX {
            Rect {
                x: 0.0,
                y: 0.0,
                width: self.bounds.max_x + DIAGRAM_ORIGIN_MARGIN,
                height: self.bounds.max_y + DIAGRAM_ORIGIN_MARGIN,
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
            elements: self.elements,
            view_box,
            zoom: 1.0,
            use_lettered_polarity: false,
        })
    }

    /// Pick a starting stock for chain layout. Returns the first stock found.
    fn pick_starting_stock<'b>(&self, stocks: &'b [String]) -> Option<&'b str> {
        stocks.first().map(|s| s.as_str())
    }

    /// Layout a single chain at the given base position using BFS.
    fn layout_chain_at_position(
        &mut self,
        stocks: &[String],
        flows: &[String],
        base_position: Position,
    ) -> Result<(), String> {
        if stocks.is_empty() && flows.is_empty() {
            return Ok(());
        }

        let start_stock = match self.pick_starting_stock(stocks) {
            Some(s) => s.to_string(),
            None => {
                // Flow-only chain (no stocks). Place flows at base_position.
                for flow_ident in flows {
                    let uid = self.get_or_alloc_uid(flow_ident);
                    self.create_flow_view_element(flow_ident, uid, base_position)?;
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
                    let inflows = self
                        .metadata
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
                    let outflows = self
                        .metadata
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

                    let (from_stock, to_stock) = self.metadata.connected_stocks(&item.id);

                    let flow_pos = match (from_stock, to_stock) {
                        (Some(from), Some(to)) => {
                            let from = from.to_string();
                            let to = to.to_string();
                            if item.connected_to == from {
                                // Position sink stock to the right
                                if !positioned.contains_key(&to) {
                                    let other_pos = Position::new(
                                        item.position.x
                                            + self.config.stock_width
                                            + self.config.horizontal_spacing,
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
                                            - self.config.stock_width
                                            - self.config.horizontal_spacing,
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
                                    + self.config.stock_width / 2.0
                                    + self.config.horizontal_spacing / 2.0,
                                item.position.y,
                            )
                        }
                        (None, Some(_to)) => {
                            // Inflow from cloud
                            Position::new(
                                item.position.x
                                    - self.config.stock_width / 2.0
                                    - self.config.horizontal_spacing / 2.0,
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
        self.create_view_elements(&positioned, stocks, flows)
    }

    /// Convert positioned stock/flow identifiers into ViewElements.
    fn create_view_elements(
        &mut self,
        positioned: &HashMap<String, Position>,
        stocks: &[String],
        flows: &[String],
    ) -> Result<(), String> {
        // Create stock view elements
        for stock_ident in stocks {
            if let Some(&pos) = positioned.get(stock_ident) {
                let uid = self.get_or_alloc_uid(stock_ident);
                let name = self.display_name(stock_ident);
                let formatted = format_label_with_line_breaks(&name);
                let elem = ViewElement::Stock(view_element::Stock {
                    name: formatted,
                    uid,
                    x: pos.x,
                    y: pos.y,
                    label_side: LabelSide::Bottom,
                });
                self.elements.push(elem);
                self.positions.insert(uid, pos);
                self.update_bounds_for_element(
                    pos.x,
                    pos.y,
                    self.config.stock_width,
                    self.config.stock_height,
                );
            }
        }

        // Create flow view elements
        for flow_ident in flows {
            if let Some(&pos) = positioned.get(flow_ident) {
                let uid = self.get_or_alloc_uid(flow_ident);
                self.create_flow_view_element(flow_ident, uid, pos)?;
            }
        }

        Ok(())
    }

    /// Create a single flow view element with its flow points and clouds.
    fn create_flow_view_element(
        &mut self,
        flow_ident: &str,
        uid: i32,
        pos: Position,
    ) -> Result<(), String> {
        let (from_stock, to_stock) = self.metadata.connected_stocks(flow_ident);
        let from_stock = from_stock.map(|s| s.to_string());
        let to_stock = to_stock.map(|s| s.to_string());
        let name = self.display_name(flow_ident);
        let formatted = format_label_with_line_breaks(&name);

        let flow_points = match (from_stock.as_deref(), to_stock.as_deref()) {
            (Some(from), Some(to)) => {
                let from_uid = self.get_or_alloc_uid(from);
                let to_uid = self.get_or_alloc_uid(to);
                let from_pos = self
                    .positions
                    .get(&from_uid)
                    .copied()
                    .unwrap_or(Position::new(pos.x - 50.0, pos.y));
                let to_pos = self
                    .positions
                    .get(&to_uid)
                    .copied()
                    .unwrap_or(Position::new(pos.x + 50.0, pos.y));
                vec![
                    FlowPoint {
                        x: from_pos.x + self.config.stock_width / 2.0,
                        y: pos.y,
                        attached_to_uid: Some(from_uid),
                    },
                    FlowPoint {
                        x: to_pos.x - self.config.stock_width / 2.0,
                        y: pos.y,
                        attached_to_uid: Some(to_uid),
                    },
                ]
            }
            (Some(from), None) => {
                let from_uid = self.get_or_alloc_uid(from);
                let from_pos = self
                    .positions
                    .get(&from_uid)
                    .copied()
                    .unwrap_or(Position::new(pos.x - 50.0, pos.y));
                vec![
                    FlowPoint {
                        x: from_pos.x + self.config.stock_width / 2.0,
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
                let to_uid = self.get_or_alloc_uid(to);
                let to_pos = self
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
                        x: to_pos.x - self.config.stock_width / 2.0,
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
        };

        // Update bounds for flow points
        for pt in &flow_elem.points {
            self.bounds.update(pt.x, pt.y, pt.x, pt.y);
        }

        // Add clouds for missing stock endpoints
        self.add_clouds_for_flow(flow_ident, &mut flow_elem);

        // Record flow template for crossing detection
        self.record_flow_template(flow_ident, &flow_elem);

        self.elements.push(ViewElement::Flow(flow_elem));
        self.positions.insert(uid, pos);
        self.update_bounds_for_element(
            pos.x,
            pos.y,
            self.config.flow_width,
            self.config.flow_height,
        );

        Ok(())
    }

    /// Add cloud elements for flow endpoints that don't connect to a stock.
    fn add_clouds_for_flow(&mut self, flow_ident: &str, flow_elem: &mut view_element::Flow) {
        let (from_stock, to_stock) = self.metadata.connected_stocks(flow_ident);
        let has_from = from_stock.is_some();
        let has_to = to_stock.is_some();

        // Source cloud (no from stock)
        if !has_from && !flow_elem.points.is_empty() {
            let cx = flow_elem.points[0].x;
            let cy = flow_elem.points[0].y;
            let cloud_uid = self.uid_manager.alloc("");
            let cloud = ViewElement::Cloud(view_element::Cloud {
                uid: cloud_uid,
                flow_uid: flow_elem.uid,
                x: cx,
                y: cy,
            });
            self.elements.push(cloud);
            flow_elem.points[0].attached_to_uid = Some(cloud_uid);
            self.bounds.update(
                cx - self.config.cloud_width / 2.0,
                cy - self.config.cloud_height / 2.0,
                cx + self.config.cloud_width / 2.0,
                cy + self.config.cloud_height / 2.0,
            );
        }

        // Sink cloud (no to stock)
        if !has_to && !flow_elem.points.is_empty() {
            let last_idx = flow_elem.points.len() - 1;
            let cx = flow_elem.points[last_idx].x;
            let cy = flow_elem.points[last_idx].y;
            let cloud_uid = self.uid_manager.alloc("");
            let cloud = ViewElement::Cloud(view_element::Cloud {
                uid: cloud_uid,
                flow_uid: flow_elem.uid,
                x: cx,
                y: cy,
            });
            self.elements.push(cloud);
            flow_elem.points[last_idx].attached_to_uid = Some(cloud_uid);
            self.bounds.update(
                cx - self.config.cloud_width / 2.0,
                cy - self.config.cloud_height / 2.0,
                cx + self.config.cloud_width / 2.0,
                cy + self.config.cloud_height / 2.0,
            );
        }
    }

    /// Cache a flow's polyline offsets (relative to valve center) for crossing detection.
    fn record_flow_template(&mut self, flow_ident: &str, flow_elem: &view_element::Flow) {
        if flow_elem.points.len() < 2 {
            return;
        }
        let offsets: Vec<Position> = flow_elem
            .points
            .iter()
            .map(|pt| Position::new(pt.x - flow_elem.x, pt.y - flow_elem.y))
            .collect();
        self.flow_templates
            .insert(flow_ident.to_string(), FlowTemplate { offsets });
    }

    /// Rebuild flow templates from current view elements.
    fn refresh_flow_templates(&mut self) {
        self.flow_templates.clear();

        let uid_to_ident: HashMap<i32, String> = self
            .model
            .variables
            .iter()
            .filter_map(|var| match var {
                datamodel::Variable::Flow(f) => {
                    f.uid.map(|uid| (uid, canonicalize(&f.ident).into_owned()))
                }
                _ => None,
            })
            .collect();

        for elem in &self.elements {
            if let ViewElement::Flow(flow_elem) = elem
                && let Some(ident) = uid_to_ident.get(&flow_elem.uid)
                && flow_elem.points.len() >= 2
            {
                let offsets: Vec<Position> = flow_elem
                    .points
                    .iter()
                    .map(|pt| Position::new(pt.x - flow_elem.x, pt.y - flow_elem.y))
                    .collect();
                self.flow_templates
                    .insert(ident.clone(), FlowTemplate { offsets });
            }
        }
    }

    /// Phase 3: Position auxiliaries using SFDP with rigid chain groups, then create connectors.
    fn layout_auxiliaries_and_connectors(
        &mut self,
        chains_data: &[(Vec<String>, Vec<String>, Vec<String>)],
    ) -> Result<(), String> {
        self.refresh_flow_templates();

        let (full_graph, var_to_node) = self.build_full_graph()?;

        if full_graph.node_count() == 0 {
            return Ok(());
        }

        // Run SFDP with rigid chains (takes ownership of full_graph)
        let layout = self.run_sfdp_with_rigid_chains(full_graph, chains_data, &var_to_node)?;

        // Apply SFDP positions to all elements
        self.apply_layout_positions(&layout, &var_to_node)?;

        // Create auxiliary view elements for any not yet created
        self.create_missing_auxiliary_elements(&layout, &var_to_node);

        // Create connector (link) view elements
        self.create_connectors()?;

        self.recalculate_bounds();
        Ok(())
    }

    /// Build an undirected graph with all model variables and cloud nodes for SFDP.
    fn build_full_graph(&mut self) -> Result<(Graph<String>, HashMap<String, String>), String> {
        // Reset cloud mappings
        self.cloud_ident_to_uid.clear();
        self.cloud_ident_to_flow_ident.clear();
        self.flow_ident_to_clouds.clear();

        let flow_uid_to_ident: HashMap<i32, String> = self
            .model
            .variables
            .iter()
            .filter_map(|var| match var {
                datamodel::Variable::Flow(f) => {
                    f.uid.map(|uid| (uid, canonicalize(&f.ident).into_owned()))
                }
                _ => None,
            })
            .collect();

        let mut var_to_node: HashMap<String, String> = HashMap::new();
        let mut node_to_var: HashMap<String, String> = HashMap::new();
        let mut builder = GraphBuilder::<String>::new_undirected();
        let mut node_index = 0;

        // Add all variables from the dependency graph as nodes
        let all_vars: BTreeSet<String> = self
            .metadata
            .dep_graph
            .keys()
            .chain(
                self.metadata
                    .dep_graph
                    .values()
                    .flat_map(|deps| deps.iter()),
            )
            .cloned()
            .collect();

        for var_ident in &all_vars {
            let node_id = format!("node_{}", node_index);
            var_to_node.insert(var_ident.clone(), node_id.clone());
            node_to_var.insert(node_id.clone(), var_ident.clone());
            builder.add_node(node_id);
            node_index += 1;
        }

        // Add edges from dependency graph
        for (from_ident, deps) in &self.metadata.dep_graph {
            if let Some(from_node) = var_to_node.get(from_ident) {
                for to_ident in deps {
                    if let Some(to_node) = var_to_node.get(to_ident) {
                        builder.add_edge(from_node.clone(), to_node.clone(), 1.0);
                    }
                }
            }
        }

        // Add cloud nodes
        for elem in &self.elements {
            if let ViewElement::Cloud(cloud) = elem {
                let flow_ident = match flow_uid_to_ident.get(&cloud.flow_uid) {
                    Some(ident) => ident.clone(),
                    None => continue,
                };

                let cloud_ident = make_cloud_node_ident(cloud.uid);
                if !var_to_node.contains_key(&cloud_ident) {
                    let node_id = format!("node_{}", node_index);
                    builder.add_node(node_id.clone());
                    var_to_node.insert(cloud_ident.clone(), node_id.clone());
                    node_to_var.insert(node_id, cloud_ident.clone());
                    node_index += 1;
                }

                if let Some(flow_node) = var_to_node.get(&flow_ident) {
                    let cloud_node = var_to_node[&cloud_ident].clone();
                    builder.add_edge(flow_node.clone(), cloud_node, 1.0);
                }

                self.cloud_ident_to_uid
                    .insert(cloud_ident.clone(), cloud.uid);
                self.cloud_ident_to_flow_ident
                    .insert(cloud_ident.clone(), flow_ident.clone());
                self.flow_ident_to_clouds
                    .entry(flow_ident)
                    .or_default()
                    .push(cloud_ident);
            }
        }

        Ok((builder.build(), var_to_node))
    }

    /// Run SFDP with chain elements locked into rigid groups.
    fn run_sfdp_with_rigid_chains(
        &self,
        full_graph: Graph<String>,
        chains_data: &[(Vec<String>, Vec<String>, Vec<String>)],
        var_to_node: &HashMap<String, String>,
    ) -> Result<Layout<String>, String> {
        let mut constrained_builder = ConstrainedGraphBuilder::new(full_graph);

        // Create one rigid group per chain
        for (_stocks, _flows, all_vars) in chains_data {
            let mut group_members: Vec<String> = Vec::new();
            let mut added: HashSet<String> = HashSet::new();

            for var_ident in all_vars {
                if let Some(node_id) = var_to_node.get(var_ident) {
                    // Only include positioned elements in the rigid group
                    let uid = self.uid_manager.get_uid(var_ident);
                    let is_positioned = uid.is_some_and(|u| self.positions.contains_key(&u));
                    if is_positioned && added.insert(node_id.clone()) {
                        group_members.push(node_id.clone());

                        // Also add clouds attached to flows in this chain
                        let canonical = canonicalize(var_ident);
                        if let Some(cloud_idents) =
                            self.flow_ident_to_clouds.get(canonical.as_ref())
                        {
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

        // Build initial layout from existing positions
        let mut initial_layout: Layout<String> = BTreeMap::new();
        let cloud_uid_to_pos: HashMap<i32, Position> = self
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

        // Pre-compute center from known positions for auxiliary placement
        let mut center_x = self.config.start_x;
        let mut center_y = self.config.start_y;
        let mut count = 0;

        for (var_ident, node_id) in var_to_node {
            // Try existing positioned elements first
            if let Some(uid) = self.uid_manager.get_uid(var_ident)
                && let Some(&pos) = self.positions.get(&uid)
            {
                initial_layout.insert(node_id.clone(), pos);
                center_x += pos.x;
                center_y += pos.y;
                count += 1;
                continue;
            }

            // Try cloud positions
            if let Some(&cloud_uid) = self.cloud_ident_to_uid.get(var_ident) {
                if let Some(&pos) = cloud_uid_to_pos.get(&cloud_uid) {
                    initial_layout.insert(node_id.clone(), pos);
                    continue;
                }
                // Fall back to flow position for clouds
                if let Some(flow_ident) = self.cloud_ident_to_flow_ident.get(var_ident)
                    && let Some(flow_uid) = self.uid_manager.get_uid(flow_ident)
                    && let Some(&pos) = self.positions.get(&flow_uid)
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
            // Unpositioned node; place in circle around center
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

        // Tighter spacing (k=75) and stronger attraction (c=3.0) than chain
        // positioning because auxiliaries are individual nodes that should
        // cluster near their dependencies.  Higher iteration count and
        // slower cooling give the optimizer time to untangle dense graphs.
        let sfdp_config = SfdpConfig {
            k: 75.0,
            max_iterations: 5000,
            convergence_threshold: 0.01,
            initial_step_size: 75.0,
            cooling_factor: 0.9995,
            p: 2.0,
            c: 3.0,
            beautify_leaves: true,
        };

        let final_layout = compute_layout_from_initial(
            &constrained_graph,
            &sfdp_config,
            &initial_layout,
            self.config.annealing_random_seed,
        );

        Ok(final_layout)
    }

    /// Update all element coordinates from SFDP results.
    fn apply_layout_positions(
        &mut self,
        layout: &Layout<String>,
        var_to_node: &HashMap<String, String>,
    ) -> Result<(), String> {
        // Build ident -> position map
        let layout_by_ident: HashMap<String, Position> = var_to_node
            .iter()
            .filter_map(|(ident, node_id)| layout.get(node_id).map(|&pos| (ident.clone(), pos)))
            .collect();

        // Build uid -> ident map
        let uid_to_ident: HashMap<i32, String> = self
            .model
            .variables
            .iter()
            .filter_map(|var| {
                let uid = match var {
                    datamodel::Variable::Stock(s) => s.uid,
                    datamodel::Variable::Flow(f) => f.uid,
                    datamodel::Variable::Aux(a) => a.uid,
                    datamodel::Variable::Module(m) => m.uid,
                }?;
                Some((uid, canonicalize(var.get_ident()).into_owned()))
            })
            .collect();

        let mut flow_deltas: HashMap<i32, Position> = HashMap::new();

        for elem in &mut self.elements {
            match elem {
                ViewElement::Stock(stock) => {
                    if let Some(ident) = uid_to_ident.get(&stock.uid)
                        && let Some(&pos) = layout_by_ident.get(ident)
                    {
                        stock.x = pos.x;
                        stock.y = pos.y;
                        self.positions.insert(stock.uid, pos);
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
                        self.positions.insert(flow.uid, pos);
                        flow_deltas.insert(flow.uid, Position::new(dx, dy));
                    }
                }
                ViewElement::Aux(aux) => {
                    if let Some(ident) = uid_to_ident.get(&aux.uid)
                        && let Some(&pos) = layout_by_ident.get(ident)
                    {
                        aux.x = pos.x;
                        aux.y = pos.y;
                        self.positions.insert(aux.uid, pos);
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

        self.recalculate_bounds();
        Ok(())
    }

    /// Create auxiliary view elements for variables not yet in the elements list.
    fn create_missing_auxiliary_elements(
        &mut self,
        layout: &Layout<String>,
        var_to_node: &HashMap<String, String>,
    ) {
        let existing_uids: HashSet<i32> = self.elements.iter().map(|e| e.get_uid()).collect();

        for var in &self.model.variables {
            if let datamodel::Variable::Aux(aux) = var {
                let canonical = canonicalize(&aux.ident);
                let uid = self.uid_manager.alloc(&canonical);
                if existing_uids.contains(&uid) {
                    continue;
                }

                // Find position from layout
                let pos = var_to_node
                    .get(canonical.as_ref())
                    .and_then(|node_id| layout.get(node_id))
                    .copied()
                    .unwrap_or(Position::new(self.config.start_x, self.config.start_y));

                let name = self.display_name(&canonical);
                let formatted = format_label_with_line_breaks(&name);
                let elem = ViewElement::Aux(view_element::Aux {
                    name: formatted,
                    uid,
                    x: pos.x,
                    y: pos.y,
                    label_side: LabelSide::Bottom,
                });
                self.elements.push(elem);
                self.positions.insert(uid, pos);
            }
        }
    }

    /// Create link view elements for all non-structural dependency edges.
    fn create_connectors(&mut self) -> Result<(), String> {
        let mut link_set: HashSet<String> = HashSet::new();

        // Build lookup sets for structural connections
        let stock_inflows: HashMap<String, HashSet<String>> = self
            .metadata
            .stock_to_inflows
            .iter()
            .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
            .collect();
        let stock_outflows: HashMap<String, HashSet<String>> = self
            .metadata
            .stock_to_outflows
            .iter()
            .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
            .collect();

        let dep_entries: Vec<(String, Vec<String>)> = self
            .metadata
            .dep_graph
            .iter()
            .map(|(k, v)| (k.clone(), v.iter().cloned().collect()))
            .collect();

        for (from_ident, to_idents) in &dep_entries {
            for to_ident in to_idents {
                // Skip structural flow-to-stock connections
                if is_structural_flow_stock(from_ident, to_ident, &stock_inflows, &stock_outflows) {
                    continue;
                }

                let link_key = format!("{}->{}", from_ident, to_ident);
                if !link_set.insert(link_key) {
                    continue;
                }

                let from_uid = match self.uid_manager.get_uid(from_ident) {
                    Some(uid) => uid,
                    None => continue,
                };
                let to_uid = match self.uid_manager.get_uid(to_ident) {
                    Some(uid) => uid,
                    None => continue,
                };

                if from_uid == 0 || to_uid == 0 {
                    continue;
                }

                let link_uid = self.uid_manager.alloc("");
                let mut shape = LinkShape::Straight;

                // Check for structural stock->flow connections that need an arc
                if is_structural_stock_flow(from_ident, to_ident, &stock_inflows, &stock_outflows) {
                    let arc_angle = if let (Some(&s_pos), Some(&f_pos)) =
                        (self.positions.get(&from_uid), self.positions.get(&to_uid))
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
                self.elements.push(link);
            }
        }

        Ok(())
    }

    /// Apply optimal label placement based on connector angles.
    fn apply_optimal_label_placement(&mut self) {
        // Build position map keyed by ident
        let uid_to_ident: HashMap<i32, String> = self
            .model
            .variables
            .iter()
            .filter_map(|var| {
                let uid = match var {
                    datamodel::Variable::Stock(s) => s.uid,
                    datamodel::Variable::Flow(f) => f.uid,
                    datamodel::Variable::Aux(a) => a.uid,
                    datamodel::Variable::Module(m) => m.uid,
                }?;
                Some((uid, canonicalize(var.get_ident()).into_owned()))
            })
            .collect();

        let ident_positions: HashMap<String, Position> = self
            .positions
            .iter()
            .filter_map(|(uid, pos)| uid_to_ident.get(uid).map(|ident| (ident.clone(), *pos)))
            .collect();

        // Build uses/used_by maps for placement functions
        let uses = &self.metadata.dep_graph;
        let used_by = &self.metadata.reverse_dep_graph;

        // Collect element update info (avoid borrow conflict)
        let updates: Vec<(usize, LabelSide)> = self
            .elements
            .iter()
            .enumerate()
            .filter_map(|(i, elem)| match elem {
                ViewElement::Stock(stock) => {
                    let ident = uid_to_ident.get(&stock.uid)?;
                    let allowed = self.calculate_allowed_label_sides_for_stock(ident);
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
                _ => None,
            })
            .collect();

        for (i, side) in updates {
            match &mut self.elements[i] {
                ViewElement::Stock(s) => s.label_side = side,
                ViewElement::Flow(f) => f.label_side = side,
                ViewElement::Aux(a) => a.label_side = side,
                _ => {}
            }
        }
    }

    /// Determine which sides are available for label placement on a stock,
    /// excluding sides where flows are attached.
    fn calculate_allowed_label_sides_for_stock(&self, stock_ident: &str) -> Vec<LabelSide> {
        let stock_uid = match self.uid_manager.get_uid(stock_ident) {
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
        let stock_pos = match self.positions.get(&stock_uid) {
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

        let all_flows: Vec<String> = self
            .metadata
            .stock_to_inflows
            .get(stock_ident)
            .into_iter()
            .chain(self.metadata.stock_to_outflows.get(stock_ident))
            .flat_map(|v| v.iter())
            .cloned()
            .collect();

        for flow_ident in &all_flows {
            if let Some(flow_uid) = self.uid_manager.get_uid(flow_ident)
                && let Some(&flow_pos) = self.positions.get(&flow_uid)
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

    /// Apply arc curvature to connectors involved in feedback loops.
    fn apply_feedback_loop_curvature(&mut self) {
        if self.metadata.feedback_loops.is_empty() {
            return;
        }

        // Build link map: (from_ident, to_ident) -> element index
        let uid_to_ident: HashMap<i32, String> = self
            .model
            .variables
            .iter()
            .filter_map(|var| {
                let uid = match var {
                    datamodel::Variable::Stock(s) => s.uid,
                    datamodel::Variable::Flow(f) => f.uid,
                    datamodel::Variable::Aux(a) => a.uid,
                    datamodel::Variable::Module(m) => m.uid,
                }?;
                Some((uid, canonicalize(var.get_ident()).into_owned()))
            })
            .collect();

        let mut link_map: HashMap<(String, String), usize> = HashMap::new();
        for (i, elem) in self.elements.iter().enumerate() {
            if let ViewElement::Link(link) = elem {
                let from_ident = uid_to_ident.get(&link.from_uid).cloned();
                let to_ident = uid_to_ident.get(&link.to_uid).cloned();
                if let (Some(from), Some(to)) = (from_ident, to_ident) {
                    link_map.insert((from, to), i);
                }
            }
        }

        // Process loops in reverse order (least to most important)
        let loops = &self.metadata.feedback_loops;
        for i in (0..loops.len()).rev() {
            let loop_info = &loops[i];
            let chain = loop_info.causal_chain();
            if chain.len() < 2 {
                continue;
            }

            // Compute loop center
            let mut sum_x = 0.0;
            let mut sum_y = 0.0;
            let mut count = 0;
            for var in chain {
                if let Some(uid) = self.uid_manager.get_uid(var)
                    && let Some(&pos) = self.positions.get(&uid)
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

            // Apply curvature to edges in the loop
            for j in 0..chain.len() - 1 {
                let from = &chain[j];
                let to = &chain[j + 1];

                let Some(&elem_idx) = link_map.get(&(from.clone(), to.clone())) else {
                    continue;
                };

                if let ViewElement::Link(link) = &self.elements[elem_idx] {
                    // Don't override existing arcs (e.g. structural stock-flow connections)
                    if matches!(link.shape, LinkShape::Arc(_)) {
                        continue;
                    }
                }

                let from_uid = self.uid_manager.get_uid(from);
                let to_uid = self.uid_manager.get_uid(to);
                if let (Some(f_uid), Some(t_uid)) = (from_uid, to_uid)
                    && let (Some(&from_pos), Some(&to_pos)) =
                        (self.positions.get(&f_uid), self.positions.get(&t_uid))
                {
                    let arc_angle = calculate_loop_arc_angle(
                        from_pos,
                        to_pos,
                        loop_center,
                        self.config.loop_curvature_factor,
                    );
                    if let ViewElement::Link(link) = &mut self.elements[elem_idx] {
                        link.shape = LinkShape::Arc(arc_angle);
                    }
                }
            }
        }
    }

    /// Recalculate bounds from all current element positions.
    fn recalculate_bounds(&mut self) {
        let mut bounds = Bounds::new();

        let update = |bounds: &mut Bounds, cx: f64, cy: f64, w: f64, h: f64| {
            let hw = w / 2.0;
            let hh = h / 2.0;
            bounds.update(cx - hw, cy - hh, cx + hw, cy + hh);
        };

        for elem in &self.elements {
            match elem {
                ViewElement::Stock(s) => {
                    update(
                        &mut bounds,
                        s.x,
                        s.y,
                        self.config.stock_width,
                        self.config.stock_height,
                    );
                }
                ViewElement::Flow(f) => {
                    update(
                        &mut bounds,
                        f.x,
                        f.y,
                        self.config.flow_width,
                        self.config.flow_height,
                    );
                    for pt in &f.points {
                        bounds.update(pt.x, pt.y, pt.x, pt.y);
                    }
                }
                ViewElement::Aux(a) => {
                    update(
                        &mut bounds,
                        a.x,
                        a.y,
                        self.config.aux_width,
                        self.config.aux_height,
                    );
                }
                ViewElement::Cloud(c) => {
                    update(
                        &mut bounds,
                        c.x,
                        c.y,
                        self.config.cloud_width,
                        self.config.cloud_height,
                    );
                }
                _ => {}
            }
        }

        self.bounds = bounds;
    }

    fn update_bounds_for_element(&mut self, cx: f64, cy: f64, width: f64, height: f64) {
        let hw = width / 2.0;
        let hh = height / 2.0;
        self.bounds.update(cx - hw, cy - hh, cx + hw, cy + hh);
    }

    /// Get or allocate a UID for a variable by its canonical ident.
    fn get_or_alloc_uid(&mut self, ident: &str) -> i32 {
        self.uid_manager.alloc(ident)
    }

    /// Get the display name for a variable, preferring the original case.
    fn display_name(&self, canonical_ident: &str) -> String {
        self.display_names
            .get(canonical_ident)
            .cloned()
            .unwrap_or_else(|| canonical_ident.to_string())
    }
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

/// Compute metadata for a model from its variable definitions and dependency structure.
pub fn compute_metadata(model: &datamodel::Model) -> ComputedMetadata {
    let mut dep_graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut reverse_dep_graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut constants: BTreeSet<String> = BTreeSet::new();
    let mut stock_to_inflows: HashMap<String, Vec<String>> = HashMap::new();
    let mut stock_to_outflows: HashMap<String, Vec<String>> = HashMap::new();
    let mut flow_to_stocks: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();

    // Collect stock inflows/outflows
    for var in &model.variables {
        if let datamodel::Variable::Stock(stock) = var {
            let stock_ident = canonicalize(&stock.ident).into_owned();
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
            }
            for f in &outflows {
                let entry = flow_to_stocks.entry(f.clone()).or_insert((None, None));
                entry.0 = Some(stock_ident.clone());
            }

            stock_to_inflows.insert(stock_ident.clone(), inflows);
            stock_to_outflows.insert(stock_ident, outflows);
        }
    }

    // Build dependency graph from equation references
    // For now, use a simple heuristic: parse equation text for variable references
    let all_idents: HashSet<String> = model
        .variables
        .iter()
        .map(|v| canonicalize(v.get_ident()).into_owned())
        .collect();

    for var in &model.variables {
        let var_ident = canonicalize(var.get_ident()).into_owned();
        dep_graph.entry(var_ident.clone()).or_default();

        // Extract dependencies from the equation
        let deps = extract_equation_deps(var, &all_idents);

        if deps.is_empty() {
            // Check if this is truly a constant (no stock inflows/outflows either)
            if !matches!(var, datamodel::Variable::Stock(_)) {
                constants.insert(var_ident.clone());
            }
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

        // Add structural stock-flow dependencies
        if let datamodel::Variable::Stock(stock) = var {
            for inflow in &stock.inflows {
                let flow_ident = canonicalize(inflow).into_owned();
                dep_graph
                    .entry(flow_ident.clone())
                    .or_default()
                    .insert(var_ident.clone());
                reverse_dep_graph
                    .entry(var_ident.clone())
                    .or_default()
                    .insert(flow_ident);
            }
            for outflow in &stock.outflows {
                let flow_ident = canonicalize(outflow).into_owned();
                dep_graph
                    .entry(flow_ident.clone())
                    .or_default()
                    .insert(var_ident.clone());
                reverse_dep_graph
                    .entry(var_ident.clone())
                    .or_default()
                    .insert(flow_ident);
            }
        }
    }

    // Detect stock-flow chains
    let chains = detect_chains(&stock_to_inflows, &stock_to_outflows, &flow_to_stocks);

    ComputedMetadata {
        chains,
        feedback_loops: Vec::new(), // LTM integration is a follow-up
        dep_graph,
        reverse_dep_graph,
        constants,
        stock_to_inflows,
        stock_to_outflows,
        flow_to_stocks,
    }
}

/// Extract variable dependencies from an equation using word-boundary
/// matching on the equation text. This is a heuristic that avoids requiring
/// full model compilation; it can produce false positives for identifiers
/// that collide with builtin function names and doesn't handle subscripted
/// references. A future improvement would wire in the engine's AST-based
/// dependency resolution (which requires a compiled model).
fn extract_equation_deps(var: &datamodel::Variable, all_idents: &HashSet<String>) -> Vec<String> {
    let equation = match var.get_equation() {
        Some(eq) => eq,
        None => return Vec::new(),
    };

    let equation_text = match equation {
        datamodel::Equation::Scalar(text, _) => text.as_str(),
        datamodel::Equation::ApplyToAll(_, text, _) => text.as_str(),
        datamodel::Equation::Arrayed(_, entries) => {
            // Use first equation as representative
            if let Some((_, text, _, _)) = entries.first() {
                text.as_str()
            } else {
                return Vec::new();
            }
        }
    };

    // Tokenize equation and find references to known variables
    let mut deps = Vec::new();
    let lower = equation_text.to_lowercase();

    for ident in all_idents {
        let var_ident = canonicalize(var.get_ident());
        if ident == var_ident.as_ref() {
            continue; // Skip self-references
        }
        // Check if the ident appears as a word in the equation
        if contains_ident(&lower, ident) {
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

/// Detect stock-flow chains using union-find over stock/flow connections.
fn detect_chains(
    stock_to_inflows: &HashMap<String, Vec<String>>,
    stock_to_outflows: &HashMap<String, Vec<String>>,
    flow_to_stocks: &HashMap<String, (Option<String>, Option<String>)>,
) -> Vec<StockFlowChain> {
    // Collect all stocks
    let all_stocks: BTreeSet<String> = stock_to_inflows
        .keys()
        .chain(stock_to_outflows.keys())
        .cloned()
        .collect();

    if all_stocks.is_empty() {
        // Create chains for flows with no stocks
        let mut chains = Vec::new();
        for flow_ident in flow_to_stocks.keys() {
            chains.push(StockFlowChain {
                stocks: Vec::new(),
                flows: vec![flow_ident.clone()],
                all_vars: vec![flow_ident.clone()],
                importance: 0.0,
            });
        }
        return chains;
    }

    // Build connected components via BFS
    let mut visited: HashSet<String> = HashSet::new();
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
            stocks: chain_stocks,
            flows: chain_flows,
            all_vars,
            importance: 0.0,
        });
    }

    chains
}

/// Count edge crossings in a completed StockFlow view.
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
                        from_node: format!("link_from_{}", link.from_uid),
                        to_node: format!("link_to_{}", link.to_uid),
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

/// Generate a complete stock-flow diagram layout for a model.
///
/// Computes metadata (dependency graph, chains) from the model variables,
/// then runs the multi-phase layout pipeline: chain positioning via SFDP,
/// auxiliary placement, connector routing, label optimization, and
/// coordinate normalization.
pub fn generate_layout(model: &datamodel::Model) -> Result<datamodel::StockFlow, String> {
    let config = LayoutConfig::default();
    let metadata = compute_metadata(model);
    let engine = LayoutEngine::new(config, model, metadata);
    engine.generate_layout()
}

/// Generate layout with a specific configuration.
pub fn generate_layout_with_config(
    model: &datamodel::Model,
    mut config: LayoutConfig,
) -> Result<datamodel::StockFlow, String> {
    config.validate();
    let metadata = compute_metadata(model);
    let engine = LayoutEngine::new(config, model, metadata);
    engine.generate_layout()
}

/// Generate multiple layouts with different seeds in parallel and pick the
/// one with fewest crossings. On tie, the lowest seed wins.
pub fn generate_best_layout(model: &datamodel::Model) -> Result<datamodel::StockFlow, String> {
    use rayon::prelude::*;

    let config = LayoutConfig::default();
    let seeds = LAYOUT_SEEDS;

    let results: Vec<Result<LayoutResult, String>> = seeds
        .par_iter()
        .map(|&seed| {
            let mut cfg = config.clone();
            cfg.annealing_random_seed = seed;
            let metadata = compute_metadata(model);
            let engine = LayoutEngine::new(cfg, model, metadata);
            let view = engine.generate_layout()?;
            let crossings = count_view_crossings(&view);
            Ok(LayoutResult {
                view,
                crossings,
                seed,
            })
        })
        .collect();

    select_best_layout(results)
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

    fn simple_model() -> datamodel::Model {
        datamodel::Model {
            name: "test".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "population".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string(), None),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["births".to_string()],
                    outflows: vec!["deaths".to_string()],
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Public,
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "births".to_string(),
                    equation: datamodel::Equation::Scalar(
                        "population * birth_rate".to_string(),
                        None,
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Public,
                    ai_state: None,
                    uid: Some(2),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "deaths".to_string(),
                    equation: datamodel::Equation::Scalar(
                        "population * death_rate".to_string(),
                        None,
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Public,
                    ai_state: None,
                    uid: Some(3),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "birth_rate".to_string(),
                    equation: datamodel::Equation::Scalar("0.03".to_string(), None),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Public,
                    ai_state: None,
                    uid: Some(4),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "death_rate".to_string(),
                    equation: datamodel::Equation::Scalar("0.01".to_string(), None),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Public,
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
        let model = datamodel::Model {
            name: "empty".to_string(),
            sim_specs: None,
            variables: Vec::new(),
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };
        let result = generate_layout(&model).unwrap();
        assert!(result.elements.is_empty());
        assert_eq!(result.zoom, 1.0);
    }

    #[test]
    fn test_generate_layout_single_chain() {
        let model = simple_model();
        let result = generate_layout(&model).unwrap();

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
        let model = simple_model();
        let result = generate_layout(&model).unwrap();

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
        let model = simple_model();
        let result = generate_layout(&model).unwrap();

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
        let model = simple_model();
        let result = generate_layout(&model).unwrap();

        let mut uids: HashSet<i32> = HashSet::new();
        for elem in &result.elements {
            let uid = elem.get_uid();
            assert!(uids.insert(uid), "duplicate UID: {}", uid);
        }
    }

    #[test]
    fn test_viewbox_encompasses_elements() {
        let model = simple_model();
        let result = generate_layout(&model).unwrap();

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
        let model = simple_model();
        let result = generate_layout(&model).unwrap();
        assert_eq!(result.zoom, 1.0);
    }

    #[test]
    fn test_flow_points_attached() {
        let model = simple_model();
        let result = generate_layout(&model).unwrap();

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
        let model = simple_model();
        let metadata = compute_metadata(&model);

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
        let model = simple_model();
        let metadata = compute_metadata(&model);

        // births depends on population and birth_rate
        let births_deps = metadata.dep_graph.get("births").unwrap();
        assert!(births_deps.contains("population"));
        assert!(births_deps.contains("birth_rate"));
    }

    #[test]
    fn test_compute_metadata_constants() {
        let model = simple_model();
        let metadata = compute_metadata(&model);

        // birth_rate and death_rate are constants (scalar equations with no variable references)
        assert!(metadata.is_constant("birth_rate"));
        assert!(metadata.is_constant("death_rate"));
    }

    #[test]
    fn test_detect_chains_multiple() {
        let mut stock_to_inflows: HashMap<String, Vec<String>> = HashMap::new();
        let mut stock_to_outflows: HashMap<String, Vec<String>> = HashMap::new();
        let mut flow_to_stocks: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();

        // Chain 1: A -> f1 -> B
        stock_to_inflows.insert("b".into(), vec!["f1".into()]);
        stock_to_outflows.insert("a".into(), vec!["f1".into()]);
        flow_to_stocks.insert("f1".into(), (Some("a".into()), Some("b".into())));

        // Chain 2: C (isolated stock)
        stock_to_inflows.insert("c".into(), vec![]);
        stock_to_outflows.insert("c".into(), vec![]);

        let chains = detect_chains(&stock_to_inflows, &stock_to_outflows, &flow_to_stocks);
        assert_eq!(chains.len(), 2);
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
            equation: datamodel::Equation::Scalar(equation.to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Public,
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
            equation: datamodel::Equation::Scalar(String::new(), None),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            non_negative: false,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Public,
            ai_state: None,
            uid: None,
        });
        let idents: HashSet<String> = ["stock", "x"].iter().map(|s| s.to_string()).collect();
        let deps = extract_equation_deps(&var, &idents);
        assert!(deps.is_empty());
    }

    #[test]
    fn test_select_best_layout_fewest_crossings() {
        let results = vec![
            Ok(LayoutResult {
                view: datamodel::StockFlow {
                    elements: Vec::new(),
                    view_box: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 100.0,
                    },
                    zoom: 1.0,
                    use_lettered_polarity: false,
                },
                crossings: 5,
                seed: 42,
            }),
            Ok(LayoutResult {
                view: datamodel::StockFlow {
                    elements: Vec::new(),
                    view_box: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 100.0,
                    },
                    zoom: 1.0,
                    use_lettered_polarity: false,
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
                    elements: vec![ViewElement::Aux(view_element::Aux {
                        name: "from_seed_123".to_string(),
                        uid: 1,
                        x: 0.0,
                        y: 0.0,
                        label_side: LabelSide::Bottom,
                    })],
                    view_box: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 100.0,
                    },
                    zoom: 1.0,
                    use_lettered_polarity: false,
                },
                crossings: 3,
                seed: 123,
            }),
            Ok(LayoutResult {
                view: datamodel::StockFlow {
                    elements: vec![ViewElement::Aux(view_element::Aux {
                        name: "from_seed_42".to_string(),
                        uid: 2,
                        x: 0.0,
                        y: 0.0,
                        label_side: LabelSide::Bottom,
                    })],
                    view_box: Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 100.0,
                    },
                    zoom: 1.0,
                    use_lettered_polarity: false,
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
            name: "aux_only".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "rate".to_string(),
                    equation: datamodel::Equation::Scalar("0.5".to_string(), None),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Public,
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "factor".to_string(),
                    equation: datamodel::Equation::Scalar("rate * 2".to_string(), None),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Public,
                    ai_state: None,
                    uid: Some(2),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };
        let result = generate_layout(&model).unwrap();
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
            name: "single".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "x".to_string(),
                equation: datamodel::Equation::Scalar("42".to_string(), None),
                documentation: String::new(),
                units: None,
                gf: None,
                can_be_module_input: false,
                visibility: datamodel::Visibility::Public,
                ai_state: None,
                uid: Some(1),
            })],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };
        let result = generate_layout(&model).unwrap();
        assert_eq!(result.elements.len(), 1);
    }

    #[test]
    fn test_generate_layout_disconnected_stocks() {
        let model = datamodel::Model {
            name: "disconnected".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "stock_a".to_string(),
                    equation: datamodel::Equation::Scalar("100".to_string(), None),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec![],
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Public,
                    ai_state: None,
                    uid: Some(1),
                }),
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "stock_b".to_string(),
                    equation: datamodel::Equation::Scalar("200".to_string(), None),
                    documentation: String::new(),
                    units: None,
                    inflows: vec![],
                    outflows: vec![],
                    non_negative: false,
                    can_be_module_input: false,
                    visibility: datamodel::Visibility::Public,
                    ai_state: None,
                    uid: Some(2),
                }),
            ],
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
        };
        let result = generate_layout(&model).unwrap();
        let stocks: Vec<_> = result
            .elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Stock(_)))
            .collect();
        assert_eq!(stocks.len(), 2);
    }
}
