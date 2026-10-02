// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A save keeps what the model means, in every pair of formats: every corpus
//! model, read from its own format (MDL, or XMILE for XMILE and Stella
//! files), is saved as MDL, as XMILE, as native JSON and as protobuf, and
//! `save_check::check_save` says what each save changes.
//!
//! [`EXPECTED`] lists every save that does not keep its model, with what the
//! check says of it and why it happens: the format cannot hold the model, or
//! a writer or reader has a defect. The list is a ratchet in both directions.
//! A save that starts changing a model fails the sweep, and so does a listed
//! save that stops, or changes otherwise than its row says, or is no longer
//! swept: a fix removes the rows it repairs.
//!
//! The check is itself held to an independent run: the sweep simulates each
//! model and the model its save reads back as, with a save and a column
//! comparison of its own, and a save whose columns differ must be one the
//! check calls a change to the results.
//!
//! The whole corpus is swept under the gates profile
//! ([`every_save_keeps_its_model_or_is_listed`], ignored in the default
//! suite); a few models that between them make every pair of formats are
//! swept in the default suite, against the same list.
//!
//! sd-ai JSON is not swept: it holds one model's variables and specs and
//! nothing of its dimensions, modules or tables, by design, so most models
//! change in it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::BufReader;
use std::path::PathBuf;

use simlin_engine::buffa::Message;
use simlin_engine::datamodel::Project;
use simlin_engine::save_check::{ChangeKind, SaveFormat, check_save, column_variable};
use simlin_engine::{Results, queue_compile};

use Cause::{Defect, Format};
use SaveFormat::{Json, Mdl, Protobuf, Xmile};
use Verdict::{Definition, Refused, Results as ResultsChange};

/// The formats a model is saved in.
const TARGETS: [SaveFormat; 4] = [Mdl, Xmile, Json, Protobuf];

/// What the check says of one save.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// The save keeps the model. Never listed.
    Keeps,
    /// Only the definition changes (`ChangeKind::Structure`).
    Definition,
    /// The results change (`ChangeKind::Results`).
    Results,
    /// The format's writer refuses the project.
    Refused,
}

