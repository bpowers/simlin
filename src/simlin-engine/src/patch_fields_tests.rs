// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::common::ErrorCode;
use crate::datamodel::{self, Equation};
use crate::patch::{ModelOperation, ModelPatch, ProjectPatch, apply_patch};
use crate::test_common::TestProject;

type Arm = (
    String,
    String,
    Option<String>,
    Option<datamodel::GraphicalFunction>,
);

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

fn arm(subscript: &str, text: &str) -> Arm {
    (subscript.to_string(), text.to_string(), None, None)
}

fn set_element(ident: &str, element: &[&str], text: Option<&str>) -> ModelOperation {
    ModelOperation::SetElementEquation {
        ident: ident.to_string(),
        element: strings(element),
        equation: text.map(str::to_string),
    }
}

fn set_dimensions(ident: &str, dims: &[&str], keep: Option<&[&str]>) -> ModelOperation {
    ModelOperation::SetDimensions {
        ident: ident.to_string(),
        dimensions: strings(dims),
        keep: keep.map(strings),
    }
}

fn values(project: datamodel::Project, ident: &str, elements: &[&str]) -> Vec<f64> {
    let results = TestProject::from_datamodel(project)
        .run_vm()
        .expect("the model runs");
    elements
        .iter()
        .map(|e| *results[&format!("{ident}[{e}]")].last().unwrap())
        .collect()
}

/// `setEquation` sets a scalar's equation and a stock's initial value; on an
/// arrayed variable it sets the equation every element follows and leaves
/// each element's own equation.
#[test]
fn set_equation_sets_the_equation_every_element_follows() {
    let base = ages()
        .stock("level", "1", &["inflow"], &[], None)
        .array_with_default_and_overrides("d[Age]", "c * 2", vec![("Young", "inflow")])
        .array_with_ranges("e[Age]", vec![("Young", "1")])
        .build_datamodel();
    let set = |ident: &str, text: &str| ModelOperation::SetEquation {
        ident: ident.to_string(),
        equation: text.to_string(),
    };
    let edited = edit(&base, set("inflow", "6")).unwrap();
    assert_eq!(
        equation(&edited, "inflow"),
        Equation::Scalar("6".to_string())
    );
    let edited = edit(&base, set("level", "2")).unwrap();
    assert_eq!(
        equation(&edited, "level"),
        Equation::Scalar("2".to_string())
    );
    let edited = edit(&base, set("c", "7")).unwrap();
    assert_eq!(
        equation(&edited, "c"),
        Equation::Arrayed(
            strings(&["Age"]),
            vec![arm("Young", "10"), arm("Adult", "20"), arm("Old", "30")],
            Some("7".to_string()),
            true
        ),
        "every element of c has its own equation, which it keeps"
    );
    let edited = edit(&base, set("d", "c * 3")).unwrap();
    assert_eq!(
        equation(&edited, "d"),
        Equation::Arrayed(
            strings(&["Age"]),
            vec![arm("Young", "inflow")],
            Some("c * 3".to_string()),
            true
        )
    );
    // An arrayed variable with no equation for every element gains one,
    // which its elements with none then follow.
    let edited = edit(&base, set("e", "0")).unwrap();
    assert_eq!(
        values(edited, "e", &["young", "adult", "old"]),
        vec![1.0, 0.0, 0.0]
    );
}

