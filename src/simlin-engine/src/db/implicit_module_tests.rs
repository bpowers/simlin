// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What a stdlib module-function call leaves behind in the implicit-var list,
//! and what every stage downstream of the parse is entitled to read off it.
//!
//! `builtins_visitor::expand_module_function` turns one `SMTH1`/`DELAY3`/...
//! call into a module instance plus one hoisted aux per argument that is not a
//! bare identifier. These tests pin the whole observable result of that
//! expansion -- the runlist idents, the target model, the input-port wiring,
//! the hoisted bodies, and the shared walk counter -- through the production
//! salsa parse.
//!
//! `ROWS` is the exhaustive artifact-shape contract for this expansion. Each
//! row pins the helper order and typed content. Helper idents determine runlist
//! order, layout's implicit section, and results-offset keys, so a different
//! spelling is an artifact change. [`describe`] is the single projection from
//! typed helpers into the row assertions, keeping those expectations
//! independent of the enum's storage details.

use std::collections::HashSet;

use super::*;
use crate::ast::print_eqn;
use crate::capture::{ImplicitModule, ImplicitVar, synthetic_ident};
use crate::model::VariableStage0;
use crate::test_common::TestProject;
use crate::variable::{ParseContext, VarKind, parse_var};

/// How the row's equation is attached to the model, which is what decides
/// whether the parse walks it once or once per element, and hence whether a
/// synthesized name carries an element suffix.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Parent {
    /// A scalar aux named `out`.
    Scalar,
    /// An apply-to-all aux `out[d]`: one equation, walked once per element
    /// because the body contains a module call (`per_element_requirements`).
    ApplyToAll,
    /// A per-element arrayed aux `out[d]` whose slots carry the SAME text but
    /// are distinct equations.
    PerElement,
    /// An arrayed aux whose default applies only to slots without an explicit
    /// body. Module instances must exist for those missing slots alone.
    ArrayedDefault,
}

/// One module-function call shape, plus everything a consumer reads off the
/// helpers it synthesizes.
struct ModuleRow {
    /// The arm of `expand_module_function` this row exercises.
    covers: &'static str,
    parent: Parent,
    equation: &'static str,
    /// Every helper the parse must synthesize, in walk order, rendered by
    /// [`describe`].
    ///
    /// The strings are written out rather than derived from a formatter,
    /// deliberately: they are the names the runlist sorts by and the offset map
    /// keys, so a test that re-derived them with the production formatter would
    /// agree with any spelling that formatter chose.
    helpers: &'static [&'static str],
}

