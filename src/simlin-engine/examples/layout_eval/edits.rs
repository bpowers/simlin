// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Edit scenarios over the corpus (`layout::edit_scenarios`): every scenario
//! kind driven from a model's shipped diagram (its production layout when it
//! ships none) through the production patch and sync path, and audited step by
//! step (`layout::edit_audit`). Each run is rendered before and after its last
//! step, marking what the sync created (green), changed (orange) and removed
//! (red, on the before render), and every finding with a location (magenta, on
//! the after render). Writes `edits.json` and the `edits.html` contact sheet.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Serialize;
use simlin_engine::datamodel::{self, StockFlow};
use simlin_engine::layout::edit_audit::{ChangeKind, Displacement, Finding, view_changes};
use simlin_engine::layout::edit_scenarios::{ScenarioKind, build_scenario, run_scenario};
use simlin_engine::layout::generate_best_layout;

use crate::corpus::{self, MAIN_MODEL, ModelSpec};
use crate::render;
use crate::report::{html_escape, write_json};

/// One scenario run on one model.
#[derive(Serialize)]
pub struct EditReport {
    pub model: String,
    pub scenario: String,
    pub description: String,
    /// Steps that applied and synced, of `planned`.
    pub steps: usize,
    pub planned: usize,
    pub findings: Vec<Finding>,
    /// For a rename spelled as a delete and a create: how far the variable
    /// moved.
    pub continuity: Vec<Displacement>,
    /// Elements rebuilt by kind changes or re-attachment, and how far each moved.
    pub displacements: Vec<Displacement>,
    pub completed_variables: Vec<String>,
    /// Layout-quality cost before the first step and after the last.
    pub cost_before: Option<f64>,
    pub cost_after: Option<f64>,
    pub before_png: Option<String>,
    pub after_png: Option<String>,
}

fn rect(region: [f64; 4], stroke: &str, fill: &str, dash: &str) -> String {
    let [l, t, r, b] = region;
    format!(
        "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" \
         style=\"fill:{fill};stroke:{stroke};stroke-width:1.5;stroke-dasharray:{dash}\"/>",
        l - 3.0,
        t - 3.0,
        (r - l).max(0.0) + 6.0,
        (b - t).max(0.0) + 6.0
    )
}

/// SVG marks for one render: what changed, and for the after render what the
/// audit found.
fn marks(before: &StockFlow, after: &StockFlow, findings: &[Finding], after_side: bool) -> String {
    let mut svg = String::from("<g class=\"eval-edit-marks\">");
    for change in view_changes(before, after) {
        let Some(region) = change.region else {
            continue;
        };
        match (change.kind, after_side) {
            (ChangeKind::Created, true) => {
                svg.push_str(&rect(region, "#2e7d32", "rgba(46,125,50,0.10)", "none"))
            }
            (ChangeKind::Changed, true) => {
                svg.push_str(&rect(region, "#ef6c00", "rgba(239,108,0,0.10)", "4,2"))
            }
            (ChangeKind::Removed, false) => {
                svg.push_str(&rect(region, "#c62828", "rgba(198,40,40,0.10)", "4,2"))
            }
            _ => {}
        }
    }
    if after_side {
        for f in findings {
            if let Some(region) = f.region {
                svg.push_str(&rect(region, "#ad1457", "rgba(173,20,87,0.20)", "none"));
            }
        }
    }
    svg.push_str("</g>");
    svg
}

fn with_view(project: &datamodel::Project, view: &StockFlow) -> datamodel::Project {
    let mut p = project.clone();
    if let Some(m) = p.get_model_mut(MAIN_MODEL) {
        m.views = vec![datamodel::View::StockFlow(view.clone())];
    }
    p
}

