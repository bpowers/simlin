# Theil Statistics

System dynamics model implementing Theil's inequality statistics for comparing
simulated output against historical data, packaged as the THEIL macro.

## Source

- MetaSD post: https://metasd.com/2011/04/theil-statistics/
- Direct download: https://metasd.com/wp-content/uploads/2011/04/Theil_2011.mdl
- Downloaded: 2026-05-13

## Attribution

- Posted by Tom Fiddaman (Ventana Systems) on the MetaSD blog (April 1, 2011).
- The THEIL macro was created by Rogelio Oliva (1995) and updated by Tom
  Fiddaman (2009, 2011) for numerical robustness. The statistical method derives
  from Sterman (1984).

## License

The MetaSD blog is licensed under a Creative Commons Attribution 3.0 Unported
License (CC BY 3.0). The site notes that some Model Library content may be
subject to the original author's copyright; this model carries no separate
license statement.

## Macro content

`Theil_2011.mdl` contains two macros:

- `THEIL(historical, simulated : R2, MAPE, RMSPE, RMSE, MSE, SSE, Dif Mea,
  Dif Var, Dif Cov, Um, Us, Uc, Count)` -- a large multi-output macro (14 named
  outputs declared after the `:` in the signature). Contains many stocks
  (`INTEG`), uses the `:RAW:` variable keyword, `:NA:` values, the `:OR:`
  operator, `ZIDZ`, and the `TIME STEP$` keyword.
- `INIT(x)` -- a one-line wrapper around the `INITIAL` builtin.

The model file also includes a non-macro "molecule" version of the same
statistics for comparison; the post explicitly discusses the macro form.
