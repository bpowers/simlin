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

use salsa::Setter;

use crate::bytecode::{ModuleDeclaration, Opcode};
use crate::common::{Canonical, Ident, canonicalize};
use crate::compiler::symbolic::VariableLayout;
use crate::datamodel;
use crate::db::{
    ModuleInputSet, SccPhase, SimlinDb, SyncResult, assemble_module, assemble_simulation,
    compute_layout, model_dependency_graph, model_flows_invariant, model_implicit_var_info,
    model_ltm_implicit_var_info, model_ltm_variables, set_project_ltm_enabled, sync_from_datamodel,
};
use crate::testutils::{x_aux, x_flow, x_model, x_module, x_module_named, x_project, x_stock};
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

/// A per-element arrayed aux over one dimension.
fn x_arrayed(ident: &str, dim: &str, elements: &[(&str, &str)]) -> datamodel::Variable {
    datamodel::Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::Arrayed(
            vec![dim.to_string()],
            elements
                .iter()
                .map(|(e, eq)| (e.to_string(), eq.to_string(), None, None))
                .collect(),
            None,
            false,
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

fn key(s: &str) -> Ident<Canonical> {
    Ident::<Canonical>::from_unchecked(s.to_string())
}

fn enumerated_module_rows(
    modules: &crate::db::assemble::ModuleInstanceMap,
) -> Vec<(String, Vec<Vec<String>>)> {
    let mut rows: Vec<_> = modules
        .iter()
        .map(|(model, input_sets)| {
            let sets = input_sets
                .iter()
                .map(|inputs| {
                    inputs
                        .iter()
                        .map(|input| input.as_str().to_string())
                        .collect()
                })
                .collect();
            (model.as_str().to_string(), sets)
        })
        .collect();
    rows.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    rows
}

fn enumeration_fixture() -> datamodel::Project {
    x_project(
        sim_specs(),
        &[
            x_model(
                "main",
                vec![
                    x_aux("source", "3", None),
                    x_stock("level", "1", &["adjustment"], &[], None),
                    // The first and third instances have the same target and
                    // bound-port set. The second has a distinct set. A dst in
                    // another namespace must not become part of this
                    // instance's compilation identity.
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
                    // Ordinary implicit-module discovery.
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
                    // Nested descent from a model reached through three
                    // parent instances.
                    x_module_named("nested", "nested", &[], None),
                ],
            ),
            x_model("nested", vec![x_aux("value", "1", None)]),
        ],
    )
}

/// The two production candidate namespaces feed one `(model, input-set)`
/// identity. The row is derived from source variables and source parsing; no
/// dependency or instance map is built by hand.
#[test]
fn module_instance_enumeration_covers_both_candidate_namespaces_and_identity_rules() {
    let project = enumeration_fixture();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    set_project_ltm_enabled(&mut db, sync.project, true);
    assert!(
        !model_ltm_variables(&db, sync.models["main"].source, sync.project)
            .vars
            .is_empty(),
        "the fixture must exercise enumeration alongside real LTM instrumentation"
    );

    let modules = crate::db::assemble::enumerate_module_instances(&db, sync.project, "main")
        .expect("both module candidate namespaces enumerate");
    let rows = enumerated_module_rows(&modules);

    assert!(
        rows.contains(&(
            "leaf".to_string(),
            vec![vec!["p".to_string()], vec!["q".to_string()]],
        )),
        "same-target instances deduplicate the repeated {{p}} set, retain the distinct {{q}} \
         set, and ignore the other instance's qualified dst: {rows:?}",
    );
    assert!(
        rows.iter()
            .any(|(model, sets)| model == "nested" && sets == &[Vec::<String>::new()]),
        "the leaf target must be descended into exactly far enough to discover its nested model: \
         {rows:?}",
    );
    assert!(
        rows.iter().any(|(model, sets)| {
            model == "stdlib⁚smth1"
                && sets == &[vec!["delay_time".to_string(), "input".to_string()]]
        }),
        "the ordinary implicit SMTH1 module and its production-derived ports must enumerate: \
         {rows:?}",
    );
    let initial_modules = crate::db::assemble::enumerate_initial_dependency_module_instances(
        &db,
        sync.project,
        "main",
    )
    .expect("the ordinary dependency universe enumerates");
    assert_eq!(
        enumerated_module_rows(&initial_modules),
        rows,
        "initial dependency analysis and complete assembly share the two live module namespaces"
    );
}

/// The datamodel can retain a reference whose source and destination are both
/// inside one module instance. Production input construction excludes that internal
/// edge, so enumeration, the emitted declaration and VM module lookup must all
/// use the target model's empty bound-port identity.
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

    let modules = crate::db::assemble::enumerate_module_instances(&db, sync.project, "main")
        .expect("the internal reference must enumerate");
    assert_eq!(
        modules
            .get(&key("leaf"))
            .expect("the target model must be discovered"),
        &[BTreeSet::new()].into_iter().collect(),
        "an own-prefix source is internal and binds no target-model input"
    );

    let sim = assemble_simulation(&db, sync.project, "main".to_string())
        .expect("the production project must compile");
    let root = sim.modules.get(&sim.root).expect("root compiled module");
    let eval_ids: Vec<_> = root
        .compiled_flows
        .code
        .iter()
        .filter_map(|opcode| match opcode {
            Opcode::EvalModule { id, .. } => Some(*id as usize),
            _ => None,
        })
        .collect();
    assert_eq!(eval_ids.len(), 1, "the source module emits one evaluation");
    let declaration = &root.context.modules[eval_ids[0]];
    assert_eq!(declaration.model_name, key("leaf"));
    assert!(
        declaration.input_set.is_empty(),
        "the EvalModule declaration must use the same empty identity as enumeration"
    );

    let mut vm = Vm::new((*sim).clone()).expect("the declaration must resolve its compiled child");
    vm.run_to_end().expect("the internal-reference model runs");
    assert_constant_series(&vm, "bridge\u{00B7}output", 3.0);
}

#[test]
fn explicit_module_missing_target_keeps_its_diagnostic() {
    let project = x_project(
        sim_specs(),
        &[x_model(
            "main",
            vec![x_module_named("gone", "missing", &[], None)],
        )],
    );
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    let error = crate::db::assemble::enumerate_module_instances(&db, sync.project, "main")
        .expect_err("an explicit missing target must be refused");
    assert_eq!(error, "model 'missing' referenced as module but not found");
}

#[test]
fn ordinary_implicit_module_missing_target_keeps_its_diagnostic() {
    let project = x_project(
        sim_specs(),
        &[x_model(
            "main",
            vec![
                x_aux("source", "1", None),
                x_aux("out", "SMTH1(source, 2)", None),
            ],
        )],
    );
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let implicit = model_implicit_var_info(&db, sync.models["main"].source, sync.project);
    let (instance, target) = implicit
        .iter()
        .find_map(|(name, meta)| {
            meta.model_name
                .as_ref()
                .map(|target| (name.clone(), target.clone()))
        })
        .expect("SMTH1 must synthesize one module instance");
    let mut models = sync.project.models(&db).clone();
    models.remove(canonicalize(&target).as_ref());
    sync.project.set_models(&mut db).to(models);

    let error = crate::db::assemble::enumerate_module_instances(&db, sync.project, "main")
        .expect_err("an ordinary implicit missing target must be refused");
    assert_eq!(
        error,
        format!("implicit module '{instance}' references model '{target}' which was not found")
    );
}

/// Explicit candidates retain namespace precedence and choose the same first
/// missing target regardless of source declaration order or fresh HashMap
/// seeds. The ordinary missing module is present to make the cross-namespace
/// precedence observable rather than assumed.
#[test]
fn multiple_missing_explicit_modules_choose_the_canonical_first_diagnostic() {
    let explicit = [
        x_module_named("alpha_instance", "missing_alpha", &[], None),
        x_module_named("zeta_instance", "missing_zeta", &[], None),
    ];

    for reverse in [false, true] {
        for _ in 0..16 {
            let mut vars = vec![
                x_aux("source", "1", None),
                x_aux("ordinary_missing", "SMTH1(source, 2)", None),
            ];
            vars.extend(if reverse {
                explicit.iter().rev().cloned().collect::<Vec<_>>()
            } else {
                explicit.to_vec()
            });
            let project = x_project(sim_specs(), &[x_model("main", vars)]);
            let mut db = SimlinDb::default();
            let sync = sync_from_datamodel(&db, &project);
            let mut models = sync.project.models(&db).clone();
            models.remove("stdlib⁚smth1");
            sync.project.set_models(&mut db).to(models);

            let error = crate::db::assemble::enumerate_module_instances(&db, sync.project, "main")
                .expect_err("the canonical first explicit target must be refused first");
            assert_eq!(
                error, "model 'missing_alpha' referenced as module but not found",
                "explicit candidates precede ordinary candidates and sort by canonical key"
            );
        }
    }
}

/// Ordinary implicit candidates are production parse outputs, not a hand-made
/// metadata map. Reversing their parent declarations and rebuilding with fresh
/// map seeds must not change which exact missing-module diagnostic wins.
#[test]
fn multiple_missing_ordinary_modules_choose_the_canonical_first_diagnostic() {
    let calls = [
        x_aux("alpha_delayed", "DELAY1(source, 2)", None),
        x_aux("zeta_smoothed", "SMTH1(source, 2)", None),
    ];

    for reverse in [false, true] {
        for _ in 0..16 {
            let mut vars = vec![x_aux("source", "1", None)];
            vars.extend(if reverse {
                calls.iter().rev().cloned().collect::<Vec<_>>()
            } else {
                calls.to_vec()
            });
            let project = x_project(sim_specs(), &[x_model("main", vars)]);
            let mut db = SimlinDb::default();
            let sync = sync_from_datamodel(&db, &project);
            let mut models = sync.project.models(&db).clone();
            models.remove("stdlib⁚delay1");
            models.remove("stdlib⁚smth1");
            sync.project.set_models(&mut db).to(models);

            let error = crate::db::assemble::enumerate_module_instances(&db, sync.project, "main")
                .expect_err("the canonical first ordinary target must be refused first");
            assert_eq!(
                error,
                "implicit module '$⁚alpha_delayed⁚0⁚delay1' references model \
                 'stdlib⁚delay1' which was not found",
                "ordinary candidates sort by their canonical production-generated keys"
            );
        }
    }
}

#[test]
fn module_instance_enumeration_terminates_on_a_cycle_and_is_order_independent() {
    let models = [
        x_model(
            "main",
            vec![
                x_module_named("to_a", "a", &[], None),
                x_module_named("also_a", "a", &[], None),
            ],
        ),
        x_model("a", vec![x_module_named("to_b", "b", &[], None)]),
        x_model("b", vec![x_module_named("to_a", "a", &[], None)]),
    ];
    let expected = vec![
        ("a".to_string(), vec![Vec::<String>::new()]),
        ("b".to_string(), vec![Vec::<String>::new()]),
        ("main".to_string(), vec![Vec::<String>::new()]),
    ];

    for project_models in [models.to_vec(), models.into_iter().rev().collect()] {
        let project = x_project(sim_specs(), &project_models);
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &project);
        let modules = crate::db::assemble::enumerate_module_instances(&db, sync.project, "main")
            .expect("the visited-model identity must terminate recursive enumeration");
        assert_eq!(enumerated_module_rows(&modules), expected);
    }
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
fn layout_range(db: &SimlinDb, sync: &SyncResult, key: &str) -> (usize, usize) {
    let project = sync.project;
    let project_models = project.models(db);
    let mut model = sync.models["main"].source;
    let mut layout = compute_layout(db, model, project).root_shifted();
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
        layout = compute_layout(db, model, project).clone();
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
) {
    let root_layout = compute_layout(db, sync.models["main"].source, sync.project).root_shifted();
    assert_eq!(
        sim.n_slots(),
        root_layout.n_slots,
        "the simulation's slot count is the root layout's"
    );
    assert_eq!(sim.get_offset(&key("time")), Some(0));
    assert_eq!(sim.get_offset(&key("dt")), Some(1));
    for (name, off) in &sim.offsets {
        let (start, size) = layout_range(db, sync, name.as_str());
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
    let body = compute_layout(db, model, sync.project);
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
    let sim = assemble_simulation(&db, sync.project, "main".to_string())
        .expect("the module-bearing model assembles");

    assert_offsets_are_the_layouts(&db, &sync, &sim);
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
    let root_layout = compute_layout(&db, sync.models["main"].source, sync.project).root_shifted();
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

/// Generated LTM equations consume the source model's post-module-expansion
/// AST. The source SMTH1 therefore remains an ordinary implicit module while
/// the real LTM score generator may add capture helpers but no second module
/// namespace.
#[test]
fn generated_ltm_helpers_do_not_create_module_instances() {
    let project = ltm_project();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let model = sync.models["main"].source;

    let source_implicit = model_implicit_var_info(&db, model, sync.project);
    assert!(
        source_implicit.values().any(|meta| meta.is_module),
        "the source SMTH1 call must be expanded into the ordinary module registry"
    );

    let ltm_implicit = model_ltm_implicit_var_info(&db, model, sync.project);
    assert!(
        !ltm_implicit.is_empty(),
        "the production flow-to-stock score must synthesize PREVIOUS capture helpers"
    );
    assert!(
        ltm_implicit.values().all(|meta| {
            meta.variable.capture().is_some() && !meta.is_module && meta.model_name.is_none()
        }),
        "generated LTM helpers must all be captures, never modules or hoisted module-call \
         arguments: {}",
        ltm_implicit
            .iter()
            .filter(|(_, meta)| {
                meta.variable.capture().is_none() || meta.is_module || meta.model_name.is_some()
            })
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    let modules = crate::db::assemble::enumerate_module_instances(&db, sync.project, "main")
        .expect("the source module universe enumerates under LTM");
    assert!(
        modules.contains_key(&Ident::<Canonical>::new("stdlib⁚smth1")),
        "qualified LTM reads of the source SMTH1 output resolve through its ordinary instance"
    );
}

/// Under LTM the results-offset map is still the assembled layout, now with
/// the synthetic and implicit LTM sections: every `$⁚ltm⁚…` key and every
/// capture helper sits at its layout slot, and an arrayed LTM score is keyed
/// once, by its bare name, at its base slot -- readers widen it by the
/// variable's own dimensions (`simulate_ltm.rs`'s C-LEARN gate).
#[test]
fn results_offsets_are_the_assembled_layouts_offsets_under_ltm() {
    let project = ltm_project();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let sim = assemble_simulation(&db, sync.project, "main".to_string())
        .expect("the LTM-instrumented model assembles");

    assert_offsets_are_the_layouts(&db, &sync, &sim);
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
    let root_layout = compute_layout(&db, sync.models["main"].source, sync.project).root_shifted();
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
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let model = sync.models["main"].source;
    let project = sync.project;
    let inputs = ModuleInputSet::empty(&db);

    let dep_graph = model_dependency_graph(&db, model, project, inputs);
    assert!(!dep_graph.has_cycle, "the element-acyclic SCC must resolve");
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
    let invariant = model_flows_invariant(&db, model, project, true, inputs);
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
    assert!(
        synthetic_tail
            .iter()
            .chain(implicit_tail.iter())
            .all(|name| !invariant.contains(name)),
        "LTM synthetic and implicit helpers are always outside the root invariant fixpoint"
    );
    assert!(
        sccs.iter()
            .flat_map(|scc| scc.members.iter())
            .all(|member| !invariant.contains(member.as_str())),
        "resolved SCC members are conservatively dynamic"
    );
    let is_ltm =
        |name: &str| synthetic_tail.iter().any(|n| n == name) || ltm_implicit.contains_key(name);

    let module = assemble_module(&db, model, project, true, inputs).expect("assembles");
    let layout = compute_layout(&db, model, project).root_shifted();
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
