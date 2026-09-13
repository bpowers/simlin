// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The diagram editing core: flow geometry, hit testing, gesture planning, and
//! the model ops a view edit implies.
//!
//! The invariants every committed edit holds are G1-G8, M1-M3 and E1-E6 of
//! `docs/design-plans/2026-09-10-diagram-editing-core.md`; how this module
//! serves native hosts and tool edits is
//! `docs/design-plans/2026-09-12-editing-core-in-rust.md`.
//!
//! The per-frame path allocates in proportion to what a gesture changes, not to
//! the size of the view: a gesture session indexes the view once when it
//! begins, paths live inline (`geometry::Path`), and routing queries obstacles
//! by region.

mod base;
mod edit_view;
mod geometry;
mod gesture;
mod heal;
mod hit;
#[cfg(any(test, feature = "test-support", feature = "layout_eval"))]
pub mod invariants;
mod links;
mod offset;
mod path;
mod preview;
mod route;
#[cfg(test)]
mod scene_gen;
#[cfg(test)]
#[path = "scene_sweep_tests.rs"]
mod scene_sweep_tests;
mod terminal;
#[cfg(test)]
mod test_support;
mod validity;

pub use base::{BaseView, VariableKind};
pub use edit_view::{plan_delete, plan_rename};
pub use geometry::{
    CORNER_CLEARANCE, Face, FlowEnd, GEOMETRY_EPSILON, MIN_SEGMENT, MIN_SINK_SEGMENT, PIPE_SPACING,
    Point, VALVE_CLAMP_MARGIN,
};
pub use gesture::{
    CommitKind, DanglingLink, GestureKind, GestureSession, Plan, PointerKind, Press, Target, Tool,
    ViewEdit, begin_drag, plan_tap,
};
pub use hit::{Hit, HitPart, hit_test};
pub use preview::{Preview, preview};

pub(crate) use edit_view::{derived_operations, edited_view};
// The flow geometry incremental layout routes through when an edit re-attaches
// a drawn flow, so a tool edit's pipes are drawn by the same core as a touch
// edit's.
pub(crate) use heal::heal;
pub(crate) use path::place_valve;
pub(crate) use route::{route, route_end};
pub(crate) use terminal::{
    CloudRef, FlowGeometry, flow_terminals, free_terminal, target_stock_terminal,
};
