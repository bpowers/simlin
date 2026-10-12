// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The dimension edits, held to the run: an edit that renames computes what
//! the model computed before, under the new names; one that removes an
//! element takes what was there with it; and every refusal leaves the
//! project as it was.

use std::collections::HashMap;

use crate::common::ErrorCode;
use crate::datamodel::{self, Dimension, Equation, Project};
use crate::patch::{ProjectOperation, ProjectPatch, apply_patch};
use crate::test_common::TestProject;

fn apply(project: &Project, ops: Vec<ProjectOperation>) -> Result<Project, (ErrorCode, String)> {
    let mut edited = project.clone();
    apply_patch(
        &mut edited,
        ProjectPatch {
            project_ops: ops,
            models: vec![],
        },
    )
    .map_err(|err| (err.code, err.details.unwrap_or_default()))?;
    Ok(edited)
}

fn run(project: &Project) -> HashMap<String, Vec<f64>> {
    TestProject::from_datamodel(project.clone())
        .run_vm()
        .expect("the model runs")
}

fn equation(project: &Project, ident: &str) -> Equation {
    project.models[0]
        .get_variable(ident)
        .and_then(|v| v.get_equation())
        .cloned()
        .unwrap_or_else(|| panic!("no equation for {ident}"))
}

fn named(name: &str, elements: &[&str]) -> Dimension {
    Dimension::named(
        name.to_string(),
        elements.iter().map(|e| e.to_string()).collect(),
    )
}

/// An aging chain over a dimension with a subrange and a mapped subrange,
/// whose equations name the dimension every way an equation can.
fn ages() -> Project {
    TestProject::new("ages")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("Age", &["young", "adult", "old"])
        .named_dimension("Older", &["adult", "old"])
        .named_dimension_with_mapping("Younger", &["young", "adult"], "Older")
        .array_with_ranges(
            "c[Age]",
            vec![("young", "10"), ("adult", "20"), ("old", "30")],
        )
        .array_with_default_and_overrides("d[Age]", "c[Age] * 2", vec![("old", "c[old] + 1")])
        .array_aux("before[Older]", "c[Younger]")
        .aux("total", "SUM(c[*:Older]) + c[young]", None)
        .array_aux("position[Age]", "IF Age > 1 THEN Age ELSE Age.adult")
        .array_aux("span[Age]", "SUM(c[young:adult])")
        .build_datamodel()
}

#[test]
fn a_renamed_dimension_computes_what_it_computed() {
    let before = ages();
    let after = apply(
        &before,
        vec![
            ProjectOperation::RenameDimension {
                from: "Age".to_string(),
                to: "Cohort".to_string(),
            },
            ProjectOperation::RenameDimension {
                from: "Older".to_string(),
                to: "Elders".to_string(),
            },
        ],
    )
    .expect("the renames apply");
    assert_eq!(run(&before), run(&after));
    assert_eq!(
        equation(&after, "position"),
        Equation::ApplyToAll(
            vec!["Cohort".to_string()],
            "IF Cohort > 1 THEN Cohort ELSE Cohort.adult".to_string()
        )
    );
    assert_eq!(
        equation(&after, "total"),
        Equation::Scalar("SUM(c[*:Elders]) + c[young]".to_string())
    );
    assert!(matches!(
        equation(&after, "d"),
        Equation::Arrayed(dims, _, Some(default), true)
            if dims == ["Cohort"] && default == "c[Cohort] * 2"
    ));
    let younger = after
        .dimensions
        .iter()
        .find(|d| d.name == "Younger")
        .unwrap();
    assert_eq!(younger.mappings[0].target, "Elders");
}

