// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Static resource registry for MCP `resources/list` and `resources/read`.
//!
//! Resources are compiled into the binary as `&'static str` content
//! via `include_str!`, so there is no runtime file I/O dependency.

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
            description: Some(
                "Loading models, running simulations, DataFrame access, matplotlib basics, error handling",
            ),
            mime_type: Some("text/markdown"),
        },
        content: include_str!("skills/pysimlin-basics.md"),
    },
    ResourceEntry {
        metadata: Resource {
            uri: "simlin://skills/scenario-analysis",
            name: "scenario-analysis",
            description: Some(
                "Parameter sweeps with overrides, intervention analysis, comparing scenarios",
            ),
            mime_type: Some("text/markdown"),
        },
        content: include_str!("skills/scenario-analysis.md"),
    },
    ResourceEntry {
        metadata: Resource {
            uri: "simlin://skills/loop-dominance",
            name: "loop-dominance",
            description: Some(
                "Plotting behavior_time_series, annotating dominant_periods on charts, interpreting importance values",
            ),
            mime_type: Some("text/markdown"),
        },
        content: include_str!("skills/loop-dominance.md"),
    },
    ResourceEntry {
        metadata: Resource {
            uri: "simlin://skills/vensim-equation-syntax",
            name: "vensim-equation-syntax",
            description: Some(
                "Vensim-specific names, logical operators, IF THEN ELSE function form, complete MDL-to-XMILE mapping table",
            ),
            mime_type: Some("text/markdown"),
        },
        content: include_str!("skills/vensim-equation-syntax.md"),
    },
];
