// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What the MDL reader reports it does not keep ([`parse_mdl_with_warnings`]).

use super::{parse_mdl, parse_mdl_with_warnings};

/// Equations for one flow into one stock, and the control variables.
const EQUATIONS: &str = "{UTF-8}
Stock= INTEG (
\tInflow,
\t\t100)
\t~\tpeople
\t~\t\t|

Inflow=
\tRate * Stock
\t~\tpeople/Month
\t~\t\t|

Rate=
\t0.1
\t~\t1/Month
\t~\t\t|

FINAL TIME  = 100
\t~\tMonth
\t~\t\t|

INITIAL TIME  = 0
\t~\tMonth
\t~\t\t|

SAVEPER  = TIME STEP
\t~\tMonth
\t~\t\t|

TIME STEP  = 1
\t~\tMonth
\t~\t\t|

\\\\\\---/// Sketch information - do not modify anything except names
V300  Do not put anything below this section - it will be ignored
";

/// A sketch holding one of each thing the reader does not keep: comments,
/// input/output objects, images, a drawing of Time, arrows to them, a
/// record of a type Vensim does not document, and, on a second view, a copy
/// of the flow whose pipe ends in a cloud no kept flow ends in, and Time
/// again. After the sketch, a custom graph, a custom table and a report.
const SKETCH: &str = "*View 1
$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0
10,1,Stock,300,200,40,20,3,3,0,0,0,0,0,0
12,2,48,150,200,10,8,0,3,0,0,-1,0,0,0
1,3,5,1,4,0,0,22,0,0,0,-1--1--1,,1|(250,200)|
1,4,5,2,100,0,0,22,0,0,0,-1--1--1,,1|(180,200)|
11,5,48,215,200,6,8,34,3,0,0,1,0,0,0
10,6,Inflow,215,225,30,11,40,3,0,0,-1,0,0,0
10,7,Rate,215,300,20,11,8,3,0,0,0,0,0,0
1,8,7,6,0,0,0,0,0,64,0,-1--1--1,,1|(0,0)|
12,9,0,300,100,80,20,8,135,0,0,-1,0,0,0
A note about the stock\\nthat wraps
12,10,0,400,150,15,15,5,4,0,0,-1,0,0,0
B
12,11,0,500,300,150,150,3,188,0,0,1,0,0,0
Stock_graph
12,12,0,500,500,80,20,3,124,0,0,0,0,0,0
Rate,0,1,0.01
12,13,0,700,300,150,150,3,188,0,0,2,0,0,0
\"Stock\",graph
12,14,0,700,500,150,150,3,188,0,0,2,0,0,0
Stock,Table
12,15,0,100,400,40,20,8,0,0,0,-1,0,0,0
30,16,logo0000.bmp,50,50,8,8,8,0,0,0,-1,0,0,0
31,17,chart.emf,60,60,8,8,8,0,0,0,-1,0,0,0
10,18,Time,300,400,26,11,8,2,0,3,-1,0,0,0,128-128-128,0-0-0,|12||128-128-128
1,19,18,9,0,0,0,0,0,64,0,-1--1--1,,1|(0,0)|
1,20,7,9,0,0,0,0,0,64,0,-1--1--1,,1|(0,0)|
99,21,mystery,10,10,1,1,0,0
\\\\\\---/// Sketch information - do not modify anything except names
V300  Do not put anything below this section - it will be ignored
*View 2
$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0
12,1,48,100,100,10,8,0,3,0,0,-1,0,0,0
1,2,3,1,100,0,0,22,0,0,0,-1--1--1,,1|(130,100)|
11,3,48,160,100,6,8,34,3,0,0,1,0,0,0
10,4,Inflow,160,125,30,11,40,2,0,3,-1,0,0,0,128-128-128,0-0-0,|12||128-128-128
10,5,Time,300,300,26,11,8,2,0,3,-1,0,0,0,128-128-128,0-0-0,|12||128-128-128
///---\\\\\\
:GRAPH Stock_graph
:TITLE Stock over time
:SCALE
:VAR Stock
:TABLE Stock_table
:VAR Stock
:REPORT Notes
\tsome text
:END-OF-REPORT
:L\x7F<%^E!@
1:Current.vdf
9:Current
15:0,0,0,0,0,0
";

#[test]
fn a_sketch_reports_its_content_view_by_view_and_its_consequences_once() {
    let source = format!("{EQUATIONS}{SKETCH}");
    let (project, warnings) = parse_mdl_with_warnings(&source).unwrap();
    let messages: Vec<&str> = warnings.iter().map(|w| w.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "3 comments on view 'View 1' are not kept, such as \
             'A note about the stock that wraps' and 'B'",
            "2 graphs on view 'View 1' are not kept: 'Stock_graph' and 'Stock'",
            "1 slider on view 'View 1' is not kept: 'Rate'",
            "1 input/output object on view 'View 1' is not kept: 'Stock'",
            "2 images on view 'View 1' are not kept: 'logo0000.bmp' and 'chart.emf'",
            // What follows from what the diagram does not draw, once for the
            // whole sketch.
            "2 drawings of variables on 2 views are not kept, such as 'Time'",
            "2 arrows on view 'View 1' are not kept",
            "1 cloud on view 'View 2' is not kept",
            "1 record of an unknown type on view 'View 1' is not kept",
            "1 custom graph in the model is not kept: 'Stock_graph'",
            "1 custom table in the model is not kept: 'Stock_table'",
            "1 report in the model is not kept: 'Notes'",
        ]
    );
    // Reporting reads the same project the plain open does.
    assert!(project == parse_mdl(&source).unwrap());
}

#[test]
fn a_sketch_that_holds_only_what_the_diagram_keeps_reports_nothing() {
    // The first view without its comments, objects, images, Time, the
    // record of an unknown type, and the definitions after the sketch.
    let kept: String = SKETCH
        .lines()
        .take_while(|line| !line.starts_with("12,9,"))
        .map(|line| format!("{line}\n"))
        .collect();
    let source = format!("{EQUATIONS}{kept}///---\\\\\\\n:L\x7F<%^E!@\n");
    let (_, warnings) = parse_mdl_with_warnings(&source).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
}
