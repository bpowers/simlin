// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::fs;
use std::path::Path;

fn main() {
    let version = fs::read_to_string("pysimlin.version")
        .expect("failed to read pysimlin.version")
        .trim()
        .to_string();

    println!("cargo:rustc-env=PYSIMLIN_VERSION={version}");

    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
    let out = Path::new(&out_dir);

    let templates: &[&str] = &["src/instructions.md", "src/skills/pysimlin-basics.md"];
    for path in templates {
        let content =
            fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
        let processed = content.replace("{PYSIMLIN_VERSION}", &version);
        let filename = Path::new(path)
            .file_name()
            .expect("template path has no filename");
        fs::write(out.join(filename), processed)
            .unwrap_or_else(|e| panic!("failed to write {}: {e}", filename.to_string_lossy()));

        println!("cargo:rerun-if-changed={path}");
    }

    println!("cargo:rerun-if-changed=pysimlin.version");
}
