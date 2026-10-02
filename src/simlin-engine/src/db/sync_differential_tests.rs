// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The one claim of the incremental sync: whatever sequence of syncs, staged
//! syncs and rollbacks a db has been through, its inputs hold exactly what a
//! fresh db synced to the same project holds, and so it answers what a fresh
//! db answers: every diagnostic, every series of a run, the expanded twin of
//! a special-stock project, the structural loops.
//!
//! The pool of projects varies every field the sync writes and shares
//! allocations among its projects in every way the skip in `InputMemory` has
//! to see through: one variable edited, renamed, deleted and added back;
//! variables reordered; a variable changing kind; two variables of one
//! canonical name; a module wired, rewired, left without its model, repointed
//! and its model renamed; two models swapping their variables; models of one
//! canonical name, in either order and three at once; a root model respelled
//! and unnamed; project models named for a stdlib model; the project's and a
//! model's specs; a dimension grown; a macro and the same model no longer
//! one; a pinned loop; no models at all; and four special-stock projects.

use std::io::BufReader;

use super::*;
use crate::patch::{ModelOperation, ModelPatch, ProjectPatch, apply_patch};
use crate::test_common::TestProject;

/// What one input holds, read back field by field. The struct literal names
/// every field of `SourceVariableFields`, so a field added to the extraction
/// fails to compile here until the comparison reads it.
fn input_fields(db: &SimlinDb, v: SourceVariable) -> SourceVariableFields {
    SourceVariableFields {
        ident: v.ident(db).clone(),
        equation: v.equation(db).clone(),
        kind: v.kind(db),
        units: v.units(db).clone(),
        gf: v.gf(db).clone(),
        inflows: v.inflows(db).clone(),
        outflows: v.outflows(db).clone(),
        repeated_inflows: v.repeated_inflows(db).clone(),
        repeated_outflows: v.repeated_outflows(db).clone(),
        module_refs: v.module_refs(db).clone(),
        referenced_model_name: v.model_name(db).clone(),
        owner_model: v.owner_model(db).clone(),
        non_negative: v.non_negative(db),
        can_be_module_input: v.can_be_module_input(db),
        compat: v.compat(db).clone(),
    }
}

/// Everything the sync writes, reachable from the project's handle, one line
/// per input: the project's own fields, then each model (the stdlib's
/// included) and each of its variables in name order.
fn inputs(db: &SimlinDb, project: SourceProject) -> Vec<String> {
    let mut lines = vec![format!(
        "project: {:?} {:?} {:?} {:?} {:?} {:?}",
        project.name(db),
        project.sim_specs(db),
        project.dimensions(db),
        project.units(db),
        project.model_names(db),
        project.macro_declarations(db),
    )];
    let mut models: Vec<(&String, &SourceModel)> = project.models(db).iter().collect();
    models.sort_by_key(|(name, _)| name.as_str());
    for (key, model) in models {
        lines.push(format!(
            "model {key}: {:?} {:?} {:?} {:?} {:?} {:?}",
            model.name(db),
            model.variable_names(db),
            model.declared_variable_idents(db),
            model.model_sim_specs(db),
            model.macro_spec(db),
            model.pinned_loops(db),
        ));
        let mut variables: Vec<(&String, &SourceVariable)> = model.variables(db).iter().collect();
        variables.sort_by_key(|(name, _)| name.as_str());
        for (name, variable) in variables {
            lines.push(format!(
                "variable {key}.{name}: {:?}",
                input_fields(db, *variable)
            ));
        }
    }
    lines
}

