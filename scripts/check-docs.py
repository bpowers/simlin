#!/usr/bin/env python3
"""Validate path references in CLAUDE.md and docs/ files.

Checks:
  - Backtick-quoted paths in CLAUDE.md files that contain `/`
    (indicating an actual file path, not just a filename mention).
  - Markdown link targets [text](path) in CLAUDE.md and docs/ files
    where the target is a local path (not a URL).
  - "Last reviewed/updated/verified" stamp comments in CLAUDE.md files,
    which the root CLAUDE.md's comment standards ban (they go stale
    immediately and are rebase-conflict magnets).

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
import subprocess
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


def blank_inline_code_spans(content: str) -> str:
    """Replace single-line inline code spans (`...`) with same-length runs of
    spaces so code/math notation like `s[d](t)` isn't parsed as a `[text](target)`
    markdown link. Length is preserved so reported line numbers stay accurate."""
    return re.sub(r"`[^`\n]+`", lambda m: " " * len(m.group(0)), content)


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


def is_git_ignored(ref: str, file_dir: Path, repo_root: Path) -> bool:
    """True when git would ignore ``ref`` (resolved like resolve_path does)."""
    for base in (file_dir, repo_root):
        candidate = (base / ref)
        try:
            rel = candidate.resolve().relative_to(repo_root.resolve())
        except ValueError:
            continue
        result = subprocess.run(
            ["git", "-C", str(repo_root), "check-ignore", "-q", "--", str(rel)],
            capture_output=True,
            check=False,
        )
        if result.returncode == 0:
            return True
    return False


def check_file(file_path: Path, repo_root: Path) -> list[str]:
    """Check a single file for broken path references."""
    errors: list[str] = []
    raw_content = file_path.read_text()
    content = strip_fenced_code_blocks(raw_content)
    # Link-target detection runs against a copy with inline code spans blanked
    # out, so notation like `s[d](t)` inside backticks isn't mistaken for a link.
    # The backtick-path check below still needs the spans, so it uses `content`.
    content_links = blank_inline_code_spans(content)
    file_dir = file_path.parent
    rel_path = file_path.relative_to(repo_root)

    # Check markdown link targets: [text](path)
    for match in re.finditer(r'\[([^\]]*)\]\(([^)\s]+)\)', content_links):
        target = match.group(2)
        # Skip URLs and anchors
        if target.startswith(("http://", "https://", "mailto:", "#")):
            continue
        # Strip anchor fragments
        target = target.split("#")[0]
        if not target:
            continue
        if resolve_path(target, file_dir, repo_root) is None:
            line_num = content_links[:match.start()].count("\n") + 1
            errors.append(f"{rel_path}:{line_num}: broken link target '{target}'")

    # Reject "Last reviewed/updated/verified" stamps in CLAUDE.md files.
    # Matched narrowly (comment form, or a stamp starting its own line) so the
    # root CLAUDE.md's prose *describing* the ban doesn't trip the check, and
    # scanned on content_links (fenced blocks stripped, inline code spans
    # blanked -- both line-number-preserving) so an example of the banned form
    # in a code block or backticks isn't rejected as a real stamp.
    if file_path.name == "CLAUDE.md":
        stamp_re = re.compile(
            r'(<!--\s*Last\s+(?:reviewed|updated|verified)'
            r'|^\s*Last\s+(?:reviewed|updated|verified)\s*:)',
            re.IGNORECASE | re.MULTILINE,
        )
        for match in stamp_re.finditer(content_links):
            line_num = content_links[:match.start()].count("\n") + 1
            errors.append(
                f"{rel_path}:{line_num}: 'Last reviewed/updated/verified' stamp "
                "(banned by root CLAUDE.md comment standards -- rely on git history)"
            )

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
            # Skip npm scoped package names (e.g. @simlin/mcp, @simlin/mcp-linux-x64)
            if re.match(r'^@[\w-]+/[\w-]+', token):
                continue
            # Skip glob patterns
            if "*" in token:
                continue
            # Skip references INTO generated output directories (the same set
            # the walker below refuses to descend into): a CLAUDE.md legitimately
            # names its package's build artifact (`dist/widget.js`), and whether
            # that file exists depends on what was last built in this checkout,
            # not on whether the docs are correct.
            if any(part in GENERATED_DIRS for part in token.split()[0].split("/")[:-1]):
                continue
            # Skip XML/XMILE tag tokens (e.g. `<overflow/>`, `<leak_integers/>`).
            # The trailing slash of a self-closing tag is not a path separator.
            if token.startswith("<") and token.endswith(">"):
                continue
            # For tokens with arguments (e.g. "scripts/foo.sh <version>"),
            # check only the path portion before the first space
            path_to_check = token.split()[0] if " " in token else token
            if resolve_path(path_to_check, file_dir, repo_root) is None:
                # A path git ignores is a build or runtime output (staged wheel
                # assets, e2e logs); its presence depends on what was last run
                # here, so a doc may name it without it existing.
                if is_git_ignored(path_to_check, file_dir, repo_root):
                    continue
                line_num = content[:match.start()].count("\n") + 1
                errors.append(f"{rel_path}:{line_num}: broken path reference '{token}'")

    return errors


# Build outputs and vendored trees: never walked for CLAUDE.md files, and a
# path reference into one of them is not checked for existence.
GENERATED_DIRS = frozenset(
    ("node_modules", "target", "build", "dist", "lib", "lib.browser", "lib.module",
     "third_party", ".claude-scratch")
)


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent

    # Collect files to check
    files_to_check: list[Path] = []

    # All CLAUDE.md files
    for claude_md in repo_root.rglob("CLAUDE.md"):
        # Skip generated/noise directories. `.claude-scratch` is git-excluded
        # agent scratch space INSIDE the repo, so a working copy of a CLAUDE.md
        # routinely lands there mid-task; walking it reports every relative path
        # in that copy as broken (~150 errors) and fails the pre-commit hook on
        # a tree that is actually fine.
        rel = claude_md.relative_to(repo_root)
        parts = rel.parts
        if any(p in GENERATED_DIRS for p in parts):
            continue
        # `.claude/worktrees` holds git-worktree checkouts -- complete copies of
        # the repo at other branches -- so their CLAUDE.md files reflect other
        # revisions, not this tree.
        if parts[:2] == (".claude", "worktrees"):
            continue
        files_to_check.append(claude_md)

    # All files in docs/ (markdown only)
    doc_dir = repo_root / "docs"
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
