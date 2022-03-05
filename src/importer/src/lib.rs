// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// FIXME: remove when wasm-bindgen is updated past 0.2.79
#![allow(clippy::unused_unit)]

use std::io::BufReader;

use wasm_bindgen::prelude::*;

use simlin_compat::engine::{self, project_io, serde};
use simlin_compat::{open_xmile, prost, to_xmile as compat_to_xmile};

#[wasm_bindgen]
pub fn from_xmile(xmile_xml: &str) -> Box<[u8]> {
    use prost::Message;
    let project = match open_xmile(&mut BufReader::new(xmile_xml.as_bytes())) {
        Ok(project) => project,
        Err(err) => panic!("open_xmile failed: {}", err),
    };
    let project_pb = engine::serde::serialize(&project);

    let mut buf: Vec<u8> = Vec::with_capacity(project_pb.encoded_len() + 8);
    match project_pb.encode(&mut buf) {
        Ok(_) => {}
        Err(err) => panic!("encode failed: {}", err),
    };

    buf.into_boxed_slice()
}

#[wasm_bindgen]
pub fn to_xmile(project_pb: &[u8]) -> Option<String> {
    use prost::Message;

    let project = match project_io::Project::decode(project_pb) {
        Ok(project) => serde::deserialize(project),
        Err(_err) => {
            return None;
        }
    };

    compat_to_xmile(&project).ok()
}

// #[wasm_bindgen]
// pub fn from_vensim(xmile_xml: &str) -> Box<[u8]> {
//     use simlin_compat::open_xmile;
//     use prost::Message;
//     let project = open_vensim(&mut BufReader::new(xmile_xml.as_bytes())).unwrap();
//     let project_pb = engine::serde::serialize(&project);
//
//     let mut buf: Vec<u8> = Vec::with_capacity(project_pb.encoded_len() + 8);
//     project_pb
//         .encode(&mut buf)
//         .unwrap();
//
//     buf.into_boxed_slice()
// }
