// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Writer for the systems format.
//!
//! Reconstructs the `.txt` format from a translated `datamodel::Project`
//! by inspecting module structure, stripping synthesized variables, and
//! recovering original declaration order via chain walking.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use crate::common::Result;
use crate::datamodel::{Equation, Module, Project, Stock, Variable};

/// Systems-format module model name prefix (U+205A two dot punctuation).
const SYSTEMS_PREFIX: &str = "stdlib\u{205A}systems_";

/// Reconstructed flow information extracted from a systems module.
struct ReconstructedFlow {
    source: String,
    dest: String,
    flow_type: FlowTypeTag,
    /// The reverse-rewritten rate equation (original stock names).
    rate_equation: String,
    /// Stocks whose drain form (_effective or _drained_N) was used in
    /// the raw equation. These stocks' outflows must appear BEFORE this
    /// flow in the output to ensure the translator drains them first
    /// (in reverse processing, the draining flow runs before the
    /// referencing flow).
    effective_deps: HashSet<String>,
    /// Drainable stocks referenced in raw form (no drain variable).
    /// These stocks' outflows must appear AFTER this flow so that
    /// in reverse processing, the referencing flow runs before the
    /// draining flow (preserving the non-drained semantics).
    raw_stock_deps: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlowTypeTag {
    Rate,
    Conversion,
    Leak,
}

/// Reconstructed stock information.
struct ReconstructedStock {
    ident: String,
    initial_eq: String,
    max_eq: Option<String>,
    is_infinite: bool,
}

pub fn project_to_systems(project: &Project) -> Result<String> {
    let model = project
        .get_model("main")
        .or_else(|| project.models.first())
        .ok_or_else(|| {
            crate::common::Error::new(
                crate::common::ErrorKind::Import,
                crate::common::ErrorCode::Generic,
                Some("no model found in project".to_owned()),
            )
        })?;

    let modules: Vec<&Module> = model
        .variables
        .iter()
        .filter_map(|v| match v {
            Variable::Module(m) if m.model_name.starts_with(SYSTEMS_PREFIX) => Some(m),
            _ => None,
        })
        .collect();

    // Note: we do NOT return early for empty modules. Stock-only systems
    // models (no flows) translate to projects with stocks but zero modules.
    // The code below handles this by emitting stock declarations from the
    // final loop when no flow groups exist.

    let var_by_ident: HashMap<&str, &Variable> =
        model.variables.iter().map(|v| (v.get_ident(), v)).collect();

    let stocks: Vec<&Stock> = model
        .variables
        .iter()
        .filter_map(|v| match v {
            Variable::Stock(s) => Some(s),
            _ => None,
        })
        .collect();

    let stock_idents: HashSet<&str> = stocks.iter().map(|s| s.ident.as_str()).collect();

    let mut inflow_stocks: HashMap<&str, &str> = HashMap::new();
    for stock in &stocks {
        for inflow in &stock.inflows {
            inflow_stocks.insert(inflow.as_str(), stock.ident.as_str());
        }
    }

    // Build module info with source stock identification
    struct ModuleInfo<'a> {
        module: &'a Module,
        source_stock: String,
        available_src: String,
    }

    let mut module_infos: Vec<ModuleInfo> = Vec::new();
    for module in &modules {
        let available_ref = module
            .references
            .iter()
            .find(|r| r.dst.ends_with(".available"));

        let available_src = match available_ref {
            Some(r) => r.src.clone(),
            None => continue,
        };

        let source_stock = if stock_idents.contains(available_src.as_str()) {
            available_src.clone()
        } else {
            find_chain_source_stock(&available_src, &var_by_ident, &stock_idents)
                .unwrap_or_else(|| available_src.clone())
        };

        module_infos.push(ModuleInfo {
            module,
            source_stock,
            available_src,
        });
    }

    let mut modules_by_source: HashMap<&str, Vec<&ModuleInfo>> = HashMap::new();
    for info in &module_infos {
        modules_by_source
            .entry(info.source_stock.as_str())
            .or_default()
            .push(info);
    }

    // Build reverse-rewrite map
    let mut reverse_rewrites: HashMap<String, String> = HashMap::new();
    // Track which stock names have drain forms (_effective or _drained_N)
    let mut effective_stock_names: HashSet<String> = HashSet::new();

