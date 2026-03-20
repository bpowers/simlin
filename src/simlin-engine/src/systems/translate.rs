// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Translator from `SystemsModel` IR to `datamodel::Project`.
//!
//! Each systems flow becomes a stdlib module instance (systems_rate,
//! systems_leak, or systems_conversion), with actual transfer flows
//! and waste flows wired to stocks. Multi-outflow stocks produce
//! chained modules where each module's `available` references the
//! previous module's `remaining` output.

use std::collections::HashMap;

use crate::canonicalize;
use crate::common::Result;
use crate::datamodel::{
    Aux, Compat, Dt, Equation, Flow, Model, Module, ModuleReference, Project, SimMethod, SimSpecs,
    Stock, Variable,
};

use super::ast::{BinOp, Expr, FlowType, SystemsModel};

/// Default number of simulation rounds when no explicit value is given.
pub const DEFAULT_ROUNDS: u64 = 10;

/// Translate a parsed `SystemsModel` into a simlin `datamodel::Project`.
///
/// `num_rounds` sets the simulation stop time (start=0, dt=1, Euler).
/// Each systems flow is instantiated as a stdlib module, with actual
/// transfer flows wired between stocks. Multi-outflow stocks use
/// chained modules where each module's `available` input references
/// the previous module's `remaining` output. Flows are processed in
/// reversed declaration order (last-declared = highest priority).
pub fn translate(model: &SystemsModel, num_rounds: u64) -> Result<Project> {
    let sim_specs = SimSpecs {
        start: 0.0,
        stop: num_rounds as f64,
        dt: Dt::Dt(1.0),
        save_step: None,
        sim_method: SimMethod::Euler,
        time_units: None,
    };

    // Build canonical stock names and initial equations
    let mut stocks: Vec<StockBuilder> = model
        .stocks
        .iter()
        .map(|s| StockBuilder {
            ident: canon(&s.name),
            equation: s.initial.to_equation_string(),
            inflows: Vec::new(),
            outflows: Vec::new(),
        })
        .collect();

    // Index stocks by canonical name for lookup
    let stock_idx: HashMap<String, usize> = stocks
        .iter()
        .enumerate()
        .map(|(i, s)| (s.ident.clone(), i))
        .collect();

    // Group flows by source stock (preserving declaration order)
    let mut flows_by_source: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, flow) in model.flows.iter().enumerate() {
        let source_canon = canon(&flow.source);
        flows_by_source.entry(source_canon).or_default().push(i);
    }

    // Collect generated variables (modules, flows, aux helpers)
    let mut variables: Vec<Variable> = Vec::new();

    // Deferred items: dest_capacity and rate auxes need the full outflow list
    // for each stock before their equations can be finalized. We collect the
    // info during flow processing and create the aux variables afterward.
    struct DeferredCapacity {
        aux_ident: String,
        dest_canon: String,
        dest_max_expr: Option<Expr>,
        /// Source stock of this flow.
        source_canon: String,
        /// The `available` value for this flow (raw stock or chained remaining).
        /// Used to substitute the source stock reference in the max expression,
        /// giving the post-drain value without creating circular dependencies.
        available_src: String,
        /// Index into model.flows for ordering.
        flow_idx: usize,
    }
    // Deferred rate aux info: rate auxes need context-dependent rewrites
    // based on the global reversed flow processing order, so we collect
    // the info during the first pass and create them in a second pass.
    struct DeferredRate {
        aux_ident: String,
        rate_expr: Expr,
        /// Source stock of this flow.
        source_canon: String,
        /// The `available` value for this flow (raw stock or chained remaining).
        available_src: String,
        /// Index into model.flows for ordering.
        flow_idx: usize,
    }
    let mut deferred_capacities: Vec<DeferredCapacity> = Vec::new();
    let mut deferred_rates: Vec<DeferredRate> = Vec::new();

    // Pre-compute disambiguated suffixes for all flows. Flows with
    // the same (source, dest) pair get numbered: first occurrence has
    // no suffix, subsequent get _2, _3, etc.
    let flow_suffixes: HashMap<usize, String> = {
        let mut pair_counts: HashMap<(String, String), usize> = HashMap::new();
        let mut suffixes = HashMap::new();
        for stock in &model.stocks {
            let source_canon = canon(&stock.name);
            let flow_indices = match flows_by_source.get(&source_canon) {
                Some(indices) => indices,
                None => continue,
            };
            for &flow_idx in flow_indices.iter().rev() {
                let flow = &model.flows[flow_idx];
                let dest_canon = canon(&flow.dest);
                let pair_key = (source_canon.clone(), dest_canon.clone());
                let occurrence = pair_counts.entry(pair_key).or_insert(0);
                *occurrence += 1;
                let suffix = if *occurrence == 1 {
                    String::new()
                } else {
                    format!("_{}", *occurrence)
                };
                suffixes.insert(flow_idx, suffix);
            }
        }
        suffixes
    };

    // Process each source stock's outflows in reversed declaration order
    for stock in &model.stocks {
        let source_canon = canon(&stock.name);
        let flow_indices = match flows_by_source.get(&source_canon) {
            Some(indices) => indices,
            None => continue,
        };

        let has_single_outflow = flow_indices.len() == 1;
        let mut prev_module_ident: Option<String> = None;

        // Reverse: last-declared flow gets highest priority (processed first)
        for &flow_idx in flow_indices.iter().rev() {
            let flow = &model.flows[flow_idx];
            let dest_canon = canon(&flow.dest);
            let suffix = &flow_suffixes[&flow_idx];

            // Module ident: "{source}_outflows_{dest}" or "{source}_outflows" if single
            let module_ident = if has_single_outflow {
                format!("{source_canon}_outflows{suffix}")
            } else {
                format!("{source_canon}_outflows_{dest_canon}{suffix}")
            };

            // Choose stdlib model based on flow type
            let model_name = match flow.flow_type {
                FlowType::Rate => "stdlib\u{205A}systems_rate",
                FlowType::Leak => "stdlib\u{205A}systems_leak",
                FlowType::Conversion => "stdlib\u{205A}systems_conversion",
            };

            // Determine `available` input binding.
            // When chaining, create an intermediate aux variable to bridge
            // module outputs to module inputs, since module references can
            // only bind outer model variables to module ports.
            let available_src = match &prev_module_ident {
                None => source_canon.clone(),
                Some(prev) => {
                    let remaining_aux = format!("{prev}_remaining");
                    variables.push(Variable::Aux(Aux {
                        ident: remaining_aux.clone(),
                        equation: Equation::Scalar(format!("{prev}.remaining")),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: Compat::default(),
                    }));
                    remaining_aux
                }
            };

            // Record dest_capacity for deferred creation
            let dest_stock = model.stocks.iter().find(|s| canon(&s.name) == dest_canon);
            let dest_capacity_ident = format!("{module_ident}_dest_capacity");
            let needs_capacity = dest_stock.map(|s| s.max != Expr::Inf).unwrap_or(false);
            deferred_capacities.push(DeferredCapacity {
                aux_ident: dest_capacity_ident.clone(),
                dest_canon: dest_canon.clone(),
                dest_max_expr: if needs_capacity {
                    Some(dest_stock.unwrap().max.clone())
                } else {
                    None
                },
                source_canon: source_canon.clone(),
                available_src: available_src.clone(),
                flow_idx,
            });

            // Record rate/requested aux for deferred creation (needs rewriting)
            let rate_port_name = match flow.flow_type {
                FlowType::Rate => "requested",
                FlowType::Leak | FlowType::Conversion => "rate",
            };
            let rate_aux_ident = format!("{module_ident}_{rate_port_name}");
            deferred_rates.push(DeferredRate {
                aux_ident: rate_aux_ident.clone(),
                rate_expr: flow.rate.clone(),
                source_canon: source_canon.clone(),
                available_src: available_src.clone(),
                flow_idx,
            });

            // Build module references
            let references = vec![
                ModuleReference {
                    src: available_src,
                    dst: format!("{module_ident}.available"),
                },
                ModuleReference {
                    src: rate_aux_ident,
                    dst: format!("{module_ident}.{rate_port_name}"),
                },
                ModuleReference {
                    src: dest_capacity_ident,
                    dst: format!("{module_ident}.dest_capacity"),
                },
            ];

            variables.push(Variable::Module(Module {
                ident: module_ident.clone(),
                model_name: model_name.to_string(),
                documentation: String::new(),
                units: None,
                references,
                ai_state: None,
                uid: None,
                compat: Compat::default(),
            }));

            // Create the actual transfer flow
            let flow_ident = format!("{source_canon}_to_{dest_canon}{suffix}");
            let flow_equation = match flow.flow_type {
                FlowType::Rate | FlowType::Leak => format!("{module_ident}.actual"),
                FlowType::Conversion => format!("{module_ident}.outflow"),
            };

            variables.push(Variable::Flow(Flow {
                ident: flow_ident.clone(),
                equation: Equation::Scalar(flow_equation),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: Compat::default(),
            }));

            // Wire flow to stocks
            if let Some(&src_idx) = stock_idx.get(&source_canon) {
                stocks[src_idx].outflows.push(flow_ident.clone());
            }
            if let Some(&dst_idx) = stock_idx.get(&dest_canon) {
                stocks[dst_idx].inflows.push(flow_ident);
            }

            // For Conversion, create a waste flow (outflow from source, no destination)
            if flow.flow_type == FlowType::Conversion {
                let waste_ident = format!("{source_canon}_to_{dest_canon}{suffix}_waste");
                let waste_equation = format!("{module_ident}.waste");

                variables.push(Variable::Flow(Flow {
                    ident: waste_ident.clone(),
                    equation: Equation::Scalar(waste_equation),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: Compat::default(),
                }));

                // Waste is an outflow from source only (drains to nowhere)
                if let Some(&src_idx) = stock_idx.get(&source_canon) {
                    stocks[src_idx].outflows.push(waste_ident);
                }
            }

            prev_module_ident = Some(module_ident);
        }
    }

    // Build flow_ident for each flow_idx using the pre-computed suffixes.
    let flow_ident_for_idx: HashMap<usize, String> = flow_suffixes
        .iter()
        .map(|(&idx, suffix)| {
            let flow = &model.flows[idx];
            let source_canon = canon(&flow.source);
            let dest_canon = canon(&flow.dest);
            (idx, format!("{source_canon}_to_{dest_canon}{suffix}"))
        })
        .collect();

    // Pre-compute incremental drain states for cross-stock references.
    //
    // In the Python systems package, flows process sequentially in reversed
    // declaration order. Each flow immediately drains its source stock.
    // When a flow from stock C references stock A in its rate formula, it
    // should see A's value after only the outflows processed so far, not
    // all outflows.
    //
    // We iterate flows in reversed declaration order and build cumulative
    // drain variables: `{stock}_drained_{n}` subtracts only the n outflows
    // processed up to that point. The `drain_at_flow` map records the
    // drain state at each flow's processing point, used later when
    // creating rate and capacity aux variables.
    let mut sorted_flow_idxs: Vec<usize> = (0..model.flows.len()).collect();
    sorted_flow_idxs.sort_by_key(|&idx| std::cmp::Reverse(idx));

    let mut drain_at_flow: HashMap<usize, HashMap<String, String>> = HashMap::new();
    let mut cumulative_outflows: HashMap<String, Vec<String>> = HashMap::new();
    let mut current_drain: HashMap<String, String> = HashMap::new();

    for &flow_idx in &sorted_flow_idxs {
        // Record the drain state BEFORE this flow's rate/capacity is evaluated
        drain_at_flow.insert(flow_idx, current_drain.clone());

        let flow = &model.flows[flow_idx];
        let source_canon = canon(&flow.source);
        let flow_ident = flow_ident_for_idx[&flow_idx].clone();

        let acc = cumulative_outflows.entry(source_canon.clone()).or_default();
        acc.push(flow_ident.clone());
        if flow.flow_type == FlowType::Conversion {
            let waste_ident = format!("{flow_ident}_waste");
            acc.push(waste_ident);
        }

        // Create incremental drain variable for non-infinite stocks
        let is_non_infinite = stocks
            .iter()
            .any(|s| s.ident == source_canon && s.equation != "inf()");
        if is_non_infinite {
            let drain_ident = format!("{source_canon}_drained_{}", acc.len());
            let outflow_terms = acc
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(" - ");
            let drain_eq = format!("{source_canon} - {outflow_terms}");
            variables.push(Variable::Aux(Aux {
                ident: drain_ident.clone(),
                equation: Equation::Scalar(drain_eq),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: Compat::default(),
            }));
            current_drain.insert(source_canon, drain_ident);
        }
    }

    // Create deferred rate aux variables with order-dependent rewrites.
    //
    // In the Python systems package, flows process in reversed declaration order.
    // Each flow immediately drains its source stock. Later-processed flows see
    // the drained values in their rate formulas.
    //
    // For the flow's own source stock, we use the `available_src` (the chained
    // remaining from earlier modules) rather than the raw stock or drain variable.
    // This avoids circular dependencies while correctly reflecting the
    // intermediate source value after higher-priority outflows.
    //
    // For other stocks drained by earlier flows, we use the pre-computed
    // incremental drain variables from `drain_at_flow`.
    deferred_rates.sort_by_key(|dr| std::cmp::Reverse(dr.flow_idx));
    for dr in &deferred_rates {
        let empty = HashMap::new();
        let drained_stocks = drain_at_flow.get(&dr.flow_idx).unwrap_or(&empty);
        // Build rewrites: other drained stocks -> their drain variables,
        // source stock -> available_src (chained remaining from earlier modules)
        let mut local_rewrites: HashMap<String, String> = drained_stocks.clone();
        local_rewrites.insert(dr.source_canon.clone(), dr.available_src.clone());

        let equation = rewrite_expr_to_equation(&dr.rate_expr, &local_rewrites);
        variables.push(Variable::Aux(Aux {
            ident: dr.aux_ident.clone(),
            equation: Equation::Scalar(equation),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        }));
    }

    // Create dest_capacity aux variables.
    //
    // dest_capacity = max_expr - stock + already_processed_outflows
    //
    // Only outflows from the destination stock that correspond to flows
    // already processed in the reversed declaration order (higher flow_idx)
    // are counted. Unprocessed outflows haven't freed capacity yet in the
    // Python sequential model, so including them would overstate capacity.
    //
    // When the max expression references the flow's own source stock,
    // we substitute the flow's `available` value (the chained remaining
    // from earlier modules). This gives the post-drain value without
    // creating circular dependencies.
    //
    // For references to other stocks drained by earlier flows, we use
    // the pre-computed incremental drain variables from `drain_at_flow`.

    // Map flow idents to their declaration indices for ordering checks.
    // Uses the pre-computed disambiguated flow_ident names so that
    // duplicate parallel flows are matched correctly.
    let mut flow_ident_to_idx: HashMap<String, usize> = HashMap::new();
    for (&idx, ident) in &flow_ident_for_idx {
        flow_ident_to_idx.insert(ident.clone(), idx);
        if model.flows[idx].flow_type == FlowType::Conversion {
            flow_ident_to_idx.insert(format!("{ident}_waste"), idx);
        }
    }

    deferred_capacities.sort_by_key(|dc| std::cmp::Reverse(dc.flow_idx));
    for dc in &deferred_capacities {
        let empty = HashMap::new();
        let cap_drained = drain_at_flow.get(&dc.flow_idx).unwrap_or(&empty);
        let equation = match &dc.dest_max_expr {
            None => "inf()".to_string(),
            Some(max_expr) => {
                // Build rewrites: source stock -> available_src (chained remaining),
                // plus other drained stocks -> their incremental drain variables.
                let mut max_rewrites: HashMap<String, String> = cap_drained.clone();
                max_rewrites.insert(dc.source_canon.clone(), dc.available_src.clone());
                let rewritten_max = rewrite_expr_to_equation(max_expr, &max_rewrites);

                // Collect outflow idents for the destination stock, but only
                // those from flows already processed (higher flow_idx in the
                // reversed declaration order). Unprocessed outflows haven't
                // freed capacity yet.
                let outflows: Vec<&str> = stocks
                    .iter()
                    .find(|s| s.ident == dc.dest_canon)
                    .map(|s| {
                        s.outflows
                            .iter()
                            .filter(|f| {
                                flow_ident_to_idx
                                    .get(f.as_str())
                                    .is_some_and(|&idx| idx > dc.flow_idx)
                            })
                            .map(|f| f.as_str())
                            .collect()
                    })
                    .unwrap_or_default();

                if outflows.is_empty() {
                    format!("{rewritten_max} - {}", dc.dest_canon)
                } else {
                    let outflow_sum = outflows.join(" + ");
                    format!("{rewritten_max} - {} + {outflow_sum}", dc.dest_canon)
                }
            }
        };
        variables.push(Variable::Aux(Aux {
            ident: dc.aux_ident.clone(),
            equation: Equation::Scalar(equation),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        }));
    }

    // Convert stock builders into datamodel stocks and prepend them
    let stock_vars: Vec<Variable> = stocks
        .into_iter()
        .map(|sb| {
            Variable::Stock(Stock {
                ident: sb.ident,
                equation: Equation::Scalar(sb.equation),
                documentation: String::new(),
                units: None,
                inflows: sb.inflows,
                outflows: sb.outflows,
                ai_state: None,
                uid: None,
                compat: Compat::default(),
            })
        })
        .collect();

    let mut all_variables = stock_vars;
    all_variables.extend(variables);

    let main_model = Model {
        name: "main".to_string(),
        sim_specs: None,
        variables: all_variables,
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
    };

    Ok(Project {
        name: "systems".to_string(),
        sim_specs,
        dimensions: vec![],
        units: vec![],
        models: vec![main_model],
        source: None,
        ai_information: None,
    })
}

