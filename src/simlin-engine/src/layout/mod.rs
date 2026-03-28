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

    /// Update a variable's identity in-place while preserving its
    /// position and UID.  Updates the element name, uid_manager
    /// mapping, and display_names entry.
    pub fn apply_rename(&mut self, old_ident: &str, new_ident: &str, new_display_name: &str) {
        let old_canonical = canonicalize(old_ident).into_owned();
        let uid = match self.uid_manager.get_uid(&old_canonical) {
            Some(uid) => uid,
            None => return,
        };

        for elem in &mut self.elements {
            if elem.get_uid() != uid {
                continue;
            }
            match elem {
                ViewElement::Aux(a) => a.name = new_display_name.to_string(),
                ViewElement::Stock(s) => s.name = new_display_name.to_string(),
                ViewElement::Flow(f) => f.name = new_display_name.to_string(),
                ViewElement::Module(m) => m.name = new_display_name.to_string(),
                _ => {}
            }
            break;
        }

        let new_canonical = canonicalize(new_ident).into_owned();
        self.uid_manager.rename(&old_canonical, &new_canonical);
        self.display_names.remove(&old_canonical);
        self.display_names
            .insert(new_canonical, new_display_name.to_string());
    }

    /// Walk model variables and identify which ones are not yet represented
    /// in this layout state (either no UID mapping or no view element with
    /// that UID), classifying each by variable type.
    pub fn identify_new_elements(&self, model: &datamodel::Model) -> NewElements {
        let existing_uids: HashSet<i32> = self.elements.iter().map(|e| e.get_uid()).collect();

        let mut new_stocks = Vec::new();
        let mut new_flows = Vec::new();
        let mut new_auxes = Vec::new();
        let mut new_modules = Vec::new();

        for var in &model.variables {
            let canonical = canonicalize(var.get_ident()).into_owned();
            let is_new = match self.uid_manager.get_uid(&canonical) {
                None => true,
                Some(uid) => !existing_uids.contains(&uid),
            };

            if is_new {
                match var {
                    datamodel::Variable::Stock(_) => new_stocks.push(canonical),
                    datamodel::Variable::Flow(_) => new_flows.push(canonical),
                    datamodel::Variable::Aux(_) => new_auxes.push(canonical),
                    datamodel::Variable::Module(_) => new_modules.push(canonical),
                }
            }
        }

        NewElements {
            new_stocks,
            new_flows,
            new_auxes,
            new_modules,
        }
    }
}

/// Variables in the model that have no corresponding view element in
/// the current layout state, classified by type.
pub struct NewElements {
    pub new_stocks: Vec<String>,
    pub new_flows: Vec<String>,
    pub new_auxes: Vec<String>,
    pub new_modules: Vec<String>,
}

impl NewElements {
    pub fn is_empty(&self) -> bool {
        self.new_stocks.is_empty()
            && self.new_flows.is_empty()
            && self.new_auxes.is_empty()
            && self.new_modules.is_empty()
    }
}

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

