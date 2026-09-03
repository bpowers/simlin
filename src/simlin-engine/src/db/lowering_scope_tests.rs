// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Pins for the `Expr2` lowering scope (`ast::LoweringScope`): what a
//! variable's `Expr0 -> Expr2` lowering knows about the names it references,
//! read through the production per-variable lowering, and that a helper the
//! parse synthesizes lowers under the same knowledge as the equation it was
//! written in -- so it reads, and is refused, as the plain spelling of its
//! body is.

use super::*;
use crate::ast::{ArrayBounds, Ast, Expr2};
use crate::builtins::BuiltinFn;
use crate::common::ErrorCode;
use crate::datamodel;
use crate::db::var_fragment::{ExplicitFragment, explicit_fragment_input};
use crate::test_common::TestProject;

/// The `Expr2` body of `main`'s `var` as the production per-variable lowering
/// produces it: `explicit_fragment_input`'s target, lowered under the
/// variable's dependency shapes. `var` has a scalar or apply-to-all equation.
fn lowered_body(db: &SimlinDb, sync: &SyncResult, var: &str) -> Expr2 {
    let model = sync.models["main"].source;
    let source = model.variables(db)[var];
    let ExplicitFragment::Ready { input, .. } =
        explicit_fragment_input(db, source, model, sync.project, &[])
    else {
        panic!("{var} must lower");
    };
    match input.target.ast() {
        Some(Ast::Scalar(expr)) | Some(Ast::ApplyToAll(_, expr)) => expr.clone(),
        _ => panic!("{var} has a scalar or apply-to-all body"),
    }
}

/// The rows are the shapes a reference can have at the `Expr2` tier: a bare
/// arrayed dependency, a scalar dependency, a subscripted arrayed dependency,
/// and a module output (bare and subscripted), which carries no bounds by
/// decision -- the compiler resolves it through the instance's shape, and the
/// `Expr2` tier does not start a second resolver.
#[test]
fn the_expr2_tier_reads_dependency_shapes() {
    let plain = TestProject::new("shapes")
        .with_sim_time(0.0, 1.0, 1.0)
        .indexed_dimension("d", 3)
        .aux("s", "2", None)
        .array_aux("arr[d]", "d")
        .array_aux("scaled[d]", "arr * s")
        .aux("total", "SUM(arr[*])", None)
        .build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &plain);

    // A bare arrayed dependency carries its declared axis by name, a scalar
    // dependency carries no bounds, and their product is a temp over the axis.
    let Expr2::Op2(_, arr, s, product, _) = lowered_body(&db, &sync, "scaled") else {
        panic!("scaled is a product");
    };
    assert_eq!(
        arr.get_array_bounds(),
        Some(&ArrayBounds::Named {
            name: "arr".to_string(),
            dims: vec![3],
            dim_names: Some(vec!["d".to_string()]),
        })
    );
    assert_eq!(s.get_array_bounds(), None);
    assert_eq!(
        product.as_deref(),
        Some(&ArrayBounds::Temp {
            dims: vec![3],
            dim_names: Some(vec!["d".to_string()]),
        })
    );

    // A wildcard over an arrayed dependency is a temp over the wildcarded axis.
    let Expr2::App(BuiltinFn::Sum(arg), _, _) = lowered_body(&db, &sync, "total") else {
        panic!("total is a SUM");
    };
    let Expr2::Subscript(_, _, bounds, _) = &*arg else {
        panic!("total reduces over a subscript");
    };
    assert_eq!(
        bounds.as_deref(),
        Some(&ArrayBounds::Temp {
            dims: vec![3],
            dim_names: Some(vec!["d".to_string()]),
        })
    );

    // A module output carries no bounds, bare (`sub.output * 2 + SUM(arr)`) or
    // subscripted (`sub.arr[2]`), while the arrayed dependency beside it does.
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(
        &db,
        &super::fragment_input_tests::module_and_array_project(),
    );
    let Expr2::Op2(_, scaled_output, reduction, _, _) = lowered_body(&db, &sync, "usesub") else {
        panic!("usesub is a sum of two terms");
    };
    let Expr2::Op2(_, output, _, _, _) = *scaled_output else {
        panic!("the first term scales the module output");
    };
    assert!(
        matches!(*output, Expr2::Var(ref id, None, _) if id.as_str() == "sub\u{00b7}output"),
        "a bare module output read carries no bounds"
    );
    let Expr2::App(BuiltinFn::Sum(arr), _, _) = *reduction else {
        panic!("the second term reduces over arr");
    };
    assert!(
        matches!(&*arr, Expr2::Var(_, Some(bounds), _) if matches!(**bounds, ArrayBounds::Named { .. })),
        "the arrayed dependency beside it carries its axis"
    );
    assert!(
        matches!(
            lowered_body(&db, &sync, "pick"),
            Expr2::Subscript(ref id, _, None, _) if id.as_str() == "sub\u{00b7}arr"
        ),
        "a subscripted module output read carries no bounds"
    );
}

