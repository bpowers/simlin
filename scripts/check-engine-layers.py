#!/usr/bin/env python3
# Copyright 2026 The Simlin Authors. All rights reserved.
# Use of this source code is governed by the Apache License,
# Version 2.0, that can be found in the LICENSE file.

"""Validate simlin-engine's module references against engine-layers.json.

Every module at the crate root belongs to one layer, and a layer lists the
layers its production code may use besides its own. A reference to a module of
any other layer is an error; there is no list of exceptions.

A reference is a path from the crate root (`crate::`, `$crate::`, `self::` and
`super::`, each through a `{...}` group), in code, in an attribute, or in an
attribute's string, and a use of a macro another module exports. A name lib.rs
re-exports is a reference to the module it comes from.

Production code is what a build without tests can compile. What is under
`#[test]` or `#[cfg(test)]` (or under a feature in `test_features`, which only
test targets turn on) is skipped, as is a file mounted under one: files are
found by following `mod` declarations from lib.rs, so what is test code is what
the crate says is, not what a file is called.

Every crate-root module must be in the table, and every module in the table must
exist, so a module is placed when it is added.

Does NOT check a call of a method that an `impl` in a higher layer gives a lower
layer's type: it is not spelled with a path. Such an `impl` cannot be written
across a crate boundary.

Exit code 0 on success, 1 on any violation.
"""

from __future__ import annotations

import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

TOKEN_RE = re.compile(
    r"""
      (?P<space>\s+|//[^\n]*)
    | (?P<comment>/\*)
    | (?P<raw_string>[bc]?r(?P<hashes>\#*)"(?s:.)*?"(?P=hashes))
    | (?P<string>[bc]?"(?:\\(?s:.)|[^"\\])*")
    | (?P<char>b?'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}|.)|[^\\'])')
    | (?P<ident>(?:r\#)?[^\W\d]\w*)
    | (?P<number>\d\w*(?:\.\d\w*)?)
    | (?P<punct>::|->|=>|.)
    """,
    re.VERBOSE,
)
PATH_STRING_RE = re.compile(r'"((?:crate|self|super)(?:::\w+)+)"')

CLOSERS = {"(": ")", "[": "]", "{": "}"}

# What an item's first word says about where it ends: at a `;`, or at its
# `{...}` (or a `;` where it has none). Anything else is a field, a variant, an
# arm or an expression, which a `,` can end.
SEMICOLON_ITEMS = frozenset("let static type use".split())
BLOCK_ITEMS = frozenset(
    "async enum extern fn for if impl loop macro_rules match mod struct trait union unsafe while {".split()
)

Segments = tuple[str, ...]


class LayerError(Exception):
    """The source or the table has a shape the check does not model."""


@dataclass(frozen=True)
class Token:
    kind: str
    text: str
    line: int


@dataclass
class Contents:
    """What the production code of one file holds."""

    # Paths from the crate root, `*` the last segment of a glob, with the line.
    references: list[tuple[Segments, int]] = field(default_factory=list)
    # Macros invoked by a bare name, with the line.
    macro_uses: list[tuple[str, int]] = field(default_factory=list)
    exported_macros: list[str] = field(default_factory=list)
    # Modules mounted from other files: the module, its `#[path]`, the line.
    mounts: list[tuple[Segments, str | None, int]] = field(default_factory=list)
    # What the file's own `use` items bind: the name, and the path as written.
    imports: list[tuple[str, Segments]] = field(default_factory=list)


def tokenize(source: str) -> list[Token]:
    """The tokens of a Rust file, without whitespace and comments."""
    tokens: list[Token] = []
    pos, line = 0, 1
    while pos < len(source):
        match = TOKEN_RE.match(source, pos)
        assert match is not None and match.lastgroup is not None  # `.` matches anything
        end = match.end()
        if match.lastgroup == "comment":
            end = block_comment_end(source, pos)
        elif match.lastgroup != "space":
            tokens.append(Token(match.lastgroup, match.group(), line))
        line += source.count("\n", pos, end)
        pos = end
    return tokens


def block_comment_end(source: str, pos: int) -> int:
    """The index past the block comment opening at `pos`; Rust's nest."""
    depth = 0
    for match in re.compile(r"/\*|\*/").finditer(source, pos):
        depth += 1 if match.group() == "/*" else -1
        if depth == 0:
            return match.end()
    return len(source)


