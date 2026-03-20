// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Parser for the systems format.
//!
//! Builds a `SystemsModel` IR from systems format text input.
//! Handles stock deduplication, implicit flow type detection,
//! and declaration-order preservation.

use std::collections::HashMap;

use super::ast::{Expr, FlowType, SystemsFlow, SystemsModel, SystemsStock};
use super::lexer::{ExplicitFlowType, LexError, Line, RateExpr, StockRef, lex_lines};
use crate::common::{Error, ErrorCode, ErrorKind, Result};

/// Default initial value for a stock: `Int(0)`.
fn default_initial() -> Expr {
    Expr::Int(0)
}

/// Default maximum for a stock: `Inf`.
fn default_max() -> Expr {
    Expr::Inf
}

fn is_default_initial(expr: &Expr) -> bool {
    *expr == default_initial()
}

fn is_default_max(expr: &Expr) -> bool {
    *expr == default_max()
}

/// Determine the implicit flow type from a bare (untyped) rate expression.
///
/// The rule follows the Python implementation:
/// - A single `Float` literal implies Conversion (TOKEN_DECIMAL)
/// - Everything else (Int, Ref, BinOp, Paren, Inf) implies Rate
///
/// The presence of a decimal point is what matters, not the numeric value.
/// `@ 1.0` is Conversion, `@ 1` is Rate.
fn implicit_flow_type(expr: &Expr) -> FlowType {
    match expr {
        Expr::Float(_) => FlowType::Conversion,
        Expr::Paren(inner) => implicit_flow_type(inner),
        _ => FlowType::Rate,
    }
}

/// Convert a `LexError` into the crate's standard `Error` type.
fn lex_error_to_error(e: LexError) -> Error {
    Error::new(ErrorKind::Import, ErrorCode::Generic, Some(e.to_string()))
}

/// Parse systems format text into a `SystemsModel` intermediate representation.
///
/// The parser processes input line by line:
/// - Comment lines (starting with `#`) are skipped
/// - Stock-only lines create stocks without flows
/// - Flow lines (`A > B @ rate`) create stocks and flows
///
/// Stock deduplication: when a name appears multiple times, initial/max values
/// are updated only if the new value is non-default and the existing is default.
/// Conflicting non-default values produce an error.
pub fn parse(input: &str) -> Result<SystemsModel> {
    let lines = lex_lines(input).map_err(lex_error_to_error)?;

    let mut stocks: Vec<SystemsStock> = Vec::new();
    let mut stock_index: HashMap<String, usize> = HashMap::new();
    let mut flows: Vec<SystemsFlow> = Vec::new();

    for line in lines {
        match line {
            Line::Comment => continue,
            Line::StockOnly(stock_ref) => {
                ensure_stock(&mut stocks, &mut stock_index, &stock_ref)?;
            }
            Line::Flow(source_ref, dest_ref, rate_expr) => {
                ensure_stock(&mut stocks, &mut stock_index, &source_ref)?;
                ensure_stock(&mut stocks, &mut stock_index, &dest_ref)?;

                let (flow_type, rate) = match rate_expr {
                    RateExpr::Explicit(explicit_type, expr) => {
                        let ft = match explicit_type {
                            ExplicitFlowType::Rate => FlowType::Rate,
                            ExplicitFlowType::Conversion => FlowType::Conversion,
                            ExplicitFlowType::Leak => FlowType::Leak,
                        };
                        (ft, expr)
                    }
                    RateExpr::Implicit(expr) => {
                        let ft = implicit_flow_type(&expr);
                        (ft, expr)
                    }
                };

                // Reject Leak/Conversion from infinite sources: these produce
                // inf/NaN values because the available stock is infinite.
                // The Python systems package raises IllegalSourceStock.
                // Use the resolved stock state (not source_ref.is_infinite)
                // because the stock may have been declared infinite on an
                // earlier line without bracket syntax on this line.
                let source_is_infinite = stock_index
                    .get(&source_ref.name)
                    .map(|&idx| stocks[idx].is_infinite || stocks[idx].initial == Expr::Inf)
                    .unwrap_or(false);
                if source_is_infinite && matches!(flow_type, FlowType::Leak | FlowType::Conversion)
                {
                    return Err(Error::new(
                        ErrorKind::Import,
                        ErrorCode::Generic,
                        Some(format!(
                            "cannot use {} from infinite stock '[{}]': \
                             only Rate flows are allowed from infinite sources",
                            match flow_type {
                                FlowType::Leak => "Leak",
                                FlowType::Conversion => "Conversion",
                                FlowType::Rate => unreachable!(),
                            },
                            source_ref.name
                        )),
                    ));
                }

                flows.push(SystemsFlow {
                    source: source_ref.name.clone(),
                    dest: dest_ref.name.clone(),
                    flow_type,
                    rate,
                });
            }
            Line::FlowNoRate(source_ref, dest_ref) => {
                // Creates stocks for both sides but no flow
                ensure_stock(&mut stocks, &mut stock_index, &source_ref)?;
                ensure_stock(&mut stocks, &mut stock_index, &dest_ref)?;
            }
        }
    }

    Ok(SystemsModel { stocks, flows })
}

