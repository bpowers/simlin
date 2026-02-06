// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

pub const AUX_RADIUS: f64 = 9.0;
pub const STOCK_WIDTH: f64 = 45.0;
pub const STOCK_HEIGHT: f64 = 35.0;
pub const MODULE_WIDTH: f64 = 55.0;
pub const MODULE_HEIGHT: f64 = 45.0;
pub const MODULE_RADIUS: f64 = 5.0;
pub const ARROWHEAD_RADIUS: f64 = 6.0;
pub const FLOW_ARROWHEAD_RADIUS: f64 = 8.0;
pub const CLOUD_RADIUS: f64 = 13.5; // 1.5 * AUX_RADIUS
pub const CLOUD_WIDTH: f64 = 55.0;
pub const STRAIGHT_LINE_MAX: f64 = 6.0; // degrees
pub const LINE_SPACING: f64 = 14.0;
pub const LABEL_PADDING: f64 = 4.0;
pub const GROUP_RADIUS: f64 = 8.0;
pub const GROUP_LABEL_PADDING: f64 = 8.0;
pub const ARRAYED_OFFSET: f64 = 3.0;
pub const FLOW_VALVE_RADIUS: f64 = 6.0; // FlowWidth/2, used in flowBounds

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloud_radius_matches_aux() {
        assert!((CLOUD_RADIUS - 1.5 * AUX_RADIUS).abs() < f64::EPSILON);
    }
}