class Reader:
    """A file's tokens, and the grammar the check needs of them."""

    def __init__(self, source: str, test_features: frozenset[str]) -> None:
        self.tokens = tokenize(source)
        self.test_features = test_features

    def text(self, i: int) -> str:
        return self.tokens[i].text if 0 <= i < len(self.tokens) else ""

    def punct(self, i: int) -> str:
        return self.text(i) if 0 <= i < len(self.tokens) and self.tokens[i].kind == "punct" else ""

    def is_ident(self, i: int) -> bool:
        return 0 <= i < len(self.tokens) and self.tokens[i].kind == "ident"

    def group_end(self, i: int) -> int:
        """The index past the bracketed group opening at `i`."""
        depth = 0
        for j in range(i, len(self.tokens)):
            if self.punct(j) in CLOSERS:
                depth += 1
            elif self.punct(j) in CLOSERS.values():
                depth -= 1
                if depth == 0:
                    return j + 1
        raise LayerError(f"line {self.tokens[i].line}: unclosed '{self.text(i)}'")

    def item_end(self, i: int) -> int:
        """The index past the item, statement, field, variant or arm at `i`."""
        if self.text(i) == "pub":
            i = self.group_end(i + 1) if self.punct(i + 1) == "(" else i + 1
        head, after = self.text(i), self.text(i + 1)
        if head == "const":
            ends_at = "{" if after in ("fn", "unsafe", "async", "extern") else ";"
        elif head in SEMICOLON_ITEMS:
            ends_at = ";"
        else:
            ends_at = "{" if head in BLOCK_ITEMS else ","

        # A `,` inside `<...>` ends nothing. Past an arm's `=>` the `<` is an
        # operator.
        angles, in_type = 0, ends_at == ","
        while i < len(self.tokens):
            punct = self.punct(i)
            if punct in CLOSERS.values():
                return i
            if punct == ";" or (punct == "," and ends_at == "," and angles <= 0):
                return i + 1
            if punct in CLOSERS:
                # An arm's block and a braced macro call end like a block item.
                is_block = ends_at == "{" or (ends_at == "," and self.punct(i - 1) in ("=>", "!"))
                i = self.group_end(i)
                if punct == "{" and is_block and self.text(i) != "else":
                    return i
                continue
            if punct == "=>":
                angles, in_type = 0, False
            elif in_type and punct in ("<", ">"):
                angles += 1 if punct == "<" else -1
            i += 1
        return i

    def cfg(self, i: int) -> tuple[bool | None, int]:
        """What the cfg predicate at `i` is in a build without tests (None when
        the build decides it), and the index past it."""
        name = self.text(i)
        if self.punct(i + 1) == "=":
            in_tests = name == "feature" and self.text(i + 2).strip('"') in self.test_features
            return (False if in_tests else None), i + 3
        if self.punct(i + 1) != "(":
            return (False if name == "test" else None), i + 1
        end = self.group_end(i + 1)
        values: list[bool | None] = []
        i += 2
        while i < end - 1:
            value, i = self.cfg(i)
            values.append(value)
            i += self.punct(i) == ","
        if name == "not":
            return (None if values[0] is None else not values[0]), end
        # One True decides an `any`, one False an `all`.
        decisive = name == "any"
        if decisive in values:
            return decisive, end
        return (None if None in values else not decisive), end

    def is_test_only(self, start: int, end: int) -> bool:
        """Whether the attribute in `start..end` is one no build without tests
        compiles its item under."""
        if self.text(start) == "test":
            return end == start + 1
        return self.text(start) == "cfg" and self.punct(start + 1) == "(" and self.cfg(start + 2)[0] is False

    def path_tree(self, i: int, prefix: Segments = ()) -> tuple[list[tuple[str, Segments]], int]:
        """The paths of the use tree or path expression at `i`, each with the
        name it binds, and the index past it."""
        if self.punct(i) == "{":
            end = self.group_end(i)
            found: list[tuple[str, Segments]] = []
            i += 1
            while i < end - 1:
                inner, i = self.path_tree(i, prefix)
                found += inner
                if self.punct(i) == ",":
                    i += 1
                elif i != end - 1:
                    raise LayerError(f"line {self.tokens[i].line}: cannot read a path group at '{self.text(i)}'")
            return found, end
        path = (*prefix, self.text(i).removeprefix("r#"))
        i += 1
        if self.punct(i) == "::" and (self.is_ident(i + 1) or self.punct(i + 1) in ("{", "*")):
            return self.path_tree(i + 1, path)
        if self.text(i) == "as" and self.is_ident(i + 1) and prefix:
            return [(self.text(i + 1), path)], i + 2
        return [(path[-1], path)], i


