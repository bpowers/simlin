// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The one diagnostic payload, from every producer to `collect_all_diagnostics`.
//!
//! Three enumerations drive the tests here, each derived from the code rather
//! than from a sample: the producers (every site that files a `Diagnostic`),
//! the `(category, severity)` cells those producers can reach, and the
//! warning families a module-referenced sub-model can raise, each of which
//! must be reported exactly once however many models reach it.

use std::any::Any;

use super::*;
use crate::common::ErrorCode;
use crate::datamodel;
use crate::test_common::TestProject;
use crate::testutils::{x_aux, x_flow, x_model, x_module_named, x_stock};

// ── shared fixtures ────────────────────────────────────────────────────

fn gf(x_points: Vec<f64>, y_points: Vec<f64>) -> datamodel::GraphicalFunction {
    datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        x_points: Some(x_points),
        y_points,
    }
}

/// A one-loop stock/flow model, the smallest LTM-instrumented fixture.
fn loop_project() -> datamodel::Project {
    TestProject::new("loop")
        .stock("s", "100", &["in_f"], &["out_f"], None)
        .flow("in_f", "s * 0.1", None)
        .flow("out_f", "s * 0.05", None)
        .build_datamodel()
}

/// `sales[Cities] + prices[Products]` in an apply-to-all body: the lowering
/// refuses it with `MismatchedDimensions`, under the plain spelling and under
/// a helper the parse hoists (`SMTH1(sales + prices, 1)`) alike.
fn mismatched_dims_project(name: &str, spelling: &str) -> datamodel::Project {
    TestProject::new(name)
        .named_dimension("Cities", &["Boston", "Seattle"])
        .named_dimension("Products", &["Widgets", "Gadgets"])
        .array_aux("sales[Cities]", "1")
        .array_aux("prices[Products]", "1")
        .array_aux("bad[Cities]", spelling)
        .build_datamodel()
}

/// `pop[region]` and a scalar `scale`, with `target` defined by `equation`:
/// the reducer-over-arithmetic shape codegen refuses in an apply-to-all body.
fn reducer_project(target: &str, equation: &str) -> datamodel::Project {
    TestProject::new("reducer")
        .named_dimension("region", &["north", "south"])
        .array_const("pop[region]", 10.0)
        .scalar_const("scale", 2.0)
        .array_aux(target, equation)
        .build_datamodel()
}

fn units_umbrella_project() -> datamodel::Project {
    TestProject::new("umbrella")
        .unit("apples", None)
        .unit("oranges", None)
        .aux("apple_count", "10", Some("apples"))
        .aux("orange_count", "20", Some("oranges"))
        .aux("fruit_total", "apple_count + orange_count", None)
        .build_datamodel()
}

fn units_consistency_project() -> datamodel::Project {
    TestProject::new("consistency")
        .unit("Person", None)
        .unit("Month", None)
        .aux("source", "1", Some("Month"))
        .aux("bad_units", "source", Some("Person"))
        .build_datamodel()
}

/// `main` holding a module `m` over `sub` whose one input reference names a
/// port `sub` does not declare.
fn miswired_module_project() -> datamodel::Project {
    crate::testutils::x_project(
        datamodel::SimSpecs::default(),
        &[
            x_model(
                "main",
                vec![
                    x_aux("local_input", "10", None),
                    x_module_named("m", "sub", &[("local_input", "m.bogus")], None),
                ],
            ),
            x_model("sub", vec![x_aux("input_var", "0", None)]),
        ],
    )
}

/// `a` instantiates `b` and `b` instantiates `a`: the module cycle every
/// entry point reports instead of recursing into.
fn module_cycle_project() -> datamodel::Project {
    crate::testutils::x_project(
        datamodel::SimSpecs::default(),
        &[
            x_model(
                "a",
                vec![
                    x_aux("x", "1", None),
                    x_module_named("to_b", "b", &[("x", "to_b.input")], None),
                ],
            ),
            x_model(
                "b",
                vec![
                    x_aux("input", "0", None),
                    x_module_named("to_a", "a", &[("input", "to_a.x")], None),
                ],
            ),
        ],
    )
}

/// A `total_nodes`-variable single SCC: `cap_stock -> aux_{N-3} -> ... ->
/// aux_0 -> cap_flow -> cap_stock`.
fn chain_scc_project(total_nodes: usize) -> datamodel::Project {
    let aux_count = total_nodes - 2;
    let mut builder = TestProject::new("chain");
    for i in 0..aux_count {
        let equation = if i + 1 == aux_count {
            "cap_stock".to_string()
        } else {
            format!("aux_{}", i + 1)
        };
        builder = builder.scalar_aux(&format!("aux_{i}"), &equation);
    }
    builder
        .flow("cap_flow", "aux_0", None)
        .stock("cap_stock", "0", &["cap_flow"], &[], None)
        .build_datamodel()
}

/// `share[r] = pop[r] / SUM(pop[*])` over `n` regions: one synthetic
/// aggregate with `n` disjoint petals through it.
fn share_reducer_project(n: usize) -> datamodel::Project {
    let elements: Vec<String> = (0..n).map(|i| format!("r{i}")).collect();
    let refs: Vec<&str> = elements.iter().map(String::as_str).collect();
    TestProject::new("share")
        .with_sim_time(0.0, 5.0, 1.0)
        .named_dimension("Region", &refs)
        .array_stock("pop[Region]", "100", &["update"], &[], None)
        .array_aux("share[Region]", "pop / SUM(pop[*])")
        .array_flow("update[Region]", "share * 0.001", None)
        .build_datamodel()
}

/// A module body with `paths` parallel `input -> mid_i -> total_flow ->
/// output` pathways.
fn parallel_pathways_project(paths: usize) -> datamodel::Project {
    let mut vars = vec![x_aux("input", "1", None)];
    let mut total = String::new();
    for i in 0..paths {
        vars.push(x_aux(
            &format!("mid_{i}"),
            &format!("input * {}", i + 1),
            None,
        ));
        if i > 0 {
            total.push_str(" + ");
        }
        total.push_str(&format!("mid_{i}"));
    }
    vars.push(x_flow("total_flow", &total, None));
    vars.push(x_stock("output", "0", &["total_flow"], &[], None));
    crate::testutils::x_project(datamodel::SimSpecs::default(), &[x_model("main", vars)])
}

