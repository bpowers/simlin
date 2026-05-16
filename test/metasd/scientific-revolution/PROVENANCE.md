# Path Dependence, Competition, and Succession in the Dynamics of Scientific Revolution

A Vensim translation of the Sterman-Wittenberg model of Kuhnian paradigm
revolutions (originally a Dynamo model).

## Source

- MetaSD post: https://metasd.com/2011/05/path-dependence-competition-and-succession-in-the-dynamics-of-scientific-revolution/
  (the later post https://metasd.com/2017/09/update-path-dependence-competition-succession-dynamics-scientific-revolution-model/
  points back to this one for the Vensim model files)
- Direct downloads:
  - https://metasd.com/wp-content/uploads/2011/05/scirev7.mdl
  - https://metasd.com/wp-content/uploads/2011/05/scirev8.mdl
- Downloaded: 2026-05-13

## Attribution

- Posted by Tom Fiddaman (Ventana Systems) on the MetaSD blog. Vensim
  translation by Tom Fiddaman.
- The underlying model is the Sterman-Wittenberg model of scientific
  revolutions (John Sterman and Jason Wittenberg).

## License

The MetaSD blog is licensed under a Creative Commons Attribution 3.0 Unported
License (CC BY 3.0). The site notes that some Model Library content may be
subject to the original author's copyright; these model files carry no separate
license statement.

## Macro content

Both `scirev7.mdl` (beta release) and `scirev8.mdl` (improved variable names and
diagrams) contain the same two macros:

- `POS(INPUT)` -- one-line macro returning `MAX(INPUT, 1e-010)` (a positive
  floor). Single output, no stock.
- `CLIP(true, false, a, b)` -- one-line macro returning
  `IF THEN ELSE(a >= b, true, false)`. Four parameters, single output, no stock.