/// Every stdlib module-function shape, one row per arm of
/// `expand_module_function`.
///
/// Arms NOT covered here, and where they are covered instead:
///
/// * the macro arms (`descriptor.is_macro`: the strict-arity refusal, and a
///   macro's non-`output` `primary_output`) -- `macro_expansion_tests`, which
///   can express the macro-marked model a `TestProject` cannot;
/// * the `#554` renamed-builtin self-call and the `#591-c1` passthrough
///   fall-throughs, which never reach `expand_module_function` --
///   `macro_expansion_tests`;
/// * `PREVIOUS`/`INIT` capture minting, which shares the walk counter but not
///   this function -- `db::capture_tests` (the shared counter itself IS covered
///   here, by the `previous-in-argument` row);
/// * the per-element expansion's dedup of identical helpers --
///   `db::fragment_determinism_tests`.
const ROWS: &[ModuleRow] = &[
    ModuleRow {
        covers: "the base shape: an identifier argument wires straight to its port and \
                 synthesizes no helper, a literal argument is hoisted",
        parent: Parent::Scalar,
        equation: "SMTH1(k, 2)",
        helpers: &[
            "$⁚out⁚0⁚arg1 = aux 2",
            "$⁚out⁚0⁚smth1 = module stdlib⁚smth1 [k->$⁚out⁚0⁚smth1.input, \
             $⁚out⁚0⁚arg1->$⁚out⁚0⁚smth1.delay_time]",
        ],
    },
    ModuleRow {
        covers: "a computed argument is hoisted into an aux whose body is the argument",
        parent: Parent::Scalar,
        equation: "SMTH1(k * 2, 3)",
        helpers: &[
            "$⁚out⁚0⁚arg0 = aux k * 2",
            "$⁚out⁚0⁚arg1 = aux 3",
            "$⁚out⁚0⁚smth1 = module stdlib⁚smth1 [$⁚out⁚0⁚arg0->$⁚out⁚0⁚smth1.input, \
             $⁚out⁚0⁚arg1->$⁚out⁚0⁚smth1.delay_time]",
        ],
    },
    ModuleRow {
        covers: "the optional trailing initial-value argument wires the third port; stdlib \
                 arity is lenient, so the row above wires only two",
        parent: Parent::Scalar,
        equation: "SMTH1(k, 2, 5)",
        helpers: &[
            "$⁚out⁚0⁚arg1 = aux 2",
            "$⁚out⁚0⁚arg2 = aux 5",
            "$⁚out⁚0⁚smth1 = module stdlib⁚smth1 [k->$⁚out⁚0⁚smth1.input, \
             $⁚out⁚0⁚arg1->$⁚out⁚0⁚smth1.delay_time, \
             $⁚out⁚0⁚arg2->$⁚out⁚0⁚smth1.initial_value]",
        ],
    },
    ModuleRow {
        covers: "a different stdlib model: the descriptor supplies model name and port list",
        parent: Parent::Scalar,
        equation: "DELAY3(k, 2)",
        helpers: &[
            "$⁚out⁚0⁚arg1 = aux 2",
            "$⁚out⁚0⁚delay3 = module stdlib⁚delay3 [k->$⁚out⁚0⁚delay3.input, \
             $⁚out⁚0⁚arg1->$⁚out⁚0⁚delay3.delay_time]",
        ],
    },
    ModuleRow {
        covers: "TREND: all three ports wired. Its port NAMES are the shared \
                 smooth/delay triple (`module_functions::stdlib_args` gives one list to \
                 smth1/smth3/delay/delay1/delay3/trend), not trend-specific spellings",
        parent: Parent::Scalar,
        equation: "TREND(k, 2, 1)",
        helpers: &[
            "$⁚out⁚0⁚arg1 = aux 2",
            "$⁚out⁚0⁚arg2 = aux 1",
            "$⁚out⁚0⁚trend = module stdlib⁚trend [k->$⁚out⁚0⁚trend.input, \
             $⁚out⁚0⁚arg1->$⁚out⁚0⁚trend.delay_time, \
             $⁚out⁚0⁚arg2->$⁚out⁚0⁚trend.initial_value]",
        ],
    },
    ModuleRow {
        covers: "SMTHN's order argument is consumed by `rewrite_alias_module_call`, which \
                 rewrites the call to SMTH3 and DEFAULTS the initial value to the input -- so \
                 the input expression is hoisted TWICE, under two names",
        parent: Parent::Scalar,
        equation: "SMTHN(k * 2, 4, 3)",
        helpers: &[
            "$⁚out⁚0⁚arg0 = aux k * 2",
            "$⁚out⁚0⁚arg1 = aux 4",
            "$⁚out⁚0⁚arg2 = aux k * 2",
            "$⁚out⁚0⁚smth3 = module stdlib⁚smth3 [$⁚out⁚0⁚arg0->$⁚out⁚0⁚smth3.input, \
             $⁚out⁚0⁚arg1->$⁚out⁚0⁚smth3.delay_time, \
             $⁚out⁚0⁚arg2->$⁚out⁚0⁚smth3.initial_value]",
        ],
    },
    ModuleRow {
        covers: "DELAYN order 1 rewrites to DELAY1, and its defaulted initial value is the \
                 duplicated input hoist the 7.5 shape list owns",
        parent: Parent::Scalar,
        equation: "DELAYN(k, 2, 1)",
        helpers: &[
            "$⁚out⁚0⁚arg1 = aux 2",
            "$⁚out⁚0⁚delay1 = module stdlib⁚delay1 [k->$⁚out⁚0⁚delay1.input, \
             $⁚out⁚0⁚arg1->$⁚out⁚0⁚delay1.delay_time, k->$⁚out⁚0⁚delay1.initial_value]",
        ],
    },
    ModuleRow {
        covers: "a nested call: the inner call is expanded first (args are walked before the \
                 outer expansion), so it takes counter 0 and the outer takes 1, and the outer \
                 reads the inner through its `module·output` name",
        parent: Parent::Scalar,
        equation: "SMTH1(SMTH1(k, 1), 2)",
        helpers: &[
            "$⁚out⁚0⁚arg1 = aux 1",
            "$⁚out⁚0⁚smth1 = module stdlib⁚smth1 [k->$⁚out⁚0⁚smth1.input, \
             $⁚out⁚0⁚arg1->$⁚out⁚0⁚smth1.delay_time]",
            "$⁚out⁚1⁚arg1 = aux 2",
            "$⁚out⁚1⁚smth1 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚smth1·output->$⁚out⁚1⁚smth1.input, \
             $⁚out⁚1⁚arg1->$⁚out⁚1⁚smth1.delay_time]",
        ],
    },
    ModuleRow {
        covers: "an argument that is a module-backed identifier stays an identifier: the \
                 wiring reads the sibling aux by name, and no helper is hoisted for it",
        parent: Parent::Scalar,
        equation: "SMTH1(smoothed, 2)",
        helpers: &[
            "$⁚out⁚0⁚arg1 = aux 2",
            "$⁚out⁚0⁚smth1 = module stdlib⁚smth1 [smoothed->$⁚out⁚0⁚smth1.input, \
             $⁚out⁚0⁚arg1->$⁚out⁚0⁚smth1.delay_time]",
        ],
    },
    ModuleRow {
        covers: "a PREVIOUS inside a module argument: the capture is minted during the \
                 argument walk, so it takes counter 0 and the module takes 1 -- the shared \
                 counter runs across both kinds of helper",
        parent: Parent::Scalar,
        equation: "SMTH1(PREVIOUS(k * 2, 0) + 1, 2)",
        helpers: &[
            "$⁚out⁚0⁚arg0 = capture[] k * 2",
            "$⁚out⁚1⁚arg0 = aux previous(\"$⁚out⁚0⁚arg0\", 0) + 1",
            "$⁚out⁚1⁚arg1 = aux 2",
            "$⁚out⁚1⁚smth1 = module stdlib⁚smth1 [$⁚out⁚1⁚arg0->$⁚out⁚1⁚smth1.input, \
             $⁚out⁚1⁚arg1->$⁚out⁚1⁚smth1.delay_time]",
        ],
    },
    ModuleRow {
        covers: "an apply-to-all body: the parse expands one instance per element, every \
                 synthesized name carries the element suffix, and a subscripted argument is \
                 substituted to that element",
        parent: Parent::ApplyToAll,
        equation: "SMTH1(vals[d], 2)",
        helpers: &[
            "$⁚out⁚0⁚arg0⁚e1 = aux vals[d·e1]",
            "$⁚out⁚0⁚arg1⁚e1 = aux 2",
            "$⁚out⁚0⁚smth1⁚e1 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e1->$⁚out⁚0⁚smth1⁚e1.input, \
             $⁚out⁚0⁚arg1⁚e1->$⁚out⁚0⁚smth1⁚e1.delay_time]",
            "$⁚out⁚0⁚arg0⁚e2 = aux vals[d·e2]",
            "$⁚out⁚0⁚arg1⁚e2 = aux 2",
            "$⁚out⁚0⁚smth1⁚e2 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e2->$⁚out⁚0⁚smth1⁚e2.input, \
             $⁚out⁚0⁚arg1⁚e2->$⁚out⁚0⁚smth1⁚e2.delay_time]",
            "$⁚out⁚0⁚arg0⁚e3 = aux vals[d·e3]",
            "$⁚out⁚0⁚arg1⁚e3 = aux 2",
            "$⁚out⁚0⁚smth1⁚e3 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e3->$⁚out⁚0⁚smth1⁚e3.input, \
             $⁚out⁚0⁚arg1⁚e3->$⁚out⁚0⁚smth1⁚e3.delay_time]",
        ],
    },
    ModuleRow {
        covers: "an apply-to-all body whose argument is the bare DIMENSION name: the \
                 identifier arm substitutes it to the qualified element and wires that name \
                 directly, hoisting nothing",
        parent: Parent::ApplyToAll,
        equation: "SMTH1(d, 2)",
        helpers: &[
            "$⁚out⁚0⁚arg1⁚e1 = aux 2",
            "$⁚out⁚0⁚smth1⁚e1 = module stdlib⁚smth1 [d·e1->$⁚out⁚0⁚smth1⁚e1.input, \
             $⁚out⁚0⁚arg1⁚e1->$⁚out⁚0⁚smth1⁚e1.delay_time]",
            "$⁚out⁚0⁚arg1⁚e2 = aux 2",
            "$⁚out⁚0⁚smth1⁚e2 = module stdlib⁚smth1 [d·e2->$⁚out⁚0⁚smth1⁚e2.input, \
             $⁚out⁚0⁚arg1⁚e2->$⁚out⁚0⁚smth1⁚e2.delay_time]",
            "$⁚out⁚0⁚arg1⁚e3 = aux 2",
            "$⁚out⁚0⁚smth1⁚e3 = module stdlib⁚smth1 [d·e3->$⁚out⁚0⁚smth1⁚e3.input, \
             $⁚out⁚0⁚arg1⁚e3->$⁚out⁚0⁚smth1⁚e3.delay_time]",
        ],
    },
    ModuleRow {
        covers: "a per-element arrayed equation: each slot is its own equation walked by its \
                 own visitor, so every slot restarts the counter at 0 and is kept apart by \
                 the element suffix alone",
        parent: Parent::PerElement,
        equation: "SMTH1(vals[d], 2)",
        helpers: &[
            "$⁚out⁚0⁚arg0⁚e1 = aux vals[d·e1]",
            "$⁚out⁚0⁚arg1⁚e1 = aux 2",
            "$⁚out⁚0⁚smth1⁚e1 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e1->$⁚out⁚0⁚smth1⁚e1.input, \
             $⁚out⁚0⁚arg1⁚e1->$⁚out⁚0⁚smth1⁚e1.delay_time]",
            "$⁚out⁚0⁚arg0⁚e2 = aux vals[d·e2]",
            "$⁚out⁚0⁚arg1⁚e2 = aux 2",
            "$⁚out⁚0⁚smth1⁚e2 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e2->$⁚out⁚0⁚smth1⁚e2.input, \
             $⁚out⁚0⁚arg1⁚e2->$⁚out⁚0⁚smth1⁚e2.delay_time]",
            "$⁚out⁚0⁚arg0⁚e3 = aux vals[d·e3]",
            "$⁚out⁚0⁚arg1⁚e3 = aux 2",
            "$⁚out⁚0⁚smth1⁚e3 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e3->$⁚out⁚0⁚smth1⁚e3.input, \
             $⁚out⁚0⁚arg1⁚e3->$⁚out⁚0⁚smth1⁚e3.delay_time]",
        ],
    },
    ModuleRow {
        covers: "an arrayed default: module-bearing defaults materialize only the missing \
                 element bodies, never the explicit override",
        parent: Parent::ArrayedDefault,
        equation: "SMTH1(vals[d], 2)",
        helpers: &[
            "$⁚out⁚0⁚arg0⁚e2 = aux vals[d·e2]",
            "$⁚out⁚0⁚arg1⁚e2 = aux 2",
            "$⁚out⁚0⁚smth1⁚e2 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e2->$⁚out⁚0⁚smth1⁚e2.input, \
             $⁚out⁚0⁚arg1⁚e2->$⁚out⁚0⁚smth1⁚e2.delay_time]",
            "$⁚out⁚0⁚arg0⁚e3 = aux vals[d·e3]",
            "$⁚out⁚0⁚arg1⁚e3 = aux 2",
            "$⁚out⁚0⁚smth1⁚e3 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e3->$⁚out⁚0⁚smth1⁚e3.input, \
             $⁚out⁚0⁚arg1⁚e3->$⁚out⁚0⁚smth1⁚e3.delay_time]",
        ],
    },
];

