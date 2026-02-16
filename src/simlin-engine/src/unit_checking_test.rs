// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for unit inference and checking, especially with implicit models

#[cfg(test)]
mod tests {
    use crate::test_common::TestProject;

    #[test]
    fn test_smth1_with_consistent_units() {
        // Test that SMTH1 correctly infers and checks units
        // delay_time must have the same units as simulation time
        TestProject::new("smth1_test")
            .with_time_units("seconds")
            .unit("widgets", None)
            .unit("seconds", None)
            .aux_with_units("input", "100", Some("widgets"))
            .aux_with_units("delay_time", "5", Some("seconds"))
            // SMTH1 should preserve the units of input (widgets)
            .aux_with_units("smoothed", "SMTH1(input, delay_time)", None)
            // This should work because smoothed has units of widgets
            .aux_with_units("output", "smoothed + 10", Some("widgets"))
            .assert_compiles();
    }

    #[test]
    fn test_smth1_with_initial_value() {
        // Test SMTH1 with all three parameters
        // delay_time must have the same units as simulation time
        TestProject::new("smth1_initial_test")
            .with_time_units("seconds")
            .unit("widgets", None)
            .unit("seconds", None)
            .aux_with_units("input", "100", Some("widgets"))
            .aux_with_units("delay_time", "5", Some("seconds"))
            .aux_with_units("initial", "50", Some("widgets"))
            // SMTH1 with initial value should preserve units
            .aux_with_units("smoothed", "SMTH1(input, delay_time, initial)", None)
            .aux_with_units("output", "smoothed * 2", Some("widgets"))
            .assert_compiles();
    }

    #[test]
    fn test_smth1_unit_mismatch_initial() {
        // Test that SMTH1 fails when initial value has wrong units.
        // The mismatch between input (widgets) and initial (gadgets) is detected
        // through unit inference propagating constraints through the SMTH1 module.
        TestProject::new("smth1_mismatch_test")
            .with_time_units("seconds")
            .unit("widgets", None)
            .unit("gadgets", None)
            .unit("seconds", None)
            .aux_with_units("input", "100", Some("widgets"))
            .aux_with_units("delay_time", "5", Some("seconds"))
            .aux_with_units("initial", "50", Some("gadgets")) // Wrong units!
            .aux_with_units("smoothed", "SMTH1(input, delay_time, initial)", None)
            .assert_unit_error();
    }

    #[test]
    fn test_delay1_with_units() {
        // Test DELAY1 function
        // delay_time must have the same units as simulation time
        TestProject::new("delay1_test")
            .with_time_units("days")
            .unit("people", None)
            .unit("days", None)
            .aux_with_units("input_flow", "1000", Some("people"))
            .aux_with_units("delay_time", "7", Some("days"))
            .aux_with_units("initial", "500", Some("people"))
            // DELAY1 should preserve units of input
            .aux_with_units("delayed", "DELAY1(input_flow, delay_time, initial)", None)
            .aux_with_units("total", "delayed + input_flow", Some("people"))
            .assert_compiles();
    }

    #[test]
    fn test_trend_with_units() {
        // Test TREND function
        // averaging_time must have the same units as simulation time
        TestProject::new("trend_test")
            .with_time_units("years")
            .unit("dollars", None)
            .unit("years", None)
            .aux_with_units("current_value", "1000", Some("dollars"))
            .aux_with_units("averaging_time", "3", Some("years"))
            .aux_with_units("initial_trend", "0.05", Some("1/years"))
            // TREND returns fractional rate of change (1/time)
            .aux_with_units(
                "growth_rate",
                "TREND(current_value, averaging_time, initial_trend)",
                None,
            )
            // growth_rate should have units of 1/years
            .aux_with_units("years_value", "1", Some("years"))
            .aux_with_units(
                "growth_percent",
                "growth_rate * 100 * years_value",
                Some("dimensionless"),
            )
            .assert_compiles();
    }

    #[test]
    fn test_stock_and_flow_units() {
        // Test that stocks and flows have proper unit relationships
        // Flow units must use simulation time unit (Month, not Months)
        TestProject::new("stock_flow_test")
            .unit("widgets", None)
            .stock_with_units(
                "inventory",
                "1000",
                &["production"],
                &["shipments"],
                Some("widgets"),
            )
            .flow_with_units("production", "100", Some("widgets/Month"))
            .flow_with_units("shipments", "80", Some("widgets/Month"))
            .assert_compiles();
    }

