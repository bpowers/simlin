// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A stock-flow chain: one or more stocks connected by flows.
pub struct StockFlowChain {
    pub stocks: Vec<String>,
    pub flows: Vec<String>,
    pub all_vars: Vec<String>,
    pub importance: f64,
}

/// A dominant period discovered via LTM analysis.
pub struct DominantPeriod {
    pub period: f64,
    pub strength: f64,
}

/// A feedback loop discovered via LTM analysis.
pub struct FeedbackLoop {
    pub name: String,
    pub polarity: LoopPolarity,
    pub variables: Vec<String>,
    pub importance_series: Vec<f64>,
    pub dominant_period: Option<DominantPeriod>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LoopPolarity {
    Reinforcing,
    Balancing,
    Undetermined,
}

impl FeedbackLoop {
    /// The ordered chain of variable names around the loop.
    pub fn causal_chain(&self) -> &[String] {
        &self.variables
    }

    /// Mean of absolute values of the importance time series.
    pub fn average_importance(&self) -> f64 {
        if self.importance_series.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.importance_series.iter().map(|v| v.abs()).sum();
        sum / self.importance_series.len() as f64
    }
}

/// Pre-computed metadata for driving layout.
pub struct ComputedMetadata {
    pub chains: Vec<StockFlowChain>,
    pub feedback_loops: Vec<FeedbackLoop>,
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
    fn test_causal_chain() {
        let fl = FeedbackLoop {
            name: "R1".to_string(),
            polarity: LoopPolarity::Reinforcing,
            variables: vec![
                "population".to_string(),
                "births".to_string(),
                "birth_rate".to_string(),
            ],
            importance_series: vec![],
            dominant_period: None,
        };
        assert_eq!(fl.causal_chain(), &["population", "births", "birth_rate"]);
    }

    #[test]
    fn test_average_importance() {
        let fl = FeedbackLoop {
            name: "B1".to_string(),
            polarity: LoopPolarity::Balancing,
            variables: vec!["a".to_string(), "b".to_string()],
            importance_series: vec![0.5, -0.3, 0.8, -0.4],
            dominant_period: None,
        };
        // abs values: 0.5 + 0.3 + 0.8 + 0.4 = 2.0, mean = 0.5
        let avg = fl.average_importance();
        assert!((avg - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_average_importance_empty() {
        let fl = FeedbackLoop {
            name: "B2".to_string(),
            polarity: LoopPolarity::Undetermined,
            variables: vec![],
            importance_series: vec![],
            dominant_period: None,
        };
        assert!((fl.average_importance() - 0.0).abs() < f64::EPSILON);
    }

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

    #[test]
    fn test_new_empty_metadata() {
        let meta = ComputedMetadata::new_empty();
        assert!(meta.chains.is_empty());
        assert!(meta.feedback_loops.is_empty());
        assert!(meta.dep_graph.is_empty());
        assert!(meta.reverse_dep_graph.is_empty());
        assert!(meta.constants.is_empty());
        assert!(meta.stock_to_inflows.is_empty());
        assert!(meta.stock_to_outflows.is_empty());
        assert!(meta.flow_to_stocks.is_empty());
    }
}
