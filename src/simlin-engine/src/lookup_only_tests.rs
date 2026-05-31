// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
// This is a test orchestrator: it builds datamodel projects (data), drives the
// salsa incremental compile + the bytecode VM (I/O-ish orchestration), and
// asserts the resulting saved series. The pure logic under test lives in
// `variable::var_is_lookup_only` / `db::source_var_is_table_only`.

//! Standalone graphical-function ("lookup-only") variable semantics (#606).
//!
//! A *lookup-only* variable is one whose entire equation is a graphical function
//! with no functional argument (a `<gf>` with an empty equation, or the legacy
//! MDL `LOOKUP_SENTINEL` form). Such a variable is a **table indexed by an
//! explicit input** (`y = table(input)`), NOT a value-bearing variable: it is a
//! static table consulted by callers, so it has no time series of its own.
//!
//! These tests pin that contract:
//!   * a lookup-only variable produces **no saved series** (it is absent from
//!     the results) across the scalar, apply-to-all, and arrayed shapes;
//!   * a consumer that *calls* it with an argument (`LOOKUP(g, x)`) still reads
//!     the table at that argument -- and two calls at different arguments are
//!     independent (the table is not a single stored value);
//!   * referencing a lookup-only table *bare* (no argument) is a compile error;
//!   * the legacy `"0+0"` sentinel form is still recognized (back-compat).
//!
//! Each fixture keys its table(s) on `x = [2000, 2001, 2002]` and runs the sim
//! over `INITIAL TIME = 2000 .. FINAL TIME = 2002` at `dt = 1`, so a consumer's
//! `LOOKUP(g, Time)` yields the table's y-values step by step: `[y@2000,
//! y@2001, y@2002]`, with clamp-to-endpoint outside the x-domain.

use crate::common::ErrorCode;
use crate::datamodel;
use crate::db::{
    DiagnosticError, DiagnosticSeverity, SimlinDb, collect_all_diagnostics,
    compile_project_incremental, sync_from_datamodel, sync_from_datamodel_incremental,
};
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
/// `Time = 2000, 2001, 2002` (dt = 1), so `LOOKUP(g, Time)` walks the table's
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

