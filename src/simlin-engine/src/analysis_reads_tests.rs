// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `model_reads`: what each variable reads, with the reads made only when the
//! model starts told apart from the links that move a variable step by step.

use std::collections::BTreeSet;

use super::*;
use crate::datamodel::{self, Variable};
use crate::db::DepPhase;
use crate::ltm::LinkPolarity;
use crate::test_common::TestProject;
use crate::variable::DepLag;

/// The reads of `project`'s first model.
fn reads_of(project: &datamodel::Project) -> Vec<ModelRead> {
    let db = SimlinDb::default();
    let source_project = crate::db::sync_from_datamodel(&db, project).project;
    let name = crate::canonicalize(&project.models[0].name).into_owned();
    let model = source_project.models(&db)[name.as_str()];
    model_reads(&db, model, source_project)
}

/// The causal links of `project`'s first model, as `(from, to)`.
fn links_of(project: &datamodel::Project) -> BTreeSet<(String, String)> {
    let db = SimlinDb::default();
    let source_project = crate::db::sync_from_datamodel(&db, project).project;
    let name = crate::canonicalize(&project.models[0].name).into_owned();
    let model = source_project.models(&db)[name.as_str()];
    model_links(&db, model, source_project, None, false)
        .into_iter()
        .map(|link| (link.from, link.to))
        .collect()
}

/// The read of `from` by `to`, which the model must have.
fn read<'a>(reads: &'a [ModelRead], from: &str, to: &str) -> &'a ModelRead {
    reads
        .iter()
        .find(|r| r.from == from && r.to == to)
        .unwrap_or_else(|| panic!("{to} reads {from}: {reads:?}"))
}

fn pairs(reads: &[ModelRead]) -> Vec<(&str, &str, bool)> {
    reads
        .iter()
        .map(|r| (r.from.as_str(), r.to.as_str(), r.start_only))
        .collect()
}

/// Whether a read is made only at the start is decided by its phase and lag,
/// a row for each: the match names every phase and every lag, so a new one
/// does not compile until it has a row.
#[test]
fn a_read_is_at_the_start_in_the_initial_phase_or_of_the_initial_snapshot() {
    for phase in [DepPhase::Dt, DepPhase::Init] {
        for lag in [DepLag::Current, DepLag::Previous, DepLag::Initial] {
            let expected = match (phase, lag) {
                (DepPhase::Init, DepLag::Current) => true,
                (DepPhase::Init, DepLag::Previous) => true,
                (DepPhase::Init, DepLag::Initial) => true,
                (DepPhase::Dt, DepLag::Initial) => true,
                (DepPhase::Dt, DepLag::Current) => false,
                (DepPhase::Dt, DepLag::Previous) => false,
            };
            assert_eq!(read_at_start(phase, lag), expected, "{phase:?} {lag:?}");
        }
    }
}

#[test]
fn a_stock_reads_its_flows_and_its_initial_value_only_at_the_start() {
    let project = TestProject::new("growth")
        .stock("level", "s0 * scale", &["growth"], &["decay"], None)
        .flow("growth", "level * rate", None)
        .flow("decay", "level / lifetime", None)
        .aux("rate", "0.05", None)
        .aux("lifetime", "20", None)
        .aux("s0", "100", None)
        .aux("scale", "2", None)
        .build_datamodel();
    let reads = reads_of(&project);
    assert_eq!(
        pairs(&reads),
        [
            ("level", "decay", false),
            ("lifetime", "decay", false),
            ("level", "growth", false),
            ("rate", "growth", false),
            ("decay", "level", false),
            ("growth", "level", false),
            ("s0", "level", true),
            ("scale", "level", true),
        ],
        "sorted by reader, then by what is read"
    );
    assert_eq!(
        read(&reads, "growth", "level").polarity,
        LinkPolarity::Positive
    );
    assert_eq!(
        read(&reads, "decay", "level").polarity,
        LinkPolarity::Negative
    );
    assert_eq!(
        read(&reads, "lifetime", "decay").polarity,
        LinkPolarity::Negative
    );
    assert_eq!(
        read(&reads, "s0", "level").polarity,
        LinkPolarity::Unknown,
        "a read at the start is no causal link, and has no polarity"
    );
}

