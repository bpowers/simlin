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

    // Get the flow equation text.  Prefer the AST when available because
    // it handles both Scalar and ApplyToAll (arrayed) equations, whereas
    // the raw `eqn` field only covers Scalar.  Without this, arrayed flows
    // fall through to "0" and produce a zero link score.
    use crate::ast::Ast;

    let flow_equation = if let Some(ast) = flow_var.ast() {
        match ast {
            Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => crate::patch::expr2_to_string(expr),
            _ => match flow_var {
                Variable::Var {
                    eqn: Some(Equation::Scalar(eq)),
                    ..
                } => eq.clone(),
                _ => "0".to_string(),
            },
        }
    } else {
        match flow_var {
            Variable::Var {
                eqn: Some(Equation::Scalar(eq)),
                ..
            } => eq.clone(),
            _ => "0".to_string(),
        }
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

/// Classification of array-reducing builtins for cross-dimensional link score generation.
///
/// When an arrayed variable feeds a scalar target through a reducing function,
/// each element gets its own scalar link score. The reducer kind determines
/// the equation generation strategy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReducerKind {
    /// SUM, MEAN: partial derivative is algebraically simple.
    /// SUM: partial = PREVIOUS(target) + (source[d] - PREVIOUS(source[d]))
    /// MEAN: same as SUM but divided by the number of elements.
    Linear,
    /// MIN, MAX, STDDEV, RANK: must enumerate all elements explicitly,
    /// wrapping all elements except the current one in PREVIOUS.
    Nonlinear,
    /// SIZE: output is constant (depends only on dimension cardinality).
    /// Link score is always 0; skip generation entirely.
    Constant,
}

/// Collect element names from a dimension as owned strings.
///
/// For `Dimension::Named`, returns the canonical element names.
/// For `Dimension::Indexed`, returns zero-based index strings ("0", "1", ...).
pub(crate) fn dimension_element_names(dim: &crate::dimensions::Dimension) -> Vec<String> {
    match dim {
        crate::dimensions::Dimension::Named(_, named) => named
            .elements
            .iter()
            .map(|e| e.as_str().to_string())
            .collect(),
        crate::dimensions::Dimension::Indexed(_, size) => {
            (0..*size).map(|i| i.to_string()).collect()
        }
    }
}

/// Examine the target variable's Expr2 AST to find the array-reducing function
/// applied to the source variable and classify it.
///
/// Walks the Expr2 tree looking for `Expr2::App(builtin, ...)` nodes where
/// the builtin is an array reducer and the argument references the source
/// variable (identified by canonical name). Returns the `ReducerKind` and
/// the uppercase function name (e.g., "SUM", "MIN") for equation generation.
///
/// Returns `None` if no reducing builtin is found for the given source.
pub(crate) fn classify_reducer(
    target_var: &Variable,
    source_ident: &str,
) -> Option<(ReducerKind, &'static str)> {
    use crate::ast::Ast;

    let ast = target_var.ast()?;
    let expr = match ast {
        Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => expr,
        // For arrayed targets with per-element equations, check the default
        // expression if available.
        Ast::Arrayed(_, _, default_expr, _) => default_expr.as_ref()?,
    };

    classify_reducer_in_expr(expr, source_ident)
}

/// Recursively search an Expr2 tree for a reducing builtin applied to
/// the source variable.
fn classify_reducer_in_expr(
    expr: &crate::ast::Expr2,
    source_ident: &str,
) -> Option<(ReducerKind, &'static str)> {
    use crate::ast::Expr2;

    match expr {
        Expr2::App(builtin, _, _) => {
            // Check if this builtin is a reducer whose argument references
            // the source variable.
            if let Some(result) = classify_builtin_if_references_source(builtin, source_ident) {
                return Some(result);
            }
            // Even if this particular App node isn't the reducer we want,
            // recurse into its arguments to find nested reducers.
            let mut result = None;
            builtin.for_each_expr_ref(|sub_expr| {
                if result.is_none() {
                    result = classify_reducer_in_expr(sub_expr, source_ident);
                }
            });
            result
        }
        Expr2::Op1(_, inner, _, _) => classify_reducer_in_expr(inner, source_ident),
        Expr2::Op2(_, lhs, rhs, _, _) => classify_reducer_in_expr(lhs, source_ident)
            .or_else(|| classify_reducer_in_expr(rhs, source_ident)),
        Expr2::If(cond, then_e, else_e, _, _) => classify_reducer_in_expr(cond, source_ident)
            .or_else(|| classify_reducer_in_expr(then_e, source_ident))
            .or_else(|| classify_reducer_in_expr(else_e, source_ident)),
        Expr2::Var(..) | Expr2::Const(..) | Expr2::Subscript(..) => None,
    }
}

