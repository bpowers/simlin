// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
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

// mimalloc on native builds: the engine compile path is allocation-heavy
// (millions of small, short-lived allocations); mimalloc roughly halves the
// allocator time vs the system malloc. See docs/design/engine-performance.md.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

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
        env!("CARGO_PKG_VERSION").to_string(),
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
    }

    /// What the tag guard does with what `git tag --list` printed.
    #[derive(Debug, PartialEq)]
    enum TagCheck<'a> {
        /// Compare `pysimlin.version` with this version, the newest tag's.
        Compare(&'a str),
        /// No tags in a clone that is not CI's: nothing to compare against.
        Skip,
        /// No tags where there must be some, or a tag of an unexpected form.
        Fail(String),
    }

    /// Decide the tag guard from the tag listing (newest first, one per
    /// line; `None` when git itself failed) and whether this is a CI run.
    ///
    /// A clone without the tags (a developer's fresh shallow clone) skips. CI
    /// does not get to: its Build job fetches the tags by name
    /// (`.github/workflows/ci.yaml`), and a guard that skipped there whenever
    /// that fetch brought nothing would pass on every run while checking
    /// nothing.
    fn tag_check(tags: Option<&str>, in_ci: bool) -> TagCheck<'_> {
        let newest = tags.and_then(|tags| tags.lines().map(str::trim).find(|l| !l.is_empty()));
        match newest {
            Some(tag) => match tag.strip_prefix("pysimlin-v") {
                Some(version) => TagCheck::Compare(version),
                None => TagCheck::Fail(format!("unexpected tag format: {tag}")),
            },
            None if in_ci => TagCheck::Fail(
                "no pysimlin-v* tags are visible in a CI checkout: the job that runs this \
                 test must fetch them (`git fetch origin \
                 'refs/tags/pysimlin-v*:refs/tags/pysimlin-v*'`)"
                    .to_string(),
            ),
            None => TagCheck::Skip,
        }
    }

    #[test]
    fn the_tag_guard_skips_only_outside_ci() {
        let newest_first = "pysimlin-v0.8.5\npysimlin-v0.8.4\n";
        for in_ci in [false, true] {
            assert_eq!(
                tag_check(Some(newest_first), in_ci),
                TagCheck::Compare("0.8.5")
            );
            assert!(matches!(
                tag_check(Some("v0.8.5\n"), in_ci),
                TagCheck::Fail(_)
            ));
        }
        // No tags, whether git listed none or could not run.
        for tags in [Some(""), Some("\n"), None] {
            assert_eq!(tag_check(tags, false), TagCheck::Skip);
            assert!(matches!(tag_check(tags, true), TagCheck::Fail(_)));
        }
    }

    // version-mgmt.AC1.7: pysimlin.version matches latest pysimlin git tag.
    //
    // pysimlin's version has no in-tree source of truth to compare against --
    // setuptools-scm derives it from the `pysimlin-v*` tag itself (see
    // `tag_regex` in src/pysimlin/pyproject.toml) -- so the tag is the only
    // thing this can be checked against, and the check needs the tags to be
    // present locally. `tag_check` says what happens when they are not.
    #[test]
    fn pysimlin_version_matches_latest_tag() {
        let output = std::process::Command::new("git")
            .args(["tag", "--list", "pysimlin-v*", "--sort=-v:refname"])
            .output()
            .expect("git tag command failed");
        let tags = output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned());
        // The runners whose workflow fetches the tags; GitHub Actions sets
        // this for every step.
        let in_ci = std::env::var_os("GITHUB_ACTIONS").is_some();
        match tag_check(tags.as_deref(), in_ci) {
            TagCheck::Compare(version) => assert_eq!(
                env!("PYSIMLIN_VERSION"),
                version,
                "pysimlin.version is stale (contains {}, latest tag is {version})",
                env!("PYSIMLIN_VERSION"),
            ),
            TagCheck::Skip => eprintln!(
                "SKIPPING pysimlin_version_matches_latest_tag: no pysimlin-v* tags are \
                 visible in this clone; run `git fetch --tags` to exercise this guard."
            ),
            TagCheck::Fail(reason) => panic!("{reason}"),
        }
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