    for stock in &stocks {
        // Legacy _effective pattern
        let effective = format!("{}_effective", stock.ident);
        reverse_rewrites.insert(effective, stock.ident.to_string());
        effective_stock_names.insert(stock.ident.to_string());
    }

    // Add _drained_N reverse rewrites for incremental drain variables
    for &ident in var_by_ident.keys() {
        for stock in &stocks {
            let prefix = format!("{}_drained_", stock.ident);
            if ident.starts_with(&prefix) {
                reverse_rewrites.insert(ident.to_string(), stock.ident.to_string());
            }
        }
    }

    for info in &module_infos {
        let remaining_aux = format!("{}_remaining", info.module.ident);
        reverse_rewrites.insert(remaining_aux, info.source_stock.clone());
    }

    for info in &module_infos {
        if !stock_idents.contains(info.available_src.as_str()) {
            reverse_rewrites.insert(info.available_src.clone(), info.source_stock.clone());
        }
    }

    // Walk chains per source stock and collect all flows with chain-order info.
    // Each flow carries its source stock and chain position for ordering.
    let mut all_flows: Vec<ReconstructedFlow> = Vec::new();
    // Track chain boundaries: for same-source flows, flow at chain
    // position i+1 must come after position i in the output.
    let mut chain_successors: Vec<Vec<usize>> = Vec::new();

    for stock in &stocks {
        let source_ident = stock.ident.as_str();
        let group = match modules_by_source.get(source_ident) {
            Some(g) => g,
            None => continue,
        };

        let head = match group.iter().find(|info| info.available_src == source_ident) {
            Some(h) => h,
            None => continue,
        };

        let mut chain: Vec<&ModuleInfo> = vec![head];
        let mut current_module_ident = head.module.ident.as_str();

        loop {
            let remaining_aux = format!("{current_module_ident}_remaining");
            match group
                .iter()
                .find(|info| info.available_src == remaining_aux)
            {
                Some(n) => {
                    chain.push(n);
                    current_module_ident = n.module.ident.as_str();
                }
                None => break,
            }
        }

        chain.reverse();

        let chain_start = all_flows.len();
        for info in chain {
            if let Some(flow) = extract_flow(
                info.module,
                source_ident,
                &var_by_ident,
                &inflow_stocks,
                &reverse_rewrites,
                &effective_stock_names,
            ) {
                all_flows.push(flow);
            }
        }
        let chain_end = all_flows.len();

        // Add chain-order edges: flow[i] must come before flow[i+1]
        chain_successors.resize_with(chain_end, Vec::new);
        if chain_end > chain_start + 1 {
            for (i, succs) in chain_successors[chain_start..chain_end - 1]
                .iter_mut()
                .enumerate()
            {
                succs.push(chain_start + i + 1);
            }
        }
    }

    // Topological sort individual flows, respecting both drain-dependency
    // constraints and same-source chain ordering.
    let ordered_flows = topological_sort_flows(all_flows, chain_successors);

    // Reconstruct stock information
    let mut reconstructed_stocks: HashMap<&str, ReconstructedStock> = HashMap::new();
    for stock in &stocks {
        let eq_str = match &stock.equation {
            Equation::Scalar(s) => s.as_str(),
            _ => "0",
        };
        let is_infinite = eq_str == "inf()";
        let max_eq = find_stock_max(stock, &modules, &var_by_ident, &reverse_rewrites);

        reconstructed_stocks.insert(
            stock.ident.as_str(),
            ReconstructedStock {
                ident: stock.ident.clone(),
                initial_eq: eq_str.to_string(),
                max_eq,
                is_infinite,
            },
        );
    }

    // Emit output
    let mut output = String::new();
    let mut emitted_stocks: HashSet<String> = HashSet::new();

    for flow in &ordered_flows {
        let source_str = format_stock_ref(&flow.source, &reconstructed_stocks, &mut emitted_stocks);
        let dest_str = format_stock_ref(&flow.dest, &reconstructed_stocks, &mut emitted_stocks);

        let type_str = match flow.flow_type {
            FlowTypeTag::Rate => format!("Rate({})", flow.rate_equation),
            FlowTypeTag::Conversion => format!("Conversion({})", flow.rate_equation),
            FlowTypeTag::Leak => format!("Leak({})", flow.rate_equation),
        };

        writeln!(output, "{source_str} > {dest_str} @ {type_str}").unwrap();
    }