/// A standalone lookup-only aux: a variable-level graphical function with the
/// canonical empty equation (no functional input). `equation_text` lets a test
/// pin the legacy `"0+0"` sentinel form instead.
fn aux_lookup_only_with_equation(
    ident: &str,
    equation_text: &str,
    gf: datamodel::GraphicalFunction,
) -> datamodel::Variable {
    datamodel::Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(equation_text.to_string()),
        documentation: String::new(),
        units: None,
        gf: Some(gf),
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

/// The canonical lookup-only form: a `<gf>` with an empty equation.
fn aux_lookup_only(ident: &str, gf: datamodel::GraphicalFunction) -> datamodel::Variable {
    aux_lookup_only_with_equation(ident, "", gf)
}

/// An ordinary scalar aux with a real equation.
fn aux(ident: &str, equation: &str) -> datamodel::Variable {
    datamodel::Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
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
/// arrayed per-element keys `name[elem]`. A lookup-only table is NOT a saved
/// variable, so it never appears as a key here.
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

/// Collect compilation diagnostics for a project (without panicking on error),
/// for tests that assert a specific compile error is raised.
fn diagnostics_of(project: &datamodel::Project) -> Vec<crate::db::Diagnostic> {
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, project);
    collect_all_diagnostics(&db, sync.project)
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

/// A standalone scalar lookup-only table produces NO saved series of its own,
/// and a consumer that calls it with an argument reads the table at that
/// argument. `out = LOOKUP(g, Time)` walks `[11, 22, 33]`; `g` itself is absent.
#[test]
fn scalar_lookup_only_has_no_series_consumer_reads_table() {
    let project = project_with(
        "scalar_lookup_only",
        vec![],
        vec![
            aux_lookup_only("g", year_gf(11.0, 22.0, 33.0)),
            aux("out", "LOOKUP(g, Time)"),
        ],
    );

    let series = run_series(&project);
    assert!(
        !series.contains_key("g"),
        "a standalone lookup-only table is not a value-bearing variable and must \
         produce no saved series; got keys {:?}",
        series.keys()
    );
    assert_series_eq(
        series_of(&series, "out"),
        &[11.0, 22.0, 33.0],
        "consumer LOOKUP(g, Time) must read the table at Time",
    );
}

/// Two calls to the same lookup-only table at different arguments are
/// independent intermediate expressions -- there is no single stored value.
/// `gap = LOOKUP(g, Time) - LOOKUP(g, Time - 1)`:
///   @2000: gf(2000) - gf(1999 clamped to 2000) = 11 - 11 = 0
///   @2001: gf(2001) - gf(2000)                 = 22 - 11 = 11
///   @2002: gf(2002) - gf(2001)                 = 33 - 22 = 11
#[test]
fn lookup_only_independent_calls_per_site() {
    let project = project_with(
        "lookup_only_gap",
        vec![],
        vec![
            aux_lookup_only("g", year_gf(11.0, 22.0, 33.0)),
            aux("gap", "LOOKUP(g, Time) - LOOKUP(g, Time - 1)"),
        ],
    );

    let series = run_series(&project);
    assert!(
        !series.contains_key("g"),
        "lookup-only table must have no series"
    );
    assert_series_eq(
        series_of(&series, "gap"),
        &[0.0, 11.0, 11.0],
        "two LOOKUP calls at different indices must evaluate independently",
    );
}

/// AC1.4 (no regression): a lookup-only table *applied* with a real argument
/// distinct from Time (`out = LOOKUP(g, idx)`, `idx = 2001`) follows that
/// argument (a flat `gf(2001) = 22`). The table `g` itself still has no series.
#[test]
fn applied_lookup_only_uses_argument_not_time() {
    let project = project_with(
        "applied_lookup_only",
        vec![],
        vec![
            aux_lookup_only("g", year_gf(11.0, 22.0, 33.0)),
            aux("idx", "2001"),
            aux("out", "LOOKUP(g, idx)"),
        ],
    );

    let series = run_series(&project);
    assert!(
        !series.contains_key("g"),
        "lookup-only table must have no series"
    );
    assert_series_eq(
        series_of(&series, "out"),
        &[22.0, 22.0, 22.0],
        "applied LOOKUP(g, idx) must follow idx (gf(2001) = 22), not Time",
    );
}

/// The legacy MDL `"0+0"` sentinel form of a lookup-only variable is still
/// recognized as a table (back-compat read-shim) and likewise produces no
/// series; its consumer still reads the table.
#[test]
fn legacy_sentinel_lookup_only_has_no_series() {
    let project = project_with(
        "legacy_sentinel_lookup_only",
        vec![],
        vec![
            aux_lookup_only_with_equation("g", LOOKUP_SENTINEL, year_gf(11.0, 22.0, 33.0)),
            aux("out", "LOOKUP(g, Time)"),
        ],
    );

    let series = run_series(&project);
    assert!(
        !series.contains_key("g"),
        "a legacy '0+0'-sentinel lookup-only table must also have no series"
    );
    assert_series_eq(
        series_of(&series, "out"),
        &[11.0, 22.0, 33.0],
        "consumer of a legacy-sentinel lookup-only table still reads it",
    );
}

/// An apply-to-all lookup-only variable (one shared variable-level table)
/// produces no per-element series. An `anchor` aux keeps the model non-degenerate.
#[test]
fn a2a_lookup_only_has_no_series() {
    let g = datamodel::Variable::Aux(datamodel::Aux {
        ident: "g".to_string(),
        equation: datamodel::Equation::ApplyToAll(vec!["Dim".to_string()], String::new()),
        documentation: String::new(),
        units: None,
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
        vec![g, aux("anchor", "1")],
    );

    let series = run_series(&project);
    for elem in ["p", "q", "r"] {
        assert!(
            !series.contains_key(&format!("g[{elem}]")),
            "A2A lookup-only element {elem} must have no series; got keys {:?}",
            series.keys()
        );
    }
    assert_series_eq(
        series_of(&series, "anchor"),
        &[1.0, 1.0, 1.0],
        "anchor present",
    );
}

/// An arrayed (`Equation::Arrayed`) lookup-only variable with per-element tables
/// produces no per-element series.
#[test]
fn arrayed_lookup_only_has_no_series() {
    let arrayed_elements: Vec<(
        String,
        String,
        Option<String>,
        Option<datamodel::GraphicalFunction>,
    )> = vec![
        (
            "A".to_string(),
            String::new(),
            None,
            Some(year_gf(1000.0, 2000.0, 3000.0)),
        ),
        (
            "M".to_string(),
            String::new(),
            None,
            Some(year_gf(10.0, 20.0, 30.0)),
        ),
        (
            "Z".to_string(),
            String::new(),
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
        vec![g, aux("anchor", "1")],
    );

    let series = run_series(&project);
    for elem in ["z", "a", "m"] {
        assert!(
            !series.contains_key(&format!("g[{elem}]")),
            "arrayed lookup-only element {elem} must have no series; got keys {:?}",
            series.keys()
        );
    }
    assert_series_eq(
        series_of(&series, "anchor"),
        &[1.0, 1.0, 1.0],
        "anchor present",
    );
}

/// An arrayed lookup-only variable carrying a single variable-level gf (shared
/// by every element) likewise produces no per-element series.
#[test]
fn arrayed_lookup_only_variable_level_gf_has_no_series() {
    let arrayed_elements: Vec<(
        String,
        String,
        Option<String>,
        Option<datamodel::GraphicalFunction>,
    )> = vec![
        ("P".to_string(), String::new(), None, None),
        ("Q".to_string(), String::new(), None, None),
        ("R".to_string(), String::new(), None, None),
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
        vec![g, aux("anchor", "1")],
    );

    let series = run_series(&project);
    for elem in ["p", "q", "r"] {
        assert!(
            !series.contains_key(&format!("g[{elem}]")),
            "arrayed (variable-level gf) lookup-only element {elem} must have no series"
        );
    }
    assert_series_eq(
        series_of(&series, "anchor"),
        &[1.0, 1.0, 1.0],
        "anchor present",
    );
}

/// Referencing a lookup-only table *bare* -- without applying it to an argument
/// -- is a compile error: a table has no scalar value of its own.
#[test]
fn bare_reference_to_lookup_only_is_compile_error() {
    let project = project_with(
        "bare_lookup_reference",
        vec![],
        vec![
            aux_lookup_only("g", year_gf(11.0, 22.0, 33.0)),
            // `y = g` references the table bare, with no argument.
            aux("y", "g"),
        ],
    );

    let diags = diagnostics_of(&project);
    let has_bare_ref_error = diags.iter().any(|d| {
        d.variable.as_deref() == Some("y")
            && matches!(
                &d.error,
                DiagnosticError::Model(crate::common::Error {
                    code: ErrorCode::LookupReferencedWithoutArgument,
                    ..
                })
            )
            && d.severity == DiagnosticSeverity::Error
    });
    assert!(
        has_bare_ref_error,
        "a bare reference to a lookup-only table must raise \
         LookupReferencedWithoutArgument for 'y'; got: {diags:?}"
    );
}
