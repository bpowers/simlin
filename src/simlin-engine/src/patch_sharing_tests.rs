// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Two properties of a patch over a project whose variables and view
//! elements are shared with a copy (`SharedVec`), each checked for every kind
//! of operation generated from a model:
//!
//! - **It copies only what it changes.** Every variable and view element the
//!   patch leaves equal to the original's is the original's allocation.
//! - **It lands the same shared or not.** Applied to a project every element
//!   of which a copy shares, it gives exactly the project it gives applied to
//!   one nothing shares.
//!
//! The default suite checks both on two models; the gates check them on every
//! corpus model. The first property is also held of the conveyor and queue
//! expansions, which rewrite a copy of the project.

use std::collections::HashMap;
use std::hash::Hash;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use crate::canonicalize;
use crate::datamodel::{self, Equation, Project, SharedVec, Variable, View, ViewElement};
use crate::patch::{
    ModelOperation, ModelPatch, ProjectOperation, ProjectPatch, apply_patch, apply_patch_in_place,
};

fn world3() -> Project {
    crate::compat::open_vensim(include_str!("../../../test/metasd/WRLD3-03/wrld3-03.mdl"))
        .expect("world3 opens")
}

/// A project of three models, two of them instantiated as modules.
fn hares_and_lynxes() -> Project {
    let xmile = include_str!(
        "../../../test/test-models/samples/bpowers-hares_and_lynxes_modules/model.xmile"
    );
    crate::compat::open_xmile(&mut BufReader::new(xmile.as_bytes())).expect("it opens")
}

fn corpus_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            corpus_files(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("mdl" | "stmx" | "xmile" | "itmx")
        ) {
            out.push(path);
        }
    }
}

/// Calls `f` with every model file under the repository's `test/` that
/// opens, in path order.
pub(crate) fn for_each_corpus_project(mut f: impl FnMut(&Path, &Project)) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test");
    let mut files = Vec::new();
    corpus_files(&root, &mut files);
    for path in files {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let project = if path.extension().and_then(|e| e.to_str()) == Some("mdl") {
            String::from_utf8(bytes)
                .ok()
                .and_then(|text| crate::compat::open_vensim(&text).ok())
        } else {
            crate::compat::open_xmile(&mut BufReader::new(bytes.as_slice())).ok()
        };
        if let Some(project) = project {
            f(&path, &project);
        }
    }
}

/// How many variants `ModelOperation` has.
pub(crate) const MODEL_OPERATION_VARIANTS: usize = 11;

/// The position of `op`'s variant. The match has no wildcard, so a variant
/// added to `ModelOperation` fails to compile here until it has a number, and
/// a test that requires every number covered then fails until the operation
/// is generated.
pub(crate) fn model_operation_variant(op: &ModelOperation) -> usize {
    match op {
        ModelOperation::UpsertStock(_) => 0,
        ModelOperation::UpsertFlow(_) => 1,
        ModelOperation::UpsertAux(_) => 2,
        ModelOperation::UpsertModule(_) => 3,
        ModelOperation::DeleteVariable { .. } => 4,
        ModelOperation::RenameVariable { .. } => 5,
        ModelOperation::UpsertView { .. } => 6,
        ModelOperation::DeleteView { .. } => 7,
        ModelOperation::UpdateStockFlows { .. } => 8,
        ModelOperation::SetLoopName { .. } => 9,
        ModelOperation::EditView { .. } => 10,
    }
}

const PROJECT_OPERATION_VARIANTS: usize = 3;

/// `model_operation_variant`, for `ProjectOperation`.
fn project_operation_variant(op: &ProjectOperation) -> usize {
    match op {
        ProjectOperation::SetSimSpecs(_) => 0,
        ProjectOperation::SetSource(_) => 1,
        ProjectOperation::AddModel { .. } => 2,
    }
}

fn tweaked(equation: &Equation) -> Equation {
    let mut equation = equation.clone();
    match &mut equation {
        Equation::Scalar(text) | Equation::ApplyToAll(_, text) => text.push_str(" + 0"),
        Equation::Arrayed(_, elements, _, _) => {
            if let Some((_, text, _, _)) = elements.first_mut() {
                text.push_str(" + 0");
            }
        }
    }
    equation
}

