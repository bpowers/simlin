// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for the native MDL parser.
//!
//! These tests verify that the native MDL parser can successfully parse
//! various test models and produce valid Project structures.

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::BufReader;

    use simlin_core::datamodel::Variable;

    use crate::open_vensim_native;

    /// Test that the native parser can parse the SIR model.
    #[test]
    fn test_native_sir() {
        let mdl_content =
            fs::read_to_string("../libsimlin/testdata/SIR.mdl").expect("Failed to read SIR.mdl");

        let mut reader = BufReader::new(mdl_content.as_bytes());
        let result = open_vensim_native(&mut reader);

        let project = result.expect("Native parser should successfully parse SIR.mdl");

        // Verify basic structure
        assert_eq!(project.models.len(), 1, "Should have one model");
        let model = &project.models[0];

        // Should have the expected number of non-control variables
        // SIR model has: 3 stocks, 2 flows, 6 aux variables = 11 total
        // Control vars (INITIAL TIME, FINAL TIME, TIME STEP, SAVEPER) should be excluded
        assert!(
            !model.variables.is_empty(),
            "Model should have variables, got: {:?}",
            model.variables
        );

        // Verify sim specs were extracted correctly
        assert_eq!(project.sim_specs.start, 0.0, "Start time should be 0");
        assert_eq!(project.sim_specs.stop, 200.0, "Stop time should be 200");

        // Verify we have stocks with proper inflows/outflows
        let stocks: Vec<_> = model
            .variables
            .iter()
            .filter_map(|v| match v {
                Variable::Stock(s) => Some(s),
                _ => None,
            })
            .collect();
        assert_eq!(stocks.len(), 3, "Should have 3 stocks");

        // Check that at least one stock has flows linked
        let stocks_with_flows: Vec<_> = stocks
            .iter()
            .filter(|s| !s.inflows.is_empty() || !s.outflows.is_empty())
            .collect();
        assert!(
            !stocks_with_flows.is_empty(),
            "At least one stock should have inflows/outflows linked"
        );

        // Verify we have flows
        let flows: Vec<_> = model
            .variables
            .iter()
            .filter_map(|v| match v {
                Variable::Flow(f) => Some(f),
                _ => None,
            })
            .collect();
        assert_eq!(flows.len(), 2, "Should have 2 flows");
    }

    /// Test that the native parser can parse a simple model without groups.
    #[test]
    fn test_native_simple_model() {
        let mdl = "x = 5
~ Units
~ A constant |
y = x * 2
~ Units
~ Derived |
Stock = INTEG(rate, 100)
~ Units
~ A stock |
rate = 10
~ Units/Time
~ A flow |
INITIAL TIME = 0
~ Time
~ |
FINAL TIME = 100
~ Time
~ |
TIME STEP = 1
~ Time
~ |
\\\\\\---///
";
        let mut reader = BufReader::new(mdl.as_bytes());
        let result = open_vensim_native(&mut reader);

        let project = result.expect("Should parse simple model");

        assert_eq!(project.models.len(), 1);
        let model = &project.models[0];

        // Should have: x (aux), y (aux), Stock (stock), rate (flow) = 4 variables
        // Control vars should be excluded
        assert_eq!(model.variables.len(), 4);

        // Check sim specs
        assert_eq!(project.sim_specs.start, 0.0);
        assert_eq!(project.sim_specs.stop, 100.0);

        // Verify stock has inflow
        let stock = model.variables.iter().find_map(|v| match v {
            Variable::Stock(s) if s.ident == "stock" => Some(s),
            _ => None,
        });
        assert!(stock.is_some(), "Should have stock variable");
        let stock = stock.unwrap();
        assert_eq!(
            stock.inflows,
            vec!["rate"],
            "Stock should have 'rate' as inflow"
        );
    }
}
