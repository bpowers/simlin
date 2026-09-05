// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Assembly pins.
//!
//! Two contracts `assemble_simulation` / `assemble_module` owe their consumers
//! are stated here against production values only -- the layouts the modules
//! are resolved against, the module declarations the assembled bytecode
//! carries, and the runlists the dependency graph schedules:
//!
//! * the results-offset map is the assembled layout, flattened: every key's
//!   slot is the composition of the layouts along its module path, a lookup-
//!   only table reserves its slot but exposes no series, and an arrayed
//!   variable is keyed per element in the VM's row-major storage order;
//! * each of a module's three programs emits its fragments in runlist order,
//!   with a resolved recurrence SCC's members replaced by the SCC's combined
//!   fragment at the first member, the run-invariant flow prefix hoisted, and
//!   the LTM tail (synthetic variables, then implicit helpers) after the
//!   runlist.

use std::collections::{BTreeSet, HashSet};

use crate::bytecode::{ModuleDeclaration, Opcode};
use crate::common::{Canonical, Ident, canonicalize};
use crate::compiler::symbolic::VariableLayout;
use crate::datamodel;
use crate::db::{
    ModuleInputSet, SccPhase, SimlinDb, SourceProject, SyncResult, assemble_module,
    assemble_simulation, compute_layout, model_dependency_graph, model_flows_invariant,
    model_implicit_var_info, model_ltm_implicit_var_info, model_ltm_variables, sync_from_datamodel,
};
use crate::testutils::{
    x_arrayed, x_aux, x_flow, x_model, x_module, x_module_named, x_project, x_stock,
};
use crate::vm::Vm;

fn sim_specs() -> datamodel::SimSpecs {
    datamodel::SimSpecs {
        start: 0.0,
        stop: 4.0,
        dt: datamodel::Dt::Dt(1.0),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    }
}

/// A standalone lookup-only table: an empty equation plus a graphical
/// function. It is a static table, not a saved variable (engine `CLAUDE.md`,
/// "Graphical functions"; GH #606).
fn x_table(ident: &str) -> datamodel::Variable {
    datamodel::Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(String::new()),
        documentation: String::new(),
        units: None,
        gf: Some(datamodel::GraphicalFunction {
            kind: datamodel::GraphicalFunctionKind::Continuous,
            x_points: Some(vec![0.0, 1.0]),
            y_points: vec![0.0, 1.0],
            x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
            y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        }),
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

fn key(s: &str) -> Ident<Canonical> {
    Ident::<Canonical>::from_unchecked(s.to_string())
}

/// `main` instantiates `leaf` through three explicit instances -- the first
/// and third bind the same port set, the second a different one, and the
/// first also names a port of ANOTHER instance's namespace -- and a stdlib
/// model through a `SMTH1` call; `leaf` instantiates `nested`. Every
/// candidate namespace and every identity rule of module-instance
/// enumeration is reached, from source variables and source parses alone.
fn enumeration_fixture() -> datamodel::Project {
    x_project(
        sim_specs(),
        &[
            x_model(
                "main",
                vec![
                    x_aux("source", "3", None),
                    x_stock("level", "1", &["adjustment"], &[], None),
                    x_module_named(
                        "leaf_p",
                        "leaf",
                        &[("source", "leaf_p.p"), ("source", "other.q")],
                        None,
                    ),
                    x_module_named("leaf_q", "leaf", &[("source", "leaf_q.q")], None),
                    x_module_named(
                        "leaf_p_again",
                        "leaf",
                        &[("source", "leaf_p_again.p")],
                        None,
                    ),
                    x_aux("smoothed", "SMTH1(level, 2)", None),
                    x_flow("adjustment", "(source - smoothed) / 2", None),
                ],
            ),
            x_model(
                "leaf",
                vec![
                    x_aux("p", "0", None),
                    x_aux("q", "0", None),
                    x_aux("out", "p + q", None),
                    x_module_named("nested", "nested", &[], None),
                ],
            ),
            x_model("nested", vec![x_aux("value", "1", None)]),
        ],
    )
}

/// The input sets `model` is instantiated with, as strings, sorted.
fn input_sets(db: &SimlinDb, project: SourceProject, model: &str) -> Vec<Vec<String>> {
    let mut sets: Vec<Vec<String>> =
        crate::db::assemble::module_input_sets_for(db, project, "main", model)
            .into_iter()
            .map(|set| set.iter().map(|i| i.as_str().to_string()).collect())
            .collect();
    sets.sort();
    sets
}

/// Both candidate namespaces -- explicit `Module` variables and the instances
/// a parse synthesizes -- feed one `(model, bound-port set)` identity: a
/// repeated port set is one instance, a distinct set another, a `dst` in a
/// foreign namespace binds nothing, and a target model is descended into
/// exactly far enough to find its own instances.
#[test]
fn module_instance_enumeration_covers_both_namespaces_with_one_identity() {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &enumeration_fixture());
    let project = sync.project;

    assert_eq!(
        input_sets(&db, project, "leaf"),
        vec![vec!["p".to_string()], vec!["q".to_string()]],
        "the repeated {{p}} set is one instance, {{q}} another, and `other.q` binds nothing"
    );
    assert_eq!(
        input_sets(&db, project, "nested"),
        vec![Vec::<String>::new()],
        "the nested model is reached through `leaf`"
    );
    assert_eq!(
        input_sets(&db, project, "stdlib\u{205A}smth1"),
        vec![vec!["delay_time".to_string(), "input".to_string()]],
        "the SMTH1 instance's ports are the ones its call wires"
    );
}