/// An upsert of `variable`: as it is, or with its equation (a module's
/// documentation) changed.
fn upsert_of(variable: &Variable, changed: bool) -> ModelOperation {
    match variable.clone() {
        Variable::Stock(mut s) => {
            if changed {
                s.equation = tweaked(&s.equation);
            }
            ModelOperation::UpsertStock(s)
        }
        Variable::Flow(mut f) => {
            if changed {
                f.equation = tweaked(&f.equation);
            }
            ModelOperation::UpsertFlow(f)
        }
        Variable::Aux(mut a) => {
            if changed {
                a.equation = tweaked(&a.equation);
            }
            ModelOperation::UpsertAux(a)
        }
        Variable::Module(mut m) => {
            if changed {
                m.documentation.push_str(" edited");
            }
            ModelOperation::UpsertModule(m)
        }
    }
}

fn moved(element: &ViewElement) -> Option<ViewElement> {
    let mut next = element.clone();
    match &mut next {
        ViewElement::Aux(e) => e.x += 3.0,
        ViewElement::Stock(e) => e.x += 3.0,
        ViewElement::Module(e) => e.x += 3.0,
        ViewElement::Alias(e) => e.x += 3.0,
        ViewElement::Cloud(e) => e.x += 3.0,
        ViewElement::Group(e) => e.x += 3.0,
        ViewElement::Flow(_) | ViewElement::Link(_) => return None,
    }
    Some(next)
}

/// At most `max` of `n` positions, evenly spaced.
fn sample(n: usize, max: usize) -> Vec<usize> {
    if n <= max {
        return (0..n).collect();
    }
    (0..max).map(|i| i * n / max).collect()
}

/// The patches the properties are checked on: for each model of `project`,
/// each operation over up to `max` of its variables and view elements, in the
/// shapes that matter to sharing (an edit, an edit that changes nothing, a
/// rename that only changes case, several operations in one patch), then each
/// project operation.
fn generated_patches(project: &Project, max: usize) -> Vec<(String, ProjectPatch)> {
    let mut out: Vec<(String, ProjectPatch)> = Vec::new();
    for model in &project.models {
        let name = model.name.as_str();
        let mut push = |what: String, ops: Vec<ModelOperation>| {
            out.push((
                format!("{name}: {what}"),
                ProjectPatch {
                    project_ops: vec![],
                    models: vec![ModelPatch {
                        name: name.to_string(),
                        ops,
                    }],
                },
            ));
        };
        let rename = |from: &str, to: String| ModelOperation::RenameVariable {
            from: from.to_string(),
            to,
        };
        let variables: Vec<&Variable> = model.variables.iter().collect();
        for i in sample(variables.len(), max) {
            let variable = variables[i];
            let ident = variable.get_ident();
            push(format!("upsert {ident}"), vec![upsert_of(variable, true)]);
            push(
                format!("upsert {ident} as it is"),
                vec![upsert_of(variable, false)],
            );
            push(
                format!("delete {ident}"),
                vec![ModelOperation::DeleteVariable {
                    ident: ident.to_string(),
                }],
            );
            for to in [
                format!("{ident} renamed"),
                ident.to_uppercase(),
                ident.to_string(),
                format!("if {ident}"),
            ] {
                push(format!("rename {ident} to {to}"), vec![rename(ident, to)]);
            }
            // The first operation leaves elements only the patched project
            // holds, so the ones after it meet both kinds.
            push(
                format!("upsert, rename and rename again {ident}"),
                vec![
                    upsert_of(variable, true),
                    rename(ident, format!("{ident} renamed")),
                    rename(&format!("{ident} renamed"), format!("{ident} again")),
                ],
            );
            if let Variable::Stock(stock) = variable {
                let mut inflows = stock.inflows.clone();
                inflows.reverse();
                let mut outflows = stock.outflows.clone();
                outflows.extend(stock.inflows.first().cloned());
                for (what, inflows, outflows) in [
                    ("other", inflows, outflows),
                    ("the same", stock.inflows.clone(), stock.outflows.clone()),
                ] {
                    push(
                        format!("give {ident} {what} flows"),
                        vec![ModelOperation::UpdateStockFlows {
                            ident: ident.to_string(),
                            inflows,
                            outflows,
                        }],
                    );
                }
            }
        }
        if variables.len() >= 2 {
            push(
                "name a loop".to_string(),
                vec![ModelOperation::SetLoopName {
                    variables: variables[..2]
                        .iter()
                        .map(|v| v.get_ident().to_string())
                        .collect(),
                    name: "a loop".to_string(),
                    description: None,
                }],
            );
        }
        let Some(View::StockFlow(view)) = model.views.first() else {
            continue;
        };
        let elements: Vec<&ViewElement> = view.elements.iter().collect();
        for i in sample(elements.len(), max) {
            let element = elements[i];
            let uid = element.get_uid();
            let edit = |upsert, remove| ModelOperation::EditView {
                index: 0,
                upsert,
                remove,
            };
            if let Some(moved) = moved(element) {
                push(
                    format!("move element {uid}"),
                    vec![edit(vec![moved], vec![])],
                );
            }
            let delete = crate::editing::plan_delete(view, &[uid]);
            if !delete.is_empty() {
                push(
                    format!("delete element {uid}"),
                    vec![edit(delete.upsert, delete.remove)],
                );
            }
            if let Some(label) = element.get_name() {
                let relabel = crate::editing::plan_rename(view, label, &format!("{label} v2"));
                push(
                    format!("rename element {uid}"),
                    vec![edit(relabel.upsert, relabel.remove)],
                );
            }
        }
        // A whole view, every element freshly allocated, one of them moved.
        let mut fresh = view.elements.to_vec();
        if let Some((element, moved)) = fresh.iter_mut().find_map(|e| moved(e).map(|m| (e, m))) {
            *element = moved;
        }
        push(
            "replace the view".to_string(),
            vec![ModelOperation::UpsertView {
                index: 0,
                view: View::StockFlow(datamodel::StockFlow {
                    elements: fresh.into(),
                    ..view.clone()
                }),
            }],
        );
        push(
            "delete the view".to_string(),
            vec![ModelOperation::DeleteView { index: 0 }],
        );
    }

    let mut specs = project.sim_specs.clone();
    specs.stop += 1.0;
    for (what, op) in [
        ("set the sim specs", ProjectOperation::SetSimSpecs(specs)),
        (
            "set the source",
            ProjectOperation::SetSource(datamodel::Source {
                extension: datamodel::Extension::Xmile,
                content: "<xmile/>".to_string(),
            }),
        ),
        (
            "add a model",
            ProjectOperation::AddModel {
                name: "another model".to_string(),
            },
        ),
    ] {
        out.push((
            what.to_string(),
            ProjectPatch {
                project_ops: vec![op],
                models: vec![],
            },
        ));
    }
    out
}