/// One row per way an equation reads a name only at the start, and per way
/// that looks like one and is not.
#[test]
fn an_init_argument_is_read_at_the_start_and_a_previous_argument_every_step() {
    for (equation, start_only) in [
        ("INIT(source)", true),
        ("INIT(source * 2)", true),
        ("source + INIT(source)", false),
        ("PREVIOUS(source)", false),
        ("PREVIOUS(source * 2)", false),
        ("PREVIOUS(source, INIT(source))", false),
        ("source * 2", false),
    ] {
        let project = TestProject::new("lags")
            .aux("reader", equation, None)
            .aux("source", "TIME", None)
            .build_datamodel();
        let reads = reads_of(&project);
        assert_eq!(
            pairs(&reads),
            [("source", "reader", start_only)],
            "{equation}: a helper the parse makes is no variable, and what it reads the reader reads"
        );
        // The causal graph links a bare `INIT(source)`, and a read at the
        // start still has no polarity.
        if start_only {
            assert_eq!(reads[0].polarity, LinkPolarity::Unknown, "{equation}");
        }
    }
}

#[test]
fn an_initial_only_equation_reads_at_the_start() {
    let mut project = TestProject::new("active_initial")
        .aux("reader", "running_input * 2", None)
        .aux("running_input", "TIME", None)
        .aux("starting_input", "3", None)
        .build_datamodel();
    let Some(Variable::Aux(reader)) = project.models[0].get_variable_mut("reader") else {
        panic!("reader is an auxiliary");
    };
    reader.compat.active_initial = Some("starting_input".to_string());
    assert_eq!(
        pairs(&reads_of(&project)),
        [
            ("running_input", "reader", false),
            ("starting_input", "reader", true),
        ]
    );
}

#[test]
fn what_a_smooth_reads_its_variable_reads() {
    let project = TestProject::new("smooth")
        .aux("smoothed", "SMTH1(input * gain, delay_time)", None)
        .aux("input", "TIME", None)
        .aux("gain", "2", None)
        .aux("delay_time", "4", None)
        .build_datamodel();
    assert_eq!(
        pairs(&reads_of(&project)),
        [
            ("delay_time", "smoothed", false),
            ("gain", "smoothed", false),
            ("input", "smoothed", false),
        ]
    );
}

/// A smooth's initial-value argument sets where the smooth starts and moves
/// nothing after, so it is read at the start, bare or hoisted into a helper:
/// the causal edges leave it out for the same reason
/// (`db::model_causal_edges`'s `start_only_inputs`).
#[test]
fn a_smooths_initial_value_argument_is_read_at_the_start() {
    let project = TestProject::new("smooth")
        .aux("bare", "SMTH1(input, delay_time, start)", None)
        .aux("hoisted", "SMTH1(input, delay_time, start * 2)", None)
        .aux("input", "TIME", None)
        .aux("delay_time", "4", None)
        .aux("start", "1", None)
        .build_datamodel();
    assert_eq!(
        pairs(&reads_of(&project)),
        [
            ("delay_time", "bare", false),
            ("input", "bare", false),
            ("start", "bare", true),
            ("delay_time", "hoisted", false),
            ("input", "hoisted", false),
            ("start", "hoisted", true),
        ]
    );
}

#[test]
fn a_table_is_read_by_the_variable_that_looks_it_up() {
    let gf = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 1.0]),
        y_points: vec![0.0, 2.0],
        x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
    };
    let project = TestProject::new("table")
        .aux_with_gf("effect_table", "", gf)
        .aux("effect", "LOOKUP(effect_table, pressure)", None)
        .aux("pressure", "TIME / 10", None)
        .build_datamodel();
    let reads = reads_of(&project);
    assert_eq!(
        pairs(&reads),
        [
            ("effect_table", "effect", false),
            ("pressure", "effect", false),
        ]
    );
    let table = read(&reads, "effect_table", "effect");
    assert!(table.table && !read(&reads, "pressure", "effect").table);
    assert_eq!(
        table.polarity,
        LinkPolarity::Unknown,
        "a table is no causal link"
    );
}