/// One synthesized helper, rendered as what a consumer is entitled to read off
/// it: its runlist ident, which kind of helper it is, and its whole definition
/// -- a module's target model and input wiring, or an aux's body printed back
/// to equation text.
///
/// This is the ONE function chunk 7.3a is allowed to rewrite. Everything it
/// renders is observable downstream, so the row expectations above must survive
/// the rewrite unchanged.
fn describe(v: &ImplicitVar) -> String {
    match v {
        ImplicitVar::Capture(c) => format!(
            "{} = capture[{}] {}",
            c.ident(),
            c.dims().join(","),
            print_eqn(c.arg())
        ),
        ImplicitVar::HoistedArg(a) => format!("{} = aux {}", a.ident(), print_eqn(a.arg())),
        ImplicitVar::Module(m) => {
            let refs: Vec<String> = m
                .references()
                .iter()
                .map(|r| format!("{}->{}", r.src, r.dst))
                .collect();
            format!(
                "{} = module {} [{}]",
                m.ident(),
                m.model_name(),
                refs.join(", ")
            )
        }
    }
}

/// The model a row describes, plus the name of the variable holding its
/// equation.
fn model_for(row: &ModuleRow) -> (TestProject, &'static str) {
    let base = TestProject::new("implicit-modules")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("d", &["e1", "e2", "e3"])
        .array_with_ranges("vals[d]", vec![("e1", "30"), ("e2", "10"), ("e3", "20")])
        .scalar_aux("k", "3")
        .aux("smoothed", "SMTH1(k, 2)", None);
    match row.parent {
        Parent::Scalar => (base.aux("out", row.equation, None), "out"),
        Parent::ApplyToAll => (base.array_aux("out[d]", row.equation), "out"),
        Parent::PerElement => (
            base.array_with_ranges(
                "out[d]",
                vec![
                    ("e1", row.equation),
                    ("e2", row.equation),
                    ("e3", row.equation),
                ],
            ),
            "out",
        ),
        Parent::ArrayedDefault => (
            base.array_with_default_and_overrides("out[d]", row.equation, vec![("e1", "999")]),
            "out",
        ),
    }
}

