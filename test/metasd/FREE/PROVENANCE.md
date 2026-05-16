# FREE (Feedback-Rich Energy-Economy model)

FREE is a system dynamics integrated assessment model of energy-economy-climate
interactions, documented in Thomas Fiddaman's MIT PhD dissertation, "Feedback
Complexity in Integrated Climate-Economy Models."

## Source

- MetaSD post: https://metasd.com/2014/07/free/
- Direct download (zip): https://metasd.com/wp-content/uploads/2010/06/FREE6.zip
  (the FREE 6 model archive, linked from the 2014 post)
- Downloaded: 2026-05-13

The contents of `FREE6.zip` are extracted here as-is. The archive ships two
parallel trees, `FREE6/FREE6-original/` and `FREE6/FREE6-corrected/`, each with
the main model plus many `.cin` change files, `.cmd` command scripts, `.voc` /
`.vsc` control files, `.dat` data files, and `.vgd` graph definitions, along
with a top-level `Readme.txt`.

## Attribution

- Posted by Tom Fiddaman (Ventana Systems) on the MetaSD blog (July 2014,
  updated August 2016).
- The FREE model was created by Thomas S. Fiddaman as part of his doctoral
  dissertation at the MIT Sloan School of Management.

## License

The MetaSD blog is licensed under a Creative Commons Attribution 3.0 Unported
License (CC BY 3.0). The site notes that some Model Library content may be
subject to the original author's copyright; these model files carry no separate
license statement.

## Macro content

The main model `FREE6/FREE6-original/free 6.mdl` contains one macro:

- `INIT(input)` -- one-line macro returning `INITIAL(input)`, i.e. the `INITIAL`
  builtin made usable anywhere in an expression. Single output, no stock.

Notes:
- The `FREE6-corrected/` tree ships only a binary `free 6-corr.vmf` for the main
  model (no `.mdl`), so the macro-bearing text source is the one in
  `FREE6-original/`.
- The smaller auxiliary `.mdl` files in the archive (`all_data.mdl`,
  `conversion*.mdl`, `energy_pos_loop.mdl`, `energy_tech_cld.mdl`,
  `tech_data.mdl`) do **not** contain macros.