/// A table looked up inside a helper (`INIT`'s captured argument, a
/// `SMTH1`'s input) is read by the variable the helper was made for, at the
/// start when the helper is; and a table also read as a value (a variable
/// whose value is its table at its own equation) is a read of a value, not
/// only of a table.
#[test]
fn a_table_a_helper_looks_up_is_read_and_one_read_as_a_value_too_is_no_table_only() {
    let gf = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 1.0]),
        y_points: vec![0.0, 2.0],
        x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 2.0 },
    };
    for (equation, table, start_only, only_table) in [
        ("INIT(LOOKUP(t, x))", "t", true, true),
        ("SMTH1(LOOKUP(t, x), 2)", "t", false, true),
        ("LOOKUP(g, x) + g", "g", false, false),
    ] {
        let project = TestProject::new("tables")
            .aux("reader", equation, None)
            .aux_with_gf("g", "TIME", gf.clone())
            .aux_with_gf("t", "", gf.clone())
            .aux("x", "TIME", None)
            .build_datamodel();
        let reads = reads_of(&project);
        let made = read(&reads, table, "reader");
        assert_eq!(
            (made.start_only, made.table),
            (start_only, only_table),
            "{equation}: {:?}",
            pairs(&reads)
        );
    }
}

/// A module instance never reads itself: a read of its own output through
/// an instance (a Stella import wires those as inputs) binds nothing, as in
/// the causal graph. Corpus models with such wiring, checked whole.
#[test]
fn a_module_instance_never_reads_itself() {
    for path in [
        "test-models/samples/bpowers-hares_and_lynxes_modules/model.xmile",
        "modules_with_complex_idents/modules_with_complex_idents.stmx",
    ] {
        let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test")
            .join(path);
        let bytes = std::fs::read(&full).unwrap_or_else(|err| panic!("{path}: {err}"));
        let project = crate::compat::open_xmile(&mut std::io::BufReader::new(&bytes[..]))
            .unwrap_or_else(|err| panic!("{path}: {err:?}"));
        let reads = reads_of(&project);
        assert!(!reads.is_empty(), "{path}");
        let own: Vec<&ModelRead> = reads.iter().filter(|r| r.from == r.to).collect();
        assert!(own.is_empty(), "{path}: {own:?}");
    }
}

/// A name the model does not declare is the reader's error, not a read: in an
/// equation, inside a helper's argument, as a table called, and as a flow a
/// stock lists. What the same variables read of the model is still read.
#[test]
fn a_name_no_variable_has_is_no_read() {
    let project = TestProject::new("misspelled")
        .stock("level", "1", &["growth", "gone_flow"], &[], None)
        .flow("growth", "level * rate + no_such_rate", None)
        .aux(
            "rate",
            "SMTH1(missing_input + 1, 2) + LOOKUP(no_table, rate_input)",
            None,
        )
        .aux("rate_input", "TIME", None)
        .build_datamodel();
    assert_eq!(
        pairs(&reads_of(&project)),
        [
            ("level", "growth", false),
            ("rate", "growth", false),
            ("growth", "level", false),
            ("rate_input", "rate", false),
        ]
    );
}

/// The two owners agree on what they share: every causal link is a read, and
/// every read made past the start is a causal link or a read of a table. A
/// read in a stock's or a flow's option, or in the text of an equation the
/// compiler cannot read, is in no causal graph of the model as written (the
/// special-stock build reads the one, nothing reads the other), and is left
/// out of the comparison.
///
/// The causal graph also links a bare `INIT(x)` argument to its reader (a
/// read of the frozen snapshot, which `model_causal_edges` records as it
/// records any per-step read), so a causal link may be a read made only at
/// the start; no other read at the start is one. It also links a name no
/// variable has, which is no read.
fn assert_reads_agree_with_the_causal_links(label: &str, project: &datamodel::Project) {
    let model = &project.models[0];
    let links: BTreeSet<(String, String)> = links_of(project)
        .into_iter()
        .filter(|(from, _)| model.get_variable(from).is_some())
        .collect();
    let reads = reads_of(project);
    let all: BTreeSet<(String, String)> = reads
        .iter()
        .map(|r| (r.from.clone(), r.to.clone()))
        .collect();
    let unread: Vec<_> = links.difference(&all).collect();
    let unlinked: Vec<_> = reads
        .iter()
        .filter(|r| !r.start_only && !r.table && r.option.is_none() && !r.unchecked)
        .map(|r| (r.from.clone(), r.to.clone()))
        .filter(|pair| !links.contains(pair))
        .collect();
    assert!(
        unread.is_empty() && unlinked.is_empty(),
        "{label}: causal links that are no read: {unread:?}; reads past the start that are no causal link: {unlinked:?}"
    );
}