/// The helpers one variable's production parse synthesized, in walk order.
///
/// Read through the production per-variable parse, so these are the helpers
/// the compiler sees rather than a re-derivation.
fn helpers_of(db: &SimlinDb, sync: &SyncResult, model_name: &str, var: &str) -> Vec<ImplicitVar> {
    let model = sync.models[model_name].source;
    let source_var = model.variables(db)[var];
    parse_source_variable(db, source_var, sync.project)
        .implicit_vars
        .to_vec()
}

/// Build a row's model and hand back its synthesized helpers.
fn helpers_for(row: &ModuleRow) -> Vec<ImplicitVar> {
    let (tp, var) = model_for(row);
    let dm = tp.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &dm);
    helpers_of(&db, &sync, "main", var)
}

/// Parse one source equation through the production parser and return one call
/// argument exactly as parsed, including every source location.
fn parsed_call_arg(equation: &str, index: usize) -> crate::ast::Expr0 {
    let parsed = crate::ast::Expr0::new(equation, crate::lexer::LexerType::Equation)
        .expect("the fixture equation must lex")
        .expect("the fixture equation must parse");
    let crate::ast::Expr0::App(crate::builtins::UntypedBuiltinFn(_, args), _) = parsed else {
        panic!("the fixture must be one top-level call")
    };
    args[index].clone()
}

