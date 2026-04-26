// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MCP server binary for Simlin.
//!
//! Composes the rmcp `ServerHandler` from `simlin-mcp-core` with a
//! stateless [`FileSystemAccess`] and the OUT_DIR-substituted resource
//! content embedded at build time, then hands the result to rmcp's
//! stdio transport.  Everything reusable lives in the library half of
//! this crate (see [`simlin_mcp::access`]) or in `simlin-mcp-core`.
//!
//! # Usage
//!
//! ```sh
//! simlin-mcp              # start the MCP server on stdin/stdout
//! simlin-mcp --version    # print version
//! ```

use rmcp::{ServiceExt, transport::stdio};
use simlin_mcp::access::FileSystemAccess;
use simlin_mcp_core::server::{ResourceContent, SimlinMcpServer};

/// Instructions content embedded at build time.  `build.rs` substitutes
/// `{PYSIMLIN_VERSION}` from `pysimlin.version` into the source
/// `src/instructions.md` and writes the processed file to `OUT_DIR`.
const INSTRUCTIONS: &str = include_str!(concat!(env!("OUT_DIR"), "/instructions.md"));

/// Skill resources exposed via MCP `resources/list` and `resources/read`.
///
/// Three of the four skills are included verbatim from source.  Only
/// `pysimlin-basics.md` goes through `build.rs`'s `{PYSIMLIN_VERSION}`
/// substitution and lives in `OUT_DIR`.  Bundling the bytes at compile
/// time avoids any runtime file I/O.
fn build_resources() -> Vec<ResourceContent> {
    vec![
        ResourceContent {
            uri: "simlin://skills/pysimlin-basics".into(),
            name: "pysimlin-basics".into(),
            description:
                "Loading models, running simulations, DataFrame access, matplotlib basics, error handling"
                    .into(),
            mime_type: "text/markdown".into(),
            body: include_str!(concat!(env!("OUT_DIR"), "/pysimlin-basics.md")).to_string(),
        },
        ResourceContent {
            uri: "simlin://skills/scenario-analysis".into(),
            name: "scenario-analysis".into(),
            description: "Parameter sweeps with overrides, intervention analysis, comparing scenarios"
                .into(),
            mime_type: "text/markdown".into(),
            body: include_str!("skills/scenario-analysis.md").to_string(),
        },
        ResourceContent {
            uri: "simlin://skills/loop-dominance".into(),
            name: "loop-dominance".into(),
            description:
                "Plotting behavior_time_series, annotating dominant_periods on charts, interpreting importance values"
                    .into(),
            mime_type: "text/markdown".into(),
            body: include_str!("skills/loop-dominance.md").to_string(),
        },
        ResourceContent {
            uri: "simlin://skills/vensim-equation-syntax".into(),
            name: "vensim-equation-syntax".into(),
            description:
                "Vensim-specific names, logical operators, IF THEN ELSE function form, complete MDL-to-XMILE mapping table"
                    .into(),
            mime_type: "text/markdown".into(),
            body: include_str!("skills/vensim-equation-syntax.md").to_string(),
        },
    ]
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("simlin-mcp {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let server = SimlinMcpServer::new(
        FileSystemAccess::new(),
        INSTRUCTIONS.to_string(),
        build_resources(),
    );

    // `serve(stdio())` performs the MCP `initialize` handshake on the
    // current task, then hands ongoing message dispatch to a background
    // task held by `RunningService`.  `waiting()` blocks the main task
    // until that background task finishes (typically when the MCP host
    // closes stdin), at which point we exit cleanly.
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    const INSTRUCTIONS: &str = include_str!(concat!(env!("OUT_DIR"), "/instructions.md"));

    // mcp-publish-ready.AC4.1: instructions field is non-empty
    #[test]
    fn instructions_not_empty() {
        assert!(
            !INSTRUCTIONS.is_empty(),
            "instructions.md must not be empty"
        );
    }

    // mcp-publish-ready.AC4.2: instructions mention core tools and concepts
    #[test]
    fn instructions_mention_core_tools() {
        for keyword in ["ReadModel", "EditModel", "CreateModel", ".mdl", "pysimlin"] {
            assert!(
                INSTRUCTIONS.contains(keyword),
                "instructions.md must mention '{keyword}'"
            );
        }
    }

    // mcp-publish-ready.AC4.3: instructions include SetLoopName guidance
    #[test]
    fn instructions_include_set_loop_name() {
        assert!(
            INSTRUCTIONS.contains("setLoopName"),
            "instructions.md must mention setLoopName"
        );
        assert!(
            INSTRUCTIONS.contains("variables"),
            "instructions.md must mention 'variables' (SetLoopName field)"
        );
    }

    // version-mgmt.AC1.7: pysimlin.version matches latest pysimlin git tag
    #[test]
    fn pysimlin_version_matches_latest_tag() {
        let output = std::process::Command::new("git")
            .args(["tag", "--list", "pysimlin-v*", "--sort=-v:refname"])
            .output()
            .expect("git tag command failed");
        let tags = String::from_utf8(output.stdout).unwrap();
        if tags.trim().is_empty() {
            return;
        }
        let latest_tag = tags.lines().next().expect("no pysimlin tags found");
        let version = latest_tag
            .strip_prefix("pysimlin-v")
            .expect("unexpected tag format");
        assert_eq!(
            env!("PYSIMLIN_VERSION"),
            version,
            "pysimlin.version is stale (contains {}, latest tag is {version})",
            env!("PYSIMLIN_VERSION"),
        );
    }

    // version-mgmt.AC1.8: compiled content contains the substituted version
    #[test]
    fn instructions_contain_pysimlin_version() {
        let version = env!("PYSIMLIN_VERSION");
        assert!(
            INSTRUCTIONS.contains(version),
            "instructions.md must contain pysimlin version {version} (placeholder may be missing)"
        );
        let pysimlin_basics = include_str!(concat!(env!("OUT_DIR"), "/pysimlin-basics.md"));
        assert!(
            pysimlin_basics.contains(version),
            "pysimlin-basics.md must contain pysimlin version {version} (placeholder may be missing)"
        );
    }
}