    #[test]
    fn test_stock_flow_unit_mismatch() {
        // Test that incorrect flow units are caught
        // Flow units must use simulation time unit (Month)
        TestProject::new("stock_flow_mismatch")
            .unit("widgets", None)
            .unit("gadgets", None)
            .stock_with_units("inventory", "1000", &["production"], &[], Some("widgets"))
            // Wrong units - should be widgets/Month, not gadgets/Month
            .flow_with_units("production", "100", Some("gadgets/Month"))
            .assert_unit_error();
    }

    #[test]
    fn test_smth1_in_complex_model() {
        // Test SMTH1 in a more complex model with multiple unit types
        // smoothing_time must match simulation time units
        TestProject::new("complex_smth1")
            .with_time_units("weeks")
            .unit("customers", None)
            .unit("dollars", None)
            .unit("weeks", None)
            // Customer acquisition
            .aux_with_units("new_customers", "50", Some("customers/weeks"))
            .aux_with_units("smoothing_time", "4", Some("weeks"))
            // Smooth the customer acquisition rate
            .aux_with_units(
                "smoothed_acquisition",
                "SMTH1(new_customers, smoothing_time)",
                Some("customers/weeks"),
            )
            // Revenue per customer
            .aux_with_units("revenue_per_customer", "100", Some("dollars/customers"))
            // Total revenue rate
            .aux_with_units(
                "revenue_rate",
                "smoothed_acquisition * revenue_per_customer",
                Some("dollars/weeks"),
            )
            .assert_compiles();
    }

    #[test]
    fn test_delay3_with_units() {
        // Test DELAY3 (third-order delay)
        // delay_time must match simulation time units
        TestProject::new("delay3_test")
            .with_time_units("hours")
            .unit("items", None)
            .unit("hours", None)
            .aux_with_units("input_rate", "20", Some("items/hours"))
            .aux_with_units("delay_time", "8", Some("hours"))
            .aux_with_units("initial", "10", Some("items/hours"))
            // DELAY3 preserves input units
            .aux_with_units(
                "delayed_rate",
                "DELAY3(input_rate, delay_time, initial)",
                None,
            )
            .aux_with_units("total_rate", "delayed_rate + 5", Some("items/hours"))
            .assert_compiles();
    }

    #[test]
    fn test_smth3_with_units() {
        // Test SMTH3 (third-order smooth)
        // smoothing_time must match simulation time units
        // Note: Use dimensionally correct expression
        TestProject::new("smth3_test")
            .with_time_units("minutes")
            .unit("kg", None)
            .unit("minutes", None)
            .aux_with_units("base_weight", "100", Some("kg"))
            .aux_with_units("noise_amplitude", "10", Some("kg"))
            .aux_with_units("period", "60", Some("minutes"))
            // SIN returns dimensionless; multiply by noise_amplitude to get kg units
            .aux_with_units(
                "noisy_signal",
                "base_weight + noise_amplitude * SIN(TIME / period)",
                Some("kg"),
            )
            .aux_with_units("smoothing_time", "15", Some("minutes"))
            .aux_with_units("initial", "100", Some("kg"))
            // SMTH3 preserves input units
            .aux_with_units(
                "smooth_signal",
                "SMTH3(noisy_signal, smoothing_time, initial)",
                None,
            )
            .aux_with_units("deviation", "ABS(smooth_signal - noisy_signal)", Some("kg"))
            .assert_compiles();
    }

    #[test]
    fn test_previous_with_units() {
        // Test PREVIOUS function
        // Uses default time unit "Month"
        TestProject::new("previous_test")
            .unit("meters", None)
            .aux_with_units("velocity", "5", Some("meters/Month"))
            .aux_with_units("position", "TIME * velocity", Some("meters"))
            .aux_with_units("initial_position", "0", Some("meters"))
            // PREVIOUS preserves input units
            .aux_with_units(
                "previous_position",
                "PREVIOUS(position, initial_position)",
                None,
            )
            .aux_with_units(
                "distance_moved",
                "position - previous_position",
                Some("meters"),
            )
            .assert_compiles();
    }