/// Two arrayed dependencies over different named axes cannot be combined
/// position for position: the `Op2` and the `If` arms of the bounds
/// unification each refuse as `MismatchedDimensions` on the variable.
#[test]
fn mismatched_dependency_axes_are_refused_at_lowering() {
    let errs = TestProject::new("mismatched")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Cities", &["Boston", "Seattle"])
        .named_dimension("Products", &["Widgets", "Gadgets"])
        .array_aux("sales[Cities]", "1")
        .array_aux("prices[Products]", "1")
        .aux("added", "sales + prices", None)
        .aux("chosen", "IF TIME > 0 THEN sales ELSE prices", None)
        .error_diagnostics();
    for var in ["added", "chosen"] {
        assert!(
            errs.contains(&(format!("main.{var}"), ErrorCode::MismatchedDimensions)),
            "{var} is refused as MismatchedDimensions: {errs:?}"
        );
    }
}

/// `sales + prices + nosuch` has two things wrong with it, and exactly one is
/// reported: the lowering refusal (`MismatchedDimensions`), which is about
/// the equation as written, outranks the unknown name, which is about the
/// model around it. An equation whose only fault is an unknown name reports
/// that.
#[test]
fn a_lowering_refusal_outranks_an_unknown_dependency() {
    let errs = TestProject::new("order")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Cities", &["Boston", "Seattle"])
        .named_dimension("Products", &["Widgets", "Gadgets"])
        .array_aux("sales[Cities]", "1")
        .array_aux("prices[Products]", "1")
        .aux("both", "sales + prices + nosuch", None)
        .aux("unknown_only", "nosuch2 + nosuch1", None)
        .error_diagnostics();
    let codes_on = |var: &str| -> Vec<ErrorCode> {
        let location = format!("main.{var}");
        errs.iter()
            .filter(|(loc, _)| *loc == location)
            .map(|(_, code)| *code)
            .collect()
    };
    assert_eq!(
        codes_on("both"),
        vec![ErrorCode::MismatchedDimensions],
        "the lowering refusal is the one row: {errs:?}"
    );
    assert_eq!(
        codes_on("unknown_only"),
        vec![ErrorCode::UnknownDependency],
        "an unknown name alone is reported as such: {errs:?}"
    );
}

