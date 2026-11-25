// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What an MDL save keeps of the project it saves, as a datamodel: a corpus
//! model read, saved and read again is the model it was, up to [`normalized`],
//! which states each thing a save is known not to keep and why.
//!
//! `simlin-tests/tests/integration/save_meaning.rs` asks whether a save keeps what a model
//! means; this asks the stronger question of whether it keeps the datamodel,
//! so a writer or reader change that moves a field no simulation reads is
//! seen too.

use crate::mdl::save_compare::{first_difference, normalized};
use crate::mdl::{parse_mdl, project_to_mdl};

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
