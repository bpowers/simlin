// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Resolution-independent path geometry for the scene display list.
//!
//! A scene path is a flat number array: an opcode followed by its operands
//! (`docs/design/diagram-scene.md`, "Shapes"). Consumers only ever see moves,
//! lines, cubic Béziers and closes -- every SVG elliptical arc the renderer
//! draws is converted to cubics here, and every SVG transform is applied to the
//! points before they leave the engine -- so no consumer implements arcs or
//! transforms and none can drift from the SVG those same numbers produce.

use std::f64::consts::PI;

use crate::diagram::common::{Point, Rect, deg_to_rad};

/// Move to `x, y`.
pub(crate) const OP_MOVE: f64 = 0.0;
/// Line to `x, y`.
pub(crate) const OP_LINE: f64 = 1.0;
/// Cubic Bézier to `c1x, c1y, c2x, c2y, x, y`.
pub(crate) const OP_CUBIC: f64 = 2.0;
/// Close the current subpath.
pub(crate) const OP_CLOSE: f64 = 3.0;

/// The widest angle one cubic Bézier spans when an arc is approximated.
///
/// A cubic whose control points sit `4/3 * tan(theta / 4)` along the tangents
/// deviates radially from its circle by about 2.7e-4 of the radius at 90
/// degrees, and the error falls with the sixth power of the angle. 30 degrees
/// keeps it under 1e-6 of the radius -- invisible at any zoom a diagram is
/// drawn at -- while staying inside the contract's "at most 90 degrees per
/// cubic".
const MAX_ARC_SEGMENT_RADIANS: f64 = PI / 6.0;

/// Builds a flat scene path.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Default, PartialEq)]
pub(crate) struct PathBuilder {
    d: Vec<f64>,
    current: Option<Point>,
    subpath_start: Option<Point>,
}

impl PathBuilder {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn move_to(&mut self, p: Point) {
        self.d.extend_from_slice(&[OP_MOVE, p.x, p.y]);
        self.current = Some(p);
        self.subpath_start = Some(p);
    }

    pub(crate) fn line_to(&mut self, p: Point) {
        self.d.extend_from_slice(&[OP_LINE, p.x, p.y]);
        self.current = Some(p);
    }

    pub(crate) fn cubic_to(&mut self, c1: Point, c2: Point, p: Point) {
        self.d
            .extend_from_slice(&[OP_CUBIC, c1.x, c1.y, c2.x, c2.y, p.x, p.y]);
        self.current = Some(p);
    }

    pub(crate) fn close(&mut self) {
        self.d.push(OP_CLOSE);
        self.current = self.subpath_start;
    }

