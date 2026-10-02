// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What a rename and a delete reach: every reference to the variable, in
//! every expression text and every place a model holds a name, in every model
//! that can name it, and nothing else of what the modeler wrote. The standing
//! claim of a rename is that it is refused, changing nothing, or the model
//! computes what it computed, under the new name.

use std::collections::{BTreeSet, HashMap};
use std::io::BufReader;

use super::*;
use crate::datamodel::{Compat, Equation, view_element};
use crate::test_common::TestProject;

fn rename_in(model: &str, from: &str, to: &str) -> ProjectPatch {
    ProjectPatch {
        project_ops: vec![],
        models: vec![ModelPatch {
            name: model.to_string(),
            ops: vec![ModelOperation::RenameVariable {
                from: from.to_string(),
                to: to.to_string(),
            }],
        }],
    }
}

fn delete_in(model: &str, ident: &str) -> ProjectPatch {
    ProjectPatch {
        project_ops: vec![],
        models: vec![ModelPatch {
            name: model.to_string(),
            ops: vec![ModelOperation::DeleteVariable {
                ident: ident.to_string(),
            }],
        }],
    }
}

fn aux(ident: &str, equation: &str) -> Variable {
    Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

fn flow(ident: &str, equation: &str) -> datamodel::Flow {
    datamodel::Flow {
        ident: ident.to_string(),
        equation: Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    }
}

fn module(ident: &str, model_name: &str, references: &[(&str, &str)]) -> Variable {
    Variable::Module(datamodel::Module {
        ident: ident.to_string(),
        model_name: model_name.to_string(),
        documentation: String::new(),
        units: None,
        references: references
            .iter()
            .map(|(src, dst)| datamodel::ModuleReference {
                src: canonicalize(src).into_owned(),
                dst: canonicalize(dst).into_owned(),
            })
            .collect(),
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

fn model(name: &str, variables: Vec<Variable>) -> datamodel::Model {
    datamodel::Model {
        name: name.to_string(),
        sim_specs: None,
        variables: variables.into(),
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    }
}

fn project(models: Vec<datamodel::Model>) -> datamodel::Project {
    datamodel::Project {
        models,
        ..TestProject::new("rename").build_datamodel()
    }
}

fn scalar(project: &datamodel::Project, model: &str, ident: &str) -> String {
    match project
        .get_model(model)
        .and_then(|m| m.get_variable(ident))
        .and_then(Variable::get_equation)
    {
        Some(Equation::Scalar(text)) | Some(Equation::ApplyToAll(_, text)) => text.clone(),
        _ => panic!("{model}.{ident} has no single equation"),
    }
}

fn references(project: &datamodel::Project, model: &str, ident: &str) -> Vec<(String, String)> {
    match project.get_model(model).and_then(|m| m.get_variable(ident)) {
        Some(Variable::Module(module)) => module
            .references
            .iter()
            .map(|r| (r.src.clone(), r.dst.clone()))
            .collect(),
        _ => panic!("{model}.{ident} is not a module"),
    }
}

/// What running the project gives: every series its run saves, by name, or
/// the code it is refused with.
fn outcome(project: &datamodel::Project) -> std::result::Result<HashMap<String, Vec<f64>>, String> {
    let Some(main) = project.models.first() else {
        return Err("has no model".to_string());
    };
    let mut vm = crate::queue_compile::build_vm(project, &main.name)
        .map_err(|err| format!("does not build: {}", err.code))?;
    vm.run_to_end()
        .map_err(|err| format!("does not run: {}", err.code))?;
    Ok(crate::test_common::collect_results(&vm.into_results()))
}

/// Every series the project's run saves, by name.
fn series(project: &datamodel::Project) -> HashMap<String, Vec<f64>> {
    outcome(project).unwrap_or_else(|err| panic!("the project {err}"))
}

/// What a run calls `key` once the variable a run saves under the path
/// `from` (`inst·x`, or `x` in the model run) is renamed to the path `to`, or
/// `None` when the series is not the variable's: the variable's own series,
/// one read through it (a renamed module instance), or one of a hidden
/// variable made for it, whose name begins `$` and holds the variable's name
/// as a whole `$`- or `⁚`-separated part (a parse's helper `$⁚x⁚0⁚smth1`, a
/// conveyor's `$conv$x$len`). The subscript stays as it is, since an element
/// named like a variable is not the variable. A trailing `·` on either path
/// is ignored.
fn renamed_key(key: &str, from: &str, to: &str) -> Option<String> {
    let (from, to) = (from.trim_end_matches('·'), to.trim_end_matches('·'));
    let (path, subscript) = key.split_at(key.find('[').unwrap_or(key.len()));
    if let Some(rest) = path.strip_prefix(from)
        && (rest.is_empty() || rest.starts_with('·'))
    {
        return Some(format!("{to}{rest}{subscript}"));
    }
    // The instance path a variable's hidden variables are under, and its own
    // name.
    let scoped = |full: &str| match full.rsplit_once('·') {
        Some((scope, name)) => (format!("{scope}·"), name.to_string()),
        None => (String::new(), full.to_string()),
    };
    let ((from_scope, from_name), (to_scope, to_name)) = (scoped(from), scoped(to));
    let hidden = path.strip_prefix(from_scope.as_str())?;
    let (head, rest) = hidden.split_at(hidden.find('·').unwrap_or(hidden.len()));
    if !head.starts_with('$') {
        return None;
    }
    let mut renamed = String::new();
    let mut found = false;
    for part in head.split_inclusive(['$', '⁚']) {
        let name = part.trim_end_matches(['$', '⁚']);
        if name == from_name {
            found = true;
            renamed.push_str(&to_name);
            renamed.push_str(&part[name.len()..]);
        } else {
            renamed.push_str(part);
        }
    }
    found.then(|| format!("{to_scope}{renamed}{rest}{subscript}"))
}

/// The series of `after`, a run of the project with `from` renamed `to`
/// (paths a run names them by), are the series of `before`, each under its
/// name after the rename (`renamed_key`).
fn assert_same_run(
    before: &HashMap<String, Vec<f64>>,
    after: HashMap<String, Vec<f64>>,
    from: &str,
    to: &str,
) {
    assert_same_run_renaming(before, after, &[(from.to_string(), to.to_string())]);
}

/// Every path a run names the variables of `model` under: empty in a run of
/// `model` itself, and `inst·` (`outer·inner·`, ...) for each instance of it
/// in the model the run is of (`project`'s first), however deep.
fn instance_paths(project: &datamodel::Project, model: &str) -> Vec<String> {
    let by_name = |name: &str| {
        project
            .models
            .iter()
            .rev()
            .find(|m| canonicalize(&m.name) == canonicalize(name))
    };
    let mut paths = Vec::new();
    let mut walk = vec![(project.models[0].name.clone(), String::new())];
    while let Some((name, path)) = walk.pop() {
        if canonicalize(&name) == canonicalize(model) {
            paths.push(path.clone());
        }
        // Deeper than any fixture or corpus model nests: a module cycle.
        if path.matches('·').count() > 8 {
            continue;
        }
        for var in by_name(&name).iter().flat_map(|m| m.variables.iter()) {
            if let Variable::Module(module) = var {
                let ident = canonicalize(&module.ident);
                walk.push((module.model_name.clone(), format!("{path}{ident}·")));
            }
        }
    }
    paths
}

/// [`assert_same_run`] for a variable of `model` renamed `from` -> `to`,
/// under every path the run names it by (`instance_paths`).
fn assert_same_run_in(
    project: &datamodel::Project,
    before: &HashMap<String, Vec<f64>>,
    after: HashMap<String, Vec<f64>>,
    model: &str,
    from: &str,
    to: &str,
) {
    let (from, to) = (canonicalize(from), canonicalize(to));
    // A call of a macro instantiates the macro's model under a name a parse
    // synthesized (`$⁚{parent}⁚{n}⁚{macro}`, `capture::synthetic_ident`, with
    // a trailing `⁚{element}` per element), which only the run's names show.
    let model_key = canonicalize(model);
    let mut paths = instance_paths(project, model);
    for name in before.keys() {
        for (at, _) in name.match_indices('·') {
            let scope = &name[..at];
            let instance = scope.rsplit('·').next().unwrap_or(scope);
            let instantiates = instance
                .strip_prefix("$⁚")
                .and_then(|synthetic| synthetic.split('⁚').nth(2))
                .is_some_and(|function| function == model_key.as_ref());
            let path = format!("{scope}·");
            if instantiates && !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    let renames: Vec<(String, String)> = paths
        .into_iter()
        .map(|path| (format!("{path}{from}"), format!("{path}{to}")))
        .collect();
    assert_same_run_renaming(before, after, &renames);
}

/// The series of `after` are the series of `before`, each under the name the
/// first of `renames` that is about it gives it (`renamed_key`), or its own.
fn assert_same_run_renaming(
    before: &HashMap<String, Vec<f64>>,
    after: HashMap<String, Vec<f64>>,
    renames: &[(String, String)],
) {
    let renamed = |name: &str| {
        renames
            .iter()
            .find_map(|(from, to)| renamed_key(name, from, to))
            .unwrap_or_else(|| name.to_string())
    };
    let mut names: Vec<(&String, String)> =
        before.keys().map(|name| (name, renamed(name))).collect();
    names.sort();
    let mut want: Vec<&String> = names.iter().map(|(_, renamed)| renamed).collect();
    want.sort();
    let mut got: Vec<&String> = after.keys().collect();
    got.sort();
    assert_eq!(got, want, "the same variables are saved, renamed");
    for (name, renamed) in &names {
        let same = before[*name]
            .iter()
            .zip(&after[renamed])
            .all(|(b, a)| b.to_bits() == a.to_bits());
        assert!(
            same,
            "{name}: {:?} became {:?}",
            before[*name], after[renamed]
        );
    }
}

/// A `main` that instantiates `sub` directly (`inst`) and through `mid`
/// (`outer`, whose `inner` is the instance), beside an instance of another
/// model, of a stdlib model and of no model at all; `x` is a variable of
/// every model, and `sub` also holds a variable whose name has a period in
/// it.
fn nested() -> datamodel::Project {
    project(vec![
        model(
            "main",
            vec![
                aux("x", "1"),
                aux("x.foo", "5"),
                module("inst", "sub", &[]),
                module("other", "elsewhere", &[]),
                module("outer", "mid", &[]),
                module("smooth", "stdlib\u{205A}smth1", &[]),
                module("dangling", "no such model", &[]),
            ],
        ),
        model("mid", vec![aux("x", "2"), module("inner", "sub", &[])]),
        model("sub", vec![aux("x", "3"), aux("y", "x + 1")]),
        model("elsewhere", vec![aux("x", "4")]),
    ])
}

#[test]
fn a_reference_names_the_renamed_variable_through_the_instances_that_reach_it() {
    let base = nested();
    let index = |name: &str| base.model_index(name).expect("the fixture has the model");
    let check = |holder: &str, old: &str, rows: &[(&str, &str, Option<&str>)]| {
        let (old, new) = (Ident::new(old), Ident::new("z"));
        let rename = Rename::new(&base, index(holder), &old, &new);
        for (model, reference, want) in rows {
            let got = rename.renamed(index(model), &Ident::new(reference), ReadAs::Variable);
            assert_eq!(
                got.as_ref().map(Ident::as_str),
                *want,
                "`{reference}` written in `{model}`, renaming {holder}'s `{old}`"
            );
        }
        rename.models_reaching()
    };

    let reaching = check(
        "sub",
        "x",
        &[
            // In the variable's own model: bare, and under either scope prefix.
            ("sub", "x", Some("z")),
            ("sub", "self·x", Some("self·z")),
            ("sub", "·x", Some("·z")),
            ("sub", "y", None),
            ("sub", "xx", None),
            // From a model that instantiates it, at any depth.
            ("main", "inst·x", Some("inst·z")),
            ("main", "self·inst·x", Some("self·inst·z")),
            ("main", "outer·inner·x", Some("outer·inner·z")),
            ("mid", "inner·x", Some("inner·z")),
            // A like-named variable of another model is another variable.
            ("main", "x", None),
            ("mid", "x", None),
            ("elsewhere", "x", None),
            ("main", "other·x", None),
            ("main", "outer·x", None),
            ("main", "smooth·x", None),
            // A path that leaves the project's models names nothing renamed.
            ("main", "dangling·x", None),
            ("main", "missing·x", None),
            ("main", "inst·y·x", None),
        ],
    );
    assert_eq!(
        reaching,
        [true, true, true, false],
        "main, mid and sub can name sub's variable; elsewhere cannot"
    );

    // A renamed module instance is the first hop of what is read through it,
    // whatever it instantiates that the engine holds.
    check(
        "main",
        "inst",
        &[
            ("main", "inst", Some("z")),
            ("main", "inst·x", Some("z·x")),
            ("main", "·inst·x", Some("·z·x")),
            ("main", "outer·inner·x", None),
        ],
    );
    check(
        "main",
        "smooth",
        &[
            ("main", "smooth", Some("z")),
            ("main", "smooth·output", Some("z·output")),
        ],
    );
    // An instance of a model nothing holds is the head of no path a read
    // takes: the spelling is one local name, as `db::DepScope::resolve` reads
    // it. A module reference's `dst` names a port through the instance it
    // starts at, whatever the instance instantiates (`db::assemble::port_of`),
    // so the wiring follows the instance however the model arrives.
    check(
        "main",
        "dangling",
        &[
            ("main", "dangling", Some("z")),
            ("main", "dangling·x", None),
        ],
    );
    let (old, new) = (Ident::new("dangling"), Ident::new("z"));
    let rename = Rename::new(&base, index("main"), &old, &new);
    for (port, want) in [
        ("dangling·input", Some("z·input")),
        ("inst·x", None),
        ("dangling", Some("z")),
    ] {
        assert_eq!(
            rename
                .renamed(index("main"), &Ident::new(port), ReadAs::Port)
                .as_ref()
                .map(Ident::as_str),
            want,
            "the port `{port}`"
        );
    }
    // So is a spelling whose head is no module at all: `x.foo` beside an
    // auxiliary `x` is the variable named `x.foo`.
    check(
        "main",
        "x",
        &[("main", "x", Some("z")), ("main", "x·foo", None)],
    );
    check(
        "main",
        "x.foo",
        &[("main", "x·foo", Some("z")), ("main", "x", None)],
    );
}

/// Of two models whose names canonicalize alike (a project the engine refuses
/// to compile), an instance names the later, the one the db files under the
/// name.
#[test]
fn an_instance_of_a_name_two_models_have_reaches_the_later_one() {
    let base = project(vec![
        model(
            "main",
            vec![module("inst", "sub", &[]), aux("reader", "inst.x")],
        ),
        model("Sub", vec![aux("x", "1")]),
        model("sub", vec![aux("x", "2")]),
    ]);
    let mut later = base.clone();
    apply_patch(&mut later, rename_in("sub", "x", "z")).unwrap();
    assert_eq!(scalar(&later, "main", "reader"), "inst·z");
    let mut earlier = base.clone();
    apply_patch(&mut earlier, rename_in("Sub", "x", "z")).unwrap();
    assert_eq!(scalar(&earlier, "main", "reader"), "inst.x");
}

/// A variable of each kind with every expression text it can hold set to
/// `1`. Each struct is written out in full, so a field added to one fails to
/// compile here until the fixture sets it.
fn loaded_variables() -> Vec<Variable> {
    let one = || "1".to_string();
    let compat = Compat {
        active_initial: Some(one()),
        non_negative: false,
        can_be_module_input: false,
        visibility: datamodel::Visibility::Private,
        data_source: None,
        conveyor: Some(datamodel::Conveyor {
            transit_time: one(),
            capacity: Some(one()),
            inflow_limit: Some(one()),
            sample: Some(one()),
            arrest: Some(one()),
            discrete: false,
            batch_integrity: false,
            one_at_a_time: true,
            exponential_leak: false,
            ignore_earlier_zone_losses: false,
        }),
        leakage: Some(datamodel::Leakage {
            fraction: Some(one()),
            integers: false,
            zone_start: Some(one()),
            zone_end: Some(one()),
        }),
        spreadflow: None,
        queue: None,
        overflow: false,
    };
    let arrayed = Equation::Arrayed(
        vec!["d".to_string()],
        vec![
            ("a".to_string(), one(), Some(one()), None),
            ("b".to_string(), one(), None, None),
        ],
        Some(one()),
        true,
    );
    vec![
        Variable::Stock(datamodel::Stock {
            ident: "a stock".to_string(),
            equation: arrayed.clone(),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            ai_state: None,
            uid: None,
            compat: compat.clone(),
        }),
        Variable::Flow(datamodel::Flow {
            ident: "a flow".to_string(),
            equation: arrayed,
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: compat.clone(),
        }),
        Variable::Aux(datamodel::Aux {
            ident: "an aux".to_string(),
            equation: Equation::ApplyToAll(vec!["d".to_string()], one()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: compat.clone(),
        }),
        Variable::Aux(datamodel::Aux {
            ident: "a scalar".to_string(),
            equation: Equation::Scalar(one()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: compat.clone(),
        }),
        Variable::Module(datamodel::Module {
            ident: "a module".to_string(),
            model_name: "sub".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![],
            ai_state: None,
            uid: None,
            compat,
        }),
    ]
}

/// Every field of `variable` the engine parses as an expression, by this
/// test's own reading of the datamodel: the oracle
/// `Variable::expression_texts` is held to, so it is not that function under
/// another name. Each struct is destructured without `..`, so a field added
/// to one fails to compile here until it is put on one side or the other.
fn expression_fields(variable: &mut Variable) -> Vec<&mut String> {
    let (equation, compat) = match variable {
        Variable::Stock(datamodel::Stock {
            equation,
            compat,
            ident: _,
            documentation: _,
            units: _,
            inflows: _,
            outflows: _,
            ai_state: _,
            uid: _,
        }) => (Some(equation), compat),
        Variable::Flow(datamodel::Flow {
            equation,
            compat,
            ident: _,
            documentation: _,
            units: _,
            gf: _,
            ai_state: _,
            uid: _,
        })
        | Variable::Aux(datamodel::Aux {
            equation,
            compat,
            ident: _,
            documentation: _,
            units: _,
            gf: _,
            ai_state: _,
            uid: _,
        }) => (Some(equation), compat),
        Variable::Module(datamodel::Module {
            compat,
            ident: _,
            model_name: _,
            documentation: _,
            units: _,
            references: _,
            ai_state: _,
            uid: _,
        }) => (None, compat),
    };
    let mut fields: Vec<&mut String> = Vec::new();
    match equation {
        Some(Equation::Scalar(text)) | Some(Equation::ApplyToAll(_, text)) => fields.push(text),
        Some(Equation::Arrayed(_, elements, default, _)) => {
            for (_, text, initial, _) in elements {
                fields.push(text);
                fields.extend(initial.as_mut());
            }
            fields.extend(default.as_mut());
        }
        None => {}
    }
    let Compat {
        active_initial,
        conveyor,
        leakage,
        // A name or a list of numbers, never an expression.
        spreadflow: _,
        non_negative: _,
        can_be_module_input: _,
        visibility: _,
        data_source: _,
        queue: _,
        overflow: _,
    } = compat;
    fields.extend(active_initial.as_mut());
    if let Some(datamodel::Conveyor {
        transit_time,
        capacity,
        inflow_limit,
        sample,
        arrest,
        discrete: _,
        batch_integrity: _,
        one_at_a_time: _,
        exponential_leak: _,
        ignore_earlier_zone_losses: _,
    }) = conveyor
    {
        fields.push(transit_time);
        for text in [capacity, inflow_limit, sample, arrest] {
            fields.extend(text.as_mut());
        }
    }
    if let Some(datamodel::Leakage {
        fraction,
        zone_start,
        zone_end,
        integers: _,
    }) = leakage
    {
        for text in [fraction, zone_start, zone_end] {
            fields.extend(text.as_mut());
        }
    }
    fields
}

/// A rename respells a reference wherever it is the ONLY one a variable
/// holds: one row per expression field of each kind of variable
/// (`expression_fields`), with the renamed name in that field and in no
/// other, so no field is renamed on the strength of another's.
#[test]
fn a_rename_reaches_each_expression_field_on_its_own() {
    for variable in loaded_variables() {
        let ident = variable.get_ident().to_string();
        let count = expression_fields(&mut variable.clone()).len();
        assert_eq!(
            variable.expression_texts().len(),
            count,
            "{ident}: every expression field is an expression text, and nothing else is"
        );
        for at in 0..count {
            let mut reader = variable.clone();
            *expression_fields(&mut reader)[at] = "target * 2".to_string();
            let mut project = project(vec![model("main", vec![aux("target", "5"), reader])]);
            apply_patch(&mut project, rename_in("main", "target", "renamed")).unwrap();
            let mut renamed = project.models[0]
                .get_variable(&ident)
                .cloned()
                .expect("the reader is still there");
            for (i, text) in expression_fields(&mut renamed).into_iter().enumerate() {
                let want = if i == at { "renamed * 2" } else { "1" };
                assert_eq!(
                    text, want,
                    "{ident}: field {i}, the reference in field {at}"
                );
            }
        }
    }
}

const NAME_ROLES: usize = 10;

/// The position of `role` among the roles a name can have. The match names
/// every role, so a role added to `NameRole` fails to compile here, and then
/// fails `a_rename_and_a_delete_decide_every_place_a_model_holds_a_name`
/// until `name_place` has a row for it.
fn name_role_index(role: NameRole) -> usize {
    match role {
        NameRole::Ident => 0,
        NameRole::Inflow => 1,
        NameRole::Outflow => 2,
        NameRole::ModuleSource => 3,
        NameRole::ModuleDestination => 4,
        NameRole::Distribution => 5,
        NameRole::GroupMember => 6,
        NameRole::MacroParameter => 7,
        NameRole::MacroOutput => 8,
        NameRole::ViewLabel => 9,
    }
}

/// One place a model holds the name `target`, by this test's own reading of
/// the datamodel: a project whose `main` holds it there, how to read the
/// names the place holds, and what they are after `target` is renamed
/// `Renamed Thing` and after it is deleted.
struct NamePlace {
    role: NameRole,
    project: datamodel::Project,
    read: fn(&datamodel::Model) -> Vec<String>,
    renamed: &'static [&'static str],
    deleted: &'static [&'static str],
}

fn name_places() -> Vec<NamePlace> {
    let stock = |inflows: &[&str], outflows: &[&str]| {
        Variable::Stock(datamodel::Stock {
            ident: "level".to_string(),
            equation: Equation::Scalar("0".to_string()),
            documentation: String::new(),
            units: None,
            inflows: inflows.iter().map(|s| s.to_string()).collect(),
            outflows: outflows.iter().map(|s| s.to_string()).collect(),
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        })
    };
    let sub = || model("sub", vec![aux("input", "0"), aux("output", "input")]);
    let in_main = |variables: Vec<Variable>| project(vec![model("main", variables), sub()]);

    let mut places = vec![
        NamePlace {
            role: NameRole::Ident,
            project: in_main(vec![aux("target", "1"), aux("kept", "2")]),
            read: |model| {
                model
                    .variables
                    .iter()
                    .map(|v| v.get_ident().to_string())
                    .filter(|ident| ident != "kept")
                    .collect()
            },
            renamed: &["Renamed Thing"],
            deleted: &[],
        },
        NamePlace {
            role: NameRole::Inflow,
            project: in_main(vec![
                Variable::Flow(flow("target", "1")),
                Variable::Flow(flow("other", "1")),
                stock(&["other", "target"], &[]),
            ]),
            read: |model| match model.get_variable("level") {
                Some(Variable::Stock(stock)) => stock.inflows.clone(),
                _ => unreachable!("the fixture has the stock"),
            },
            renamed: &["other", "renamed_thing"],
            deleted: &["other"],
        },
        NamePlace {
            role: NameRole::Outflow,
            project: in_main(vec![
                Variable::Flow(flow("target", "1")),
                Variable::Flow(flow("other", "1")),
                stock(&[], &["target", "other"]),
            ]),
            read: |model| match model.get_variable("level") {
                Some(Variable::Stock(stock)) => stock.outflows.clone(),
                _ => unreachable!("the fixture has the stock"),
            },
            renamed: &["renamed_thing", "other"],
            deleted: &["other"],
        },
        NamePlace {
            role: NameRole::ModuleSource,
            project: in_main(vec![
                aux("target", "1"),
                module("reader", "sub", &[("target", "reader.input")]),
            ]),
            read: |model| match model.get_variable("reader") {
                Some(Variable::Module(module)) => {
                    module.references.iter().map(|r| r.src.clone()).collect()
                }
                _ => unreachable!("the fixture has the module"),
            },
            renamed: &["renamed_thing"],
            deleted: &[],
        },
        NamePlace {
            role: NameRole::ModuleDestination,
            // The connection is recorded on the instance it reads from.
            project: in_main(vec![
                module("feeder", "sub", &[("feeder.output", "target.input")]),
                module("target", "sub", &[]),
            ]),
            read: |model| match model.get_variable("feeder") {
                Some(Variable::Module(module)) => {
                    module.references.iter().map(|r| r.dst.clone()).collect()
                }
                _ => unreachable!("the fixture has the module"),
            },
            renamed: &["renamed_thing\u{00B7}input"],
            deleted: &[],
        },
        NamePlace {
            role: NameRole::Distribution,
            project: in_main(vec![
                aux("target", "1"),
                Variable::Flow(datamodel::Flow {
                    compat: Compat {
                        spreadflow: Some(datamodel::SpreadFlow::Dist("Target".to_string())),
                        ..Compat::default()
                    },
                    ..flow("entering", "1")
                }),
            ]),
            read: |model| match model.get_variable("entering") {
                Some(Variable::Flow(datamodel::Flow {
                    compat:
                        Compat {
                            spreadflow: Some(datamodel::SpreadFlow::Dist(name)),
                            ..
                        },
                    ..
                })) => vec![name.clone()],
                _ => unreachable!("the fixture has the spread flow"),
            },
            renamed: &["renamed_thing"],
            // The placement names a variable that is gone, as an equation
            // that read it would: the conveyor compile reports it.
            deleted: &["Target"],
        },
    ];

    let mut grouped = in_main(vec![aux("target", "1"), aux("other", "2")]);
    grouped.models[0].groups = vec![datamodel::ModelGroup {
        name: "sector".to_string(),
        doc: None,
        parent: None,
        members: vec!["other".to_string(), "Target".to_string()],
        run_enabled: false,
    }];
    places.push(NamePlace {
        role: NameRole::GroupMember,
        project: grouped,
        read: |model| model.groups[0].members.clone(),
        renamed: &["other", "renamed_thing"],
        deleted: &["other"],
    });

    let macro_project = |spec: datamodel::MacroSpec| {
        let mut project = in_main(vec![
            aux("main", "target + other"),
            aux("target", "0"),
            aux("other", "0"),
        ]);
        project.models[0].macro_spec = Some(spec);
        project
    };
    places.push(NamePlace {
        role: NameRole::MacroParameter,
        project: macro_project(datamodel::MacroSpec {
            parameters: vec!["other".to_string(), "Target".to_string()],
            primary_output: "main".to_string(),
            additional_outputs: vec![],
        }),
        read: |model| match &model.macro_spec {
            Some(spec) => spec.parameters.clone(),
            None => unreachable!("the fixture is a macro"),
        },
        renamed: &["other", "renamed_thing"],
        // A macro whose parameter is gone is a macro to repair, as an
        // equation that read the variable is.
        deleted: &["other", "Target"],
    });
    places.push(NamePlace {
        role: NameRole::MacroOutput,
        project: macro_project(datamodel::MacroSpec {
            parameters: vec![],
            primary_output: "target".to_string(),
            additional_outputs: vec!["other".to_string(), "TARGET".to_string()],
        }),
        read: |model| match &model.macro_spec {
            Some(spec) => std::iter::once(&spec.primary_output)
                .chain(&spec.additional_outputs)
                .cloned()
                .collect(),
            None => unreachable!("the fixture is a macro"),
        },
        renamed: &["renamed_thing", "other", "renamed_thing"],
        deleted: &["target", "other", "TARGET"],
    });

    let mut drawn = in_main(vec![aux("target", "1")]);
    drawn.models[0].views = vec![datamodel::View::StockFlow(datamodel::StockFlow {
        name: None,
        elements: vec![datamodel::ViewElement::Aux(view_element::Aux {
            name: "Target".to_string(),
            uid: 1,
            x: 0.0,
            y: 0.0,
            label_side: view_element::LabelSide::Bottom,
            compat: None,
        })]
        .into(),
        view_box: datamodel::Rect::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    })];
    places.push(NamePlace {
        role: NameRole::ViewLabel,
        project: drawn,
        read: |model| {
            let datamodel::View::StockFlow(view) = &model.views[0];
            view.elements
                .iter()
                .filter_map(|element| element.get_name().map(str::to_string))
                .collect()
        },
        // A diagram's label is the view's own: the edit that carries the view
        // relabels or removes the element.
        renamed: &["Target"],
        deleted: &["Target"],
    });
    places
}

/// Every place a model holds a variable's name outside an equation
/// (`Model::map_variable_names`) is decided by a rename and by a delete: one
/// row per `NameRole`, each read back from the datamodel's own fields.
#[test]
fn a_rename_and_a_delete_decide_every_place_a_model_holds_a_name() {
    let places = name_places();
    let covered: BTreeSet<usize> = places
        .iter()
        .map(|place| name_role_index(place.role))
        .collect();
    assert_eq!(
        covered,
        (0..NAME_ROLES).collect::<BTreeSet<usize>>(),
        "a row for every role"
    );
    for place in places {
        let role = name_role_index(place.role);
        // The place is one `Model::map_variable_names` visits, under its role.
        let mut visited = Vec::new();
        place.project.models[0]
            .clone()
            .map_variable_names(|role, name| {
                visited.push((name_role_index(role), name.to_string()));
                None
            });
        for name in (place.read)(&place.project.models[0]) {
            assert!(
                visited.contains(&(role, name.clone())),
                "role {role}: {name}"
            );
        }

        let mut renamed = place.project.clone();
        apply_patch(&mut renamed, rename_in("main", "target", "Renamed Thing")).unwrap();
        assert_eq!(
            (place.read)(&renamed.models[0]),
            place.renamed,
            "role {role}, after the rename"
        );

        let mut deleted = place.project.clone();
        apply_patch(&mut deleted, delete_in("main", "target")).unwrap();
        assert_eq!(
            (place.read)(&deleted.models[0]),
            place.deleted,
            "role {role}, after the delete"
        );
    }
}

/// A macro's parameters and output are variables of its body, named by its
/// spec: renamed with the spec, a macro computes what it computed.
#[test]
fn a_rename_inside_a_macro_keeps_its_calls_computing_the_same() {
    let build = || {
        let mut macro_model = model(
            "mymacro",
            vec![
                aux("mymacro", "p1 + p2 * 10"),
                aux("p1", "0"),
                aux("p2", "0"),
            ],
        );
        macro_model.macro_spec = Some(datamodel::MacroSpec {
            parameters: vec!["p1".to_string(), "p2".to_string()],
            primary_output: "mymacro".to_string(),
            additional_outputs: vec![],
        });
        project(vec![
            model(
                "main",
                vec![aux("a", "3"), aux("b", "4"), aux("y", "MYMACRO(a, b)")],
            ),
            macro_model,
        ])
    };
    let before = series(&build());
    assert_eq!(before["y"][0], 43.0);
    for (from, to) in [("p1", "first"), ("p2", "second"), ("mymacro", "result")] {
        let mut project = build();
        apply_patch(&mut project, rename_in("mymacro", from, to)).unwrap();
        assert_eq!(series(&project)["y"], before["y"], "renaming {from}");
    }
}

/// A conveyor whose inflow is spread by a named variable's graphical function
/// (`<isee:distrib_eq>`, the reader's own rendering of it) runs the same with
/// that variable renamed, however the distribution spells its name.
#[test]
fn a_rename_of_a_distribution_keeps_the_conveyor_computing_the_same() {
    let xmile = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0" xmlns:isee="http://iseesystems.com/XMILE">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
<options><uses_conveyor/></options></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>8</stop><dt>0.25</dt></sim_specs>
  <model><variables>
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f" isee:spreadflow="dist"><eqn>250</eqn><isee:distrib_eq>profile</isee:distrib_eq></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <aux name="profile"><eqn>0+0</eqn><gf><xscale min="0" max="1"/><yscale min="0" max="1"/><ypts>0,0.25,0.5,0.75,1</ypts></gf></aux>
  </variables></model>
</xmile>"#;
    let base = crate::compat::open_xmile(&mut BufReader::new(xmile.as_bytes())).expect("it opens");
    let before = series(&base);
    assert!(
        before["out_f"][2] > 0.0,
        "the distribution is in effect: material leaves before the belt has turned over"
    );

    for spelling in ["profile", "Profile", " profile ", "\"Profile\""] {
        let mut project = base.clone();
        let mut respelled = 0;
        project.models[0].variables.update(|var| {
            var.map_names(|role, _| {
                (role == NameRole::Distribution).then(|| {
                    respelled += 1;
                    spelling.to_string()
                })
            })
        });
        assert_eq!(respelled, 1, "the model has one distribution");
        assert_same_run(&before, series(&project), "profile", "profile");

        apply_patch(&mut project, rename_in("main", "profile", "Arrival Shape")).unwrap();
        assert_same_run(&before, series(&project), "profile", "arrival_shape");
    }
}

#[test]
fn a_conveyor_whose_transit_time_names_a_renamed_variable_still_builds() {
    let xmile = include_str!("../../../test/conveyors/arrayed_conveyor.xmile");
    let mut project =
        crate::compat::open_xmile(&mut BufReader::new(xmile.as_bytes())).expect("it opens");
    let before = series(&project);
    apply_patch(&mut project, rename_in("main", "transit", "belt time")).unwrap();
    assert_same_run(&before, series(&project), "transit", "belt_time");
}

fn hares_and_lynxes() -> datamodel::Project {
    let xmile = include_str!(
        "../../../test/test-models/samples/bpowers-hares_and_lynxes_modules/model.xmile"
    );
    crate::compat::open_xmile(&mut BufReader::new(xmile.as_bytes())).expect("it opens")
}

#[test]
fn a_rename_in_a_sub_model_reaches_the_models_that_instantiate_it() {
    let base = hares_and_lynxes();
    let before = series(&base);

    // An output another instance's input is wired from.
    let mut project = base.clone();
    apply_patch(&mut project, rename_in("hares", "hare density", "density")).unwrap();
    assert!(
        references(&project, "main", "lynxes")
            .iter()
            .any(|(src, dst)| src == "hares·density" && dst == "lynxes·hare_density"),
        "the parent's wiring reads the output under its new name: {:?}",
        references(&project, "main", "lynxes")
    );
    assert_same_run(
        &before,
        series(&project),
        "hares·hare_density",
        "hares·density",
    );

    // An input port the parent wires into.
    let mut project = base.clone();
    apply_patch(
        &mut project,
        rename_in("lynxes", "hare density", "prey density"),
    )
    .unwrap();
    assert!(
        references(&project, "main", "lynxes")
            .iter()
            .any(|(src, dst)| src == "hares·hare_density" && dst == "lynxes·prey_density")
    );
    assert_same_run(
        &before,
        series(&project),
        "lynxes·hare_density",
        "lynxes·prey_density",
    );

    // A module instance: what is read and wired through it follows.
    let mut project = base.clone();
    apply_patch(&mut project, rename_in("main", "hares", "rabbits")).unwrap();
    let wiring = references(&project, "main", "rabbits");
    assert!(
        wiring
            .iter()
            .any(|(src, dst)| canonicalize(src) == "·area" && dst == "rabbits·area"),
        "{wiring:?}"
    );
    assert!(
        references(&project, "main", "lynxes")
            .iter()
            .any(|(src, _)| src == "rabbits·hare_density")
    );
    assert_same_run(&before, series(&project), "hares·", "rabbits·");
}

#[test]
fn a_rename_of_a_sub_models_variable_reaches_the_equations_that_read_it() {
    let mut project = project(vec![
        model(
            "main",
            vec![
                module("inst", "sub", &[]),
                aux("reader", "inst.out * 2 + out"),
                aux("out", "7"),
            ],
        ),
        model("sub", vec![aux("out", "3"), aux("double", "out * 2")]),
    ]);
    let before = series(&project);
    apply_patch(&mut project, rename_in("sub", "out", "result")).unwrap();
    assert_eq!(
        scalar(&project, "main", "reader"),
        "inst·result * 2 + out",
        "the read through the instance is respelled; main's own `out` is not"
    );
    assert_eq!(scalar(&project, "sub", "double"), "result * 2");
    assert_eq!(scalar(&project, "main", "out"), "7");
    assert_same_run(&before, series(&project), "inst·out", "inst·result");
}

/// A module instance renamed while the project lacks the model it
/// instantiates keeps its wiring: its `dst` names its ports under the new
/// name, so once the model arrives the instance's input is fed as before.
#[test]
fn a_renamed_instance_keeps_its_wiring_before_its_model_arrives() {
    let mut project = project(vec![model(
        "main",
        vec![
            aux("a", "3"),
            module("inst", "later", &[("a", "inst.input")]),
        ],
    )]);
    apply_patch(&mut project, rename_in("main", "inst", "unit")).unwrap();
    assert_eq!(
        references(&project, "main", "unit"),
        [("a".to_string(), "unit·input".to_string())]
    );
    project.models.push(model(
        "later",
        vec![aux("input", "0"), aux("output", "input * 2")],
    ));
    apply_patch(
        &mut project,
        ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: "main".to_string(),
                ops: vec![ModelOperation::UpsertAux(datamodel::Aux {
                    ident: "reader".to_string(),
                    equation: Equation::Scalar("unit.output".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: Compat::default(),
                })],
            }],
        },
    )
    .unwrap();
    assert_eq!(series(&project)["reader"][0], 6.0, "the input is wired");
}

/// A delete unwires every module reference that runs through the deleted
/// variable, in each spelling an end can name it by.
#[test]
fn a_delete_unwires_a_module_reference_however_it_names_the_variable() {
    for spelling in ["source", ".source", "self.source", "feeder.output"] {
        let mut project = project(vec![
            model(
                "main",
                vec![
                    aux("source", "1"),
                    aux("kept", "2"),
                    module("feeder", "sub", &[]),
                    module(
                        "reader",
                        "sub",
                        &[(spelling, "reader.input"), ("kept", "reader.other")],
                    ),
                ],
            ),
            model(
                "sub",
                vec![aux("input", "0"), aux("other", "0"), aux("output", "input")],
            ),
        ]);
        let deleted = if spelling.starts_with("feeder") {
            "feeder"
        } else {
            "source"
        };
        apply_patch(&mut project, delete_in("main", deleted)).unwrap();
        assert_eq!(
            references(&project, "main", "reader"),
            [("kept".to_string(), "reader·other".to_string())],
            "a source spelled `{spelling}`"
        );
    }

    // A connection into a deleted instance, recorded on the instance it reads
    // from, goes with the instance it wrote into.
    let mut project = project(vec![
        model(
            "main",
            vec![
                module("feeder", "sub", &[("feeder.output", "sink.input")]),
                module("sink", "sub", &[]),
                aux("reader", "feeder.output"),
            ],
        ),
        model("sub", vec![aux("input", "1"), aux("output", "input * 2")]),
    ]);
    let before = series(&project);
    apply_patch(&mut project, delete_in("main", "sink")).unwrap();
    assert_eq!(references(&project, "main", "feeder"), []);
    assert_eq!(series(&project)["reader"], before["reader"]);

    // The XMILE reader's own spelling of a parent-scope source.
    let mut project = hares_and_lynxes();
    assert!(
        references(&project, "main", "hares")
            .iter()
            .any(|(src, _)| src == "·area"),
        "the reader stores the source as `·area`"
    );
    apply_patch(&mut project, delete_in("main", "area")).unwrap();
    assert!(
        references(&project, "main", "hares")
            .iter()
            .all(|(src, _)| !src.contains("area")),
        "{:?}",
        references(&project, "main", "hares")
    );
}

/// A model with an arrayed variable over `city`, and a second dimension the
/// equations over `city` do not iterate.
fn cities() -> TestProject {
    TestProject::new("cities")
        .named_dimension("city", &["boston", "la"])
        .named_dimension("kind", &["small", "large"])
        .array_with_ranges("arr[city]", vec![("boston", "10"), ("la", "20")])
}

/// `cities` with `sub`, a copy of its model, instantiated in `main` as
/// `inst`, and `main`'s own variables after it.
fn cities_with_an_instance(main: TestProject) -> datamodel::Project {
    let mut project = main.build_datamodel();
    project.models[0].variables.push(module("inst", "sub", &[]));
    let sub = cities()
        .aux("pick", "2", None)
        .build_datamodel()
        .models
        .into_iter()
        .next()
        .map(|model| datamodel::Model {
            name: "sub".to_string(),
            ..model
        })
        .unwrap_or_else(|| unreachable!("the builder makes a model"));
    project.models.push(sub);
    project
}

/// Why a rename that has to decide what a bracketed identifier names is
/// refused (`patch::Reader`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Refusal {
    /// The old name is a dimension's.
    OldIsADimension,
    /// The new name is a dimension's.
    NewIsADimension,
    /// The old name is an element, written alone in a subscript.
    OldIsAnElementInBrackets,
    /// The new name is an element, and the variable is written alone in a
    /// subscript.
    NewIsAnElementInBrackets,
    /// A reference's new spelling is a `dimension·element`.
    NewSpellsAQualifiedElement,
}

impl Refusal {
    /// Every refusal; the match names each, so a refusal added fails to
    /// compile here until it is listed, and then fails
    /// `a_rename_is_refused_or_changes_no_series` until a row has it.
    const ALL: [Refusal; 5] = [
        Refusal::OldIsADimension,
        Refusal::NewIsADimension,
        Refusal::OldIsAnElementInBrackets,
        Refusal::NewIsAnElementInBrackets,
        Refusal::NewSpellsAQualifiedElement,
    ];

    /// What the refusal's reason says.
    fn says(self) -> &'static str {
        match self {
            Refusal::OldIsADimension | Refusal::NewIsADimension => {
                "is also the name of a dimension"
            }
            Refusal::OldIsAnElementInBrackets => "cannot tell whether it names the element",
            Refusal::NewIsAnElementInBrackets => "it would name the element there",
            Refusal::NewSpellsAQualifiedElement => "the position of a dimension's element",
        }
    }
}

/// What one rename is expected to do.
enum Expect {
    /// Refused, naming the equation of `reader` (for a refusal an equation
    /// decides) and saying why.
    Refused(Refusal),
    /// Carried out, `reader`'s equation reading `after`.
    Respelled(&'static str),
}

/// For every row, a rename either leaves every series of the model as it
/// was, under the new name, or is refused with a reason and changes nothing.
/// Each row is a place the compiler reads an identifier in more than one way
/// or its unambiguous neighbor, so the rows pin which renames are refused, not
/// only that the property holds.
#[test]
fn a_rename_is_refused_or_changes_no_series() {
    struct Row {
        what: &'static str,
        project: datamodel::Project,
        model: &'static str,
        from: &'static str,
        to: &'static str,
        reader: &'static str,
        expect: Expect,
    }
    let row = |what, project, from, to, expect| Row {
        what,
        project,
        model: "main",
        from,
        to,
        reader: "reader",
        expect,
    };
    use Expect::{Refused, Respelled};
    let rows = vec![
        // The old name is an element, alone in brackets: the compiler's
        // static subscript reads the element, its range reads the variable
        // unless both ends are elements, and its dependency walk skips it.
        row(
            "an element of the axis alone in the brackets, read as a value too",
            cities()
                .aux("boston", "2", None)
                .aux("reader", "arr[boston] + boston", None)
                .build_datamodel(),
            "boston",
            "nyc",
            Refused(Refusal::OldIsAnElementInBrackets),
        ),
        row(
            "a range end that is an element and a variable, the other end a variable",
            cities()
                .aux("boston", "1", None)
                .aux("n", "2", None)
                .aux("reader", "SUM(arr[boston:n]) + boston * 0", None)
                .build_datamodel(),
            "boston",
            "nyc",
            Refused(Refusal::OldIsAnElementInBrackets),
        ),
        row(
            "both range ends elements of the axis, one also a variable",
            cities()
                .aux("boston", "1", None)
                .aux("reader", "SUM(arr[boston:la])", None)
                .build_datamodel(),
            "boston",
            "nyc",
            Refused(Refusal::OldIsAnElementInBrackets),
        ),
        row(
            "an element of a dimension the subscript does not index",
            cities()
                .aux("small", "1", None)
                .aux("reader", "arr[small] + small", None)
                .build_datamodel(),
            "small",
            "tiny",
            Refused(Refusal::OldIsAnElementInBrackets),
        ),
        row(
            "two dimensions, an element of the other axis",
            cities()
                .array_aux("m[city,kind]", "3")
                .aux("small", "1", None)
                .aux("reader", "m[boston, small] + small", None)
                .build_datamodel(),
            "small",
            "tiny",
            Refused(Refusal::OldIsAnElementInBrackets),
        ),
        // The old name is a dimension's: in brackets it is the active axis's
        // element where an axis supplies it, the variable where none does.
        row(
            "an arrayed variable named for a dimension, subscripted and bare",
            cities()
                .array_aux("kind[city]", "5")
                .array_aux("reader[city]", "kind[city] + kind")
                .build_datamodel(),
            "kind",
            "sort",
            Refused(Refusal::OldIsADimension),
        ),
        row(
            "a variable named for the dimension the equation iterates",
            cities()
                .aux("city", "2", None)
                .array_aux("reader[city]", "city * 10")
                .build_datamodel(),
            "city",
            "town",
            Refused(Refusal::OldIsADimension),
        ),
        row(
            "a variable named for a dimension, only ever subscripted",
            cities()
                .array_aux("kind[city]", "5")
                .aux("reader", "kind[boston] + 1", None)
                .build_datamodel(),
            "kind",
            "sort",
            Refused(Refusal::OldIsADimension),
        ),
        row(
            "a module instance named for a dimension, beside a qualified element",
            {
                let mut project = cities()
                    .aux("reader", "city.la * 10", None)
                    .build_datamodel();
                project.models[0].variables.push(module("city", "sub", &[]));
                project.models.push(model("sub", vec![aux("la", "7")]));
                project
            },
            "city",
            "town",
            Refused(Refusal::OldIsADimension),
        ),
        // The new name is a dimension's or an element's.
        row(
            "a variable renamed to the dimension an equation iterates",
            cities()
                .aux("x", "5", None)
                .array_aux("reader[city]", "x * 10")
                .build_datamodel(),
            "x",
            "city",
            Refused(Refusal::NewIsADimension),
        ),
        row(
            "a variable renamed to a dimension no equation iterates",
            cities()
                .aux("x", "5", None)
                .aux("reader", "x * 10", None)
                .build_datamodel(),
            "x",
            "kind",
            Refused(Refusal::NewIsADimension),
        ),
        row(
            "a dynamic index renamed to an element of the axis",
            cities()
                .aux("pick", "2", None)
                .aux("reader", "arr[pick]", None)
                .build_datamodel(),
            "pick",
            "boston",
            Refused(Refusal::NewIsAnElementInBrackets),
        ),
        row(
            "a range end renamed to an element of the axis",
            cities()
                .aux("lo", "1", None)
                .aux("hi", "2", None)
                .aux("reader", "SUM(arr[lo:hi])", None)
                .build_datamodel(),
            "hi",
            "la",
            Refused(Refusal::NewIsAnElementInBrackets),
        ),
        Row {
            model: "sub",
            ..row(
                "a sub-model's variable renamed so a read through an instance named for \
                 a dimension spells one of its elements",
                {
                    let mut project = cities()
                        .aux("reader", "city.x * 10", None)
                        .build_datamodel();
                    project.models[0].variables.push(module("city", "sub", &[]));
                    project.models.push(model("sub", vec![aux("x", "7")]));
                    project
                },
                "x",
                "boston",
                Refused(Refusal::NewSpellsAQualifiedElement),
            )
        },
        Row {
            model: "sub",
            ..row(
                "a sub-model's dynamic index renamed to an element, read through an \
                 instance by a model the rename visits first",
                {
                    let mut project =
                        cities_with_an_instance(cities().aux("reader", "inst.pick * 2", None));
                    project.models[1].variables.push(aux("reader", "arr[pick]"));
                    project
                },
                "pick",
                "boston",
                Refused(Refusal::NewIsAnElementInBrackets),
            )
        },
        Row {
            model: "sub",
            ..row(
                "a qualified element beside an instance named for its dimension, the \
                 instantiated model's like-named variable renamed",
                {
                    let mut project = cities()
                        .aux("reader", "city.la * 10", None)
                        .build_datamodel();
                    project.models[0].variables.push(module("city", "sub", &[]));
                    project.models.push(model("sub", vec![aux("la", "7")]));
                    project
                },
                "la",
                "other",
                Respelled("city.la * 10"),
            )
        },
        // Where no name is a dimension's or an element's, an identifier in
        // brackets is the variable, and the rename respells it.
        row(
            "a dynamic index",
            cities()
                .aux("pick", "2", None)
                .aux("reader", "arr[pick]", None)
                .build_datamodel(),
            "pick",
            "choice",
            Respelled("arr[choice]"),
        ),
        row(
            "the ends of a dynamic range",
            cities()
                .aux("lo", "1", None)
                .aux("hi", "2", None)
                .aux("reader", "SUM(arr[lo:hi])", None)
                .build_datamodel(),
            "hi",
            "top",
            Respelled("SUM(arr[lo:top])"),
        ),
        row(
            "a dynamic index among two dimensions",
            cities()
                .array_aux("m[city,kind]", "3")
                .aux("pick", "2", None)
                .aux("reader", "m[pick, small] + m[la, pick]", None)
                .build_datamodel(),
            "pick",
            "choice",
            Respelled("m[choice, small] + m[la, choice]"),
        ),
        row(
            "an element's name inside an index expression is the variable",
            cities()
                .aux("boston", "2", None)
                .aux("reader", "arr[boston + 0]", None)
                .build_datamodel(),
            "boston",
            "nyc",
            Respelled("arr[nyc + 0]"),
        ),
        row(
            "an element's name outside the brackets is the variable",
            cities()
                .aux("boston", "2", None)
                .aux("reader", "arr[la] * boston", None)
                .build_datamodel(),
            "boston",
            "nyc",
            Respelled("arr[la] * nyc"),
        ),
        row(
            "a variable renamed to an element's name, never in brackets",
            cities()
                .aux("x", "5", None)
                .aux("reader", "arr[la] * x", None)
                .build_datamodel(),
            "x",
            "boston",
            Respelled("arr[la] * boston"),
        ),
        Row {
            model: "sub",
            ..row(
                "a dynamic index read through an instance",
                cities_with_an_instance(cities().aux("reader", "arr[inst.pick]", None)),
                "pick",
                "choice",
                Respelled("arr[inst·choice]"),
            )
        },
        Row {
            model: "sub",
            ..row(
                "a dynamic index read through an instance, renamed to an element's name",
                cities_with_an_instance(cities().aux("reader", "arr[inst.pick]", None)),
                "pick",
                "boston",
                Respelled("arr[inst·boston]"),
            )
        },
        row(
            "a subscript of a variable read through an instance",
            cities_with_an_instance(cities().aux("reader", "inst.arr[la] * inst.pick", None)),
            "inst",
            "unit",
            Respelled("unit·arr[la] * unit·pick"),
        ),
    ];

    let covered: BTreeSet<Refusal> = rows
        .iter()
        .filter_map(|row| match row.expect {
            Expect::Refused(refusal) => Some(refusal),
            Expect::Respelled(_) => None,
        })
        .collect();
    assert_eq!(
        covered,
        Refusal::ALL.into_iter().collect::<BTreeSet<Refusal>>(),
        "a row for every refusal"
    );

    for row in rows {
        let before = outcome(&row.project);
        let mut project = row.project.clone();
        // In place, so a refusal is held to leaving the project as it was.
        let applied = apply_patch_in_place(&mut project, rename_in(row.model, row.from, row.to));
        match row.expect {
            Expect::Refused(refusal) => {
                let err = applied.err().unwrap_or_else(|| {
                    panic!(
                        "{}: renamed, the reader reading {:?}",
                        row.what,
                        scalar(&project, "main", row.reader)
                    )
                });
                let reason = err.to_string();
                assert!(reason.contains(refusal.says()), "{}: {reason}", row.what);
                let names_the_equation = matches!(
                    refusal,
                    Refusal::OldIsAnElementInBrackets
                        | Refusal::NewIsAnElementInBrackets
                        | Refusal::NewSpellsAQualifiedElement
                );
                if names_the_equation {
                    assert!(
                        reason.contains(&format!("the equation of '{}'", row.reader)),
                        "{}: {reason}",
                        row.what
                    );
                }
                assert!(
                    project == row.project,
                    "{}: refused, it changed nothing",
                    row.what
                );
            }
            Expect::Respelled(after) => {
                applied.unwrap_or_else(|err| panic!("{}: {err}", row.what));
                assert_eq!(scalar(&project, "main", row.reader), after, "{}", row.what);
                let before = before.unwrap_or_else(|err| panic!("{}: the model {err}", row.what));
                let after = outcome(&project)
                    .unwrap_or_else(|err| panic!("{}: renamed, the model {err}", row.what));
                assert_same_run_in(&row.project, &before, after, row.model, row.from, row.to);
            }
        }
    }
}

/// A model whose `reader` reads `x`, for renaming `x` to a builtin's name.
fn reads_x() -> datamodel::Project {
    project(vec![model(
        "main",
        vec![aux("x", "5"), aux("reader", "x * 10")],
    )])
}

/// A variable named for one of the simulation's own globals (an imported
/// `FINAL TIME`) is renamed, and an equation that writes the global keeps
/// reading it: the parser reads that name as the builtin, as it reads `TIME`,
/// so it is no reference to the variable, and the reader computes what it
/// did.
#[test]
fn a_rename_of_a_variable_named_for_a_global_leaves_the_equations_reading_it() {
    let base = project(vec![model(
        "main",
        vec![
            aux("Final Time", "50"),
            aux("reader", "FINAL_TIME * 2 + TIME"),
        ],
    )]);
    let before = series(&base);
    let mut renamed = base.clone();
    apply_patch(&mut renamed, rename_in("main", "Final Time", "horizon")).unwrap();
    assert_eq!(scalar(&renamed, "main", "reader"), "FINAL_TIME * 2 + TIME");
    assert!(renamed.models[0].get_variable("horizon").is_some());
    let after = series(&renamed);
    assert_eq!(after["reader"], before["reader"]);
    assert_eq!(after["horizon"], vec![50.0; after["horizon"].len()]);
}

/// A rename does not give a variable one of the engine's built-in clock names
/// (`db::IMPLICIT_GLOBALS`): one row per name, as written canonically and in
/// a display spelling, each refused, changing nothing. A name that only
/// contains one is a name like another, and a model that already declares
/// one keeps it, its display spelling included.
#[test]
fn a_rename_to_a_clock_name_is_refused() {
    let display = |name: &str| name.replace('_', " ").to_uppercase();
    for clock in crate::db::IMPLICIT_GLOBALS {
        for to in [clock.to_string(), display(clock)] {
            let base = reads_x();
            let mut project = base.clone();
            let applied = apply_patch_in_place(&mut project, rename_in("main", "x", &to));
            let reason = applied.err().map(|err| err.to_string()).unwrap_or_default();
            assert!(reason.contains("clock names"), "{to}: {reason:?}");
            assert!(project == base, "{to}: refused, it changed nothing");
        }
        for to in [format!("{clock} spent"), format!("{clock}2")] {
            let base = reads_x();
            let mut project = base.clone();
            apply_patch(&mut project, rename_in("main", "x", &to))
                .unwrap_or_else(|err| panic!("{to}: {err}"));
            assert_same_run(&series(&base), series(&project), "x", &canonicalize(&to));
        }
    }

    let mut declared = project(vec![model(
        "main",
        vec![aux("Final Time", "50"), aux("reader", "TIME")],
    )]);
    apply_patch(&mut declared, rename_in("main", "Final Time", "FINAL TIME")).unwrap();
    assert_eq!(declared.models[0].variables[0].get_ident(), "FINAL TIME");
}

/// A model that declares a variable under a clock slot's name can rename it
/// away: its quoted references are the variable's and are respelled, the bare
/// word is the builtin and is left, and the model computes what it computed.
/// The slot's results key, which the variable held, is the clock's again.
#[test]
fn a_variable_named_for_a_clock_slot_is_renamed_away() {
    let base = project(vec![model(
        "main",
        vec![aux("dt", "0.5"), aux("reader", "\"dt\" * 10 + dt")],
    )]);
    let mut renamed = base.clone();
    apply_patch(&mut renamed, rename_in("main", "dt", "half")).unwrap();
    assert_eq!(scalar(&renamed, "main", "reader"), "half * 10 + dt");
    let (before, after) = (series(&base), series(&renamed));
    assert!(after["reader"] == before["reader"], "the reader's series");
    assert!(after["half"] == before["dt"], "the variable's series");
    assert!(before["dt"].iter().all(|value| *value == 0.5));
    let step = match base.sim_specs.dt {
        datamodel::Dt::Dt(dt) => dt,
        datamodel::Dt::Reciprocal(per) => 1.0 / per,
    };
    assert!(step != 0.5 && after["dt"].iter().all(|value| *value == step));
}

/// A variable renamed to the name of a builtin that takes no arguments
/// computes what it computed: the rename does not refuse such a name, so the
/// spelling it writes must read as the variable, which quoted it does
/// (`"pi" * 10`).
#[test]
fn a_rename_to_a_zero_argument_builtins_name_changes_no_series() {
    let base = reads_x();
    let mut renamed = base.clone();
    apply_patch(&mut renamed, rename_in("main", "x", "pi")).unwrap();
    let text = scalar(&renamed, "main", "reader");
    let after = outcome(&renamed).unwrap_or_else(|err| panic!("{text:?}: the model {err}"));
    assert!(
        after["reader"] == series(&base)["reader"],
        "renamed to `pi`, the reader computes something else: {text:?}"
    );
}

/// A name with a literal period in it is one name, spelled quoted: renaming
/// to it and away from it keeps the model computing the same.
#[test]
fn a_rename_to_and_from_a_name_with_a_period_keeps_the_model_the_same() {
    let base = project(vec![model(
        "main",
        vec![aux("x", "5"), aux("reader", "x * 2")],
    )]);
    let before = series(&base);

    let mut project = base.clone();
    apply_patch(&mut project, rename_in("main", "x", "\"a.b\"")).unwrap();
    assert_eq!(scalar(&project, "main", "reader"), "\"a.b\" * 2");
    assert_eq!(series(&project)["reader"], before["reader"]);

    apply_patch(&mut project, rename_in("main", "\"a.b\"", "c")).unwrap();
    assert_eq!(scalar(&project, "main", "reader"), "c * 2");
    assert_eq!(series(&project)["reader"], before["reader"]);
}

fn to(new: &'static str) -> impl Fn(Site, &Ident<Canonical>) -> Respelled {
    move |_, reference| Ok((reference.as_str() == "target").then(|| Ident::new(new)))
}

/// The text `renamed` respells, for a respelling that finds nothing
/// ambiguous.
fn spliced(text: &str, renamed: &Respelling<'_>) -> Option<String> {
    renamed_text(text, renamed).unwrap_or_else(|Ambiguity(why)| panic!("{text:?} {why}"))
}

/// The tree the one walk gives `expr` under `renamed`.
fn renamed_tree(expr: &Expr0, renamed: &Respelling<'_>) -> Expr0 {
    Walk {
        renamed,
        splices: Vec::new(),
        ambiguity: None,
    }
    .expr(expr)
}

/// A rename changes the references it renames and nothing else of the text.
#[test]
fn a_rename_keeps_everything_but_the_reference_as_written() {
    let rows: &[(&str, Option<&str>)] = &[
        ("target + 1", Some("renamed_thing + 1")),
        ("  Target   *2", Some("  renamed_thing   *2")),
        (
            "\"target\" + \"Other Thing\"",
            Some("renamed_thing + \"Other Thing\""),
        ),
        (
            "IF target > 0 THEN NAN ELSE -0.0",
            Some("IF renamed_thing > 0 THEN NAN ELSE -0.0"),
        ),
        (
            "SMTH1( target, 3 ) + PREVIOUS(target,0) + Init(TARGET)",
            Some("SMTH1( renamed_thing, 3 ) + PREVIOUS(renamed_thing,0) + Init(renamed_thing)"),
        ),
        (
            "target[Dim_A, 1] + Other_Var[target] * MAX( target , 3)",
            Some("renamed_thing[Dim_A, 1] + Other_Var[renamed_thing] * MAX( renamed_thing , 3)"),
        ),
        (
            "Other_Var[target:Target, *:target, target + 1]",
            Some("Other_Var[renamed_thing:renamed_thing, *:target, renamed_thing + 1]"),
        ),
        ("é * target", Some("é * renamed_thing")),
        (
            "target+target*target-target",
            Some("renamed_thing+renamed_thing*renamed_thing-renamed_thing"),
        ),
        (
            "{a comment about target} target",
            Some("{a comment about target} renamed_thing"),
        ),
        ("Other_Thing + targeted", None),
        ("-0.0 + 0.0 + 1e308 * 10", None),
        ("target +* (", None),
        ("", None),
    ];
    for (text, want) in rows {
        assert_eq!(
            spliced(text, &to("renamed thing")).as_deref(),
            *want,
            "{text:?}"
        );
    }

    // A new name the lexer cannot read bare is spelled quoted, a literal
    // period in it included.
    for (new, want) in [("if", "\"if\" + 1"), ("\"a.b\"", "\"a.b\" + 1")] {
        assert_eq!(
            spliced("target + 1", &to(new)).as_deref(),
            Some(want),
            "to {new}"
        );
    }

    // A builtin's name is no reference, however a variable is named.
    for text in ["TIME + pi * max(1, 2)", "time"] {
        for builtin in ["time", "pi", "max"] {
            let renames = |_: Site, reference: &Ident<Canonical>| {
                Ok((reference.as_str() == builtin).then(|| Ident::new("renamed")))
            };
            assert_eq!(spliced(text, &renames), None, "{builtin} in {text:?}");
        }
    }
}

/// The walk puts each reference to the respelling with the site it is
/// written at, and one reference the respelling finds ambiguous refuses the
/// whole text, wherever in the text it stands.
#[test]
fn the_walk_reads_each_reference_at_its_site() {
    // The match names every site, so a site added fails to compile here.
    let site_name = |site: Site| match site {
        Site::Value => "value",
        Site::Subscripted => "subscripted",
        Site::Index => "index",
    };
    let rows: &[(&str, &[(&str, &str)])] = &[
        (
            "a + f(b) * -c",
            &[("a", "value"), ("b", "value"), ("c", "value")],
        ),
        (
            "a[b, c:d, e + f, *:g, @1, *]",
            &[
                ("a", "subscripted"),
                ("b", "index"),
                ("c", "index"),
                ("d", "index"),
                ("e", "value"),
                ("f", "value"),
            ],
        ),
        (
            "a[b[c]] + a[f(c)]",
            &[
                ("a", "subscripted"),
                ("b", "subscripted"),
                ("c", "index"),
                ("a", "subscripted"),
                ("c", "value"),
            ],
        ),
        (
            "IF a THEN b[c] ELSE PREVIOUS(d[e], 0)",
            &[
                ("a", "value"),
                ("b", "subscripted"),
                ("c", "index"),
                ("d", "subscripted"),
                ("e", "index"),
            ],
        ),
    ];
    for (text, want) in rows {
        let seen = std::cell::RefCell::new(Vec::new());
        let record = |site: Site, reference: &Ident<Canonical>| {
            seen.borrow_mut()
                .push((reference.as_str().to_string(), site_name(site)));
            Ok(None)
        };
        assert_eq!(spliced(text, &record), None);
        let want: Vec<(String, &str)> = want
            .iter()
            .map(|(name, site)| (name.to_string(), *site))
            .collect();
        assert_eq!(seen.into_inner(), want, "{text:?}");
    }

    let refusing = |site: Site, reference: &Ident<Canonical>| match reference.as_str() {
        "target" if site == Site::Index => Err(Ambiguity("is ambiguous".to_string())),
        "target" => Ok(Some(Ident::new("renamed"))),
        _ => Ok(None),
    };
    for text in [
        "target + arr[target]",
        "arr[target] + target",
        "arr[1:target] * target",
    ] {
        assert!(renamed_text(text, &refusing).is_err(), "{text:?}");
    }
    assert_eq!(
        spliced("target + arr[target + 1] + target[1]", &refusing).as_deref(),
        Some("renamed + arr[renamed + 1] + renamed[1]")
    );
}

/// The spliced text is the renamed expression: it parses to the tree the
/// rename gives.
fn assert_renamed(text: &str, renamed: &str, new: &'static str) {
    let original = Expr0::new(text, LexerType::Equation)
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("the text parses"));
    let tree = renamed_tree(&original, &to(new));
    let reparsed = Expr0::new(renamed, LexerType::Equation)
        .ok()
        .flatten()
        .unwrap_or_else(|| panic!("the renamed text parses"));
    assert!(
        print_eqn(&reparsed) == print_eqn(&tree),
        "the renamed text is {:.60}..., not {:.60}",
        print_eqn(&reparsed),
        print_eqn(&tree)
    );
    assert!(
        reparsed.get_var_loc("target").is_none(),
        "no reference is left"
    );
}

/// A span is a pair of `u16`s, so a reference past that reach has a span that
/// has wrapped to another place in the text. A text that long is respelled by
/// the printer, whatever stands where the wrapped span points: the same
/// letters as another reference, or the same letters inside another name.
#[test]
fn a_rename_in_a_text_past_a_spans_reach_does_not_trust_the_spans() {
    // The second `target` starts at byte 65,536, which a `u16` reads as 0,
    // where the first one is.
    let text = format!("target +{}target", " ".repeat(SPAN_REACH + 1 - 8));
    assert_eq!(text.rfind("target"), Some(SPAN_REACH + 1));
    let renamed = spliced(&text, &to("renamed thing")).expect("the text names the target");
    assert_eq!(renamed, "renamed_thing + renamed_thing");

    // The last `target` wraps onto the same letters inside `untarget`.
    let text = format!(
        "target + untarget{} + target",
        " ".repeat(SPAN_REACH + 1 + 11 - 17 - 3)
    );
    assert_eq!(text.rfind("target"), Some(SPAN_REACH + 1 + 11));
    assert_eq!(text.find("untarget"), Some(9));
    let renamed = spliced(&text, &to("renamed thing")).expect("the text names the target");
    assert_eq!(renamed, "renamed_thing + untarget + renamed_thing");
}

/// Up to a span's reach the text is spliced, keeping the modeler's layout;
/// one byte past it, it is printed. Either way it is the renamed expression.
#[test]
fn a_rename_splices_a_text_exactly_as_long_as_a_span_reaches() {
    for len in [SPAN_REACH - 1, SPAN_REACH, SPAN_REACH + 1, SPAN_REACH + 2] {
        for head in ["", "1 +"] {
            let text = format!("{head}{}target", " ".repeat(len - head.len() - 6));
            assert_eq!(text.len(), len);
            let renamed = spliced(&text, &to("other")).expect("the text names the target");
            assert_renamed(&text, &renamed, "other");
            let spliced = text.replace("target", "other");
            if len <= SPAN_REACH {
                assert!(renamed == spliced, "a text of {len} bytes keeps its layout");
            } else {
                assert!(renamed != spliced, "a text of {len} bytes is printed");
            }
        }
    }
}

/// For a sample of `limit` variables of each model, renamed in turn: every
/// text the rename changes parses to the tree with that reference renamed.
fn check_spliced_renames_parse_to_the_renamed_tree(project: &datamodel::Project, limit: usize) {
    for (index, model) in project.models.iter().enumerate() {
        let idents: Vec<String> = model
            .variables
            .iter()
            .map(|v| v.get_ident().to_string())
            .collect();
        let step = idents.len().div_ceil(limit).max(1);
        for ident in idents.iter().step_by(step) {
            let (old, new) = (Ident::new(ident), Ident::new(&format!("{ident} renamed")));
            let rename = Rename::new(project, index, &old, &new);
            let reader = Reader {
                rename: &rename,
                model: index,
            };
            let renamed =
                |site: Site, reference: &Ident<Canonical>| reader.renamed(site, reference);
            for variable in &model.variables {
                for (_, text) in variable.expression_texts() {
                    // A rename refused for an ambiguous reference writes no
                    // text at all.
                    let Ok(Some(spliced)) = renamed_text(text, &renamed) else {
                        continue;
                    };
                    let Ok(Some(original)) = Expr0::new(text, LexerType::Equation) else {
                        unreachable!("a text the rename changed parses")
                    };
                    let tree = renamed_tree(&original, &renamed);
                    let reparsed = Expr0::new(&spliced, LexerType::Equation)
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| panic!("{spliced:?} does not parse"));
                    assert_eq!(
                        print_eqn(&reparsed),
                        print_eqn(&tree),
                        "renaming {ident} in {text:?} gave {spliced:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn a_spliced_rename_parses_to_the_renamed_tree() {
    check_spliced_renames_parse_to_the_renamed_tree(&hares_and_lynxes(), usize::MAX);
}

#[test]
#[ignore = "renames variables of every corpus model; run under the gates profile"]
fn a_spliced_rename_parses_to_the_renamed_tree_across_the_corpus() {
    let mut models = 0;
    crate::patch_sharing_tests::for_each_corpus_project(|_, project| {
        models += 1;
        check_spliced_renames_parse_to_the_renamed_tree(project, 40);
    });
    assert!(models > 100, "the corpus opens: {models} models");
}

/// A rename is refused, changing nothing, or the model computes what it
/// computed: for a sample of `limit` variables of each of the project's
/// models, renamed in turn, the run saves the same series under the new name
/// unless the rename is refused. Returns how many renames ran and how many
/// were refused; none when the project does not run to begin with.
fn check_renames_change_no_series(project: &datamodel::Project, limit: usize) -> (usize, usize) {
    let Ok(before) = outcome(project) else {
        return (0, 0);
    };
    let (mut checked, mut refused) = (0, 0);
    for model in &project.models {
        let idents: Vec<&str> = model.variables.iter().map(|v| v.get_ident()).collect();
        let step = idents.len().div_ceil(limit).max(1);
        for ident in idents.into_iter().step_by(step) {
            let to = "Renamed By The Test";
            let mut renamed = project.clone();
            if apply_patch_in_place(&mut renamed, rename_in(&model.name, ident, to)).is_err() {
                assert!(
                    renamed == *project,
                    "renaming {ident} was refused, and changed the project"
                );
                refused += 1;
                continue;
            }
            let mut after = outcome(&renamed)
                .unwrap_or_else(|err| panic!("renamed {ident}, and the project {err}"));
            if crate::db::is_implicit_global(&canonicalize(ident)) {
                // The run saves the global under that name, and the variable
                // has no series of its own until the rename gives it one:
                // every series that was saved is saved as it was.
                let new = canonicalize(to);
                after.retain(|name, _| renamed_key(name, &new, &new).is_none());
                assert_same_run_renaming(&before, after, &[]);
            } else {
                assert_same_run_in(project, &before, after, &model.name, ident, to);
            }
            checked += 1;
        }
    }
    (checked, refused)
}

#[test]
fn a_rename_changes_no_series() {
    assert_eq!(
        check_renames_change_no_series(&hares_and_lynxes(), 3),
        (8, 0)
    );
}

/// The corpus samples ordinary names: of its 7,590 variables one is named
/// like a dimension or an element, and the sample misses it, so the renames
/// the subscript rule refuses are covered by the fixture rows of
/// `a_rename_is_refused_or_changes_no_series` alone.
#[test]
#[ignore = "renames variables of every corpus model and runs each; run under the gates profile"]
fn a_rename_changes_no_series_across_the_corpus() {
    let (mut checked, mut refused) = (0, 0);
    crate::patch_sharing_tests::for_each_corpus_project(|_, project| {
        let (ran, declined) = check_renames_change_no_series(project, 3);
        checked += ran;
        refused += declined;
    });
    assert!(checked > 300, "the corpus runs: {checked} renames checked");
    assert_eq!(refused, 0, "the sample holds no name the rename refuses");
}
