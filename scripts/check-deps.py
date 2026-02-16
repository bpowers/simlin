#!/usr/bin/env python3
"""Validate workspace dependencies against the policy in dep-policy.json.

Checks:
  - Rust: workspace `path` dependencies in [dependencies] only
    (not [dev-dependencies] or [build-dependencies]).
    Optional dependencies gated behind features are included.
  - TypeScript: @simlin/* entries in both dependencies and devDependencies.
    Policy keys match the `name` field in package.json.
  - Bidirectional: all actual workspace packages must appear in the policy,
    and all policy entries must correspond to actual packages.

Workspace members are auto-discovered from Cargo.toml and pnpm-workspace.yaml.

Exit code 0 on success, 1 on any violation.
"""

from __future__ import annotations

import json
import re
import sys
import tomllib
from pathlib import Path


def load_policy(repo_root: Path) -> dict[str, dict[str, list[str]]]:
    policy_path = repo_root / "scripts" / "dep-policy.json"
    with open(policy_path) as f:
        return json.load(f)


def discover_rust_members(repo_root: Path) -> list[str]:
    """Auto-discover Rust workspace members from root Cargo.toml."""
    cargo_path = repo_root / "Cargo.toml"
    with open(cargo_path, "rb") as f:
        data = tomllib.load(f)
    return data.get("workspace", {}).get("members", [])


def discover_typescript_members(repo_root: Path) -> list[Path]:
    """Auto-discover TypeScript workspace members from pnpm-workspace.yaml."""
    workspace_path = repo_root / "pnpm-workspace.yaml"
    # Simple parser for the pnpm-workspace.yaml format (list of quoted paths)
    members: list[Path] = []
    for line in workspace_path.read_text().splitlines():
        stripped = line.strip()
        # Match lines like:   - 'src/engine'  or  - "src/engine"
        match = re.match(r"-\s+['\"]([^'\"]+)['\"]", stripped)
        if match:
            members.append(repo_root / match.group(1))
    return members


def extract_path_deps(cargo_path: Path) -> tuple[str, set[str]]:
    """Extract package name and path dependencies from a Cargo.toml using tomllib."""
    with open(cargo_path, "rb") as f:
        data = tomllib.load(f)

    pkg_name = data.get("package", {}).get("name", "")
    deps = data.get("dependencies", {})

    path_deps: set[str] = set()
    for dep_name, dep_val in deps.items():
        if isinstance(dep_val, dict) and "path" in dep_val:
            path_deps.add(dep_name)

    return pkg_name, path_deps


def check_rust_deps(repo_root: Path, policy: dict[str, list[str]]) -> list[str]:
    """Check Rust workspace path dependencies against policy."""
    errors: list[str] = []
    workspace_members = discover_rust_members(repo_root)
    actual_pkg_names: set[str] = set()

    for member in workspace_members:
        cargo_path = repo_root / member / "Cargo.toml"
        if not cargo_path.exists():
            errors.append(f"ERROR: {cargo_path} not found")
            continue

        pkg_name, actual_deps = extract_path_deps(cargo_path)
        if not pkg_name:
            errors.append(f"ERROR: Could not find package name in {cargo_path}")
            continue
        actual_pkg_names.add(pkg_name)

        if pkg_name not in policy:
            errors.append(
                f"ERROR: Rust package '{pkg_name}' from {cargo_path} not found in dep-policy.json. "
                f"Add it to scripts/dep-policy.json."
            )
            continue

        allowed = set(policy[pkg_name])

        for dep in sorted(actual_deps):
            if dep not in allowed:
                errors.append(
                    f"ERROR: {pkg_name} must not depend on {dep}.\n"
                    f"  Allowed dependencies for {pkg_name}: {', '.join(sorted(allowed)) or '(none)'}\n"
                    f"  See doc/architecture.md for the dependency graph.\n"
                    f"  To add a new allowed dependency, update scripts/dep-policy.json."
                )

    # Check for stale policy entries (packages in policy but not in workspace)
    for policy_name in sorted(policy):
        if policy_name not in actual_pkg_names:
            errors.append(
                f"ERROR: Rust policy entry '{policy_name}' has no corresponding workspace package. "
                f"Remove it from scripts/dep-policy.json or add the package to the workspace."
            )

    return errors


def check_typescript_deps(repo_root: Path, policy: dict[str, list[str]]) -> list[str]:
    """Check TypeScript @simlin/* dependencies against policy."""
    errors: list[str] = []
    package_dirs = discover_typescript_members(repo_root)
    actual_pkg_names: set[str] = set()

    for pkg_dir in package_dirs:
        pkg_path = pkg_dir / "package.json"
        if not pkg_path.exists():
            errors.append(f"ERROR: {pkg_path} not found")
            continue

        with open(pkg_path) as f:
            pkg = json.load(f)

        pkg_name = pkg.get("name", "")
        if not pkg_name:
            errors.append(f"ERROR: No 'name' field in {pkg_path}")
            continue
        actual_pkg_names.add(pkg_name)

        if pkg_name not in policy:
            errors.append(
                f"ERROR: TypeScript package '{pkg_name}' from {pkg_path} not found in dep-policy.json. "
                f"Add it to scripts/dep-policy.json."
            )
            continue

        allowed = set(policy[pkg_name])

        # Collect @simlin/* deps from both dependencies and devDependencies
        actual_deps: set[str] = set()
        for section in ("dependencies", "devDependencies"):
            deps = pkg.get(section, {})
            for dep_name in deps:
                if dep_name.startswith("@simlin/"):
                    actual_deps.add(dep_name)

        for dep in sorted(actual_deps):
            if dep not in allowed:
                errors.append(
                    f"ERROR: {pkg_name} must not depend on {dep}.\n"
                    f"  Allowed dependencies for {pkg_name}: {', '.join(sorted(allowed)) or '(none)'}\n"
                    f"  See doc/architecture.md for the dependency graph.\n"
                    f"  To add a new allowed dependency, update scripts/dep-policy.json."
                )

    # Check for stale policy entries
    for policy_name in sorted(policy):
        if policy_name not in actual_pkg_names:
            errors.append(
                f"ERROR: TypeScript policy entry '{policy_name}' has no corresponding workspace package. "
                f"Remove it from scripts/dep-policy.json or add the package to the workspace."
            )

    return errors


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent
    policy = load_policy(repo_root)

    errors: list[str] = []
    errors.extend(check_rust_deps(repo_root, policy.get("rust", {})))
    errors.extend(check_typescript_deps(repo_root, policy.get("typescript", {})))

    if errors:
        for err in errors:
            print(err, file=sys.stderr)
        return 1

    print("Dependency policy check passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
