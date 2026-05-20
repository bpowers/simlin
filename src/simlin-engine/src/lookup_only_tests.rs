// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
// This is a test orchestrator: it builds datamodel projects (data), drives the
// salsa incremental compile + the bytecode VM (I/O-ish orchestration), and
// asserts the resulting saved series. The pure logic under test lives in
// `compiler::Var::new` / `compiler::is_lookup_only`.

//! Standalone graphical-function ("lookup-only") variable saved-value
//! semantics (#590).
//!
//! A *lookup-only* variable is one whose entire equation is an inline
//! graphical function with no functional argument (e.g. Vensim `g( (x,y)... )`,
//! imported as `equation = LOOKUP_SENTINEL` + a `gf`). These tests pin that
//! such a variable produces the same saved series across the scalar, arrayed
//! (`Equation::Arrayed`), and apply-to-all shapes, and that the fix does not
//! perturb a *applied* lookup (`out = LOOKUP(g, idx)`).
//!
//! ## Phase 1 determination: the standalone index is `gf(Time)`
//!
//! Genuine Vensim evaluates a standalone lookup-only variable's table at the
//! current simulation time -- `gf(Time)` -- NOT at a constant `0` (the engine's
//! prior scalar behavior) and NOT as a literal `0` (the engine's prior
//! arrayed/A2A behavior). This was confirmed empirically: applied consumers of
//! C-LEARN's lookup-only tables byte-match `gf(Time)` against `Ref.vdf` (the
//! table y-values stepped by year, with clamp-to-endpoint outside the
//! x-domain), and the codebase's own `MdlEquation::Implicit` arm
//! (`mdl/convert/variables.rs`) already lowers an unspecified-input gf to
//! `equation = "TIME"`. So `index_expr = Expr::App(BuiltinFn::Time, loc)`.
//!
//! Each fixture below keys its table(s) on `x = [2000, 2001, 2002]` and runs
//! the sim over `INITIAL TIME = 2000 .. FINAL TIME = 2002` at `dt = 1`, so
//! `gf(Time)` yields the table's y-values step by step: `[y@2000, y@2001,
//! y@2002]`. The y-values are chosen distinct and element-identifying so a
//! wrong index (a constant `gf(0)` clamps below the x-domain to `y@2000`,
//! giving a flat `[y@2000, y@2000, y@2000]`) or a zeroed series (literal `0`)
//! is unambiguously detectable.

use crate::datamodel;
use crate::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
use crate::mdl::LOOKUP_SENTINEL;
use crate::vm::Vm;

/// A 3-point continuous graphical function keyed on `x = [2000, 2001, 2002]`.
/// Evaluated at integer years 2000..=2002 it returns `y2000`, `y2001`, `y2002`
/// exactly (an exact x-match in `vm::lookup`), and clamps to `y2000` / `y2002`
/// outside the x-domain.
fn year_gf(y2000: f64, y2001: f64, y2002: f64) -> datamodel::GraphicalFunction {
    let ys = vec![y2000, y2001, y2002];
    let y_min = ys.iter().cloned().fold(f64::INFINITY, f64::min);
    let y_max = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![2000.0, 2001.0, 2002.0]),
        y_points: ys,
        x_scale: datamodel::GraphicalFunctionScale {
            min: 2000.0,
            max: 2002.0,
        },
        y_scale: datamodel::GraphicalFunctionScale {
            min: y_min,
            max: y_max,
        },
    }
}

/// Sim window aligned to the `year_gf` x-domain: three save steps at
/// `Time = 2000, 2001, 2002` (dt = 1), so `gf(Time)` walks the table's
/// y-values one per step.
fn year_specs() -> datamodel::SimSpecs {
    datamodel::SimSpecs {
        start: 2000.0,
        stop: 2002.0,
        dt: datamodel::Dt::Dt(1.0),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    }
}