/// A reference whose source and destination are both inside one instance
/// binds no port: the instance's compilation identity, its lowered wiring and
/// the module the VM evaluates all agree on the target model's empty port
/// set, so the sub-model's own `input` is read and `output` is `input + 1`.
///
/// A divergence pin: with two owners of the bound-port rule the identity
/// counted the port (`{input}`) while the wiring wrote nothing, and `Vm::new`
/// panicked looking up the compiled child under the identity it was never
/// compiled for (`vm.rs` `key_to_idx`, "no entry found for key"). One rule
/// makes the shape compile; `model_module_wiring_diagnostics` warns that the
/// reference binds nothing (`module_wiring_tests`). XMILE 1.0 section 4.7.1
/// places connections at the lowest common ancestor of the submodel
/// hierarchy, which is where an instance-qualified `from` arises; whether a
/// connect from an instance to that same instance has a defined meaning is
/// unverified.
#[test]
fn internal_module_reference_is_not_a_bound_input() {
    let project = x_project(
        sim_specs(),
        &[
            x_model(
                "main",
                vec![x_module_named(
                    "bridge",
                    "leaf",
                    &[("bridge.output", "bridge.input")],
                    None,
                )],
            ),
            x_model(
                "leaf",
                vec![
                    x_aux("input", "2", None),
                    x_aux("output", "input + 1", None),
                ],
            ),
        ],
    );
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    assert_eq!(
        input_sets(&db, sync.project, "leaf"),
        vec![Vec::<String>::new()],
        "an own-namespace source is internal and binds no port"
    );

    let sim = assemble_simulation(
        &db,
        sync.project,
        "main".to_string(),
        crate::db::LtmOverlay::Off,
    )
    .expect("the internal reference compiles");
    let root = sim.modules.get(&sim.root).expect("root compiled module");
    let declarations: Vec<&ModuleDeclaration> = root
        .compiled_flows
        .code
        .iter()
        .filter_map(|opcode| match opcode {
            Opcode::EvalModule { id, .. } => Some(&root.context.modules[*id as usize]),
            _ => None,
        })
        .collect();
    assert_eq!(declarations.len(), 1, "one instance is evaluated");
    assert_eq!(declarations[0].model_name, key("leaf"));
    assert!(
        declarations[0].input_set.is_empty(),
        "the evaluated module's identity is the enumerated one"
    );

    let mut vm = Vm::new((*sim).clone()).expect("the declaration resolves its compiled child");
    vm.run_to_end().expect("the internal-reference model runs");
    assert_constant_series(&vm, "bridge\u{00B7}output", 3.0);
}

