// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#[test]
fn test_section6_ref_stream_counts() {
    let water = vdf_file("../../test/bobby/vdf/water/Current.vdf");
    let pop = vdf_file("../../test/bobby/vdf/pop/Current.vdf");
    let econ = vdf_file("../../test/bobby/vdf/econ/base.vdf");
    let wrld3 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

    let (skip_w, entries_w, _) = water.parse_section6_ref_stream().unwrap();
    let (skip_p, entries_p, _) = pop.parse_section6_ref_stream().unwrap();
    let (skip_e, entries_e, _) = econ.parse_section6_ref_stream().unwrap();
    let (skip_r, entries_r, _) = wrld3.parse_section6_ref_stream().unwrap();

    assert_eq!(skip_w, 0);
    assert_eq!(entries_w.len(), 7);
    assert_eq!(skip_p, 0);
    assert_eq!(entries_p.len(), 8);
    assert_eq!(skip_e, 1);
    assert_eq!(entries_e.len(), 79);
    assert_eq!(skip_r, 1);
    assert_eq!(entries_r.len(), 342);
}

#[test]
fn test_section6_ot_class_codes_have_expected_shape() {
    let water = vdf_file("../../test/bobby/vdf/water/Current.vdf");
    let pop = vdf_file("../../test/bobby/vdf/pop/Current.vdf");
    let econ = vdf_file("../../test/bobby/vdf/econ/base.vdf");
    let wrld3 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

    let water_codes = water.section6_ot_class_codes().unwrap();
    let pop_codes = pop.section6_ot_class_codes().unwrap();
    let econ_codes = econ.section6_ot_class_codes().unwrap();
    let wrld3_codes = wrld3.section6_ot_class_codes().unwrap();

    assert_eq!(water_codes.len(), water.offset_table_count);
    assert_eq!(pop_codes.len(), pop.offset_table_count);
    assert_eq!(econ_codes.len(), econ.offset_table_count);
    assert_eq!(wrld3_codes.len(), wrld3.offset_table_count);

    assert_eq!(water_codes[0], VDF_SECTION6_OT_CODE_TIME);
    assert_eq!(pop_codes[0], VDF_SECTION6_OT_CODE_TIME);
    assert_eq!(econ_codes[0], VDF_SECTION6_OT_CODE_TIME);
    assert_eq!(wrld3_codes[0], VDF_SECTION6_OT_CODE_TIME);

    assert_eq!(
        water_codes,
        vec![0x0f, 0x08, 0x17, 0x17, 0x17, 0x11, 0x11, 0x17, 0x11, 0x17]
    );
    assert_eq!(
        pop_codes,
        vec![
            0x0f, 0x08, 0x08, 0x17, 0x11, 0x17, 0x11, 0x17, 0x17, 0x17, 0x11, 0x17, 0x17,
        ]
    );
    assert_eq!(
        econ_codes
            .iter()
            .filter(|&&code| code == VDF_SECTION6_OT_CODE_STOCK)
            .count(),
        11
    );
    assert_eq!(
        wrld3_codes
            .iter()
            .filter(|&&code| code == VDF_SECTION6_OT_CODE_STOCK)
            .count(),
        41
    );
}

#[test]
fn test_section6_final_values_match_extracted_last_values() {
    let models = [
        ("water", "../../test/bobby/vdf/water/Current.vdf"),
        ("pop", "../../test/bobby/vdf/pop/Current.vdf"),
        ("econ", "../../test/bobby/vdf/econ/base.vdf"),
        ("wrld3", "../../test/metasd/WRLD3-03/SCEN01.VDF"),
    ];

    for (label, vdf_path) in models {
        let vdf = vdf_file(vdf_path);
        let final_values = vdf.section6_ot_final_values().unwrap();
        let data = vdf.extract_data().unwrap();

        assert_eq!(
            final_values.len(),
            data.entries.len(),
            "{label}: final-value vector length should match OT/data entries",
        );

        for (ot, (final_value, series)) in final_values.iter().zip(data.entries.iter()).enumerate()
        {
            let expected = series.last().copied().unwrap_or(f64::NAN) as f32;
            assert!(
                (final_value - expected).abs() < 1e-5
                    || (final_value.is_nan() && expected.is_nan()),
                "{label}: OT[{ot}] final value mismatch: parsed={final_value} expected={expected}",
            );
        }
    }
}