/// What the db answers for `project`, the project it is synced to: every
/// diagnostic (with the LTM overlay off and on), the run of its first model
/// bit for bit (or the refusal), the expanded twin's inputs when the run
/// expanded one, and how many loops its structure has.
fn answers(db: &mut SimlinDb, project: &datamodel::Project) -> Vec<String> {
    let source = db
        .current_source_project()
        .unwrap_or_else(|| unreachable!("the db is synced"));
    let mut out = Vec::new();
    for overlay in [LtmOverlay::Off, LtmOverlay::On] {
        let mut rows: Vec<String> = collect_all_diagnostics(db, source, overlay)
            .into_iter()
            .map(|d| format!("diagnostic {overlay:?}: {d:?}"))
            .collect();
        rows.sort();
        out.extend(rows);
    }
    let Some(main) = project.models.first() else {
        out.push("no model".to_string());
        return out;
    };
    let special = crate::conveyor_compile::project_has_conveyor(project, &main.name)
        || crate::queue_compile::project_has_queue(project, &main.name);
    match crate::queue_compile::build_sim(db, source, project, &main.name, LtmOverlay::Off) {
        Ok(mut vm) => match vm.run_to_end() {
            Ok(()) => {
                let results = crate::test_common::collect_results(&vm.into_results());
                let mut names: Vec<&String> = results.keys().collect();
                names.sort();
                for name in names {
                    let bits: Vec<u64> = results[name].iter().map(|v| v.to_bits()).collect();
                    out.push(format!("series {name}: {bits:?}"));
                }
            }
            Err(err) => out.push(format!("run error: {err}")),
        },
        Err(err) => out.push(format!("build error: {} {:?}", err.code, err.details)),
    }
    if special && let Some(expanded) = db.expanded_source_project() {
        out.extend(
            inputs(db, expanded)
                .into_iter()
                .map(|line| format!("expanded {line}")),
        );
    }
    let canonical = canonicalize(&main.name);
    if let Some(model) = source.models(db).get(canonical.as_ref()).copied()
        && project_module_graph(db, source)
            .cycle_error_from(canonical.as_ref())
            .is_none()
    {
        out.push(format!(
            "loops: {}",
            model_detected_loops(db, model, source).loops.len()
        ));
    }
    out
}

/// What a fresh db synced to `project` holds and, when `with_answers`, what
/// it answers.
fn fresh(project: &datamodel::Project, with_answers: bool) -> (Vec<String>, Vec<String>) {
    let mut db = SimlinDb::default();
    let source = db.sync(project);
    let held = inputs(&db, source);
    let answered = if with_answers {
        answers(&mut db, project)
    } else {
        Vec::new()
    };
    (held, answered)
}

fn aux(ident: &str, equation: &str) -> datamodel::Aux {
    datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    }
}

fn module(ident: &str, model_name: &str, references: &[(&str, &str)]) -> datamodel::Module {
    datamodel::Module {
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
        compat: datamodel::Compat::default(),
    }
}

fn model(name: &str, variables: Vec<datamodel::Aux>) -> datamodel::Model {
    datamodel::Model {
        name: name.to_string(),
        sim_specs: None,
        variables: variables
            .into_iter()
            .map(datamodel::Variable::Aux)
            .collect::<Vec<_>>()
            .into(),
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    }
}

/// `project` with `ops` applied to its model `model`.
fn patched_in(
    project: &datamodel::Project,
    model: &str,
    ops: Vec<ModelOperation>,
) -> datamodel::Project {
    let mut edited = project.clone();
    apply_patch(
        &mut edited,
        ProjectPatch {
            project_ops: vec![],
            models: vec![ModelPatch {
                name: model.to_string(),
                ops,
            }],
        },
    )
    .expect("the edit applies");
    edited
}

fn patched(project: &datamodel::Project, ops: Vec<ModelOperation>) -> datamodel::Project {
    patched_in(project, "main", ops)
}

fn open(xmile: &str) -> datamodel::Project {
    crate::compat::open_xmile(&mut BufReader::new(xmile.as_bytes())).expect("it opens")
}

