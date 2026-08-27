// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The `variable_dimensions` decision, arm by arm.
//!
//! `variable_dimensions` derives a variable's declared dimensions from its
//! `datamodel::Equation` instead of demanding a parse. The rows here are
//! derived from the enumeration that decision ranges over -- the three
//! `datamodel::Equation` variants, crossed with the two ways resolution can
//! fail (an unresolvable dimension name, and an equation that does not parse),
//! plus the `Module` kind, which carries an equation but has no array shape.
//!
//! Every row states what the PARSE-backed implementation answered, because
//! this query is a behavioural mirror of it in all but one cell, and that cell
//! is the reason the file exists: an A2A variable whose equation does not
//! parse reported no dimensions and now reports its declared ones. A test that
//! covered only the healthy rows would pass under an implementation that got
//! that cell wrong in either direction.

use super::*;
use crate::datamodel;

fn dims() -> Vec<datamodel::Dimension> {
    vec![
        datamodel::Dimension::named(
            "DimA".to_string(),
            vec!["a1".to_string(), "a2".to_string(), "a3".to_string()],
        ),
        datamodel::Dimension::named("DimB".to_string(), vec!["b1".to_string(), "b2".to_string()]),
    ]
}

fn aux(ident: &str, equation: datamodel::Equation) -> datamodel::Variable {
    datamodel::Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation,
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

fn arrayed(dim_names: &[&str], elements: &[(&str, &str)]) -> datamodel::Equation {
    datamodel::Equation::Arrayed(
        dim_names.iter().map(|d| d.to_string()).collect(),
        elements
            .iter()
            .map(|(e, eqn)| (e.to_string(), eqn.to_string(), None, None))
            .collect(),
        None,
        false,
    )
}

fn a2a(dim_names: &[&str], eqn: &str) -> datamodel::Equation {
    datamodel::Equation::ApplyToAll(
        dim_names.iter().map(|d| d.to_string()).collect(),
        eqn.to_string(),
    )
}

/// The implementation `variable_dimensions` replaced, kept verbatim as the
/// ORACLE: parse the variable through the production query and read the shape
/// off the resulting `Ast`.
///
/// Asserting against this rather than against hand-written expectations is
/// what makes the agreement claim mean anything. Writing the rows out by hand
/// got the cased-dimension row wrong in the first draft of this file -- the
/// parse's pre-filter seeds `expanded` with the equation's RAW dimension names
/// and then filters `project_datamodel_dims` by display name, so a reference
/// spelled `dima` against a dimension declared `DimA` never reaches
/// `variable::get_dimensions`' canonical matching and resolves to nothing on
/// BOTH paths. That is a property of the shared narrowing, not of either
/// implementation, and only an oracle catches it.
fn oracle_dimension_names(db: &dyn Db, var: SourceVariable, project: SourceProject) -> Vec<String> {
    let parsed = parse_source_variable(db, var, project);
    match parsed.variable.get_dimensions() {
        Some(dims) => dims.iter().map(|d| d.name().to_string()).collect(),
        None => Vec::new(),
    }
}

/// Every arm of the enumeration, checked against the parse-backed oracle.
///
/// The rows are the three `datamodel::Equation` variants crossed with the two
/// ways resolution can fail, plus the spellings that exercise the shared
/// narrowing. `broken_a2a` is the one row the two implementations are expected
/// to DISAGREE on and is asserted separately below; every other row must agree
/// with the oracle exactly.
#[test]
fn variable_dimensions_matches_the_parse_on_every_agreeing_arm() {
    let variables = vec![
        // Scalar: no declared dimensions.
        aux("scalar", datamodel::Equation::Scalar("1 + 1".to_string())),
        // A2A, one and two dimensions, resolvable and parseable.
        aux("a2a_1d", a2a(&["DimA"], "1")),
        aux("a2a_2d", a2a(&["DimA", "DimB"], "1")),
        // A2A naming a dimension the project does not declare.
        aux("a2a_bad_dim", a2a(&["NoSuchDim"], "1")),
        // Arrayed, resolvable and parseable.
        aux(
            "arrayed_ok",
            arrayed(&["DimB"], &[("b1", "1"), ("b2", "2")]),
        ),
        // Arrayed naming a dimension the project does not declare.
        aux("arrayed_bad_dim", arrayed(&["NoSuchDim"], &[("b1", "1")])),
        // Arrayed whose element equations do not parse: the parse builds the
        // `Ast::Arrayed` anyway once its dims resolve, dropping the elements.
        aux(
            "arrayed_bad_eqn",
            arrayed(&["DimB"], &[("b1", "1 +"), ("b2", ")(")]),
        ),
        // A reference spelled canonically against a dimension declared with
        // original casing. Both paths share the raw-name pre-filter, so both
        // resolve nothing -- the row exists to hold that agreement, not to
        // claim the resolution succeeds.
        aux("a2a_cased", a2a(&["dima"], "1")),
    ];

    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "vardims".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: dims(),
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: variables.clone(),
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };
    let state = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync = state.to_sync_result();
    let model = &sync.models["main"];

    let mut checked = 0usize;
    let mut idents: Vec<&String> = model.variables.keys().collect();
    idents.sort_unstable();
    for ident in idents {
        let sv = model.variables[ident].source;
        let derived: Vec<String> = crate::db::query::variable_dimensions(&db, sv, sync.project)
            .iter()
            .map(|d| d.name().to_string())
            .collect();
        let oracle = oracle_dimension_names(&db, sv, sync.project);
        assert_eq!(
            derived, oracle,
            "variable_dimensions disagrees with the parse for {ident}"
        );
        checked += 1;
    }
    assert_eq!(
        checked,
        variables.len(),
        "every declared fixture variable must have been compared"
    );
}

