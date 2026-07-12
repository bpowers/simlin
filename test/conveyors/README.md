# Conveyor test fixtures

Models that exercise XMILE conveyor stocks, per
[docs/design/conveyors.md](/docs/design/conveyors.md).

**No fixture here ships an expected-output CSV.** The bytecode VM is the oracle: no
other SD tool writes the queue/conveyor XMILE dialect we parse. What the integration
harness adds is that every other path — protobuf round-trip, XMILE round-trip, and
the wasm backend — reproduces the VM column-for-column, synthetic driven flows and
container auxes included. That harness is `simulate_special_path` in
`src/simlin-engine/tests/integration/simulate.rs`, NOT the ordinary `simulate_path`
(which would trip the `ConveyorNotExpanded` guard).

**Know what that does and does not buy you.** The harness checks round-trip fidelity
and VM-vs-wasm parity, not numeric truth. `wasmgen/belt.rs` is written to reproduce
`conveyor.rs` bit for bit, quirks included (GH #942), so a belt-pass bug that both
backends share passes every test here. The only automatic pin on the VM's numerics is
`src/simlin-engine/src/conveyor_tests.rs`, whose expected values are transcribed by
hand from `reference_prototype.py` (below) — nothing runs that script. GH #951 tracks
extending it to the leak-zone, discrete, and coupled cases and running it from a Rust
test, which is the only check that would catch a lockstep drift. GH #950 is a live
instance: `queue_coupled_conveyor.xmile`'s belt contents are fractional where
conveyors.md §6.4 says they must be integral, both backends agree, and no test notices.

**Every model file in this directory is accounted for by name**, in one of two lists
in that same file. `CONVEYOR_CORPUS_FIXTURES` gets one test each;
`BLOCKED_CONVEYOR_FIXTURES` pins the `ErrorCode` that keeps a fixture out.
`conveyor_fixture_directory_is_fully_accounted_for` fails if a new model file
belongs to neither — adding a fixture to a directory is not the same as running it,
and both `test/queues/` fixtures once sat in a corpus that never executed them.

## Files

| File | Source | License | Conveyor features | Status |
|------|--------|---------|-------------------|--------|
| `minimal_conveyor.xmile` | hand-authored (this repo) | project | transit time + capacity, single outflow, no leak | Corpus. The clean core-conveyor oracle: initialized at its steady state (`V = 250·4 = 1000`), so contents and outflow stay constant. |
| `arrayed_conveyor.xmile` | hand-authored (this repo) | project | one independent belt per element, per-element `<len>` (design doc §10) | Corpus. |
| `leaky_conveyor.xmile` | hand-authored (this repo) | project | two leak flows with different path zones (§5.1, §5.3) | Corpus. Each leak drains to its own sink, so conservation is a checked column rather than only a VM-vs-wasm diff. |
| `conveyor_containers.xmile` | hand-authored (this repo) | project | `SUM`/`MEAN`/`MIN`/`MAX`/`STDDEV`/`SIZE` over a belt, `conv[j]` slat access, `INIT(SUM(belt))` (§10) | Corpus. Seeded at the steady-state fill and then ramped, so no two slats carry the same volume and a reducer folding the wrong slat range cannot coincide with the right answer. |
| `discrete_conveyor.xmile` | hand-authored (this repo) | project | whole-unit admission + quantization carry (§6.4), per-time-unit `<in_limit>` (§6.3) | Corpus. |
| `queue_coupled_conveyor.xmile` | hand-authored (this repo) | project | queue → discrete conveyor coupling (§11 / queues.md §9), with the queue's `<overflow/>` | Corpus. The only fixture in the repo covering the interleaved phase-A / serve / phase-B order. |
| `sir_social_distancing_mixnot.stmx` | peterhovmand/COVID-19-SD-generic-structures | CC BY 4.0 | transit time (`<len>transit_time</len>`) + **distribution-based spread inflow** (`isee:spreadflow="dist"`, §8) | **Blocked**, see below. `dt = 1/4`, Days, 0–100. |
| `covid19_severity.stmx` | peterhovmand/COVID-19-SD-generic-structures | CC BY 4.0 | transit time + leakage (`<leak/>` flows) + arrayed conveyors (`Severity` dim) + `exponential_leak="true"` | **Blocked**, see below. |
| `reference_prototype.py` | this repo | project | — | Not a model. Executable reference implementation of the spec's per-DT algorithm (§4–§7). Run `python3 test/conveyors/reference_prototype.py`; it prints the §15 worked-example trajectories and asserts every invariant (steady state, transit delay, linear/exponential leak conservation, capacity/inflow-limit clipping, non-integer-transit rounding). NOT production code — a faithful transcription of the spec, and the acceptance oracle for the core continuous conveyor. |

### Why the two vendored models are blocked

Neither is blocked by anything to do with conveyor support, and neither is evidence
of a wasm-backend gap: both are refused by the SHARED `queue_compile::compile_sim`
dispatch, so the VM never simulates them either.

- `sir_social_distancing_mixnot.stmx`: both conveyors are `Infected` stocks inside
  the `Not_Mixing` / `Not_Mixing_at_all` sub-models, and expansion never descends
  into a sub-model — `ConveyorInSubmodelUnsupported` (GH #941, with the underlying
  limitation tracked as GH #940). Even once that is fixed it needs the §8 `dist`
  placement (GH #946) and the isee builtin `LOOKUPMEAN` for its transit time.
- `covid19_severity.stmx`: its `death rate` aux is `SUM(contagious_deaths[*]) + ...`,
  a same-step read of four conveyor-driven leak flows, which the engine refuses with
  `ConveyorDrivenFlowRead` (GH #944) because the belt pass runs between the Flows and
  Stocks phases. Real Stella models use exactly this idiom to report leak rates, so
  #944 blocks a legitimate model rather than an edge case. It also has several
  unit-consistency errors (`bad_binary_op_in_units`), independent of conveyors.

Both are pinned on their exact `ErrorCode` (in `simulate.rs` and again, at the wasm
lowering layer, in `src/simlin-engine/src/wasmgen/belt_tests.rs`), so fixing #944 or
#941 turns those pins red and forces the fixture into the corpus — rather than
leaving it parked forever as a silently-tolerated "expected failure".

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
`<discrete>`, `<in_limit>`, `<arrest>`, or an upstream queue; that is why the
capacity, discrete, `<in_limit>`, and queue-coupling fixtures above are
hand-authored.

Known corpus gaps. GH #949 tracks the missing single-belt feature fixtures:
`<arrest>` and `<sample>` have no fixture, nor do `exponential_leak`,
`<leak_integers/>`, the §7.2 explicit init list, or a time-varying `<len>` — only
unit coverage in `wasmgen/belt_tests.rs` and `conveyor_tests.rs`, which build
their XMILE in-test and so never cross the protobuf or XMILE round-trip. Three
more that are easy to assume away (the first two tracked as GH #954, the third as
GH #950):

- **No fixture combines a queue-coupled inflow with an equation-driven inflow on
  the same belt.** `queue_coupled_conveyor.xmile`'s belt has only the coupled
  inflow, so the `rem_cap` drawdown between `conv_vol` and the equation-driven
  apportion loop (`belt::emit_phase_b_active` step 4) is never exercised. A fault
  injected into the apportion loop alone leaves that fixture green. (GH #954.)
- **No fixture exercises FIFO backlog or the four `(one_at_a_time,
  batch_integrity)` batch rules.** `queue_coupled_conveyor.xmile`'s queue carries
  an `<overflow/>`, so it drains fully every DT (queues.md §4.5) and never holds a
  second cohort. `test/queues/queue_wait.xmile`, the backlog oracle queues.md §12
  asks for, does not exist. (GH #954.)
- **`discrete="true"` in the coupled fixture is a gate, not an observable.** XMILE
  §3.7.2 requires a queue-fed belt to be discrete; it does not make that belt's
  contents integral here. See GH #950.

### Search method note

GitHub code search does not index `.stmx`/`.xmile` file *content*, so
`extension:stmx`, `path:*.stmx`, and content queries like `"uses_conveyor"` all
return nothing. The only query that surfaced real Stella models was
`"<conveyor>" language:XML` (which also returns a lot of AnyLogic/game/i18n
noise to filter out).
