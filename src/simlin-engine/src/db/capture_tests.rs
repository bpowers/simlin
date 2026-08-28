// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What a `PREVIOUS`/`INIT` capture carries, and what every stage downstream
//! of the parse is entitled to read off it.
//!
//! The sibling
//! `db::prev_init_tests::every_prev_init_argument_shape_agrees_between_the_parse_and_codegen`
//! decides WHICH arguments capture; these tests pin what the capture then IS.
//! Together they cover the `PREVIOUS`/`INIT` routing arms of
//! `builtins_visitor::walk` and both branches of its `hoist_capture`.

use super::*;
use crate::ast::{Expr0, print_eqn};
use crate::capture::{Capture, CaptureKind, ImplicitVar};
use crate::lexer::LexerType;
use crate::test_common::TestProject;

/// How the row's equation is attached to the model, which is what decides
/// whether the parse walks it once or once per element.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Parent {
    /// A scalar aux named `lagged`.
    Scalar,
    /// An apply-to-all aux `out[d]`: one equation, walked once per element
    /// because the body contains `PREVIOUS`/`INIT` (`contains_module_call`).
    ApplyToAll,
    /// A per-element arrayed aux `out[d]` whose slots carry the SAME text but
    /// are distinct equations, so a capture of one slot must not be able to
    /// claim another slot's name (PR #668).
    PerElement,
}

/// One `PREVIOUS`/`INIT` shape that synthesizes a capture, plus everything a
/// consumer reads off the result.
struct CaptureRow {
    /// The arm of `builtins_visitor` this row exercises.
    covers: &'static str,
    parent: Parent,
    equation: &'static str,
    /// Every capture the parse must synthesize, as
    /// `(runlist ident, kind, argument printed back to equation text,
    /// declared dimension names)`.
    ///
    /// The idents are written out rather than derived from a formatter,
    /// deliberately: they are the strings the runlist sorts by, the layout's
    /// implicit section files, and the results offset map keys, so a test that
    /// re-derived them with the production formatter would agree with any
    /// spelling that formatter chose.
    captures: &'static [(
        &'static str,
        CaptureKind,
        &'static str,
        &'static [&'static str],
    )],
    /// `Some(reason)` when the walk rewrites the argument before capturing it,
    /// so the capture is NOT the source subtree and
    /// [`a_capture_holds_the_argument_subtree_itself`] skips the row.
    rewritten: Option<&'static str>,
}