    for stock in &stocks {
        if !emitted_stocks.contains(stock.ident.as_str())
            && let Some(rs) = reconstructed_stocks.get(stock.ident.as_str())
        {
            writeln!(output, "{}", format_stock_declaration(rs)).unwrap();
        }
    }

    let output = output.trim_end().to_string();
    if output.is_empty() {
        Ok(output)
    } else {
        Ok(output + "\n")
    }
}

/// Topological sort of individual flows based on drain dependency information
/// and same-source chain ordering constraints.
///
/// Three types of ordering constraints:
/// 1. Chain order: same-source flows maintain their chain position (from
///    `chain_successors`).
/// 2. effective_deps: if a flow used a drain variable for stock S, ALL
///    flows from S must appear AFTER this flow (so S is drained by the
///    time this flow processes in reverse).
/// 3. raw_stock_deps: if a flow uses raw stock S (no drain variable), ALL
///    flows from S must appear BEFORE this flow (so S is NOT yet drained).
fn topological_sort_flows(
    flows: Vec<ReconstructedFlow>,
    chain_successors: Vec<Vec<usize>>,
) -> Vec<ReconstructedFlow> {
    if flows.len() <= 1 {
        return flows;
    }

    let n = flows.len();

    // Map source stock -> indices of flows from that stock
    let mut stock_to_flows: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, flow) in flows.iter().enumerate() {
        stock_to_flows
            .entry(flow.source.as_str())
            .or_default()
            .push(i);
    }

    // Build dependency graph: deps[i] = set of flows that must come before flow i.
    let mut deps: Vec<HashSet<usize>> = vec![HashSet::new(); n];

    // Chain ordering constraints
    for (i, succs) in chain_successors.iter().enumerate() {
        for &succ in succs {
            deps[succ].insert(i);
        }
    }

    // Drain dependency constraints
    for (i, flow) in flows.iter().enumerate() {
        // effective_deps: drain variable for S was used -> S's outflows must
        // come AFTER this flow. In the output, this flow comes first, then
        // S's flows follow.
        for dep_stock in &flow.effective_deps {
            if let Some(stock_flows) = stock_to_flows.get(dep_stock.as_str()) {
                for &sf in stock_flows {
                    if sf != i {
                        deps[sf].insert(i);
                    }
                }
            }
        }
        // raw_stock_deps: raw S was used -> S's outflows must come BEFORE
        // this flow. In the output, S's flows come first, then this flow.
        for dep_stock in &flow.raw_stock_deps {
            if let Some(stock_flows) = stock_to_flows.get(dep_stock.as_str()) {
                for &sf in stock_flows {
                    if sf != i {
                        deps[i].insert(sf);
                    }
                }
            }
        }
    }

    // Kahn's algorithm
    let mut in_degree: Vec<usize> = vec![0; n];
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, dep_set) in deps.iter().enumerate() {
        in_degree[i] = dep_set.len();
        for &dep in dep_set {
            successors[dep].push(i);
        }
    }

    // Use a stable queue (VecDeque) to preserve relative ordering for
    // flows with no ordering constraints between them.
    let mut queue: std::collections::VecDeque<usize> =
        (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut order: Vec<usize> = Vec::with_capacity(n);

    while let Some(node) = queue.pop_front() {
        order.push(node);
        for &succ in &successors[node] {
            in_degree[succ] -= 1;
            if in_degree[succ] == 0 {
                queue.push_back(succ);
            }
        }
    }

    // Fall back for cycles (shouldn't happen in valid models)
    if order.len() < n {
        let ordered: HashSet<usize> = order.iter().copied().collect();
        for i in 0..n {
            if !ordered.contains(&i) {
                order.push(i);
            }
        }
    }

    let mut indexed_flows: Vec<Option<ReconstructedFlow>> = flows.into_iter().map(Some).collect();
    let mut result = Vec::with_capacity(n);
    for idx in order {
        if let Some(flow) = indexed_flows[idx].take() {
            result.push(flow);
        }
    }
    result
}

