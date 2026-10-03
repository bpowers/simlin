// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What an edit reaches beyond the variable it names: the references a
//! rename respells and a delete leaves dangling, in every model and in every
//! text a variable holds (a macro's parameters, a module instance's reads, a
//! conveyor's options). A rename's own rule is `patch::Rename`'s; these hold
//! the tool to it through `edit_model`, by the runs.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use super::tests::{corpus, edit, operations, read, refused};
use crate::datamodel;
use crate::tools::Session;
use crate::tools::test_support::Host;

/// Every series of the run of the project's model `model` that belongs to a
/// variable of the model (`save_check::column_variable`: the compiler's own
/// helpers aside), by variable and the rest of its key, as a session of its
/// own reads them.
fn series(host: &mut Host, model: &str) -> BTreeMap<(String, String), Vec<f64>> {
    let declared: std::collections::BTreeSet<String> =
        crate::tools::resolve_datamodel_model(&host.project, model)
            .expect("the model")
            .variables
            .iter()
            .map(|var| crate::canonicalize(var.get_ident()).into_owned())
            .collect();
    let mut session = Session::new(model);
    let run = session
        .run_results(host.workspace(), "current")
        .unwrap_or_else(|unavailable| panic!("{model} runs: {}", unavailable.reason));
    run.results
        .offsets
        .iter()
        .filter_map(|(key, &offset)| {
            let key = key.as_str();
            let owner = crate::save_check::column_variable(key, &declared)?;
            let values = run.results.iter().map(|row| row[offset]).collect();
            Some(((owner.to_string(), key[owner.len()..].to_string()), values))
        })
        .collect()
}

/// `before` with each key respelled as `respell` says, its variable and the
/// rest of it apart: what the same run reads as after a rename.
fn respelled(
    before: &BTreeMap<(String, String), Vec<f64>>,
    respell: impl Fn(&str, &str) -> (String, String),
) -> BTreeMap<(String, String), Vec<f64>> {
    before
        .iter()
        .map(|((owner, rest), values)| (respell(owner, rest), values.clone()))
        .collect()
}

fn rename(variable: &str, to: &str) -> Value {
    operations(json!([{"op": "rename", "variable": variable, "to": to}]))
}

/// A rename of a macro's parameter respells the parameter where the macro
/// declares it, so every call of the macro computes what it computed: the
/// corpus macro's call stays 5.5.
#[test]
fn a_rename_of_a_macros_parameter_keeps_its_calls_computing_as_before() {
    let project = corpus("test-models/tests/macro_expression/test_macro_expression.mdl");
    let macro_model = project
        .models
        .iter()
        .find(|model| model.macro_spec.is_some())
        .expect("the corpus model declares a macro");
    let spec = macro_model.macro_spec.clone().unwrap();
    let macro_name = macro_model.name.clone();
    let mut host = Host::new(project);
    let before = series(&mut host, "main");
    assert_eq!(
        before[&("macro_output".to_string(), String::new())],
        [5.5, 5.5],
        "the premise"
    );

    let mut session = Session::new(&macro_name);
    read(&mut host, &mut session);
    edit(
        &mut host,
        &mut session,
        rename(&spec.parameters[0], "renamed parameter"),
    );
    let after = host
        .project
        .models
        .iter()
        .find(|model| model.name == macro_name)
        .and_then(|model| model.macro_spec.clone())
        .unwrap();
    assert_eq!(after.parameters[0], "renamed_parameter", "{after:?}");
    assert_eq!(series(&mut host, "main"), before);

    // And of its output.
    edit(
        &mut host,
        &mut session,
        rename(&spec.primary_output, "renamed output"),
    );
    assert_eq!(series(&mut host, "main"), before);
}

/// A rename of the variable a conveyor's transit time names respells the
/// conveyor's `len` with it: the edit is made and the model computes the
/// same, the renamed column aside.
#[test]
fn a_rename_of_a_conveyors_transit_time_respells_the_conveyor() {
    let mut host = Host::new(corpus("conveyors/arrayed_conveyor.xmile"));
    let before = series(&mut host, "main");
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    edit(&mut host, &mut session, rename("transit", "belt length"));
    let belt = host.project.models[0]
        .variables
        .iter()
        .find_map(|var| match var {
            datamodel::Variable::Stock(stock) => stock.compat.conveyor.clone(),
            _ => None,
        })
        .expect("the conveyor");
    assert_eq!(belt.transit_time, "belt_length");
    let expected = respelled(&before, |owner, rest| {
        let owner = if owner == "transit" {
            "belt_length"
        } else {
            owner
        };
        (owner.to_string(), rest.to_string())
    });
    assert_eq!(series(&mut host, "main"), expected);
}

