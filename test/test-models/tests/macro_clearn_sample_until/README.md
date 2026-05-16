# macro_clearn_sample_until

A focused isolation fixture for **C-LEARN's `SAMPLE UNTIL` macro**
(macros.AC6.3). The `:MACRO:` block is copied **byte-verbatim** from
`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl` (lines 47-52) and
invoked with a **time-varying** input so the expected output can be
hand-computed by applying the macro body formula -- and so the fixture
genuinely exercises SAMPLE UNTIL's *defining* behavior (sample a CHANGING
signal, then FREEZE it at `lastTime`), not just an init->constant
first-step jump.

## Macro (verbatim from C-LEARN)

```
:MACRO: SAMPLE UNTIL(lastTime,input,initval)
SAMPLE UNTIL = INTEG( (1-STEP(1,lastTime))*(input-SAMPLE UNTIL)/TIME STEP$, initval)
:END OF MACRO:
```

`SAMPLE UNTIL` is a **stock** (`INTEG`). Its flow is
`(1 - STEP(1, lastTime)) * (input - SAMPLE UNTIL) / dt`:

- Before `lastTime`, `STEP(1, lastTime) = 0`, so the flow drives the stock
  exactly onto the *current* `input` in a single Euler step:
  `SU[k+1] = SU[k] + dt * (input[k] - SU[k]) / dt = input[k]`. So while the
  gate is open the stock **tracks `input` with a one-step lag**.
- At and after `lastTime`, `STEP(1, lastTime) = 1`, the flow is 0, and the
  stock **holds (freezes)** whatever value it last sampled.

The engine's `STEP(height, t)` returns `height` when `time + dt/2 > t`,
else 0 (`vm.rs::step`); `RAMP(slope, start, end)` returns 0 for
`time <= start`, `slope*(time-start)` for `start < time < end`, and
`slope*(end-start)` for `time >= end` (`vm.rs::ramp`). Vensim `INTEG`'s
value at step `k` (t = k) is `init + integral` evaluated with forward
Euler (`SU[k+1] = SU[k] + dt*flow[k]`, flow read at time `k` with the
current stock value -- the same convention the `macro_stock` fixture
pins).

## Caller (time-varying input)

```
last time     = 4
the input     = 5 + RAMP(1, 0, 10)
initial value = 99
sampled       = SAMPLE UNTIL(last time, the input, initial value)
```

Two-plus arguments, so the call is **not** rewritten to `LOOKUP`
(GH #553). `initial value = 99` is a distinct sentinel (it differs from
every sampled/frozen value, so a wrong init can never accidentally
match).

## Hand-computed expected values

`INITIAL TIME = 0`, `FINAL TIME = 8`, `TIME STEP = 1`, `SAVEPER = 1`
=> 9 saved steps (t = 0..8). `dt = 1`.

`the input[t] = 5 + RAMP(1, 0, 10)`: `RAMP(1,0,10)` is 0 at t=0
(`time > start` is `0 > 0` => false) then `time` for `0 < time < 10`, so

| t          | 0 | 1 | 2 | 3 | 4 | 5  | 6  | 7  | 8  |
|------------|---|---|---|---|---|----|----|----|----|
| the input  | 5 | 6 | 7 | 8 | 9 | 10 | 11 | 12 | 13 |

`STEP(1, 4)` at time t (dt=1) is 1 iff `t + 0.5 > 4`, i.e. `t >= 4`;
0 for t = 0,1,2,3. So the gate `(1 - STEP(1,4))` is **1 for t=0..3**
(flow active) and **0 for t=4..8** (flow zeroed). The last *active*
flow is the one evaluated at **t=3**.

`SU = sampled`, forward Euler `SU[t+1] = SU[t] + (1-STEP(1,4))*(input[t]-SU[t])`:

| t | gate `1-STEP(1,4)` | input[t] | flow[t]            | SU at t |
|---|--------------------|----------|--------------------|---------|
| 0 | 1 (0.5 > 4? no)    | 5        | 1*(5-99)  = -94    | 99 (init) |
| 1 | 1 (1.5 > 4? no)    | 6        | 1*(6-5)   = 1      | 5       |
| 2 | 1 (2.5 > 4? no)    | 7        | 1*(7-6)   = 1      | 6       |
| 3 | 1 (3.5 > 4? no)    | 8        | 1*(8-7)   = 1      | 7       |
| 4 | 0 (4.5 > 4? yes)   | 9        | 0*...     = 0      | 8  <- FROZEN |
| 5 | 0                  | 10       | 0                  | 8       |
| 6 | 0                  | 11       | 0                  | 8       |
| 7 | 0                  | 12       | 0                  | 8       |
| 8 | 0                  | 13       | 0                  | 8       |

So `sampled = [99, 5, 6, 7, 8, 8, 8, 8, 8]`.

## Why the freeze is now discriminating

Because `input` keeps **rising** after `lastTime`, the frozen value
(**8**, captured from `input[3]` by the last active flow) is genuinely
distinct from the init (**99**) *and* from every post-freeze input
(**9, 10, 11, 12, 13**). A model that dropped the `(1 - STEP(1,lastTime))`
gate (i.e. tracked `input` forever) would instead produce
`[99, 5, 6, 7, 8, 9, 10, 11, 12]` -- **differing at t = 5,6,7,8**. So
this fixture actually tests SAMPLE UNTIL's defining behavior (sample a
*changing* signal, then *hold* it at `lastTime`), unlike the previous
constant-input version where the stock reached `input` after the first
step and held regardless of the gate (the gate force-zeroed an
already-zero flow, so the series would have been identical with or
without it).

`output.tab` is tab-separated with CRLF line terminators, matching the
other bundled Vensim fixtures. `ensure_results` checks only the listed
columns; the first column is treated as `time`.

## Reference output

No Vensim DSS reference `.vdf` is checked in for this focused fixture
(authoring one is a documented prerequisite/setup task per the Phase 7
design's "Test prerequisites" note, not implementation work). The
formula-derived `output.tab` above is the gate; if a Vensim DSS `.vdf` is
later added alongside, the test should prefer it via `ensure_vdf_results`.