#[test]
fn a_renamed_element_computes_what_it_computed_under_its_new_name() {
    let before = ages();
    let after = apply(
        &before,
        vec![ProjectOperation::RenameElement {
            dimension: "Age".to_string(),
            from: "adult".to_string(),
            to: "grown".to_string(),
        }],
    )
    .expect("the rename applies");
    let (was, is) = (run(&before), run(&after));
    for (key, series) in &was {
        let renamed = key.replace("[adult]", "[grown]");
        assert_eq!(is.get(&renamed), Some(series), "{key}");
    }
    // Both subranges declare `adult` as one element with `Age`'s, and every
    // subscript and qualified position that read it reads `grown`.
    for name in ["Age", "Older", "Younger"] {
        let dim = after.dimensions.iter().find(|d| d.name == name).unwrap();
        assert!(elements_named(dim).contains(&"grown".to_string()), "{name}");
    }
    assert_eq!(
        equation(&after, "span"),
        Equation::ApplyToAll(vec!["Age".to_string()], "SUM(c[young:grown])".to_string())
    );
    assert_eq!(
        equation(&after, "position"),
        Equation::ApplyToAll(
            vec!["Age".to_string()],
            "IF Age > 1 THEN Age ELSE Age.grown".to_string()
        )
    );
    let Equation::Arrayed(_, arms, ..) = equation(&after, "c") else {
        panic!("c stays arrayed");
    };
    assert_eq!(arms[1].0, "grown");
}

#[test]
fn an_element_map_follows_a_renamed_element() {
    let before = TestProject::new("mapped")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Region", &["north", "south"])
        .named_dimension_with_element_mapping(
            "Area",
            &["n", "s"],
            "Region",
            &[("n", "south"), ("s", "north")],
        )
        .array_with_ranges("x[Region]", vec![("north", "1"), ("south", "2")])
        .array_aux("y[Area]", "x[Region]")
        .build_datamodel();
    let after = apply(
        &before,
        vec![
            ProjectOperation::RenameElement {
                dimension: "Region".to_string(),
                from: "north".to_string(),
                to: "upper".to_string(),
            },
            ProjectOperation::RenameElement {
                dimension: "Area".to_string(),
                from: "s".to_string(),
                to: "lower area".to_string(),
            },
        ],
    )
    .expect("the renames apply");
    let (was, is) = (run(&before), run(&after));
    assert_eq!(was["y[n]"], is["y[n]"]);
    assert_eq!(was["y[s]"], is["y[lower_area]"]);
}

#[test]
fn an_element_removed_takes_its_own_equations_and_one_added_follows_the_default() {
    let before = ages();
    let after = apply(
        &before,
        vec![
            ProjectOperation::UpsertDimension(named("Older", &["adult"])),
            ProjectOperation::UpsertDimension(named("Younger", &["young"])),
        ],
    );
    // `Younger` maps positionally onto `Older`; both shrink together.
    let after = after.expect("the subranges shrink");
    let after = apply(
        &after,
        vec![ProjectOperation::UpsertDimension(named(
            "Age",
            &["young", "adult", "elder"],
        ))],
    )
    .expect("old goes, elder comes");
    // `d[old]` had an equation of its own, which goes with the element; `d`
    // then follows its one equation everywhere, and `elder` follows it too.
    assert_eq!(
        equation(&after, "d"),
        Equation::ApplyToAll(vec!["Age".to_string()], "c[Age] * 2".to_string())
    );
    let Equation::Arrayed(_, arms, None, false) = equation(&after, "c") else {
        panic!("c keeps its own elements");
    };
    let keys: Vec<&str> = arms.iter().map(|arm| arm.0.as_str()).collect();
    assert_eq!(keys, ["young", "adult"]);
}

#[test]
fn reordered_elements_keep_each_elements_equation_in_the_new_order() {
    let before = ages();
    let after = apply(
        &before,
        vec![ProjectOperation::UpsertDimension(named(
            "Age",
            &["old", "young", "adult"],
        ))],
    )
    .expect("the order changes");
    let Equation::Arrayed(_, arms, ..) = equation(&after, "c") else {
        panic!("c stays arrayed");
    };
    let keys: Vec<&str> = arms.iter().map(|arm| arm.0.as_str()).collect();
    assert_eq!(keys, ["old", "young", "adult"]);
    let (was, is) = (run(&before), run(&after));
    for element in ["young", "adult", "old"] {
        let key = format!("c[{element}]");
        assert_eq!(was[&key], is[&key], "{key}");
    }
}

#[test]
fn a_new_dimension_is_added_and_an_unused_one_deleted() {
    let before = ages();
    let after = apply(
        &before,
        vec![ProjectOperation::UpsertDimension(named(
            "Size",
            &["s", "m"],
        ))],
    )
    .expect("added");
    assert_eq!(after.dimensions.last().unwrap().name, "Size");
    let after = apply(
        &after,
        vec![ProjectOperation::DeleteDimension {
            name: "size".to_string(),
        }],
    )
    .expect("deleted");
    assert!(after == before);
}

