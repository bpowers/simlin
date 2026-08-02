// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! One-shot helper: give each vensim-probes/*.mdl a generated diagram so the
//! probes open with a visible view in Vensim, WITHOUT touching the equations.
//!
//! The obvious route -- read, lay out, and re-serialize the whole project
//! through the MDL writer -- rewrites the equation section too, and the writer
//! spells an apply-to-all equation per element. That changes exactly what a
//! probe asks Vensim to parse (an element-pinned left-hand side over a
//! right-hand side naming subscript ranges), so instead the generated output is
//! used only as a donor: the sketch block between the `\\\---///` and
//! `///---\\\` markers is spliced into the original file, whose hand-written
//! equation text stays byte-identical. Sketch entries reference variables by
//! name, so a donor sketch over the original equations is well-formed.

use std::fs;

const SKETCH_START: &str = "\\\\\\---///";
const SKETCH_END: &str = "///---\\\\\\";

fn sketch_block(mdl: &str) -> &str {
    let start = mdl.find(SKETCH_START).expect("no sketch start marker");
    let end = mdl.find(SKETCH_END).expect("no sketch end marker") + SKETCH_END.len();
    &mdl[start..end]
}

fn main() {
    for path in [
        "vensim-probes/elm_map_computed_source.mdl",
        "vensim-probes/repeated_dimension.mdl",
    ] {
        let original = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let mut project = simlin_engine::compat::open_vensim(&original)
            .unwrap_or_else(|e| panic!("open {path}: {e}"));
        let model_name = project.models[0].name.clone();
        let view = simlin_engine::layout::generate_best_layout(&project, &model_name, None)
            .unwrap_or_else(|e| panic!("layout {path}: {e}"));
        project.models[0].views = vec![simlin_engine::datamodel::View::StockFlow(view)];
        let (rendered, warnings) = simlin_engine::compat::to_mdl_with_warnings(&project)
            .unwrap_or_else(|e| panic!("render {path}: {e}"));
        for w in &warnings {
            eprintln!("warning ({path}): {}", w.message);
        }

        let donor = sketch_block(&rendered);
        let start = original.find(SKETCH_START).expect("no sketch in original");
        let end = original
            .find(SKETCH_END)
            .expect("no sketch end in original")
            + SKETCH_END.len();
        let spliced = format!("{}{}{}", &original[..start], donor, &original[end..]);
        assert_eq!(
            &spliced[..start],
            &original[..start],
            "equation text must be untouched"
        );
        fs::write(path, spliced).unwrap_or_else(|e| panic!("write {path}: {e}"));
        println!("spliced sketch into {path}");
    }
}
