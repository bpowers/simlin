// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Property-based tests for the MDL writer.
//!
//! These close the coverage gaps the curated `writer_tests.rs` /
//! `writer_lossiness_tests.rs` cases miss by generating a large space of inputs
//! for each of three properties the hardening work established:
//!
//! 1. **Free-text sanitization** (#849): adversarial documentation strings --
//!    full of the structural characters (`|`, `~`, newlines, `\r`) and the
//!    section-terminator runs the confirmed corruption exploited -- must
//!    re-parse to exactly the real variable, never injecting a phantom or
//!    dropping it. Adversarial *units* get the weaker structural-only guarantee
//!    (see `units_free_text_is_structurally_safe` for why the units field, a
//!    typed unit expression, cannot promise the same).
//! 2. **Equation round-trip** (#846/#847/#850/#852): the context-aware
//!    expression printer's output must re-parse as MDL and be a re-write
//!    fixpoint.
//! 3. **Idempotence**: a whole generated model (scalar + arrayed variables with
//!    adversarial docs and valid units) must reach a `write(parse(...))`
//!    fixpoint.
//!
//! ## Generator design choices
//!
//! - Property 2 generates `Expr0` ASTs directly from a bounded recursive grammar
//!   and serializes them with `ast::print_eqn` to obtain the XMILE-syntax
//!   equation the datamodel stores. This is strictly more robust than generating
//!   MDL/XMILE equation *strings*: `print_eqn` produces valid XMILE by
//!   construction, so no case is lost to an accidentally-malformed input, and it
//!   lets the grammar aim precisely at the printer fixes (wildcard subscript
//!   recovery, the `pi` literal, INITIAL arity). Any well-formed AST that fails
//!   to re-parse or is not a fixpoint is a genuine writer bug, which is the point.
//! - The fixpoint for properties 2 and 3 is asserted between the *second* and
//!   *third* MDL renders (`write -> parse -> write -> parse -> write`), never the
//!   first, so the one-time XMILE->MDL normalization and any first-import
//!   variable reordering are absorbed before the comparison. Only the equations
//!   section is compared, sidestepping the separately-tracked control-variable
//!   value-substitution non-idempotence; the generated models carry no views, so
//!   the sketch-instability class never arises.
//! - Case counts are deliberately modest (each case runs a full
//!   `project_to_mdl` + `parse_mdl`, far heavier than a JSON round-trip) so the
//!   whole module stays within the few-seconds debug budget.

use super::*;
use crate::ast::{Loc, print_eqn};
use crate::builtins::UntypedBuiltinFn;
use crate::common::RawIdent;
use crate::datamodel::{
    Aux, Compat, Dimension, DimensionElements, Dt, Equation, Model, Project, SimMethod, SimSpecs,
    Variable,
};
use crate::mdl::{parse_mdl, project_to_mdl, project_to_mdl_with_warnings};
use proptest::prelude::*;

// ---- shared helpers ----

/// The equations section of MDL output (everything before the `.Control`
/// group). A round-trip fixpoint check compares only this, ignoring the
/// separately-tracked control-variable value-substitution non-idempotence.
fn equations_section(mdl: &str) -> &str {
    mdl.split("\t.Control").next().unwrap_or(mdl)
}

/// Sorted list of canonical variable idents in a project's single model.
fn model_idents(project: &Project) -> Vec<String> {
    let mut idents: Vec<String> = project.models[0]
        .variables
        .iter()
        .map(|v| v.get_ident().to_owned())
        .collect();
    idents.sort();
    idents
}

fn empty_sim_specs() -> SimSpecs {
    SimSpecs {
        start: 0.0,
        stop: 100.0,
        dt: Dt::Dt(1.0),
        save_step: None,
        sim_method: SimMethod::Euler,
        time_units: None,
    }
}

fn project_of(model: Model, dimensions: Vec<Dimension>) -> Project {
    Project {
        name: "prop".to_owned(),
        sim_specs: empty_sim_specs(),
        dimensions,
        units: vec![],
        models: vec![model],
        source: None,
        ai_information: None,
    }
}

fn model_of(variables: Vec<Variable>) -> Model {
    Model {
        name: "main".to_owned(),
        sim_specs: None,
        variables,
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    }
}