/// The same shape as an XMILE file reads it (`<connect to="bridge.input"
/// from="bridge.output"/>`, the spelling a writer emits), through
/// `open_xmile`: the instance binds no port, the project compiles, and a
/// reader of the instance's output sees the sub-model's own value.
#[test]
fn an_xmile_internal_module_reference_compiles_and_binds_nothing() {
    let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
<header><name>r3</name><vendor>simlin</vendor><product version="1.0">simlin</product></header>
<sim_specs method="Euler"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
<model name="main"><variables>
<module name="bridge" model_name="leaf"><connect to="bridge.input" from="bridge.output"/></module>
<aux name="reader"><eqn>bridge.output</eqn></aux>
</variables></model>
<model name="leaf"><variables>
<aux name="input"><eqn>2</eqn></aux>
<aux name="output"><eqn>input + 1</eqn></aux>
</variables></model>
</xmile>"#;
    let project = crate::compat::open_xmile(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("the fixture is well-formed XMILE");
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    assert_eq!(
        input_sets(&db, sync.project, "leaf"),
        vec![Vec::<String>::new()],
        "the instance-qualified source binds no port"
    );
    let sim = assemble_simulation(
        &db,
        sync.project,
        "main".to_string(),
        crate::db::LtmOverlay::Off,
    )
    .expect("the internal reference compiles");
    let mut vm = Vm::new((*sim).clone()).expect("the child is compiled under the same identity");
    vm.run_to_end().expect("runs");
    assert_constant_series(&vm, "bridge\u{00B7}output", 3.0);
    assert_constant_series(&vm, "reader", 3.0);
}

/// A missing target model is refused with the namespace's own wording, and
/// with several missing the refusal names the first in name order whatever
/// the declaration order -- explicit candidates before implicit ones.
///
/// The implicit rows edit the synced `models` input to drop the stdlib
/// templates: a missing IMPLICIT target has no natural fixture, since every
/// project syncs with the whole stdlib present, so the only way to reach the
/// refusal is to remove the template after the sync.
#[test]
fn a_missing_module_target_is_refused_in_name_order_per_namespace() {
    use salsa::Setter;

    // Explicit namespace: two missing targets, declared in both orders.
    let explicit = [
        x_module_named("alpha_instance", "missing_alpha", &[], None),
        x_module_named("zeta_instance", "missing_zeta", &[], None),
    ];
    for reverse in [false, true] {
        let mut vars = vec![
            x_aux("source", "1", None),
            x_aux("ordinary_missing", "SMTH1(source, 2)", None),
        ];
        let mut modules = explicit.to_vec();
        if reverse {
            modules.reverse();
        }
        vars.extend(modules);
        let project = x_project(sim_specs(), &[x_model("main", vars)]);
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);
        let mut models = sync.project.models(&db).clone();
        models.remove("stdlib\u{205A}smth1");
        sync.project.set_models(&mut db).to(models);

        let error = assemble_simulation(
            &db,
            sync.project,
            "main".to_string(),
            crate::db::LtmOverlay::Off,
        )
        .expect_err("a missing explicit target is refused");
        assert_eq!(
            error, "model 'missing_alpha' referenced as module but not found",
            "reverse={reverse}: explicit candidates first, in name order"
        );
    }

    // Implicit namespace: two stdlib calls whose models are gone.
    let calls = [
        x_aux("alpha_delayed", "DELAY1(source, 2)", None),
        x_aux("zeta_smoothed", "SMTH1(source, 2)", None),
    ];
    for reverse in [false, true] {
        let mut vars = vec![x_aux("source", "1", None)];
        let mut helpers = calls.to_vec();
        if reverse {
            helpers.reverse();
        }
        vars.extend(helpers);
        let project = x_project(sim_specs(), &[x_model("main", vars)]);
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);
        let mut models = sync.project.models(&db).clone();
        models.remove("stdlib\u{205A}delay1");
        models.remove("stdlib\u{205A}smth1");
        sync.project.set_models(&mut db).to(models);

        let error = assemble_simulation(
            &db,
            sync.project,
            "main".to_string(),
            crate::db::LtmOverlay::Off,
        )
        .expect_err("a missing implicit target is refused");
        assert_eq!(
            error,
            "implicit module '$\u{205A}alpha_delayed\u{205A}0\u{205A}delay1' references model \
             'stdlib\u{205A}delay1' which was not found",
            "reverse={reverse}: implicit candidates in name order"
        );
    }
}

/// Generated LTM equations are built from a target's expanded tree, so their
/// parses synthesize captures and never a module instance: the source
/// `SMTH1` stays the ordinary implicit instance, every LTM helper is a
/// capture, and the module universe under LTM is the source models' own.
#[test]
fn generated_ltm_helpers_are_captures_only() {
    let project = ltm_project();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;

    assert!(
        model_implicit_var_info(&db, model, sync.project)
            .values()
            .any(|meta| meta.is_module),
        "the source SMTH1 call is an ordinary implicit instance"
    );
    let ltm_implicit = model_ltm_implicit_var_info(&db, model, sync.project);
    assert!(
        !ltm_implicit.is_empty(),
        "the flow-to-stock score synthesizes PREVIOUS capture helpers"
    );
    assert!(
        ltm_implicit
            .values()
            .all(|meta| meta.variable.capture().is_some()),
        "every generated helper is a capture: {:?}",
        ltm_implicit.keys().collect::<Vec<_>>()
    );
    assert_eq!(
        input_sets(&db, sync.project, "stdlib\u{205A}smth1"),
        vec![vec!["delay_time".to_string(), "input".to_string()]],
        "the LTM reads of the SMTH1 output resolve through the ordinary instance"
    );
}

/// The model a module variable instantiates: an explicit `Module` variable's
/// target, or an implicit (stdlib) module's.
fn module_target(
    db: &SimlinDb,
    model: crate::db::SourceModel,
    project: crate::db::SourceProject,
    var: &str,
) -> String {
    if let Some(svar) = model.variables(db).get(var) {
        assert_eq!(
            svar.kind(db),
            crate::db::SourceVariableKind::Module,
            "`{var}` is used as a module path segment but is not a module variable"
        );
        return svar.model_name(db).clone();
    }
    model_implicit_var_info(db, model, project)
        .get(var)
        .and_then(|meta| meta.model_name.clone())
        .unwrap_or_else(|| panic!("`{var}` is neither an explicit nor an implicit module"))
}