/// A computed stdlib argument is the parser's exact subtree, not a tree made by
/// printing and reparsing that subtree.
#[test]
fn computed_stdlib_argument_keeps_the_exact_source_subtree() {
    let equation = "SMTH1(k * 2 + 1, 3)";
    let expected = parsed_call_arg(equation, 0);
    let loc = expected.get_loc();
    assert!(
        loc.start > 0 && loc.end > loc.start,
        "the defect detector requires a nonzero argument span, got {loc}"
    );

    let project = TestProject::new("stdlib-subtree")
        .scalar_aux("k", "3")
        .aux("out", equation, None)
        .build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let helpers = helpers_of(&db, &sync, "main", "out");
    let hoisted = helpers
        .iter()
        .find_map(|helper| match helper {
            ImplicitVar::HoistedArg(arg) if arg.ident() == "$⁚out⁚0⁚arg0" => Some(arg),
            ImplicitVar::Capture(_) | ImplicitVar::HoistedArg(_) | ImplicitVar::Module(_) => None,
        })
        .expect("the computed first argument must be hoisted");

    assert!(
        hoisted.arg() == &expected,
        "the hoisted argument must retain the production parser's exact Expr0, including Loc"
    );
    let reparsed = crate::ast::Expr0::new(&print_eqn(&expected), crate::lexer::LexerType::Equation)
        .expect("the printed comparison tree must lex")
        .expect("the printed comparison tree must parse");
    assert!(
        reparsed != expected,
        "the nonzero source span must distinguish the carried subtree from a print/reparse tree"
    );
}

