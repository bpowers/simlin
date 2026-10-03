// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A save is a fixed point: what the writer writes reads back as a model the
//! writer writes the same text for. The corpus holds the whole-file property
//! (`tests/integration/mdl_roundtrip.rs`, `writer_output_idempotence_ratchet`);
//! these tests pin the rules that make it hold, each over the cases its rule
//! decides between.

use super::*;
use crate::datamodel::{self, ViewElement};
use crate::mdl::builtins::BUILTINS;
use crate::mdl::view::processing::{ANGLE_EPSILON_DEGREES, angle_between, connector_shape};
use crate::mdl::{parse_mdl, project_to_mdl};

/// The variable named `name` (canonically) in a project's first model.
fn variable<'a>(project: &'a datamodel::Project, name: &str) -> Option<&'a datamodel::Variable> {
    project.models[0]
        .variables
        .iter()
        .find(|v| crate::common::canonicalize(v.get_ident()) == crate::common::canonicalize(name))
}

/// A variable with its equation text lowercased.
fn case_folded(var: &datamodel::Variable) -> datamodel::Variable {
    let mut var = var.clone();
    let fold = |eq: &mut datamodel::Equation| match eq {
        datamodel::Equation::Scalar(text) | datamodel::Equation::ApplyToAll(_, text) => {
            *text = text.to_lowercase();
        }
        datamodel::Equation::Arrayed(_, elements, default, _) => {
            for (_, text, initial, _) in elements.iter_mut() {
                *text = text.to_lowercase();
                if let Some(initial) = initial {
                    *initial = initial.to_lowercase();
                }
            }
            if let Some(default) = default {
                *default = default.to_lowercase();
            }
        }
    };
    match &mut var {
        datamodel::Variable::Stock(s) => fold(&mut s.equation),
        datamodel::Variable::Flow(f) => fold(&mut f.equation),
        datamodel::Variable::Aux(a) => fold(&mut a.equation),
        datamodel::Variable::Module(_) => {}
    }
    var
}

/// Parse, write, parse again and write again.
fn two_saves(source: &str) -> (datamodel::Project, String, datamodel::Project, String) {
    let first = parse_mdl(source).expect("the source parses");
    let save1 = project_to_mdl(&first).expect("the first save writes");
    let second =
        parse_mdl(&save1).unwrap_or_else(|err| panic!("the first save reads back: {err}\n{save1}"));
    let save2 = project_to_mdl(&second).expect("the second save writes");
    (first, save1, second, save2)
}

const CONTROL: &str = "
INITIAL TIME = 0 ~~|
FINAL TIME = 10 ~~|
TIME STEP = 1 ~~|
SAVEPER = TIME STEP ~~|
";

/// What the builtin rows below are written against.
const BUILTIN_SCAFFOLD: &str = "
d: d1, d2, d3 ~~|
p: ptype, ppriority, pwidth, pextra ~~|
x = 1 ~~|
arr[d] = 1, 2, 3 ~~|
sel[d] = 1, 0, 1 ~~|
pp[d, p] = 1, 1, 1, 0; 1, 2, 1, 0; 1, 3, 1, 0 ~~|
tbl([(0,0)-(2,2)],(0,0),(1,1),(2,2)) ~~|
";

/// A call of every builtin the MDL reader knows, as the equation of `y`.
/// Every entry of `BUILTINS` has a row here or in `BUILTINS_NOT_SAVED`.
const BUILTIN_CALLS: &[(&str, &str)] = &[
    ("abs", "y = ABS(x)"),
    ("exp", "y = EXP(x)"),
    ("sqrt", "y = SQRT(x)"),
    ("ln", "y = LN(x)"),
    ("log", "y = LOG(x, 10)"),
    ("sin", "y = SIN(x)"),
    ("cos", "y = COS(x)"),
    ("tan", "y = TAN(x)"),
    ("arcsin", "y = ARCSIN(x)"),
    ("arccos", "y = ARCCOS(x)"),
    ("arctan", "y = ARCTAN(x)"),
    ("integer", "y = INTEGER(x)"),
    ("modulo", "y = MODULO(x, 3)"),
    ("quantum", "y = QUANTUM(x, 2)"),
    ("max", "y = MAX(x, 1)"),
    ("min", "y = MIN(x, 1)"),
    ("vmax", "y = VMAX(arr[d!])"),
    ("vmin", "y = VMIN(arr[d!])"),
    ("if then else", "y = IF THEN ELSE(x > 1, 1, 0)"),
    ("zidz", "y = ZIDZ(x, 2)"),
    ("xidz", "y = XIDZ(x, 2, 0)"),
    ("pulse", "y = PULSE(1, 2)"),
    ("pulse train", "y = PULSE TRAIN(1, 2, 3, 10)"),
    ("step", "y = STEP(1, 2)"),
    ("ramp", "y = RAMP(1, 2, 5)"),
    ("smooth", "y = SMOOTH(x, 2)"),
    ("smoothi", "y = SMOOTHI(x, 2, 1)"),
    ("smooth3", "y = SMOOTH3(x, 2)"),
    ("smooth3i", "y = SMOOTH3I(x, 2, 1)"),
    ("smooth n", "y = SMOOTH N(x, 2, 1, 3)"),
    ("delay1", "y = DELAY1(x, 2)"),
    ("delay1i", "y = DELAY1I(x, 2, 1)"),
    ("delay3", "y = DELAY3(x, 2)"),
    ("delay3i", "y = DELAY3I(x, 2, 1)"),
    ("delay fixed", "y = DELAY FIXED(x, 2, 1)"),
    ("delay n", "y = DELAY N(x, 2, 1, 3)"),
    ("delay conveyor", "y = DELAY CONVEYOR(x, 2, 0, 0, 0, 0)"),
    ("trend", "y = TREND(x, 2, 0)"),
    ("forecast", "y = FORECAST(x, 2, 3)"),
    ("integ", "y = INTEG(x, 1)"),
    ("active initial", "y = ACTIVE INITIAL(x, 1)"),
    ("initial", "y = INITIAL(x)"),
    ("reinitial", "y = REINITIAL(x)"),
    ("sample if true", "y = SAMPLE IF TRUE(x > 1, x, 0)"),
    (
        "with lookup",
        "y = WITH LOOKUP(x, ([(0,0)-(2,2)],(0,0),(1,1),(2,2)))",
    ),
    ("lookup invert", "y = LOOKUP INVERT(tbl, 0.5)"),
    ("lookup area", "y = LOOKUP AREA(tbl, 0, 1)"),
    ("lookup extrapolate", "y = LOOKUP EXTRAPOLATE(tbl, 3)"),
    ("lookup forward", "y = LOOKUP FORWARD(tbl, 0.5)"),
    ("lookup backward", "y = LOOKUP BACKWARD(tbl, 0.5)"),
    ("tabxl", "y = TABXL(tbl, 3)"),
    ("sum", "y = SUM(arr[d!])"),
    ("prod", "y = PROD(arr[d!])"),
    ("elmcount", "y = ELMCOUNT(d)"),
    (
        "vector select",
        "y = VECTOR SELECT(sel[d!], arr[d!], 0, 0, 0)",
    ),
    ("vector elm map", "y[d] = VECTOR ELM MAP(arr[d1], 0)"),
    ("vector rank", "y[d] = VECTOR RANK(arr[d], 1)"),
    ("vector sort order", "y[d] = VECTOR SORT ORDER(arr[d], 1)"),
    ("vector reorder", "y[d] = VECTOR REORDER(arr[d], sel[d])"),
    ("vector lookup", "y = VECTOR LOOKUP(arr[d!], x, 0, 2, 0)"),
    ("random 0 1", "y = RANDOM 0 1()"),
    ("random uniform", "y = RANDOM UNIFORM(0, 1, 0)"),
    ("random normal", "y = RANDOM NORMAL(0, 1, 0, 1, 0)"),
    ("random pink noise", "y = RANDOM PINK NOISE(0, 1, 1, 0)"),
    ("random poisson", "y = RANDOM POISSON(0, 10, 1, 0, 1, 0)"),
    ("a function of", "y = A FUNCTION OF(x)"),
    ("game", "y = GAME(x)"),
    ("time base", "y = TIME BASE(0, 1)"),
    ("npv", "y = NPV(x, 0.1, 0, 1)"),
    ("sshape", "y = SSHAPE(x, 1)"),
    ("ramp from to", "y = RAMP FROM TO(1, 0, 2, 10)"),
    (
        "allocate available",
        "y[d] = ALLOCATE AVAILABLE(arr[d], pp[d, ptype], x)",
    ),
    (
        "allocate by priority",
        "y[d] = ALLOCATE BY PRIORITY(arr[d], sel[d], ELMCOUNT(d), 1, x)",
    ),
    ("tabbed array", "y[d] = TABBED ARRAY(1\t2\t3)"),
];