/// The keys of the elements an edit changed, after asserting that every
/// element `same` as the one before it under its key is the same allocation:
/// nothing was copied that the edit left as it was.
fn changed_by<T: Clone, K: Eq + Hash + Clone + std::fmt::Debug>(
    before: &SharedVec<T>,
    after: &SharedVec<T>,
    key: impl Fn(&T) -> K,
    same: impl Fn(&T, &T) -> bool,
    what: &str,
) -> Vec<K> {
    let mut before_at: HashMap<K, (usize, &T)> = HashMap::new();
    for (element, address) in before.iter().zip(before.addresses()) {
        before_at.entry(key(element)).or_insert((address, element));
    }
    let mut changed = Vec::new();
    for (e, address) in after.iter().zip(after.addresses()) {
        let k = key(e);
        match before_at.get(&k) {
            Some((before_address, before_value)) if same(before_value, e) => assert_eq!(
                *before_address, address,
                "{what}: {k:?} is unchanged but was copied"
            ),
            _ => changed.push(k),
        }
    }
    changed
}

fn ident(v: &Variable) -> String {
    canonicalize(v.get_ident()).into_owned()
}

/// Asserts that `after`, a patched copy of `before`, shares every variable
/// and view element the patch left equal to `before`'s.
fn assert_unchanged_is_shared(before: &Project, after: &Project, what: &str) {
    for (b, a) in before.models.iter().zip(&after.models) {
        changed_by(&b.variables, &a.variables, ident, |x, y| x == y, what);
        for (View::StockFlow(b), View::StockFlow(a)) in b.views.iter().zip(&a.views) {
            changed_by(
                &b.elements,
                &a.elements,
                ViewElement::get_uid,
                |x, y| x == y,
                what,
            );
        }
    }
}

/// `project`, with no variable or view element shared with it.
fn unshared(project: &Project) -> Project {
    let mut copy = project.clone();
    for model in &mut copy.models {
        model.variables = model.variables.to_vec().into();
        for View::StockFlow(view) in &mut model.views {
            view.elements = view.elements.to_vec().into();
        }
    }
    copy
}