/// Why a save does not keep its model. A label for the reader of the list,
/// which the sweep does not check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cause {
    /// The format cannot hold something the model has. What, in a few words.
    Format(&'static str),
    /// A writer or a reader has a defect. What goes wrong, in a few words.
    Defect(&'static str),
}

/// A save that does not keep its model: the file, the format it is read from
/// and the format it is saved in, what the check says, and why.
struct Expected {
    file: &'static str,
    from: SaveFormat,
    to: SaveFormat,
    verdict: Verdict,
    #[allow(dead_code)]
    cause: Cause,
}

const fn row(
    file: &'static str,
    from: SaveFormat,
    to: SaveFormat,
    verdict: Verdict,
    cause: Cause,
) -> Expected {
    Expected {
        file,
        from,
        to,
        verdict,
        cause,
    }
}

// Causes more than one row has.
const NON_NEGATIVE: Cause = Format("MDL has no non-negative marking");
const ONE_MODEL: Cause = Format("MDL holds one model");
const SPECIAL_STOCK: Cause = Format("MDL has no conveyor or queue");
const ELEMENT_FAMILY: Cause = Defect(
    "an arrayed variable written one element equation each reads back over the elements' whole family",
);
const EXCEPT_DEFAULT: Cause = Defect(
    "the writer drops an :EXCEPT: default (it warns where the default names the variable's own dimensions)",
);
const RECIPROCAL_DT: Cause =
    Defect("a reciprocal time step is written as `1/n`, which the reader cannot evaluate");
const UNREADABLE: Cause = Defect("the writer writes text the reader refuses");
const RESPELLED_CALL: Cause =
    Defect("a call is written under a name the reader takes for a lookup or another function");
const SHARED_FLOW: Cause =
    Defect("a flow two stocks share reads back as an auxiliary beside a net flow of each stock");
const MACRO_INPUT: Cause = Defect("a macro input with no equation is written back as `0`");
const QUANTUM: Cause =
    Defect("`q * INT(x / q)` is written as QUANTUM, which truncates toward zero where INT floors");
const REPEATING_PULSE: Cause = Defect("a repeating PULSE is written as a single pulse");
const UNPARSED_EQUATION: Cause =
    Defect("an equation that does not parse is written as another equation");
const NAME_NEWLINE: Cause = Defect("a newline in a name is written as a space");
const NET_FLOW: Cause =
    Defect("a stock whose INTEG is not a plain sum of flows reads back with a net flow");

/// Every save that does not keep its model, by file. One row a line, so a
/// fix removes exactly the rows it repairs.
#[rustfmt::skip]
const EXPECTED: &[Expected] = &[
    row("test/ai-information/GeneratedByAIThenEdited.stmx", Xmile, Mdl, ResultsChange, NON_NEGATIVE),
    row("test/ai-information/PureHumanModel.stmx", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/ai-information/WithModulesAndArrays.stmx", Xmile, Mdl, Refused, ONE_MODEL),
    row("test/alias1/alias1.stmx", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/array_broadcast/array_broadcast.xmile", Xmile, Mdl, ResultsChange, RESPELLED_CALL),
    row("test/array_multi_source/array_multi_source.xmile", Xmile, Mdl, ResultsChange, RESPELLED_CALL),
    row("test/array_sum_simple/array_sum_simple.xmile", Xmile, Mdl, ResultsChange, RESPELLED_CALL),
    row("test/arrays1/arrays.stmx", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/builtin_init/builtin_init.stmx", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/conveyors/arrayed_conveyor.xmile", Xmile, Mdl, ResultsChange, SPECIAL_STOCK),
    row("test/conveyors/conveyor_containers.xmile", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/conveyors/covid19_severity.stmx", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/conveyors/discrete_conveyor.xmile", Xmile, Mdl, ResultsChange, SPECIAL_STOCK),
    row("test/conveyors/leaky_conveyor.xmile", Xmile, Mdl, ResultsChange, SPECIAL_STOCK),
    row("test/conveyors/minimal_conveyor.xmile", Xmile, Mdl, ResultsChange, SPECIAL_STOCK),
    row("test/conveyors/queue_coupled_conveyor.xmile", Xmile, Mdl, ResultsChange, SPECIAL_STOCK),
    row("test/conveyors/sir_social_distancing_mixnot.stmx", Xmile, Mdl, Refused, ONE_MODEL),
    row("test/delays/model.xmile", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/ltm_dynamic_range_unsupported/model.stmx", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/metasd/FREE/FREE6/FREE6-corrected/conversion.mdl", Mdl, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/metasd/FREE/FREE6/FREE6-corrected/conversion2.mdl", Mdl, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/metasd/FREE/FREE6/FREE6-original/conversion.mdl", Mdl, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/metasd/FREE/FREE6/FREE6-original/conversion2.mdl", Mdl, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/metasd/FREE/FREE6/FREE6-original/free 6.mdl", Mdl, Mdl, Definition, ELEMENT_FAMILY),
    row("test/metasd/beer-game/RealBeer4-Sterman13.mdl", Mdl, Mdl, Definition, ELEMENT_FAMILY),
    row("test/modules2/modules2.xmile", Xmile, Mdl, Refused, ONE_MODEL),
    row("test/modules_hares_and_foxes/modules_hares_and_foxes.stmx", Xmile, Mdl, Refused, ONE_MODEL),
    row("test/modules_with_complex_idents/modules_with_complex_idents.stmx", Xmile, Mdl, Refused, ONE_MODEL),
    row("test/previous/model.stmx", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/queues/minimal_queue.xmile", Xmile, Mdl, ResultsChange, SPECIAL_STOCK),
    row("test/queues/queue_drain.xmile", Xmile, Mdl, ResultsChange, SPECIAL_STOCK),
    row("test/sdeverywhere/models/arrays_cname/arrays_cname.mdl", Mdl, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/sdeverywhere/models/arrays_cname/arrays_cname.xmile", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/sdeverywhere/models/arrays_varname/arrays_varname.mdl", Mdl, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/sdeverywhere/models/arrays_varname/arrays_varname.xmile", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/sdeverywhere/models/delay/delay.mdl", Mdl, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/sdeverywhere/models/delay/delay.xmile", Xmile, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/sdeverywhere/models/directsubs/directsubs.xmile", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/sdeverywhere/models/except/except.mdl", Mdl, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/sdeverywhere/models/except/except.xmile", Xmile, Mdl, Definition, ELEMENT_FAMILY),
    row("test/sdeverywhere/models/except2/except2.mdl", Mdl, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/sdeverywhere/models/except2/except2.xmile", Xmile, Mdl, Definition, ELEMENT_FAMILY),
    row("test/sdeverywhere/models/extdata/extdata.xmile", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/sdeverywhere/models/longeqns/longeqns.xmile", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/sdeverywhere/models/prune/prune.xmile", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/sdeverywhere/models/quantum/quantum.xmile", Xmile, Mdl, ResultsChange, QUANTUM),
    row("test/sdeverywhere/models/smooth/smooth.mdl", Mdl, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/sdeverywhere/models/subalias/subalias.mdl", Mdl, Mdl, Definition, ELEMENT_FAMILY),
    row("test/sdeverywhere/models/sum/sum.mdl", Mdl, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/sdeverywhere/models/sum/sum.xmile", Xmile, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/sdeverywhere/models/sumif/sumif.xmile", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/step_into_smth1/model.stmx", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/subscript_index_name_values/model.stmx", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/test-models/samples/SIR/SIR_reciprocal-dt.xmile", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/test-models/samples/arrays/a2a/a2a.stmx", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/test-models/samples/arrays/non-a2a/non-a2a-gf.stmx", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/test-models/samples/arrays/non-a2a/non-a2a.stmx", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/test-models/samples/bpowers-hares_and_lynxes_modules/model.stmx", Xmile, Mdl, Refused, ONE_MODEL),
    row("test/test-models/samples/bpowers-hares_and_lynxes_modules/model.xmile", Xmile, Mdl, Refused, ONE_MODEL),
    row("test/test-models/samples/bpowers-hares_and_lynxes_modules/model_legacy.stmx", Xmile, Mdl, Refused, ONE_MODEL),
    row("test/test-models/samples/display/1style.stmx", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/test-models/samples/display/multipoint-connection.stmx", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/test-models/samples/teacup/teacup.stmx", Xmile, Mdl, ResultsChange, NON_NEGATIVE),
    row("test/test-models/tests/allocate_by_priority/test_allocate_by_priority.mdl", Mdl, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/test-models/tests/builtin_int/builtin_int.stmx", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/test-models/tests/builtin_int/builtin_int.xmile", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/test-models/tests/builtin_mean/builtin_mean.stmx", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/test-models/tests/builtin_mean/builtin_mean.xmile", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/test-models/tests/data_from_other_model/test_data_from_other_model.mdl", Mdl, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/test-models/tests/delay_xmile/test_delay_xmile.xmile", Xmile, Mdl, ResultsChange, NON_NEGATIVE),
    row("test/test-models/tests/delays2/delays.xmile", Xmile, Mdl, ResultsChange, RECIPROCAL_DT),
    row("test/test-models/tests/except/test_except.mdl", Mdl, Mdl, Definition, EXCEPT_DEFAULT),
    row("test/test-models/tests/except_multiple/test_except_multiple.mdl", Mdl, Mdl, Definition, EXCEPT_DEFAULT),
    row("test/test-models/tests/except_subranges/test_except_subranges.mdl", Mdl, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/test-models/tests/input_functions/test_inputs.xmile", Xmile, Mdl, ResultsChange, REPEATING_PULSE),
    row("test/test-models/tests/invert_matrix/test_invert_matrix.mdl", Mdl, Mdl, Definition, ELEMENT_FAMILY),
    row("test/test-models/tests/logicals/test_logicals.stmx", Xmile, Mdl, Definition, UNPARSED_EQUATION),
    row("test/test-models/tests/lookups/test_lookups.xmile", Xmile, Mdl, Definition, RESPELLED_CALL),
    row("test/test-models/tests/lookups/test_lookups_xpts_sep.xmile", Xmile, Mdl, Definition, RESPELLED_CALL),
    row("test/test-models/tests/lookups/test_lookups_xscale.xmile", Xmile, Mdl, Definition, RESPELLED_CALL),
    row("test/test-models/tests/lookups/test_lookups_ypts_sep.xmile", Xmile, Mdl, Definition, RESPELLED_CALL),
    row("test/test-models/tests/lookups_funcnames/test_lookups_funcnames.xmile", Xmile, Mdl, ResultsChange, UNPARSED_EQUATION),
    row("test/test-models/tests/macro_expression/test_macro_expression.stmx", Xmile, Mdl, Definition, UNPARSED_EQUATION),
    row("test/test-models/tests/macro_multi_expression/test_macro_multi_expression.stmx", Xmile, Mdl, Definition, UNPARSED_EQUATION),
    row("test/test-models/tests/macro_multi_macros/test_macro_multi_macros.stmx", Xmile, Mdl, Definition, UNPARSED_EQUATION),
    row("test/test-models/tests/macro_stock/test_macro_stock.stmx", Xmile, Mdl, Definition, UNPARSED_EQUATION),
    row("test/test-models/tests/macro_stock/test_macro_stock.xmile", Xmile, Mdl, Definition, MACRO_INPUT),
    row("test/test-models/tests/macro_stock/test_macro_stock.xmile", Xmile, Xmile, Definition, MACRO_INPUT),
    row("test/test-models/tests/min_max_1arg/test_min_max_1arg.xmile", Xmile, Mdl, Definition, UNPARSED_EQUATION),
    row("test/test-models/tests/non_negative_all/test_non_negative_all1.xmile", Xmile, Mdl, ResultsChange, SHARED_FLOW),
    row("test/test-models/tests/non_negative_all/test_non_negative_all2.xmile", Xmile, Mdl, ResultsChange, SHARED_FLOW),
    row("test/test-models/tests/non_negative_flows/test_non_negative_flows.xmile", Xmile, Mdl, ResultsChange, NON_NEGATIVE),
    row("test/test-models/tests/non_negative_flows/test_non_negative_flows_behavior.xmile", Xmile, Mdl, ResultsChange, NON_NEGATIVE),
    row("test/test-models/tests/non_negative_stocks/test_non_negative_stocks.xmile", Xmile, Mdl, ResultsChange, SHARED_FLOW),
    row("test/test-models/tests/non_negative_stocks/test_non_negative_stocks_behavior.xmile", Xmile, Mdl, ResultsChange, SHARED_FLOW),
    row("test/test-models/tests/special_characters/test_special_variable_names.mdl", Mdl, Mdl, Definition, NAME_NEWLINE),
    row("test/test-models/tests/subscript_aggregation/test_subscript_aggregation.xmile", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/test-models/tests/subscript_constant_call/test_subscript_constant_call.stmx", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/test-models/tests/subscript_copy/test_subscript_copy.mdl", Mdl, Mdl, Definition, ELEMENT_FAMILY),
    row("test/test-models/tests/subscript_copy/test_subscript_copy2.mdl", Mdl, Mdl, Definition, ELEMENT_FAMILY),
    row("test/test-models/tests/subscript_mixed_assembly/test_subscript_mixed_assembly.stmx", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/test-models/tests/subscript_multiples/test_multiple_subscripts.stmx", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/test-models/tests/subscript_subranges/test_subscript_subrange.stmx", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/test-models/tests/subscript_subranges_equal/test_subscript_subrange_equal.mdl", Mdl, Mdl, Definition, ELEMENT_FAMILY),
    row("test/test-models/tests/subscript_subranges_equal/test_subscript_subrange_equal.stmx", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/test-models/tests/subscript_switching/subscript_switching.mdl", Mdl, Mdl, Definition, ELEMENT_FAMILY),
    row("test/test-models/tests/subscript_switching/subscript_switching.xmile", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/test-models/tests/subscript_transposition/test_subscript_transposition.mdl", Mdl, Mdl, Definition, ELEMENT_FAMILY),
    row("test/test-models/tests/subscript_updimensioning/test_subscript_updimensioning.stmx", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/test-models/tests/subscripted_flows/test_subscripted_flows.stmx", Xmile, Mdl, ResultsChange, UNREADABLE),
    row("test/test-models/tests/subscripted_ramp_step/test_subscripted_ramp_step.mdl", Mdl, Mdl, ResultsChange, ELEMENT_FAMILY),
    row("test/test-models/tests/subset_duplicated_coord/test_subset_duplicated_coord.mdl", Mdl, Mdl, Definition, ELEMENT_FAMILY),
    row("test/test-models/tests/zeroled_decimals/test_zeroled_decimals.xmile", Xmile, Mdl, ResultsChange, NET_FLOW),
    row("test/vector_snapshot_operand/vector_snapshot_operand.xmile", Xmile, Mdl, ResultsChange, RESPELLED_CALL),
    row("test/xmutil_test_models/C-LEARN v77 for Vensim.xmile", Xmile, Mdl, Refused, ONE_MODEL),
];

/// Every model file under `test/`, sorted, by its path from the checkout's
/// root.
fn corpus() -> Vec<String> {
    let root = PathBuf::from("../..");
    let mut files = Vec::new();
    let mut dirs = vec![root.join("test")];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            let model = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                ["mdl", "xmile", "stmx", "itmx"]
                    .iter()
                    .any(|x| e.eq_ignore_ascii_case(x))
            });
            if path.is_dir() {
                dirs.push(path);
            } else if model {
                let rel = path
                    .strip_prefix(&root)
                    .expect("the walk starts at the root")
                    .to_string_lossy()
                    .into_owned();
                files.push(rel);
            }
        }
    }
    files.sort();
    files
}

/// The format a file is read from.
fn format_of(path: &str) -> SaveFormat {
    if path.to_ascii_lowercase().ends_with(".mdl") {
        Mdl
    } else {
        Xmile
    }
}

fn open(path: &str) -> Option<Project> {
    let bytes = fs::read(format!("../../{path}")).ok()?;
    match format_of(path) {
        Mdl => simlin_engine::open_vensim(std::str::from_utf8(&bytes).ok()?).ok(),
        _ => simlin_engine::open_xmile(&mut BufReader::new(bytes.as_slice())).ok(),
    }
}

/// The model a host simulates: `main`, or else the first that is not a macro.
fn main_model(project: &Project) -> Option<&simlin_engine::datamodel::Model> {
    project
        .models
        .iter()
        .find(|m| m.name == "main")
        .or_else(|| project.models.iter().find(|m| m.macro_spec.is_none()))
}

/// The model's run as a host makes it: through the dispatch every host
/// compiles through (so a conveyor or a queue simulates), of [`main_model`].
fn simulate(project: &Project) -> Option<Results> {
    let main = main_model(project)?;
    let mut vm = queue_compile::build_vm(project, &main.name).ok()?;
    vm.run_to_end().ok()?;
    Some(vm.into_results())
}

/// The model a save of `project` in `format` reads back as, by this sweep's
/// own save, apart from the check's.
fn saved(project: &Project, format: SaveFormat) -> Option<Project> {
    match format {
        Mdl => simlin_engine::open_vensim(&simlin_engine::to_mdl(project).ok()?).ok(),
        Xmile => {
            let text = simlin_engine::to_xmile(project).ok()?;
            simlin_engine::open_xmile(&mut BufReader::new(text.as_bytes())).ok()
        }
        Json => {
            let json: simlin_engine::json::Project = project.clone().into();
            let bytes = serde_json::to_vec(&json).ok()?;
            let back = simlin_engine::json::Project::from_reader(bytes.as_slice()).ok()?;
            Some(back.into())
        }
        Protobuf => {
            let bytes = simlin_engine::serde::serialize(project)
                .ok()?
                .try_encode_to_vec()
                .ok()?;
            let back = simlin_engine::project_io::Project::decode_from_slice(&bytes).ok()?;
            Some(simlin_engine::serde::deserialize(back))
        }
        SaveFormat::SdaiJson => None,
    }
}

/// The control variables a model can hold as variables, which a format that
/// keeps them as its specs reads back as specs: the check calls that no
/// change when the values agree, so their columns are left out of the run
/// comparison.
const CONTROL_COLUMNS: &[&str] = &["initial_time", "final_time", "time_step", "saveper"];

/// Each column `project`'s run (`results`) has, by name: the clock and the
/// columns of the simulated model's variables, which column of which variable
/// asked of `save_check::column_variable` (the rule the check reads a run
/// by), so the compiler's own helper columns are left out, and the control
/// columns. Every NaN is the same value, and -0 is 0.
fn columns(project: &Project, results: &Results) -> BTreeMap<String, Vec<u64>> {
    let variables: BTreeSet<String> = main_model(project)
        .map(|model| {
            model
                .variables
                .iter()
                .map(|var| simlin_engine::canonicalize(var.get_ident()).into_owned())
                .collect()
        })
        .unwrap_or_default();
    results
        .offsets
        .iter()
        .filter(|(name, _)| {
            let name = name.as_str();
            (name == "time" || column_variable(name, &variables).is_some())
                && !CONTROL_COLUMNS.contains(&name)
        })
        .map(|(name, &at)| {
            let column = results
                .iter()
                .map(|row| {
                    if row[at].is_nan() {
                        f64::NAN.to_bits()
                    } else if row[at] == 0.0 {
                        0.0f64.to_bits()
                    } else {
                        row[at].to_bits()
                    }
                })
                .collect();
            (name.to_string(), column)
        })
        .collect()
}

/// One save, swept.
struct Swept {
    file: String,
    to: SaveFormat,
    verdict: Verdict,
    /// The first of the check's reasons, for a failure's message.
    reasons: Vec<String>,
    /// Whether this sweep's own runs of the model and of its save differ;
    /// None when either does not simulate or the save does not read back.
    run_differs: Option<bool>,
}

/// Sweep `files`, each saved in every target format; the files that do not
/// open are left out.
fn sweep(files: Vec<String>) -> Vec<Swept> {
    use rayon::prelude::*;
    files
        .into_par_iter()
        .flat_map_iter(|file| {
            let Some(project) = open(&file) else {
                return Vec::new();
            };
            let run = simulate(&project).map(|results| columns(&project, &results));
            TARGETS
                .iter()
                .map(|&to| {
                    let (verdict, reasons) = match check_save(&project, to) {
                        Err(e) => (Refused, vec![e.to_string()]),
                        Ok(changes) if changes.is_empty() => (Verdict::Keeps, Vec::new()),
                        Ok(changes) => {
                            let results = changes.iter().any(|c| c.kind == ChangeKind::Results);
                            (
                                if results { ResultsChange } else { Definition },
                                changes.iter().take(6).map(|c| c.reason.clone()).collect(),
                            )
                        }
                    };
                    let run_differs = run.as_ref().and_then(|before| {
                        let save = saved(&project, to)?;
                        let after = columns(&save, &simulate(&save)?);
                        Some(*before != after)
                    });
                    Swept {
                        file: file.clone(),
                        to,
                        verdict,
                        reasons,
                        run_differs,
                    }
                })
                .collect()
        })
        .collect()
}

/// What the swept saves get wrong against [`EXPECTED`], one line each: a save
/// that changes its model and is not listed, a listed save whose verdict is
/// another (or none), a listed save of a swept file's that was not swept, and
/// a save whose run differs though the check does not call it a change to the
/// results. `files` are the files asked for, so a listed file that did not
/// open is caught.
fn failures(files: &[String], swept: &[Swept]) -> Vec<String> {
    let mut failures = Vec::new();
    let listed: BTreeMap<(&str, SaveFormat), &Expected> = EXPECTED
        .iter()
        .map(|expected| ((expected.file, expected.to), expected))
        .collect();
    if listed.len() != EXPECTED.len() {
        failures.push("a save is listed twice".to_string());
    }
    for expected in EXPECTED {
        if expected.from != format_of(expected.file) {
            failures.push(format!(
                "{} is read as {:?}, not {:?}",
                expected.file,
                format_of(expected.file),
                expected.from
            ));
        }
        if expected.verdict == Verdict::Keeps {
            failures.push(format!(
                "{} as {:?} is listed as kept",
                expected.file, expected.to
            ));
        }
    }
    let mut seen: BTreeSet<(&str, SaveFormat)> = BTreeSet::new();
    for save in swept {
        seen.insert((save.file.as_str(), save.to));
        let expected = listed.get(&(save.file.as_str(), save.to));
        let as_save = format!("{} saved as {:?}", save.file, save.to);
        match (save.verdict, expected.map(|e| e.verdict)) {
            (Verdict::Keeps, None) => {}
            (is, Some(was)) if is == was => {}
            (Verdict::Keeps, Some(_)) => {
                failures.push(format!("{as_save} keeps the model: remove its row"))
            }
            (is, None) => failures.push(format!(
                "{as_save} does not keep the model ({is:?}): {}",
                save.reasons.join("; ")
            )),
            (is, Some(was)) => failures.push(format!(
                "{as_save} is listed as {was:?} and is {is:?}: {}",
                save.reasons.join("; ")
            )),
        }
        if save.run_differs == Some(true) && save.verdict != ResultsChange {
            failures.push(format!(
                "{as_save} simulates differently, and the check says {:?}",
                save.verdict
            ));
        }
    }
    let asked: BTreeSet<&str> = files.iter().map(String::as_str).collect();
    for expected in EXPECTED {
        if asked.contains(expected.file) && !seen.contains(&(expected.file, expected.to)) {
            failures.push(format!(
                "{} as {:?} is listed and was not swept",
                expected.file, expected.to
            ));
        }
    }
    failures
}

/// The number of saves the whole corpus makes: every file that opens, in
/// every target format. A file added to the corpus moves it; a reader that
/// stops opening files moves it too, which is what it is asserted for.
const CORPUS_SAVES: usize = 1940;

/// Every corpus model, C-LEARN included, saved in every format.
#[test]
#[ignore = "sweeps the whole corpus through every format pair; run under the gates profile"]
fn every_save_keeps_its_model_or_is_listed() {
    let files = corpus();
    let swept = sweep(files.clone());
    let mut failures = failures(&files, &swept);
    for expected in EXPECTED {
        if !files.iter().any(|file| file == expected.file) {
            failures.push(format!(
                "{} is listed and is not in the corpus",
                expected.file
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(swept.len(), CORPUS_SAVES, "the number of saves swept");
}

/// A few models that between them are read from each format and saved in
/// each: the same sweep, against the same list, in the default suite. One of
/// them has a row (Stella's teacup marks its stock non-negative, which MDL
/// cannot hold), so a stale list fails here too.
#[test]
fn a_few_models_keep_their_meaning_in_every_format_pair() {
    let files: Vec<String> = [
        "test/test-models/samples/teacup/teacup.mdl",
        "test/test-models/samples/teacup/teacup.xmile",
        "test/test-models/samples/teacup/teacup.stmx",
        "test/test-models/tests/subscript_2d_arrays/test_subscript_2d_arrays.mdl",
        "test/test-models/tests/subscript_2d_arrays/test_subscript_2d_arrays.xmile",
    ]
    .iter()
    .map(|file| file.to_string())
    .collect();
    let swept = sweep(files.clone());
    let failures = failures(&files, &swept);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(swept.len(), files.len() * TARGETS.len());
    assert!(
        swept.iter().all(|save| save.run_differs == Some(false)),
        "every one of these simulates, and its save simulates the same"
    );
    let listed = swept
        .iter()
        .filter(|save| save.verdict != Verdict::Keeps)
        .count();
    assert_eq!(listed, 1, "teacup.stmx saved as MDL");
}

/// A model that reads external data is read back with the data provider it
/// was opened with, so its save's references resolve as the model's did.
/// Without it, the save does not read back, which is a change to the results.
#[test]
fn a_save_that_reads_data_is_read_back_with_its_provider() {
    use simlin_engine::save_check::check_save_with_data;
    let path = "../../test/test-models/tests/get_data/test_get_data.mdl";
    let dir = std::path::Path::new(path)
        .parent()
        .expect("the model is in a directory");
    let provider = simlin_engine::FilesystemDataProvider::new(dir);
    let text = fs::read_to_string(path).expect("the model reads");
    let project =
        simlin_engine::open_vensim_with_data(&text, Some(&provider)).expect("the model opens");

    let changes =
        check_save_with_data(&project, Mdl, Some(&provider)).expect("MDL holds the model");
    assert!(changes.is_empty(), "{changes:?}");

    let changes = check_save(&project, Mdl).expect("MDL holds the model");
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert!(
        changes[0]
            .reason
            .starts_with("the save does not read back: "),
        "{changes:?}"
    );
    assert_eq!(changes[0].kind, ChangeKind::Results);
}