/// The two VALID shapes whose A2A equation is empty, which is the whole reason
/// the derivation gates on the equation having tokens.
///
/// Both are legal and both COMPILE, so a divergence here is not confined to
/// broken projects the way the unparseable arm below is -- it would widen
/// `variable_size` from 1 to the declared extent and shift every later
/// variable's layout offset on a working model. `parse_equation` builds an A2A
/// as `ast.map(|ast| ApplyToAll(dims, ast))` and `parser::parse` answers
/// `Ok(None)` for a token-free input, so the parse reports no dimensions for
/// them; the derivation must agree, and is asserted against the oracle rather
/// than against a hand-written expectation.
///
/// Enumerated from that MECHANISM rather than from the two shapes: any
/// `Equation::ApplyToAll` with a token-free equation reaches it. These two are
/// the ones `variable.rs`'s empty-equation suppression makes valid -- a
/// standalone lookup-only table (issue #606) and a module input port whose own
/// equation is dead -- but a third would be covered by the same gate.
#[test]
fn an_empty_a2a_equation_reports_no_dimensions_on_both_paths() {
    let lookup_only = datamodel::Variable::Aux(datamodel::Aux {
        ident: "a2a_table".to_string(),
        equation: a2a(&["DimA"], ""),
        documentation: String::new(),
        units: None,
        gf: Some(datamodel::GraphicalFunction {
            kind: datamodel::GraphicalFunctionKind::Continuous,
            x_points: None,
            y_points: vec![0.0, 1.0],
            x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
            y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        }),
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });
    let mut port_aux = datamodel::Aux {
        ident: "a2a_port".to_string(),
        // Comment-only, not merely blank: `parse`'s own contract says
        // `Ok(None)` covers "empty or comment-only input", and the lexer skips
        // a `{...}` comment rather than emitting a token for it. A gate written
        // as `eqn.trim().is_empty()` would answer differently here, which is
        // why the predicate is a lex.
        equation: a2a(&["DimA"], "{just a comment}"),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    };
    port_aux.compat.can_be_module_input = true;

    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "empty_a2a".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: dims(),
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![lookup_only, datamodel::Variable::Aux(port_aux)],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };
    let state = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync = state.to_sync_result();
    for name in ["a2a_table", "a2a_port"] {
        let sv = sync.models["main"].variables[name].source;
        let derived: Vec<String> = crate::db::query::variable_dimensions(&db, sv, sync.project)
            .iter()
            .map(|d| d.name().to_string())
            .collect();
        assert_eq!(
            derived,
            oracle_dimension_names(&db, sv, sync.project),
            "{name}: the derivation must agree with the parse on an empty A2A equation"
        );
        assert_eq!(
            derived,
            Vec::<String>::new(),
            "{name}: an empty A2A equation yields no Ast and hence no dimensions"
        );
        assert_eq!(
            crate::db::query::variable_size(&db, sv, sync.project),
            1,
            "{name}: reporting the declared extent here would shift every later \
             variable's layout offset"
        );
    }
    // The layout is the thing the divergence was observable in, so pin it too.
    assert_eq!(
        crate::db::compute_layout(&db, sync.models["main"].source, sync.project).n_slots,
        2,
        "two size-1 variables occupy two slots; the pre-gate derivation made this 6"
    );
}