/// A root model with an instance `part` of `component`, which reads the
/// instance's output and wires one of its own variables to its input.
fn parent_and_component() -> datamodel::Project {
    let aux = |ident: &str, equation: &str, input: bool| {
        datamodel::Variable::Aux(datamodel::Aux {
            ident: ident.to_string(),
            equation: datamodel::Equation::Scalar(equation.to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat {
                can_be_module_input: input,
                ..datamodel::Compat::default()
            },
        })
    };
    let mut project = crate::test_common::TestProject::new("kinds")
        .with_sim_time(0.0, 4.0, 1.0)
        .stock("Level", "10", &["filling"], &[], None)
        .flow("filling", "rate * 2 + part.output", None)
        .aux("rate", "3", None)
        .aux("idle", "1", None)
        .build_datamodel();
    project.models[0]
        .variables
        .push(datamodel::Variable::Module(datamodel::Module {
            ident: "part".to_string(),
            model_name: "component".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![datamodel::ModuleReference {
                src: "idle".to_string(),
                dst: "part\u{00B7}input".to_string(),
            }],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    project.models.push(datamodel::Model {
        name: "component".to_string(),
        sim_specs: None,
        variables: vec![
            aux("input", "500", true),
            aux("output", "input * gain", false),
            aux("gain", "2", false),
        ]
        .into(),
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    });
    project
}

/// A rename of a sub-model's output respells the root's read of it through
/// the instance, and of its input port the root's wiring to it; a rename of
/// the instance respells everything read through it. Each is made, and the
/// root computes what it did, its columns under their new names.
#[test]
fn a_rename_across_models_respells_every_read_through_an_instance() {
    let mut host = Host::new(parent_and_component());
    let before = series(&mut host, "main");
    let mut component = Session::new("component");
    read(&mut host, &mut component);

    let made = edit(&mut host, &mut component, rename("output", "result"));
    assert!(
        made["changes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|line| line["variable"] == "filling (in main)"),
        "the root's reader is a line of the edit: {made}"
    );
    let output = |owner: &str, rest: &str| {
        let rest = if rest == "\u{00B7}output" {
            "\u{00B7}result"
        } else {
            rest
        };
        (owner.to_string(), rest.to_string())
    };
    assert_eq!(series(&mut host, "main"), respelled(&before, output));

    edit(&mut host, &mut component, rename("input", "feed"));
    let input = |owner: &str, rest: &str| {
        let (owner, rest) = output(owner, rest);
        let rest = if rest == "\u{00B7}input" {
            "\u{00B7}feed".to_string()
        } else {
            rest
        };
        (owner, rest)
    };
    assert_eq!(series(&mut host, "main"), respelled(&before, input));

    let mut root = Session::new("main");
    read(&mut host, &mut root);
    edit(&mut host, &mut root, rename("part", "piece"));
    let instance = |owner: &str, rest: &str| {
        let (owner, rest) = input(owner, rest);
        let owner = if owner == "part" {
            "piece".to_string()
        } else {
            owner
        };
        (owner, rest)
    };
    assert_eq!(series(&mut host, "main"), respelled(&before, instance));
}

/// A value the edit makes not a number is named as a tool takes it back: a
/// variable of the model the edit is of by its own name, also where the
/// root model's run reads it through an instance (`part·output` is the
/// session's `output`), and a variable of the root model with the root
/// model named.
#[test]
fn a_value_not_a_number_is_named_with_its_model() {
    let mut host = Host::new(parent_and_component());
    let mut component = Session::new("component");
    read(&mut host, &mut component);
    let refusal = refused(
        &mut host,
        &mut component,
        operations(json!([{"op": "set_equation", "variable": "gain", "equation": "0/0"}])),
    );
    let named: Vec<(Value, Value)> = refusal["refusedEdit"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "non_finite")
        .map(|d| (d["variable"].clone(), d["model"].clone()))
        .collect();
    assert!(
        named.contains(&(json!("gain"), Value::Null)),
        "the session's own: {named:?}"
    );
    assert!(
        named.contains(&(json!("filling"), json!("main"))),
        "the root's, with its model: {named:?}"
    );
    for (variable, model) in &named {
        let variable = variable.as_str().unwrap();
        let declared = match model.as_str() {
            Some(model) => host.project.get_model(model).unwrap(),
            None => host.project.get_model("component").unwrap(),
        };
        assert!(
            declared.get_variable(variable).is_some(),
            "{variable} in {model}: a name the agent can pass back"
        );
    }
}

/// The refusal's error, and the readers it names in its diagnostics.
fn dangling_readers(refusal: &Value) -> Vec<String> {
    refusal["refusedEdit"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "names_a_removed_variable")
        .map(|d| d["variable"].as_str().unwrap().to_string())
        .collect()
}

/// A delete is judged by what the project's texts still name, as a rename
/// finds references, besides by what the compiler reports: a variable whose
/// equation fails already reports no new error for a name it loses, and the
/// delete is refused naming it.
#[test]
fn a_delete_of_what_a_broken_equation_still_names_is_refused() {
    let project = crate::test_common::TestProject::new("broken")
        .with_sim_time(0.0, 4.0, 1.0)
        .stock("level", "100", &["arrivals"], &[], None)
        .flow("arrivals", "level * r + missing_thing", None)
        .aux("r", "0.1", None)
        .aux("spare", "1", None)
        .build_datamodel();
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let refusal = refused(
        &mut host,
        &mut session,
        operations(json!([{"op": "delete", "variable": "r"}])),
    );
    assert_eq!(refusal["refusedEdit"]["rule"], "errors", "{refusal}");
    assert!(
        refusal["error"].as_str().unwrap().contains("arrivals"),
        "{refusal}"
    );
    assert_eq!(dangling_readers(&refusal), ["arrivals"]);
    // What nothing names is deleted.
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "delete", "variable": "spare"}])),
    );
}