fn xmile_project(xml: &str) -> datamodel::Project {
    crate::xmile::project_from_reader(&mut std::io::BufReader::new(xml.as_bytes()))
        .expect("the checked-in fixture parses")
}

/// Pin the loop through `variables` on `model`, the `SetLoopName` shape.
fn pin_loop(model: &mut datamodel::Model, name: &str, variables: &[&str]) {
    let mut uid_of = std::collections::HashMap::new();
    for (index, variable) in model.variables.iter_mut().enumerate() {
        let uid = (index as i32) + 1;
        uid_of.insert(crate::canonicalize(variable.get_ident()).into_owned(), uid);
        match variable {
            datamodel::Variable::Stock(stock) => stock.uid = Some(uid),
            datamodel::Variable::Flow(flow) => flow.uid = Some(uid),
            datamodel::Variable::Aux(aux) => aux.uid = Some(uid),
            datamodel::Variable::Module(module) => module.uid = Some(uid),
        }
    }
    model.loop_metadata.push(datamodel::LoopMetadata {
        uids: variables.iter().map(|v| uid_of[*v]).collect(),
        deleted: false,
        name: name.to_string(),
        description: String::new(),
    });
}

fn assembly_reason_contains(d: &Diagnostic, needle: &str) -> bool {
    d.category() == DiagnosticCategory::Assembly && d.reason().is_some_and(|r| r.contains(needle))
}

fn all_diagnostics(project: &datamodel::Project, ltm: bool) -> Vec<Diagnostic> {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, project);
    collect_all_diagnostics(&db, sync.project, crate::db::LtmOverlay::from(ltm))
}

// ── the producer x category x severity matrix ──────────────────────────

/// One producer, driven through the production inputs, with the cell of the
/// `(category, severity)` grid it lands in.
struct Producer {
    site: &'static str,
    project: fn() -> datamodel::Project,
    ltm: bool,
    guards: fn() -> Vec<Box<dyn Any>>,
    matches: fn(&Diagnostic) -> bool,
    category: DiagnosticCategory,
    severity: DiagnosticSeverity,
}

fn no_guards() -> Vec<Box<dyn Any>> {
    Vec::new()
}

