# Critical Slowing (Bifurcations from Strogatz)

A model demonstrating "critical slowing down" near a bifurcation, added as an
update to the MetaSD post replicating bifurcation examples from Steven
Strogatz's *Nonlinear Dynamics and Chaos*.

## Source

- MetaSD post: https://metasd.com/2011/10/bifurcations-from-strogatz-nonlinear-dynamics-and-chaos/
- Direct download: https://metasd.com/wp-content/uploads/2011/10/critical-slowing.mdl
- Downloaded: 2026-05-13

## Attribution

- Posted by Tom Fiddaman (Ventana Systems) on the MetaSD blog.
- The PINK NOISE macro used in the model was contributed by Ed Anderson (MIT /
  University of Texas - Austin) and updated by Tom Fiddaman (2010).
- The bifurcation examples in the surrounding post are replications from Steven
  Strogatz's *Nonlinear Dynamics and Chaos*.

## License

The MetaSD blog is licensed under a Creative Commons Attribution 3.0 Unported
License (CC BY 3.0). The site notes that some Model Library content may be
subject to the original author's copyright; this model carries no separate
license statement.

## Macro content

`critical-slowing.mdl` contains one macro:

- `PINK NOISE(mean, std deviation, correlation time, seed)` -- contains a stock
  (`INTEG`), uses `RANDOM NORMAL` and the `TIME STEP$` keyword, with several
  internal auxiliary equations. Single output.

Note: the other bifurcation models from the same post
(`3-1-saddle-node-bifurcation.mdl`, `3-2-transcritical-bifurcation.mdl`,
`3-4-pitchfork-bifurcation.mdl`, `8.2-Hopf-bifurcation.mdl`) do **not** use
macros and were not added.