/// The project `patch` leaves, applied in place to `project`, or the error
/// it fails with.
fn applied(mut project: Project, patch: &ProjectPatch) -> Result<Project, String> {
    match apply_patch_in_place(&mut project, patch.clone()) {
        Ok(()) => Ok(project),
        Err(err) => Err(format!("{:?}: {:?}", err.code, err.details)),
    }
}

/// Checks both properties for every generated patch of `project`, marking in
/// `covered` each operation variant a patch that applied holds.
fn check_sharing(
    project: &Project,
    max: usize,
    covered: &mut (
        [bool; MODEL_OPERATION_VARIANTS],
        [bool; PROJECT_OPERATION_VARIANTS],
    ),
    label: &str,
) -> usize {
    let patches = generated_patches(project, max);
    let count = patches.len();
    for (what, patch) in patches {
        let what = format!("{label}: {what}");
        // Every element shared with `project`, which stays alive.
        let shared = applied(project.clone(), &patch);
        let alone = applied(unshared(project), &patch);
        assert!(
            shared == alone,
            "{what}: the patch lands differently on a shared project"
        );
        let Ok(after) = shared else {
            continue;
        };
        assert_unchanged_is_shared(project, &after, &what);
        for op in patch.models.iter().flat_map(|m| &m.ops) {
            covered.0[model_operation_variant(op)] = true;
        }
        for op in &patch.project_ops {
            covered.1[project_operation_variant(op)] = true;
        }
    }
    count
}

#[test]
fn a_patch_copies_only_what_it_changes_and_lands_the_same_shared_or_not() {
    let mut covered = Default::default();
    // One variable and one element of a large model (each patch walks its
    // 311 variables and 864 elements), and four of each from a small one with
    // modules: every operation variant is still generated, which the last
    // assertion holds, and the corpus gate samples every model more widely.
    check_sharing(&world3(), 1, &mut covered, "world3");
    check_sharing(&hares_and_lynxes(), 4, &mut covered, "hares and lynxes");
    assert_eq!(
        covered,
        (
            [true; MODEL_OPERATION_VARIANTS],
            [true; PROJECT_OPERATION_VARIANTS]
        ),
        "every operation is generated and applies on one of the two models"
    );
}

#[test]
#[ignore = "applies every kind of patch to every corpus model; run under the gates profile"]
fn a_patch_copies_only_what_it_changes_and_lands_the_same_across_the_corpus() {
    let mut covered = Default::default();
    let (mut models, mut patches) = (0, 0);
    for_each_corpus_project(|path, project| {
        models += 1;
        patches += check_sharing(project, 12, &mut covered, &path.display().to_string());
    });
    assert!(models > 100, "the corpus opens: {models} models");
    assert!(patches > 10_000, "{patches} patches");
    assert_eq!(
        covered,
        (
            [true; MODEL_OPERATION_VARIANTS],
            [true; PROJECT_OPERATION_VARIANTS]
        )
    );
}

/// Naming a loop gives a uid to each of its variables that has none, and
/// copies no variable that has one: named again, every variable is the
/// allocation the first naming left.
#[test]
fn naming_a_loop_copies_no_variable_that_has_its_uid() {
    let base = hares_and_lynxes();
    let main = &base.models[0];
    let loop_variables: Vec<String> = main
        .variables
        .iter()
        .take(2)
        .map(|v| v.get_ident().to_string())
        .collect();
    let name_loop = |name: &str| ProjectPatch {
        project_ops: vec![],
        models: vec![ModelPatch {
            name: main.name.clone(),
            ops: vec![ModelOperation::SetLoopName {
                variables: loop_variables.clone(),
                name: name.to_string(),
                description: None,
            }],
        }],
    };

    let named = applied(base.clone(), &name_loop("a loop")).expect("the loop is named");
    let minted = changed_by(
        &main.variables,
        &named.models[0].variables,
        ident,
        |x, y| x == y,
        "the first naming",
    );
    assert_eq!(
        minted.len(),
        2,
        "the fixture's variables have no uids, and each of the two is given one: {minted:?}"
    );

    let renamed = applied(named.clone(), &name_loop("the same loop")).expect("it is named again");
    assert_eq!(
        renamed.models[0].variables.addresses(),
        named.models[0].variables.addresses(),
        "the second naming copies nothing"
    );
    assert_eq!(renamed.models[0].loop_metadata.len(), 1);
    assert_eq!(renamed.models[0].loop_metadata[0].name, "the same loop");
}

