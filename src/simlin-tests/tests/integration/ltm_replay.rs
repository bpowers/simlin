// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Every link score of every model in the corpus, against the score the
//! model's own code gives when it is run again with that one input changed
//! (`simlin_engine::ltm_replay`).
//!
//! The scores the engine generates and compiles beside a model are its
//! equations rewritten; the replayed ones are the program the model already
//! is. They share nothing past the compiler, so a generated score that is off
//! shows here whether it is off by a number, zero, or of a link the model does
//! not have. [`KNOWN`] is every way in which one is. A row that goes is a score
//! put right, and is deleted; a row that appears is a regression.

use std::collections::BTreeMap;
use std::io::BufReader;

use simlin_base::datamodel::Project;
use simlin_engine::ltm_replay_compare::{Comparison, Why, compare};

use crate::test_helpers::corpus;

/// What is known to be off in a model: how many of its links differ for each
/// reason, and how many only one of the two scores.
struct Known {
    path: &'static str,
    differ: &'static [(Why, usize)],
    not_read: usize,
    not_slots: usize,
    not_scored: usize,
}

/// A dimension's name read as the element's number, in an equation of a scalar
/// source (`flow[sub] = v * sub`): the generated equation does not compile, a
/// warning says so, and the score is 0.
const DIMENSION_AS_NUMBER: &[(Why, usize)] = &[(Why::Zero, 2)];

const KNOWN: &[Known] = &[
    row(
        "test/bobby/vdf/model_editing/10_reformat.mdl",
        DIMENSION_AS_NUMBER,
    ),
    row(
        "test/bobby/vdf/model_editing/6_make_arrayed.mdl",
        DIMENSION_AS_NUMBER,
    ),
    row(
        "test/bobby/vdf/model_editing/7_add_new_subscript.mdl",
        DIMENSION_AS_NUMBER,
    ),
    row(
        "test/bobby/vdf/model_editing/8_change_subscript.mdl",
        DIMENSION_AS_NUMBER,
    ),
    row(
        "test/bobby/vdf/model_editing/9_smooth_time.mdl",
        DIMENSION_AS_NUMBER,
    ),
    row("test/previous/model.stmx", &[(Why::LiveLags, 1)]),
    // A composite is the strongest of the ways to every output any instance of
    // the module is read through, and one of these flows is read through fewer.
    row(
        "test/systems-format/cross_stock.txt",
        &[(Why::OtherPathway, 1)],
    ),
    row(
        "test/systems-format/extended_syntax.txt",
        &[(Why::OtherPathway, 3), (Why::Unexplained, 1)],
    ),
    // A flow's equation that reads its source through a mapping.
    Known {
        not_scored: 9,
        ..row(
            "test/test-models/tests/subscript_mapping_simple/test_subscript_mapping_simple.mdl",
            &[],
        )
    },
    // Array builtins over snapshots.
    Known {
        not_scored: 11,
        ..row(
            "test/vector_snapshot_operand/vector_snapshot_operand.xmile",
            &[(Why::LiveLags, 9), (Why::WholeChange, 2)],
        )
    },
    // The 44 are the elements of `SUM(RS HFC[COP,HFC type!] * GWP of HFC[HFC
    // type!])`, each credited with the whole change of a sum of many terms; the
    // 15 are read by a `STEP`, whose clock the generated equation leaves at the
    // new step when the call reads the source; the 107 are read by a `VECTOR
    // SELECT` or a `VECTOR ELM MAP`, whose links the engine declines to score,
    // with a warning.
    Known {
        not_scored: 107,
        ..row(
            "test/xmutil_test_models/C-LEARN v77 for Vensim.mdl",
            &[
                (Why::Rounding, 8),
                (Why::LiveClock, 15),
                (Why::WholeChange, 44),
            ],
        )
    },
];

/// A model with links that differ and none only one of the two scores.
const fn row(path: &'static str, differ: &'static [(Why, usize)]) -> Known {
    Known {
        path,
        differ,
        not_read: 0,
        not_slots: 0,
        not_scored: 0,
    }
}

