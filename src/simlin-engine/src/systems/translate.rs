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

            // Determine `available` input binding
            let available_src = match &prev_module_ident {
                None => source_canon.clone(),
                Some(prev) => format!("{prev}.remaining"),
            };

            // Determine `dest_capacity` input: create an aux if dest has a non-inf max
            let dest_stock = model.stocks.iter().find(|s| canon(&s.name) == dest_canon);
            let dest_capacity_src =
                make_dest_capacity_aux(&module_ident, dest_stock, &dest_canon, &mut variables);

            // Determine rate/requested input: create an aux for the rate expression
            let (rate_port_name, rate_src) =
                make_rate_aux(&module_ident, &flow.flow_type, &flow.rate, &mut variables);

            // Build module references
            let references = vec![
                ModuleReference {
                    src: available_src,
                    dst: format!("{module_ident}.available"),
                },
                ModuleReference {
                    src: rate_src,
                    dst: format!("{module_ident}.{rate_port_name}"),
                },
                ModuleReference {
                    src: dest_capacity_src,
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

/// Intermediate builder for stocks, accumulating inflows/outflows
/// as flows are processed.
struct StockBuilder {
    ident: String,
    equation: String,
    inflows: Vec<String>,
    outflows: Vec<String>,
}

/// Create a dest_capacity aux variable if the destination stock has a
/// non-infinite maximum. Returns the src identifier for the module reference.
fn make_dest_capacity_aux(
    module_ident: &str,
    dest_stock: Option<&super::ast::SystemsStock>,
    dest_canon: &str,
    variables: &mut Vec<Variable>,
) -> String {
    let needs_capacity_aux = dest_stock.map(|s| s.max != Expr::Inf).unwrap_or(false);

    if !needs_capacity_aux {
        // Use a simple aux with "inf()" for infinite capacity
        let aux_ident = format!("{module_ident}_dest_capacity");
        variables.push(Variable::Aux(Aux {
            ident: aux_ident.clone(),
            equation: Equation::Scalar("inf()".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        }));
        aux_ident
    } else {
        let dest_max_expr = dest_stock.unwrap().max.to_equation_string();
        let aux_ident = format!("{module_ident}_dest_capacity");
        let equation = format!("{dest_max_expr} - {dest_canon}");
        variables.push(Variable::Aux(Aux {
            ident: aux_ident.clone(),
            equation: Equation::Scalar(equation),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        }));
        aux_ident
    }
}

/// Create a rate/requested aux variable for the flow's rate expression.
/// Returns (port_name, aux_ident) where port_name is "requested" for Rate
/// and "rate" for Leak/Conversion.
fn make_rate_aux(
    module_ident: &str,
    flow_type: &FlowType,
    rate_expr: &Expr,
    variables: &mut Vec<Variable>,
) -> (String, String) {
    let port_name = match flow_type {
        FlowType::Rate => "requested",
        FlowType::Leak | FlowType::Conversion => "rate",
    };

    let aux_ident = format!("{module_ident}_{port_name}");
    let equation = rate_expr.to_equation_string();

    variables.push(Variable::Aux(Aux {
        ident: aux_ident.clone(),
        equation: Equation::Scalar(equation),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    }));

    (port_name.to_string(), aux_ident)
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

        // B module gets available=a_outflows_c.remaining (chained)
        let b_refs = module_refs(&project, "a_outflows_b");
        assert!(
            b_refs.iter().any(
                |(src, dst)| src == "a_outflows_c.remaining" && dst == "a_outflows_b.available"
            ),
            "B module should chain from C's remaining: {:?}",
            b_refs
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

        // B (first declared) has lower priority: available=a_outflows_c.remaining
        let b_refs = module_refs(&project, "a_outflows_b");
        let b_available = b_refs
            .iter()
            .find(|(_, dst)| dst == "a_outflows_b.available");
        assert_eq!(
            b_available.map(|(src, _)| src.as_str()),
            Some("a_outflows_c.remaining"),
            "B (first declared) should chain from C's remaining"
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
}
