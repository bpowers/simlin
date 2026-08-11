#!/usr/bin/env python3
# Copyright 2026 The Simlin Authors. All rights reserved.
# Use of this source code is governed by the Apache License,
# Version 2.0, that can be found in the LICENSE file.

"""A path-filtered workflow's `push` and `pull_request` `paths` lists must match.

They are maintained by hand and read as one filter, so an entry added to only one
of them silently means "runs on merge but not on the PR" (or the reverse) -- a gap
that looks like coverage.

Writes one error per line to stdout and exits non-zero when any workflow differs.

Deliberately parses the subset of YAML this needs with the standard library rather
than importing PyYAML, which this repo does not declare or install: an undeclared
import turns the check into a no-op on any machine that happens to lack it, which
is the same silent-non-coverage failure the rule exists to catch. The parser is
strict in the direction that matters -- it raises rather than returning nothing
when it meets a shape it does not understand, so a workflow it cannot read fails
loudly instead of passing vacuously.
"""

from __future__ import annotations

import glob
import re
import sys

KEY_RE = re.compile(r"^(?P<indent> *)(?P<key>\"[^\"]+\"|'[^']+'|[A-Za-z_][\w-]*)\s*:\s*(?P<rest>.*?)\s*$")
ITEM_RE = re.compile(r"^(?P<indent> *)-\s+(?P<value>.*?)\s*$")


def _unquote(text: str) -> str:
    text = text.strip()
    if len(text) >= 2 and text[0] == text[-1] and text[0] in "\"'":
        return text[1:-1]
    return text


def _strip_comment(text: str) -> str:
    # Only an unquoted `#` starts a comment. Workflow path entries are quoted
    # or bare globs, neither of which contains one, so this stays simple.
    if text.startswith(("'", '"')):
        return text
    return text.split("#", 1)[0].strip()


def _significant(lines: list[str]) -> list[tuple[int, str]]:
    """(index, line) for lines that are neither blank nor whole-line comments."""
    out = []
    for i, line in enumerate(lines):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        out.append((i, line))
    return out


def trigger_paths(path: str) -> dict[str, list[str]]:
    """`{trigger: paths}` for the `push`/`pull_request` triggers that declare `paths`.

    Raises ValueError on a shape this parser does not understand.
    """
    with open(path, encoding="utf-8") as fh:
        lines = fh.read().splitlines()
    sig = _significant(lines)

    # Locate the top-level trigger block. GitHub reads bare `on` as the YAML
    # boolean true, so workflows here quote it; accept both spellings.
    on_at = None
    for pos, (_, line) in enumerate(sig):
        m = KEY_RE.match(line)
        if m and len(m.group("indent")) == 0 and _unquote(m.group("key")) == "on":
            on_at = pos
            break
    if on_at is None:
        return {}

    # Everything indented under `on:` until the next top-level key.
    block = []
    for _, line in sig[on_at + 1 :]:
        m = KEY_RE.match(line)
        if m and len(m.group("indent")) == 0:
            break
        block.append(line)

    result: dict[str, list[str]] = {}
    trigger = None
    trigger_indent = None
    in_paths = False
    paths_indent = None

    for line in block:
        item = ITEM_RE.match(line)
        if item and in_paths and len(item.group("indent")) > paths_indent:
            result[trigger].append(_unquote(_strip_comment(item.group("value"))))
            continue

        key_m = KEY_RE.match(line)
        if not key_m:
            if in_paths:
                raise ValueError(f"{path}: unparsed line inside a `paths` list: {line!r}")
            continue

        indent = len(key_m.group("indent"))
        key = _unquote(key_m.group("key"))

        if trigger_indent is not None and indent <= trigger_indent:
            trigger = None
            in_paths = False
        if in_paths and indent <= paths_indent:
            in_paths = False
        elif in_paths:
            # `paths` is a flat list of strings, so a mapping key nested inside
            # it is a shape this parser does not model. Raise rather than skip:
            # silently ignoring it would let a workflow the parser cannot read
            # report "no difference", which is the non-coverage this rule exists
            # to catch.
            raise ValueError(f"{path}: unexpected key inside a `paths` list: {line!r}")

        if key in ("push", "pull_request") and trigger is None:
            trigger = key
            trigger_indent = indent
            continue

        if key == "paths" and trigger is not None:
            if key_m.group("rest"):
                raise ValueError(f"{path}: inline `paths:` value is not supported: {line!r}")
            in_paths = True
            paths_indent = indent
            result.setdefault(trigger, [])

    if in_paths and not result.get(trigger):
        raise ValueError(f"{path}: `paths:` under `{trigger}` parsed as empty")
    return result


def main() -> int:
    status = 0
    for path in sorted(glob.glob(".github/workflows/*.y*ml")):
        try:
            triggers = trigger_paths(path)
        except (OSError, ValueError) as exc:
            print(f"{path}: could not check trigger paths: {exc}")
            status = 1
            continue

        push = triggers.get("push")
        pull = triggers.get("pull_request")
        if push is None and pull is None:
            continue
        if push != pull:
            only_push = [p for p in (push or []) if p not in (pull or [])]
            only_pull = [p for p in (pull or []) if p not in (push or [])]
            print(
                f"{path}: push and pull_request `paths` differ; "
                f"push-only={only_push} pull_request-only={only_pull}"
            )
            status = 1
    return status


if __name__ == "__main__":
    sys.exit(main())