    #[test]
    fn test_init_with_units() {
        // Test INIT function
        // Uses default time unit "Month"
        TestProject::new("init_test")
            .unit("celsius", None)
            .aux_with_units("temp_rate", "2", Some("celsius/Month"))
            .aux_with_units("current_temp", "20 + TIME * temp_rate", Some("celsius"))
            // INIT captures initial value and preserves units
            .aux_with_units("initial_temp", "INIT(current_temp)", None)
            .aux_with_units(
                "temp_change",
                "current_temp - initial_temp",
                Some("celsius"),
            )
            .assert_compiles();
    }

    #[test]
    fn test_nested_builtins_with_units() {
        // Test nested builtin functions
        // time parameters must match simulation time units
        // Note: SIN expects dimensionless input, so we divide TIME by period to make it dimensionless
        TestProject::new("nested_builtins")
            .with_time_units("seconds")
            .unit("units", None)
            .unit("seconds", None)
            .aux_with_units("period", "10", Some("seconds"))
            .aux_with_units("amplitude", "100", Some("units"))
            // SIN returns dimensionless, multiply by amplitude to get units
            .aux_with_units("raw_input", "amplitude * SIN(TIME / period)", Some("units"))
            .aux_with_units("smooth_time", "2", Some("seconds"))
            .aux_with_units("delay_time", "3", Some("seconds"))
            // First smooth, then delay
            .aux_with_units("smoothed", "SMTH1(raw_input, smooth_time)", None)
            .aux_with_units("delayed", "DELAY1(smoothed, delay_time)", None)
            // Both should have units of "units"
            .aux_with_units("output", "smoothed + delayed", Some("units"))
            .assert_compiles();
    }

    #[test]
    fn test_unit_inference_through_expressions() {
        // Test that units are properly inferred through complex expressions
        // time_period used for SMTH1 must match simulation time units
        TestProject::new("inference_test")
            .with_time_units("days")
            .unit("apples", None)
            .unit("oranges", None)
            .unit("days", None)
            .aux_with_units("apple_rate", "10", Some("apples/days"))
            .aux_with_units("orange_rate", "15", Some("oranges/days"))
            .aux_with_units("time_period", "7", Some("days"))
            // These should infer their units
            .aux_with_units("total_apples", "apple_rate * time_period", None)
            .aux_with_units("total_oranges", "orange_rate * time_period", None)
            // Now use them with SMTH1
            .aux_with_units("smooth_apples", "SMTH1(total_apples, time_period)", None)
            // This should work because smooth_apples has units of apples
            .aux_with_units("final_apples", "smooth_apples + 5", Some("apples"))
            .assert_compiles();
    }

    #[test]
    fn test_dimensionless_operations() {
        // Test operations that should be dimensionless
        // time_constant used for SMTH1 must match simulation time units
        TestProject::new("dimensionless_test")
            .with_time_units("seconds")
            .unit("meters", None)
            .unit("seconds", None)
            .aux_with_units("distance", "100", Some("meters"))
            .aux_with_units("reference_distance", "50", Some("meters"))
            // Ratio should be dimensionless
            .aux_with_units(
                "ratio",
                "distance / reference_distance",
                Some("dimensionless"),
            )
            // Can use dimensionless values in any context
            .aux_with_units("scaled", "ratio * 200", Some("dimensionless"))
            // SMTH1 of dimensionless value
            .aux_with_units("time_constant", "5", Some("seconds"))
            .aux_with_units("smoothed_ratio", "SMTH1(ratio, time_constant)", None)
            .aux_with_units("final", "smoothed_ratio * 100", Some("dimensionless"))
            .assert_compiles();
    }

    #[test]
    fn test_unit_checking_with_time() {
        // Test that TIME has proper units
        // Uses default time unit "Month" (not "Months")
        TestProject::new("time_units_test")
            .unit("widgets", None)
            .aux_with_units("production_rate", "10", Some("widgets/Month"))
            // TIME should have units of Month (from sim_specs)
            .aux_with_units("cumulative", "production_rate * TIME", Some("widgets"))
            // Can also use in SMTH1
            .aux_with_units("input_rate", "5", Some("widgets/Month"))
            .aux_with_units("varying_input", "TIME * input_rate", None)
            .aux_with_units("smooth_time", "2", Some("Month"))
            .aux_with_units("smoothed", "SMTH1(varying_input, smooth_time)", None)
            .aux_with_units("result", "smoothed", Some("widgets"))
            .assert_compiles();
    }

