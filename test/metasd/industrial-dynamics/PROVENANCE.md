# Industrial Dynamics (Forrester, Chapter 15)

A Vensim replication of the Chapter 15 model from Jay Forrester's *Industrial
Dynamics*.

## Source

- MetaSD post: https://metasd.com/2010/03/industrial-dynamics/
- Direct download (zip with Vensim .vmf, .mdl and auxiliary files):
  https://metasd.com/wp-content/uploads/2010/03/IDch15.zip
- Downloaded: 2026-05-13

The contents of `IDch15.zip` are extracted here as-is: `IDch15d.mdl` and
`IDch15d.vmf` (the same model in text and binary Vensim formats), plus the
`.vgd` graph definition and `.cin` scenario change files.

## Attribution

- Posted by Tom Fiddaman (Ventana Systems) on the MetaSD blog. Vensim
  replication by Tom Fiddaman.
- The underlying model is from Jay W. Forrester's *Industrial Dynamics*
  (Chapter 15).

## License

The MetaSD blog is licensed under a Creative Commons Attribution 3.0 Unported
License (CC BY 3.0). The site notes that some Model Library content may be
subject to the original author's copyright; this model carries no separate
license statement.

## Macro content

`IDch15d.mdl` contains one macro:

- `CLIP(x1, x2, y1, y2)` -- one-line macro returning
  `IF THEN ELSE(x1 > x2, y1, y2)`. Four parameters, single output, no stock.
  (This is the classic Dynamo `CLIP` function, reimplemented as a Vensim macro.)