#[test]
fn every_causal_link_is_a_read_and_every_read_past_the_start_a_link() {
    let project = TestProject::new("mixed")
        .stock("level", "s0", &["growth"], &[], None)
        .flow("growth", "level * rate * SMTH1(pressure, delay_time)", None)
        .aux("rate", "INIT(base_rate * 2)", None)
        .aux("base_rate", "0.05", None)
        .aux("pressure", "PREVIOUS(level) / s0", None)
        .aux("delay_time", "3", None)
        .aux("s0", "100", None)
        .aux("first_level", "INIT(level)", None)
        .aux(
            "perceived",
            "SMTH1(level, delay_time, first_level * 2)",
            None,
        )
        .build_datamodel();
    assert_reads_agree_with_the_causal_links("mixed", &project);
}

#[test]
#[ignore = "every corpus model's reads against its causal links; run under the gates profile"]
fn every_corpus_models_reads_agree_with_its_causal_links() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test");
    let mut dirs = vec![root];
    let mut swept = 0;
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_lowercase);
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let project = std::panic::catch_unwind(|| match extension.as_deref() {
                Some("mdl") => crate::compat::open_vensim(&String::from_utf8_lossy(&bytes)).ok(),
                Some("xmile" | "stmx" | "itmx") => {
                    crate::compat::open_xmile(&mut std::io::BufReader::new(&bytes[..])).ok()
                }
                _ => None,
            })
            .ok()
            .flatten();
            let Some(project) = project.filter(|p| !p.models.is_empty()) else {
                continue;
            };
            swept += 1;
            assert_reads_agree_with_the_causal_links(&path.display().to_string(), &project);
        }
    }
    assert!(swept > 400, "the corpus is present: {swept} models");
}

/// The aux an option of the fixture below names, by the option: a row per
/// option, by a match a new option breaks.
fn named_by(option: datamodel::StockOption) -> &'static str {
    use datamodel::StockOption;
    match option {
        StockOption::TransitTime => "opt_len",
        StockOption::Capacity => "opt_capacity",
        StockOption::InflowLimit => "opt_in_limit",
        StockOption::Sample => "opt_sample",
        StockOption::Arrest => "opt_arrest",
        StockOption::LeakFraction => "opt_fraction",
        StockOption::LeakZoneStart => "opt_zone_start",
        StockOption::LeakZoneEnd => "opt_zone_end",
    }
}

/// A conveyor stock reads what its options name, and a leak flow what its
/// leak options name: the special-stock build evaluates them outside the
/// variable's equation, so the dependency set has none of them, and a read
/// record says which option each is.
#[test]
fn a_stock_and_a_flow_read_what_their_options_name() {
    use datamodel::StockOption;
    let mut project = TestProject::new("options")
        .with_sim_time(0.0, 4.0, 1.0)
        .stock("belt", "0", &["arriving"], &["leaking"], None)
        .flow("arriving", "1", None)
        .flow("leaking", "0", None);
    for option in StockOption::ALL {
        project = project.aux(named_by(option), "1", None);
    }
    let mut project = project.build_datamodel();
    let model = &mut project.models[0];
    if let Some(Variable::Stock(belt)) = model.get_variable_mut("belt") {
        belt.compat.conveyor = Some(datamodel::Conveyor {
            transit_time: named_by(StockOption::TransitTime).to_string(),
            capacity: Some(named_by(StockOption::Capacity).to_string()),
            inflow_limit: Some(named_by(StockOption::InflowLimit).to_string()),
            sample: Some(named_by(StockOption::Sample).to_string()),
            arrest: Some(format!("{} > 0", named_by(StockOption::Arrest))),
            discrete: false,
            batch_integrity: false,
            one_at_a_time: true,
            exponential_leak: false,
            ignore_earlier_zone_losses: false,
        });
    }
    if let Some(Variable::Flow(leaking)) = model.get_variable_mut("leaking") {
        leaking.compat.leakage = Some(datamodel::Leakage {
            fraction: Some(named_by(StockOption::LeakFraction).to_string()),
            integers: false,
            zone_start: Some(named_by(StockOption::LeakZoneStart).to_string()),
            zone_end: Some(named_by(StockOption::LeakZoneEnd).to_string()),
        });
    }
    let reads = reads_of(&project);
    for option in StockOption::ALL {
        let owner = match option {
            StockOption::TransitTime
            | StockOption::Capacity
            | StockOption::InflowLimit
            | StockOption::Sample
            | StockOption::Arrest => "belt",
            StockOption::LeakFraction | StockOption::LeakZoneStart | StockOption::LeakZoneEnd => {
                "leaking"
            }
        };
        let found = read(&reads, named_by(option), owner);
        assert_eq!(found.option, Some(option), "{found:?}");
        assert!(!found.unchecked && !found.start_only, "{found:?}");
    }
}

