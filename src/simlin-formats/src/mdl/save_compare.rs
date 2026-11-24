// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Two projects compared as an MDL save keeps them.

use std::collections::BTreeMap;

use simlin_base::ast::{Expr0, print_eqn};
use simlin_base::datamodel::{Equation, Project, Variable};
use simlin_base::lexer::LexerType;

/// An equation's text as its parsed tree prints, so two spellings of one
/// tree compare equal: the reader keeps the parentheses a file writes, and
/// the writer writes the fewest the tree needs. Text that does not parse is
/// compared as written, trimmed.
fn canonical_text(text: &str) -> String {
    match Expr0::new(text, LexerType::Equation) {
        Ok(Some(expr)) => print_eqn(&expr),
        _ => text.trim().to_owned(),
    }
}

fn canonical_equation(equation: &Equation) -> Equation {
    match equation {
        Equation::Scalar(text) => Equation::Scalar(canonical_text(text)),
        Equation::ApplyToAll(dims, text) => {
            Equation::ApplyToAll(dims.clone(), canonical_text(text))
        }
        Equation::Arrayed(dims, slots, default, applies) => Equation::Arrayed(
            dims.clone(),
            slots
                .iter()
                .map(|(key, text, initial, gf)| {
                    (
                        key.clone(),
                        canonical_text(text),
                        initial.as_deref().map(canonical_text),
                        gf.clone(),
                    )
                })
                .collect(),
            // The reader keeps the default of an `:EXCEPT:` equation that is
            // its variable's only equation without applying it; nothing reads
            // a default that does not apply.
            default.as_deref().filter(|_| *applies).map(canonical_text),
            *applies,
        ),
    }
}

/// `project` without what an MDL save is known not to keep:
///
/// - the views, which the first save lays out anew (a flow label's own
///   position, a connector's point it no longer reads as the link's shape);
///   `writer_output_idempotence_ratchet` holds the saved sketch to a fixed
///   point instead;
/// - an equation's spelling, compared as its parsed tree ([`canonical_text`]);
/// - an `:EXCEPT:` default the variable does not apply;
/// - the order of a model's variables, which a save writes by group.
pub fn normalized(project: &Project) -> Project {
    let mut project = project.clone();
    for model in &mut project.models {
        model.views.clear();
        model.variables.rewrite(|variables| {
            variables.sort_by(|a, b| a.get_ident().cmp(b.get_ident()));
            for variable in variables.iter_mut() {
                let equation = match variable {
                    Variable::Stock(v) => Some(&mut v.equation),
                    Variable::Flow(v) => Some(&mut v.equation),
                    Variable::Aux(v) => Some(&mut v.equation),
                    Variable::Module(_) => None,
                };
                if let Some(equation) = equation {
                    *equation = canonical_equation(equation);
                }
            }
        });
    }
    project
}

/// The first difference between `a` and `b`, by where it is.
pub fn first_difference(a: &Project, b: &Project) -> Option<String> {
    if a.sim_specs != b.sim_specs {
        return Some(format!("sim specs {:?} -> {:?}", a.sim_specs, b.sim_specs));
    }
    if a.dimensions != b.dimensions {
        return Some(format!(
            "dimensions {:?} -> {:?}",
            a.dimensions, b.dimensions
        ));
    }
    if a.units != b.units {
        return Some("unit equivalences".to_owned());
    }
    if a.models.len() != b.models.len() {
        return Some(format!("{} models -> {}", a.models.len(), b.models.len()));
    }
    for (ma, mb) in a.models.iter().zip(&b.models) {
        let vars = |m: &simlin_base::datamodel::Model| -> BTreeMap<String, Variable> {
            m.variables
                .iter()
                .map(|v| (v.get_ident().to_owned(), v.clone()))
                .collect()
        };
        let (va, vb) = (vars(ma), vars(mb));
        for (ident, var) in &va {
            match vb.get(ident) {
                None => return Some(format!("{}: '{ident}' is not in the save", ma.name)),
                Some(other) if other != var => {
                    return Some(format!("{}: '{ident}' {var:?} -> {other:?}", ma.name));
                }
                Some(_) => {}
            }
        }
        if let Some(ident) = vb.keys().find(|ident| !va.contains_key(*ident)) {
            return Some(format!("{}: '{ident}' is new in the save", ma.name));
        }
        if ma != mb {
            return Some(format!("{}: the model's other fields", ma.name));
        }
    }
    (a != b).then(|| "the project's other fields".to_owned())
}
