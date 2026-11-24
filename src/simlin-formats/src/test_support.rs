// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The models of the repository's `test/`, for tests that sweep them.

use std::io::BufReader;
use std::path::{Path, PathBuf};

use simlin_base::datamodel::Project;

fn corpus_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            corpus_files(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("mdl" | "stmx" | "xmile" | "itmx")
        ) {
            out.push(path);
        }
    }
}

/// Calls `f` with every model file under the repository's `test/` that
/// opens, in path order.
pub fn for_each_corpus_project(mut f: impl FnMut(&Path, &Project)) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test");
    let mut files = Vec::new();
    corpus_files(&root, &mut files);
    for path in files {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let project = if path.extension().and_then(|e| e.to_str()) == Some("mdl") {
            String::from_utf8(bytes)
                .ok()
                .and_then(|text| crate::compat::open_vensim(&text).ok())
        } else {
            crate::compat::open_xmile(&mut BufReader::new(bytes.as_slice())).ok()
        };
        if let Some(project) = project {
            f(&path, &project);
        }
    }
}
