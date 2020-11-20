use wasm_bindgen::prelude::*;

use std::io::BufReader;

use system_dynamics_engine::{xmile, Project};

#[wasm_bindgen]
pub fn from(xmile_xml: &str) -> String {
    let mut reader = BufReader::new(xmile_xml.as_bytes());
    let datamodel_project = xmile::project_from_reader(&mut reader).unwrap();
    let _project = Project::from(datamodel_project);

    String::from("no problems here")
}