fn find_chain_source_stock(
    available_src: &str,
    var_by_ident: &HashMap<&str, &Variable>,
    stock_idents: &HashSet<&str>,
) -> Option<String> {
    let mut current = available_src.to_string();
    for _ in 0..100 {
        let var = var_by_ident.get(current.as_str())?;
        let eq = match var {
            Variable::Aux(a) => match &a.equation {
                Equation::Scalar(s) => s.as_str(),
                _ => return None,
            },
            _ => return None,
        };

        let module_ident = eq.strip_suffix(".remaining")?;
        let module_var = var_by_ident.get(module_ident)?;
        let module = match module_var {
            Variable::Module(m) => m,
            _ => return None,
        };

        let available_ref = module
            .references
            .iter()
            .find(|r| r.dst.ends_with(".available"))?;

        if stock_idents.contains(available_ref.src.as_str()) {
            return Some(available_ref.src.clone());
        }

        current = available_ref.src.clone();
    }
    None
}

/// Extract flow information from a systems module.
///
/// Also detects which stocks were referenced via drain variables in the
/// original (pre-reverse-rewrite) equation, storing them in `effective_deps`.
fn extract_flow(
    module: &Module,
    source_stock: &str,
    var_by_ident: &HashMap<&str, &Variable>,
    inflow_stocks: &HashMap<&str, &str>,
    reverse_rewrites: &HashMap<String, String>,
    effective_stock_names: &HashSet<String>,
) -> Option<ReconstructedFlow> {
    let flow_type = if module.model_name.ends_with("systems_rate") {
        FlowTypeTag::Rate
    } else if module.model_name.ends_with("systems_conversion") {
        FlowTypeTag::Conversion
    } else if module.model_name.ends_with("systems_leak") {
        FlowTypeTag::Leak
    } else {
        return None;
    };

    let rate_port = match flow_type {
        FlowTypeTag::Rate => "requested",
        FlowTypeTag::Leak | FlowTypeTag::Conversion => "rate",
    };

    let rate_ref = module
        .references
        .iter()
        .find(|r| r.dst.ends_with(&format!(".{rate_port}")))?;

    // Get the raw equation (before reverse-rewrite)
    let raw_equation = get_raw_equation(&rate_ref.src, var_by_ident);

    // Detect which stocks were referenced via drain variables (_effective
    // or _drained_N) vs raw (not drained). This determines ordering
    // constraints for the topological sort.
    let mut effective_deps = HashSet::new();
    let mut raw_stock_deps = HashSet::new();
    let mut effective_in_eq: HashSet<String> = HashSet::new();

    for token in tokenize_idents(&raw_equation) {
        // Check for _effective suffix (legacy)
        if let Some(suffix) = token.strip_suffix("_effective")
            && effective_stock_names.contains(suffix)
        {
            effective_deps.insert(suffix.to_string());
            effective_in_eq.insert(suffix.to_string());
        }
        // Check for _drained_N pattern (incremental drain variables)
        else if let Some(pos) = token.find("_drained_") {
            let stock_name = &token[..pos];
            if effective_stock_names.contains(stock_name) {
                effective_deps.insert(stock_name.to_string());
                effective_in_eq.insert(stock_name.to_string());
            }
        }
    }

    // Second pass: find raw stock references (stocks that are drainable but
    // NOT referenced via a drain variable in this equation)
    // Replace internal inf() with systems-format inf (no parens).
    // The internal representation uses inf() but the systems parser only
    // accepts bare inf.
    let rate_equation =
        reverse_rewrite_equation(&raw_equation, reverse_rewrites).replace("inf()", "inf");
    for token in tokenize_idents(&rate_equation) {
        if effective_stock_names.contains(token)
            && !effective_in_eq.contains(token)
            && token != source_stock
        {
            raw_stock_deps.insert(token.to_string());
        }
    }

    let actual_suffix = match flow_type {
        FlowTypeTag::Rate | FlowTypeTag::Leak => "actual",
        FlowTypeTag::Conversion => "outflow",
    };

    let transfer_flow_eq = format!("{}.{actual_suffix}", module.ident);
    let dest_stock = inflow_stocks
        .iter()
        .find_map(|(flow_ident, stock_ident)| {
            let var = var_by_ident.get(flow_ident)?;
            let eq = match var {
                Variable::Flow(f) => match &f.equation {
                    Equation::Scalar(s) => s.as_str(),
                    _ => return None,
                },
                _ => return None,
            };
            if eq == transfer_flow_eq {
                Some(*stock_ident)
            } else {
                None
            }
        })
        .unwrap_or("unknown");

    Some(ReconstructedFlow {
        source: source_stock.to_string(),
        dest: dest_stock.to_string(),
        flow_type,
        rate_equation,
        effective_deps,
        raw_stock_deps,
    })
}

