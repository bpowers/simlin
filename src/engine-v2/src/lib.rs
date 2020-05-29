use wasm_bindgen::prelude::*;

use std::io::BufReader;

#[wasm_bindgen]
pub fn from(xmile_xml: &str) -> String {
    let mut reader = BufReader::new(xmile_xml.as_bytes());
    let project = engine_core::Project::from_xmile_reader(&mut reader);

    if let Err(ref err) = project {
        return err.to_string();
    }

    String::from("no problems here")
}