/// An arrayed equation is typed as a whole before any arm gets its bounds
/// (`ast::typed_ast`, the tier the dependency classification walks), so of
/// two genuine refusals on different arms the typed tier's is the one row:
/// `ABS(1, 2)` on the `a` element (a builtin arity, `BadBuiltinArgs`)
/// outranks `x + s` on the default, which `control` shows is a bounds
/// refusal on its own (`MismatchedDimensions`: `Small` pairs with `Big` as
/// its subdimension at a different length). Within one tier the default's
/// arm outranks the elements'.
#[test]
fn a_typed_tier_refusal_outranks_a_bounds_refusal_on_another_arm() {
    let mut project = TestProject::new("tiers")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Big", &["a", "b", "c"])
        .named_dimension("Small", &["a", "b"])
        .array_aux("x[Big]", "1")
        .array_aux("s[Small]", "1")
        .array_aux("control[Big]", "x + s")
        .build_datamodel();
    project.models[0]
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "y".to_string(),
            equation: datamodel::Equation::Arrayed(
                vec!["Big".to_string()],
                vec![("a".to_string(), "ABS(1, 2)".to_string(), None, None)],
                Some("x + s".to_string()),
                false,
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    let errs = TestProject::from_datamodel(project).error_diagnostics();
    let codes_on = |var: &str| -> Vec<ErrorCode> {
        let location = format!("main.{var}");
        errs.iter()
            .filter(|(loc, _)| *loc == location)
            .map(|(_, code)| *code)
            .collect()
    };
    assert_eq!(
        codes_on("control"),
        vec![ErrorCode::MismatchedDimensions],
        "the default's arm alone is a bounds refusal: {errs:?}"
    );
    assert_eq!(
        codes_on("y"),
        vec![ErrorCode::BadBuiltinArgs],
        "the typed tier's refusal is the one row: {errs:?}"
    );
}

/// `a[Rows,Cols]` and `b[Cols,Rows]` with `b[c,r] = 10 * a[r,c]`, so a product
/// whose axes are paired by NAME sums to `10 * (1 + 4 + 9 + 16 + 25 + 36) =
/// 910`, and one paired by storage position to `860`. `plain` spells the
/// reduction directly; `sm` and `prev` spell it as a stdlib-call argument and
/// as a `PREVIOUS` argument, each of which the parse hoists into a scalar
/// helper.
fn transposed_operands() -> TestProject {
    TestProject::new("transposed")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("Rows", &["r1", "r2"])
        .named_dimension("Cols", &["c1", "c2", "c3"])
        .array_aux("a[Rows,Cols]", "(Rows - 1) * 3 + Cols")
        .array_aux("b[Cols,Rows]", "10 * ((Rows - 1) * 3 + Cols)")
        .aux("plain", "SUM(a * b)", None)
        .aux("sm", "SMTH1(SUM(a * b), 1)", None)
        .aux("prev", "PREVIOUS(SUM(a * b), 0)", None)
}

/// A hoisted argument and a `PREVIOUS` capture read what the plain spelling
/// reads: the operands of the product are paired by axis name in the helper
/// exactly as in `plain`.
#[test]
fn a_hoisted_argument_pairs_axes_as_the_plain_equation_does() {
    let series = transposed_operands().run_vm_expecting_success();
    assert_eq!(series["a[r2,c3]"], vec![6.0, 6.0, 6.0]);
    assert_eq!(series["b[c3,r2]"], vec![60.0, 60.0, 60.0]);
    assert_eq!(series["plain"], vec![910.0, 910.0, 910.0]);

    let helper = |parent: &str| -> (String, Vec<f64>) {
        let prefix = format!("$\u{205a}{parent}\u{205a}");
        let mut keys: Vec<&String> = series
            .keys()
            .filter(|k| k.starts_with(&prefix) && k.contains("arg0"))
            .collect();
        keys.sort();
        let key = keys
            .first()
            .unwrap_or_else(|| panic!("{parent} hoists an argument helper"))
            .to_string();
        let values = series[&key].clone();
        (key, values)
    };
    let (sm_key, sm_values) = helper("sm");
    assert_eq!(
        sm_values,
        vec![910.0, 910.0, 910.0],
        "{sm_key}: the hoisted argument must equal `plain`"
    );
    assert_eq!(series["sm"], vec![910.0, 910.0, 910.0]);
    let (prev_key, prev_values) = helper("prev");
    assert_eq!(
        prev_values,
        vec![910.0, 910.0, 910.0],
        "{prev_key}: the captured argument must equal `plain`"
    );
    assert_eq!(series["prev"], vec![0.0, 910.0, 910.0]);
}

// ── a helper reads what the plain spelling reads ───────────────────────────
//
// The mechanism under test: a parse-synthesized helper lowers under its
// parent's dependency shapes (`implicit_fragment_input`), so the compiler's
// bare-reference rewrite (`lower_pass0`) runs on the helper's arrayed
// references exactly as on the parent's. Where that rewrite decides the answer
// -- a bare arrayed name under a reducer inside an apply-to-all body -- the
// helper and the plain spelling must agree. The decision has three axes, and
// the rows below are their product.

/// How a helper spells a reducer `R(x)` written inside an apply-to-all body,
/// and how its series relates to the plain twin `t[i] = R(x)`'s.
#[derive(Clone, Copy)]
enum HelperKind {
    /// `PREVIOUS(R(x))`: a structural apply-to-all capture; the lagged twin,
    /// from `PREVIOUS`'s default of 0.
    PreviousCapture,
    /// `INIT(R(x))`: a structural apply-to-all capture; the twin's initial
    /// value throughout.
    InitCapture,
    /// `SMTH1(R(x), 1)`: a per-element hoisted argument; the twin's initial
    /// value, then the lagged twin (Euler, `dt` = delay = 1).
    HoistedArg,
    /// `SMTH1(x, 1) + PREVIOUS(R(x))`: a capture minted inside a
    /// module-bearing body, which the parse expands per element; the smooth
    /// plus the lagged twin.
    CaptureInModuleBody,
}

impl HelperKind {
    const ALL: [HelperKind; 4] = [
        HelperKind::PreviousCapture,
        HelperKind::InitCapture,
        HelperKind::HoistedArg,
        HelperKind::CaptureInModuleBody,
    ];

    fn name(self) -> &'static str {
        match self {
            HelperKind::PreviousCapture => "prev",
            HelperKind::InitCapture => "init",
            HelperKind::HoistedArg => "hoist",
            HelperKind::CaptureInModuleBody => "modbody",
        }
    }

    /// The parent's equation for reducer `r` over source `x`, with `smooth_of`
    /// the operand the module-bearing kind smooths.
    fn equation(self, r: &str, x: &str, smooth_of: &str) -> String {
        match self {
            HelperKind::PreviousCapture => format!("PREVIOUS({r}({x}))"),
            HelperKind::InitCapture => format!("INIT({r}({x}))"),
            HelperKind::HoistedArg => format!("SMTH1({r}({x}), 1)"),
            HelperKind::CaptureInModuleBody => {
                format!("SMTH1({smooth_of}, 1) + PREVIOUS({r}({x}))")
            }
        }
    }

    /// The helper's expected series from the twin's and the smooth's.
    fn expected(self, twin: &[f64], smooth: &[f64]) -> Vec<f64> {
        let n = twin.len();
        match self {
            HelperKind::PreviousCapture => std::iter::once(0.0)
                .chain(twin[..n - 1].iter().copied())
                .collect(),
            HelperKind::InitCapture => vec![twin[0]; n],
            HelperKind::HoistedArg => std::iter::once(twin[0])
                .chain(twin[..n - 1].iter().copied())
                .collect(),
            HelperKind::CaptureInModuleBody => (0..n)
                .map(|i| smooth[i] + if i == 0 { 0.0 } else { twin[i - 1] })
                .collect(),
        }
    }
}

