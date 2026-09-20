// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What the readers report they do not keep from real files: World3's
//! Vensim sketch and a Stella model's interface pages. Each reporting open
//! reads the project the plain open reads.

use std::fs;
use std::io::BufReader;

use simlin_engine::{open_vensim, open_vensim_with_warnings, open_xmile, open_xmile_with_warnings};

fn resolve_path(relative: &str) -> String {
    format!("../../{relative}")
}

#[test]
fn world3_reports_its_sketch_content_view_by_view() {
    let source = fs::read_to_string(resolve_path("test/metasd/WRLD3-03/wrld3-03.mdl")).unwrap();
    let (project, warnings) = open_vensim_with_warnings(&source).unwrap();
    let messages: Vec<&str> = warnings.iter().map(|w| w.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "15 comments on view 'Title Page' are not kept, such as 'Developed from the World model by Jay W....', 'World3-2003 Model', and 'Use Page Down / Page Up keys to move thr...'",
            "1 comment on view 'Demographics' is not kept: 'Demographics'",
            "1 comment on view 'Fertility' is not kept: 'Fertility'",
            "1 comment on view 'Life Expectancy' is not kept: 'Life Expectancy'",
            "1 comment on view 'Persistant Pollution' is not kept: 'Persistant Pollution'",
            "1 comment on view 'Nonrenewable Resources' is not kept: 'Nonrenewable Resources'",
            "1 comment on view 'Food Production' is not kept: 'Food Production'",
            "1 comment on view 'Agriculture Productivity' is not kept: 'Agriculture Productivity'",
            "1 comment on view 'Land Development, Loss, Fertility' is not kept: 'Land Development, Loss, Fertility'",
            "1 comment on view 'Industrial Output' is not kept: 'Industrial Output'",
            "1 comment on view 'Services Output' is not kept: 'Services Output'",
            "1 comment on view 'Jobs' is not kept: 'Jobs'",
            "1 comment on view 'Welfare & Footprint' is not kept: 'Welfare & Footprint'",
            "2 comments on view 'Output Graphs' are not kept: 'Click on the SyntheSim Icon' and 'and move sliders to see what changes'",
            "3 graphs on view 'Output Graphs' are not kept: 'STATE_OF_WORLD', 'MATERIAL_STANDARD_LIVING', and 'HUMAN_WELFARE'",
            "7 sliders on view 'Output Graphs' are not kept, such as 'initial nonrenewable resources', 'land life policy implementation time', and 'technology development delay'",
            "1 image on view 'Output Graphs' is not kept: 'wrld3-030000.bmp'",
            "23 drawings of variables on 10 views are not kept, such as 'Time'",
            "24 arrows on 10 views are not kept",
            "3 clouds on 2 views are not kept",
            "4 custom graphs in the model are not kept, such as 'STATE_OF_WORLD', 'MATERIAL_STANDARD_LIVING', and 'WIP_STATE_OF_WORLD'",
            "5 reports in the model are not kept, such as 'COMM1', 'COMM2', and 'COMM3'",
        ]
    );
    assert!(project == open_vensim(&source).unwrap());
}

#[test]
fn a_stella_model_reports_its_interface_pages() {
    let source = fs::read(resolve_path("test/conveyors/covid19_severity.stmx")).unwrap();
    let (project, warnings) =
        open_xmile_with_warnings(&mut BufReader::new(source.as_slice())).unwrap();
    let messages: Vec<&str> = warnings.iter().map(|w| w.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "2 graphs and tables on the diagram are not kept, such as 'active_by_condition_for_display[*]'",
            "1 graph or table on interface page 1 is not kept: 'Infection Curve'",
            "1 text box on interface page 1 is not kept: 'The graphs in the published sim, are bel...'",
            "4 sliders on interface page 2 are not kept, such as 'Baseline Contact', 'Mildly Symptomatic Adjustment', and 'Symptomatic Adjustment'",
            "4 annotations on interface page 2 are not kept, such as 'The number of unique individuals that a...', 'The adjustment to contacts that mildly s...', and 'The adjustment to contacts that symptoma...'",
            "4 sliders on interface page 3 are not kept, such as 'Baseline Contact', 'Mildly Symptomatic Adjustment', and 'Symptomatic Adjustment'",
            "4 annotations on interface page 3 are not kept, such as 'The number of unique individuals that a...', 'The adjustment to contacts that mildly s...', and 'The adjustment to contacts that symptoma...'",
            "4 sliders on interface page 4 are not kept, such as 'Infected not Contagious', 'Contagious not Symptomatic', and 'Symptomatic and Contagious by Severity'",
            "4 annotations on interface page 4 are not kept, such as 'After first being infected, an individua...', 'After being infected, there may be a per...', and 'The COVID-19 disease progression is rela...'",
            "1 selector on interface page 4 is not kept",
            "1 text box on interface page 5 is not kept: 'Note: Asymptomatic is not noticable, Mil...'",
            "1 pie input on interface page 5 is not kept: 'Severity Spread'",
            "1 slider on interface page 5 is not kept: 'Infectivity'",
            "2 annotations on interface page 5 are not kept: 'This lets you set the distribution of se...' and 'The probability that the COV-19 will pas...'",
            "6 sliders on interface page 6 are not kept, such as 'Quarantine Start Day', 'Quarantine Duration', and 'Symptomatic Test Rate'",
            "6 annotations on interface page 6 are not kept, such as 'Start – The time when an action is taken...', 'Duration – How long the change to behavi...', and 'Effectiveness – The extent to which cont...'",
            "2 text boxes on interface page 6 are not kept: 'Global Quarantine Settings' and 'Testing Settings'",
        ]
    );
    assert!(project == open_xmile(&mut BufReader::new(source.as_slice())).unwrap());
}