/// Every capture-synthesizing shape, one row per arm.
///
/// The arms that do NOT capture -- a bare non-module variable, an all-static
/// subscript, an index that leaves a dimension standing -- are the
/// `captures: false` rows of
/// `prev_init_tests::every_prev_init_argument_shape_agrees_between_the_parse_and_codegen`,
/// which derives its rows from the same routing. A capture inside a macro body
/// is covered separately by [`a_capture_inside_a_macro_body_names_the_body_variable`],
/// because a macro body needs a project a `TestProject` cannot express.
const ROWS: &[CaptureRow] = &[
    CaptureRow {
        covers: "hoist_capture scalar branch: an argument that references no storage at all",
        parent: Parent::Scalar,
        equation: "PREVIOUS(k * 2, 0)",
        captures: &[("$⁚lagged⁚0⁚arg0", CaptureKind::Previous, "k * 2", &[])],
        rewritten: None,
    },
    CaptureRow {
        covers: "the INIT twin of the catch-all row: the kind is the call, not the shape",
        parent: Parent::Scalar,
        equation: "INIT(k * 2)",
        captures: &[("$⁚lagged⁚0⁚arg0", CaptureKind::Init, "k * 2", &[])],
        rewritten: None,
    },
    CaptureRow {
        covers: "a non-default fallback is NOT captured -- only arg0 is",
        parent: Parent::Scalar,
        equation: "PREVIOUS(k * 2, k + 7)",
        captures: &[("$⁚lagged⁚0⁚arg0", CaptureKind::Previous, "k * 2", &[])],
        rewritten: None,
    },
    CaptureRow {
        covers: "a dynamic subscript index: the capture is what gives it lagged semantics",
        parent: Parent::Scalar,
        equation: "PREVIOUS(vals[idx], 0)",
        captures: &[("$⁚lagged⁚0⁚arg0", CaptureKind::Previous, "vals[idx]", &[])],
        rewritten: None,
    },
    CaptureRow {
        covers: "two captures in one equation: the walk counter is shared and increments once \
                 per capture, so the second is `1`",
        parent: Parent::Scalar,
        equation: "PREVIOUS(k * 2, 0) + INIT(k * 3)",
        captures: &[
            ("$⁚lagged⁚0⁚arg0", CaptureKind::Previous, "k * 2", &[]),
            ("$⁚lagged⁚1⁚arg0", CaptureKind::Init, "k * 3", &[]),
        ],
        rewritten: None,
    },
    CaptureRow {
        covers: "hoist_capture scalar branch inside apply-to-all: one capture per element, \
                 each carrying its element suffix. Substitution is a no-op on this body -- \
                 the index is a VARIABLE, not a dimension -- so the three bodies are equal \
                 and only the suffixes tell the captures apart",
        parent: Parent::ApplyToAll,
        equation: "PREVIOUS(vals[idx], 0)",
        captures: &[
            ("$⁚out⁚0⁚arg0⁚e1", CaptureKind::Previous, "vals[idx]", &[]),
            ("$⁚out⁚0⁚arg0⁚e2", CaptureKind::Previous, "vals[idx]", &[]),
            ("$⁚out⁚0⁚arg0⁚e3", CaptureKind::Previous, "vals[idx]", &[]),
        ],
        rewritten: Some(
            "the scalar branch substitutes, even where the substitution changes nothing",
        ),
    },
    CaptureRow {
        covers: "hoist_capture scalar branch inside apply-to-all, with the substitution \
                 actually firing: a dimension reference in the body is rewritten to the \
                 active element, so each element's capture holds a DIFFERENT body. This is \
                 the one arm where the capture is deliberately not the source subtree",
        parent: Parent::ApplyToAll,
        // `Op2`, so the routing's pre-substitution (which only fires on a bare
        // `Subscript` arg0) does not run and `hoist_capture` owns the whole
        // substitution; `k` is the bare variable reference and `vals[d]` the
        // subscript, which together select the SCALAR branch over the arrayed
        // one (`arg_has_bare_var_ref && !arg_has_subscript`).
        equation: "PREVIOUS(vals[d] * k, 0)",
        captures: &[
            (
                "$⁚out⁚0⁚arg0⁚e1",
                CaptureKind::Previous,
                "vals[d·e1] * k",
                &[],
            ),
            (
                "$⁚out⁚0⁚arg0⁚e2",
                CaptureKind::Previous,
                "vals[d·e2] * k",
                &[],
            ),
            (
                "$⁚out⁚0⁚arg0⁚e3",
                CaptureKind::Previous,
                "vals[d·e3] * k",
                &[],
            ),
        ],
        rewritten: Some("substitute_dimension_refs rewrites the body per element"),
    },
    CaptureRow {
        covers: "hoist_capture ARRAYED branch (GH #541): a bare arrayed name inside the \
                 argument keeps its array shape, so ONE apply-to-all capture is synthesized \
                 for every element and the suffix is omitted so they dedup to one",
        parent: Parent::ApplyToAll,
        equation: "PREVIOUS(PREVIOUS(vals), 0)",
        captures: &[(
            "$⁚out⁚0⁚arg0",
            CaptureKind::Previous,
            "previous(vals, 0)",
            &["d"],
        )],
        rewritten: Some("the walk gives the inner PREVIOUS its default fallback"),
    },
    CaptureRow {
        covers: "hoist_capture ARRAYED branch under a PER-ELEMENT parent: each slot gets a \
                 fresh visitor, so the walk counter restarts at 0 for every one of them and \
                 the element suffix is the whole of what keeps their names apart (PR #668)",
        parent: Parent::PerElement,
        equation: "PREVIOUS(PREVIOUS(vals), 0)",
        captures: &[
            (
                "$⁚out⁚0⁚arg0⁚e1",
                CaptureKind::Previous,
                "previous(vals, 0)",
                &["d"],
            ),
            (
                "$⁚out⁚0⁚arg0⁚e2",
                CaptureKind::Previous,
                "previous(vals, 0)",
                &["d"],
            ),
            (
                "$⁚out⁚0⁚arg0⁚e3",
                CaptureKind::Previous,
                "previous(vals, 0)",
                &["d"],
            ),
        ],
        rewritten: Some("the walk gives the inner PREVIOUS its default fallback"),
    },
];

