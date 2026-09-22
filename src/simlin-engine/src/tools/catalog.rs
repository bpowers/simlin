// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The tool catalog: every tool's name, description, input and output JSON
//! Schema, and effect.
//!
//! The schemas are derived from the tools' serde types with schemars (the
//! `schema` feature) and checked in as `catalog.json`, which
//! [`catalog_json`] embeds, so a build without `schema` -- libsimlin's, the
//! wasm bundle's -- carries the catalog and no schemars code. A freshness test
//! regenerates it and compares; `UPDATE_TOOL_CATALOG=1` rewrites it.
//!
//! Property order in every schema is declaration order (schemars'
//! `preserve_order`), so a host that bridges the catalog into a structured
//! generation framework can take a property order from it.

use serde::Serialize;

/// Every tool, in catalog order.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolName {
    ReadModel,
    ReadVariables,
    FindVariables,
    RunExperiment,
    ReadBehavior,
    ListRuns,
}

/// What calling a tool does to the project.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEffect {
    /// Reads the project and changes nothing.
    Read,
    /// Returns a plan for the host to apply; applies nothing itself.
    PlanEdit,
}

impl ToolName {
    pub const ALL: [ToolName; 6] = [
        ToolName::ReadModel,
        ToolName::ReadVariables,
        ToolName::FindVariables,
        ToolName::RunExperiment,
        ToolName::ReadBehavior,
        ToolName::ListRuns,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ToolName::ReadModel => "read_model",
            ToolName::ReadVariables => "read_variables",
            ToolName::FindVariables => "find_variables",
            ToolName::RunExperiment => "run_experiment",
            ToolName::ReadBehavior => "read_behavior",
            ToolName::ListRuns => "list_runs",
        }
    }

    pub fn from_name(name: &str) -> Option<ToolName> {
        ToolName::ALL.into_iter().find(|tool| tool.name() == name)
    }

    /// What the tool is for and when to reach for it: how an agent selects it.
    /// What it returns in detail is its output schema's to say.
    pub fn description(self) -> &'static str {
        match self {
            ToolName::ReadModel => {
                "Outlines the model: its sim specs; each stock with its initial value, inflows and \
                 outflows; the flows, computed variables, constants, lookup tables and modules, \
                 each with its equation and units; and the engine's diagnostics, each with an id \
                 (D1, D2, ...) that later calls and claims can cite. Also says what changed since \
                 you last read it. Read the model before saying anything about it."
            }
            ToolName::ReadVariables => {
                "Reads up to 12 variables whole: kind, units, documentation, equation or initial \
                 value, per-element equations, lookup points, a stock's flows or the stocks a \
                 flow fills and drains, the variables each reads and is read by with every \
                 link's polarity, and its diagnostic ids. A name that matches nothing comes back \
                 with the closest names."
            }
            ToolName::FindVariables => {
                "Finds variables by a name, part of a name, a misspelling, or a few words of \
                 description, closest first. Use it when a name the person said or wrote does \
                 not match the model's."
            }
            ToolName::RunExperiment => {
                "Runs a what-if experiment on a copy of the model and keeps it under a name for \
                 later calls and claims to cite: set constants to values or multiply them, \
                 replace equations (which cuts the links from what they read: how to test an \
                 explanation), from a time on or from the start, and change DT, the integration \
                 method or the stop time. Returns each change as applied and each recorded \
                 variable's behavior beside the run it started from. The model itself is not \
                 changed."
            }
            ToolName::ReadBehavior => {
                "Summarizes what variables did in runs: start and end, minimum and maximum with \
                 their times, turning points, when it first went negative, its behavior mode \
                 (at rest, linear, exponential, goal seeking, S-shaped, overshoot, rise and \
                 fall, fall and rise, oscillation, undefined for a non-finite value, or other), \
                 and a dozen samples. Name an element to read one (population[north]). \
                 \"current\" is the model as it is; other runs are experiments'."
            }
            ToolName::ListRuns => {
                "Lists the runs experiments made, oldest first, and says for each whether the \
                 model has changed since it was made and what it changed from the model: each \
                 constant's value (from a time on, or from the start), each replacement \
                 equation, the run specs, and the run it started from. Use it to learn what a \
                 run the person made, or one of yours from earlier, tried."
            }
        }
    }

    pub fn effect(self) -> ToolEffect {
        match self {
            ToolName::ReadModel
            | ToolName::ReadVariables
            | ToolName::FindVariables
            | ToolName::RunExperiment
            | ToolName::ReadBehavior
            | ToolName::ListRuns => ToolEffect::Read,
        }
    }
}