#[test]
fn every_refusal_says_why_and_changes_nothing() {
    let before = ages();
    let refused = |ops: Vec<ProjectOperation>, code: ErrorCode, says: &str| {
        let (got, details) = apply(&before, ops).expect_err(says);
        assert_eq!(got, code, "{details}");
        assert!(details.contains(says), "{details:?} should say {says:?}");
    };
    let rename_dim = |from: &str, to: &str| ProjectOperation::RenameDimension {
        from: from.to_string(),
        to: to.to_string(),
    };
    let rename_elem = |dim: &str, from: &str, to: &str| ProjectOperation::RenameElement {
        dimension: dim.to_string(),
        from: from.to_string(),
        to: to.to_string(),
    };
    refused(
        vec![rename_dim("Age", "Older")],
        ErrorCode::DuplicateVariable,
        "already exists",
    );
    refused(
        vec![rename_dim("Age", "total")],
        ErrorCode::DuplicateVariable,
        "is a variable",
    );
    refused(
        vec![rename_dim("Age", "young")],
        ErrorCode::DuplicateVariable,
        "also an element of 'Age'",
    );
    refused(
        vec![rename_dim("Nothing", "Else")],
        ErrorCode::BadDimensionName,
        "not a declared dimension",
    );
    refused(
        vec![rename_elem("Age", "adult", "old")],
        ErrorCode::DuplicateVariable,
        "already has an element named 'old'",
    );
    refused(
        vec![rename_elem("Age", "adult", "total")],
        ErrorCode::DuplicateVariable,
        "is a variable",
    );
    refused(
        vec![rename_elem("Age", "adult", "Older")],
        ErrorCode::DuplicateVariable,
        "a dimension's name",
    );
    refused(
        vec![rename_elem("Age", "nobody", "else")],
        ErrorCode::BadDimensionName,
        "not an element",
    );
    refused(
        vec![ProjectOperation::UpsertDimension(named(
            "Age",
            &["young", "adult"],
        ))],
        ErrorCode::BadDimensionName,
        "a subrange of 'Age'",
    );
    refused(
        vec![ProjectOperation::UpsertDimension(named(
            "Size",
            &["s", "S"],
        ))],
        ErrorCode::BadDimensionName,
        "twice",
    );
    refused(
        vec![ProjectOperation::UpsertDimension(named("c", &["s"]))],
        ErrorCode::DuplicateVariable,
        "variable 'c'",
    );
    refused(
        vec![ProjectOperation::DeleteDimension {
            name: "Older".to_string(),
        }],
        ErrorCode::BadDimensionName,
        "'before', 'total', the dimension 'Younger' still use it",
    );
    refused(
        vec![ProjectOperation::DeleteDimension {
            name: "Older".to_string(),
        }],
        ErrorCode::BadDimensionName,
        "the dimension 'Younger'",
    );
    // A name written in quotes cannot qualify an element (`Age.adult`) or a
    // star range (`Age.*`); the equation would no longer read back.
    refused(
        vec![rename_elem("Age", "adult", "65+")],
        ErrorCode::BadDimensionName,
        "the equation of 'position' writes it qualified",
    );
    refused(
        vec![rename_dim("Age", "Age (years)")],
        ErrorCode::BadDimensionName,
        "Choose a name that needs no quotes",
    );
    // An element named as a variable would make `c[idx]` mean the element,
    // where it meant the variable.
    let with_idx = TestProject::new("idx")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Age", &["young", "adult", "old"])
        .array_with_ranges(
            "c[Age]",
            vec![("young", "10"), ("adult", "20"), ("old", "30")],
        )
        .aux("idx", "2", None)
        .array_aux("e[Age]", "c[idx]")
        .build_datamodel();
    let (got, details) = apply(
        &with_idx,
        vec![ProjectOperation::UpsertDimension(named(
            "Age",
            &["young", "adult", "old", "idx"],
        ))],
    )
    .expect_err("an element named as a variable");
    assert_eq!(got, ErrorCode::DuplicateVariable, "{details}");
    assert!(details.contains("'idx' is a variable"), "{details}");
    let (got, details) = apply(
        &with_idx,
        vec![ProjectOperation::UpsertDimension(named(
            "Size",
            &["s", "idx"],
        ))],
    )
    .expect_err("an element named as a variable, in a new dimension");
    assert_eq!(got, ErrorCode::DuplicateVariable, "{details}");
}