/// Every diagnostic producer files one payload, and the payload's projections
/// agree with the arm it was raised on.
///
/// The rows are the raising sites, in pipeline order: the parse
/// (`variable::parse_var`), the lowering memo (`model::lower_variable`), the
/// explicit fragment constructor's gates (`db/var_fragment.rs`), the two
/// emitters (`db/fragment_compile.rs`), the dependency graph, the per-model
/// advisories and the unit pass (`db/diagnostic.rs`, `db/units.rs`), the LTM
/// facts (`db/ltm/`), and the project-level facts `collect_all_diagnostics`
/// reads from memos. Together they reach eight cells of the
/// `DiagnosticCategory x DiagnosticSeverity` grid, asserted below; the other
/// four -- `Equation`/`Warning`, `UnitDefinition`/`Warning`,
/// `UnitConsistency`/`Error`, `UnitInference`/`Error` -- have no producer.
///
/// Producers with no row here, each pinned elsewhere: the conveyor spec and
/// parameter-unit advisories and the conveyor/queue LTM-degraded warnings
/// (`db::diagnostic_tests`, `db::units`' tests, the once-across-revisions
/// matrix below), the macro-registry build error
/// (`macro_registry_build_error_is_reported_exactly_once`), the explicit
/// per-phase `lower_fragment` refusal
/// (`test_compile_var_fragment_per_phase_var_new_failure`), a helper's
/// per-phase refusal filed at the parent's argument span
/// (`implicit_diag_tests::implicit_helper_lowering_failure_is_an_equation_error_on_the_parent`),
/// an LTM implicit helper whose fragment fails, owned by its link score
/// (`ltm_unified_tests::test_model_ltm_fragment_diagnostics_covers_implicit_helpers`),
/// and the LTM edge, pin, pathway and width warnings (the matrix below).
#[test]
fn every_producer_files_one_payload_whose_projections_agree() {
    use DiagnosticCategory::*;
    use DiagnosticSeverity::*;

    let producers: Vec<Producer> = vec![
        Producer {
            site: "parse: an equation that does not parse",
            project: || {
                TestProject::new("m")
                    .aux("bad", "1 +", None)
                    .build_datamodel()
            },
            ltm: false,
            guards: no_guards,
            matches: |d| d.variable.as_deref() == Some("bad") && d.category() == Equation,
            category: Equation,
            severity: Error,
        },
        Producer {
            site: "parse: a malformed <units> string",
            project: || {
                TestProject::new("m")
                    .aux("u", "1", Some("bad units here!!!"))
                    .build_datamodel()
            },
            ltm: false,
            guards: no_guards,
            matches: |d| d.variable.as_deref() == Some("u") && d.category() == UnitDefinition,
            category: UnitDefinition,
            severity: Error,
        },
        Producer {
            site: "lowering memo: mismatched dimensions",
            project: || mismatched_dims_project("m", "sales + prices"),
            ltm: false,
            guards: no_guards,
            matches: |d| d.is(Equation, ErrorCode::MismatchedDimensions),
            category: Equation,
            severity: Error,
        },
        Producer {
            site: "fragment constructor: unknown dependency",
            project: || {
                TestProject::new("m")
                    .aux("x", "1 + bogus", None)
                    .build_datamodel()
            },
            ltm: false,
            guards: no_guards,
            matches: |d| d.is(Equation, ErrorCode::UnknownDependency),
            category: Equation,
            severity: Error,
        },
        Producer {
            site: "fragment constructor: a lookup table read bare",
            project: || {
                TestProject::new("m")
                    .aux_with_gf("tbl", "", gf(vec![0.0, 1.0], vec![0.0, 1.0]))
                    .aux("x", "tbl", None)
                    .build_datamodel()
            },
            ltm: false,
            guards: no_guards,
            matches: |d| d.is(Model, ErrorCode::LookupReferencedWithoutArgument),
            category: Model,
            severity: Error,
        },
        Producer {
            site: "fragment constructor: a table that does not build",
            project: || {
                TestProject::new("m")
                    .aux_with_gf("bad_table", "1", gf(vec![0.0, 1.0], vec![0.0, 0.5, 1.0]))
                    .build_datamodel()
            },
            ltm: false,
            guards: no_guards,
            matches: |d| d.is(Model, ErrorCode::BadTable),
            category: Model,
            severity: Error,
        },
        Producer {
            site: "explicit emitter: a codegen refusal",
            project: || reducer_project("plainx[region]", "SUM(pop * scale)"),
            ltm: false,
            guards: no_guards,
            matches: |d| d.variable.as_deref() == Some("plainx") && d.category() == Assembly,
            category: Assembly,
            severity: Error,
        },
        Producer {
            site: "implicit emitter: a helper codegen refusal, owned by its parent",
            project: || reducer_project("aggx[region]", "PREVIOUS(SUM(pop * scale))"),
            ltm: false,
            guards: no_guards,
            matches: |d| d.owner.as_deref() == Some("aggx") && d.category() == Assembly,
            category: Assembly,
            severity: Error,
        },
        Producer {
            site: "implicit emitter: a helper that does not lower, on its parent",
            project: || mismatched_dims_project("m", "SMTH1(sales + prices, 1)"),
            ltm: false,
            guards: no_guards,
            matches: |d| {
                d.variable.as_deref() == Some("bad")
                    && d.is(Equation, ErrorCode::MismatchedDimensions)
            },
            category: Equation,
            severity: Error,
        },
        Producer {
            site: "dependency graph: a cycle",
            project: || {
                TestProject::new("m")
                    .aux("a", "b", None)
                    .aux("b", "a", None)
                    .build_datamodel()
            },
            ltm: false,
            guards: no_guards,
            matches: |d| d.is(Model, ErrorCode::CircularDependency),
            category: Model,
            severity: Error,
        },
        Producer {
            site: "per-model owner: duplicate canonical names",
            project: || {
                TestProject::new("m")
                    .aux("My Var", "1", None)
                    .aux("my_var", "2", None)
                    .build_datamodel()
            },
            ltm: false,
            guards: no_guards,
            matches: |d| d.is(Model, ErrorCode::DuplicateVariable),
            category: Model,
            severity: Error,
        },
        Producer {
            site: "unit pass: the inference umbrella",
            project: units_umbrella_project,
            ltm: false,
            guards: no_guards,
            matches: |d| d.variable.is_none() && d.is(UnitInference, ErrorCode::UnitMismatch),
            category: UnitInference,
            severity: Warning,
        },
        Producer {
            site: "unit pass: a consistency mismatch",
            project: units_consistency_project,
            ltm: false,
            guards: no_guards,
            matches: |d| {
                d.variable.as_deref() == Some("bad_units")
                    && d.is(UnitConsistency, ErrorCode::UnitMismatch)
            },
            category: UnitConsistency,
            severity: Warning,
        },
        Producer {
            site: "unit pass: a stdlib module's arguments feeding one stock in different units",
            project: || {
                TestProject::new("m")
                    .unit("Person", None)
                    .unit("Dollar", None)
                    .aux("inp", "1", Some("Person"))
                    .aux("init", "2", Some("Dollar"))
                    .aux("x", "SMTH1(inp, 3, init)", Some("Person"))
                    .build_datamodel()
            },
            ltm: false,
            guards: no_guards,
            matches: |d| {
                d.is(UnitConsistency, ErrorCode::UnitMismatch)
                    && d.reason()
                        .is_some_and(|r| r.contains("feed the same internal variable"))
            },
            category: UnitConsistency,
            severity: Warning,
        },
        Producer {
            site: "module wiring: a reference to no port",
            project: miswired_module_project,
            ltm: false,
            guards: no_guards,
            matches: |d| d.is(Model, ErrorCode::BadModuleInputDst),
            category: Model,
            severity: Warning,
        },
        Producer {
            site: "module wiring: a source naming no variable of this model",
            project: || {
                crate::testutils::x_project(
                    datamodel::SimSpecs::default(),
                    &[
                        x_model(
                            "main",
                            vec![x_module_named(
                                "m",
                                "sub",
                                &[("ghost", "m.input_var")],
                                None,
                            )],
                        ),
                        x_model("sub", vec![x_aux("input_var", "0", None)]),
                    ],
                )
            },
            ltm: false,
            guards: no_guards,
            matches: |d| d.is(Model, ErrorCode::BadModuleInputSrc),
            category: Model,
            severity: Warning,
        },
        Producer {
            site: "module wiring: a module over no model",
            project: || {
                crate::testutils::x_project(
                    datamodel::SimSpecs::default(),
                    &[x_model(
                        "main",
                        vec![x_module_named("m", "ghost", &[], None)],
                    )],
                )
            },
            ltm: false,
            guards: no_guards,
            matches: |d| d.is(Model, ErrorCode::BadModelName),
            category: Model,
            severity: Error,
        },
        Producer {
            site: "advisory: an element no dimension declares",
            project: || {
                TestProject::new("m")
                    .named_dimension("d", &["e1"])
                    .array_with_ranges("y[d]", vec![("e1", "1"), ("bogus", "2")])
                    .build_datamodel()
            },
            ltm: false,
            guards: no_guards,
            matches: |d| d.is(Model, ErrorCode::UnknownElementSubscript),
            category: Model,
            severity: Warning,
        },
        Producer {
            site: "advisory: an unfilled equation",
            project: || {
                TestProject::new("m")
                    .aux("x", "NAN", None)
                    .build_datamodel()
            },
            ltm: false,
            guards: no_guards,
            matches: |d| d.is(Model, ErrorCode::UnfilledEquation),
            category: Model,
            severity: Warning,
        },
        Producer {
            site: "LTM facts: a synthetic fragment that fails to compile",
            project: loop_project,
            ltm: true,
            guards: || vec![Box::new(LtmFragmentFailureGuard::new("link_score"))],
            matches: |d| assembly_reason_contains(d, "failed to compile"),
            category: Assembly,
            severity: Warning,
        },
        Producer {
            site: "LTM facts: the flip to discovery mode",
            project: loop_project,
            ltm: true,
            guards: || vec![Box::new(crate::ltm::LtmCircuitBudgetGuard::new(1))],
            matches: |d| assembly_reason_contains(d, "MAX_LTM_CIRCUITS"),
            category: Assembly,
            severity: Warning,
        },
        Producer {
            site: "project facts: a unit declaration that does not parse",
            project: || {
                TestProject::new("m")
                    .unit("BadUnit", Some("1///invalid"))
                    .aux("x", "1", Some("BadUnit"))
                    .build_datamodel()
            },
            ltm: false,
            guards: no_guards,
            matches: |d| d.model.is_empty() && d.category() == UnitDefinition,
            category: UnitDefinition,
            severity: Error,
        },
        Producer {
            site: "project facts: a module cycle, per reaching model",
            project: module_cycle_project,
            ltm: false,
            guards: no_guards,
            matches: |d| {
                d.model == "a" && d.variable.is_none() && d.is(Model, ErrorCode::CircularDependency)
            },
            category: Model,
            severity: Error,
        },
    ];

    let mut cells: std::collections::BTreeSet<(String, String)> = Default::default();
    for producer in &producers {
        let _guards = (producer.guards)();
        let project = (producer.project)();
        let diagnostics = all_diagnostics(&project, producer.ltm);
        let hits: Vec<&Diagnostic> = diagnostics
            .iter()
            .filter(|d| (producer.matches)(d))
            .collect();
        assert!(
            !hits.is_empty(),
            "{}: no diagnostic matched; got {diagnostics:#?}",
            producer.site
        );
        for hit in hits {
            assert_eq!(
                hit.category(),
                producer.category,
                "{}: {hit:?}",
                producer.site
            );
            assert_eq!(
                hit.severity, producer.severity,
                "{}: {hit:?}",
                producer.site
            );
            // The projections are one reading of the typed arm: `is` composes
            // `category` and `code`, and `reason`/`location` come from the
            // same arm the category names.
            assert!(
                hit.is(hit.category(), hit.code()),
                "{}: {hit:?}",
                producer.site
            );
            let has_span = hit.location().is_some();
            let span_bearing = matches!(
                hit.category(),
                Equation | UnitDefinition | UnitConsistency | UnitInference
            );
            assert!(
                !has_span || span_bearing,
                "{}: only an equation, unit-string or unit error carries a span: {hit:?}",
                producer.site
            );
        }
        cells.insert((
            format!("{:?}", producer.category),
            format!("{:?}", producer.severity),
        ));
    }

    let reachable: std::collections::BTreeSet<(String, String)> = [
        (Equation, Error),
        (Model, Error),
        (Model, Warning),
        (UnitDefinition, Error),
        (UnitConsistency, Warning),
        (UnitInference, Warning),
        (Assembly, Error),
        (Assembly, Warning),
    ]
    .into_iter()
    .map(|(c, s)| (format!("{c:?}"), format!("{s:?}")))
    .collect();
    assert_eq!(
        cells, reachable,
        "the producers must cover exactly the reachable cells of the grid"
    );
}

