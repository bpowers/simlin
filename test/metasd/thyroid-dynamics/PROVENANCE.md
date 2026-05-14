# Thyroid Dynamics

A model of thyroid hormone regulation (a translation of a published
physiological model).

## Source

- MetaSD post: https://metasd.com/2017/11/thyroid-dynamics/
- Direct download: https://metasd.com/wp-content/uploads/2017/11/thyroid-2008-d.mdl
- Downloaded: 2026-05-13

## Attribution

- Posted by Tom Fiddaman (Ventana Systems) on the MetaSD blog (November 2017).
  Vensim translation by Tom Fiddaman.
- The underlying thyroid model is from the published physiological-modeling
  literature; see the MetaSD post for references.

## License

The MetaSD blog is licensed under a Creative Commons Attribution 3.0 Unported
License (CC BY 3.0). The site notes that some Model Library content may be
subject to the original author's copyright; this model carries no separate
license statement.

## Macro content

`thyroid-2008-d.mdl` contains two macros, both thin wrappers that expose a
Vensim delay builtin under a different name:

- `DELAYN(Input, DelayTime, Init, Order)` -- one-line wrapper around the
  `DELAY N` builtin.
- `PIPELINE(Input, DelayTime, Init)` -- one-line wrapper around the
  `DELAY MATERIAL` builtin.

Single output each; the stock behavior is delegated to the wrapped builtins.
