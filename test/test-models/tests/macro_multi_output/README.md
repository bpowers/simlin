# macro_multi_output

Exercises Vensim's `:` multi-output macro call form
(`total = ADD3(in1, in2, in3 : the min, the max)`), which Phase 4
materializes at MDL import as an explicit `Variable::Module` plus one
binding `Variable::Aux` per output.

## Model

A **stockless** macro with one primary output and two `:`-list additional
outputs:

```
:MACRO: ADD3(a, b, c : minval, maxval)
ADD3   = a + b + c
minval = MIN(a, MIN(b, c))
maxval = MAX(a, MAX(b, c))
:END OF MACRO:
```

Caller:

```
in1   = 7
in2   = 2
in3   = 5
total = ADD3(in1, in2, in3 : the min, the max)
spread = the max - the min
```

`total` receives the macro's **primary** output (`ADD3`). `the min` and
`the max` are bound (via the `:`-list) to the **additional** outputs
`minval` / `maxval`. `spread` is a downstream equation that *references*
the two bound additional-output variables -- this is what proves
macros.AC3.2 (the `:`-list names are real, referenceable model variables
carrying the correct values).

## Hand-computed expected values

The model is stockless, so every value is constant over time:

- `total  = in1 + in2 + in3      = 7 + 2 + 5            = 14`
- `the min = MIN(7, MIN(2, 5))   = MIN(7, 2)            =  2`
- `the max = MAX(7, MAX(2, 5))   = MAX(7, 5)            =  7`
- `spread  = the max - the min   = 7 - 2                =  5`

`INITIAL TIME = 0`, `FINAL TIME = 2`, `TIME STEP = 1`, `SAVEPER = 1`
=> 3 saved steps (t = 0, 1, 2), each row identical.

`output.tab` is tab-separated with CR (`\r`) line terminators, matching the
other bundled Vensim fixtures. Only the columns asserted by the test are
listed (`ensure_results` checks expected columns only); the harness treats
the first column as `time`.
