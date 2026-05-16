# macro_clearn_ramp_from_to

A focused isolation fixture for **C-LEARN's `RAMP FROM TO` macro**
(macros.AC6.3). The `:MACRO:` block is copied **verbatim** from
`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl` (lines 67-100) and
invoked with known constant inputs so the expected output can be
hand-computed by applying the macro body formula.

`RAMP FROM TO` is the most involved of C-LEARN's invoked macros: a
multi-equation body with seven body variables, a `RAMP` builtin call, an
`IF THEN ELSE` branch selector, and `Time$` / `TIME STEP$` time escapes in
the (unused-here) exponential branch.

## Macro (verbatim from C-LEARN)

```
:MACRO: RAMP FROM TO( xfrom, xto, tstart, tend, islinear)
RAMP FROM TO = IF THEN ELSE( linear,linear ramp,exp ramp)
linear   = IF THEN ELSE( xfrom > 0 :AND: xto > 0, islinear, 1)
linear ramp = xfrom + RAMP(slope,tstart,tend)
exp ramp = IF THEN ELSE(Time$ <= tstart, xfrom
,IF THEN ELSE( Time$ > tend, xto, xfrom*EXP( rate*(Time$-tstart)) ))
slope    = (xto-xfrom)/interval
rate     = IF THEN ELSE( xfrom > 0 :AND: xto > 0, LN(xto/xfrom)/interval, :NA: )
interval = MAX(tend-tstart,TIME STEP$)
:END OF MACRO:
```

## Caller (known constant inputs, linear branch)

```
x from    = 2
x to      = 10
t start   = 1
t end     = 5
is linear = 1
ramped    = RAMP FROM TO(x from, x to, t start, t end, is linear)
```

Five arguments, so the call is not rewritten to `LOOKUP` (GH #553).

## Hand-computed expected values

Both endpoints are positive, so:

- `linear   = IF (2 > 0 AND 10 > 0) THEN is linear(1) ELSE 1  = 1`
  => `RAMP FROM TO = linear ramp` (the exponential branch is **not**
  taken, so `rate` / `exp ramp` / the `Time$` escapes do not affect the
  result here).
- `interval = MAX(t end - t start, TIME STEP$) = MAX(5 - 1, 1) = 4`
- `slope    = (x to - x from) / interval = (10 - 2) / 4 = 2`
- `linear ramp = x from + RAMP(slope, t start, t end) = 2 + RAMP(2, 1, 5)`

The engine's `RAMP(slope, start, end)` (`vm.rs::ramp`) is `0` for
`time <= start`, `slope*(time - start)` for `start < time < end`, and
`slope*(end - start)` for `time >= end`.

`INITIAL TIME = 0`, `FINAL TIME = 6`, `TIME STEP = 1`, `SAVEPER = 1`
=> 7 saved steps (t = 0..6):

| t | RAMP(2,1,5) at t                | ramped = 2 + RAMP |
|---|---------------------------------|-------------------|
| 0 | 0      (0 <= 1)                 | 2                 |
| 1 | 0      (1 <= 1)                 | 2                 |
| 2 | 2*(2-1) = 2                     | 4                 |
| 3 | 2*(3-1) = 4                     | 6                 |
| 4 | 2*(4-1) = 6                     | 8                 |
| 5 | 2*(5-1) = 8   (5 >= 5)          | 10                |
| 6 | 2*(5-1) = 8   (6 >= 5)          | 10                |

So `ramped = [2, 2, 4, 6, 8, 10, 10]` -- a clamped linear ramp from
`x from = 2` (held until `t start`) to `x to = 10` (held after `t end`).

`output.tab` is tab-separated with CRLF line terminators, matching the
other bundled Vensim fixtures. `ensure_results` checks only the listed
columns; the first column is treated as `time`.

## Reference output

No Vensim DSS reference `.vdf` is checked in for this focused fixture
(authoring one is a documented prerequisite/setup task per the Phase 7
design's "Test prerequisites" note, not implementation work). The
formula-derived `output.tab` above is the gate; if a Vensim DSS `.vdf` is
later added alongside, the test should prefer it via `ensure_vdf_results`.
