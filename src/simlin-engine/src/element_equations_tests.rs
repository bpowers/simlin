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
fn an_element_that_says_another_thing_keeps_its_own() {
    let project = open(&format!(
        "{AGES}d[young] = 1 ~~|\nd[Older] = c[Older] * 2 ~~|\n"
    ));
    let Equation::Arrayed(dims, arms, Some(default), true) = equation(&project, "d") else {
        panic!("a default and an exception: {:?}", equation(&project, "d"));
    };
    assert_eq!(dims, ["Age"]);
    assert_eq!(default, "c[Age] * 2");
    let arms: Vec<(&str, &str)> = arms.iter().map(|a| (a.0.as_str(), a.1.as_str())).collect();
    assert_eq!(arms, [("young", "1")]);
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

/// An element whose own equation is `NaN` keeps it, though the equation for
/// every other element reads no element there: whether that equation is NaN
/// there is not plain from the text. `IF THEN ELSE(switch, c[Age - 1], 5)`
/// is 5 at the first age when `switch` is 0, not NaN.
#[test]
fn an_element_whose_equation_is_nan_keeps_it() {
    let switch = "switch = 0 ~~|\n";
    let project = open(&format!(
        "{AGES}{switch}d[young] = NaN ~~|\nd[adult] = IF THEN ELSE(switch, c[young], 5) ~~|\n\
         d[old] = IF THEN ELSE(switch, c[adult], 5) ~~|\n"
    ));
    assert!(run(&project)["d[young]"].iter().all(|v| v.is_nan()));
    assert_eq!(run(&project)["d[old]"], vec![5.0, 5.0, 5.0]);

    let plain = open(&format!(
        "{AGES}d[young] = NaN ~~|\nd[adult] = c[young] ~~|\nd[old] = c[adult] ~~|\n"
    ));
    let Equation::Arrayed(dims, arms, Some(default), true) = equation(&plain, "d") else {
        panic!("a default and an exception: {:?}", equation(&plain, "d"));
    };
    assert_eq!(dims, ["Age"]);
    assert_eq!(default, "c[Age - 1]");
    let arms: Vec<(&str, &str)> = arms.iter().map(|a| (a.0.as_str(), a.1.as_str())).collect();
    assert_eq!(arms, [("young", "NaN")]);
    assert!(run(&plain)["d[young]"].iter().all(|v| v.is_nan()));
    assert_eq!(run(&plain)["d[old]"], vec![20.0, 20.0, 20.0]);
}

#[test]
fn what_cannot_be_read_as_one_equation_stays_per_element() {
    let rows = [
        // Each element says something else.
        "d[young] = 1 ~~|\nd[adult] = c[old] ~~|\nd[old] = 3 ~~|\n",
        // Numbers are a list of their own, negative ones too.
        "d[young] = 1 ~~|\nd[Older] = 1 ~~|\n",
        "d[young] = -1 ~~|\nd[Older] = -1, 5 ~~|\n",
        // `Older` lacks `young`, so `x[young]` is no reading of `x[Age]`.
        "x[Older] = 5, 6 ~~|\nd[young] = c[young] ~~|\nd[Older] = x[Older] ~~|\n",
        // An element left out has no equation, which a default would give it.
        "d[young] = c[young] ~~|\nd[adult] = c[adult] ~~|\n",
    ];
    for mdl in rows {
        let project = open(&format!("{AGES}{mdl}"));
        assert!(
            matches!(
                equation(&project, "d"),
                Equation::Arrayed(_, _, None, false)
            ),
            "{mdl}: {:?}",
            equation(&project, "d")
        );
    }
}

#[test]
fn an_element_a_fixed_number_of_places_along_reads_an_offset() {
    // Read as "this element", each older cohort reads the one before it: an
    // offset, with the youngest, which has none before it, as an exception.
    let project = open(&format!(
        "{AGES}d[young] = 0 ~~|\nd[adult] = c[young] ~~|\nd[old] = c[adult] ~~|\n"
    ));
    let Equation::Arrayed(_, arms, Some(default), true) = equation(&project, "d") else {
        panic!("a default and an exception: {:?}", equation(&project, "d"));
    };
    assert_eq!(default, "c[Age - 1]");
    let arms: Vec<(&str, &str)> = arms.iter().map(|a| (a.0.as_str(), a.1.as_str())).collect();
    assert_eq!(arms, [("young", "0")]);
    assert_eq!(run(&project)["d[old]"], vec![20.0, 20.0, 20.0]);
}

/// An equation for every element that names a fixed element of its own
/// dimension (`c[young]`, a base it normalizes by) is found in the copies,
/// though read at each element the copies differ (`c[Age - 1]` at `adult`):
/// the equation written out at each element is that element's copy. Saved as
/// MDL and opened, it is the same equation.
#[test]
fn an_equation_naming_a_fixed_element_opens_as_one_equation() {
    let project = open(&format!(
        "{AGES}d[young] = c[young] / c[young] ~~|\nd[adult] = c[adult] / c[young] ~~|\n\
         d[old] = 0 ~~|\n"
    ));
    let Equation::Arrayed(_, arms, Some(default), true) = equation(&project, "d") else {
        panic!("a default and an exception: {:?}", equation(&project, "d"));
    };
    assert_eq!(default, "c[Age] / c[young]");
    let arms: Vec<(&str, &str)> = arms.iter().map(|a| (a.0.as_str(), a.1.as_str())).collect();
    assert_eq!(arms, [("old", "0")]);

    let built = TestProject::new("fixed")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Age", &["young", "adult", "old"])
        .array_with_ranges(
            "c[Age]",
            vec![("young", "10"), ("adult", "20"), ("old", "30")],
        )
        .array_with_default_and_overrides("d[Age]", "c[Age] / c[young]", vec![("old", "0")])
        .build_datamodel();
    let save = crate::mdl::project_to_mdl(&built).expect("the model writes");
    let back = crate::open_vensim(&save).expect("the save opens");
    assert_eq!(equation(&back, "d"), equation(&built, "d"));
    assert!(
        crate::save_check::check_save(&built, crate::save_check::SaveFormat::Mdl)
            .unwrap()
            .is_empty()
    );
}

/// An element read in whole by an array-producing builtin, opened as an
/// offset, computes what the file's elements did.
#[test]
fn an_offset_opened_under_vector_elm_map_keeps_its_numbers() {
    let project = open(
        "T: t1, t2, t3 ~~|\narr[T] = 10, 20, 30 ~~|\noff[T] = 0, 1, 1 ~~|\nv[t1] = 0 ~~|\n\
         v[t2] = VECTOR ELM MAP(arr[t1], off[t2]) ~~|\n\
         v[t3] = VECTOR ELM MAP(arr[t2], off[t3]) ~~|\n",
    );
    let results = run(&project);
    assert_eq!(results["v[t2]"][0], 20.0);
    assert_eq!(results["v[t3]"][0], 30.0);
}

/// The save check reads each element's equation at its element, so an
/// element written with its names in place is the equation every element
/// follows.
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
    assert_eq!(
        reader.read("c[adult] * 2", &dims, "adult").as_deref(),
        Some("c[Age] * 2")
    );
    assert_eq!(
        reader.read("c[young] * 2", &dims, "adult").as_deref(),
        Some("c[Age - 1] * 2")
    );
    assert_eq!(reader.read("c[Age] * 2", &dims, "adult"), None);
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
        compacted >= 15,
        "files with a compacted variable: {compacted}"
    );
    assert!(
        compacted_loops >= 150,
        "loops of files with a compacted variable: {compacted_loops}"
    );
    assert_eq!(differ, Vec::<String>::new());
}