/// A name that has to be written in quotes is written in quotes as the
/// model spells it, wherever no qualified spelling needs it bare.
#[test]
fn a_rename_to_a_name_written_in_quotes_computes_what_it_computed() {
    let before = TestProject::new("quoted")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Age", &["young", "adult", "old"])
        .array_with_ranges(
            "c[Age]",
            vec![("young", "10"), ("adult", "20"), ("old", "30")],
        )
        .aux("t", "c[old] + SUM(c[*:Age])", None)
        .build_datamodel();
    let after = apply(
        &before,
        vec![
            ProjectOperation::RenameElement {
                dimension: "Age".to_string(),
                from: "old".to_string(),
                to: "65+".to_string(),
            },
            ProjectOperation::RenameDimension {
                from: "Age".to_string(),
                to: "Age (Years)".to_string(),
            },
        ],
    )
    .expect("nothing qualifies either name");
    assert_eq!(
        equation(&after, "t"),
        Equation::Scalar("c[\"65+\"] + SUM(c[*:\"Age (Years)\"])".to_string())
    );
    assert_eq!(run(&after)["t"], run(&before)["t"]);
}

#[test]
fn an_element_two_unrelated_dimensions_declare_is_not_renamed() {
    let before = TestProject::new("shared")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Region", &["north", "south"])
        .named_dimension("Wind", &["north", "east"])
        .array_aux("x[Region]", "1")
        .array_aux("y[Wind]", "x[north]")
        .build_datamodel();
    let (code, details) = apply(
        &before,
        vec![ProjectOperation::RenameElement {
            dimension: "Region".to_string(),
            from: "north".to_string(),
            to: "top".to_string(),
        }],
    )
    .expect_err("north means either");
    assert_eq!(code, ErrorCode::DuplicateVariable);
    assert!(details.contains("'Wind'"), "{details}");
}

#[test]
fn a_star_range_written_either_way_follows_its_renamed_dimension() {
    let before = TestProject::new("star")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Age", &["young", "old"])
        .named_dimension("Sub", &["old"])
        .array_with_ranges("c[Age]", vec![("young", "1"), ("old", "2")])
        .aux("a", "SUM(c[*:Sub])", None)
        .aux("b", "SUM(c[Sub.*]) + SUM(c[*: Sub])", None)
        .build_datamodel();
    let after = apply(
        &before,
        vec![ProjectOperation::RenameDimension {
            from: "Sub".to_string(),
            to: "Tail".to_string(),
        }],
    )
    .expect("renamed");
    assert_eq!(
        equation(&after, "a"),
        Equation::Scalar("SUM(c[*:Tail])".to_string())
    );
    assert_eq!(
        equation(&after, "b"),
        Equation::Scalar("SUM(c[Tail.*]) + SUM(c[*: Tail])".to_string())
    );
    assert_eq!(run(&before), run(&after));
}

fn elements_named(dim: &Dimension) -> Vec<String> {
    match &dim.elements {
        datamodel::DimensionElements::Named(names) => names.clone(),
        datamodel::DimensionElements::Indexed(_) => Vec::new(),
    }
}

/// A dimension may not be named like an element of another, nor gain an
/// element named like a dimension: a subscript that writes the name could
/// mean either. The rename edits refuse the same.
#[test]
fn a_dimension_and_an_element_may_not_share_a_name() {
    let before = ages();
    let element = before.dimensions[0].clone();
    let taken = match &element.elements {
        datamodel::DimensionElements::Named(names) => names[0].clone(),
        _ => panic!("named"),
    };
    let (code, why) = apply(
        &before,
        vec![ProjectOperation::UpsertDimension(named(
            &taken,
            &["p", "q"],
        ))],
    )
    .expect_err("a dimension named like an element");
    assert_eq!(code, ErrorCode::DuplicateVariable, "{why}");
    let (code, why) = apply(
        &before,
        vec![ProjectOperation::UpsertDimension(named(
            "Size",
            &["s", &element.name],
        ))],
    )
    .expect_err("an element named like a dimension");
    assert_eq!(code, ErrorCode::DuplicateVariable, "{why}");
}