/// The model a row describes, plus the name of the variable holding its
/// equation.
fn model_for(row: &CaptureRow) -> (TestProject, &'static str) {
    let base = TestProject::new("captures")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("d", &["e1", "e2", "e3"])
        .array_with_ranges("vals[d]", vec![("e1", "30"), ("e2", "10"), ("e3", "20")])
        .scalar_aux("k", "3")
        .aux("idx", "1 + MIN(TIME, 1)", None)
        .aux("smoothed", "SMTH1(k, 2)", None);
    match row.parent {
        Parent::Scalar => (base.aux("lagged", row.equation, None), "lagged"),
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

/// The captures one variable's production parse synthesized, in walk order.
///
/// Read through the production per-variable parse, so these are the captures
/// the compiler sees rather than a re-derivation.
fn captures_of(db: &SimlinDb, sync: &SyncResult, model_name: &str, var: &str) -> Vec<Capture> {
    let model = sync.models[model_name].source;
    let source_var = model.variables(db)[var];
    parse_source_variable(db, source_var, sync.project)
        .implicit_vars
        .iter()
        .filter_map(ImplicitVar::capture)
        .cloned()
        .collect()
}

/// Build a row's model and hand back its captures.
fn captures_for(row: &CaptureRow) -> Vec<Capture> {
    let (tp, var) = model_for(row);
    let dm = tp.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &dm);
    captures_of(&db, &sync, "main", var)
}

/// Every capture-synthesizing arm, by what the capture is filed under, which
/// call it came from, what its body is, and what shape it declares.
///
/// The ident is the load-bearing assertion: every runlist is a lexicographic
/// sort, the layout's implicit section and the results offset map are
/// name-sorted, so a capture filed under a different string moves the compiled
/// artifact even when it computes the same value.
#[test]
fn every_capture_shape_carries_its_ident_kind_and_argument() {
    for row in ROWS {
        let what = row.covers;
        let eqn = row.equation;
        let captures = captures_for(row);
        let observed: Vec<(String, CaptureKind, String, Vec<String>)> = captures
            .iter()
            .map(|c| {
                (
                    c.ident().to_string(),
                    c.kind(),
                    print_eqn(c.arg()),
                    c.dims().to_vec(),
                )
            })
            .collect();
        let expected: Vec<(String, CaptureKind, String, Vec<String>)> = row
            .captures
            .iter()
            .map(|(ident, kind, arg, dims)| {
                (
                    (*ident).to_string(),
                    *kind,
                    (*arg).to_string(),
                    dims.iter().map(|d| (*d).to_string()).collect(),
                )
            })
            .collect();
        assert_eq!(
            observed.len(),
            expected.len(),
            "{what}: `{eqn}` -- capture count; got {observed:?}"
        );
        for (got, want) in observed.iter().zip(expected.iter()) {
            assert_eq!(got.0, want.0, "{what}: `{eqn}` -- capture ident");
            assert_eq!(
                got.1.as_str(),
                want.1.as_str(),
                "{what}: `{eqn}` -- capture kind for {}",
                want.0
            );
            assert_eq!(
                got.2, want.2,
                "{what}: `{eqn}` -- captured argument for {}",
                want.0
            );
            assert_eq!(
                got.3, want.3,
                "{what}: `{eqn}` -- declared dimensions for {}",
                want.0
            );
        }
    }
}

/// A capture holds the argument SUBTREE the parent's parse produced -- source
/// positions included -- not a re-parse of it.
///
/// This is the difference the whole design turns on, and positions are what
/// make it observable: printing an expression and parsing it back resets every
/// span to an offset into the printed text, so a capture whose spans still
/// point into its PARENT's equation cannot have made that round trip. It is
/// also what stops the printer and the lexer from having to agree on every
/// spelling for a model to compile (GH #913).
///
/// Rows the walk rewrites before capturing (per-element substitution, a
/// defaulted fallback on a nested call) are skipped with their reason: their
/// argument is legitimately not the source subtree, and
/// [`every_capture_shape_carries_its_ident_kind_and_argument`] pins what it is
/// instead.
#[test]
fn a_capture_holds_the_argument_subtree_itself() {
    /// The first `PREVIOUS`/`INIT` argument reachable from `expr`, in walk
    /// order.
    fn first_snapshot_arg(expr: &Expr0) -> Option<&Expr0> {
        use crate::builtins::UntypedBuiltinFn;
        match expr {
            Expr0::App(UntypedBuiltinFn(func, args), _) => {
                if (func == "previous" || func == "init") && !args.is_empty() {
                    return Some(&args[0]);
                }
                args.iter().find_map(first_snapshot_arg)
            }
            Expr0::Op1(_, r, _) => first_snapshot_arg(r),
            Expr0::Op2(_, l, r, _) => first_snapshot_arg(l).or_else(|| first_snapshot_arg(r)),
            Expr0::If(c, t, f, _) => first_snapshot_arg(c)
                .or_else(|| first_snapshot_arg(t))
                .or_else(|| first_snapshot_arg(f)),
            _ => None,
        }
    }

    let mut checked = 0usize;
    let mut saw_offset_span = false;
    for row in ROWS {
        if row.rewritten.is_some() {
            continue;
        }
        let what = row.covers;
        let eqn = row.equation;
        // The same string the model stores, so the spans this parse produces
        // are the spans the model's own parse produced.
        let parsed = Expr0::new(row.equation, LexerType::Equation)
            .unwrap_or_else(|e| panic!("{what}: `{eqn}` must lex: {e:?}"))
            .unwrap_or_else(|| panic!("{what}: `{eqn}` must parse"));
        let source_arg = first_snapshot_arg(&parsed)
            .unwrap_or_else(|| panic!("{what}: `{eqn}` has no argument"));
        // An argument that does not start at offset 0 is what makes the
        // comparison below able to fail: a printed-and-reparsed body's spans
        // are offsets into the printed text, so they start at 0. Without at
        // least one such row the test would pass on a round trip too.
        saw_offset_span |= source_arg.get_loc().start > 0;

        let captures = captures_for(row);
        let first = captures
            .first()
            .unwrap_or_else(|| panic!("{what}: `{eqn}` must synthesize a capture"));
        assert_eq!(
            first.arg(),
            source_arg,
            "{what}: `{eqn}` -- the capture must BE the argument subtree, spans included; \
             a differing span means it was printed and parsed back"
        );
        checked += 1;
    }
    assert_eq!(
        checked,
        ROWS.iter().filter(|r| r.rewritten.is_none()).count(),
        "every row that does not declare a rewrite must be exercised here"
    );
    assert!(
        saw_offset_span,
        "at least one row's argument must start past offset 0, or a \
         printed-and-reparsed capture would satisfy this test too"
    );
}

/// A capture builds the same parse-stage variable that parsing its printed
/// equation built.
///
/// This is the equivalence the whole chunk rests on: every consumer that used
/// to re-parse a helper's equation text now calls `Capture::variable_stage0`,
/// so if the two disagree the dependency graph, the layout and the bytecode
/// all move. The `datamodel::Variable` on the right is built by the recipe the
/// parse used to build it with -- the capture's own ident, its argument
/// printed, and `Scalar` or `ApplyToAll` over its own declared dimensions --
/// and every field of it comes off the capture, so nothing about the
/// comparison is invented.
///
/// Both sides are reduced by [`normalize_expr`] first -- spans cleared,
/// identifiers canonicalized -- and that function states what each reduction
/// hides and why neither is observable.
#[test]
fn a_capture_builds_the_variable_parsing_its_printed_equation_built() {
    for row in ROWS {
        let what = row.covers;
        let eqn = row.equation;
        let (tp, var) = model_for(row);
        let dm = tp.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &dm);
        let dim_ctx = project_dimensions_context(&db, sync.project);
        let units_ctx = project_units_context(&db, sync.project);

        for capture in captures_of(&db, &sync, "main", var) {
            let equation = if capture.dims().is_empty() {
                datamodel::Equation::Scalar(print_eqn(capture.arg()))
            } else {
                datamodel::Equation::ApplyToAll(capture.dims().to_vec(), print_eqn(capture.arg()))
            };
            let dm_var = datamodel::Variable::Aux(datamodel::Aux {
                ident: capture.ident().to_string(),
                equation,
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            });
            let mut nested = Vec::new();
            let ctx = crate::variable::ParseContext::new(dim_ctx, units_ctx);
            let reparsed =
                crate::variable::parse_var(&ctx, &dm_var, &mut nested, |mi| Ok(Some(mi.clone())));
            assert!(
                nested.is_empty(),
                "{what}: `{eqn}` -- a capture body must synthesize no further helpers"
            );

            let built = capture.variable_stage0(dim_ctx);
            let id = capture.ident();
            assert_eq!(
                built.ident, reparsed.ident,
                "{what}: `{eqn}` -- ident of {id}"
            );
            assert_eq!(built.eqn, reparsed.eqn, "{what}: `{eqn}` -- eqn of {id}");
            assert_eq!(
                built.errors, reparsed.errors,
                "{what}: `{eqn}` -- equation errors of {id}"
            );
            assert_eq!(
                built.units, reparsed.units,
                "{what}: `{eqn}` -- units of {id}"
            );
            assert!(
                built.unit_errors == reparsed.unit_errors,
                "{what}: `{eqn}` -- unit errors of {id} ({} vs {})",
                built.unit_errors.len(),
                reparsed.unit_errors.len()
            );
            // The bodies first, printed, because that is the readable failure;
            // then the WHOLE `VarKind`, which additionally covers `tables`,
            // `non_negative`, `is_flow`, `is_table_only` and any structural
            // difference the printer would hide -- every field a consumer of
            // the parse-stage variable can read.
            assert_eq!(
                built.ast().map(print_ast),
                reparsed.ast().map(print_ast),
                "{what}: `{eqn}` -- body of {id}"
            );
            assert_eq!(
                built.init_ast().map(print_ast),
                reparsed.init_ast().map(print_ast),
                "{what}: `{eqn}` -- initial-phase body of {id}"
            );
            assert_eq!(
                normalize_kind(built.kind.clone()),
                normalize_kind(reparsed.kind.clone()),
                "{what}: `{eqn}` -- VarKind of {id}"
            );
        }
    }
}

