// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A generated model with a diagram laid out for it, saved as MDL: the first
//! save keeps the model and is a fixed point, as it is without a diagram
//! (simlin-formats' `mdl::writer` tests).

use proptest::prelude::*;
use simlin_base::datamodel::{self, Project};
use simlin_formats::mdl::writer::test_models::{first_save_is_a_fixed_point, rich_model};

/// `project` with a stock-and-flow view laid out for it, kept by its save.
fn a_save_with_a_view_is_a_fixed_point(project: Project) -> std::result::Result<(), TestCaseError> {
    let mut project = project;
    let view = simlin_layout::generate_layout(&project, "main", None)
        .map_err(|e| TestCaseError::fail(format!("layout: {e}")))?;
    project.models[0].views = vec![datamodel::View::StockFlow(view)];
    first_save_is_a_fixed_point(&project).map_err(TestCaseError::fail)
}

proptest! {
    // Each case lays the model out before writing it; the gate below runs
    // many more.
    #![proptest_config(ProptestConfig::with_cases(2))]

    /// The same, with a stock-and-flow view laid out for the model.
    #[test]
    fn a_rich_models_first_save_with_a_view_is_a_fixed_point(project in rich_model()) {
        a_save_with_a_view_is_a_fixed_point(project)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    #[ignore = "lays out and saves 200 generated models; run under the gates profile"]
    fn many_rich_models_first_saves_with_a_view_are_fixed_points(project in rich_model()) {
        a_save_with_a_view_is_a_fixed_point(project)?;
    }
}