/// The corpus's conveyors read their transit times: `transit` is the belt's
/// in the arrayed conveyor, and each stage's duration its stock's in the
/// COVID model.
#[test]
fn a_corpus_conveyor_reads_its_transit_time() {
    let open = |path: &str| {
        let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test/conveyors")
            .join(path);
        let file = std::fs::File::open(&full).expect("the model is in the corpus");
        crate::compat::open_xmile(&mut std::io::BufReader::new(file)).unwrap()
    };
    let reads = reads_of(&open("arrayed_conveyor.xmile"));
    assert_eq!(
        read(&reads, "transit", "belt").option,
        Some(datamodel::StockOption::TransitTime)
    );
    let covid = open("covid19_severity.stmx");
    let reads = reads_of(&covid);
    let mut stocks = 0;
    for var in covid.models[0].variables.iter() {
        let Variable::Stock(stock) = var else {
            continue;
        };
        let Some(conveyor) = &stock.compat.conveyor else {
            continue;
        };
        stocks += 1;
        let owner = crate::canonicalize(&stock.ident).into_owned();
        let len = crate::canonicalize(&conveyor.transit_time).into_owned();
        assert!(read(&reads, &len, &owner).option.is_some());
    }
    assert!(stocks > 0, "the premise: the model has conveyors");
}

/// An equation the compiler cannot read (here an unknown function) has no
/// dependency set, and still reads what its text writes: those reads are
/// marked unchecked. An equation that does not parse reads what no one can
/// know, and is listed as such; a model of readable equations lists none.
#[test]
fn an_equation_the_compiler_cannot_read_still_reads_what_it_writes() {
    let project = TestProject::new("broken")
        .with_sim_time(0.0, 4.0, 1.0)
        .aux("rate", "0.1", None)
        .aux("start", "5", None)
        .aux("step", "NO_SUCH_FUNCTION(rate, start)", None)
        .aux("fine", "rate * 2", None)
        .build_datamodel();
    let reads = reads_of(&project);
    for from in ["rate", "start"] {
        assert!(read(&reads, from, "step").unchecked, "{reads:?}");
    }
    assert!(!read(&reads, "rate", "fine").unchecked);

    let db = SimlinDb::default();
    let unread = |project: &datamodel::Project| {
        let source = crate::db::sync_from_datamodel(&db, project).project;
        let model = source.models(&db)["main"];
        model_unread_equations(&db, model)
    };
    assert_eq!(unread(&project), Vec::<String>::new());
    let garbled = TestProject::new("garbled")
        .with_sim_time(0.0, 4.0, 1.0)
        .aux("rate", "0.1", None)
        .aux("half", "rate +", None)
        .build_datamodel();
    assert_eq!(unread(&garbled), ["half"]);
}

/// How a text writes a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Written {
    /// A bare word.
    Bare,
    /// A bare word before `(`: a function, or a table called by its name.
    Called,
    /// In double quotes, which only a variable's name is.
    Quoted,
}

/// The names `text` writes, canonically, by a reading of its characters that
/// shares nothing with the parser: bare words and quoted names, comments
/// skipped, each with how it is written ([`Written`]). Independent of the dependency query
/// `model_reads` and `model_links` both read, so what both miss shows here.
fn names_written(text: &str) -> BTreeSet<(String, Written)> {
    let chars: Vec<char> = text.chars().collect();
    let mut names = BTreeSet::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            let start = i + 1;
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                i += 1;
            }
            let name: String = chars[start..i.min(chars.len())].iter().collect();
            names.insert((crate::canonicalize(&name).into_owned(), Written::Quoted));
            i += 1;
        } else if c == '{' {
            while i < chars.len() && chars[i] != '}' {
                i += 1;
            }
            i += 1;
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '.' | '$'))
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let called = chars[i..]
                .iter()
                .find(|c| !c.is_whitespace())
                .is_some_and(|&c| c == '(');
            let written = if called {
                Written::Called
            } else {
                Written::Bare
            };
            names.insert((crate::canonicalize(&word).into_owned(), written));
        } else {
            i += 1;
        }
    }
    names
}