/// One `Expr0` reduced to what the next stage will see: every span cleared and
/// every identifier canonicalized.
///
/// Both reductions are what makes the comparison in
/// [`a_capture_builds_the_variable_parsing_its_printed_equation_built`] an
/// equality rather than a near-equality, and neither hides anything a consumer
/// can observe.
///
/// Spans: the right-hand side made the round trip this change deletes, so its
/// spans index the printed helper text.
/// [`a_capture_holds_the_argument_subtree_itself`] is where spans are the
/// property under test.
///
/// Identifiers: a capture keeps the SOURCE spelling of an identifier where a
/// re-parse kept the lexer's. The live case is a qualified element index --
/// `PREVIOUS(vals[d.e2], 0)` captures `RawIdent("d.e2")`, where re-parsing
/// `print_eqn`'s output produced `RawIdent("d·e2")`. `Expr0` -> `Expr1` lowering
/// canonicalizes every identifier, and `common::canonicalize` maps an unquoted
/// `.` to `·`, so the two are one identifier from that point on. That is an
/// argument, not a measurement, and the measurement is
/// [`a_captures_fragment_is_its_argument_compiled`]: it compiles that row's
/// capture and an ordinary aux holding the same expression and requires
/// identical bytecode.
fn normalize_expr(expr: Expr0) -> Expr0 {
    use crate::ast::IndexExpr0;
    use crate::common::{RawIdent, canonicalize};
    fn ident(id: RawIdent) -> RawIdent {
        RawIdent::new_from_str(&canonicalize(id.as_str()))
    }
    fn index(idx: IndexExpr0) -> IndexExpr0 {
        match idx {
            IndexExpr0::Wildcard(l) => IndexExpr0::Wildcard(l),
            IndexExpr0::StarRange(d, l) => IndexExpr0::StarRange(ident(d), l),
            IndexExpr0::Range(a, b, l) => {
                IndexExpr0::Range(normalize_expr(a), normalize_expr(b), l)
            }
            IndexExpr0::DimPosition(n, l) => IndexExpr0::DimPosition(n, l),
            IndexExpr0::Expr(e) => IndexExpr0::Expr(normalize_expr(e)),
        }
    }
    let expr = expr.strip_loc();
    match expr {
        Expr0::Const(_, _, _) => expr,
        Expr0::Var(id, l) => Expr0::Var(ident(id), l),
        Expr0::App(crate::builtins::UntypedBuiltinFn(f, args), l) => Expr0::App(
            crate::builtins::UntypedBuiltinFn(f, args.into_iter().map(normalize_expr).collect()),
            l,
        ),
        Expr0::Subscript(id, idx, l) => {
            Expr0::Subscript(ident(id), idx.into_iter().map(index).collect(), l)
        }
        Expr0::Op1(op, r, l) => Expr0::Op1(op, Box::new(normalize_expr(*r)), l),
        Expr0::Op2(op, a, b, l) => Expr0::Op2(
            op,
            Box::new(normalize_expr(*a)),
            Box::new(normalize_expr(*b)),
            l,
        ),
        Expr0::If(c, t, f, l) => Expr0::If(
            Box::new(normalize_expr(*c)),
            Box::new(normalize_expr(*t)),
            Box::new(normalize_expr(*f)),
            l,
        ),
    }
}

