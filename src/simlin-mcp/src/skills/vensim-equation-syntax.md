# Vensim Equation Syntax

Simlin uses XMILE equation syntax internally. When importing Vensim MDL files, function names and argument orders are automatically converted. This reference documents the mapping for users writing equations or interpreting converted models.

## Function Name Mapping

The complete mapping from Vensim MDL function names to XMILE (Simlin) equivalents. Derived from the engine's `format_function_name()` and `format_call_ctx()` conversion logic.

### Core Functions

| Vensim | XMILE (Simlin) | Notes |
|--------|----------------|-------|
| `IF THEN ELSE(c, t, f)` | `IF c THEN t ELSE f` | Function form becomes ternary expression |
| `INTEG(rate, init)` | Stock with `equation=rate`, `initial_equation=init` | Not a callable function in XMILE |
| `ZIDZ(a, b)` | `SAFEDIV(a, b)` | Zero-safe division (returns 0 when b=0) |
| `XIDZ(a, b, x)` | `SAFEDIV(a, b, x)` | Zero-safe division with custom fallback x |
| `INITIAL(x)` | `INIT(x)` | Capture value at simulation start |
| `ACTIVE INITIAL(active, init)` | `INIT(active)` | |

### Delay and Smoothing

| Vensim | XMILE (Simlin) | Notes |
|--------|----------------|-------|
| `SMOOTH(input, delay)` | `SMTH1(input, delay)` | First-order exponential smooth |
| `SMOOTHI(input, delay, init)` | `SMTH1(input, delay, init)` | With initial value |
| `SMOOTH3(input, delay)` | `SMTH3(input, delay)` | Third-order exponential smooth |
| `SMOOTH3I(input, delay, init)` | `SMTH3(input, delay, init)` | With initial value |
| `SMOOTH N(input, dt, init, n)` | `SMTHN(input, dt, n, init)` | Nth-order; arguments reordered |
| `DELAY1(input, delay)` | `DELAY1(input, delay)` | First-order material delay |
| `DELAY1I(input, delay, init)` | `DELAY1(input, delay, init)` | With initial value |
| `DELAY3(input, delay)` | `DELAY3(input, delay)` | Third-order material delay |
| `DELAY3I(input, delay, init)` | `DELAY3(input, delay, init)` | With initial value |
| `DELAY FIXED(input, delay, init)` | `DELAY(input, delay, init)` | Fixed (pipeline) delay |
| `DELAY N(input, dt, init, n)` | `DELAYN(input, dt, n, init)` | Nth-order; arguments reordered |
| `FORECAST(input, avg_time, horizon)` | `FORCST(input, avg_time, horizon)` | |

### Math and Logic

| Vensim | XMILE (Simlin) | Notes |
|--------|----------------|-------|
| `VMAX(a, b)` | `MAX(a, b)` | |
| `VMIN(a, b)` | `MIN(a, b)` | |
| `LOG(x)` (1 arg) | `LOG10(x)` | Vensim LOG is base-10 |
| `LOG(x, base)` (2 args) | `(LN(x) / LN(base))` | Arbitrary base |
| `INTEGER(x)` | `INT(x)` | Truncate to integer |
| `MODULO(a, b)` | `(a) MOD (b)` | Infix operator in XMILE |
| `:AND:` | `AND` / `and` | Logical operator |
| `:OR:` | `OR` / `or` | Logical operator |
| `:NOT:` | `NOT` / `not` | Logical operator |

### Array Functions

| Vensim | XMILE (Simlin) | Notes |
|--------|----------------|-------|
| `ELMCOUNT(arr)` | `SIZE(arr)` | Number of elements in dimension |
| `VECTOR RANK(arr)` | `RANK(arr)` | |
| `VECTOR SELECT(...)` | `VECTOR SELECT(...)` | Same name |
| `VECTOR ELM MAP(...)` | `VECTOR ELM MAP(...)` | Same name |
| `VECTOR SORT ORDER(...)` | `VECTOR SORT ORDER(...)` | Same name |

### Random and Special Functions

| Vensim | XMILE (Simlin) | Notes |
|--------|----------------|-------|
| `RANDOM UNIFORM(min, max, seed)` | `UNIFORM(min, max, seed)` | |
| `RANDOM PINK NOISE(...)` | `NORMALPINK(...)` | |
| `LOOKUP INVERT(table, val)` | `LOOKUPINV(table, val)` | |
| `NPV(stream, rate, init, factor)` | `NPV(stream, rate, init, factor)` | Same args |
| `SSHAPE(x)` | `SSHAPE(x)` | Same name |

### Same-Name Functions

These functions keep the same name (spaces become underscores):

`ABS`, `EXP`, `SQRT`, `LN`, `SIN`, `COS`, `TAN`, `ARCSIN`, `ARCCOS`, `ARCTAN`,
`PULSE`, `STEP`, `RAMP`, `QUANTUM`, `SUM`, `SIGN`

## Naming Conventions

- **Vensim** uses spaces in variable names: `Birth Rate`
- **XMILE / Simlin** uses underscores: `birth_rate`
- Names are case-insensitive in equations
- The engine canonicalizes all names to lowercase with underscores

## Equation Format Differences

Vensim equations use a block format with units and comments:

```
Birth Rate = Birth Fraction * Population
  ~ people/year
  ~ The annual number of births.
  |
```

In XMILE / Simlin, units and documentation are separate XML attributes or JSON fields, not embedded in the equation string. Equations contain only the mathematical expression:

```
birth_fraction * population
```

## Subscript (Array) Syntax

Vensim subscript bang syntax sums over a dimension:

```
Total Sales = SUM(Sales[Region!])
```

This becomes a wildcard sum in XMILE:

```
SUM(sales[*])
```

For named subdimensions, the bang resolves to a subrange wildcard:

```
SUM(sales[Region.*])
```

## Stocks in Vensim vs XMILE

In Vensim, stocks use the `INTEG` function:

```
Population = INTEG(births - deaths, initial_population)
```

In XMILE / Simlin, stocks are a variable type with separate fields:
- `equation`: the net flow expression (e.g., `births - deaths`)
- `initial_equation`: the initial value (e.g., `initial_population`)

The `INTEG` function does not exist in XMILE. The conversion is automatic when importing MDL files.

## Default Fallback

Any Vensim function not in the mapping table above is uppercased with spaces replaced by underscores. For example, `Some Custom Function` becomes `SOME_CUSTOM_FUNCTION`.
