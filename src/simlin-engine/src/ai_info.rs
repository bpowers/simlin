// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::canonicalize;
use crate::datamodel::{AiInformation, AiState, Equation, Model, Module, Project, Variable};
use ed25519::Signature;
use ed25519::signature::Verifier;

pub fn verify<V: Verifier<Signature>>(
    project: &Project,
    verifier: &V,
) -> Result<(), ed25519::Error> {
    let ai_info = project.ai_information.as_ref().unwrap();

    let signed_msg_body = build_signed_message_body(ai_info, project);

    if let Some(ref testing) = ai_info.testing {
        let debug_log = testing.signed_message_body.as_str();
        assert_eq!(debug_log, signed_msg_body);
    }

    let sig_encoded = ai_info.status.signature.as_bytes();

    use base64::{Engine as _, engine::general_purpose};
    let sig_decoded = general_purpose::STANDARD.decode(sig_encoded).unwrap();
    let sig_bytes: [u8; 64] = sig_decoded.as_slice().try_into().unwrap();
    let sig = Signature::from_bytes(&sig_bytes);

    verifier.verify(signed_msg_body.as_bytes(), &sig)
}

fn escape(s: &str) -> String {
    s.replace(" ", "")
        .replace("_", "")
        .replace("\n", "")
        .replace("\\n", "")
}

fn build_signed_message_body(ai_info: &AiInformation, project: &Project) -> String {
    let mut s = String::new();

    let mut all_tags = ai_info.status.tags.clone();
    all_tags.insert("algorithm".into(), ai_info.status.algorithm.clone());
    all_tags.insert("keyurl".into(), ai_info.status.key_url.clone());

    let mut sorted_keys = all_tags.keys().map(|k| k.as_str()).collect::<Vec<&str>>();
    sorted_keys.sort();

    let sample: i32 = all_tags
        .get("sampling")
        .unwrap_or(&"1".to_string())
        .parse()
        .unwrap();

    let mut initial = true;
    for key in sorted_keys {
        if initial {
            initial = false;
        } else {
            s.push(' ');
        }
        // e.g. want_ver_info -> wantvarinfo
        s.push_str(escape(key).as_str());
        s.push('=');
        s.push_str(all_tags.get(key).unwrap());
    }

    for model in project.models.iter() {
        let mut prefix = " ".to_string();
        if model.name != "main" {
            prefix = format!(" {}.", escape(model.name.as_str()));
        }
        append_model(&mut s, &prefix, 0, sample, model);
    }

    for model in project.models.iter() {
        append_ai_state(&mut s, true, project, model);
    }

    if let Some(log) = &ai_info.log {
        s.push(' ');
        s.push_str(log.replace(" ", "").as_str());
    }

    s
}

fn is_module(v: &Variable) -> bool {
    if let Variable::Module(_) = v {
        true
    } else {
        false
    }
}

fn append_model(s: &mut String, prefix: &str, mut i: i32, sample: i32, model: &Model) -> i32 {
    let mut var_names = model
        .variables
        .iter()
        .map(|v| v.get_ident())
        .collect::<Vec<_>>();
    var_names.sort_by_cached_key(|v| canonicalize(v));

    for var_name in var_names.iter() {
        let var = model.get_variable(var_name).unwrap();
        let name = escape(var.get_ident());

        if i % sample == 0 && !is_module(&var) {
            s.push_str(prefix);
            s.push_str(&name);
            match var.get_equation() {
                Some(Equation::Scalar(eqn, ..)) | Some(Equation::ApplyToAll(_, eqn, _)) => {
                    s.push('=');
                    let eqn = escape(eqn);
                    s.push_str(&eqn);
                }
                Some(Equation::Arrayed(_, eqns)) => {
                    s.push('=');
                    for (_, eqn, _) in eqns.iter() {
                        let eqn = escape(eqn);
                        s.push_str(&eqn);
                    }
                }
                None => {}
            }
        }

        i += 1;
    }

    i
}

fn append_ai_state(s: &mut String, mut need_initial_space: bool, project: &Project, model: &Model) {
    let mut var_names = model
        .variables
        .iter()
        .map(|v| v.get_ident())
        .collect::<Vec<_>>();
    var_names.sort_by_cached_key(|v| canonicalize(v));

    for var_name in var_names.iter() {
        let var = model.get_variable(var_name).unwrap();
        if let Some(ai_state) = var.get_ai_state() {
            if need_initial_space {
                need_initial_space = false;
                s.push(' ');
            }
            s.push_str(ai_state_to_letter(ai_state));
        }

        if let Variable::Module(Module { model_name, .. }) = var {
            if let Some(model) = project.get_model(model_name) {
                append_ai_state(s, need_initial_space, project, model);
            }
        }
    }
}

#[allow(dead_code)]
fn ai_state_to_letter(ai_state: AiState) -> &'static str {
    match ai_state {
        AiState::A => "A",
        AiState::B => "B",
        AiState::C => "C",
        AiState::D => "D",
        AiState::E => "E",
        AiState::F => "F",
        AiState::G => "G",
        AiState::H => "H",
    }
}
