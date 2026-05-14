# Interpolating Arrays (Polynomials & Interpolating Functions for Decision Rules)

A model demonstrating cubic spline interpolation over arrayed data, intended for
representing smooth decision rules.

## Source

- MetaSD post: https://metasd.com/2018/02/polynomials-interpolating-functions-decision-rules/
- Direct download: https://metasd.com/wp-content/uploads/2018/02/InterpolatingArrays.mdl
- Downloaded: 2026-05-13

## Attribution

- Posted by Tom Fiddaman (Ventana Systems) on the MetaSD blog (February 2018).

## License

The MetaSD blog is licensed under a Creative Commons Attribution 3.0 Unported
License (CC BY 3.0). The site notes that some Model Library content may be
subject to the original author's copyright; this model carries no separate
license statement.

## Macro content

`InterpolatingArrays.mdl` contains one macro:

- `CUBIC SPLINE(t, p0, p1, m0, m1, left extrap, right extrap)` -- seven
  parameters, single output. Uses nested `IF THEN ELSE` and several internal
  auxiliary equations (Hermite basis functions). No stock.

Note: the companion model from the same post, `Polynomials1.mdl`, does **not**
use macros and was not added.
