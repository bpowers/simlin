// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

const PROTOS: &[&str] = &["src/project_io.proto"];

const PB_SUFFIX: &str = ".pb";

fn build_stdlib() -> io::Result<()> {
    let crate_dir = env::var_os("CARGO_MANIFEST_DIR").unwrap();
    let stdlib_path = Path::new(&crate_dir).join("src/stdlib");

    let mut stdlib_model_paths = fs::read_dir(stdlib_path)?
        .map(|dent| dent.map(|e| e.path()))
        .collect::<Result<Vec<_>, io::Error>>()?;

    // ensure a stable order
    stdlib_model_paths.sort();

    // build a list of model_names for the stdlib
    let models: Vec<String> = stdlib_model_paths
        .iter()
        .map(|path| {
            // extract "model_name" from "blah/../model_name.stmx"
            String::from(
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .strip_suffix(PB_SUFFIX)
                    .unwrap(),
            )
        })
        .collect();

    // then materialize the contents of the crate::stdlib module, which provides
    // access to the datamodel::Model's on demand
    write_stdlib_module(&models)
}

fn write_stdlib_module(models: &[String]) -> io::Result<()> {
    let out_dir = env::var_os("OUT_DIR").unwrap();

    let dest_path = Path::new(&out_dir).join("stdlib.rs");
    let mut writer = io::BufWriter::new(fs::File::create(dest_path).unwrap());

    writeln!(
        writer,
        "use prost::Message;

use crate::{{datamodel, project_io}};

pub const MODEL_NAMES: [&str; {}] = [",
        models.len()
    )
    .unwrap();

    for model_name in models.iter() {
        writeln!(writer, "    \"{}\",", model_name).unwrap();
    }

    writeln!(
        writer,
        "];

fn hydrate(bytes: &[u8]) -> datamodel::Model {{
    let model = project_io::Model::decode(bytes).unwrap();
    datamodel::Model::from(model)
}}

pub fn get(name: &str) -> Option<datamodel::Model> {{
    match name {{"
    )
    .unwrap();

    for model_name in models.iter() {
        writeln!(
            writer,
            "        \"{}\" => Some(hydrate(include_bytes!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/src/stdlib/{}.pb\")))),",
            model_name, model_name
        )
        .unwrap();
    }

    writeln!(
        writer,
        "        _ => None,
    }}
}}
"
    )?;

    Ok(())
}

fn main() {
    let mut prost_build = prost_build::Config::new();
    prost_build.protoc_arg("--experimental_allow_proto3_optional");
    prost_build.compile_protos(PROTOS, &["src/"]).unwrap();
    lalrpop::process_root().unwrap();

    build_stdlib().unwrap();
}
