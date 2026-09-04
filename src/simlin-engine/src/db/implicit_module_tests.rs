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
//! The idents in the rows are written out rather than derived from a
//! formatter, deliberately: they are the names every runlist sorts by, the
//! layout's implicit section files, and the results offset map keys, so a
//! single moved string moves the compiled artifact. [`describe`] is the only
//! part of this file that renders a representation.

use std::collections::HashSet;

use super::*;
use crate::ast::{Expr0, print_eqn};
use crate::capture::ImplicitVar;
use crate::common::ErrorCode;
use crate::lexer::LexerType;
use crate::test_common::{TestProject, implicit_vars_of};

/// How the row's equation is attached to the model, which is what decides
/// whether the parse walks it once or once per element, and hence whether a
/// synthesized name carries an element suffix.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Parent {
    /// A scalar aux named `out`.
    Scalar,
    /// An apply-to-all aux `out[d]`: one equation, walked once per element
    /// because the body contains a module call
    /// (`builtins_visitor::per_element_requirements`).
    ApplyToAll,
    /// A per-element arrayed aux `out[d]` whose slots carry the SAME text but
    /// are distinct equations.
    PerElement,
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
    helpers: &'static [&'static str],
}

/// Every stdlib module-function shape, one row per arm of
/// `expand_module_function`.
///
/// Arms NOT covered here, and where they are covered instead:
///
/// * the macro arms (a macro's strict arity, a non-`output` primary output,
///   the routing exceptions that never reach `expand_module_function`) --
///   `macro_expansion_tests`, which can express the macro-marked model a
///   `TestProject` cannot;
/// * `PREVIOUS`/`INIT` capture minting, which shares the walk counter but not
///   this function -- `db::capture_tests` (the shared counter itself IS covered
///   here, by the `previous-in-argument` row);
/// * the per-element expansion's collapse of identical helpers --
///   `db::capture_tests` (the GH #541 row) and `db::fragment_determinism_tests`.
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
                 rewrites the call to SMTH3 and wires the two ports the call names; the \
                 initial-value port stays unwired, and the stdlib model's \
                 `isModuleInput(initial_value)` guard falls back to the input, as XMILE 1.0 \
                 section 3.5.3 says it must",
        parent: Parent::Scalar,
        equation: "SMTHN(k * 2, 4, 3)",
        helpers: &[
            "$⁚out⁚0⁚arg0 = aux k * 2",
            "$⁚out⁚0⁚arg1 = aux 4",
            "$⁚out⁚0⁚smth3 = module stdlib⁚smth3 [$⁚out⁚0⁚arg0->$⁚out⁚0⁚smth3.input, \
             $⁚out⁚0⁚arg1->$⁚out⁚0⁚smth3.delay_time]",
        ],
    },
    ModuleRow {
        covers: "DELAYN order 1 rewrites to DELAY1 with the same two ports",
        parent: Parent::Scalar,
        equation: "DELAYN(k, 2, 1)",
        helpers: &[
            "$⁚out⁚0⁚arg1 = aux 2",
            "$⁚out⁚0⁚delay1 = module stdlib⁚delay1 [k->$⁚out⁚0⁚delay1.input, \
             $⁚out⁚0⁚arg1->$⁚out⁚0⁚delay1.delay_time]",
        ],
    },
    ModuleRow {
        covers: "DELAYN's explicit fourth argument is an independent port: it is hoisted \
                 under the position it takes after the order is consumed and wired to \
                 `initial_value`",
        parent: Parent::Scalar,
        equation: "DELAYN(k, 2, 3, 7)",
        helpers: &[
            "$⁚out⁚0⁚arg1 = aux 2",
            "$⁚out⁚0⁚arg2 = aux 7",
            "$⁚out⁚0⁚delay3 = module stdlib⁚delay3 [k->$⁚out⁚0⁚delay3.input, \
             $⁚out⁚0⁚arg1->$⁚out⁚0⁚delay3.delay_time, \
             $⁚out⁚0⁚arg2->$⁚out⁚0⁚delay3.initial_value]",
        ],
    },
    ModuleRow {
        covers: "SMTHN order 1 with an explicit initial value: the SMTH1 twin of the row above",
        parent: Parent::Scalar,
        equation: "SMTHN(k, 2, 1, k * 3)",
        helpers: &[
            "$⁚out⁚0⁚arg1 = aux 2",
            "$⁚out⁚0⁚arg2 = aux k * 3",
            "$⁚out⁚0⁚smth1 = module stdlib⁚smth1 [k->$⁚out⁚0⁚smth1.input, \
             $⁚out⁚0⁚arg1->$⁚out⁚0⁚smth1.delay_time, \
             $⁚out⁚0⁚arg2->$⁚out⁚0⁚smth1.initial_value]",
        ],
    },
    ModuleRow {
        covers: "DELAY is a rename to DELAY1 and nothing else: its arguments pass through \
                 untouched",
        parent: Parent::Scalar,
        equation: "DELAY(k, 2)",
        helpers: &[
            "$⁚out⁚0⁚arg1 = aux 2",
            "$⁚out⁚0⁚delay1 = module stdlib⁚delay1 [k->$⁚out⁚0⁚delay1.input, \
             $⁚out⁚0⁚arg1->$⁚out⁚0⁚delay1.delay_time]",
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
                 hoisted UNREWRITTEN into a helper scoped to that element -- the compiler \
                 lowers it as element `e` of the body, so it reads what the plain equation \
                 reads (GH #1035)",
        parent: Parent::ApplyToAll,
        equation: "SMTH1(vals[d], 2)",
        helpers: &[
            "$⁚out⁚0⁚arg0⁚e1 = aux vals[d] in d=e1",
            "$⁚out⁚0⁚arg1⁚e1 = aux 2 in d=e1",
            "$⁚out⁚0⁚smth1⁚e1 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e1->$⁚out⁚0⁚smth1⁚e1.input, \
             $⁚out⁚0⁚arg1⁚e1->$⁚out⁚0⁚smth1⁚e1.delay_time]",
            "$⁚out⁚0⁚arg0⁚e2 = aux vals[d] in d=e2",
            "$⁚out⁚0⁚arg1⁚e2 = aux 2 in d=e2",
            "$⁚out⁚0⁚smth1⁚e2 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e2->$⁚out⁚0⁚smth1⁚e2.input, \
             $⁚out⁚0⁚arg1⁚e2->$⁚out⁚0⁚smth1⁚e2.delay_time]",
            "$⁚out⁚0⁚arg0⁚e3 = aux vals[d] in d=e3",
            "$⁚out⁚0⁚arg1⁚e3 = aux 2 in d=e3",
            "$⁚out⁚0⁚smth1⁚e3 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e3->$⁚out⁚0⁚smth1⁚e3.input, \
             $⁚out⁚0⁚arg1⁚e3->$⁚out⁚0⁚smth1⁚e3.delay_time]",
        ],
    },
    ModuleRow {
        covers: "an apply-to-all body whose argument is the bare DIMENSION name: it means \
                 nothing without the element, so the identifier arm hoists it into an \
                 element-scoped helper rather than wiring the name",
        parent: Parent::ApplyToAll,
        equation: "SMTH1(d, 2)",
        helpers: &[
            "$⁚out⁚0⁚arg0⁚e1 = aux d in d=e1",
            "$⁚out⁚0⁚arg1⁚e1 = aux 2 in d=e1",
            "$⁚out⁚0⁚smth1⁚e1 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e1->$⁚out⁚0⁚smth1⁚e1.input, \
             $⁚out⁚0⁚arg1⁚e1->$⁚out⁚0⁚smth1⁚e1.delay_time]",
            "$⁚out⁚0⁚arg0⁚e2 = aux d in d=e2",
            "$⁚out⁚0⁚arg1⁚e2 = aux 2 in d=e2",
            "$⁚out⁚0⁚smth1⁚e2 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e2->$⁚out⁚0⁚smth1⁚e2.input, \
             $⁚out⁚0⁚arg1⁚e2->$⁚out⁚0⁚smth1⁚e2.delay_time]",
            "$⁚out⁚0⁚arg0⁚e3 = aux d in d=e3",
            "$⁚out⁚0⁚arg1⁚e3 = aux 2 in d=e3",
            "$⁚out⁚0⁚smth1⁚e3 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e3->$⁚out⁚0⁚smth1⁚e3.input, \
             $⁚out⁚0⁚arg1⁚e3->$⁚out⁚0⁚smth1⁚e3.delay_time]",
        ],
    },
    ModuleRow {
        covers: "an apply-to-all body whose argument is a bare ARRAYED identifier: the \
                 plain equation reads its active element, so the identifier arm hoists it \
                 into an element-scoped helper for the scalar port",
        parent: Parent::ApplyToAll,
        equation: "SMTH1(vals, 2)",
        helpers: &[
            "$⁚out⁚0⁚arg0⁚e1 = aux vals in d=e1",
            "$⁚out⁚0⁚arg1⁚e1 = aux 2 in d=e1",
            "$⁚out⁚0⁚smth1⁚e1 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e1->$⁚out⁚0⁚smth1⁚e1.input, \
             $⁚out⁚0⁚arg1⁚e1->$⁚out⁚0⁚smth1⁚e1.delay_time]",
            "$⁚out⁚0⁚arg0⁚e2 = aux vals in d=e2",
            "$⁚out⁚0⁚arg1⁚e2 = aux 2 in d=e2",
            "$⁚out⁚0⁚smth1⁚e2 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e2->$⁚out⁚0⁚smth1⁚e2.input, \
             $⁚out⁚0⁚arg1⁚e2->$⁚out⁚0⁚smth1⁚e2.delay_time]",
            "$⁚out⁚0⁚arg0⁚e3 = aux vals in d=e3",
            "$⁚out⁚0⁚arg1⁚e3 = aux 2 in d=e3",
            "$⁚out⁚0⁚smth1⁚e3 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e3->$⁚out⁚0⁚smth1⁚e3.input, \
             $⁚out⁚0⁚arg1⁚e3->$⁚out⁚0⁚smth1⁚e3.delay_time]",
        ],
    },
    ModuleRow {
        covers: "an apply-to-all body whose argument is a bare SCALAR identifier: no element \
                 changes what it means, so it is wired to the port by name as in the scalar \
                 base shape",
        parent: Parent::ApplyToAll,
        equation: "SMTH1(k, 2)",
        helpers: &[
            "$⁚out⁚0⁚arg1⁚e1 = aux 2 in d=e1",
            "$⁚out⁚0⁚smth1⁚e1 = module stdlib⁚smth1 [k->$⁚out⁚0⁚smth1⁚e1.input, \
             $⁚out⁚0⁚arg1⁚e1->$⁚out⁚0⁚smth1⁚e1.delay_time]",
            "$⁚out⁚0⁚arg1⁚e2 = aux 2 in d=e2",
            "$⁚out⁚0⁚smth1⁚e2 = module stdlib⁚smth1 [k->$⁚out⁚0⁚smth1⁚e2.input, \
             $⁚out⁚0⁚arg1⁚e2->$⁚out⁚0⁚smth1⁚e2.delay_time]",
            "$⁚out⁚0⁚arg1⁚e3 = aux 2 in d=e3",
            "$⁚out⁚0⁚smth1⁚e3 = module stdlib⁚smth1 [k->$⁚out⁚0⁚smth1⁚e3.input, \
             $⁚out⁚0⁚arg1⁚e3->$⁚out⁚0⁚smth1⁚e3.delay_time]",
        ],
    },
    ModuleRow {
        covers: "a per-element arrayed equation: each slot is its own equation walked by its \
                 own visitor under its own element, so every slot restarts the counter at 0 \
                 and is kept apart by the element suffix alone",
        parent: Parent::PerElement,
        equation: "SMTH1(vals[d], 2)",
        helpers: &[
            "$⁚out⁚0⁚arg0⁚e1 = aux vals[d] in d=e1",
            "$⁚out⁚0⁚arg1⁚e1 = aux 2 in d=e1",
            "$⁚out⁚0⁚smth1⁚e1 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e1->$⁚out⁚0⁚smth1⁚e1.input, \
             $⁚out⁚0⁚arg1⁚e1->$⁚out⁚0⁚smth1⁚e1.delay_time]",
            "$⁚out⁚0⁚arg0⁚e2 = aux vals[d] in d=e2",
            "$⁚out⁚0⁚arg1⁚e2 = aux 2 in d=e2",
            "$⁚out⁚0⁚smth1⁚e2 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e2->$⁚out⁚0⁚smth1⁚e2.input, \
             $⁚out⁚0⁚arg1⁚e2->$⁚out⁚0⁚smth1⁚e2.delay_time]",
            "$⁚out⁚0⁚arg0⁚e3 = aux vals[d] in d=e3",
            "$⁚out⁚0⁚arg1⁚e3 = aux 2 in d=e3",
            "$⁚out⁚0⁚smth1⁚e3 = module stdlib⁚smth1 \
             [$⁚out⁚0⁚arg0⁚e3->$⁚out⁚0⁚smth1⁚e3.input, \
             $⁚out⁚0⁚arg1⁚e3->$⁚out⁚0⁚smth1⁚e3.delay_time]",
        ],
    },
];

/// One synthesized helper, rendered as what a consumer is entitled to read off
/// it: its runlist ident, which kind of helper it is, and its whole definition
/// -- a module's target model and input wiring, or an aux's body printed back
/// to equation text followed by the element the body is scoped to (`in
/// d=e1`), when it is one element of an apply-to-all body.
fn describe(v: &ImplicitVar) -> String {
    let scope = v
        .element_scope()
        .map(|s| {
            let pairs: Vec<String> = s
                .dims
                .iter()
                .zip(&s.element)
                .map(|(d, e)| format!("{}={}", d.as_str(), e.as_str()))
                .collect();
            format!(" in {}", pairs.join(","))
        })
        .unwrap_or_default();
    match v {
        ImplicitVar::Capture(c) => format!(
            "{} = capture[{}] {}{scope}",
            c.ident(),
            c.dims().join(","),
            print_eqn(c.arg())
        ),
        ImplicitVar::HoistedArg(a) => {
            format!("{} = aux {}{scope}", a.ident(), print_eqn(a.arg()))
        }
        ImplicitVar::Module(m) => {
            let refs: Vec<String> = m
                .references
                .iter()
                .map(|r| format!("{}->{}", r.src, r.dst))
                .collect();
            format!(
                "{} = module {} [{}]",
                m.ident,
                m.model_name,
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
    }
}

/// Build a row's model and hand back its synthesized helpers.
fn helpers_for(row: &ModuleRow) -> Vec<ImplicitVar> {
    let (tp, var) = model_for(row);
    let dm = tp.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &dm);
    implicit_vars_of(&db, &sync, "main", var)
}

/// `DELAYN`/`SMTHN` refuse every arity and order the canonical stdlib models
/// cannot express, before any argument is hoisted: two or five arguments, an
/// order other than the literal 1 or 3, and a non-literal order. Each refusal
/// spans the whole call and names the reason, and files no helper.
#[test]
fn delayn_and_smthn_refuse_bad_arities_and_unsupported_orders() {
    // `(equation, code, details)`. The orders the canonical models express
    // are 1 and 3, so 2 is the smallest refused one.
    let rows: &[(&str, ErrorCode, &str)] = &[
        (
            "SMTHN(k, 2)",
            ErrorCode::BadBuiltinArgs,
            "smthn takes 3 or 4 arguments, but 2 were given",
        ),
        (
            "DELAYN(k, 2, 3, 7, 9)",
            ErrorCode::BadBuiltinArgs,
            "delayn takes 3 or 4 arguments, but 5 were given",
        ),
        (
            "DELAYN(k, 2, 2)",
            ErrorCode::UnknownBuiltin,
            "delayn of order 2 is not supported; use order 1 or 3",
        ),
        (
            "SMTHN(k, 2, n)",
            ErrorCode::UnknownBuiltin,
            "smthn's order argument must be the literal 1 or 3",
        ),
    ];
    for (equation, code, details) in rows {
        let project = TestProject::new("alias_refusals")
            .scalar_aux("k", "3")
            .scalar_aux("n", "3")
            .aux("out", equation, None);
        let refusal = project
            .diagnostics_incremental()
            .into_iter()
            .find_map(|d| match d.error {
                DiagnosticError::Equation(e)
                    if e.code == *code && d.variable.as_deref() == Some("out") =>
                {
                    Some(e)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("`{equation}` is refused on `out` with {code:?}"));
        assert_eq!(refusal.details.as_deref(), Some(*details), "`{equation}`");
        assert_eq!(
            (refusal.start, refusal.end),
            (0, equation.len() as u16),
            "`{equation}`: the span covers the whole call"
        );

        let dm = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &dm);
        assert!(
            implicit_vars_of(&db, &sync, "main", "out").is_empty(),
            "`{equation}`: a refused call files no helper at all"
        );
    }
}

/// Vensim's `DELAY N`/`SMOOTH N` carry their initial value THIRD and their
/// order FOURTH; the MDL reader restructures them to the `DELAYN`/`SMTHN`
/// argument order (`mdl/xmile_compat.rs`), so an imported call wires its
/// initial value as the independent fourth port.
#[test]
fn mdl_delay_n_and_smooth_n_import_their_initial_value_as_the_fourth_port() {
    let mdl = "\
{UTF-8}
inp = 10 + Time ~~|
d = DELAY N(inp, 2, 7, 3) ~~|
s = SMOOTH N(inp, 2, 7, 1) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 4 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let dm = crate::open_vensim(mdl).expect("the MDL parses");
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &dm);
    let helpers = |var: &str| -> Vec<String> {
        implicit_vars_of(&db, &sync, "main", var)
            .iter()
            .map(describe)
            .collect()
    };
    assert_eq!(
        helpers("d"),
        [
            "$⁚d⁚0⁚arg1 = aux 2",
            "$⁚d⁚0⁚arg2 = aux 7",
            "$⁚d⁚0⁚delay3 = module stdlib⁚delay3 [inp->$⁚d⁚0⁚delay3.input, \
             $⁚d⁚0⁚arg1->$⁚d⁚0⁚delay3.delay_time, $⁚d⁚0⁚arg2->$⁚d⁚0⁚delay3.initial_value]",
        ]
    );
    assert_eq!(
        helpers("s"),
        [
            "$⁚s⁚0⁚arg1 = aux 2",
            "$⁚s⁚0⁚arg2 = aux 7",
            "$⁚s⁚0⁚smth1 = module stdlib⁚smth1 [inp->$⁚s⁚0⁚smth1.input, \
             $⁚s⁚0⁚arg1->$⁚s⁚0⁚smth1.delay_time, $⁚s⁚0⁚arg2->$⁚s⁚0⁚smth1.initial_value]",
        ]
    );
}

/// A call with more arguments than its target model has ports is refused
/// before any argument is hoisted, so no orphan helper is filed and no helper
/// computes a value nothing reads. Under-arity (`SMTH1(k)`, the trailing
/// ports unwired) is accepted, as the row table's optional-initial-value row
/// says.
#[test]
fn a_call_with_more_arguments_than_ports_is_refused_before_any_hoist() {
    let project =
        TestProject::new("over_arity")
            .scalar_aux("k", "3")
            .aux("out", "SMTH1(k, 2, 5, 7)", None);
    let refusal = project
        .diagnostics_incremental()
        .into_iter()
        .find_map(|d| match d.error {
            DiagnosticError::Equation(e)
                if e.code == ErrorCode::BadBuiltinArgs && d.variable.as_deref() == Some("out") =>
            {
                Some(e)
            }
            _ => None,
        })
        .expect("SMTH1 with four arguments is refused on `out`");
    assert_eq!(
        refusal.details.as_deref(),
        Some("smth1 takes at most 3 argument(s), but 4 were given")
    );
    assert_eq!(
        (refusal.start, refusal.end),
        (0, "SMTH1(k, 2, 5, 7)".len() as u16),
        "the span covers the whole call"
    );

    let dm = project.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &dm);
    assert!(
        implicit_vars_of(&db, &sync, "main", "out").is_empty(),
        "a refused call files no helper at all"
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
            let ports = stdlib.get(&m.model_name).unwrap_or_else(|| {
                panic!(
                    "{}: `{}` -- synthesized module targets unknown model {}",
                    row.covers, row.equation, m.model_name
                )
            });
            for mr in &m.references {
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
                    m.model_name
                );
            }
        }
    }
}

