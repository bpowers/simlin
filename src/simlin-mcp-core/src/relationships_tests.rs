// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! [`generate_relationships`] over hand-written polarity maps: which edges it
//! keeps, the polarity each gets, and the names it writes.

use std::collections::HashMap;

use super::generate_relationships;
use simlin_engine::json_sdai::Polarity;
use simlin_engine::ltm::LinkPolarity;

#[test]
fn test_generate_relationships_basic_equation_deps() {
    use simlin_engine::testutils::{x_aux, x_model};

    // Polarity map uses canonical (lowercase) keys, as returned by
    // compute_link_polarities(). Model variables use authored names.
    let polarities: HashMap<(String, String), LinkPolarity> = HashMap::from([
        (("a".into(), "c".into()), LinkPolarity::Positive),
        (("b".into(), "c".into()), LinkPolarity::Positive),
    ]);
    let model = x_model(
        "main",
        vec![
            x_aux("A", "10", None),
            x_aux("B", "20", None),
            x_aux("C", "A + B", None),
        ],
    );

    let rels = generate_relationships(&polarities, &model);
    assert_eq!(rels.len(), 2);
    assert!(rels.iter().any(|r| r.from == "A" && r.to == "C"));
    assert!(rels.iter().any(|r| r.from == "B" && r.to == "C"));
    for r in &rels {
        assert!(r.reasoning.is_none());
        assert!(r.polarity_reasoning.is_none());
    }
}

#[test]
fn test_generate_relationships_flow_referencing_stock() {
    use simlin_engine::testutils::{x_aux, x_flow, x_model, x_stock};

    let polarities: HashMap<(String, String), LinkPolarity> = HashMap::from([
        (
            ("population".into(), "deaths".into()),
            LinkPolarity::Positive,
        ),
        (
            ("death_rate".into(), "deaths".into()),
            LinkPolarity::Positive,
        ),
    ]);
    let model = x_model(
        "main",
        vec![
            x_stock("population", "1000", &[], &["deaths"], None),
            x_flow("deaths", "population * death_rate", None),
            x_aux("death_rate", "0.01", None),
        ],
    );

    let rels = generate_relationships(&polarities, &model);
    assert_eq!(rels.len(), 2);
    assert!(
        rels.iter()
            .any(|r| r.from == "population" && r.to == "deaths")
    );
    assert!(
        rels.iter()
            .any(|r| r.from == "death_rate" && r.to == "deaths")
    );
}

#[test]
fn test_generate_relationships_no_equation_no_relationships() {
    use simlin_engine::testutils::{x_aux, x_model};

    let polarities: HashMap<(String, String), LinkPolarity> =
        HashMap::from([(("A".into(), "C".into()), LinkPolarity::Positive)]);
    let model = x_model(
        "main",
        vec![
            x_aux("A", "10", None),
            x_aux("B", "", None),
            x_aux("C", "A", None),
        ],
    );

    let rels = generate_relationships(&polarities, &model);
    assert!(!rels.iter().any(|r| r.to == "B"));
}

#[test]
fn test_generate_relationships_positive_polarity() {
    use simlin_engine::testutils::{x_aux, x_model};

    let polarities: HashMap<(String, String), LinkPolarity> = HashMap::from([
        (("a".into(), "c".into()), LinkPolarity::Positive),
        (("b".into(), "c".into()), LinkPolarity::Positive),
    ]);
    let model = x_model(
        "main",
        vec![
            x_aux("A", "10", None),
            x_aux("B", "20", None),
            x_aux("C", "A + B", None),
        ],
    );

    let rels = generate_relationships(&polarities, &model);
    for r in &rels {
        assert_eq!(r.polarity, Polarity::Positive);
    }
}

#[test]
fn test_generate_relationships_mixed_polarity() {
    use simlin_engine::testutils::{x_aux, x_model};

    let polarities: HashMap<(String, String), LinkPolarity> = HashMap::from([
        (("a".into(), "c".into()), LinkPolarity::Positive),
        (("b".into(), "c".into()), LinkPolarity::Negative),
    ]);
    let model = x_model(
        "main",
        vec![
            x_aux("A", "10", None),
            x_aux("B", "20", None),
            x_aux("C", "A - B", None),
        ],
    );

    let rels = generate_relationships(&polarities, &model);
    let a_rel = rels.iter().find(|r| r.from == "A").unwrap();
    let b_rel = rels.iter().find(|r| r.from == "B").unwrap();
    assert_eq!(a_rel.polarity, Polarity::Positive);
    assert_eq!(b_rel.polarity, Polarity::Negative);
}

#[test]
fn test_generate_relationships_unknown_polarity() {
    use simlin_engine::testutils::{x_aux, x_model};

    let polarities: HashMap<(String, String), LinkPolarity> =
        HashMap::from([(("x".into(), "y".into()), LinkPolarity::Unknown)]);
    let model = x_model("main", vec![x_aux("X", "10", None), x_aux("Y", "X", None)]);

    let rels = generate_relationships(&polarities, &model);
    assert_eq!(rels.len(), 1);
    assert_eq!(rels[0].polarity, Polarity::Unknown);
    assert!(rels[0].reasoning.is_none());
    assert!(rels[0].polarity_reasoning.is_none());
}

