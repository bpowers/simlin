// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Static resource registry for MCP `resources/list` and `resources/read`.
//!
//! Resources are compiled into the binary as `&'static str` content,
//! so there is no runtime file I/O dependency. Phase 7 will replace
//! the placeholder content with `include_str!` of actual skill files.

/// Metadata for a resource exposed via MCP `resources/list`.
pub struct Resource {
    pub uri: &'static str,
    pub name: &'static str,
    pub description: Option<&'static str>,
    pub mime_type: Option<&'static str>,
}

/// A resource entry: metadata plus embedded content.
pub struct ResourceEntry {
    pub metadata: Resource,
    pub content: &'static str,
}

/// Returns all registered resources.
pub fn list() -> &'static [ResourceEntry] {
    RESOURCES
}

/// Look up a resource by URI. Returns None if not found.
pub fn get(uri: &str) -> Option<&'static ResourceEntry> {
    RESOURCES.iter().find(|r| r.metadata.uri == uri)
}

static RESOURCES: &[ResourceEntry] = &[
    ResourceEntry {
        metadata: Resource {
            uri: "simlin://skills/pysimlin-basics",
            name: "pysimlin-basics",
            description: Some("Loading models, running simulations, DataFrame access"),
            mime_type: Some("text/markdown"),
        },
        content: "# pysimlin Basics\n\nPlaceholder -- see Phase 7.",
    },
    ResourceEntry {
        metadata: Resource {
            uri: "simlin://skills/scenario-analysis",
            name: "scenario-analysis",
            description: Some("Parameter sweeps, interventions, sensitivity analysis"),
            mime_type: Some("text/markdown"),
        },
        content: "# Scenario Analysis\n\nPlaceholder -- see Phase 7.",
    },
    ResourceEntry {
        metadata: Resource {
            uri: "simlin://skills/loop-dominance",
            name: "loop-dominance",
            description: Some("Plotting importance over time, annotating dominant periods"),
            mime_type: Some("text/markdown"),
        },
        content: "# Loop Dominance\n\nPlaceholder -- see Phase 7.",
    },
    ResourceEntry {
        metadata: Resource {
            uri: "simlin://skills/vensim-equation-syntax",
            name: "vensim-equation-syntax",
            description: Some("Vensim-to-XMILE function mapping, MDL syntax reference"),
            mime_type: Some("text/markdown"),
        },
        content: "# Vensim Equation Syntax\n\nPlaceholder -- see Phase 7.",
    },
];
