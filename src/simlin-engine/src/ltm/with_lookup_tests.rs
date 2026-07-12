// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Implicit WITH-LOOKUP link-polarity tests (GH #910).
//!
//! Split out of `ltm/tests.rs` to keep that file under the project line-count
//! lint; included via `#[path]` as a child of it, so `use super::*` resolves
//! the shared fixtures (`continuous_gf`, `arrayed_aux`, `link_polarities_for`).

use super::*;

/// A scalar aux with BOTH a real input equation and a variable-level
/// graphical function -- the implicit WITH LOOKUP shape (GH #910): at
/// compile time `apply_implicit_with_lookup` lowers it to
/// `LOOKUP(self, input)`, so link polarity must compose the gf.
#[cfg(test)]
fn with_lookup_scalar_aux(
    ident: &str,
    equation: &str,
    gf: Option<crate::datamodel::GraphicalFunction>,
) -> crate::datamodel::Variable {
    crate::datamodel::Variable::Aux(crate::datamodel::Aux {
        ident: ident.to_string(),
        equation: crate::datamodel::Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf,
        ai_state: None,
        uid: None,
        compat: crate::datamodel::Compat::default(),
    })
}

/// An apply-to-all arrayed aux with a variable-level graphical function --
/// the arrayed WITH LOOKUP shape sharing ONE table across all elements.
#[cfg(test)]
fn with_lookup_a2a_aux(
    ident: &str,
    dim: &str,
    equation: &str,
    gf: crate::datamodel::GraphicalFunction,
) -> crate::datamodel::Variable {
    crate::datamodel::Variable::Aux(crate::datamodel::Aux {
        ident: ident.to_string(),
        equation: crate::datamodel::Equation::ApplyToAll(
            vec![dim.to_string()],
            equation.to_string(),
        ),
        documentation: String::new(),
        units: None,
        gf: Some(gf),
        ai_state: None,
        uid: None,
        compat: crate::datamodel::Compat::default(),
    })
}

/// A per-element-equation arrayed aux where each element carries a real
/// input equation and an OPTIONAL per-element graphical function -- the
/// arrayed WITH LOOKUP shape of GH #909 (a gf-less element keeps its raw
/// input equation).
#[cfg(test)]
fn with_lookup_arrayed_aux(
    ident: &str,
    dim: &str,
    elements: &[(&str, &str, Option<crate::datamodel::GraphicalFunction>)],
) -> crate::datamodel::Variable {
    let arrayed = elements
        .iter()
        .map(|(elem, eqn, gf)| (elem.to_string(), eqn.to_string(), None, gf.clone()))
        .collect();
    crate::datamodel::Variable::Aux(crate::datamodel::Aux {
        ident: ident.to_string(),
        equation: crate::datamodel::Equation::Arrayed(vec![dim.to_string()], arrayed, None, false),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: crate::datamodel::Compat::default(),
    })
}

#[test]
fn test_with_lookup_scalar_polarity_composes_gf() {
    // GH #910: `effect = input` with a variable-level gf lowers to
    // `LOOKUP(effect_gf, input)` -- the link polarity must compose the raw
    // input polarity with the gf's monotonicity even though the equation
    // text contains no LOOKUP call.
    let run = |eqn: &str, gf: Option<crate::datamodel::GraphicalFunction>| {
        let polarities = link_polarities_for(
            "region",
            &["nyc", "boston"],
            vec![
                with_lookup_scalar_aux("input", "5", None),
                with_lookup_scalar_aux("effect", eqn, gf),
            ],
        );
        polarities[&("input".to_string(), "effect".to_string())]
    };

    // A decreasing gf flips the positive raw-input polarity.
    assert_eq!(
        run("input", Some(continuous_gf(vec![4.0, 2.0, 0.0]))),
        LinkPolarity::Negative,
        "decreasing with-lookup gf must flip the input link polarity"
    );
    // An increasing gf keeps it.
    assert_eq!(
        run("input", Some(continuous_gf(vec![0.0, 1.0, 2.0]))),
        LinkPolarity::Positive,
        "increasing with-lookup gf must keep the input link polarity"
    );
    // A non-monotone gf makes the composed link Unknown.
    assert_eq!(
        run("input", Some(continuous_gf(vec![0.0, 2.0, 1.0]))),
        LinkPolarity::Unknown,
        "non-monotone with-lookup gf must yield Unknown link polarity"
    );
    // A decreasing gf on a NEGATIVE raw-input relationship composes to
    // Positive (two sign flips).
    assert_eq!(
        run("10 - input", Some(continuous_gf(vec![4.0, 2.0, 0.0]))),
        LinkPolarity::Positive,
        "decreasing gf composed with a negative input relationship -> Positive"
    );
    // A zero-point gf is treated as absent by the compiler
    // (`apply_implicit_with_lookup`): the raw input polarity stands.
    let empty_gf = crate::datamodel::GraphicalFunction {
        kind: crate::datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![]),
        y_points: vec![],
        x_scale: crate::datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        y_scale: crate::datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
    };
    assert_eq!(
        run("input", Some(empty_gf)),
        LinkPolarity::Positive,
        "a zero-point gf applies no wrap, so the raw polarity stands"
    );
    // No gf at all: unchanged baseline.
    assert_eq!(
        run("input", None),
        LinkPolarity::Positive,
        "no gf: the raw input polarity stands"
    );
}