/// A hoisted argument holds the argument SUBTREE the parent's parse produced
/// -- source positions included -- not a re-parse of it.
///
/// Positions are what make the property observable: printing an expression
/// and parsing it back resets every span to an offset into the printed text,
/// so a helper whose spans still point into its PARENT's equation cannot have
/// made that round trip (GH #913). The rows are the scalar ones whose first
/// hoisted argument is written verbatim in the call, so the walk leaves it
/// untouched; the per-element rows substitute their argument and are pinned
/// as rewritten by the row table.
#[test]
fn a_hoisted_argument_holds_the_argument_subtree_itself() {
    use crate::builtins::UntypedBuiltinFn;

    /// Does the walk rewrite this argument -- a nested call anywhere in it?
    fn has_call(e: &Expr0) -> bool {
        match e {
            Expr0::App(..) => true,
            Expr0::Op1(_, r, _) => has_call(r),
            Expr0::Op2(_, l, r, _) => has_call(l) || has_call(r),
            Expr0::If(c, t, f, _) => has_call(c) || has_call(t) || has_call(f),
            Expr0::Const(..) | Expr0::Var(..) | Expr0::Subscript(..) => false,
        }
    }

    let mut checked = 0usize;
    for row in ROWS.iter().filter(|r| r.parent == Parent::Scalar) {
        let what = row.covers;
        let eqn = row.equation;
        let parsed = Expr0::new(row.equation, LexerType::Equation)
            .unwrap_or_else(|e| panic!("{what}: `{eqn}` must lex: {e:?}"))
            .unwrap_or_else(|| panic!("{what}: `{eqn}` must parse"));
        // The outermost call's first argument that is hoisted verbatim: not a
        // bare identifier (wired, not hoisted) and holding no nested call (the
        // walk rewrites those before the hoist).
        let Expr0::App(UntypedBuiltinFn(_, args), _) = &parsed else {
            panic!("{what}: `{eqn}` must be a call")
        };
        let Some((index, source_arg)) = args
            .iter()
            .enumerate()
            .find(|(_, a)| !matches!(a, Expr0::Var(..)) && !has_call(a))
        else {
            continue;
        };
        assert!(
            source_arg.get_loc().start > 0,
            "{what}: `{eqn}` -- the argument must start past offset 0, or a \
             printed-and-reparsed body would satisfy this test too"
        );
        // The LAST helper filed for that position: a nested call is expanded
        // before the outer one, so the outermost call's helpers come last.
        let helpers = helpers_for(row);
        let hoisted = helpers
            .iter()
            .filter_map(|h| match h {
                ImplicitVar::HoistedArg(a) if a.ident().ends_with(&format!("arg{index}")) => {
                    Some(a)
                }
                _ => None,
            })
            .next_back()
            .unwrap_or_else(|| panic!("{what}: `{eqn}` must hoist argument {index}"));
        assert_eq!(
            hoisted.arg(),
            source_arg,
            "{what}: `{eqn}` -- the helper must BE the argument subtree, spans included; \
             a differing span means it was printed and parsed back"
        );
        checked += 1;
    }
    assert!(
        checked >= 5,
        "the scalar rows must actually be exercised, checked {checked}"
    );
}

