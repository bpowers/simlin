"""Create a new Simlin project and build a simple population model using pysimlin's edit API."""

from __future__ import annotations

import simlin
import simlin.pb as pb


def build_population_project() -> simlin.Project:
    """Return a project containing a logistic population model created via model.edit()."""

    project = simlin.Project.new(
        name="pysimlin-population-example",
        sim_start=0.0,
        sim_stop=80.0,
        dt=0.25,
        time_units="years",
    )

    project.set_sim_specs(
        start=0.0,
        stop=80.0,
        dt={"value": 0.25},
        time_units="years",
    )

    model = project.get_model()
    with model.edit() as (_, patch):
        population = pb.Variable.Stock()
        population.ident = "population"
        population.equation.scalar.equation = "50"
        population.inflows.extend(["births"])
        population.outflows.extend(["deaths"])
        patch.upsert_stock(population)

        births = pb.Variable.Flow()
        births.ident = "births"
        births.equation.scalar.equation = "population * birth_rate"
        patch.upsert_flow(births)

        deaths = pb.Variable.Flow()
        deaths.ident = "deaths"
        deaths.equation.scalar.equation = "population * birth_rate * (population / 1000)"
        patch.upsert_flow(deaths)

        birth_rate = pb.Variable.Aux()
        birth_rate.ident = "birth_rate"
        birth_rate.equation.scalar.equation = "0.08"
        patch.upsert_aux(birth_rate)

    return project


def validate_population_curve(values: list[float]) -> None:
    """Ensure the generated population series shows logistic (S-shaped) growth."""

    if len(values) < 3:
        raise RuntimeError("Population series is unexpectedly short")

    if any(b < a for a, b in zip(values, values[1:])):
        raise RuntimeError("Population should not decline in this model")

    initial = values[0]
    mid = values[len(values) // 2]
    last = values[-1]
    growth_first_half = mid - initial
    growth_second_half = last - mid

    if not growth_first_half > 0:
        raise RuntimeError("Population failed to grow early in the simulation")

    if not growth_second_half > 0:
        raise RuntimeError("Population failed to grow late in the simulation")

    if not growth_second_half < growth_first_half:
        raise RuntimeError("Logistic growth should slow over time")

    if not 950 <= last <= 1025:
        raise RuntimeError(
            "Population should approach the carrying capacity (~1000), "
            f"but ended at {last:.2f}"
        )


def main() -> None:
    """Build, simulate, and validate the population model."""

    project = build_population_project()
    errors = project.get_errors()
    if errors:
        raise RuntimeError(f"Generated project contains validation errors: {errors}")

    model = project.get_model()
    with model.simulate() as sim:
        sim.run_to_end()
        population_series = [float(value) for value in sim.get_series("population")]

    validate_population_curve(population_series)

    print(
        "Population grows from "
        f"{population_series[0]:.1f} to {population_series[-1]:.1f}, forming an S-shaped trajectory."
    )


if __name__ == "__main__":
    main()