/// `setElementEquation` gives one element an equation of its own and clears
/// it again; the stored form stays as small as it can be.
#[test]
fn an_element_equation_is_set_and_cleared() {
    let base = ages().array_aux("d[Age]", "c * 2").build_datamodel();
    let own = edit(&base, set_element("d", &["old"], Some("inflow"))).unwrap();
    assert_eq!(
        equation(&own, "d"),
        Equation::Arrayed(
            strings(&["Age"]),
            vec![arm("Old", "inflow")],
            Some("c * 2".to_string()),
            true
        ),
        "the element is spelled as the dimension spells it"
    );
    assert_eq!(
        values(own.clone(), "d", &["young", "adult", "old"]),
        vec![20.0, 40.0, 5.0]
    );
    // Elements are stored in their dimension's order, whatever order they
    // are given in.
    let two = edit(&own, set_element("d", &["Young"], Some("1"))).unwrap();
    let Equation::Arrayed(_, arms, ..) = equation(&two, "d") else {
        panic!("arrayed")
    };
    assert_eq!(
        arms.iter().map(|a| a.0.as_str()).collect::<Vec<_>>(),
        vec!["Young", "Old"]
    );
    // Clearing the last element's own equation leaves the equation for every
    // element alone: apply-to-all again.
    let cleared = edit(&own, set_element("d", &["Old"], None)).unwrap();
    assert_eq!(
        equation(&cleared, "d"),
        Equation::ApplyToAll(strings(&["Age"]), "c * 2".to_string())
    );
    // Clearing an element of a variable with no equation for every element
    // leaves it with none.
    let cleared = edit(
        &ages().build_datamodel(),
        set_element("c", &["Adult"], None),
    )
    .unwrap();
    assert_eq!(
        values(cleared, "c", &["young", "adult", "old"]),
        vec![10.0, 0.0, 30.0]
    );
}

/// An element of two dimensions names one element on each, in order.
#[test]
fn an_element_of_two_dimensions() {
    let base = ages().array_aux("p[Region, Age]", "1").build_datamodel();
    let edited = edit(&base, set_element("p", &["south", "old"], Some("9"))).unwrap();
    assert_eq!(
        values(edited, "p", &["north,old", "south,old"]),
        vec![1.0, 9.0]
    );
}

/// What `setElementEquation` refuses, changing nothing: a scalar, an element
/// that is not one, the wrong number of parts, a variable the model lacks.
#[test]
fn what_an_element_edit_refuses() {
    let base = ages().array_aux("p[Region, Age]", "1").build_datamodel();
    assert_eq!(
        edit(&base, set_element("inflow", &["young"], Some("1"))).err(),
        Some(ErrorCode::Generic)
    );
    assert_eq!(
        edit(&base, set_element("c", &["elder"], Some("1"))).err(),
        Some(ErrorCode::BadDimensionName)
    );
    assert_eq!(
        edit(&base, set_element("p", &["north"], Some("1"))).err(),
        Some(ErrorCode::MismatchedDimensions)
    );
    assert_eq!(
        edit(&base, set_element("nothing", &["north"], Some("1"))).err(),
        Some(ErrorCode::DoesNotExist)
    );
}

/// Units and notes are set and cleared.
#[test]
fn units_and_notes() {
    let base = ages().build_datamodel();
    let units = |units: &str| ModelOperation::SetUnits {
        ident: "inflow".to_string(),
        units: Some(units.to_string()),
    };
    let edited = edit(&base, units(" people ")).unwrap();
    assert_eq!(
        edited.models[0].get_variable("inflow").unwrap().get_units(),
        Some(&"people".to_string())
    );
    let cleared = edit(&edited, units("")).unwrap();
    assert_eq!(
        cleared.models[0]
            .get_variable("inflow")
            .unwrap()
            .get_units(),
        None
    );
    let noted = edit(
        &base,
        ModelOperation::SetNotes {
            ident: "inflow".to_string(),
            notes: "how many arrive".to_string(),
        },
    )
    .unwrap();
    let datamodel::Variable::Aux(aux) = noted.models[0].get_variable("inflow").unwrap() else {
        panic!("an aux")
    };
    assert_eq!(aux.documentation, "how many arrive");
}