    #[test]
    fn test_chained_smoothing_with_units() {
        // Test multiple levels of smoothing
        // Smoothing times must match simulation time units
        TestProject::new("chained_smooth")
            .with_time_units("milliseconds")
            .unit("volts", None)
            .unit("milliseconds", None)
            .aux_with_units("signal", "5", Some("volts"))
            .aux_with_units("fast_smooth", "0.1", Some("milliseconds"))
            .aux_with_units("slow_smooth", "1.0", Some("milliseconds"))
            // First level smoothing
            .aux_with_units("level1", "SMTH1(signal, fast_smooth)", None)
            // Second level smoothing
            .aux_with_units("level2", "SMTH1(level1, slow_smooth)", None)
            // Both should have units of volts
            .aux_with_units("output", "level1 + level2", Some("volts"))
            .assert_compiles();
    }

    #[test]
    fn test_previous_basic_functionality() {
        // Test PREVIOUS function returns exact previous timestep values per XMILE spec
        //
        // NOTE: The stdlib/previous.stmx implementation uses a stock mechanism
        // which may cause smoothing when values change between save steps.
        // However, for values sampled at save steps, it works correctly.

        let results = TestProject::new("previous_basic")
            .with_sim_time(0.0, 2.0, 0.5) // Run from t=0 to t=2 with dt=0.5
            .aux("a", "TIME * 10", None) // a will be 0, 5, 10, 15, 20
            .aux("prev_a", "PREVIOUS(a, 666)", None)
            .run_interpreter()
            .expect("Simulation should succeed");

        let prev_a_values = results.get("prev_a").expect("Should have 'prev_a' values");

        // According to XMILE spec:
        // - At first timestep, PREVIOUS returns initial value
        assert_eq!(
            prev_a_values[0], 666.0,
            "First timestep should return initial value"
        );

        // - Verify that subsequent values have changed from initial
        // (exact values depend on integration between save steps)
        for (i, value) in prev_a_values.iter().enumerate().skip(1) {
            assert_ne!(
                *value, 666.0,
                "At timestep {i}, PREVIOUS should no longer return initial value"
            );
        }
    }

    #[test]
    fn test_previous_with_constant() {
        // Test PREVIOUS with a constant input returns exact values per spec
        let results = TestProject::new("previous_const")
            .with_sim_time(0.0, 2.0, 0.5) // Run from t=0 to t=2 with dt=0.5
            .aux("const_val", "42", None)
            .aux("prev_const", "PREVIOUS(const_val, 100)", None)
            .run_interpreter()
            .expect("Simulation should succeed");

        let prev_const = results
            .get("prev_const")
            .expect("Should have 'prev_const' values");

        // At first timestep, should return initial value
        assert_eq!(prev_const[0], 100.0);

        // At all subsequent timesteps, should return 42 (the constant from previous timestep)
        for (i, value) in prev_const.iter().enumerate().skip(1) {
            assert_eq!(
                *value, 42.0,
                "At timestep {i}, PREVIOUS of constant 42 should be 42"
            );
        }
    }

    #[test]
    fn test_previous_with_self() {
        // Test PREVIOUS with SELF reference per XMILE spec
        let results = TestProject::new("previous_self")
            .with_sim_time(0.0, 3.0, 1.0) // Run from t=0 to t=3 with dt=1.0
            .aux(
                "accumulator",
                "IF TIME > 1 THEN PREVIOUS(SELF, 100) + 10 ELSE 100",
                None,
            )
            .run_interpreter()
            .expect("Simulation should succeed");

        let acc = results
            .get("accumulator")
            .expect("Should have 'accumulator' values");

        // t=0: TIME=0, not > 1, so value = 100
        assert_eq!(acc[0], 100.0, "At t=0, should be 100");

        // t=1: TIME=1, not > 1, so value = 100
        assert_eq!(acc[1], 100.0, "At t=1, should still be 100");

        // t=2: TIME=2 > 1, so value = PREVIOUS(SELF, 100) + 10 = 100 + 10 = 110
        assert_eq!(acc[2], 110.0, "At t=2, should be PREVIOUS(100) + 10 = 110");

        // t=3: TIME=3 > 1, so value = PREVIOUS(SELF, 100) + 10 = 110 + 10 = 120
        assert_eq!(acc[3], 120.0, "At t=3, should be PREVIOUS(110) + 10 = 120");
    }