/// Get the raw equation string from an aux variable (without reverse-rewriting).
fn get_raw_equation(aux_ident: &str, var_by_ident: &HashMap<&str, &Variable>) -> String {
    let var = match var_by_ident.get(aux_ident) {
        Some(v) => v,
        None => return aux_ident.to_string(),
    };

    match var {
        Variable::Aux(a) => match &a.equation {
            Equation::Scalar(s) => s.clone(),
            _ => aux_ident.to_string(),
        },
        _ => aux_ident.to_string(),
    }
}

fn reverse_rewrite_equation(eq: &str, reverse_rewrites: &HashMap<String, String>) -> String {
    if reverse_rewrites.is_empty() {
        return eq.to_string();
    }

    let mut result = String::with_capacity(eq.len());
    let mut token_start = None;

    for (i, ch) in eq.char_indices() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if token_start.is_none() {
                token_start = Some(i);
            }
        } else {
            if let Some(start) = token_start.take() {
                let token = &eq[start..i];
                if let Some(replacement) = reverse_rewrites.get(token) {
                    result.push_str(replacement);
                } else {
                    result.push_str(token);
                }
            }
            result.push(ch);
        }
    }

    if let Some(start) = token_start {
        let token = &eq[start..];
        if let Some(replacement) = reverse_rewrites.get(token) {
            result.push_str(replacement);
        } else {
            result.push_str(token);
        }
    }

    result
}

/// Extract identifier tokens from an equation string.
fn tokenize_idents(eq: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (i, ch) in eq.char_indices() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if start.is_none() {
                start = Some(i);
            }
        } else if let Some(s) = start.take() {
            tokens.push(&eq[s..i]);
        }
    }
    if let Some(s) = start {
        tokens.push(&eq[s..]);
    }
    tokens
}

fn find_stock_max(
    stock: &Stock,
    modules: &[&Module],
    var_by_ident: &HashMap<&str, &Variable>,
    reverse_rewrites: &HashMap<String, String>,
) -> Option<String> {
    for module in modules {
        let dest_cap_ref = match module
            .references
            .iter()
            .find(|r| r.dst.ends_with(".dest_capacity"))
        {
            Some(r) => r,
            None => continue,
        };

        let cap_var = match var_by_ident.get(dest_cap_ref.src.as_str()) {
            Some(v) => v,
            None => continue,
        };
        let cap_eq = match cap_var {
            Variable::Aux(a) => match &a.equation {
                Equation::Scalar(s) => s.as_str(),
                _ => continue,
            },
            _ => continue,
        };

        if cap_eq == "inf()" {
            continue;
        }

        // Match " - {stock}" exactly at the LAST occurrence: the translator
        // appends " - {dest_stock}" after the max expression, so when the
        // max expression itself references the dest stock (e.g., max = A - B
        // produces "a - b - b"), the last match is the separator.
        let stock_pattern = format!(" - {}", stock.ident);
        let pattern_idx = match cap_eq.rfind(&stock_pattern) {
            Some(idx) => idx,
            None => continue,
        };
        let after = pattern_idx + stock_pattern.len();
        if after < cap_eq.len() {
            let next_byte = cap_eq.as_bytes()[after];
            if next_byte.is_ascii_alphanumeric() || next_byte == b'_' {
                continue; // prefix match, not exact
            }
        }

        let max_part = &cap_eq[..pattern_idx];
        return Some(reverse_rewrite_equation(max_part, reverse_rewrites));
    }
    None
}

fn format_stock_declaration(stock: &ReconstructedStock) -> String {
    if stock.is_infinite {
        format!("[{}]", stock.ident)
    } else if let Some(max) = &stock.max_eq {
        format!("{}({}, {})", stock.ident, stock.initial_eq, max)
    } else if stock.initial_eq != "0" && stock.initial_eq != "inf()" {
        format!("{}({})", stock.ident, stock.initial_eq)
    } else {
        stock.ident.clone()
    }
}

