# simlin-formats

The files a model is kept in, read and written: Vensim MDL and VDF, XMILE, the systems format, JSON (native and SD-AI), and the protobuf a stored project is.
It uses [simlin-base](/src/simlin-base/CLAUDE.md) and [simlin-diagram](/src/simlin-diagram/CLAUDE.md), whose flow geometry the readers settle a drawn view by.
The root `CLAUDE.md` holds the development rules; this file is a map of where functionality lives (paths are relative to this crate's `src/`) and of the standing invariants code in this crate keeps.

Design references: [MDL parser](/docs/design/mdl-parser.md), [VDF format](/docs/design/vdf.md), and [XMILE v1.0](/docs/reference/xmile-v1.0.html), the interchange spec; grep it before claiming what it says.

## Formats

- `compat.rs` is the top-level entry: `open_xmile`, `open_vensim`, `open_vensim_with_data`, `open_systems`, `to_xmile`, `to_mdl`, `to_systems`.
  `open_xmile_with_warnings` and `open_vensim_with_warnings` (`open_vensim_with_data_and_warnings` for a file whose data references need a `DataProvider`: the MDL report is made as the file is converted, which fails on a reference it cannot resolve) open the same project and also return an `ImportWarning` for each kind of thing the file holds that the project does not keep (`import_losses.rs` holds the type and the wording both readers share: `29 comments on view 'View 1' are not kept, such as ...`).
- `xmile/` reads and writes XMILE with `simlin:` vendor extensions for element mappings, data sources, EXCEPT metadata, and multi-output macro invocations.
  Two contracts the XMILE writer keeps: every attribute it emits goes through `push_attribute`, and a newline inside a name is spelled as the two-character XMILE escape `\n` (the datamodel's stored form, which Stella also writes), never a raw newline -- XML attribute normalization would read one back as a space; only the free-text attributes decode the escape (`decode_free_text_attribute`).
  `xmile/views.rs` is the one place the view `zoom` changes unit: XMILE stores a percentage, while the datamodel, protobuf, JSON, and TypeScript all carry a factor (1.0 = 100%); `zoom_factor_from_xmile_percent`/`xmile_percent_from_zoom_factor` convert, reading Stella's `zoom="0"` as 1.0.
  `xmile/unread.rs` reports what a file holds that the reader skips, as a pass of its own over the file's XML events that `project_from_reader_with_warnings` runs after the ordinary read: the read is untouched, so a report can neither change the project nor fail the open.
  It also reports what the reader reads as something other than the file says: a graphical function whose `type` names no kind the reader knows (the kinds are matched whatever their case, `GraphicalFunctionKind::from_type_attribute`) opens as the default kind, never as a failed open, and is reported there with its variable, as is a draft-format `discrete="true"`, which the reader does not act on; each warning says the kind the function is read as (the default, or the one its `type` names).
  It looks wherever the reader reads a `<gf>` -- a flow's or an auxiliary's own (`OWN_TABLE`), any variable's `<element>` (`ELEMENT_TABLES`), in a model or a `<macro>` body -- and `unread_tests` derives its rows from what the reader reads, so the two lists cannot drift from it.
  The pass holds no recursion, reads only as deep into an unread element as a label needs (`LABEL_DEPTH`), and leaves out of a label whatever does not unescape, so ill-formed or deeply nested content the read skips is counted without examples.
- `mdl/` is the native Vensim parser and writer (`lexer` -> `normalizer` -> `parser` -> `reader`, `mdl/convert/`, `mdl/view/`, `writer.rs`); `mdl/xmile_compat.rs` formats expressions, and its restructuring arms for builtin-named calls run before macro resolution, so a like-named macro cannot shadow them.
  `vdf.rs` and `vdf/` read Vensim binary data files; the readers are total on arbitrary bytes.
- `serde.rs` maps the datamodel to and from `project_io.proto`, the form the web app's database stores, so the schema only ever gains fields and an absent field reads as the project read without it.
  `serde_proptest.rs` holds the format to keeping a project: `deserialize(serialize(p)) == normalize(p)` on the FIRST trip, over generated `datamodel::Project`s and over what the importers produce from the corpus, where `normalize` is the one list of what protobuf does not keep (variable order, a proto3 zero read as "none", an element subscript's display spelling, and ISEE's AI information, which is outside the engine's scope and has no field in the schema).
  A period in an ident field is rewritten on read (`migrate_stored_ident`, GH #690), so the generators write none and the corpus gate lists the one file it reaches.
  `serde_stored_tests.rs` holds the two rules of the schema to stored projects, the stand-ins for the database's rows (`test/protobuf-compat/`, each `.pb` beside the `Debug` form of the project it reads to): a field's number and type never change, since every field the schema defines is on the wire of some stored project (the test reads `project_io.proto` and walks the stored bytes, so a field added to the schema fails until a stored project carries it), and a field a stored project lacks reads as before.
  The `.pb` files are never rewritten -- bytes a current writer produces pin nothing about reading old ones -- and the test holds each to its SHA-256.
  `deserialize` returns a `ProtobufDecode` error, with what is missing and where, for a message that lacks what every writer writes and the reader has no reading for: the project's sim specs, a variable's kind or equation, an equation's kind, a view element's kind, a table's scales (an empty buffer decodes to such a message).
- `systems/` is the line-oriented systems format.
  Its reader refuses a stock named for one of the run's clock slots (`results::is_implicit_global`, the rule a rename keeps), naming the stock and its line: a variable of that name takes the slot's results key, where hosts read the clock.
  Another builtin's name (`pi`, `time_step`) is a stock like any other, which the translator quotes.
  `data_provider/` resolves `GET DIRECT` references (`FilesystemDataProvider` behind `file_io`).
  `json.rs` and `json_sdai.rs` are the JSON schemas.
- `element_equations.rs` -- where an MDL file's reader wrote a variable out once per element, the copies are taken back in the two cases where they say nothing the one equation does not (`compact_element_equations`, the last pass of `mdl::convert`): every element has a copy and the copies all read the same, which is one equation over the dimensions, or the file wrote an `:EXCEPT:` equation, which is its default with the elements whose copies read otherwise as exceptions.
  A copy is read with its element as "this element", a subscript naming it on an axis that declares every element of the dimension being that dimension's name; `ElementReader` reads an element's equation so for the save check to compare element equations by.
  Nothing else is read into a file: elements most of which agree, or that each read the element before their own, stay one equation per element, because an equation for every element also defines the elements a dimension gains, and only the file can say that of a variable.
  So an equation that computes a subscript (`x[Age - 1]`) is not one equation after a save as MDL: Vensim does not allow numeric subscripts (vensim.com/documentation/22095.html), the writer writes the equation once per element (`mdl::writer::arrayed::per_element_for_vensim`) and warns that the file opens with an equation for each.

## Tests

A test that compiles or runs what it reads uses simlin-engine, which is a dev-dependency.
A test of a save that asks the save check, lands a patch or lays out a diagram is in simlin-tests (`mdl_saves_*.rs`, `json_patch.rs`, `stored_projects.rs` in `src/simlin-tests/tests/integration/`), because those are built on this crate.

## Cargo features and generated files

Features: `schema` (JSON schemas of the JSON formats), `file_io` (CSV/filesystem providers; a production dependency kept out of wasm, which a test build has whatever its features: `cfg(any(test, feature = "file_io"))`), `ext_data` (the Excel data provider), `test-support` (the test helpers other crates' tests use, for `[dev-dependencies]` edges alone).

Generated, do not edit: `src/project_io.gen.rs` from `project_io.proto` (`pnpm build:gen-protobufs`), which carries a hash that a test checks.