// ── exactly once, across revisions, per warning family ─────────────────

/// One warning family a module-referenced sub-model can raise.
struct Family {
    name: &'static str,
    /// The project whose model named `child` raises the warning; every other
    /// model it holds rides along (a module the child instantiates).
    child: fn() -> datamodel::Project,
    /// What the parent reads of the child, which is what pulls the parent's
    /// LTM derivation into the child's.
    probe: &'static str,
    /// The parent's wiring of the child's input ports, when the family needs
    /// one (`driver` is a parent aux).
    wiring: &'static [(&'static str, &'static str)],
    discovery: bool,
    guards: fn() -> Vec<Box<dyn Any>>,
    matches: fn(&Diagnostic) -> bool,
}

/// A one-model project as a `child`.
fn as_child(mut project: datamodel::Project) -> datamodel::Project {
    let mut child = project.models.pop().expect("one model");
    child.name = "child".to_string();
    project.models = vec![child];
    project
}

fn loop_child() -> datamodel::Project {
    as_child(loop_project())
}

/// Every warning family a sub-model can raise is reported exactly once for
/// the whole project, however many models reach the sub-model, and again
/// exactly once after an unrelated revision.
///
/// The double-drain shape: a parent instantiates `child` and reads a
/// variable of it, so the parent's LTM derivation and layout reach `child`'s
/// `model_ltm_variables`. A warning accumulated inside that query would sit
/// in every reaching model's accumulator subtree and be reported once per
/// reaching model (GH #866); a fact on the value is emitted by the child's
/// own per-model owner only. Each family is driven under every reach: one
/// parent, two parents (`main` and a second model `other`, which `main`
/// instantiates), and one parent instantiating the child twice. The
/// unrelated revision (the project's name) bumps salsa's revision without
/// touching any input the families read, which is where an accumulated value
/// pruned out of the DFS would vanish.
///
/// The rows are the `Warning` families from the family enumeration -- the
/// LTM facts of `model_ltm_variables` and `model_ltm_fragment_diagnostics`,
/// the special-stock LTM advisories, and the per-model warnings of the unit
/// pass, the wiring check and the two equation advisories. Not here, each
/// pinned in its own test: the auto-flip on an oversized slow-path SCC and
/// the unresolved loop-score / pathway-link warnings (no small natural
/// fixture reaches them), the conveyor spec advisories
/// (`db::diagnostic_tests`, single-model) and the conveyor parameter-unit
/// warnings (`db::units`' tests, single-model).
#[test]
fn every_warning_family_is_emitted_once_across_revisions() {
    use salsa::Setter;

    let families: Vec<Family> = vec![
        Family {
            name: "ltm: a synthetic fragment that fails to compile",
            child: loop_child,
            probe: "s",
            wiring: &[],
            discovery: false,
            guards: || vec![Box::new(LtmFragmentFailureGuard::new("s\u{2192}in_f"))],
            matches: |d| {
                assembly_reason_contains(d, "failed to compile")
                    && d.variable
                        .as_deref()
                        .is_some_and(|v| v.contains("s\u{2192}in_f"))
            },
        },
        Family {
            name: "ltm: the circuit-budget flip to discovery",
            child: loop_child,
            probe: "s",
            wiring: &[],
            discovery: false,
            guards: || vec![Box::new(crate::ltm::LtmCircuitBudgetGuard::new(1))],
            matches: |d| assembly_reason_contains(d, "MAX_LTM_CIRCUITS"),
        },
        Family {
            name: "ltm: the variable-level SCC flip to discovery",
            child: || as_child(chain_scc_project(crate::ltm::MAX_LTM_SCC_NODES + 1)),
            probe: "cap_stock",
            wiring: &[],
            discovery: false,
            guards: no_guards,
            matches: |d| assembly_reason_contains(d, "variable-level causal graph"),
        },
        Family {
            name: "ltm: an equation too wide for one occurrence path",
            child: || {
                as_child(
                    TestProject::new("wide")
                        .stock("s", "100", &["in_f"], &[], None)
                        .flow("in_f", "MAX(s, 1) * 0.1", None)
                        .build_datamodel(),
                )
            },
            probe: "s",
            wiring: &[],
            discovery: false,
            guards: || vec![Box::new(crate::db::ltm_ir::SiteChildrenLimitGuard::new(1))],
            matches: |d| assembly_reason_contains(d, "LTM analysis was skipped"),
        },
        Family {
            name: "ltm: module pathways truncated at the budget",
            child: || as_child(parallel_pathways_project(4)),
            probe: "output",
            wiring: &[("driver", "child.input")],
            discovery: false,
            guards: || vec![Box::new(crate::ltm::ModulePathwayBudgetGuard::new(2))],
            matches: |d| assembly_reason_contains(d, "module-pathway enumeration was truncated"),
        },
        Family {
            name: "ltm: cross-aggregate loop recovery truncated",
            child: || as_child(share_reducer_project(5)),
            probe: "pop[r0]",
            wiring: &[],
            discovery: false,
            guards: || vec![Box::new(AggLoopBudgetGuard::new(3))],
            matches: |d| assembly_reason_contains(d, "loop recovery was truncated"),
        },
        Family {
            name: "ltm: a pin that is no loop",
            child: || {
                let mut project = loop_child();
                project.models[0]
                    .variables
                    .push(x_aux("dangling", "s", None));
                pin_loop(&mut project.models[0], "bogus", &["s", "dangling"]);
                project
            },
            probe: "s",
            wiring: &[],
            discovery: false,
            guards: no_guards,
            matches: |d| assembly_reason_contains(d, "pinned loop 'bogus'"),
        },
        Family {
            name: "ltm: an arrayed edge whose dimensions do not correspond",
            child: || {
                as_child(
                    TestProject::new("conservative")
                        .named_dimension("SourceDim", &["a", "b"])
                        .named_dimension("TargetDim", &["x", "y", "z"])
                        .array_stock("source[SourceDim]", "10", &[], &[], None)
                        .scalar_aux("selector", "1")
                        .array_aux("target[TargetDim]", "source[selector]")
                        .build_datamodel(),
                )
            },
            probe: "source[a]",
            wiring: &[],
            discovery: true,
            guards: no_guards,
            matches: |d| assembly_reason_contains(d, "dimensions do not correspond"),
        },
        Family {
            name: "ltm: a per-element target reading a disjoint source dynamically",
            child: || {
                as_child(
                    TestProject::new("disjoint")
                        .named_dimension("SourceDim", &["a", "b"])
                        .named_dimension("TargetDim", &["x", "y"])
                        .array_stock("source[SourceDim]", "10", &[], &[], None)
                        .scalar_aux("selector", "1")
                        .array_with_ranges(
                            "target[TargetDim]",
                            vec![("x", "source[selector]"), ("y", "source[selector]")],
                        )
                        .build_datamodel(),
                )
            },
            probe: "source[a]",
            wiring: &[],
            discovery: true,
            guards: no_guards,
            matches: |d| assembly_reason_contains(d, "dynamic index or an un-hoisted reducer"),
        },
        Family {
            name: "ltm: a partial equation that cannot be generated",
            child: || {
                as_child(
                    TestProject::new("partial")
                        .named_dimension("d1", &["a", "b"])
                        .array_stock("pop[d1]", "100", &["growth"], &["drain"], None)
                        .array_flow("growth[d1]", "pop[d1] * 0.1", None)
                        .array_flow("drain[d1]", "pop[d1] * 0.05", None)
                        .build_datamodel(),
                )
            },
            probe: "pop[a]",
            wiring: &[],
            discovery: true,
            guards: || {
                vec![Box::new(ForcePartialEquationErrorGuard::new(
                    "pop", "growth",
                ))]
            },
            matches: |d| {
                assembly_reason_contains(d, "could not be generated")
                    && d.variable
                        .as_deref()
                        .is_some_and(|v| v.contains("pop\u{2192}growth"))
            },
        },
        Family {
            name: "ltm: a pin through an edge that could not be scored",
            child: || {
                let mut project = as_child(
                    TestProject::new("pinned")
                        .named_dimension("d1", &["a", "b"])
                        .array_stock("pop[d1]", "100", &["growth"], &["drain"], None)
                        .array_flow("growth[d1]", "pop[d1] * 0.1", None)
                        .array_flow("drain[d1]", "pop[d1] * 0.05", None)
                        .build_datamodel(),
                );
                pin_loop(&mut project.models[0], "growth loop", &["pop", "growth"]);
                project
            },
            probe: "pop[a]",
            wiring: &[],
            discovery: true,
            guards: || {
                vec![Box::new(ForcePartialEquationErrorGuard::new(
                    "pop", "growth",
                ))]
            },
            matches: |d| assembly_reason_contains(d, "pinned loop 'growth loop' traverses"),
        },
        Family {
            name: "ltm: a conveyor stock, scored as INTEG",
            child: || {
                as_child(xmile_project(include_str!(
                    "../../../../test/conveyors/minimal_conveyor.xmile"
                )))
            },
            probe: "graduating",
            wiring: &[],
            discovery: false,
            guards: no_guards,
            matches: |d| {
                d.variable.as_deref() == Some("Students")
                    && d.is(DiagnosticCategory::Model, ErrorCode::ConveyorLtmDegraded)
            },
        },
        Family {
            name: "ltm: a queue stock, scored as INTEG",
            child: || {
                as_child(xmile_project(include_str!(
                    "../../../../test/queues/queue_drain.xmile"
                )))
            },
            probe: "served",
            wiring: &[],
            discovery: false,
            guards: no_guards,
            matches: |d| {
                d.variable.as_deref() == Some("waiting")
                    && d.is(DiagnosticCategory::Model, ErrorCode::QueueLtmDegraded)
            },
        },
        Family {
            name: "units: the inference umbrella",
            child: || as_child(units_umbrella_project()),
            probe: "fruit_total",
            wiring: &[],
            discovery: false,
            guards: no_guards,
            matches: |d| {
                d.variable.is_none()
                    && d.is(DiagnosticCategory::UnitInference, ErrorCode::UnitMismatch)
            },
        },
        Family {
            name: "units: a consistency mismatch",
            child: || as_child(units_consistency_project()),
            probe: "bad_units",
            wiring: &[],
            discovery: false,
            guards: no_guards,
            matches: |d| {
                d.variable.as_deref() == Some("bad_units")
                    && d.is(DiagnosticCategory::UnitConsistency, ErrorCode::UnitMismatch)
            },
        },
        Family {
            name: "wiring: a module reference to no port",
            child: || {
                let mut project = miswired_module_project();
                project.models[0].name = "child".to_string();
                project
            },
            probe: "local_input",
            wiring: &[],
            discovery: false,
            guards: no_guards,
            matches: |d| d.is(DiagnosticCategory::Model, ErrorCode::BadModuleInputDst),
        },
        Family {
            name: "advisory: an element no dimension declares",
            child: || {
                as_child(
                    TestProject::new("elements")
                        .named_dimension("d", &["e1"])
                        .array_with_ranges("y[d]", vec![("e1", "1"), ("bogus", "2")])
                        .build_datamodel(),
                )
            },
            probe: "y[e1]",
            wiring: &[],
            discovery: false,
            guards: no_guards,
            matches: |d| {
                d.is(
                    DiagnosticCategory::Model,
                    ErrorCode::UnknownElementSubscript,
                )
            },
        },
        Family {
            name: "advisory: an unfilled equation",
            child: || {
                as_child(
                    TestProject::new("nan")
                        .aux("x", "NAN", None)
                        .build_datamodel(),
                )
            },
            probe: "x",
            wiring: &[],
            discovery: false,
            guards: no_guards,
            matches: |d| d.is(DiagnosticCategory::Model, ErrorCode::UnfilledEquation),
        },
    ];

    /// How many models reach the child, and how.
    #[derive(Clone, Copy, Debug)]
    enum Reach {
        OneParent,
        TwoParents,
        TwoInstances,
    }
    let reaches = [Reach::OneParent, Reach::TwoParents, Reach::TwoInstances];

    for family in &families {
        for reach in reaches {
            let _guards = (family.guards)();
            let mut project = (family.child)();
            assert!(
                project.models.iter().any(|m| m.name == "child"),
                "{}: the fixture names its warning model `child`",
                family.name
            );
            // The wiring names the instance: `child.port` is written against
            // whichever module variable holds the child.
            let wired = |instance: &str| -> Vec<(String, String)> {
                family
                    .wiring
                    .iter()
                    .map(|(src, dst)| {
                        (
                            src.to_string(),
                            dst.replace("child.", &format!("{instance}.")),
                        )
                    })
                    .collect()
            };
            let instance = |name: &str, wiring: &[(String, String)]| {
                let wiring: Vec<(&str, &str)> = wiring
                    .iter()
                    .map(|(src, dst)| (src.as_str(), dst.as_str()))
                    .collect();
                x_module_named(name, "child", &wiring, None)
            };
            let (w1, w2) = (wired("c1"), wired("c2"));
            let mut main_vars = vec![
                x_aux("driver", "1", None),
                instance("c1", &w1),
                x_aux("reader", &format!("c1\u{00b7}{}", family.probe), None),
            ];
            match reach {
                Reach::OneParent => {}
                Reach::TwoInstances => {
                    main_vars.push(instance("c2", &w2));
                    main_vars.push(x_aux(
                        "reader2",
                        &format!("c2\u{00b7}{}", family.probe),
                        None,
                    ));
                }
                Reach::TwoParents => {
                    main_vars.push(x_module_named("o", "other", &[], None));
                    main_vars.push(x_aux("reader_o", "o\u{00b7}reader2", None));
                    project.models.push(x_model(
                        "other",
                        vec![
                            x_aux("driver", "1", None),
                            instance("c2", &w2),
                            x_aux("reader2", &format!("c2\u{00b7}{}", family.probe), None),
                        ],
                    ));
                }
            }
            project.models.insert(0, x_model("main", main_vars));

            let mut db = SimlinDb::default();
            let sync = sync_from_datamodel(&db, &project);
            if family.discovery {
                sync.project.set_ltm_discovery_mode(&mut db).to(true);
            }
            let emitted = |db: &SimlinDb| -> Vec<Diagnostic> {
                collect_all_diagnostics(db, sync.project, crate::db::LtmOverlay::On)
                    .into_iter()
                    .filter(|d| d.model == "child" && (family.matches)(d))
                    .collect()
            };
            let before = emitted(&db);
            assert_eq!(
                before.len(),
                1,
                "{} under {reach:?}: the child's warning must be reported exactly once for \
                 the whole project; got {before:#?}",
                family.name
            );
            sync.project
                .set_name(&mut db)
                .to(format!("{} (renamed)", project.name));
            assert_eq!(
                emitted(&db),
                before,
                "{} under {reach:?}: an unrelated revision must reproduce the warning exactly \
                 once",
                family.name
            );
        }
    }
}