/// A lookup is set on the variable or on one element, and removed.
#[test]
fn a_lookup_on_the_variable_or_one_element() {
    let base = ages().array_aux("d[Age]", "c").build_datamodel();
    let table = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 100.0]),
        y_points: vec![0.0, 1.0],
        x_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 100.0,
        },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
    };
    let set = |element: Option<&[&str]>, lookup: Option<datamodel::GraphicalFunction>| {
        ModelOperation::SetLookup {
            ident: "d".to_string(),
            element: element.map(strings),
            lookup,
        }
    };
    let on_old = edit(&base, set(Some(&["old"]), Some(table.clone()))).unwrap();
    // The element reads its own table at the equation every element follows.
    assert_eq!(
        values(on_old.clone(), "d", &["young", "adult", "old"]),
        vec![10.0, 20.0, 0.3]
    );
    let removed = edit(&on_old, set(Some(&["old"]), None)).unwrap();
    assert_eq!(equation(&removed, "d"), equation(&base, "d"));
    let whole = edit(&base, set(None, Some(table))).unwrap();
    let datamodel::Variable::Aux(aux) = whole.models[0].get_variable("d").unwrap() else {
        panic!("an aux")
    };
    assert!(aux.gf.is_some());
    assert_eq!(
        edit(
            &ages().stock("level", "1", &[], &[], None).build_datamodel(),
            ModelOperation::SetLookup {
                ident: "level".to_string(),
                element: None,
                lookup: None,
            }
        )
        .err(),
        Some(ErrorCode::Generic),
        "a stock has no lookup"
    );
}

/// A lookup set on one element of a variable that has a lookup of its own
/// leaves the other elements reading through the variable's.
#[test]
fn an_elements_lookup_leaves_the_others_reading_through_the_variables() {
    let table = |top: f64| datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 100.0]),
        y_points: vec![0.0, top],
        x_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 100.0,
        },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: top },
    };
    let mut base = ages().array_aux("d[Age]", "c").build_datamodel();
    base = edit(
        &base,
        ModelOperation::SetLookup {
            ident: "d".to_string(),
            element: None,
            lookup: Some(table(1.0)),
        },
    )
    .unwrap();
    assert_eq!(
        values(base.clone(), "d", &["young", "adult", "old"]),
        vec![0.1, 0.2, 0.3]
    );
    let on_old = edit(
        &base,
        ModelOperation::SetLookup {
            ident: "d".to_string(),
            element: Some(strings(&["old"])),
            lookup: Some(table(10.0)),
        },
    )
    .unwrap();
    assert_eq!(
        values(on_old, "d", &["young", "adult", "old"]),
        vec![0.1, 0.2, 3.0]
    );
}

/// An element given an empty equation has none of its own: it follows the
/// equation every element follows, and an MDL save keeps that.
#[test]
fn an_empty_element_equation_is_none() {
    use crate::save_check::{SaveFormat, check_save};
    let base = ages().array_aux("d[Age]", "c * 2").build_datamodel();
    for text in ["", "  "] {
        let edited = edit(&base, set_element("d", &["old"], Some(text))).unwrap();
        assert_eq!(equation(&edited, "d"), equation(&base, "d"), "{text:?}");
        assert!(check_save(&edited, SaveFormat::Mdl).unwrap().is_empty());
    }
}

/// A variable made a scalar from an element whose only definition of its own
/// is a lookup keeps the equation every element follows as that lookup's
/// input.
#[test]
fn a_scalar_kept_from_an_element_with_a_lookup_alone() {
    let table = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 100.0]),
        y_points: vec![0.0, 50.0],
        x_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 100.0,
        },
        y_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 50.0,
        },
    };
    let mut base = ages().array_aux("d[Age]", "0").build_datamodel();
    let at = base.models[0]
        .variables
        .iter()
        .position(|v| v.get_ident() == "d")
        .unwrap();
    let mut d = base.models[0].variables[at].clone();
    if let datamodel::Variable::Aux(aux) = &mut d {
        aux.equation = Equation::Arrayed(
            strings(&["Age"]),
            vec![("Old".to_string(), String::new(), None, Some(table))],
            Some("c[Young] * 3".to_string()),
            true,
        );
    }
    base.models[0].variables.replace(at, d);
    let before = values(base.clone(), "d", &["old"]);
    let scalar = edit(&base, set_dimensions("d", &[], Some(&["Old"]))).unwrap();
    assert_eq!(
        equation(&scalar, "d"),
        Equation::Scalar("c[Young] * 3".to_string())
    );
    let results = TestProject::from_datamodel(scalar).run_vm().unwrap();
    assert_eq!(results["d"].last(), before.last());
}

