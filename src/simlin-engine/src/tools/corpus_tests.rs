// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The corpus sweep: every model in the repository's test corpus through the
//! tools, each answer checked against the schema the catalog publishes for it
//! and against the answer budget, and each refusal against the refusal's
//! shape. The unit tests pin what a tool answers on fixtures made for it; the
//! corpus has what no fixture was made for -- a series that goes undefined, a
//! Vensim macro, a model too large to enumerate.
//!
//! Run with: cargo test --release -p simlin-engine --lib tools::corpus_tests
//! -- --ignored --nocapture

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::test_support::Host;
use super::{OUTLINE_BUDGET, Session, ToolName, ToolOutput, catalog_json};
use crate::datamodel;

/// Every model file under the repository's `test` directory, in order.
fn corpus() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test");
    let mut found = Vec::new();
    let mut dirs = vec![root];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_lowercase);
            if path.is_dir() {
                dirs.push(path);
            } else if matches!(
                extension.as_deref(),
                Some("xmile" | "stmx" | "mdl" | "itmx")
            ) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The project a corpus file holds, or `None` when the importer cannot read
/// it: the sweep is of the tools, not the importers.
fn open(path: &Path) -> Option<datamodel::Project> {
    let bytes = std::fs::read(path).ok()?;
    let is_mdl = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("mdl"));
    std::panic::catch_unwind(|| {
        if is_mdl {
            crate::compat::open_vensim(&String::from_utf8_lossy(&bytes)).ok()
        } else {
            crate::compat::open_xmile(&mut std::io::BufReader::new(&bytes[..])).ok()
        }
    })
    .ok()
    .flatten()
}

/// The schemas answers are checked against, and what the checks found.
struct Sweep {
    outputs: HashMap<&'static str, jsonschema::Validator>,
    refusal: jsonschema::Validator,
    failures: Vec<String>,
    answers: usize,
}