#[test]
fn test_section6_display_record_stream_shape() {
    let water = vdf_file("../../test/bobby/vdf/water/Current.vdf");
    let pop = vdf_file("../../test/bobby/vdf/pop/Current.vdf");
    let econ = vdf_file("../../test/bobby/vdf/econ/base.vdf");
    let wrld3 = vdf_file("../../test/metasd/WRLD3-03/SCEN01.VDF");

    let water_records = water.section6_display_records().unwrap();
    let pop_records = pop.section6_display_records().unwrap();
    let econ_records = econ.section6_display_records().unwrap();
    let wrld3_records = wrld3.section6_display_records().unwrap();

    assert!(
        water_records.is_empty(),
        "water should have no parsed display records"
    );
    assert!(
        pop_records.is_empty(),
        "pop should have no parsed display records"
    );
    assert_eq!(
        econ_records.len(),
        4,
        "econ display-record count should be stable"
    );
    assert_eq!(
        wrld3_records.len(),
        55,
        "WRLD3 display-record count should be stable"
    );

    for (label, vdf, records) in [
        ("econ", &econ, &econ_records),
        ("wrld3", &wrld3, &wrld3_records),
    ] {
        for rec in records {
            assert!(
                rec.ot_index() < vdf.offset_table_count,
                "{label}: display record OT {} out of range",
                rec.ot_index()
            );
            assert_eq!(
                rec.words[11], 1,
                "{label}: expected stable display-record flag"
            );
            assert_eq!(rec.words[12], 0, "{label}: expected zero terminator word");
        }
    }
}

#[test]
fn test_to_results_with_model_uses_vdf_visible_names_only() {
    for (mdl_path, vdf_path) in [
        (
            "../../test/bobby/vdf/water/water.mdl",
            "../../test/bobby/vdf/water/Current.vdf",
        ),
        (
            "../../test/bobby/vdf/pop/pop.mdl",
            "../../test/bobby/vdf/pop/Current.vdf",
        ),
    ] {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
        let vdf = vdf_file(vdf_path);

        let results = vdf
            .to_results_with_model(&project, "main")
            .unwrap_or_else(|e| panic!("to_results_with_model failed: {e}"));

        assert!(
            results.offsets.keys().all(|id| {
                let name = id.as_str();
                name == "time"
                    || (!name.starts_with('#')
                        && !name.starts_with('$')
                        && !is_probable_lookup_table_name(name))
            }),
            "expected Results offsets to expose only visible VDF names"
        );
    }
}

#[test]
fn test_to_results_with_model_succeeds_on_econ() {
    let mdl_path = "../../test/bobby/vdf/econ/mark2.mdl";
    let vdf_path = "../../test/bobby/vdf/econ/base.vdf";

    let contents = std::fs::read_to_string(mdl_path).unwrap();
    let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
    let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
    let vdf = vdf_file(vdf_path);

    let results = vdf
        .to_results_with_model(&project, "main")
        .unwrap_or_else(|e| panic!("to_results_with_model should succeed on econ: {e}"));
    assert!(
        results.offsets.len() > 1,
        "econ: expected mapped variable columns"
    );
}

fn visible_result_names(results: &crate::Results) -> std::collections::BTreeSet<String> {
    results
        .offsets
        .keys()
        .map(|id| id.as_str().to_owned())
        .collect()
}

fn constant_column_value(results: &crate::Results, id: &Ident<Canonical>) -> f64 {
    let offset = results
        .offsets
        .get(id)
        .copied()
        .unwrap_or_else(|| panic!("missing Results column for {}", id.as_str()));
    let first = results.data[offset];
    for step in 1..results.step_count {
        let value = results.data[step * results.step_size + offset];
        assert!(
            (value - first).abs() < 1e-9,
            "{} should be flat across the saved run",
            id.as_str()
        );
    }
    first
}

