#!/usr/bin/env python3
"""Validate path references in CLAUDE.md and doc/ files.

Checks:
  - Backtick-quoted paths in CLAUDE.md files that contain `/`
    (indicating an actual file path, not just a filename mention).
  - Markdown link targets [text](path) in CLAUDE.md and doc/ files
    where the target is a local path (not a URL).

Paths are resolved relative to the file's directory first, then repo root.

Does NOT check:
  - Bare filenames in backticks without `/` (e.g. `Canvas.tsx`)
  - Code identifiers in backticks (e.g. `Result`)
  - URL links
  - Content freshness
  - Content inside fenced code blocks
"""

from __future__ import annotations

import re
import sys
from pathlib import Path


def strip_fenced_code_blocks(content: str) -> str:
    """Replace fenced code block content with empty lines to preserve line numbers."""
    result: list[str] = []
    in_block = False
    for line in content.splitlines(keepends=True):
        stripped = line.strip()
        if stripped.startswith("```"):
            in_block = not in_block
            result.append("\n")
        elif in_block:
            result.append("\n")
        else:
            result.append(line)
    return "".join(result)


def resolve_path(ref: str, file_dir: Path, repo_root: Path) -> Path | None:
    """Try to resolve a path reference, returning the resolved Path or None."""
    # Strip leading / which means repo-root-relative
    if ref.startswith("/"):
        candidate = repo_root / ref.lstrip("/")
        if candidate.exists():
            return candidate
        return None

    # Try relative to the file's directory first
    candidate = file_dir / ref
    if candidate.exists():
        return candidate

    # Try relative to repo root
    candidate = repo_root / ref
    if candidate.exists():
        return candidate

    return None


def check_file(file_path: Path, repo_root: Path) -> list[str]:
    """Check a single file for broken path references."""
    errors: list[str] = []
    raw_content = file_path.read_text()
    content = strip_fenced_code_blocks(raw_content)
    file_dir = file_path.parent
    rel_path = file_path.relative_to(repo_root)

    # Check markdown link targets: [text](path)
    for match in re.finditer(r'\[([^\]]*)\]\(([^)\s]+)\)', content):
        target = match.group(2)
        # Skip URLs and anchors
        if target.startswith(("http://", "https://", "mailto:", "#")):
            continue
        # Strip anchor fragments
        target = target.split("#")[0]
        if not target:
            continue
        if resolve_path(target, file_dir, repo_root) is None:
            line_num = content[:match.start()].count("\n") + 1
            errors.append(f"{rel_path}:{line_num}: broken link target '{target}'")

    # Check backtick-quoted paths in CLAUDE.md files only
    # Only check tokens that contain `/` (actual paths, not bare filenames)
    if file_path.name == "CLAUDE.md":
        # Match single-line backtick tokens only (no newlines inside)
        for match in re.finditer(r'`([^`\n]+)`', content):
            token = match.group(1)
            # Only check tokens with `/` -- these are actual path references
            if "/" not in token:
                continue
            # Skip command-like tokens
            if token.startswith(("cargo ", "pnpm ", "git ", "npm ", "cd ", "uv ",
                                 "python ", "ruff ", "mypy ", "pytest ",
                                 "--", "RUST_", "DISABLE_")):
                continue
            # Skip glob patterns
            if "*" in token:
                continue
            if resolve_path(token, file_dir, repo_root) is None:
                line_num = content[:match.start()].count("\n") + 1
                errors.append(f"{rel_path}:{line_num}: broken path reference '{token}'")

    return errors


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent

    # Collect files to check
    files_to_check: list[Path] = []

    # All CLAUDE.md files
    for claude_md in repo_root.rglob("CLAUDE.md"):
        # Skip generated/noise directories
        rel = claude_md.relative_to(repo_root)
        parts = rel.parts
        if any(p in ("node_modules", "target", "build", "lib", "lib.browser", "lib.module",
                     "third_party")
               for p in parts):
            continue
        files_to_check.append(claude_md)

    # All files in doc/ (markdown only)
    doc_dir = repo_root / "doc"
    if doc_dir.exists():
        for md_file in doc_dir.rglob("*.md"):
            files_to_check.append(md_file)

    errors: list[str] = []
    for f in sorted(files_to_check):
        errors.extend(check_file(f, repo_root))

    if errors:
        for err in errors:
            print(err, file=sys.stderr)
        return 1

    print("Documentation link check passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
