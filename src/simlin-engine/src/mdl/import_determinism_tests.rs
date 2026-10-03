// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The MDL reader imports a file one way on every run. Each `HashMap` the
//! reader builds takes its own random seed, so a decision that reads one in
//! its iteration order shows up as two imports of one file that differ, in
//! one process.

use crate::compat::open_vensim_with_data;
use crate::datamodel::Project;

/// How many times each file is imported.
const IMPORTS: usize = 13;

/// One import of the MDL file `path` (from the checkout's root) without a
/// data provider and, where the build reads data files, one with the data
/// its directory holds: each the project or the reader's refusal. A file that
/// reads data is refused without it, and which reference the refusal names is
/// part of what an import decides.
type Imports = (Result<Project, String>, Option<Result<Project, String>>);

fn import(path: &str) -> Option<Imports> {
    let file = std::path::PathBuf::from("../..").join(path);
    let bytes = std::fs::read(&file).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let without = open_vensim_with_data(&text, None).map_err(|e| e.to_string());
    #[cfg(feature = "file_io")]
    let with = file.parent().map(|dir| {
        let provider = crate::data_provider::FilesystemDataProvider::new(dir);
        open_vensim_with_data(&text, Some(&provider)).map_err(|e| e.to_string())
    });
    #[cfg(not(feature = "file_io"))]
    let with = None;
    Some((without, with))
}

/// The first import of `path` that differs from its first, if one does.
fn nondeterminism(path: &str) -> Option<String> {
    let first = import(path)?;
    (1..IMPORTS).find_map(|round| {
        let again = import(path)?;
        (again != first).then(|| {
            let describe = |r: &Result<Project, String>| match r {
                Ok(_) => "a project".to_owned(),
                Err(e) => format!("the error {e}"),
            };
            let (now, then) = if again.0 != first.0 {
                (describe(&again.0), describe(&first.0))
            } else {
                (
                    again.1.as_ref().map_or("none".to_owned(), describe),
                    first.1.as_ref().map_or("none".to_owned(), describe),
                )
            };
            format!("{path}: import {round} is {now} where import 0 is {then}")
        })
    })
}

#[test]
#[ignore = "imports every corpus .mdl 13 times; run under the gates profile"]
fn every_corpus_file_imports_one_way() {
    use rayon::prelude::*;
    let files = crate::mdl::save_roundtrip_tests::corpus();
    assert!(files.len() >= 200, "only {} corpus files", files.len());
    let differing: Vec<String> = files
        .par_iter()
        .filter_map(|path| nondeterminism(path))
        .collect();
    assert!(differing.is_empty(), "{}", differing.join("\n"));
}

/// The same on two models in the default suite: one with arrayed variables
/// over subranges, mappings and an `:EXCEPT:`, and one that reads data.
#[test]
fn a_file_imports_one_way() {
    for path in [
        "test/test-models/tests/except_subranges/test_except_subranges.mdl",
        "test/sdeverywhere/models/directconst/directconst.mdl",
    ] {
        assert!(import(path).is_some(), "{path} reads");
        assert_eq!(nondeterminism(path), None);
    }
}
