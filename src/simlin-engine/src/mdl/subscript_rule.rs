// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Vensim's rule for a subscript range on the right-hand side of an
//! equation: "When you use a Subscript Range in an equation it must appear on
//! the left hand side" (vensim.com/documentation/ref_subscripts.html), unless
//! it is mapped onto one that does: "a mapping is an indication to Vensim
//! that a Subscript that appears on the right but not the left of an
//! equation has a valid interpretation"
//! (vensim.com/documentation/ref_subscript_mapping.html), as in
//! `WORK FLOW[PRODUCTS] = WORK FORCE[MFG SKILLS]` with `MFG SKILLS: ... ->
//! PRODUCTS`. A range marked with `!` is summed over and is not one of the
//! equation's subscripts.
//!
//! The tests hold every equation the writer writes to it: a save is a file
//! Vensim reads.
//!
//! It does not check a range used as a value, macro bodies, or `:EXCEPT:`
//! lists.

use std::collections::{HashMap, HashSet};

use crate::common::canonicalize;
use crate::datamodel::Project;
use crate::mdl::ast::{Equation, Expr, MdlItem, Subscript};
use crate::mdl::reader::EquationReader;

/// Each equation of the MDL text `mdl` that names a subscript range on its
/// right-hand side that its left-hand side neither holds nor has a range
/// mapped from it, with the range; `project` is what `mdl` reads as, for its
/// dimensions and their mappings. Macro bodies are not checked.
pub(in crate::mdl) fn ranges_not_on_the_left(mdl: &str, project: &Project) -> Vec<String> {
    let dimensions: HashSet<String> = project
        .dimensions
        .iter()
        .map(|d| canonicalize(&d.name).into_owned())
        .collect();
    // Each range and the ranges it is mapped onto.
    let mapped_onto: HashMap<String, HashSet<String>> = project
        .dimensions
        .iter()
        .map(|d| {
            (
                canonicalize(&d.name).into_owned(),
                d.mappings
                    .iter()
                    .map(|m| canonicalize(&m.target).into_owned())
                    .collect(),
            )
        })
        .collect();
    let mut found = Vec::new();
    for item in EquationReader::new(mdl) {
        let Ok(MdlItem::Equation(eq)) = item else {
            continue;
        };
        let (lhs, rhs): (_, Vec<&Expr<'_>>) = match &eq.equation {
            Equation::Regular(lhs, expr) => (lhs, vec![expr]),
            Equation::WithLookup(lhs, input, _) => (lhs, vec![input.as_ref()]),
            Equation::Data(lhs, Some(expr)) => (lhs, vec![expr]),
            _ => continue,
        };
        let on_the_left: HashSet<String> = lhs
            .subscripts
            .iter()
            .map(|s| match s {
                Subscript::Element(n, _) | Subscript::BangElement(n, _) => {
                    canonicalize(n).into_owned()
                }
            })
            .collect();
        let mut named = Vec::new();
        for expr in rhs {
            ranges_named(expr, &mut named);
        }
        for range in named {
            if !dimensions.contains(&range) || on_the_left.contains(&range) {
                continue;
            }
            let mapped = mapped_onto
                .get(&range)
                .is_some_and(|targets| targets.iter().any(|t| on_the_left.contains(t)));
            if !mapped {
                found.push(format!("{}: {range}", lhs.name));
            }
        }
    }
    found
}