/// [`normalize_expr`] over one `Ast<Expr0>`.
fn normalize_ast(ast: crate::ast::Ast<Expr0>) -> crate::ast::Ast<Expr0> {
    use crate::ast::Ast;
    match ast {
        Ast::Scalar(e) => Ast::Scalar(normalize_expr(e)),
        Ast::ApplyToAll(dims, e) => Ast::ApplyToAll(dims, normalize_expr(e)),
        Ast::Arrayed(dims, elements, default, apply_default) => Ast::Arrayed(
            dims,
            elements
                .into_iter()
                .map(|(k, e)| (k, normalize_expr(e)))
                .collect(),
            default.map(normalize_expr),
            apply_default,
        ),
    }
}

/// [`normalize_expr`] over one parse-stage `VarKind`. The non-`Aux` arms carry
/// no `Expr0` of their own at this stage, so they pass through.
fn normalize_kind(
    kind: crate::variable::VarKind<datamodel::ModuleReference, Expr0>,
) -> crate::variable::VarKind<datamodel::ModuleReference, Expr0> {
    use crate::variable::VarKind;
    match kind {
        VarKind::Aux {
            ast,
            init_ast,
            tables,
            non_negative,
            is_flow,
            is_table_only,
        } => VarKind::Aux {
            ast: ast.map(normalize_ast),
            init_ast: init_ast.map(normalize_ast),
            tables,
            non_negative,
            is_flow,
            is_table_only,
        },
        other => other,
    }
}