def resolve(path: Segments, module: Segments, line: int) -> Segments:
    """A `crate`, `self` or `super` path written in `module`, from the crate root."""
    base = () if path[0] == "crate" else module
    rest = path if path[0] == "super" else path[1:]
    while rest and rest[0] == "super":
        if not base:
            raise LayerError(f"line {line}: '{'::'.join(path)}' is above the crate root")
        base, rest = base[:-1], rest[1:]
    return (*base, *rest)


def read_file(source: str, module: Segments, test_features: frozenset[str]) -> Contents:
    """The production contents of a file whose code is in `module`."""
    reader = Reader(source, test_features)
    tokens, text, punct = reader.tokens, reader.text, reader.punct
    out = Contents()
    here = list(module)
    # For each open brace, whether it is an inline module's.
    braces: list[bool] = []
    mount_path: str | None = None
    exported = False

    def read_path(i: int) -> int | None:
        """Records the path at `i`, if one starts there; the index past it."""
        is_root = tokens[i].kind == "ident" and text(i) in ("crate", "self", "super")
        if not is_root or punct(i + 1) != "::":
            return None
        found, end = reader.path_tree(i)
        for _, path in found:
            out.references.append((resolve(path, tuple(here), tokens[i].line), tokens[i].line))
        return end

    i = 0
    while i < len(tokens):
        if punct(i) == "#" and punct(i + 1) == "!" and punct(i + 2) == "[":
            end = reader.group_end(i + 2)
            if reader.is_test_only(i + 3, end - 1):
                return Contents()
            i = end
            continue
        if punct(i) == "#" and punct(i + 1) == "[":
            attributes: list[tuple[int, int]] = []
            while punct(i) == "#" and punct(i + 1) == "[":
                end = reader.group_end(i + 1)
                attributes.append((i + 2, end - 1))
                i = end
            if any(reader.is_test_only(start, end) for start, end in attributes):
                i = reader.item_end(i)
                continue
            for start, end in attributes:
                if text(start) == "path" and punct(start + 1) == "=":
                    mount_path = text(start + 2).strip('"')
                exported = exported or text(start) == "macro_export"
                while start < end:
                    string = PATH_STRING_RE.fullmatch(text(start))
                    if string:
                        line = tokens[start].line
                        out.references.append((resolve(tuple(string[1].split("::")), tuple(here), line), line))
                    start = read_path(start) or start + 1
            continue

        word = text(i) if reader.is_ident(i) else ""
        if word == "mod" and reader.is_ident(i + 1) and punct(i + 2) in (";", "{"):
            name = text(i + 1).removeprefix("r#")
            if punct(i + 2) == ";":
                out.mounts.append(((*here, name), mount_path, tokens[i].line))
            else:
                here.append(name)
                braces.append(True)
            mount_path = None
            i += 3
            continue
        if word == "use" and not braces:
            # Its paths are read below as well, as references.
            out.imports += reader.path_tree(i + 1)[0]
        elif word == "macro_rules" and punct(i + 1) == "!":
            if exported:
                out.exported_macros.append(text(i + 2))
            exported = False
        elif word and punct(i + 1) == "!" and punct(i + 2) in CLOSERS and punct(i - 1) != "::":
            out.macro_uses.append((word, tokens[i].line))
        elif punct(i) == "{":
            braces.append(False)
        elif punct(i) == "}":
            if not braces:
                raise LayerError(f"line {tokens[i].line}: unmatched '}}'")
            if braces.pop():
                here.pop()
        i = read_path(i) or i + 1
    return out


def mounted_file(path: Segments, attribute: str | None, declared_in: Path, file_module: Segments) -> Path:
    """The file of a `mod name;` in `declared_in`, whose own module is `file_module`."""
    own_dir = declared_in.parent
    if declared_in.stem not in ("lib", "mod"):
        own_dir /= declared_in.stem
    # Inline modules around the declaration are directories.
    nested = path[len(file_module) : -1]
    if attribute is not None:
        return (own_dir.joinpath(*nested) if nested else declared_in.parent) / attribute
    file = own_dir.joinpath(*nested, f"{path[-1]}.rs")
    return file if file.exists() else own_dir.joinpath(*nested, path[-1], "mod.rs")