fn format_stock_ref(
    stock_ident: &str,
    reconstructed_stocks: &HashMap<&str, ReconstructedStock>,
    emitted_stocks: &mut HashSet<String>,
) -> String {
    let stock = match reconstructed_stocks.get(stock_ident) {
        Some(s) => s,
        None => return stock_ident.to_string(),
    };

    if emitted_stocks.contains(stock_ident) {
        if stock.is_infinite {
            format!("[{}]", stock.ident)
        } else {
            stock.ident.clone()
        }
    } else {
        emitted_stocks.insert(stock_ident.to_string());
        format_stock_declaration(stock)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems;
    use crate::systems::translate;

    fn roundtrip_write(input: &str) -> String {
        let model = systems::parse(input).unwrap();
        let project = translate::translate(&model, translate::DEFAULT_ROUNDS).unwrap();
        project_to_systems(&project).unwrap()
    }

    #[test]
    fn ac4_1_no_module_idents_in_output() {
        let input = "[A] > B @ 5\nB > C @ 3\n";
        let output = roundtrip_write(input);
        assert!(
            !output.contains("_outflows"),
            "should not contain module idents: {output}"
        );
    }

    #[test]
    fn ac4_2_no_waste_flows_in_output() {
        let input = "A > B @ Conversion(0.5)\n";
        let output = roundtrip_write(input);
        assert!(
            !output.contains("waste"),
            "should not contain waste flows: {output}"
        );
    }

    #[test]
    fn ac4_3_rate_type_reconstructed() {
        let input = "A > B @ Rate(5)\n";
        let output = roundtrip_write(input);
        assert!(
            output.contains("Rate("),
            "should contain Rate(...): {output}"
        );
    }

    #[test]
    fn ac4_3_conversion_type_reconstructed() {
        let input = "A > B @ Conversion(0.5)\n";
        let output = roundtrip_write(input);
        assert!(
            output.contains("Conversion("),
            "should contain Conversion(...): {output}"
        );
    }

    #[test]
    fn ac4_3_leak_type_reconstructed() {
        let input = "A > B @ Leak(0.1)\n";
        let output = roundtrip_write(input);
        assert!(
            output.contains("Leak("),
            "should contain Leak(...): {output}"
        );
    }

    #[test]
    fn ac4_4_multi_outflow_declaration_order() {
        let input = "[A] > B @ Rate(5)\n[A] > C @ Rate(3)\n";
        let output = roundtrip_write(input);
        let b_pos = output.find("> b").expect("B should be in output");
        let c_pos = output.find("> c").expect("C should be in output");
        assert!(b_pos < c_pos, "B before C (declaration order): {output}");
    }

    #[test]
    fn ac4_5_infinite_stock_bracket_syntax() {
        let input = "[Source] > Dest @ Rate(5)\n";
        let output = roundtrip_write(input);
        assert!(
            output.contains("[source]"),
            "should contain [source]: {output}"
        );
    }

    #[test]
    fn stock_with_initial_value() {
        let input = "A(10) > B @ Rate(5)\n";
        let output = roundtrip_write(input);
        assert!(
            output.contains("a(10)"),
            "should show initial value: {output}"
        );
    }

    #[test]
    fn leak_flow_roundtrip() {
        let input = "A > B @ Leak(0.1)\n";
        let output = roundtrip_write(input);
        assert!(output.contains("Leak("), "should contain Leak: {output}");
        assert!(output.contains("0.1"), "should contain rate 0.1: {output}");
    }

    #[test]
    fn reverse_rewrite_replaces_effective() {
        let mut rewrites = HashMap::new();
        rewrites.insert("stock_effective".to_string(), "stock".to_string());
        assert_eq!(
            reverse_rewrite_equation("stock_effective + 1", &rewrites),
            "stock + 1"
        );
    }

    #[test]
    fn reverse_rewrite_preserves_non_matching() {
        let rewrites = HashMap::new();
        assert_eq!(
            reverse_rewrite_equation("a + b * 3", &rewrites),
            "a + b * 3"
        );
    }

    #[test]
    fn stock_only_model_emits_declarations() {
        let input = "[Candidates]\nRecruiter(5)\n";
        let output = roundtrip_write(input);
        assert!(!output.is_empty(), "stock-only model should produce output");
        assert!(
            output.contains("recruiter"),
            "should contain recruiter stock: {output}"
        );
        assert!(
            output.contains("[candidates]"),
            "should contain infinite stock: {output}"
        );
    }

    #[test]
    fn inf_rate_not_emitted_as_inf_parens() {
        let input = "A > B @ inf\n";
        let output = roundtrip_write(input);
        assert!(
            !output.contains("inf()"),
            "should not contain inf(): {output}"
        );
        assert!(output.contains("inf"), "should contain inf: {output}");
    }

    #[test]
    fn max_referencing_dest_stock_roundtrips() {
        // B(0, A - B) means max = A - B. The capacity equation is
        // "a - b - b". The writer must split at the LAST " - b" to
        // reconstruct max = "a - b", not truncate to "a".
        let input = "[Source] > B(0, A - B) @ Rate(1)\nA(10)\n";
        let output = roundtrip_write(input);
        assert!(
            output.contains("a - b"),
            "should preserve max expression 'a - b': {output}"
        );
    }

    #[test]
    fn cross_source_interleaving_preserved() {
        // A(30) > X @ B, B(10) > Y @ 5, A > Z @ B
        // The B>Y flow must sit between the two A flows to preserve
        // drain semantics: A>Z sees raw B, A>X sees drained B.
        let input = "A(30) > X @ Rate(B)\nB(10) > Y @ Rate(5)\nA > Z @ Rate(B)\n";
        let output = roundtrip_write(input);

        // Verify all three flows are present
        assert!(output.contains("> x"), "flow to X missing: {output}");
        assert!(output.contains("> y"), "flow to Y missing: {output}");
        assert!(output.contains("> z"), "flow to Z missing: {output}");

        // The two A-source flows must NOT be adjacent: B>Y must be between them.
        // Find positions of the three flows.
        let x_pos = output.find("> x").expect("X flow");
        let y_pos = output.find("> y").expect("Y flow");
        let z_pos = output.find("> z").expect("Z flow");

        // One A flow before B>Y, the other after
        let a_flows_before_b = (x_pos < y_pos) as u8 + (z_pos < y_pos) as u8;
        let a_flows_after_b = (x_pos > y_pos) as u8 + (z_pos > y_pos) as u8;
        assert_eq!(
            a_flows_before_b, 1,
            "exactly one A flow should come before B>Y: {output}"
        );
        assert_eq!(
            a_flows_after_b, 1,
            "exactly one A flow should come after B>Y: {output}"
        );
    }

    /// Round-trip simulation equivalence: cross-source interleaved model
    /// must produce identical results after parse -> translate -> write -> parse.
    #[test]
    fn cross_source_interleaving_simulation_equivalence() {
        use crate::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};

        let input = "A(30) > X @ Rate(B)\nB(10) > Y @ Rate(5)\nA > Z @ Rate(B)\n";

        // Simulate original
        let model1 = crate::systems::parse(input).unwrap();
        let project1 = crate::systems::translate::translate(&model1, 5).unwrap();
        let mut db1 = SimlinDb::default();
        let sync1 = sync_from_datamodel_incremental(&mut db1, &project1, None);
        let compiled1 = compile_project_incremental(&db1, sync1.project, "main").unwrap();
        let mut vm1 = crate::Vm::new(compiled1).unwrap();
        vm1.run_to_end().unwrap();
        let results1 = vm1.into_results();

        // Round-trip and simulate
        let written = project_to_systems(&project1).unwrap();
        let model2 = crate::systems::parse(&written).unwrap();
        let project2 = crate::systems::translate::translate(&model2, 5).unwrap();
        let mut db2 = SimlinDb::default();
        let sync2 = sync_from_datamodel_incremental(&mut db2, &project2, None);
        let compiled2 = compile_project_incremental(&db2, sync2.project, "main").unwrap();
        let mut vm2 = crate::Vm::new(compiled2).unwrap();
        vm2.run_to_end().unwrap();
        let results2 = vm2.into_results();

        // Compare stock values at each timestep
        for stock in &["a", "b", "x", "y", "z"] {
            let ident = crate::common::Ident::new(stock);
            let off1 = results1.offsets[&ident];
            let off2 = results2.offsets[&ident];
            for (step, (row1, row2)) in results1.iter().zip(results2.iter()).enumerate() {
                assert_eq!(
                    row1[off1], row2[off2],
                    "stock {stock} diverges at step {step}: original={}, roundtrip={}",
                    row1[off1], row2[off2]
                );
            }
        }
    }

    #[test]
    fn multi_outflow_rate_references_restored() {
        let input = "A(5) > B @ Rate(A * 2)\nA > C @ Rate(A * 1)\n";
        let output = roundtrip_write(input);
        assert!(
            !output.contains("_remaining"),
            "should not contain _remaining: {output}"
        );
    }
}
