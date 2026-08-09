// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::ltm_dominance::{DominantPeriod, FeedbackLoop};

/// A stock-flow chain: one or more stocks connected by flows.
#[derive(Clone, serde::Serialize)]
pub struct StockFlowChain {
    pub stocks: Vec<String>,
    pub flows: Vec<String>,
    pub all_vars: Vec<String>,
    pub importance: f64,
}

/// Pre-computed metadata for driving layout.
#[derive(Clone, serde::Serialize)]
pub struct ComputedMetadata {
    pub chains: Vec<StockFlowChain>,
    pub feedback_loops: Vec<FeedbackLoop>,
    pub dominant_periods: Vec<DominantPeriod>,
    pub dep_graph: BTreeMap<String, BTreeSet<String>>,
    pub reverse_dep_graph: BTreeMap<String, BTreeSet<String>>,
    pub constants: BTreeSet<String>,
    pub stock_to_inflows: HashMap<String, Vec<String>>,
    pub stock_to_outflows: HashMap<String, Vec<String>>,
    pub flow_to_stocks: HashMap<String, (Option<String>, Option<String>)>,
}

impl ComputedMetadata {
    pub fn new_empty() -> Self {
        Self {
            chains: Vec::new(),
            feedback_loops: Vec::new(),
            dominant_periods: Vec::new(),
            dep_graph: BTreeMap::new(),
            reverse_dep_graph: BTreeMap::new(),
            constants: BTreeSet::new(),
            stock_to_inflows: HashMap::new(),
            stock_to_outflows: HashMap::new(),
            flow_to_stocks: HashMap::new(),
        }
    }

    /// Check if a variable is a constant (no dependencies).
    pub fn is_constant(&self, ident: &str) -> bool {
        self.constants.contains(ident)
    }

    /// Get the stocks connected by a flow: (from_stock, to_stock).
    pub fn connected_stocks(&self, flow_ident: &str) -> (Option<&str>, Option<&str>) {
        self.flow_to_stocks
            .get(flow_ident)
            .map(|(from, to)| (from.as_deref(), to.as_deref()))
            .unwrap_or((None, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_constant() {
        let mut meta = ComputedMetadata::new_empty();
        meta.constants.insert("gravity".to_string());
        meta.constants.insert("pi".to_string());

        assert!(meta.is_constant("gravity"));
        assert!(meta.is_constant("pi"));
        assert!(!meta.is_constant("population"));
    }

    #[test]
    fn test_connected_stocks() {
        let mut meta = ComputedMetadata::new_empty();
        meta.flow_to_stocks.insert(
            "birth_flow".to_string(),
            (None, Some("population".to_string())),
        );
        meta.flow_to_stocks.insert(
            "transfer".to_string(),
            (Some("source".to_string()), Some("sink".to_string())),
        );

        let (from, to) = meta.connected_stocks("birth_flow");
        assert_eq!(from, None);
        assert_eq!(to, Some("population"));

        let (from, to) = meta.connected_stocks("transfer");
        assert_eq!(from, Some("source"));
        assert_eq!(to, Some("sink"));

        let (from, to) = meta.connected_stocks("nonexistent");
        assert_eq!(from, None);
        assert_eq!(to, None);
    }
}