// -- generate_relationships tests (Task 2: stock-flow filtering, ordering, empty) --

#[test]
fn test_generate_relationships_filters_stock_flow_structural_edges() {
    use simlin_engine::testutils::{x_aux, x_flow, x_model, x_stock};

    let polarities: HashMap<(String, String), LinkPolarity> = HashMap::from([
        // structural: inflow -> stock
        (
            ("births".into(), "population".into()),
            LinkPolarity::Positive,
        ),
        // structural: outflow -> stock
        (
            ("deaths".into(), "population".into()),
            LinkPolarity::Negative,
        ),
        // equation-derived: stock referenced in flow equation
        (
            ("population".into(), "deaths".into()),
            LinkPolarity::Positive,
        ),
        // equation-derived
        (
            ("death_rate".into(), "deaths".into()),
            LinkPolarity::Positive,
        ),
    ]);
    let model = x_model(
        "main",
        vec![
            x_stock("population", "1000", &["births"], &["deaths"], None),
            x_flow("births", "population * 0.1", None),
            x_flow("deaths", "population * death_rate", None),
            x_aux("death_rate", "0.01", None),
        ],
    );

    let rels = generate_relationships(&polarities, &model);

    // AC3.1: structural inflow edge filtered
    assert!(
        !rels
            .iter()
            .any(|r| r.from == "births" && r.to == "population"),
        "structural inflow edge should be filtered"
    );
    // AC3.2: structural outflow edge filtered
    assert!(
        !rels
            .iter()
            .any(|r| r.from == "deaths" && r.to == "population"),
        "structural outflow edge should be filtered"
    );
    // AC3.3: equation-derived stock->flow edge preserved
    assert!(
        rels.iter()
            .any(|r| r.from == "population" && r.to == "deaths"),
        "equation-derived edge from stock to flow should be present"
    );
    assert!(
        rels.iter()
            .any(|r| r.from == "death_rate" && r.to == "deaths"),
        "equation-derived edge should be present"
    );
    assert_eq!(rels.len(), 2);
}

#[test]
fn test_generate_relationships_deterministic_ordering() {
    use simlin_engine::testutils::{x_aux, x_model};

    let polarities: HashMap<(String, String), LinkPolarity> = HashMap::from([
        (("z".into(), "out".into()), LinkPolarity::Positive),
        (("a".into(), "out".into()), LinkPolarity::Positive),
        (("m".into(), "out".into()), LinkPolarity::Negative),
        (("a".into(), "mid".into()), LinkPolarity::Positive),
    ]);
    let model = x_model(
        "main",
        vec![
            x_aux("A", "1", None),
            x_aux("M", "2", None),
            x_aux("Z", "3", None),
            x_aux("mid", "A", None),
            x_aux("out", "A + M + Z", None),
        ],
    );

    let rels = generate_relationships(&polarities, &model);
    assert_eq!(rels.len(), 4);

    let keys: Vec<(&str, &str)> = rels
        .iter()
        .map(|r| (r.from.as_str(), r.to.as_str()))
        .collect();
    assert_eq!(
        keys,
        vec![("A", "mid"), ("A", "out"), ("M", "out"), ("Z", "out")]
    );
}

#[test]
fn test_generate_relationships_empty_model() {
    use simlin_engine::testutils::x_model;

    let polarities: HashMap<(String, String), LinkPolarity> = HashMap::new();
    let model = x_model("main", vec![]);

    let rels = generate_relationships(&polarities, &model);
    assert!(rels.is_empty());
}

#[test]
fn test_generate_relationships_filters_undeclared_variables() {
    // compute_link_polarities() can return edges involving variables not
    // present in the model: simulator builtins (TIME, DT) and implicit
    // compiler nodes from SMOOTH/DELAY expansion ($⁚-prefixed names).
    // Only relationships where both endpoints are declared model
    // variables should appear in the output.
    use simlin_engine::testutils::{x_aux, x_model};

    let polarities: HashMap<(String, String), LinkPolarity> = HashMap::from([
        // real equation-derived edge (both endpoints in model)
        (("a".into(), "c".into()), LinkPolarity::Positive),
        // TIME builtin as source (not a declared variable)
        (("time".into(), "c".into()), LinkPolarity::Positive),
        // DT builtin as source
        (("dt".into(), "c".into()), LinkPolarity::Positive),
    ]);

    let model = x_model(
        "main",
        vec![x_aux("a", "10", None), x_aux("c", "a + TIME", None)],
    );

    let rels = generate_relationships(&polarities, &model);

    assert_eq!(
        rels.len(),
        1,
        "only the edge between declared variables should survive; got {:?}",
        rels
    );
    assert_eq!(rels[0].from, "a");
    assert_eq!(rels[0].to, "c");
}