/// An implicit module's lookup key and port destinations are derived from its
/// logical callsite inputs; callers cannot supply a contradictory ident.
#[test]
fn implicit_module_derives_its_ident_and_wiring_from_callsite_inputs() {
    let module = ImplicitModule::new(
        "parent",
        7,
        "smth1",
        "stdlib⁚smth1".to_string(),
        vec![("source".to_string(), "input".to_string())],
        Some("e2".to_string()),
    );
    let expected_ident = synthetic_ident("parent", 7, "smth1", Some("e2"));
    assert_eq!(module.ident(), expected_ident);
    assert_eq!(module.id(), 7);
    assert_eq!(module.suffix(), Some("e2"));
    assert_eq!(
        module.references(),
        &[datamodel::ModuleReference {
            src: "source".to_string(),
            dst: format!("{expected_ident}.input"),
        }]
    );
}

/// Every production module shares its typed walk id and active-element suffix
/// with each hoisted argument wired into it.
#[test]
fn every_module_and_its_hoisted_arguments_share_callsite_fields() {
    for row in ROWS {
        let helpers = helpers_for(row);
        for module in helpers.iter().filter_map(ImplicitVar::module) {
            let expected_suffix = module.suffix();
            let call_name = module
                .model_name()
                .strip_prefix("stdlib⁚")
                .expect("the row table covers stdlib module calls");
            assert_eq!(
                module.ident(),
                synthetic_ident("out", module.id(), call_name, expected_suffix),
                "{}: module ident must derive from its typed callsite inputs",
                row.covers
            );
            for reference in module.references() {
                let Some(arg) = helpers.iter().find_map(|helper| match helper {
                    ImplicitVar::HoistedArg(arg) if arg.ident() == reference.src => Some(arg),
                    ImplicitVar::Capture(_)
                    | ImplicitVar::HoistedArg(_)
                    | ImplicitVar::Module(_) => None,
                }) else {
                    continue;
                };
                assert_eq!(arg.id(), module.id(), "{}: shared call id", row.covers);
                assert_eq!(
                    arg.suffix(),
                    expected_suffix,
                    "{}: shared active-element suffix",
                    row.covers
                );
            }
        }
    }
}

/// A cross-kind helper collision reachable from user source is refused by the
/// dt/initial merge and surfaces as an attributed diagnostic.
///
/// Both equation passes restart the helper counter at zero. The dt `SMTH1`
/// hoists its computed first argument as `$⁚out⁚0⁚arg0`; ACTIVE INITIAL reads
/// a different equation and mints a `PREVIOUS` capture under that same name.
/// The production parse must not retain one body and silently run it in both
/// phases.
#[test]
fn active_initial_capture_cannot_replace_dt_hoisted_argument() {
    let mut project = TestProject::new("cross-kind-helper-collision")
        .with_sim_time(0.0, 2.0, 1.0)
        .scalar_aux("k", "3")
        .scalar_aux("out", "SMTH1(k * 2, 1)")
        .build_datamodel();
    let out = project.models[0]
        .variables
        .iter_mut()
        .find(|variable| variable.get_ident() == "out")
        .expect("the fixture must contain out");
    let datamodel::Variable::Aux(out) = out else {
        panic!("out must be an aux")
    };
    out.compat.active_initial = Some("PREVIOUS(k * 100, 0)".to_string());

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let out = sync.models["main"].variables["out"].source;
    let parsed = parse_source_variable(&db, out, sync.project);
    let collision_name = "$⁚out⁚0⁚arg0";
    assert!(
        parsed.implicit_vars.iter().any(
            |helper| matches!(helper, ImplicitVar::HoistedArg(arg) if arg.ident() == collision_name)
        ),
        "the retained dt helper must be the computed module argument"
    );
    assert!(
        parsed.variable.errors.iter().any(|error| {
            error.code == crate::common::ErrorCode::DuplicateVariable
                && error
                    .details
                    .as_deref()
                    .is_some_and(|details| details.contains(collision_name))
        }),
        "the production dt/initial merge must report the cross-kind collision"
    );

    let diagnostics = collect_all_diagnostics(&db, sync.project);
    assert!(
        diagnostics.iter().any(|diagnostic| {
            diagnostic.severity == DiagnosticSeverity::Error
                && diagnostic.variable.as_deref() == Some("out")
                && matches!(
                    &diagnostic.error,
                    DiagnosticError::Equation(error)
                        if error.code == crate::common::ErrorCode::DuplicateVariable
                            && error.details.as_deref().is_some_and(|details| details.contains(collision_name))
                )
        }),
        "the cross-kind collision must surface through collection: {diagnostics:?}"
    );
    let error = compile_project_incremental(&db, sync.project, "main")
        .expect_err("a cross-kind same-name helper collision must refuse compilation");
    assert!(
        error.to_string().contains("out"),
        "the refusal must name the source variable: {error}"
    );
}