    /// Appends the SVG path command `A rx ry x-axis-rotation large-arc-flag
    /// sweep-flag x y` from the current point, as cubic Béziers of at most
    /// [`MAX_ARC_SEGMENT_RADIANS`] each.
    ///
    /// This is the endpoint-to-center conversion of the SVG 1.1 implementation
    /// notes (F.6.5), with the out-of-range parameter handling of F.6.6: an arc
    /// whose endpoints coincide draws nothing, a zero radius draws a straight
    /// line, and radii too small to span the chord are scaled up until they do.
    /// The last cubic ends exactly on `end`, so a following command starts
    /// where the SVG's would.
    pub(crate) fn svg_arc_to(
        &mut self,
        rx: f64,
        ry: f64,
        x_axis_rotation_deg: f64,
        large_arc: bool,
        sweep: bool,
        end: Point,
    ) {
        let Some(start) = self.current else {
            // SVG requires a current point before an arc; with none there is
            // nothing to draw from, so the arc only establishes its endpoint.
            self.move_to(end);
            return;
        };
        if start == end {
            return;
        }
        let (mut rx, mut ry) = (rx.abs(), ry.abs());
        if rx == 0.0 || ry == 0.0 {
            self.line_to(end);
            return;
        }

        let phi = deg_to_rad(x_axis_rotation_deg % 360.0);
        let (sin_phi, cos_phi) = (phi.sin(), phi.cos());

        // F.6.5 step 1: the midpoint-relative start point in the ellipse's
        // own axes.
        let dx2 = (start.x - end.x) / 2.0;
        let dy2 = (start.y - end.y) / 2.0;
        let x1p = cos_phi * dx2 + sin_phi * dy2;
        let y1p = -sin_phi * dx2 + cos_phi * dy2;

        // F.6.6 step 3: radii that cannot reach both endpoints are scaled up.
        let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
        if lambda > 1.0 {
            let scale = lambda.sqrt();
            rx *= scale;
            ry *= scale;
        }

        // F.6.5 step 2: the center in the ellipse's axes. The radicand is
        // clamped at zero because a scaled-up radius makes it zero only up to
        // rounding.
        let rx2 = rx * rx;
        let ry2 = ry * ry;
        let numerator = rx2 * ry2 - rx2 * y1p * y1p - ry2 * x1p * x1p;
        let denominator = rx2 * y1p * y1p + ry2 * x1p * x1p;
        let mut coefficient = (numerator / denominator).max(0.0).sqrt();
        if large_arc == sweep {
            coefficient = -coefficient;
        }
        let cxp = coefficient * rx * y1p / ry;
        let cyp = -coefficient * ry * x1p / rx;

        // F.6.5 step 3: the center in canvas coordinates.
        let cx = cos_phi * cxp - sin_phi * cyp + (start.x + end.x) / 2.0;
        let cy = sin_phi * cxp + cos_phi * cyp + (start.y + end.y) / 2.0;

        // F.6.5 step 4: the start angle and the signed sweep.
        let u = ((x1p - cxp) / rx, (y1p - cyp) / ry);
        let v = ((-x1p - cxp) / rx, (-y1p - cyp) / ry);
        let theta1 = signed_angle((1.0, 0.0), u);
        let mut delta = signed_angle(u, v);
        if !sweep && delta > 0.0 {
            delta -= 2.0 * PI;
        } else if sweep && delta < 0.0 {
            delta += 2.0 * PI;
        }

        // A small tolerance keeps an exact multiple of the segment angle (a
        // quarter circle) from rounding up into one extra, needless cubic.
        let segments = ((delta.abs() / MAX_ARC_SEGMENT_RADIANS) - 1e-9)
            .ceil()
            .max(1.0) as usize;
        let ellipse_point = |t: f64| -> Point {
            let (u, v) = (t.cos(), t.sin());
            Point {
                x: cx + rx * cos_phi * u - ry * sin_phi * v,
                y: cy + rx * sin_phi * u + ry * cos_phi * v,
            }
        };
        // The tangent direction at angle `t`, scaled by the ellipse's radii
        // and rotated into canvas coordinates.
        let ellipse_tangent = |t: f64| -> Point {
            let (u, v) = (-t.sin(), t.cos());
            Point {
                x: rx * cos_phi * u - ry * sin_phi * v,
                y: rx * sin_phi * u + ry * cos_phi * v,
            }
        };

        for i in 0..segments {
            let t0 = theta1 + delta * (i as f64) / (segments as f64);
            let t1 = theta1 + delta * ((i + 1) as f64) / (segments as f64);
            let k = 4.0 / 3.0 * ((t1 - t0) / 4.0).tan();
            let p0 = ellipse_point(t0);
            let tangent0 = ellipse_tangent(t0);
            let tangent1 = ellipse_tangent(t1);
            let p3 = if i + 1 == segments {
                end
            } else {
                ellipse_point(t1)
            };
            let p3_on_curve = ellipse_point(t1);
            let c1 = Point {
                x: p0.x + k * tangent0.x,
                y: p0.y + k * tangent0.y,
            };
            let c2 = Point {
                x: p3_on_curve.x - k * tangent1.x,
                y: p3_on_curve.y - k * tangent1.y,
            };
            self.cubic_to(c1, c2, p3);
        }
    }

    /// Replaces every point operand with `f(point)`. An affine `f` maps each
    /// Bézier onto the transformed curve exactly, which is how SVG transforms
    /// (rotations, the cloud's matrix) are applied without a consumer seeing
    /// them.
    pub(crate) fn map_points(&mut self, f: impl Fn(Point) -> Point) {
        let mut i = 0;
        while i < self.d.len() {
            let operands = operand_count(self.d[i]);
            let mut j = i + 1;
            while j < i + 1 + operands {
                let p = f(Point {
                    x: self.d[j],
                    y: self.d[j + 1],
                });
                self.d[j] = p.x;
                self.d[j + 1] = p.y;
                j += 2;
            }
            i += 1 + operands;
        }
        self.current = self.current.map(&f);
        self.subpath_start = self.subpath_start.map(&f);
    }

    pub(crate) fn into_d(self) -> Vec<f64> {
        self.d
    }
}