/// The target's rank against the source's: a 1-D target over the 1-D `pop`,
/// a 2-D target over the 2-D `pop2`, and a 1-D target over `pop2` (the plain
/// spelling reads one ROW).
#[derive(Clone, Copy)]
enum Rank {
    OneD,
    TwoD,
    Row,
}

impl Rank {
    const ALL: [Rank; 3] = [Rank::OneD, Rank::TwoD, Rank::Row];

    fn name(self) -> &'static str {
        match self {
            Rank::OneD => "1d",
            Rank::TwoD => "2d",
            Rank::Row => "row",
        }
    }

    fn dims(self) -> &'static str {
        match self {
            Rank::OneD | Rank::Row => "[region]",
            Rank::TwoD => "[region,product]",
        }
    }

    fn source(self) -> &'static str {
        match self {
            Rank::OneD => "pop",
            Rank::TwoD | Rank::Row => "pop2",
        }
    }

    /// The whole-array spelling of the source, which the plain twin is pinned
    /// apart from.
    fn whole(self) -> &'static str {
        match self {
            Rank::OneD => "pop[*]",
            Rank::TwoD | Rank::Row => "pop2[*, *]",
        }
    }

    /// The smoothed operand and its series for the module-bearing kind: the
    /// target's own rank, so the module instances are per element of it.
    fn smooth(self) -> (&'static str, &'static str) {
        match self {
            Rank::OneD | Rank::Row => ("pop", "smooth_1d"),
            Rank::TwoD => ("pop2", "smooth_2d"),
        }
    }

    fn elements(self) -> &'static [&'static str] {
        match self {
            Rank::OneD | Rank::Row => &["north", "south"],
            Rank::TwoD => &["north,pa", "north,pb", "south,pa", "south,pb"],
        }
    }
}

