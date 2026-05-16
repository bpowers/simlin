# macro_clearn_sshape

A focused isolation fixture for **C-LEARN's `SSHAPE` macro**
(macros.AC6.3). The `:MACRO:` block is copied **verbatim** from
`test/xmutil_test_models/C-LEARN v77 for Vensim.mdl` (lines 53-66,
including its line continuation) and invoked with known constant inputs so
the expected output can be hand-computed by applying the macro body
formula.

`SSHAPE` is interesting for two reasons:

1. A real `SSHAPE` builtin exists (a 3-arg S-curve); the project macro
   **shadows** it. A 2-arg call is not rewritten to `LOOKUP` (GH #553) and
   resolves to the macro.
2. The macro body has a **macro-local helper** (`input = MIN(1, MAX(0,
   xin))`) and a two-branch `IF THEN ELSE`; both branches are exercised.

## Macro (verbatim from C-LEARN)

```
:MACRO: SSHAPE(xin,profile)
SSHAPE = IF THEN ELSE( input>0.5, 1-(1-input)^profile*0.5/0.5^profile, input^profile*\
		0.5/0.5^profile)
input = MIN(1,MAX(0,xin))
:END OF MACRO:
```

So, with `input = MIN(1, MAX(0, xin))` (clamp to [0, 1]):

```
SSHAPE = IF input > 0.5
         THEN 1 - (1 - input)^profile * 0.5 / 0.5^profile
         ELSE     input^profile       * 0.5 / 0.5^profile
```

## Caller (known constant inputs)

```
profile    = 2
high input = 0.8     ->  s high = SSHAPE(0.8, 2)   (upper branch)
low input  = 0.3     ->  s low  = SSHAPE(0.3, 2)   (lower branch)
```

Two-plus arguments, so neither call is rewritten to `LOOKUP` (GH #553).

## Hand-computed expected values

The model is stockless, so every value is constant over time.
`0.5^2 = 0.25`, so `0.5 / 0.5^2 = 2`.

**`s high` = SSHAPE(0.8, 2)** (upper branch, `input = 0.8 > 0.5`):

```
1 - (1 - 0.8)^2 * 0.5 / 0.5^2
= 1 - (0.2)^2 * 2
= 1 - 0.04 * 2
= 1 - 0.08
= 0.92
```

**`s low` = SSHAPE(0.3, 2)** (lower branch, `input = 0.3`, not `> 0.5`):

```
0.3^2 * 0.5 / 0.5^2
= 0.09 * 2
= 0.18
```

`INITIAL TIME = 0`, `FINAL TIME = 2`, `TIME STEP = 1`, `SAVEPER = 1`
=> 3 saved steps (t = 0, 1, 2), each row identical:
`s high = 0.92`, `s low = 0.18`.

`output.tab` is tab-separated with CRLF line terminators, matching the
other bundled Vensim fixtures. `ensure_results` checks only the listed
columns; the first column is treated as `time`.

## Reference output

No Vensim DSS reference `.vdf` is checked in for this focused fixture
(authoring one is a documented prerequisite/setup task per the Phase 7
design's "Test prerequisites" note, not implementation work). The
formula-derived `output.tab` above is the gate; if a Vensim DSS `.vdf` is
later added alongside, the test should prefer it via `ensure_vdf_results`.