// ── one edit, one variable ─────────────────────────────────────────────

/// Editing one variable's equation re-runs that variable's parse, lowering
/// and fragment alone, and its diagnostic follows the edit while every other
/// variable's row is byte-identical.
#[test]
fn an_equation_edit_recomputes_only_the_edited_variables_diagnostic() {
    use crate::db::exec_probe::ProbedDb;

    let project = |c_equation: &str, b_equation: &str| {
        TestProject::new("edit")
            .aux("a", "1", None)
            .aux("b", b_equation, None)
            .aux("c", c_equation, None)
            .build_datamodel()
    };
    let mut probed = ProbedDb::new();
    let sync = sync_from_datamodel_incremental(probed.db_mut(), &project("2", "a + bogus"), None);
    let before = collect_all_diagnostics(probed.db(), sync.project, crate::db::LtmOverlay::Off);
    assert_eq!(
        before.len(),
        1,
        "the fixture raises one diagnostic, `b`'s unknown dependency: {before:?}"
    );

    // An unrelated edit: `c` changes, `b`'s row does not.
    probed.reset();
    let sync =
        sync_from_datamodel_incremental(probed.db_mut(), &project("3", "a + bogus"), Some(&sync));
    let after = collect_all_diagnostics(probed.db(), sync.project, crate::db::LtmOverlay::Off);
    assert_eq!(
        after, before,
        "an unrelated edit leaves `b`'s row byte-identical"
    );
    let counts = probed.counts();
    for query in [
        "parse_source_variable",
        "lowered_source_variable",
        "compile_var_fragment",
    ] {
        assert_eq!(
            counts.get(query).copied(),
            Some((1, 1)),
            "{query} must re-run for the edited variable alone; got {counts:?}"
        );
    }

    // The edit that fixes `b`: its row goes, and only `b` recompiles.
    probed.reset();
    let sync =
        sync_from_datamodel_incremental(probed.db_mut(), &project("3", "a + 1"), Some(&sync));
    let fixed = collect_all_diagnostics(probed.db(), sync.project, crate::db::LtmOverlay::Off);
    assert!(fixed.is_empty(), "fixing `b` clears its row: {fixed:?}");
    let counts = probed.counts();
    assert_eq!(
        counts.get("compile_var_fragment").copied(),
        Some((1, 1)),
        "fixing `b` recompiles `b` alone; got {counts:?}"
    );
}