/// Builtins no row calls, and why.
const BUILTINS_NOT_SAVED: &[(&str, &str)] = &[
    (
        "get data at time",
        "reads a data variable, which needs a data provider to import",
    ),
    (
        "get data between times",
        "reads a data variable, which needs a data provider to import",
    ),
    (
        "get data last time",
        "reads a data variable, which needs a data provider to import",
    ),
    (
        "get direct data",
        "reads an external file, which needs a data provider to import",
    ),
    (
        "get data mean",
        "reads a data variable, which needs a data provider to import",
    ),
];

/// Builtins the writer writes under another spelling, and that spelling.
const WRITTEN_AS: &[(&str, &str)] = &[
    // The reader keeps only the initial value REINITIAL has in common with
    // INITIAL.
    ("reinitial", "INITIAL(x)"),
    // The placeholder for a variable with no equation.
    ("a function of", "NaN"),
    // A list of numbers over a dimension: tabs are one way to write it.
    ("tabbed array", "y[d]= 1, 2, 3"),
];

#[test]
fn every_builtin_the_reader_knows_is_saved_as_it_reads() {
    let mut named: Vec<&str> = BUILTIN_CALLS
        .iter()
        .chain(BUILTINS_NOT_SAVED)
        .map(|(name, _)| *name)
        .collect();
    named.sort_unstable();
    let mut known: Vec<&str> = BUILTINS.iter().copied().collect();
    known.sort_unstable();
    assert_eq!(
        named, known,
        "every builtin the reader knows needs a row (or a reason it has none)"
    );

    let mut failures = Vec::new();
    for (name, call) in BUILTIN_CALLS {
        let source = format!("{BUILTIN_SCAFFOLD}\n{call} ~~|\n{CONTROL}");
        let first = match parse_mdl(&source) {
            Ok(project) => project,
            Err(err) => {
                failures.push(format!("{name}: the row does not parse: {err}"));
                continue;
            }
        };
        let save = project_to_mdl(&first).expect("the model writes");
        // The save calls the function the file called, by the name the file
        // gave it or the one `WRITTEN_AS` gives: what the reader expands (a
        // PULSE into a comparison) the writer has to find again.
        let written = WRITTEN_AS
            .iter()
            .find(|(builtin, _)| builtin == name)
            .map_or_else(|| name.to_uppercase(), |(_, written)| written.to_string());
        let line = save
            .replace("\r\n", "\n")
            .replace("\n\t", " ")
            .lines()
            .find(|line| line.starts_with("y"))
            .unwrap_or("<no y line>")
            .to_string();
        if !line.contains(&written) {
            failures.push(format!("{name}: saved without {written}: {line}"));
        }
        let second = match parse_mdl(&save) {
            Ok(project) => project,
            Err(err) => {
                failures.push(format!("{name}: its save does not read back: {err}"));
                continue;
            }
        };
        // Vensim reads names and numbers without regard to case, and the
        // writer spells the NaN a `A FUNCTION OF` placeholder imports as
        // `NaN`, so equations compare case-insensitively.
        let before = variable(&first, "y").map(case_folded);
        let after = variable(&second, "y").map(case_folded);
        if before.is_none() || before != after {
            failures.push(format!(
                "{name}: y reads back differently after a save\n  save: {}",
                save.lines()
                    .find(|line| line.trim_start().starts_with('y'))
                    .unwrap_or("<no y line>")
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---- Connector control points ----

/// A transform placing a segment at `(dx, dy)` in the view.
fn offset(dx: f64, dy: f64) -> SketchTransform {
    SketchTransform {
        x_offset: dx,
        y_offset: dy,
    }
}

/// A point in the segment, placed in the view.
fn in_view(point: (i32, i32), transform: SketchTransform) -> (i32, i32) {
    (
        point.0 + transform.x_offset as i32,
        point.1 + transform.y_offset as i32,
    )
}

/// The shape the importer reads a written point as.
fn reads_as(point: (i32, i32), from: (i32, i32), to: (i32, i32), t: SketchTransform) -> LinkShape {
    let at = |p: (i32, i32)| {
        let p = in_view(p, t);
        (p.0 as f64, p.1 as f64)
    };
    let written = if point == (0, 0) {
        (0, 0)
    } else {
        in_view(point, t)
    };
    connector_shape(at(from), at(to), written)
}

#[test]
fn a_recorded_point_is_written_while_it_reads_as_the_links_shape() {
    let (from, to) = ((100, 100), (300, 140));
    for t in [SketchTransform::identity(), offset(100.0, 1220.0)] {
        // A point Vensim put off the bisector: the arc through it.
        let recorded = in_view((180, 60), t);
        let shape = reads_as((180, 60), from, to, t);
        assert!(matches!(shape, LinkShape::Arc(_)));
        assert_eq!(
            connector_control_point(&shape, Some(recorded), from, to, t),
            (180, 60),
            "an untouched arc keeps the point its file had"
        );
        // A straight connector's point that sits just off the chord.
        let recorded = in_view((200, 120), t);
        let straight = reads_as((200, 120), from, to, t);
        assert!(matches!(straight, LinkShape::Straight));
        assert_eq!(
            connector_control_point(&straight, Some(recorded), from, to, t),
            (200, 120),
            "an untouched straight connector keeps its point too"
        );
    }
}

#[test]
fn a_moved_or_reshaped_link_gets_a_point_that_reads_as_its_shape() {
    let (from, to) = ((100, 100), (300, 140));
    let t = offset(40.0, 60.0);
    let recorded = in_view((180, 60), t);
    let LinkShape::Arc(angle) = reads_as((180, 60), from, to, t) else {
        panic!("an arc");
    };
    // The endpoint moved: the recorded point no longer reads as the arc.
    let moved_to = (320, 180);
    let point = connector_control_point(&LinkShape::Arc(angle), Some(recorded), from, moved_to, t);
    assert_ne!(point, (180, 60));
    assert!(matches!(
        reads_as(point, from, moved_to, t),
        LinkShape::Arc(_)
    ));
    // The arc was reshaped: likewise.
    let reshaped = LinkShape::Arc(angle + 20.0);
    let point = connector_control_point(&reshaped, Some(recorded), from, to, t);
    assert_ne!(point, (180, 60));
    let LinkShape::Arc(read) = reads_as(point, from, to, t) else {
        panic!("reads as an arc");
    };
    assert!(
        angle_between(read, angle + 20.0) < 1.0,
        "{read} vs {}",
        angle + 20.0
    );
    // A link made straight writes the straight sentinel.
    assert_eq!(
        connector_control_point(&LinkShape::Straight, Some(recorded), from, to, t),
        (0, 0)
    );
}

/// A splitmix64 generator, so the property rows are the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo + 1) as u64) as i32
    }

    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Every link shape the writer can be handed: arcs at every bend, down to
/// ones too flat for a whole point to bend, and straight links, in segments
/// placed anywhere in the view. Whatever point the writer computes, the file
/// read back gives the link a shape and a recorded point that the next save
/// writes unchanged.
#[test]
fn a_written_connector_point_is_written_again_unchanged() {
    let mut rng = Rng(0x5eed);
    let (mut arcs, mut flattened) = (0, 0);
    for _ in 0..4000 {
        let from = (rng.range(0, 1500), rng.range(0, 1500));
        let to = (rng.range(0, 1500), rng.range(0, 1500));
        if from == to {
            continue;
        }
        let t = offset(
            rng.range(0, 3) as f64 * 100.0,
            rng.range(0, 30) as f64 * 100.0,
        );
        let chord = ((to.1 - from.1) as f64)
            .atan2((to.0 - from.0) as f64)
            .to_degrees();
        // Bends from a hair off the chord to strongly curved.
        let bend = if rng.unit() < 0.3 {
            rng.unit() * 0.5
        } else {
            rng.unit() * 170.0
        };
        let side = if rng.unit() < 0.5 { 1.0 } else { -1.0 };
        let mut angle = chord + side * bend;
        if angle > 180.0 {
            angle -= 360.0;
        } else if angle < -180.0 {
            angle += 360.0;
        }
        let shape = if bend < ANGLE_EPSILON_DEGREES {
            LinkShape::Straight
        } else {
            LinkShape::Arc(angle)
        };

        let written = connector_control_point(&shape, None, from, to, t);
        let read = reads_as(written, from, to, t);
        match (&shape, &read) {
            (LinkShape::Arc(_), LinkShape::Arc(_)) => arcs += 1,
            (LinkShape::Arc(_), LinkShape::Straight) => flattened += 1,
            _ => {}
        }
        let recorded = (written != (0, 0)).then(|| in_view(written, t));
        let rewritten = connector_control_point(&read, recorded, from, to, t);
        assert_eq!(
            rewritten, written,
            "{shape:?} from {from:?} to {to:?} at ({}, {}): wrote {written:?}, read {read:?}",
            t.x_offset, t.y_offset
        );
    }
    assert!(
        arcs > 1500 && flattened > 50,
        "coverage: {arcs} arcs, {flattened} flattened"
    );
}

// ---- Arrayed entries, the save step, names ----

#[test]
fn arrayed_entries_are_written_in_one_order_whatever_the_stored_order() {
    let source = "
d: d3, d1, d2 ~~|
x[d3] = 3 ~~|
x[d1] = 1 ~~|
x[d2] = 2 ~~|
y[d] = 7, 8, 9 ~~|
";
    let project = parse_mdl(&format!("{source}{CONTROL}")).expect("parses");
    let mut reversed = project.clone();
    reversed.models[0].variables.rewrite(|variables| {
        for var in variables.iter_mut() {
            let equation = match var {
                datamodel::Variable::Stock(s) => &mut s.equation,
                datamodel::Variable::Flow(f) => &mut f.equation,
                datamodel::Variable::Aux(a) => &mut a.equation,
                datamodel::Variable::Module(_) => continue,
            };
            if let datamodel::Equation::Arrayed(_, elements, _, _) = equation {
                elements.reverse();
            }
        }
    });
    assert_eq!(
        project_to_mdl(&project).unwrap(),
        project_to_mdl(&reversed).unwrap()
    );
    let (_, save1, _, save2) = two_saves(&format!("{source}{CONTROL}"));
    assert_eq!(save1, save2);
}

#[test]
fn a_save_step_is_written_as_it_was_read() {
    // A SAVEPER that follows the time step stays `TIME STEP`, including one the
    // importer cannot evaluate; a number stays that number, even the time
    // step's own, so a later change to TIME STEP in Vensim moves only the
    // SAVEPERs that followed it. Each is a fixed point.
    let cases = [
        ("TIME STEP", "TIME STEP"),
        ("IF THEN ELSE(Time < 1, TIME STEP, 0.5)", "TIME STEP"),
        ("0.5", "0.5"),
        ("2", "2"),
    ];
    for (saveper, written) in cases {
        let source = format!(
            "x = 1 ~~|\nINITIAL TIME = 0 ~~|\nFINAL TIME = 10 ~~|\nTIME STEP = 0.5 ~~|\nSAVEPER = {saveper} ~~|\n"
        );
        let (_, save1, _, save2) = two_saves(&source);
        assert_eq!(save1, save2, "SAVEPER = {saveper}");
        assert!(
            save1
                .replace("\r\n", "\n")
                .contains(&format!("SAVEPER  = \n\t{written}\n")),
            "SAVEPER = {saveper}:\n{save1}"
        );
    }
}

#[test]
fn a_step_reads_back_as_the_step_it_was_written_from() {
    // A step is a number or a count per time unit (`1/4`), for the time step
    // and the save step alike, and the reader takes each back as it was.
    use datamodel::Dt;
    let steps = [Dt::Dt(0.25), Dt::Reciprocal(4.0), Dt::Dt(2.0)];
    let mut project = parse_mdl(&format!("x = 1 ~~|\n{CONTROL}")).expect("parses");
    for dt in &steps {
        for save_step in steps.iter().map(Some).chain([None]) {
            project.sim_specs.dt = dt.clone();
            project.sim_specs.save_step = save_step.cloned();
            let save = project_to_mdl(&project).expect("writes");
            let read = parse_mdl(&save).expect("the save reads back");
            assert_eq!(read.sim_specs, project.sim_specs, "{save}");
        }
    }
}

#[test]
fn a_display_newline_is_written_as_the_escape_vensim_writes() {
    // Vensim spells a name holding a line break with the `\n` escape inside
    // quotes, in the equation and the sketch record alike
    // (`test/test-models/tests/special_characters`), and the reader keeps
    // the escape, so the variable keeps its name and its sketch element.
    assert_eq!(
        format_mdl_ident(r"Stock_with_\n_Newline"),
        r#""Stock with \n Newline""#
    );
    assert_eq!(format_mdl_ident("Two\nlines"), r#""Two\nlines""#);
    let source = format!(
        "\"Stock with \\n Newline\" = INTEG(0, 1) ~~|\n{CONTROL}\\\\\\---/// Sketch information - do not modify anything except names
V300  Do not put anything below this section - it will be ignored
*View 1
$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0
10,1,\"Stock with \\n Newline\",300,200,40,20,3,3,0,0,0,0,0,0
///---\\\\\\
"
    );
    let (first, save1, second, save2) = two_saves(&source);
    assert_eq!(save1, save2);
    assert!(save1.contains(r#""Stock with \n Newline"="#), "{save1}");
    assert!(
        save1.contains(r#"10,1,"Stock with \n Newline","#),
        "{save1}"
    );
    let idents = |project: &datamodel::Project| -> Vec<String> {
        project.models[0]
            .variables
            .iter()
            .map(|v| v.get_ident().to_owned())
            .collect()
    };
    assert_eq!(idents(&first), [r"stock_with_\n_newline"]);
    assert_eq!(idents(&second), idents(&first));
    let stocks = |project: &datamodel::Project| {
        let datamodel::View::StockFlow(sf) = &project.models[0].views[0];
        sf.elements
            .iter()
            .filter(|e| matches!(e, ViewElement::Stock(_)))
            .count()
    };
    assert_eq!((stocks(&first), stocks(&second)), (1, 1));
}

#[test]
fn a_name_with_an_apostrophe_is_written_bare_as_vensim_writes_it() {
    // `DimC'` is a name to the MDL lexer, which reads `'` after a name's
    // first character as part of it, and Vensim writes it bare
    // (`test/sdeverywhere/models/arrays_cname`). Quoted, it would read back
    // as a name holding the quotes.
    assert_eq!(format_mdl_ident("DimC'"), "DimC'");
    assert_eq!(format_mdl_ident("'quoted"), "\"'quoted\"");
    let source = format!(
        "DimC: C1, C2 ~~|\nDimC': DimC ~~|\nsc[DimC, DimC'] = 1 ~~|\nx'y = sc[C1, C2] ~~|\n{CONTROL}"
    );
    let (first, save1, second, save2) = two_saves(&source);
    assert_eq!(save1, save2);
    assert_eq!(second.dimensions, first.dimensions, "{save1}");
    assert_eq!(
        second.models[0]
            .variables
            .iter()
            .map(|v| v.get_ident())
            .collect::<Vec<_>>(),
        first.models[0]
            .variables
            .iter()
            .map(|v| v.get_ident())
            .collect::<Vec<_>>(),
        "{save1}"
    );
}

#[test]
fn a_variable_named_for_a_builtin_is_never_defined_by_itself() {
    // `dt = TIME STEP` is ordinary Vensim (`test/metasd/theil-statistics`).
    // The variable's own equation reads the builtin, so it is written as the
    // builtin; another equation's `dt` is written as the name it is, which
    // the reader reads as it read the source.
    let source = format!("dt = TIME STEP ~~|\nx = dt * 2 ~~|\n{CONTROL}");
    let (first, save1, second, save2) = two_saves(&source);
    assert!(save1.contains("dt = TIME STEP"), "{save1}");
    assert!(save1.contains("x = dt * 2"), "{save1}");
    assert_eq!(save1, save2);
    let equations = |project: &datamodel::Project| {
        project.models[0]
            .variables
            .iter()
            .map(|v| (v.get_ident().to_owned(), v.get_equation().cloned()))
            .collect::<Vec<_>>()
    };
    assert_eq!(equations(&second), equations(&first), "{save1}");
}

#[test]
fn a_file_imports_its_dimensions_one_way() {
    // `y`'s elements are held by two dimensions of one size; the one declared
    // first owns them, on every parse (a hash map's order decided it before).
    let source = format!(
        "DimA: A1, A2, A3 ~~|\nSubA: A2, A3 ~~|\nDimX: SubA, A1 ~~|\ny[A1] = 1 ~~|\ny[A2] = 2 ~~|\ny[A3] = 3 ~~|\n{CONTROL}"
    );
    for _ in 0..12 {
        let project = parse_mdl(&source).expect("parses");
        let Some(datamodel::Equation::Arrayed(dims, _, _, _)) =
            variable(&project, "y").and_then(|v| v.get_equation())
        else {
            panic!("y is arrayed");
        };
        assert_eq!(dims, &["DimA".to_string()]);
    }
}

#[test]
fn a_mapped_dimension_takes_one_element_on_every_parse() {
    // `DimD` maps to both of `x`'s dimensions, and `DimC` maps to `DimD`
    // alone. At `x[A2, B1]`, `DimD` takes its element through the first
    // dimension `x` names (`A2` is `D2`; `B1` would be `D1`), and `DimC`
    // takes its element through `DimD`'s, on every parse.
    let source = format!(
        "DimA: A1, A2 ~~|\nDimB: B1, B2 ~~|\nDimD: D1, D2 -> DimA, DimB ~~|\nDimC: C1, C2 -> DimD ~~|\ny[DimD] = 10, 20 ~~|\nz[DimC] = 1, 2 ~~|\nx[DimA, DimB] :EXCEPT: [A1, B1] = y[DimD] + z[DimC] ~~|\nx[A1, B1] = 0 ~~|\n{CONTROL}"
    );
    for _ in 0..12 {
        let project = parse_mdl(&source).expect("parses");
        let Some(datamodel::Equation::Arrayed(_, elements, _, _)) =
            variable(&project, "x").and_then(|v| v.get_equation())
        else {
            panic!("x is arrayed");
        };
        let equation = |key: &str| {
            elements
                .iter()
                .find(|(k, _, _, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, text, _, _)| text.to_lowercase())
        };
        assert_eq!(equation("a2,b1").as_deref(), Some("y[d2] + z[c2]"));
        assert_eq!(equation("a1,b2").as_deref(), Some("y[d1] + z[c1]"));
    }
}

#[test]
fn a_sibling_subrange_takes_its_element_through_the_first_the_lhs_names() {
    // `Mid` is a subrange of `x`'s two dimensions' parent. At `x[L2, L3]` it
    // takes its element by position through `Upper`, which `x` names first
    // (`L2` is Upper's 2nd, so Mid's 2nd, `L3`), not through `Lower`, which
    // sorts first by name (`L3` is Lower's 1st, so `L2`).
    let source = format!(
        "Layers: L1, L2, L3, L4 ~~|\nUpper: L1, L2 ~~|\nLower: L3, L4 ~~|\nMid: L2, L3 ~~|\ny[Layers] = 1, 2, 3, 4 ~~|\nx[Upper, Lower] :EXCEPT: [L1, L3] = y[Mid] ~~|\nx[L1, L3] = 0 ~~|\n{CONTROL}"
    );
    for _ in 0..12 {
        let project = parse_mdl(&source).expect("parses");
        let Some(datamodel::Equation::Arrayed(_, elements, _, _)) =
            variable(&project, "x").and_then(|v| v.get_equation())
        else {
            panic!("x is arrayed");
        };
        let equation = elements
            .iter()
            .find(|(k, _, _, _)| k.eq_ignore_ascii_case("l2,l3"))
            .map(|(_, text, _, _)| text.to_lowercase());
        assert_eq!(equation.as_deref(), Some("y[l3]"));
    }
}

// ---- Flows drawn in one view ending on a stock drawn in another ----

const SKETCH_HEADER: &str = "\\\\\\---/// Sketch information - do not modify anything except names
V300  Do not put anything below this section - it will be ignored";
const VIEW_FONT: &str =
    "$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0";

/// A model whose flow `g` fills `S`, with `S` drawn in View 1 and `g` drawn
/// in View 2 from a cloud, plus whatever else View 2 holds.
fn two_view_flow(view2_extra: &str) -> String {
    format!(
        "S = INTEG(g, 0) ~~|\ng = 1 ~~|\n{CONTROL}{SKETCH_HEADER}
*View 1
{VIEW_FONT}
10,1,S,300,200,40,20,3,3,0,0,0,0,0,0
{SKETCH_HEADER}
*View 2
{VIEW_FONT}
12,1,48,100,200,10,8,0,3,0,0,-1,0,0,0
1,2,4,1,100,0,0,22,0,0,0,-1--1--1,,1|(125,200)|
{view2_extra}11,4,48,200,200,6,8,34,3,0,0,1,0,0,0
10,5,g,200,220,10,11,40,3,0,0,-1,0,0,0
///---\\\\\\
"
    )
}

/// The records of each view of a save, by view title.
fn views_of(save: &str) -> Vec<(String, Vec<String>)> {
    let mut views: Vec<(String, Vec<String>)> = Vec::new();
    let sketch = save.find("\\\\\\---///").map_or("", |start| &save[start..]);
    for line in sketch.lines() {
        if let Some(title) = line.strip_prefix('*') {
            views.push((title.to_string(), Vec::new()));
        } else if line.starts_with("///---") {
            break;
        } else if let Some((_, records)) = views.last_mut()
            && line.chars().next().is_some_and(|c| c.is_ascii_digit())
        {
            records.push(line.to_string());
        }
    }
    views
}

/// Every connector of a view names elements the view holds.
fn assert_connectors_resolve(save: &str) {
    for (title, records) in views_of(save) {
        let uids: std::collections::HashSet<&str> = records
            .iter()
            .filter(|r| !r.starts_with("1,"))
            .filter_map(|r| r.split(',').nth(1))
            .collect();
        for connector in records.iter().filter(|r| r.starts_with("1,")) {
            let fields: Vec<&str> = connector.split(',').collect();
            for end in [fields[2], fields[3]] {
                assert!(
                    uids.contains(end),
                    "{title}: {connector} names {end}, which the view does not hold\n{save}"
                );
            }
        }
    }
}

#[test]
fn a_flow_into_a_stock_drawn_only_in_another_view_is_cut_at_a_cloud() {
    // The model links `g` to `S`, which View 2 does not draw, so the importer
    // routes `g` to `S` in View 1; the file cannot hold that pipe.
    let (first, save1, _, save2) = two_saves(&two_view_flow(""));
    let datamodel::View::StockFlow(sf) = &first.models[0].views[0];
    assert!(
        sf.elements.iter().any(|e| matches!(e,
            ViewElement::Flow(f) if f.points.last().and_then(|p| p.attached_to_uid).is_some()
        )),
        "the importer routes g to S"
    );
    assert_connectors_resolve(&save1);
    let view2 = &views_of(&save1)[1].1;
    assert_eq!(
        view2.iter().filter(|r| r.starts_with("12,")).count(),
        2,
        "g's drawn cloud, and the cloud its cut end runs into: {view2:?}"
    );
    assert_eq!(save1, save2);
}

#[test]
fn a_cloud_placed_after_the_views_merge_is_written_with_its_flow() {
    // `f` fills `T`, and its sketch draws its upstream pipe into `S`, which
    // does not list it, so that end is a cloud near `S` -- one the importer
    // places after the views merge, at the end of the element list, in View
    // 2's segment.
    let source = format!(
        "S = INTEG(1, 5) ~~|\nT = INTEG(f, 0) ~~|\nf = 1 ~~|\nx = 1 ~~|\n{CONTROL}{SKETCH_HEADER}
*View 1
{VIEW_FONT}
10,1,S,100,200,40,20,3,3,0,0,0,0,0,0
10,2,T,400,200,40,20,3,3,0,0,0,0,0,0
1,3,5,2,4,0,0,22,0,0,0,-1--1--1,,1|(360,200)|
1,4,5,1,100,0,0,22,0,0,0,-1--1--1,,1|(140,200)|
11,5,48,250,200,6,8,34,3,0,0,1,0,0,0
10,6,f,250,220,10,11,40,3,0,0,-1,0,0,0
{SKETCH_HEADER}
*View 2
{VIEW_FONT}
10,1,x,300,200,40,20,8,3,0,0,0,0,0,0
///---\\\\\\
"
    );
    let (first, save1, _, save2) = two_saves(&source);
    let datamodel::View::StockFlow(sf) = &first.models[0].views[0];
    let Some(ViewElement::Cloud(cloud)) = sf.elements.last() else {
        panic!("the importer appends f's cloud after both views' elements");
    };
    let view1 = &sf.sketch_compat.as_ref().expect("an MDL view").segments[0];
    let at = format!(
        "{},{}",
        (cloud.x - view1.x_offset).round(),
        (cloud.y - view1.y_offset).round()
    );
    assert_connectors_resolve(&save1);
    let views = views_of(&save1);
    let clouds: Vec<&String> = views[0].1.iter().filter(|r| r.starts_with("12,")).collect();
    assert!(
        clouds.len() == 1
            && clouds[0]
                .split(',')
                .skip(3)
                .take(2)
                .collect::<Vec<_>>()
                .join(",")
                == at,
        "View 1 draws f's cloud where the importer placed it ({at}): {clouds:?}"
    );
    assert!(
        views[1].1.iter().all(|r| !r.starts_with("12,")),
        "View 2 holds no cloud: {:?}",
        views[1].1
    );
    assert_eq!(save1, save2);
}

// ---- Groups ----

/// A group marker as Vensim writes one.
fn group_marker(name: &str) -> String {
    format!(
        "********************************************************\n\t.{name}\n********************************************************~\n\t|\n"
    )
}

/// Each group's name, parent and members, in the model's order.
fn groups_of(project: &datamodel::Project) -> Vec<(String, Option<String>, Vec<String>)> {
    project.models[0]
        .groups
        .iter()
        .map(|g| (g.name.clone(), g.parent.clone(), g.members.clone()))
        .collect()
}

#[test]
fn a_save_keeps_every_variable_in_its_group() {
    // A variable before any group, a Control group in the middle that holds
    // a variable of the model's own, and groups on either side of it. Where a
    // variable is written is the group the reader gives it, and where a group
    // is written is what its parent is read from.
    let source = format!(
        "loose = 1 ~~|\n{first}a = 2 ~~|\n{control}horizon = 10 ~~|\n{CONTROL}{second}b = 3 ~~|\nc = 4 ~~|\n",
        first = group_marker("First"),
        control = group_marker("Control"),
        second = group_marker("Second"),
    );
    let (first, save1, second, save2) = two_saves(&source);
    let some = |s: &str| Some(s.to_string());
    let names = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    assert_eq!(
        groups_of(&first),
        [
            ("First".to_string(), None, names(&["a"])),
            ("Control".to_string(), some("First"), names(&["horizon"])),
            ("Second".to_string(), some("Control"), names(&["b", "c"])),
        ]
    );
    assert_eq!(groups_of(&second), groups_of(&first), "{save1}");
    assert_eq!(save1, save2);
}

#[test]
fn a_grouped_multi_output_macro_call_stays_in_its_group() {
    // The call is written as the `:`-list invocation it was read from, in
    // the group its left-hand side is in.
    let source = format!(
        ":MACRO: ADD3(a, b, c : minval, maxval)\nADD3 = a + b + c ~~|\n\
         minval = MIN(a, MIN(b, c)) ~~|\nmaxval = MAX(a, MAX(b, c)) ~~|\n:END OF MACRO:\n\
         in1 = 7 ~~|\n{only}total = ADD3(in1, in1, in1 : the min, the max) ~~|\n{CONTROL}",
        only = group_marker("Only")
    );
    let (first, save1, second, save2) = two_saves(&source);
    let main = |project: &datamodel::Project| {
        project
            .models
            .iter()
            .find(|m| m.macro_spec.is_none())
            .map(|m| {
                m.groups
                    .iter()
                    .map(|g| (g.name.clone(), g.members.clone()))
                    .collect::<Vec<_>>()
            })
            .expect("a main model")
    };
    assert!(
        main(&first)[0].1.contains(&"total".to_owned()),
        "{:?}",
        main(&first)
    );
    assert_eq!(main(&second), main(&first), "{save1}");
    assert_eq!(save1, save2);
}

#[test]
fn a_save_adds_no_group() {
    // Without a Control group the control variables are written with the
    // variables no group holds, so the save reads back with the groups the
    // model had: none, or its own.
    for source in [
        format!("x = 1 ~~|\n{CONTROL}"),
        format!(
            "x = 1 ~~|\n{CONTROL}{only}y = 2 ~~|\n",
            only = group_marker("Only")
        ),
    ] {
        let (first, save1, second, save2) = two_saves(&source);
        assert_eq!(groups_of(&second), groups_of(&first), "{save1}");
        assert_eq!(second.sim_specs, first.sim_specs);
        assert_eq!(save1, save2);
    }
}

#[test]
fn the_group_holding_the_control_variables_is_written_as_the_model_names_it() {
    // The control variables are written in the model's own Control group,
    // whatever its case, and the group keeps its name.
    let source = format!("x = 1 ~~|\n{}{CONTROL}", group_marker("control"));
    let (first, save1, second, save2) = two_saves(&source);
    assert!(
        save1.replace("\r\n", "\n").contains("\t.control\n"),
        "{save1}"
    );
    assert_eq!(groups_of(&second), groups_of(&first), "{save1}");
    assert_eq!(save1, save2);
}

// ---- Time units ----

#[test]
fn the_time_unit_is_written_as_a_units_field() {
    // A `~` ends the units field and a `|` the equation, so either written
    // raw would cut the control variable short and leave the rest of the
    // unit to be read as something else.
    for (units, read_back) in [
        (Some("Months"), "Months"),
        // The separator becomes a space, and the reader joins the words of a
        // unit's name as it does for any variable's units.
        (Some("person~years"), "person_years"),
        (Some("a|b"), "a/b"),
        // No time unit is the engine's `time`, by name.
        (None, "time"),
        (Some(""), "time"),
        (Some("  "), "time"),
    ] {
        let mut project = parse_mdl(&format!("x = 1 ~~|\n{CONTROL}")).expect("the source parses");
        project.sim_specs.time_units = units.map(str::to_owned);
        let save1 = project_to_mdl(&project).expect("the first save writes");
        let second = parse_mdl(&save1)
            .unwrap_or_else(|err| panic!("the save of {units:?} reads back: {err}\n{save1}"));
        assert_eq!(
            second.sim_specs.time_units.as_deref(),
            Some(read_back),
            "{units:?}"
        );
        assert_eq!(
            idents_of(&second),
            idents_of(&project),
            "{units:?} changed the variables: {save1}"
        );
        let save2 = project_to_mdl(&second).expect("the second save writes");
        let third = parse_mdl(&save2).expect("the second save reads back");
        let save3 = project_to_mdl(&third).expect("the third save writes");
        assert_eq!(save2, save3, "{units:?}");
        if !read_back.contains('_') {
            assert_eq!(save1, save2, "{units:?}");
        }
    }
}

/// The names of a project's variables, sorted.
fn idents_of(project: &datamodel::Project) -> Vec<String> {
    let mut idents: Vec<String> = project.models[0]
        .variables
        .iter()
        .map(|v| v.get_ident().to_owned())
        .collect();
    idents.sort();
    idents
}

#[test]
fn a_model_with_no_time_unit_keeps_its_unit_check() {
    // Without a time unit the engine checks a flow against its stock per
    // `time`. The save names that unit, so the model it reads back as passes
    // the check the model passed.
    let unit_errors = |project: &datamodel::Project| {
        let mut db = crate::db::SimlinDb::default();
        let source = db.sync(project);
        crate::db::collect_all_diagnostics(&db, source, crate::db::LtmOverlay::Off)
            .iter()
            .map(|d| format!("{:?}: {:?}", d.variable, d.reason()))
            .collect::<Vec<_>>()
    };
    let mut project = parse_mdl(&format!(
        "level = INTEG(rate, 0) ~ widgets ~|\nrate = 2 ~ widgets/time ~|\n{CONTROL}"
    ))
    .expect("the source parses");
    project.sim_specs.time_units = None;
    assert_eq!(unit_errors(&project), Vec::<String>::new());

    let save = project_to_mdl(&project).expect("the save writes");
    let read_back = parse_mdl(&save).expect("the save reads back");
    assert_eq!(unit_errors(&read_back), Vec::<String>::new(), "{save}");
}

#[test]
fn the_settings_follow_the_main_models_specs() {
    // A macro is a model of the project, listed first here, and the run the
    // file defines is the main model's.
    let source = format!(
        ":MACRO: DOUBLE(x)\nDOUBLE = 2 * x ~~|\n:END OF MACRO:\ny = DOUBLE(3) ~~|\n{CONTROL}"
    );
    let mut project = parse_mdl(&source).expect("the source parses");
    let at = project
        .models
        .iter()
        .position(|m| m.macro_spec.is_some())
        .expect("the macro is a model");
    project.models.swap(0, at);
    let mut rk4 = project.sim_specs.clone();
    rk4.sim_method = datamodel::SimMethod::RungeKutta4;
    project.models[0].sim_specs = Some(rk4);
    let save = project_to_mdl(&project).expect("the model writes");
    assert!(save.contains("15:0,0,0,0,0,0"), "{save}");
    assert_eq!(
        parse_mdl(&save)
            .expect("the save reads back")
            .sim_specs
            .sim_method,
        datamodel::SimMethod::Euler
    );
}

// ---- Numbers ----

/// Numbers a shortest spelling finds awkward: subnormal, the smallest
/// normal, not representable in decimal, past where f64 counts integers, and
/// the largest.
const AWKWARD_NUMBERS: &[f64] = &[
    5e-324,
    1e-320,
    2.2250738585072014e-308,
    0.1 + 0.2,
    1.0 / 3.0,
    4.35,
    1e15,
    1e15 + 1.0,
    9007199254740993.0,
    1e21,
    1e22,
    123456789012345680000.0,
    1.7976931348623157e308,
    1e-7,
    0.000001,
    2.5e-5,
    1e100,
    1.5e300,
];

/// The values a number spelled `literal` holds after an import and after a
/// save of it: as a constant, a number-list element and a lookup point.
/// Err with the first that is not `value`, bit for bit.
fn a_number_survives(value: f64, literal: &str) -> std::result::Result<(), String> {
    let source = format!(
        "D: d1, d2 ~~|\nx = {literal} ~~|\narr[D] = {literal}, 1 ~~|\n\
         t([(0,0)-(10,10)],(0,{literal}),(1,2)) ~~|\ny = t(0) ~~|\n{CONTROL}"
    );
    let first = parse_mdl(&source).map_err(|e| format!("{literal}: {e}"))?;
    let save = project_to_mdl(&first).map_err(|e| format!("{literal}: {e}"))?;
    let second = parse_mdl(&save).map_err(|e| format!("{literal}: the save: {e}"))?;
    for (which, project) in [("import", &first), ("save", &second)] {
        let var = |name: &str| {
            project.models[0]
                .variables
                .iter()
                .find(|v| v.get_ident() == name)
                .cloned()
                .ok_or_else(|| format!("{literal}: no {name} after {which}"))
        };
        let number = |text: &str| text.trim().parse::<f64>().unwrap_or(f64::NAN);
        let x = match var("x")?.get_equation() {
            Some(datamodel::Equation::Scalar(text)) => number(text),
            other => return Err(format!("{literal}: x after {which} is {other:?}")),
        };
        let element = match var("arr")?.get_equation() {
            Some(datamodel::Equation::Arrayed(_, slots, _, _)) => slots
                .iter()
                .find(|(key, _, _, _)| key == "d1")
                .map_or(f64::NAN, |(_, text, _, _)| number(text)),
            other => return Err(format!("{literal}: arr after {which} is {other:?}")),
        };
        let point = match var("t")? {
            datamodel::Variable::Aux(aux) => aux.gf.map_or(f64::NAN, |gf| gf.y_points[0]),
            other => return Err(format!("{literal}: t after {which} is {other:?}")),
        };
        for (what, got) in [("x", x), ("arr[d1]", element), ("t's point", point)] {
            if got.to_bits() != value.to_bits() {
                return Err(format!("{literal}: {what} after {which} is {got:e}"));
            }
        }
    }
    Ok(())
}

/// The numbers to try, `extra` random finite ones after the awkward ones,
/// each in Rust's shortest spelling, plain and in exponent form.
fn numbers(extra: usize) -> Vec<(f64, String)> {
    let mut values: Vec<f64> = AWKWARD_NUMBERS.to_vec();
    // A fixed xorshift sequence of bit patterns.
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    while values.len() < AWKWARD_NUMBERS.len() + extra {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let v = f64::from_bits(state);
        if v.is_finite() && v != 0.0 {
            values.push(v.abs());
        }
    }
    values
        .into_iter()
        .flat_map(|v| [(v, format!("{v}")), (v, format!("{v:e}"))])
        .collect()
}

#[test]
fn a_number_reads_back_bit_for_bit() {
    let failures: Vec<String> = numbers(0)
        .into_iter()
        .filter_map(|(v, literal)| a_number_survives(v, &literal).err())
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[ignore = "3000 random numbers through an import and a save; run under the gates profile"]
fn any_number_reads_back_bit_for_bit() {
    use rayon::prelude::*;
    let failures: Vec<String> = numbers(3000)
        .into_par_iter()
        .filter_map(|(v, literal)| a_number_survives(v, &literal).err())
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_reference_is_spelled_as_its_definition() {
    // Rows: a name holding an apostrophe, inside and at the end, which the
    // reader stores quoted in a referring equation and Vensim writes bare.
    let source =
        format!("it's ok = 1 ~~|\nw = it's ok + 2 ~~|\nx' = 3 ~~|\ny = x' + 1 ~~|\n{CONTROL}");
    let project = parse_mdl(&source).expect("the source reads");
    let save = project_to_mdl(&project).expect("the model writes");
    for written in ["it's ok = 1", "w = it's ok + 2", "x' = 3", "y = x' + 1"] {
        assert!(save.contains(written), "{written}\n{save}");
    }
}

#[test]
fn a_quoted_name_holding_a_line_break_is_written_with_the_escape() {
    // A name that is already quoted keeps its interior as written, but a real
    // line break in it is written as the `\n` escape: written raw it would
    // end the equation. Such a name comes from an API or XMILE, not the MDL
    // reader, so the project is built with one.
    let source = format!("x = 1 ~~|\ny = x * 2 ~~|\n{CONTROL}");
    let mut project = parse_mdl(&source).expect("the source reads");
    project.models[0].variables.edit_where(
        |v| v.get_ident() == "x",
        |v| {
            if let datamodel::Variable::Aux(aux) = v {
                aux.ident = "\"a\nb\"".to_owned();
            }
        },
    );
    let save = project_to_mdl(&project).expect("the model writes");
    let save = save.replace("\r\n", "\n");
    assert!(save.contains("\"a\\nb\" = 1"), "{save}");
    parse_mdl(&save).expect("the save reads back");
}
