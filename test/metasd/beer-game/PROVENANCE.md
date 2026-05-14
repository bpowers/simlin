# The Beer Game

A Vensim implementation of the classic Beer Distribution Game from system
dynamics. This is the **arrayed/subscripted** version, which is the one that
uses Vensim macros.

## Source

- MetaSD post: https://metasd.com/2018/03/the-beer-game/
- Direct download (zip): https://metasd.com/wp-content/uploads/2018/03/Beer-Game-Fiddaman-Array.zip
- Downloaded: 2026-05-13

The contents of `Beer-Game-Fiddaman-Array.zip` are extracted here as-is:
`RealBeer4-Sterman13.mdl`, its `.vpm` published companion, `.cin` / `.voc`
control files, `.cmd` command scripts, `.vpd` payoff definitions, and the
`Beer Table 4b.xls` / `StermanTable4b.txt` data files.

## Attribution

- Posted by Tom Fiddaman (Ventana Systems) on the MetaSD blog (March 2018).
  Vensim implementation by Tom Fiddaman.
- The underlying Beer Game is the classic MIT system dynamics exercise; John
  Sterman's *Management Science* analysis is the basis for the calibrated
  decision heuristics.
- The PINK NOISE macro included in the model was contributed by Ed Anderson
  (MIT / University of Texas - Austin).

## License

The MetaSD blog is licensed under a Creative Commons Attribution 3.0 Unported
License (CC BY 3.0). The site notes that some Model Library content may be
subject to the original author's copyright; these model files carry no separate
license statement.

## Macro content

`RealBeer4-Sterman13.mdl` contains three macros:

- `PINK NOISE(noise mean, std deviation, correlation time, time step, seed)` --
  contains a stock (`INTEG`), uses `RANDOM NORMAL`, and has several internal
  auxiliary equations.
- `ROUND(x, interval)` -- one-line macro rounding `x` to the nearest `interval`;
  uses the `INTEGER` builtin.
- `PEAK(x)` -- one-line macro tracking the running maximum of `x` using
  `SAMPLE IF TRUE`; the macro body references its own output name.

Note: the companion non-arrayed version from the same post
(`RB4-S13-NoSS-6.mdl` in `Beer-Game-Fiddaman-NoSubscripts.zip`) does **not** use
macros and was not added.
