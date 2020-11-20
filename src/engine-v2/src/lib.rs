use wasm_bindgen::prelude::*;

use std::io::BufReader;

#[wasm_bindgen]
pub fn from(xmile_xml: &str) -> String {
    let _project =
        system_dynamics_compat::open_xmile(&mut BufReader::new(xmile_xml.as_bytes())).unwrap();

    String::from("no problems here")
}