/// The slot range the assembled layouts give an offsets-map key: the
/// root-shifted body layout for a top-level name, composed through each module
/// variable's slot and its sub-model's body layout along a `module·sub` path.
/// A per-element key `name[e]` resolves to its variable's whole range. This is
/// the layout `resolve_module` addresses each module against, so it is what the
/// offsets map owes its consumers.
fn layout_range(
    db: &SimlinDb,
    sync: &SyncResult,
    key: &str,
    overlay: crate::db::LtmOverlay,
) -> (usize, usize) {
    let project = sync.project;
    let project_models = project.models(db);
    let mut model = sync.models["main"].source;
    let mut layout = compute_layout(db, model, project, overlay).root_shifted();
    let mut base = 0usize;
    let mut segments: Vec<&str> = key.split('\u{00B7}').collect();
    let leaf = segments.pop().expect("a key has at least one segment");
    for seg in segments {
        let entry = layout
            .get(seg)
            .unwrap_or_else(|| panic!("module segment `{seg}` of `{key}` is not in its layout"));
        base += entry.offset;
        let target = module_target(db, model, project, seg);
        model = *project_models
            .get(canonicalize(&target).as_ref())
            .unwrap_or_else(|| panic!("module `{seg}` targets the unknown model `{target}`"));
        layout = compute_layout(db, model, project, overlay).clone();
    }
    let name = leaf.split('[').next().expect("a leaf has a name");
    let entry = layout
        .get(name)
        .unwrap_or_else(|| panic!("`{name}` of `{key}` is not in its model's layout"));
    (base + entry.offset, entry.size)
}

/// Every key of `sim.offsets` sits in the range the layouts compose to, and a
/// key without an element subscript sits exactly at its variable's base.
fn assert_offsets_are_the_layouts(
    db: &SimlinDb,
    sync: &SyncResult,
    sim: &crate::vm::CompiledSimulation,
    overlay: crate::db::LtmOverlay,
) {
    let root_layout =
        compute_layout(db, sync.models["main"].source, sync.project, overlay).root_shifted();
    assert_eq!(
        sim.n_slots(),
        root_layout.n_slots,
        "the simulation's slot count is the root layout's"
    );
    assert_eq!(sim.get_offset(&key("time")), Some(0));
    assert_eq!(sim.get_offset(&key("dt")), Some(1));
    for (name, off) in &sim.offsets {
        let (start, size) = layout_range(db, sync, name.as_str(), overlay);
        if name.as_str().contains('[') {
            assert!(
                start <= *off && *off < start + size,
                "per-element key `{name}` at slot {off} lies outside its variable's \
                 layout range [{start}, {})",
                start + size
            );
        } else {
            assert_eq!(
                *off, start,
                "`{name}` is keyed at slot {off} but the layouts place it at {start}"
            );
        }
    }
}

/// Each module variable's layout slot is the `off` of a module declaration
/// in the assembled module -- the layouts the offsets map composes are the
/// ones the bytecode relocates sub-model instances by.
fn assert_module_decls_sit_at_layout_slots(
    db: &SimlinDb,
    sync: &SyncResult,
    sim: &crate::vm::CompiledSimulation,
    model_name: &str,
    is_root: bool,
) {
    let model = sync.models[model_name].source;
    let body = compute_layout(db, model, sync.project, crate::db::LtmOverlay::Off);
    let layout = if is_root {
        body.root_shifted()
    } else {
        body.clone()
    };
    let compiled = &sim.modules[&(Ident::<Canonical>::new(model_name), BTreeSet::new())];
    let source_vars = model.variables(db);
    let implicit = model_implicit_var_info(db, model, sync.project);
    let mut module_vars = 0usize;
    for (name, entry) in layout.iter() {
        let is_module = source_vars
            .get(name)
            .is_some_and(|v| v.kind(db) == crate::db::SourceVariableKind::Module)
            || implicit.get(name).is_some_and(|m| m.is_module);
        if !is_module {
            continue;
        }
        module_vars += 1;
        assert!(
            compiled
                .context
                .modules
                .iter()
                .any(|d| d.off == entry.offset),
            "module variable `{name}` of `{model_name}` sits at layout slot {} but no module \
             declaration is relocated there: {:?}",
            entry.offset,
            compiled
                .context
                .modules
                .iter()
                .map(|d| d.off)
                .collect::<Vec<_>>()
        );
    }
    assert!(
        module_vars > 0,
        "`{model_name}` must hold a module variable"
    );
}

fn series(vm: &Vm, name: &str) -> Vec<f64> {
    vm.get_series(&key(name))
        .unwrap_or_else(|| panic!("`{name}` must be a saved series"))
}