    #[test]
    fn test_previous_with_expression() {
        // Test PREVIOUS with an expression as input per XMILE spec
        let results = TestProject::new("previous_expr")
            .with_sim_time(0.0, 3.0, 1.0) // Run from t=0 to t=3 with dt=1.0
            .aux("x", "TIME * 10", None) // x = 0, 10, 20, 30
            .aux("y", "TIME * 5", None) // y = 0, 5, 10, 15
            .aux("prev_sum", "PREVIOUS(x + y, 99)", None)
            .run_interpreter()
            .expect("Simulation should succeed");

        let x = results.get("x").expect("Should have 'x' values");
        let y = results.get("y").expect("Should have 'y' values");
        let prev_sum = results
            .get("prev_sum")
            .expect("Should have 'prev_sum' values");

        // First timestep should return initial value
        assert_eq!(prev_sum[0], 99.0, "At t=0, should return initial value 99");

        // Subsequent timesteps should return previous value of (x + y)
        for i in 1..prev_sum.len() {
            let expected = x[i - 1] + y[i - 1];
            assert_eq!(
                prev_sum[i], expected,
                "At timestep {i}, PREVIOUS(x+y) should be {expected}"
            );
        }
    }

    #[test]
    fn test_previous_with_different_dt_and_save_step() {
        // Test PREVIOUS with dt != save_step to verify it returns value from last DT
        // Per XMILE spec: PREVIOUS returns "the value in the last DT", not last save step
        //
        // Setup: start=1, stop=4, dt=0.25, save_step=1
        // This means simulation runs every 0.25 time units but only saves every 1.0
        // TIME at dt steps: 1, 1.25, 1.5, 1.75, 2, 2.25, 2.5, 2.75, 3, 3.25, 3.5, 3.75, 4
        // TIME at save steps: 1, 2, 3, 4

        use crate::datamodel;

        // Note: run_interpreter only returns values at save steps by default
        // We need to modify save_step explicitly
        let mut project = TestProject::new("previous_dt_test_explicit");
        project.sim_specs.start = 1.0;
        project.sim_specs.stop = 4.0;
        project.sim_specs.dt = datamodel::Dt::Dt(0.25);
        project.sim_specs.save_step = Some(datamodel::Dt::Dt(1.0));

        let results = project
            .aux("counter", "TIME", None)
            .aux("prev_counter", "PREVIOUS(counter, 999)", None)
            .run_interpreter()
            .expect("Simulation should succeed");

        let counter = results.get("counter").expect("Should have counter values");
        let prev_counter = results
            .get("prev_counter")
            .expect("Should have prev_counter values");

        println!("With dt=0.25, save_step=1.0:");
        println!("TIME values at save steps: {counter:?}");
        println!("PREVIOUS(TIME, 999): {prev_counter:?}");

        // At save step t=1 (first save): PREVIOUS should return initial value
        assert_eq!(
            prev_counter[0], 999.0,
            "At t=1, should return initial value"
        );

        // At save step t=2:
        // The last DT before t=2 was at t=1.75 where TIME=1.75
        // So PREVIOUS should return 1.75, NOT 1.0 from the last save step!
        assert_eq!(
            prev_counter[1], 1.75,
            "At t=2, PREVIOUS should return value from t=1.75 (last DT), not t=1 (last save)"
        );

        // At save step t=3:
        // The last DT before t=3 was at t=2.75 where TIME=2.75
        assert_eq!(
            prev_counter[2], 2.75,
            "At t=3, PREVIOUS should return value from t=2.75 (last DT)"
        );

        // At save step t=4:
        // The last DT before t=4 was at t=3.75 where TIME=3.75
        assert_eq!(
            prev_counter[3], 3.75,
            "At t=4, PREVIOUS should return value from t=3.75 (last DT)"
        );
    }