fn scalar_aux(ident: &str, eqn: &str, units: Option<String>, doc: &str) -> Variable {
    Variable::Aux(Aux {
        ident: ident.to_owned(),
        equation: Equation::Scalar(eqn.to_owned()),
        documentation: doc.to_owned(),
        units,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

fn apply_to_all_aux(ident: &str, dims: &[&str], eqn: &str) -> Variable {
    Variable::Aux(Aux {
        ident: ident.to_owned(),
        equation: Equation::ApplyToAll(
            dims.iter().map(|d| (*d).to_owned()).collect(),
            eqn.to_owned(),
        ),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

fn named_dim(name: &str, elems: &[&str]) -> Dimension {
    Dimension {
        name: name.to_owned(),
        elements: DimensionElements::Named(elems.iter().map(|e| (*e).to_owned()).collect()),
        mappings: vec![],
        parent: None,
    }
}

// ---- free-text generator (properties 1 & 3) ----

/// A single fragment of adversarial free text. Includes every structural
/// character the #849 sanitization contract covers for a variable's doc/units
/// field -- `|` (entry terminator), `~` (field separator), line breaks, and each
/// of the four section-terminator runs -- as indivisible building blocks, so the
/// combined string actually exercises them rather than only benign prose. (A
/// literal `,` is deliberately excluded: it is a `22:`-token separator, not a
/// doc/units structural character, and a bare comma is separately an invalid
/// *unit-expression* token -- a units-grammar concern, not a #849 corruption.)
fn free_text_fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        // benign prose (may be empty, may hold interior spaces)
        "[a-zA-Z0-9 ]{0,10}".prop_map(|s| s),
        Just("|".to_owned()),
        Just("~".to_owned()),
        Just("\n".to_owned()),
        Just("\r".to_owned()),
        Just("\r\n".to_owned()),
        Just(SECTION_TERMINATOR_OPEN.to_owned()),
        Just(SECTION_TERMINATOR_CLOSE.to_owned()),
        Just(SECTION_TERMINATOR_OPEN_SHORT.to_owned()),
        Just(SECTION_TERMINATOR_CLOSE_SHORT.to_owned()),
    ]
}

/// An arbitrary free-text field: a concatenation of adversarial fragments.
fn free_text() -> impl Strategy<Value = String> {
    prop::collection::vec(free_text_fragment(), 0..8).prop_map(|frags| frags.concat())
}

/// A *valid unit expression* (or empty). The units field is not free text: the
/// reader parses it as a unit expression, so arbitrary text -- a bare number, a
/// dangling operator (including the `/` the sanitizer substitutes for a `|`), or
/// a stray comma -- fails the unit-expression grammar with a hard error rather
/// than corrupting the variable set. Round-trip / idempotence properties that
/// need a well-formed model therefore draw units from this valid set; the
/// separate `units_free_text_is_structurally_safe` property stresses the field
/// with adversarial text and only asserts the structural (no phantom/dropped
/// variable) guarantee #849 actually makes.
fn valid_units() -> impl Strategy<Value = Option<String>> {
    prop_oneof![
        Just(None),
        Just(Some(String::new())),
        Just(Some("people".to_owned())),
        Just(Some("widgets".to_owned())),
        Just(Some("widgets/year".to_owned())),
        Just(Some("1/year".to_owned())),
        Just(Some("kg*m".to_owned())),
    ]
}

// ---- Expr0 grammar (property 2) ----

const GRAMMAR_VARS: &[&str] = &["a", "b", "c"];

fn const_leaf() -> impl Strategy<Value = Expr0> {
    prop_oneof![
        (0i64..1000).prop_map(|n| Expr0::Const(n.to_string(), n as f64, Loc::default())),
        Just(Expr0::Const("0.5".to_owned(), 0.5, Loc::default())),
        Just(Expr0::Const("1.5".to_owned(), 1.5, Loc::default())),
        Just(Expr0::Const("3.5".to_owned(), 3.5, Loc::default())),
        Just(Expr0::Const("2.25".to_owned(), 2.25, Loc::default())),
    ]
}

fn var_leaf() -> impl Strategy<Value = Expr0> {
    prop::sample::select(GRAMMAR_VARS)
        .prop_map(|name| Expr0::Var(RawIdent::new_from_str(name), Loc::default()))
}

fn app0(name: &str) -> Expr0 {
    Expr0::App(UntypedBuiltinFn(name.to_owned(), vec![]), Loc::default())
}

/// A bounded recursive `Expr0` generator aimed at the writer's printer fixes:
/// arithmetic + precedence, unary negate, the `pi` literal (#850), wildcard
/// subscript recovery `SUM(arr[*])` -> `SUM(arr[DimA!])` (#847), INITIAL arity
/// (#852), plus the common one/two-argument builtins and IF/THEN/ELSE.
fn expr0_strategy() -> BoxedStrategy<Expr0> {
    let leaf = prop_oneof![
        const_leaf(),
        var_leaf(),
        // Genuine `pi` builtin reference (no grammar var is named `pi`, so the
        // writer emits the numeric literal, exercising #850).
        Just(app0("pi")),
        // Wildcard subscript over the declared arrayed `arr[DimA]` (#847).
        Just(Expr0::App(
            UntypedBuiltinFn(
                "sum".to_owned(),
                vec![Expr0::Subscript(
                    RawIdent::new_from_str("arr"),
                    vec![IndexExpr0::Wildcard(Loc::default())],
                    Loc::default(),
                )],
            ),
            Loc::default(),
        )),
    ];

    leaf.prop_recursive(4, 48, 4, move |inner| {
        // DELIBERATELY narrow: arithmetic only, no comparisons and no `and`/`or`.
        //
        // This generator drives an MDL write -> `mdl::parser` re-read fixpoint, and
        // `mdl::parser`'s BINARY precedence table is inverted relative to Vensim and
        // XMILE (GH #914): it puts `+`/`-` at the lowest level and `:AND:` above the
        // comparisons. `mdl_paren_if_necessary` correctly targets *Vensim's* table,
        // so widening this generator to comparisons or logical operators would fail
        // the fixpoint against our own reader -- a true finding about #914, but not
        // one this property can act on. Widen it when #914 lands.
        //
        // The full-operator-set `print_eqn` round trip (`Not`, `Neq`, `Transpose`,
        // `Mod`, comparisons, `and`/`or`) lives in `ast::mod`'s
        // `print_eqn_roundtrips_over_the_full_operator_set`, which re-parses with
        // the XMILE grammar and so is not blocked on #914.
        let bin_op = prop_oneof![
            Just(BinaryOp::Add),
            Just(BinaryOp::Sub),
            Just(BinaryOp::Mul),
            Just(BinaryOp::Div),
            Just(BinaryOp::Exp),
        ];
        prop_oneof![
            (bin_op, inner.clone(), inner.clone()).prop_map(|(op, l, r)| Expr0::Op2(
                op,
                Box::new(l),
                Box::new(r),
                Loc::default()
            )),
            inner
                .clone()
                .prop_map(|l| Expr0::Op1(UnaryOp::Negative, Box::new(l), Loc::default())),
            // one-argument builtins
            (
                prop::sample::select(&["abs", "exp", "ln", "int"][..]),
                inner.clone()
            )
                .prop_map(|(f, e)| Expr0::App(
                    UntypedBuiltinFn(f.to_owned(), vec![e]),
                    Loc::default()
                )),
            // INITIAL arity DISPATCH (#852): 1-arg `init` -> INITIAL, 2-arg
            // `init` -> ACTIVE INITIAL. Both arms exercise the call-site arity
            // branch the writer uses; the fixpoint then pins that each survives
            // a full re-import (ACTIVE INITIAL re-imports back to a 2-arg init).
            inner.clone().prop_map(|e| Expr0::App(
                UntypedBuiltinFn("init".to_owned(), vec![e]),
                Loc::default()
            )),
            (inner.clone(), inner.clone()).prop_map(|(e, ai)| Expr0::App(
                UntypedBuiltinFn("init".to_owned(), vec![e, ai]),
                Loc::default()
            )),
            // two-argument builtins
            (
                prop::sample::select(&["min", "max"][..]),
                inner.clone(),
                inner.clone()
            )
                .prop_map(|(f, l, r)| Expr0::App(
                    UntypedBuiltinFn(f.to_owned(), vec![l, r]),
                    Loc::default()
                )),
            (inner.clone(), inner.clone(), inner.clone()).prop_map(|(c, t, f)| Expr0::If(
                Box::new(c),
                Box::new(t),
                Box::new(f),
                Loc::default()
            )),
        ]
    })
    .boxed()
}

/// The fixed scaffold every property-2 case shares: the grammar's referenced
/// variables (`a`, `b`, `c` scalars, `arr` apply-to-all over `DimA`) plus the
/// `DimA` dimension, so the generated `target` equation's references resolve and
/// wildcard recovery has a declared dimension to recover.
fn scaffold_vars() -> Vec<Variable> {
    vec![
        scalar_aux("a", "1", None, ""),
        scalar_aux("b", "2", None, ""),
        scalar_aux("c", "3", None, ""),
        apply_to_all_aux("arr", &["DimA"], "1"),
    ]
}

// ---- arrayed-model generator (property 3) ----

/// A small model: 1-3 scalar auxes and one arrayed (apply-to-all) aux over a
/// generated dimension. Each variable carries an adversarial (structural-char /
/// section-terminator-laden) *documentation* string -- which stresses the
/// sanitization choke point's idempotence (the `\r`-normalization and trailing
/// `|`->`/` fixpoint) -- and a *valid unit expression* (units is a typed field,
/// see `valid_units`).
fn idempotence_model() -> impl Strategy<Value = Project> {
    let scalar_names = &["alpha", "beta", "gamma"];
    let scalars = (
        1usize..=3,
        prop::collection::vec((free_text(), valid_units()), 3),
    )
        .prop_map(move |(n, texts)| {
            (0..n)
                .map(|i| {
                    let (doc, units) = &texts[i];
                    scalar_aux(scalar_names[i], &((i + 1).to_string()), units.clone(), doc)
                })
                .collect::<Vec<_>>()
        });

    // 2-3 named dimension elements, declared-order (avoids the separately-tracked
    // arrayed element-order non-idempotence).
    let dim = (2usize..=3).prop_map(|n| {
        let elems: Vec<String> = (0..n).map(|i| format!("e{}", i + 1)).collect();
        elems
    });

    (scalars, dim, free_text(), valid_units()).prop_map(|(mut vars, elems, arr_doc, arr_units)| {
        let elem_refs: Vec<&str> = elems.iter().map(String::as_str).collect();
        let arr = Variable::Aux(Aux {
            ident: "arr".to_owned(),
            equation: Equation::ApplyToAll(vec!["DimB".to_owned()], "1 + 1".to_owned()),
            documentation: arr_doc,
            units: arr_units,
            gf: None,
            ai_state: None,
            uid: None,
            compat: Compat::default(),
        });
        vars.push(arr);
        project_of(model_of(vars), vec![named_dim("DimB", &elem_refs)])
    })
}

// ---- properties ----

proptest! {
    // The default case count: each case runs a full project_to_mdl + parse_mdl,
    // but even 256 completes in a fraction of a second on a debug build.
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// #849 (the confirmed corruption vector): a variable's *documentation* is
    /// genuine free prose, so however full of structural characters (`|`, `~`,
    /// line breaks) and section-terminator runs it is, it must re-parse cleanly
    /// to exactly the one real variable -- never terminating the entry early,
    /// injecting a phantom variable, or dropping the real one. This is the exact
    /// property the confirmed `|`-in-doc corruption violated.
    #[test]
    fn doc_free_text_never_corrupts_variable_set(doc in free_text(), units in valid_units()) {
        let var = scalar_aux("x", "1", units, &doc);
        let project = project_of(model_of(vec![var]), vec![]);

        let mdl = project_to_mdl(&project);
        prop_assert!(mdl.is_ok(), "write failed: {:?}", mdl.err());
        let mdl = mdl.unwrap();

        let reparsed = parse_mdl(&mdl);
        prop_assert!(
            reparsed.is_ok(),
            "re-parse failed for doc={:?}; mdl:\n{}",
            doc,
            mdl
        );
        let reparsed = reparsed.unwrap();

        prop_assert_eq!(
            model_idents(&reparsed),
            vec!["x".to_owned()],
            "documentation free text corrupted the variable set; mdl:\n{}",
            mdl
        );
    }

    /// #849 structural guarantee for the *units* field. Units is not free text
    /// -- the reader parses it as a unit *expression* -- so adversarial content
    /// may legitimately fail the unit-expression grammar with a HARD, LOUD error
    /// (a bare number, or the `/` the sanitizer substitutes for a `|` left
    /// dangling). What the sanitizer DOES guarantee, and what this asserts, is
    /// that adversarial units never *silently* corrupt the model: the round trip
    /// either yields exactly the one real variable, or fails to parse outright --
    /// it never succeeds with a phantom or dropped variable. See the module
    /// handoff for the tracked observation that garbage units hard-fail re-import.
    #[test]
    fn units_free_text_is_structurally_safe(units in free_text()) {
        let var = scalar_aux("x", "1", Some(units.clone()), "");
        let project = project_of(model_of(vec![var]), vec![]);

        let mdl = project_to_mdl(&project).expect("write should never fail");
        // A hard parse failure is acceptable (unit-expression grammar); a
        // *successful* parse must recover exactly the real variable set.
        if let Ok(reparsed) = parse_mdl(&mdl) {
            prop_assert_eq!(
                model_idents(&reparsed),
                vec!["x".to_owned()],
                "adversarial units silently corrupted the variable set (units={:?}); mdl:\n{}",
                units,
                mdl
            );
        }
    }

    /// #846/#847/#850/#852: the context-aware printer's output must re-parse as
    /// MDL and be a re-write fixpoint.
    ///
    /// A `prop_filter` used to restrict this generator to ASTs whose `print_eqn`
    /// serialization re-parses, on the theory that the `print_eqn` <-> parser
    /// asymmetry was a separate concern. It was not: it masked #912. Every stored
    /// datamodel equation came from `Expr0::new`, so the printer's output must be
    /// text the parser accepts -- that is now asserted, not assumed away.
    ///
    /// The assertion is `parse(print_eqn(e)) == e` on the **AST**, not merely
    /// `is_ok()`. The weak form is satisfied by text that parses to a DIFFERENT
    /// expression -- a silent semantic corruption, strictly worse than a loud
    /// parse error. It is what the left-associative `^` printing (`(a^b)^c` as
    /// the bare `a^b^c`) and the un-parenthesized negated base (`(-a)^b` as
    /// `-a^b`, a sign flip) both slipped through.
    #[test]
    fn equation_write_reparse_is_fixpoint(expr in expr0_strategy()) {
        let xmile_eqn = print_eqn(&expr);
        let reparsed = Expr0::new(&xmile_eqn, crate::lexer::LexerType::Equation);
        prop_assert!(
            matches!(reparsed, Ok(Some(_))),
            "print_eqn output is not a valid datamodel equation: {}",
            xmile_eqn
        );
        prop_assert_eq!(
            expr.clone().strip_loc(),
            reparsed.unwrap().unwrap().strip_loc(),
            "print_eqn output re-parsed to a DIFFERENT AST: {}",
            xmile_eqn
        );

        let mut vars = scaffold_vars();
        vars.push(scalar_aux("target", &xmile_eqn, None, ""));
        let project = project_of(model_of(vars), vec![named_dim("DimA", &["A1", "A2"])]);

        let mdl1 = project_to_mdl(&project);
        prop_assert!(mdl1.is_ok(), "first write failed: {:?}", mdl1.err());
        let mdl1 = mdl1.unwrap();

        // The printer's output MUST re-parse as MDL.
        let p2 = parse_mdl(&mdl1);
        prop_assert!(p2.is_ok(), "printer output did not re-parse; mdl:\n{}", mdl1);
        let mdl2 = project_to_mdl(&p2.unwrap()).expect("second write");

        let p3 = parse_mdl(&mdl2);
        prop_assert!(p3.is_ok(), "second render did not re-parse; mdl:\n{}", mdl2);
        let mdl3 = project_to_mdl(&p3.unwrap()).expect("third write");

        // A second write is a fixpoint (first render absorbs XMILE->MDL
        // normalization and any first-import reordering).
        prop_assert_eq!(
            equations_section(&mdl2),
            equations_section(&mdl3),
            "equation render is not a fixpoint\nmdl2:\n{}\nmdl3:\n{}",
            mdl2,
            mdl3
        );
    }

    /// A whole generated model reaches a `write(parse(...))` fixpoint, and the
    /// warnings channel never errors on it.
    #[test]
    fn model_write_is_idempotent(project in idempotence_model()) {
        let (mdl1, _warnings) = project_to_mdl_with_warnings(&project)
            .expect("first write should succeed");

        let p2 = parse_mdl(&mdl1);
        prop_assert!(p2.is_ok(), "first re-parse failed; mdl:\n{}", mdl1);
        let mdl2 = project_to_mdl(&p2.unwrap()).expect("second write");

        let p3 = parse_mdl(&mdl2);
        prop_assert!(p3.is_ok(), "second re-parse failed; mdl:\n{}", mdl2);
        let mdl3 = project_to_mdl(&p3.unwrap()).expect("third write");

        prop_assert_eq!(
            equations_section(&mdl2),
            equations_section(&mdl3),
            "model write is not idempotent\nmdl2:\n{}\nmdl3:\n{}",
            mdl2,
            mdl3
        );
    }
}