const REDUCERS: [&str; 4] = ["SUM", "MAX", "MEAN", "SIZE"];

fn assert_series_close(label: &str, got: &[f64], want: &[f64]) {
    assert_eq!(got.len(), want.len(), "{label}: step count");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert!(
            (g - w).abs() <= 1e-9 * w.abs().max(1.0),
            "{label} at step {i}: got {g}, want {w} (got {got:?}, want {want:?})"
        );
    }
}

/// Every reducer over a bare arrayed name in an apply-to-all body, spelled
/// through every helper kind at every target rank, reads what the plain
/// spelling reads. `pop[region]` is 100 / 200 growing 10% a step and
/// `pop2[region, product]` is `pop` times 1 / 3, so under `[region]` the
/// plain `SUM(pop)` is the element `pop[region]`, `SIZE(pop)` is 1, and
/// `SUM(pop2)` is the row; the whole-array spellings (`SUM(pop[*])` = 300,
/// `SIZE` = 2, `SUM(pop2[*, *])` = 1200) are pinned apart from every twin so
/// the agreement is not vacuous. The rows are helper kind x reducer x rank;
/// the arm the table does not cover -- a helper whose body the compiler
/// refuses -- is `a_helper_is_refused_where_the_plain_spelling_is`.
#[test]
fn a_helper_reads_what_the_plain_spelling_reads() {
    let mut project = TestProject::new("helper_reducers")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .named_dimension("product", &["pa", "pb"])
        .array_stock("pop[region]", "100 * region", &["growth"], &[], None)
        .array_flow("growth[region]", "pop * 0.1", None)
        .array_aux("pop2[region,product]", "pop * (2 * product - 1)")
        .array_aux("smooth_1d[region]", "SMTH1(pop, 1)")
        .array_aux("smooth_2d[region,product]", "SMTH1(pop2, 1)");
    for rank in Rank::ALL {
        for r in REDUCERS {
            let (smooth_of, _) = rank.smooth();
            project = project
                .array_aux(
                    &format!("plain_{}_{}{}", r.to_lowercase(), rank.name(), rank.dims()),
                    &format!("{r}({})", rank.source()),
                )
                .array_aux(
                    &format!("whole_{}_{}{}", r.to_lowercase(), rank.name(), rank.dims()),
                    &format!("{r}({})", rank.whole()),
                );
            for kind in HelperKind::ALL {
                project = project.array_aux(
                    &format!(
                        "{}_{}_{}{}",
                        kind.name(),
                        r.to_lowercase(),
                        rank.name(),
                        rank.dims()
                    ),
                    &kind.equation(r, rank.source(), smooth_of),
                );
            }
        }
    }
    let series = project.run_vm_expecting_success();

    // One concrete anchor, so the relations below are relations between the
    // numbers the fixture describes.
    assert_series_close(
        "plain_sum_1d[north]",
        &series["plain_sum_1d[north]"],
        &[100.0, 110.0, 121.0, 133.1],
    );
    assert_series_close(
        "whole_sum_1d[north]",
        &series["whole_sum_1d[north]"],
        &[300.0, 330.0, 363.0, 399.3],
    );

    for rank in Rank::ALL {
        for r in REDUCERS {
            let r = r.to_lowercase();
            let twin_name = format!("plain_{r}_{}", rank.name());
            let whole_name = format!("whole_{r}_{}", rank.name());
            let apart = rank.elements().iter().any(|e| {
                series[&format!("{twin_name}[{e}]")] != series[&format!("{whole_name}[{e}]")]
            });
            assert!(
                apart,
                "{twin_name} must differ from {whole_name} on some element, or the rows are vacuous"
            );
            for kind in HelperKind::ALL {
                for e in rank.elements() {
                    let twin = &series[&format!("{twin_name}[{e}]")];
                    let smooth = &series[&format!("{}[{e}]", rank.smooth().1)];
                    let helper = format!("{}_{r}_{}[{e}]", kind.name(), rank.name());
                    assert_series_close(&helper, &series[&helper], &kind.expected(twin, smooth));
                }
            }
        }
    }
}

