#!/usr/bin/env python3
"""Build and stage the notebook widget's assets into ``simlin/_widget/``.

pattern: Functional Core (manifest/verification helpers) + Imperative Shell
(``main``: pnpm build, file copies)

pysimlin's ``ModelWidget`` ships two build outputs of the TypeScript
workspace as package data:

- ``simlin/_widget/widget.js`` -- ``src/notebook-widget``'s single-file
  anywidget module (``dist/widget.js``);
- ``simlin/_widget/libsimlin-browser.wasm`` -- the engine, from
  ``src/engine/core/`` (``src/engine/build.sh`` runs ``wasm-opt`` over it
  when binaryen is installed and stamps the mode in a ``.mode`` sibling).

This script is the ONE owner of that staging: the pysimlin ``Makefile``
(``make assets``), ``scripts/build_wheels.py``, the notebook-widget package's
``pnpm stage`` and the release workflow all call it rather than copying the
files themselves. Beside the two assets it writes ``ASSETS.json`` -- the
source commit, the wasm-opt mode, and each file's size and sha256 -- which is
deterministic (no timestamps), so re-running on an unchanged tree rewrites
nothing and every platform wheel built from one staging carries byte-identical
assets. ``setup.py`` reads the manifest to refuse building a wheel/sdist from
missing, empty or inconsistent assets.

Usage (flags combine):

- no flags: pnpm build of the widget and its deps, then stage
- ``--no-build``: stage from the existing build outputs
- ``--check``: verify what is staged; exit 1 on a problem
- ``--require-opt``: the wasm must be wasm-opt'd (release builds)

Only the standard library is used: ``setup.py`` imports the verification
helpers from an isolated build environment where nothing else is guaranteed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path


def _repo_root() -> Path:
    # <repo>/src/pysimlin/scripts/<this file>. In an unpacked sdist the tree
    # is shallower and no repo exists; only ``verify_staged`` is used there,
    # so any directory will do -- but the lookup must not raise at import.
    parents = Path(__file__).resolve().parents
    return parents[3] if len(parents) > 3 else parents[-1]


REPO_ROOT = _repo_root()
PYSIMLIN_DIR = REPO_ROOT / "src" / "pysimlin"
DEFAULT_DEST = PYSIMLIN_DIR / "simlin" / "_widget"

WIDGET_JS = "widget.js"
WASM_FILE = "libsimlin-browser.wasm"
MANIFEST_FILE = "ASSETS.json"
ASSET_FILES = (WIDGET_JS, WASM_FILE)

WIDGET_DIST_JS = REPO_ROOT / "src" / "notebook-widget" / "dist" / WIDGET_JS
ENGINE_WASM = REPO_ROOT / "src" / "engine" / "core" / WASM_FILE
# Written by src/engine/build.sh: "opt" when wasm-opt ran, "raw" otherwise.
ENGINE_WASM_MODE = ENGINE_WASM.with_name(WASM_FILE + ".mode")

MANIFEST_SCHEMA = 1
WASM_MODE_OPT = "opt"
WASM_MODE_UNKNOWN = "unknown"

# `...` selects the widget package AND its workspace dependencies (engine,
# core, diagram) in topological order, so this is a from-scratch-safe build.
BUILD_COMMAND = ("pnpm", "--filter", "@simlin/notebook-widget...", "run", "build")


# ── functional core ─────────────────────────────────────────────────────


@dataclass(frozen=True)
class FileDigest:
    size: int
    sha256: str

    @staticmethod
    def of_bytes(data: bytes) -> FileDigest:
        return FileDigest(size=len(data), sha256=hashlib.sha256(data).hexdigest())

    @staticmethod
    def of_path(path: Path) -> FileDigest:
        h = hashlib.sha256()
        size = 0
        with path.open("rb") as f:
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
                size += len(chunk)
        return FileDigest(size=size, sha256=h.hexdigest())


def build_manifest(
    files: dict[str, FileDigest],
    *,
    source_commit: str | None,
    source_dirty: bool,
    wasm_mode: str,
) -> dict[str, object]:
    """The manifest as a JSON-ready dict.

    Deterministic by construction: no timestamps, no host details, keys sorted
    on serialisation. Two stagings of the same tree therefore produce the same
    bytes, which is what makes the "up to date" short-circuit and the
    cross-platform identical-assets guarantee checkable.
    """
    return {
        "schema": MANIFEST_SCHEMA,
        "source_commit": source_commit,
        "source_dirty": source_dirty,
        "wasm_opt": wasm_mode,
        "files": {
            name: {"size": digest.size, "sha256": digest.sha256}
            for name, digest in sorted(files.items())
        },
    }


def manifest_json(manifest: dict[str, object]) -> str:
    return json.dumps(manifest, indent=2, sort_keys=True) + "\n"


def parse_manifest(text: str) -> dict[str, object] | None:
    """The manifest dict, or ``None`` when the text is not a manifest we
    understand (invalid JSON, wrong shape, or a schema we do not know)."""
    try:
        data = json.loads(text)
    except ValueError:
        return None
    if not isinstance(data, dict) or data.get("schema") != MANIFEST_SCHEMA:
        return None
    files = data.get("files")
    if not isinstance(files, dict):
        return None
    for entry in files.values():
        if not isinstance(entry, dict) or not isinstance(entry.get("sha256"), str):
            return None
        if not isinstance(entry.get("size"), int):
            return None
    return data


def manifest_digests(manifest: dict[str, object]) -> dict[str, FileDigest]:
    files = manifest["files"]
    assert isinstance(files, dict)
    return {name: FileDigest(size=e["size"], sha256=e["sha256"]) for name, e in files.items()}


@dataclass
class Verification:
    """The outcome of checking a staged asset directory.

    ``errors`` are conditions a wheel must never ship with (missing/empty
    files, a manifest that does not describe them); ``warnings`` are things
    worth telling a human about (assets built from another commit or a dirty
    tree, an un-optimised wasm when that was not required).
    """

    errors: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)

    @property
    def ok(self) -> bool:
        return not self.errors


def verify_staged(
    dest: Path,
    *,
    require_opt: bool = False,
    head_commit: str | None = None,
) -> Verification:
    """Check that ``dest`` holds complete, non-empty, manifest-consistent assets.

    ``head_commit`` (when known) is compared against the manifest's source
    commit; a mismatch is a warning, not an error, because a wheel legitimately
    ships assets staged one commit earlier (a version-bump commit) and the
    hashes are the real guarantee of what is inside.
    """
    v = Verification()
    for name in ASSET_FILES:
        path = dest / name
        if not path.is_file():
            v.errors.append(f"{name} is missing from {dest}")
        elif path.stat().st_size == 0:
            v.errors.append(f"{name} in {dest} is empty")

    manifest_path = dest / MANIFEST_FILE
    if not manifest_path.is_file():
        v.errors.append(f"{MANIFEST_FILE} is missing from {dest}")
        return v
    manifest = parse_manifest(manifest_path.read_text(encoding="utf-8"))
    if manifest is None:
        v.errors.append(f"{manifest_path} is not a schema-{MANIFEST_SCHEMA} asset manifest")
        return v

    recorded = manifest_digests(manifest)
    for name in ASSET_FILES:
        path = dest / name
        if name not in recorded:
            v.errors.append(f"{MANIFEST_FILE} does not describe {name}")
        elif path.is_file() and FileDigest.of_path(path) != recorded[name]:
            v.errors.append(f"{name} does not match its {MANIFEST_FILE} entry (stale staging?)")

    mode = manifest.get("wasm_opt")
    if mode != WASM_MODE_OPT:
        msg = f"{WASM_FILE} was staged without wasm-opt (mode {mode!r})"
        (v.errors if require_opt else v.warnings).append(msg)

    commit = manifest.get("source_commit")
    if head_commit is not None and commit != head_commit:
        v.warnings.append(f"assets were staged from commit {commit}, HEAD is {head_commit}")
    if manifest.get("source_dirty"):
        v.warnings.append("assets were staged from a working tree with uncommitted changes")
    return v


# Escape hatch for deliberately building a package without the widget (e.g.
# measuring what the assets cost). Never set this in a release path.
ALLOW_MISSING_ENV = "SIMLIN_ALLOW_MISSING_WIDGET_ASSETS"


def packaging_guard(
    dest: Path,
    *,
    command: str,
    head_commit: str | None,
    allow_missing: bool,
) -> tuple[list[str], str | None]:
    """Decide whether a packaging ``command`` (``bdist_wheel``/``sdist``) may run.

    Returns ``(messages_to_print, refusal)``: ``refusal`` is ``None`` when the
    command may proceed, else the text to abort with. ``allow_missing`` (the
    ``SIMLIN_ALLOW_MISSING_WIDGET_ASSETS=1`` bypass) skips the check entirely
    but still says so on stdout, so a package built without the widget is
    never built silently. Pure: ``setup.py`` and its tests both call this.
    """
    if allow_missing:
        return [f"warning: {ALLOW_MISSING_ENV}=1: {command} without checking widget assets"], None
    v = verify_staged(dest, head_commit=head_commit)
    messages = [f"warning: widget assets: {w}" for w in v.warnings]
    if v.ok:
        return messages, None
    problems = "\n".join(f"  - {e}" for e in v.errors)
    refusal = (
        f"refusing to run {command}: the notebook widget assets are not staged:\n"
        f"{problems}\n"
        f"Run `python scripts/stage_widget_assets.py` (or `make assets`) first, "
        f"or set {ALLOW_MISSING_ENV}=1 to build a package without the widget."
    )
    return messages, refusal


def is_up_to_date(dest: Path, sources: dict[str, Path], manifest_text: str) -> bool:
    """True when every staged file equals its source and the manifest text is
    exactly what would be written -- i.e. staging would be a no-op."""
    manifest_path = dest / MANIFEST_FILE
    if not manifest_path.is_file() or manifest_path.read_text(encoding="utf-8") != manifest_text:
        return False
    for name, src in sources.items():
        staged = dest / name
        if not staged.is_file() or FileDigest.of_path(staged) != FileDigest.of_path(src):
            return False
    return True


# ── imperative shell ────────────────────────────────────────────────────


def git_output(*args: str, cwd: Path = REPO_ROOT) -> str | None:
    try:
        proc = subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True)
    except (OSError, subprocess.CalledProcessError):
        return None
    return proc.stdout.strip()


def head_commit() -> str | None:
    return git_output("rev-parse", "HEAD")


def tree_is_dirty() -> bool:
    status = git_output("status", "--porcelain", "--untracked-files=no")
    return bool(status)


def read_wasm_mode() -> str:
    if not ENGINE_WASM_MODE.is_file():
        return WASM_MODE_UNKNOWN
    return ENGINE_WASM_MODE.read_text(encoding="utf-8").strip() or WASM_MODE_UNKNOWN


def run_build() -> None:
    print("Building @simlin/notebook-widget and its workspace dependencies...", flush=True)
    subprocess.run(BUILD_COMMAND, cwd=REPO_ROOT, check=True)


def stage(dest: Path, *, require_opt: bool) -> int:
    sources = {WIDGET_JS: WIDGET_DIST_JS, WASM_FILE: ENGINE_WASM}
    missing = [str(p) for p in sources.values() if not p.is_file()]
    if missing:
        print(
            "error: build outputs missing; run without --no-build (or `pnpm build`) first:",
            file=sys.stderr,
        )
        for m in missing:
            print(f"  {m}", file=sys.stderr)
        return 1

    wasm_mode = read_wasm_mode()
    if require_opt and wasm_mode != WASM_MODE_OPT:
        print(
            f"error: {ENGINE_WASM} was built without wasm-opt (mode {wasm_mode!r}); "
            "install binaryen (scripts/install-binaryen.sh) and rebuild without "
            "DISABLE_WASM_OPT",
            file=sys.stderr,
        )
        return 1

    manifest = build_manifest(
        {name: FileDigest.of_path(src) for name, src in sources.items()},
        source_commit=head_commit(),
        source_dirty=tree_is_dirty(),
        wasm_mode=wasm_mode,
    )
    text = manifest_json(manifest)

    dest.mkdir(parents=True, exist_ok=True)
    if is_up_to_date(dest, sources, text):
        print(f"widget assets in {dest} are up to date")
    else:
        # Assets first, manifest last: a manifest is only ever on disk once
        # the files it describes are, so an interrupted staging fails
        # verification (hash mismatch / missing manifest) rather than passing.
        (dest / MANIFEST_FILE).unlink(missing_ok=True)
        for name, src in sources.items():
            shutil.copyfile(src, dest / name)
        (dest / MANIFEST_FILE).write_text(text, encoding="utf-8")
        print(f"staged widget assets into {dest}")

    for name, digest in manifest_digests(manifest).items():
        print(f"  {name}: {digest.size} bytes sha256={digest.sha256}")
    print(f"  wasm-opt: {wasm_mode}; source commit: {manifest['source_commit']}")
    return 0


def check(dest: Path, *, require_opt: bool) -> int:
    v = verify_staged(dest, require_opt=require_opt, head_commit=head_commit())
    for w in v.warnings:
        print(f"warning: {w}")
    for e in v.errors:
        print(f"error: {e}", file=sys.stderr)
    if v.ok:
        print(f"widget assets in {dest} verified")
        return 0
    return 1


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="do not run the pnpm build; stage the existing dist/core outputs",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="verify the staged assets instead of staging (implies --no-build)",
    )
    parser.add_argument(
        "--require-opt",
        action="store_true",
        help="fail unless the wasm was built with wasm-opt (release builds)",
    )
    parser.add_argument(
        "--dest",
        type=Path,
        default=DEFAULT_DEST,
        help=f"destination directory (default: {DEFAULT_DEST})",
    )
    args = parser.parse_args(argv)

    if args.check:
        return check(args.dest, require_opt=args.require_opt)
    if not args.no_build:
        run_build()
    return stage(args.dest, require_opt=args.require_opt)


if __name__ == "__main__":
    sys.exit(main())