fn aux_lookup_only(ident: &str, gf: datamodel::GraphicalFunction) -> datamodel::Variable {
    datamodel::Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        // Standalone lookup-only: the sentinel equation (no functional input)
        // plus a variable-level graphical function.
        equation: datamodel::Equation::Scalar(LOOKUP_SENTINEL.to_string()),
        documentation: String::new(),
        units: None,
        gf: Some(gf),
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

fn project_with(
    name: &str,
    dimensions: Vec<datamodel::Dimension>,
    variables: Vec<datamodel::Variable>,
) -> datamodel::Project {
    datamodel::Project {
        name: name.to_string(),
        sim_specs: year_specs(),
        dimensions,
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables,
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// Compile + run a project via the incremental salsa path and return every
/// variable's full saved series keyed by (canonicalized) name -- including
/// arrayed per-element keys `name[elem]`.
fn run_series(project: &datamodel::Project) -> std::collections::HashMap<String, Vec<f64>> {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .unwrap_or_else(|e| panic!("lookup-only project should compile: {e:?}"));
    let mut vm = Vm::new(compiled).expect("VM creation should succeed");
    vm.run_to_end().expect("VM run should succeed");
    let results = vm.into_results();
    crate::test_common::collect_results(&results)
}

fn series_of<'a>(series: &'a std::collections::HashMap<String, Vec<f64>>, key: &str) -> &'a [f64] {
    series
        .get(key)
        .unwrap_or_else(|| panic!("missing series for {key}; have {:?}", series.keys()))
        .as_slice()
}

fn assert_series_eq(actual: &[f64], expected: &[f64], what: &str) {
    assert_eq!(
        actual.len(),
        expected.len(),
        "{what}: step count mismatch (got {actual:?}, want {expected:?})"
    );
    for (i, (a, e)) in actual.iter().zip(expected.iter()).enumerate() {
        assert!(
            (a - e).abs() < 1e-9,
            "{what}: step {i} got {a}, want {e} (full got {actual:?}, want {expected:?})"
        );
    }
}

/// AC1.1: a SCALAR lookup-only variable is evaluated at `gf(Time)`, producing
/// the per-year table values `[y@2000, y@2001, y@2002]` -- NOT a constant
/// `gf(0)` (which clamps below the x-domain to a flat `[y@2000, y@2000,
/// y@2000]`).
#[test]
fn scalar_lookup_only_evaluates_at_time() {
    let project = project_with(
        "scalar_lookup_only",
        vec![],
        vec![aux_lookup_only("g", year_gf(11.0, 22.0, 33.0))],
    );

    let series = run_series(&project);
    assert_series_eq(
        series_of(&series, "g"),
        &[11.0, 22.0, 33.0],
        "scalar lookup-only g must evaluate at gf(Time), not a constant gf(0) \
         (which would be a flat [11, 11, 11])",
    );
}

/// AC1.2 + AC1.5: an ARRAYED (`Equation::Arrayed`) lookup-only variable, with a
/// NON-alphabetical declared element order, evaluates each element's OWN table
/// at `gf(Time)` (per-element series), NOT a literal `0`. The `elems` Vec is
/// given in alphabetical order (the order the MDL importer's sort produces)
/// while the dimension is declared `Z, A, M`, so a positional table mis-map
/// would swap the per-element series -- pinning the per-element-GF layout
/// invariant for the standalone case too.
#[test]
fn arrayed_lookup_only_non_sorted_order_evaluates_own_table_at_time() {
    // Declared order Z, A, M (NOT alphabetical). Each element's table carries
    // an element-identifying triple:
    //   Z -> [100, 200, 300], A -> [1000, 2000, 3000], M -> [10, 20, 30].
    // `Equation::Arrayed` element tuples: (element name, equation, EXCEPT
    // default, per-element gf). `ElementName` is a `String` alias.
    let arrayed_elements: Vec<(
        String,
        String,
        Option<String>,
        Option<datamodel::GraphicalFunction>,
    )> = vec![
        // elems Vec in ALPHABETICAL order (A, M, Z), each lookup-only.
        (
            "A".to_string(),
            LOOKUP_SENTINEL.to_string(),
            None,
            Some(year_gf(1000.0, 2000.0, 3000.0)),
        ),
        (
            "M".to_string(),
            LOOKUP_SENTINEL.to_string(),
            None,
            Some(year_gf(10.0, 20.0, 30.0)),
        ),
        (
            "Z".to_string(),
            LOOKUP_SENTINEL.to_string(),
            None,
            Some(year_gf(100.0, 200.0, 300.0)),
        ),
    ];

    let g = datamodel::Variable::Aux(datamodel::Aux {
        ident: "g".to_string(),
        equation: datamodel::Equation::Arrayed(
            vec!["Dim".to_string()],
            arrayed_elements,
            None,
            false,
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });

    let project = project_with(
        "arrayed_lookup_only",
        vec![datamodel::Dimension::named(
            "Dim".to_string(),
            vec!["Z".to_string(), "A".to_string(), "M".to_string()],
        )],
        vec![g],
    );

    let series = run_series(&project);
    // Each element reads its OWN table (declared index Z=0, A=1, M=2) at
    // gf(Time). A literal-0 lowering would give [0, 0, 0]; a positional
    // mis-map would swap Z and A's series.
    assert_series_eq(
        series_of(&series, "g[z]"),
        &[100.0, 200.0, 300.0],
        "arrayed lookup-only element Z (declared index 0) at gf(Time)",
    );
    assert_series_eq(
        series_of(&series, "g[a]"),
        &[1000.0, 2000.0, 3000.0],
        "arrayed lookup-only element A (declared index 1) at gf(Time)",
    );
    assert_series_eq(
        series_of(&series, "g[m]"),
        &[10.0, 20.0, 30.0],
        "arrayed lookup-only element M (declared index 2) at gf(Time)",
    );
}

/// AC1.3: an apply-to-all lookup-only variable (one variable-level table shared
/// by every element) evaluates that single shared table at `gf(Time)` for every
/// element, NOT a literal `0`. Critically, every element must read the BASE
/// table offset (table_count == 1); a per-element `off + i` would push the VM's
/// Lookup bounds check out of range for i > 0 and return NaN.
#[test]
fn a2a_lookup_only_shares_one_table_at_time() {
    let g = datamodel::Variable::Aux(datamodel::Aux {
        ident: "g".to_string(),
        equation: datamodel::Equation::ApplyToAll(
            vec!["Dim".to_string()],
            LOOKUP_SENTINEL.to_string(),
        ),
        documentation: String::new(),
        units: None,
        // One variable-level GF, shared by every element.
        gf: Some(year_gf(7.0, 8.0, 9.0)),
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });

    let project = project_with(
        "a2a_lookup_only",
        vec![datamodel::Dimension::named(
            "Dim".to_string(),
            vec!["P".to_string(), "Q".to_string(), "R".to_string()],
        )],
        vec![g],
    );

    let series = run_series(&project);
    for elem in ["p", "q", "r"] {
        assert_series_eq(
            series_of(&series, &format!("g[{elem}]")),
            &[7.0, 8.0, 9.0],
            &format!(
                "A2A lookup-only element {elem} must read the single shared table at \
                 gf(Time) (a literal 0 -> [0,0,0]; an off+i bounds overflow -> NaN)"
            ),
        );
    }
}

/// AC1.2 (layout robustness): an ARRAYED (`Equation::Arrayed`) lookup-only
/// variable that carries a SINGLE *variable-level* gf (no per-element gfs) must
/// have every element read that one shared table at `gf(Time)`.
///
/// This is the symmetric twin of the A2A bounds hazard. `variable.rs::build_tables`
/// only produces a per-element table layout (`tables.len() == n`) when at least
/// one element carries its OWN gf; an `Equation::Arrayed` with empty/sentinel
/// element equations and a single *variable-level* gf falls through to the
/// shared-table branch (`tables.len() == 1`), exactly like A2A. So the arrayed
/// lookup-only lowering must wrap the BASE offset (`elem_off = 0`,
/// `table_count = 1`) here, NOT `off + i` -- a per-element `off + i` would make
/// `element_offset == i >= 1 == table_count` for i > 0 and the VM's `Lookup`
/// opcode would push NaN for every element after the first.
///
/// The MDL/XMILE importers never emit this exact shape (they attach per-element
/// gfs to an arrayed lookup), but `datamodel::Project` is a public compile input
/// (protobuf/JSON/serde/MCP/pysimlin/libsimlin all preserve it verbatim), so a
/// directly-constructed project can reach it and must compile correctly.
#[test]
fn arrayed_lookup_only_variable_level_gf_shares_one_table_at_time() {
    // `Equation::Arrayed` with empty/sentinel element equations and NO
    // per-element gfs, plus a single variable-level gf shared by every element.
    let arrayed_elements: Vec<(
        String,
        String,
        Option<String>,
        Option<datamodel::GraphicalFunction>,
    )> = vec![
        ("P".to_string(), LOOKUP_SENTINEL.to_string(), None, None),
        ("Q".to_string(), LOOKUP_SENTINEL.to_string(), None, None),
        ("R".to_string(), LOOKUP_SENTINEL.to_string(), None, None),
    ];

    let g = datamodel::Variable::Aux(datamodel::Aux {
        ident: "g".to_string(),
        equation: datamodel::Equation::Arrayed(
            vec!["Dim".to_string()],
            arrayed_elements,
            None,
            false,
        ),
        documentation: String::new(),
        units: None,
        // One variable-level GF, shared by every element (tables.len() == 1).
        gf: Some(year_gf(7.0, 8.0, 9.0)),
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });

    let project = project_with(
        "arrayed_lookup_only_variable_level_gf",
        vec![datamodel::Dimension::named(
            "Dim".to_string(),
            vec!["P".to_string(), "Q".to_string(), "R".to_string()],
        )],
        vec![g],
    );

    let series = run_series(&project);
    for elem in ["p", "q", "r"] {
        assert_series_eq(
            series_of(&series, &format!("g[{elem}]")),
            &[7.0, 8.0, 9.0],
            &format!(
                "arrayed lookup-only element {elem} with a single variable-level gf must \
                 read the one shared table at gf(Time) (an off+i bounds overflow -> NaN \
                 for every element after the first)"
            ),
        );
    }
}

/// AC1.4 (no regression): a graphical-function variable that is *applied* with
/// an argument elsewhere (`out = LOOKUP(g, idx)` where `idx` is a real input
/// distinct from Time) still produces the applied value `gf(idx)`. The
/// standalone-lookup fix only changes which expression `g` itself is evaluated
/// at; it must not perturb the applied path. We assert BOTH halves on one
/// model: `out` follows `idx` (constant 2001 -> a flat `gf(2001) = 22`), while
/// the standalone `g` follows `gf(Time)` (`[11, 22, 33]`).
#[test]
fn applied_lookup_only_uses_argument_not_time_no_regression() {
    let g = aux_lookup_only("g", year_gf(11.0, 22.0, 33.0));
    // `idx` is a real input distinct from Time: a constant 2001. So
    // out = LOOKUP(g, 2001) = gf(2001) = 22 at every step.
    let idx = datamodel::Variable::Aux(datamodel::Aux {
        ident: "idx".to_string(),
        equation: datamodel::Equation::Scalar("2001".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });
    let out = datamodel::Variable::Aux(datamodel::Aux {
        ident: "out".to_string(),
        equation: datamodel::Equation::Scalar("LOOKUP(g, idx)".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });

    let project = project_with("applied_lookup_only", vec![], vec![g, idx, out]);

    let series = run_series(&project);
    assert_series_eq(
        series_of(&series, "out"),
        &[22.0, 22.0, 22.0],
        "applied LOOKUP(g, idx) must follow idx (gf(2001) = 22), not Time -- \
         the standalone fix must not perturb the applied path",
    );
    assert_series_eq(
        series_of(&series, "g"),
        &[11.0, 22.0, 33.0],
        "standalone g still evaluates at gf(Time) even when applied elsewhere",
    );
}
