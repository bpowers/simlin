// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Integration test for build-npm-packages.sh.
//!
//! Mirrors `simlin-mcp/tests/build_npm_packages.rs`. Runs the shell script in a
//! controlled temp directory (so we don't pollute the source tree) and validates
//! that each platform package.json contains the correct name, version, os, cpu,
//! publishConfig, and repository fields.

use std::process::Command;

struct Platform {
    suffix: &'static str,
    os: &'static str,
    cpu: &'static str,
}

const PLATFORMS: &[Platform] = &[
    Platform {
        suffix: "darwin-arm64",
        os: "darwin",
        cpu: "arm64",
    },
    Platform {
        suffix: "linux-arm64",
        os: "linux",
        cpu: "arm64",
    },
    Platform {
        suffix: "linux-x64",
        os: "linux",
        cpu: "x64",
    },
    Platform {
        suffix: "win32-x64",
        os: "win32",
        cpu: "x64",
    },
];

fn cargo_version() -> String {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let contents = std::fs::read_to_string(&manifest).expect("failed to read Cargo.toml");
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("version = \"")
            && let Some(ver) = rest.strip_suffix('"')
        {
            return ver.to_string();
        }
    }
    panic!("could not parse version from Cargo.toml");
}

#[test]
fn platform_packages_have_correct_fields() {
    let tmp = tempfile::tempdir().expect("failed to create tempdir");

    // The script is self-relative: it derives SCRIPT_DIR from BASH_SOURCE and
    // reads Cargo.toml + writes npm/ next to itself. Copying the script and
    // Cargo.toml into a tempdir gives us isolation without pollating the
    // source tree.
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("build-npm-packages.sh");

    let cargo_toml_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    std::fs::copy(&cargo_toml_src, tmp.path().join("Cargo.toml"))
        .expect("failed to copy Cargo.toml to temp dir");

    let original = std::fs::read_to_string(&script).expect("failed to read build-npm-packages.sh");
    let tmp_script_path = tmp.path().join("build-npm-packages.sh");
    std::fs::write(&tmp_script_path, &original).expect("failed to write script copy to temp dir");

    let status = Command::new("bash")
        .arg(&tmp_script_path)
        .current_dir(tmp.path())
        .status()
        .expect("failed to run build-npm-packages.sh");

    assert!(
        status.success(),
        "build-npm-packages.sh exited with non-zero status: {status}"
    );

    let expected_version = cargo_version();

    for plat in PLATFORMS {
        let pkg_path = tmp
            .path()
            .join("npm")
            .join("@simlin")
            .join(format!("serve-{}", plat.suffix))
            .join("package.json");

        let contents = std::fs::read_to_string(&pkg_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", pkg_path.display()));

        let pkg: serde_json::Value =
            serde_json::from_str(&contents).expect("package.json is not valid JSON");

        assert_eq!(
            pkg["name"],
            format!("@simlin/serve-{}", plat.suffix),
            "wrong name for platform {}",
            plat.suffix
        );

        assert_eq!(
            pkg["version"], expected_version,
            "version mismatch for platform {}",
            plat.suffix
        );

        let os_arr = pkg["os"]
            .as_array()
            .unwrap_or_else(|| panic!("os field is not an array for {}", plat.suffix));
        assert_eq!(
            os_arr.len(),
            1,
            "expected exactly one os entry for {}",
            plat.suffix
        );
        assert_eq!(os_arr[0], plat.os, "wrong os for platform {}", plat.suffix);

        let cpu_arr = pkg["cpu"]
            .as_array()
            .unwrap_or_else(|| panic!("cpu field is not an array for {}", plat.suffix));
        assert_eq!(
            cpu_arr.len(),
            1,
            "expected exactly one cpu entry for {}",
            plat.suffix
        );
        assert_eq!(
            cpu_arr[0], plat.cpu,
            "wrong cpu for platform {}",
            plat.suffix
        );

        let publish_config = &pkg["publishConfig"];
        assert_eq!(
            publish_config["access"], "public",
            "missing publishConfig.access for {}",
            plat.suffix
        );

        let repo = &pkg["repository"];
        assert_eq!(
            repo["type"], "git",
            "missing repository.type for {}",
            plat.suffix
        );
        assert!(
            repo["url"].as_str().unwrap_or("").contains("simlin"),
            "repository.url should reference simlin for {}",
            plat.suffix
        );
    }

    let simlin_dir = tmp.path().join("npm").join("@simlin");
    let dir_count = std::fs::read_dir(&simlin_dir)
        .expect("failed to read @simlin directory")
        .filter(|e| {
            e.as_ref()
                .map(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(
        dir_count, 4,
        "expected exactly 4 platform directories under npm/@simlin/, got {dir_count}"
    );
}

#[test]
fn js_launcher_windows_triple() {
    let js_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bin/simlin-serve.js");
    let contents = std::fs::read_to_string(&js_path).expect("read simlin-serve.js");
    assert!(
        contents.contains("x86_64-pc-windows-gnu"),
        "JS launcher should map Windows to x86_64-pc-windows-gnu for npm packages"
    );
    assert!(
        contents.contains("x86_64-pc-windows-msvc"),
        "JS launcher dev fallback should also try the msvc triple for Windows"
    );
}

#[test]
fn wrapper_package_json_has_publish_config() {
    let pkg_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("package.json");
    let contents = std::fs::read_to_string(&pkg_path).expect("read package.json");
    let pkg: serde_json::Value = serde_json::from_str(&contents).expect("valid JSON");
    assert_eq!(pkg["publishConfig"]["access"], "public");
    assert_eq!(pkg["repository"]["type"], "git");
    assert!(
        pkg["repository"]["url"]
            .as_str()
            .unwrap_or("")
            .contains("simlin"),
        "repository.url should reference simlin"
    );
}
