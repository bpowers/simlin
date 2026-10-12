// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! An MDL file's per-element copies opened as one equation.

use crate::datamodel::{Equation, Project};
use crate::test_common::TestProject;

const CONTROL: &str =
    "INITIAL TIME = 0 ~~|\nFINAL TIME = 2 ~~|\nTIME STEP = 1 ~~|\nSAVEPER = 1 ~~|\n";

fn open(mdl: &str) -> Project {
    crate::open_vensim(&format!("{mdl}{CONTROL}\\\\\\---///\n")).expect("the model opens")
}

fn equation(project: &Project, ident: &str) -> Equation {
    project.models[0]
        .get_variable(ident)
        .and_then(|v| v.get_equation())
        .cloned()
        .unwrap_or_else(|| panic!("no {ident}"))
}

fn run(project: &Project) -> std::collections::HashMap<String, Vec<f64>> {
    TestProject::from_datamodel(project.clone())
        .run_vm()
        .expect("the model runs")
}

const AGES: &str = "Age: young, adult, old ~~|\nOlder: adult, old ~~|\nc[Age] = 10, 20, 30 ~~|\n";

#[test]
fn elements_that_say_one_thing_open_as_one_equation() {
    // Two equations define `d`, so the reader writes it out per element, with
    // each element's name in its copy.
    let project = open(&format!(
        "{AGES}d[young] = c[young] * 2 ~~|\nd[Older] = c[Older] * 2 ~~|\n"
    ));
    assert_eq!(
        equation(&project, "d"),
        Equation::ApplyToAll(vec!["Age".to_string()], "c[Age] * 2".to_string())
    );
    assert_eq!(run(&project)["d[old]"], vec![60.0, 60.0, 60.0]);
}

#[test]
fn an_except_equations_copies_follow_its_default() {
    let project = open(&format!(
        "{AGES}d[Age] :EXCEPT: [young] = c[Age] + 1 ~~|\nd[young] = 0 ~~|\n"
    ));
    let Equation::Arrayed(_, arms, Some(default), true) = equation(&project, "d") else {
        panic!("a default and an exception: {:?}", equation(&project, "d"));
    };
    assert_eq!(default, "c[Age] + 1");
    let keys: Vec<&str> = arms.iter().map(|a| a.0.as_str()).collect();
    assert_eq!(keys, ["young"]);
}

/// A number for every element but its exceptions is one equation, as the
/// file writes it, though each element is a number: the elements that are
/// that number follow it.
#[test]
fn a_number_for_every_element_keeps_its_followers() {
    let project = open(&format!(
        "{AGES}d[Age] :EXCEPT: [old] = 0.01 ~~|\nd[old] = 0.08 ~~|\n"
    ));
    let Equation::Arrayed(dims, arms, Some(default), true) = equation(&project, "d") else {
        panic!("a default and an exception: {:?}", equation(&project, "d"));
    };
    assert_eq!(dims, ["Age"]);
    assert_eq!(default, "0.01");
    let arms: Vec<(&str, &str)> = arms.iter().map(|a| (a.0.as_str(), a.1.as_str())).collect();
    assert_eq!(arms, [("old", "0.08")]);
}

/// A copy that does not read as the default keeps its equation, though it
/// computes what the default does there: at `young`, `c[young] / c[young]`
/// reads `c[Age] / c[Age]`, which is not the default's text. Keeping it costs
/// an equation and changes no number.
#[test]
fn a_copy_that_does_not_read_as_the_default_keeps_its_equation() {
    let project = open(&format!(
        "{AGES}d[Age] :EXCEPT: [old] = c[Age] / c[young] ~~|\nd[old] = 0 ~~|\n"
    ));
    let Equation::Arrayed(_, arms, Some(default), true) = equation(&project, "d") else {
        panic!(
            "a default and its exceptions: {:?}",
            equation(&project, "d")
        );
    };
    assert_eq!(default, "c[Age] / c[young]");
    let arms: Vec<(&str, &str)> = arms.iter().map(|a| (a.0.as_str(), a.1.as_str())).collect();
    assert_eq!(arms, [("old", "0"), ("young", "c[young] / c[young]")]);
    let results = run(&project);
    assert_eq!(results["d[young]"], vec![1.0, 1.0, 1.0]);
    assert_eq!(results["d[adult]"], vec![2.0, 2.0, 2.0]);

    // So such a default made elsewhere gains that exception when it is saved
    // as MDL, and the save check, which compares the element as it reads,
    // calls it a change to how `d` is written. It changes no result.
    let built = TestProject::new("fixed")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("Age", &["young", "adult", "old"])
        .array_with_ranges(
            "c[Age]",
            vec![("young", "10"), ("adult", "20"), ("old", "30")],
        )
        .array_with_default_and_overrides("d[Age]", "c[Age] / c[young]", vec![("old", "0")])
        .build_datamodel();
    let changes = crate::save_check::check_save(&built, crate::save_check::SaveFormat::Mdl)
        .expect("the check runs");
    let changes: Vec<(crate::save_check::ChangeKind, &str)> = changes
        .iter()
        .map(|change| (change.kind, change.reason.as_str()))
        .collect();
    assert_eq!(
        changes,
        [(
            crate::save_check::ChangeKind::Structure,
            "'d' is computed as 'c[age] / c[age]' for element 'young' in the save, not as \
             'c[age] / c[young]'"
        )]
    );
}