/// Canonicalize a name: lowercase, spaces to underscores.
fn canon(name: &str) -> String {
    canonicalize(name).into_owned()
}

/// Rewrite an Expr to an equation string, substituting Ref nodes whose
/// canonical name appears in `rewrites` with the raw target string.
/// Unlike `Expr::to_equation_string`, this avoids re-canonicalizing the
/// substituted targets, preserving `.` separators in module references
/// like `module.remaining`.
///
/// Applies the same left-to-right parenthesization logic as
/// `Expr::to_equation_string`: when the left child of a BinOp has lower
/// precedence than the outer operator, it is wrapped in parentheses so
/// that the emitted equation string preserves the left-to-right evaluation
/// semantics of the systems format.
fn rewrite_expr_to_equation(expr: &Expr, rewrites: &HashMap<String, String>) -> String {
    // Returns true when the left operand of `outer_op` needs explicit
    // parentheses to preserve left-to-right evaluation order.
    fn needs_parens_in_rewrite(expr: &Expr, outer_op: BinOp) -> bool {
        match expr {
            Expr::BinOp(_, inner_op, _) => inner_op.precedence() < outer_op.precedence(),
            _ => false,
        }
    }

    match expr {
        Expr::Ref(name) => {
            let canon_name = canonicalize(name).into_owned();
            if let Some(target) = rewrites.get(&canon_name) {
                target.clone()
            } else {
                canon_name
            }
        }
        Expr::Int(n) => format!("{n}"),
        Expr::Float(f) => {
            let s = format!("{f}");
            if s.contains('.') { s } else { format!("{f}.0") }
        }
        Expr::Inf => "inf()".to_string(),
        Expr::Paren(inner) => format!("({})", rewrite_expr_to_equation(inner, rewrites)),
        Expr::BinOp(left, op, right) => {
            let left_str = if needs_parens_in_rewrite(left, *op) {
                format!("({})", rewrite_expr_to_equation(left, rewrites))
            } else {
                rewrite_expr_to_equation(left, rewrites)
            };
            let right_str = rewrite_expr_to_equation(right, rewrites);
            format!("{left_str} {op} {right_str}")
        }
    }
}

