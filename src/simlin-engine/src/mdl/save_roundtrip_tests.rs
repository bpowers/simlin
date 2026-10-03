// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What an MDL save keeps of the project it saves, as a datamodel: a corpus
//! model read, saved and read again is the model it was, up to [`normalized`],
//! which states each thing a save is known not to keep and why.
//!
//! `tests/integration/save_meaning.rs` asks whether a save keeps what a model
//! means; this asks the stronger question of whether it keeps the datamodel,
//! so a writer or reader change that moves a field no simulation reads is
//! seen too.

use std::collections::BTreeMap;

use crate::ast::{Expr0, print_eqn};
use crate::datamodel::{Equation, Project, Variable};
use crate::lexer::LexerType;
use crate::mdl::{parse_mdl, project_to_mdl};

/// An equation's text as its parsed tree prints, so two spellings of one
/// tree compare equal: the reader keeps the parentheses a file writes, and
/// the writer writes the fewest the tree needs. Text that does not parse is
/// compared as written, trimmed.
fn canonical_text(text: &str) -> String {
    match Expr0::new(text, LexerType::Equation) {
        Ok(Some(expr)) => print_eqn(&expr),
        _ => text.trim().to_owned(),
    }
}

fn canonical_equation(equation: &Equation) -> Equation {
    match equation {
        Equation::Scalar(text) => Equation::Scalar(canonical_text(text)),
        Equation::ApplyToAll(dims, text) => {
            Equation::ApplyToAll(dims.clone(), canonical_text(text))
        }
        Equation::Arrayed(dims, slots, default, applies) => Equation::Arrayed(
            dims.clone(),
            slots
                .iter()
                .map(|(key, text, initial, gf)| {
                    (
                        key.clone(),
                        canonical_text(text),
                        initial.as_deref().map(canonical_text),
                        gf.clone(),
                    )
                })
                .collect(),
            // The reader keeps the default of an `:EXCEPT:` equation that is
            // its variable's only equation without applying it; nothing reads
            // a default that does not apply.
            default.as_deref().filter(|_| *applies).map(canonical_text),
            *applies,
        ),
    }
}

/// `project` without what an MDL save is known not to keep:
///
/// - the views, which the first save lays out anew (a flow label's own
///   position, a connector's point it no longer reads as the link's shape);
///   `writer_output_idempotence_ratchet` holds the saved sketch to a fixed
///   point instead;
/// - an equation's spelling, compared as its parsed tree ([`canonical_text`]);
/// - an `:EXCEPT:` default the variable does not apply;
/// - the order of a model's variables, which a save writes by group.
pub(in crate::mdl) fn normalized(project: &Project) -> Project {
    let mut project = project.clone();
    for model in &mut project.models {
        model.views.clear();
        model.variables.rewrite(|variables| {
            variables.sort_by(|a, b| a.get_ident().cmp(b.get_ident()));
            for variable in variables.iter_mut() {
                let equation = match variable {
                    Variable::Stock(v) => Some(&mut v.equation),
                    Variable::Flow(v) => Some(&mut v.equation),
                    Variable::Aux(v) => Some(&mut v.equation),
                    Variable::Module(_) => None,
                };
                if let Some(equation) = equation {
                    *equation = canonical_equation(equation);
                }
            }
        });
    }
    project
}

/// The first difference between `a` and `b`, by where it is.
pub(in crate::mdl) fn first_difference(a: &Project, b: &Project) -> Option<String> {
    if a.sim_specs != b.sim_specs {
        return Some(format!("sim specs {:?} -> {:?}", a.sim_specs, b.sim_specs));
    }
    if a.dimensions != b.dimensions {
        return Some(format!(
            "dimensions {:?} -> {:?}",
            a.dimensions, b.dimensions
        ));
    }
    if a.units != b.units {
        return Some("unit equivalences".to_owned());
    }
    if a.models.len() != b.models.len() {
        return Some(format!("{} models -> {}", a.models.len(), b.models.len()));
    }
    for (ma, mb) in a.models.iter().zip(&b.models) {
        let vars = |m: &crate::datamodel::Model| -> BTreeMap<String, Variable> {
            m.variables
                .iter()
                .map(|v| (v.get_ident().to_owned(), v.clone()))
                .collect()
        };
        let (va, vb) = (vars(ma), vars(mb));
        for (ident, var) in &va {
            match vb.get(ident) {
                None => return Some(format!("{}: '{ident}' is not in the save", ma.name)),
                Some(other) if other != var => {
                    return Some(format!("{}: '{ident}' {var:?} -> {other:?}", ma.name));
                }
                Some(_) => {}
            }
        }
        if let Some(ident) = vb.keys().find(|ident| !va.contains_key(*ident)) {
            return Some(format!("{}: '{ident}' is new in the save", ma.name));
        }
        if ma != mb {
            return Some(format!("{}: the model's other fields", ma.name));
        }
    }
    (a != b).then(|| "the project's other fields".to_owned())
}