fn assert_constant_series(vm: &Vm, name: &str, want: f64) {
    let got = series(vm, name);
    assert!(
        !got.is_empty() && got.iter().all(|v| *v == want),
        "`{name}` must read {want} at every step through its results offset; got {got:?}"
    );
}

/// `main` holds a scalar, a stdlib module (SMTH3), an explicit module whose
/// sub-model carries a lookup-only table, a per-element arrayed aux and a
/// nested module, a root-level lookup-only table, and two variables sorted
/// after the module.
fn module_bearing_project() -> datamodel::Project {
    let mut project = x_project(
        sim_specs(),
        &[
            x_model(
                "main",
                vec![
                    x_aux("aaa", "time * 2", None),
                    x_aux("smoothed", "SMTH3(aaa, 5)", None),
                    x_module("sub", &[], None),
                    x_table("t_root"),
                    x_aux("trailing", "42", None),
                    x_aux("zzz", "aaa + 1", None),
                ],
            ),
            x_model(
                "sub",
                vec![
                    x_table("a_tbl"),
                    x_arrayed("arr", "d", &[("d1", "1"), ("d2", "2")]),
                    x_module("inner", &[], None),
                    x_aux("out", "7", None),
                ],
            ),
            x_model("inner", vec![x_aux("k", "3", None)]),
        ],
    );
    project.dimensions = vec![datamodel::Dimension::named(
        "d".to_string(),
        vec!["d1".to_string(), "d2".to_string()],
    )];
    project
}

/// The results-offset map on a module-bearing model is the assembled layout:
/// every key -- scalar, `module·sub`, nested `module·inner·var`, per-element
/// `module·arr[e]`, stdlib-module sub-variable -- sits where the layouts
/// compose to, a lookup-only table exposes no key while keeping its slot, and
/// the VM reads each named series from the slot its equation writes.
///
/// A sub-model's lookup-only table is the load-bearing shape: its slot is
/// reserved in the sub-model's layout, so every parent variable laid out after
/// the module instance must be keyed PAST it. A flatten that advanced the parent
/// by the sub-model's exposed series rather than its slot count keyed
/// `trailing` and `zzz` one slot early: `zzz` read `trailing`'s 42 and
/// `trailing` read the root table's reserved, never-written slot.
#[test]
fn results_offsets_are_the_assembled_layouts_offsets_on_a_module_bearing_model() {
    let project = module_bearing_project();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let sim = assemble_simulation(
        &db,
        sync.project,
        "main".to_string(),
        crate::db::LtmOverlay::Off,
    )
    .expect("the module-bearing model assembles");

    assert_offsets_are_the_layouts(&db, &sync, &sim, crate::db::LtmOverlay::Off);
    assert_module_decls_sit_at_layout_slots(&db, &sync, &sim, "main", true);
    assert_module_decls_sit_at_layout_slots(&db, &sync, &sim, "sub", false);

    let has = |k: &str| sim.offsets.contains_key(&key(k));
    for present in [
        "aaa",
        "smoothed",
        "trailing",
        "zzz",
        "sub\u{00B7}out",
        "sub\u{00B7}inner\u{00B7}k",
        "sub\u{00B7}arr[d1]",
        "sub\u{00B7}arr[d2]",
    ] {
        assert!(has(present), "`{present}` must be a results key");
    }
    for absent in [
        "t_root",
        "sub\u{00B7}a_tbl",
        "sub",
        "sub\u{00B7}arr",
        "sub\u{00B7}inner",
    ] {
        assert!(
            !has(absent),
            "`{absent}` must not be a results key (a table has no series; a module and an \
             arrayed variable are keyed by their parts)"
        );
    }
    assert!(
        sim.offsets.keys().any(|k| {
            k.as_str().starts_with("$\u{205A}smoothed\u{205A}") && k.as_str().contains('\u{00B7}')
        }),
        "the SMTH3 instance's sub-variables are flattened like an explicit module's: {:?}",
        sim.offsets.keys().map(|k| k.as_str()).collect::<Vec<_>>()
    );
    // Elements are keyed in declaration order, which is the VM's row-major
    // storage order (the conveyor and queue plans index belts by it).
    assert_eq!(
        sim.get_offset(&key("sub\u{00B7}arr[d2]")),
        sim.get_offset(&key("sub\u{00B7}arr[d1]")).map(|o| o + 1)
    );
    let root_layout = compute_layout(
        &db,
        sync.models["main"].source,
        sync.project,
        crate::db::LtmOverlay::Off,
    )
    .root_shifted();
    assert_eq!(
        sim.get_offset(&key("trailing")),
        Some(root_layout.get("trailing").expect("trailing").offset),
        "`trailing` is laid out after `sub`, whose lookup-only table reserves a slot"
    );

    let mut vm = Vm::new((*sim).clone()).expect("vm");
    vm.run_to_end().expect("runs");
    assert_constant_series(&vm, "trailing", 42.0);
    assert_constant_series(&vm, "sub\u{00B7}out", 7.0);
    assert_constant_series(&vm, "sub\u{00B7}inner\u{00B7}k", 3.0);
    assert_constant_series(&vm, "sub\u{00B7}arr[d1]", 1.0);
    assert_constant_series(&vm, "sub\u{00B7}arr[d2]", 2.0);
}