/// A helper is refused exactly where the plain spelling of its body is, and
/// the refusal lands where the Phase 7.5 rule puts it: an `Expr2` refusal on
/// the PARENT at the argument's span (the helper's spans index the parent's
/// equation), a codegen refusal on the helper. The rows: the `Expr2`
/// `MismatchedDimensions` of `sales[Cities] + prices[Products]` for a scalar
/// parent and for an apply-to-all parent over either axis, hoisted or
/// captured; and the codegen refusal of `SUM(pop * scale)` in an apply-to-all
/// body, captured.
#[test]
fn a_helper_is_refused_where_the_plain_spelling_is() {
    // Rows 1-4: `(parent, its equation, the argument, the plain twin)`.
    let rows: [(&str, &str, &str, &str); 4] = [
        ("sm", "SMTH1(sales + prices, 1)", "sales + prices", "plain"),
        (
            "sm_a2a",
            "SMTH1(sales + prices, 1)",
            "sales + prices",
            "plain_a2a",
        ),
        (
            "pv_a2a",
            "PREVIOUS(sales + prices, 0)",
            "sales + prices",
            "plain_a2a",
        ),
        (
            "sm_prod",
            "SMTH1(sales + prices, 1)",
            "sales + prices",
            "plain_a2a",
        ),
    ];
    let diags = TestProject::new("mismatched")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Cities", &["Boston", "Seattle"])
        .named_dimension("Products", &["Widgets", "Gadgets"])
        .array_with_ranges("sales[Cities]", vec![("Boston", "1"), ("Seattle", "2")])
        .array_with_ranges(
            "prices[Products]",
            vec![("Widgets", "10"), ("Gadgets", "20")],
        )
        .aux("plain", "sales + prices", None)
        .array_aux("plain_a2a[Cities]", "sales + prices")
        .aux("sm", "SMTH1(sales + prices, 1)", None)
        .array_aux("sm_a2a[Cities]", "SMTH1(sales + prices, 1)")
        .array_aux("pv_a2a[Cities]", "PREVIOUS(sales + prices, 0)")
        .array_aux("sm_prod[Products]", "SMTH1(sales + prices, 1)")
        .diagnostics_incremental();
    // The one equation refusal a variable carries: `(code, start, end)`.
    let refusal = |var: &str| -> (ErrorCode, usize, usize) {
        let mut found: Vec<(ErrorCode, usize, usize)> = diags
            .iter()
            .filter(|d| d.variable.as_deref() == Some(var))
            .map(|d| match &d.error {
                DiagnosticError::Equation(e) => (e.code, e.start as usize, e.end as usize),
                other => panic!("{var}: expected an equation refusal, got {other:?}"),
            })
            .collect();
        assert_eq!(
            found.len(),
            1,
            "{var} carries exactly one refusal: {found:?}"
        );
        found.pop().unwrap()
    };
    assert!(
        !diags
            .iter()
            .any(|d| d.variable.as_deref().is_some_and(|v| v.starts_with('$'))),
        "no refusal lands on a helper: {diags:?}"
    );
    for (parent, equation, argument, twin) in rows {
        let (twin_code, twin_start, twin_end) = refusal(twin);
        assert_eq!(twin_code, ErrorCode::MismatchedDimensions, "{twin}");
        let (code, start, end) = refusal(parent);
        assert_eq!(code, ErrorCode::MismatchedDimensions, "{parent}");
        let offset = equation
            .find(argument)
            .expect("the argument is in the equation");
        assert_eq!(
            (start, end),
            (twin_start + offset, twin_end + offset),
            "{parent}: the refusal spans the argument inside the parent's equation"
        );
    }

    // Row 5: a reducer over an arrayed EXPRESSION in an apply-to-all body is
    // refused by codegen, for the plain spelling and for its capture alike --
    // on the variable and on the helper respectively, the assembly row a
    // codegen refusal keeps.
    let mut errs = TestProject::new("reducer_expr")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100 * region", &["growth"], &[], None)
        .array_flow("growth[region]", "pop * 0.1", None)
        .aux("scale", "2", None)
        .array_aux("plainx[region]", "SUM(pop * scale)")
        .array_aux("aggx[region]", "PREVIOUS(SUM(pop * scale))")
        .error_diagnostics();
    // Emission order is the assembly's walk; the rows are compared as a set.
    errs.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        errs,
        vec![
            (
                "main.$\u{205a}aggx\u{205a}0\u{205a}arg0".to_string(),
                ErrorCode::NotSimulatable
            ),
            ("main.plainx".to_string(), ErrorCode::NotSimulatable),
        ],
        "the capture is refused as its plain twin is, by codegen, on the helper"
    );
}