def read_crate(source: Path, test_features: frozenset[str]) -> dict[Path, tuple[Segments, Contents]]:
    """Every production file of the crate, with the module it is and its contents."""
    files: dict[Path, tuple[Segments, Contents]] = {}
    pending: list[tuple[Path, Segments]] = [(source / "lib.rs", ())]
    while pending:
        file, module = pending.pop()
        try:
            contents = read_file(file.read_text(encoding="utf-8"), module, test_features)
        except (LayerError, OSError) as err:
            raise LayerError(f"{file}: {err}") from err
        files[file] = (module, contents)
        for path, attribute, _ in contents.mounts:
            pending.append((mounted_file(path, attribute, file, module), path))
    return files


def check_table(layers: dict[str, dict[str, list[str]]], modules: set[str]) -> tuple[dict[str, str], list[str]]:
    """The layer of each module, and what is wrong with the table."""
    errors: list[str] = []
    layer_of: dict[str, str] = {}
    seen: set[str] = set()
    for layer, entry in layers.items():
        # Layers are listed bottom up, so their uses cannot form a cycle.
        for used in entry["uses"]:
            if used not in seen:
                errors.append(f"ERROR: layer '{layer}' uses '{used}', which is not a layer listed before it.")
        seen.add(layer)
        for module in entry["modules"]:
            if module in layer_of:
                errors.append(f"ERROR: module '{module}' is in two layers, '{layer_of[module]}' and '{layer}'.")
            layer_of[module] = layer
    for module in sorted(modules - set(layer_of)):
        errors.append(f"ERROR: module '{module}' is in no layer. Add it to scripts/engine-layers.json.")
    for module in sorted(set(layer_of) - modules):
        errors.append(f"ERROR: module '{module}' does not exist. Remove it from scripts/engine-layers.json.")
    return layer_of, errors


def check(repo_root: Path, table: dict) -> list[str]:
    """Every way the crate `table` names breaks it."""
    source = repo_root / table["source"]
    layers = table["layers"]
    files = read_crate(source, frozenset(table["test_features"]))
    _, root = files.pop(source / "lib.rs")
    modules = {path[0] for path, _, _ in root.mounts}
    layer_of, errors = check_table(layers, modules)

    # What each name at the crate root is a reference to: a module is itself,
    # and a re-export or an exported macro is the module it comes from (a
    # re-export of another crate's is none of them).
    home: dict[str, str | None] = {}
    for name, path in root.imports:
        home[name] = path[1] if path[0] in ("crate", "self") else None
    if "*" in home:
        errors.append("ERROR: lib.rs: a glob re-export hides which module its names come from.")
    macros = {macro: module[0] for module, contents in files.values() for macro in contents.exported_macros}
    home.update(macros)
    home.update((module, module) for module in modules)

    for file, (module, contents) in sorted(files.items()):
        own = layer_of.get(module[0])
        if own is None:
            continue
        uses = [(path, "crate::" + "::".join(path), line) for path, line in contents.references]
        uses += [((name,), f"{name}!", line) for name, line in contents.macro_uses if name in macros]
        for path, spelled, line in uses:
            where = f"{file.relative_to(repo_root)}:{line}"
            if not path or path[0] not in home:
                errors.append(f"ERROR: {where}: '{spelled}' names no module of the crate root.")
                continue
            used = layer_of.get(home[path[0]] or "", own)
            if used != own and used not in layers[own]["uses"]:
                errors.append(
                    f"ERROR: {where}: {own} must not use {used} ({spelled}).\n"
                    f"  Layers {own} may use: {', '.join(layers[own]['uses']) or '(none)'}\n"
                    f"  See scripts/engine-layers.json for the layers."
                )
    return errors


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent
    with open(repo_root / "scripts" / "engine-layers.json", encoding="utf-8") as f:
        table = json.load(f)

    try:
        errors = check(repo_root, table)
    except LayerError as err:
        errors = [f"ERROR: {err}"]

    if errors:
        for err in errors:
            print(err, file=sys.stderr)
        return 1

    print("Engine layer check passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