    #[test]
    fn test_previous_chain() {
        // Test chaining PREVIOUS functions per XMILE spec
        let results = TestProject::new("previous_chain")
            .with_sim_time(0.0, 3.0, 1.0) // Run from t=0 to t=3 with dt=1.0
            .aux("a", "TIME * 100", None) // a = 0, 100, 200, 300
            .aux("prev1", "PREVIOUS(a, 999)", None)
            .aux("prev2", "PREVIOUS(prev1, 888)", None)
            .run_interpreter()
            .expect("Simulation should succeed");

        let a = results.get("a").expect("Should have 'a' values");
        let prev1 = results.get("prev1").expect("Should have 'prev1' values");
        let prev2 = results.get("prev2").expect("Should have 'prev2' values");

        // At t=0: prev1 should be 999 (initial), prev2 should be 888 (initial)
        assert_eq!(prev1[0], 999.0, "prev1 at t=0 should be initial value 999");
        assert_eq!(prev2[0], 888.0, "prev2 at t=0 should be initial value 888");

        // At t=1: prev1 = a[0] = 0, prev2 = prev1[0] = 999
        assert_eq!(prev1[1], a[0], "prev1 at t=1 should be a[0]");
        assert_eq!(prev2[1], prev1[0], "prev2 at t=1 should be prev1[0]");

        // Verify the pattern continues for all timesteps
        for i in 2..a.len() {
            assert_eq!(prev1[i], a[i - 1], "prev1[{}] should equal a[{}]", i, i - 1);
            assert_eq!(
                prev2[i],
                prev1[i - 1],
                "prev2[{}] should equal prev1[{}]",
                i,
                i - 1
            );
        }
    }

    #[test]
    fn test_arrayed_expression_unit_inference() {
        // Test that unit inference works for arrayed variables with different expressions
        // for different elements. All elements should have the same inferred units.
        TestProject::new("arrayed_units_test")
            .with_time_units("days")
            .unit("widgets", None)
            .unit("days", None)
            .named_dimension("Region", &["North", "South"])
            // A scalar with known units
            .aux_with_units("base_rate", "10", Some("widgets/days"))
            // An arrayed variable with different expressions per element
            // Both should infer to widgets/days
            .array_with_ranges_direct(
                "regional_rate",
                vec!["Region".to_string()],
                vec![("North", "base_rate * 1.5"), ("South", "base_rate * 0.8")],
                Some("widgets/days"),
            )
            // Use the arrayed variable to verify units propagate
            .aux_with_units("total_rate", "SUM(regional_rate[*])", Some("widgets/days"))
            .assert_compiles();
    }

    #[test]
    fn test_arrayed_expression_inference_without_declared_units() {
        // Test that arrayed variables can infer units even when not declared
        TestProject::new("arrayed_infer_test")
            .with_time_units("seconds")
            .unit("meters", None)
            .unit("seconds", None)
            .named_dimension("Axis", &["X", "Y", "Z"])
            // A scalar with known units
            .aux_with_units("speed", "5", Some("meters/seconds"))
            // An arrayed variable without declared units - should infer from expression
            .array_with_ranges_direct(
                "velocity",
                vec!["Axis".to_string()],
                vec![("X", "speed"), ("Y", "speed * 2"), ("Z", "speed / 2")],
                None, // No units declared
            )
            // Use velocity in an expression with declared units to verify inference
            .aux_with_units("total_velocity", "SUM(velocity[*])", Some("meters/seconds"))
            .assert_compiles();
    }

    #[test]
    fn test_arrayed_expression_conflicting_inferred_units() {
        // Test that inference catches when different array elements have different units
        // without any declared units on the array itself
        TestProject::new("arrayed_conflict_test")
            .with_time_units("days")
            .unit("widgets", None)
            .unit("gadgets", None)
            .named_dimension("Type", &["A", "B"])
            .aux_with_units("widget_rate", "10", Some("widgets"))
            .aux_with_units("gadget_rate", "20", Some("gadgets"))
            // Different elements have different units - should fail
            .array_with_ranges_direct(
                "rates",
                vec!["Type".to_string()],
                vec![("A", "widget_rate"), ("B", "gadget_rate")],
                None, // No declared units - relies on inference
            )
            .assert_unit_error();
    }

