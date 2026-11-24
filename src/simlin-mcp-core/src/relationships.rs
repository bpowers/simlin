// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
//! The relationships an SD-AI file states, from the polarities of the model's
//! causal links.

use std::collections::{HashMap, HashSet};

use simlin_base::datamodel;
use simlin_engine::ltm::LinkPolarity;
use simlin_formats::json_sdai::{Polarity, Relationship};

/// Generate SD-AI relationships from pre-computed equation dependency
/// polarities.
///
/// Takes the polarity map produced by `compute_link_polarities()` and the
/// datamodel, filters out stock-flow structural edges (which the SD-AI
/// conformance evaluator generates independently), and maps remaining
/// equation-derived edges to `Relationship` values with computed polarity.
pub(crate) fn generate_relationships(
    polarities: &HashMap<(String, String), LinkPolarity>,
    model: &datamodel::Model,
) -> Vec<Relationship> {
    // Map canonical identifiers back to authored variable names so the
    // relationships array matches the names in the variables array.
    let canonical_to_authored: HashMap<String, &str> = model
        .variables
        .iter()
        .map(|v| {
            let authored = v.get_ident();
            (
                simlin_base::common::canonicalize(authored).into_owned(),
                authored,
            )
        })
        .collect();

    // Stock-flow structural edges are excluded because the conformance
    // evaluator generates them itself via makeRelationshipsFromStocks().
    // Identifiers are canonicalized so that mixed-case names in the datamodel
    // (e.g. stock "Population" with inflow "Births") correctly match the
    // lowercase canonical keys produced by compute_link_polarities().
    let mut stock_flow_edges: HashSet<(String, String)> = HashSet::new();
    for var in &model.variables {
        if let datamodel::Variable::Stock(stock) = var {
            let stock_ident = simlin_base::common::canonicalize(&stock.ident).into_owned();
            for inflow in &stock.inflows {
                stock_flow_edges.insert((
                    simlin_base::common::canonicalize(inflow).into_owned(),
                    stock_ident.clone(),
                ));
            }
            for outflow in &stock.outflows {
                stock_flow_edges.insert((
                    simlin_base::common::canonicalize(outflow).into_owned(),
                    stock_ident.clone(),
                ));
            }
        }
    }

    let mut relationships: Vec<Relationship> = polarities
        .iter()
        .filter(|((from, to), _)| !stock_flow_edges.contains(&(from.clone(), to.clone())))
        // Only include edges where both endpoints are declared model
        // variables.  This excludes simulator builtins (TIME, DT) and
        // implicit compiler nodes ($-prefixed SMOOTH/DELAY expansions).
        .filter_map(|((from, to), polarity)| {
            let authored_from = canonical_to_authored.get(from)?;
            let authored_to = canonical_to_authored.get(to)?;
            Some(Relationship {
                reasoning: None,
                from: authored_from.to_string(),
                to: authored_to.to_string(),
                polarity: match polarity {
                    LinkPolarity::Positive => Polarity::Positive,
                    LinkPolarity::Negative => Polarity::Negative,
                    LinkPolarity::Unknown => Polarity::Unknown,
                },
                polarity_reasoning: None,
            })
        })
        .collect();

    relationships.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
    relationships
}

#[cfg(test)]
#[path = "relationships_tests.rs"]
mod relationships_tests;
