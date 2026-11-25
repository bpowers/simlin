# simlin-save-check

Whether saving a project in a format keeps what its model means.
It is above the formats, which it saves and reads back with, and the engine, which it runs both sides with; libsimlin, simlin-tools and the corpus gates in simlin-tests ask it.
It is a crate of its own because the browser bundle has the check and not the agent tools, which use it too.
The root `CLAUDE.md` holds the development rules; this file is a map of the standing invariants code in this crate keeps.

- `check_save` (`check_save_with_data` for a project opened with a data provider, which the MDL read-back reuses) says whether saving a project in a format keeps what its model means: the project is saved, the save read back, and the two compared, reporting a change whenever it cannot be sure there is none.
  The structure is the definition, compared whether or not the model runs: specs (the project's and each model's own), each dimension's elements (numbered or named), parent and mappings, and each variable's kind, dimensions, elements, per-element equations, applied `:EXCEPT:` default, whether it is a table alone (`variable::is_lookup_only`, the compiler's own verdict), initial values, graphical functions as the compiler reads them, flows, module inputs and the markings that change a simulation, each with what it carries.
  The walk is exhaustive by construction: every `datamodel` type it reads is destructured with no `..`, a field that is no part of the meaning named with the reason, and `save_check_field_tests.rs` rows every field (an edit to it, and the reason the check gives or none).
  Equations are the same when their `ast::CanonicalEqn`s under `Aliases::Engine` are.
  Behaviour compares every variable's series value for value (-0 is 0) when the project simulates, and the errors when it does not; it sees only the run the specs ask for.
  Each `MeaningChange` has a `ChangeKind`: `Results` when the save's results differ, `Structure` when only the definition does; a non-negative marking only one side's stock or flow has is `Results` wherever there are results, because the engine does not enforce the marking and so no run of it shows whether the marking binds (an auxiliary's marking means nothing to the compiler and is not compared).
  The run reads a results column as a variable's through `column_variable` (the whole column first, so a name may hold `$`, `[` or `·`; a compiler helper by `ltm::SYNTHETIC_NODE_PREFIX`, the prefix `capture::synthetic_ident` spells), which the corpus gate and the agent tools' edit gate ask too.
  Variables pair by canonical ident exactly.
  Units, documentation and views are not meaning, and a control variable a format keeps as the specs, with the same value, is no change.
  The check reads the save back with this engine's reader, so a writer and a reader that agree with each other and not with the format's own tool are a fixed point it cannot see through.
  `src/simlin-tests/tests/integration/save_meaning.rs` holds it to the corpus in every pair of formats (read as MDL or XMILE; saved as MDL, XMILE, native JSON, protobuf): one table lists every save that does not keep its model, with the check's verdict and the cause (the format cannot hold it, or a defect), ratcheted both ways, and an independent run comparison holds the check to calling every save whose series differ a change to the results.
  The whole-corpus sweep is a gate (ignored; C-LEARN included); a few models that make every pair run in the default suite against the same table.
