use wasm_bindgen::prelude::*;

use std::io::BufReader;

#[wasm_bindgen]
pub fn from(xmile_xml: &str) -> String {
    let mut reader = BufReader::new(xmile_xml.as_bytes());
    let datamodel_project = engine_core::xmile::project_from_reader(&mut reader).unwrap();
    let _project = engine_core::Project::from(datamodel_project);

    String::from("no problems here")
}