/// Intermediate builder for stocks, accumulating inflows/outflows
/// as flows are processed.
struct StockBuilder {
    ident: String,
    equation: String,
    inflows: Vec<String>,
    outflows: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::ast::{BinOp, SystemsFlow, SystemsStock};

    /// Helper to find a variable by ident in the main model.
    fn find_var<'a>(project: &'a Project, ident: &str) -> Option<&'a Variable> {
        project
            .get_model("main")
            .and_then(|m| m.variables.iter().find(|v| v.get_ident() == ident))
    }

    /// Helper to find a module variable and return its model_name.
    fn module_model_name(project: &Project, ident: &str) -> Option<String> {
        match find_var(project, ident) {
            Some(Variable::Module(m)) => Some(m.model_name.clone()),
            _ => None,
        }
    }

    /// Helper to find a module variable and return its references.
    fn module_refs(project: &Project, ident: &str) -> Vec<(String, String)> {
        match find_var(project, ident) {
            Some(Variable::Module(m)) => m
                .references
                .iter()
                .map(|r| (r.src.clone(), r.dst.clone()))
                .collect(),
            _ => vec![],
        }
    }

    /// Helper to get stock inflows.
    fn stock_inflows(project: &Project, ident: &str) -> Vec<String> {
        match find_var(project, ident) {
            Some(Variable::Stock(s)) => s.inflows.clone(),
            _ => vec![],
        }
    }

    /// Helper to get stock outflows.
    fn stock_outflows(project: &Project, ident: &str) -> Vec<String> {
        match find_var(project, ident) {
            Some(Variable::Stock(s)) => s.outflows.clone(),
            _ => vec![],
        }
    }

    /// Helper to get the scalar equation string of a variable.
    fn scalar_eqn(project: &Project, ident: &str) -> Option<String> {
        match find_var(project, ident)?.get_equation()? {
            Equation::Scalar(s) => Some(s.clone()),
            _ => None,
        }
    }

    /// Simple model: A > B @ 7 (Rate)
    fn simple_rate_model() -> SystemsModel {
        SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "A".to_string(),
                    initial: Expr::Int(10),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "B".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![SystemsFlow {
                source: "A".to_string(),
                dest: "B".to_string(),
                flow_type: FlowType::Rate,
                rate: Expr::Int(7),
            }],
        }
    }

    // -------------------------------------------------------------------
    // AC2.1: Each flow produces a stdlib module with correct model_name
    // -------------------------------------------------------------------

    #[test]
    fn ac2_1_rate_module_name() {
        let model = simple_rate_model();
        let project = translate(&model, 10).unwrap();
        assert_eq!(
            module_model_name(&project, "a_outflows"),
            Some("stdlib\u{205A}systems_rate".to_string())
        );
    }

    #[test]
    fn ac2_1_leak_module_name() {
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "A".to_string(),
                    initial: Expr::Int(10),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "B".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![SystemsFlow {
                source: "A".to_string(),
                dest: "B".to_string(),
                flow_type: FlowType::Leak,
                rate: Expr::Float(0.1),
            }],
        };
        let project = translate(&model, 10).unwrap();
        assert_eq!(
            module_model_name(&project, "a_outflows"),
            Some("stdlib\u{205A}systems_leak".to_string())
        );
    }

    #[test]
    fn ac2_1_conversion_module_name() {
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "A".to_string(),
                    initial: Expr::Int(10),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "B".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![SystemsFlow {
                source: "A".to_string(),
                dest: "B".to_string(),
                flow_type: FlowType::Conversion,
                rate: Expr::Float(0.5),
            }],
        };
        let project = translate(&model, 10).unwrap();
        assert_eq!(
            module_model_name(&project, "a_outflows"),
            Some("stdlib\u{205A}systems_conversion".to_string())
        );
    }

    // -------------------------------------------------------------------
    // AC2.2: Module references have correct src/dst bindings
    // -------------------------------------------------------------------

    #[test]
    fn ac2_2_rate_module_references() {
        let model = simple_rate_model();
        let project = translate(&model, 10).unwrap();
        let refs = module_refs(&project, "a_outflows");

        // available -> stock a
        assert!(
            refs.iter()
                .any(|(src, dst)| src == "a" && dst == "a_outflows.available"),
            "expected available binding, got {:?}",
            refs
        );
        // requested -> rate aux
        assert!(
            refs.iter()
                .any(|(src, dst)| src == "a_outflows_requested" && dst == "a_outflows.requested"),
            "expected requested binding, got {:?}",
            refs
        );
        // dest_capacity -> capacity aux
        assert!(
            refs.iter()
                .any(|(src, dst)| src == "a_outflows_dest_capacity"
                    && dst == "a_outflows.dest_capacity"),
            "expected dest_capacity binding, got {:?}",
            refs
        );

        // Verify the rate aux equation
        assert_eq!(
            scalar_eqn(&project, "a_outflows_requested"),
            Some("7".to_string())
        );
        // Verify the dest_capacity aux equation (infinite dest)
        assert_eq!(
            scalar_eqn(&project, "a_outflows_dest_capacity"),
            Some("inf()".to_string())
        );
    }

    #[test]
    fn ac2_2_leak_module_references() {
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "Emp".to_string(),
                    initial: Expr::Int(5),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "Dep".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![SystemsFlow {
                source: "Emp".to_string(),
                dest: "Dep".to_string(),
                flow_type: FlowType::Leak,
                rate: Expr::Float(0.1),
            }],
        };
        let project = translate(&model, 10).unwrap();
        let refs = module_refs(&project, "emp_outflows");

        // Leak uses "rate" port, not "requested"
        assert!(
            refs.iter()
                .any(|(src, dst)| src == "emp_outflows_rate" && dst == "emp_outflows.rate"),
            "expected rate binding for leak, got {:?}",
            refs
        );
        assert_eq!(
            scalar_eqn(&project, "emp_outflows_rate"),
            Some("0.1".to_string())
        );
    }

    // -------------------------------------------------------------------
    // AC2.3: Conversion produces waste flow
    // -------------------------------------------------------------------

    #[test]
    fn ac2_3_conversion_waste_flow() {
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "A".to_string(),
                    initial: Expr::Int(10),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "B".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![SystemsFlow {
                source: "A".to_string(),
                dest: "B".to_string(),
                flow_type: FlowType::Conversion,
                rate: Expr::Float(0.5),
            }],
        };
        let project = translate(&model, 10).unwrap();

        // Waste flow exists
        let waste = find_var(&project, "a_to_b_waste");
        assert!(waste.is_some(), "waste flow should exist");
        assert_eq!(
            scalar_eqn(&project, "a_to_b_waste"),
            Some("a_outflows.waste".to_string())
        );

        // Waste is in source stock's outflows
        let a_outflows = stock_outflows(&project, "a");
        assert!(
            a_outflows.contains(&"a_to_b_waste".to_string()),
            "waste should be in source outflows: {:?}",
            a_outflows
        );

        // Waste is NOT in any stock's inflows
        let b_inflows = stock_inflows(&project, "b");
        assert!(
            !b_inflows.contains(&"a_to_b_waste".to_string()),
            "waste should not be in dest inflows: {:?}",
            b_inflows
        );
    }

    // -------------------------------------------------------------------
    // AC2.4: Multi-outflow chaining
    // -------------------------------------------------------------------

    #[test]
    fn ac2_4_multi_outflow_chaining() {
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "A".to_string(),
                    initial: Expr::Int(10),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "B".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "C".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![
                SystemsFlow {
                    source: "A".to_string(),
                    dest: "B".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(7),
                },
                SystemsFlow {
                    source: "A".to_string(),
                    dest: "C".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(3),
                },
            ],
        };
        let project = translate(&model, 10).unwrap();

        // With reversed order: C is processed first (highest priority), then B
        // C module gets available=a (direct stock reference)
        let c_refs = module_refs(&project, "a_outflows_c");
        assert!(
            c_refs
                .iter()
                .any(|(src, dst)| src == "a" && dst == "a_outflows_c.available"),
            "C module should reference stock directly: {:?}",
            c_refs
        );

        // B module gets available from C's remaining via intermediate aux
        let b_refs = module_refs(&project, "a_outflows_b");
        assert!(
            b_refs.iter().any(
                |(src, dst)| src == "a_outflows_c_remaining" && dst == "a_outflows_b.available"
            ),
            "B module should chain from C's remaining aux: {:?}",
            b_refs
        );
        // The intermediate aux bridges the module output
        assert_eq!(
            scalar_eqn(&project, "a_outflows_c_remaining"),
            Some("a_outflows_c.remaining".to_string()),
        );
    }

    // -------------------------------------------------------------------
    // AC2.5: Chain order matches reversed declaration order
    // -------------------------------------------------------------------

    #[test]
    fn ac2_5_reversed_declaration_order() {
        // Flows declared in order [B, C]
        // Chain should be: C gets available=stock (highest priority),
        //                  B gets available=C.remaining
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "A".to_string(),
                    initial: Expr::Int(10),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "B".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "C".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![
                SystemsFlow {
                    source: "A".to_string(),
                    dest: "B".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(5),
                },
                SystemsFlow {
                    source: "A".to_string(),
                    dest: "C".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(3),
                },
            ],
        };
        let project = translate(&model, 10).unwrap();

        // C (last declared) has highest priority: available=a
        let c_refs = module_refs(&project, "a_outflows_c");
        let c_available = c_refs
            .iter()
            .find(|(_, dst)| dst == "a_outflows_c.available");
        assert_eq!(
            c_available.map(|(src, _)| src.as_str()),
            Some("a"),
            "C (last declared) should get direct stock reference"
        );

        // B (first declared) has lower priority: available from C's remaining via aux
        let b_refs = module_refs(&project, "a_outflows_b");
        let b_available = b_refs
            .iter()
            .find(|(_, dst)| dst == "a_outflows_b.available");
        assert_eq!(
            b_available.map(|(src, _)| src.as_str()),
            Some("a_outflows_c_remaining"),
            "B (first declared) should chain from C's remaining aux"
        );
    }

    // -------------------------------------------------------------------
    // AC2.6: Infinite stocks translate to stocks with equation "inf()"
    // -------------------------------------------------------------------

    #[test]
    fn ac2_6_infinite_stock() {
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "Source".to_string(),
                    initial: Expr::Inf,
                    max: Expr::Inf,
                    is_infinite: true,
                },
                SystemsStock {
                    name: "Dest".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![SystemsFlow {
                source: "Source".to_string(),
                dest: "Dest".to_string(),
                flow_type: FlowType::Rate,
                rate: Expr::Int(5),
            }],
        };
        let project = translate(&model, 10).unwrap();

        assert_eq!(
            scalar_eqn(&project, "source"),
            Some("inf()".to_string()),
            "infinite stock should have equation inf()"
        );
    }

    // -------------------------------------------------------------------
    // AC2.7: SimSpecs set correctly
    // -------------------------------------------------------------------

    #[test]
    fn ac2_7_sim_specs() {
        let model = simple_rate_model();
        let project = translate(&model, 10).unwrap();

        assert_eq!(project.sim_specs.start, 0.0);
        assert_eq!(project.sim_specs.stop, 10.0);
        assert_eq!(project.sim_specs.dt, Dt::Dt(1.0));
        assert_eq!(project.sim_specs.sim_method, SimMethod::Euler);
    }

    // -------------------------------------------------------------------
    // Dynamic max parameter: Expr::Ref in max produces correct capacity
    // -------------------------------------------------------------------

    #[test]
    fn dynamic_max_produces_capacity_expression() {
        // EngRecruiter(1, Recruiter) means max = Ref("Recruiter")
        // dest_capacity should be "recruiter - engrecruiter"
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "Recruiter".to_string(),
                    initial: Expr::Int(5),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "EngRecruiter".to_string(),
                    initial: Expr::Int(1),
                    max: Expr::Ref("Recruiter".to_string()),
                    is_infinite: false,
                },
            ],
            flows: vec![SystemsFlow {
                source: "Recruiter".to_string(),
                dest: "EngRecruiter".to_string(),
                flow_type: FlowType::Rate,
                rate: Expr::BinOp(
                    Box::new(Expr::Ref("Recruiter".to_string())),
                    BinOp::Mul,
                    Box::new(Expr::Int(2)),
                ),
            }],
        };
        let project = translate(&model, 10).unwrap();

        assert_eq!(
            scalar_eqn(&project, "recruiter_outflows_dest_capacity"),
            Some("recruiter - engrecruiter".to_string()),
            "dynamic max should produce capacity = max_expr - stock"
        );
    }

    // -------------------------------------------------------------------
    // 1.0 Conversion detection: decimal rate produces conversion module
    // -------------------------------------------------------------------

    #[test]
    fn decimal_one_point_zero_is_conversion() {
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "Departures".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "Departed".to_string(),
                    initial: Expr::Inf,
                    max: Expr::Inf,
                    is_infinite: true,
                },
            ],
            flows: vec![SystemsFlow {
                source: "Departures".to_string(),
                dest: "Departed".to_string(),
                flow_type: FlowType::Conversion,
                rate: Expr::Float(1.0),
            }],
        };
        let project = translate(&model, 10).unwrap();

        assert_eq!(
            module_model_name(&project, "departures_outflows"),
            Some("stdlib\u{205A}systems_conversion".to_string()),
            "1.0 decimal rate should produce a conversion module"
        );
    }

    // -------------------------------------------------------------------
    // Conversion flow uses "rate" port and equation references "outflow"
    // -------------------------------------------------------------------

    #[test]
    fn conversion_flow_equation_uses_outflow() {
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "A".to_string(),
                    initial: Expr::Int(10),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "B".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![SystemsFlow {
                source: "A".to_string(),
                dest: "B".to_string(),
                flow_type: FlowType::Conversion,
                rate: Expr::Float(0.5),
            }],
        };
        let project = translate(&model, 10).unwrap();

        assert_eq!(
            scalar_eqn(&project, "a_to_b"),
            Some("a_outflows.outflow".to_string()),
            "conversion flow should reference .outflow"
        );
    }

    // -------------------------------------------------------------------
    // Rate/Leak flow equation uses "actual"
    // -------------------------------------------------------------------

    #[test]
    fn rate_flow_equation_uses_actual() {
        let model = simple_rate_model();
        let project = translate(&model, 10).unwrap();

        assert_eq!(
            scalar_eqn(&project, "a_to_b"),
            Some("a_outflows.actual".to_string()),
            "rate flow should reference .actual"
        );
    }

    // ===================================================================
    // Integration tests: parse and translate example files, verify compilation
    // ===================================================================

    use crate::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
    use crate::systems::parse;

    fn read_example(name: &str) -> String {
        let path = format!(
            "{}/../../test/systems-format/{}",
            env!("CARGO_MANIFEST_DIR"),
            name
        );
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read example file {}: {}", path, e))
    }

    /// Parse, translate, and compile an example file. Panics on failure.
    fn parse_translate_compile(name: &str) -> Project {
        let contents = read_example(name);
        let model = parse(&contents).unwrap_or_else(|e| panic!("{name}: parse failed: {e:?}"));
        let project =
            translate(&model, 5).unwrap_or_else(|e| panic!("{name}: translate failed: {e:?}"));
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        let _compiled = compile_project_incremental(&db, sync.project, "main")
            .unwrap_or_else(|e| panic!("{name}: compilation failed: {e:?}"));
        project
    }

    #[test]
    fn test_translate_hiring() {
        let project = parse_translate_compile("hiring.txt");

        // [Candidates] translates to a stock with equation "inf()"
        assert_eq!(
            scalar_eqn(&project, "candidates"),
            Some("inf()".to_string()),
            "Candidates should be infinite"
        );

        // Candidates -> PhoneScreens uses systems_rate (integer rate)
        assert_eq!(
            module_model_name(&project, "candidates_outflows"),
            Some("stdlib\u{205A}systems_rate".to_string()),
            "Candidates->PhoneScreens should use systems_rate"
        );

        // PhoneScreens -> Onsites uses systems_conversion (0.5 decimal)
        assert_eq!(
            module_model_name(&project, "phonescreens_outflows"),
            Some("stdlib\u{205A}systems_conversion".to_string()),
            "PhoneScreens->Onsites should use systems_conversion"
        );

        // Employees -> Departures uses systems_leak (explicit Leak)
        assert_eq!(
            module_model_name(&project, "employees_outflows"),
            Some("stdlib\u{205A}systems_leak".to_string()),
            "Employees->Departures should use systems_leak"
        );

        // Waste flows exist for conversion flows (PhoneScreens->Onsites)
        let ps_outflows = stock_outflows(&project, "phonescreens");
        assert!(
            ps_outflows.contains(&"phonescreens_to_onsites_waste".to_string()),
            "PhoneScreens should have waste outflow: {:?}",
            ps_outflows
        );

        // Waste flow is not in any stock's inflows
        let onsites_inflows = stock_inflows(&project, "onsites");
        assert!(
            !onsites_inflows.contains(&"phonescreens_to_onsites_waste".to_string()),
            "waste flow should not be in Onsites inflows: {:?}",
            onsites_inflows
        );
    }

    #[test]
    fn test_translate_links() {
        parse_translate_compile("links.txt");
    }

    #[test]
    fn test_translate_maximums() {
        parse_translate_compile("maximums.txt");
    }

    #[test]
    fn test_translate_projects() {
        parse_translate_compile("projects.txt");
    }

    #[test]
    fn test_translate_extended_syntax() {
        parse_translate_compile("extended_syntax.txt");
    }

    // -------------------------------------------------------------------
    // dest_capacity only counts destination outflows already processed
    //
    // When computing dest_capacity for `a > b`, only outflows from b
    // that have higher flow_idx (processed earlier in reversed order)
    // should be counted.
    // -------------------------------------------------------------------

    #[test]
    fn dest_capacity_excludes_unprocessed_outflows() {
        // b(4,5) > c @ 5 (flow_idx=0, low priority)
        // a(10) > b @ 10 (flow_idx=1, high priority, processed first)
        //
        // dest_cap for a>b: b's outflow b_to_c has flow_idx=0.
        // Since 0 < 1, b_to_c is NOT yet processed -> NOT counted.
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "a".to_string(),
                    initial: Expr::Int(10),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "b".to_string(),
                    initial: Expr::Int(4),
                    max: Expr::Int(5),
                    is_infinite: false,
                },
                SystemsStock {
                    name: "c".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![
                SystemsFlow {
                    source: "b".to_string(),
                    dest: "c".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(5),
                },
                SystemsFlow {
                    source: "a".to_string(),
                    dest: "b".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(10),
                },
            ],
        };
        let project = translate(&model, 5).unwrap();
        let cap_eq = scalar_eqn(&project, "a_outflows_dest_capacity")
            .expect("a_outflows_dest_capacity should exist");
        assert_eq!(
            cap_eq, "5 - b",
            "dest_cap should not include unprocessed b_to_c outflow"
        );
    }

    #[test]
    fn dest_capacity_includes_processed_outflows() {
        // a(10) > b(0, 5) @ 10 (flow_idx=0, low priority, processed second)
        // b > c @ 5 (flow_idx=1, high priority, processed first)
        //
        // dest_cap for a>b: b's outflow b_to_c has flow_idx=1.
        // Since 1 > 0, b_to_c IS already processed -> counted.
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "a".to_string(),
                    initial: Expr::Int(10),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "b".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Int(5),
                    is_infinite: false,
                },
                SystemsStock {
                    name: "c".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![
                SystemsFlow {
                    source: "a".to_string(),
                    dest: "b".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(10),
                },
                SystemsFlow {
                    source: "b".to_string(),
                    dest: "c".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(5),
                },
            ],
        };
        let project = translate(&model, 5).unwrap();
        let cap_eq = scalar_eqn(&project, "a_outflows_dest_capacity")
            .expect("a_outflows_dest_capacity should exist");
        assert_eq!(
            cap_eq, "5 - b + b_to_c",
            "dest_cap should include already-processed b_to_c outflow"
        );
    }

    // -------------------------------------------------------------------
    // Cross-stock rate references use incremental drain
    //
    // When a flow's rate formula references a stock other than its own
    // source, and that stock has outflows, the rewrite must use the
    // incremental drain value (reflecting only outflows processed so far
    // in reversed declaration order), not the full post-drain value.
    // -------------------------------------------------------------------

    #[test]
    fn cross_stock_rate_uses_incremental_drain() {
        // Model: A(10) > B @ 1, C(2) > D @ A, A > E @ 8
        //
        // Processing in reversed declaration order: [A>E, C>D, A>B]
        // After A>E: A drained by a_to_e only.
        // When C>D processes, its rate references A. It should see A
        // after only A>E, not after both A>E and A>B.
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "A".to_string(),
                    initial: Expr::Int(10),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "B".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "C".to_string(),
                    initial: Expr::Int(2),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "D".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "E".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![
                SystemsFlow {
                    source: "A".to_string(),
                    dest: "B".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(1),
                },
                SystemsFlow {
                    source: "C".to_string(),
                    dest: "D".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Ref("A".to_string()),
                },
                SystemsFlow {
                    source: "A".to_string(),
                    dest: "E".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(8),
                },
            ],
        };
        let project = translate(&model, 5).unwrap();

        // c_outflows_requested should reference the incremental drain
        // (a - a_to_e), not the full drain (a - a_to_e - a_to_b)
        let rate_eqn = scalar_eqn(&project, "c_outflows_requested")
            .expect("c_outflows_requested should exist");
        assert_eq!(
            rate_eqn, "a_drained_1",
            "cross-stock reference should use incremental drain after A>E only"
        );
    }

    // -------------------------------------------------------------------
    // Critical: rewrite_expr_to_equation preserves left-to-right parens
    //
    // When a rate formula has mixed-precedence operators and the source
    // stock gets rewritten (e.g., `A` -> `a_effective`), the emitted
    // equation must retain the parenthesization that preserves
    // left-to-right evaluation semantics. Without the fix, `A + D * 2`
    // (parsed as `(A + D) * 2`) would emit `a_effective + d * 2`,
    // which standard math precedence evaluates as `a_effective + (d * 2)`.
    // -------------------------------------------------------------------

    // -------------------------------------------------------------------
    // Duplicate parallel flows: multiple flows from same source to same dest
    //
    // A(10) > B @ 1 and A > B @ 2 should produce distinct module/flow
    // identifiers. Without disambiguation, both get "a_outflows_b" and
    // "a_to_b", causing the second declaration to overwrite the first.
    // -------------------------------------------------------------------

    #[test]
    fn duplicate_parallel_flows_produce_distinct_idents() {
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "A".to_string(),
                    initial: Expr::Int(10),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "B".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![
                SystemsFlow {
                    source: "A".to_string(),
                    dest: "B".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(1),
                },
                SystemsFlow {
                    source: "A".to_string(),
                    dest: "B".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(2),
                },
            ],
        };
        let project = translate(&model, 5).unwrap();

        // Both flows should exist as distinct transfer flow variables
        let flow1 = find_var(&project, "a_to_b");
        let flow2 = find_var(&project, "a_to_b_2");
        assert!(flow1.is_some(), "first a_to_b flow should exist");
        assert!(
            flow2.is_some(),
            "second a_to_b flow (a_to_b_2) should exist"
        );

        // Both modules should exist with distinct idents
        let mod1 = find_var(&project, "a_outflows_b");
        let mod2 = find_var(&project, "a_outflows_b_2");
        assert!(mod1.is_some(), "first module a_outflows_b should exist");
        assert!(mod2.is_some(), "second module a_outflows_b_2 should exist");

        // Both flows should be in stock outflows
        let a_outflows = stock_outflows(&project, "a");
        assert!(
            a_outflows.contains(&"a_to_b".to_string()),
            "a_to_b should be in a's outflows: {:?}",
            a_outflows
        );
        assert!(
            a_outflows.contains(&"a_to_b_2".to_string()),
            "a_to_b_2 should be in a's outflows: {:?}",
            a_outflows
        );
    }

    #[test]
    fn duplicate_parallel_flows_simulate_correctly() {
        // A(10) > B @ 1 and A > B @ 2 should transfer 3 total per step
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "A".to_string(),
                    initial: Expr::Int(10),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "B".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![
                SystemsFlow {
                    source: "A".to_string(),
                    dest: "B".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(1),
                },
                SystemsFlow {
                    source: "A".to_string(),
                    dest: "B".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(2),
                },
            ],
        };
        let project = translate(&model, 3).unwrap();

        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        let compiled = compile_project_incremental(&db, sync.project, "main")
            .expect("duplicate parallel flows should compile");
        let mut vm = crate::Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        let results = vm.into_results();

        // After 1 step: A should lose 3 (1+2), B should gain 3
        let a_offset = results.offsets[&crate::common::Ident::new("a")];
        let b_offset = results.offsets[&crate::common::Ident::new("b")];
        let rows: Vec<&[f64]> = results.iter().collect();
        // Row 0 is initial (t=0), Row 1 is after first step (t=1)
        assert_eq!(rows[1][a_offset], 7.0, "A should be 10-3=7 after step 1");
        assert_eq!(rows[1][b_offset], 3.0, "B should be 0+3=3 after step 1");
    }

    #[test]
    fn rewrite_preserves_mixed_precedence_parens() {
        // Model: A > B @ A + D * 2
        //        A > E @ 1
        // Parsed left-to-right: "A + D * 2" => BinOp(BinOp(A, Add, D), Mul, 2)
        // With two outflows from A, E is processed first (highest priority).
        // After E is processed, A's ref in B's rate gets rewritten to a_effective.
        // The correct equation string is "(a_effective + d) * 2".
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "A".to_string(),
                    initial: Expr::Int(10),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "B".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "D".to_string(),
                    initial: Expr::Int(3),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "E".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![
                // B flow: rate = A + D * 2 (left-to-right: (A + D) * 2)
                SystemsFlow {
                    source: "A".to_string(),
                    dest: "B".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::BinOp(
                        Box::new(Expr::BinOp(
                            Box::new(Expr::Ref("A".to_string())),
                            BinOp::Add,
                            Box::new(Expr::Ref("D".to_string())),
                        )),
                        BinOp::Mul,
                        Box::new(Expr::Int(2)),
                    ),
                },
                // E flow: rate = 1 (simple, no rewrite needed)
                SystemsFlow {
                    source: "A".to_string(),
                    dest: "E".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(1),
                },
            ],
        };
        let project = translate(&model, 10).unwrap();

        // A has two outflows so chained module remaining is used for B.
        // E is processed first (last declared = highest priority, gets available=a).
        // B is processed second; its source stock `a` is rewritten to
        // `a_outflows_e_remaining` (the chained remaining after E drains A).
        // The rate expr (A + D) * 2 must retain its parentheses after rewrite.
        // Without the parenthesization fix: "a_outflows_e_remaining + d * 2"
        // With the fix:                    "(a_outflows_e_remaining + d) * 2"
        let eqn = scalar_eqn(&project, "a_outflows_b_requested")
            .expect("a_outflows_b_requested should exist");
        assert_eq!(
            eqn, "(a_outflows_e_remaining + d) * 2",
            "mixed-precedence rewrite must parenthesize the lower-precedence left subexpr"
        );
    }

    // -------------------------------------------------------------------
    // Negative dest_capacity produces reverse flows (by design)
    //
    // When a stock exceeds its dynamic max (e.g., multiple concurrent
    // inflows), negative dest_capacity causes a reverse transfer that
    // brings the stock back toward its maximum. This matches the Python
    // systems package behavior (see extended_syntax.txt test fixture).
    // -------------------------------------------------------------------

    #[test]
    fn negative_dest_capacity_produces_reverse_flow() {
        // Two sources each send 10 into B(0, 5). After step 1, B=10>5.
        // In step 2, dest_cap = 5-10 = -5, so actual = MIN(-5, ...) = -5:
        // a reverse transfer drains B back toward its max.
        let model = SystemsModel {
            stocks: vec![
                SystemsStock {
                    name: "A".to_string(),
                    initial: Expr::Int(20),
                    max: Expr::Inf,
                    is_infinite: false,
                },
                SystemsStock {
                    name: "B".to_string(),
                    initial: Expr::Int(0),
                    max: Expr::Int(5),
                    is_infinite: false,
                },
                SystemsStock {
                    name: "C".to_string(),
                    initial: Expr::Int(20),
                    max: Expr::Inf,
                    is_infinite: false,
                },
            ],
            flows: vec![
                SystemsFlow {
                    source: "A".to_string(),
                    dest: "B".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(10),
                },
                SystemsFlow {
                    source: "C".to_string(),
                    dest: "B".to_string(),
                    flow_type: FlowType::Rate,
                    rate: Expr::Int(10),
                },
            ],
        };
        let project = translate(&model, 3).unwrap();

        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        let compiled =
            compile_project_incremental(&db, sync.project, "main").expect("should compile");
        let mut vm = crate::Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        let results = vm.into_results();

        let b_off = results.offsets[&crate::common::Ident::new("b")];
        let rows: Vec<&[f64]> = results.iter().collect();

        // Step 1: B = 0 + 5 + 5 = 10 (each flow capped at dest_cap=5)
        assert_eq!(rows[1][b_off], 10.0, "B should be 10 after step 1");
        // Step 2: dest_cap = 5-10 = -5, reverse flows drain B
        assert!(
            rows[2][b_off] < rows[1][b_off],
            "B should decrease in step 2 due to reverse flow (negative dest_capacity)"
        );
    }
}
