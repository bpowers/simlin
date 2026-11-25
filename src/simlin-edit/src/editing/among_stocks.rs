// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The core's routing with the stocks given by their centers: what incremental
//! layout draws a re-attached flow with. Not generic over the obstacles, as the
//! core is, so the routing is compiled in this crate and not again in the
//! caller's.

use crate::editing::geometry::{FlowEnd, Point};
use crate::editing::terminal::{FlowGeometry, Terminal, Terminals};
use simlin_base::datamodel::view_element::Flow;

/// [`super::heal::heal`] among `stocks`.
pub fn heal(flow: &Flow, terminals: &Terminals, stocks: &[Point]) -> FlowGeometry {
    super::heal::heal(flow, terminals, stocks)
}

/// [`super::route::route`] among `stocks`.
pub fn route(
    source: Terminal,
    sink: Terminal,
    base: &Flow,
    valve_from: FlowEnd,
    occupied: &[Point],
    stocks: &[Point],
) -> FlowGeometry {
    super::route::route(source, sink, base, valve_from, occupied, stocks)
}

/// [`super::route::route_end`] among `stocks`.
pub fn route_end(
    flow: &Flow,
    end: FlowEnd,
    terminal: Terminal,
    fixed: Terminal,
    occupied: &[Point],
    stocks: &[Point],
) -> FlowGeometry {
    super::route::route_end(flow, end, terminal, fixed, occupied, stocks)
}