/// The conveyor and queue expansions replace the variables they rewrite and
/// leave every other one in the allocation it was in, so the expanded
/// project's sync compares only those (`db::sync`'s memory is by allocation).
#[test]
fn an_expansion_shares_every_variable_it_leaves_as_it_was() {
    type Expansion = fn(&Project) -> Project;
    let conveyors: Expansion = |project| {
        crate::conveyor_compile::expand_conveyors(project, &project.models[0].name)
            .expect("the conveyors expand")
            .0
    };
    let queues: Expansion = |project| {
        crate::queue_compile::expand_queues(project, &project.models[0].name)
            .expect("the queues expand")
            .0
    };
    // Each fixture with the variables it must leave as they were: a driven
    // flow already holding the placeholder `0` the expansion writes is one.
    let fixtures: [(&str, String, Expansion, &[&str]); 3] = [
        (
            "a conveyor whose driven outflow already holds the placeholder",
            include_str!("../../../test/conveyors/minimal_conveyor.xmile").replace(
                "<flow name=\"graduating\">",
                "<flow name=\"graduating\"><eqn>0</eqn>",
            ),
            conveyors,
            &["graduating"],
        ),
        (
            "a conveyor",
            include_str!("../../../test/conveyors/leaky_conveyor.xmile").to_string(),
            conveyors,
            &[],
        ),
        (
            "a queue",
            include_str!("../../../test/queues/queue_drain.xmile").to_string(),
            queues,
            &["into_service"],
        ),
    ];
    for (what, xmile, expand, kept) in fixtures {
        let project =
            crate::compat::open_xmile(&mut BufReader::new(xmile.as_bytes())).expect("it opens");
        let expanded = expand(&project);
        let (before, after) = (&project.models[0].variables, &expanded.models[0].variables);
        let changed = changed_by(before, after, ident, |x, y| x == y, what);
        assert!(
            !changed.is_empty(),
            "{what}: the expansion rewrites something"
        );
        for name in kept {
            assert!(
                !changed.contains(&name.to_string()),
                "{what}: {name} is left as it was"
            );
        }
        let shared = after
            .addresses()
            .iter()
            .filter(|address| before.addresses().contains(address))
            .count();
        assert_eq!(
            shared,
            after.len() - changed.len(),
            "{what}: everything else is shared"
        );
        assert!(shared > 0, "{what}: and something is left as it was");
    }
}

fn elements(project: &Project) -> &SharedVec<ViewElement> {
    match &project.models[0].views[0] {
        View::StockFlow(sf) => &sf.elements,
    }
}

#[test]
fn a_layout_sync_shares_every_element_it_leaves_in_place() {
    let original = world3();
    let mut copy = original.clone();
    let aux = original.models[0]
        .variables
        .iter()
        .rev()
        .find_map(|v| match v {
            Variable::Aux(a) if matches!(a.equation, Equation::Scalar(_)) => Some(a.clone()),
            _ => None,
        })
        .expect("world3 has a scalar aux");
    let mut edited = aux;
    edited.equation = tweaked(&edited.equation);
    let patch = ProjectPatch {
        project_ops: vec![],
        models: vec![ModelPatch {
            name: "main".to_string(),
            ops: vec![ModelOperation::UpsertAux(edited)],
        }],
    };
    apply_patch(&mut copy, patch.clone()).unwrap();
    let View::StockFlow(old) = &copy.models[0].views[0];
    let old = old.clone();
    // The layout lays every element out afresh, as a host's diagram sync
    // does after each edit.
    let synced = crate::layout::incremental_layout(&old, &copy, "main", &patch.models[0], None)
        .expect("the layout syncs");
    copy.models[0].views[0] = View::StockFlow(synced);

    let moved = changed_by(
        elements(&original),
        elements(&copy),
        ViewElement::get_uid,
        |a, b| a == b,
        "a sync after an equation edit",
    );
    assert!(moved.is_empty(), "an equation edit moves nothing");
}

#[test]
fn planning_shares_the_view_it_plans_on() {
    let project = world3();
    let View::StockFlow(sf) = &project.models[0].views[0];
    let base = crate::editing::BaseView::new(&project.models[0], sf);
    assert_eq!(base.elements().addresses(), sf.elements.addresses());
}