/// The ONE arm that changed, pinned in the direction it changed to.
///
/// The parse builds `Ast::ApplyToAll` as `ast.map(|ast| ApplyToAll(dims, ast))`,
/// so an unparseable A2A equation yielded no `Ast` and hence no dimensions --
/// which gave the variable a `variable_size` of 1 despite being declared over
/// a 3-element dimension. The derivation reports the declared shape.
///
/// This is only reachable on a project that already fails to assemble (the
/// parse error still reaches `compile_var_fragment`, which drops the fragment
/// and accumulates the diagnostic), so no compiling model can observe it. The
/// assertion below is the record of that decision; a future change that wants
/// the old answer must restate it here rather than silently flip it.
#[test]
fn an_unparseable_a2a_equation_reports_its_declared_dimensions() {
    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "vardims_diverge".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: dims(),
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![aux("broken", a2a(&["DimA"], "1 +"))],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };
    let state = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync = state.to_sync_result();
    let sv = sync.models["main"].variables["broken"].source;

    let derived: Vec<String> = crate::db::query::variable_dimensions(&db, sv, sync.project)
        .iter()
        .map(|d| d.name().to_string())
        .collect();
    assert_eq!(
        derived,
        vec!["dima".to_string()],
        "an A2A variable's declared shape is a property of its declaration, \
         not of whether its equation parses"
    );
    // Both halves are asserted so the divergence is a recorded decision rather
    // than a coincidence: the parse really did answer differently here.
    assert_eq!(
        oracle_dimension_names(&db, sv, sync.project),
        Vec::<String>::new(),
        "the parse-backed oracle is expected to answer with no dimensions here"
    );
    assert_eq!(
        crate::db::query::variable_size(&db, sv, sync.project),
        3,
        "the declared extent follows the declared shape"
    );
}

/// The same fixture still fails to compile, which is what confines the arm
/// above to projects that were already rejected.
#[test]
fn an_unparseable_a2a_equation_still_fails_to_compile() {
    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "vardims_broken".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: dims(),
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![aux("broken", a2a(&["DimA"], "1 +"))],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    };
    let state = sync_from_datamodel_incremental(&mut db, &project, None);
    let diagnostics = collect_all_diagnostics(&db, state.project);
    assert!(
        diagnostics
            .iter()
            .any(|d| d.variable.as_deref() == Some("broken")),
        "the parse error must still be reported: {diagnostics:?}"
    );
}

/// A module variable carries a synthesized equation but has no array shape of
/// its own -- the parse's `Variable::Module` has no `ast` for `get_dimensions`
/// to read, so it answered `None`. Derived from the equation alone this needs
/// an explicit kind check, which is why it is a row rather than a corollary.
#[test]
fn a_module_variable_reports_no_dimensions() {
    let mut db = SimlinDb::default();
    let project = datamodel::Project {
        name: "vardims_module".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: dims(),
        units: vec![],
        models: vec![
            datamodel::Model {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![
                    aux("driver", datamodel::Equation::Scalar("3".to_string())),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: "inst".to_string(),
                        model_name: "sub".to_string(),
                        documentation: String::new(),
                        units: None,
                        references: vec![datamodel::ModuleReference {
                            src: "driver".to_string(),
                            dst: "inst.input".to_string(),
                        }],
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
            datamodel::Model {
                name: "sub".to_string(),
                sim_specs: None,
                variables: vec![
                    aux("input", datamodel::Equation::Scalar("0".to_string())),
                    aux("out", datamodel::Equation::Scalar("input * 2".to_string())),
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
                macro_spec: None,
            },
        ],
        source: None,
        ai_information: None,
    };
    let state = sync_from_datamodel_incremental(&mut db, &project, None);
    let sync = state.to_sync_result();
    let inst = sync.models["main"].variables["inst"].source;
    assert!(
        crate::db::query::variable_dimensions(&db, inst, sync.project).is_empty(),
        "a module instance has no array shape of its own"
    );
}