fn open(path: &str) -> Option<Project> {
    let bytes = std::fs::read(format!("../../{path}")).ok()?;
    let text = || std::str::from_utf8(&bytes).ok();
    let is = |extension: &str| path.to_ascii_lowercase().ends_with(extension);
    if is(".mdl") {
        simlin_formats::open_vensim(text()?).ok()
    } else if is(".txt") {
        simlin_formats::open_systems(text()?).ok()
    } else {
        simlin_formats::open_xmile(&mut BufReader::new(bytes.as_slice())).ok()
    }
}

/// What a comparison found, as a row of [`KNOWN`] says it.
type Found = (BTreeMap<Why, usize>, usize, usize, usize);

fn found(comparison: &Comparison) -> Found {
    (
        comparison.counts(),
        comparison.not_read.len(),
        comparison.not_slots.len(),
        comparison.not_scored.len(),
    )
}

fn known(path: &str) -> Found {
    KNOWN
        .iter()
        .find(|known| known.path == path)
        .map(|known| {
            (
                known.differ.iter().copied().collect(),
                known.not_read,
                known.not_slots,
                known.not_scored,
            )
        })
        .unwrap_or_default()
}

/// Compare each model, and what is found with what is known. Returns how many
/// were compared and how many active scores agree.
fn check(paths: &[String]) -> (usize, usize) {
    use rayon::prelude::*;

    let compared: Vec<(&str, Comparison)> = paths
        .par_iter()
        .filter_map(|path| {
            let project = open(path)?;
            // A model that does not compile, or whose stocks the overlay does
            // not take (a conveyor, a queue), has no scores.
            let comparison = compare(&project).ok()?;
            Some((path.as_str(), comparison))
        })
        .collect();
    let mut off: Vec<String> = Vec::new();
    for (path, comparison) in &compared {
        if !comparison.misreproduced.is_empty() {
            off.push(format!("{path}: {:#?}", comparison.misreproduced));
        }
        let (found, known) = (found(comparison), known(path));
        if found != known {
            let first: Vec<_> = comparison.differences.iter().take(3).collect();
            off.push(format!(
                "{path}: (differ, not read, not slots, not scored) is {found:?}, \
                 known as {known:?}; the first that differ: {first:#?}"
            ));
        }
    }
    assert!(off.is_empty(), "{}", off.join("\n"));
    let active = compared.iter().map(|(_, c)| c.agreed_active).sum();
    (compared.len(), active)
}

#[test]
fn small_models_scores_are_the_replayed_ones() {
    let paths = [
        "test/arms_race_3party/arms_race.stmx",
        "test/arrayed_population_ltm/arrayed_population.stmx",
        "test/cross_agg_ltm/cross_agg.stmx",
        "test/cross_element_ltm/cross_element.stmx",
        "test/logistic_growth_ltm/logistic_growth.stmx",
        "test/metasd/WRLD3-03/wrld3-03.mdl",
        "test/systems-format/hiring.txt",
    ]
    .map(str::to_string);
    let (compared, active) = check(&paths);
    assert_eq!(compared, paths.len());
    assert!(active >= 400, "{active} active scores");
}

#[test]
#[ignore = "every corpus model run with and without its scores; run under the gates profile"]
fn the_corpus_scores_are_the_replayed_ones() {
    let mut paths = corpus();
    for entry in std::fs::read_dir("../../test/systems-format").expect("the systems models") {
        let path = entry.expect("an entry").path();
        if path.extension().is_some_and(|e| e == "txt") {
            let name = path.file_name().expect("a file").to_string_lossy();
            paths.push(format!("test/systems-format/{name}"));
        }
    }
    let (compared, active) = check(&paths);
    // Fewer means the corpus walk or the comparison broke, which would
    // otherwise pass with nothing to say.
    assert!(compared >= 400, "{compared} models");
    assert!(active >= 5_700, "{active} active scores");
    for known in KNOWN {
        assert!(
            paths.iter().any(|path| path == known.path),
            "{}",
            known.path
        );
    }
}
