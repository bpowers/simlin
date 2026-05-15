# macro_arrayed

Exercises an **arrayed (apply-to-all) macro invocation**. Phase 3 made the
engine's `instantiate_implicit_modules` apply-to-all path macro-aware (via
`contains_module_call`), so an arrayed macro call rides the *existing*
per-element module-expansion machinery -- one independent macro instance per
dimension element, with no new mechanism. This fixture is the
fixture-driven verification of macros.AC3.4.

## Model

A **stockless** macro, invoked apply-to-all over a 3-element dimension:

```
:MACRO: SCALE(x, k)
SCALE = x * k
:END OF MACRO:

Region: R1, R2, R3
inp[Region] = 10, 20, 30
factor      = 3
out[Region] = SCALE(inp[Region], factor)
```

`SCALE` has **two** parameters so the call is not rewritten to `LOOKUP`
(GH #553: a single-argument `NAME(arg)` MDL call is a lookup invocation).

At compile time `out[Region] = SCALE(inp[Region], factor)` expands into one
independent synthetic `Variable::Module` per `Region` element
(`$⁚out⁚0⁚scale⁚r1`, `$⁚out⁚0⁚scale⁚r2`, `$⁚out⁚0⁚scale⁚r3`), each wiring
its own `inp[element]` and the shared `factor` -- exactly the per-element
unrolling stdlib functions already get.

## Hand-computed expected values

Stockless => constant over time:

- `out[R1] = inp[R1] * factor = 10 * 3 = 30`
- `out[R2] = inp[R2] * factor = 20 * 3 = 60`
- `out[R3] = inp[R3] * factor = 30 * 3 = 90`

`INITIAL TIME = 0`, `FINAL TIME = 2`, `TIME STEP = 1`, `SAVEPER = 1`
=> 3 saved steps (t = 0, 1, 2), every row identical.

`output.tab` is tab-separated with CR (`\r`) line terminators. Arrayed
columns use the `out[R1]` bracket form (the harness canonicalizes the
header, so case/spacing of element names does not matter).
