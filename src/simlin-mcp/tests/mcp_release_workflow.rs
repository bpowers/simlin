// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Static validation tests for .github/workflows/mcp-release.yml (AC1, AC2, AC5).
//!
//! These tests parse the YAML workflow file and assert structural properties
//! that would otherwise only be detectable via a live CI run.  Catching
//! regressions here is cheaper than waiting for a failed release workflow.

use std::path::Path;

fn load_workflow() -> serde_yml::Value {
    // CARGO_MANIFEST_DIR is src/simlin-mcp; repo root is two levels up.
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let wf_path = repo_root.join(".github/workflows/mcp-release.yml");
    let contents = std::fs::read_to_string(&wf_path).expect("read workflow YAML");
    serde_yml::from_str(&contents).expect("parse workflow YAML")
}

fn load_workflow_text() -> String {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let wf_path = repo_root.join(".github/workflows/mcp-release.yml");
    std::fs::read_to_string(&wf_path).expect("read workflow YAML")
}

// AC1.1 / AC1.6: only mcp-v* tags trigger the workflow (no broader patterns)
#[test]
fn ac1_1_tag_trigger_is_mcp_v_star() {
    let wf = load_workflow();
    let tags = wf["on"]["push"]["tags"]
        .as_sequence()
        .expect("on.push.tags should be a sequence");
    assert_eq!(
        tags.len(),
        1,
        "expected exactly one tag pattern, got {}",
        tags.len()
    );
    assert_eq!(
        tags[0].as_str().unwrap_or(""),
        "mcp-v*",
        "tag trigger pattern should be exactly 'mcp-v*'"
    );
}

// AC1.2 - AC1.5: build matrix has exactly 4 entries with the expected targets/os
#[test]
fn ac1_2_to_1_5_build_matrix_has_all_four_targets() {
    let wf = load_workflow();
    let matrix = wf["jobs"]["build"]["strategy"]["matrix"]["include"]
        .as_sequence()
        .expect("jobs.build.strategy.matrix.include should be a sequence");

    assert_eq!(
        matrix.len(),
        4,
        "expected exactly 4 build matrix entries, got {}",
        matrix.len()
    );

    let targets: Vec<&str> = matrix
        .iter()
        .map(|e| e["target"].as_str().expect("matrix entry missing target"))
        .collect();

    assert!(
        targets.contains(&"x86_64-unknown-linux-musl"),
        "matrix missing x86_64-unknown-linux-musl (AC1.2)"
    );
    assert!(
        targets.contains(&"aarch64-unknown-linux-musl"),
        "matrix missing aarch64-unknown-linux-musl (AC1.3)"
    );
    assert!(
        targets.contains(&"x86_64-pc-windows-gnu"),
        "matrix missing x86_64-pc-windows-gnu (AC1.4)"
    );
    assert!(
        targets.contains(&"aarch64-apple-darwin"),
        "matrix missing aarch64-apple-darwin (AC1.5)"
    );

    // macOS arm64 must use macos-latest runner
    let darwin_entry = matrix
        .iter()
        .find(|e| e["target"].as_str() == Some("aarch64-apple-darwin"))
        .expect("aarch64-apple-darwin entry not found");
    assert_eq!(
        darwin_entry["os"].as_str().unwrap_or(""),
        "macos-latest",
        "aarch64-apple-darwin build must run on macos-latest (AC1.5)"
    );
}

// AC5.1: top-level permissions.contents is "read"
#[test]
fn ac5_1_top_level_permissions_contents_read() {
    let wf = load_workflow();
    assert_eq!(
        wf["permissions"]["contents"].as_str().unwrap_or(""),
        "read",
        "top-level permissions.contents should be 'read'"
    );
}

// AC5.2: only publish-platform and publish-wrapper have id-token: write;
//        build and validate jobs must not have it
#[test]
fn ac5_2_only_publish_jobs_have_id_token_write() {
    let wf = load_workflow();

    assert_eq!(
        wf["jobs"]["publish-platform"]["permissions"]["id-token"]
            .as_str()
            .unwrap_or(""),
        "write",
        "publish-platform must have id-token: write"
    );
    assert_eq!(
        wf["jobs"]["publish-wrapper"]["permissions"]["id-token"]
            .as_str()
            .unwrap_or(""),
        "write",
        "publish-wrapper must have id-token: write"
    );

    // build and validate jobs must not grant id-token
    let build_id_token = &wf["jobs"]["build"]["permissions"]["id-token"];
    assert!(
        build_id_token.is_null(),
        "build job must not have id-token permission"
    );
    let validate_id_token = &wf["jobs"]["validate"]["permissions"]["id-token"];
    assert!(
        validate_id_token.is_null(),
        "validate job must not have id-token permission"
    );
}

// AC5.3: no long-lived npm tokens in the workflow
#[test]
fn ac5_3_no_npm_token_in_workflow() {
    let text = load_workflow_text();
    assert!(
        !text.contains("NPM_TOKEN"),
        "workflow must not reference NPM_TOKEN"
    );
    assert!(
        !text.contains("NODE_AUTH_TOKEN"),
        "workflow must not reference NODE_AUTH_TOKEN"
    );
    assert!(
        !text.contains("secrets.NPM"),
        "workflow must not reference secrets.NPM"
    );
}

