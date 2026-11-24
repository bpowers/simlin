// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What the readers report they do not keep from real files: World3's
//! Vensim sketch and a Stella model's interface pages. Each reporting open
//! reads the project the plain open reads, and a file Simlin saved reports
//! no loss the file it was saved from did not.

use std::collections::BTreeMap;
use std::fs;
use std::io::BufReader;
use std::path::PathBuf;

use simlin_engine::{
    ImportWarning, open_vensim, open_vensim_with_warnings, open_xmile, open_xmile_with_warnings,
    to_mdl, to_xmile,
};

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

/// Files larger than this are left out of the corpus walk, so a debug build
/// stays within its time budget (as the MDL writer's corpus ratchets do).
const MAX_CORPUS_BYTES: u64 = 200 * 1024;

/// Every file under `test/` with one of `extensions`, sorted.
fn corpus(extensions: &[&str]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![PathBuf::from(resolve_path("test"))];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            let wanted = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| extensions.iter().any(|x| e.eq_ignore_ascii_case(x)));
            if path.is_dir() {
                dirs.push(path);
            } else if wanted && fs::metadata(&path).is_ok_and(|m| m.len() <= MAX_CORPUS_BYTES) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// How many things of each kind `warnings` say are not kept, over the whole
/// file: what a save of the file loses, whatever the places are called.
fn losses_by_kind(warnings: &[ImportWarning]) -> BTreeMap<String, usize> {
    let mut kinds = BTreeMap::new();
    for warning in warnings {
        *kinds.entry(warning.many.clone()).or_default() += warning.count;
    }
    kinds
}

/// What `saved` reports losing beyond what `original` does, by kind.
fn losses_beyond(
    saved: &[ImportWarning],
    original: &[ImportWarning],
) -> Vec<(String, usize, usize)> {
    let original = losses_by_kind(original);
    losses_by_kind(saved)
        .into_iter()
        .filter_map(|(kind, n)| {
            let had = original.get(&kind).copied().unwrap_or(0);
            (n > had).then_some((kind, n, had))
        })
        .collect()
}

/// A file Simlin saved holds only what Simlin kept, so opening it again
/// reports nothing lost that opening the original did not: no more of any
/// kind of thing than the original lost. A save that wrote something its own
/// reader does not keep (a cloud the writer drew, say) would tell a person
/// who reopens the file that saving it loses what it never held.
#[test]
fn a_file_simlin_saved_reports_no_loss_its_original_did_not() {
    use rayon::prelude::*;

    type Resave = fn(&[u8]) -> Option<(Vec<ImportWarning>, Vec<ImportWarning>)>;
    let mdl: Resave = |bytes| {
        let text = std::str::from_utf8(bytes).ok()?;
        let (project, original) = open_vensim_with_warnings(text).ok()?;
        let saved = to_mdl(&project).ok()?;
        let (_, reopened) = open_vensim_with_warnings(&saved).ok()?;
        Some((original, reopened))
    };
    let xmile: Resave = |bytes| {
        let (project, original) = open_xmile_with_warnings(&mut BufReader::new(bytes)).ok()?;
        let saved = to_xmile(&project).ok()?;
        let (_, reopened) = open_xmile_with_warnings(&mut saved.as_bytes()).ok()?;
        Some((original, reopened))
    };

    for (extensions, resave, floor) in [
        (&["mdl"][..], mdl, 200),
        (&["xmile", "stmx", "itmx"][..], xmile, 200),
    ] {
        let outcomes: Vec<(PathBuf, Option<_>)> = corpus(extensions)
            .into_par_iter()
            .map(|path| {
                let outcome = fs::read(&path).ok().and_then(|bytes| resave(&bytes));
                let beyond =
                    outcome.map(|(original, reopened)| losses_beyond(&reopened, &original));
                (path, beyond)
            })
            .collect();
        let checked = outcomes.iter().filter(|(_, o)| o.is_some()).count();
        assert!(
            checked >= floor,
            "only {checked} {extensions:?} files open, save and open again"
        );
        let failures: Vec<String> = outcomes
            .into_iter()
            .filter_map(|(path, beyond)| {
                let beyond = beyond.filter(|b| !b.is_empty())?;
                Some(format!("{}: {beyond:?}", path.display()))
            })
            .collect();
        assert!(
            failures.is_empty(),
            "saved files report losses their originals did not (kind, saved, original):\n{}",
            failures.join("\n")
        );
    }
}