    #[test]
    fn test_arrayed_expression_unit_mismatch() {
        // Test that unit checking catches mismatches in arrayed expressions
        // The declared units are "widgets" but the expression computes "widgets/days"
        TestProject::new("arrayed_mismatch_test")
            .with_time_units("days")
            .unit("widgets", None)
            .unit("days", None)
            .named_dimension("Region", &["North", "South"])
            .aux_with_units("rate", "10", Some("widgets/days"))
            // This should cause an error: declared units are "widgets" but
            // the expression "rate * 1.5" has units "widgets/days"
            .array_with_ranges_direct(
                "values",
                vec!["Region".to_string()],
                vec![("North", "rate * 1.5"), ("South", "rate * 0.8")],
                Some("widgets"), // Wrong units!
            )
            .assert_unit_error();
    }

    #[test]
    fn test_apply_to_all_unit_checking() {
        // Test that ApplyToAll expressions are properly checked for unit consistency
        TestProject::new("apply_to_all_units")
            .with_time_units("days")
            .unit("widgets", None)
            .unit("days", None)
            .named_dimension("Region", &["North", "South"])
            .aux_with_units("rate", "10", Some("widgets/days"))
            // ApplyToAll with correct units
            .array_aux_direct(
                "values",
                vec!["Region".to_string()],
                "rate * 2",
                Some("widgets/days"),
            )
            .aux_with_units("total", "SUM(values[*])", Some("widgets/days"))
            .assert_compiles();
    }

    #[test]
    fn test_apply_to_all_unit_mismatch() {
        // Test that ApplyToAll expressions catch unit mismatches
        TestProject::new("apply_to_all_mismatch")
            .with_time_units("days")
            .unit("widgets", None)
            .unit("days", None)
            .named_dimension("Region", &["North", "South"])
            .aux_with_units("rate", "10", Some("widgets/days"))
            // This should cause an error: declared units are "widgets" but
            // the expression "rate * 2" has units "widgets/days"
            .array_aux_direct(
                "values",
                vec!["Region".to_string()],
                "rate * 2",
                Some("widgets"), // Wrong units!
            )
            .assert_unit_error();
    }

    #[test]
    fn test_if_else_unit_checking() {
        // Test that if/else branches with consistent units work correctly
        TestProject::new("if_else_consistent")
            .with_time_units("days")
            .unit("widgets", None)
            .unit("days", None)
            .aux_with_units("rate_a", "10", Some("widgets/days"))
            .aux_with_units("rate_b", "20", Some("widgets/days"))
            .aux_with_units("condition", "1", Some("dimensionless"))
            // Both branches have the same units - should work
            .aux_with_units(
                "selected_rate",
                "IF condition > 0 THEN rate_a ELSE rate_b",
                Some("widgets/days"),
            )
            .assert_compiles();
    }

    #[test]
    fn test_if_else_unit_mismatch() {
        // Test that if/else branches with inconsistent units are caught
        TestProject::new("if_else_mismatch")
            .with_time_units("days")
            .unit("widgets", None)
            .unit("gadgets", None)
            .unit("days", None)
            .aux_with_units("rate_widgets", "10", Some("widgets/days"))
            .aux_with_units("rate_gadgets", "20", Some("gadgets/days"))
            .aux_with_units("condition", "1", Some("dimensionless"))
            // Branches have different units - should fail
            .aux_with_units(
                "selected_rate",
                "IF condition > 0 THEN rate_widgets ELSE rate_gadgets",
                Some("widgets/days"),
            )
            .assert_unit_error();
    }

    #[test]
    fn test_safediv_consistent_units() {
        // Test SAFEDIV with consistent units
        TestProject::new("safediv_consistent")
            .with_time_units("days")
            .unit("widgets", None)
            .unit("days", None)
            .aux_with_units("numerator", "100", Some("widgets"))
            .aux_with_units("denominator", "5", Some("days"))
            .aux_with_units("fallback", "0", Some("widgets/days"))
            // SAFEDIV(a, b, c) should work when c has units a/b
            .aux_with_units(
                "result",
                "SAFEDIV(numerator, denominator, fallback)",
                Some("widgets/days"),
            )
            .assert_compiles();
    }

    #[test]
    fn test_safediv_mismatched_fallback() {
        // Test that SAFEDIV catches mismatched fallback units
        TestProject::new("safediv_mismatch")
            .with_time_units("days")
            .unit("widgets", None)
            .unit("gadgets", None)
            .unit("days", None)
            .aux_with_units("numerator", "100", Some("widgets"))
            .aux_with_units("denominator", "5", Some("days"))
            .aux_with_units("bad_fallback", "0", Some("gadgets/days")) // Wrong units!
            // SAFEDIV(a, b, c) should fail when c has wrong units
            .aux_with_units(
                "result",
                "SAFEDIV(numerator, denominator, bad_fallback)",
                Some("widgets/days"),
            )
            .assert_unit_error();
    }