/// One `Ast<Expr0>` printed back to text, for the readable half of the
/// comparisons above.
fn print_ast(ast: &crate::ast::Ast<Expr0>) -> String {
    use crate::ast::Ast;
    match ast {
        Ast::Scalar(e) => print_eqn(e),
        Ast::ApplyToAll(dims, e) => format!(
            "[{}] {}",
            dims.iter()
                .map(|d| d.name().to_string())
                .collect::<Vec<_>>()
                .join(","),
            print_eqn(e)
        ),
        Ast::Arrayed(dims, elements, _, _) => {
            let mut parts: Vec<String> = elements
                .iter()
                .map(|(k, e)| format!("{}={}", k.as_str(), print_eqn(e)))
                .collect();
            parts.sort();
            format!(
                "[{}] {}",
                dims.iter()
                    .map(|d| d.name().to_string())
                    .collect::<Vec<_>>()
                    .join(","),
                parts.join(";")
            )
        }
    }
}

/// A capture's compiled fragment is its argument, compiled.
///
/// The comparison is against a sibling aux of the same model holding the same
/// expression, compiled through the ordinary explicit path, with each
/// variable's own name normalized away. Both sides go through
/// `lower_fragment` and the same codegen, so what is left to differ is whether
/// the capture handed lowering the argument -- which is the claim.
///
/// Scalar rows only: an apply-to-all or per-element row's capture is one slot
/// of an unrolled parent and has no single-aux sibling to compare against.
/// Those shapes are pinned instead by the checked-in fragment goldens
/// (`db/fragment_char_golden/prev_init.txt` and its neighbours), which render
/// every capture's whole bytecode.
#[test]
fn a_captures_fragment_is_its_argument_compiled() {
    use crate::db::fragment_compile::compile_implicit_var_fragment;

    let mut checked = 0usize;
    for row in ROWS {
        if row.parent != Parent::Scalar {
            continue;
        }
        let what = row.covers;
        let eqn = row.equation;
        let (tp, _) = model_for(row);
        // The sibling holds the captured argument as an ordinary equation.
        let arg_text = row.captures[0].2;
        let tp = tp.aux("sibling", arg_text, None);
        let dm = tp.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &dm);
        let model = sync.models["main"].source;

        let capture_ident = row.captures[0].0;
        let capture_bc = compile_implicit_var_fragment(
            &db,
            model,
            sync.project,
            capture_ident.to_string(),
            ModuleInputSet::empty(&db),
        )
        .as_ref()
        .unwrap_or_else(|| panic!("{what}: `{eqn}` -- the capture must compile"))
        .fragment
        .flow_bytecodes
        .as_ref()
        .unwrap_or_else(|| panic!("{what}: `{eqn}` -- the capture must have a flow fragment"));

        let sibling_sv = model.variables(&db)["sibling"];
        let sibling_bc = compile_var_fragment(
            &db,
            sibling_sv,
            model,
            sync.project,
            ModuleInputSet::empty(&db),
        )
        .as_ref()
        .unwrap_or_else(|| panic!("{what}: `{eqn}` -- the sibling must compile"))
        .fragment
        .flow_bytecodes
        .as_ref()
        .unwrap_or_else(|| panic!("{what}: `{eqn}` -- the sibling must have a flow fragment"));

        assert_eq!(
            format!("{capture_bc:?}").replace(capture_ident, "SELF"),
            format!("{sibling_bc:?}").replace("sibling", "SELF"),
            "{what}: `{eqn}` -- the capture's fragment must be `{arg_text}` compiled"
        );
        checked += 1;
    }
    assert!(
        checked >= 5,
        "the scalar rows must actually be exercised, checked {checked}"
    );
}