#[test]
fn test_to_results_with_model_includes_scalar_consts_but_not_lookup_tables() {
    let mdl_path = "../../test/bobby/vdf/consts/consts.mdl";
    let contents = std::fs::read_to_string(mdl_path).unwrap();
    let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
    let project = std::rc::Rc::new(crate::Project::from(datamodel_project));

    for (label, vdf_path, expected_stock_final, expected_net_flow) in [
        (
            "consts-b-is-3",
            "../../test/bobby/vdf/consts/b_is_3.vdf",
            617.5,
            6.12,
        ),
        (
            "consts-b-is-4",
            "../../test/bobby/vdf/consts/b_is_4.vdf",
            717.5,
            7.12,
        ),
    ] {
        let vdf = vdf_file(vdf_path);
        let results = vdf
            .to_results_with_model(&project, "main")
            .unwrap_or_else(|e| panic!("{label}: to_results_with_model failed: {e}"));

        let names = visible_result_names(&results);
        let expected = std::collections::BTreeSet::from([
            "time".to_owned(),
            "a".to_owned(),
            "a_stock".to_owned(),
            "b".to_owned(),
            "c".to_owned(),
            "d".to_owned(),
            "net_flow".to_owned(),
        ]);
        assert_eq!(
            names, expected,
            "{label}: Results should expose time plus all non-control, non-lookup model variables",
        );

        assert!(
            !results
                .offsets
                .contains_key(&Ident::<Canonical>::new("graphical_function")),
            "{label}: lookup definitions should not become Results columns",
        );

        assert!((constant_column_value(&results, &Ident::new("a")) - 1.0).abs() < 1e-9);

        let b_value = constant_column_value(&results, &Ident::new("b"));
        let c_value = constant_column_value(&results, &Ident::new("c"));
        let mut scalar_pair = [b_value, c_value];
        scalar_pair.sort_by(|lhs, rhs| lhs.partial_cmp(rhs).unwrap());
        assert_eq!(
            scalar_pair,
            [3.0, if label.ends_with("4") { 4.0 } else { 3.0 }],
            "{label}: expected scalar constants to survive in Results",
        );

        let d_value = constant_column_value(&results, &Ident::new("d"));
        assert!(
            d_value.is_finite(),
            "{label}: computed auxiliary constant should remain available",
        );

        assert!(
            (constant_column_value(&results, &Ident::new("net_flow")) - expected_net_flow).abs()
                < 1e-6,
            "{label}: expected net_flow constant from the VDF",
        );

        let stock_off = results.offsets[&Ident::new("a_stock")];
        let stock_start = results.data[stock_off];
        let stock_final = results.data[(results.step_count - 1) * results.step_size + stock_off];
        assert!(
            (stock_start - 5.5).abs() < 1e-9,
            "{label}: unexpected a_stock initial value"
        );
        assert!(
            (stock_final - expected_stock_final).abs() < 1e-6,
            "{label}: unexpected a_stock final value",
        );
    }
}

fn result_series(results: &crate::Results, offset: usize) -> Vec<f64> {
    results.iter().map(|row| row[offset]).collect()
}

fn sampled_series_error(
    actual: &crate::Results,
    reference: &crate::Results,
    id: &Ident<Canonical>,
) -> Option<f64> {
    let actual_off = *actual.offsets.get(id)?;
    let reference_off = *reference.offsets.get(id)?;
    let sample_indices = build_sample_indices(actual.step_count);
    let actual_series = result_series(actual, actual_off);
    let reference_series = result_series(reference, reference_off);
    Some(compute_match_error(
        &reference_series,
        &actual_series,
        &sample_indices,
    ))
}

fn matching_visible_series_count(
    actual: &crate::Results,
    reference: &crate::Results,
) -> (usize, usize) {
    let mut shared = 0usize;
    let mut matching = 0usize;

    for id in actual.offsets.keys() {
        let name = id.as_str();
        if name == "time" || name.starts_with('$') || name.starts_with('#') {
            continue;
        }

        let Some(error) = sampled_series_error(actual, reference, id) else {
            continue;
        };
        shared += 1;
        if error < 0.01 {
            matching += 1;
        }
    }

    (shared, matching)
}

