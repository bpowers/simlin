#!/usr/bin/env python3
"""Generate .lint-baseline.json from current codebase state.

Run this script to update the baseline after reducing unwrap_or_default() counts.
The baseline file is committed to the repo.
"""

from __future__ import annotations

import json
import subprocess
import sys
from datetime import date, timezone
from pathlib import Path


def count_unwrap_or_default(repo_root: Path) -> dict[str, int]:
    """Count unwrap_or_default() occurrences per file in simlin-engine."""
    result = subprocess.run(
        ["rg", "unwrap_or_default\\(\\)", "--type", "rust", "-c",
         "src/simlin-engine/"],
        capture_output=True, text=True, cwd=repo_root,
    )
    counts: dict[str, int] = {}
    for line in result.stdout.strip().splitlines():
        if ":" in line:
            file_path, count_str = line.rsplit(":", 1)
            counts[file_path] = int(count_str)
    return counts


def main() -> int:
    repo_root = Path(__file__).resolve().parent.parent

    counts = count_unwrap_or_default(repo_root)
    total = sum(counts.values())

    baseline = {
        "unwrap_or_default": {
            "description": "unwrap_or_default() usage in simlin-engine (ratchet: counts must not increase)",
            "as_of": date.today().isoformat(),
            "total": total,
            "counts": dict(sorted(counts.items())),
        },
    }

    baseline_path = repo_root / ".lint-baseline.json"
    with open(baseline_path, "w") as f:
        json.dump(baseline, f, indent=2)
        f.write("\n")

    print(f"Generated {baseline_path}")
    print(f"  unwrap_or_default: {total} total across {len(counts)} files")
    return 0


if __name__ == "__main__":
    sys.exit(main())