/// Adding a dimension repeats the variable along it: a scalar's equation
/// becomes the equation every element follows, and an element's own
/// equation is its equation at every element of the new dimension.
#[test]
fn adding_a_dimension_repeats_the_variable_along_it() {
    let base = ages().build_datamodel();
    let split = edit(&base, set_dimensions("inflow", &["Region"], None)).unwrap();
    assert_eq!(
        equation(&split, "inflow"),
        Equation::ApplyToAll(strings(&["Region"]), "5".to_string())
    );
    let wider = edit(&base, set_dimensions("c", &["Region", "Age"], None)).unwrap();
    assert_eq!(
        values(wider, "c", &["north,young", "south,young", "south,old"]),
        vec![10.0, 10.0, 30.0]
    );
}

/// Removing a dimension keeps what every element along it says, or the
/// element `keep` names, and is refused when the elements differ and no
/// element is named.
#[test]
fn removing_a_dimension() {
    let base = ages()
        .array_aux("same[Region, Age]", "c")
        .array_with_default_and_overrides("mixed[Region]", "1", vec![("South", "2")])
        .build_datamodel();
    let narrower = edit(&base, set_dimensions("same", &["Age"], None)).unwrap();
    assert_eq!(
        equation(&narrower, "same"),
        Equation::ApplyToAll(strings(&["Age"]), "c".to_string())
    );
    assert_eq!(
        edit(&base, set_dimensions("mixed", &[], None)).err(),
        Some(ErrorCode::MismatchedDimensions),
        "North and South differ"
    );
    let south = edit(&base, set_dimensions("mixed", &[], Some(&["south"]))).unwrap();
    assert_eq!(equation(&south, "mixed"), Equation::Scalar("2".to_string()));
    let north = edit(&base, set_dimensions("mixed", &[], Some(&["North"]))).unwrap();
    assert_eq!(equation(&north, "mixed"), Equation::Scalar("1".to_string()));
    // Every element of c has its own equation: removing Age keeps one.
    let one = edit(&base, set_dimensions("c", &[], Some(&["Adult"]))).unwrap();
    assert_eq!(equation(&one, "c"), Equation::Scalar("20".to_string()));
}

/// Reordering the dimensions moves each element's own equation with it.
#[test]
fn reordering_the_dimensions_moves_each_elements_equation() {
    let base = ages()
        .array_with_default_and_overrides("p[Region, Age]", "0", vec![("South,Old", "9")])
        .build_datamodel();
    let turned = edit(&base, set_dimensions("p", &["Age", "Region"], None)).unwrap();
    assert_eq!(
        equation(&turned, "p"),
        Equation::Arrayed(
            strings(&["Age", "Region"]),
            vec![arm("Old,South", "9")],
            Some("0".to_string()),
            true
        )
    );
}

/// What `setDimensions` refuses: a dimension the project lacks, one named
/// twice, a `keep` of the wrong length.
#[test]
fn what_a_dimensions_edit_refuses() {
    let base = ages().build_datamodel();
    assert_eq!(
        edit(&base, set_dimensions("inflow", &["Nowhere"], None)).err(),
        Some(ErrorCode::BadDimensionName)
    );
    assert_eq!(
        edit(&base, set_dimensions("inflow", &["Age", "age"], None)).err(),
        Some(ErrorCode::MismatchedDimensions)
    );
    assert_eq!(
        edit(&base, set_dimensions("c", &[], Some(&["Young", "North"]))).err(),
        Some(ErrorCode::MismatchedDimensions)
    );
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
    use crate::save_check::{SaveFormat, check_save};
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