// ── the two carried items ──────────────────────────────────────────────

/// A codegen refusal names the kind of expression it cannot read as an array
/// operand, never a Rust discriminant.
#[test]
fn a_codegen_refusal_names_the_expression_kind() {
    let project = TestProject::new("refusal")
        .named_dimension("region", &["north", "south"])
        .array_const("pop[region]", 10.0)
        .scalar_const("scale", 2.0)
        .array_aux("plainx[region]", "SUM(pop * scale)");
    assert_eq!(
        project.error_diagnostics(),
        vec![("main.plainx".to_string(), ErrorCode::NotSimulatable)]
    );
    let refusal = project
        .diagnostics_incremental()
        .into_iter()
        .find(|d| d.variable.as_deref() == Some("plainx"))
        .expect("plainx is refused");
    let reason = refusal
        .reason()
        .expect("a codegen refusal carries its reason");
    assert!(
        reason.contains("an arithmetic or comparison expression")
            && reason.contains("must be a variable, a subscripted array or an array temp"),
        "the refusal names the expression kind and what it needed: {reason}"
    );
    assert!(
        !reason.contains("Discriminant"),
        "a refusal never names a Rust discriminant: {reason}"
    );
}

/// When several arms of an arrayed equation fail, the arm reported is the
/// first in the dimension's declared element order -- the first arm the
/// compiler would have compiled -- on every fresh database, not whichever a
/// `HashMap` happened to iterate first.
#[test]
fn the_first_failing_element_arm_in_declared_order_is_reported() {
    // `sales + prices` mismatches over the whole 14-character expression and
    // `1 + sales + prices` over all 18, so the span says which arm was
    // reported. The rows are the three arms of the choice (`ast::typed_ast`,
    // `ast::lower_arrayed_arms`): several declared arms failing, the default
    // and an arm both failing, and failing arms naming no declared element.
    struct Arm {
        name: &'static str,
        project: fn() -> datamodel::Project,
        span: (u16, u16),
    }
    fn cities() -> TestProject {
        TestProject::new("arms")
            .named_dimension("Cities", &["Boston", "Seattle"])
            .named_dimension("Products", &["Widgets", "Gadgets"])
            .array_aux("sales[Cities]", "1")
            .array_aux("prices[Products]", "1")
    }
    let arms = [
        Arm {
            name: "Boston, the first declared element, over Seattle",
            project: || {
                cities()
                    .array_with_ranges(
                        "y[Cities]",
                        vec![
                            ("Boston", "sales + prices"),
                            ("Seattle", "1 + sales + prices"),
                        ],
                    )
                    .build_datamodel()
            },
            span: (0, 14),
        },
        Arm {
            name: "the default's error over a failing element arm's",
            project: || {
                cities()
                    .array_with_default_and_overrides(
                        "y[Cities]",
                        "1 + sales + prices",
                        vec![("Boston", "sales + prices")],
                    )
                    .build_datamodel()
            },
            span: (0, 18),
        },
        Arm {
            name: "the smallest key among arms naming no declared element",
            project: || {
                cities()
                    .array_with_ranges(
                        "y[Cities]",
                        vec![("zeta", "1 + sales + prices"), ("bogus", "sales + prices")],
                    )
                    .build_datamodel()
            },
            span: (0, 14),
        },
    ];
    for arm in &arms {
        let project = (arm.project)();
        for run in 0..16 {
            let diagnostics = all_diagnostics(&project, false);
            let rows: Vec<&Diagnostic> = diagnostics
                .iter()
                .filter(|d| {
                    d.variable.as_deref() == Some("y")
                        && d.category() == DiagnosticCategory::Equation
                })
                .collect();
            assert_eq!(
                rows.len(),
                1,
                "{} (run {run}): one row for `y`: {rows:?}",
                arm.name
            );
            assert!(
                rows[0].is(
                    DiagnosticCategory::Equation,
                    ErrorCode::MismatchedDimensions
                ),
                "{} (run {run}): {rows:?}",
                arm.name
            );
            let span = rows[0]
                .location()
                .expect("a lowering error carries its span");
            assert_eq!(
                (span.start, span.end),
                arm.span,
                "{} (run {run}): the reported arm",
                arm.name
            );
        }
    }
}

