// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for unit inference and checking, especially with implicit models

#[cfg(test)]
mod tests {
    use crate::test_common::TestProject;

    #[test]
    fn test_smth1_with_consistent_units() {
        // Test that SMTH1 correctly infers and checks units
        TestProject::new("smth1_test")
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
        TestProject::new("smth1_initial_test")
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
    #[ignore]
    fn test_smth1_unit_mismatch_initial() {
        // Test that SMTH1 fails when initial value has wrong units
        TestProject::new("smth1_mismatch_test")
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
        TestProject::new("delay1_test")
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
        TestProject::new("trend_test")
            .unit("dollars", None)
            .unit("years", None)
            .unit("fraction", None)
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
        TestProject::new("stock_flow_test")
            .unit("widgets", None)
            .stock_with_units(
                "inventory",
                "1000",
                &["production"],
                &["shipments"],
                Some("widgets"),
            )
            .flow_with_units("production", "100", Some("widgets/Months"))
            .flow_with_units("shipments", "80", Some("widgets/Months"))
            .assert_compiles();
    }

    #[test]
    #[ignore]
    fn test_stock_flow_unit_mismatch() {
        // Test that incorrect flow units are caught
        TestProject::new("stock_flow_mismatch")
            .unit("widgets", None)
            .unit("gadgets", None)
            .stock_with_units("inventory", "1000", &["production"], &[], Some("widgets"))
            // Wrong units - should be widgets/Months, not gadgets/Months
            .flow_with_units("production", "100", Some("gadgets/Months"))
            .assert_unit_error();
    }

    #[test]
    fn test_smth1_in_complex_model() {
        // Test SMTH1 in a more complex model with multiple unit types
        TestProject::new("complex_smth1")
            .unit("customers", None)
            .unit("dollars", None)
            .unit("weeks", None)
            // Customer acquisition
            .aux_with_units("new_customers", "50", Some("customers/Months"))
            .aux_with_units("smoothing_time", "4", Some("weeks"))
            // Smooth the customer acquisition rate
            .aux_with_units("conversion_factor", "4.33", Some("weeks/Months"))
            .aux_with_units(
                "smoothed_acquisition",
                "SMTH1(new_customers * conversion_factor, smoothing_time)",
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
        TestProject::new("delay3_test")
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
        TestProject::new("smth3_test")
            .unit("kg", None)
            .unit("minutes", None)
            .aux_with_units("noisy_signal", "100 + SIN(TIME)", Some("kg"))
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
        TestProject::new("previous_test")
            .unit("meters", None)
            .aux_with_units("velocity", "5", Some("meters/Months"))
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
        TestProject::new("init_test")
            .unit("celsius", None)
            .aux_with_units("temp_rate", "2", Some("celsius/Months"))
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
        TestProject::new("nested_builtins")
            .unit("units", None)
            .unit("seconds", None)
            .aux_with_units("raw_input", "100 * SIN(TIME)", Some("units"))
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
        TestProject::new("inference_test")
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
        TestProject::new("dimensionless_test")
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
        TestProject::new("time_units_test")
            .unit("widgets", None)
            .aux_with_units("production_rate", "10", Some("widgets/Months"))
            // TIME should have units of Months (from sim_specs)
            .aux_with_units("cumulative", "production_rate * TIME", Some("widgets"))
            // Can also use in SMTH1
            .aux_with_units("input_rate", "5", Some("widgets/Months"))
            .aux_with_units("varying_input", "TIME * input_rate", None)
            .aux_with_units("smooth_time", "2", Some("Months"))
            .aux_with_units("smoothed", "SMTH1(varying_input, smooth_time)", None)
            .aux_with_units("result", "smoothed", Some("widgets"))
            .assert_compiles();
    }

    #[test]
    fn test_chained_smoothing_with_units() {
        // Test multiple levels of smoothing
        TestProject::new("chained_smooth")
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
}
