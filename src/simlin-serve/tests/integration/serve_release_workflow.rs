// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Static validation tests for .github/workflows/serve-release.yml.
//!
//! Mirrors `simlin-mcp/tests/mcp_release_workflow.rs`. Parses the YAML
//! workflow and asserts structural properties that would otherwise only be
//! detectable by running a live release. Catching regressions here is
//! cheaper than waiting for a failed npm publish.

use std::path::Path;

fn load_workflow() -> serde_yaml::Value {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let wf_path = repo_root.join(".github/workflows/serve-release.yml");
    let contents = std::fs::read_to_string(&wf_path).expect("read workflow YAML");
    serde_yaml::from_str(&contents).expect("parse workflow YAML")
}

fn load_workflow_text() -> String {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let wf_path = repo_root.join(".github/workflows/serve-release.yml");
    std::fs::read_to_string(&wf_path).expect("read workflow YAML")
}

// Only serve-v* tags trigger the workflow (no broader patterns).
#[test]
fn tag_trigger_is_serve_v_star() {
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
        "serve-v*",
        "tag trigger pattern should be exactly 'serve-v*'"
    );
}

// Build matrix has exactly 4 entries: 3 cross-built via zigbuild on Linux,
// plus aarch64-apple-darwin native on macOS.
#[test]
fn build_matrix_has_all_four_targets() {
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
        "matrix missing x86_64-unknown-linux-musl"
    );
    assert!(
        targets.contains(&"aarch64-unknown-linux-musl"),
        "matrix missing aarch64-unknown-linux-musl"
    );
    assert!(
        targets.contains(&"x86_64-pc-windows-gnu"),
        "matrix missing x86_64-pc-windows-gnu"
    );
    assert!(
        targets.contains(&"aarch64-apple-darwin"),
        "matrix missing aarch64-apple-darwin"
    );

    // macOS arm64 must use macos-latest runner.
    let darwin_entry = matrix
        .iter()
        .find(|e| e["target"].as_str() == Some("aarch64-apple-darwin"))
        .expect("aarch64-apple-darwin entry not found");
    assert_eq!(
        darwin_entry["os"].as_str().unwrap_or(""),
        "macos-latest",
        "aarch64-apple-darwin build must run on macos-latest"
    );
}

#[test]
fn top_level_permissions_contents_read() {
    let wf = load_workflow();
    assert_eq!(
        wf["permissions"]["contents"].as_str().unwrap_or(""),
        "read",
        "top-level permissions.contents should be 'read'"
    );
}

// Only publish jobs may grant id-token: write (OIDC for npm provenance);
// build and validate jobs must not have it.
#[test]
fn only_publish_jobs_have_id_token_write() {
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

// No long-lived npm tokens. NODE_AUTH_TOKEN: '' is allowed (clears any value
// injected by actions/setup-node so npm CLI falls through to OIDC).
#[test]
fn no_npm_token_in_workflow() {
    let text = load_workflow_text();
    assert!(
        !text.contains("NPM_TOKEN"),
        "workflow must not reference NPM_TOKEN"
    );
    assert!(
        !text.contains("secrets.NPM"),
        "workflow must not reference secrets.NPM"
    );
    assert!(
        !text.contains("secrets.NODE"),
        "workflow must not reference secrets.NODE"
    );
    let cleared = text.matches("NODE_AUTH_TOKEN").count();
    let empty_overrides = text.matches("NODE_AUTH_TOKEN: ''").count();
    assert_eq!(
        cleared, empty_overrides,
        "every NODE_AUTH_TOKEN reference must be an empty-string override"
    );
}

// Wrapper publish must wait until all platform packages are published.
#[test]
fn publish_wrapper_needs_publish_platform() {
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
        "publish-wrapper must list publish-platform in its needs"
    );
}