impl Sweep {
    fn new() -> Sweep {
        let catalog: Value = serde_json::from_str(catalog_json()).unwrap();
        let outputs = ToolName::ALL
            .iter()
            .map(|tool| {
                let schema = catalog["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|entry| entry["name"] == tool.name())
                    .unwrap()["outputSchema"]
                    .clone();
                (tool.name(), jsonschema::validator_for(&schema).unwrap())
            })
            .collect();
        let refusal = jsonschema::validator_for(&json!({
            "type": "object",
            "properties": {
                "error": {"type": "string"},
                "suggestions": {"type": "array", "items": {"type": "string"}},
                "interrupted": {"type": "boolean"},
            },
            "required": ["error"],
            "additionalProperties": false,
        }))
        .unwrap();
        Sweep {
            outputs,
            refusal,
            failures: Vec::new(),
            answers: 0,
        }
    }

    /// Call `tool` with `input` and check the answer, which it returns
    /// parsed, `None` for a refusal.
    fn call(
        &mut self,
        model: &str,
        host: &mut Host,
        session: &mut Session,
        tool: ToolName,
        input: Value,
    ) -> Option<Value> {
        let output = host.call_raw(session, tool.name(), &input.to_string());
        self.check(model, tool, &output)
    }

    fn check(&mut self, model: &str, tool: ToolName, output: &ToolOutput) -> Option<Value> {
        self.answers += 1;
        let name = tool.name();
        if output.json.len() > OUTLINE_BUDGET {
            self.failures.push(format!(
                "{model}: {name} answered {} bytes, over the {OUTLINE_BUDGET}-byte budget",
                output.json.len()
            ));
        }
        let Ok(value) = serde_json::from_str::<Value>(&output.json) else {
            self.failures
                .push(format!("{model}: {name} answered text that is not JSON"));
            return None;
        };
        let validator = if output.is_error {
            &self.refusal
        } else {
            &self.outputs[name]
        };
        for error in validator.iter_errors(&value) {
            self.failures.push(format!(
                "{model}: {name}{} at {}: {error}",
                if output.is_error { "'s refusal" } else { "" },
                error.instance_path
            ));
        }
        (!output.is_error).then_some(value)
    }
}

/// A loop whose chain is signed link by link has the polarity its negative
/// links make, an even number reinforcing and an odd one balancing; a chain
/// with a link its run never signed says nothing.
fn check_parity(sweep: &mut Sweep, model: &str, answer: &Value) {
    let loops = answer["partitions"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|partition| partition["loops"].as_array().into_iter().flatten());
    for report in loops {
        let (Some(chain), Some(polarity)) =
            (report["chain"].as_array(), report["polarity"].as_str())
        else {
            continue;
        };
        let signs: Option<Vec<bool>> = chain
            .iter()
            .map(|link| match link["polarity"].as_str() {
                Some("-") => Some(true),
                Some("+") => Some(false),
                _ => None,
            })
            .collect();
        let Some(signs) = signs else { continue };
        let negatives = signs.iter().filter(|&&negative| negative).count();
        let expected = if negatives % 2 == 0 {
            "reinforcing"
        } else {
            "balancing"
        };
        if polarity != "undetermined" && !polarity.ends_with(expected) {
            sweep.failures.push(format!(
                "{model}: {} is {polarity} with {negatives} negative links",
                report["id"]
            ));
        }
    }
}

/// The first constant of `model` an experiment can double: a scalar
/// auxiliary whose equation is a number.
fn first_constant(model: &datamodel::Model) -> Option<String> {
    model.variables.iter().find_map(|v| match v {
        datamodel::Variable::Aux(aux) if aux.gf.is_none() => match &aux.equation {
            datamodel::Equation::Scalar(text) if text.trim().parse::<f64>().is_ok() => {
                Some(aux.ident.clone())
            }
            _ => None,
        },
        _ => None,
    })
}

/// What each model is asked: an outline, every variable read twelve at a
/// time (up to four calls) and its behavior, a search, an experiment that
/// doubles a constant, the runs, and the loops of the model as it is, each
/// loop's polarity checked against its chain's signs.
fn sweep_model(sweep: &mut Sweep, display: &str, project: datamodel::Project) {
    let model = project.models.iter().find(|m| m.macro_spec.is_none());
    let names: Vec<String> = model
        .map(|m| {
            m.variables
                .iter()
                .map(|v| v.get_ident().to_string())
                .collect()
        })
        .unwrap_or_default();
    let constant = model.and_then(first_constant);
    let mut host = Host::new(project);
    let mut session = Session::new("main");
    if sweep
        .call(
            display,
            &mut host,
            &mut session,
            ToolName::ReadModel,
            json!({}),
        )
        .is_none()
    {
        return;
    }
    for chunk in names.chunks(12).take(4) {
        sweep.call(
            display,
            &mut host,
            &mut session,
            ToolName::ReadVariables,
            json!({ "names": chunk }),
        );
        sweep.call(
            display,
            &mut host,
            &mut session,
            ToolName::ReadBehavior,
            json!({ "variables": chunk }),
        );
    }
    if let Some(constant) = constant {
        sweep.call(
            display,
            &mut host,
            &mut session,
            ToolName::RunExperiment,
            json!({"name": "doubled", "set": [{"variable": constant, "multiply": 2}]}),
        );
    }
    sweep.call(
        display,
        &mut host,
        &mut session,
        ToolName::ListRuns,
        json!({}),
    );
    if let Some(loops) = sweep.call(
        display,
        &mut host,
        &mut session,
        ToolName::AnalyzeLoops,
        json!({}),
    ) {
        check_parity(sweep, display, &loops);
    }
    if let Some(first) = names.first() {
        sweep.call(
            display,
            &mut host,
            &mut session,
            ToolName::FindVariables,
            json!({ "phrase": first }),
        );
    }
}

#[test]
#[ignore = "every corpus model through every tool: minutes on a debug build"]
fn every_corpus_answer_matches_its_schema_and_fits_the_budget() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../");
    let mut sweep = Sweep::new();
    let (mut swept, mut unread) = (0, 0);
    for path in corpus() {
        let display = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let Some(project) = open(&path) else {
            unread += 1;
            continue;
        };
        swept += 1;
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sweep_model(&mut sweep, &display, project)
        }))
        .is_err();
        if panicked {
            sweep.failures.push(format!("{display}: a tool panicked"));
        }
    }
    eprintln!(
        "{swept} models swept ({unread} the importers could not read), {} answers",
        sweep.answers
    );
    assert!(swept > 400, "the corpus is present: {swept} models");
    assert!(
        sweep.failures.is_empty(),
        "{} failures:\n{}",
        sweep.failures.len(),
        sweep.failures.join("\n")
    );
}
