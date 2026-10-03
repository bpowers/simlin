// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Property-based tests for the MDL writer.
//!
//! The properties:
//!
//! 1. **Free-text sanitization** (#849): adversarial documentation strings --
//!    full of the structural characters (`|`, `~`, newlines, `\r`) and the
//!    section-terminator runs a corruption would exploit -- re-parse to exactly
//!    the real variable, never injecting a phantom or dropping it. Adversarial
//!    *units* get the weaker structural-only guarantee (see
//!    `units_free_text_is_structurally_safe` for why the units field, a typed
//!    unit expression, cannot promise the same).
//! 2. **A save keeps what it saves and is a fixed point**
//!    (`first_save_is_a_fixed_point`), over generated equations, small
//!    arrayed models and rich models with and without a view: the save reads
//!    back as the variables, elements, equations, initials and tables the
//!    generated project defines (`reads_back_as_generated`); every equation it
//!    writes holds Vensim's subscript rule (`subscript_rule`); the second save
//!    is the first; and the project the second save reads back as is the one
//!    the first does.
//!
//! ## Generator design choices
//!
//! - Equations are generated as `Expr0` ASTs from a bounded recursive grammar
//!   and serialized with `ast::print_eqn` to obtain the XMILE-syntax equation
//!   the datamodel stores, so no case is lost to an accidentally malformed
//!   input.
//! - A generator makes only what MDL can hold: a two-argument `INIT` (ACTIVE
//!   INITIAL) only at the top of an equation, and the grammar only the
//!   operators whose grouping `mdl::parser` reads as Vensim does (#914).
//! - Case counts are modest (each case runs full writes and reads) so the
//!   module stays within the few-seconds debug budget; a release run takes
//!   thousands (`PROPTEST_CASES`).

use super::*;
use crate::ast::{Loc, print_eqn};
use crate::builtins::UntypedBuiltinFn;
use crate::common::RawIdent;
use crate::datamodel::{
    self, Aux, Compat, Dimension, DimensionElements, Dt, Equation, Flow, GraphicalFunction,
    GraphicalFunctionKind, GraphicalFunctionScale, Model, Project, SimMethod, SimSpecs, Stock,
    Variable,
};
use crate::mdl::{parse_mdl, project_to_mdl, project_to_mdl_with_warnings};
use proptest::prelude::*;
use std::collections::BTreeMap;

// ---- shared helpers ----

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
        variables: variables.into(),
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
        (0i64..1000).prop_map(|n| Expr0::Const(
            n.to_string(),
            crate::ast::Literal::new(n as f64),
            Loc::default()
        )),
        Just(Expr0::Const(
            "0.5".to_owned(),
            crate::ast::Literal::new(0.5),
            Loc::default()
        )),
        Just(Expr0::Const(
            "1.5".to_owned(),
            crate::ast::Literal::new(1.5),
            Loc::default()
        )),
        Just(Expr0::Const(
            "3.5".to_owned(),
            crate::ast::Literal::new(3.5),
            Loc::default()
        )),
        Just(Expr0::Const(
            "2.25".to_owned(),
            crate::ast::Literal::new(2.25),
            Loc::default()
        )),
    ]
}

fn var_leaf() -> impl Strategy<Value = Expr0> {
    prop::sample::select(GRAMMAR_VARS)
        .prop_map(|name| Expr0::Var(RawIdent::new_from_str(name), Loc::default()))
}

