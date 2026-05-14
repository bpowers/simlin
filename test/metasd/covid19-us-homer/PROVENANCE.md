# COVID-19 in the US (Homer model)

A model of COVID-19 in the US with endogenous testing, containment measures, and
social distancing. This is the "homer v8" revision distributed on the MetaSD
post.

## Source

- MetaSD post: https://metasd.com/2020/03/model-covid-19-us/
- Direct download (zip): https://metasd.com/wp-content/uploads/2020/03/homer-v8.zip
- Downloaded: 2026-05-13

The contents of `homer-v8.zip` are extracted here as-is (under the `homer v8/`
subdirectory from the archive): `Covid19US v8.mdl`, its `.vpmx` published
companion, the `.vgd` graph definition, the `Covid19US data.xlsx` data file, and
a PDF write-up.

## Attribution

- Posted by Tom Fiddaman (Ventana Systems) on the MetaSD blog (March 2020).
- The model is based on work by Jack Homer; Tom Fiddaman contributed revisions
  ("tf" appears in related filenames on the post). See the MetaSD post and the
  bundled PDF for details.

## License

The MetaSD blog is licensed under a Creative Commons Attribution 3.0 Unported
License (CC BY 3.0). The site notes that some Model Library content may be
subject to the original author's copyright; these model files carry no separate
license statement.

## Macro content

`Covid19US v8.mdl` contains one large macro:

- `SSTATS(historical, simulated : R2, MAE, MAE over Mean, MAPE, RMSE, MSE, Um,
  Us, Uc, Count)` -- a multi-output summary-statistics macro (10 named outputs
  declared after the `:` in the signature). Contains many stocks (`INTEG`), uses
  the `:RAW:` variable keyword, `:NA:` values, the `:OR:` operator, `ZIDZ`, and
  the `TIME STEP$` keyword. This is a renamed variant of the THEIL / summary
  statistics macro that recurs throughout Tom Fiddaman's models.
