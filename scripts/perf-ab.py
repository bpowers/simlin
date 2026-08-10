#!/usr/bin/env python3
# Copyright 2026 The Simlin Authors. All rights reserved.
# Use of this source code is governed by the Apache License,
# Version 2.0, that can be found in the LICENSE file.

"""Interleaved A/B timing for the `clearn_profile` engine harness.

Why interleaved, and why medians: `docs/design/engine-performance.md` records
two rounds where a perf "win" turned out to be an artifact.  Machine conditions
drift over minutes, so running all of A then all of B compares two different
machines; interleaving A,B,A,B... controls for that.  Binary layout is a second,
independent lottery -- two builds of the *same* source can differ by several
percent -- which interleaving does NOT control, so treat a delta under ~4% as
unresolved unless you rebuild both sides and reproduce it.

Both sides are warmed before the measured rounds because whichever binary runs
cold reliably looks slower.

Usage:

    # build each side into its own target dir first, e.g.
    #   git worktree add ../simlin-base main
    #   CARGO_TARGET_DIR=/path/to/base-target cargo build --release \
    #       -p simlin-engine --example clearn_profile --features file_io
    scripts/perf-ab.py --a base-target/release/examples/clearn_profile \
                       --b target/release/examples/clearn_profile \
                       --rounds 7 --model test/metasd/WRLD3-03/wrld3-03.mdl --ltm

`--perf` additionally reports retired-instruction counts via `perf stat`, which
are insensitive to machine load and to binary layout; when a wall-clock delta is
near the noise floor, the instruction delta is the number to trust.
"""

from __future__ import annotations

import argparse
import os
import re
import statistics
import subprocess
import sys

# `phase()` in examples/clearn_profile.rs prints:
#   "<name padded to 22>  <ms> ms | allocs ..."
PHASE_RE = re.compile(r"^(\S.*?)\s{2,}([0-9.]+) ms \|")
# Trailing "compile x20: 12.34 ms/iter" / "run x200: 5.67 ms/iter" lines.
ITER_RE = re.compile(r"^(compile|run) x(\d+): ([0-9.]+) ms/iter")
PERF_INSNS_RE = re.compile(r"^\s*([0-9,]+)\s+instructions")


def run_once(binary: str, env: dict[str, str], use_perf: bool) -> dict[str, float]:
    """One harness invocation; returns {phase name: milliseconds}."""
    cmd = [binary]
    if use_perf:
        cmd = ["perf", "stat", "-e", "instructions", "--"] + cmd
    proc = subprocess.run(
        cmd, env=env, capture_output=True, text=True, check=False
    )
    if proc.returncode != 0:
        sys.exit(
            f"{binary} exited {proc.returncode}\n"
            f"--- stdout ---\n{proc.stdout}\n--- stderr ---\n{proc.stderr}"
        )

    out: dict[str, float] = {}
    for line in proc.stdout.splitlines():
        m = PHASE_RE.match(line)
        if m:
            out[m.group(1).strip()] = float(m.group(2))
            continue
        m = ITER_RE.match(line)
        if m:
            out[f"{m.group(1)} x{m.group(2)}"] = float(m.group(3))
    # perf writes its summary to stderr.
    for line in proc.stderr.splitlines():
        m = PERF_INSNS_RE.match(line)
        if m:
            out["instructions (M)"] = int(m.group(1).replace(",", "")) / 1e6
    if not out:
        sys.exit(f"no timings parsed from {binary}; stdout was:\n{proc.stdout}")
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--a", required=True, help="baseline clearn_profile binary")
    ap.add_argument("--b", required=True, help="candidate clearn_profile binary")
    ap.add_argument("--rounds", type=int, default=7, help="measured rounds per side")
    ap.add_argument("--warmup", type=int, default=2, help="discarded rounds per side")
    ap.add_argument("--model", help="value for CLEARN_MODEL")
    ap.add_argument("--ltm", action="store_true", help="set CLEARN_LTM=1")
    ap.add_argument(
        "--profile",
        choices=["compile", "run", "both"],
        help="CLEARN_PROFILE for the extra-iteration loops",
    )
    ap.add_argument("--compile-iters", type=int, help="CLEARN_COMPILE_ITERS")
    ap.add_argument("--run-iters", type=int, help="CLEARN_RUN_ITERS")
    ap.add_argument(
        "--perf", action="store_true", help="also report perf-stat instruction counts"
    )
    args = ap.parse_args()

    env = dict(os.environ)
    if args.model:
        env["CLEARN_MODEL"] = args.model
    if args.ltm:
        env["CLEARN_LTM"] = "1"
    if args.profile:
        env["CLEARN_PROFILE"] = args.profile
    if args.compile_iters is not None:
        env["CLEARN_COMPILE_ITERS"] = str(args.compile_iters)
    if args.run_iters is not None:
        env["CLEARN_RUN_ITERS"] = str(args.run_iters)
    # Allocation counting adds a pair of atomics to every allocation, which
    # distorts exactly the phase this script is timing.
    env.pop("CLEARN_COUNT_ALLOCS", None)

    for _ in range(args.warmup):
        run_once(args.a, env, args.perf)
        run_once(args.b, env, args.perf)

    samples: dict[str, dict[str, list[float]]] = {"a": {}, "b": {}}
    for i in range(args.rounds):
        # Alternate which side leads so a systematic first-vs-second-in-round
        # effect (thermal, frequency ramp) cancels rather than always favouring
        # one side.
        order = [("a", args.a), ("b", args.b)]
        if i % 2:
            order.reverse()
        for side, binary in order:
            for phase, ms in run_once(binary, env, args.perf).items():
                samples[side].setdefault(phase, []).append(ms)

    phases = [p for p in samples["a"] if p in samples["b"]]
    width = max((len(p) for p in phases), default=10)
    print(f"\nrounds={args.rounds} (warmup {args.warmup}), medians")
    print(f"{'phase':<{width}}  {'A':>12}  {'B':>12}  {'delta':>9}")
    for phase in phases:
        a = statistics.median(samples["a"][phase])
        b = statistics.median(samples["b"][phase])
        delta = (b - a) / a * 100.0 if a else float("nan")
        print(f"{phase:<{width}}  {a:>12.2f}  {b:>12.2f}  {delta:>+8.1f}%")
    print(
        "\nA delta under ~4% on wall-clock is not resolved by one build pair "
        "(binary-layout lottery); rebuild both sides and reproduce, or compare "
        "instruction counts with --perf."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
