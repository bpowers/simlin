# Conveyor test fixtures

Reference models that exercise XMILE conveyor stocks. Vendored to support the
conveyor implementation described in [docs/design/conveyors.md](/docs/design/conveyors.md).

**Status: reference fixtures, not yet wired into the test harness.** Simlin does
not simulate conveyors today (see the design doc), and none of these models ship
expected-output CSVs. They become executable oracles only once (a) conveyor
support lands and (b) reference output is generated from Stella (or another
conforming engine). Until then they are here so the implementer has real,
authoritative XMILE to parse and represent. Do not add them to
`tests/integration/main.rs` until both conditions hold.

## Files

| File | Source | License | Conveyor features | Notes |
|------|--------|---------|-------------------|-------|
| `minimal_conveyor.xmile` | hand-authored (this repo) | project | transit time + capacity, single outflow, no leak | The Phase 1 target. Smallest possible conveyor; obvious expected behavior. |
| `sir_social_distancing_mixnot.stmx` | peterhovmand/COVID-19-SD-generic-structures | CC BY 4.0 | transit time only (`<len>transit_time</len>`) | Simplest real Stella oracle. `dt = 1/4`, Days, 0–100. Deterministic. |
| `covid19_severity.stmx` | peterhovmand/COVID-19-SD-generic-structures | CC BY 4.0 | transit time + leakage (`<leak/>` flows) + arrayed conveyors (`Severity` dim) + `exponential_leak="true"` | Exercises leakage and subscripted conveyors. Deterministic. |

### Non-conveyor blockers (must be resolved or trimmed before these become oracles)

Verified against HEAD on 2026-07-05 with `simlin-cli simulate`:

- `sir_social_distancing_mixnot.stmx`: uses the isee builtin `LOOKUPMEAN`
  (unimplemented -> `unknown_builtin` on `transit time`).
- `covid19_severity.stmx`: several unit-consistency errors
  (`bad_binary_op_in_units`) plus `PREVIOUS`/`isee:spreadflow` usage; these are
  independent of conveyor support.

The plan: once conveyors parse and simulate, either implement the missing
builtins or hand-trim minimized copies of these models so the conveyor behavior
can be isolated and compared against a Stella reference run.

## Provenance and attribution

### peterhovmand/COVID-19-SD-generic-structures

- Repository: https://github.com/peterhovmand/COVID-19-SD-generic-structures
- Commit: `4da2febd19953efb9816425678f1c5e246ceac3a` (2020-04-18)
- License: Creative Commons Attribution 4.0 (CC BY 4.0),
  https://creativecommons.org/licenses/by/4.0
- Authors: Karim Chichakly, Bob Eberlein, Mark Heffernan, Peter Hovmand.
  (Chichakly and Eberlein are isee/Ventana principals, so the XMILE encoding is
  authoritative.)
- `sir_social_distancing_mixnot.stmx` = repo path
  `Disease duration distribution/SIRSocialDistancingMixNot.stmx`
- `covid19_severity.stmx` = repo path
  `Assymptomatic expression/Covid-19-Severity.stmx`

CC BY 4.0 requires attribution; this section satisfies it. Do not remove.

### Other candidates (not vendored)

Available in the same peterhovmand repo if more coverage is needed:
`Extinction/SIRSocialDistancingExtinction.stmx` (1 conveyor),
`Disease duration distribution/IC for K.stmx` (17 conveyors + 10 leak flows),
`Special populations/COVID-19-ICU08.stmx` (4 conveyors, complex).

`henriksen-marcus/Moose-Gamification` (`STELLA/MODEL/Forest.stmx`, Apache-2.0,
commit `03c74bc9`) has 20 conveyors with varied transit times and 7 leak flows,
but every conveyor stock is initialized with `RANDOM(100, 200)`, so it is
non-deterministic and unusable as an exact oracle without stripping the random
initializers. Kept out of the vendored set for that reason.

No open-source model was found that exercises conveyor `<capacity>`,
`<discrete>`, `<in_limit>`, `<arrest>`, or an upstream queue. Those features
need hand-authored fixtures (and Stella reference output) when implemented.

### Search method note

GitHub code search does not index `.stmx`/`.xmile` file *content*, so
`extension:stmx`, `path:*.stmx`, and content queries like `"uses_conveyor"` all
return nothing. The only query that surfaced real Stella models was
`"<conveyor>" language:XML` (which also returns a lot of AnyLogic/game/i18n
noise to filter out).
