// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

/// Layout configuration matching the Go `layoutConfig` struct from Praxis.
///
/// All spacing and dimension values are in logical layout units (not pixels).
#[derive(Clone, Debug)]
pub struct LayoutConfig {
    // Spacing between elements
    /// Horizontal space between stocks and flows in a chain.
    pub horizontal_spacing: f64,
    /// Vertical space between different chains (lanes).
    pub vertical_spacing: f64,

    // Element dimensions (layout-specific, not rendering)
    pub stock_width: f64,
    pub stock_height: f64,
    pub flow_width: f64,
    pub flow_height: f64,
    pub cloud_width: f64,
    pub cloud_height: f64,

    // Canvas positioning
    /// Starting X position for first chain.
    pub start_x: f64,
    /// Starting Y position for first chain.
    pub start_y: f64,

    /// Number of parallel layout attempts to generate.
    pub parallel_attempts: usize,

    // Feedback loop visualization
    /// How much to curve connectors in feedback loops (0.0-1.0).
    pub loop_curvature_factor: f64,

    // Simulated annealing parameters for crossing reduction
    /// Max iterations for annealing per round.
    pub annealing_iterations: usize,
    /// Initial temperature for annealing.
    pub annealing_temperature: f64,
    /// Cooling factor per iteration (multiplicative).
    pub annealing_cooling_rate: f64,
    /// Iterations between reheating within a single annealing run.
    pub annealing_reheat_period: usize,
    /// Random seed for deterministic annealing.
    pub annealing_random_seed: u64,
    /// SFDP iterations between annealing rounds.
    pub annealing_interval: usize,
    /// Maximum number of annealing rounds per SFDP run.
    pub annealing_max_rounds: usize,
    /// Temperature to reset to when reheating between rounds.
    /// Zero signals dynamic reheating using the initial temperature.
    pub annealing_reheat_temperature: f64,
    /// Maximum auxiliary displacement from annealing baseline.
    pub annealing_max_delta_aux: f64,
    /// Maximum chain displacement from annealing baseline.
    pub annealing_max_delta_chain: f64,
    /// Scales average edge length to compute initial temperature.
    pub annealing_temperature_scale: f64,

    /// Enable verbose debug logging.
    pub debug: bool,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            horizontal_spacing: 100.0,
            vertical_spacing: 150.0,
            stock_width: 45.0,
            stock_height: 35.0,
            flow_width: 12.0,
            flow_height: 12.0,
            cloud_width: 20.0,
            cloud_height: 20.0,
            start_x: 100.0,
            start_y: 100.0,
            parallel_attempts: 4,
            loop_curvature_factor: 0.3,
            annealing_iterations: 200,
            annealing_temperature: 30.0,
            annealing_cooling_rate: 0.995,
            annealing_reheat_period: 12,
            annealing_random_seed: 42,
            annealing_interval: 200,
            annealing_max_rounds: 6,
            annealing_reheat_temperature: 0.0,
            annealing_max_delta_aux: 200.0,
            annealing_max_delta_chain: 25.0,
            annealing_temperature_scale: 0.4,
            debug: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = LayoutConfig::default();

        // Element dimensions
        assert!((config.stock_width - 45.0).abs() < f64::EPSILON);
        assert!((config.stock_height - 35.0).abs() < f64::EPSILON);
        assert!((config.flow_width - 12.0).abs() < f64::EPSILON);
        assert!((config.flow_height - 12.0).abs() < f64::EPSILON);
        assert!((config.cloud_width - 20.0).abs() < f64::EPSILON);
        assert!((config.cloud_height - 20.0).abs() < f64::EPSILON);

        // Spacing
        assert!((config.horizontal_spacing - 100.0).abs() < f64::EPSILON);
        assert!((config.vertical_spacing - 150.0).abs() < f64::EPSILON);

        // Canvas positioning
        assert!((config.start_x - 100.0).abs() < f64::EPSILON);
        assert!((config.start_y - 100.0).abs() < f64::EPSILON);

        // Parallel layout
        assert_eq!(config.parallel_attempts, 4);

        // Feedback loop
        assert!((config.loop_curvature_factor - 0.3).abs() < f64::EPSILON);

        // Annealing parameters
        assert_eq!(config.annealing_iterations, 200);
        assert!((config.annealing_temperature - 30.0).abs() < f64::EPSILON);
        assert!((config.annealing_cooling_rate - 0.995).abs() < f64::EPSILON);
        assert_eq!(config.annealing_reheat_period, 12);
        assert_eq!(config.annealing_random_seed, 42);
        assert_eq!(config.annealing_interval, 200);
        assert_eq!(config.annealing_max_rounds, 6);
        assert!((config.annealing_reheat_temperature - 0.0).abs() < f64::EPSILON);
        assert!((config.annealing_max_delta_aux - 200.0).abs() < f64::EPSILON);
        assert!((config.annealing_max_delta_chain - 25.0).abs() < f64::EPSILON);
        assert!((config.annealing_temperature_scale - 0.4).abs() < f64::EPSILON);

        // Debug
        assert!(!config.debug);
    }
}