/// A variable the compiler refuses through a module-output read is still
/// unit-checked: `mism[d] = m.arr + prices` carries the compiler's
/// `MismatchedDimensions` AND the `unit_mismatch` warning between `m.arr`'s
/// `person` and `prices`' `dollar`. The unit path lowers the module output
/// without bounds (`ast::LoweringScope`), so the `Expr2` tier raises nothing
/// for it and the variable keeps its AST for the unit check.
#[test]
fn a_module_output_read_the_compiler_refuses_is_still_unit_checked() {
    let aux = |ident: &str, equation: datamodel::Equation, units: &str, input: bool| {
        datamodel::Variable::Aux(datamodel::Aux {
            ident: ident.to_string(),
            equation,
            documentation: String::new(),
            units: Some(units.to_string()),
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat {
                can_be_module_input: input,
                ..datamodel::Compat::default()
            },
        })
    };
    let mut project = TestProject::new("module_output_units")
        .with_sim_time(0.0, 1.0, 1.0)
        .indexed_dimension("d", 3)
        .named_dimension("p", &["w1", "w2"])
        .aux("driver", "2", Some("person"))
        .array_aux_direct("prices", vec!["p".to_string()], "5", Some("dollar"))
        .array_aux_direct(
            "mism",
            vec!["d".to_string()],
            "m.arr + prices",
            Some("person"),
        )
        .build_datamodel();
    project.models.push(datamodel::Model {
        name: "sub".to_string(),
        sim_specs: None,
        variables: vec![
            aux(
                "input",
                datamodel::Equation::Scalar("1".to_string()),
                "person",
                true,
            ),
            aux(
                "arr",
                datamodel::Equation::ApplyToAll(vec!["d".to_string()], "input * d".to_string()),
                "person",
                false,
            ),
        ],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    });
    project.models[0]
        .variables
        .push(datamodel::Variable::Module(datamodel::Module {
            ident: "m".to_string(),
            model_name: "sub".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![datamodel::ModuleReference {
                src: "driver".to_string(),
                dst: "m.input".to_string(),
            }],
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let _ = compile_project_incremental(&db, sync.project, "main");
    let diags = collect_all_diagnostics(&db, sync.project);

    let on_mism: Vec<&Diagnostic> = diags
        .iter()
        .filter(|d| d.variable.as_deref() == Some("mism"))
        .collect();
    assert!(
        on_mism.iter().any(|d| matches!(&d.error,
            DiagnosticError::Equation(e) if e.code == ErrorCode::MismatchedDimensions)),
        "the compiler refuses mism as MismatchedDimensions: {diags:?}"
    );
    assert!(
        on_mism.iter().any(|d| d.severity == DiagnosticSeverity::Warning
            && matches!(&d.error,
                DiagnosticError::Unit(crate::common::UnitError::ConsistencyError(
                    ErrorCode::UnitMismatch, _, Some(detail))) if detail.contains("units to match"))),
        "mism is still unit-checked and reports the person/dollar mismatch: {diags:?}"
    );
}
