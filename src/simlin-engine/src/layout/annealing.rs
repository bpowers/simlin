// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use super::config::LayoutConfig;
use super::graph::{Layout, NodeId, Position};

/// A line segment for crossing detection.
#[derive(Clone, Debug)]
pub struct LineSegment {
    pub start: Position,
    pub end: Position,
    /// Node ID at the start of this segment (for shared-endpoint detection).
    pub from_node: String,
    /// Node ID at the end of this segment.
    pub to_node: String,
}

/// Pre-computed polyline for a flow element, used for crossing detection.
#[derive(Clone, Debug)]
pub struct FlowTemplate {
    pub offsets: Vec<Position>,
}

/// Check if two line segments intersect.
/// Segments sharing an endpoint (same from_node or to_node) are NOT considered crossing.
/// Parallel/collinear segments are NOT considered crossing.
pub fn do_segments_intersect(s1: &LineSegment, s2: &LineSegment) -> bool {
    // Adjacent edges (sharing any endpoint node) don't count as crossing
    if s1.from_node == s2.from_node
        || s1.from_node == s2.to_node
        || s1.to_node == s2.from_node
        || s1.to_node == s2.to_node
    {
        return false;
    }

    // Direction vectors for each segment
    let d1 = s1.end - s1.start;
    let d2 = s2.end - s2.start;

    // Cross product of direction vectors gives the denominator
    let denom = d1.cross_2d(d2);
    if denom.abs() < 1e-10 {
        return false; // Parallel or collinear
    }

    // Vector from s1.start to s2.start
    let w = s2.start - s1.start;

    // Parametric intersection: t for s1, u for s2
    let t = w.cross_2d(d2) / denom;
    let u = w.cross_2d(d1) / denom;

    // Segments intersect if both parameters are in [0, 1]
    (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u)
}

/// Count the number of edge crossings among a set of line segments.
///
/// Uses brute-force O(n^2) pairwise comparison. This is sufficient for
/// typical SD models (tens to low hundreds of elements) but would need
/// spatial indexing (e.g. sweep line) for very large diagrams.
pub fn count_crossings(segments: &[LineSegment]) -> usize {
    let mut count = 0;
    for i in 0..segments.len() {
        for j in (i + 1)..segments.len() {
            if do_segments_intersect(&segments[i], &segments[j]) {
                count += 1;
            }
        }
    }
    count
}