/// Every name `expr` uses as a subscript without a `!`, canonical.
fn ranges_named(expr: &Expr<'_>, out: &mut Vec<String>) {
    let subscripts = |subs: &[Subscript<'_>], out: &mut Vec<String>| {
        for s in subs {
            if let Subscript::Element(n, _) = s {
                out.push(canonicalize(n).into_owned());
            }
        }
    };
    match expr {
        Expr::Var(_, subs, _) => subscripts(subs, out),
        Expr::App(_, subs, args, _, outputs, _) => {
            subscripts(subs, out);
            for a in args.iter().chain(outputs) {
                ranges_named(a, out);
            }
        }
        Expr::Op1(_, e, _) | Expr::Paren(e, _) => ranges_named(e, out),
        Expr::Op2(_, l, r, _) => {
            ranges_named(l, out);
            ranges_named(r, out);
        }
        Expr::Const(..) | Expr::Literal(..) | Expr::Na(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::ranges_not_on_the_left;

    const DIMS: &str = "DimA: A1, A2, A3 ~~|\nSubA: A2, A3 ~~|\nDimB: B1, B2, B3 ~~|\n\
        DimM: M1, M2, M3 -> DimA ~~|\nPrev: P1, P2 -> SubA ~~|\nDimC <-> DimA ~~|\n\
        y[DimA] = 1 ~~|\nyb[DimB] = 2 ~~|\nym[DimM] = 3 ~~|\nyp[Prev] = 4 ~~|\n\
        yc[DimC] = 5 ~~|\nys[SubA] = 6 ~~|\ntbl[DimB]((0,0),(1,1)) ~~|\n";
    const CONTROL: &str = "INITIAL TIME = 0 ~~|\nFINAL TIME = 1 ~~|\nTIME STEP = 1 ~~|\n\
        SAVEPER = TIME STEP ~~|\n\\\\\\---///\n";

    /// Each row: the equations, and the ranges flagged (as `variable: range`).
    /// Every arm the checker walks has a row it flags: a plain reference, a
    /// call's argument, a call's own subscripts (a subscripted lookup), a
    /// `WITH LOOKUP` input, a data equation, an `:EXCEPT:` equation, a range
    /// mapped the wrong way; beside rows it passes.
    #[test]
    fn a_range_is_flagged_exactly_where_the_left_hand_side_does_not_hold_it() {
        let rows: &[(&str, &[&str])] = &[
            // Flagged.
            ("x[DimA] = yb[DimB] ~~|", &["x: dimb"]),
            ("x[DimA] = ys[SubA] ~~|", &["x: suba"]),
            ("x[SubA] = y[DimA] ~~|", &["x: dima"]),
            ("x[A1] = y[DimA] ~~|", &["x: dima"]),
            (
                "x[DimA] = INITIAL(yb[DimB]) + SMOOTH(ys[SubA], 2) ~~|",
                &["x: dimb", "x: suba"],
            ),
            ("S[DimA] = INTEG(yb[DimB], 0) ~~|", &["S: dimb"]),
            ("x[DimA] = tbl[DimB](Time) ~~|", &["x: dimb"]),
            (
                "x[DimA] = WITH LOOKUP(yb[DimB], ((0,0),(1,1))) ~~|",
                &["x: dimb"],
            ),
            ("x[DimA] := yb[DimB] ~~|", &["x: dimb"]),
            (
                "x[DimA] :EXCEPT: [A1] = yb[DimB] ~~|\nx[A1] = 1 ~~|",
                &["x: dimb"],
            ),
            // A range mapped the wrong way.
            ("x[DimM] = y[DimA] ~~|", &["x: dima"]),
            ("x[DimC] = y[DimA] ~~|", &["x: dima"]),
            ("x[SubA] = ym[DimM] ~~|", &["x: dimm"]),
            ("x[DimA] = yp[Prev] ~~|", &["x: prev"]),
            // Passed.
            ("x[DimA] = SUM(yb[DimB!]) ~~|", &[]),
            ("x[DimA] = SUM(y[DimA!]) ~~|", &[]),
            ("x[DimA] = ym[DimM] ~~|", &[]),
            ("x[DimA] = yc[DimC] ~~|", &[]),
            ("x[SubA] = yp[Prev] ~~|", &[]),
            (
                "x[DimA] = VECTOR SELECT(yb[DimB!], yb[DimB!], 0, 1, 1) ~~|",
                &[],
            ),
            ("x[DimA] = VMAX(yb[DimB!]) + y[DimA] ~~|", &[]),
            ("x[DimA] = VECTOR ELM MAP(y[A1], DimA - 1) ~~|", &[]),
            ("x[DimA] :EXCEPT: [SubA] = y[DimA] ~~|", &[]),
            ("x[dima] = y[DIMA] ~~|", &[]),
            ("x[DimA] = tbl[B1](Time) ~~|", &[]),
            ("\"q[DimB]\" = 1 ~~|\nx[DimA] = \"q[DimB]\" + 1 ~~|", &[]),
            // Not checked: a range as a value.
            ("x = DimA ~~|", &[]),
        ];
        for (equations, flagged) in rows {
            let source = format!("{DIMS}{equations}\n{CONTROL}");
            let project = crate::mdl::parse_mdl(&source).expect("the source reads");
            let found = ranges_not_on_the_left(&source, &project);
            assert_eq!(found, *flagged, "{equations}");
        }
    }
}
