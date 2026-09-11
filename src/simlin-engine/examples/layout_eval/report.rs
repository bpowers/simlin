// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The on-disk artifacts: `metrics.json` (per-model breakdowns), `corpus.json`
//! (the per-seed samples a later run diffs against), and the `index.html`
//! contact sheet. Building the report and rendering HTML are pure reads over
//! the sweep's results; the only I/O is in the `write_*`/`read_*` shells.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use simlin_engine::layout::eval_stats::{Comparison, CorpusReport, ModelStats};
use simlin_engine::layout::metrics::{LayoutMetrics, MetricWeights};

use crate::corpus::{Reference, Tier};
use crate::render::Render;
use crate::taste::{TasteCheck, tally};

/// Everything rendered for one model. A render that failed is `None` (already
/// WARN-logged); the contact sheet records the gap rather than hiding it.
pub struct ModelRenders {
    pub reference: Option<Render>,
    pub production: Option<Render>,
    pub median: Option<Render>,
    pub worst: Option<Render>,
}

/// Facts about a model the report carries beside its statistics.
pub struct ModelFacts {
    pub tier: Tier,
    pub reference: Reference,
    pub variables: usize,
    /// Wall-clock milliseconds of the production `generate_best_layout` call.
    pub production_ms: Option<f64>,
    /// Taste checks over the reference (empty when it is not one diagram).
    pub taste_reference: Vec<TasteCheck>,
    /// Taste checks over the production layout.
    pub taste_production: Vec<TasteCheck>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RenderReport {
    pub file: String,
    pub seed: Option<u64>,
    pub metrics: LayoutMetrics,
    pub weighted_cost: f64,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct ModelReport {
    pub model: String,
    pub tier: String,
    pub reference_kind: String,
    pub variables: usize,
    /// Number of seeds swept.
    pub m: usize,
    pub median_cost: f64,
    /// `(p25, p75)` of the per-seed weighted costs.
    pub spread: (f64, f64),
    /// Production proxy: min weighted cost over the `LAYOUT_SEEDS` seed set.
    pub best_of_k_cost: f64,
    pub best_seed: u64,
    pub median_seed: u64,
    pub worst_seed: u64,
    pub production_ms: Option<f64>,
    #[serde(default)]
    pub taste_reference: Vec<TasteCheck>,
    #[serde(default)]
    pub taste_production: Vec<TasteCheck>,
    pub reference: Option<RenderReport>,
    pub production: Option<RenderReport>,
    pub median: Option<RenderReport>,
    pub worst: Option<RenderReport>,
}

/// The `metrics.json` document. `baseline_comparison` diffs against the
/// committed baseline, `run_comparison` against `LAYOUT_EVAL_COMPARE`'s run;
/// both are re-scored under this run's weights before comparing.
#[derive(Serialize, Deserialize)]
pub struct EvalReport {
    /// Models sorted worst-cost-first (highest `median_cost` at the front), the
    /// order the contact sheet inspects top-down.
    pub models: Vec<ModelReport>,
    /// Shifted geometric mean of the per-model medians.
    pub aggregate_cost: f64,
    pub weights: MetricWeights,
    #[serde(skip_serializing_if = "Option::is_none", default, skip_deserializing)]
    pub baseline_comparison: Option<Comparison>,
    #[serde(skip_serializing_if = "Option::is_none", default, skip_deserializing)]
    pub run_comparison: Option<Comparison>,
}

fn render_report(render: &Render) -> RenderReport {
    RenderReport {
        file: render.file.clone(),
        seed: render.seed,
        metrics: render.metrics,
        weighted_cost: render.weighted_cost,
    }
}

fn snake<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// Build the serializable report. PURE: a read over the positionally paired
/// `(stats, renders, facts)` plus the aggregate and weights. Models are sorted
/// worst-cost-first; ties break on the name so the order is deterministic.
pub fn build_report(
    per_model: &[ModelStats],
    renders: &[ModelRenders],
    facts: &[ModelFacts],
    aggregate_cost: f64,
    weights: &MetricWeights,
    baseline_comparison: Option<Comparison>,
    run_comparison: Option<Comparison>,
) -> EvalReport {
    let mut models: Vec<ModelReport> = per_model
        .iter()
        .zip(renders)
        .zip(facts)
        .map(|((stats, render), fact)| ModelReport {
            model: stats.model.clone(),
            tier: snake(&fact.tier),
            reference_kind: snake(&fact.reference),
            variables: fact.variables,
            m: stats.samples.len(),
            median_cost: stats.median_cost,
            spread: stats.spread,
            best_of_k_cost: stats.best_of_k_cost,
            best_seed: stats.best_seed,
            median_seed: stats.median_seed,
            worst_seed: stats.worst_seed,
            production_ms: fact.production_ms,
            taste_reference: fact.taste_reference.clone(),
            taste_production: fact.taste_production.clone(),
            reference: render.reference.as_ref().map(render_report),
            production: render.production.as_ref().map(render_report),
            median: render.median.as_ref().map(render_report),
            worst: render.worst.as_ref().map(render_report),
        })
        .collect();
    models.sort_by(|a, b| {
        b.median_cost
            .partial_cmp(&a.median_cost)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.model.cmp(&b.model))
    });
    EvalReport {
        models,
        aggregate_cost,
        weights: *weights,
        baseline_comparison,
        run_comparison,
    }
}

/// HTML-escape the five characters special in element text or attribute
/// values. Model keys and filenames are static, so this is defense in depth.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

fn fmt_delta_pct(ratio: f64) -> String {
    format!("{:+.2}%", ratio * 100.0)
}

/// One render's cell: heading, a thumbnail linking to the full-size PNG, and
/// the per-term breakdown. `before` is the same render kind from the compared
/// run, when present, whose terms are shown alongside. PURE.
fn write_render_cell(
    html: &mut String,
    kind: &str,
    render: Option<&RenderReport>,
    before: Option<&RenderReport>,
) {
    html.push_str("<div class=\"cell\">");
    let _ = write!(html, "<h4>{}</h4>", html_escape(kind));
    let Some(r) = render else {
        html.push_str("<p class=\"missing\">(not rendered)</p></div>");
        return;
    };
    let src = html_escape(&r.file);
    let _ = write!(
        html,
        "<a href=\"{src}\" target=\"_blank\"><img src=\"{src}\" alt=\"{} layout\"></a>",
        html_escape(kind)
    );
    if let Some(seed) = r.seed {
        let _ = write!(html, "<p class=\"seed\">seed {seed}</p>");
    }
    html.push_str("<table class=\"metrics\">");
    let before_rows = before.map(|b| b.metrics.terms());
    for (i, (name, value)) in r.metrics.terms().into_iter().enumerate() {
        let _ = write!(html, "<tr><td>{name}</td><td class=\"num\">{value:.4}</td>");
        if let Some(rows) = &before_rows {
            let old = rows[i].1;
            let class = if (value - old).abs() < 1e-9 {
                "same"
            } else if value < old {
                "better"
            } else {
                "worse"
            };
            let _ = write!(html, "<td class=\"num {class}\">{old:.4}</td>");
        }
        html.push_str("</tr>");
    }
    let _ = write!(
        html,
        "<tr class=\"wcost\"><td>weighted_cost</td><td class=\"num\">{:.4}</td>",
        r.weighted_cost
    );
    if let Some(b) = before {
        let _ = write!(html, "<td class=\"num\">{:.4}</td>", b.weighted_cost);
    }
    html.push_str("</tr></table></div>");
}

/// One model's taste checks as a compact row of `degradation +delta%` chips,
/// red where the metric failed to penalize the degradation. PURE.
fn write_taste_row(html: &mut String, kind: &str, checks: &[TasteCheck]) {
    if checks.is_empty() {
        return;
    }
    let (noticed, applicable) = tally(checks);
    let _ = write!(
        html,
        "<p class=\"taste\">taste checks on {kind}: {noticed}/{applicable} noticed &middot; "
    );
    for c in checks {
        match (c.delta_ratio, c.noticed) {
            (Some(r), Some(true)) => {
                let _ = write!(
                    html,
                    "<span class=\"better\">{} {}</span> ",
                    c.degradation,
                    fmt_delta_pct(r)
                );
            }
            (Some(r), _) => {
                let _ = write!(
                    html,
                    "<span class=\"worse\">{} {}</span> ",
                    c.degradation,
                    fmt_delta_pct(r)
                );
            }
            _ => {
                let _ = write!(html, "<span class=\"nonsig\">{} n/a</span> ", c.degradation);
            }
        }
    }
    html.push_str("</p>");
}

/// The corpus-wide taste matrix: for each degradation, how many diagrams the
/// metric penalized it on, over references and over production layouts. PURE.
fn write_taste_summary(html: &mut String, report: &EvalReport) {
    let Some(first) = report
        .models
        .iter()
        .find(|m| !m.taste_production.is_empty())
    else {
        return;
    };
    html.push_str(
        "<div class=\"baseline\"><h3>Taste checks (does the metric penalize a visibly worse edit?)</h3>\
         <table class=\"diff\"><tr><th>degradation</th><th>references</th><th>production</th></tr>",
    );
    for (i, check) in first.taste_production.iter().enumerate() {
        let column = |select: fn(&ModelReport) -> &Vec<TasteCheck>| {
            tally(report.models.iter().filter_map(|m| select(m).get(i)))
        };
        let (rn, ra) = column(|m| &m.taste_reference);
        let (pn, pa) = column(|m| &m.taste_production);
        let class = |n: usize, a: usize| if n == a { "better" } else { "worse" };
        let _ = write!(
            html,
            "<tr><td>{}</td><td class=\"num {}\">{rn}/{ra}</td><td class=\"num {}\">{pn}/{pa}</td></tr>",
            check.degradation,
            class(rn, ra),
            class(pn, pa),
        );
    }
    html.push_str("</table></div>\n");
}

/// A comparison table: the aggregate verdict and per-model deltas. PURE.
fn write_comparison(html: &mut String, title: &str, cmp: &Comparison) {
    let _ = write!(
        html,
        "<div class=\"baseline\"><h3>{}</h3>",
        html_escape(title)
    );
    let (class, verdict) = if cmp.aggregate_significant {
        ("sig", "significant")
    } else {
        ("nonsig", "not significant")
    };
    let _ = write!(
        html,
        "<p class=\"agg\">aggregate delta <code>{}</code> &middot; paired p={:.4} &middot; \
         <span class=\"{class}\">{verdict}</span></p>",
        fmt_delta_pct(cmp.aggregate_delta_ratio),
        cmp.aggregate_p_value,
    );
    html.push_str(
        "<table class=\"diff\"><tr><th>model</th><th>before</th><th>after</th>\
         <th>delta</th><th>p</th><th>verdict</th></tr>",
    );
    for m in &cmp.per_model {
        let (cls, verdict) = if !m.significant {
            ("nonsig", "&mdash;")
        } else if m.delta_ratio < 0.0 {
            ("better", "better")
        } else {
            ("worse", "worse")
        };
        let _ = write!(
            html,
            "<tr><td>{}</td><td class=\"num\">{:.4}</td><td class=\"num\">{:.4}</td>\
             <td class=\"num\">{}</td><td class=\"num\">{:.4}</td><td class=\"{cls}\">{verdict}</td></tr>",
            html_escape(&m.model),
            m.baseline_median,
            m.candidate_median,
            fmt_delta_pct(m.delta_ratio),
            m.p_value,
        );
    }
    html.push_str("</table></div>\n");
}

/// Render the self-contained `index.html` contact sheet. `before` is the
/// compared run's report (for per-term deltas), when one was given. PURE.
pub fn render_index_html(report: &EvalReport, before: Option<&EvalReport>) -> String {
    let mut html = String::new();
    html.push_str(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Layout quality eval</title>\n<style>\n\
         :root { font-family: Roboto, Helvetica, Arial, sans-serif; }\n\
         body { margin: 24px; color: #1a1a1a; background: #fafafa; }\n\
         h1 { font-size: 20px; margin: 0 0 4px; }\n\
         .summary { color: #555; font-size: 13px; margin-bottom: 16px; }\n\
         code { background: #eee; padding: 1px 4px; border-radius: 4px; }\n\
         table.weights { border-collapse: collapse; font-size: 12px; margin: 8px 0 24px; }\n\
         table.weights td { border: 1px solid #ddd; padding: 2px 8px; }\n\
         .baseline { border: 1px solid #ddd; border-radius: 4px; background: #fff;\n\
                     padding: 8px 12px; margin: 8px 0 24px; }\n\
         .baseline h3 { font-size: 13px; margin: 0 0 6px; }\n\
         .baseline .agg { font-size: 12px; color: #555; margin: 0 0 6px; }\n\
         table.diff { border-collapse: collapse; font-size: 12px; }\n\
         table.diff th, table.diff td { border: 1px solid #eee; padding: 2px 8px; text-align: right; }\n\
         table.diff th:first-child, table.diff td:first-child { text-align: left; }\n\
         .num { font-variant-numeric: tabular-nums; }\n\
         .better { color: #2e7d32; font-weight: 600; }\n\
         .worse { color: #c62828; font-weight: 600; }\n\
         .sig { color: #c62828; font-weight: 600; }\n\
         .nonsig, .same { color: #888; }\n\
         .none { color: #999; font-style: italic; font-size: 12px; margin: 0 0 24px; }\n\
         .model { border: 1px solid #ddd; border-radius: 4px; background: #fff;\n\
                  padding: 12px 16px; margin-bottom: 20px; }\n\
         .model h2 { font-size: 16px; margin: 0 0 2px; }\n\
         .model .stats { color: #555; font-size: 12px; margin-bottom: 12px; }\n\
         .renders { display: flex; flex-wrap: wrap; gap: 16px; }\n\
         .cell { flex: 0 0 auto; max-width: 440px; }\n\
         .cell h4 { font-size: 13px; margin: 0 0 4px; text-transform: capitalize; }\n\
         .cell img { max-width: 440px; max-height: 520px; height: auto; border: 1px solid #eee;\n\
                     background: #fff; display: block; }\n\
         .cell .seed { font-size: 11px; color: #888; margin: 4px 0 2px; }\n\
         .cell .missing { font-size: 12px; color: #999; font-style: italic; }\n\
         table.metrics { border-collapse: collapse; font-size: 11px; margin-top: 4px; width: 100%; }\n\
         table.metrics td { border-bottom: 1px solid #f0f0f0; padding: 1px 4px; }\n\
         table.metrics td.num { text-align: right; }\n\
         table.metrics tr.wcost td { font-weight: 600; border-top: 1px solid #ccc; }\n\
         .taste { font-size: 12px; margin: 8px 0 0; }\n\
         </style>\n</head>\n<body>\n",
    );
    html.push_str("<h1>Layout quality eval</h1>\n");
    let total_ms: f64 = report.models.iter().filter_map(|m| m.production_ms).sum();
    let _ = writeln!(
        &mut html,
        "<p class=\"summary\">Corpus <code>aggregate_cost = {:.4}</code> over {} model(s), \
         sorted worst-cost-first &middot; production layout time {:.1}s total{}.</p>",
        report.aggregate_cost,
        report.models.len(),
        total_ms / 1000.0,
        if before.is_some() {
            " &middot; grey columns are the compared run"
        } else {
            ""
        },
    );

    html.push_str("<table class=\"weights\"><caption>weights</caption>");
    for (name, value) in report.weights.terms() {
        let _ = write!(
            &mut html,
            "<tr><td>{name}</td><td class=\"num\">{value:.4}</td></tr>"
        );
    }
    html.push_str("</table>\n");

    write_taste_summary(&mut html, report);

    if let Some(cmp) = &report.run_comparison {
        write_comparison(&mut html, "Compared run", cmp);
    }
    match &report.baseline_comparison {
        Some(cmp) => write_comparison(&mut html, "Committed baseline", cmp),
        None => html.push_str(
            "<p class=\"none\">No baseline diff (run with \
             <code>LAYOUT_EVAL_WRITE_BASELINE=1</code> to seed one).</p>\n",
        ),
    }

    for model in &report.models {
        let prior = before.and_then(|b| b.models.iter().find(|m| m.model == model.model));
        html.push_str("<section class=\"model\">");
        let _ = write!(&mut html, "<h2>{}</h2>", html_escape(&model.model));
        let timing = match (model.production_ms, prior.and_then(|p| p.production_ms)) {
            (Some(ms), Some(old)) => format!(" &middot; production {ms:.0}ms (was {old:.0}ms)"),
            (Some(ms), None) => format!(" &middot; production {ms:.0}ms"),
            _ => String::new(),
        };
        let _ = write!(
            &mut html,
            "<p class=\"stats\">{} &middot; {} variables &middot; reference: {} &middot; \
             median={:.4} &middot; p25/p75={:.4}/{:.4} &middot; best_of_k={:.4} &middot; M={}{timing}</p>",
            html_escape(&model.tier),
            model.variables,
            html_escape(&model.reference_kind),
            model.median_cost,
            model.spread.0,
            model.spread.1,
            model.best_of_k_cost,
            model.m,
        );
        html.push_str("<div class=\"renders\">");
        write_render_cell(&mut html, "reference", model.reference.as_ref(), None);
        write_render_cell(
            &mut html,
            "production",
            model.production.as_ref(),
            prior.and_then(|p| p.production.as_ref()),
        );
        write_render_cell(
            &mut html,
            "median",
            model.median.as_ref(),
            prior.and_then(|p| p.median.as_ref()),
        );
        write_render_cell(
            &mut html,
            "worst",
            model.worst.as_ref(),
            prior.and_then(|p| p.worst.as_ref()),
        );
        html.push_str("</div>");
        write_taste_row(&mut html, "reference", &model.taste_reference);
        write_taste_row(&mut html, "production", &model.taste_production);
        html.push_str("</section>\n");
    }
    html.push_str("</body>\n</html>\n");
    html
}

/// Print a comparison to stdout, one line per model plus the aggregate.
pub fn print_comparison(title: &str, cmp: &Comparison) {
    println!("{title} (after vs before):");
    for m in &cmp.per_model {
        let verdict = if m.significant {
            "significant"
        } else {
            "not significant"
        };
        println!(
            "  {}: {:.4} -> {:.4} delta={} p={:.4} ({verdict})",
            m.model,
            m.baseline_median,
            m.candidate_median,
            fmt_delta_pct(m.delta_ratio),
            m.p_value,
        );
    }
    let verdict = if cmp.aggregate_significant {
        "significant"
    } else {
        "not significant"
    };
    println!(
        "  aggregate: delta={} paired p={:.4} ({verdict})",
        fmt_delta_pct(cmp.aggregate_delta_ratio),
        cmp.aggregate_p_value,
    );
}

/// Serialize `value` as pretty JSON to `path`, WARN-logging any failure.
pub fn write_json<T: Serialize>(path: &str, value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(json) => match std::fs::write(path, json) {
            Ok(()) => println!("wrote {path}"),
            Err(err) => eprintln!("WARN: failed to write {path}: {err}"),
        },
        Err(err) => eprintln!("WARN: failed to serialize {path}: {err}"),
    }
}

/// Read a `CorpusReport` (the committed baseline, or a run's `corpus.json`).
/// A missing file is a quiet `None`; an unreadable or unparseable one WARNs.
pub fn read_corpus(path: &str) -> Option<CorpusReport> {
    let json = match std::fs::read_to_string(path) {
        Ok(json) => json,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            println!("no corpus report at {path}");
            return None;
        }
        Err(err) => {
            eprintln!("WARN: failed to read {path}: {err}");
            return None;
        }
    };
    match serde_json::from_str(&json) {
        Ok(report) => Some(report),
        Err(err) => {
            eprintln!("WARN: failed to parse {path}: {err}");
            None
        }
    }
}

/// Read a previous run's `metrics.json`, for per-term deltas in the sheet.
pub fn read_eval_report(path: &str) -> Option<EvalReport> {
    let json = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&json) {
        Ok(report) => Some(report),
        Err(err) => {
            eprintln!("WARN: failed to parse {path}: {err}");
            None
        }
    }
}
