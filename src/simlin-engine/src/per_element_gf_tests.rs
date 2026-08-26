// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests pinning the per-element graphical-function (arrayed GF) layout
//! invariant: element `e`'s lookup table must live at `e`'s *declared
//! dimension index* in the compiled `graphical_functions` run, NOT at its
//! position in the `Equation::Arrayed` `elems` Vec.
//!
//! The runtime selects a per-element table by the flat array offset
//! (`base_gf + element_offset`, `vm.rs` `Lookup`/`LookupArray`), where
//! `element_offset` is the row-major declared-dimension index. So the compiled
//! table layout must be keyed by element name -> dimension index. The MDL
//! importer sorts arrayed-equation elements alphabetically
//! (`mdl/convert/variables.rs`), so for a dimension whose declared order is not
//! alphabetical (e.g. C-LEARN's `COP`), a positional layout feeds every element
//! the wrong table. These tests use deliberately non-sorted declared orders so
//! a positional mis-map is observable (existing arrayed-GF tests all declare
//! elements in sorted order, where position == dim-index, and assert only
//! order-invariant sums).

use crate::datamodel;
use crate::db::{
    SimlinDb, compile_project_incremental, extract_tables_from_source_var,
    sync_from_datamodel_incremental,
};
use crate::vm::Vm;

/// A monotone-increasing two-point GF whose two y-values are
/// `(base, base + slope)` over x in [0, 1]. Evaluated at x = `time` (with
/// `time` running 0..=1 at dt=1) it yields `base` at t=0 and `base + slope`
/// at t=1, so each element's output is an identity-revealing constant pair.
fn ramp_gf(base: f64, slope: f64) -> datamodel::GraphicalFunction {
    datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 1.0]),
        y_points: vec![base, base + slope],
        x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        y_scale: datamodel::GraphicalFunctionScale {
            min: base.min(base + slope),
            max: base.max(base + slope),
        },
    }
}

fn one_step_specs() -> datamodel::SimSpecs {
    datamodel::SimSpecs {
        start: 0.0,
        stop: 1.0,
        dt: datamodel::Dt::Dt(1.0),
        save_step: None,
        sim_method: datamodel::SimMethod::Euler,
        time_units: None,
    }
}

/// Build the per-element-GF table-holder aux `g[Dim]` (mirroring C-LEARN's
/// `UN population HIGH LOOKUP[COP]`: a pure table-holder whose equation is
/// `time` and which carries one GF per element) plus the consumer apply-to-all
/// `out[Dim] = g[Dim](drive)` that calls each element's table (mirroring
/// C-LEARN's `UN population HIGH[COP] = UN population HIGH LOOKUP[COP](
/// Time/One year)`). `dim_elements` is the *declared* dimension order; `elems`
/// is the `(element_name, gf)` list in whatever order the caller wants the
/// `Equation::Arrayed` `elems` Vec to be in (independent of the declared
/// order) -- so a positional table layout is observable.
fn arrayed_gf_project(
    dim_name: &str,
    dim_elements: &[&str],
    elems: Vec<(&str, Option<datamodel::GraphicalFunction>)>,
) -> datamodel::Project {
    arrayed_gf_project_with_elem_eqn(dim_name, dim_elements, elems, "time")
}