/// A goal-seeking loop through a stdlib SMTH1 instance plus an arrayed
/// growth loop: under LTM the layout grows a synthetic-variable section
/// (scalar and arrayed link scores, a loop score) and an LTM implicit section
/// (the flow-to-stock score's nested `PREVIOUS` capture helpers).
fn ltm_project() -> datamodel::Project {
    let mut project = x_project(
        sim_specs(),
        &[x_model(
            "main",
            vec![
                x_aux("goal", "100", None),
                x_stock("level", "50", &["adjustment"], &[], None),
                x_aux("smoothed_level", "SMTH1(level, 3)", None),
                x_aux("gap", "goal - smoothed_level", None),
                x_flow("adjustment", "gap / 5", None),
                datamodel::Variable::Stock(datamodel::Stock {
                    ident: "pop".to_string(),
                    equation: datamodel::Equation::ApplyToAll(
                        vec!["d".to_string()],
                        "10".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["grow".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                datamodel::Variable::Flow(datamodel::Flow {
                    ident: "grow".to_string(),
                    equation: datamodel::Equation::ApplyToAll(
                        vec!["d".to_string()],
                        "pop[d] * 0.01".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
                x_aux("z_after", "gap * 2", None),
            ],
        )],
    );
    project.dimensions = vec![datamodel::Dimension::named(
        "d".to_string(),
        vec!["d1".to_string(), "d2".to_string()],
    )];
    project
}

/// Under LTM the results-offset map is still the assembled layout, now with
/// the synthetic and implicit LTM sections: every `$⁚ltm⁚…` key and every
/// capture helper sits at its layout slot, and an arrayed LTM score is keyed
/// once, by its bare name, at its base slot -- readers widen it by the
/// variable's own dimensions (`simulate_ltm.rs`'s C-LEARN gate).
#[test]
fn results_offsets_are_the_assembled_layouts_offsets_under_ltm() {
    let project = ltm_project();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let sim = assemble_simulation(
        &db,
        sync.project,
        "main".to_string(),
        crate::db::LtmOverlay::On,
    )
    .expect("the LTM-instrumented model assembles");

    assert_offsets_are_the_layouts(&db, &sync, &sim, crate::db::LtmOverlay::On);
    assert_module_decls_sit_at_layout_slots(&db, &sync, &sim, "main", true);

    let keys: Vec<&str> = sim.offsets.keys().map(|k| k.as_str()).collect();
    let any_with = |needle: &str| keys.iter().any(|k| k.contains(needle));
    assert!(
        any_with("$\u{205A}ltm\u{205A}link_score\u{205A}"),
        "link scores are saved series: {keys:?}"
    );
    assert!(
        any_with("$\u{205A}ltm\u{205A}loop_score\u{205A}"),
        "loop scores are saved series: {keys:?}"
    );
    assert!(
        any_with("arg0"),
        "the flow-to-stock score's nested PREVIOUS capture helpers are saved series: {keys:?}"
    );

    // The arrayed `grow -> pop` link score occupies two slots and is keyed once.
    let root_layout = compute_layout(
        &db,
        sync.models["main"].source,
        sync.project,
        crate::db::LtmOverlay::On,
    )
    .root_shifted();
    let arrayed = "$\u{205A}ltm\u{205A}link_score\u{205A}grow\u{2192}pop";
    let entry = root_layout
        .get(arrayed)
        .unwrap_or_else(|| panic!("`{arrayed}` must be an LTM variable of this model"));
    assert_eq!(
        entry.size, 2,
        "an A2A link score over `d` spans its two elements"
    );
    assert_eq!(sim.get_offset(&key(arrayed)), Some(entry.offset));
    assert!(
        !keys
            .iter()
            .any(|k| k.starts_with(arrayed) && k.contains('[')),
        "an arrayed LTM score is not keyed per element: {keys:?}"
    );

    let mut vm = Vm::new((*sim).clone()).expect("vm");
    vm.run_to_end().expect("runs");
    assert_constant_series(&vm, "goal", 100.0);
}

/// `layout`'s owner of every slot, so an emitted write or module evaluation
/// can be attributed to the variable whose fragment produced it.
fn slot_owners(layout: &VariableLayout) -> Vec<Option<String>> {
    let mut owners = vec![None; layout.n_slots];
    for (name, entry) in layout.iter() {
        for owner in &mut owners[entry.offset..entry.offset + entry.size] {
            *owner = Some(name.to_string());
        }
    }
    owners
}

/// The variables a program's opcodes emit for, in order, consecutive repeats
/// collapsed: a write names the slot's owner, a module evaluation names the
/// module variable its declaration relocates to.
fn emitted_owners(
    code: &[Opcode],
    decls: &[ModuleDeclaration],
    owners: &[Option<String>],
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for op in code {
        let slot = match op {
            Opcode::AssignCurr { off }
            | Opcode::AssignConstCurr { off, .. }
            | Opcode::BinOpAssignCurr { off, .. }
            | Opcode::BinOpAssignNext { off, .. } => *off as usize,
            Opcode::EvalModule { id, .. } => decls[*id as usize].off,
            _ => continue,
        };
        let name = owners[slot]
            .clone()
            .unwrap_or_else(|| panic!("slot {slot} is written but no layout entry owns it"));
        if out.last() != Some(&name) {
            out.push(name);
        }
    }
    out
}

fn dedup_consecutive(names: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for n in names {
        if out.last() != Some(&n) {
            out.push(n);
        }
    }
    out
}

/// A resolved recurrence SCC (`ref.mdl`-shaped `ce`/`ecc`, whose element graph
/// is acyclic), a stock initialized from it so the SCC's members are scheduled
/// in the initials too, a goal-seeking loop through a stdlib SMTH1 instance,
/// and LTM enabled.
fn scc_stdlib_ltm_project() -> datamodel::Project {
    let mut project = x_project(
        sim_specs(),
        &[x_model(
            "main",
            vec![
                x_arrayed(
                    "ce",
                    "t",
                    &[("t1", "1"), ("t2", "ecc[t1] + 1"), ("t3", "ecc[t2] + 1")],
                ),
                x_arrayed(
                    "ecc",
                    "t",
                    &[
                        ("t1", "ce[t1] + 1"),
                        ("t2", "ce[t2] + 1"),
                        ("t3", "ce[t3] + 1"),
                    ],
                ),
                x_stock("acc", "ecc[t3] * 10", &["inflow"], &[], None),
                x_flow("inflow", "gap / 5", None),
                x_aux("goal", "100", None),
                x_aux("smoothed", "SMTH1(acc, 3)", None),
                x_aux("gap", "goal - smoothed", None),
            ],
        )],
    );
    project.dimensions = vec![datamodel::Dimension::named(
        "t".to_string(),
        vec!["t1".to_string(), "t2".to_string(), "t3".to_string()],
    )];
    project
}

/// Each program of an assembled module emits its fragments in runlist order
/// with the LTM tail after it, whatever source a fragment came from.
///
/// The expected order is derived from the production schedule, not read off
/// the output: the dependency graph's runlists, its resolved SCCs (whose
/// `element_order` replaces the members at the first member's position -- the
/// initials under the synthetic `$⁚scc⁚init⁚{n}` ident), the run-invariance
/// classification (the invariant flows are hoisted ahead of the dynamic ones,
/// each group keeping its order), the LTM synthetic variables in generation
/// order, and the LTM implicit helpers in name order. The only thing taken
/// from the output is WHICH LTM fragments exist (a synthetic variable whose
/// fragment did not compile is dropped from the tail, and an implicit helper
/// appears only in the phases it has a program for); a runlist member must
/// always be present, since a missing one fails assembly.
///
/// Arms this fixture does not reach, and where they are pinned: a module
/// input's copy fragment is emitted in the initials and flows and skipped in
/// the stocks (every module-bearing corpus model: a stocks-phase copy would
/// overwrite the integrated value and the golden outputs would disagree);
/// an `Initial`-phase SCC's members take their own flow fragments (the
/// `two_stock_init_recurrence` fixtures in `combined_fragment_tests`).
#[test]
fn each_program_emits_in_runlist_order_then_the_ltm_tail() {
    let project = scc_stdlib_ltm_project();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let model = sync.models["main"].source;
    let project = sync.project;
    let inputs = ModuleInputSet::empty(&db);

    let dep_graph = model_dependency_graph(&db, model, project, inputs);
    assert!(
        !dep_graph.has_cycle(),
        "the element-acyclic SCC must resolve"
    );
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "exactly the {{ce, ecc}} SCC: {:?}",
        dep_graph.resolved_sccs
    );
    let sccs = &dep_graph.resolved_sccs;
    let scc_of = |name: &str| -> Option<usize> {
        sccs.iter()
            .position(|scc| scc.members.contains(&Ident::<Canonical>::new(name)))
    };
    let invariant =
        model_flows_invariant(&db, model, project, true, inputs, crate::db::LtmOverlay::On);
    let ltm_vars = model_ltm_variables(&db, model, project);
    let ltm_implicit = model_ltm_implicit_var_info(&db, model, project);
    assert!(
        !ltm_vars.vars.is_empty(),
        "the goal-seeking loop yields LTM variables"
    );
    assert!(
        !ltm_implicit.is_empty(),
        "the flow-to-stock score yields nested PREVIOUS capture helpers"
    );
    let synthetic_tail: Vec<String> = ltm_vars
        .vars
        .iter()
        .map(|v| canonicalize(&v.name).into_owned())
        .collect();
    let mut implicit_tail: Vec<String> = ltm_implicit.keys().cloned().collect();
    implicit_tail.sort();
    let is_ltm =
        |name: &str| synthetic_tail.iter().any(|n| n == name) || ltm_implicit.contains_key(name);

    let module = assemble_module(&db, model, project, true, inputs, crate::db::LtmOverlay::On)
        .expect("assembles");
    let layout = compute_layout(&db, model, project, crate::db::LtmOverlay::On).root_shifted();
    let owners = slot_owners(&layout);
    let decls: &[ModuleDeclaration] = &module.context.modules;

    // ── initials ──
    let observed: Vec<String> = module
        .compiled_initials
        .iter()
        .map(|init| init.ident.as_str().to_string())
        .collect();
    let observed_set: HashSet<&String> = observed.iter().collect();
    let mut expected: Vec<String> = Vec::new();
    let mut injected: HashSet<usize> = HashSet::new();
    for name in &dep_graph.runlist_initials {
        if let Some(i) = scc_of(name) {
            if injected.insert(i) {
                expected.push(format!("$\u{205A}scc\u{205A}init\u{205A}{i}"));
            }
            continue;
        }
        expected.push(name.clone());
    }
    assert!(
        injected.len() == 1,
        "the SCC's members are scheduled in the initials (the stock is initialized from `ecc`)"
    );
    expected.extend(
        implicit_tail
            .iter()
            .filter(|n| observed_set.contains(n))
            .cloned(),
    );
    assert_eq!(
        observed, expected,
        "initials: runlist order with the SCC combined once, then the LTM implicit tail"
    );

    // ── flows ──
    let observed = emitted_owners(&module.compiled_flows.code, decls, &owners);
    let observed_set: HashSet<&String> = observed.iter().collect();
    let mut scheduled: Vec<(String, bool)> = Vec::new();
    let mut injected: HashSet<usize> = HashSet::new();
    for name in &dep_graph.runlist_flows {
        if let Some(i) = scc_of(name)
            && sccs[i].phase == SccPhase::Dt
        {
            if injected.insert(i) {
                for (member, _) in &sccs[i].element_order {
                    scheduled.push((member.as_str().to_string(), false));
                }
            }
            continue;
        }
        scheduled.push((name.clone(), invariant.contains(name)));
    }
    assert_eq!(
        injected.len(),
        1,
        "the dt-phase SCC is injected into the flows"
    );
    assert!(
        scheduled.iter().any(|(_, inv)| *inv) && scheduled.iter().any(|(_, inv)| !*inv),
        "both halves of the invariant/dynamic partition are populated: {scheduled:?}"
    );
    let mut expected: Vec<String> = scheduled
        .iter()
        .filter(|(_, inv)| *inv)
        .chain(scheduled.iter().filter(|(_, inv)| !*inv))
        .map(|(name, _)| name.clone())
        .collect();
    expected.extend(
        synthetic_tail
            .iter()
            .filter(|n| observed_set.contains(n))
            .cloned(),
    );
    expected.extend(
        implicit_tail
            .iter()
            .filter(|n| observed_set.contains(n))
            .cloned(),
    );
    let expected = dedup_consecutive(expected);
    assert!(
        expected.iter().filter(|n| is_ltm(n)).count() >= 2,
        "the LTM tail must be populated: {expected:?}"
    );
    assert_eq!(
        observed, expected,
        "flows: invariant prefix, then the dynamic runlist with the SCC interleave at its first \
         member, then LTM synthetic variables, then LTM implicit helpers"
    );
    let prefix_len = module.flows_invariant_opcode_len;
    assert!(
        prefix_len > 0,
        "the constant flows form a run-invariant prefix"
    );
    let prefix_owners = emitted_owners(&module.compiled_flows.code[..prefix_len], decls, &owners);
    assert!(
        prefix_owners.iter().all(|n| invariant.contains(n)),
        "every fragment inside the invariant prefix is run-invariant: {prefix_owners:?}"
    );

    // ── stocks ──
    let observed = emitted_owners(&module.compiled_stocks.code, decls, &owners);
    let observed_set: HashSet<&String> = observed.iter().collect();
    let mut expected: Vec<String> = dep_graph.runlist_stocks.clone();
    expected.extend(
        implicit_tail
            .iter()
            .filter(|n| observed_set.contains(n))
            .cloned(),
    );
    assert_eq!(
        observed, expected,
        "stocks: runlist order, then the LTM implicit tail"
    );
}