/// Bounding box of all existing positioned elements.
/// Returns ((min_x, min_y), (max_x, max_y)).
/// When no positions exist, returns a default origin area.
fn existing_bounding_box(state: &LayoutState) -> (Position, Position) {
    if state.positions.is_empty() {
        return (
            Position::new(DIAGRAM_ORIGIN_MARGIN, DIAGRAM_ORIGIN_MARGIN),
            Position::new(DIAGRAM_ORIGIN_MARGIN, DIAGRAM_ORIGIN_MARGIN),
        );
    }
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    for pos in state.positions.values() {
        min_x = min_x.min(pos.x);
        min_y = min_y.min(pos.y);
        max_x = max_x.max(pos.x);
        max_y = max_y.max(pos.y);
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

    for ident in new_idents {
        let connected = connected_existing_positions(state, metadata, ident, new_set);
        if connected.is_empty() {
            // No connections to existing elements: place at periphery
            let periphery_x = bbox_max.x + 150.0;
            let center_y = (bbox_min.y + bbox_max.y) / 2.0;
            result.insert(ident.clone(), Position::new(periphery_x, center_y));
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
                result.insert(ident.clone(), base);
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

    let (full_graph, var_to_node) = build_full_graph(state, model, metadata)?;

    // Build constrained graph: pin existing elements, make new chains rigid groups
    let mut constrained_builder = ConstrainedGraphBuilder::new(full_graph);

    // Pin all existing (non-new) nodes
    let existing_node_ids: Vec<String> = var_to_node
        .iter()
        .filter(|(ident, _)| !new_ident_set.contains(ident.as_str()))
        .map(|(_, node_id)| node_id.clone())
        .collect();
    constrained_builder.pin(&existing_node_ids);

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

            if result.crossings < best_crossings {
                best_crossings = result.crossings;
                best_layout = Some(result.layout.clone());
                Some(result.layout)
            } else {
                None
            }
        },
    );

    let settled_layout = if let Some(saved) = best_layout {
        let final_crossings = annealing::count_crossings(&build_segments(&final_layout));
        if final_crossings > best_crossings {
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

    Ok(())
}

/// Perform three-way connector diff: compare old links in LayoutState
/// against edges derived from the current dep_graph, then preserve
/// unchanged links, remove stale ones, and create new links with
/// default shapes.
pub fn diff_connectors(
    state: &mut LayoutState,
    _model: &datamodel::Model,
    metadata: &ComputedMetadata,
) {
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

    // Remove all old links from elements
    state
        .elements
        .retain(|elem| !matches!(elem, ViewElement::Link(_)));

    // Add back preserved links (unchanged) and create new links
    for &(from_uid, to_uid) in &new_edges {
        if let Some(old_link) = old_links.get(&(from_uid, to_uid)) {
            // Preserved: keep the old link exactly as-is
            state.elements.push(old_link.clone());
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

        // Preserve existing clouds up to the number needed
        let mut preserved = 0;
        for cloud in &old_clouds {
            if preserved >= needed_count {
                if let ViewElement::Cloud(c) = cloud {
                    state.positions.remove(&c.uid);
                }
                continue;
            }
            state.elements.push(cloud.clone());
            preserved += 1;
        }

        // Create new clouds for any remaining needed count
        let remaining = needed_count.saturating_sub(preserved);
        let endpoints = flow_endpoints.get(&flow_uid);
        for i in 0..remaining {
            let is_source = if preserved == 0 {
                if i == 0 { wants_source } else { !wants_source }
            } else {
                wants_source && wants_sink && i == 0
            };

            let pos = if is_source {
                endpoints.map(|(src, _)| *src)
            } else {
                endpoints.map(|(_, sink)| *sink)
            };
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

/// Assemble a [`datamodel::StockFlow`] from finalized layout state, copying
/// metadata (name, zoom, font, sketch_compat) from `template`.
///
/// The view box is derived from the bounding box of `state.elements`; an
/// empty or degenerate element set produces a zero-area default box.
fn build_stock_flow_from_state(
    state: LayoutState,
    config: &LayoutConfig,
    template: &datamodel::StockFlow,
) -> datamodel::StockFlow {
    let (bmin_x, _bmin_y, bmax_x, bmax_y) = compute_bounds(&state.elements, config);
    let view_box = if !state.elements.is_empty() && bmin_x != f64::MAX {
        Rect {
            x: 0.0,
            y: 0.0,
            width: bmax_x + DIAGRAM_ORIGIN_MARGIN,
            height: bmax_y + DIAGRAM_ORIGIN_MARGIN,
        }
    } else {
        Rect::default()
    };
    datamodel::StockFlow {
        name: template.name.clone(),
        elements: state.elements,
        view_box,
        zoom: template.zoom,
        use_lettered_polarity: template.use_lettered_polarity,
        font: template.font.clone(),
        sketch_compat: template.sketch_compat.clone(),
    }
}

/// Seeds for parallel layout generation. Each seed produces a different SFDP
/// layout; the one with fewest connector crossings is selected.
const LAYOUT_SEEDS: [u64; 4] = [42, 123, 456, 789];

/// Apply a model patch incrementally to an existing diagram view,
/// preserving existing element positions and only placing new or
/// modified elements.
///
/// The `project` must already reflect the post-patch model state
/// (i.e., `apply_patch` has been called). The `patch` is taken by
/// reference so callers can inspect the operations; phase 6 adds
/// `Clone` derives to enable this.
///
/// Composition:
/// 1. Compute metadata for the post-patch model
/// 2. Seed LayoutState from old view
/// 3. Process deletions and renames from the patch
/// 4. Identify new elements, compute initial positions
/// 5. Create view elements and settle via pinned SFDP
/// 6. Diff connectors/clouds, polish labels and loop curvature
/// 7. Build StockFlow from final state
pub fn incremental_layout(
    old_view: &datamodel::StockFlow,
    project: &datamodel::Project,
    model_name: &str,
    patch: &crate::patch::ModelPatch,
    db_state: Option<(&mut crate::db::SimlinDb, crate::db::SourceProject)>,
) -> Result<datamodel::StockFlow, String> {
    if old_view.elements.is_empty() {
        return generate_best_layout(project, model_name, db_state);
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

    // Step 4: Identify new elements and compute initial positions
    let new_elements = state.identify_new_elements(model);

    if new_elements.is_empty() {
        // No new elements: just diff connectors/clouds and rebuild
        diff_connectors(&mut state, model, &metadata);
        diff_clouds(&mut state, &metadata);
        optimize_labels(&mut state, model, &metadata);
        apply_loop_curvature(&mut state, &config, model, &metadata);
        validate_view_completeness(&state, model)?;
        return Ok(build_stock_flow_from_state(state, &config, old_view));
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
        if let Some(&pos) = initial_positions.get(flow_ident) {
            let uid = state.get_or_alloc_uid(flow_ident);
            create_flow_view_element(&mut state, &config, &metadata, flow_ident, uid, pos)?;
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

    // Step 7: Diff connectors and clouds
    diff_connectors(&mut state, model, &metadata);
    diff_clouds(&mut state, &metadata);

    // Step 8: Polish
    optimize_labels(&mut state, model, &metadata);
    apply_loop_curvature(&mut state, &config, model, &metadata);

    validate_view_completeness(&state, model)?;

    // Step 9: Build StockFlow
    Ok(build_stock_flow_from_state(state, &config, old_view))
}

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
#[path = "layout_tests.rs"]
mod tests;