/// Nothing is read into a file that it does not say: each of these opens as
/// the reader wrote it, one equation per element. The rows are every way the
/// two rules can fail to hold -- the elements disagree (wholly, or all but
/// one, or by reading a neighbour each), an element has no equation, a
/// subscript names an element its axis lacks -- and the one form left alone
/// though the rules hold, a list of numbers.
#[test]
fn what_the_file_does_not_say_stays_per_element() {
    let rows = [
        // Each element says something else.
        "d[young] = 1 ~~|\nd[adult] = c[old] ~~|\nd[old] = 3 ~~|\n",
        // Most elements agree: no default is made of them, which would be an
        // equation for an element the dimension gains.
        "d[young] = 1 ~~|\nd[Older] = c[Older] * 2 ~~|\n",
        // Each reads the element before its own: no offset is made of them.
        "d[young] = 0 ~~|\nd[adult] = c[young] ~~|\nd[old] = c[adult] ~~|\n",
        // All but one read a fixed element beside their own.
        "d[young] = c[young] / c[young] ~~|\nd[adult] = c[adult] / c[young] ~~|\nd[old] = 0 ~~|\n",
        // Numbers are a list of their own, negative ones too.
        "d[young] = 1 ~~|\nd[Older] = 1 ~~|\n",
        "d[young] = -1 ~~|\nd[Older] = -1, 5 ~~|\n",
        // `Older` lacks `young`, so `x[young]` is no reading of `x[Age]`.
        "x[Older] = 5, 6 ~~|\nd[young] = c[young] ~~|\nd[Older] = x[Older] ~~|\n",
        // An element left out has no equation, which one equation over the
        // dimension would give it.
        "d[young] = c[young] ~~|\nd[adult] = c[adult] ~~|\n",
    ];
    for mdl in rows {
        let text = format!("{AGES}{mdl}{CONTROL}\\\\\\---///\n");
        let opened = crate::open_vensim(&text).expect("the model opens");
        assert!(
            matches!(equation(&opened, "d"), Equation::Arrayed(_, _, None, false)),
            "{mdl}: {:?}",
            equation(&opened, "d")
        );
        let written = crate::mdl::parse_mdl_per_element(&text).expect("the model opens");
        assert_eq!(opened, written, "{mdl}");
    }
}

/// The save check reads each element's equation at its element, so an
/// element written with its names in place is the equation every element
/// follows. Another element's name is left as written.
#[test]
fn an_element_equation_is_compared_at_its_element() {
    let project = TestProject::new("compare")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Age", &["young", "adult", "old"])
        .array_with_ranges(
            "c[Age]",
            vec![("young", "10"), ("adult", "20"), ("old", "30")],
        )
        .build_datamodel();
    let reader = crate::element_equations::ElementReader::new(&project, &project.models[0]);
    let dims = ["Age".to_string()];
    let rows = [
        ("c[adult] * 2", Some("c[Age] * 2")),
        ("c[adult] / c[young]", Some("c[Age] / c[young]")),
        ("c[young] * 2", None),
        ("c[Age] * 2", None),
    ];
    for (text, read) in rows {
        assert_eq!(reader.read(text, &dims, "adult").as_deref(), read, "{text}");
    }
}

/// Every MDL file of the corpus scores the same loops, with the same loop
/// scores, the same loops found by the model's analysis and the same LTM
/// diagnostics, opened with the compaction as opened with each variable left
/// as the reader writes it out: a variable kept as one equation is the same
/// model to loop analysis as it is to the run. Every file that runs under
/// loop analysis is compared, those the compaction leaves as they were
/// included, and the floors keep the comparison from passing on a corpus
/// that has lost its loops.
#[test]
#[ignore = "runs loop analysis on every MDL file of the corpus twice; run under the gates profile"]
fn the_corpus_scores_the_same_loops_compacted_or_not() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test");
    let mut files = Vec::new();
    let mut dirs = vec![root];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for path in entries.flatten().map(|e| e.path()) {
            if path.is_dir() {
                dirs.push(path);
            } else if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("mdl"))
            {
                files.push(path);
            }
        }
    }
    files.sort();
    // Files compared, those with a compacted variable, and the loops scored
    // or found in each set.
    let (mut compared, mut compacted, mut loops, mut compacted_loops) = (0, 0, 0, 0);
    let mut differ = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let (Ok(per_element), Ok(opened)) = (
            crate::mdl::parse_mdl_per_element(&text),
            crate::open_vensim(&text),
        ) else {
            continue;
        };
        let changed = per_element != opened;
        let (before, after) = (
            crate::ltm_loop_scores_test_support::loop_scores(&per_element),
            crate::ltm_loop_scores_test_support::loop_scores(&opened),
        );
        match (&before, &after) {
            (Ok(b), Ok(a)) => {
                compared += 1;
                let found = b.loops.len().max(b.discovered.len());
                loops += found;
                if changed {
                    compacted += 1;
                    compacted_loops += found;
                }
                if a != b {
                    differ.push(format!(
                        "{}: {} loops scored, {} compacted; {} loops found, {}; diagnostics {} then {}",
                        path.display(),
                        b.loops.len(),
                        a.loops.len(),
                        b.discovered.len(),
                        a.discovered.len(),
                        b.diagnostics.len(),
                        a.diagnostics.len()
                    ));
                }
            }
            (Err(b), Err(a)) if a == b => {}
            _ => differ.push(format!("{}: {before:?} then {after:?}", path.display())),
        }
    }
    println!(
        "{compared} files run with loop analysis, with {loops} loops; {compacted} of them hold a \
         compacted variable, with {compacted_loops} loops"
    );
    assert!(compared >= 190, "files compared: {compared}");
    assert!(loops >= 450, "loops compared: {loops}");
    assert!(
        compacted >= 12,
        "files with a compacted variable: {compacted}"
    );
    assert!(
        compacted_loops >= 150,
        "loops of files with a compacted variable: {compacted_loops}"
    );
    assert_eq!(differ, Vec::<String>::new());
}
