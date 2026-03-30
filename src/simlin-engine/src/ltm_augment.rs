// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! LTM project augmentation - adds synthetic variables for link and loop scores
//!
//! This module generates synthetic variables for Loops That Matter (LTM) analysis.
//! The generated equations use the intrinsic two-argument `PREVIOUS(value, initial)`
//! function. First- and second-timestep guards are expressed explicitly with
//! `TIME = INITIAL_TIME` and `PREVIOUS(TIME, INITIAL_TIME) = INITIAL_TIME`.

use crate::ast::{Expr0, IndexExpr0, print_eqn};
use crate::builtins::UntypedBuiltinFn;
use crate::canonicalize;
use crate::common::{Canonical, Ident};
use crate::datamodel::{self, Equation};
use crate::lexer::LexerType;
use crate::ltm::{CyclePartitions, Loop, normalize_module_ref};
use crate::variable::{Variable, identifier_set};
use std::collections::{HashMap, HashSet};

/// Recursively walk an Expr0 tree, wrapping variable references that appear in
/// `deps` with `PREVIOUS(...)`.  Function names in App nodes are never touched,
/// so a variable named `max` won't corrupt `MAX(max, s)`.
fn wrap_deps_in_previous(expr: Expr0, deps: &HashSet<Ident<Canonical>>) -> Expr0 {
    match expr {
        Expr0::Var(ref ident, loc) => {
            let canonical = Ident::new(&canonicalize(ident.as_str()));
            if deps.contains(&canonical) {
                Expr0::App(UntypedBuiltinFn("PREVIOUS".to_string(), vec![expr]), loc)
            } else {
                expr
            }
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            let args = args
                .into_iter()
                .map(|a| wrap_deps_in_previous(a, deps))
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, inner, loc) => {
            Expr0::Op1(op, Box::new(wrap_deps_in_previous(*inner, deps)), loc)
        }
        Expr0::Op2(op, lhs, rhs, loc) => Expr0::Op2(
            op,
            Box::new(wrap_deps_in_previous(*lhs, deps)),
            Box::new(wrap_deps_in_previous(*rhs, deps)),
            loc,
        ),
        Expr0::If(cond, then_expr, else_expr, loc) => Expr0::If(
            Box::new(wrap_deps_in_previous(*cond, deps)),
            Box::new(wrap_deps_in_previous(*then_expr, deps)),
            Box::new(wrap_deps_in_previous(*else_expr, deps)),
            loc,
        ),
        Expr0::Subscript(ident, indices, loc) => {
            let indices = indices
                .into_iter()
                .map(|idx| wrap_index_deps_in_previous(idx, deps))
                .collect();
            let canonical = Ident::new(&canonicalize(ident.as_str()));
            let subscript = Expr0::Subscript(ident, indices, loc);
            if deps.contains(&canonical) {
                Expr0::App(
                    UntypedBuiltinFn("PREVIOUS".to_string(), vec![subscript]),
                    loc,
                )
            } else {
                subscript
            }
        }
        Expr0::Const(..) => expr,
    }
}

fn wrap_index_deps_in_previous(index: IndexExpr0, deps: &HashSet<Ident<Canonical>>) -> IndexExpr0 {
    match index {
        IndexExpr0::Expr(e) => IndexExpr0::Expr(wrap_deps_in_previous(e, deps)),
        IndexExpr0::Range(l, r, loc) => IndexExpr0::Range(
            wrap_deps_in_previous(l, deps),
            wrap_deps_in_previous(r, deps),
            loc,
        ),
        other => other,
    }
}

/// Parse an equation, wrap all dependency references except `exclude` in PREVIOUS(),
/// and return the resulting equation text.  Falls back to lowercased original text
/// if parsing fails.
fn build_partial_equation(
    equation_text: &str,
    deps: &HashSet<Ident<Canonical>>,
    exclude: &Ident<Canonical>,
) -> String {
    let deps_to_wrap: HashSet<Ident<Canonical>> = deps
        .iter()
        .filter(|d| *d != exclude && normalize_module_ref(d) != *exclude)
        .cloned()
        .collect();

    if deps_to_wrap.is_empty() {
        return equation_text.to_lowercase();
    }

    let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) else {
        return equation_text.to_lowercase();
    };

    let transformed = wrap_deps_in_previous(ast, &deps_to_wrap);
    print_eqn(&transformed)
}

