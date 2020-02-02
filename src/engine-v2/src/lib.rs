use wasm_bindgen::prelude::*;

use engine_core;
use std::error::Error;
use std::io::BufReader;

#[wasm_bindgen]
pub fn from(xmile_xml: &str) -> String {
    let mut reader = BufReader::new(xmile_xml.as_bytes());
    let project = engine_core::Project::from_xmile_reader(&mut reader);

    if let Err(ref err) = project {
        return String::from(err.description());
    }

    String::from("no problems here")
}