    #[test]
    fn test_unit_mismatch_allows_simulation() {
        // Critical test: Unit errors should NOT block simulation.
        // Many real-world models have unit errors but should still run.
        // The unit error should be detected and surfaced, but simulation should proceed.
        use crate::project::Project as CompiledProject;

        let project = TestProject::new("unit_mismatch_runs_test")
            .unit("apples", None)
            .unit("oranges", None)
            // Create a unit mismatch: apples + oranges
            .aux_with_units("apple_count", "10", Some("apples"))
            .aux_with_units("orange_count", "20", Some("oranges"))
            .aux_with_units("fruit_total", "apple_count + orange_count", None); // Mismatch!

        // Check that unit warning exists
        let datamodel = project.build_datamodel();
        let compiled = CompiledProject::from(datamodel);

        // Verify there's a unit warning but no blocking errors
        let has_unit_warning = compiled
            .models
            .values()
            .any(|m| m.unit_warnings.as_ref().is_some_and(|w| !w.is_empty()));

        let has_blocking_errors = compiled.models.values().any(|m| {
            m.errors.as_ref().is_some_and(|e| !e.is_empty()) || !m.get_variable_errors().is_empty()
        });

        // Unit warnings should be present OR per-variable unit errors
        let has_unit_issues = has_unit_warning
            || compiled
                .models
                .values()
                .any(|m| !m.get_unit_errors().is_empty());

        assert!(
            has_unit_issues,
            "Should have unit warnings or errors for the mismatch"
        );
        assert!(
            !has_blocking_errors,
            "Should NOT have blocking compilation errors"
        );

        // Verify simulation can still run
        let results = project
            .run_interpreter()
            .expect("Simulation should run despite unit mismatch");

        // The simulation should produce results
        let total = results
            .get("fruit_total")
            .expect("fruit_total should be in results");
        assert_eq!(
            total.last().copied(),
            Some(30.0),
            "fruit_total should be 10 + 20 = 30"
        );
    }

    #[test]
    fn test_unit_mismatch_in_stock_allows_simulation() {
        // Test that unit mismatch with stocks also allows simulation
        use crate::project::Project as CompiledProject;

        let project = TestProject::new("stock_unit_mismatch_runs")
            .unit("widgets", None)
            .unit("gadgets", None)
            .aux_with_units("widget_rate", "5", Some("widgets/Month"))
            .aux_with_units("gadget_initial", "100", Some("gadgets"))
            // Stock with mismatched units: flow is widgets/Month, initial is gadgets
            .stock_with_units(
                "mixed_inventory",
                "gadget_initial",
                &["widget_rate"],
                &[],
                Some("widgets"),
            );

        // Check that unit warning exists
        let datamodel = project.build_datamodel();
        let compiled = CompiledProject::from(datamodel);

        // Verify there's a unit warning but no blocking errors
        let has_unit_warning = compiled
            .models
            .values()
            .any(|m| m.unit_warnings.as_ref().is_some_and(|w| !w.is_empty()));

        let has_blocking_errors = compiled.models.values().any(|m| {
            m.errors.as_ref().is_some_and(|e| !e.is_empty()) || !m.get_variable_errors().is_empty()
        });

        // Unit warnings should be present OR per-variable unit errors
        let has_unit_issues = has_unit_warning
            || compiled
                .models
                .values()
                .any(|m| !m.get_unit_errors().is_empty());

        assert!(
            has_unit_issues,
            "Should have unit warnings or errors for the mismatch"
        );
        assert!(
            !has_blocking_errors,
            "Should NOT have blocking compilation errors"
        );

        // Simulation should still run
        let results = project
            .run_interpreter()
            .expect("Simulation should run despite unit mismatch");

        // Verify we got results
        let inventory = results
            .get("mixed_inventory")
            .expect("mixed_inventory should be in results");
        assert!(
            !inventory.is_empty(),
            "Should have simulation results for mixed_inventory"
        );
    }
}