#[test]
fn test_to_results_with_model_matches_wrld3_reference_outputs() {
    let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
    let contents = std::fs::read_to_string(mdl_path).unwrap();
    let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
    let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
    let reference = crate::interpreter::Simulation::new(&project, "main")
        .unwrap()
        .run_to_end()
        .unwrap();

    for (label, vdf_path) in [
        ("wrld3-scen01", "../../test/metasd/WRLD3-03/SCEN01.VDF"),
        (
            "wrld3-experiment",
            "../../test/metasd/WRLD3-03/experiment.vdf",
        ),
    ] {
        let vdf = vdf_file(vdf_path);
        let results = vdf
            .to_results_with_model(&project, "main")
            .unwrap_or_else(|e| panic!("{label}: to_results_with_model failed: {e}"));

        let (shared, matching) = matching_visible_series_count(&results, &reference);
        assert!(
            shared >= 230,
            "{label}: expected broad visible overlap with the simulation reference, got {shared}",
        );
        assert!(
            matching >= 225,
            "{label}: expected many WRLD3 series to match the simulation reference, got {matching} of {shared}",
        );

        for name in [
            "population",
            "food",
            "industrial_output",
            "persistent_pollution_index",
            "nonrenewable_resources",
        ] {
            let id = Ident::<Canonical>::new(name);
            let error = sampled_series_error(&results, &reference, &id)
                .unwrap_or_else(|| panic!("{label}: missing sampled comparison for {name}"));
            assert!(
                error < 0.01,
                "{label}: expected {name} to match the simulation reference, error={error}",
            );
        }
    }
}

#[test]
fn test_section6_stock_code_matches_small_model_stock_ots() {
    let models = [
        (
            "../../test/bobby/vdf/water/water.mdl",
            "../../test/bobby/vdf/water/Current.vdf",
        ),
        (
            "../../test/bobby/vdf/pop/pop.mdl",
            "../../test/bobby/vdf/pop/Current.vdf",
        ),
    ];

    for (mdl_path, vdf_path) in models {
        let contents = std::fs::read_to_string(mdl_path).unwrap();
        let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
        let stock_backed = normalized_stock_backed_outputs(&datamodel_project);
        let vdf = vdf_file(vdf_path);
        let codes = vdf.section6_ot_class_codes().unwrap();
        let sf_map = vdf.build_stocks_first_ot_map(&datamodel_project).unwrap();

        for (name, ot) in sf_map {
            if name.as_str() == "time" {
                assert_eq!(codes[ot], VDF_SECTION6_OT_CODE_TIME);
                continue;
            }

            let is_stock_backed = stock_backed.contains(&normalize_vdf_name(name.as_str()));
            assert_eq!(
                codes[ot] == VDF_SECTION6_OT_CODE_STOCK,
                is_stock_backed,
                "{vdf_path}: expected OT[{ot}] {} to be stock_backed={is_stock_backed}, codes={codes:?}",
                name.as_str()
            );
        }
    }
}

#[test]
fn test_section6_stock_code_matches_empirical_econ_results() {
    let mdl_path = "../../test/bobby/vdf/econ/mark2.mdl";
    let vdf_path = "../../test/bobby/vdf/econ/base.vdf";

    let contents = std::fs::read_to_string(mdl_path).unwrap();
    let datamodel_project = crate::compat::open_vensim(&contents).unwrap();
    let stock_backed = normalized_stock_backed_outputs(&datamodel_project);

    let project = std::rc::Rc::new(crate::Project::from(datamodel_project));
    let sim = crate::interpreter::Simulation::new(&project, "main").unwrap();
    let results = sim.run_to_end().unwrap();

    let vdf = vdf_file(vdf_path);
    let codes = vdf.section6_ot_class_codes().unwrap();
    let vdf_data = vdf.extract_data().unwrap();
    let empirical = build_empirical_ot_map(&vdf_data, &results).unwrap();

    for (name, &ot) in &empirical {
        if name.as_str() == "time" {
            assert_eq!(codes[ot], VDF_SECTION6_OT_CODE_TIME);
            continue;
        }
        let expected_stock = stock_backed.contains(&normalize_vdf_name(name.as_str()));
        assert_eq!(
            codes[ot] == VDF_SECTION6_OT_CODE_STOCK,
            expected_stock,
            "econ: OT[{ot}] {} expected stock_backed={expected_stock}",
            name.as_str()
        );
    }
}
