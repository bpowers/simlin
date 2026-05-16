# Early Warnings of Catastrophe

A predator-prey model used to explore statistical early-warning signals
(rising variance and autocorrelation) preceding a catastrophic bifurcation.

## Source

- MetaSD post: https://metasd.com/2013/04/early-warnings-of-catastrophe-2/
- Direct download (zip with model and .cin change files):
  https://metasd.com/wp-content/uploads/2013/04/CatastropheWarning.zip
- Downloaded: 2026-05-13

The contents of `CatastropheWarning.zip` are extracted here as-is:
`catastropeWarning2.mdl` (filename spelling preserved from the source archive),
its `.vpm` published companion, and the `.cin` scenario change files.

## Attribution

- Posted by Tom Fiddaman (Ventana Systems) on the MetaSD blog.
- The PINK NOISE macro was contributed by Ed Anderson (MIT / University of
  Texas - Austin) and updated by Tom Fiddaman (2010).

## License

The MetaSD blog is licensed under a Creative Commons Attribution 3.0 Unported
License (CC BY 3.0). The site notes that some Model Library content may be
subject to the original author's copyright; this model carries no separate
license statement.

## Macro content

`catastropeWarning2.mdl` contains four macros, and notably includes macros that
call other macros:

- `MOVING MEAN(x, horizon)` -- contains a stock (`INTEG`), uses `DELAY FIXED`.
- `MOVING AUTOCOV(x, horizon, tau)` -- **calls `MOVING MEAN`** (three times) and
  uses `DELAY FIXED`.
- `MOVING VAR(x, horizon)` -- **calls `MOVING MEAN`** (twice).
- `PINK NOISE(mean, std deviation, correlation time, seed)` -- contains a stock
  (`INTEG`), uses `RANDOM NORMAL` and the `TIME STEP$` keyword.

This model is a good test of macro cross-references (one macro invoking another).
