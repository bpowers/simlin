# Social Network Valuation with Logistic Models

Logistic-growth models used to value social-network / daily-deal companies. The
MetaSD post covers Facebook and Groupon; the Groupon models are the ones that
use a Vensim macro.

## Source

- MetaSD post: https://metasd.com/2011/11/facebook-valuation-with-a-logistic-model/
- Direct download (zip): https://metasd.com/wp-content/uploads/2011/11/groupon3.zip
- Downloaded: 2026-05-13

The post links several archives; `groupon3.zip` is the most complete and is
extracted here as-is. It contains three model revisions -- `groupon 1.mdl`,
`groupon 2.mdl`, and `groupon 3.mdl` (all of which use the same macro) -- along
with their `.vpm` published companions and `.cin` / `.voc` / `.vdf` / `.vpd`
and data files.

## Attribution

- Posted by Tom Fiddaman (Ventana Systems) on the MetaSD blog (November 2011).

## License

The MetaSD blog is licensed under a Creative Commons Attribution 3.0 Unported
License (CC BY 3.0). The site notes that some Model Library content may be
subject to the original author's copyright; these model files carry no separate
license statement.

## Macro content

`groupon 1.mdl`, `groupon 2.mdl`, and `groupon 3.mdl` each contain the same
single macro:

- `REPORT(flow, reporting interval)` -- contains a stock (`INTEG`) and uses
  `DELAY FIXED`. Models the accumulation of a flow over a reporting period.
  Single output.

Note: the companion `facebook 3.mdl` (from `facebook3.zip` on the same post)
does **not** use macros and was not added.