fn app0(name: &str) -> Expr0 {
    Expr0::App(
        UntypedBuiltinFn(name.to_owned(), Box::new([])),
        Loc::default(),
    )
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
                Box::new([Expr0::Subscript(
                    RawIdent::new_from_str("arr"),
                    Box::new([IndexExpr0::Wildcard(Loc::default())]),
                    Loc::default(),
                )]),
            ),
            Loc::default(),
        )),
    ];

    let expr = leaf.prop_recursive(4, 48, 4, move |inner| {
        // DELIBERATELY narrow: arithmetic only, no comparisons and no `and`/`or`.
        //
        // This generator drives an MDL write -> `mdl::parser` re-read fixpoint, and
        // `mdl::parser`'s BINARY precedence table is inverted relative to Vensim and
        // XMILE (GH #914): it puts `+`/`-` at the lowest level and `:AND:` above the
        // comparisons. The writer's grouping (`written_shape` with
        // `ast::paren_if_necessary`) correctly targets *Vensim's* table,
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
                    UntypedBuiltinFn(f.to_owned(), Box::new([e])),
                    Loc::default()
                )),
            // INITIAL arity DISPATCH (#852): 1-arg `init` -> INITIAL here,
            // 2-arg `init` -> ACTIVE INITIAL at the top of the equation only
            // (below), the one place Vensim allows it: it "must appear first
            // on the right of the = sign and not be followed by anything
            // else" (vensim.com/documentation/fn_active_initial.html).
            inner.clone().prop_map(|e| Expr0::App(
                UntypedBuiltinFn("init".to_owned(), Box::new([e])),
                Loc::default()
            )),
            // two-argument builtins
            (
                prop::sample::select(&["min", "max"][..]),
                inner.clone(),
                inner.clone()
            )
                .prop_map(|(f, l, r)| Expr0::App(
                    UntypedBuiltinFn(f.to_owned(), Box::new([l, r])),
                    Loc::default()
                )),
            (inner.clone(), inner.clone(), inner.clone()).prop_map(|(c, t, f)| Expr0::If(
                Box::new(c),
                Box::new(t),
                Box::new(f),
                Loc::default()
            )),
        ]
    });
    prop_oneof![
        4 => expr.clone(),
        1 => (expr.clone(), expr).prop_map(|(e, ai)| Expr0::App(
            UntypedBuiltinFn("init".to_owned(), Box::new([e, ai])),
            Loc::default()
        )),
    ]
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

// ---- whole-model generator (property 4) ----

/// Numbers whose shortest spelling is awkward: tiny, huge, subnormal, not
/// representable in decimal, and one past where f64 counts integers.
const AWKWARD_NUMBERS: &[f64] = &[
    1e-7,
    0.1 + 0.2,
    1e21,
    5e-324,
    f64::MAX,
    2.5e-5,
    123_456_789.123,
    1.0 / 3.0,
    4.35,
    1e15 + 1.0,
    -2.5,
    0.0,
];

/// A number's text, as Rust spells an f64 shortest.
fn number_text() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(AWKWARD_NUMBERS).prop_map(|v| format!("{v}")),
        any::<f64>()
            .prop_filter("finite", |v| v.is_finite())
            .prop_map(|v| format!("{v}")),
    ]
}

/// An element's equation: a number, or one that reads the model.
fn element_equation() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => number_text(),
        1 => Just("TIME".to_owned()),
        1 => Just("alpha * 2 + 1".to_owned()),
    ]
}

const DIM_A: &[&str] = &["a1", "a2", "a3"];
const SUB_A: &[&str] = &["a2", "a3"];
const DIM_B: &[&str] = &["b1", "b2"];

/// The element keys of each choice of dimensions an arrayed variable is
/// over: `DimA`, its subrange `SubA`, or `DimA` by `DimB`.
fn arrayed_shape() -> impl Strategy<Value = (Vec<String>, Vec<String>)> {
    prop_oneof![
        Just((
            vec!["DimA".to_owned()],
            DIM_A.iter().map(|e| e.to_string()).collect()
        )),
        Just((
            vec!["SubA".to_owned()],
            SUB_A.iter().map(|e| e.to_string()).collect()
        )),
        Just((
            vec!["DimA".to_owned(), "DimB".to_owned()],
            DIM_A
                .iter()
                .flat_map(|a| DIM_B.iter().map(move |b| format!("{a},{b}")))
                .collect()
        )),
    ]
}

