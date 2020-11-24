use wasm_bindgen::prelude::*;

use system_dynamics_engine as engine;
use system_dynamics_engine::{project_io, prost, serde};

#[wasm_bindgen]
pub struct Project {
    #[allow(dead_code)]
    project: engine::Project,
}

#[wasm_bindgen]
pub fn open(project_pb: &[u8]) -> Project {
    use prost::Message;
    let project = match project_io::Project::decode_length_delimited(project_pb) {
        Ok(project) => serde::deserialize(project),
        Err(err) => panic!("encode_length_delimited failed: {}", err),
    };

    Project {
        project: engine::Project::from(project),
    }
}

// #[wasm_bindgen]
// pub fn from_vensim(xmile_xml: &str) -> Box<[u8]> {
//     use system_dynamics_compat::open_xmile;
//     use prost::Message;
//     let project = open_vensim(&mut BufReader::new(xmile_xml.as_bytes())).unwrap();
//     let project_pb = engine::serde::serialize(&project);
//
//     let mut buf: Vec<u8> = Vec::with_capacity(project_pb.encoded_len() + 8);
//     project_pb
//         .encode_length_delimited(&mut buf)
//         .unwrap();
//
//     buf.into_boxed_slice()
// }