/// Every module-function shape, by what each helper is filed under, what it is,
/// and what it is wired to.
///
/// The idents are the load-bearing assertion: every runlist is a lexicographic
/// sort, the layout's implicit section and the results offset map are
/// name-sorted, so a helper filed under a different string moves the compiled
/// artifact even when it computes the same value. The ORDER is load-bearing
/// too: it rides two salsa-cached values with derived `PartialEq`, so a
/// reordering defeats backdating (GH #1002).
#[test]
fn every_module_call_shape_carries_its_idents_model_and_wiring() {
    for row in ROWS {
        let what = row.covers;
        let eqn = row.equation;
        let observed: Vec<String> = helpers_for(row).iter().map(describe).collect();
        let expected: Vec<String> = row.helpers.iter().map(|h| (*h).to_string()).collect();
        assert_eq!(
            observed, expected,
            "{what}: `{eqn}` -- synthesized helper list"
        );
    }
}

/// Every module instance the parse synthesizes names a stdlib model that
/// actually exists, and wires only ports that model declares.
///
/// The row table above pins the exact spelling; this pins that the spelling
/// means something, so a rename of a stdlib port or model cannot leave the rows
/// agreeing with a dangling reference.
#[test]
fn every_synthesized_module_targets_a_real_stdlib_model_and_port() {
    let stdlib: HashMap<String, HashSet<String>> = crate::stdlib::MODEL_NAMES
        .iter()
        .filter_map(|name| {
            let model = crate::stdlib::get(name)?;
            let inputs: HashSet<String> = model
                .variables
                .iter()
                .map(|v| canonicalize(v.get_ident()).into_owned())
                .collect();
            Some((format!("stdlib\u{205A}{name}"), inputs))
        })
        .collect();

    for row in ROWS {
        for helper in helpers_for(row) {
            let Some(m) = helper.module() else { continue };
            let ports = stdlib.get(m.model_name()).unwrap_or_else(|| {
                panic!(
                    "{}: `{}` -- synthesized module targets unknown model {}",
                    row.covers,
                    row.equation,
                    m.model_name()
                )
            });
            for mr in m.references() {
                let (instance, port) = mr
                    .dst
                    .rsplit_once('.')
                    .expect("a synthesized module reference dst is `{instance}.{port}`");
                assert_eq!(
                    instance,
                    helper.ident(),
                    "{}: `{}` -- wiring dst names its own instance",
                    row.covers,
                    row.equation
                );
                assert!(
                    ports.contains(&canonicalize(port).into_owned()),
                    "{}: `{}` -- module {} has no port {port}",
                    row.covers,
                    row.equation,
                    m.model_name()
                );
            }
        }
    }
}

