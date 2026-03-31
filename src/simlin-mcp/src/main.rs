// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MCP server binary for Simlin.
//!
//! Exposes the Simlin simulation engine as MCP tools over stdio,
//! allowing AI assistants to read, create, and edit system dynamics
//! models.
//!
//! # Usage
//!
//! ```sh
//! simlin-mcp              # start the MCP server on stdin/stdout
//! simlin-mcp --version    # print version
//! ```

mod protocol;
mod resource;
mod tool;
mod tools;
mod transport;

use transport::StdioTransport;

#[tokio::main]
async fn main() {
    // Handle --version
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("simlin-mcp {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let config = protocol::ServerConfig {
        name: "simlin-mcp".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        instructions: Some(include_str!("instructions.md").to_string()),
    };

    let mut registry = tool::Registry::new();
    tools::register_all(&mut registry);

    let mut transport = StdioTransport::new();

    let result = protocol::serve_async(&mut transport, &config, &registry).await;
    // Wait for the stdout writer to drain all queued responses before exiting.
    // Without this, a client that closes stdin immediately after sending a
    // request may not receive the response.
    transport.shutdown().await;
    if let Err(e) = result {
        eprintln!("simlin-mcp: fatal error: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    const INSTRUCTIONS: &str = include_str!("instructions.md");

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

    // mcp-publish-ready.AC4.4: instructions reference current pysimlin version
    #[test]
    fn instructions_reference_pysimlin_version() {
        assert!(
            INSTRUCTIONS.contains("0.6.2"),
            "instructions.md must reference pysimlin version 0.6.2"
        );
    }

    // mcp-publish-ready.AC4.5: version matches latest pysimlin git tag.
    // Validates instructions.md and all skill files that reference the
    // pysimlin version so a new release tag surfaces any stale references.
    #[test]
    fn instructions_reference_current_pysimlin_version() {
        let output = std::process::Command::new("git")
            .args(["tag", "--list", "pysimlin-v*", "--sort=-v:refname"])
            .output()
            .expect("git tag command failed");
        let tags = String::from_utf8(output.stdout).unwrap();
        if tags.trim().is_empty() {
            // Shallow checkout or no tags -- cannot validate version freshness.
            // The non-git test (instructions_reference_pysimlin_version) still
            // validates the hardcoded version string.
            return;
        }
        let latest_tag = tags.lines().next().expect("no pysimlin tags found");
        let version = latest_tag
            .strip_prefix("pysimlin-v")
            .expect("unexpected tag format");

        let versioned_files: &[(&str, &str)] = &[
            ("instructions.md", INSTRUCTIONS),
            (
                "skills/pysimlin-basics.md",
                include_str!("skills/pysimlin-basics.md"),
            ),
        ];
        for (name, content) in versioned_files {
            assert!(
                content.contains(version),
                "{name} references outdated pysimlin version. Latest: {version}"
            );
        }
    }
}
