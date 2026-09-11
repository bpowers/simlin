// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Rasterize views to PNG at a size a person can actually read.

use simlin_engine::datamodel;
use simlin_engine::diagram::{PngRenderOpts, render_svg, svg_to_png};
use simlin_engine::layout::config::LayoutConfig;
use simlin_engine::layout::metrics::{
    Defect, DefectKind, LayoutMetrics, MetricWeights, analyze_layout, compute_layout_metrics,
};

use crate::corpus::MAIN_MODEL;

/// Target width for small diagrams: a textbook model rendered 1:1 is a few
/// hundred pixels wide and its 12px labels are unreadable once a contact sheet
/// shrinks it, so small diagrams are upscaled toward this width.
const TARGET_WIDTH_PX: f64 = 1200.0;

/// The largest upscale applied: past this a tiny diagram is just blurry.
const MAX_UPSCALE: f64 = 2.5;

/// One rendered diagram: its PNG filename (relative to the out dir), the seed
/// that produced it (`None` for the reference and for production, which picks
/// among several seeds), and the metrics of exactly the geometry rendered.
pub struct Render {
    pub file: String,
    pub seed: Option<u64>,
    pub metrics: LayoutMetrics,
    pub weighted_cost: f64,
}

/// The intrinsic width of a rendered SVG, from its `viewBox`.
fn svg_width(svg: &str) -> Option<f64> {
    let start = svg.find("viewBox=\"")? + "viewBox=\"".len();
    let end = start + svg[start..].find('"')?;
    svg[start..end].split_whitespace().nth(2)?.parse().ok()
}

/// Upscale factor for a diagram of intrinsic width `w`: grow small diagrams
/// toward `TARGET_WIDTH_PX` (capped), never shrink a large one -- its full
/// resolution is what a zoomed-in look needs.
fn upscale_for(w: f64) -> f64 {
    if w <= 0.0 {
        1.0
    } else {
        (TARGET_WIDTH_PX / w).clamp(1.0, MAX_UPSCALE)
    }
}

/// SVG marks for what the metric charged: one shape per defect, colored by
/// kind, drawn over the diagram so a look at the picture shows what the score
/// sees -- a defect the eye finds but no mark covers is a blind spot, and a
/// mark over something that reads fine is a false positive.
fn defect_overlay(defects: &[Defect]) -> String {
    let mut svg = String::from("<g class=\"eval-defects\">");
    for d in defects {
        let [l, t, r, b] = d.region;
        let (w, h) = ((r - l).max(0.0), (b - t).max(0.0));
        let rect = |stroke: &str, fill: &str, dash: &str| {
            format!(
                "<rect x=\"{l}\" y=\"{t}\" width=\"{w}\" height=\"{h}\" \
                 style=\"fill:{fill};stroke:{stroke};stroke-width:1.5;stroke-dasharray:{dash}\"/>"
            )
        };
        let dot = |color: &str, radius: f64| {
            format!(
                "<circle cx=\"{}\" cy=\"{}\" r=\"{radius}\" style=\"fill:{color};fill-opacity:0.7\"/>",
                (l + r) / 2.0,
                (t + b) / 2.0
            )
        };
        svg.push_str(&match d.kind {
            DefectKind::NodeOverlap => rect("#d32f2f", "rgba(211,47,47,0.35)", "none"),
            DefectKind::ConnectorThroughNode => rect("#ef6c00", "none", "3,2"),
            DefectKind::LabelObscured => rect("#c2185b", "rgba(194,24,91,0.15)", "none"),
            DefectKind::LabelCrossed => rect("#7b1fa2", "none", "2,2"),
            DefectKind::Crowded => rect("#f9a825", "none", "1,2"),
            DefectKind::Crossing => dot("#1565c0", 3.5),
            DefectKind::LongConnector => dot("#2e7d32", 6.0),
        });
    }
    svg.push_str("</g>");
    svg
}

fn rasterize(svg: &str, file: &str, out: &str) -> bool {
    let width = svg_width(svg).map(|w| (w * upscale_for(w)).round() as u32);
    let png = match svg_to_png(
        svg,
        &PngRenderOpts {
            width,
            height: None,
        },
    ) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("WARN: failed to rasterize {file}: {err}");
            return false;
        }
    };
    let path = format!("{out}/{file}");
    if let Err(err) = std::fs::write(&path, &png) {
        eprintln!("WARN: failed to write {path}: {err}");
        return false;
    }
    true
}

/// Render `view` (installed into a clone of `project`) to `{out}/{file}` and
/// score it; with `overlay`, also write `{stem}_defects.png` with the metric's
/// defects drawn over it. On any failure WARN and return `None` so the sweep
/// continues.
pub fn render_view(
    project: &datamodel::Project,
    view: &datamodel::StockFlow,
    seed: Option<u64>,
    file: &str,
    out: &str,
    overlay: bool,
) -> Option<Render> {
    let mut p = project.clone();
    p.get_model_mut(MAIN_MODEL)?.views = vec![datamodel::View::StockFlow(view.clone())];
    let svg = match render_svg(&p, MAIN_MODEL) {
        Ok(svg) => svg,
        Err(err) => {
            eprintln!("WARN: failed to render {file}: {err}");
            return None;
        }
    };
    if !rasterize(&svg, file, out) {
        return None;
    }
    // The rendered view itself, for inspection and for replaying a judgment
    // against a later metric: `{stem}.view.json` in the JSON project schema.
    if let Some(stem) = file.strip_suffix(".png") {
        let json_view = simlin_engine::json::View::from(datamodel::View::StockFlow(view.clone()));
        if let Ok(text) = serde_json::to_string(&json_view) {
            let _ = std::fs::write(format!("{out}/{stem}.view.json"), text);
        }
    }
    if overlay && let Some(stem) = file.strip_suffix(".png") {
        let analysis = analyze_layout(view);
        if let Some(end) = svg.rfind("</svg>") {
            let marked = format!("{}{}</svg>", &svg[..end], defect_overlay(&analysis.defects));
            rasterize(&marked, &format!("{stem}_defects.png"), out);
        }
    }
    let metrics = compute_layout_metrics(view, &LayoutConfig::default());
    Some(Render {
        file: file.to_string(),
        seed,
        metrics,
        weighted_cost: metrics.weighted_cost(&MetricWeights::default()),
    })
}