/// The catalog as JSON: `{"tools": [{"name", "description", "effect",
/// "inputSchema", "outputSchema"}, ...]}`, in [`ToolName::ALL`] order.
pub fn catalog_json() -> &'static str {
    include_str!("catalog.json")
}

/// The catalog as [`catalog_json`] embeds it, generated from the tools' types.
#[cfg(feature = "schema")]
pub fn generate_catalog_json() -> String {
    use super::{
        FindVariablesInput, FindVariablesOutput, ListRunsInput, ListRunsOutput, ReadBehaviorInput,
        ReadBehaviorOutput, ReadModelInput, ReadModelOutput, ReadVariablesInput,
        ReadVariablesOutput, RunExperimentInput, RunExperimentOutput,
    };

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Entry {
        name: &'static str,
        description: &'static str,
        effect: ToolEffect,
        input_schema: serde_json::Value,
        output_schema: serde_json::Value,
    }

    #[derive(Serialize)]
    struct Catalog {
        tools: Vec<Entry>,
    }

    /// An input's schema describes what the tool deserializes, an output's
    /// what it serializes: under schemars' serialize contract a field the
    /// output leaves out when empty (`skip_serializing_if`) is optional, where
    /// the deserialize contract would require it.
    fn schema<T: schemars::JsonSchema>(
        settings: schemars::generate::SchemaSettings,
    ) -> serde_json::Value {
        let mut value = serde_json::to_value(settings.into_generator().into_root_schema_for::<T>())
            .expect("schemas serialize");
        // The metaschema URI says nothing a tool consumer uses.
        if let Some(object) = value.as_object_mut() {
            object.remove("$schema");
        }
        value
    }
    fn input<T: schemars::JsonSchema>() -> serde_json::Value {
        schema::<T>(schemars::generate::SchemaSettings::draft2020_12().for_deserialize())
    }
    fn output<T: schemars::JsonSchema>() -> serde_json::Value {
        schema::<T>(schemars::generate::SchemaSettings::draft2020_12().for_serialize())
    }

    let tools = ToolName::ALL
        .into_iter()
        .map(|tool| {
            let (input_schema, output_schema) = match tool {
                ToolName::ReadModel => (input::<ReadModelInput>(), output::<ReadModelOutput>()),
                ToolName::ReadVariables => (
                    input::<ReadVariablesInput>(),
                    output::<ReadVariablesOutput>(),
                ),
                ToolName::FindVariables => (
                    input::<FindVariablesInput>(),
                    output::<FindVariablesOutput>(),
                ),
                ToolName::RunExperiment => (
                    input::<RunExperimentInput>(),
                    output::<RunExperimentOutput>(),
                ),
                ToolName::ReadBehavior => {
                    (input::<ReadBehaviorInput>(), output::<ReadBehaviorOutput>())
                }
                ToolName::ListRuns => (input::<ListRunsInput>(), output::<ListRunsOutput>()),
            };
            Entry {
                name: tool.name(),
                description: tool.description(),
                effect: tool.effect(),
                input_schema,
                output_schema,
            }
        })
        .collect();
    let mut json =
        serde_json::to_string_pretty(&Catalog { tools }).expect("the catalog serializes");
    json.push('\n');
    json
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
