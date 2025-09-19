#!/usr/bin/env python3
"""Example demonstrating project editing with the Python simlin bindings."""

from __future__ import annotations

from pathlib import Path

import simlin


def main() -> None:
    project_path = Path(__file__).resolve().parent.parent / "tests/fixtures/teacup.mdl"

    project = simlin.Project.from_file(project_path)
    model = project.get_model()

    with model.edit() as (current, patch):
        heat_loss = current["Heat Loss to Room"]
        heat_loss.set_equation("(Teacup Temperature - Room Temperature) * Cooling Factor")

        cooling_factor = simlin.AuxVariable.new("Cooling Factor").set_equation("0.1 / Characteristic Time")
        patch.upsert(cooling_factor)
        patch.upsert(heat_loss)

    with model.new_sim() as sim:
        sim.run_to_end()
        final_temp = sim.get_value("Teacup Temperature")
        print(f"Final teacup temperature: {final_temp:.2f}")


if __name__ == "__main__":
    main()