// ── a cycle's row follows its members' rows ─────────────────────────────

/// A dependency cycle's row comes after the rows of the variables in it,
/// and a cycle whose every member is itself fatal is reported.
#[test]
fn a_cycles_row_follows_its_members_rows() {
    let rows = |project: &datamodel::Project| -> Vec<(Option<String>, ErrorCode)> {
        all_diagnostics(project, false)
            .iter()
            .map(|d| (d.variable.clone(), d.code()))
            .collect()
    };
    // `a` fails at the fragment constructor and `b` is sound on its own; the
    // two are a cycle. The member's own failure is the first row (the code
    // libsimlin's `SimlinError` reports), the cycle row the last.
    let mixed = TestProject::new("m")
        .aux("a", "b + bogus", None)
        .aux("b", "a", None)
        .aux("ok", "1", None)
        .build_datamodel();
    assert_eq!(
        rows(&mixed),
        vec![
            (Some("a".to_string()), ErrorCode::UnknownDependency),
            (Some("b".to_string()), ErrorCode::CircularDependency),
        ]
    );
    // Every member fatal: the cycle is a fact of the dependency graph, not of
    // any member's fragment reaching the runlists, so it is still reported.
    let all_fatal = TestProject::new("m")
        .aux("a", "b + bogus", None)
        .aux("b", "a + bogus", None)
        .build_datamodel();
    let rows = rows(&all_fatal);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(
        rows[..2]
            .iter()
            .map(|(variable, code)| (variable.as_deref(), *code))
            .collect::<Vec<_>>(),
        vec![
            (Some("a"), ErrorCode::UnknownDependency),
            (Some("b"), ErrorCode::UnknownDependency)
        ]
    );
    assert_eq!(rows[2].1, ErrorCode::CircularDependency, "{rows:?}");
}