/// Run every applicable scenario on `spec`'s model, rendering each run into
/// `out`. A model that fails to load or lay out is reported and yields nothing.
pub fn run_model(spec: &ModelSpec, out: &str) -> Vec<EditReport> {
    let project = match corpus::load_model(spec) {
        Ok(p) => p,
        Err(err) => {
            eprintln!("WARN: {} edits: {err}", spec.key);
            return Vec::new();
        }
    };
    let view = match corpus::reference_view(&project) {
        Some(sf) => sf.clone(),
        None => match generate_best_layout(&project, MAIN_MODEL, None) {
            Ok(v) => v,
            Err(err) => {
                eprintln!("WARN: {} edits: no starting diagram: {err}", spec.key);
                return Vec::new();
            }
        },
    };
    let start = with_view(&project, &view);
    let mut reports = Vec::new();
    for kind in ScenarioKind::ALL {
        let Some(scenario) = build_scenario(&start, MAIN_MODEL, kind) else {
            continue;
        };
        let outcome = run_scenario(&start, MAIN_MODEL, &view, &scenario);
        let stem = format!("{}_edit_{}", spec.key, kind.name());
        let (before_png, after_png) = match outcome.steps.last() {
            Some(step) => {
                let before_project = match outcome.steps.len() {
                    1 => &start,
                    n => &outcome.steps[n - 2].after,
                };
                let step_findings: Vec<Finding> = step.audit.findings.clone();
                let before_file = format!("{stem}_before.png");
                let after_file = format!("{stem}_after.png");
                let before_ok = render::render_marked(
                    before_project,
                    &step.before_view,
                    &before_file,
                    out,
                    &marks(&step.before_view, &step.after_view, &[], false),
                );
                let after_ok = render::render_marked(
                    &step.after,
                    &step.after_view,
                    &after_file,
                    out,
                    &marks(&step.before_view, &step.after_view, &step_findings, true),
                );
                (
                    before_ok.then_some(before_file),
                    after_ok.then_some(after_file),
                )
            }
            None => (None, None),
        };
        let audits = outcome.steps.iter().map(|s| &s.audit);
        reports.push(EditReport {
            model: spec.key.clone(),
            scenario: kind.name().to_string(),
            description: outcome.description.clone(),
            steps: outcome.steps.len(),
            planned: scenario.steps.len(),
            findings: outcome.findings.clone(),
            continuity: outcome.continuity.clone(),
            displacements: audits
                .clone()
                .flat_map(|a| a.displacements.clone())
                .collect(),
            completed_variables: audits
                .clone()
                .flat_map(|a| a.completed_variables.clone())
                .collect(),
            cost_before: outcome.steps.first().map(|s| s.audit.cost_before),
            cost_after: outcome.steps.last().map(|s| s.audit.cost_after),
            before_png,
            after_png,
        });
    }
    let with_findings = reports.iter().filter(|r| !r.findings.is_empty()).count();
    println!(
        "{}: edits: {} scenario(s), {with_findings} with findings",
        spec.key,
        reports.len()
    );
    reports
}

/// Write `edits.json` and `edits.html` under `out`.
pub fn write(reports: &[EditReport], out: &str) {
    write_json(&format!("{out}/edits.json"), &reports);
    let path = format!("{out}/edits.html");
    match std::fs::write(&path, render_html(reports)) {
        Ok(()) => println!("wrote {path}"),
        Err(err) => eprintln!("WARN: failed to write {path}: {err}"),
    }
}

