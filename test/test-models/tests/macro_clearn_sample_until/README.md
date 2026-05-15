# macro_clearn_sample_until

A focused isolation fixture for **C-LEARN's `SAMPLE UNTIL` macro**
(macros.AC6.3). The `:MACRO:` block is copied **verbatim** from
`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl` (lines 47-52) and
invoked with known constant inputs so the expected output can be
hand-computed by applying the macro body formula.

## Macro (verbatim from C-LEARN)

```
:MACRO: SAMPLE UNTIL(lastTime,input,initval)
SAMPLE UNTIL = INTEG( (1-STEP(1,lastTime))*(input-SAMPLE UNTIL)/TIME STEP$, initval)
:END OF MACRO:
```

`SAMPLE UNTIL` is a **stock** (`INTEG`). Its flow is
`(1 - STEP(1, lastTime)) * (input - SAMPLE UNTIL) / dt`:

- Before `lastTime`, `STEP(1, lastTime) = 0`, so the flow drives the stock
  toward `input` in a single Euler step: `SU[k+1] = SU[k] + dt * (input -
  SU[k]) / dt = input`.
- At and after `lastTime`, `STEP(1, lastTime) = 1`, the flow is 0, and the
  stock **holds** its last sampled value.

The engine's `STEP(height, t)` returns `height` when `time + dt/2 > t`,
else 0 (`vm.rs::step`), and Vensim `INTEG`'s value at step `k` (t = k) is
`init + integral` evaluated with Euler (the same convention the
`macro_stock` fixture pins).

## Caller (known constant inputs)

```
last time     = 3
the input     = 7
initial value = 2
sampled       = SAMPLE UNTIL(last time, the input, initial value)
```

Two-plus arguments, so the call is **not** rewritten to `LOOKUP`
(GH #553).

## Hand-computed expected values

`INITIAL TIME = 0`, `FINAL TIME = 5`, `TIME STEP = 1`, `SAVEPER = 1`
=> 6 saved steps (t = 0..5). `SU` = `sampled`:

| t | STEP(1,3) at t | flow at t                    | SU at t |
|---|----------------|------------------------------|---------|
| 0 | 0 (0.5 > 3? no) | (1-0)*(7-2)/1 = 5           | 2 (init) |
| 1 | 0 (1.5 > 3? no) | (1-0)*(7-7)/1 = 0           | 7       |
| 2 | 0 (2.5 > 3? no) | (1-0)*(7-7)/1 = 0           | 7       |
| 3 | 1 (3.5 > 3? yes)| (1-1)*...      = 0           | 7       |
| 4 | 1               | 0                            | 7       |
| 5 | 1               | 0                            | 7       |

So `sampled = [2, 7, 7, 7, 7, 7]`. With a constant `input` the stock jumps
to `input` after the first step and holds; the `lastTime` gate is exercised
(the flow is force-zeroed from t = 3 on) even though the held value is
unchanged because `input` is constant.

`output.tab` is tab-separated with CRLF line terminators, matching the
other bundled Vensim fixtures. `ensure_results` checks only the listed
columns; the first column is treated as `time`.

## Reference output

No Vensim DSS reference `.vdf` is checked in for this focused fixture
(authoring one is a documented prerequisite/setup task per the Phase 7
design's "Test prerequisites" note, not implementation work). The
formula-derived `output.tab` above is the gate; if a Vensim DSS `.vdf` is
later added alongside, the test should prefer it via `ensure_vdf_results`.