// ── the projections, arm by arm ────────────────────────────────────────

/// `code`, `category`, `location` and `reason` read every arm of
/// `DiagnosticError` -- its four variants with `Unit` split by `UnitError`'s
/// three. Hand-built values: the projections are pure over the typed sum, and
/// every arm's production producer is driven above.
#[test]
fn the_projections_read_every_arm() {
    use crate::builtins::Loc;
    use crate::common::{EquationError, Error, ErrorKind, UnitError};

    type Row = (
        DiagnosticError,
        ErrorCode,
        DiagnosticCategory,
        Option<Loc>,
        Option<&'static str>,
    );
    let rows: Vec<Row> = vec![
        (
            DiagnosticError::Equation(EquationError::detailed(
                ErrorCode::UnknownDependency,
                2,
                7,
                "'x' is not a variable",
            )),
            ErrorCode::UnknownDependency,
            DiagnosticCategory::Equation,
            Some(Loc::new(2, 7)),
            Some("'x' is not a variable"),
        ),
        (
            DiagnosticError::Model(Error::new(
                ErrorKind::Model,
                ErrorCode::CircularDependency,
                None,
            )),
            ErrorCode::CircularDependency,
            DiagnosticCategory::Model,
            None,
            None,
        ),
        (
            DiagnosticError::Unit(UnitError::DefinitionError(EquationError::detailed(
                ErrorCode::ExtraToken,
                1,
                4,
                "in units 'a b'",
            ))),
            ErrorCode::ExtraToken,
            DiagnosticCategory::UnitDefinition,
            Some(Loc::new(1, 4)),
            Some("in units 'a b'"),
        ),
        (
            DiagnosticError::Unit(UnitError::ConsistencyError(
                ErrorCode::UnitMismatch,
                Loc::new(3, 9),
                Some("kg vs m".to_string()),
            )),
            ErrorCode::UnitMismatch,
            DiagnosticCategory::UnitConsistency,
            Some(Loc::new(3, 9)),
            Some("kg vs m"),
        ),
        (
            DiagnosticError::Unit(UnitError::InferenceError {
                code: ErrorCode::UnitMismatch,
                sources: vec![
                    ("a".to_string(), Some(Loc::new(0, 1))),
                    ("b".to_string(), None),
                ],
                details: Some("1 == kg".to_string()),
            }),
            ErrorCode::UnitMismatch,
            DiagnosticCategory::UnitInference,
            Some(Loc::new(0, 1)),
            Some("1 == kg"),
        ),
        (
            DiagnosticError::Assembly("refused".to_string()),
            ErrorCode::NotSimulatable,
            DiagnosticCategory::Assembly,
            None,
            Some("refused"),
        ),
    ];
    for (error, code, category, location, reason) in rows {
        let diagnostic = Diagnostic {
            model: "m".to_string(),
            variable: Some("v".to_string()),
            owner: None,
            severity: DiagnosticSeverity::Error,
            error: error.clone(),
        };
        assert_eq!(diagnostic.code(), code, "{error:?}");
        assert_eq!(diagnostic.category(), category, "{error:?}");
        assert_eq!(diagnostic.location(), location, "{error:?}");
        assert_eq!(diagnostic.reason(), reason, "{error:?}");
        assert!(diagnostic.is(category, code));
    }
}