/// Like [`arrayed_gf_project`] but with a caller-chosen per-element equation
/// text. `""` models what a gf-only XMILE `<element>` (no `<eqn>` child, GH
/// #907) imports as: an empty per-element equation alongside the element's gf.
fn arrayed_gf_project_with_elem_eqn(
    dim_name: &str,
    dim_elements: &[&str],
    elems: Vec<(&str, Option<datamodel::GraphicalFunction>)>,
    elem_eqn: &str,
) -> datamodel::Project {
    let arrayed_elements: Vec<(
        String,
        String,
        Option<String>,
        Option<datamodel::GraphicalFunction>,
    )> = elems
        .into_iter()
        .map(|(name, gf)| (name.to_string(), elem_eqn.to_string(), None, gf))
        .collect();

    datamodel::Project {
        name: "per_element_gf".to_string(),
        sim_specs: one_step_specs(),
        dimensions: vec![datamodel::Dimension::named(
            dim_name.to_string(),
            dim_elements.iter().map(|s| s.to_string()).collect(),
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "g".to_string(),
                    equation: datamodel::Equation::Arrayed(
                        vec![dim_name.to_string()],
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
                }),
                // The consumer: each element calls its OWN table at x=time.
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "out".to_string(),
                    equation: datamodel::Equation::ApplyToAll(
                        vec![dim_name.to_string()],
                        format!("LOOKUP(g[{dim_name}], time)"),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// Run an arrayed-GF project and return the t=1 value of each named element of
/// `out` (keyed `out[<elem>]`, canonicalized) -- the result of applying each
/// element's own per-element table.
fn simulate_out(project: &datamodel::Project) -> std::collections::HashMap<String, f64> {
    simulate_final_values(project, "out")
}

/// Run a project and return the t=1 (final) value of each per-element key of
/// `var` (keyed `<var>[<elem>]`, canonicalized).
fn simulate_final_values(
    project: &datamodel::Project,
    var: &str,
) -> std::collections::HashMap<String, f64> {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .unwrap_or_else(|e| panic!("arrayed-GF project should compile: {e:?}"));
    let mut vm = Vm::new(compiled).expect("VM creation should succeed");
    vm.run_to_end().expect("VM run should succeed");
    let results = vm.into_results();
    let series = crate::test_common::collect_results(&results);
    let prefix = format!("{var}[");
    series
        .iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(k, v)| (k.clone(), *v.last().expect("at least one save step")))
        .collect()
}

/// TEST 1 (core RED->GREEN): a dimension declared in NON-alphabetical order
/// (`Z, A, M`) whose per-element GF `elems` Vec is in a DIFFERENT (alphabetical
/// `A, M, Z`) order. Each element's table returns a distinct identifying
/// constant at t=1. Each element must evaluate ITS OWN table.
///
/// Before the fix the tables are placed positionally (Vec order `A, M, Z`), so
/// runtime element offset 0 (declared `Z`) reads `A`'s table, etc. -- a
/// permutation. After the fix element `Z`'s table lands at `Z`'s declared
/// dimension index 0, regardless of the `elems` Vec order.
#[test]
fn non_sorted_declared_order_arrayed_gf_evaluates_own_table() {
    // Declared order Z, A, M (NOT alphabetical). t=1 value for each element is
    // base + slope = the element-identifying constant.
    //   Z -> 1000, A -> 2000, M -> 3000
    // The elems Vec is in ALPHABETICAL order (A, M, Z), matching what the MDL
    // importer's sort would produce -- so a positional layout permutes them.
    let project = arrayed_gf_project(
        "Dim",
        &["Z", "A", "M"],
        vec![
            ("A", Some(ramp_gf(0.0, 2000.0))),
            ("M", Some(ramp_gf(0.0, 3000.0))),
            ("Z", Some(ramp_gf(0.0, 1000.0))),
        ],
    );

    let values = simulate_out(&project);

    let get = |elem: &str| {
        *values
            .get(&format!("out[{elem}]"))
            .unwrap_or_else(|| panic!("missing out[{elem}]; have {:?}", values.keys()))
    };

    assert!(
        (get("z") - 1000.0).abs() < 1e-9,
        "element Z (declared index 0) must read its OWN table (1000), got {} \
         -- a positional layout would read A's table (2000)",
        get("z")
    );
    assert!(
        (get("a") - 2000.0).abs() < 1e-9,
        "element A (declared index 1) must read its OWN table (2000), got {}",
        get("a")
    );
    assert!(
        (get("m") - 3000.0).abs() < 1e-9,
        "element M (declared index 2) must read its OWN table (3000), got {}",
        get("m")
    );
}

/// TEST 4 (sorted-order IDENTITY pin): a sorted declared order (`A, B, C`)
/// arrayed GF where position == dimension index. The fix must be the identity
/// permutation here -- this asserts INDIVIDUAL per-element outputs and must
/// stay byte-identical before and after the fix (it passes both ways). It is
/// the GREEN companion to the non-sorted RED tests, guarding against the fix
/// breaking the common (already-correct) sorted case.
#[test]
fn sorted_declared_order_arrayed_gf_is_identity() {
    // Declared and Vec order both alphabetical (A, B, C); position == index.
    //   A -> 100, B -> 200, C -> 300
    let project = arrayed_gf_project(
        "Dim",
        &["A", "B", "C"],
        vec![
            ("A", Some(ramp_gf(0.0, 100.0))),
            ("B", Some(ramp_gf(0.0, 200.0))),
            ("C", Some(ramp_gf(0.0, 300.0))),
        ],
    );

    let values = simulate_out(&project);
    let get = |elem: &str| {
        *values
            .get(&format!("out[{elem}]"))
            .unwrap_or_else(|| panic!("missing out[{elem}]; have {:?}", values.keys()))
    };

    assert!(
        (get("a") - 100.0).abs() < 1e-9,
        "A -> 100, got {}",
        get("a")
    );
    assert!(
        (get("b") - 200.0).abs() < 1e-9,
        "B -> 200, got {}",
        get("b")
    );
    assert!(
        (get("c") - 300.0).abs() < 1e-9,
        "C -> 300, got {}",
        get("c")
    );
}

/// TEST 5 (sparse + non-sorted combination): a non-alphabetical declared order
/// (`Z, A, M`) where the GF-MISSING element (`A`, declared index 1) is NOT at
/// the alphabetical-first position. The empty placeholder must land at the
/// MISSING element's dimension index (1, i.e. `A`), and the present tables at
/// THEIRS (`Z`->index 0, `M`->index 2). A positional layout would put the
/// placeholder at the wrong slot AND permute the present tables.
///
/// `A` has no GF, so `LOOKUP(g[A], time)` reads an empty placeholder table ->
/// NaN. `Z` and `M` evaluate their own tables.
#[test]
fn sparse_non_sorted_placeholder_lands_at_missing_element_index() {
    // Declared order Z, A, M. A is missing its GF. Vec order alphabetical.
    //   Z -> 1000, A -> (no GF -> NaN), M -> 3000
    let project = arrayed_gf_project(
        "Dim",
        &["Z", "A", "M"],
        vec![
            ("A", None),
            ("M", Some(ramp_gf(0.0, 3000.0))),
            ("Z", Some(ramp_gf(0.0, 1000.0))),
        ],
    );

    let values = simulate_out(&project);
    let get = |elem: &str| {
        *values
            .get(&format!("out[{elem}]"))
            .unwrap_or_else(|| panic!("missing out[{elem}]; have {:?}", values.keys()))
    };

    assert!(
        (get("z") - 1000.0).abs() < 1e-9,
        "Z (index 0) reads its own table (1000), got {}",
        get("z")
    );
    assert!(
        get("a").is_nan(),
        "A (index 1) is the GF-missing element -> empty placeholder -> NaN, got {}",
        get("a")
    );
    assert!(
        (get("m") - 3000.0).abs() < 1e-9,
        "M (index 2) reads its own table (3000), got {}",
        get("m")
    );
}

/// TEST 8 (compile-time / perf-regression structural assertion): the
/// per-element table reorder MUST happen at COMPILE time, and the VM hot-path
/// opcode (`Lookup` / `LookupArray`) carries only `(base_gf, table_count[,
/// mode, write_temp_id])` -- NO element-name field. So the reorder cannot add
/// a per-step name lookup in the VM. This pins both halves of the contract:
///   1. the compiled `graphical_functions` run for `g` is laid out in DECLARED
///      dimension order (Z's table at base+0, A's at base+1, M's at base+2),
///      not `elems` Vec order -- the reorder is materialized at compile time;
///   2. the consumer's lookup opcode is a plain `Lookup`/`LookupArray` with no
///      name-bearing field (a compile error here would catch any future opcode
///      that smuggled an element name into the hot path).
#[test]
fn per_element_gf_reorder_is_compile_time_with_nameless_opcode() {
    use crate::bytecode::Opcode;

    // Non-sorted declared order Z, A, M; Vec order alphabetical A, M, Z.
    //   Z -> table y=[0,1000], A -> [0,2000], M -> [0,3000]
    let project = arrayed_gf_project(
        "Dim",
        &["Z", "A", "M"],
        vec![
            ("A", Some(ramp_gf(0.0, 2000.0))),
            ("M", Some(ramp_gf(0.0, 3000.0))),
            ("Z", Some(ramp_gf(0.0, 1000.0))),
        ],
    );

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("arrayed-GF project should compile");

    let root = compiled
        .modules
        .get(&compiled.root)
        .expect("root module present");

    // --- Part 1: compile-time layout in DECLARED order ---------------------
    // `g`'s three per-element tables occupy a contiguous run of
    // `graphical_functions`. Each is a 2-point table whose second y-value
    // identifies the element (Z=1000, A=2000, M=3000). Find the run by its
    // identifying y-values and assert the DECLARED-order placement
    // [1000, 2000, 3000] (Z, A, M), not the Vec-order [2000, 3000, 1000].
    let gfs = &root.context.graphical_functions;
    let identifying_y: Vec<f64> = gfs
        .iter()
        .filter_map(|t| t.last().map(|(_, y)| *y))
        .filter(|y| *y >= 1000.0)
        .collect();
    assert_eq!(
        identifying_y,
        vec![1000.0, 2000.0, 3000.0],
        "per-element GF tables must be laid out in DECLARED dimension order \
         (Z=1000, A=2000, M=3000) at compile time, not Equation::Arrayed Vec \
         order (A=2000, M=3000, Z=1000); got {identifying_y:?}"
    );

    // --- Part 2: the VM hot-path opcode carries no element name ------------
    // The consumer `out[Dim] = LOOKUP(g[Dim], time)` must compile to a
    // Lookup/LookupArray opcode. Exhaustively destructuring it here is the
    // structural guard: the opcode's fields are exactly (base_gf, table_count,
    // mode[, write_temp_id]) -- if a future change added an element-name
    // field, this match arm would fail to compile, catching a per-step VM
    // name lookup. The match yields the opcode's `base_gf` (a value, not a
    // bool, so it is not a `matches!`-collapsible predicate) so the
    // destructuring is load-bearing.
    let lookup_base_gfs: Vec<_> = root
        .compiled_flows
        .code
        .iter()
        .chain(root.compiled_stocks.code.iter())
        .filter_map(|op| match op {
            Opcode::Lookup {
                base_gf,
                table_count: _,
                mode: _,
            } => Some(*base_gf),
            Opcode::LookupArray {
                base_gf,
                table_count: _,
                mode: _,
                write_temp_id: _,
            } => Some(*base_gf),
            // The constant-element form belongs to the same family and makes
            // this claim stronger, not weaker: it resolves the element offset
            // at COMPILE time, so the hot path does not even push it.
            Opcode::LookupDirect {
                base_gf,
                elem: _,
                mode: _,
            } => Some(*base_gf),
            _ => None,
        })
        .collect();
    assert!(
        !lookup_base_gfs.is_empty(),
        "the consumer must emit at least one nameless Lookup/LookupArray opcode \
         (the reorder is purely compile-time; the hot path does no name lookup)"
    );
}

/// Build a project exercising the `LookupArray` opcode path (the C-LEARN
/// `SUM(RS_X[COP!](...))` / `VECTOR SELECT(..., RS_CO2_FF[COP!](...), ...)`
/// shape): the per-element-GF holder `g[Dim]` plus a SCALAR consumer
/// `total = VECTOR SELECT(sel[*], LOOKUP(g[*], time), 0, 0, 0)` that selects a
/// single element via `sel` (a 0/1 per-element mask). Because exactly one
/// element is selected, `total` equals THAT element's own table value -- so
/// picking a NON-first declared element makes a positional table mis-map
/// observable (a plain SUM would be order-invariant and useless here).
fn arrayed_gf_vector_select_project(
    dim_name: &str,
    dim_elements: &[&str],
    elems: Vec<(&str, Option<datamodel::GraphicalFunction>)>,
    sel_mask: &[&str],
) -> datamodel::Project {
    let arrayed_elements: Vec<(
        String,
        String,
        Option<String>,
        Option<datamodel::GraphicalFunction>,
    )> = elems
        .into_iter()
        .map(|(name, gf)| (name.to_string(), "time".to_string(), None, gf))
        .collect();

    datamodel::Project {
        name: "per_element_gf_vs".to_string(),
        sim_specs: one_step_specs(),
        dimensions: vec![datamodel::Dimension::named(
            dim_name.to_string(),
            dim_elements.iter().map(|s| s.to_string()).collect(),
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "g".to_string(),
                    equation: datamodel::Equation::Arrayed(
                        vec![dim_name.to_string()],
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
                }),
                // sel mask: one 0/1 per-element constant, in DECLARED order
                // (an Arrayed equation, matching how Vensim's `sel[D]=1,0,1`
                // imports). Pairing each mask value with its declared element
                // name keeps the mask aligned to the dimension index.
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "sel".to_string(),
                    equation: datamodel::Equation::Arrayed(
                        vec![dim_name.to_string()],
                        dim_elements
                            .iter()
                            .zip(sel_mask.iter())
                            .map(|(elem, m)| (elem.to_string(), m.to_string(), None, None))
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
                }),
                // The C-LEARN-canonical form of `VECTOR SELECT(sel[Dim!],
                // g[Dim!](time), ...)`: the `!` iterator and call-syntax desugar
                // (as the MDL importer does) to `*` wildcards and an explicit
                // `LOOKUP(g[*], time)` -- a per-element lookup array that the
                // vector op consumes via the `LookupArray` opcode.
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "total".to_string(),
                    equation: datamodel::Equation::Scalar(
                        "VECTOR SELECT(sel[*], LOOKUP(g[*], time), 0, 0, 0)".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// TEST 3 (LookupArray-path variant): the C-LEARN-relevant per-element-GF
/// inside an array-producing builtin (`VECTOR SELECT`). Non-sorted declared
/// order (`Z, A, M`); `sel` selects ONLY `M` (declared index 2). `total` must
/// equal M's own table value (3000), proving the LookupArray opcode reads M's
/// table at M's dimension index. A positional layout would feed M's slot
/// (index 2) the third Vec entry's table (`Z` -> 1000), so `total` would be
/// 1000.
#[test]
fn non_sorted_declared_order_lookup_array_selects_own_table() {
    // Declared Z, A, M; Vec alphabetical A, M, Z. sel picks M (index 2).
    //   Z -> 1000, A -> 2000, M -> 3000; select M => total == 3000.
    let project = arrayed_gf_vector_select_project(
        "Dim",
        &["Z", "A", "M"],
        vec![
            ("A", Some(ramp_gf(0.0, 2000.0))),
            ("M", Some(ramp_gf(0.0, 3000.0))),
            ("Z", Some(ramp_gf(0.0, 1000.0))),
        ],
        // mask in DECLARED order: Z=0, A=0, M=1
        &["0", "0", "1"],
    );

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("VECTOR SELECT over per-element GF should compile");
    let mut vm = Vm::new(compiled).expect("VM creation should succeed");
    vm.run_to_end().expect("VM run should succeed");
    let results = vm.into_results();
    let series = crate::test_common::collect_results(&results);
    let total = *series
        .get("total")
        .unwrap_or_else(|| panic!("total not in results; have {:?}", series.keys()))
        .last()
        .expect("at least one save step");

    assert!(
        (total - 3000.0).abs() < 1e-9,
        "VECTOR SELECT of element M (declared index 2) must read M's OWN table \
         (3000), got {total} -- a positional layout would read the 3rd Vec \
         entry (Z's table, 1000)"
    );
}

/// TEST 2 (MDL twin -- the importer path end-to-end): the C-LEARN structure
/// (`UN population HIGH LOOKUP[COP]` per-element GF holder + `UN population
/// HIGH[COP] = ...LOOKUP[COP](Time/One year)` consumer) declared via Vensim
/// MDL with a dimension whose declared order is NON-alphabetical. The MDL
/// importer SORTS the GF `elems` Vec alphabetically (`mdl/convert/variables.rs`
/// `elements.sort_by`) but PRESERVES the declared dimension order, so this
/// exercises the full importer -> compile -> simulate path for the mis-map.
///
/// `COP: B_third, A_first, M_second` declared; tables `A_first->2000`,
/// `B_third->1000`, `M_second->3000` keyed by element. Each `out[elem]` must
/// equal its OWN element's table value. A positional layout reads the
/// alphabetically-sorted Vec entry at each declared index.
#[test]
fn mdl_twin_non_sorted_declared_order_per_element_gf() {
    // Declared COP order: B_third (idx 0), A_first (idx 1), M_second (idx 2).
    // Per-element tables (y at x=1): A_first=2000, B_third=1000, M_second=3000.
    // MDL sorts the GF elems alphabetically -> Vec order A_first, B_third,
    // M_second -> a positional layout would map declared idx 0 (B_third) to
    // A_first's table (2000), etc.
    let mdl = "{UTF-8}
COP: B_third, A_first, M_second ~~|
g[A_first]( (0,0),(1,2000) ) ~~|
g[B_third]( (0,0),(1,1000) ) ~~|
g[M_second]( (0,0),(1,3000) ) ~~|
out[COP] = LOOKUP(g[COP], Time) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 1 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let project = crate::open_vensim(mdl).expect("MDL should parse to a datamodel project");
    let values = simulate_out(&project);
    let get = |elem: &str| {
        *values
            .get(&format!("out[{elem}]"))
            .unwrap_or_else(|| panic!("missing out[{elem}]; have {:?}", values.keys()))
    };

    assert!(
        (get("b_third") - 1000.0).abs() < 1e-9,
        "B_third (declared idx 0) must read its OWN table (1000), got {} \
         -- the importer sorts the GF Vec, so a positional layout reads \
         A_first's table (2000)",
        get("b_third")
    );
    assert!(
        (get("a_first") - 2000.0).abs() < 1e-9,
        "A_first (declared idx 1) must read its OWN table (2000), got {}",
        get("a_first")
    );
    assert!(
        (get("m_second") - 3000.0).abs() < 1e-9,
        "M_second (declared idx 2) must read its OWN table (3000), got {}",
        get("m_second")
    );
}

/// TEST 6 (2-D multi-dim, non-sorted axis -- pins the row-major-flatten
/// caveat): a per-element GF over `[D1, D2]` where D1's declared order is
/// NON-alphabetical (`Y, X`). The flat element offset is row-major across BOTH
/// axes (`offset = idx(D1)*|D2| + idx(D2)`), so the reorder must flatten over
/// every variable's dimension, not just one axis. Each (D1,D2) cell's table
/// returns a distinct constant; each `out[d1,d2]` must read ITS OWN cell.
///
/// Declared D1 = [Y, X] (Y idx0, X idx1), D2 = [P, Q] (P idx0, Q idx1).
/// Cells (y at x=1): (Y,P)=10, (Y,Q)=20, (X,P)=30, (X,Q)=40.
/// Row-major DECLARED flat layout: [(Y,P), (Y,Q), (X,P), (X,Q)] = [10,20,30,40].
/// The `elems` Vec is given in ALPHABETICAL-by-key order (x,p / x,q / y,p /
/// y,q -> [30,40,10,20]) -- the order the MDL importer's sort would produce --
/// so it DIFFERS from the row-major declared layout and a positional layout
/// permutes the cells (declared offset 0 (Y,P) would read X,P's cell -> 30).
#[test]
fn two_dim_non_sorted_axis_per_element_gf_row_major_flatten() {
    // Vec in alphabetical-by-"d1,d2"-key order (NOT row-major declared order).
    let elems: Vec<(&str, datamodel::GraphicalFunction)> = vec![
        ("X,P", ramp_gf(0.0, 30.0)),
        ("X,Q", ramp_gf(0.0, 40.0)),
        ("Y,P", ramp_gf(0.0, 10.0)),
        ("Y,Q", ramp_gf(0.0, 20.0)),
    ];
    let arrayed_elements: Vec<(
        String,
        String,
        Option<String>,
        Option<datamodel::GraphicalFunction>,
    )> = elems
        .into_iter()
        .map(|(name, gf)| (name.to_string(), "time".to_string(), None, Some(gf)))
        .collect();

    let project = datamodel::Project {
        name: "per_element_gf_2d".to_string(),
        sim_specs: one_step_specs(),
        dimensions: vec![
            // D1 declared NON-alphabetically: Y (idx 0), X (idx 1).
            datamodel::Dimension::named("D1".to_string(), vec!["Y".to_string(), "X".to_string()]),
            datamodel::Dimension::named("D2".to_string(), vec!["P".to_string(), "Q".to_string()]),
        ],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "g".to_string(),
                    equation: datamodel::Equation::Arrayed(
                        vec!["D1".to_string(), "D2".to_string()],
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
                }),
                datamodel::Variable::Aux(datamodel::Aux {
                    ident: "out".to_string(),
                    equation: datamodel::Equation::ApplyToAll(
                        vec!["D1".to_string(), "D2".to_string()],
                        "LOOKUP(g[D1, D2], time)".to_string(),
                    ),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: None,
                    compat: datamodel::Compat::default(),
                }),
            ],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("2-D arrayed-GF project should compile");
    let mut vm = Vm::new(compiled).expect("VM creation should succeed");
    vm.run_to_end().expect("VM run should succeed");
    let results = vm.into_results();
    let series = crate::test_common::collect_results(&results);
    let get = |d1: &str, d2: &str| {
        let key = format!("out[{d1},{d2}]");
        *series
            .get(&key)
            .unwrap_or_else(|| panic!("missing {key}; have {:?}", series.keys()))
            .last()
            .expect("at least one save step")
    };

    assert!(
        (get("y", "p") - 10.0).abs() < 1e-9,
        "(Y,P) flat offset 0 must read its OWN cell (10), got {} -- a \
         positional layout reads the alphabetically-first Vec cell (X,P -> 30)",
        get("y", "p")
    );
    assert!(
        (get("y", "q") - 20.0).abs() < 1e-9,
        "(Y,Q) flat offset 1 must read its OWN cell (20), got {}",
        get("y", "q")
    );
    assert!(
        (get("x", "p") - 30.0).abs() < 1e-9,
        "(X,P) flat offset 2 must read its OWN cell (30), got {}",
        get("x", "p")
    );
    assert!(
        (get("x", "q") - 40.0).abs() < 1e-9,
        "(X,Q) flat offset 3 must read its OWN cell (40), got {}",
        get("x", "q")
    );
}

/// GH #907 semantic pin: a gf-only XMILE `<element>` (a `<gf>` with no
/// `<eqn>`) imports as an EMPTY per-element equation. When EVERY element
/// equation is empty and there is no EXCEPT default, the variable is a pure
/// per-element table holder -- lookup-only (`var_is_lookup_only`, #606): it
/// is excluded from the results (it produces NO series of its own), and each
/// element's table is reachable from consumers via `LOOKUP(g[elem], x)`.
/// This is the arrayed analogue of a whole-variable empty-eqn-with-gf static
/// table, and is what makes an eqn-less `<element>` useful rather than inert.
#[test]
fn gf_only_elements_form_a_lookup_only_table_holder() {
    // Same non-sorted declared order as the value-bearing tests (Z, A, M with
    // an alphabetical elems Vec) so the declared-order table layout is also
    // covered on the lookup-only path.
    let project = arrayed_gf_project_with_elem_eqn(
        "Dim",
        &["Z", "A", "M"],
        vec![
            ("A", Some(ramp_gf(0.0, 2000.0))),
            ("M", Some(ramp_gf(0.0, 3000.0))),
            ("Z", Some(ramp_gf(0.0, 1000.0))),
        ],
        "",
    );

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .unwrap_or_else(|e| panic!("a gf-only table holder must compile: {e:?}"));
    let mut vm = Vm::new(compiled).expect("VM creation should succeed");
    vm.run_to_end().expect("VM run should succeed");
    let results = vm.into_results();
    let series = crate::test_common::collect_results(&results);

    // the table holder itself produces no series (it is not value-bearing)
    assert!(
        !series.keys().any(|k| k == "g" || k.starts_with("g[")),
        "a lookup-only table holder must be excluded from results; have {:?}",
        series.keys()
    );

    // each consumer element reads its OWN element's table at x=time
    let get = |elem: &str| {
        *series
            .get(&format!("out[{elem}]"))
            .unwrap_or_else(|| panic!("missing out[{elem}]; have {:?}", series.keys()))
            .last()
            .expect("at least one save step")
    };
    assert!(
        (get("z") - 1000.0).abs() < 1e-9,
        "Z -> 1000, got {}",
        get("z")
    );
    assert!(
        (get("a") - 2000.0).abs() < 1e-9,
        "A -> 2000, got {}",
        get("a")
    );
    assert!(
        (get("m") - 3000.0).abs() < 1e-9,
        "M -> 3000, got {}",
        get("m")
    );
}

/// GH #909: a VALUE-BEARING arrayed variable (real per-element input
/// equations) carrying per-element gfs is the arrayed WITH LOOKUP shape --
/// Stella evaluates each element to `gf_elem(input)`, applying each element's
/// OWN table to that element's input equation. The engine must auto-apply the
/// gf, exactly as it does for the scalar WITH LOOKUP wrap. Uses the same
/// non-sorted declared order (Z, A, M; alphabetical elems Vec) as the layout
/// tests so a positional table mis-map in the wrap would also be caught.
#[test]
fn value_bearing_per_element_gf_applies_to_own_equation() {
    // Each element's input equation is `time` (== 1 at t=1); each element's
    // own table maps 1 -> its identifying constant (Z=1000, A=2000, M=3000).
    let project = arrayed_gf_project(
        "Dim",
        &["Z", "A", "M"],
        vec![
            ("A", Some(ramp_gf(0.0, 2000.0))),
            ("M", Some(ramp_gf(0.0, 3000.0))),
            ("Z", Some(ramp_gf(0.0, 1000.0))),
        ],
    );

    let values = simulate_final_values(&project, "g");
    let get = |elem: &str| {
        *values
            .get(&format!("g[{elem}]"))
            .unwrap_or_else(|| panic!("missing g[{elem}]; have {:?}", values.keys()))
    };

    assert!(
        (get("z") - 1000.0).abs() < 1e-9,
        "g[Z] must evaluate gf_Z(time) = 1000, got {} -- the raw input \
         equation (time = 1) means the gf was silently dropped (GH #909)",
        get("z")
    );
    assert!(
        (get("a") - 2000.0).abs() < 1e-9,
        "g[A] must evaluate gf_A(time) = 2000, got {}",
        get("a")
    );
    assert!(
        (get("m") - 3000.0).abs() < 1e-9,
        "g[M] must evaluate gf_M(time) = 3000, got {}",
        get("m")
    );
}

/// GH #909 (mixed elements): only elements that HAVE a gf get the implicit
/// WITH LOOKUP wrap; an element without a gf (an empty placeholder table in
/// the compiled layout) keeps its raw input equation -- Stella has nothing to
/// apply there. This also pins that the wrap keys tables by the element's
/// DECLARED dimension index, not elems Vec position: with declared order
/// (Z, A, M) and A missing its gf, a positional wrap would feed Z or M the
/// empty placeholder.
#[test]
fn value_bearing_mixed_elements_apply_gf_only_where_present() {
    let project = arrayed_gf_project(
        "Dim",
        &["Z", "A", "M"],
        vec![
            ("A", None),
            ("M", Some(ramp_gf(0.0, 3000.0))),
            ("Z", Some(ramp_gf(0.0, 1000.0))),
        ],
    );

    let values = simulate_final_values(&project, "g");
    let get = |elem: &str| {
        *values
            .get(&format!("g[{elem}]"))
            .unwrap_or_else(|| panic!("missing g[{elem}]; have {:?}", values.keys()))
    };

    assert!(
        (get("z") - 1000.0).abs() < 1e-9,
        "g[Z] (declared index 0) must evaluate ITS OWN gf: gf_Z(time) = 1000, got {}",
        get("z")
    );
    assert!(
        (get("a") - 1.0).abs() < 1e-9,
        "g[A] has NO gf: it must evaluate the raw input equation (time = 1), \
         NOT an empty placeholder lookup (NaN), got {}",
        get("a")
    );
    assert!(
        (get("m") - 3000.0).abs() < 1e-9,
        "g[M] (declared index 2) must evaluate ITS OWN gf: gf_M(time) = 3000, got {}",
        get("m")
    );
}

/// GH #909 (apply-to-all shape): an A2A arrayed variable with a VARIABLE-level
/// gf and a real input equation is the arrayed WITH LOOKUP with one shared
/// table -- every element must evaluate `gf(input)` against that single table
/// (`tables.len() == 1`, table index 0 for every element).
#[test]
fn a2a_variable_level_gf_applies_to_equation() {
    let project = datamodel::Project {
        name: "a2a_gf".to_string(),
        sim_specs: one_step_specs(),
        dimensions: vec![datamodel::Dimension::named(
            "Dim".to_string(),
            vec!["A".to_string(), "B".to_string()],
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "h".to_string(),
                equation: datamodel::Equation::ApplyToAll(
                    vec!["Dim".to_string()],
                    "time".to_string(),
                ),
                documentation: String::new(),
                units: None,
                gf: Some(ramp_gf(0.0, 500.0)),
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let values = simulate_final_values(&project, "h");
    for elem in ["a", "b"] {
        let v = *values
            .get(&format!("h[{elem}]"))
            .unwrap_or_else(|| panic!("missing h[{elem}]; have {:?}", values.keys()));
        assert!(
            (v - 500.0).abs() < 1e-9,
            "h[{elem}] must evaluate the shared variable-level gf at the input \
             equation: gf(time) = 500 at t=1, got {v} (GH #909)"
        );
    }
}

/// GH #909 (degenerate zero-point gf, scalar): a gf with NO points is
/// treated as ABSENT -- the variable evaluates its raw input equation. The
/// pre-#909 scalar wrap keyed only on `tables` being non-empty, so a
/// zero-point gf produced NaN (the empty-table lookup result); the
/// deliberate rule now matches the arrayed empty-placeholder handling
/// (see `apply_implicit_with_lookup`).
#[test]
fn zero_point_gf_on_scalar_keeps_raw_input_equation() {
    let empty_gf = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![]),
        y_points: vec![],
        x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
    };
    let project = datamodel::Project {
        name: "zero_point_gf".to_string(),
        sim_specs: one_step_specs(),
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "s".to_string(),
                equation: datamodel::Equation::Scalar("time".to_string()),
                documentation: String::new(),
                units: None,
                gf: Some(empty_gf),
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .unwrap_or_else(|e| panic!("zero-point-gf project should compile: {e:?}"));
    let mut vm = Vm::new(compiled).expect("VM creation should succeed");
    vm.run_to_end().expect("VM run should succeed");
    let results = vm.into_results();
    let series = crate::test_common::collect_results(&results);
    let v = *series
        .get("s")
        .unwrap_or_else(|| panic!("missing s; have {:?}", series.keys()))
        .last()
        .expect("at least one save step");
    assert!(
        (v - 1.0).abs() < 1e-9,
        "a zero-point gf is treated as absent: s must evaluate its raw input \
         equation (time = 1), NOT NaN (the empty-table lookup result), got {v}"
    );
}

/// GH #909 x GH #907 (gf-only element among real-eqn siblings, no EXCEPT
/// default): the element's MISSING input equation gets the arrayed
/// expansion's fabricated `Const(0.0)`, and the wrap applies uniformly, so
/// the element evaluates `gf(0)` (pre-#909 it evaluated the bare fabricated
/// 0). Both are silent fabrications -- Stella rejects this element shape --
/// and the zero-fill diagnostic is the open remainder of GH #905; this pins
/// the uniform-wrap choice.
#[test]
fn gf_only_element_without_default_evaluates_gf_of_fabricated_zero() {
    // Z has a real input equation (time); A is gf-only (empty eqn) with NO
    // EXCEPT default. gf_A = ramp from 100 at x=0 to 2100 at x=1, so
    // gf_A(0) = 100 -- distinguishable from both the fabricated 0 and any
    // raw-input value.
    let project = arrayed_gf_project_with_mixed_eqns(
        "Dim",
        &["Z", "A"],
        vec![
            ("Z", "time", Some(ramp_gf(0.0, 1000.0))),
            ("A", "", Some(ramp_gf(100.0, 2000.0))),
        ],
    );

    let values = simulate_final_values(&project, "g");
    let get = |elem: &str| {
        *values
            .get(&format!("g[{elem}]"))
            .unwrap_or_else(|| panic!("missing g[{elem}]; have {:?}", values.keys()))
    };

    assert!(
        (get("z") - 1000.0).abs() < 1e-9,
        "g[Z] has a real input equation: gf_Z(time) = 1000 at t=1, got {}",
        get("z")
    );
    assert!(
        (get("a") - 100.0).abs() < 1e-9,
        "g[A] is gf-only with no default: the fabricated 0 input feeds its \
         gf, so gf_A(0) = 100, got {} (0 would mean the gf was dropped; NaN \
         would mean an empty placeholder was consulted)",
        get("a")
    );
}

/// Like [`arrayed_gf_project_with_elem_eqn`] but with a PER-element equation
/// text, so gf-only (empty-eqn) elements can be mixed with real-equation
/// siblings in one variable.
fn arrayed_gf_project_with_mixed_eqns(
    dim_name: &str,
    dim_elements: &[&str],
    elems: Vec<(&str, &str, Option<datamodel::GraphicalFunction>)>,
) -> datamodel::Project {
    let arrayed_elements: Vec<(
        String,
        String,
        Option<String>,
        Option<datamodel::GraphicalFunction>,
    )> = elems
        .into_iter()
        .map(|(name, eqn, gf)| (name.to_string(), eqn.to_string(), None, gf))
        .collect();

    datamodel::Project {
        name: "per_element_gf_mixed".to_string(),
        sim_specs: one_step_specs(),
        dimensions: vec![datamodel::Dimension::named(
            dim_name.to_string(),
            dim_elements.iter().map(|s| s.to_string()).collect(),
        )],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "g".to_string(),
                equation: datamodel::Equation::Arrayed(
                    vec![dim_name.to_string()],
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
            })],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// TEST (extract_tables_from_source_var, non-sorted declared order): the
/// production salsa DEPENDENCY-table path (`extract_tables_from_source_var`,
/// db.rs, consumed via `db/var_fragment.rs` / db.rs for `LOOKUP(dep, x)`
/// dependency tables) must also lay each element's table at its DECLARED
/// dimension index, not its `Equation::Arrayed` Vec position. Declared order
/// `Z, A, M` with the GF Vec in alphabetical order; assert the compiled tables
/// land in declared order (Z=1000 at index 0, A=2000 at 1, M=3000 at 2) by
/// their identifying y-values.
#[test]
fn extract_tables_non_sorted_declared_order_lands_by_dimension_index() {
    use crate::db::sync_from_datamodel;

    let project = arrayed_gf_project(
        "Dim",
        &["Z", "A", "M"],
        vec![
            ("A", Some(ramp_gf(0.0, 2000.0))),
            ("M", Some(ramp_gf(0.0, 3000.0))),
            ("Z", Some(ramp_gf(0.0, 1000.0))),
        ],
    );

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let var = sync.models["main"].variables["g"].source;
    let tables = extract_tables_from_source_var(&db, &var, sync.project);

    assert_eq!(
        tables.len(),
        3,
        "one table per element, got {}",
        tables.len()
    );
    // identifying y-value (second point) of each table, in compiled order.
    let ys: Vec<f64> = tables
        .iter()
        .map(|t| t.data.last().map(|(_, y)| *y).unwrap_or(f64::NAN))
        .collect();
    assert_eq!(
        ys,
        vec![1000.0, 2000.0, 3000.0],
        "extract_tables_from_source_var must order tables by DECLARED dimension \
         index (Z=1000 @0, A=2000 @1, M=3000 @2), not Equation::Arrayed Vec \
         order (A=2000, M=3000, Z=1000); got {ys:?}"
    );
}

// ---------------------------------------------------------------------------
// A BARE per-element table reference -- `LOOKUP(g, time)` with no subscript on
// the arrayed table holder `g` -- resolves under pass 0 exactly as a bare
// arrayed VARIABLE reference does (`Context::make_dimension_subscripts`,
// reached through `BuiltinFn::map_ref` in `Context::lower_pass0`): an axis the
// enclosing apply-to-all equation iterates is pinned to the active element,
// and every other axis is a wildcard the consumer iterates. The rows below pin
// that rule for the table position of the LOOKUP family; the explicitly
// subscripted spelling (`LOOKUP(g[COP], time)`) is the tests above. The
// justification is structural -- one resolution rule for every bare arrayed
// reference, table or not -- and makes no claim about Vensim or Stella. The
// fixtures are Vensim-syntax lookup-only tables, the shape the MDL importer
// produces for a per-element table holder.
// ---------------------------------------------------------------------------

const BARE_TABLE_SIM_SPECS: &str =
    "INITIAL TIME = 0 ~~|\nFINAL TIME = 1 ~~|\nSAVEPER = 1 ~~|\nTIME STEP = 1 ~~|\n";

/// A 1-D per-element table `g[COP]` whose three tables read 100, 200, 300 at
/// `time = 1`, followed by the caller's consumer equation(s).
fn one_dim_table_model(consumers: &str) -> datamodel::Project {
    let body = format!(
        "COP: a, b, c ~~|\n\
         g[a]( (0,0),(1,100) ) ~~|\n\
         g[b]( (0,0),(1,200) ) ~~|\n\
         g[c]( (0,0),(1,300) ) ~~|\n\
         {consumers}"
    );
    probe_mdl(&body)
}

/// A 2-D per-element table `g[COP, ROW]` whose four tables read 10, 20, 30, 40
/// at `time = 1` (row-major: `[a,p] [a,q] [b,p] [b,q]`).
fn two_dim_table_model(consumers: &str) -> datamodel::Project {
    let body = format!(
        "COP: a, b ~~|\n\
         ROW: p, q ~~|\n\
         g[a,p]( (0,0),(1,10) ) ~~|\n\
         g[a,q]( (0,0),(1,20) ) ~~|\n\
         g[b,p]( (0,0),(1,30) ) ~~|\n\
         g[b,q]( (0,0),(1,40) ) ~~|\n\
         {consumers}"
    );
    probe_mdl(&body)
}

fn probe_mdl(body: &str) -> datamodel::Project {
    crate::open_vensim(&format!("{{UTF-8}}\n{body}{BARE_TABLE_SIM_SPECS}"))
        .expect("the probe MDL parses")
}

/// The t=1 values of `var`'s elements, in key order (`var[a]`, `var[a,p]`,
/// ...: ascending element order for these fixtures).
fn final_values_in_key_order(project: &datamodel::Project, var: &str) -> Vec<(String, f64)> {
    let values: std::collections::BTreeMap<String, f64> =
        simulate_final_values(project, var).into_iter().collect();
    values.into_iter().collect()
}

fn assert_elements(project: &datamodel::Project, var: &str, expected: &[(&str, f64)]) {
    let got = final_values_in_key_order(project, var);
    let got_view: Vec<(&str, f64)> = got.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    assert_eq!(
        got_view,
        expected.to_vec(),
        "{var}: per-element values at t=1"
    );
}

/// `out[COP] = LOOKUP(g, time)` over a 1-D table: the bare `g` is `g[COP]`, so
/// each element applies its own table.
#[test]
fn bare_one_dim_table_in_a_matching_a2a_equation_applies_each_elements_table() {
    let project = one_dim_table_model("out[COP] = LOOKUP(g, Time) ~~|\n");
    assert_elements(
        &project,
        "out",
        &[("out[a]", 100.0), ("out[b]", 200.0), ("out[c]", 300.0)],
    );
}

/// `cell[COP, ROW] = LOOKUP(g, time)` over a 2-D table: both axes are pinned by
/// the enclosing element, so each cell applies its own table.
#[test]
fn bare_two_dim_table_in_a_matching_a2a_equation_applies_each_cells_table() {
    let project = two_dim_table_model("cell[COP,ROW] = LOOKUP(g, Time) ~~|\n");
    assert_elements(
        &project,
        "cell",
        &[
            ("cell[a,p]", 10.0),
            ("cell[a,q]", 20.0),
            ("cell[b,p]", 30.0),
            ("cell[b,q]", 40.0),
        ],
    );
}

/// `out[COP] = SUM(LOOKUP(g, time))` over a 2-D table: `COP` is pinned to the
/// element and `ROW` is the wildcard the reducer iterates, so each element sums
/// ITS row (`[a]`: 10 + 20, `[b]`: 30 + 40) -- the same row a bare arrayed
/// variable `v[COP, ROW]` yields under `SUM(v)` in this equation.
#[test]
fn bare_two_dim_table_under_a_reducer_sums_the_elements_own_row() {
    let project = two_dim_table_model("out[COP] = SUM(LOOKUP(g, Time)) ~~|\n");
    assert_elements(&project, "out", &[("out[a]", 30.0), ("out[b]", 70.0)]);
}

/// An array-valued per-element table apply in a position that consumes ONE
/// value has no meaning and is REFUSED with a diagnostic attributed to the
/// variable, never emitted. The rule has one owner: codegen's `TempArray` arm
/// (`compiler::codegen::Compiler::walk_expr`) refuses a temp array outside an
/// iteration body, and every operand site inherits the refusal through `?`
/// (an `unwrap` on a missing stack value there is a process abort under
/// `panic = abort`). The rows are a sample of the consumers the apply can land
/// in -- the whole right-hand side, an `Apply` operand, an `Op2` operand, an
/// `IF` condition, an n-ary `MEAN` operand, and the per-element right-hand side
/// of an apply-to-all equation over a 2-D table (each element's apply is the
/// element's whole `ROW` row). The other consumers (an `Op1` operand, an `IF`
/// branch, a lookup index, a subscript index) reach the same arm through the
/// same `?`; a subscript index is not rowed only because Vensim has no
/// expression subscripts, so the MDL fixtures these rows share cannot spell it.
#[test]
fn array_valued_table_apply_assigned_to_one_slot_is_refused_not_aborted() {
    use crate::db::{DiagnosticError, DiagnosticSeverity, collect_all_diagnostics};

    let rows = [
        (one_dim_table_model("s = ABS(LOOKUP(g, Time)) ~~|\n"), "s"),
        (one_dim_table_model("s = LOOKUP(g, Time) ~~|\n"), "s"),
        (one_dim_table_model("s = LOOKUP(g, Time) + 1 ~~|\n"), "s"),
        (
            one_dim_table_model("s = IF THEN ELSE(LOOKUP(g, Time) > 0, 1, 0) ~~|\n"),
            "s",
        ),
        (
            one_dim_table_model("s = MEAN(LOOKUP(g, Time), 1) ~~|\n"),
            "s",
        ),
        (
            two_dim_table_model("out[COP] = LOOKUP(g, Time) ~~|\n"),
            "out",
        ),
        (
            two_dim_table_model("out[COP] = ABS(LOOKUP(g, Time)) ~~|\n"),
            "out",
        ),
    ];
    for (project, var) in rows {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &project, None);
        assert!(
            compile_project_incremental(&db, sync.project, "main").is_err(),
            "{var}: an array used as one value must not compile"
        );
        let diagnostics = collect_all_diagnostics(&db, sync.project);
        let attributed: Vec<String> = diagnostics
            .iter()
            .filter(|d| d.severity == DiagnosticSeverity::Error)
            .map(|d| format!("{:?} on {:?}", d.error, d.variable))
            .collect();
        // The refusal is the `TempArray` arm's, carried verbatim in the
        // assembly diagnostic, and lands on the variable -- not some other
        // error that happens to make the model fail to compile.
        assert!(
            diagnostics.iter().any(|d| {
                d.severity == DiagnosticSeverity::Error
                    && d.variable.as_deref() == Some(var)
                    && matches!(
                        &d.error,
                        DiagnosticError::Assembly(reason)
                            if reason.contains("where a single value is required")
                    )
            }),
            "{var}: the refusal must be attributed to the variable; errors: {attributed:?}"
        );
    }
}