/// What a save of the MDL file `path` (from the checkout's root) changes,
/// or None when it keeps the datamodel; Err when the file does not import
/// without a data provider, or when the save writes an equation Vensim's
/// subscript rule refuses (`subscript_rule`).
fn save_changes(path: &str) -> Result<Option<String>, String> {
    let bytes = std::fs::read(format!("../../{path}")).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let first = parse_mdl(&text).map_err(|e| e.to_string())?;
    let save = project_to_mdl(&first).map_err(|e| format!("the save fails: {e}"))?;
    let unheld = crate::mdl::subscript_rule::ranges_not_on_the_left(&save, &first);
    if !unheld.is_empty() {
        return Err(format!(
            "the save names a subscript range its left-hand side does not hold: {}",
            unheld.join("; ")
        ));
    }
    let second = parse_mdl(&save).map_err(|e| format!("the save does not read back: {e}"))?;
    Ok(first_difference(&normalized(&first), &normalized(&second)))
}

/// Every corpus `.mdl`, sorted, by its path from the checkout's root.
pub(in crate::mdl) fn corpus() -> Vec<String> {
    let root = std::path::PathBuf::from("../..");
    let mut files = Vec::new();
    let mut dirs = vec![root.join("test")];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.is_dir() {
                dirs.push(path);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("mdl"))
                && let Ok(rel) = path.strip_prefix(&root)
            {
                files.push(rel.to_string_lossy().into_owned());
            }
        }
    }
    files.sort();
    files
}

/// Every corpus file whose save changes its datamodel past [`normalized`],
/// and why. A file whose save starts changing it fails the gate, and so does
/// a listed one that stops: a fix removes its row.
const EXPECTED: &[(&str, &str)] = &[];

/// Fewer saves than this means the corpus walk or the reader broke, which
/// would otherwise pass the gate vacuously.
const MIN_SAVED: usize = 200;

#[test]
#[ignore = "saves every corpus .mdl and reads it back; run under the gates profile"]
fn a_save_keeps_the_datamodel_over_the_corpus() {
    use rayon::prelude::*;
    let results: Vec<(String, Result<Option<String>, String>)> = corpus()
        .into_par_iter()
        .map(|path| {
            let result = save_changes(&path);
            (path, result)
        })
        .collect();
    let saved = results.iter().filter(|(_, r)| r.is_ok()).count();
    assert!(saved >= MIN_SAVED, "only {saved} corpus files saved");
    let mut failures = Vec::new();
    for (path, result) in &results {
        let listed = EXPECTED.iter().any(|(p, _)| p == path);
        match result {
            // A file the reader refuses without a data provider says nothing
            // about the writer.
            Err(why) if why.starts_with("the save") => failures.push(format!("{path}: {why}")),
            Err(_) => {}
            Ok(Some(change)) if !listed => failures.push(format!("{path}: {change}")),
            Ok(None) if listed => {
                failures.push(format!("{path} keeps its datamodel: remove its row"))
            }
            Ok(_) => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The same property on small models, in the default suite: arrayed
/// variables with `:EXCEPT:` defaults, two of them over a subrange their
/// defaults name; and stocks, flows, lookups and a macro.
#[test]
fn a_save_keeps_the_datamodel() {
    for path in [
        "test/test-models/tests/except/test_except.mdl",
        "test/test-models/tests/except_subranges/test_except_subranges.mdl",
        "test/test-models/tests/except_multiple/test_except_multiple.mdl",
        "test/test-models/tests/macro_stock/test_macro_stock.mdl",
    ] {
        assert_eq!(save_changes(path), Ok(None), "{path}");
    }
}