/// Quote an identifier for use in an equation string.
/// Identifiers with special characters (like $, ⁚) need double quotes.
pub(crate) fn quote_ident(ident: &str) -> String {
    if ident.chars().all(|c| c.is_alphanumeric() || c == '_') {
        ident.to_string()
    } else {
        format!("\"{ident}\"")
    }
}

/// Generate loop score variables for all loops.
///
/// Absolute loop scores are unchanged (product of link scores).
/// Relative loop scores use partition-scoped denominators: each loop's
/// relative score equation only references loops in the same partition.
/// Loops with no stocks form their own unpartitioned group.
pub(crate) fn generate_loop_score_variables(
    loops: &[Loop],
    partitions: &CyclePartitions,
) -> HashMap<Ident<Canonical>, datamodel::Variable> {
    let mut loop_vars = HashMap::new();

    // First, generate absolute loop scores (unchanged)
    for loop_item in loops {
        let var_name = format!("$⁚ltm⁚loop_score⁚{}", loop_item.id);
        let equation = generate_loop_score_equation(loop_item);
        let ltm_var = create_aux_variable(&var_name, &equation);
        loop_vars.insert(Ident::new(&var_name), ltm_var);
    }

    // Group loops by partition for relative score computation
    let mut partition_groups: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (i, loop_item) in loops.iter().enumerate() {
        let partition = partitions.partition_for_loop(loop_item);
        partition_groups.entry(partition).or_default().push(i);
    }

    // Pre-collect IDs per partition group to avoid cloning Loop structs
    let group_ids: HashMap<Option<usize>, Vec<&str>> = partition_groups
        .iter()
        .map(|(&partition, indices)| {
            let ids: Vec<&str> = indices.iter().map(|&idx| loops[idx].id.as_str()).collect();
            (partition, ids)
        })
        .collect();

    // Generate relative loop scores with partition-scoped denominators
    for loop_item in loops {
        let var_name = format!("$⁚ltm⁚rel_loop_score⁚{}", loop_item.id);
        let partition = partitions.partition_for_loop(loop_item);
        let same_group_ids = &group_ids[&partition];
        let equation = generate_relative_loop_score_equation(&loop_item.id, same_group_ids);
        let ltm_var = create_aux_variable(&var_name, &equation);
        loop_vars.insert(Ident::new(&var_name), ltm_var);
    }

    loop_vars
}

/// Generate the equation for a link score variable.
/// Exposed as `generate_link_score_equation_for_link` for use by tracked
/// functions in `db.rs`.
pub(crate) fn generate_link_score_equation_for_link(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    to_var: &Variable,
    all_vars: &HashMap<Ident<Canonical>, Variable>,
) -> String {
    generate_link_score_equation(from, to, to_var, all_vars)
}

/// Generate the equation for a link score variable
fn generate_link_score_equation(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    to_var: &Variable,
    all_vars: &HashMap<Ident<Canonical>, Variable>,
) -> String {
    // Check if this is a flow-to-stock link
    let is_flow_to_stock = matches!(to_var, Variable::Stock { .. })
        && matches!(
            all_vars.get(from),
            Some(Variable::Var { is_flow: true, .. })
        );

    // Check if this is a stock-to-flow link
    let is_stock_to_flow = matches!(all_vars.get(from), Some(Variable::Stock { .. }))
        && matches!(to_var, Variable::Var { is_flow: true, .. });

    if is_flow_to_stock {
        // Use flow-to-stock formula
        generate_flow_to_stock_equation(from.as_str(), to.as_str(), to_var)
    } else if is_stock_to_flow {
        // Use stock-to-flow formula
        generate_stock_to_flow_equation(from, to, to_var, all_vars)
    } else {
        // Use standard auxiliary-to-auxiliary formula
        generate_auxiliary_to_auxiliary_equation(from, to, to_var)
    }
}

