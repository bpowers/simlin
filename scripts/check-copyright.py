#!/usr/bin/env python3
"""Check and optionally fix copyright headers on Rust and TypeScript source files.

Every .rs, .ts, and .tsx source file (excluding generated files) must have
a copyright header.  The canonical line-comment format is:

    // Copyright 2026 The Simlin Authors. All rights reserved.
    // Use of this source code is governed by the Apache License,
    // Version 2.0, that can be found in the LICENSE file.

Files that need a @jest-environment directive use a block comment:

    /**
     * @jest-environment node
     *
     * Copyright 2026 The Simlin Authors. All rights reserved.
     * Use of this source code is governed by the Apache License,
     * Version 2.0, that can be found in the LICENSE file.
     */

Run with --fix to automatically update or add headers.
Exit code 0 on success, 1 on any violation.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

EXPECTED_YEAR = "2026"

HEADER_LINES = [
    f"// Copyright {EXPECTED_YEAR} The Simlin Authors. All rights reserved.",
    "// Use of this source code is governed by the Apache License,",
    "// Version 2.0, that can be found in the LICENSE file.",
]

HEADER_TEXT = "\n".join(HEADER_LINES)

# Matches "Copyright YYYY <author>. All rights reserved." in any comment style.
COPYRIGHT_RE = re.compile(
    r"Copyright\s+(\d{4})\s+(.*?)\.\s+All rights reserved\."
)

# Directories to skip entirely.
SKIP_DIRS = frozenset({
    "target", "node_modules", "lib", "lib.browser", "lib.module", "build",
    ".git", "playwright-report", "test-results",
})


def should_skip_file(path: Path) -> bool:
    """Return True for generated files that should not have copyright headers."""
    name = path.name
    if name.endswith(".gen.rs"):
        return True
    if name.endswith("_pb.d.ts"):
        return True
    return False


def find_source_files(repo_root: Path) -> list[Path]:
    """Find all Rust and TypeScript source files, excluding generated/noise."""
    files: list[Path] = []
    search_roots = [repo_root / "src"]
    website_dir = repo_root / "website"
    if website_dir.exists():
        search_roots.append(website_dir)

    for search_root in search_roots:
        for path in sorted(search_root.rglob("*")):
            rel_parts = path.relative_to(repo_root).parts
            if any(part in SKIP_DIRS for part in rel_parts):
                continue
            if path.is_file() and not should_skip_file(path):
                if path.suffix in (".rs", ".ts", ".tsx"):
                    files.append(path)

    return files


def check_header(path: Path) -> tuple[bool, str]:
    """Check if a file has the correct copyright header.

    Returns (ok, message).
    """
    content = path.read_text()
    first_lines = content.splitlines()[:10]
    first_chunk = "\n".join(first_lines)

    match = COPYRIGHT_RE.search(first_chunk)
    if not match:
        return False, "missing copyright header"

    year = match.group(1)
    author = match.group(2)

    errors: list[str] = []
    if year != EXPECTED_YEAR:
        errors.append(f"wrong year: {year} (expected {EXPECTED_YEAR})")
    if author != "The Simlin Authors":
        errors.append(f"wrong author: '{author}' (expected 'The Simlin Authors')")

    if errors:
        return False, "; ".join(errors)

    return True, ""


def fix_header(path: Path) -> bool:
    """Fix the copyright header in a file. Returns True if file was modified."""
    content = path.read_text()

    if not content.strip():
        path.write_text(HEADER_TEXT + "\n")
        return True

    first_lines = content.splitlines()[:10]
    first_chunk = "\n".join(first_lines)
    match = COPYRIGHT_RE.search(first_chunk)

    if match:
        # Header exists but has wrong year or author -- fix in place.
        old_text = match.group(0)
        new_text = f"Copyright {EXPECTED_YEAR} The Simlin Authors. All rights reserved."
        if old_text != new_text:
            content = content.replace(old_text, new_text, 1)
            path.write_text(content)
            return True
        return False

    # No header at all -- prepend the canonical line-comment form.
    path.write_text(HEADER_TEXT + "\n\n" + content)
    return True


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent
    fix_mode = "--fix" in sys.argv

    files = find_source_files(repo_root)
    errors: list[str] = []
    fixed = 0

    for path in files:
        ok, message = check_header(path)
        if not ok:
            rel = path.relative_to(repo_root)
            if fix_mode:
                if fix_header(path):
                    fixed += 1
                    print(f"  Fixed: {rel} ({message})")
            else:
                errors.append(f"{rel}: {message}")

    if fix_mode:
        if fixed:
            print(f"Fixed {fixed} file(s).")
        else:
            print("All copyright headers are correct.")
        return 0

    if errors:
        # Per-file errors to stdout (one per line) for machine consumption.
        for err in errors:
            print(err)
        # Human-readable summary to stderr (not captured by lint-project.sh).
        print(
            f"{len(errors)} file(s) with copyright header issues.",
            file=sys.stderr,
        )
        print(
            "Run 'python3 scripts/check-copyright.py --fix' to auto-fix.",
            file=sys.stderr,
        )
        return 1

    print("Copyright header check passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
