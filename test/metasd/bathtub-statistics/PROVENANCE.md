# Bathtub Statistics Sandbox

A sandbox model exploring the statistics of stocks and flows (integration,
measurement error, noise).

## Source

- MetaSD post: https://metasd.com/2012/05/bathtub-statistics-sandbox/
- Direct download: https://metasd.com/wp-content/uploads/2012/05/integration3.mdl
- Downloaded: 2026-05-13

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

`integration3.mdl` (the original Vensim model linked directly on the post)
contains three macros:

- `TREND2(x, horizon, smoothing time)` -- second-order trend estimate; uses the
  `SMOOTH` builtin and has several internal auxiliary equations. Single output,
  no explicit stock.
- `INIT(x)` -- one-line wrapper around the `INITIAL` builtin.
- `PINK NOISE(mean, std deviation, correlation time, seed)` -- contains a stock
  (`INTEG`), uses `RANDOM NORMAL` and `TIME STEP$`.

Note: a later updated package for this post (`integration6.mdl`, dated 2025) has
the macros removed and was not added; only the macro-bearing `integration3.mdl`
is included here.
