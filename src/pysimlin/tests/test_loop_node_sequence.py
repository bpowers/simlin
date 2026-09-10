"""A detected loop's ``variables`` is its element circuit, each node once.

The fixture is the arrays investigation's ``bare_reducer`` model: a
per-element growth loop closed through ``total = SUM(pop)``.  Its three mixed
loops were reported as ``growth -> pop[boston] -> total -> growth[boston]``
(four nodes, ``growth`` twice, once bare); the engine's
``mixed_loops_through_a_variable_backed_reducer_report_the_element_circuit``
pins the same sequences on the Rust side.
"""

from __future__ import annotations

import json
from typing import TYPE_CHECKING, Any

import simlin

if TYPE_CHECKING:
    from pathlib import Path


def _bare_reducer_json() -> dict[str, Any]:
    return {
        "name": "bare_reducer",
        "simSpecs": {"startTime": 0.0, "endTime": 20.0, "dt": "1", "method": "euler"},
        "models": [
            {
                "name": "main",
                "stocks": [
                    {
                        "name": "pop",
                        "inflows": ["growth"],
                        "outflows": [],
                        "arrayedEquation": {
                            "dimensions": ["Region"],
                            "elements": [
                                {"subscript": "nyc", "equation": "100"},
                                {"subscript": "boston", "equation": "200"},
                                {"subscript": "la", "equation": "300"},
                            ],
                        },
                    }
                ],
                "flows": [
                    {
                        "name": "growth",
                        "arrayedEquation": {
                            "dimensions": ["Region"],
                            "equation": "pop * 0.02 * (1 - total / 1000)",
                        },
                    }
                ],
                "auxiliaries": [{"name": "total", "equation": "SUM(pop)"}],
                "views": [],
            }
        ],
        "dimensions": [{"name": "Region", "elements": ["nyc", "boston", "la"]}],
        "units": [],
    }


def _rotation(nodes: tuple[str, ...]) -> tuple[str, ...]:
    start = min(range(len(nodes)), key=lambda i: nodes[i])
    return nodes[start:] + nodes[:start]


def test_mixed_loops_through_a_reducer_report_the_element_circuit(tmp_path: Path) -> None:
    path = tmp_path / "bare_reducer.sd.json"
    path.write_text(json.dumps(_bare_reducer_json()))
    model = simlin.load(path)
    cycles = set()
    for loop in model.loops:
        assert len(set(loop.variables)) == len(loop.variables), (loop.id, loop.variables)
        cycles.add(_rotation(tuple(loop.variables)))
    assert cycles == {
        _rotation(("pop", "growth")),
        _rotation(("growth[nyc]", "pop[nyc]", "total")),
        _rotation(("growth[boston]", "pop[boston]", "total")),
        _rotation(("growth[la]", "pop[la]", "total")),
    }
    # The runtime surface reports the same circuits.
    run = model.run(analyze_loops=True)
    assert {_rotation(tuple(lp.variables)) for lp in run.loops} == cycles