/// Every typed helper's `variable_stage0` agrees with `parse_var` over an
/// equivalent datamodel source.
///
/// Inputs come from the production salsa parse. The test-only datamodel value
/// is built field-for-field from the emitted helper and passed through the same
/// `parse_var` production entry point ordinary variables use.
#[test]
fn every_helper_stage0_matches_parsing_its_equivalent_datamodel_variable() {
    for row in ROWS {
        let (tp, var) = model_for(row);
        let dm = tp.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &dm);
        let dim_ctx = project_dimensions_context(&db, sync.project);
        let units_ctx = project_units_context(&db, sync.project);
        let parse_ctx = ParseContext::new(dim_ctx, units_ctx);

        for helper in helpers_of(&db, &sync, "main", var) {
            let (actual, equivalent) = match &helper {
                ImplicitVar::Capture(_) => continue,
                ImplicitVar::HoistedArg(arg) => (
                    arg.variable_stage0(dim_ctx),
                    datamodel::Variable::Aux(datamodel::Aux {
                        ident: arg.ident().to_string(),
                        equation: datamodel::Equation::Scalar(print_eqn(arg.arg())),
                        documentation: String::new(),
                        units: None,
                        gf: None,
                        ai_state: None,
                        uid: None,
                        compat: datamodel::Compat::default(),
                    }),
                ),
                ImplicitVar::Module(module) => (
                    module.variable_stage0(),
                    datamodel::Variable::Module(datamodel::Module {
                        ident: module.ident().to_string(),
                        model_name: module.model_name().to_string(),
                        documentation: String::new(),
                        units: None,
                        references: module.references().to_vec(),
                        compat: datamodel::Compat::default(),
                        ai_state: None,
                        uid: None,
                    }),
                ),
            };
            let mut nested = Vec::new();
            let expected = parse_var(&parse_ctx, &equivalent, &mut nested, |input| {
                Ok(Some(input.clone()))
            });
            assert!(
                nested.is_empty(),
                "a helper must not synthesize another helper"
            );
            assert_stage0_equivalent(row, helper.ident(), &actual, &expected);
        }
    }
}

fn assert_stage0_equivalent(
    row: &ModuleRow,
    ident: &str,
    actual: &VariableStage0,
    expected: &VariableStage0,
) {
    let what = row.covers;
    assert_eq!(actual.ident, expected.ident, "{what}: ident of {ident}");
    assert_eq!(actual.units, expected.units, "{what}: units of {ident}");
    assert_eq!(actual.eqn, expected.eqn, "{what}: equation of {ident}");
    assert_eq!(actual.errors, expected.errors, "{what}: errors of {ident}");
    assert!(
        actual.unit_errors == expected.unit_errors,
        "{what}: unit errors of {ident}"
    );

    match (&actual.kind, &expected.kind) {
        (
            VarKind::Aux {
                ast: actual_ast,
                init_ast: actual_init,
                tables: actual_tables,
                non_negative: actual_non_negative,
                is_flow: actual_is_flow,
                is_table_only: actual_is_table_only,
            },
            VarKind::Aux {
                ast: expected_ast,
                init_ast: expected_init,
                tables: expected_tables,
                non_negative: expected_non_negative,
                is_flow: expected_is_flow,
                is_table_only: expected_is_table_only,
            },
        ) => {
            let print_ast = |ast: &crate::ast::Ast<crate::ast::Expr0>| match ast {
                crate::ast::Ast::Scalar(expr) | crate::ast::Ast::ApplyToAll(_, expr) => {
                    print_eqn(expr)
                }
                crate::ast::Ast::Arrayed(_, elements, default, apply_default) => format!(
                    "{:?}|{:?}|{apply_default}",
                    elements
                        .iter()
                        .map(|(element, expr)| (element, print_eqn(expr)))
                        .collect::<Vec<_>>(),
                    default.as_ref().map(print_eqn)
                ),
            };
            assert_eq!(
                actual_ast.as_ref().map(print_ast),
                expected_ast.as_ref().map(print_ast),
                "{what}: body of {ident}"
            );
            assert_eq!(
                actual_init.as_ref().map(print_ast),
                expected_init.as_ref().map(print_ast),
                "{what}: initial body of {ident}"
            );
            assert_eq!(actual_tables, expected_tables, "{what}: tables of {ident}");
            assert_eq!(
                (*actual_non_negative, *actual_is_flow, *actual_is_table_only),
                (
                    *expected_non_negative,
                    *expected_is_flow,
                    *expected_is_table_only
                ),
                "{what}: aux flags of {ident}"
            );
        }
        (
            VarKind::Module {
                model_name: actual_model,
                inputs: actual_inputs,
            },
            VarKind::Module {
                model_name: expected_model,
                inputs: expected_inputs,
            },
        ) => {
            assert_eq!(actual_model, expected_model, "{what}: model of {ident}");
            assert_eq!(actual_inputs, expected_inputs, "{what}: wiring of {ident}");
        }
        _ => panic!("{what}: helper {ident} changed variable kind"),
    }
}
