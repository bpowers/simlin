"""Example showing how to edit an existing model's flow equation with pysimlin."""

from __future__ import annotations

import simlin


EXAMPLE_XMILE = b"""<?xml version='1.0' encoding='utf-8'?>
<xmile version=\"1.0\" xmlns=\"http://docs.oasis-open.org/xmile/ns/XMILE/v1.0\" xmlns:isee=\"http://iseesystems.com/XMILE\" xmlns:simlin=\"https://simlin.com/XMILE/v1.0\">
  <header>
    <name>pysimlin-edit-example</name>
    <vendor>Simlin</vendor>
    <product version=\"0.1.0\" lang=\"en\">Simlin</product>
  </header>
  <sim_specs method=\"Euler\" time_units=\"Year\">
    <start>0</start>
    <stop>80</stop>
    <dt>0.25</dt>
  </sim_specs>
  <model name=\"main\">
    <variables>
      <stock name=\"population\">
        <eqn>25</eqn>
        <inflow>net_birth_rate</inflow>
      </stock>
      <flow name=\"net_birth_rate\">
        <eqn>fractional_growth_rate * population</eqn>
      </flow>
      <aux name=\"fractional_growth_rate\">
        <eqn>maximum_growth_rate * (1 - population / carrying_capacity)</eqn>
      </aux>
      <aux name=\"maximum_growth_rate\">
        <eqn>0.10</eqn>
      </aux>
      <aux name=\"carrying_capacity\">
        <eqn>1000</eqn>
      </aux>
    </variables>
  </model>
</xmile>
"""


def run_simulation(model: simlin.Model) -> float:
    """Run the model to the configured stop time and return the ending population."""

    with model.simulate() as sim:
        sim.run_to_end()
        return float(sim.get_value("population"))


def main() -> None:
    """Demonstrate editing a flow equation and verify the change takes effect."""

    project = simlin.Project.from_xmile(EXAMPLE_XMILE)
    model = project.get_model()

    baseline_final = run_simulation(model)

    with model.edit() as (current, patch):
        flow_var = current["net_birth_rate"]
        flow_var.flow.equation.scalar.equation = (
            "fractional_growth_rate * population * 1.5"
        )
        patch.upsert_flow(flow_var.flow)

    accelerated_final = run_simulation(model)

    if not accelerated_final > baseline_final + 10:
        raise RuntimeError(
            "Edited model did not accelerate growth as expected: "
            f"baseline={baseline_final:.2f} accelerated={accelerated_final:.2f}"
        )

    print(
        "Updated growth equation increased the final population from "
        f"{baseline_final:.1f} to {accelerated_final:.1f}."
    )


if __name__ == "__main__":
    main()