fn render_html(reports: &[EditReport]) -> String {
    let mut html = String::new();
    html.push_str(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Diagram edit eval</title>\n<style>\n\
         :root { font-family: Roboto, Helvetica, Arial, sans-serif; }\n\
         body { margin: 24px; color: #1a1a1a; background: #fafafa; }\n\
         h1 { font-size: 20px; margin: 0 0 4px; }\n\
         h2 { font-size: 16px; margin: 24px 0 8px; }\n\
         .summary { color: #555; font-size: 13px; margin-bottom: 16px; }\n\
         table.counts { border-collapse: collapse; font-size: 12px; margin: 8px 0 16px; }\n\
         table.counts td { border: 1px solid #ddd; padding: 2px 8px; }\n\
         .run { border: 1px solid #ddd; border-radius: 4px; background: #fff; padding: 10px 14px; margin-bottom: 14px; }\n\
         .run.clean { border-left: 4px solid #2e7d32; }\n\
         .run.dirty { border-left: 4px solid #ad1457; }\n\
         .run h3 { font-size: 13px; margin: 0 0 2px; }\n\
         .run .desc { color: #555; font-size: 12px; margin: 0 0 6px; }\n\
         .run ul { font-size: 12px; margin: 4px 0; padding-left: 18px; }\n\
         .run .note { color: #666; font-size: 11px; margin: 2px 0; }\n\
         .renders { display: flex; flex-wrap: wrap; gap: 12px; margin-top: 6px; }\n\
         .renders img { max-width: 520px; max-height: 420px; border: 1px solid #eee; background: #fff; display: block; }\n\
         .legend span { font-size: 12px; margin-right: 12px; }\n\
         </style>\n</head>\n<body>\n<h1>Diagram edit eval</h1>\n",
    );
    let dirty = reports.iter().filter(|r| !r.findings.is_empty()).count();
    let _ = writeln!(
        html,
        "<p class=\"summary\">{} scenario run(s) over {} model(s); {dirty} with findings.</p>",
        reports.len(),
        reports
            .iter()
            .map(|r| &r.model)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );
    html.push_str(
        "<p class=\"legend\"><span style=\"color:#2e7d32\">created</span>\
         <span style=\"color:#ef6c00\">changed</span>\
         <span style=\"color:#c62828\">removed (before)</span>\
         <span style=\"color:#ad1457\">finding</span></p>",
    );

    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for r in reports {
        for f in &r.findings {
            *counts.entry(f.kind.name()).or_default() += 1;
        }
    }
    html.push_str("<table class=\"counts\"><caption>findings by kind</caption>");
    for (kind, n) in &counts {
        let _ = write!(html, "<tr><td>{kind}</td><td>{n}</td></tr>");
    }
    html.push_str("</table>\n");

    let mut model = "";
    for r in reports {
        if r.model != model {
            model = &r.model;
            let _ = writeln!(html, "<h2>{}</h2>", html_escape(model));
        }
        let class = if r.findings.is_empty() {
            "clean"
        } else {
            "dirty"
        };
        let _ = write!(
            html,
            "<section class=\"run {class}\"><h3>{}</h3><p class=\"desc\">{}",
            html_escape(&r.scenario),
            html_escape(&r.description)
        );
        if let (Some(before), Some(after)) = (r.cost_before, r.cost_after) {
            let _ = write!(html, " &middot; cost {before:.3} &rarr; {after:.3}");
        }
        if r.steps < r.planned {
            let _ = write!(
                html,
                " &middot; stopped after {} of {} steps",
                r.steps, r.planned
            );
        }
        html.push_str("</p>");
        if !r.findings.is_empty() {
            html.push_str("<ul>");
            for f in &r.findings {
                let _ = write!(
                    html,
                    "<li><b>{}</b> {}: {}</li>",
                    f.kind.name(),
                    html_escape(&f.subject),
                    html_escape(&f.detail)
                );
            }
            html.push_str("</ul>");
        }
        for d in r.displacements.iter().chain(&r.continuity) {
            let _ = write!(
                html,
                "<p class=\"note\">{} moved {:.1}</p>",
                html_escape(&d.subject),
                d.distance
            );
        }
        if !r.completed_variables.is_empty() {
            let _ = write!(
                html,
                "<p class=\"note\">variables drawn for the first time: {}</p>",
                html_escape(&r.completed_variables.join(", "))
            );
        }
        html.push_str("<div class=\"renders\">");
        for file in [&r.before_png, &r.after_png].into_iter().flatten() {
            let src = html_escape(file);
            let _ = write!(
                html,
                "<a href=\"{src}\" target=\"_blank\"><img src=\"{src}\" alt=\"{src}\"></a>"
            );
        }
        html.push_str("</div></section>\n");
    }
    html.push_str("</body>\n</html>\n");
    html
}