/// A capture synthesized inside a macro body is filed under the BODY
/// variable's name, not the invoking variable's.
///
/// The macro-body arm reaches `hoist_capture` through the same routing as any
/// other equation -- what a macro body changes is which calls resolve to the
/// builtin (`enclosing_model`, GH #554), not how a capture is minted -- so
/// this row exists to say that the parent a capture names is the variable
/// being parsed, which for a macro body is the body variable of the macro's
/// own model.
#[test]
fn a_capture_inside_a_macro_body_names_the_body_variable() {
    let source = format!(
        "{{UTF-8}}\n{}",
        r#":MACRO: LAGSUM(x)
LAGSUM = PREVIOUS(x * 2, 0)
	~	a
	~		|

:END OF MACRO:
driver=
	3
	~
	~		|

wrapped=
	LAGSUM(driver)
	~
	~		|

********************************************************
	.Control
********************************************************~
		Simulation Control Parameters
	|

INITIAL TIME  = 0
	~	Month
	~		|

FINAL TIME  = 2
	~	Month
	~		|

TIME STEP  = 1
	~	Month
	~		|

SAVEPER  = TIME STEP
	~	Month
	~		|
"#
    );
    let project = crate::compat::open_vensim(&source).expect("the macro source must import");
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let macro_model = sync
        .models
        .iter()
        .find(|(name, _)| name.as_str() == "lagsum")
        .map(|(_, m)| m.source)
        .expect("the macro model must sync");

    let body_var = macro_model.variables(&db)["lagsum"];
    let captures: Vec<Capture> = parse_source_variable(&db, body_var, sync.project)
        .implicit_vars
        .iter()
        .filter_map(ImplicitVar::capture)
        .cloned()
        .collect();

    assert_eq!(
        captures
            .iter()
            .map(|c| (c.ident().to_string(), print_eqn(c.arg())))
            .collect::<Vec<_>>(),
        vec![("$⁚lagsum⁚0⁚arg0".to_string(), "x * 2".to_string())],
        "a capture in a macro body is named for the body variable it was hoisted out of"
    );
    assert_eq!(captures[0].kind().as_str(), "PREVIOUS");
}