/// An arrayed variable's equation: some or all of its elements, each with
/// its own equation and maybe its own initial (an element's ACTIVE INITIAL,
/// so elements that agree on the equation can differ in it), and maybe an
/// `:EXCEPT:` default, applied or not.
fn arrayed_equation() -> impl Strategy<Value = Equation> {
    arrayed_shape()
        .prop_flat_map(|(dims, keys)| {
            let n = keys.len();
            (
                Just(dims),
                Just(keys),
                prop::collection::vec(any::<bool>(), n),
                prop::collection::vec(element_equation(), n),
                prop::collection::vec(prop::option::weighted(0.25, number_text()), n),
                prop::option::of(number_text()),
                any::<bool>(),
            )
        })
        .prop_map(
            |(dims, keys, kept, equations, initials, default, applies)| {
                let mut slots: Vec<_> = keys
                    .into_iter()
                    .zip(kept)
                    .zip(equations.into_iter().zip(initials))
                    .filter(|((_, kept), _)| *kept)
                    .map(|((key, _), (eqn, initial))| (key, eqn, initial, None))
                    .collect();
                if slots.is_empty() {
                    slots.push((
                        match dims[0].as_str() {
                            "SubA" => "a2".to_owned(),
                            _ if dims.len() == 2 => "a1,b1".to_owned(),
                            _ => "a1".to_owned(),
                        },
                        "1".to_owned(),
                        None,
                        None,
                    ));
                }
                let applies = applies && default.is_some();
                Equation::Arrayed(dims, slots, default, applies)
            },
        )
}

fn table(ys: Vec<f64>) -> GraphicalFunction {
    let n = ys.len();
    GraphicalFunction {
        kind: GraphicalFunctionKind::Continuous,
        x_points: Some((0..n).map(|i| i as f64).collect()),
        y_points: ys,
        x_scale: GraphicalFunctionScale {
            min: 0.0,
            max: (n - 1) as f64,
        },
        y_scale: GraphicalFunctionScale { min: 0.0, max: 1.0 },
    }
}

