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

use super::ast::{Expr, FlowType, SystemsModel};

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

            // Module ident: "{source}_outflows_{dest}" or "{source}_outflows" if single
            let module_ident = if has_single_outflow {
                format!("{source_canon}_outflows")
            } else {
                format!("{source_canon}_outflows_{dest_canon}")
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
            let flow_ident = format!("{source_canon}_to_{dest_canon}");
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
                let waste_ident = format!("{source_canon}_to_{dest_canon}_waste");
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

    // Build "effective" (post-outflow) aux variables and a rewrite map.
    //
    // In the Python systems package, flows are processed sequentially and the
    // shared state dictionary is updated immediately as each flow's source is
    // drained. Later-processed flows see the decremented stock values in their
    // rate formulas. In simlin's simultaneous SD model, stocks retain their
    // start-of-step values during flow evaluation.
    //
    // To bridge this gap, we create an "{stock}_effective" aux for each
    // non-infinite stock that has outflows. Its equation is:
    //   stock - outflow1 - outflow2 - ...
    // Rate formulas that reference such stocks are rewritten to use the
    // effective variable, reproducing the Python sequential semantics.
    let mut ref_rewrites: HashMap<String, String> = HashMap::new();
    for sb in &stocks {
        if sb.outflows.is_empty() {
            continue;
        }
        // Infinite stocks (equation = "inf()") don't change with outflows
        if sb.equation == "inf()" {
            continue;
        }
        let eff_ident = format!("{}_effective", sb.ident);
        let outflow_terms: Vec<&str> = sb.outflows.iter().map(|f| f.as_str()).collect();
        let equation = format!("{} - {}", sb.ident, outflow_terms.join(" - "));
        variables.push(Variable::Aux(Aux {
            ident: eff_ident.clone(),
            equation: Equation::Scalar(equation),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        }));
        ref_rewrites.insert(sb.ident.clone(), eff_ident);
    }

    // Create deferred rate aux variables with order-dependent rewrites.
    //
    // In the Python systems package, flows process in reversed declaration order.
    // Each flow immediately drains its source stock. Later-processed flows see
    // the drained values in their rate formulas.
    //
    // For the flow's own source stock, we use the `available_src` (the chained
    // remaining from earlier modules) rather than the raw stock or _effective.
    // This avoids circular dependencies while correctly reflecting the
    // intermediate source value after higher-priority outflows.
    //
    // For other stocks drained by earlier flows, we use their _effective values.
    deferred_rates.sort_by_key(|dr| std::cmp::Reverse(dr.flow_idx));
    let mut drained_stocks: HashMap<String, String> = HashMap::new();
    for dr in &deferred_rates {
        // Build rewrites: source stock -> available_src, other drained -> effective
        let mut local_rewrites: HashMap<String, String> = drained_stocks.clone();
        // For the source stock, use the chained available value instead of
        // excluding it entirely -- this lets the rate formula see the post-drain
        // source value (including any reverse flows from negative capacity).
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
        // Mark this flow's source as drained for subsequent flows
        if let Some(eff) = ref_rewrites.get(&dr.source_canon) {
            drained_stocks.insert(dr.source_canon.clone(), eff.clone());
        }
    }

    // Create dest_capacity aux variables.
    //
    // dest_capacity = max_expr - stock + total_outflows
    //
    // When the max expression references the flow's own source stock,
    // we substitute the flow's `available` value (the chained remaining
    // from earlier modules). This gives the post-drain value without
    // creating circular dependencies: each flow in the chain sees the
    // source's remaining value after higher-priority outflows, matching
    // the Python sequential semantics.
    //
    // For references to other stocks that have been drained by earlier flows,
    // we use the same order-dependent rewrite mechanism as for rate formulas.
    deferred_capacities.sort_by_key(|dc| std::cmp::Reverse(dc.flow_idx));
    let mut cap_drained: HashMap<String, String> = HashMap::new();
    for dc in &deferred_capacities {
        let equation = match &dc.dest_max_expr {
            None => "inf()".to_string(),
            Some(max_expr) => {
                // Build rewrites: source stock -> available_src (chained remaining),
                // plus any other drained stocks -> their effective values.
                // We use rewrite_to_equation to avoid re-canonicalization of
                // generated identifiers (which may contain `.` for module refs).
                let mut max_rewrites: HashMap<String, String> = cap_drained.clone();
                max_rewrites.insert(dc.source_canon.clone(), dc.available_src.clone());
                let rewritten_max = rewrite_expr_to_equation(max_expr, &max_rewrites);

                // Collect all outflow idents for the destination stock
                let outflows: Vec<&str> = stocks
                    .iter()
                    .find(|s| s.ident == dc.dest_canon)
                    .map(|s| s.outflows.iter().map(|f| f.as_str()).collect())
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
        // Mark source as drained for other stocks' capacity checks
        if let Some(eff) = ref_rewrites.get(&dc.source_canon) {
            cap_drained.insert(dc.source_canon.clone(), eff.clone());
        }
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
fn rewrite_expr_to_equation(expr: &Expr, rewrites: &HashMap<String, String>) -> String {
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
            let left_str = rewrite_expr_to_equation(left, rewrites);
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
            "{}/../../third_party/systems/examples/{}",
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
}
