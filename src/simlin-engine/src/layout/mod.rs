// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

pub mod annealing;
mod aux_placement;
pub mod chain;
pub mod config;
pub mod connector;
pub mod declutter;
pub mod eval_stats;
pub mod graph;
pub mod metadata;
pub mod metrics;
pub mod placement;
pub mod sfdp;
pub mod text;
pub mod uid;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::f64::consts::PI;

use self::annealing::{FlowTemplate, LineSegment, run_annealing_with_filter};
#[cfg(test)]
use self::aux_placement::MIN_AUX_LANE_OFFSET;
use self::aux_placement::{
    AuxiliaryPlacementContext, auxiliary_initial_position, enforce_auxiliary_lane_clearance,
    positioned_variables_from_layout, spread_auxiliary_initial_positions,
};
use self::chain::{DIAGRAM_ORIGIN_MARGIN, compute_chain_positions, make_cloud_node_ident};
use self::config::LayoutConfig;
use self::connector::{
    FlowOrientation, calc_stock_flow_arc_angle, calculate_loop_arc_angle, compute_flow_orientation,
    normalize_angle,
};
use self::graph::{ConstrainedGraphBuilder, Graph, GraphBuilder, Layout, Position};
use self::metadata::{ComputedMetadata, LoopPolarity, StockFlowChain};
use self::placement::{
    calculate_optimal_label_side, calculate_restricted_label_side, normalize_coordinates,
};
use self::sfdp::{SfdpConfig, compute_layout_from_initial_with_callback, should_trigger_annealing};
use self::text::{estimate_label_bounds, format_label_with_line_breaks};
use self::uid::UidManager;
use crate::common::canonicalize;
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

/// Which edge of a stock a flow attaches to during layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum StockAttachSide {
    Top,
    Bottom,
    Left,
    Right,
}

/// Computed attachment info for a flow: which stock edge and where along it.
#[derive(Clone, Copy, Debug)]
struct FlowAttachment {
    side: StockAttachSide,
    /// Fractional offset along the edge (0.0 = start, 1.0 = end).
    /// For top/bottom edges, 0.0 is the left end, 1.0 is the right end.
    /// For left/right edges, 0.0 is the top end, 1.0 is the bottom end.
    offset: f64,
}

/// Result of a single layout generation, used to select the best among parallel attempts.
struct LayoutResult {
    view: datamodel::StockFlow,
    /// The full calibrated layout-quality metric for `view` (Sigma w_i * term_i,
    /// with `MetricWeights::default()`). `select_best_layout` minimizes this; its
    /// `crossings` term already captures the accurate connector-crossing count, so
    /// there is no separate `crossings` field.
    weighted_cost: f64,
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
                    // Groups are not looked up by name for variable operations,
                    // so register with an empty ident to prevent collisions with
                    // model variables that happen to share the same name.
                    uid_manager.add(g.uid, "");
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