fn aux(ident: &str, equation: Equation, gf: Option<GraphicalFunction>) -> Variable {
    Variable::Aux(Aux {
        ident: ident.to_owned(),
        equation,
        documentation: String::new(),
        units: None,
        gf,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

fn flow(ident: &str, equation: Equation) -> Variable {
    Variable::Flow(Flow {
        ident: ident.to_owned(),
        equation,
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

fn stock(ident: &str, equation: Equation, inflows: &[&str], outflows: &[&str]) -> Variable {
    Variable::Stock(Stock {
        ident: ident.to_owned(),
        equation,
        documentation: String::new(),
        units: None,
        inflows: inflows.iter().map(|f| f.to_string()).collect(),
        outflows: outflows.iter().map(|f| f.to_string()).collect(),
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

/// A model with a bit of everything an MDL save writes: awkward numbers,
/// arrayed variables over a dimension, a subrange and two dimensions (some
/// elements defined, an `:EXCEPT:` default or not), a scalar and an arrayed
/// stock with their flows, a lookup table and its call, a WITH LOOKUP,
/// groups and a time unit or none.
fn rich_model() -> impl Strategy<Value = Project> {
    (
        number_text(),
        arrayed_equation(),
        arrayed_equation(),
        number_text(),
        prop::collection::vec(prop::sample::select(AWKWARD_NUMBERS), 2..5),
        prop::option::of(prop::sample::subsequence(
            vec!["alpha", "level", "fill", "tbl", "arr"],
            1..4,
        )),
        prop::option::of(Just("Months".to_owned())),
    )
        .prop_map(|(alpha, arr, arr2, level, ys, grouped, time_units)| {
            let ys: Vec<f64> = ys.into_iter().map(|y| y.clamp(-1e300, 1e300)).collect();
            let variables = vec![
                aux("alpha", Equation::Scalar(alpha), None),
                aux("arr", arr, None),
                aux("arr2", arr2, None),
                stock("level", Equation::Scalar(level), &["fill"], &["drain"]),
                flow("fill", Equation::Scalar("alpha".to_owned())),
                flow("drain", Equation::Scalar("level / 4".to_owned())),
                stock(
                    "pool",
                    Equation::Arrayed(
                        vec!["DimA".to_owned()],
                        DIM_A
                            .iter()
                            .enumerate()
                            .map(|(i, e)| (e.to_string(), (i + 1).to_string(), None, None))
                            .collect(),
                        None,
                        false,
                    ),
                    &["pour"],
                    &[],
                ),
                flow(
                    "pour",
                    Equation::ApplyToAll(vec!["DimA".to_owned()], "1".to_owned()),
                ),
                aux(
                    "tbl",
                    Equation::Scalar(String::new()),
                    Some(table(ys.clone())),
                ),
                aux(
                    "looked up",
                    Equation::Scalar("LOOKUP(tbl, TIME)".to_owned()),
                    None,
                ),
                aux(
                    "with lookup",
                    Equation::Scalar("TIME".to_owned()),
                    Some(table(ys)),
                ),
            ];
            let mut model = model_of(variables);
            if let Some(members) = grouped {
                model.groups = vec![datamodel::ModelGroup {
                    name: "Sector".to_owned(),
                    doc: None,
                    parent: None,
                    members: members.into_iter().map(str::to_owned).collect(),
                    run_enabled: false,
                }];
            }
            let mut project = project_of(
                model,
                vec![
                    named_dim("DimA", DIM_A),
                    named_dim("SubA", SUB_A),
                    named_dim("DimB", DIM_B),
                ],
            );
            project.sim_specs.time_units = time_units;
            project
        })
}

/// What each element of each variable of `project` is: its kind, and per
/// element (canonical element names, in its dimensions' order; none for a
/// scalar) the equation, initial and table that define it -- a slot's, or the
/// `:EXCEPT:` default where it applies to an element no slot holds. An element
/// nothing defines is not in the map.
type Elements = BTreeMap<Vec<String>, (String, Option<String>, Option<GraphicalFunction>)>;

fn defined_elements(project: &Project) -> BTreeMap<String, (&'static str, Vec<String>, Elements)> {
    let elements_of = |dim: &str| -> Vec<String> {
        project
            .dimensions
            .iter()
            .find(|d| crate::common::canonicalize(&d.name) == crate::common::canonicalize(dim))
            .map(|d| match &d.elements {
                DimensionElements::Named(names) => names
                    .iter()
                    .map(|e| crate::common::canonicalize(e).into_owned())
                    .collect(),
                DimensionElements::Indexed(n) => (1..=*n).map(|i| i.to_string()).collect(),
            })
            .unwrap_or_default()
    };
    let product = |dims: &[String]| -> Vec<Vec<String>> {
        dims.iter().fold(vec![Vec::new()], |keys, dim| {
            keys.into_iter()
                .flat_map(|key| {
                    elements_of(dim).into_iter().map(move |e| {
                        let mut next = key.clone();
                        next.push(e);
                        next
                    })
                })
                .collect()
        })
    };
    let key_of = |key: &str| -> Vec<String> {
        key.split(',')
            .map(|p| crate::common::canonicalize(p.trim()).into_owned())
            .collect()
    };
    let mut out = BTreeMap::new();
    for var in project.models[0].variables.iter() {
        let (kind, gf, compat) = match var {
            Variable::Stock(s) => ("stock", None, &s.compat),
            Variable::Flow(f) => ("flow", f.gf.clone(), &f.compat),
            Variable::Aux(a) => ("aux", a.gf.clone(), &a.compat),
            Variable::Module(_) => continue,
        };
        // A two-argument INIT is an ACTIVE INITIAL, which the reader keeps as
        // the equation and its initial.
        let active = |eqn: &str| -> (String, Option<String>) {
            if let Ok(Some(Expr0::App(UntypedBuiltinFn(f, args), _))) =
                Expr0::new(eqn, crate::lexer::LexerType::Equation)
                && f == "init"
                && let [equation, initial] = &args[..]
            {
                return (print_eqn(equation), Some(print_eqn(initial)));
            }
            (eqn.to_owned(), compat.active_initial.clone())
        };
        let mut elements = Elements::new();
        match var.get_equation() {
            Some(Equation::Scalar(eqn)) => {
                let (eqn, initial) = active(eqn);
                elements.insert(Vec::new(), (eqn, initial, gf));
            }
            Some(Equation::ApplyToAll(dims, eqn)) => {
                let (eqn, initial) = active(eqn);
                for key in product(dims) {
                    elements.insert(key, (eqn.clone(), initial.clone(), gf.clone()));
                }
            }
            Some(Equation::Arrayed(dims, slots, default, applies)) => {
                for (key, eqn, initial, slot_gf) in slots {
                    elements.insert(key_of(key), (eqn.clone(), initial.clone(), slot_gf.clone()));
                }
                if let Some(default) = default.as_ref().filter(|_| *applies) {
                    for key in product(dims) {
                        elements
                            .entry(key)
                            .or_insert_with(|| (default.clone(), None, None));
                    }
                }
            }
            None => {}
        }
        let dims: Vec<String> = match var.get_equation() {
            Some(Equation::ApplyToAll(dims, _)) | Some(Equation::Arrayed(dims, ..)) => dims
                .iter()
                .map(|d| crate::common::canonicalize(d).into_owned())
                .collect(),
            _ => Vec::new(),
        };
        out.insert(
            crate::common::canonicalize(var.get_ident()).into_owned(),
            (kind, dims, elements),
        );
    }
    out
}

/// Whether `read` defines what `generated` does: the same variables, each of
/// the same kind, over the same dimensions by name unless the save warns
/// that it is read back over others, with the same elements, each the same equation (numbers by
/// value, names canonically), initial and table. Err with the first
/// difference.
fn reads_back_as_generated(
    generated: &Project,
    read: &Project,
    warnings: &[super::ExportWarning],
) -> std::result::Result<(), String> {
    use super::arrayed::same_equation;
    let (a, b) = (defined_elements(generated), defined_elements(read));
    for (ident, (kind, dims, elements)) in &a {
        let Some((read_kind, read_dims, read_elements)) = b.get(ident) else {
            return Err(format!("'{ident}' does not read back"));
        };
        if kind != read_kind {
            return Err(format!(
                "'{ident}' is a {kind} and reads back as a {read_kind}"
            ));
        }
        // A variable over some elements of a dimension reads back over the
        // dimension of exactly those, and the save says so.
        let warned = warnings.iter().any(|w| {
            w.message.contains(&format!("'{ident}'")) && w.message.contains("read back over")
        });
        if dims != read_dims && !warned {
            return Err(format!(
                "'{ident}' is over {dims:?} and reads back over {read_dims:?}"
            ));
        }
        let keys = |e: &Elements| e.keys().cloned().collect::<Vec<_>>();
        if keys(elements) != keys(read_elements) {
            return Err(format!(
                "'{ident}' defines {:?} and reads back defining {:?}",
                keys(elements),
                keys(read_elements)
            ));
        }
        for (key, (eqn, initial, gf)) in elements {
            let (read_eqn, read_initial, read_gf) = &read_elements[key];
            let same_initial = match (initial, read_initial) {
                (Some(x), Some(y)) => same_equation(x, y),
                (None, None) => true,
                _ => false,
            };
            if !same_equation(eqn, read_eqn) || !same_initial || gf != read_gf {
                return Err(format!(
                    "'{ident}'{key:?} is {eqn:?} (initial {initial:?}) and reads back as \
                     {read_eqn:?} (initial {read_initial:?})"
                ));
            }
        }
    }
    if let Some(extra) = b.keys().find(|ident| !a.contains_key(*ident)) {
        return Err(format!("'{extra}' is new in the save"));
    }
    Ok(())
}

/// The first save of `project` keeps it and is a fixed point: it reads back
/// as what `project` defines (`reads_back_as_generated`), and as a project
/// whose save is the same text, and the project read back from that is the
/// one read back from the first (`save_roundtrip_tests::normalized`). Err
/// with what differs.
fn first_save_is_a_fixed_point(project: &Project) -> std::result::Result<(), String> {
    use crate::mdl::save_roundtrip_tests::{first_difference, normalized};
    let (save1, warnings) =
        project_to_mdl_with_warnings(project).map_err(|e| format!("first write: {e}"))?;
    let read1 = parse_mdl(&save1).map_err(|e| format!("first read: {e}\n{save1}"))?;
    reads_back_as_generated(project, &read1, &warnings).map_err(|why| format!("{why}\n{save1}"))?;
    let unheld = crate::mdl::subscript_rule::ranges_not_on_the_left(&save1, &read1);
    if !unheld.is_empty() {
        return Err(format!(
            "the save names a subscript range its left-hand side does not hold: {}\n{save1}",
            unheld.join("; ")
        ));
    }
    let save2 = project_to_mdl(&read1).map_err(|e| format!("second write: {e}"))?;
    if save1 != save2 {
        return Err(format!(
            "the save is not a fixed point:\n{save1}\n----\n{save2}"
        ));
    }
    let read2 = parse_mdl(&save2).map_err(|e| format!("second read: {e}"))?;
    match first_difference(&normalized(&read1), &normalized(&read2)) {
        Some(change) => Err(format!("{change}\n{save1}")),
        None => Ok(()),
    }
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

        // The printer's output MUST re-parse as MDL, and the first save is
        // the last that changes anything.
        if let Err(why) = first_save_is_a_fixed_point(&project) {
            prop_assert!(false, "{}", why);
        }
    }

    /// A whole generated model reaches a `write(parse(...))` fixpoint, and the
    /// warnings channel never errors on it.
    #[test]
    fn model_write_is_idempotent(project in idempotence_model()) {
        project_to_mdl_with_warnings(&project).expect("first write should succeed");
        if let Err(why) = first_save_is_a_fixed_point(&project) {
            prop_assert!(false, "{}", why);
        }
    }
}

proptest! {
    // Each case writes and reads a whole model twice.
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// A model with a bit of everything an MDL save writes: the first save is
    /// a fixed point, as text and as the datamodel it reads back as.
    #[test]
    fn a_rich_models_first_save_is_a_fixed_point(project in rich_model()) {
        if let Err(why) = first_save_is_a_fixed_point(&project) {
            prop_assert!(false, "{}", why);
        }
    }
}

/// `project` with a stock-and-flow view laid out for it, kept by its save.
fn a_save_with_a_view_is_a_fixed_point(project: Project) -> std::result::Result<(), TestCaseError> {
    let mut project = project;
    let view = crate::layout::generate_layout(&project, "main", None)
        .map_err(|e| TestCaseError::fail(format!("layout: {e}")))?;
    project.models[0].views = vec![datamodel::View::StockFlow(view)];
    first_save_is_a_fixed_point(&project).map_err(TestCaseError::fail)
}

proptest! {
    // Each case lays the model out before writing it; the gate below runs
    // many more.
    #![proptest_config(ProptestConfig::with_cases(2))]

    /// The same, with a stock-and-flow view laid out for the model.
    #[test]
    fn a_rich_models_first_save_with_a_view_is_a_fixed_point(project in rich_model()) {
        a_save_with_a_view_is_a_fixed_point(project)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    #[ignore = "lays out and saves 200 generated models; run under the gates profile"]
    fn many_rich_models_first_saves_with_a_view_are_fixed_points(project in rich_model()) {
        a_save_with_a_view_is_a_fixed_point(project)?;
    }
}
