// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Import every `.mdl` under `test/` and report which ones fail, and why.
//!
//! Prints one `IMPORT-FAIL` / `COMPILE-FAIL` line per failing model with its
//! diagnostics, and a summary to stderr. Diffing two runs is how a change to the
//! MDL importer is shown not to regress the corpus: the apply-to-all import fix
//! moved exactly one model (`sdeverywhere/models/vector/vector.mdl`, which had
//! been failing codegen on `y`'s dimension arithmetic) from fail to ok, and
//! moved none the other way, across 262 files.
//!
//! The remaining failures are pre-existing and unrelated -- unimplemented Vensim
//! builtins dominate -- so the summary counts are a ratchet, not a target.
use std::path::{Path, PathBuf};
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "mdl") {
            out.push(p);
        }
    }
}
fn main() {
    let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../test"));
    let mut files = Vec::new();
    walk(&root, &mut files);
    files.sort();
    let (mut ok, mut import_err, mut compile_err) = (0, 0, 0);
    for f in &files {
        let rel = f.strip_prefix(&root).unwrap_or(f).display().to_string();
        let Ok(contents) = std::fs::read_to_string(f) else {
            continue;
        };
        let dm = match simlin_engine::open_vensim(&contents) {
            Ok(d) => d,
            Err(e) => {
                import_err += 1;
                println!("IMPORT-FAIL\t{rel}\t{e}");
                continue;
            }
        };
        // Use the production incremental path: sync + collect diagnostics.
        let mut db = simlin_engine::db::SimlinDb::default();
        let sync = simlin_engine::db::sync_from_datamodel_incremental(&mut db, &dm, None);
        let diags = simlin_engine::db::collect_all_diagnostics(&db, sync.project);
        let mut msgs: Vec<String> = diags
            .iter()
            .filter(|d| d.severity == simlin_engine::db::DiagnosticSeverity::Error)
            .map(|d| format!("{}:{:?}", d.variable.as_deref().unwrap_or("-"), d.error))
            .collect();
        msgs.sort();
        msgs.dedup();
        if msgs.is_empty() {
            ok += 1;
        } else {
            compile_err += 1;
            println!("COMPILE-FAIL\t{rel}\t{}", msgs.join(" | "));
        }
    }
    eprintln!(
        "total={} ok={ok} import_err={import_err} compile_err={compile_err}",
        files.len()
    );
}