    /// Remove a variable and its associated view elements (clouds, aliases, links)
    /// from the layout state.
    ///
    /// The ident-to-UID mapping is intentionally retained in `uid_manager` so that
    /// `identify_new_elements` can detect the UID as orphaned (present in uid_manager
    /// but absent from elements) and rebuild the element with the correct type.
    /// This is used by the flow-reset and kind-change paths in `incremental_layout`.
    pub fn apply_deletion(&mut self, deleted_ident: &str) {
        let canonical = canonicalize(deleted_ident);
        let deleted_uid = match self.uid_manager.get_uid(&canonical) {
            Some(uid) => uid,
            None => return,
        };

        self.positions.remove(&deleted_uid);

        // Collect UIDs of clouds and aliases being removed so we can clean up their positions
        let removed_cloud_uids: Vec<i32> = self
            .elements
            .iter()
            .filter_map(|elem| match elem {
                ViewElement::Cloud(c) if c.flow_uid == deleted_uid => Some(c.uid),
                _ => None,
            })
            .collect();

        let removed_alias_uids: Vec<i32> = self
            .elements
            .iter()
            .filter_map(|elem| match elem {
                ViewElement::Alias(a) if a.alias_of_uid == deleted_uid => Some(a.uid),
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
            ViewElement::Alias(a) if a.alias_of_uid == deleted_uid => false,
            _ => true,
        });

        for cloud_uid in &removed_cloud_uids {
            self.positions.remove(cloud_uid);
        }

        for alias_uid in &removed_alias_uids {
            self.positions.remove(alias_uid);
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
            let formatted = format_label_with_line_breaks(new_display_name);
            match elem {
                ViewElement::Aux(a) => a.name = formatted,
                ViewElement::Stock(s) => s.name = formatted,
                ViewElement::Flow(f) => f.name = formatted,
                ViewElement::Module(m) => m.name = formatted,
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

/// Bounding box of variable elements (stocks, flows, auxes, modules) only.
/// Excludes aliases, groups, and clouds so that outlier non-variable elements
/// don't push new variable placement far from the actual model graph.
/// Returns ((min_x, min_y), (max_x, max_y)).
/// When no variable elements exist, returns a default origin area.
fn existing_bounding_box(state: &LayoutState) -> (Position, Position) {
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

/// Classify which stock edge each flow should attach to and compute
/// even spacing offsets.
///
/// When a stock has a chain flow (stock-to-stock) going right, non-chain
/// outflows (stock-to-cloud) exit from the bottom. Symmetrically, when a
/// stock has a chain inflow from the left, non-chain inflows enter from
/// the top. Multiple flows on the same side are distributed using the
/// `(i+1)/(n+1)` formula (matching the TS editor's `computeFlowOffsets`).
///
/// Returns a map from flow ident to its attachment info for all flows
/// connected to this stock.
fn classify_flow_sides(
    stock_ident: &str,
    metadata: &ComputedMetadata,
) -> HashMap<String, FlowAttachment> {
    let mut result = HashMap::new();

    let outflows: Vec<String> = metadata
        .stock_to_outflows
        .get(stock_ident)
        .cloned()
        .unwrap_or_default();
    let inflows: Vec<String> = metadata
        .stock_to_inflows
        .get(stock_ident)
        .cloned()
        .unwrap_or_default();

    // Classify outflows: chain (stock-to-stock) vs side (stock-to-cloud)
    let mut chain_outflows = Vec::new();
    let mut side_outflows = Vec::new();
    for flow in &outflows {
        let (_, to_stock) = metadata.connected_stocks(flow);
        if to_stock.is_some() {
            chain_outflows.push(flow.clone());
        } else {
            side_outflows.push(flow.clone());
        }
    }

    // Classify inflows: chain (stock-to-stock) vs side (cloud-to-stock)
    let mut chain_inflows = Vec::new();
    let mut side_inflows = Vec::new();
    for flow in &inflows {
        let (from_stock, _) = metadata.connected_stocks(flow);
        if from_stock.is_some() {
            chain_inflows.push(flow.clone());
        } else {
            side_inflows.push(flow.clone());
        }
    }

    // Outflow placement: if chain outflows exist, side outflows go to Bottom
    let side_outflow_side = if !chain_outflows.is_empty() {
        StockAttachSide::Bottom
    } else {
        StockAttachSide::Right
    };

    // Inflow placement: if chain inflows exist, side inflows go to Top
    let side_inflow_side = if !chain_inflows.is_empty() {
        StockAttachSide::Top
    } else {
        StockAttachSide::Left
    };

    // Group all outflows by their assigned side
    let mut right_flows: Vec<String> = Vec::new();
    let mut bottom_flows: Vec<String> = Vec::new();
    for flow in &chain_outflows {
        right_flows.push(flow.clone());
    }
    for flow in &side_outflows {
        match side_outflow_side {
            StockAttachSide::Bottom => bottom_flows.push(flow.clone()),
            StockAttachSide::Right => right_flows.push(flow.clone()),
            StockAttachSide::Top | StockAttachSide::Left => right_flows.push(flow.clone()),
        }
    }

    // Group all inflows by their assigned side
    let mut left_flows: Vec<String> = Vec::new();
    let mut top_flows: Vec<String> = Vec::new();
    for flow in &chain_inflows {
        left_flows.push(flow.clone());
    }
    for flow in &side_inflows {
        match side_inflow_side {
            StockAttachSide::Top => top_flows.push(flow.clone()),
            StockAttachSide::Left => left_flows.push(flow.clone()),
            StockAttachSide::Bottom | StockAttachSide::Right => left_flows.push(flow.clone()),
        }
    }

    // Distribute flows within each side group using (i+1)/(n+1)
    let assign_side = |flows: &mut [String],
                       side: StockAttachSide,
                       result: &mut HashMap<String, FlowAttachment>| {
        flows.sort(); // deterministic ordering by ident
        let n = flows.len();
        for (i, flow) in flows.iter().enumerate() {
            let offset = if n == 1 {
                0.5
            } else {
                (i as f64 + 1.0) / (n as f64 + 1.0)
            };
            result.insert(flow.clone(), FlowAttachment { side, offset });
        }
    };

    assign_side(&mut right_flows, StockAttachSide::Right, &mut result);
    assign_side(&mut bottom_flows, StockAttachSide::Bottom, &mut result);
    assign_side(&mut left_flows, StockAttachSide::Left, &mut result);
    assign_side(&mut top_flows, StockAttachSide::Top, &mut result);

    result
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
    Some(match attachment.side {
        StockAttachSide::Bottom => {
            let x = stock_pos.x - config.stock_width / 2.0 + config.stock_width * attachment.offset;
            Position::new(
                x,
                stock_pos.y + config.stock_height / 2.0 + config.horizontal_spacing / 2.0,
            )
        }
        StockAttachSide::Top => {
            let x = stock_pos.x - config.stock_width / 2.0 + config.stock_width * attachment.offset;
            Position::new(
                x,
                stock_pos.y - config.stock_height / 2.0 - config.horizontal_spacing / 2.0,
            )
        }
        StockAttachSide::Right => {
            let y =
                stock_pos.y - config.stock_height / 2.0 + config.stock_height * attachment.offset;
            Position::new(
                stock_pos.x + config.stock_width / 2.0 + config.horizontal_spacing / 2.0,
                y,
            )
        }
        StockAttachSide::Left => {
            let y =
                stock_pos.y - config.stock_height / 2.0 + config.stock_height * attachment.offset;
            Position::new(
                stock_pos.x - config.stock_width / 2.0 - config.horizontal_spacing / 2.0,
                y,
            )
        }
    })
}

/// Create a single flow view element with its flow points and clouds.
fn create_flow_view_element(
    state: &mut LayoutState,
    config: &LayoutConfig,
    metadata: &ComputedMetadata,
    flow_ident: &str,
    uid: i32,
    pos: Position,
    flow_attachments: &HashMap<String, FlowAttachment>,
) -> Result<(), String> {
    let (from_stock, to_stock) = metadata.connected_stocks(flow_ident);
    let from_stock = from_stock.map(|s| s.to_string());
    let to_stock = to_stock.map(|s| s.to_string());
    let name = state.display_name(flow_ident);
    let formatted = format_label_with_line_breaks(&name);
    let attachment = flow_attachments.get(flow_ident).copied();

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

            // One endpoint per stock, on the face the valve approaches from --
            // the same aspect-normalized rule `resnap_flow_endpoints` uses, so
            // a later resnap of a chain-built flow is a no-op. For two stocks
            // on the same horizontal line this reduces to the classic
            // right-edge -> left-edge horizontal pipe; for a vertically fanned
            // branch stock it exits/enters the top or bottom face instead.
            let endpoint = |stock_pos: Position, stock_uid: i32| -> FlowPoint {
                let half_w = config.stock_width / 2.0;
                let half_h = config.stock_height / 2.0;
                let dx = pos.x - stock_pos.x;
                let dy = pos.y - stock_pos.y;
                if half_h * dx.abs() >= half_w * dy.abs() {
                    FlowPoint {
                        x: stock_pos.x + dx.signum() * half_w,
                        y: pos.y.clamp(stock_pos.y - half_h, stock_pos.y + half_h),
                        attached_to_uid: Some(stock_uid),
                    }
                } else {
                    FlowPoint {
                        x: pos.x.clamp(stock_pos.x - half_w, stock_pos.x + half_w),
                        y: stock_pos.y + dy.signum() * half_h,
                        attached_to_uid: Some(stock_uid),
                    }
                }
            };
            vec![endpoint(from_pos, from_uid), endpoint(to_pos, to_uid)]
        }
        (Some(from), None) => {
            let from_uid = state.get_or_alloc_uid(from);
            let from_pos = state
                .positions
                .get(&from_uid)
                .copied()
                .unwrap_or(Position::new(pos.x - 50.0, pos.y));
            match attachment {
                Some(FlowAttachment {
                    side: StockAttachSide::Bottom,
                    offset,
                }) => {
                    // Vertical flow exiting from the bottom of the stock
                    let attach_x =
                        from_pos.x - config.stock_width / 2.0 + config.stock_width * offset;
                    vec![
                        FlowPoint {
                            x: attach_x,
                            y: from_pos.y + config.stock_height / 2.0,
                            attached_to_uid: Some(from_uid),
                        },
                        FlowPoint {
                            x: attach_x,
                            y: pos.y + 50.0,
                            attached_to_uid: None,
                        },
                    ]
                }
                Some(FlowAttachment {
                    side: StockAttachSide::Right,
                    offset,
                }) => {
                    // Horizontal flow exiting to the right, offset along
                    // the right edge for multi-flow distribution
                    let attach_y =
                        from_pos.y - config.stock_height / 2.0 + config.stock_height * offset;
                    vec![
                        FlowPoint {
                            x: from_pos.x + config.stock_width / 2.0,
                            y: attach_y,
                            attached_to_uid: Some(from_uid),
                        },
                        FlowPoint {
                            x: pos.x + 50.0,
                            y: pos.y,
                            attached_to_uid: None,
                        },
                    ]
                }
                _ => {
                    // Default: horizontal flow exiting to the right
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
            }
        }
        (None, Some(to)) => {
            let to_uid = state.get_or_alloc_uid(to);
            let to_pos = state
                .positions
                .get(&to_uid)
                .copied()
                .unwrap_or(Position::new(pos.x + 50.0, pos.y));
            match attachment {
                Some(FlowAttachment {
                    side: StockAttachSide::Top,
                    offset,
                }) => {
                    // Vertical flow entering from the top of the stock
                    let attach_x =
                        to_pos.x - config.stock_width / 2.0 + config.stock_width * offset;
                    vec![
                        FlowPoint {
                            x: attach_x,
                            y: pos.y - 50.0,
                            attached_to_uid: None,
                        },
                        FlowPoint {
                            x: attach_x,
                            y: to_pos.y - config.stock_height / 2.0,
                            attached_to_uid: Some(to_uid),
                        },
                    ]
                }
                Some(FlowAttachment {
                    side: StockAttachSide::Left,
                    offset,
                }) => {
                    // Horizontal flow entering from the left, offset along
                    // the left edge for multi-flow distribution
                    let attach_y =
                        to_pos.y - config.stock_height / 2.0 + config.stock_height * offset;
                    vec![
                        FlowPoint {
                            x: pos.x - 50.0,
                            y: pos.y,
                            attached_to_uid: None,
                        },
                        FlowPoint {
                            x: to_pos.x - config.stock_width / 2.0,
                            y: attach_y,
                            attached_to_uid: Some(to_uid),
                        },
                    ]
                }
                _ => {
                    // Default: horizontal flow entering from the left
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
            }
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
    flow_attachments: &HashMap<String, FlowAttachment>,
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
            create_flow_view_element(
                state,
                config,
                metadata,
                flow_ident,
                uid,
                pos,
                flow_attachments,
            )?;
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
            let empty_attachments = HashMap::new();
            for flow_ident in flows {
                let uid = state.get_or_alloc_uid(flow_ident);
                create_flow_view_element(
                    state,
                    config,
                    metadata,
                    flow_ident,
                    uid,
                    base_position,
                    &empty_attachments,
                )?;
            }
            return Ok(());
        }
    };

    // Precompute flow attachment sides for all stocks in this chain.
    // This avoids redundant calls to classify_flow_sides during BFS.
    let mut flow_attachments: HashMap<String, FlowAttachment> = HashMap::new();
    for stock_ident in stocks {
        let sides = classify_flow_sides(stock_ident, metadata);
        flow_attachments.extend(sides);
    }

    // Flows connecting the same stock pair (bidirectional compartment
    // exchange) draw as parallel pipes: precompute each flow's slot among its
    // pair's flows so its valve takes a distinct perpendicular offset. Keyed by
    // the canonically-ordered (sorted) pair so both directions of an exchange
    // agree on the perpendicular axis. The map's CONTENT is deterministic
    // (each sibling list comes from the deterministic `flows` slice order)
    // even though the outer HashMap iteration order is not.
    let pair_slots: HashMap<String, (usize, usize)> = {
        let mut pair_flows: HashMap<(String, String), Vec<String>> = HashMap::new();
        for flow_ident in flows {
            let (from, to) = metadata.connected_stocks(flow_ident);
            if let (Some(from), Some(to)) = (from, to) {
                let key = if from <= to {
                    (from.to_string(), to.to_string())
                } else {
                    (to.to_string(), from.to_string())
                };
                pair_flows.entry(key).or_default().push(flow_ident.clone());
            }
        }
        let mut slots = HashMap::new();
        for siblings in pair_flows.values() {
            for (idx, flow_ident) in siblings.iter().enumerate() {
                slots.insert(flow_ident.clone(), (idx, siblings.len()));
            }
        }
        slots
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

                // Positions of every stock placed so far in this chain. The
                // `positioned` map also holds flow valves, so filter to the
                // chain's stock idents (deterministic slice order).
                let occupied_stock_positions = |positioned: &HashMap<String, Position>| {
                    stocks
                        .iter()
                        .filter_map(|s| positioned.get(s).copied())
                        .collect::<Vec<Position>>()
                };

                let flow_pos = match (from_stock, to_stock) {
                    (Some(from), Some(to)) => {
                        let from = from.to_string();
                        let to = to.to_string();
                        if item.connected_to == from {
                            // Position sink stock to the right; if a stock
                            // already occupies that spot (a branching
                            // topology), fan vertically to the nearest free
                            // position.
                            if !positioned.contains_key(&to) {
                                let natural = Position::new(
                                    item.position.x
                                        + config.stock_width
                                        + config.horizontal_spacing,
                                    item.position.y,
                                );
                                let occupied = occupied_stock_positions(&positioned);
                                let other_pos =
                                    chain::find_free_stock_position(natural, &occupied, config);
                                positioned.insert(to.clone(), other_pos);
                                queue.push_back(WorkItem {
                                    id: to.clone(),
                                    item_type: WorkItemType::Stock,
                                    position: other_pos,
                                    connected_to: String::new(),
                                });
                            }
                            // Valve at the midpoint of the two stocks it
                            // connects (a fanned branch stock is no longer at
                            // the source's y), offset perpendicular when
                            // several flows connect this same pair.
                            let to_pos = positioned[&to];
                            let (a_pos, b_pos) = if from <= to {
                                (item.position, to_pos)
                            } else {
                                (to_pos, item.position)
                            };
                            let (idx, count) = pair_slots.get(&item.id).copied().unwrap_or((0, 1));
                            chain::stock_pair_valve_position(a_pos, b_pos, idx, count)
                        } else {
                            // Position source stock to the left, fanning past
                            // any stock already on that spot.
                            if !positioned.contains_key(&from) {
                                let natural = Position::new(
                                    item.position.x
                                        - config.stock_width
                                        - config.horizontal_spacing,
                                    item.position.y,
                                );
                                let occupied = occupied_stock_positions(&positioned);
                                let other_pos =
                                    chain::find_free_stock_position(natural, &occupied, config);
                                positioned.insert(from.clone(), other_pos);
                                queue.push_back(WorkItem {
                                    id: from.clone(),
                                    item_type: WorkItemType::Stock,
                                    position: other_pos,
                                    connected_to: String::new(),
                                });
                            }
                            // Mirror of the branch above: valve at the pair
                            // midpoint with a perpendicular slot offset.
                            let from_pos = positioned[&from];
                            let (a_pos, b_pos) = if from <= to {
                                (from_pos, item.position)
                            } else {
                                (item.position, from_pos)
                            };
                            let (idx, count) = pair_slots.get(&item.id).copied().unwrap_or((0, 1));
                            chain::stock_pair_valve_position(a_pos, b_pos, idx, count)
                        }
                    }
                    (Some(_), None) => {
                        // Outflow to cloud: check if it should go downward
                        match flow_attachments.get(&item.id).copied() {
                            Some(FlowAttachment {
                                side: StockAttachSide::Bottom,
                                offset,
                            }) => {
                                let x_offset = item.position.x - config.stock_width / 2.0
                                    + config.stock_width * offset;
                                Position::new(
                                    x_offset,
                                    item.position.y
                                        + config.stock_height / 2.0
                                        + config.horizontal_spacing / 2.0,
                                )
                            }
                            Some(FlowAttachment {
                                side: StockAttachSide::Right,
                                offset,
                            }) => {
                                let y_offset = item.position.y - config.stock_height / 2.0
                                    + config.stock_height * offset;
                                Position::new(
                                    item.position.x
                                        + config.stock_width / 2.0
                                        + config.horizontal_spacing / 2.0,
                                    y_offset,
                                )
                            }
                            _ => Position::new(
                                item.position.x
                                    + config.stock_width / 2.0
                                    + config.horizontal_spacing / 2.0,
                                item.position.y,
                            ),
                        }
                    }
                    (None, Some(_)) => {
                        // Inflow from cloud: check if it should come from above
                        match flow_attachments.get(&item.id).copied() {
                            Some(FlowAttachment {
                                side: StockAttachSide::Top,
                                offset,
                            }) => {
                                let x_offset = item.position.x - config.stock_width / 2.0
                                    + config.stock_width * offset;
                                Position::new(
                                    x_offset,
                                    item.position.y
                                        - config.stock_height / 2.0
                                        - config.horizontal_spacing / 2.0,
                                )
                            }
                            Some(FlowAttachment {
                                side: StockAttachSide::Left,
                                offset,
                            }) => {
                                let y_offset = item.position.y - config.stock_height / 2.0
                                    + config.stock_height * offset;
                                Position::new(
                                    item.position.x
                                        - config.stock_width / 2.0
                                        - config.horizontal_spacing / 2.0,
                                    y_offset,
                                )
                            }
                            _ => Position::new(
                                item.position.x
                                    - config.stock_width / 2.0
                                    - config.horizontal_spacing / 2.0,
                                item.position.y,
                            ),
                        }
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
    create_view_elements(
        state,
        config,
        metadata,
        &positioned,
        stocks,
        flows,
        &flow_attachments,
    )
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

/// The force graph for auxiliary placement plus its variable bookkeeping:
/// which variable each node represents, and which variables were left OUT of
/// the graph because nothing connects them (see `build_full_graph`).
struct FullGraph {
    graph: Graph<String>,
    var_to_node: HashMap<String, String>,
    /// Variables with no dependency edges and no chain position, sorted.
    /// They take no part in the force simulation; the fresh-layout path parks
    /// them below the diagram, the incremental path leaves them where
    /// `compute_new_element_positions` put them.
    isolated_vars: Vec<String>,
}

/// Build an undirected graph with all model variables and cloud nodes for SFDP.
fn build_full_graph(
    state: &mut LayoutState,
    model: &datamodel::Model,
    metadata: &ComputedMetadata,
) -> Result<FullGraph, String> {
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

    // A variable is ISOLATED when nothing ties it to the rest of the diagram:
    // no dependency edge in either direction and no place in an
    // already-positioned stock-flow chain. Isolated variables get a
    // `var_to_node` entry (downstream element creation looks every variable up
    // there) but NO graph node: an edge-less node only ever repels, so leaving
    // it in the force simulation both distorts its neighbors and -- under any
    // properly-converging step scheme -- flings it unboundedly. The fresh
    // layout parks them in a row below the diagram instead
    // (`park_isolated_nodes`); the incremental path leaves them where
    // `compute_new_element_positions` put them.
    let mut connected: BTreeSet<String> = BTreeSet::new();
    for (from_ident, deps) in &metadata.dep_graph {
        for to_ident in deps {
            if all_vars.contains(to_ident) {
                connected.insert(from_ident.clone());
                connected.insert(to_ident.clone());
            }
        }
    }
    for var_ident in &all_vars {
        let positioned = state
            .uid_manager
            .get_uid(var_ident)
            .is_some_and(|uid| state.positions.contains_key(&uid));
        if positioned {
            connected.insert(var_ident.clone());
        }
    }
    let isolated: Vec<String> = all_vars
        .iter()
        .filter(|v| !connected.contains(*v))
        .cloned()
        .collect();

    for var_ident in &all_vars {
        let node_id = format!("node_{}", node_index);
        var_to_node.insert(var_ident.clone(), node_id.clone());
        node_to_var.insert(node_id.clone(), var_ident.clone());
        if connected.contains(var_ident) {
            builder.add_node(node_id);
        }
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

    Ok(FullGraph {
        graph: builder.build(),
        var_to_node,
        isolated_vars: isolated,
    })
}

/// Breathing room between adjacent parked elements' label boxes.
const PARKED_LABEL_GAP: f64 = 12.0;

/// Vertical pitch between parked rows: enough for an aux shape plus a
/// two-line label below it.
const PARKED_ROW_PITCH: f64 = 75.0;

/// Place isolated variables in a tidy reading-order grid just below the
/// laid-out diagram, mirroring how human modelers park unused/exogenous
/// constants at the edge of a sketch (e.g. the parameter rows in the
/// beer-game reference view).
///
/// Spacing is LABEL-AWARE: each element is placed far enough from its neighbor
/// that their (centered, below-element) labels cannot overlap, using the same
/// text measurement the renderer uses. A fixed pitch is not enough -- real
/// models park constants with very long names (covid19's
/// "cumulative reported deaths ..." data variables).
///
/// `isolated_vars` must be sorted (it is: `build_full_graph` derives it from a
/// `BTreeSet`) so the parking order -- and therefore the layout -- is
/// deterministic per seed (#633). Variables missing from `var_to_node` are
/// skipped defensively.
fn park_isolated_nodes(
    layout: &mut Layout<String>,
    isolated_vars: &[String],
    var_to_node: &HashMap<String, String>,
    state: &LayoutState,
    config: &LayoutConfig,
) {
    if isolated_vars.is_empty() {
        return;
    }

    // Park below everything the force pass placed. A layout with no positioned
    // nodes at all (a model of only isolated constants) parks at the origin.
    let (min_x, max_x, max_y) = if layout.is_empty() {
        (
            config.start_x,
            config.start_x,
            config.start_y - config.vertical_spacing,
        )
    } else {
        layout
            .values()
            .fold((f64::MAX, f64::MIN, f64::MIN), |(mnx, mxx, mxy), pos| {
                (mnx.min(pos.x), mxx.max(pos.x), mxy.max(pos.y))
            })
    };

    // The half-width of an element's label box, measured EXACTLY as the
    // layout-quality metric measures it (`metrics::element_label_props` ->
    // `diagram::label::label_bounds` over the diagram display name of the
    // element's formatted name). Using the same measurement guarantees the
    // parking spacing keeps `label_overlap` at zero by construction; the
    // layout-internal `estimate_label_bounds` uses a different (Praxis) text
    // width model and disagrees on long names.
    let label_half_width = |var_ident: &str| -> f64 {
        use crate::diagram::constants::AUX_RADIUS;
        use crate::diagram::label::{LabelProps, label_bounds};

        // What the created element will be named, and what the metric measures.
        let elem_name = format_label_with_line_breaks(&state.display_name(var_ident));
        let metric_text = crate::diagram::common::display_name(&elem_name);
        let props = LabelProps::new(0.0, 0.0, LabelSide::Bottom, metric_text)
            .with_radii(AUX_RADIUS, AUX_RADIUS);
        let rect = label_bounds(&props);
        ((rect.right - rect.left) / 2.0).max(config.aux_width / 2.0)
    };

    // Wrap rows at the diagram's own width (or a few lanes for tiny diagrams),
    // so the parking area extends the diagram downward rather than sideways.
    let row_width_limit = (max_x - min_x).max(4.0 * config.horizontal_spacing);

    let mut x = min_x;
    let mut y = max_y + config.vertical_spacing;
    let mut prev_half_width: Option<f64> = None;
    for var_ident in isolated_vars {
        let Some(node_id) = var_to_node.get(var_ident) else {
            continue;
        };
        let half_width = label_half_width(var_ident);
        if let Some(prev) = prev_half_width {
            // Far enough that the two centered labels cannot touch.
            x += (prev + half_width + PARKED_LABEL_GAP).max(config.horizontal_spacing);
            if x - min_x > row_width_limit {
                x = min_x;
                y += PARKED_ROW_PITCH;
            }
        }
        layout.insert(node_id.clone(), Position::new(x, y));
        prev_half_width = Some(half_width);
    }
}

/// The polyline segments the renderer will draw for a RECIPROCAL dependency
/// pair (A depends on B and B depends on A) at the candidate positions: two
/// arcs, sampled with the renderer's own arc geometry.
///
/// `build_connectors` turns each direction of a reciprocal pair into an
/// `Arc(calc_reciprocal_arc_angle(..))` link, so the drawn geometry bulges away
/// from the straight chord. The annealing must count crossings on that drawn
/// geometry: a straight chord between reciprocal nodes never crosses what the
/// arc crosses, which previously left arc-vs-link crossings invisible to (and
/// therefore never fixed by) the crossing-reduction pass.
///
/// Interior polyline vertices get per-arc names so two arcs of the same pair
/// (which share both endpoints) never count as crossing each other, while
/// arc-vs-other-link crossings are counted normally.
fn reciprocal_arc_segments(
    from_pos: Position,
    to_pos: Position,
    from_node: &str,
    to_node: &str,
) -> Vec<LineSegment> {
    let make_aux = |uid: i32, pos: Position| {
        ViewElement::Aux(view_element::Aux {
            name: String::new(),
            uid,
            x: pos.x,
            y: pos.y,
            label_side: LabelSide::Bottom,
            compat: None,
        })
    };
    let not_arrayed = |_: &str| false;

    let mut segments = Vec::new();
    // Both directions of the pair are drawn, each with its own arc angle.
    for (a_pos, b_pos, a_node, b_node) in [
        (from_pos, to_pos, from_node, to_node),
        (to_pos, from_pos, to_node, from_node),
    ] {
        let from_elem = make_aux(1, a_pos);
        let to_elem = make_aux(2, b_pos);
        let link = view_element::Link {
            uid: 3,
            from_uid: 1,
            to_uid: 2,
            shape: LinkShape::Arc(calc_reciprocal_arc_angle(a_pos, b_pos)),
            polarity: None,
        };
        let polyline = crate::diagram::connector::connector_polyline(
            &link,
            &from_elem,
            &to_elem,
            &not_arrayed,
            crate::diagram::connector::ARC_POLYLINE_SAMPLES,
        );
        let last = polyline.len().saturating_sub(1);
        for (i, w) in polyline.windows(2).enumerate() {
            // Endpoints keep the node names (shared-endpoint suppression);
            // interior vertices are unique to this arc.
            let seg_from = if i == 0 {
                a_node.to_string()
            } else {
                format!("{a_node}\u{2192}{b_node}#{i}")
            };
            let seg_to = if i + 1 == last {
                b_node.to_string()
            } else {
                format!("{a_node}\u{2192}{b_node}#{}", i + 1)
            };
            segments.push(LineSegment {
                start: Position::new(w[0].x, w[0].y),
                end: Position::new(w[1].x, w[1].y),
                from_node: seg_from,
                to_node: seg_to,
            });
        }
    }
    segments
}

/// Minimum center-to-center distance between two point nodes (auxes/modules)
/// before the annealing's cost charges them as piled up. Two auxes need at
/// least their shapes (2 x AUX_RADIUS = 18) plus label breathing room apart;
/// half a lane (50) is the same floor `MIN_AUX_LANE_OFFSET` builds on.
const MIN_POINT_NODE_SEPARATION: f64 = 50.0;

/// The number of point-node pairs in `layout` that sit closer than
/// `MIN_POINT_NODE_SEPARATION`. Added to the annealing cost so the
/// crossing-reduction pass cannot "fix" a crossing by piling nodes on top of
/// each other -- crossings and pile-ups are both unreadable, and a
/// crossings-only objective is blind to the latter (it once collapsed two
/// auxes onto the same spot to remove a chord crossing).
fn point_node_pileup_count(layout: &Layout<String>, point_node_ids: &HashSet<String>) -> usize {
    let positions: Vec<Position> = point_node_ids
        .iter()
        .filter_map(|node_id| layout.get(node_id).copied())
        .collect();
    let mut count = 0;
    for i in 0..positions.len() {
        for j in (i + 1)..positions.len() {
            let d = positions[i] - positions[j];
            if d.length() < MIN_POINT_NODE_SEPARATION {
                count += 1;
            }
        }
    }
    count
}

/// Run SFDP with chain elements locked into rigid groups.
fn run_sfdp_with_rigid_chains(
    state: &LayoutState,
    config: &LayoutConfig,
    model: &datamodel::Model,
    metadata: &ComputedMetadata,
    full_graph: FullGraph,
    chains_data: &[(Vec<String>, Vec<String>, Vec<String>)],
) -> Result<Layout<String>, String> {
    let FullGraph {
        graph: full_graph,
        var_to_node,
        isolated_vars,
    } = full_graph;
    let var_to_node = &var_to_node;
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

    // `var_to_node` is a HashMap, so its iteration order is per-process random.
    // Two loops below are order-sensitive: the centroid accumulation sums floats
    // (non-associative, so hash order perturbs the result) and the aux-placement
    // loop assigns each unpositioned aux a polar seed angle by its iteration rank.
    // Materialize a deterministic sorted view and iterate THAT in both loops so a
    // fixed (model, seed) yields a bit-identical layout across repeated calls (#633).
    let mut entries: Vec<(&String, &String)> = var_to_node.iter().collect();
    entries.sort();

    for &(var_ident, node_id) in &entries {
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

    let positioned_by_ident = positioned_variables_from_layout(var_to_node, &initial_layout);
    let aux_ctx = AuxiliaryPlacementContext::new(
        &metadata.dep_graph,
        &metadata.reverse_dep_graph,
        &metadata.flow_to_stocks,
        &metadata.feedback_loops,
    );
    let global_center = Position::new(center_x, center_y);
    // Isolated variables take no part in the force simulation (they have no
    // graph node); skip them here so they get neither an anchor proposal nor a
    // fallback ring slot -- they are parked after the layout settles.
    let isolated_set: HashSet<&str> = isolated_vars.iter().map(|s| s.as_str()).collect();
    let mut proposals = Vec::new();
    let mut fallback_index = 0;
    for &(var_ident, node_id) in &entries {
        if initial_layout.contains_key(node_id) || isolated_set.contains(var_ident.as_str()) {
            continue;
        }
        if let Some(proposal) = auxiliary_initial_position(
            var_ident,
            &positioned_by_ident,
            &aux_ctx,
            global_center,
            fallback_index,
        ) {
            proposals.push((var_ident.clone(), node_id.clone(), proposal));
        }
        fallback_index += 1;
    }
    for (node_id, pos) in spread_auxiliary_initial_positions(proposals) {
        initial_layout.insert(node_id, pos);
    }

    for &(var_ident, node_id) in &entries {
        if initial_layout.contains_key(node_id) || isolated_set.contains(var_ident.as_str()) {
            continue;
        }
        let angle = fallback_index as f64 * 2.0 * PI / 8.0;
        initial_layout.insert(
            node_id.clone(),
            Position::new(
                center_x + 120.0 * angle.cos(),
                center_y + 120.0 * angle.sin(),
            ),
        );
        fallback_index += 1;
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
    let point_node_ids: HashSet<String> = model
        .variables
        .iter()
        .filter_map(|var| {
            let ident = match var {
                datamodel::Variable::Aux(aux) => &aux.ident,
                datamodel::Variable::Module(module) => &module.ident,
                _ => return None,
            };
            let canonical = canonicalize(ident);
            var_to_node.get(canonical.as_ref()).cloned()
        })
        .collect();
    let point_idents: HashSet<String> = model
        .variables
        .iter()
        .filter_map(|var| match var {
            datamodel::Variable::Aux(aux) => Some(canonicalize(&aux.ident).into_owned()),
            datamodel::Variable::Module(module) => Some(canonicalize(&module.ident).into_owned()),
            _ => None,
        })
        .collect();

    // Reciprocal dependency pairs (A depends on B AND B depends on A) will be
    // drawn as a pair of arcs by `build_connectors`; the annealing must count
    // crossings on that arc geometry, not on the straight chord (see
    // `reciprocal_arc_segments`).
    let reciprocal_idents: HashSet<(String, String)> = metadata
        .dep_graph
        .iter()
        .flat_map(|(from, deps)| {
            deps.iter()
                .filter(|to| {
                    metadata
                        .dep_graph
                        .get(*to)
                        .is_some_and(|back| back.contains(from))
                })
                .map(|to| (from.clone(), to.clone()))
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

            let idents = (node_to_ident.get(&edge.from), node_to_ident.get(&edge.to));
            if let (Some(from_ident), Some(to_ident)) = idents {
                if is_structural_stock_flow(from_ident, to_ident, &stock_inflows, &stock_outflows) {
                    continue;
                }
                if reciprocal_idents.contains(&(from_ident.clone(), to_ident.clone())) {
                    // Drawn as two arcs; count crossings on the drawn geometry.
                    segments.extend(reciprocal_arc_segments(
                        from_pos, to_pos, &edge.from, &edge.to,
                    ));
                    continue;
                }
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

    // The annealing cost: drawn-geometry crossings PLUS a penalty for point
    // nodes piled closer than `MIN_POINT_NODE_SEPARATION`. Without the penalty
    // the search is free to remove a crossing by stacking two auxes on the
    // same spot (both unreadable; only one was counted).
    let pileup_penalty =
        |layout: &Layout<String>| point_node_pileup_count(layout, &point_node_ids) as f64;

    // The full annealing cost of a layout, for keep-or-discard comparisons.
    let annealing_cost = |layout: &Layout<String>| -> f64 {
        annealing::count_crossings(&build_segments(layout)) as f64 + pileup_penalty(layout)
    };

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
                pileup_penalty,
                &annealing_config,
                annealing_seed.wrapping_add(annealing_round as u64),
                |node_id: &String| point_node_ids.contains(node_id),
                |node_id: &String| {
                    if point_node_ids.contains(node_id) {
                        max_delta_aux
                    } else {
                        max_delta_chain
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

    // If SFDP drifted after a good annealing round, the final layout
    // may be worse than the best we found. Compare and keep the better one.
    let mut chosen = match best_layout {
        Some(saved) if annealing_cost(&final_layout) > best_cost => saved,
        _ => final_layout,
    };

    // Lane clearance pushes auxes off the lines connecting their anchors. It is
    // crossing-OBLIVIOUS, so it must run before the final crossing-reduction
    // pass below -- otherwise it silently undoes the annealing's work (it once
    // turned a crossing-free 4-element layout into one with two crossings).
    enforce_auxiliary_lane_clearance(
        &mut chosen,
        var_to_node,
        &aux_ctx,
        &point_idents,
        metadata.chains.len() <= 1,
    );

    // The LAST positioning word goes to a crossing-reduction pass on the
    // settled, lane-cleared layout. This both cleans up anything lane clearance
    // disturbed and guarantees at least one annealing pass runs even for
    // layouts that converge before the first interleaved annealing interval
    // (`annealing_interval` SFDP iterations -- which small models finish well
    // within).
    let settled_result = run_annealing_with_filter(
        &chosen,
        build_segments,
        pileup_penalty,
        &annealing_config,
        annealing_seed.wrapping_add(annealing_round as u64),
        |node_id: &String| point_node_ids.contains(node_id),
        |node_id: &String| {
            if point_node_ids.contains(node_id) {
                max_delta_aux
            } else {
                max_delta_chain
            }
        },
        &adjacency,
    );
    if settled_result.cost < annealing_cost(&chosen) {
        chosen = settled_result.layout;
    }

    // Isolated variables took no part in the force simulation; give them tidy
    // parking positions below everything that did.
    park_isolated_nodes(&mut chosen, &isolated_vars, var_to_node, state, config);

    Ok(chosen)
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

    let full_graph = build_full_graph(state, model, metadata)?;

    if full_graph.graph.node_count() == 0 && full_graph.isolated_vars.is_empty() {
        return Ok(());
    }

    // Clone the bookkeeping the post-SFDP element-creation steps need;
    // `run_sfdp_with_rigid_chains` consumes the graph itself.
    let var_to_node = full_graph.var_to_node.clone();

    let layout =
        run_sfdp_with_rigid_chains(state, config, model, metadata, full_graph, chains_data)?;

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
            ViewElement::Alias(a) => {
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
            }
            ViewElement::Group(g) => {
                update_rect(
                    &mut min_x,
                    &mut min_y,
                    &mut max_x,
                    &mut max_y,
                    g.x,
                    g.y,
                    g.x + g.width,
                    g.y + g.height,
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

    // Phase 4b: Deterministic label-aware declutter -- choose label sides and
    // push overlapping element footprints (shape + label boxes) apart by the
    // minimal amount, on the exact geometry the quality metric scores. This is
    // where `label_overlap` (the dominant cost term) is driven down
    // deterministically.
    if config.declutter {
        declutter::declutter_view(&mut state.elements);
    }

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

fn calc_reciprocal_arc_angle(from_pos: Position, to_pos: Position) -> f64 {
    let base_angle = (to_pos.y - from_pos.y)
        .atan2(to_pos.x - from_pos.x)
        .to_degrees();
    normalize_angle(base_angle - 45.0)
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

fn rendered_dependency_ident(
    dep: &str,
    dependent: &str,
    all_idents: &HashSet<String>,
) -> Option<String> {
    let mut mapped = None;

    if let Some(root_ident) = dep.strip_prefix('·').or_else(|| dep.strip_prefix('.'))
        && all_idents.contains(root_ident)
    {
        mapped = Some(root_ident.to_string());
    }

    if mapped.is_none() && all_idents.contains(dep) {
        mapped = Some(dep.to_string());
    }

    if mapped.is_none()
        && let Some(prefix) = dep.split('·').next()
        && !prefix.is_empty()
        && prefix != dep
        && all_idents.contains(prefix)
    {
        mapped = Some(prefix.to_string());
    }

    mapped.filter(|ident| ident != dependent)
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
    let (source_model, detected) = {
        let canonical_name = crate::canonicalize(&actual_name_owned);
        let source_model = *source_project.models(db).get(canonical_name.as_ref())?;
        let detected = crate::db::model_detected_loops(db, source_model, source_project);
        (source_model, detected)
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

    // Capture the loop_partitions mapping AND per-loop slot counts while
    // LTM is still enabled so the cached `model_ltm_variables` query sees
    // the same flag value the VM ran under.  Per-element rel scores need
    // both the partition map (which loops normalize together) and the
    // per-loop slot count (how many elements each A2A loop occupies).
    let (loop_partitions, n_slots_by_loop) = if vm_result.is_some() {
        let ltm_vars = crate::db::model_ltm_variables(db, source_model, source_project);
        let dm_dims = crate::db::project_datamodel_dims(db, source_project);
        let dim_size: HashMap<&str, usize> = dm_dims.iter().map(|d| (d.name(), d.len())).collect();
        let prefix = "$\u{205A}ltm\u{205A}loop_score\u{205A}";
        let n_slots: HashMap<String, usize> = ltm_vars
            .vars
            .iter()
            .filter_map(|v| {
                let id = v.name.strip_prefix(prefix)?;
                let n = if v.dimensions.is_empty() {
                    1
                } else {
                    v.dimensions
                        .iter()
                        .map(|d| dim_size.get(d.as_str()).copied().unwrap_or(1))
                        .product()
                };
                Some((id.to_string(), n))
            })
            .collect();
        (ltm_vars.loop_partitions.clone(), n_slots)
    } else {
        (HashMap::new(), HashMap::new())
    };

    source_project.set_ltm_enabled(db).to(false);

    let vm = vm_result?;
    let results = vm.into_results();

    // `rel_loop_score` is no longer a VM variable; derive it post-sim from
    // the `loop_score` series the VM does emit, using the per-slot partition
    // mapping cached on `model_ltm_variables`.  See
    // `docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md`.
    //
    // For arrayed (A2A) loops we compute per-element rel scores then
    // aggregate to a single signed series via argmax-abs across slots --
    // i.e. each step's importance is the dominant element's contribution,
    // with sign preserved.  For scalar loops this reduces to identity.
    // The aggregation is delegated to `ltm_post::aggregate_per_element_argmax_abs`
    // so the partition-stride handling (mixed partitions where stride >
    // per-loop n_slots) is centralized and unit-testable.  See issue #463.
    // `compute_rel_loop_scores_per_element` derives each loop's slot count
    // from `loop_partitions[id].len()`, so no separate slot-count map is
    // threaded; `aggregate_per_element_argmax_abs` still takes one.
    let per_element_rel_scores =
        crate::ltm_post::compute_rel_loop_scores_per_element(&results, &loop_partitions);
    let importance_by_loop = crate::ltm_post::aggregate_per_element_argmax_abs(
        &per_element_rel_scores,
        &n_slots_by_loop,
        results.step_count,
    );

    // Phase 3: Build feedback loop structs from VM results.
    let mut feedback_loops = Vec::new();
    for dl in &detected.loops {
        // metadata::LoopPolarity only carries R/B/U: the layout legend does
        // not visually distinguish "mostly R" from "R" today, so the
        // mostly-* variants collapse onto their dominant equivalents here.
        // The polarity_confidence on `dl` is dropped at this boundary --
        // when the layout pipeline learns to surface confidence it should
        // pass `dl.polarity_confidence` through alongside the polarity.
        let polarity = match dl.polarity {
            crate::db::DetectedLoopPolarity::Reinforcing
            | crate::db::DetectedLoopPolarity::MostlyReinforcing => LoopPolarity::Reinforcing,
            crate::db::DetectedLoopPolarity::Balancing
            | crate::db::DetectedLoopPolarity::MostlyBalancing => LoopPolarity::Balancing,
            crate::db::DetectedLoopPolarity::Undetermined => LoopPolarity::Undetermined,
        };

        let variables: Vec<String> = {
            let mut vars = dl.variables.clone();
            if let Some(first) = vars.first().cloned() {
                vars.push(first);
            }
            vars
        };

        let importance_series = importance_by_loop.get(&dl.id).cloned().unwrap_or_default();

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
                if let Some(src_ident) =
                    rendered_dependency_ident(&src_ident, &var_ident, &all_idents)
                {
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
                // The old no-arg variant used an empty module-ident context and
                // the `None`-inputs path; reproduce that with the empty sets.
                let empty_ctx = crate::db::ModuleIdentContext::new(db, vec![]);
                let empty_inputs = crate::db::ModuleInputSet::empty(db);
                let var_deps = crate::db::variable_direct_dependencies(
                    db,
                    sv,
                    source_project,
                    empty_ctx,
                    empty_inputs,
                );
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
        // For dotted identifiers like "module·output", map back to the
        // module name when that prefix is in all_idents -- the module
        // is the rendered element, not the qualified output port.
        // Also exclude self-references: the string heuristic skips them,
        // but AST extraction doesn't (stocks reference themselves through
        // init expressions, SMOOTH/DELAY patterns, etc.).
        let deps: Vec<String> = deps
            .into_iter()
            .filter_map(|d| rendered_dependency_ident(&d, &var_ident, &all_idents))
            .collect::<BTreeSet<_>>()
            .into_iter()
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
        // The VM saves at most once per dt step
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

/// Whether `p` lies on the segment from flow point `a` to flow point `b`,
/// within a small pixel tolerance. Used to find the pipe segment a flow's valve
/// sits on so the valve can be injected as a shared `elem_{flow.uid}` vertex.
///
/// The perpendicular distance from `p` to the line must be tiny, and `p` must
/// project within the segment (parameter in `[0, 1]`). A degenerate segment
/// (`a == b`) only matches when `p` coincides with it.
fn point_on_segment(
    p: Position,
    a: &datamodel::view_element::FlowPoint,
    b: &datamodel::view_element::FlowPoint,
) -> bool {
    const TOL: f64 = 0.5; // pixels
    let a = Position::new(a.x, a.y);
    let b = Position::new(b.x, b.y);
    let ab = b - a;
    let ap = p - a;
    let len_sq = ab.dot(ab);
    if len_sq < f64::EPSILON {
        // Degenerate segment: only "on" it if p coincides with the point.
        return ap.dot(ap) < TOL * TOL;
    }
    // Project p onto the line; require it to fall within the segment.
    let t = ap.dot(ab) / len_sq;
    if !(0.0..=1.0).contains(&t) {
        return false;
    }
    // Perpendicular distance: |ap x ab| / |ab|.
    let perp = ap.cross_2d(ab).abs() / len_sq.sqrt();
    perp < TOL
}

/// Build the set of [`LineSegment`]s that crossing detection runs over for a
/// completed StockFlow view. This is the single source of geometry shared by
/// [`count_view_crossings`] and the layout quality metric, so a layout's
/// crossing score can never disagree with the geometry the renderer draws.
///
/// Connector geometry comes from [`crate::diagram::connector::connector_polyline`],
/// the exact polyline the SVG renderer draws: straight links are clipped to
/// element boundaries, arcs are sampled along their arc circle, and MultiPoint
/// links contribute nothing (the renderer draws nothing for them today).
///
/// Element endpoints are resolved over *all* element kinds, so a link incident
/// on a Module or Alias is no longer dropped (the previous chord-based code
/// only mapped Stock/Flow/Aux/Cloud, silently undercounting such crossings).
///
/// Node naming suppresses self- and shared-endpoint "crossings" exactly like
/// before: a connector's first vertex is `elem_{from_uid}` and its last is
/// `elem_{to_uid}` (so two connectors sharing an element endpoint never count),
/// while internal arc-sample vertices are `link_{link.uid}#{i}` (so the
/// consecutive segments of one arc share an internal node name and never count
/// as self-crossings).
///
/// A flow's pipe vertices share those same `elem_{uid}` names with whatever
/// element they connect to, so a link incident on the flow grazes but does not
/// "cross" the pipe at the shared connection point. A point attached to a
/// stock/cloud is named `elem_{attached_to_uid}` (matching a link whose
/// endpoint is that stock/cloud), and the flow's valve -- which sits on the
/// pipe, not necessarily at a stored point -- is injected as an extra vertex
/// named `elem_{flow.uid}` so a link incident on the valve (its `to_uid`/
/// `from_uid` is the flow's own element uid) is suppressed there too. A
/// genuinely free interior point (no attachment, not the valve) keeps the
/// historic per-flow `flow_{uid}#{i}` name, so a link that crosses the pipe
/// mid-span -- sharing no element with the flow -- is still counted.
fn build_view_segments(view: &datamodel::StockFlow) -> Vec<LineSegment> {
    if view.elements.is_empty() {
        return Vec::new();
    }

    // Resolve every element by uid so a link can find its endpoints regardless
    // of the endpoint's kind (Module/Alias included).
    let mut uid_elements: HashMap<i32, &ViewElement> = HashMap::new();
    for elem in &view.elements {
        uid_elements.insert(elem.get_uid(), elem);
    }

    // Crossing detection is center-based and deterministic; no element is
    // treated as arrayed (matching the historic behavior).
    let not_arrayed = |_: &str| false;

    let mut segments: Vec<LineSegment> = Vec::new();

    for elem in &view.elements {
        match elem {
            ViewElement::Link(link) => {
                let (Some(&from), Some(&to)) = (
                    uid_elements.get(&link.from_uid),
                    uid_elements.get(&link.to_uid),
                ) else {
                    continue; // an endpoint is genuinely missing
                };

                let polyline = crate::diagram::connector::connector_polyline(
                    link,
                    from,
                    to,
                    &not_arrayed,
                    crate::diagram::connector::ARC_POLYLINE_SAMPLES,
                );
                if polyline.len() < 2 {
                    continue; // MultiPoint / degenerate: nothing drawn
                }

                let last_idx = polyline.len() - 1;
                // Name the first vertex after the source element and the last
                // after the target element so two connectors sharing an element
                // endpoint are suppressed; name internal vertices per-link so a
                // connector never crosses itself.
                let vertex_name = |i: usize| -> String {
                    if i == 0 {
                        format!("elem_{}", link.from_uid)
                    } else if i == last_idx {
                        format!("elem_{}", link.to_uid)
                    } else {
                        format!("link_{}#{}", link.uid, i)
                    }
                };

                for i in 0..last_idx {
                    let a = polyline[i];
                    let b = polyline[i + 1];
                    segments.push(LineSegment {
                        start: Position::new(a.x, a.y),
                        end: Position::new(b.x, b.y),
                        from_node: vertex_name(i),
                        to_node: vertex_name(i + 1),
                    });
                }
            }
            ViewElement::Flow(flow) => {
                if flow.points.len() < 2 {
                    continue;
                }

                // Build the pipe as a sequence of named vertices. A point
                // attached to a stock/cloud shares that element's `elem_{uid}`
                // name; a free interior point keeps a per-flow `flow_{uid}#{i}`
                // name. The valve (the flow's own element, at `flow.x/flow.y`)
                // is injected as an `elem_{flow.uid}` vertex on the pipe segment
                // whose span contains it, so a link incident on the valve is
                // suppressed at that shared connection point. Consecutive
                // segments of one flow always share the joining vertex name, so
                // a flow never self-crosses.
                let point_name = |i: usize| -> String {
                    match flow.points[i].attached_to_uid {
                        Some(uid) => format!("elem_{uid}"),
                        None => format!("flow_{}#{}", flow.uid, i),
                    }
                };

                let valve = Position::new(flow.x, flow.y);
                let valve_name = format!("elem_{}", flow.uid);
                // The pipe segment the valve sits strictly interior to. `None`
                // when the valve coincides with a stored point or (in a
                // hand-edited view) drifted off the polyline; the pipe is then
                // not split and the existing point names hold.
                let valve_seg = (0..flow.points.len() - 1).find(|&i| {
                    let a = Position::new(flow.points[i].x, flow.points[i].y);
                    let b = Position::new(flow.points[i + 1].x, flow.points[i + 1].y);
                    valve != a
                        && valve != b
                        && point_on_segment(valve, &flow.points[i], &flow.points[i + 1])
                });

                for i in 0..flow.points.len() - 1 {
                    let a = Position::new(flow.points[i].x, flow.points[i].y);
                    let b = Position::new(flow.points[i + 1].x, flow.points[i + 1].y);
                    let a_name = point_name(i);
                    let b_name = point_name(i + 1);

                    if Some(i) == valve_seg {
                        // Split this pipe segment at the valve so both halves
                        // share the `elem_{flow.uid}` vertex.
                        segments.push(LineSegment {
                            start: a,
                            end: valve,
                            from_node: a_name,
                            to_node: valve_name.clone(),
                        });
                        segments.push(LineSegment {
                            start: valve,
                            end: b,
                            from_node: valve_name.clone(),
                            to_node: b_name,
                        });
                    } else {
                        segments.push(LineSegment {
                            start: a,
                            end: b,
                            from_node: a_name,
                            to_node: b_name,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    segments
}

/// Count edge crossings in a completed StockFlow view.
///
/// Crossings are counted on the connectors' sampled drawn polylines: straight
/// links clipped to element boundaries, arcs sampled along their arc circle,
/// and flow pipes as their point polylines. All element endpoints are resolved
/// (Module/Alias included), so the count reflects the geometry the renderer
/// actually draws rather than a straight chord approximation.
pub fn count_view_crossings(view: &datamodel::StockFlow) -> usize {
    annealing::count_crossings(&build_view_segments(view))
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
    let (bmin_x, bmin_y, bmax_x, bmax_y) = compute_bounds(&state.elements, config);
    let view_box = if !state.elements.is_empty() && bmin_x != f64::MAX {
        // Account for elements at negative coordinates (e.g. from imported
        // or hand-edited views preserved by incremental layout).
        let vb_x = (bmin_x - DIAGRAM_ORIGIN_MARGIN).min(0.0);
        let vb_y = (bmin_y - DIAGRAM_ORIGIN_MARGIN).min(0.0);
        Rect {
            x: vb_x,
            y: vb_y,
            width: bmax_x - vb_x + DIAGRAM_ORIGIN_MARGIN,
            height: bmax_y - vb_y + DIAGRAM_ORIGIN_MARGIN,
        }
    } else {
        Rect::default()
    };
    datamodel::StockFlow {
        name: template.name.clone(),
        elements: state.elements,
        view_box,
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

/// Seeds for parallel layout generation. Each seed produces a different SFDP
/// layout; the one with fewest connector crossings is selected.
///
/// These are also the layout-quality sweep's best-of-k production proxy: the
/// `layout_eval` example scores the best layout over exactly this seed set to
/// estimate what production (which picks best-of-`LAYOUT_SEEDS`) would ship,
/// so it is exposed publicly. The value and behavior are unchanged.
pub const LAYOUT_SEEDS: [u64; 4] = [42, 123, 456, 789];

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

    for stock in &affected_stocks {
        let sides = classify_flow_sides(stock, &metadata);
        incr_flow_attachments.extend(sides);
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

    if new_elements.is_empty() {
        // No new elements and no settlement step, so rebuilt flows
        // already have correct geometry from create_flow_view_element.
        // Skip resnap entirely to avoid rewriting unrelated manual or
        // imported flow endpoints elsewhere in the diagram.
        diff_connectors(&mut state, &metadata);
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

/// Generate multiple layouts with different seeds in parallel and pick the one
/// that minimizes the full calibrated layout-quality metric (`weighted_cost`,
/// which includes the accurate connector-crossing count alongside node/label
/// overlap and loop compactness). On tie, the lowest seed wins.
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
        // Score the candidate with the full calibrated metric. Its `crossings`
        // term computes the accurate connector-crossing count internally, so we
        // no longer call `count_view_crossings` directly here.
        let metrics = metrics::compute_layout_metrics(&view, &cfg);
        let weighted_cost = metrics.weighted_cost(&metrics::MetricWeights::default());
        Ok(LayoutResult {
            view,
            weighted_cost,
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

/// Pick the layout that minimizes the full calibrated layout-quality metric
/// (`weighted_cost`); on tie, the one from the lowest seed. NaN-cost candidates
/// (degenerate layouts) never win over a finite one regardless of position in
/// the result set; if ALL candidates are NaN the earliest is kept
/// deterministically. The first `Err` short-circuits, and an empty result set is
/// an error.
fn select_best_layout(
    results: Vec<Result<LayoutResult, String>>,
) -> Result<datamodel::StockFlow, String> {
    let mut best: Option<LayoutResult> = None;

    for result in results {
        let lr = result?;
        best = Some(match best {
            None => lr,
            Some(prev) => {
                // NaN-safe and order-independent: a degenerate NaN-cost
                // candidate never wins over a finite one regardless of which
                // came first. A plain `<` already drops a NaN *challenger*
                // (`NaN < finite` is false), but it would NOT let a finite
                // challenger overtake a NaN *running best* (`finite < NaN` and
                // `finite == NaN` are both false), so the first seed's NaN would
                // be sticky. The explicit NaN branches fix that asymmetry. If
                // ALL candidates are NaN the challenger is never better, so the
                // earliest is kept -- deterministic regardless.
                let better = if lr.weighted_cost.is_nan() {
                    false // a NaN challenger never wins
                } else if prev.weighted_cost.is_nan() {
                    true // a finite challenger always beats a NaN running best
                } else {
                    lr.weighted_cost < prev.weighted_cost
                        || (lr.weighted_cost == prev.weighted_cost && lr.seed < prev.seed)
                };
                if better { lr } else { prev }
            }
        });
    }

    best.map(|lr| lr.view)
        .ok_or_else(|| "no layout results".to_string())
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "crossings_tests.rs"]
mod crossings_tests;

#[cfg(test)]
#[path = "layout_selection_tests.rs"]
mod layout_selection_tests;

#[cfg(test)]
#[path = "layout_isolated_tests.rs"]
mod layout_isolated_tests;

#[cfg(test)]
#[path = "layout_branching_tests.rs"]
mod layout_branching_tests;
