// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

extern crate lalrpop;
extern crate bincode;

mod xmile {
    include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/xmile.rs"));
}

use std::env;
use std::io;
use std::fs;
use std::path::Path;
use std::io::Write;

const XMILE_SUFFIX: &str = ".stmx";

fn build_stdlib() -> io::Result<()> {
    let crate_dir = env::var_os("CARGO_MANIFEST_DIR").unwrap();
    let stdlib_path = Path::new(&crate_dir).join("../../stdlib");

    let mut entries = fs::read_dir(stdlib_path)?
        .map(|dent| dent.map(|e| e.path()))
        .collect::<Result<Vec<_>, io::Error>>()?;

    // ensure a stable order
    entries.sort();

    use quick_xml::de;

    let files: Vec<(String, xmile::File)> = entries.iter()
        .map(|path| (String::from(path.file_name().unwrap().to_string_lossy().strip_suffix(XMILE_SUFFIX).unwrap()), io::BufReader::new(fs::File::open(path).unwrap())))
        .map(|(file, reader)| (file, de::from_reader(reader).unwrap()))
        .collect();

    // write the binary serialized models to temporary files
    let out_dir = env::var_os("OUT_DIR").unwrap();
    for (model_name, file) in files.iter() {
        let dest_path = Path::new(&out_dir).join(model_name.to_owned() + ".bin");

        let writer = io::BufWriter::new(fs::File::create(dest_path).unwrap());
        bincode::serialize_into(writer, file).unwrap();
    }

    // then write the contentx of the stdlib.rs module
    let dest_path = Path::new(&out_dir).join("stdlib.rs");
    let mut writer = io::BufWriter::new(fs::File::create(dest_path).unwrap());

    writeln!(writer, "use crate::xmile;

fn hydrate(bytes: &[u8]) -> Option<xmile::File> {{
    Some(bincode::deserialize(bytes).unwrap())
}}

pub fn get(name: &str) -> Option<xmile::File> {{
    match name {{").unwrap();

    for (model_name, _) in files.iter() {
        writeln!(writer, "        \"{}\" => hydrate(include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{}.bin\"))),", model_name, model_name).unwrap();
    }

    writeln!(writer, "        _ => None,
    }}
}}
")?;

    Ok(())
}

fn main() {
    prost_build::compile_protos(&["src/ast_io.proto"], &["src/"]).unwrap();
    lalrpop::process_root().unwrap();

    build_stdlib().unwrap();
}