#[test]
fn test_generate_relationships_filters_implicit_compiler_nodes() {
    // SMOOTH/DELAY builtins synthesize implicit variables with $⁚-prefixed
    // names. These are not declared model variables so they must be filtered
    // by the same model-membership guard that handles TIME/DT.
    use simlin_engine::testutils::{x_aux, x_model};

    let polarities: HashMap<(String, String), LinkPolarity> = HashMap::from([
        // real equation-derived edge
        (("a".into(), "c".into()), LinkPolarity::Positive),
        // implicit compiler node as source
        (
            ("$\u{205A}x\u{205A}0\u{205A}smth1".into(), "c".into()),
            LinkPolarity::Positive,
        ),
        // implicit compiler node as target
        (
            ("a".into(), "$\u{205A}x\u{205A}0\u{205A}arg1".into()),
            LinkPolarity::Positive,
        ),
        // both implicit
        (
            (
                "$\u{205A}y\u{205A}0\u{205A}smth1".into(),
                "$\u{205A}y\u{205A}0\u{205A}arg1".into(),
            ),
            LinkPolarity::Positive,
        ),
    ]);

    let model = x_model(
        "main",
        vec![x_aux("a", "10", None), x_aux("c", "SMTH1(a, 5)", None)],
    );

    let rels = generate_relationships(&polarities, &model);

    assert_eq!(
        rels.len(),
        1,
        "only the real equation-derived edge should survive; got {:?}",
        rels
    );
    assert_eq!(rels[0].from, "a");
    assert_eq!(rels[0].to, "c");
}

#[test]
fn test_generate_relationships_uses_authored_variable_names() {
    // compute_link_polarities() returns canonical (lowercase) keys, but the
    // output relationships should use the authored variable names from the
    // model so they match the names in the variables array.
    use simlin_engine::testutils::{x_aux, x_model};

    let polarities: HashMap<(String, String), LinkPolarity> = HashMap::from([
        (
            ("birth_rate".into(), "births".into()),
            LinkPolarity::Positive,
        ),
        (
            ("population".into(), "births".into()),
            LinkPolarity::Positive,
        ),
    ]);

    let model = x_model(
        "main",
        vec![
            x_aux("Population", "1000", None),
            x_aux("Birth Rate", "0.03", None),
            x_aux("Births", "Population * Birth Rate", None),
        ],
    );

    let rels = generate_relationships(&polarities, &model);

    assert_eq!(rels.len(), 2);
    // Relationships should use authored names from the model, not
    // canonical keys from the polarity map.
    assert!(
        rels.iter()
            .any(|r| r.from == "Birth Rate" && r.to == "Births"),
        "expected authored name 'Birth Rate' and 'Births', got {:?}",
        rels
    );
    assert!(
        rels.iter()
            .any(|r| r.from == "Population" && r.to == "Births"),
        "expected authored name 'Population' and 'Births', got {:?}",
        rels
    );
}

#[test]
fn test_generate_relationships_filters_mixed_case_stock_flow_edges() {
    // compute_link_polarities() returns canonical (lowercase) keys.  The
    // datamodel may preserve original casing (e.g. "Population", "Births").
    // Verify that structural edges are filtered regardless of casing in the
    // datamodel, so mixed-case names do not leak into the output.
    use simlin_engine::testutils::{x_flow, x_model, x_stock};

    let polarities: HashMap<(String, String), LinkPolarity> = HashMap::from([
        // structural inflow edge (canonical keys from compute_link_polarities)
        (
            ("births".into(), "population".into()),
            LinkPolarity::Positive,
        ),
        // structural outflow edge
        (
            ("deaths".into(), "population".into()),
            LinkPolarity::Negative,
        ),
        // equation-derived: stock appears in flow equation
        (
            ("population".into(), "deaths".into()),
            LinkPolarity::Positive,
        ),
    ]);

    // Datamodel uses mixed-case names, matching the SD-AI JSON as received.
    let model = x_model(
        "main",
        vec![
            x_stock("Population", "1000", &["Births"], &["Deaths"], None),
            x_flow("Births", "Population * 0.1", None),
            x_flow("Deaths", "Population * 0.02", None),
        ],
    );

    let rels = generate_relationships(&polarities, &model);

    // Both structural edges must be filtered despite the case mismatch
    // between the datamodel identifiers and the canonical polarity map keys.
    assert!(
        !rels
            .iter()
            .any(|r| r.from == "Births" && r.to == "Population"),
        "structural inflow edge should be filtered even with mixed-case datamodel names"
    );
    assert!(
        !rels
            .iter()
            .any(|r| r.from == "Deaths" && r.to == "Population"),
        "structural outflow edge should be filtered even with mixed-case datamodel names"
    );

    // The equation-derived edge must survive filtering, with authored names.
    assert!(
        rels.iter()
            .any(|r| r.from == "Population" && r.to == "Deaths"),
        "equation-derived edge should use authored names; got {:?}",
        rels
    );
    assert_eq!(rels.len(), 1);
}
