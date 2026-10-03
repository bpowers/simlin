// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MDL writer semantic-loss + lossiness-warnings tests (#854 #856 #857 #858).
//! Split out of `writer_tests.rs` to stay under the per-file line cap (GH #645);
//! shares the `make_*` fixture helpers from the sibling `tests` module.

use super::tests::{make_aux, make_model, make_project, make_stock};
use crate::datamodel::{
    self, Aux, Compat, Equation, Flow, GraphicalFunction, GraphicalFunctionKind,
    GraphicalFunctionScale, Variable,
};
use crate::mdl::{ExportWarning, project_to_mdl_with_warnings};

fn make_flow(ident: &str, eqn: &str) -> Variable {
    Variable::Flow(Flow {
        ident: ident.to_owned(),
        equation: Equation::Scalar(eqn.to_owned()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}
/// Build a graphical function with a specific interpolation kind.
fn make_gf_kind(kind: GraphicalFunctionKind) -> GraphicalFunction {
    GraphicalFunction {
        kind,
        x_points: Some(vec![0.0, 1.0, 2.0]),
        y_points: vec![0.0, 1.0, 4.0],
        x_scale: GraphicalFunctionScale { min: 0.0, max: 2.0 },
        y_scale: GraphicalFunctionScale { min: 0.0, max: 4.0 },
    }
}

fn make_lookup_only_aux(ident: &str, kind: GraphicalFunctionKind) -> Variable {
    Variable::Aux(Aux {
        ident: ident.to_owned(),
        equation: Equation::Scalar(String::new()),
        documentation: String::new(),
        units: None,
        gf: Some(make_gf_kind(kind)),
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

fn warnings_of(project: &datamodel::Project) -> Vec<ExportWarning> {
    project_to_mdl_with_warnings(project)
        .expect("write should succeed")
        .1
}

fn message_mentioning<'a>(
    warnings: &'a [ExportWarning],
    needle: &str,
) -> Option<&'a ExportWarning> {
    warnings.iter().find(|w| w.message.contains(needle))
}

// ---- #856: warnings channel plumbing ----

#[test]
fn plain_project_to_mdl_discards_warnings() {
    // A dropped non-negative flag warns via the warnings entry point, but the
    // plain wrapper still returns just the text and never errors on it.
    let mut aux = make_aux("thing", "3", None, "");
    if let Variable::Aux(a) = &mut aux {
        a.compat.non_negative = true;
    }
    let project = make_project(vec![make_model(vec![aux])]);
    let text_only = crate::mdl::project_to_mdl(&project).expect("plain write");
    let (text_warn, warnings) = project_to_mdl_with_warnings(&project).expect("warn write");
    assert_eq!(text_only, text_warn, "warnings must not change the text");
    assert!(
        message_mentioning(&warnings, "non-negative").is_some(),
        "expected a non-negative warning, got {warnings:?}"
    );
}

#[test]
fn no_warnings_for_ordinary_model() {
    let project = make_project(vec![make_model(vec![make_aux("x", "1 + 2", None, "")])]);
    assert!(
        warnings_of(&project).is_empty(),
        "an ordinary model must export with no warnings"
    );
}

// ---- #857: stock ACTIVE INITIAL / GET DIRECT initial ----

#[test]
fn stock_active_initial_roundtrips() {
    let mut stock = make_stock("level", "base + 5", Some("widgets"), "");
    if let Variable::Stock(s) = &mut stock {
        s.inflows = vec!["inflow".to_owned()];
        s.compat.active_initial = Some("base".to_owned());
    }
    let inflow = make_aux("inflow", "1", None, "");
    let base = make_aux("base", "10", None, "");
    let project = make_project(vec![make_model(vec![stock, inflow, base])]);

    let mdl = crate::mdl::project_to_mdl(&project).expect("write");
    assert!(
        mdl.contains("ACTIVE INITIAL"),
        "stock initial should re-wrap ACTIVE INITIAL, got:\n{mdl}"
    );

    // Re-import represents ACTIVE INITIAL inline in the stock's INTEG initial
    // (as the equivalent INIT(...) form) rather than re-splitting it into
    // compat.active_initial, so the strongest round-trip check is that the
    // initial value is preserved and re-writes stably to ACTIVE INITIAL.
    let reparsed = crate::mdl::parse_mdl(&mdl).expect("reparse");
    let level = reparsed.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == "level")
        .expect("level present");
    let initial = level.get_equation().unwrap().source_text();
    assert!(
        initial.contains("base + 5") && initial.contains("base"),
        "re-imported stock must retain its ACTIVE INITIAL initial value, got: {initial}"
    );
    let mdl2 = crate::mdl::project_to_mdl(&reparsed).expect("second write");
    assert!(
        mdl2.contains("ACTIVE INITIAL"),
        "ACTIVE INITIAL must survive a full parse->write->parse->write cycle, got:\n{mdl2}"
    );
}

#[test]
fn stock_get_direct_initial_emitted() {
    let mut stock = make_stock("reservoir", "0", None, "");
    if let Variable::Stock(s) = &mut stock {
        s.inflows = vec!["fill".to_owned()];
        s.compat.data_source = Some(datamodel::DataSource {
            kind: datamodel::DataSourceKind::Constants,
            file: "data.xlsx".to_owned(),
            tab_or_delimiter: "Sheet1".to_owned(),
            row_or_col: "A".to_owned(),
            cell: String::new(),
        });
    }
    let fill = make_aux("fill", "1", None, "");
    let project = make_project(vec![make_model(vec![stock, fill])]);
    let mdl = crate::mdl::project_to_mdl(&project).expect("write");
    assert!(
        mdl.contains("INTEG(fill, GET DIRECT CONSTANTS('data.xlsx', 'Sheet1', 'A')"),
        "stock GET DIRECT initial must be reconstructed inside INTEG, got:\n{mdl}"
    );
}

// ---- #857: arrayed active_initial ----

#[test]
fn arrayed_aux_active_initial_preserved() {
    let mut aux = Variable::Aux(Aux {
        ident: "arr".to_owned(),
        equation: Equation::Arrayed(
            vec!["dima".to_owned()],
            vec![
                ("a1".to_owned(), "1".to_owned(), None, None),
                ("a2".to_owned(), "2".to_owned(), None, None),
            ],
            None,
            false,
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    if let Variable::Aux(a) = &mut aux {
        a.compat.active_initial = Some("0".to_owned());
    }
    let project = make_project(vec![make_model(vec![aux])]);
    let mdl = crate::mdl::project_to_mdl(&project).expect("write");
    let active_initial_count = mdl.matches("ACTIVE INITIAL").count();
    assert_eq!(
        active_initial_count, 2,
        "each arrayed element must keep ACTIVE INITIAL, got:\n{mdl}"
    );
}

// ---- #854: graphical-function kind ----

#[test]
fn extrapolate_lookup_roundtrips_via_tabxl() {
    // A standalone Extrapolate lookup whose only caller uses a plain LOOKUP:
    // the writer must rewrite the call to TABXL so the table re-imports as
    // Extrapolate rather than being clamped to Continuous.
    let table = make_lookup_only_aux("demand_curve", GraphicalFunctionKind::Extrapolate);
    let result = make_aux("result", "LOOKUP(demand_curve, time)", None, "");
    let project = make_project(vec![make_model(vec![table, result])]);

    let (mdl, warnings) = project_to_mdl_with_warnings(&project).expect("write");
    assert!(
        mdl.contains("TABXL(demand curve, Time)"),
        "plain LOOKUP to an Extrapolate table must be emitted as TABXL, got:\n{mdl}"
    );
    assert!(
        message_mentioning(&warnings, "demand curve").is_none(),
        "a standalone Extrapolate lookup should round-trip without warning: {warnings:?}"
    );

    let reparsed = crate::mdl::parse_mdl(&mdl).expect("reparse");
    let table = reparsed.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == "demand_curve")
        .expect("table present");
    let Variable::Aux(a) = table else {
        panic!("demand_curve should be an aux")
    };
    assert_eq!(
        a.gf.as_ref().map(|g| g.kind),
        Some(GraphicalFunctionKind::Extrapolate),
        "Extrapolate kind must survive the round trip"
    );
}

#[test]
fn unreferenced_extrapolate_lookup_warns() {
    // A standalone Extrapolate lookup with NO LOOKUP call site has no TABXL
    // emitted to mark it, so its extrapolate kind silently clamps to
    // Continuous on re-import. That loss must surface as a warning rather than
    // being suppressed on the assumption a preserving TABXL was written
    // (#854/#856; a dead/unreferenced table is the residual the referenced
    // case in `extrapolate_lookup_roundtrips_via_tabxl` does not cover).
    let table = make_lookup_only_aux("demand_curve", GraphicalFunctionKind::Extrapolate);
    let project = make_project(vec![make_model(vec![table])]);
    let warnings = warnings_of(&project);
    assert!(
        message_mentioning(&warnings, "demand curve").is_some(),
        "an unreferenced extrapolating lookup must warn, got {warnings:?}"
    );
}

#[test]
fn discrete_gf_emits_warning() {
    let table = make_lookup_only_aux("steps", GraphicalFunctionKind::Discrete);
    let project = make_project(vec![make_model(vec![table])]);
    let warnings = warnings_of(&project);
    let w = message_mentioning(&warnings, "steps").expect("discrete warning expected");
    assert!(
        w.message.contains("discrete"),
        "warning should mention discrete: {}",
        w.message
    );
}

#[test]
fn embedded_extrapolate_gf_warns() {
    // An Extrapolate gf on a WITH LOOKUP (real input equation) cannot be
    // represented and must warn.
    let mut aux = make_aux("interp", "time", None, "");
    if let Variable::Aux(a) = &mut aux {
        a.gf = Some(make_gf_kind(GraphicalFunctionKind::Extrapolate));
    }
    let project = make_project(vec![make_model(vec![aux])]);
    let warnings = warnings_of(&project);
    assert!(
        message_mentioning(&warnings, "interp").is_some(),
        "embedded Extrapolate WITH LOOKUP must warn: {warnings:?}"
    );
}

// ---- #858: EXCEPT default reconstruction ----

fn named_dim(name: &str, elems: &[&str]) -> datamodel::Dimension {
    datamodel::Dimension {
        name: name.to_owned(),
        elements: datamodel::DimensionElements::Named(
            elems.iter().map(|e| (*e).to_owned()).collect(),
        ),
        mappings: vec![],
        parent: None,
    }
}

fn project_with_dims(
    models: Vec<datamodel::Model>,
    dimensions: Vec<datamodel::Dimension>,
) -> datamodel::Project {
    let mut p = make_project(models);
    p.dimensions = dimensions;
    p
}

fn arrayed_aux(
    ident: &str,
    dims: &[&str],
    elements: Vec<(&str, &str)>,
    default: Option<&str>,
    has_except_default: bool,
) -> Variable {
    Variable::Aux(Aux {
        ident: ident.to_owned(),
        equation: Equation::Arrayed(
            dims.iter().map(|d| (*d).to_owned()).collect(),
            elements
                .into_iter()
                .map(|(k, e)| (k.to_owned(), e.to_owned(), None, None))
                .collect(),
            default.map(|d| d.to_owned()),
            has_except_default,
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

/// Extract the value text for a specific element of an arrayed aux, applying
/// the EXCEPT default fill for elements not explicitly listed.
fn arrayed_element_value(project: &datamodel::Project, var: &str, element: &str) -> Option<String> {
    let v = project.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == var)?;
    let Some(Equation::Arrayed(_, elements, default, has_except_default)) = v.get_equation() else {
        return None;
    };
    if let Some((_, eqn, _, _)) = elements
        .iter()
        .find(|(k, _, _, _)| k.eq_ignore_ascii_case(element))
    {
        return Some(eqn.clone());
    }
    if *has_except_default {
        return default.clone();
    }
    None
}

#[test]
fn except_default_fills_missing_element_and_roundtrips() {
    // s over DimA={A1,A2,A3}: A2=14, A3=13 explicit; A1 covered only by the
    // EXCEPT default (14). The prior writer dropped the default, silently
    // turning A1 into 0.
    let s = arrayed_aux(
        "s",
        &["DimA"],
        vec![("A2", "14"), ("A3", "13")],
        Some("14"),
        true,
    );
    let project = project_with_dims(
        vec![make_model(vec![s])],
        vec![named_dim("DimA", &["A1", "A2", "A3"])],
    );

    let (mdl, warnings) = project_to_mdl_with_warnings(&project).expect("write");
    assert!(
        message_mentioning(&warnings, "'s'").is_none(),
        "a reconstructable EXCEPT default should not warn: {warnings:?}"
    );

    let reparsed = crate::mdl::parse_mdl(&mdl).expect("reparse");
    assert_eq!(
        arrayed_element_value(&reparsed, "s", "A1").as_deref(),
        Some("14"),
        "the EXCEPT default must fill A1 through the round trip (mdl:\n{mdl})"
    );
    assert_eq!(
        arrayed_element_value(&reparsed, "s", "A3").as_deref(),
        Some("13")
    );

    // Idempotence: writing the re-imported model again is byte-identical.
    let mdl2 = crate::mdl::project_to_mdl(&reparsed).expect("second write");
    assert_eq!(
        mdl, mdl2,
        "EXCEPT reconstruction must be a re-import fixpoint"
    );
}

/// What a save of `project` changes, by `check_save`, and whether a second
/// save writes what the first did.
fn except_round_trip(
    project: &datamodel::Project,
) -> (Vec<crate::save_check::MeaningChange>, bool) {
    let changes = crate::save_check::check_save(project, crate::save_check::SaveFormat::Mdl)
        .expect("the check runs");
    let save1 = crate::mdl::project_to_mdl(project).expect("first write");
    let save2 =
        crate::mdl::project_to_mdl(&crate::mdl::parse_mdl(&save1).expect("reparse")).unwrap();
    (changes, save1 == save2)
}

#[test]
fn an_except_default_is_written_as_the_except_equation_it_was() {
    // Each variable is written as an :EXCEPT: equation, so the save keeps the
    // default and every element. Rows: every element explicit and one taking
    // the default; a default that names the variable's own dimension, which
    // the reader substitutes per element, beside an element that is that
    // substitution and one that is not; a default no element differs from;
    // and a default that fills elements no slot holds.
    let a = arrayed_aux(
        "a",
        &["DimA"],
        vec![("A1", "1"), ("A2", "2"), ("A3", "3")],
        None,
        false,
    );
    let z = |elements: Vec<(&str, &str)>, default: &str| {
        arrayed_aux("z", &["DimA"], elements, Some(default), true)
    };
    let rows = [
        (z(vec![("A1", "10"), ("A2", "1"), ("A3", "1")], "10"), false),
        (
            z(
                vec![("A1", "a[A1] * 2"), ("A2", "5"), ("A3", "a[A3] * 2")],
                "a[DimA] * 2",
            ),
            false,
        ),
        (z(vec![("A1", "4"), ("A2", "4"), ("A3", "4")], "4"), false),
        (z(vec![("A2", "14"), ("A3", "13")], "14"), false),
        // The reader spells a filled element's dimension reference per
        // element (`a[a1] * 2`), which means what the default does for it.
        (z(vec![("A2", "5")], "a[DimA] * 2"), true),
    ];
    for (var, respelled) in rows {
        let project = project_with_dims(
            vec![make_model(vec![a.clone(), var])],
            vec![named_dim("DimA", &["A1", "A2", "A3"])],
        );
        let (mdl, warnings) = project_to_mdl_with_warnings(&project).expect("write");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(mdl.contains(":EXCEPT:"), "{mdl}");
        let (changes, fixed_point) = except_round_trip(&project);
        assert!(fixed_point, "{mdl}");
        if respelled {
            assert!(
                !changes.is_empty()
                    && changes.iter().all(|c| {
                        c.kind == crate::save_check::ChangeKind::Structure
                            && c.reason.starts_with("'z' is computed as 'a[a")
                    }),
                "{changes:?}"
            );
        } else {
            assert!(changes.is_empty(), "{changes:?}\n{mdl}");
        }
    }
}

#[test]
fn except_default_warns_when_dimension_unknown() {
    // has_except_default with a missing element but no dimension membership
    // available: cannot reconstruct, so warn and keep explicit-only.
    let s = arrayed_aux("s", &["DimA"], vec![("A2", "14")], Some("14"), true);
    // No dimensions registered on the project.
    let project = make_project(vec![make_model(vec![s])]);
    let warnings = warnings_of(&project);
    assert!(
        message_mentioning(&warnings, "'s'").is_some(),
        "unknown dimension membership must warn: {warnings:?}"
    );
}

// ---- #856: group name/doc reread lossiness ----

#[test]
fn group_multiword_name_and_doc_warn() {
    let mut model = make_model(vec![make_aux("x", "1", None, "")]);
    model.groups = vec![datamodel::ModelGroup {
        name: "Financial Sector".to_owned(),
        doc: Some("some documentation here".to_owned()),
        parent: None,
        members: vec!["x".to_owned()],
        run_enabled: false,
    }];
    let project = make_project(vec![model]);
    let warnings = warnings_of(&project);
    assert!(
        message_mentioning(&warnings, "truncates it to its first word").is_some(),
        "multi-word group name must warn: {warnings:?}"
    );
    assert!(
        message_mentioning(&warnings, "documentation for group").is_some(),
        "group doc drop must warn: {warnings:?}"
    );
}

// ---- loop metadata dropped on export ----

/// A model's `loop_metadata` has no MDL representation -- the MDL reader
/// never produces any -- so every entry is dropped on export and must be
/// reported. Rows cover each arm of the message dispatch: a named loop, an
/// unnamed loop with a description, a hidden (deleted) marker, and a bare
/// unnamed entry (an LTM pin: `db/sync.rs` pins every non-deleted entry).
#[test]
fn loop_metadata_entries_warn_on_export() {
    let mut model = make_model(vec![
        make_stock("s", "1", None, ""),
        make_flow("f", "s * 0.1"),
    ]);
    model.loop_metadata = vec![
        datamodel::LoopMetadata {
            uids: vec![1, 2],
            deleted: false,
            name: "Growth".to_owned(),
            description: "reinforcing growth".to_owned(),
        },
        datamodel::LoopMetadata {
            uids: vec![7, 8],
            deleted: false,
            name: String::new(),
            description: "described but unnamed".to_owned(),
        },
        datamodel::LoopMetadata {
            uids: vec![3, 4],
            deleted: true,
            name: String::new(),
            description: String::new(),
        },
        datamodel::LoopMetadata {
            uids: vec![5],
            deleted: false,
            name: String::new(),
            description: String::new(),
        },
    ];
    let project = make_project(vec![model]);
    let warnings = warnings_of(&project);
    let loop_warnings: Vec<&ExportWarning> = warnings
        .iter()
        .filter(|w| w.message.contains("loop"))
        .collect();
    assert_eq!(
        loop_warnings.len(),
        4,
        "one warning per entry: {warnings:?}"
    );
    assert!(message_mentioning(&warnings, "loop name 'Growth'").is_some());
    assert!(
        message_mentioning(
            &warnings,
            "description of the unnamed loop over variable uids [7, 8]"
        )
        .is_some()
    );
    assert!(
        message_mentioning(
            &warnings,
            "hidden-loop marker for the loop over variable uids [3, 4]"
        )
        .is_some()
    );
    assert!(message_mentioning(&warnings, "pinned loop over variable uids [5]").is_some());
    assert!(
        message_mentioning(&warnings, "loop name ''").is_none(),
        "an unnamed entry must not be reported as an empty name: {warnings:?}"
    );

    let mut clean = make_model(vec![make_aux("x", "1", None, "")]);
    clean.loop_metadata = vec![];
    assert!(warnings_of(&make_project(vec![clean])).is_empty());
}

// ---- #887: conveyor/queue compat dropped on export ----

/// A conveyor block with only the required transit time set.
fn minimal_conveyor() -> datamodel::Conveyor {
    datamodel::Conveyor {
        transit_time: "4".to_owned(),
        capacity: None,
        inflow_limit: None,
        sample: None,
        arrest: None,
        discrete: false,
        batch_integrity: false,
        one_at_a_time: true,
        exponential_leak: false,
        ignore_earlier_zone_losses: false,
    }
}

#[test]
fn conveyor_stock_warns_and_still_parses() {
    let mut stock = make_stock("students", "1000", None, "");
    if let Variable::Stock(s) = &mut stock {
        s.inflows = vec!["matriculating".to_owned()];
        s.outflows = vec!["graduating".to_owned()];
        s.compat.conveyor = Some(minimal_conveyor());
    }
    let inflow = make_flow("matriculating", "250");
    let outflow = make_flow("graduating", "0");
    let project = make_project(vec![make_model(vec![stock, inflow, outflow])]);

    let (mdl, warnings) = project_to_mdl_with_warnings(&project).expect("write");
    let w = message_mentioning(&warnings, "students").expect("conveyor warning expected");
    assert!(
        w.message.contains("conveyor") && w.message.contains("INTEG"),
        "warning should say the conveyor became a plain INTEG stock: {}",
        w.message
    );
    // The degraded export must still be valid MDL.
    crate::mdl::parse_mdl(&mdl).expect("exported MDL must reparse");
}

#[test]
fn queue_stock_warns() {
    let mut stock = make_stock("backlog", "0", None, "");
    if let Variable::Stock(s) = &mut stock {
        s.inflows = vec!["arriving".to_owned()];
        s.compat.queue = Some(datamodel::Queue {});
    }
    let inflow = make_flow("arriving", "5");
    let project = make_project(vec![make_model(vec![stock, inflow])]);
    let warnings = warnings_of(&project);
    let w = message_mentioning(&warnings, "backlog").expect("queue warning expected");
    assert!(
        w.message.contains("queue") && w.message.contains("INTEG"),
        "warning should say the queue became a plain INTEG stock: {}",
        w.message
    );
}

#[test]
fn leak_flow_explicit_fraction_warns() {
    // The `<leak>0.1</leak>` encoding: the fraction rides in compat.leakage.
    let mut flow = make_flow("dropping_out", "");
    if let Variable::Flow(f) = &mut flow {
        f.compat.leakage = Some(datamodel::Leakage {
            fraction: Some("0.1".to_owned()),
            integers: false,
            zone_start: None,
            zone_end: None,
        });
    }
    let project = make_project(vec![make_model(vec![flow])]);
    let warnings = warnings_of(&project);
    let w = message_mentioning(&warnings, "dropping out").expect("leak warning expected");
    assert!(
        w.message.contains("leak"),
        "warning should name the leak marker: {}",
        w.message
    );
}

#[test]
fn leak_flow_bare_marker_warns() {
    // The bare `<leak/>` marker encoding: the fraction lives in the flow's
    // own equation and compat.leakage.fraction is None.
    let mut flow = make_flow("evaporating", "0.05");
    if let Variable::Flow(f) = &mut flow {
        f.compat.leakage = Some(datamodel::Leakage {
            fraction: None,
            integers: false,
            zone_start: None,
            zone_end: None,
        });
    }
    let project = make_project(vec![make_model(vec![flow])]);
    let warnings = warnings_of(&project);
    let w = message_mentioning(&warnings, "evaporating").expect("leak warning expected");
    assert!(
        w.message.contains("leak"),
        "warning should name the leak marker: {}",
        w.message
    );
}

#[test]
fn spreadflow_flow_warns() {
    let mut flow = make_flow("loading", "10");
    if let Variable::Flow(f) = &mut flow {
        f.compat.spreadflow = Some(datamodel::SpreadFlow::Even);
    }
    let project = make_project(vec![make_model(vec![flow])]);
    let warnings = warnings_of(&project);
    let w = message_mentioning(&warnings, "loading").expect("spreadflow warning expected");
    assert!(
        w.message.contains("placement"),
        "warning should name the inflow-placement marker: {}",
        w.message
    );
}

#[test]
fn overflow_flow_warns() {
    let mut flow = make_flow("spilling", "0");
    if let Variable::Flow(f) = &mut flow {
        f.compat.overflow = true;
    }
    let project = make_project(vec![make_model(vec![flow])]);
    let warnings = warnings_of(&project);
    let w = message_mentioning(&warnings, "spilling").expect("overflow warning expected");
    assert!(
        w.message.contains("overflow"),
        "warning should name the overflow marker: {}",
        w.message
    );
}

#[test]
fn plain_stock_and_flow_do_not_warn() {
    // A conveyor-free stock/flow pair must export with no new warnings.
    let mut stock = make_stock("level", "100", None, "");
    if let Variable::Stock(s) = &mut stock {
        s.inflows = vec!["filling".to_owned()];
    }
    let flow = make_flow("filling", "3");
    let project = make_project(vec![make_model(vec![stock, flow])]);
    assert!(
        warnings_of(&project).is_empty(),
        "an ordinary stock/flow model must export with no warnings"
    );
}

#[test]
fn single_word_group_without_doc_does_not_warn() {
    let mut model = make_model(vec![make_aux("x", "1", None, "")]);
    model.groups = vec![datamodel::ModelGroup {
        name: "Sector".to_owned(),
        doc: None,
        parent: None,
        members: vec!["x".to_owned()],
        run_enabled: false,
    }];
    let project = make_project(vec![model]);
    assert!(
        warnings_of(&project).is_empty(),
        "a single-word group with no doc round-trips losslessly"
    );
}

// ---- #912: unparseable-equation fallback ----

/// The writer's last-resort fallback emits the raw XMILE equation text when
/// `Expr0::new` cannot parse it. XMILE syntax is not MDL syntax, so that text
/// means something different on re-import (the builtin-rename table -- `int` ->
/// `INTEGER`, `smth1` -> `SMOOTH` -- is exactly what gets skipped, and Vensim
/// reads an unknown call as a lookup of an undefined table). The writer cannot
/// vouch for it, so it must say so rather than degrade silently.
#[test]
fn unparseable_equation_warns_and_names_the_variable() {
    let project = make_project(vec![make_model(vec![make_aux("target", "1 +", None, "")])]);
    let warnings = warnings_of(&project);
    let w = message_mentioning(&warnings, "'target'").expect("unparseable-equation warning");
    assert!(
        w.message.contains("1 +"),
        "warning should quote the offending equation: {}",
        w.message
    );
    assert!(
        w.message.contains("could not be parsed"),
        "warning should say the equation could not be parsed: {}",
        w.message
    );
}

/// Warnings are a side channel: the emitted text is byte-identical with and
/// without them, so this cannot perturb the corpus round-trip ratchets.
#[test]
fn unparseable_equation_warning_does_not_change_emitted_text() {
    let project = make_project(vec![make_model(vec![make_aux("target", "1 +", None, "")])]);
    let text_only = crate::mdl::project_to_mdl(&project).expect("plain write");
    let (text_warn, warnings) = project_to_mdl_with_warnings(&project).expect("warn write");
    assert_eq!(text_only, text_warn);
    assert!(!warnings.is_empty());
}

/// A stacked unary minus is what the MDL importer produces for the legal Vensim
/// `x = - -3` (#912). Once the equation grammar accepts it, the writer parses it
/// like any other equation -- no fallback, no warning, and `INTEGER` is properly
/// restored from the engine's `trunc`.
#[test]
fn stacked_unary_equation_parses_and_does_not_warn() {
    let project = make_project(vec![make_model(vec![make_aux(
        "target",
        "--(0 ^ trunc(0))",
        None,
        "",
    )])]);
    let (text, warnings) = project_to_mdl_with_warnings(&project).expect("write");
    assert!(
        warnings.is_empty(),
        "a parseable equation must not warn: {warnings:?}"
    );
    assert!(
        text.contains("INTEGER(0)"),
        "the builtin-rename table must have run (no raw-text fallback):\n{text}"
    );
    assert!(
        !text.contains("TRUNC(0)"),
        "the engine's `trunc` must not leak into the MDL:\n{text}"
    );
}

/// Opaque data equations (GET DIRECT DATA, GET XLS, ...) are unparseable *by
/// design* -- the writer emits them verbatim and that is correct, so they take
/// the early-return path and must not warn.
#[test]
fn data_equation_does_not_warn() {
    let project = make_project(vec![make_model(vec![make_aux(
        "imported",
        "{GET DIRECT DATA('data.csv', ',', 'A', '2')}",
        None,
        "",
    )])]);
    assert!(
        warnings_of(&project).is_empty(),
        "a GET DIRECT data equation is opaque by design and must not warn"
    );
}

/// The warning fires for every equation-bearing shape the writer renders, not
/// just the scalar aux path: a stock's INITIAL, an arrayed element equation.
#[test]
fn unparseable_equation_warns_from_stock_and_arrayed_paths() {
    let mut stock = make_stock("level", "1 +", None, "");
    if let Variable::Stock(s) = &mut stock {
        s.inflows = vec!["filling".to_owned()];
    }
    let flow = make_flow("filling", "3");
    let project = make_project(vec![make_model(vec![stock, flow])]);
    assert!(
        message_mentioning(&warnings_of(&project), "'level'").is_some(),
        "a stock's unparseable INITIAL must warn"
    );

    let arrayed = Variable::Aux(Aux {
        ident: "arr".to_owned(),
        equation: Equation::Arrayed(
            vec!["DimA".to_owned()],
            vec![("a1".to_owned(), "1 +".to_owned(), None, None)],
            None,
            false,
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    });
    let mut project = make_project(vec![make_model(vec![arrayed])]);
    project.dimensions = vec![datamodel::Dimension {
        name: "DimA".to_owned(),
        elements: datamodel::DimensionElements::Named(vec!["a1".to_owned()]),
        mappings: vec![],
        parent: None,
    }];
    assert!(
        message_mentioning(&warnings_of(&project), "'arr'").is_some(),
        "an arrayed element's unparseable equation must warn"
    );
}

// ---- #913: transpose has no Vensim equivalent ----

/// Vensim has no transpose operator -- `mdl::parser` has no token for `'` and no
/// `Transpose` AST node -- so an equation carrying one CANNOT be represented in
/// MDL. The export is degraded no matter what text we emit, and the writer's job
/// in that situation is to say so.
///
/// It is also grouped, so the text at least denotes the tree the model has: the
/// postfix `'` binds tighter than any infix, so the bare `a + b'` reads as
/// transposing `b` alone.
#[test]
fn transpose_warns_and_is_grouped() {
    let project = make_project(vec![make_model(vec![make_aux(
        "moved", "(a + b)'", None, "",
    )])]);
    let (text, warnings) = project_to_mdl_with_warnings(&project).expect("write");
    let w = message_mentioning(&warnings, "'moved'").expect("transpose warning");
    assert!(
        w.message.contains("transpose"),
        "warning should name the construct: {}",
        w.message
    );
    assert!(
        text.contains("(a + b)'"),
        "the transpose operand must stay grouped, got:\n{text}"
    );
}

/// The grouping rule is the same one `ast::paren_if_necessary` applies: any
/// non-atomic operand, including a prefix unary.
#[test]
fn transpose_of_a_prefix_unary_is_grouped() {
    let project = make_project(vec![make_model(vec![make_aux("moved", "(-a)'", None, "")])]);
    let (text, _) = project_to_mdl_with_warnings(&project).expect("write");
    assert!(
        text.contains("(-a)'"),
        "a negated transpose operand must stay grouped, got:\n{text}"
    );
}

// ---- ROUND is a Simlin extension with no Vensim equivalent ----

/// Vensim defines no ROUND function, so an equation calling the Simlin ROUND
/// builtin cannot be represented in MDL. The catch-all rename emits
/// `ROUND(...)` as-is (there is no Vensim-expressible round-half-to-even to
/// degrade to), and the writer's job is to say so.
#[test]
fn round_builtin_warns_on_export() {
    let project = make_project(vec![make_model(vec![
        make_aux("x", "2.5", None, ""),
        make_aux("rounded", "round(x)", None, ""),
    ])]);
    let (text, warnings) = project_to_mdl_with_warnings(&project).expect("write");
    let w = message_mentioning(&warnings, "'rounded'").expect("round warning");
    assert!(
        w.message.contains("ROUND"),
        "warning should name the construct: {}",
        w.message
    );
    assert!(
        text.contains("ROUND(x)"),
        "the call must still be emitted as-is, got:\n{text}"
    );
}

/// A ROUND nested under other expression shapes still warns (the predicate
/// walks the whole tree), and TRUNC -- which is Vensim's INTEGER -- does not.
#[test]
fn nested_round_warns_and_trunc_does_not() {
    let project = make_project(vec![make_model(vec![make_aux(
        "nested",
        "1 + trunc(round(a) / 2)",
        None,
        "",
    )])]);
    let (_, warnings) = project_to_mdl_with_warnings(&project).expect("write");
    assert!(
        message_mentioning(&warnings, "'nested'").is_some(),
        "a nested ROUND must still warn"
    );

    let project = make_project(vec![make_model(vec![make_aux(
        "plain", "trunc(a)", None, "",
    )])]);
    assert!(
        warnings_of(&project).is_empty(),
        "TRUNC is INTEGER and must not warn"
    );
}

/// The flooring pair is not Vensim's: `INT` and `MOD` are written by name,
/// which Simlin reads back as what they were and Vensim does not read, and
/// each says so. The truncating pair is Vensim's own INTEGER and MODULO and is
/// written without a word, as is the `MOD` a PULSE TRAIN holds, which the
/// writer spells as the call it came from.
#[test]
fn the_flooring_functions_warn_and_the_truncating_ones_do_not() {
    let rows: &[(&str, &str, Option<&str>)] = &[
        ("int(a)", "INT(a)", Some("calls INT")),
        ("a mod 3", "MOD(a, 3)", Some("calls MOD")),
        ("trunc(a)", "INTEGER(a)", None),
        ("rem(a, 3)", "MODULO(a, 3)", None),
        (
            "if time + dt / 2 > 1 and time <= 9 and (time + dt / 2 - 1) mod 2 < max(dt, 1) then 1 else 0",
            "PULSE TRAIN(1, 1, 2, 9)",
            None,
        ),
    ];
    for (equation, written, warning) in rows {
        let project = make_project(vec![make_model(vec![make_aux("y", equation, None, "")])]);
        let (text, warnings) = project_to_mdl_with_warnings(&project).expect("write");
        assert!(text.contains(written), "{equation}:\n{text}");
        match warning {
            Some(words) => {
                assert_eq!(warnings.len(), 1, "{equation}: {warnings:?}");
                assert!(
                    warnings[0].message.contains(words) && warnings[0].message.contains("'y'"),
                    "{equation}: {warnings:?}"
                );
            }
            None => assert!(warnings.is_empty(), "{equation}: {warnings:?}"),
        }
    }
}

/// In a model that defines its own `mod`, a `MOD(a, b)` call is that
/// variable's, so the floored modulus is written as Vensim's MODULO, which
/// truncates, and the warning says so.
#[test]
fn a_modulus_beside_a_variable_named_mod_is_written_as_modulo_and_warns() {
    let project = make_project(vec![make_model(vec![
        make_aux("mod", "3", None, ""),
        make_aux("y", "a mod 3", None, ""),
    ])]);
    let (text, warnings) = project_to_mdl_with_warnings(&project).expect("write");
    assert!(text.contains("MODULO(a, 3)"), "{text}");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].message.contains("'y'") && warnings[0].message.contains("MODULO"),
        "{warnings:?}"
    );
}

/// An equation with no transpose must not warn.
#[test]
fn no_transpose_does_not_warn() {
    let project = make_project(vec![make_model(vec![make_aux("plain", "a + b", None, "")])]);
    assert!(
        warnings_of(&project).is_empty(),
        "an ordinary equation must not warn"
    );
}

// ---- a save keeps what the model means ----

const CONTROL: &str = "
INITIAL TIME = 0 ~~|
FINAL TIME = 10 ~~|
TIME STEP = 1 ~~|
SAVEPER = TIME STEP ~~|
";

/// `source` as it reads, and as its save reads back, with the save's warnings.
fn read_and_reread(source: &str) -> (datamodel::Project, datamodel::Project, Vec<ExportWarning>) {
    let original = crate::mdl::parse_mdl(source).expect("the source parses");
    let (save, warnings) = project_to_mdl_with_warnings(&original).expect("the save writes");
    let reread = crate::mdl::parse_mdl(&save)
        .unwrap_or_else(|err| panic!("the save reads back: {err}\n{save}"));
    (original, reread, warnings)
}

/// Each element of an arrayed variable with its initial equation, by key.
fn element_initials(project: &datamodel::Project, ident: &str) -> Vec<(String, Option<String>)> {
    let var = project.models[0]
        .variables
        .iter()
        .find(|v| v.get_ident() == ident)
        .unwrap_or_else(|| panic!("no variable {ident}"));
    let Variable::Aux(Aux {
        equation: Equation::Arrayed(_, elements, _, _),
        ..
    }) = var
    else {
        panic!("{ident} is not an arrayed auxiliary");
    };
    let mut initials: Vec<(String, Option<String>)> = elements
        .iter()
        .map(|(key, _, initial, _)| (key.to_lowercase(), initial.clone()))
        .collect();
    initials.sort();
    initials
}

/// The importer stores an arrayed ACTIVE INITIAL on each element, whether the
/// file wrote it over the dimension or element by element; a save that left
/// the initials out made the variable start from its active equation, and
/// in C-LEARN made the initial values depend on themselves.
#[test]
fn an_arrayed_active_initial_keeps_each_elements_initial_through_a_save() {
    let source = format!(
        "DimA: a1, a2, a3 ~~|
y[DimA] = 1, 2, 3 ~~|
x[DimA] = ACTIVE INITIAL(y[DimA] * 2, 100) ~~|
z[a1] = ACTIVE INITIAL(y[a1], 10) ~~|
z[a2] = ACTIVE INITIAL(y[a2] * 2, 20) ~~|
{CONTROL}"
    );
    let (original, reread, warnings) = read_and_reread(&source);
    for ident in ["x", "z"] {
        let before = element_initials(&original, ident);
        assert!(
            before.iter().all(|(_, initial)| initial.is_some()),
            "{before:?}"
        );
        assert_eq!(element_initials(&reread, ident), before, "{ident}");
    }
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// `DimB: B1, B2 -> (DimA: SubA, A3)` maps B1 to every element of SubA. The
/// save writes the subrange's name back where the element map has several
/// targets for one element; writing only `-> DimA` lost the map, and a
/// mapped reference then named an element DimB does not have.
#[test]
fn a_mapping_through_a_subrange_keeps_its_element_map_through_a_save() {
    let source = format!(
        "DimA: A1, A2, A3 ~~|
SubA: A1, A2 ~~|
DimB: B1, B2 -> (DimA: SubA, A3) ~~|
b[DimB] = 1, 2 ~~|
a[DimA] = b[DimB] * 10 ~~|
{CONTROL}"
    );
    let (original, reread, warnings) = read_and_reread(&source);
    let mappings = |project: &datamodel::Project| {
        let dim = project
            .dimensions
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case("DimB"))
            .expect("DimB");
        dim.mappings.clone()
    };
    assert_eq!(
        mappings(&original)[0].element_map.len(),
        3,
        "B1 maps to A1 and A2"
    );
    assert_eq!(mappings(&reread), mappings(&original));
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn a_function_of_is_written_as_nan_with_a_warning() {
    let source = "x = 1 ~~|\ny = A FUNCTION OF(x) ~~|\nINITIAL TIME = 0 ~~|\nFINAL TIME = 1 ~~|\n\
                  TIME STEP = 1 ~~|\nSAVEPER = TIME STEP ~~|\n\\\\\\---///\n";
    let project = crate::mdl::parse_mdl(source).expect("the source reads");
    let (mdl, warnings) = project_to_mdl_with_warnings(&project).expect("write");
    assert!(
        mdl.contains("y = NaN") || mdl.contains("y=\n\tNaN"),
        "{mdl}"
    );
    assert!(
        message_mentioning(&warnings, "'y'").is_some_and(|m| m.message.contains("not a number")),
        "{warnings:?}"
    );
}

#[test]
fn an_active_initial_inside_an_equation_is_warned_of() {
    // Rows: inside arithmetic, and inside the top one's own arguments; and
    // the top of an equation, which is not warned of.
    let rows = [
        ("1 + INIT(TIME, 2)", true),
        ("INIT(INIT(TIME, 2), 3)", true),
        ("INIT(TIME, 2)", false),
    ];
    for (equation, warned) in rows {
        let var = Variable::Aux(datamodel::Aux {
            ident: "y".to_owned(),
            equation: Equation::Scalar(equation.to_owned()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        });
        let project = project_with_dims(vec![make_model(vec![var])], vec![]);
        let (_, warnings) = project_to_mdl_with_warnings(&project).expect("write");
        assert_eq!(
            message_mentioning(&warnings, "initial value inside it").is_some(),
            warned,
            "{equation}: {warnings:?}"
        );
    }
}

#[test]
fn an_except_default_written_as_elements_names_the_elements_it_loses() {
    // A default that reads external data has no :EXCEPT: equation; the
    // elements only it defines are not written, and the warning names them.
    // Hand-built: the MDL reader keeps such a default only with every element
    // in a slot, but a datamodel from elsewhere (XMILE, an edit) need not.
    let z = arrayed_aux(
        "z",
        &["DimA"],
        vec![("A1", "1")],
        Some("{GET DIRECT DATA('data.csv', 'B', '2', 'C')}"),
        true,
    );
    let project = project_with_dims(
        vec![make_model(vec![z])],
        vec![named_dim("DimA", &["A1", "A2", "A3"])],
    );
    let (_, warnings) = project_to_mdl_with_warnings(&project).expect("write");
    let warned = message_mentioning(&warnings, "default is not kept").expect("a warning");
    assert!(
        warned.message.contains("(a2; a3) are not written")
            && !warned.message.contains("table's input"),
        "{warnings:?}"
    );
}