/// The reads `model`'s texts write, by [`names_written`]: each name a
/// variable of the model has (a module-qualified one as its instance), a
/// stock's flows, and a module's wired sources. A name that is also a
/// dimension's or an element's may be a subscript, and is left out; a bare
/// name the engine reads as a builtin (`time_step`, `dt`, `pi`) is no
/// variable, whatever the model declares, and the same name quoted is the
/// variable (how the MDL reader writes a reference to a declared `pi` or
/// `dt`); and a called name is a read only of a
/// variable with a table (`MOD(a, b)` calls a function beside a variable
/// `mod`).
fn reads_written(
    project: &datamodel::Project,
    model: &datamodel::Model,
) -> BTreeSet<(String, String)> {
    let declared: BTreeSet<String> = model
        .variables
        .iter()
        .map(|v| crate::canonicalize(v.get_ident()).into_owned())
        .collect();
    let subscripts: BTreeSet<String> = project
        .dimensions
        .iter()
        .flat_map(|d| {
            let mut names = vec![crate::canonicalize(&d.name).into_owned()];
            if let datamodel::DimensionElements::Named(elements) = &d.elements {
                names.extend(elements.iter().map(|e| crate::canonicalize(e).into_owned()));
            }
            names
        })
        .collect();
    let tables: BTreeSet<String> = model
        .variables
        .iter()
        .filter(|v| match v {
            Variable::Aux(aux) => aux.gf.is_some(),
            Variable::Flow(flow) => flow.gf.is_some(),
            _ => false,
        })
        .map(|v| crate::canonicalize(v.get_ident()).into_owned())
        .collect();
    let mut reads = BTreeSet::new();
    for var in &model.variables {
        let me = crate::canonicalize(var.get_ident()).into_owned();
        let mut written: BTreeSet<String> = var
            .expression_texts()
            .into_iter()
            .flat_map(|(_, text)| names_written(text))
            .filter(|(name, written)| match written {
                Written::Quoted => true,
                Written::Bare => !crate::builtins::is_0_arity_builtin_fn(name),
                Written::Called => {
                    !crate::builtins::is_0_arity_builtin_fn(name) && tables.contains(name)
                }
            })
            .map(|(name, _)| name)
            .collect();
        match var {
            Variable::Stock(stock) => written.extend(
                stock
                    .inflows
                    .iter()
                    .chain(&stock.outflows)
                    .map(|f| crate::canonicalize(f).into_owned()),
            ),
            // A module wired from its own output binds nothing (a Stella
            // import wires those as inputs), so that wire is no read.
            Variable::Module(module) => written.extend(
                module
                    .references
                    .iter()
                    .map(|r| {
                        crate::canonicalize(&r.src)
                            .trim_start_matches('\u{00B7}')
                            .to_string()
                    })
                    .filter(|src| src.split('\u{00B7}').next() != Some(me.as_str())),
            ),
            _ => {}
        }
        for name in written {
            // `SELF` names the variable whose equation it is.
            let name = if name == "self" { me.clone() } else { name };
            let head = name
                .split(['.', '\u{00B7}'])
                .next()
                .unwrap_or(&name)
                .to_string();
            let read = if declared.contains(&name) { name } else { head };
            if declared.contains(&read) && !subscripts.contains(&read) {
                reads.insert((read, me.clone()));
            }
        }
    }
    reads
}