/// Ensure a stock exists in the model, creating it if needed. If it already
/// exists, update initial/max only when the new values are non-default and
/// the existing values are still default. Conflicting non-default values
/// produce an error.
fn ensure_stock(
    stocks: &mut Vec<SystemsStock>,
    stock_index: &mut HashMap<String, usize>,
    stock_ref: &StockRef,
) -> Result<()> {
    let new_initial = if stock_ref.params.is_empty() {
        if stock_ref.is_infinite {
            Expr::Inf
        } else {
            default_initial()
        }
    } else {
        stock_ref.params[0].clone()
    };

    let new_max = if stock_ref.params.len() >= 2 {
        stock_ref.params[1].clone()
    } else {
        default_max()
    };

    if let Some(&idx) = stock_index.get(&stock_ref.name) {
        let existing = &mut stocks[idx];

        // Update initial if new is non-default
        if !is_default_initial(&new_initial) {
            if is_default_initial(&existing.initial) {
                existing.initial = new_initial;
            } else if existing.initial != new_initial {
                return Err(Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some(format!(
                        "conflicting initial values for stock '{}'",
                        stock_ref.name
                    )),
                ));
            }
        }

        // Update max if new is non-default
        if !is_default_max(&new_max) {
            if is_default_max(&existing.max) {
                existing.max = new_max;
            } else if existing.max != new_max {
                return Err(Error::new(
                    ErrorKind::Import,
                    ErrorCode::Generic,
                    Some(format!(
                        "conflicting maximum values for stock '{}'",
                        stock_ref.name
                    )),
                ));
            }
        }

        // Update is_infinite flag if new stock is infinite
        if stock_ref.is_infinite {
            existing.is_infinite = true;
        }
    } else {
        let idx = stocks.len();
        stocks.push(SystemsStock {
            name: stock_ref.name.clone(),
            initial: new_initial,
            max: new_max,
            is_infinite: stock_ref.is_infinite,
        });
        stock_index.insert(stock_ref.name.clone(), idx);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to assert a stock exists with expected properties.
    fn assert_stock(
        model: &SystemsModel,
        name: &str,
        initial: &Expr,
        max: &Expr,
        is_infinite: bool,
    ) {
        let stock = model
            .stocks
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("stock '{}' not found in model", name));
        assert_eq!(
            &stock.initial, initial,
            "initial mismatch for stock '{}'",
            name
        );
        assert_eq!(&stock.max, max, "max mismatch for stock '{}'", name);
        assert_eq!(
            stock.is_infinite, is_infinite,
            "is_infinite mismatch for stock '{}'",
            name
        );
    }

    // -----------------------------------------------------------------------
    // AC1.1: Plain stock declaration creates stock with initial=0, max=inf
    // -----------------------------------------------------------------------

    #[test]
    fn ac1_1_plain_stock_declaration() {
        let model = parse("Name").unwrap();
        assert_eq!(model.stocks.len(), 1);
        assert_stock(&model, "Name", &Expr::Int(0), &Expr::Inf, false);
        assert!(model.flows.is_empty());
    }

    // -----------------------------------------------------------------------
    // AC1.2: Parameterized stock sets initial and max values
    // -----------------------------------------------------------------------

    #[test]
    fn ac1_2_parameterized_stock_initial_only() {
        let model = parse("Name(10)").unwrap();
        assert_eq!(model.stocks.len(), 1);
        assert_stock(&model, "Name", &Expr::Int(10), &Expr::Inf, false);
    }

    #[test]
    fn ac1_2_parameterized_stock_initial_and_max() {
        let model = parse("Name(10, 20)").unwrap();
        assert_eq!(model.stocks.len(), 1);
        assert_stock(&model, "Name", &Expr::Int(10), &Expr::Int(20), false);
    }

    // -----------------------------------------------------------------------
    // AC1.3: Infinite stock creates stock with initial=inf, show=false equiv
    // -----------------------------------------------------------------------

    #[test]
    fn ac1_3_infinite_stock() {
        let model = parse("[Name]").unwrap();
        assert_eq!(model.stocks.len(), 1);
        assert_stock(&model, "Name", &Expr::Inf, &Expr::Inf, true);
        assert!(model.flows.is_empty());
    }

    // -----------------------------------------------------------------------
    // AC1.4: Rate flow with integer produces Rate type
    // -----------------------------------------------------------------------

    #[test]
    fn ac1_4_rate_flow_integer() {
        let model = parse("A > B @ 5").unwrap();
        assert_eq!(model.flows.len(), 1);
        let flow = &model.flows[0];
        assert_eq!(flow.source, "A");
        assert_eq!(flow.dest, "B");
        assert_eq!(flow.flow_type, FlowType::Rate);
        assert_eq!(flow.rate, Expr::Int(5));
    }

    // -----------------------------------------------------------------------
    // AC1.5: Conversion flow with decimal produces Conversion type
    // -----------------------------------------------------------------------

    #[test]
    fn ac1_5_conversion_flow_decimal() {
        let model = parse("A > B @ 0.5").unwrap();
        assert_eq!(model.flows.len(), 1);
        let flow = &model.flows[0];
        assert_eq!(flow.source, "A");
        assert_eq!(flow.dest, "B");
        assert_eq!(flow.flow_type, FlowType::Conversion);
        assert_eq!(flow.rate, Expr::Float(0.5));
    }

    /// 1.0 is a decimal (TOKEN_DECIMAL), so it should be Conversion
    #[test]
    fn ac1_5_conversion_one_point_zero() {
        let model = parse("Hires > Employees(5) @ 1.0").unwrap();
        let flow = &model.flows[0];
        assert_eq!(flow.flow_type, FlowType::Conversion);
        assert_eq!(flow.rate, Expr::Float(1.0));
    }

    /// Parenthesized decimal should still be classified as Conversion
    #[test]
    fn implicit_conversion_parenthesized_float() {
        let model = parse("A > B @ (0.5)").unwrap();
        let flow = &model.flows[0];
        assert_eq!(flow.flow_type, FlowType::Conversion);
    }

    // -----------------------------------------------------------------------
    // AC1.6: Explicit flow types parse correctly
    // -----------------------------------------------------------------------

    #[test]
    fn ac1_6_explicit_rate() {
        let model = parse("A > B @ Rate(5)").unwrap();
        let flow = &model.flows[0];
        assert_eq!(flow.flow_type, FlowType::Rate);
        assert_eq!(flow.rate, Expr::Int(5));
    }

    #[test]
    fn ac1_6_explicit_conversion() {
        let model = parse("A > B @ Conversion(0.5)").unwrap();
        let flow = &model.flows[0];
        assert_eq!(flow.flow_type, FlowType::Conversion);
        assert_eq!(flow.rate, Expr::Float(0.5));
    }

    #[test]
    fn ac1_6_explicit_leak() {
        let model = parse("A > B @ Leak(0.2)").unwrap();
        let flow = &model.flows[0];
        assert_eq!(flow.flow_type, FlowType::Leak);
        assert_eq!(flow.rate, Expr::Float(0.2));
    }

    #[test]
    fn ac1_6_explicit_case_insensitive() {
        let model = parse("A > B @ rate(3)").unwrap();
        assert_eq!(model.flows[0].flow_type, FlowType::Rate);

        let model = parse("A > B @ CONVERSION(0.5)").unwrap();
        assert_eq!(model.flows[0].flow_type, FlowType::Conversion);

        let model = parse("A > B @ lEaK(0.1)").unwrap();
        assert_eq!(model.flows[0].flow_type, FlowType::Leak);
    }

    // -----------------------------------------------------------------------
    // AC1.7: Formula expressions with references, arithmetic, parentheses
    // -----------------------------------------------------------------------

    #[test]
    fn ac1_7_formula_with_reference_and_multiply() {
        let model = parse("A > B @ Recruiters * 3").unwrap();
        let flow = &model.flows[0];
        assert_eq!(flow.flow_type, FlowType::Rate);
        assert_eq!(
            flow.rate,
            Expr::BinOp(
                Box::new(Expr::Ref("Recruiters".to_owned())),
                super::super::ast::BinOp::Mul,
                Box::new(Expr::Int(3))
            )
        );
    }

    #[test]
    fn ac1_7_formula_with_division_and_parens() {
        let model = parse("A > B @ Developers / (Projects+1)").unwrap();
        let flow = &model.flows[0];
        assert_eq!(flow.flow_type, FlowType::Rate);
        // Left-to-right: Developers / (Projects + 1)
        assert_eq!(
            flow.rate,
            Expr::BinOp(
                Box::new(Expr::Ref("Developers".to_owned())),
                super::super::ast::BinOp::Div,
                Box::new(Expr::Paren(Box::new(Expr::BinOp(
                    Box::new(Expr::Ref("Projects".to_owned())),
                    super::super::ast::BinOp::Add,
                    Box::new(Expr::Int(1))
                ))))
            )
        );
    }

    // -----------------------------------------------------------------------
    // AC1.8: Comment lines are ignored
    // -----------------------------------------------------------------------

    #[test]
    fn ac1_8_comments_ignored() {
        let input = "# this is a comment\nA > B @ 5\n# another comment";
        let model = parse(input).unwrap();
        assert_eq!(model.stocks.len(), 2);
        assert_eq!(model.flows.len(), 1);
        assert_eq!(model.flows[0].source, "A");
    }

    // -----------------------------------------------------------------------
    // AC1.9: Stock-only lines create stocks but no flow
    // -----------------------------------------------------------------------

    #[test]
    fn ac1_9_stock_only_no_flow() {
        let model = parse("Name").unwrap();
        assert_eq!(model.stocks.len(), 1);
        assert!(model.flows.is_empty());
    }

    #[test]
    fn ac1_9_stock_only_with_params() {
        let model = parse("Name(5)").unwrap();
        assert_eq!(model.stocks.len(), 1);
        assert_stock(&model, "Name", &Expr::Int(5), &Expr::Inf, false);
        assert!(model.flows.is_empty());
    }

    // -----------------------------------------------------------------------
    // AC1.10: Stock initialized at later reference resolves correctly
    // -----------------------------------------------------------------------

    #[test]
    fn ac1_10_later_initialization() {
        let input = "a > b @ 5\nb(2) > c @ 3";
        let model = parse(input).unwrap();
        // b should have initial=2 (set by second line, first line had default)
        assert_stock(&model, "b", &Expr::Int(2), &Expr::Inf, false);
        assert_stock(&model, "a", &Expr::Int(0), &Expr::Inf, false);
        assert_stock(&model, "c", &Expr::Int(0), &Expr::Inf, false);
        assert_eq!(model.flows.len(), 2);
    }

    // -----------------------------------------------------------------------
    // AC1.11: Duplicate stock initialization with conflicting values -> error
    // -----------------------------------------------------------------------

    #[test]
    fn ac1_11_conflicting_initial_values() {
        let input = "a(5) > b @ 1\na(10) > c @ 2";
        let result = parse(input);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.details.as_ref().unwrap().contains("conflicting"),
            "expected conflicting error, got: {:?}",
            err
        );
    }

    #[test]
    fn ac1_11_conflicting_max_values() {
        let input = "a(0, 5) > b @ 1\na(0, 10) > c @ 2";
        let result = parse(input);
        assert!(result.is_err());
    }

    /// Same non-default value repeated is NOT a conflict
    #[test]
    fn ac1_11_same_value_not_conflicting() {
        let input = "a(5) > b @ 1\na(5) > c @ 2";
        let model = parse(input).unwrap();
        assert_stock(&model, "a", &Expr::Int(5), &Expr::Inf, false);
    }

    // -----------------------------------------------------------------------
    // AC7.2: Parenthesized formulas translate correctly
    // -----------------------------------------------------------------------

    #[test]
    fn ac7_2_parenthesized_formula() {
        let model = parse("A > B @ (a + b) / 2").unwrap();
        let flow = &model.flows[0];
        assert_eq!(
            flow.rate,
            Expr::BinOp(
                Box::new(Expr::Paren(Box::new(Expr::BinOp(
                    Box::new(Expr::Ref("a".to_owned())),
                    super::super::ast::BinOp::Add,
                    Box::new(Expr::Ref("b".to_owned()))
                )))),
                super::super::ast::BinOp::Div,
                Box::new(Expr::Int(2))
            )
        );
    }

    // -----------------------------------------------------------------------
    // Example file tests: parse real systems format files
    // -----------------------------------------------------------------------

    fn read_example(name: &str) -> String {
        let path = format!(
            "{}/../../test/systems-format/{}",
            env!("CARGO_MANIFEST_DIR"),
            name
        );
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read example file {}: {}", path, e))
    }

    /// hiring.txt: infinite stocks, implicit Conversion/Rate, explicit Leak, stock params
    #[test]
    fn example_hiring() {
        let input = read_example("hiring.txt");
        let model = parse(&input).unwrap();

        // Stocks: Candidates (infinite), PhoneScreens, Onsites, Offers,
        //         Hires, Employees(5), Departures, Departed (infinite)
        assert_stock(&model, "Candidates", &Expr::Inf, &Expr::Inf, true);
        assert_stock(&model, "PhoneScreens", &Expr::Int(0), &Expr::Inf, false);
        assert_stock(&model, "Onsites", &Expr::Int(0), &Expr::Inf, false);
        assert_stock(&model, "Offers", &Expr::Int(0), &Expr::Inf, false);
        assert_stock(&model, "Hires", &Expr::Int(0), &Expr::Inf, false);
        assert_stock(&model, "Employees", &Expr::Int(5), &Expr::Inf, false);
        assert_stock(&model, "Departures", &Expr::Int(0), &Expr::Inf, false);
        assert_stock(&model, "Departed", &Expr::Inf, &Expr::Inf, true);

        // 7 flows
        assert_eq!(model.flows.len(), 7);

        // [Candidates] > PhoneScreens @ 25 -> Rate
        assert_eq!(model.flows[0].source, "Candidates");
        assert_eq!(model.flows[0].dest, "PhoneScreens");
        assert_eq!(model.flows[0].flow_type, FlowType::Rate);
        assert_eq!(model.flows[0].rate, Expr::Int(25));

        // PhoneScreens > Onsites @ 0.5 -> Conversion
        assert_eq!(model.flows[1].flow_type, FlowType::Conversion);
        assert_eq!(model.flows[1].rate, Expr::Float(0.5));

        // Employees > Departures @ Leak(0.1)
        assert_eq!(model.flows[5].flow_type, FlowType::Leak);
        assert_eq!(model.flows[5].rate, Expr::Float(0.1));

        // Departures > [Departed] @ 1.0 -> Conversion (decimal)
        assert_eq!(model.flows[6].flow_type, FlowType::Conversion);
        assert_eq!(model.flows[6].rate, Expr::Float(1.0));
    }

    /// links.txt: formula references (Recruiters * 3), stock with (initial, max)
    #[test]
    fn example_links() {
        let input = read_example("links.txt");
        let model = parse(&input).unwrap();

        // Recruiters(10, 15)
        assert_stock(&model, "Recruiters", &Expr::Int(10), &Expr::Int(15), false);

        // Second flow: [Candidates] > PhoneScreens @ Recruiters * 3
        assert_eq!(model.flows[1].source, "Candidates");
        assert_eq!(model.flows[1].dest, "PhoneScreens");
        assert_eq!(model.flows[1].flow_type, FlowType::Rate);
        assert_eq!(
            model.flows[1].rate,
            Expr::BinOp(
                Box::new(Expr::Ref("Recruiters".to_owned())),
                super::super::ast::BinOp::Mul,
                Box::new(Expr::Int(3))
            )
        );
    }

    /// maximums.txt: (initial, max) on both source and dest
    #[test]
    fn example_maximums() {
        let input = read_example("maximums.txt");
        let model = parse(&input).unwrap();

        // [a] -> infinite
        assert_stock(&model, "a", &Expr::Inf, &Expr::Inf, true);

        // b(0, 5)
        assert_stock(&model, "b", &Expr::Int(0), &Expr::Int(5), false);

        // c(0, 10)
        assert_stock(&model, "c", &Expr::Int(0), &Expr::Int(10), false);

        assert_eq!(model.flows.len(), 2);
    }

    /// extended_syntax.txt: stock-only lines, explicit Rate, formula max in stock params
    #[test]
    fn example_extended_syntax() {
        let input = read_example("extended_syntax.txt");
        let model = parse(&input).unwrap();

        // [Candidate] appears as a stock-only line and then as a flow source
        assert_stock(&model, "Candidate", &Expr::Inf, &Expr::Inf, true);

        // Recruiter(5)
        assert_stock(&model, "Recruiter", &Expr::Int(5), &Expr::Inf, false);

        // EngRecruiter(1, Recruiter) -- max is a reference
        assert_stock(
            &model,
            "EngRecruiter",
            &Expr::Int(1),
            &Expr::Ref("Recruiter".to_owned()),
            false,
        );

        // MgrRecruiter(1, Recruiter)
        assert_stock(
            &model,
            "MgrRecruiter",
            &Expr::Int(1),
            &Expr::Ref("Recruiter".to_owned()),
            false,
        );

        // Explicit Rate flows
        let recruiter_to_eng = model
            .flows
            .iter()
            .find(|f| f.dest == "EngRecruiter")
            .unwrap();
        assert_eq!(recruiter_to_eng.flow_type, FlowType::Rate);
        assert_eq!(
            recruiter_to_eng.rate,
            Expr::BinOp(
                Box::new(Expr::Ref("Recruiter".to_owned())),
                super::super::ast::BinOp::Mul,
                Box::new(Expr::Int(2))
            )
        );
    }

    /// projects.txt: complex formulas with division and parenthesized sub-expressions
    #[test]
    fn example_projects() {
        let input = read_example("projects.txt");
        let model = parse(&input).unwrap();

        // [Ideas] > Projects @ Developers / (Projects+1)
        let ideas_to_projects = model.flows.iter().find(|f| f.source == "Ideas").unwrap();
        assert_eq!(ideas_to_projects.flow_type, FlowType::Rate);
        // Developers / (Projects+1) -- left-to-right
        assert_eq!(
            ideas_to_projects.rate,
            Expr::BinOp(
                Box::new(Expr::Ref("Developers".to_owned())),
                super::super::ast::BinOp::Div,
                Box::new(Expr::Paren(Box::new(Expr::BinOp(
                    Box::new(Expr::Ref("Projects".to_owned())),
                    super::super::ast::BinOp::Add,
                    Box::new(Expr::Int(1))
                ))))
            )
        );

        // Projects > Started @ Developers - (Started+1)
        let projects_to_started = model
            .flows
            .iter()
            .find(|f| f.source == "Projects" && f.dest == "Started")
            .unwrap();
        assert_eq!(
            projects_to_started.rate,
            Expr::BinOp(
                Box::new(Expr::Ref("Developers".to_owned())),
                super::super::ast::BinOp::Sub,
                Box::new(Expr::Paren(Box::new(Expr::BinOp(
                    Box::new(Expr::Ref("Started".to_owned())),
                    super::super::ast::BinOp::Add,
                    Box::new(Expr::Int(1))
                ))))
            )
        );
    }

    // -----------------------------------------------------------------------
    // Edge cases and additional coverage
    // -----------------------------------------------------------------------

    #[test]
    fn empty_input() {
        let model = parse("").unwrap();
        assert!(model.stocks.is_empty());
        assert!(model.flows.is_empty());
    }

    #[test]
    fn whitespace_only_input() {
        let model = parse("  \n  \n  ").unwrap();
        assert!(model.stocks.is_empty());
        assert!(model.flows.is_empty());
    }

    #[test]
    fn comments_only() {
        let model = parse("# comment 1\n# comment 2").unwrap();
        assert!(model.stocks.is_empty());
        assert!(model.flows.is_empty());
    }

    #[test]
    fn multiple_stock_only_lines() {
        let model = parse("[Candidate]\nRecruiter(5)\nEngRecruiter(1, Recruiter)").unwrap();
        assert_eq!(model.stocks.len(), 3);
        assert_stock(&model, "Candidate", &Expr::Inf, &Expr::Inf, true);
        assert_stock(&model, "Recruiter", &Expr::Int(5), &Expr::Inf, false);
        assert_stock(
            &model,
            "EngRecruiter",
            &Expr::Int(1),
            &Expr::Ref("Recruiter".to_owned()),
            false,
        );
    }

    /// Declaration order is preserved in the stocks vec
    #[test]
    fn declaration_order_preserved() {
        let input = "C > D @ 1\nA > B @ 2";
        let model = parse(input).unwrap();
        assert_eq!(model.stocks[0].name, "C");
        assert_eq!(model.stocks[1].name, "D");
        assert_eq!(model.stocks[2].name, "A");
        assert_eq!(model.stocks[3].name, "B");
        assert_eq!(model.flows[0].source, "C");
        assert_eq!(model.flows[1].source, "A");
    }

    /// Flow with formula containing multiple operators (left-to-right)
    #[test]
    fn formula_multiple_operators_left_to_right() {
        let model = parse("A > B @ 1 + 2 * 3").unwrap();
        let flow = &model.flows[0];
        // Left-to-right: (1 + 2) * 3
        assert_eq!(
            flow.rate,
            Expr::BinOp(
                Box::new(Expr::BinOp(
                    Box::new(Expr::Int(1)),
                    super::super::ast::BinOp::Add,
                    Box::new(Expr::Int(2))
                )),
                super::super::ast::BinOp::Mul,
                Box::new(Expr::Int(3))
            )
        );
    }

    /// Implicit type detection: formula with reference is Rate (not Conversion)
    #[test]
    fn implicit_type_reference_is_rate() {
        let model = parse("A > B @ SomeName").unwrap();
        assert_eq!(model.flows[0].flow_type, FlowType::Rate);
    }

    /// Implicit type detection: formula with operators is Rate
    #[test]
    fn implicit_type_complex_formula_is_rate() {
        let model = parse("A > B @ a + b").unwrap();
        assert_eq!(model.flows[0].flow_type, FlowType::Rate);
    }

    /// Negative integer in stock params
    #[test]
    fn negative_integer_param() {
        let model = parse("A(-5)").unwrap();
        assert_stock(&model, "A", &Expr::Int(-5), &Expr::Inf, false);
    }

    /// Stock with inf as parameter
    #[test]
    fn inf_as_formula() {
        let model = parse("A > B @ inf").unwrap();
        assert_eq!(model.flows[0].rate, Expr::Inf);
        assert_eq!(model.flows[0].flow_type, FlowType::Rate);
    }

    /// Explicit Leak with formula
    #[test]
    fn explicit_leak_with_formula() {
        let model = parse("Employees > Departures @ Leak(0.1)").unwrap();
        assert_eq!(model.flows[0].flow_type, FlowType::Leak);
        assert_eq!(model.flows[0].rate, Expr::Float(0.1));
    }

    /// Explicit Rate with formula expression
    #[test]
    fn explicit_rate_with_formula_expr() {
        let model = parse("Recruiter > EngRecruiter @ Rate(Recruiter * 2)").unwrap();
        assert_eq!(model.flows[0].flow_type, FlowType::Rate);
        assert_eq!(
            model.flows[0].rate,
            Expr::BinOp(
                Box::new(Expr::Ref("Recruiter".to_owned())),
                super::super::ast::BinOp::Mul,
                Box::new(Expr::Int(2))
            )
        );
    }

    /// Flow line with extra whitespace is handled
    #[test]
    fn extra_whitespace_in_flow() {
        let model = parse("  A  >  B  @  5  ").unwrap();
        assert_eq!(model.flows.len(), 1);
        assert_eq!(model.flows[0].source, "A");
        assert_eq!(model.flows[0].dest, "B");
        assert_eq!(model.flows[0].rate, Expr::Int(5));
    }

    /// Leak from infinite source is rejected
    #[test]
    fn leak_from_infinite_source_rejected() {
        let result = parse("[a] > b @ Leak(0.5)");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.details.as_ref().unwrap().contains("Leak"),
            "should mention Leak: {:?}",
            err
        );
    }

    /// Conversion from infinite source is rejected
    #[test]
    fn conversion_from_infinite_source_rejected() {
        let result = parse("[a] > b @ 0.5");
        assert!(result.is_err());
    }

    /// Rate from infinite source is allowed
    #[test]
    fn rate_from_infinite_source_allowed() {
        let result = parse("[a] > b @ 5");
        assert!(result.is_ok());
    }

    /// Leak/Conversion from infinite source declared on earlier line
    #[test]
    fn leak_from_earlier_infinite_source_rejected() {
        let result = parse("[A]\nA > B @ Leak(0.5)");
        assert!(
            result.is_err(),
            "should reject Leak from earlier-declared infinite source"
        );
    }

    #[test]
    fn conversion_from_earlier_infinite_source_rejected() {
        let result = parse("[A]\nA > B @ 0.5");
        assert!(
            result.is_err(),
            "should reject Conversion from earlier-declared infinite source"
        );
    }

    /// Leak from stock with infinite initial value (not bracket syntax)
    #[test]
    fn leak_from_inf_initial_value_rejected() {
        let result = parse("A(inf) > B @ Leak(0.5)");
        assert!(
            result.is_err(),
            "should reject Leak from stock with inf initial value"
        );
    }

    /// Conversion from stock with infinite initial value (not bracket syntax)
    #[test]
    fn conversion_from_inf_initial_value_rejected() {
        let result = parse("A(inf) > B @ 0.5");
        assert!(
            result.is_err(),
            "should reject Conversion from stock with inf initial value"
        );
    }

    /// FlowNoRate creates both stocks but no flow
    #[test]
    fn flow_no_rate_creates_stocks_but_no_flow() {
        let model = parse("A > B").unwrap();
        assert_eq!(model.stocks.len(), 2);
        assert_stock(&model, "A", &Expr::Int(0), &Expr::Inf, false);
        assert_stock(&model, "B", &Expr::Int(0), &Expr::Inf, false);
        assert!(
            model.flows.is_empty(),
            "FlowNoRate should not create a flow"
        );
    }

    /// Stock name with underscores and digits
    #[test]
    fn stock_name_with_underscores_and_digits() {
        let model = parse("Stock_1 > Stock_2 @ 5").unwrap();
        assert_eq!(model.flows[0].source, "Stock_1");
        assert_eq!(model.flows[0].dest, "Stock_2");
    }
}