/// Check if a BuiltinFn is an array reducer and its argument references the
/// source variable. Returns the `(ReducerKind, function_name)` if so.
fn classify_builtin_if_references_source(
    builtin: &crate::builtins::BuiltinFn<crate::ast::Expr2>,
    source_ident: &str,
) -> Option<(ReducerKind, &'static str)> {
    use crate::builtins::BuiltinFn;

    let canonical_source = canonicalize(source_ident);

    match builtin {
        BuiltinFn::Sum(arg) => {
            if expr_references_var(arg, canonical_source.as_ref()) {
                Some((ReducerKind::Linear, "SUM"))
            } else {
                None
            }
        }
        BuiltinFn::Mean(args) => {
            if args
                .iter()
                .any(|a| expr_references_var(a, canonical_source.as_ref()))
            {
                Some((ReducerKind::Linear, "MEAN"))
            } else {
                None
            }
        }
        // Single-arg MIN/MAX (no second argument) is the array reducer form.
        BuiltinFn::Min(arg, None) => {
            if expr_references_var(arg, canonical_source.as_ref()) {
                Some((ReducerKind::Nonlinear, "MIN"))
            } else {
                None
            }
        }
        BuiltinFn::Max(arg, None) => {
            if expr_references_var(arg, canonical_source.as_ref()) {
                Some((ReducerKind::Nonlinear, "MAX"))
            } else {
                None
            }
        }
        BuiltinFn::Stddev(arg) => {
            if expr_references_var(arg, canonical_source.as_ref()) {
                Some((ReducerKind::Nonlinear, "STDDEV"))
            } else {
                None
            }
        }
        BuiltinFn::Rank(arg, _) => {
            if expr_references_var(arg, canonical_source.as_ref()) {
                Some((ReducerKind::Nonlinear, "RANK"))
            } else {
                None
            }
        }
        BuiltinFn::Size(arg) => {
            if expr_references_var(arg, canonical_source.as_ref()) {
                Some((ReducerKind::Constant, "SIZE"))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Check if an Expr2 references a variable with the given canonical name,
/// either directly (Var) or via subscript (Subscript).
fn expr_references_var(expr: &crate::ast::Expr2, canonical_name: &str) -> bool {
    use crate::ast::Expr2;

    match expr {
        Expr2::Var(ident, _, _) => ident.as_str() == canonical_name,
        Expr2::Subscript(ident, _, _, _) => ident.as_str() == canonical_name,
        Expr2::App(builtin, _, _) => {
            let mut found = false;
            builtin.for_each_expr_ref(|sub_expr| {
                if !found {
                    found = expr_references_var(sub_expr, canonical_name);
                }
            });
            found
        }
        Expr2::Op1(_, inner, _, _) => expr_references_var(inner, canonical_name),
        Expr2::Op2(_, lhs, rhs, _, _) => {
            expr_references_var(lhs, canonical_name) || expr_references_var(rhs, canonical_name)
        }
        Expr2::If(cond, then_e, else_e, _, _) => {
            expr_references_var(cond, canonical_name)
                || expr_references_var(then_e, canonical_name)
                || expr_references_var(else_e, canonical_name)
        }
        Expr2::Const(..) => false,
    }
}

/// Generate a per-element link score equation for an arrayed-to-scalar edge.
///
/// For element `current_element` of source variable `source_var_name`,
/// produces the partial equation where ONLY `source[current_element]` varies
/// while all other elements are held at PREVIOUS values.
///
/// `reducer_kind` determines the generation strategy:
/// - `Linear`: algebraic shortcut (SUM/MEAN) avoids enumerating all elements
/// - `Nonlinear`: explicit element expansion with selective PREVIOUS wrapping
/// - `Constant`: caller should skip generation (SIZE always produces 0)
///
/// `reducer_name` is the uppercase function name ("MIN", "MAX", "STDDEV", "RANK")
/// used for nonlinear reducers when reconstructing the function call.
pub(crate) fn generate_element_to_scalar_equation(
    source_var_name: &str,
    target_var_name: &str,
    current_element: &str,
    all_elements: &[String],
    reducer_kind: &ReducerKind,
    reducer_name: &str,
) -> String {
    let source_q = quote_ident(source_var_name);
    let target_q = quote_ident(target_var_name);
    let source_elem = format!("{source_q}[{current_element}]");

    let partial_eq = match reducer_kind {
        ReducerKind::Linear => generate_linear_partial(
            &source_q,
            &target_q,
            current_element,
            all_elements.len(),
            reducer_name,
        ),
        ReducerKind::Nonlinear => generate_nonlinear_partial(
            &source_q,
            &target_q,
            current_element,
            all_elements,
            reducer_name,
        ),
        ReducerKind::Constant => {
            // SIZE is constant; caller should not generate link scores.
            // Return a zero equation as a defensive fallback.
            return "0".to_string();
        }
    };

    // Standard link score formula wrapping the partial equation.
    let abs_part = format!(
        "ABS(SAFEDIV(({partial_eq} - PREVIOUS({target_q})), ({target_q} - PREVIOUS({target_q})), 0))"
    );
    let sign_part = format!(
        "SIGN(SAFEDIV(({partial_eq} - PREVIOUS({target_q})), ({source_elem} - PREVIOUS({source_elem})), 0))"
    );

    format!(
        "if \
            (TIME = INITIAL_TIME) \
            then 0 \
            else if \
                (({target_q} - PREVIOUS({target_q})) = 0) OR (({source_elem} - PREVIOUS({source_elem})) = 0) \
                then 0 \
                else {abs_part} * {sign_part}"
    )
}

/// Generate the partial evaluation for a linear reducer (SUM or MEAN).
///
/// SUM: PREVIOUS(target) + (source[elem] - PREVIOUS(source[elem]))
/// MEAN: PREVIOUS(target) + (source[elem] - PREVIOUS(source[elem])) / N
fn generate_linear_partial(
    source_q: &str,
    target_q: &str,
    current_element: &str,
    n_elements: usize,
    reducer_name: &str,
) -> String {
    let delta =
        format!("({source_q}[{current_element}] - PREVIOUS({source_q}[{current_element}]))");

    match reducer_name.to_uppercase().as_str() {
        "MEAN" => {
            format!("PREVIOUS({target_q}) + {delta} / {n_elements}")
        }
        // SUM is the default linear case
        _ => {
            format!("PREVIOUS({target_q}) + {delta}")
        }
    }
}

/// Generate the partial evaluation for a nonlinear reducer.
///
/// For MIN/MAX (binary), nests 2-argument calls to enumerate all elements
/// with selective PREVIOUS wrapping. For STDDEV/RANK (which only accept
/// array arguments), falls back to the target variable directly -- the
/// link score then measures the delta-ratio between the target and the
/// element, which is the best available approximation when ceteris paribus
/// decomposition is not expressible in the equation language.
fn generate_nonlinear_partial(
    source_q: &str,
    target_q: &str,
    current_element: &str,
    all_elements: &[String],
    reducer_name: &str,
) -> String {
    match reducer_name.to_uppercase().as_str() {
        "MIN" | "MAX" => {
            // Nest binary calls: MIN(a, MIN(b, MIN(c, d))) etc.
            // Each element is either current (live) or wrapped in PREVIOUS.
            let args: Vec<String> = all_elements
                .iter()
                .map(|elem| {
                    if elem == current_element {
                        format!("{source_q}[{elem}]")
                    } else {
                        format!("PREVIOUS({source_q}[{elem}])")
                    }
                })
                .collect();

            let fn_name = reducer_name.to_uppercase();
            if args.len() == 1 {
                return args[0].clone();
            }
            // Build nested binary calls from right to left:
            // MIN(a, MIN(b, c)) for [a, b, c]
            let mut result = args[args.len() - 1].clone();
            for arg in args[..args.len() - 1].iter().rev() {
                result = format!("{fn_name}({arg}, {result})");
            }
            result
        }
        _ => {
            // STDDEV, RANK: cannot decompose into per-element scalar
            // expressions because these builtins only accept array
            // arguments. Fall back to the target variable itself, which
            // gives a delta-ratio link score (how much the target
            // changed relative to how much this element changed).
            target_q.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{CanonicalDimensionName, CanonicalElementName};
    use crate::dimensions::{Dimension, NamedDimension};

    fn make_named_dimension(name: &str, elements: &[&str]) -> Dimension {
        use std::collections::HashMap;
        let canonical_elements: Vec<CanonicalElementName> = elements
            .iter()
            .map(|e| CanonicalElementName::from_raw(e))
            .collect();
        let indexed: HashMap<CanonicalElementName, usize> = canonical_elements
            .iter()
            .enumerate()
            .map(|(i, e)| (e.clone(), i))
            .collect();
        Dimension::Named(
            CanonicalDimensionName::from_raw(name),
            NamedDimension {
                elements: canonical_elements,
                indexed_elements: indexed,
                maps_to: None,
                mappings: vec![],
            },
        )
    }

    fn make_indexed_dimension(name: &str, size: u32) -> Dimension {
        Dimension::Indexed(CanonicalDimensionName::from_raw(name), size)
    }

    // -- dimension_element_names tests --

    #[test]
    fn test_dimension_element_names_named() {
        let dim = make_named_dimension("Region", &["NYC", "Boston", "LA"]);
        let names = dimension_element_names(&dim);
        assert_eq!(names, vec!["nyc", "boston", "la"]);
    }

    #[test]
    fn test_dimension_element_names_indexed() {
        let dim = make_indexed_dimension("Index", 4);
        let names = dimension_element_names(&dim);
        assert_eq!(names, vec!["0", "1", "2", "3"]);
    }

    #[test]
    fn test_dimension_element_names_empty() {
        let dim = make_named_dimension("Empty", &[]);
        let names = dimension_element_names(&dim);
        assert!(names.is_empty());
    }

    #[test]
    fn test_dimension_element_names_indexed_zero() {
        let dim = make_indexed_dimension("Zero", 0);
        let names = dimension_element_names(&dim);
        assert!(names.is_empty());
    }

    // -- ReducerKind tests --

    #[test]
    fn test_reducer_kind_equality() {
        assert_eq!(ReducerKind::Linear, ReducerKind::Linear);
        assert_eq!(ReducerKind::Nonlinear, ReducerKind::Nonlinear);
        assert_eq!(ReducerKind::Constant, ReducerKind::Constant);
        assert_ne!(ReducerKind::Linear, ReducerKind::Nonlinear);
        assert_ne!(ReducerKind::Linear, ReducerKind::Constant);
        assert_ne!(ReducerKind::Nonlinear, ReducerKind::Constant);
    }

    #[test]
    fn test_reducer_kind_clone() {
        let kind = ReducerKind::Linear;
        let cloned = kind.clone();
        assert_eq!(kind, cloned);
    }

    // -- classify_reducer tests --

    use crate::ast::{Ast, Expr2, IndexExpr2};
    use crate::builtins::{BuiltinFn, Loc};

    /// Build a Variable::Var with a hand-built Expr2 AST.
    fn var_with_expr(expr: Expr2) -> Variable {
        Variable::Var {
            ident: Ident::new("target"),
            ast: Some(Ast::Scalar(expr)),
            init_ast: None,
            eqn: None,
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        }
    }

    /// Build an Expr2 representing `var_name[*]` (subscript with wildcard).
    fn subscript_wildcard(var_name: &str) -> Expr2 {
        Expr2::Subscript(
            Ident::new(var_name),
            vec![IndexExpr2::Wildcard(Loc::default())],
            None,
            Loc::default(),
        )
    }

    /// Build an Expr2 representing a plain variable reference.
    fn var_ref(name: &str) -> Expr2 {
        Expr2::Var(Ident::new(name), None, Loc::default())
    }

    #[test]
    fn test_classify_reducer_sum() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Linear, "SUM")));
    }

    #[test]
    fn test_classify_reducer_mean() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Mean(vec![inner]), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Linear, "MEAN")));
    }

    #[test]
    fn test_classify_reducer_min() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Min(Box::new(inner), None), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Nonlinear, "MIN")));
    }

    #[test]
    fn test_classify_reducer_max() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Max(Box::new(inner), None), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Nonlinear, "MAX")));
    }

    #[test]
    fn test_classify_reducer_stddev() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Stddev(Box::new(inner)), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Nonlinear, "STDDEV")));
    }

    #[test]
    fn test_classify_reducer_rank() {
        let inner = subscript_wildcard("population");
        let direction = Expr2::Const("1".to_string(), 1.0, Loc::default());
        let expr = Expr2::App(
            BuiltinFn::Rank(Box::new(inner), Box::new(direction)),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Nonlinear, "RANK")));
    }

    #[test]
    fn test_classify_reducer_size() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Size(Box::new(inner)), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Constant, "SIZE")));
    }

    #[test]
    fn test_classify_reducer_no_reducer() {
        // A plain addition: x + y
        let expr = Expr2::Op2(
            crate::ast::BinaryOp::Add,
            Box::new(var_ref("x")),
            Box::new(var_ref("y")),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "x");
        assert_eq!(result, None);
    }

    #[test]
    fn test_classify_reducer_wrong_source() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
        let var = var_with_expr(expr);
        // Looking for a different source variable
        let result = classify_reducer(&var, "other_var");
        assert_eq!(result, None);
    }

    #[test]
    fn test_classify_reducer_nested_in_expression() {
        // 2 * SUM(population[*]) + 1
        let inner = subscript_wildcard("population");
        let sum_expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
        let two = Expr2::Const("2".to_string(), 2.0, Loc::default());
        let one = Expr2::Const("1".to_string(), 1.0, Loc::default());
        let mul = Expr2::Op2(
            crate::ast::BinaryOp::Mul,
            Box::new(two),
            Box::new(sum_expr),
            None,
            Loc::default(),
        );
        let expr = Expr2::Op2(
            crate::ast::BinaryOp::Add,
            Box::new(mul),
            Box::new(one),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Linear, "SUM")));
    }

    #[test]
    fn test_classify_reducer_var_ref_no_subscript() {
        // SUM with a plain var reference (no subscript) should still match
        let inner = var_ref("population");
        let expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Linear, "SUM")));
    }

    #[test]
    fn test_classify_reducer_no_ast() {
        // Variable without an AST
        let var: Variable = Variable::Var {
            ident: Ident::new("target"),
            ast: None,
            init_ast: None,
            eqn: None,
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };
        let result = classify_reducer(&var, "population");
        assert_eq!(result, None);
    }

    #[test]
    fn test_classify_reducer_two_arg_min_not_reducer() {
        // MIN(x, y) with two args is NOT an array reducer
        let inner1 = var_ref("population");
        let inner2 = var_ref("threshold");
        let expr = Expr2::App(
            BuiltinFn::Min(Box::new(inner1), Some(Box::new(inner2))),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, None);
    }

    #[test]
    fn test_classify_reducer_two_arg_max_not_reducer() {
        // MAX(x, y) with two args is NOT an array reducer
        let inner1 = var_ref("population");
        let inner2 = var_ref("threshold");
        let expr = Expr2::App(
            BuiltinFn::Max(Box::new(inner1), Some(Box::new(inner2))),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, None);
    }

    // -- generate_element_to_scalar_equation tests --

    #[test]
    fn test_generate_sum_equation() {
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "total_pop",
            "nyc",
            &elements,
            &ReducerKind::Linear,
            "SUM",
        );
        // Should contain the algebraic shortcut
        assert!(eq.contains("PREVIOUS(total_pop)"), "equation: {eq}");
        assert!(eq.contains("population[nyc]"), "equation: {eq}");
        assert!(eq.contains("PREVIOUS(population[nyc])"), "equation: {eq}");
        // Should not enumerate other elements (algebraic shortcut avoids them)
        assert!(
            !eq.contains("[boston]"),
            "equation should not enumerate boston: {eq}"
        );
        assert!(
            !eq.contains("[la]"),
            "equation should not enumerate la: {eq}"
        );
    }

    #[test]
    fn test_generate_mean_equation() {
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "avg_pop",
            "nyc",
            &elements,
            &ReducerKind::Linear,
            "MEAN",
        );
        // MEAN divides by N
        assert!(eq.contains("/ 3"), "equation: {eq}");
        assert!(eq.contains("PREVIOUS(avg_pop)"), "equation: {eq}");
    }

    #[test]
    fn test_generate_min_equation() {
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "min_pop",
            "nyc",
            &elements,
            &ReducerKind::Nonlinear,
            "MIN",
        );
        // Should enumerate all elements with nested binary MIN calls
        assert!(eq.contains("population[nyc]"), "equation: {eq}");
        assert!(
            eq.contains("PREVIOUS(population[boston])"),
            "equation: {eq}"
        );
        assert!(eq.contains("PREVIOUS(population[la])"), "equation: {eq}");
        // Nested binary calls: MIN(a, MIN(b, c))
        assert!(
            eq.contains(
                "MIN(population[nyc], MIN(PREVIOUS(population[boston]), PREVIOUS(population[la])))"
            ),
            "equation: {eq}"
        );
    }

    #[test]
    fn test_generate_max_equation() {
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "max_pop",
            "boston",
            &elements,
            &ReducerKind::Nonlinear,
            "MAX",
        );
        // boston is the current element, so nyc and la are wrapped
        // Nested binary calls: MAX(a, MAX(b, c))
        assert!(
            eq.contains(
                "MAX(PREVIOUS(population[nyc]), MAX(population[boston], PREVIOUS(population[la])))"
            ),
            "equation: {eq}"
        );
    }

    #[test]
    fn test_generate_constant_returns_zero() {
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "size_pop",
            "nyc",
            &elements,
            &ReducerKind::Constant,
            "SIZE",
        );
        assert_eq!(eq, "0");
    }

    #[test]
    fn test_generate_link_score_wrapping() {
        let elements = vec!["a".to_string(), "b".to_string()];
        let eq = generate_element_to_scalar_equation(
            "src",
            "tgt",
            "a",
            &elements,
            &ReducerKind::Linear,
            "SUM",
        );
        // Should have initial time guard
        assert!(eq.contains("TIME = INITIAL_TIME"), "equation: {eq}");
        // Should have zero-change guards
        assert!(eq.contains("(tgt - PREVIOUS(tgt)) = 0"), "equation: {eq}");
        assert!(
            eq.contains("(src[a] - PREVIOUS(src[a])) = 0"),
            "equation: {eq}"
        );
        // Should have ABS and SIGN parts
        assert!(eq.contains("ABS(SAFEDIV("), "equation: {eq}");
        assert!(eq.contains("SIGN(SAFEDIV("), "equation: {eq}");
    }

    #[test]
    fn test_generate_special_chars_quoted() {
        let elements = vec!["nyc".to_string()];
        let eq = generate_element_to_scalar_equation(
            "$\u{205A}ltm\u{205A}var",
            "total",
            "nyc",
            &elements,
            &ReducerKind::Linear,
            "SUM",
        );
        // Source name with special chars should be quoted
        assert!(eq.contains("\"$\u{205A}ltm\u{205A}var\""), "equation: {eq}");
    }
}