/// The corpus case: in the beer game `order step` calls a function the engine
/// does not have, so its equation is read from its text, which names `order
/// step size`; a readers citation of none fails, and the delete is refused.
#[test]
fn a_variable_an_unread_equation_names_has_a_reader_and_is_not_deleted() {
    let mut host = Host::new(corpus("metasd/beer-game/RealBeer4-Sterman13.mdl"));
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let record = host.call(
        &mut session,
        "read_variables",
        json!({"names": ["order step size"]}),
    );
    let readers = record["variables"][0]["readers"].as_array().unwrap();
    assert!(
        readers
            .iter()
            .any(|r| r["name"] == "order_step" && r["unchecked"] == true),
        "{record}"
    );
    let verdict = host.call(
        &mut session,
        "verify_findings",
        json!({"findings": [{"kind": "flaw", "claim": "Nothing reads the order step size.",
            "citations": [{"cites": "readers", "variable": "order step size", "readers": []}]}]}),
    );
    assert_eq!(verdict["findings"][0]["holds"], false, "{verdict}");
    let refusal = refused(
        &mut host,
        &mut session,
        operations(json!([{"op": "delete", "variable": "order step size"}])),
    );
    assert_eq!(dangling_readers(&refusal), ["order_step"], "{refusal}");
}

/// A sub-model's variable the root model reads through an instance, in an
/// equation that fails already, is not deleted: the reader is named with its
/// model.
#[test]
fn a_delete_a_broken_reader_in_another_model_names_is_refused() {
    let mut project = parent_and_component();
    project.models[0]
        .get_variable_mut("filling")
        .unwrap()
        .set_scalar_equation("rate * 2 + part.output + missing_thing");
    let mut host = Host::new(project);
    let mut component = Session::new("component");
    read(&mut host, &mut component);
    let refusal = refused(
        &mut host,
        &mut component,
        operations(json!([{"op": "delete", "variable": "output"}])),
    );
    let readers: Vec<(Value, Value)> = refusal["refusedEdit"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["code"] == "names_a_removed_variable")
        .map(|d| (d["variable"].clone(), d["model"].clone()))
        .collect();
    assert_eq!(readers, [(json!("filling"), json!("main"))], "{refusal}");
}

/// The engine reports one cycle of a model, so a cycle the edit writes hides
/// behind one the model has; judged with the broken variables set aside, it
/// is the edit's own error, and refused.
#[test]
fn a_cycle_written_beside_one_the_model_had_is_refused() {
    let project = crate::test_common::TestProject::new("cycles")
        .with_sim_time(0.0, 4.0, 1.0)
        .aux("a", "b + 1", None)
        .aux("b", "a + 1", None)
        .aux("c", "1", None)
        .build_datamodel();
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    let refusal = refused(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "c", "equation": "c + 1"}])),
    );
    assert_eq!(refusal["refusedEdit"]["rule"], "errors", "{refusal}");
    assert!(
        refusal["error"]
            .as_str()
            .unwrap()
            .contains("in an equation it writes"),
        "{refusal}"
    );
}

/// A repair is not blamed for an error the model had and hid: with two
/// cycles, the engine reports one; repairing it brings out the other, which
/// was there before the edit, and the repair is made.
#[test]
fn a_repair_is_not_blamed_for_an_error_the_model_hid() {
    let project = crate::test_common::TestProject::new("cycles")
        .with_sim_time(0.0, 4.0, 1.0)
        .aux("a", "b + 1", None)
        .aux("b", "a + 1", None)
        .aux("c", "c + 1", None)
        .build_datamodel();
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    for repair in [("a", "5"), ("c", "5")] {
        edit(
            &mut host,
            &mut session,
            operations(json!([{"op": "set_equation", "variable": repair.0, "equation": repair.1}])),
        );
    }
}

/// An error of a variable that is no error of its equation (a units string
/// that does not parse) does not refuse an edit that writes the equation
/// well: it is the variable's, and the edit did not write it.
#[test]
fn an_equation_written_well_is_not_refused_for_its_variables_units() {
    let mut project = crate::test_common::TestProject::new("units")
        .with_sim_time(0.0, 4.0, 1.0)
        .aux("rate", "0.1", None)
        .aux("doubled", "rate * 2", None)
        .build_datamodel();
    if let Some(datamodel::Variable::Aux(rate)) = project.models[0].get_variable_mut("rate") {
        rate.units = Some("widget/".to_string());
    }
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    read(&mut host, &mut session);
    edit(
        &mut host,
        &mut session,
        operations(json!([{"op": "set_equation", "variable": "rate", "equation": "0.2"}])),
    );
}