/// The number of operands (two per point) that follow an opcode.
fn operand_count(op: f64) -> usize {
    if op == OP_MOVE || op == OP_LINE {
        2
    } else if op == OP_CUBIC {
        6
    } else if op == OP_CLOSE {
        0
    } else {
        unreachable!("scene paths are only built from the four opcodes, found {op}")
    }
}

/// The angle from `u` to `v`, in `(-pi, pi]`, positive in the direction SVG's
/// sweep-flag 1 draws (clockwise on a y-down canvas).
fn signed_angle(u: (f64, f64), v: (f64, f64)) -> f64 {
    let cross = u.0 * v.1 - u.1 * v.0;
    let dot = u.0 * v.0 + u.1 * v.1;
    cross.atan2(dot)
}

/// The bounding box of a path's points, control points included. A Bézier
/// lies inside the convex hull of its control points, so this box contains
/// the drawn curve; it is conservative, not tight. `None` for an empty path.
pub(crate) fn control_point_bounds(d: &[f64]) -> Option<Rect> {
    let mut bounds: Option<Rect> = None;
    let mut i = 0;
    while i < d.len() {
        let operands = operand_count(d[i]);
        let mut j = i + 1;
        while j < i + 1 + operands {
            let (x, y) = (d[j], d[j + 1]);
            bounds = Some(match bounds {
                None => Rect {
                    top: y,
                    left: x,
                    right: x,
                    bottom: y,
                },
                Some(b) => Rect {
                    top: b.top.min(y),
                    left: b.left.min(x),
                    right: b.right.max(x),
                    bottom: b.bottom.max(y),
                },
            });
            j += 2;
        }
        i += 1 + operands;
    }
    bounds
}

/// Parses an absolute SVG path made of `M`, `L`, `C` and `Z` commands.
///
/// The one path the engine stores as SVG text is the cloud outline (both
/// renderers read `elements::CLOUD_PATH`), and it uses only these commands;
/// anything else is refused rather than guessed at, so a future constant with
/// relative commands fails loudly in the cloud's tests.
pub(crate) fn parse_absolute_svg_path(text: &str) -> Result<PathBuilder, String> {
    let mut path = PathBuilder::new();
    let mut tokens = text
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|t| !t.is_empty())
        .peekable();
    let number = next_path_number;
    let mut command: Option<char> = None;
    while let Some(token) = tokens.peek().copied() {
        let mut chars = token.chars();
        if let (Some(c), None) = (chars.next(), chars.next())
            && c.is_ascii_alphabetic()
        {
            tokens.next();
            command = Some(c);
            if c == 'Z' || c == 'z' {
                path.close();
                command = None;
            }
            continue;
        }
        match command {
            Some('M') => {
                let x = number(&mut tokens)?;
                let y = number(&mut tokens)?;
                path.move_to(Point { x, y });
                // Coordinates after an M's first pair are implicit L's.
                command = Some('L');
            }
            Some('L') => {
                let x = number(&mut tokens)?;
                let y = number(&mut tokens)?;
                path.line_to(Point { x, y });
            }
            Some('C') => {
                let c1 = Point {
                    x: number(&mut tokens)?,
                    y: number(&mut tokens)?,
                };
                let c2 = Point {
                    x: number(&mut tokens)?,
                    y: number(&mut tokens)?,
                };
                let p = Point {
                    x: number(&mut tokens)?,
                    y: number(&mut tokens)?,
                };
                path.cubic_to(c1, c2, p);
            }
            Some(other) => return Err(format!("unsupported path command '{other}'")),
            None => return Err(format!("path number '{token}' has no command")),
        }
    }
    Ok(path)
}

