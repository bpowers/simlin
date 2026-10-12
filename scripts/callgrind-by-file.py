#!/usr/bin/env python3
# Copyright 2026 The Simlin Authors. All rights reserved.
# Use of this source code is governed by the Apache License,
# Version 2.0, that can be found in the LICENSE file.

"""Sum a callgrind profile's instructions by the source file they came from.

Why by file: a release build inlines small hot code into its callers, so a
per-function listing (`callgrind_annotate`) files a hasher's rounds under
whichever `HashMap::get` monomorph they were inlined into, and no row says how
much hashing there was.  Callgrind does record, for every cost line, the file of
the code that was inlined there, so summing by FILE sees through inlining:
SipHash is `core/src/hash/sip.rs` wherever it ended up, FxHash is the
`rustc-hash` crate.  That is how the identifier-hashing shares in
`docs/design/engine-performance.md` were measured.

Usage:

    # build with symbols, then profile one compile (see docs/dev/benchmarks.md)
    cargo build --profile bench -p simlin-engine --example clearn_profile
    valgrind --tool=callgrind --callgrind-out-file=callgrind.out \\
        --toggle-collect='clearn_profile::compile_retained' \\
        target/release/examples/clearn_profile

    scripts/callgrind-by-file.py callgrind.out              # top files and functions
    scripts/callgrind-by-file.py callgrind.out hash/sip.rs  # who pays for one file
    scripts/callgrind-by-file.py callgrind.out --idents     # identifier costs

Counts are self cost (the `Ir` event): a call's inclusive cost line is skipped,
so every instruction is counted once, under the function whose symbol it
executed in and the file its source line belongs to.
"""

from __future__ import annotations

import argparse
import re
import sys
from collections import defaultdict

NAME_RE = re.compile(r"^\((\d+)\)(?: (.*))?$")

# What `--idents` reports: (label, function substrings, file substrings).  A
# cost line counts toward a row when its function OR its file matches.
IDENT_ROWS = [
    (
        "SipHash (std RandomState / DefaultHasher)",
        [
            "RandomState as core::hash::BuildHasher>::hash_one",
            "core::hash::sip::",
            "DefaultHasher as core::hash::Hasher",
        ],
        ["hash/sip.rs"],
    ),
    ("FxHash (the rustc-hash crate)", [], ["rustc-hash"]),
    (
        "identifier interner (intern, and a payload's drop)",
        [
            "CanonicalStorage>::intern",
            "simlin_engine::common::Interned",
            "simlin_engine::common::Interner",
        ],
        [],
    ),
    ("Arc refcounts, every Arc (alloc/src/sync.rs)", [], ["alloc/src/sync.rs"]),
    ("canonicalize", ["simlin_engine::common::canonicalize", "is_canonical"], []),
]


class Profile:
    """Self cost of a callgrind output file, by function and by source file."""

    def __init__(self, path: str) -> None:
        self.total = 0
        self.by_file: dict[str, int] = defaultdict(int)
        self.by_fn: dict[str, int] = defaultdict(int)
        self.by_fn_file: dict[tuple[str, str], int] = defaultdict(int)
        # Callgrind compresses names: `(7) name` defines id 7, a bare `(7)`
        # refers back to it.  Files and functions number separately.
        names: dict[str, dict[str, str]] = {"file": {}, "fn": {}}

        def resolve(kind: str, value: str) -> str:
            m = NAME_RE.match(value)
            if not m:
                return value
            ident, name = m.group(1), m.group(2)
            if name is not None:
                names[kind][ident] = name
                return name
            return names[kind][ident]

        cur_file = cur_fn = "?"
        skip_next = False
        with open(path, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                line = line.rstrip("\n")
                if not line:
                    continue
                first = line[0]
                if first.isdigit() or first in "+-*":
                    # A cost line: `<position> <Ir> ...`.  The one after a
                    # `calls=` line is that call's inclusive cost.
                    if skip_next:
                        skip_next = False
                        continue
                    parts = line.split()
                    if len(parts) < 2:
                        continue
                    cost = int(parts[1])
                    self.total += cost
                    self.by_file[cur_file] += cost
                    self.by_fn[cur_fn] += cost
                    self.by_fn_file[(cur_fn, cur_file)] += cost
                elif line.startswith(("fl=", "fi=", "fe=")):
                    cur_file = resolve("file", line[3:])
                elif line.startswith("fn="):
                    cur_fn = resolve("fn", line[3:])
                elif line.startswith("cfn="):
                    resolve("fn", line[4:])
                elif line.startswith(("cfi=", "cfl=")):
                    resolve("file", line[4:])
                elif line.startswith("calls="):
                    skip_next = True

    def share(self, cost: int) -> str:
        return f"{cost:>15,} {100 * cost / self.total:6.2f}%"

    def matching(self, fn_pats: list[str], file_pats: list[str]) -> int:
        return sum(
            cost
            for (fn, fl), cost in self.by_fn_file.items()
            if any(p in fl for p in file_pats) or any(p in fn for p in fn_pats)
        )


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("profile", help="a callgrind output file")
    ap.add_argument(
        "file", nargs="*", help="source-file substring: list the functions paying for it"
    )
    ap.add_argument(
        "--idents", action="store_true", help="report identifier hashing and cloning costs"
    )
    ap.add_argument("--top", type=int, default=30, help="rows per listing")
    args = ap.parse_args()

    prof = Profile(args.profile)
    if prof.total == 0:
        sys.exit(f"{args.profile}: no cost lines; is this a callgrind output file?")
    print(f"total Ir: {prof.total:,}")

    if not args.file and not args.idents:
        print("\nby source file:")
        for name, cost in sorted(prof.by_file.items(), key=lambda kv: -kv[1])[: args.top]:
            print(f"{prof.share(cost)}  {name}")
        print("\nby function (self cost, inlined callees included):")
        for name, cost in sorted(prof.by_fn.items(), key=lambda kv: -kv[1])[: args.top]:
            print(f"{prof.share(cost)}  {name[:160]}")

    for pat in args.file:
        print(f"\nfiles matching {pat!r}: {prof.share(prof.matching([], [pat])).strip()}")
        fns: dict[str, int] = defaultdict(int)
        for (fn, fl), cost in prof.by_fn_file.items():
            if pat in fl:
                fns[fn] += cost
        for name, cost in sorted(fns.items(), key=lambda kv: -kv[1])[: args.top]:
            print(f"{prof.share(cost)}  {name[:170]}")

    if args.idents:
        print("\nidentifier costs:")
        for label, fn_pats, file_pats in IDENT_ROWS:
            print(f"{prof.share(prof.matching(fn_pats, file_pats))}  {label}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