/// Generate auxiliary-to-auxiliary link score equation
fn generate_auxiliary_to_auxiliary_equation(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    to_var: &Variable,
) -> String {
    use crate::ast::Ast;

    // Get the equation text of the 'to' variable.  Prefer the AST when
    // available because the `eqn` field holds the *original* text (e.g.,
    // "SMTH1(x, 5)") while the AST holds the post-module-expansion form
    // (e.g., Var("$⁚s⁚0⁚smth1·output")).  Using the AST-derived text
    // ensures the identifiers in the equation match those in `deps`.
    let to_equation = if let Some(ast) = to_var.ast() {
        match ast {
            Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => crate::patch::expr2_to_string(expr),
            _ => match to_var {
                Variable::Stock {
                    eqn: Some(Equation::Scalar(eq)),
                    ..
                }
                | Variable::Var {
                    eqn: Some(Equation::Scalar(eq)),
                    ..
                } => eq.clone(),
                _ => "0".to_string(),
            },
        }
    } else {
        match to_var {
            Variable::Stock {
                eqn: Some(Equation::Scalar(eq)),
                ..
            }
            | Variable::Var {
                eqn: Some(Equation::Scalar(eq)),
                ..
            } => eq.clone(),
            _ => "0".to_string(),
        }
    };

    // Get dependencies of the 'to' variable
    let deps = if let Some(ast) = to_var.ast() {
        identifier_set(ast, &[], None)
    } else {
        HashSet::new()
    };

    let partial_eq = build_partial_equation(&to_equation, &deps, from);

    let from_q = quote_ident(from.as_str());
    let to_q = quote_ident(to.as_str());

    // Using SAFEDIV for both divisions
    // Note: We still need the outer check for when EITHER is zero, since we multiply the results
    let abs_part = format!(
        "ABS(SAFEDIV((({partial_eq}) - PREVIOUS({to_q})), ({to_q} - PREVIOUS({to_q})), 0))",
    );
    let sign_part = format!(
        "SIGN(SAFEDIV((({partial_eq}) - PREVIOUS({to_q})), ({from_q} - PREVIOUS({from_q})), 0))",
    );

    // Return 0 at the initial timestep when PREVIOUS values don't exist yet
    format!(
        "if \
            (TIME = INITIAL_TIME) \
            then 0 \
            else if \
                (({to_q} - PREVIOUS({to_q})) = 0) OR (({from_q} - PREVIOUS({from_q})) = 0) \
                then 0 \
                else {abs_part} * {sign_part}",
    )
}

/// Generate flow-to-stock link score equation
fn generate_flow_to_stock_equation(flow: &str, stock: &str, stock_var: &Variable) -> String {
    // Check if this flow is an inflow or outflow
    let is_inflow = if let Variable::Stock { inflows, .. } = stock_var {
        inflows.iter().any(|f| f.as_str() == flow)
    } else {
        true // Default to inflow
    };

    let sign = if is_inflow { "" } else { "-" };

    // Per the corrected 2023 formula (Schoenberg et al., Eq. 3):
    //   LS(inflow -> S)  = |Delta(i) / (Delta(S_t) - Delta(S_{t-dt}))| * (+1)
    //   LS(outflow -> S) = |Delta(o) / (Delta(S_t) - Delta(S_{t-dt}))| * (-1)
    //
    // The polarity is structural (fixed +1/-1), not dynamic.  ABS ensures
    // the magnitude is always positive; the sign is applied outside.
    //
    // The numerator uses PREVIOUS values to align timing with the denominator.
    // At time t, the flow at t-1 (PREVIOUS(flow)) is what drove the stock change from t-1 to t.
    // We measure the change in that causal flow: flow(t-1) - flow(t-2).
    let numerator = format!("(PREVIOUS({flow}) - PREVIOUS(PREVIOUS({flow})))");
    let denominator = format!(
        "(({stock} - PREVIOUS({stock})) - (PREVIOUS({stock}) - PREVIOUS(PREVIOUS({stock}))))"
    );

    // Return 0 for the first two timesteps when we don't have enough history for second-order differences
    format!(
        "if \
            (TIME = INITIAL_TIME) OR (PREVIOUS(TIME, INITIAL_TIME) = INITIAL_TIME) \
            then 0 \
            else {sign}ABS(SAFEDIV({numerator}, {denominator}, 0))"
    )
}