fn pool() -> Vec<(&'static str, datamodel::Project)> {
    let mut pool: Vec<(&'static str, datamodel::Project)> = Vec::new();
    let mut add = |name: &'static str, project: datamodel::Project| pool.push((name, project));

    let base = TestProject::new("differential")
        .aux("rate", "0.5", None)
        .aux("scale", "2", None)
        .flow("inflow", "rate * scale", None)
        .stock("level", "10", &["inflow"], &[], None)
        .aux("reader", "SMTH1(level * scale, 2)", None)
        .build_datamodel();
    add("the base", base.clone());
    let edited = patched(&base, vec![ModelOperation::UpsertAux(aux("scale", "3"))]);
    add("one variable edited", edited.clone());
    add(
        "one renamed",
        patched(
            &edited,
            vec![ModelOperation::RenameVariable {
                from: "rate".to_string(),
                to: "speed".to_string(),
            }],
        ),
    );
    let deleted = patched(
        &base,
        vec![ModelOperation::DeleteVariable {
            ident: "reader".to_string(),
        }],
    );
    add("one deleted", deleted.clone());
    add(
        "one added back",
        patched(
            &deleted,
            vec![ModelOperation::UpsertAux(aux("reader", "level * 7"))],
        ),
    );
    let mut reversed = base.clone();
    reversed.models[0].variables.reverse();
    add("variables reversed", reversed);

    // One name, another kind of variable: an aux, then a flow, then a stock.
    let as_flow = patched(
        &base,
        vec![ModelOperation::UpsertFlow(datamodel::Flow {
            ident: "scale".to_string(),
            equation: datamodel::Equation::Scalar("2".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        })],
    );
    add("scale a flow", as_flow);
    add(
        "scale a stock",
        patched(
            &base,
            vec![ModelOperation::UpsertStock(datamodel::Stock {
                ident: "scale".to_string(),
                equation: datamodel::Equation::Scalar("2".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["inflow".to_string()],
                outflows: vec![],
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })],
        ),
    );

    let mut twins = edited.clone();
    for variable in [aux("twin", "1"), aux("Twin", "2")] {
        twins.models[0]
            .variables
            .push(datamodel::Variable::Aux(variable));
    }
    add("two of one name", twins.clone());
    let mut twins_reversed = twins;
    twins_reversed.models[0].variables.reverse();
    add("two of one name, reversed", twins_reversed);

    // A module, wired; rewired; without its model; the sub-model edited; the
    // model renamed (the instance dangling) and the instance repointed.
    let mut modular = base.clone();
    modular.models.push(model(
        "other",
        vec![
            aux("input", "0"),
            aux("output", "input * 2"),
            aux("scale", "5"),
        ],
    ));
    let modular = patched(
        &modular,
        vec![
            ModelOperation::UpsertModule(module("inst", "other", &[("scale", "inst.input")])),
            ModelOperation::UpsertAux(aux("via", "inst.output + 1")),
        ],
    );
    add("a module", modular.clone());
    add(
        "the module rewired",
        patched(
            &modular,
            vec![ModelOperation::UpsertModule(module(
                "inst",
                "other",
                &[("rate", "inst.input")],
            ))],
        ),
    );
    let mut no_model = modular.clone();
    no_model.models.pop();
    add("the module without its model", no_model);
    add(
        "the module's model edited",
        patched_in(
            &modular,
            "other",
            vec![ModelOperation::UpsertAux(aux("output", "input * 3"))],
        ),
    );
    let mut model_renamed = modular.clone();
    model_renamed.models[1].name = "another".to_string();
    add("the module's model renamed", model_renamed.clone());
    add(
        "the module repointed",
        patched(
            &model_renamed,
            vec![ModelOperation::UpsertModule(module(
                "inst",
                "another",
                &[("scale", "inst.input")],
            ))],
        ),
    );
    let mut swapped = modular.clone();
    let (first, second) = (
        swapped.models[0].variables.clone(),
        swapped.models[1].variables.clone(),
    );
    swapped.models[0].variables = second;
    swapped.models[1].variables = first;
    add("two models' variables swapped", swapped);
    let mut order = modular;
    order.models.reverse();
    add("the models reversed", order);

    // A second model sharing every variable of the first, then edited; both
    // models holding the edited variables.
    let mut two_models = base.clone();
    let mut other = two_models.models[0].clone();
    other.name = "other".to_string();
    two_models.models.push(other);
    add("a second model sharing every variable", two_models.clone());
    let mut second_edited = two_models;
    if let Some(datamodel::Variable::Aux(scale)) = second_edited.models[1].get_variable_mut("scale")
    {
        scale.equation = datamodel::Equation::Scalar("9".to_string());
    }
    add("the second model edited", second_edited);
    let mut moved = edited.clone();
    let mut other = edited.models[0].clone();
    other.name = "other".to_string();
    moved.models.push(other);
    add("both models holding the edited variables", moved);

    // Models of one canonical name: each with a `scale` of its own, in both
    // orders; with disjoint variables, in both orders; three at once.
    let mut same_name = base.clone();
    let mut duplicate = edited.models[0].clone();
    duplicate.name = "Main".to_string();
    same_name.models.push(duplicate);
    add("two models of one name", same_name.clone());
    let mut same_name_reversed = same_name.clone();
    same_name_reversed.models.reverse();
    add("two models of one name, reversed", same_name_reversed);
    let mut disjoint = base.clone();
    disjoint.models.push(model(
        "MAIN",
        vec![aux("only here", "1"), aux("scale", "11")],
    ));
    add("two models of one name, disjoint", disjoint.clone());
    let mut disjoint_reversed = disjoint;
    disjoint_reversed.models.reverse();
    add(
        "two models of one name, disjoint, reversed",
        disjoint_reversed,
    );
    let mut three = same_name;
    let mut third = base.models[0].clone();
    third.name = "MAIN".to_string();
    three.models.push(third);
    add("three models of one name", three);

    let mut respelled = base.clone();
    respelled.models[0].name = "Main".to_string();
    add("the root model named Main", respelled);
    let mut unnamed = base.clone();
    unnamed.models[0].name = String::new();
    add("the root model unnamed", unnamed);

    // A project model named for a stdlib model: present, edited, the stdlib
    // model's own definition, respelled, and twice.
    let stand_in = |name: &str, output: &str| {
        model(
            name,
            vec![
                aux("output", output),
                aux("input", "0"),
                aux("delay_time", "1"),
                aux("initial_value", "0"),
            ],
        )
    };
    let mut shadow = base.clone();
    shadow.models.push(stand_in("stdlib\u{205A}smth1", "42"));
    add("a model named for a stdlib model", shadow.clone());
    let mut shadow_edited = shadow.clone();
    if let Some(datamodel::Variable::Aux(output)) =
        shadow_edited.models[1].get_variable_mut("output")
    {
        output.equation = datamodel::Equation::Scalar("43".to_string());
    }
    add("that model edited", shadow_edited);
    let mut stdlib_own = base.clone();
    stdlib_own
        .models
        .push(crate::stdlib::get("smth1").expect("the stdlib has smth1"));
    add("the stdlib model's own definition", stdlib_own);
    let mut shadow_respelled = base.clone();
    shadow_respelled
        .models
        .push(stand_in("STDLIB\u{205A}SMTH1", "7"));
    add(
        "a model named for a stdlib model, respelled",
        shadow_respelled,
    );
    let mut shadow_twice = shadow.clone();
    let mut again = shadow.models[1].clone();
    again.name = "Stdlib\u{205A}Smth1".to_string();
    shadow_twice.models.push(again);
    add("two models named for one stdlib model", shadow_twice);

    // The project's specs and a model's own; a dimension grown.
    let mut specs = base.clone();
    specs.sim_specs.stop = 5.0;
    specs.models[0].sim_specs = Some(datamodel::SimSpecs {
        stop: 3.0,
        ..specs.sim_specs.clone()
    });
    add("specs", specs);
    let arrayed = TestProject::new("differential")
        .named_dimension("city", &["boston", "la"])
        .aux("rate", "0.5", None)
        .aux("scale", "2", None)
        .array_aux("pop[city]", "scale * 3")
        .aux("total", "SUM(pop[*])", None)
        .build_datamodel();
    add("arrayed", arrayed.clone());
    let mut grown = arrayed;
    grown.dimensions[0] = datamodel::Dimension::named(
        "city".to_string(),
        vec!["boston".to_string(), "la".to_string(), "nyc".to_string()],
    );
    add("arrayed, a dimension grown", grown);

    let mut none = base.clone();
    none.models.clear();
    add("no models", none);

    // Special stocks, whose runs fill the expanded twin's slot.
    let conveyor = open(include_str!(
        "../../../../test/conveyors/minimal_conveyor.xmile"
    ));
    add("a conveyor", conveyor.clone());
    add(
        "a leaky conveyor",
        open(include_str!(
            "../../../../test/conveyors/leaky_conveyor.xmile"
        )),
    );
    add(
        "a queue",
        open(include_str!("../../../../test/queues/minimal_queue.xmile")),
    );
    add(
        "a queue coupled to a conveyor",
        open(include_str!(
            "../../../../test/conveyors/queue_coupled_conveyor.xmile"
        )),
    );
    let first = conveyor.models[0].name.clone();
    add(
        "a conveyor and one more variable",
        patched_in(
            &conveyor,
            &first,
            vec![ModelOperation::UpsertAux(aux("extra thing", "TIME * 2"))],
        ),
    );
    let mut conveyor_twice = conveyor.clone();
    let mut duplicate = conveyor.models[0].clone();
    duplicate.name = format!("{} ", first.to_uppercase());
    conveyor_twice.models.push(duplicate);
    add("a conveyor model twice", conveyor_twice);

    // A macro, and the same model no longer one.
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
    let mut with_macro = patched(
        &base,
        vec![ModelOperation::UpsertAux(aux("y", "MYMACRO(rate, scale)"))],
    );
    with_macro.models.push(macro_model);
    add("a macro", with_macro.clone());
    let mut not_a_macro = with_macro;
    not_a_macro.models[1].macro_spec = None;
    add("the macro no longer one", not_a_macro);

    // A loop, and the loop pinned.
    let looped = TestProject::new("differential")
        .aux("rate", "0.5", None)
        .aux("scale", "2", None)
        .flow("inflow", "level * rate", None)
        .stock("level", "10", &["inflow"], &[], None)
        .build_datamodel();
    add("a loop", looped.clone());
    add(
        "the loop pinned",
        patched(
            &looped,
            vec![ModelOperation::SetLoopName {
                variables: vec!["level".to_string(), "inflow".to_string()],
                name: "growth".to_string(),
                description: None,
            }],
        ),
    );
    pool
}

/// The pool's project named `name`.
fn find(pool: &[(&'static str, datamodel::Project)], name: &str) -> usize {
    pool.iter()
        .position(|(n, _)| *n == name)
        .unwrap_or_else(|| unreachable!("the pool holds {name}"))
}

/// A small linear congruential generator: the sequences only need to be
/// varied and repeatable.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % n
    }
}

/// The first line two descriptions differ at, for a failure to show.
fn first_difference(got: &[String], want: &[String]) -> String {
    got.iter()
        .zip(want)
        .find(|(g, w)| g != w)
        .map(|(g, w)| format!("the db has\n  {g}\na fresh db has\n  {w}"))
        .unwrap_or_else(|| format!("the db has {} lines, a fresh db {}", got.len(), want.len()))
}

/// The db, synced to the pool's project at `current`, holds what a fresh db
/// holds and, when `with_answers`, answers what it answers.
fn check(
    db: &mut SimlinDb,
    pool: &[(&'static str, datamodel::Project)],
    fresh: &[(Vec<String>, Vec<String>)],
    current: usize,
    trail: &[String],
    with_answers: bool,
) {
    let source = db
        .current_source_project()
        .unwrap_or_else(|| unreachable!("the db is synced"));
    let held = inputs(db, source);
    if held != fresh[current].0 {
        panic!(
            "inputs after {trail:#?}:\n{}",
            first_difference(&held, &fresh[current].0)
        );
    }
    if with_answers {
        let answered = answers(db, &pool[current].1);
        if answered != fresh[current].1 {
            panic!(
                "answers after {trail:#?}:\n{}",
                first_difference(&answered, &fresh[current].1)
            );
        }
    }
}

/// One random sequence of `steps` syncs, staged syncs (kept, or rolled back,
/// sometimes asked something first or with a second staging inside) and
/// re-syncs over the pool, checking the db against a fresh one after each,
/// with the answers when `fresh` holds them.
fn run_sequence(
    pool: &[(&'static str, datamodel::Project)],
    fresh: &[(Vec<String>, Vec<String>)],
    seed: u64,
    steps: usize,
    with_answers: bool,
) {
    let mut rng = Lcg(seed);
    let mut db = SimlinDb::default();
    let mut current = rng.below(pool.len());
    db.sync(&pool[current].1);
    let mut trail = vec![format!("seed {seed}: sync {}", pool[current].0)];
    for _ in 0..steps {
        let pick = rng.below(pool.len());
        match rng.below(4) {
            0 => {
                db.sync(&pool[pick].1);
                current = pick;
                trail.push(format!("sync {}", pool[pick].0));
            }
            1 => {
                let (_, before) = db.sync_staged(&pool[pick].1);
                trail.push(format!("stage {}", pool[pick].0));
                // A dry-run patch compiles the staged project before it
                // rolls back.
                if with_answers && rng.below(2) == 0 {
                    answers(&mut db, &pool[pick].1);
                    trail.push("ask the staged project".to_string());
                }
                if rng.below(2) == 0 {
                    let inner = rng.below(pool.len());
                    let (_, before_inner) = db.sync_staged(&pool[inner].1);
                    db.restore(&pool[pick].1, before_inner);
                    trail.push(format!("stage {} and roll it back", pool[inner].0));
                }
                db.restore(&pool[current].1, before);
                trail.push(format!("roll back to {}", pool[current].0));
            }
            2 => {
                db.sync_staged(&pool[pick].1);
                current = pick;
                trail.push(format!("stage {} and keep it", pool[pick].0));
            }
            _ => {
                db.sync(&pool[current].1);
                trail.push(format!("sync {} again", pool[current].0));
            }
        }
        let ask = with_answers && rng.below(2) == 0;
        check(&mut db, pool, fresh, current, &trail, ask);
    }
    check(&mut db, pool, fresh, current, &trail, with_answers);
}

#[test]
fn a_db_holds_what_a_fresh_sync_holds_after_any_sequence_of_syncs() {
    let pool = pool();
    let fresh: Vec<_> = pool.iter().map(|(_, p)| fresh(p, false)).collect();
    for seed in 0..40 {
        run_sequence(&pool, &fresh, seed, 12, false);
    }
}

/// From one project to another, and back, through a sync and through a
/// staged sync, each field the sync writes changes in the db as it does in a
/// fresh one: a variable's kind, a model's own specs, a model's macro spec,
/// a module's references. (The gates compare the answers too.)
#[test]
fn each_field_the_sync_writes_follows_an_edit() {
    let pool = pool();
    let pairs = [
        ("the base", "scale a flow"),
        ("scale a flow", "scale a stock"),
        ("the base", "specs"),
        ("a macro", "the macro no longer one"),
        ("a module", "the module rewired"),
    ];
    let fresh: Vec<_> = pool.iter().map(|(_, p)| fresh(p, false)).collect();
    for (a, b) in pairs {
        for (from, to) in [
            (find(&pool, a), find(&pool, b)),
            (find(&pool, b), find(&pool, a)),
        ] {
            let trail = [
                format!("sync {}", pool[from].0),
                format!("sync {}", pool[to].0),
            ];
            let mut db = SimlinDb::default();
            db.sync(&pool[from].1);
            db.sync(&pool[to].1);
            check(&mut db, &pool, &fresh, to, &trail, false);

            let mut db = SimlinDb::default();
            db.sync(&pool[from].1);
            db.sync_staged(&pool[to].1);
            check(&mut db, &pool, &fresh, to, &trail, false);
        }
    }
}

/// Of several models of one canonical name the db syncs only the last
/// (`filed_models`), so a re-sync of such a project makes no input: salsa
/// never reclaims one, and the earlier models' would be dropped each time.
#[test]
fn a_resync_makes_no_input_for_a_model_no_name_files() {
    let pool = pool();
    // The id the next input gets: one past every input made so far.
    let next_input = |db: &SimlinDb| {
        let probe = SourceVariable::new(
            db,
            String::new(),
            datamodel::Equation::Scalar(String::new()),
            SourceVariableKind::Aux,
            None,
            None,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            String::new(),
            String::new(),
            false,
            false,
            datamodel::Compat::default(),
        );
        salsa::plumbing::AsId::as_id(&probe).index()
    };
    for name in [
        "two models of one name",
        "two models of one name, disjoint",
        "two models of one name, disjoint, reversed",
        "three models of one name",
        "two models named for one stdlib model",
    ] {
        let project = &pool[find(&pool, name)].1;
        let mut db = SimlinDb::default();
        db.sync(project);
        let before = next_input(&db);
        db.sync(project);
        assert_eq!(
            next_input(&db),
            before + 1,
            "{name}: the re-sync made inputs"
        );
    }
}

#[test]
#[ignore = "runs 500 random sync sequences over 47 projects, comparing every answer; run under the gates profile"]
fn a_db_answers_what_a_fresh_sync_answers_after_many_sequences_of_syncs() {
    let pool = pool();
    let fresh: Vec<_> = pool.iter().map(|(_, p)| fresh(p, true)).collect();
    for seed in 0..500 {
        run_sequence(&pool, &fresh, seed, 40, true);
    }
}

/// Every ordered pair of the pool's projects, three ways: one synced after
/// the other; synced back; staged, asked and rolled back.
#[test]
#[ignore = "syncs every ordered pair of 47 projects three ways, comparing every answer; run under the gates profile"]
fn a_db_answers_what_a_fresh_sync_answers_between_any_two_projects() {
    let pool = pool();
    let fresh: Vec<_> = pool.iter().map(|(_, p)| fresh(p, true)).collect();
    for a in 0..pool.len() {
        for b in 0..pool.len() {
            let (name_a, name_b) = (pool[a].0, pool[b].0);
            let mut db = SimlinDb::default();
            db.sync(&pool[a].1);
            answers(&mut db, &pool[a].1);
            db.sync(&pool[b].1);
            check(
                &mut db,
                &pool,
                &fresh,
                b,
                &[format!("{name_a} -> {name_b}")],
                true,
            );
            db.sync(&pool[a].1);
            check(
                &mut db,
                &pool,
                &fresh,
                a,
                &[format!("{name_a} -> {name_b} -> {name_a}")],
                true,
            );
            let (_, before) = db.sync_staged(&pool[b].1);
            answers(&mut db, &pool[b].1);
            db.restore(&pool[a].1, before);
            check(
                &mut db,
                &pool,
                &fresh,
                a,
                &[format!("{name_a}, {name_b} staged and rolled back")],
                true,
            );
        }
    }
}

/// The later of two models of one canonical name is what the inputs hold,
/// whichever of the two the previous state remembers: here the earlier model
/// replaces `scale` and the later holds the very allocation the inputs were
/// last set from. (The project itself is one the engine refuses to compile,
/// `project_duplicate_models`, and a host can still sync it.)
#[test]
fn the_later_of_two_models_of_one_name_is_what_the_inputs_hold() {
    let pool = pool();
    let base = &pool[find(&pool, "the base")].1;
    let same_name = &pool[find(&pool, "two models of one name, reversed")].1;
    let want = fresh(same_name, false).0;

    let mut db = SimlinDb::default();
    db.sync(base);
    let source = db.sync(same_name);
    assert_eq!(inputs(&db, source), want);
    let source = db.sync(same_name);
    assert_eq!(inputs(&db, source), want, "after a re-sync");

    let scale = |db: &SimlinDb| {
        source.models(db)["main"].variables(db)["scale"]
            .equation(db)
            .clone()
    };
    assert_eq!(
        scale(&db),
        datamodel::Equation::Scalar("2".to_string()),
        "the later model's `scale`, not the earlier model's 3"
    );
}

/// A project model named for a stdlib model is the project's own while it is
/// there, and the stdlib model is the stdlib's again once it is gone.
#[test]
fn a_model_named_for_a_stdlib_model_leaves_the_stdlib_model_as_it_was() {
    let run = |db: &mut SimlinDb, project: &datamodel::Project| -> Vec<f64> {
        let source = db
            .current_source_project()
            .unwrap_or_else(|| unreachable!("the db is synced"));
        let mut vm = crate::queue_compile::build_sim(db, source, project, "main", LtmOverlay::Off)
            .expect("the project builds");
        vm.run_to_end().expect("the project runs");
        crate::test_common::collect_results(&vm.into_results())["s"].clone()
    };
    let plain = TestProject::new("shadow")
        .aux("s", "SMTH1(TIME, 2)", None)
        .build_datamodel();
    let mut shadow = plain.clone();
    shadow.models.push(model(
        "stdlib\u{205A}smth1",
        vec![
            aux("output", "42"),
            aux("input", "0"),
            aux("delay_time", "1"),
            aux("initial_value", "0"),
        ],
    ));

    let mut db = SimlinDb::default();
    db.sync(&plain);
    let smoothed = run(&mut db, &plain);
    assert!(smoothed.iter().any(|v| *v != 42.0));

    db.sync(&shadow);
    assert!(
        run(&mut db, &shadow).iter().all(|v| *v == 42.0),
        "the project's own model answers the call while it is there"
    );
    db.sync(&plain);
    assert_eq!(run(&mut db, &plain), smoothed, "the stdlib model is back");
}