// AC2.1: publish-wrapper depends on publish-platform
#[test]
fn ac2_1_publish_wrapper_needs_publish_platform() {
    let wf = load_workflow();
    let needs = wf["jobs"]["publish-wrapper"]["needs"]
        .as_sequence()
        .expect("publish-wrapper.needs should be a sequence");
    let need_names: Vec<&str> = needs
        .iter()
        .map(|n| n.as_str().expect("needs entry should be a string"))
        .collect();
    assert!(
        need_names.contains(&"publish-platform"),
        "publish-wrapper must list publish-platform in its needs (AC2.1)"
    );
}

// AC2.3: every npm publish invocation includes --provenance
#[test]
fn ac2_3_all_npm_publish_commands_have_provenance() {
    let text = load_workflow_text();
    // Find every line that calls `npm publish` and assert --provenance is present.
    for (lineno, line) in text.lines().enumerate() {
        if line.contains("npm publish") {
            assert!(
                line.contains("--provenance"),
                "line {} calls npm publish without --provenance: {line}",
                lineno + 1
            );
        }
    }
}

// AC2.5: chmod +x is present for the 3 non-Windows binaries.
// Each assertion verifies the chmod and the platform appear on the SAME line,
// not just somewhere independently in the file.
#[test]
fn ac2_5_chmod_for_non_windows_binaries() {
    let text = load_workflow_text();
    for platform in &["mcp-linux-x64", "mcp-linux-arm64", "mcp-darwin-arm64"] {
        assert!(
            text.lines()
                .any(|line| line.contains("chmod +x") && line.contains(platform)),
            "workflow must chmod +x the {platform} binary on the same line (AC2.5)"
        );
    }
}

// Dockerfile.cross must default to the same Rust version as rust-toolchain.toml
// so that local cross-builds use the same compiler as the rest of the project.
#[test]
fn dockerfile_rust_version_matches_toolchain() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();

    let toolchain = std::fs::read_to_string(repo_root.join("rust-toolchain.toml"))
        .expect("read rust-toolchain.toml");
    let toolchain_version = toolchain
        .lines()
        .find_map(|line| {
            line.strip_prefix("channel = \"")
                .and_then(|rest| rest.strip_suffix('"'))
        })
        .expect("rust-toolchain.toml must have channel = \"...\"");

    let dockerfile = std::fs::read_to_string(repo_root.join("src/simlin-mcp/Dockerfile.cross"))
        .expect("read Dockerfile.cross");
    let docker_version = dockerfile
        .lines()
        .find_map(|line| line.strip_prefix("ARG RUST_VERSION="))
        .expect("Dockerfile.cross must have ARG RUST_VERSION=...");

    assert_eq!(
        docker_version, toolchain_version,
        "Dockerfile.cross RUST_VERSION ({docker_version}) must match rust-toolchain.toml ({toolchain_version})"
    );
}

// Each npm publish step must be guarded by an existence check so that
// reruns after partial failure skip already-published packages.
// We verify that `npm view` appears at least as many times as `npm publish`,
// ensuring every publish is individually guarded.
#[test]
fn publish_steps_are_rerunnable() {
    let text = load_workflow_text();
    let publish_count = text.lines().filter(|l| l.contains("npm publish")).count();
    let view_count = text.lines().filter(|l| l.contains("npm view")).count();
    assert!(
        publish_count > 0,
        "workflow should have at least one npm publish command"
    );
    assert!(
        view_count >= publish_count,
        "each npm publish must be guarded by an npm view check ({view_count} views < {publish_count} publishes)"
    );
}

// Publish jobs must guard on `refs/tags/mcp-v`, not just `refs/tags/`,
// so that workflow_dispatch on an arbitrary tag cannot trigger publishing.
#[test]
fn publish_jobs_guard_on_mcp_v_tag() {
    let wf = load_workflow();
    for job_name in &["publish-platform", "publish-wrapper"] {
        let condition = wf["jobs"][*job_name]["if"]
            .as_str()
            .unwrap_or_else(|| panic!("{job_name} must have an if condition"));
        assert!(
            condition.contains("refs/tags/mcp-v"),
            "{job_name} if-guard must check for 'refs/tags/mcp-v', not just 'refs/tags/': {condition}"
        );
    }
}

// The CI build job must use the repo's pinned Rust toolchain, not @stable,
// so release binaries are compiled with the same compiler as local/CI builds.
#[test]
fn build_job_uses_pinned_rust_toolchain() {
    let text = load_workflow_text();
    assert!(
        !text.contains("rust-toolchain@stable"),
        "workflow must not use rust-toolchain@stable; use the repo's rust-toolchain.toml version"
    );
}

// Dockerfile.cross must pin cargo-zigbuild to a specific version
// to stay in sync with the CI workflow.
#[test]
fn dockerfile_pins_cargo_zigbuild_version() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let dockerfile = std::fs::read_to_string(repo_root.join("src/simlin-mcp/Dockerfile.cross"))
        .expect("read Dockerfile.cross");
    assert!(
        dockerfile
            .lines()
            .any(|line| line.contains("cargo-zigbuild@")),
        "Dockerfile.cross must pin cargo-zigbuild to a specific version"
    );
}

// cross-build.sh must skip the execution smoke test on non-Linux hosts,
// since the output binary targets Linux and cannot run on macOS/Windows.
#[test]
fn cross_build_script_skips_smoke_on_non_linux() {
    let script_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/cross-build.sh");
    let text = std::fs::read_to_string(&script_path).expect("read cross-build.sh");
    assert!(
        text.contains("uname"),
        "cross-build.sh should detect the host OS to skip smoke tests on non-Linux"
    );
}