/// Generate stock-to-flow link score equation
fn generate_stock_to_flow_equation(
    stock: &Ident<Canonical>,
    flow: &Ident<Canonical>,
    flow_var: &Variable,
    all_vars: &HashMap<Ident<Canonical>, Variable>,
) -> String {
    // For stock-to-flow, we need to calculate how the stock influences the flow
    // This is similar to auxiliary-to-auxiliary but we know the 'from' is a stock

    // Get the flow equation text
    let flow_equation = match flow_var {
        Variable::Var {
            eqn: Some(Equation::Scalar(eq)),
            ..
        } => eq.clone(),
        _ => "0".to_string(),
    };

    // Get dependencies of the flow variable
    let deps = if let Some(ast) = flow_var.ast() {
        identifier_set(ast, &[], None)
    } else {
        HashSet::new()
    };

    let partial_eq = build_partial_equation(&flow_equation, &deps, stock);

    // Check if this flow affects the stock (is it an inflow or outflow?)
    let stock_var = all_vars.get(stock);
    let _is_affecting_stock = if let Some(Variable::Stock {
        inflows, outflows, ..
    }) = stock_var
    {
        inflows.contains(flow) || outflows.contains(flow)
    } else {
        false
    };

    // Link score formula from LTM paper: |Δxz/Δz| × sign(Δxz/Δx)
    // For stock-to-flow: x=stock, z=flow
    let flow_diff = format!("({flow} - PREVIOUS({flow}))", flow = flow.as_str());
    let stock_diff = format!("({stock} - PREVIOUS({stock}))", stock = stock.as_str());
    let partial_change = format!(
        "(({partial_eq}) - PREVIOUS({flow}))",
        partial_eq = partial_eq,
        flow = flow.as_str()
    );

    let abs_part = format!("ABS(SAFEDIV({partial_change}, {flow_diff}, 0))");
    let sign_part = format!("SIGN(SAFEDIV({partial_change}, {stock_diff}, 0))");

    // Return 0 at the initial timestep when PREVIOUS values don't exist yet
    format!(
        "if \
            (TIME = INITIAL_TIME) \
            then 0 \
            else if \
                ({flow_diff} = 0) OR ({stock_diff} = 0) \
                then 0 \
                else {abs_part} * {sign_part}"
    )
}

/// Generate the equation for a loop score variable
fn generate_loop_score_equation(loop_item: &Loop) -> String {
    // Product of all link scores in the loop
    // Use double quotes around variable names with $ to make them parseable
    let link_score_names: Vec<String> = loop_item
        .links
        .iter()
        .map(|link| {
            // Double-quote the variable name so it can be parsed
            format!(
                "\"$⁚ltm⁚link_score⁚{}→{}\"",
                link.from.as_str(),
                link.to.as_str()
            )
        })
        .collect();

    if link_score_names.is_empty() {
        "0".to_string()
    } else {
        link_score_names.join(" * ")
    }
}

/// Generate the equation for a relative loop score variable.
///
/// `same_group_ids` contains the IDs of all loops in the same partition group
/// (including this loop itself). The denominator sums only these loops.
fn generate_relative_loop_score_equation(loop_id: &str, same_group_ids: &[&str]) -> String {
    let loop_score_var = format!("\"$⁚ltm⁚loop_score⁚{loop_id}\"");

    let all_loop_scores: Vec<String> = same_group_ids
        .iter()
        .map(|id| format!("ABS(\"$⁚ltm⁚loop_score⁚{id}\")"))
        .collect();

    let sum_expr = if all_loop_scores.is_empty() {
        "1".to_string() // Avoid division by zero
    } else {
        all_loop_scores.join(" + ")
    };

    // Relative score formula using SAFEDIV for division by zero protection
    format!("SAFEDIV({loop_score_var}, ({sum_expr}), 0)")
}

/// Create an auxiliary variable with the given equation
fn create_aux_variable(name: &str, equation: &str) -> crate::datamodel::Variable {
    use crate::datamodel;

    datamodel::Variable::Aux(datamodel::Aux {
        ident: canonicalize(name).into_owned(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: "LTM".to_string(),
        units: None, // LTM scores are dimensionless by design, no need to declare
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat {
            visibility: datamodel::Visibility::Public,
            ..datamodel::Compat::default()
        },
    })
}