#[test]
fn test_with_lookup_a2a_polarity_composes_shared_gf() {
    // Arrayed A2A WITH LOOKUP sharing one variable-level table: the wrap
    // applies the single table to every element (GH #909), so the link
    // polarity composes with that one table's monotonicity.
    let polarities = link_polarities_for(
        "region",
        &["nyc", "boston"],
        vec![
            arrayed_aux("dose", "region", "5"),
            with_lookup_a2a_aux(
                "effect",
                "region",
                "dose[region]",
                continuous_gf(vec![4.0, 2.0, 0.0]),
            ),
        ],
    );
    assert_eq!(
        polarities[&("dose".to_string(), "effect".to_string())],
        LinkPolarity::Negative,
        "shared decreasing gf on an A2A with-lookup target must flip the link"
    );
}

#[test]
fn test_with_lookup_arrayed_per_element_polarity() {
    // Per-element-equation arrayed WITH LOOKUP (GH #909): each element's
    // input equation is wrapped by that element's OWN table; a gf-less
    // element keeps its raw input equation.
    let run = |elements: &[(&str, &str, Option<crate::datamodel::GraphicalFunction>)]| {
        let polarities = link_polarities_for(
            "region",
            &["nyc", "boston"],
            vec![
                arrayed_aux("dose", "region", "5"),
                with_lookup_arrayed_aux("effect", "region", elements),
            ],
        );
        polarities[&("dose".to_string(), "effect".to_string())]
    };

    let dec = || Some(continuous_gf(vec![4.0, 2.0, 0.0]));
    let inc = || Some(continuous_gf(vec![0.0, 1.0, 2.0]));

    // Both elements decreasing: the composed link flips.
    assert_eq!(
        run(&[
            ("nyc", "dose[nyc]", dec()),
            ("boston", "dose[boston]", dec())
        ]),
        LinkPolarity::Negative,
        "agreeing decreasing per-element gfs must flip the link"
    );
    // Directions disagree: Unknown.
    assert_eq!(
        run(&[
            ("nyc", "dose[nyc]", dec()),
            ("boston", "dose[boston]", inc())
        ]),
        LinkPolarity::Unknown,
        "disagreeing per-element gf directions -> Unknown"
    );
    // A decreasing gf on one element and NO gf on the other: the wrapped
    // element flips while the raw element keeps Positive -- the disagreement
    // collapses to Unknown.
    assert_eq!(
        run(&[
            ("nyc", "dose[nyc]", dec()),
            ("boston", "dose[boston]", None)
        ]),
        LinkPolarity::Unknown,
        "a decreasing gf mixed with a gf-less (raw) element -> Unknown"
    );
    // An increasing gf mixed with a gf-less element: both effective
    // polarities are Positive, so the link stays concrete.
    assert_eq!(
        run(&[
            ("nyc", "dose[nyc]", inc()),
            ("boston", "dose[boston]", None)
        ]),
        LinkPolarity::Positive,
        "an increasing gf mixed with a gf-less element keeps Positive"
    );
}
