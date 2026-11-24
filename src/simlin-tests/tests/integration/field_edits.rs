// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What an edit by field stores, saved.

use simlin_base::common::ErrorCode;
use simlin_base::datamodel::{self, Equation};
use simlin_base::project_patch::{ModelOperation, ModelPatch, ProjectPatch};
use simlin_edit::apply_patch;
use simlin_engine::save_check::{SaveFormat, check_save};
use simlin_engine::test_common::TestProject;

fn ages() -> TestProject {
    TestProject::new("fields")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Age", &["Young", "Adult", "Old"])
        .named_dimension("Region", &["North", "South"])
        .array_with_ranges(
            "c[Age]",
            vec![("Young", "10"), ("Adult", "20"), ("Old", "30")],
        )
        .aux("inflow", "5", None)
}

fn edit(project: &datamodel::Project, op: ModelOperation) -> Result<datamodel::Project, ErrorCode> {
    let mut edited = project.clone();
    apply_patch(
        &mut edited,
        ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![op],
            }],
        },
    )
    .map(|()| edited)
    .map_err(|e| e.code)
}

fn equation(project: &datamodel::Project, ident: &str) -> Equation {
    project.models[0]
        .get_variable(ident)
        .unwrap()
        .get_equation()
        .unwrap()
        .clone()
}

fn strings(s: &[&str]) -> Vec<String> {
    s.iter().map(|s| s.to_string()).collect()
}

fn set_element(ident: &str, element: &[&str], text: Option<&str>) -> ModelOperation {
    ModelOperation::SetElementEquation {
        ident: ident.to_string(),
        element: strings(element),
        equation: text.map(str::to_string),
    }
}

/// An element given an empty equation has none of its own: it follows the
/// equation every element follows, and an MDL save keeps that.
#[test]
fn an_empty_element_equation_is_none() {
    let base = ages().array_aux("d[Age]", "c * 2").build_datamodel();
    for text in ["", "  "] {
        let edited = edit(&base, set_element("d", &["old"], Some(text))).unwrap();
        assert_eq!(equation(&edited, "d"), equation(&base, "d"), "{text:?}");
        assert!(check_save(&edited, SaveFormat::Mdl).unwrap().is_empty());
    }
}

/// What the field edits store -- one equation for every element, elements
/// with equations and lookups of their own, elements of two dimensions -- is
/// kept by a save in every format Simlin writes and reads back: the save
/// check reads each save back and finds the same definitions and, run, the
/// same results. (An equation for every element that no element follows is
/// the one thing an MDL save does not keep: `:EXCEPT:` cannot except every
/// element.)
#[test]
fn what_the_field_edits_store_round_trips_through_every_format() {
    let mut project = ages()
        .with_sim_time(0.0, 2.0, 1.0)
        .array_aux("d[Age]", "c * 2")
        .array_aux("p[Region, Age]", "c + 1")
        .build_datamodel();
    for op in [
        set_element("d", &["Old"], Some("inflow")),
        set_element("p", &["North", "Young"], Some("c[Adult] / 4")),
        set_element("p", &["South", "Adult"], Some("0")),
        ModelOperation::SetLookup {
            ident: "d".to_string(),
            element: Some(strings(&["Adult"])),
            lookup: Some(datamodel::GraphicalFunction {
                kind: datamodel::GraphicalFunctionKind::Continuous,
                x_points: Some(vec![0.0, 100.0]),
                y_points: vec![0.0, 1.0],
                x_scale: datamodel::GraphicalFunctionScale {
                    min: 0.0,
                    max: 100.0,
                },
                y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
            }),
        },
    ] {
        project = edit(&project, op).unwrap();
    }
    assert!(matches!(
        equation(&project, "d"),
        Equation::Arrayed(_, _, Some(_), true)
    ));
    for format in [
        SaveFormat::Xmile,
        SaveFormat::Json,
        SaveFormat::Mdl,
        SaveFormat::Protobuf,
    ] {
        let changes = check_save(&project, format).expect("the save reads back");
        assert!(
            changes.is_empty(),
            "a {} save keeps the model: {:?}",
            format.name(),
            changes.iter().map(|c| c.reason.clone()).collect::<Vec<_>>()
        );
    }
}
