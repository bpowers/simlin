# Wonderland

A Vensim implementation of the Wonderland model -- a compact integrated
economy-population-environment model.

## Source

- MetaSD post: https://metasd.com/2013/07/wonderland/
- Direct download (zip with mdl + cin files):
  https://metasd.com/wp-content/uploads/2013/07/wonderland3.zip
- Downloaded: 2026-05-13

The contents of `wonderland3.zip` are extracted here as-is: `Wonderland3.mdl`
plus the `dream.cin` and `nightmare.cin` scenario change files.

## Attribution

- Posted by Tom Fiddaman (Ventana Systems) on the MetaSD blog. Vensim
  implementation by Tom Fiddaman.
- The Wonderland model originates with Sanderson, and was further developed by
  Herbert/Leeves and by Alexandra Milik, Alexia Prskawetz, Gustav Feichtinger,
  and Warren Sanderson (see the MetaSD post for references).

## License

The MetaSD blog is licensed under a Creative Commons Attribution 3.0 Unported
License (CC BY 3.0). The site notes that some Model Library content may be
subject to the original author's copyright; this model carries no separate
license statement.

## Macro content

`Wonderland3.mdl` contains two macros:

- `P EXP(input)` -- one-line macro for over/underflow-protected exponentiation:
  `EXP(MAX(-50, MIN(50, input)))`. Single output, no stock.
- `SSHAPE(input)` -- one-line macro for an exponential S-shaped (sigmoid) curve.
  Single output, no stock.