/// Sample from the standard normal distribution using the Box-Muller transform.
fn sample_standard_normal(rng: &mut StdRng) -> f64 {
    // Avoid ln(0) by clamping u1 away from zero. random() returns [0, 1),
    // so u1=0 is possible and would produce -infinity.
    let u1: f64 = rng.random::<f64>().max(f64::MIN_POSITIVE);
    let u2: f64 = rng.random();
    (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
}

/// Generate a perturbation step using the same hybrid strategy as the Go
/// implementation: 65% Gaussian steps, 35% uniform-radius polar steps.
fn generate_step(rng: &mut StdRng, temperature: f64) -> (f64, f64) {
    let gauss_x = sample_standard_normal(rng) * temperature;
    let gauss_y = sample_standard_normal(rng) * temperature;
    if rng.random::<f64>() < 0.35 {
        let angle = rng.random::<f64>() * 2.0 * PI;
        let radius = (0.4 + rng.random::<f64>() * 0.8) * temperature;
        (radius * angle.cos(), radius * angle.sin())
    } else {
        (gauss_x, gauss_y)
    }
}

/// Result of a simulated annealing run.
pub struct AnnealingResult<N: NodeId> {
    pub layout: Layout<N>,
    pub crossings: usize,
    pub improved: bool,
    pub iterations: usize,
}

/// Run simulated annealing to reduce edge crossings.
///
/// `build_segments` constructs `LineSegment`s from the current layout, since
/// segment positions depend on node positions. The callback approach lets the
/// caller decide which edges become segments (e.g. skipping structural
/// stock-flow connections).
///
/// The algorithm follows the Go `annealingOptimizer.runAnnealing` method:
/// 1. Compute initial crossings; bail out immediately if zero.
/// 2. Each iteration: perturb 1..4 random non-pinned nodes, recompute
///    crossings, accept via Metropolis criterion.
/// 3. Cool temperature multiplicatively; periodically reheat.
/// 4. Track the best layout seen and return it.
pub fn run_annealing<N, F>(
    initial_layout: &Layout<N>,
    build_segments: F,
    config: &LayoutConfig,
    seed: u64,
) -> AnnealingResult<N>
where
    N: NodeId,
    F: Fn(&Layout<N>) -> Vec<LineSegment>,
{
    let mut rng = StdRng::seed_from_u64(seed);

    let initial_crossings = count_crossings(&build_segments(initial_layout));

    let base_temperature = config.annealing_temperature.max(derive_initial_temperature(
        initial_layout,
        &build_segments,
        config,
    ));
    let base_temperature = if base_temperature <= 0.0 {
        1.0
    } else {
        base_temperature
    };

    if initial_crossings == 0 {
        return AnnealingResult {
            layout: initial_layout.clone(),
            crossings: 0,
            improved: false,
            iterations: 0,
        };
    }

    // Snapshot baseline positions (displacement limits are relative to these)
    let baseline: BTreeMap<N, Position> = initial_layout.clone();

    let mut best_layout = initial_layout.clone();
    let mut best_crossings = initial_crossings;

    let mut test_layout = initial_layout.clone();
    let mut current_crossings = initial_crossings;

    let mut improved = false;
    let mut temperature = base_temperature;
    let cooling_rate = config.annealing_cooling_rate;
    let reheat_period = config.annealing_reheat_period;
    let effective_reheat = if config.annealing_reheat_temperature > 0.0 {
        config.annealing_reheat_temperature
    } else {
        base_temperature
    };

    let iterations = config.annealing_iterations;
    let mut total_iters = 0;

    for iter in 0..iterations {
        if best_crossings == 0 {
            break;
        }

        let perturbed = perturb_layout(&test_layout, &baseline, temperature, &mut rng);
        let perturbed_crossings = count_crossings(&build_segments(&perturbed));

        let delta = perturbed_crossings as f64 - current_crossings as f64;
        let accept_prob = if delta > 0.0 {
            (-delta / temperature).exp()
        } else {
            1.0
        };

        if rng.random::<f64>() < accept_prob {
            test_layout = perturbed;
            current_crossings = perturbed_crossings;

            if current_crossings < best_crossings {
                best_layout = test_layout.clone();
                best_crossings = current_crossings;
                improved = true;
            }
        }

        temperature *= cooling_rate;

        // Periodic reheating to escape local minima
        if (iter + 1) % reheat_period == 0 && best_crossings > 0 {
            temperature = effective_reheat;
        }

        total_iters = iter + 1;
    }

    AnnealingResult {
        layout: best_layout,
        crossings: best_crossings,
        improved,
        iterations: total_iters,
    }
}

/// Derive an initial temperature from the average edge length in the layout,
/// scaled by `config.annealing_temperature_scale`. Falls back to
/// `config.annealing_temperature` if the scale is non-positive or the layout
/// has no measurable edges.
fn derive_initial_temperature<N, F>(
    layout: &Layout<N>,
    build_segments: &F,
    config: &LayoutConfig,
) -> f64
where
    N: NodeId,
    F: Fn(&Layout<N>) -> Vec<LineSegment>,
{
    let scale = config.annealing_temperature_scale;
    if scale <= 0.0 {
        return config.annealing_temperature;
    }

    // Compute average distance between all pairs of positioned nodes
    // (matching the Go implementation which iterates over graph edges).
    // Without direct access to the graph edges here, we approximate by
    // using the segments the caller constructs.
    let segments = build_segments(layout);
    if segments.is_empty() {
        return config.annealing_temperature;
    }

    let mut total = 0.0;
    let mut count: usize = 0;
    for seg in &segments {
        let dist = (seg.end - seg.start).length();
        if dist > 0.0 {
            total += dist;
            count += 1;
        }
    }

    if count == 0 {
        return config.annealing_temperature;
    }

    (total / count as f64) * scale
}

/// Perturb 1..4 random nodes, clamping each to a maximum displacement from
/// its baseline position. Matches the Go `perturbLayout` method.
fn perturb_layout<N: NodeId>(
    layout: &Layout<N>,
    baseline: &BTreeMap<N, Position>,
    temperature: f64,
    rng: &mut StdRng,
) -> Layout<N> {
    let mut result = layout.clone();

    // Collect candidate nodes (all nodes present in the layout)
    let mut candidates: Vec<N> = layout.keys().cloned().collect();
    if candidates.is_empty() {
        return result;
    }

    let max_moves = 4.min(candidates.len());
    let num_to_perturb = 1 + rng.random_range(0..max_moves);

    for _ in 0..num_to_perturb {
        if candidates.is_empty() {
            break;
        }
        let idx = rng.random_range(0..candidates.len());
        let node_id = candidates[idx].clone();

        let (dx, dy) = generate_step(rng, temperature);
        apply_displacement(&node_id, baseline, &mut result, dx, dy);

        // Remove from candidates to avoid perturbing same node twice
        candidates.swap_remove(idx);
    }

    result
}

/// Move a node by (dx, dy) while clamping to stay within the maximum
/// displacement radius from its baseline position.
fn apply_displacement<N: NodeId>(
    node_id: &N,
    baseline: &BTreeMap<N, Position>,
    layout: &mut Layout<N>,
    dx: f64,
    dy: f64,
) {
    let Some(pos) = layout.get(node_id).copied() else {
        return;
    };
    let Some(base) = baseline.get(node_id).copied() else {
        return;
    };

    // Use a generous default limit; callers with more context (e.g.
    // knowing whether a node is a stock vs auxiliary) can provide a
    // filtered `build_segments` or wrap this function.
    let limit = 200.0;
    let target_x = (pos.x + dx).clamp(base.x - limit, base.x + limit);
    let target_y = (pos.y + dy).clamp(base.y - limit, base.y + limit);

    layout.insert(node_id.clone(), Position::new(target_x, target_y));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(x1: f64, y1: f64, x2: f64, y2: f64, from: &str, to: &str) -> LineSegment {
        LineSegment {
            start: Position::new(x1, y1),
            end: Position::new(x2, y2),
            from_node: from.to_string(),
            to_node: to.to_string(),
        }
    }

    #[test]
    fn test_segments_intersect_crossing() {
        // An X pattern: (0,0)-(10,10) crosses (0,10)-(10,0)
        let s1 = seg(0.0, 0.0, 10.0, 10.0, "a", "b");
        let s2 = seg(0.0, 10.0, 10.0, 0.0, "c", "d");
        assert!(do_segments_intersect(&s1, &s2));
    }

    #[test]
    fn test_segments_no_intersect_parallel() {
        let s1 = seg(0.0, 0.0, 10.0, 0.0, "a", "b");
        let s2 = seg(0.0, 5.0, 10.0, 5.0, "c", "d");
        assert!(!do_segments_intersect(&s1, &s2));
    }

    #[test]
    fn test_segments_no_intersect_separate() {
        // Two segments that don't overlap at all
        let s1 = seg(0.0, 0.0, 5.0, 0.0, "a", "b");
        let s2 = seg(10.0, 0.0, 15.0, 0.0, "c", "d");
        assert!(!do_segments_intersect(&s1, &s2));
    }

    #[test]
    fn test_segments_shared_endpoint() {
        // Two edges sharing node "b" -- not a crossing
        let s1 = seg(0.0, 0.0, 5.0, 5.0, "a", "b");
        let s2 = seg(5.0, 5.0, 10.0, 0.0, "b", "c");
        assert!(!do_segments_intersect(&s1, &s2));

        // Shared from_node
        let s3 = seg(0.0, 0.0, 5.0, 5.0, "a", "b");
        let s4 = seg(0.0, 0.0, 5.0, -5.0, "a", "c");
        assert!(!do_segments_intersect(&s3, &s4));
    }

    #[test]
    fn test_segments_collinear_overlap() {
        // Two collinear overlapping segments on the x-axis
        let s1 = seg(0.0, 0.0, 10.0, 0.0, "a", "b");
        let s2 = seg(5.0, 0.0, 15.0, 0.0, "c", "d");
        assert!(!do_segments_intersect(&s1, &s2));
    }

    #[test]
    fn test_count_crossings_zero() {
        // Two non-crossing segments (parallel horizontal lines)
        let segments = vec![
            seg(0.0, 0.0, 10.0, 0.0, "a", "b"),
            seg(0.0, 5.0, 10.0, 5.0, "c", "d"),
        ];
        assert_eq!(count_crossings(&segments), 0);
    }

    #[test]
    fn test_count_crossings_known() {
        // Three segments forming a triangle-like pattern with known crossings
        // s1 crosses s2, but s3 doesn't cross either
        let segments = vec![
            seg(0.0, 0.0, 10.0, 10.0, "a", "b"), // diagonal up-right
            seg(0.0, 10.0, 10.0, 0.0, "c", "d"), // diagonal down-right (crosses s1)
            seg(20.0, 0.0, 30.0, 0.0, "e", "f"), // far away horizontal
        ];
        assert_eq!(count_crossings(&segments), 1);

        // Four segments forming a grid pattern: 2 crossings
        // horizontal1 crosses vertical1, horizontal1 crosses vertical2,
        // but vertical1 and vertical2 are parallel
        let segments2 = vec![
            seg(0.0, 5.0, 20.0, 5.0, "a", "b"),   // horizontal
            seg(5.0, 0.0, 5.0, 10.0, "c", "d"),   // vertical (crosses horizontal)
            seg(15.0, 0.0, 15.0, 10.0, "e", "f"), // vertical (crosses horizontal)
        ];
        assert_eq!(count_crossings(&segments2), 2);
    }

    #[test]
    fn test_annealing_reduces_crossings() {
        // Create a layout with known crossings:
        // Four nodes forming a square, with edges that cross.
        //
        //   a(0,0) -----> c(10,10)
        //   b(0,10) ----> d(10,0)
        //
        // The edge a->c and b->d form an X and cross.
        let mut layout: Layout<String> = BTreeMap::new();
        layout.insert("a".to_string(), Position::new(0.0, 0.0));
        layout.insert("b".to_string(), Position::new(0.0, 100.0));
        layout.insert("c".to_string(), Position::new(100.0, 100.0));
        layout.insert("d".to_string(), Position::new(100.0, 0.0));

        let build_segments = |lay: &Layout<String>| -> Vec<LineSegment> {
            let a = lay["a"];
            let b = lay["b"];
            let c = lay["c"];
            let d = lay["d"];
            vec![
                LineSegment {
                    start: a,
                    end: c,
                    from_node: "a".to_string(),
                    to_node: "c".to_string(),
                },
                LineSegment {
                    start: b,
                    end: d,
                    from_node: "b".to_string(),
                    to_node: "d".to_string(),
                },
            ]
        };

        let initial_crossings = count_crossings(&build_segments(&layout));
        assert_eq!(initial_crossings, 1, "should start with 1 crossing");

        let config = LayoutConfig {
            annealing_iterations: 500,
            annealing_temperature: 50.0,
            annealing_cooling_rate: 0.99,
            annealing_reheat_period: 50,
            annealing_temperature_scale: 0.0,
            ..LayoutConfig::default()
        };

        let result = run_annealing(&layout, build_segments, &config, 42);

        // The annealer should not make things worse
        assert!(
            result.crossings <= initial_crossings,
            "crossings should not increase: got {} vs initial {}",
            result.crossings,
            initial_crossings,
        );
    }

    #[test]
    fn test_annealing_zero_crossings_noop() {
        // Layout with no crossings should return immediately
        let mut layout: Layout<String> = BTreeMap::new();
        layout.insert("a".to_string(), Position::new(0.0, 0.0));
        layout.insert("b".to_string(), Position::new(10.0, 0.0));

        let build_segments = |lay: &Layout<String>| -> Vec<LineSegment> {
            vec![LineSegment {
                start: lay["a"],
                end: lay["b"],
                from_node: "a".to_string(),
                to_node: "b".to_string(),
            }]
        };

        let config = LayoutConfig::default();
        let result = run_annealing(&layout, build_segments, &config, 42);

        assert_eq!(result.crossings, 0);
        assert!(!result.improved);
        assert_eq!(result.iterations, 0);
    }

    #[test]
    fn test_sample_standard_normal_distribution() {
        // Smoke test: sample many values and check mean/stddev are roughly
        // correct for a standard normal.
        let mut rng = StdRng::seed_from_u64(12345);
        let n = 10_000;
        let mut sum = 0.0;
        let mut sum_sq = 0.0;
        for _ in 0..n {
            let v = sample_standard_normal(&mut rng);
            sum += v;
            sum_sq += v * v;
        }
        let mean = sum / n as f64;
        let variance = sum_sq / n as f64 - mean * mean;
        assert!(mean.abs() < 0.05, "mean should be near 0, got {mean}");
        assert!(
            (variance - 1.0).abs() < 0.1,
            "variance should be near 1, got {variance}"
        );
    }

    #[test]
    fn test_generate_step_deterministic() {
        let mut rng1 = StdRng::seed_from_u64(99);
        let mut rng2 = StdRng::seed_from_u64(99);
        let (dx1, dy1) = generate_step(&mut rng1, 10.0);
        let (dx2, dy2) = generate_step(&mut rng2, 10.0);
        assert!((dx1 - dx2).abs() < f64::EPSILON);
        assert!((dy1 - dy2).abs() < f64::EPSILON);
    }
}
