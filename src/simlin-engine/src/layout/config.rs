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
    pub aux_width: f64,
    pub aux_height: f64,
    pub cloud_width: f64,
    pub cloud_height: f64,

    // Canvas positioning
    /// Starting X position for first chain.
    pub start_x: f64,
    /// Starting Y position for first chain.
    pub start_y: f64,

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
    /// Scales average edge length to compute initial temperature.
    pub annealing_temperature_scale: f64,
}

impl LayoutConfig {
    /// Clamp fields to physically meaningful ranges so that nonsensical
    /// caller-provided values (e.g. negative dimensions or cooling rate
    /// above 1.0) don't produce undefined layout behavior.
    pub fn validate(&mut self) {
        self.horizontal_spacing = self.horizontal_spacing.max(1.0);
        self.vertical_spacing = self.vertical_spacing.max(1.0);
        self.stock_width = self.stock_width.max(1.0);
        self.stock_height = self.stock_height.max(1.0);
        self.flow_width = self.flow_width.max(1.0);
        self.flow_height = self.flow_height.max(1.0);
        self.aux_width = self.aux_width.max(1.0);
        self.aux_height = self.aux_height.max(1.0);
        self.cloud_width = self.cloud_width.max(1.0);
        self.cloud_height = self.cloud_height.max(1.0);
        self.annealing_cooling_rate = self.annealing_cooling_rate.clamp(0.0, 1.0);
        self.loop_curvature_factor = self.loop_curvature_factor.clamp(0.0, 1.0);
    }
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
            aux_width: 18.0,
            aux_height: 18.0,
            cloud_width: 20.0,
            cloud_height: 20.0,
            start_x: 100.0,
            start_y: 100.0,
            loop_curvature_factor: 0.3,
            annealing_iterations: 200,
            annealing_temperature: 30.0,
            annealing_cooling_rate: 0.995,
            annealing_reheat_period: 12,
            annealing_random_seed: 42,
            annealing_interval: 200,
            annealing_max_rounds: 6,
            annealing_reheat_temperature: 0.0,
            annealing_temperature_scale: 0.4,
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
        assert!((config.aux_width - 18.0).abs() < f64::EPSILON);
        assert!((config.aux_height - 18.0).abs() < f64::EPSILON);
        assert!((config.cloud_width - 20.0).abs() < f64::EPSILON);
        assert!((config.cloud_height - 20.0).abs() < f64::EPSILON);

        // Spacing
        assert!((config.horizontal_spacing - 100.0).abs() < f64::EPSILON);
        assert!((config.vertical_spacing - 150.0).abs() < f64::EPSILON);

        // Canvas positioning
        assert!((config.start_x - 100.0).abs() < f64::EPSILON);
        assert!((config.start_y - 100.0).abs() < f64::EPSILON);

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
        assert!((config.annealing_temperature_scale - 0.4).abs() < f64::EPSILON);
    }

    #[test]
    fn test_validate_clamps_negative_dimensions() {
        let mut config = LayoutConfig {
            stock_width: -10.0,
            stock_height: 0.0,
            horizontal_spacing: -5.0,
            vertical_spacing: 0.5,
            ..LayoutConfig::default()
        };
        config.validate();
        assert!((config.stock_width - 1.0).abs() < f64::EPSILON);
        assert!((config.stock_height - 1.0).abs() < f64::EPSILON);
        assert!((config.horizontal_spacing - 1.0).abs() < f64::EPSILON);
        assert!((config.vertical_spacing - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_validate_clamps_cooling_rate() {
        let mut config = LayoutConfig {
            annealing_cooling_rate: 1.5,
            ..LayoutConfig::default()
        };
        config.validate();
        assert!((config.annealing_cooling_rate - 1.0).abs() < f64::EPSILON);

        config.annealing_cooling_rate = -0.1;
        config.validate();
        assert!(config.annealing_cooling_rate.abs() < f64::EPSILON);
    }

    #[test]
    fn test_validate_preserves_valid_config() {
        let mut config = LayoutConfig::default();
        let before = config.clone();
        config.validate();
        assert!((config.stock_width - before.stock_width).abs() < f64::EPSILON);
        assert!(
            (config.annealing_cooling_rate - before.annealing_cooling_rate).abs() < f64::EPSILON
        );
    }
}