/// `model_reads` of `model` against what its texts write: the pairs each
/// has that the other lacks.
fn reads_against_text(
    project: &datamodel::Project,
    model_name: &str,
) -> (Vec<String>, Vec<String>) {
    let model = project.get_model(model_name).unwrap();
    let mut db = crate::db::SimlinDb::default();
    db.sync(project);
    let source_project = db.current_source_project().unwrap();
    let canonical = crate::canonicalize(model_name);
    let source_model = *source_project.models(&db).get(canonical.as_ref()).unwrap();
    // A name a dimension or an element also has may be a subscript in the
    // text, so neither side counts it.
    let subscripts: BTreeSet<String> = project
        .dimensions
        .iter()
        .flat_map(|d| {
            let mut names = vec![crate::canonicalize(&d.name).into_owned()];
            if let datamodel::DimensionElements::Named(elements) = &d.elements {
                names.extend(elements.iter().map(|e| crate::canonicalize(e).into_owned()));
            }
            names
        })
        .collect();
    let by_engine: BTreeSet<(String, String)> = model_reads(&db, source_model, source_project)
        .into_iter()
        .filter(|r| !subscripts.contains(&r.from))
        .map(|r| (r.from, r.to))
        .collect();
    // An equation that does not parse reads what no one can know
    // (`model_unread_equations`), so its reader is no part of the comparison.
    let unread: BTreeSet<String> = model_unread_equations(&db, source_model)
        .into_iter()
        .collect();
    let by_engine: BTreeSet<(String, String)> = by_engine
        .into_iter()
        .filter(|(_, to)| !unread.contains(to))
        .collect();
    let by_text: BTreeSet<(String, String)> = reads_written(project, model)
        .into_iter()
        .filter(|(_, to)| !unread.contains(to))
        .collect();
    let show = |pairs: BTreeSet<&(String, String)>| -> Vec<String> {
        pairs
            .into_iter()
            .map(|(from, to)| format!("{to} reads {from}"))
            .collect()
    };
    (
        show(by_text.difference(&by_engine).collect()),
        show(by_engine.difference(&by_text).collect()),
    )
}

/// A corpus model's reads, every one of its models: what its texts write,
/// and nothing they do not.
fn assert_reads_match_the_text(path: &str) {
    let full = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../")
        .join(path);
    let bytes = std::fs::read(&full).unwrap_or_else(|err| panic!("{path}: {err}"));
    let project = if path.to_lowercase().ends_with(".mdl") {
        crate::compat::open_vensim(&String::from_utf8_lossy(&bytes)).unwrap()
    } else {
        crate::compat::open_xmile(&mut std::io::BufReader::new(&bytes[..])).unwrap()
    };
    for model in project.models.iter().filter(|m| m.macro_spec.is_none()) {
        let (missed, invented) = reads_against_text(&project, &model.name);
        assert!(
            missed.is_empty() && invented.is_empty(),
            "{path} ({}): missed {missed:?}, invented {invented:?}",
            model.name
        );
    }
}

/// Models with modules, arrays, tables, helpers and special stocks: reads
/// agree with the text, which a gate comparing two views of one dependency
/// query cannot show (a conveyor's options, an equation that does not
/// compile).
#[test]
fn reads_match_what_the_text_writes() {
    for path in [
        "test/test-models/samples/teacup/teacup.xmile",
        "test/modules_hares_and_foxes/modules_hares_and_foxes.stmx",
        "test/arrays1/arrays.stmx",
        "test/lookup_arrayed/lookup_arrayed.xmile",
        "test/previous/model.stmx",
        "test/conveyors/leaky_conveyor.xmile",
    ] {
        assert_reads_match_the_text(path);
    }
}

#[test]
#[ignore = "model_reads against every corpus model's text; run under the gates profile"]
fn every_corpus_models_reads_match_what_the_text_writes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test");
    let mut dirs = vec![root];
    let (mut swept, mut failures) = (0, Vec::new());
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_lowercase);
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let project = std::panic::catch_unwind(|| match extension.as_deref() {
                Some("mdl") => crate::compat::open_vensim(&String::from_utf8_lossy(&bytes)).ok(),
                Some("xmile" | "stmx" | "itmx") => {
                    crate::compat::open_xmile(&mut std::io::BufReader::new(&bytes[..])).ok()
                }
                _ => None,
            })
            .ok()
            .flatten();
            let Some(project) = project.filter(|p| !p.models.is_empty()) else {
                continue;
            };
            swept += 1;
            // `z` lists element A2 twice, and the engine computes its last
            // arm, which reads neither: the text gate reads every arm.
            if path.ends_with("except.xmile") || path.ends_with("except2.xmile") {
                continue;
            }
            for model in project.models.iter().filter(|m| m.macro_spec.is_none()) {
                let (missed, invented) = reads_against_text(&project, &model.name);
                if !missed.is_empty() || !invented.is_empty() {
                    failures.push(format!(
                        "{} ({}): missed {missed:?}, invented {invented:?}",
                        path.display(),
                        model.name
                    ));
                }
            }
        }
    }
    assert!(swept > 400, "the corpus is present: {swept} models");
    assert!(
        failures.is_empty(),
        "{}\n{}",
        failures.len(),
        failures.join("\n")
    );
}