/// A hoisted argument's compiled fragment is its argument, compiled.
///
/// The comparison is against a sibling aux of the same model holding the same
/// expression, compiled through the ordinary explicit path, with each
/// variable's own name normalized away. Both sides go through
/// `lower_fragment` and the same codegen, so what is left to differ is whether
/// the helper handed lowering the argument -- which is the claim.
#[test]
fn a_hoisted_arguments_fragment_is_the_argument_compiled() {
    use crate::db::fragment_compile::compile_implicit_var_fragment;

    let row = ROWS
        .iter()
        .find(|r| r.equation == "SMTH1(k * 2, 3)")
        .expect("the computed-argument row");
    let (tp, _) = model_for(row);
    let tp = tp.aux("sibling", "k * 2", None);
    let dm = tp.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &dm);
    let model = sync.models["main"].source;

    let helper_ident = "$⁚out⁚0⁚arg0";
    let helper_bc = compile_implicit_var_fragment(
        &db,
        model,
        sync.project,
        helper_ident.to_string(),
        ModuleInputSet::empty(&db),
    )
    .as_ref()
    .expect("the hoisted argument must compile")
    .fragment
    .flow_bytecodes
    .as_ref()
    .expect("the hoisted argument must have a flow fragment");

    let sibling_sv = model.variables(&db)["sibling"];
    let sibling_bc = compile_var_fragment(
        &db,
        sibling_sv,
        model,
        sync.project,
        ModuleInputSet::empty(&db),
    )
    .as_ref()
    .expect("the sibling must compile")
    .fragment
    .flow_bytecodes
    .as_ref()
    .expect("the sibling must have a flow fragment");

    assert_eq!(
        format!("{helper_bc:?}").replace(helper_ident, "SELF"),
        format!("{sibling_bc:?}").replace("sibling", "SELF"),
        "the hoisted argument's fragment must be `k * 2` compiled"
    );
}
