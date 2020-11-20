// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// because of our creative use of modules below, rustc wants to
// warn about unused code in common.  Ignore those warnings here.
#![allow(dead_code, unused_macros)]

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

#[macro_use]
mod common {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/common.rs"));
}
mod datamodel {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/datamodel.rs"));
}
mod xmile {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/xmile.rs"));
}

const PROTOS: &[&str] = &["src/project_io.proto"];

const XMILE_SUFFIX: &str = ".stmx";

fn build_stdlib() -> io::Result<()> {
    let crate_dir = env::var_os("CARGO_MANIFEST_DIR").unwrap();
    let stdlib_path = Path::new(&crate_dir).join("../../stdlib");

    let mut stdlib_model_paths = fs::read_dir(stdlib_path)?
        .map(|dent| dent.map(|e| e.path()))
        .collect::<Result<Vec<_>, io::Error>>()?;

    // ensure a stable order
    stdlib_model_paths.sort();

    // build a list of (model_name, xmile::File) pairs for the stdlib
    let files: Vec<(String, xmile::File)> = stdlib_model_paths
        .iter()
        .map(|path| {
            (
                // extract "model_name" from "blah/../model_name.stmx"
                String::from(
                    path.file_name()
                        .unwrap()
                        .to_string_lossy()
                        .strip_suffix(XMILE_SUFFIX)
                        .unwrap(),
                ),
                io::BufReader::new(fs::File::open(path).unwrap()),
            )
        })
        .map(|(file, reader)| (file, quick_xml::de::from_reader(reader).unwrap()))
        .collect();

    // integrity checks
    for (_, file) in files.iter() {
        // we expect a single model in each stdlib xmile file.
        // If we see something else, exit loudly.
        assert!(file.models.len() == 1);
    }

    // we don't want to serialize the whole xmile::File, just the models
    let models: Vec<(String, datamodel::Model)> = files
        .iter()
        .map(|(name, f)| (name.clone(), datamodel::Model::from(f.models[0].clone())))
        .map(|(name, mut m)| {
            // this was the 1 hand-edit we did to stmx files, so lets just
            // automate adding it here when embedding them in the library
            m.name = format!("stdlib·{}", name);
            (name, m)
        })
        .collect();

    // write the serialized binary models to temporary files
    let out_dir = env::var_os("OUT_DIR").unwrap();
    for (model_name, model) in models.iter() {
        let dest_path = Path::new(&out_dir).join(model_name.to_owned() + ".bin");

        let writer = io::BufWriter::new(fs::File::create(dest_path).unwrap());
        bincode::serialize_into(writer, model).unwrap();

        let serialized = bincode::serialize(model).unwrap();
        let model2: datamodel::Model = bincode::deserialize(serialized.as_slice()).unwrap();

        // check that roundtripping through bincode is lossless
        assert!(*model == model2);
    }

    // then materialize the contents of the crate::stdlib module, which provides
    // access to the xmile::Model's on demand
    write_stdlib_module(models)
}

fn write_stdlib_module(models: Vec<(String, datamodel::Model)>) -> io::Result<()> {
    let out_dir = env::var_os("OUT_DIR").unwrap();

    let dest_path = Path::new(&out_dir).join("stdlib.rs");
    let mut writer = io::BufWriter::new(fs::File::create(dest_path).unwrap());

    writeln!(
        writer,
        "use crate::datamodel;

pub const MODEL_NAMES: [&str; {}] = [",
        models.len()
    )
    .unwrap();

    for (model_name, _) in models.iter() {
        writeln!(writer, "    \"{}\",", model_name).unwrap();
    }

    writeln!(
        writer,
        "];

fn hydrate(bytes: &[u8]) -> Option<datamodel::Model> {{
    Some(bincode::deserialize(bytes).unwrap())
}}

pub fn get(name: &str) -> Option<datamodel::Model> {{
    match name {{"
    )
    .unwrap();

    for (model_name, _) in models.iter() {
        writeln!(
            writer,
            "        \"{}\" => hydrate(include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{}.bin\"))),",
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
