# Pink Noise

System dynamics model demonstrating the PINK NOISE macro, which generates an
autocorrelated ("pink") random series whose mean and standard deviation are
insensitive to the time step and correlation time.

## Source

- MetaSD post: https://metasd.com/2010/03/pink-noise/
- Direct download: https://metasd.com/wp-content/uploads/2010/03/PinkNoise2010.mdl
- Downloaded: 2026-05-13

## Attribution

- Posted by Tom Fiddaman (Ventana Systems) on the MetaSD blog.
- The PINK NOISE macro itself was contributed by Ed Anderson (MIT / University
  of Texas - Austin), and updated by Tom Fiddaman in 2010 to add a random
  initial value, correct units, and use the `TIME STEP$` keyword.

## License

The MetaSD blog is licensed under a Creative Commons Attribution 3.0 Unported
License (CC BY 3.0). The site notes that some Model Library content may be
subject to the original author's copyright; this model carries no separate
license statement.

## Macro content

`PinkNoise2010.mdl` contains one macro:

- `PINK NOISE(mean, std deviation, correlation time, seed)` -- contains a stock
  (`INTEG`), uses `RANDOM NORMAL` and the `TIME STEP$` keyword, and has several
  internal auxiliary equations. Single output. The macro appears after a leading
  documentation group in the file (i.e. it is not the very first construct).