#[test]
fn all_npm_publish_commands_have_provenance() {
    let text = load_workflow_text();
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

// chmod +x on the 3 non-Windows binaries; each assertion verifies the
// platform name appears on the SAME line as chmod +x, not just somewhere
// independently in the file.
#[test]
fn chmod_for_non_windows_binaries() {
    let text = load_workflow_text();
    for platform in &["serve-linux-x64", "serve-linux-arm64", "serve-darwin-arm64"] {
        assert!(
            text.lines()
                .any(|line| line.contains("chmod +x") && line.contains(platform)),
            "workflow must chmod +x the {platform} binary on the same line"
        );
    }
}

// Dockerfile.cross's RUST_VERSION default must match rust-toolchain.toml so
// that local cross-builds use the same compiler as CI.
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

    let dockerfile = std::fs::read_to_string(repo_root.join("src/simlin-serve/Dockerfile.cross"))
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

// Each npm publish step must be guarded by an existence check so reruns
// after partial failure skip already-published packages.
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

// Publish jobs must guard on `refs/tags/serve-v` so workflow_dispatch on an
// arbitrary tag cannot trigger publishing.
#[test]
fn publish_jobs_guard_on_serve_v_tag() {
    let wf = load_workflow();
    for job_name in &["publish-platform", "publish-wrapper"] {
        let condition = wf["jobs"][*job_name]["if"]
            .as_str()
            .unwrap_or_else(|| panic!("{job_name} must have an if condition"));
        assert!(
            condition.contains("refs/tags/serve-v"),
            "{job_name} if-guard must check for 'refs/tags/serve-v', not just 'refs/tags/': {condition}"
        );
    }
}

#[test]
fn build_job_uses_pinned_rust_toolchain() {
    let text = load_workflow_text();
    assert!(
        !text.contains("rust-toolchain@stable"),
        "workflow must not use rust-toolchain@stable; use the repo's rust-toolchain.toml version"
    );
}

#[test]
fn release_builds_use_locked() {
    let text = load_workflow_text();
    for (lineno, line) in text.lines().enumerate() {
        if (line.contains("cargo zigbuild") || line.contains("cargo build"))
            && line.contains("--release")
        {
            assert!(
                line.contains("--locked"),
                "line {} has a release build without --locked: {line}",
                lineno + 1
            );
        }
    }
}

#[test]
fn dockerfile_pins_cargo_zigbuild_version() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let dockerfile = std::fs::read_to_string(repo_root.join("src/simlin-serve/Dockerfile.cross"))
        .expect("read Dockerfile.cross");
    assert!(
        dockerfile
            .lines()
            .any(|line| line.contains("cargo-zigbuild@")),
        "Dockerfile.cross must pin cargo-zigbuild to a specific version"
    );
}

#[test]
fn build_cache_key_includes_toolchain() {
    let text = load_workflow_text();
    assert!(
        text.lines()
            .any(|line| line.contains("hashFiles") && line.contains("rust-toolchain.toml")),
        "build cache key must hash rust-toolchain.toml to invalidate on toolchain changes"
    );
}

// Zig version in Dockerfile.cross must match the version in serve-release.yml.
#[test]
fn dockerfile_zig_version_matches_workflow() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();

    let dockerfile = std::fs::read_to_string(repo_root.join("src/simlin-serve/Dockerfile.cross"))
        .expect("read Dockerfile.cross");
    let docker_zig = dockerfile
        .lines()
        .find_map(|line| line.strip_prefix("ARG ZIG_VERSION="))
        .expect("Dockerfile.cross must have ARG ZIG_VERSION=...");

    let wf = load_workflow();
    let steps = wf["jobs"]["build"]["steps"]
        .as_sequence()
        .expect("build.steps should be a sequence");
    let zig_step = steps
        .iter()
        .find(|s| s["uses"].as_str().is_some_and(|u| u.contains("setup-zig")))
        .expect("build steps should include a setup-zig action");
    let wf_zig = zig_step["with"]["version"]
        .as_str()
        .expect("setup-zig step should have with.version");

    assert_eq!(
        docker_zig, wf_zig,
        "Dockerfile.cross ZIG_VERSION ({docker_zig}) must match serve-release.yml ({wf_zig})"
    );
}

// cross-build.sh must skip the execution smoke test on non-x86_64-Linux hosts
// since the binary it tests targets x86_64-unknown-linux-musl.
#[test]
fn cross_build_script_skips_smoke_on_incompatible_hosts() {
    let script_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/cross-build.sh");
    let text = std::fs::read_to_string(&script_path).expect("read cross-build.sh");
    assert!(
        text.contains("uname -s"),
        "cross-build.sh should check the host OS"
    );
    assert!(
        text.contains("uname -m"),
        "cross-build.sh should check the host CPU architecture"
    );
}

// release-serve.sh must do the same things release-mcp.sh does, transcribed
// to serve: bump Cargo.toml, bump wrapper package.json + optionalDependencies,
// regenerate platform packages, sanity-check version agreement, commit, tag.
#[test]
fn release_script_creates_serve_v_tag() {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let script_path = repo_root.join("scripts/release-serve.sh");
    let text = std::fs::read_to_string(&script_path).expect("read release-serve.sh");
    assert!(
        text.contains("git tag \"serve-v"),
        "release-serve.sh must create a serve-v<version> tag"
    );
    assert!(
        text.contains("Cargo.toml"),
        "release-serve.sh must update Cargo.toml"
    );
    assert!(
        text.contains("build-npm-packages.sh"),
        "release-serve.sh must run build-npm-packages.sh to regenerate platform packages"
    );
    assert!(
        text.contains("optionalDependencies"),
        "release-serve.sh must update wrapper optionalDependencies versions"
    );
    // Allow `git push` to appear inside an echo (informational hint to the
    // operator) but not as a standalone command line.
    for (lineno, line) in text.lines().enumerate() {
        if line.contains("git push") {
            let trimmed = line.trim_start();
            assert!(
                trimmed.starts_with("echo ") || trimmed.starts_with("#"),
                "release-serve.sh must not actually run `git push` (line {}): {line}",
                lineno + 1
            );
        }
    }
}