/// The next token of a path as a number.
fn next_path_number<'a>(tokens: &mut impl Iterator<Item = &'a str>) -> Result<f64, String> {
    let token = tokens
        .next()
        .ok_or_else(|| "path ended where a number was expected".to_string())?;
    token
        .parse::<f64>()
        .map_err(|e| format!("bad path number '{token}': {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Splits a flat path back into (opcode, points) rows for assertions.
    fn rows(d: &[f64]) -> Vec<(f64, Vec<Point>)> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < d.len() {
            let op = d[i];
            let n = operand_count(op);
            let points = (0..n / 2)
                .map(|k| Point {
                    x: d[i + 1 + 2 * k],
                    y: d[i + 2 + 2 * k],
                })
                .collect();
            out.push((op, points));
            i += 1 + n;
        }
        out
    }

    fn cubic_at(p0: Point, c1: Point, c2: Point, p3: Point, t: f64) -> Point {
        let mt = 1.0 - t;
        let a = mt * mt * mt;
        let b = 3.0 * mt * mt * t;
        let c = 3.0 * mt * t * t;
        let d = t * t * t;
        Point {
            x: a * p0.x + b * c1.x + c * c2.x + d * p3.x,
            y: a * p0.y + b * c1.y + c * c2.y + d * p3.y,
        }
    }

    /// Every cubic of `d` sampled at eleven parameters.
    fn sampled_points(d: &[f64]) -> Vec<Point> {
        let mut current: Option<Point> = None;
        let mut samples = Vec::new();
        for (op, points) in rows(d) {
            if op == OP_CUBIC {
                let p0 = current.expect("a cubic needs a current point");
                samples.extend(
                    (0..=10)
                        .map(|s| cubic_at(p0, points[0], points[1], points[2], s as f64 / 10.0)),
                );
            }
            if let Some(last) = points.last() {
                current = Some(*last);
            }
        }
        samples
    }

    /// Asserts that every cubic of `d` lies on the circle `(cx, cy, r)` to
    /// within 1e-6 of the radius, sampled at eleven parameters per cubic, and
    /// that no cubic spans more than 90 degrees. Returns the cubics' endpoints
    /// in order, the first being the arc's start.
    fn assert_cubics_on_circle(d: &[f64], cx: f64, cy: f64, r: f64) -> Vec<Point> {
        let mut current: Option<Point> = None;
        let mut endpoints = Vec::new();
        for (op, points) in rows(d) {
            if op == OP_MOVE {
                current = Some(points[0]);
                endpoints.push(points[0]);
                continue;
            }
            assert_eq!(op, OP_CUBIC, "an arc becomes cubics only");
            let p0 = current.expect("a cubic needs a current point");
            let (c1, c2, p3) = (points[0], points[1], points[2]);
            for s in 0..=10 {
                let p = cubic_at(p0, c1, c2, p3, s as f64 / 10.0);
                let dist = ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt();
                assert!(
                    (dist - r).abs() <= 1e-6 * r,
                    "sample at t={} is {dist} from the center, radius {r}",
                    s as f64 / 10.0
                );
            }
            let a0 = (p0.y - cy).atan2(p0.x - cx);
            let a3 = (p3.y - cy).atan2(p3.x - cx);
            let mut span = (a3 - a0).abs();
            if span > PI {
                span = 2.0 * PI - span;
            }
            assert!(span <= PI / 2.0 + 1e-9, "a cubic spans {span} radians");
            current = Some(p3);
            endpoints.push(p3);
        }
        endpoints
    }

    /// The signed angular extent swept from `start` through `endpoints` around
    /// the center, summing each cubic's short-way step.
    fn swept_angle(endpoints: &[Point], cx: f64, cy: f64) -> f64 {
        let mut total = 0.0;
        for pair in endpoints.windows(2) {
            let a0 = (pair[0].y - cy).atan2(pair[0].x - cx);
            let a1 = (pair[1].y - cy).atan2(pair[1].x - cx);
            let mut step = a1 - a0;
            while step > PI {
                step -= 2.0 * PI;
            }
            while step <= -PI {
                step += 2.0 * PI;
            }
            total += step;
        }
        total
    }

    fn arc(start: Point, rx: f64, large: bool, sweep: bool, end: Point) -> Vec<f64> {
        let mut path = PathBuilder::new();
        path.move_to(start);
        path.svg_arc_to(rx, rx, 0.0, large, sweep, end);
        path.into_d()
    }

    #[test]
    fn a_quarter_circle_in_each_flag_combination() {
        // From (10, 0) to (0, 10) on a radius-10 circle. The two candidate
        // centers are (0, 0) and (10, 10); the flags pick one center and one
        // of the two arcs around it. Rows derive from the four flag pairs.
        let start = Point { x: 10.0, y: 0.0 };
        let end = Point { x: 0.0, y: 10.0 };
        // (large, sweep) -> (center, signed sweep in radians)
        let rows = [
            (false, true, (0.0, 0.0), PI / 2.0),
            (false, false, (10.0, 10.0), -PI / 2.0),
            (true, true, (10.0, 10.0), 3.0 * PI / 2.0),
            (true, false, (0.0, 0.0), -3.0 * PI / 2.0),
        ];
        for (large, sweep, (cx, cy), expected_sweep) in rows {
            let d = arc(start, 10.0, large, sweep, end);
            let endpoints = assert_cubics_on_circle(&d, cx, cy, 10.0);
            assert_eq!(endpoints.first(), Some(&start));
            assert_eq!(
                endpoints.last(),
                Some(&end),
                "the arc ends exactly on its endpoint"
            );
            let swept = swept_angle(&endpoints, cx, cy);
            assert!(
                (swept - expected_sweep).abs() < 1e-9,
                "large={large} sweep={sweep}: swept {swept}, expected {expected_sweep}"
            );
        }
    }

    #[test]
    fn a_half_circle_in_both_directions() {
        // Endpoints on a diameter: the center is the midpoint whatever the
        // large-arc flag says, and the sweep flag picks the side.
        let start = Point { x: -5.0, y: 0.0 };
        let end = Point { x: 5.0, y: 0.0 };
        for (sweep, expected) in [(true, PI), (false, -PI)] {
            let d = arc(start, 5.0, false, sweep, end);
            let endpoints = assert_cubics_on_circle(&d, 0.0, 0.0, 5.0);
            assert_eq!(endpoints.last(), Some(&end));
            let swept = swept_angle(&endpoints, 0.0, 0.0);
            assert!(
                (swept - expected).abs() < 1e-9,
                "sweep={sweep}: {swept} vs {expected}"
            );
            // Sweep 1 runs toward increasing angle (F.6.5: `y = cy + ry sin
            // theta`), clockwise on a y-down canvas, so from the left end it
            // passes over the top.
            let mid = endpoints[endpoints.len() / 2];
            if sweep {
                assert!(mid.y < 0.0, "sweep 1 passes above the chord: {}", mid.y);
            } else {
                assert!(mid.y > 0.0, "sweep 0 passes below the chord: {}", mid.y);
            }
        }
    }

    #[test]
    fn a_radius_too_small_for_its_chord_is_scaled_up() {
        // Radius 1 cannot span a chord of 10: F.6.6 scales it to 5, the
        // half-circle through the midpoint.
        let d = arc(
            Point { x: 0.0, y: 0.0 },
            1.0,
            false,
            true,
            Point { x: 10.0, y: 0.0 },
        );
        let endpoints = assert_cubics_on_circle(&d, 5.0, 0.0, 5.0);
        assert_eq!(endpoints.last(), Some(&Point { x: 10.0, y: 0.0 }));
    }

    #[test]
    fn a_zero_radius_is_a_line_and_coincident_endpoints_draw_nothing() {
        let mut line = PathBuilder::new();
        line.move_to(Point { x: 0.0, y: 0.0 });
        line.svg_arc_to(0.0, 7.0, 0.0, false, true, Point { x: 3.0, y: 4.0 });
        assert_eq!(line.into_d(), vec![OP_MOVE, 0.0, 0.0, OP_LINE, 3.0, 4.0]);

        let mut nothing = PathBuilder::new();
        nothing.move_to(Point { x: 2.0, y: 2.0 });
        nothing.svg_arc_to(5.0, 5.0, 0.0, true, true, Point { x: 2.0, y: 2.0 });
        assert_eq!(nothing.into_d(), vec![OP_MOVE, 2.0, 2.0]);
    }

    #[test]
    fn an_arc_without_a_current_point_only_moves_to_its_endpoint() {
        let mut path = PathBuilder::new();
        path.svg_arc_to(5.0, 5.0, 0.0, false, true, Point { x: 1.0, y: 2.0 });
        assert_eq!(path.into_d(), vec![OP_MOVE, 1.0, 2.0]);
    }

    #[test]
    fn the_arrowhead_back_arc() {
        // The connector arrowhead's back edge from the byte-identical SVG
        // guard: `L189.374164,195.284057 A18,18 0 0,1 189.374164,189.284057`
        // (size 6, radius 3 * 6). The chord is 6, so the center sits
        // sqrt(18^2 - 3^2) from the chord's midpoint.
        let start = Point {
            x: 189.374164,
            y: 195.284057,
        };
        let end = Point {
            x: 189.374164,
            y: 189.284057,
        };
        let d = arc(start, 18.0, false, true, end);
        let offset = (18.0_f64 * 18.0 - 3.0 * 3.0).sqrt();
        // Sweep 1 turns clockwise on a y-down canvas: from the lower endpoint
        // up to the upper one around a center on the +x side, the tip's side.
        let (cx, cy) = (start.x + offset, (start.y + end.y) / 2.0);
        let endpoints = assert_cubics_on_circle(&d, cx, cy, 18.0);
        assert_eq!(endpoints.last(), Some(&end));
        // So the edge bows toward -x, away from the tip, by the arc's sagitta.
        let sagitta = 18.0 - offset;
        let leftmost = sampled_points(&d)
            .into_iter()
            .map(|p| p.x)
            .fold(f64::INFINITY, f64::min);
        assert!(
            (leftmost - (start.x - sagitta)).abs() < 1e-3,
            "the back edge bows {} left of its chord; the sagitta is {sagitta}",
            start.x - leftmost
        );
    }

    #[test]
    fn the_connector_arc_from_the_byte_identical_guard() {
        // `M100,100A273.205081,273.205081 0 0,1 200,200`.
        let start = Point { x: 100.0, y: 100.0 };
        let end = Point { x: 200.0, y: 200.0 };
        let r = 273.205081;
        let d = arc(start, r, false, true, end);
        // Recover the center exactly as F.6.5 defines it and check the samples
        // against it.
        let (mx, my) = (150.0, 150.0);
        let half_chord = (50.0_f64 * 50.0 * 2.0).sqrt();
        let dist = (r * r - half_chord * half_chord).sqrt();
        // Perpendicular to the chord direction (1, 1)/sqrt(2). F.6.5's sign is
        // positive when the large-arc and sweep flags differ, which puts the
        // center on the (-1, 1) side: the circle `connector::arc_circle`
        // builds for this link, centered near (-36.6, 336.6).
        let (cx, cy) = (mx - dist / 2.0_f64.sqrt(), my + dist / 2.0_f64.sqrt());
        let endpoints = assert_cubics_on_circle(&d, cx, cy, r);
        assert_eq!(endpoints.first(), Some(&start));
        assert_eq!(endpoints.last(), Some(&end));
    }

    #[test]
    fn map_points_transforms_every_operand_and_keeps_opcodes() {
        let mut path = PathBuilder::new();
        path.move_to(Point { x: 1.0, y: 2.0 });
        path.line_to(Point { x: 3.0, y: 4.0 });
        path.cubic_to(
            Point { x: 5.0, y: 6.0 },
            Point { x: 7.0, y: 8.0 },
            Point { x: 9.0, y: 10.0 },
        );
        path.close();
        path.map_points(|p| Point {
            x: p.x * 2.0,
            y: p.y + 1.0,
        });
        assert_eq!(
            path.into_d(),
            vec![
                OP_MOVE, 2.0, 3.0, OP_LINE, 6.0, 5.0, OP_CUBIC, 10.0, 7.0, 14.0, 9.0, 18.0, 11.0,
                OP_CLOSE
            ]
        );
    }

    #[test]
    fn control_point_bounds_cover_every_point() {
        let d = vec![
            OP_MOVE, 0.0, 5.0, OP_CUBIC, -3.0, 1.0, 8.0, 12.0, 4.0, 2.0, OP_CLOSE,
        ];
        let b = control_point_bounds(&d).unwrap();
        assert_eq!((b.left, b.top, b.right, b.bottom), (-3.0, 1.0, 8.0, 12.0));
        assert!(control_point_bounds(&[]).is_none());
    }

    #[test]
    fn parse_absolute_svg_path_reads_each_command_and_refuses_others() {
        let path = parse_absolute_svg_path("M 1,2 L 3 4 C 5,6 7,8 9,10 z").unwrap();
        assert_eq!(
            path.into_d(),
            vec![
                OP_MOVE, 1.0, 2.0, OP_LINE, 3.0, 4.0, OP_CUBIC, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0,
                OP_CLOSE
            ]
        );
        // Implicit L after the first M pair.
        let implicit = parse_absolute_svg_path("M 0,0 5,5").unwrap();
        assert_eq!(
            implicit.into_d(),
            vec![OP_MOVE, 0.0, 0.0, OP_LINE, 5.0, 5.0]
        );

        assert!(parse_absolute_svg_path("m 1,2").is_err());
        assert!(parse_absolute_svg_path("M 1").is_err());
        assert!(parse_absolute_svg_path("3,4").is_err());
    }
}
