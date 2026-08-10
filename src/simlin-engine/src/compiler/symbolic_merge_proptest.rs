// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Property tests for [`FragmentMerger`]'s obligations M1-M8, stated over
//! arbitrary well-formed fragment sets.
//!
//! The obligations themselves are written out on `FragmentMerger`'s rustdoc;
//! this file is where they are checked. Every property here is a statement
//! about the merged fragment ALONE -- what its opcodes name, how its resource
//! ranges are laid out, how many opcodes it has. None of them consults another
//! compiler, and none of them re-implements the rule under test as a test-local
//! oracle. The distinction matters: the temp-merging tests these replace
//! computed their expected value with a hand-written copy of the merge rule and
//! then asserted that copy equalled a hard-coded constant, so they could only
//! ever fail if the merger disagreed with a belief about a THIRD implementation
//! -- one that has since become `#[cfg(test)]` and no longer shares an address
//! representation with the code they constrained.
//!
//! The central device is [`Denotation`]: what an opcode's resource-id operands
//! NAME, resolved against the tables that opcode is addressed relative to, plus
//! the opcode with every resource id blanked out. Two opcodes have equal
//! denotations exactly when they are the same instruction over the same
//! resources, whatever ids they spell those resources with -- which is M1 and
//! M7 in one comparison.
//!
//! Case counts are deliberately modest (the whole file is well under a second
//! on a debug build): these are pure in-memory merges with no salsa, no
//! parsing, and no simulation, so a few hundred cases per property costs
//! almost nothing, but the fragment generator is small on purpose too. It is
//! not trying to be a fuzzer for codegen; it is trying to cover the SHAPES the
//! obligations quantify over -- several fragments, resources both referenced
//! and unreferenced, GF blocks that collide by content and blocks that do not,
//! nested GF references, temps, temp-based views, and a self-contained
//! iteration loop.

use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

use super::*;

// ============================================================================
// Generated fragments
// ============================================================================

/// One graphical-function block of a generated fragment: `len` tables whose
/// content is a pure function of `content`, so two blocks generated with the
/// same `(content, len)` are bit-identical and MUST de-duplicate, while any
/// other pair must not.
#[derive(Clone, Debug)]
struct GfBlockSpec {
    content: u8,
    len: usize,
    /// Whether any opcode reads this block. An unread block is the
    /// "over-collected dependency table" case `gf_blocks_of_fragment` fills in
    /// as a gap, and it still consumes ids.
    referenced: bool,
}

/// A generated fragment.
///
/// `tag` makes every resource VALUE unique to this fragment, so an id that
/// lands on the wrong fragment's entry is always detectable rather than
/// accidentally equal. That applies to literals, module names, view dims AND
/// dim-list contents -- `build_fragment` mixes `tag` into each of them, so a
/// spec field here carries the SHAPE and the tag supplies the identity.
///
/// Graphical-function content is the single exception: it is keyed on
/// `GfBlockSpec::content` precisely so collisions across fragments are
/// generated deliberately, which is what M4's de-duplication needs.
#[derive(Clone, Debug)]
struct FragSpec {
    tag: usize,
    n_literals: usize,
    gf_blocks: Vec<GfBlockSpec>,
    n_modules: usize,
    n_views: usize,
    /// Base the generated views on a temp rather than a variable (only honored
    /// when the fragment declares at least one temp).
    views_on_temps: bool,
    /// Sizes of this fragment's temps, at dense 0-based ids.
    temp_sizes: Vec<usize>,
    dim_lists: Vec<Vec<u16>>,
    /// Emit a self-contained `BeginIter .. NextIterOrJump .. EndIter` loop, so
    /// the properties see a backward jump.
    with_loop: bool,
    /// How many per-element writes this fragment makes. Every fragment writes
    /// at least one element, which is what makes it segmentable.
    n_elements: usize,
}

fn gf_block_spec() -> impl Strategy<Value = GfBlockSpec> {
    // Only three distinct contents, so collisions across fragments are common.
    (0u8..3, 1usize..3, any::<bool>()).prop_map(|(content, len, referenced)| GfBlockSpec {
        content,
        len,
        referenced,
    })
}

fn frag_spec() -> impl Strategy<Value = FragSpec> {
    (
        0usize..3, // n_literals
        prop::collection::vec(gf_block_spec(), 0..3),
        0usize..2,                              // n_modules
        0usize..3,                              // n_views
        any::<bool>(),                          // views_on_temps
        prop::collection::vec(1usize..4, 0..3), // temp_sizes
        prop::collection::vec(prop::collection::vec(1u16..8, 1..5), 0..3),
        any::<bool>(), // with_loop
        1usize..4,     // n_elements
    )
        .prop_map(
            |(
                n_literals,
                gf_blocks,
                n_modules,
                n_views,
                views_on_temps,
                temp_sizes,
                dim_lists,
                with_loop,
                n_elements,
            )| FragSpec {
                // Overwritten by `build_fragments` with the fragment's index.
                tag: 0,
                n_literals,
                gf_blocks,
                n_modules,
                n_views,
                views_on_temps,
                temp_sizes,
                dim_lists,
                with_loop,
                n_elements,
            },
        )
}

/// One to four fragments, each tagged with its position so its resource values
/// are unique.
fn frag_specs() -> impl Strategy<Value = Vec<FragSpec>> {
    prop::collection::vec(frag_spec(), 1..5).prop_map(|mut specs| {
        for (i, spec) in specs.iter_mut().enumerate() {
            spec.tag = i + 1;
        }
        specs
    })
}

/// Every fragment carries every resource kind, plus a multi-table GF block, an
/// unreferenced GF block, temp-based views and a loop. The non-vacuity
/// companion to `frag_specs`.
fn forced_rich_specs() -> impl Strategy<Value = Vec<FragSpec>> {
    let rich = (1usize..3, 0u8..3, 1usize..4, 1usize..4).prop_map(
        |(n_literals, content, temp_a, temp_b)| FragSpec {
            tag: 0,
            n_literals,
            gf_blocks: vec![
                GfBlockSpec {
                    content,
                    len: 2,
                    referenced: true,
                },
                GfBlockSpec {
                    content: (content + 1) % 3,
                    len: 1,
                    referenced: false,
                },
            ],
            n_modules: 1,
            n_views: 2,
            views_on_temps: true,
            temp_sizes: vec![temp_a, temp_b],
            dim_lists: vec![vec![2, 3], vec![4]],
            with_loop: true,
            // Two elements, so every fragment has two segments and the loop
            // sits inside one of them.
            n_elements: 2,
        },
    );
    // At least three, so `check_phase_split`'s thirds give every phase a
    // fragment and the derived `ctx_base`s are non-zero.
    prop::collection::vec(rich, 3..5).prop_map(|mut specs| {
        for (i, spec) in specs.iter_mut().enumerate() {
            spec.tag = i + 1;
        }
        specs
    })
}

fn frag_var(tag: usize) -> Ident<Canonical> {
    Ident::new(&format!("v{tag}"))
}

/// The content of table `k` of a block generated with `content`. A pure
/// function of both, so equality of generated blocks is decidable from their
/// specs alone.
fn gf_table(content: u8, k: usize) -> Vec<(f64, f64)> {
    vec![(k as f64, content as f64 * 100.0 + k as f64)]
}

/// Materialize a `FragSpec` into the `PerVarBytecodes` shape
/// `compile_phase_to_per_var_bytecodes` produces: 0-based resource ids, one
/// contiguous run of opcodes per written element, and a single trailing `Ret`.
fn build_fragment(spec: &FragSpec) -> PerVarBytecodes {
    let name = frag_var(spec.tag);

    let literals: Vec<f64> = (0..spec.n_literals)
        .map(|i| (spec.tag * 1000 + i) as f64)
        .collect();

    let mut graphical_functions: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut block_starts: Vec<usize> = Vec::new();
    for block in &spec.gf_blocks {
        block_starts.push(graphical_functions.len());
        for k in 0..block.len {
            graphical_functions.push(gf_table(block.content, k));
        }
    }

    let module_decls: Vec<SymbolicModuleDecl> = (0..spec.n_modules)
        .map(|i| SymbolicModuleDecl {
            model_name: Ident::new(&format!("sub{}_{i}", spec.tag)),
            input_set: BTreeSet::new(),
            var: SymVarRef::new(name.clone(), i),
        })
        .collect();

    let on_temps = spec.views_on_temps && !spec.temp_sizes.is_empty();
    let static_views: Vec<SymbolicStaticView> = (0..spec.n_views)
        .map(|i| SymbolicStaticView {
            // Cycle the three VARIABLE-backed bases (GH #995 added the two
            // snapshot regions): all three name a variable rather than a merged
            // resource, so M1 (referential integrity) and M5 (temp
            // non-aliasing) must hold for each, and only `Temp` is renumbered.
            // Generating just `Var` would leave the two new arms free to be
            // renumbered like a temp with nothing to notice.
            base: if on_temps {
                SymStaticViewBase::Temp((i % spec.temp_sizes.len()) as u32)
            } else {
                let var = SymVarRef::new(name.clone(), i);
                match i % 3 {
                    0 => SymStaticViewBase::Var(var),
                    1 => SymStaticViewBase::PrevVar(var),
                    _ => SymStaticViewBase::InitialVar(var),
                }
            },
            dims: smallvec::smallvec![(spec.tag * 10 + i) as u16],
            strides: smallvec::smallvec![1],
            offset: i as u32,
            sparse: SmallVec::new(),
            dim_ids: SmallVec::new(),
        })
        .collect();

    let temp_sizes: Vec<(u32, usize)> = spec
        .temp_sizes
        .iter()
        .enumerate()
        .map(|(i, size)| (i as u32, *size))
        .collect();

    // Tag-derived, like the literals and the view dims above: two fragments
    // generated from the same shape must still carry DIFFERENT dim lists, or a
    // mis-assigned dim-list id lands on an entry that happens to be equal and
    // nothing notices. `forced_rich_specs` gives every fragment the same shape
    // on purpose, so without this its dim lists were identical and the M8 guard
    // could not see a dim-list base error at all.
    let dim_lists: Vec<Vec<u16>> = spec
        .dim_lists
        .iter()
        .map(|dl| dl.iter().map(|v| v + (spec.tag as u16) * 100).collect())
        .collect();

    // Spread references to the fragment's resources across its elements, so a
    // fragment with several elements exercises several segments and a fragment
    // with one element carries all of them in a single segment.
    let mut code: Vec<SymbolicOpcode> = Vec::new();
    let mut next_lit = 0usize;
    let mut next_block = 0usize;
    let mut next_module = 0usize;
    let mut next_view = 0usize;
    let mut next_dl = 0usize;
    let mut next_temp = 0usize;
    for e in 0..spec.n_elements {
        if next_lit < literals.len() {
            code.push(SymbolicOpcode::LoadConstant {
                id: next_lit as LiteralId,
            });
            next_lit += 1;
        }
        if next_block < spec.gf_blocks.len() {
            let block = &spec.gf_blocks[next_block];
            if block.referenced {
                let start = block_starts[next_block];
                // The whole-block read, exactly the shape codegen emits for a
                // subscripted arrayed GF (`base_gf` + the variable's table
                // count).
                code.push(SymbolicOpcode::Lookup {
                    base_gf: start as GraphicalFunctionId,
                    table_count: block.len as u16,
                    mode: LookupMode::Interpolate,
                });
                if block.len >= 2 {
                    // A NESTED single-table read inside the same block: the
                    // `g[e](x)`-alongside-`g[D!](x)` shape whose runs must be
                    // relocated together.
                    code.push(SymbolicOpcode::Lookup {
                        base_gf: (start + 1) as GraphicalFunctionId,
                        table_count: 1,
                        mode: LookupMode::Interpolate,
                    });
                }
            }
            next_block += 1;
        }
        if next_module < module_decls.len() {
            code.push(SymbolicOpcode::EvalModule {
                id: next_module as ModuleId,
                n_inputs: 0,
            });
            next_module += 1;
        }
        if next_view < static_views.len() {
            code.push(SymbolicOpcode::PushStaticView {
                view_id: next_view as ViewId,
            });
            code.push(SymbolicOpcode::PopView {});
            next_view += 1;
        }
        if next_dl < spec.dim_lists.len() {
            code.push(SymbolicOpcode::PushVarViewDirect {
                var: SymVarRef::base(name.clone()),
                dim_list_id: next_dl as DimListId,
            });
            code.push(SymbolicOpcode::PopView {});
            next_dl += 1;
        }
        if next_temp < temp_sizes.len() {
            code.push(SymbolicOpcode::LoadTempConst {
                temp_id: next_temp as TempId,
                index: 0,
            });
            code.push(SymbolicOpcode::VectorSortOrder {
                write_temp_id: next_temp as TempId,
            });
            next_temp += 1;
        }
        if spec.with_loop && !temp_sizes.is_empty() {
            // Self-contained: `BeginIter` and its backward jump both live
            // inside THIS element's segment, which is what lets the SCC
            // interleaver reorder segments without invalidating the jump.
            code.push(SymbolicOpcode::BeginIter {
                write_temp_id: 0,
                has_write_temp: true,
            });
            code.push(SymbolicOpcode::LoadIterViewAt { offset: 0 });
            code.push(SymbolicOpcode::StoreIterElement {});
            code.push(SymbolicOpcode::NextIterOrJump { jump_back: -2 });
            code.push(SymbolicOpcode::EndIter {});
        }
        code.push(SymbolicOpcode::AssignCurr {
            var: SymVarRef::new(name.clone(), e),
        });
    }
    code.push(SymbolicOpcode::Ret);

    PerVarBytecodes {
        symbolic: SymbolicByteCode { literals, code },
        graphical_functions,
        module_decls,
        static_views,
        temp_sizes,
        dim_lists,
    }
}

fn build_fragments(specs: &[FragSpec]) -> Vec<PerVarBytecodes> {
    specs.iter().map(build_fragment).collect()
}

fn as_refs(frags: &[PerVarBytecodes]) -> Vec<&PerVarBytecodes> {
    frags.iter().collect()
}

/// A fragment's opcode count after its single trailing `Ret` is stripped --
/// the quantity `db::assemble` sums to place the run-invariant flow prefix
/// boundary (M6).
fn ret_stripped_len(frag: &PerVarBytecodes) -> usize {
    let code = &frag.symbolic.code;
    if code.last() == Some(&SymbolicOpcode::Ret) {
        code.len() - 1
    } else {
        code.len()
    }
}

// ============================================================================
// Denotation: what an opcode's ids NAME
// ============================================================================

/// The tables an opcode's resource ids are addressed relative to, plus the
/// base to subtract before indexing them.
///
/// A merged table is PHASE-local while the ids the merger hands out are global
/// (they carry `ctx_base`), so dereferencing a merged opcode against a merged
/// table means subtracting the base back off. That asymmetry is real -- it is
/// how `assemble_module` can renumber a phase against preceding phases' counts
/// while every phase reports the same tables -- so the test models it rather
/// than hiding it.
struct ResourceTables<'a> {
    literals: &'a [f64],
    graphical_functions: &'a [Vec<(f64, f64)>],
    module_decls: &'a [SymbolicModuleDecl],
    static_views: &'a [SymbolicStaticView],
    dim_lists: Vec<Vec<u16>>,
    base: ContextResourceCounts,
}

impl<'a> ResourceTables<'a> {
    /// The tables a fragment's own (0-based) opcodes are addressed against.
    fn of_fragment(frag: &'a PerVarBytecodes) -> Self {
        ResourceTables {
            literals: &frag.symbolic.literals,
            graphical_functions: &frag.graphical_functions,
            module_decls: &frag.module_decls,
            static_views: &frag.static_views,
            dim_lists: frag.dim_lists.iter().map(|dl| truncate4(dl)).collect(),
            base: ContextResourceCounts::default(),
        }
    }

    /// The tables a merged stream's opcodes are addressed against, given the
    /// `ctx_base` the merge was run with.
    fn of_merged(merged: &'a ConcatenatedBytecodes, base: &ContextResourceCounts) -> Self {
        ResourceTables {
            literals: &merged.bytecode.literals,
            graphical_functions: &merged.graphical_functions,
            module_decls: &merged.module_decls,
            static_views: &merged.static_views,
            dim_lists: merged
                .dim_lists
                .iter()
                .map(|(n, arr)| arr[..(*n as usize)].to_vec())
                .collect(),
            base: base.clone(),
        }
    }
}

/// The 4-element truncation `absorb_non_gf` applies to a dim list on the way
/// into the merged `(u8, [u16; 4])` representation.
fn truncate4(dl: &[u16]) -> Vec<u16> {
    dl.iter().take(4).copied().collect()
}

/// What one opcode denotes: the resource VALUES its ids name, plus the opcode
/// itself with every resource id blanked.
///
/// Temps are deliberately absent. A temp id does not name a value, it names a
/// slot, and whether two fragments may share a slot is M5 -- checked by
/// `merged_temp_slot_uses_never_interleave`, not here. Blanking the temp ids
/// keeps this comparison from failing on the sharing M5 exists to permit.
#[derive(Clone, Debug, PartialEq)]
struct Denotation {
    skeleton: SymbolicOpcode,
    literals: Vec<u64>,
    graphical_functions: Vec<Vec<(u64, u64)>>,
    module_decls: Vec<SymbolicModuleDecl>,
    static_views: Vec<SymbolicStaticView>,
    dim_lists: Vec<Vec<u16>>,
}

/// Blank every resource id in `op`, leaving the instruction and all its
/// non-id operands (including `SymVarRef`s and jump offsets) untouched.
///
/// The match is EXHAUSTIVE with no catch-all arm, on purpose: a new
/// `SymbolicOpcode` variant must be classified here before this file compiles,
/// where a catch-all would silently treat its ids as constants.
///
/// That is a narrower guarantee than it looks, and the gap is worth naming.
/// Exhaustiveness forces a DECISION about a new variant; it does not force the
/// decision to be right, and it says nothing about the production side.
/// `renumber_opcode` still ends in `other => other.clone()`, so a variant
/// classified here as id-carrying but never given a renumber arm there would
/// come through unrenumbered -- and the properties would not see it either,
/// because the generators emit a fixed opcode set that a new variant is not in
/// until someone adds it. Adding a resource-bearing opcode means touching three
/// places, and only the first is compiler-enforced.
fn blank_resource_ids(op: &SymbolicOpcode) -> SymbolicOpcode {
    match op {
        // ── carry resource ids ──────────────────────────────────────────
        SymbolicOpcode::LoadConstant { .. } => SymbolicOpcode::LoadConstant { id: 0 },
        SymbolicOpcode::AssignConstCurr { var, .. } => SymbolicOpcode::AssignConstCurr {
            var: var.clone(),
            literal_id: 0,
        },
        SymbolicOpcode::Lookup {
            table_count, mode, ..
        } => SymbolicOpcode::Lookup {
            base_gf: 0,
            table_count: *table_count,
            mode: *mode,
        },
        SymbolicOpcode::LookupDirect {
            table_count,
            elem,
            mode,
            ..
        } => SymbolicOpcode::LookupDirect {
            base_gf: 0,
            table_count: *table_count,
            elem: *elem,
            mode: *mode,
        },
        SymbolicOpcode::LookupArray {
            table_count, mode, ..
        } => SymbolicOpcode::LookupArray {
            base_gf: 0,
            table_count: *table_count,
            mode: *mode,
            write_temp_id: 0,
        },
        SymbolicOpcode::EvalModule { n_inputs, .. } => SymbolicOpcode::EvalModule {
            id: 0,
            n_inputs: *n_inputs,
        },
        SymbolicOpcode::PushStaticView { .. } => SymbolicOpcode::PushStaticView { view_id: 0 },
        SymbolicOpcode::PushTempView { .. } => SymbolicOpcode::PushTempView {
            temp_id: 0,
            dim_list_id: 0,
        },
        SymbolicOpcode::PushVarViewDirect { var, .. } => SymbolicOpcode::PushVarViewDirect {
            var: var.clone(),
            dim_list_id: 0,
        },
        SymbolicOpcode::LoadTempConst { index, .. } => SymbolicOpcode::LoadTempConst {
            temp_id: 0,
            index: *index,
        },
        SymbolicOpcode::LoadTempDynamic { .. } => SymbolicOpcode::LoadTempDynamic { temp_id: 0 },
        SymbolicOpcode::BeginIter { has_write_temp, .. } => SymbolicOpcode::BeginIter {
            write_temp_id: 0,
            has_write_temp: *has_write_temp,
        },
        SymbolicOpcode::LoadIterTempElement { .. } => {
            SymbolicOpcode::LoadIterTempElement { temp_id: 0 }
        }
        SymbolicOpcode::BeginBroadcastIter { n_sources, .. } => {
            SymbolicOpcode::BeginBroadcastIter {
                n_sources: *n_sources,
                dest_temp_id: 0,
            }
        }
        SymbolicOpcode::VectorElmMap {
            full_source_len, ..
        } => SymbolicOpcode::VectorElmMap {
            write_temp_id: 0,
            // NOT a resource id: an absolute element count of the source
            // variable, invariant under renumbering, so it stays in the
            // skeleton where a change to it fails the comparison.
            full_source_len: *full_source_len,
        },
        SymbolicOpcode::VectorSortOrder { .. } => {
            SymbolicOpcode::VectorSortOrder { write_temp_id: 0 }
        }
        SymbolicOpcode::Rank { .. } => SymbolicOpcode::Rank { write_temp_id: 0 },
        SymbolicOpcode::AllocateAvailable { .. } => {
            SymbolicOpcode::AllocateAvailable { write_temp_id: 0 }
        }
        SymbolicOpcode::AllocateByPriority { .. } => {
            SymbolicOpcode::AllocateByPriority { write_temp_id: 0 }
        }

        // ── carry no resource id ────────────────────────────────────────
        //
        // `ViewStarRange::subdim_relation_id` indexes `subdim_relations`, which
        // the fragment path never populates (`assemble_module` sets it to
        // `vec![]`) and codegen never emits the opcode, so there is nothing to
        // renumber and nothing this file can pin. Recorded here rather than
        // silently grouped: if that opcode is ever revived, it becomes a sixth
        // renumberable resource and belongs in the arm above.
        SymbolicOpcode::Op2 { .. }
        | SymbolicOpcode::Not { .. }
        | SymbolicOpcode::LoadVar { .. }
        | SymbolicOpcode::SymLoadPrev { .. }
        | SymbolicOpcode::SymLoadInitial { .. }
        | SymbolicOpcode::LoadGlobalVar { .. }
        | SymbolicOpcode::PushSubscriptIndex { .. }
        | SymbolicOpcode::LoadSubscript { .. }
        | SymbolicOpcode::SetCond { .. }
        | SymbolicOpcode::If { .. }
        | SymbolicOpcode::Ret
        | SymbolicOpcode::LoadModuleInput { .. }
        | SymbolicOpcode::AssignCurr { .. }
        | SymbolicOpcode::Apply { .. }
        | SymbolicOpcode::BinOpAssignCurr { .. }
        | SymbolicOpcode::BinOpAssignNext { .. }
        | SymbolicOpcode::ViewSubscriptConst { .. }
        | SymbolicOpcode::ViewSubscriptDynamic { .. }
        | SymbolicOpcode::ViewRange { .. }
        | SymbolicOpcode::ViewRangeDynamic { .. }
        | SymbolicOpcode::ViewStarRange { .. }
        | SymbolicOpcode::ViewWildcard { .. }
        | SymbolicOpcode::ViewTranspose { .. }
        | SymbolicOpcode::PopView { .. }
        | SymbolicOpcode::DupView { .. }
        | SymbolicOpcode::LoadIterElement { .. }
        | SymbolicOpcode::LoadIterViewTop { .. }
        | SymbolicOpcode::LoadIterViewAt { .. }
        | SymbolicOpcode::StoreIterElement { .. }
        | SymbolicOpcode::NextIterOrJump { .. }
        | SymbolicOpcode::EndIter { .. }
        | SymbolicOpcode::ArraySum { .. }
        | SymbolicOpcode::ArrayMax { .. }
        | SymbolicOpcode::ArrayMin { .. }
        | SymbolicOpcode::ArrayMean { .. }
        | SymbolicOpcode::ArrayStddev { .. }
        | SymbolicOpcode::ArraySize { .. }
        | SymbolicOpcode::VectorSelect { .. }
        | SymbolicOpcode::LoadBroadcastElement { .. }
        | SymbolicOpcode::StoreBroadcastElement { .. }
        | SymbolicOpcode::NextBroadcastOrJump { .. }
        | SymbolicOpcode::EndBroadcastIter { .. } => op.clone(),
    }
}

/// Bit patterns of one GF table, so tables compare by exact content.
fn table_bits(table: &[(f64, f64)]) -> Vec<(u64, u64)> {
    table
        .iter()
        .map(|(x, y)| (x.to_bits(), y.to_bits()))
        .collect()
}

/// Resolve `op`'s resource ids against `tables`. `Err` names the id that did
/// not resolve, which is what an M1/M2/M3 break looks like from the outside:
/// an opcode pointing past the table it is supposed to index.
fn denote(op: &SymbolicOpcode, tables: &ResourceTables<'_>) -> Result<Denotation, String> {
    let mut d = Denotation {
        skeleton: blank_resource_ids(op),
        literals: Vec::new(),
        graphical_functions: Vec::new(),
        module_decls: Vec::new(),
        static_views: Vec::new(),
        dim_lists: Vec::new(),
    };
    let index = |id: u16, base: usize, len: usize, what: &str| -> Result<usize, String> {
        let idx = (id as usize)
            .checked_sub(base)
            .ok_or_else(|| format!("{what} id {id} is below its ctx_base {base}"))?;
        if idx >= len {
            return Err(format!(
                "{what} id {id} (index {idx}) is past its table of {len}"
            ));
        }
        Ok(idx)
    };

    match op {
        SymbolicOpcode::LoadConstant { id } => {
            let i = index(*id, 0, tables.literals.len(), "literal")?;
            d.literals.push(tables.literals[i].to_bits());
        }
        SymbolicOpcode::AssignConstCurr { literal_id, .. } => {
            let i = index(*literal_id, 0, tables.literals.len(), "literal")?;
            d.literals.push(tables.literals[i].to_bits());
        }
        SymbolicOpcode::Lookup {
            base_gf,
            table_count,
            ..
        }
        | SymbolicOpcode::LookupArray {
            base_gf,
            table_count,
            ..
        } => {
            for k in 0..(*table_count as usize) {
                let slot = *base_gf as usize + k;
                if slot >= tables.graphical_functions.len() {
                    return Err(format!(
                        "GF run [{base_gf}, {base_gf}+{table_count}) is past its table of {}",
                        tables.graphical_functions.len()
                    ));
                }
                d.graphical_functions
                    .push(table_bits(&tables.graphical_functions[slot]));
            }
        }
        SymbolicOpcode::EvalModule { id, .. } => {
            let i = index(
                *id,
                tables.base.modules,
                tables.module_decls.len(),
                "module",
            )?;
            d.module_decls.push(tables.module_decls[i].clone());
        }
        SymbolicOpcode::PushStaticView { view_id } => {
            let i = index(
                *view_id,
                tables.base.views,
                tables.static_views.len(),
                "view",
            )?;
            d.static_views.push(tables.static_views[i].clone());
        }
        SymbolicOpcode::PushTempView { dim_list_id, .. }
        | SymbolicOpcode::PushVarViewDirect { dim_list_id, .. } => {
            let i = index(
                *dim_list_id,
                tables.base.dim_lists,
                tables.dim_lists.len(),
                "dim list",
            )?;
            d.dim_lists.push(tables.dim_lists[i].clone());
        }
        _ => {}
    }
    Ok(d)
}

/// A `SymbolicStaticView` with a `Temp` base normalized away.
///
/// A view's `Temp` base is a temp reference, so it is renumbered like one and
/// its identity is M5's business; everything else about the view is M1's.
fn view_shape(view: &SymbolicStaticView) -> SymbolicStaticView {
    SymbolicStaticView {
        base: match &view.base {
            SymStaticViewBase::Temp(_) => SymStaticViewBase::Temp(0),
            other => other.clone(),
        },
        ..view.clone()
    }
}

fn denote_shaped(op: &SymbolicOpcode, tables: &ResourceTables<'_>) -> Result<Denotation, String> {
    let mut d = denote(op, tables)?;
    d.static_views = d.static_views.iter().map(view_shape).collect();
    Ok(d)
}

// ============================================================================
// M1 + M6 + M7: every merged opcode names its own fragment's resources
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// M1, M6 and M7 together, which is the merge's whole reason to exist.
    ///
    /// M6 gives the slicing: the merged stream is each fragment's Ret-stripped
    /// opcodes, contiguous and in order, plus one terminal `Ret`. Walking the
    /// original opcodes beside their merged twins, M1 says each pair must
    /// DENOTE the same resources -- the same literal value, the same module
    /// declaration, the same run of GF tables, the same view, the same
    /// dimension list -- even though the ids spelling them differ. M7 says the
    /// rest of the opcode, `SymVarRef` operands and jump offsets included, is
    /// byte-identical.
    ///
    /// The two halves are what a merge bug actually looks like: an id that
    /// resolves to something (so nothing crashes) but to the WRONG something.
    #[test]
    fn merged_ids_dereference_to_their_own_fragments_resources(specs in frag_specs()) {
        check_denotations_and_ret(&build_fragments(&specs))?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Non-vacuity guard for the property above (the GH #739 pattern).
    ///
    /// The base strategy draws resource counts from zero upwards, so an
    /// individual case can carry no modules, no views, no temps and no GF
    /// blocks at all -- and a property that only ever saw those cases would
    /// pass no matter what the merger did with the resources it never got.
    /// `forced_rich_specs` guarantees every fragment carries every resource
    /// kind, and this property additionally ASSERTS that the merged output
    /// contains all six, so coverage cannot silently drain away.
    #[test]
    fn forced_rich_fragments_exercise_every_resource_kind(specs in forced_rich_specs()) {
        let frags = build_fragments(&specs);
        let merged = check_denotations_and_ret(&frags)?;

        prop_assert!(!merged.bytecode.literals.is_empty(), "no literals generated");
        prop_assert!(
            !merged.graphical_functions.is_empty(),
            "no graphical functions generated"
        );
        prop_assert!(!merged.module_decls.is_empty(), "no module decls generated");
        prop_assert!(!merged.static_views.is_empty(), "no static views generated");
        prop_assert!(!merged.temp_offsets.is_empty(), "no temps generated");
        prop_assert!(!merged.dim_lists.is_empty(), "no dim lists generated");
        prop_assert!(
            merged
                .bytecode
                .code
                .iter()
                .any(|op| matches!(op, SymbolicOpcode::NextIterOrJump { .. })),
            "no backward jump generated, so M7's jump-offset half is untested"
        );
        // At least two fragments must share a temp slot, or M5's whole reason
        // to exist is untested by this case.
        prop_assert!(
            frags.iter().filter(|f| !f.temp_sizes.is_empty()).count() >= 2,
            "fewer than two fragments carry temps"
        );
    }
}

/// M1, M6 and M7 together, which is the merge's whole reason to exist. Shared
/// by the arbitrary and the forced-rich properties.
///
/// M6 gives the slicing: the merged stream is each fragment's Ret-stripped
/// opcodes, contiguous and in order, plus one terminal `Ret`. Walking the
/// original opcodes beside their merged twins, M1 says each pair must DENOTE
/// the same resources -- the same literal value, the same module declaration,
/// the same run of GF tables, the same view, the same dimension list -- even
/// though the ids spelling them differ. M7 says the rest of the opcode,
/// `SymVarRef` operands and jump offsets included, is byte-identical.
///
/// The two halves are what a merge bug actually looks like: an id that
/// resolves to something (so nothing crashes) but to the WRONG something.
fn check_denotations_and_ret(
    frags: &[PerVarBytecodes],
) -> Result<ConcatenatedBytecodes, TestCaseError> {
    let refs = as_refs(frags);
    let no_base = ContextResourceCounts::default();
    let merged = concatenate_fragments(&refs, &no_base)
        .map_err(|e| TestCaseError::fail(format!("well-formed fragments must merge: {e}")))?;

    {
        let merged_tables = ResourceTables::of_merged(&merged, &no_base);
        let mut pos = 0usize;
        for frag in frags {
            let n = ret_stripped_len(frag);
            let frag_tables = ResourceTables::of_fragment(frag);
            for k in 0..n {
                let orig = &frag.symbolic.code[k];
                let got = &merged.bytecode.code[pos + k];
                let want = denote_shaped(orig, &frag_tables)
                    .map_err(|e| TestCaseError::fail(format!("fragment opcode {k}: {e}")))?;
                let have = denote_shaped(got, &merged_tables)
                    .map_err(|e| TestCaseError::fail(format!("merged opcode {}: {e}", pos + k)))?;
                prop_assert_eq!(
                    &want,
                    &have,
                    "merged opcode {} does not denote what fragment opcode {} denoted",
                    pos + k,
                    k
                );
            }
            pos += n;
        }

        // M6: exactly the fragments' opcodes plus one terminal `Ret`.
        prop_assert_eq!(
            merged.bytecode.code.len(),
            pos + 1,
            "merged length must be the sum of the Ret-stripped fragment lengths plus one Ret"
        );
        prop_assert_eq!(
            merged.bytecode.code.last(),
            Some(&SymbolicOpcode::Ret),
            "the merged stream must end in a single Ret"
        );
        let rets = merged
            .bytecode
            .code
            .iter()
            .filter(|op| matches!(op, SymbolicOpcode::Ret))
            .count();
        prop_assert_eq!(rets, 1, "no interior Ret may survive the merge");
    }

    Ok(merged)
}

// ============================================================================
// M2: flat resources are appended, so their ranges tile the merged table
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// M2 stated positively: each flat merged table is EXACTLY the
    /// concatenation of the fragments' tables, in fragment order, with the two
    /// documented normalizations (a dim list truncated to four elements, a
    /// `Temp`-based view's base shifted onto the merged temp slot). That is
    /// disjointness and tiling in one statement -- a fragment whose entries
    /// overlapped another's, or that was dropped, or that was reordered, fails
    /// it.
    ///
    /// Graphical functions are deliberately absent: they are the one resource
    /// that shares rather than tiles (M4), and they have their own property.
    #[test]
    fn flat_resource_ranges_are_disjoint_and_tile_the_merged_table(specs in frag_specs()) {
        let frags = build_fragments(&specs);
        let refs = as_refs(&frags);
        // A non-zero recycle base, so the view/temp shift is exercised rather
        // than being the identity.
        let base = ContextResourceCounts {
            temps: 2,
            ..ContextResourceCounts::default()
        };
        let merged = concatenate_fragments(&refs, &base)
            .map_err(|e| TestCaseError::fail(format!("well-formed fragments must merge: {e}")))?;

        let want_literals: Vec<f64> = frags
            .iter()
            .flat_map(|f| f.symbolic.literals.iter().copied())
            .collect();
        prop_assert_eq!(&merged.bytecode.literals, &want_literals);

        let want_modules: Vec<SymbolicModuleDecl> = frags
            .iter()
            .flat_map(|f| f.module_decls.iter().cloned())
            .collect();
        prop_assert_eq!(&merged.module_decls, &want_modules);

        let want_views: Vec<SymbolicStaticView> = frags
            .iter()
            .flat_map(|f| {
                f.static_views.iter().map(|v| SymbolicStaticView {
                    base: match &v.base {
                        SymStaticViewBase::Temp(id) => SymStaticViewBase::Temp(id + base.temps),
                        other => other.clone(),
                    },
                    ..v.clone()
                })
            })
            .collect();
        prop_assert_eq!(&merged.static_views, &want_views);

        let want_dim_lists: Vec<Vec<u16>> = frags
            .iter()
            .flat_map(|f| f.dim_lists.iter().map(|dl| truncate4(dl)))
            .collect();
        let got_dim_lists: Vec<Vec<u16>> = merged
            .dim_lists
            .iter()
            .map(|(n, arr)| arr[..(*n as usize)].to_vec())
            .collect();
        prop_assert_eq!(&got_dim_lists, &want_dim_lists);
    }
}

// ============================================================================
// M4: graphical functions share by content and only by content
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// M4 in three parts.
    ///
    /// **Total content preservation.** The remap covers `[0, gf_len)`, and for
    /// EVERY local slot -- including one no opcode reads -- the merged table at
    /// the remapped id holds that slot's content. This is the safety direction:
    /// it means two slots can share an id only when their content is
    /// bit-identical, so a `Lookup` can never be redirected onto a different
    /// table.
    ///
    /// **Run contiguity.** For every `Lookup`/`LookupArray` run
    /// `[base, base + table_count)`, the remap shifts the whole run by one
    /// delta. `LookupArray` reads `graphical_functions[base_gf ..]` at runtime,
    /// so a run scattered by de-duplication would read the wrong tables even
    /// though each individual table is present somewhere.
    ///
    /// **De-duplication actually happens.** Two fragments built from identical
    /// specs must get identical remaps -- otherwise "de-duplicate" could be
    /// satisfied by never sharing anything, and the property above would still
    /// pass.
    #[test]
    fn gf_dedup_preserves_runs_and_never_merges_distinct_content(specs in frag_specs()) {
        // Duplicate the first fragment at the end, so at least one pair of
        // fragments always carries bit-identical GF blocks.
        let mut specs = specs;
        let mut dup = specs[0].clone();
        dup.tag += 100;
        let dup_index = specs.len();
        specs.push(dup);

        let frags = build_fragments(&specs);
        let refs = as_refs(&frags);
        let dedup = GfDedup::build(&refs)
            .map_err(|e| TestCaseError::fail(format!("well-formed fragments must dedup: {e}")))?;

        for (i, frag) in frags.iter().enumerate() {
            let remap = dedup.remap(i);
            prop_assert_eq!(
                remap.len(),
                frag.graphical_functions.len(),
                "fragment {}'s remap must cover every local GF slot, read or not",
                i
            );
            for (local, table) in frag.graphical_functions.iter().enumerate() {
                let global = remap[local] as usize;
                prop_assert!(
                    global < dedup.tables.len(),
                    "fragment {} slot {} remapped past the deduped table",
                    i,
                    local
                );
                prop_assert_eq!(
                    table_bits(&dedup.tables[global]),
                    table_bits(table),
                    "fragment {} slot {} lost its content under de-duplication",
                    i,
                    local
                );
            }
            for op in &frag.symbolic.code {
                let (base, count) = match op {
                    SymbolicOpcode::Lookup {
                        base_gf,
                        table_count,
                        ..
                    }
                    | SymbolicOpcode::LookupArray {
                        base_gf,
                        table_count,
                        ..
                    } => (*base_gf as usize, *table_count as usize),
                    _ => continue,
                };
                for k in 0..count {
                    prop_assert_eq!(
                        remap[base + k] as usize,
                        remap[base] as usize + k,
                        "fragment {}'s GF run [{}, {}+{}) was scattered by de-duplication",
                        i,
                        base,
                        base,
                        count
                    );
                }
            }
        }

        prop_assert_eq!(
            dedup.remap(0),
            dedup.remap(dup_index),
            "two fragments carrying bit-identical GF blocks must share them"
        );
    }
}

// ============================================================================
// M5: temps
// ============================================================================

/// Every merged temp slot `code` references, paired with the position of the
/// opcode that references it.
///
/// There are TWO channels by which an opcode reaches a temp, and a scan that
/// covers only the first is blind to half of M5:
///
/// 1. an opcode carrying a temp id directly (`VectorSortOrder`, `BeginIter`
///    with a write temp, `LoadTempConst`, ...);
/// 2. a `PushStaticView { view_id }` whose `static_views[view_id].base` is a
///    `Temp`. That base is renumbered by `absorb_non_gf` alongside the opcodes,
///    with the SAME per-fragment `temp_offset`, and it is how an
///    array-producing builtin's scratch array is read back as a view.
///
/// Channel 2 is why this takes the view table. A merger that shifted a view's
/// `Temp` base by the wrong offset would leave every opcode-carried id correct
/// and still hand one fragment another's scratch array -- and until that
/// channel was scanned, nothing in the repository noticed.
fn temp_uses(code: &[SymbolicOpcode], views: &[SymbolicStaticView]) -> Vec<(usize, u32)> {
    code.iter()
        .enumerate()
        .filter_map(|(pc, op)| {
            let id: u32 = match op {
                SymbolicOpcode::PushTempView { temp_id, .. }
                | SymbolicOpcode::LoadTempConst { temp_id, .. }
                | SymbolicOpcode::LoadTempDynamic { temp_id }
                | SymbolicOpcode::LoadIterTempElement { temp_id } => *temp_id as u32,
                SymbolicOpcode::BeginIter {
                    write_temp_id,
                    has_write_temp: true,
                } => *write_temp_id as u32,
                SymbolicOpcode::BeginBroadcastIter { dest_temp_id, .. } => *dest_temp_id as u32,
                SymbolicOpcode::VectorElmMap { write_temp_id, .. }
                | SymbolicOpcode::VectorSortOrder { write_temp_id }
                | SymbolicOpcode::Rank { write_temp_id }
                | SymbolicOpcode::LookupArray { write_temp_id, .. }
                | SymbolicOpcode::AllocateAvailable { write_temp_id }
                | SymbolicOpcode::AllocateByPriority { write_temp_id } => *write_temp_id as u32,
                // Channel 2: the view this opcode pushes may itself be based on
                // a temp.
                SymbolicOpcode::PushStaticView { view_id } => {
                    // Loud on an id the view table cannot answer. A silent
                    // `None` here is the exact shape this scan exists to
                    // eliminate -- a temp channel that goes unread -- so an
                    // out-of-range id must not look like "this opcode uses no
                    // temp". (M1's `denote` reports it too; this is the
                    // belt-and-braces inside the scan itself.)
                    let view = views.get(*view_id as usize).unwrap_or_else(|| {
                        panic!(
                            "PushStaticView names view {view_id} but the table holds {}",
                            views.len()
                        )
                    });
                    match &view.base {
                        SymStaticViewBase::Temp(id) => *id,
                        // Variable-backed bases -- `curr` and the two snapshot
                        // regions -- name no temp channel.
                        SymStaticViewBase::Var(_)
                        | SymStaticViewBase::PrevVar(_)
                        | SymStaticViewBase::InitialVar(_) => return None,
                    }
                }
                _ => return None,
            };
            Some((pc, id))
        })
        .collect()
}

/// The per-slot sizes behind a `ConcatenatedBytecodes`'s `temp_offsets` /
/// `temp_total_size` pair.
fn merged_temp_sizes(merged: &ConcatenatedBytecodes) -> Vec<usize> {
    let offs = &merged.temp_offsets;
    (0..offs.len())
        .map(|i| {
            let end = offs.get(i + 1).copied().unwrap_or(merged.temp_total_size);
            end - offs[i]
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// M5 for the sequential emitter, stated as the live-range condition
    /// itself rather than as a layout.
    ///
    /// Tag every temp use in the merged stream with the fragment that produced
    /// it. For each merged temp SLOT, the tagged uses must form runs: once the
    /// stream leaves fragment A's uses of a slot and enters fragment B's, it
    /// must never come back to A. That is exactly "no two fragments' uses of a
    /// slot interleave", which is what makes identity sharing safe. The same
    /// property is what forbids `Recycle` under an interleaving emitter, where
    /// it fails by construction -- see
    /// `db::combined_fragment_proptest::interleaved_members_never_share_a_temp_slot`
    /// for the other half.
    ///
    /// Also pinned here, because they are the rest of what "share a slot"
    /// means: storage regions derived from `temp_offsets` are disjoint and
    /// cover `temp_total_size`, and every referenced slot exists.
    #[test]
    fn merged_temp_slot_uses_never_interleave(specs in frag_specs()) {
        let frags = build_fragments(&specs);
        let refs = as_refs(&frags);
        let no_base = ContextResourceCounts::default();
        let merged = concatenate_fragments(&refs, &no_base)
            .map_err(|e| TestCaseError::fail(format!("well-formed fragments must merge: {e}")))?;

        // Which merged positions belong to which fragment (M6's slicing).
        let mut owner_of_pc: Vec<usize> = Vec::new();
        for (i, frag) in frags.iter().enumerate() {
            owner_of_pc.extend((0..ret_stripped_len(frag)).map(|_| i));
        }

        let mut seen_owners: HashMap<u32, Vec<usize>> = HashMap::new();
        for (pc, temp) in temp_uses(&merged.bytecode.code, &merged.static_views) {
            if pc >= owner_of_pc.len() {
                continue; // the terminal Ret carries no temp
            }
            let owner = owner_of_pc[pc];
            let runs = seen_owners.entry(temp).or_default();
            if runs.last() != Some(&owner) {
                prop_assert!(
                    !runs.contains(&owner),
                    "merged temp slot {} is used by fragment {} again after another \
                     fragment used it -- two fragments' live ranges interleave on a \
                     shared slot",
                    temp,
                    owner
                );
                runs.push(owner);
            }
        }

        // Storage: `temp_offsets` is a prefix sum, so slots never overlap.
        let sizes = merged_temp_sizes(&merged);
        let mut running = 0usize;
        for (i, size) in sizes.iter().enumerate() {
            prop_assert_eq!(
                merged.temp_offsets[i],
                running,
                "temp slot {} does not start where the previous slot ended",
                i
            );
            running += size;
        }
        prop_assert_eq!(running, merged.temp_total_size);

        // Every referenced slot must exist, or the VM indexes `temp_offsets`
        // out of bounds.
        for (_, temp) in temp_uses(&merged.bytecode.code, &merged.static_views) {
            prop_assert!(
                (temp as usize) < merged.temp_offsets.len(),
                "merged opcode references temp slot {} but the pool has {}",
                temp,
                merged.temp_offsets.len()
            );
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// The `Recycle` half of M5: sharing is by IDENTITY (fragment-local temp
    /// `t` becomes merged slot `t + ctx_base.temps`, whichever fragment it came
    /// from), and a shared slot is sized to the LARGEST of its users.
    ///
    /// Both halves are load-bearing and fail differently. Identity is what
    /// bounds the merged count by the widest single fragment instead of the sum
    /// -- the `u8` `TempId` capacity #583 blew through. Max sizing is what
    /// keeps the fragment that needs the most elements from writing past its
    /// storage into the next slot.
    #[test]
    fn recycle_shares_by_identity_and_sizes_by_max(specs in frag_specs()) {
        let frags = build_fragments(&specs);
        let refs = as_refs(&frags);
        let base = ContextResourceCounts {
            temps: 1,
            ..ContextResourceCounts::default()
        };
        let merged = concatenate_fragments(&refs, &base)
            .map_err(|e| TestCaseError::fail(format!("well-formed fragments must merge: {e}")))?;

        // Identity: read each fragment's temp uses back out of its slice of the
        // merged stream and check the shift is the fixed base, not a per-
        // fragment running sum.
        let mut pos = 0usize;
        for frag in &frags {
            let n = ret_stripped_len(frag);
            let local = temp_uses(&frag.symbolic.code[..n], &frag.static_views);
            let global = temp_uses(&merged.bytecode.code[pos..pos + n], &merged.static_views);
            prop_assert_eq!(local.len(), global.len());
            for ((_, l), (_, g)) in local.iter().zip(global.iter()) {
                prop_assert_eq!(
                    *g,
                    *l + base.temps,
                    "recycle must shift every fragment's temps by the same fixed base"
                );
            }
            pos += n;
        }

        // Max sizing: the slot for local id `t` must be at least as large as
        // every fragment that declared `t`, and no larger than the largest of
        // them (no silent inflation).
        let sizes = merged_temp_sizes(&merged);
        let mut want: HashMap<u32, usize> = HashMap::new();
        for frag in &frags {
            for (id, size) in &frag.temp_sizes {
                let slot = *id + base.temps;
                let e = want.entry(slot).or_insert(0);
                *e = (*e).max(*size);
            }
        }
        for (slot, size) in &want {
            prop_assert!(
                (*slot as usize) < sizes.len(),
                "slot {} was never allocated",
                slot
            );
            prop_assert_eq!(
                sizes[*slot as usize],
                *size,
                "shared temp slot {} must hold exactly the largest of its users",
                slot
            );
        }
    }
}

// ============================================================================
// M6: a prefix of the fragment list is a prefix of the merged stream
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// M6's consequence, and the reason `db::assemble` may compute the
    /// run-invariant flow prefix (GH #712) as a SUM of fragment lengths before
    /// the merge has run: merging only the first `k` fragments produces exactly
    /// the first `sum(ret_stripped[..k])` opcodes of merging all of them.
    ///
    /// Nothing about a fragment's renumbering may depend on what FOLLOWS it,
    /// which is a stronger and more useful statement than "the counts add up".
    /// A merger that, say, assigned GF ids by global frequency would still
    /// preserve opcode counts but would move the boundary's contents.
    #[test]
    fn merge_is_one_to_one_on_opcodes_and_prefix_lengths_are_boundaries(
        specs in frag_specs(),
    ) {
        let frags = build_fragments(&specs);
        let refs = as_refs(&frags);
        let no_base = ContextResourceCounts::default();
        let full = concatenate_fragments(&refs, &no_base)
            .map_err(|e| TestCaseError::fail(format!("well-formed fragments must merge: {e}")))?;

        let mut boundary = 0usize;
        for k in 1..=frags.len() {
            boundary += ret_stripped_len(&frags[k - 1]);
            let prefix = concatenate_fragments(&refs[..k], &no_base)
                .map_err(|e| TestCaseError::fail(format!("prefix merge failed: {e}")))?;
            prop_assert_eq!(
                prefix.bytecode.code.len(),
                boundary + 1,
                "a {}-fragment merge must be {} opcodes plus a Ret",
                k,
                boundary
            );
            prop_assert_eq!(
                &prefix.bytecode.code[..boundary],
                &full.bytecode.code[..boundary],
                "merging {} fragments alone must reproduce the first {} opcodes of \
                 merging all {}",
                k,
                boundary,
                frags.len()
            );
        }
    }
}

// ============================================================================
// M8: the phase split agrees with the whole-model merge
// ============================================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// M8, stated the way the VM experiences it.
    ///
    /// `assemble_module` renumbers initials, flows and stocks SEPARATELY (each
    /// against a `ctx_base` summed from the preceding phases) but builds the
    /// module's `module_decls` / `static_views` / `dim_lists` / `temp_offsets`
    /// from ONE all-phases merge. So the test derefs a phase's renumbered
    /// opcodes against the ALL-PHASES tables and requires them to name what the
    /// fragment named -- which is literally what happens at run time.
    ///
    /// Literals are the deliberate exception, and the property is written to
    /// show why rather than to skip them: each phase's bytecode carries its own
    /// literal pool and the all-phases pool is discarded, so a phase's literal
    /// ids are checked against the PHASE's pool.
    #[test]
    fn phase_split_assigns_the_same_ids_as_the_all_phases_merge(specs in frag_specs()) {
        check_phase_split(&build_fragments(&specs), false)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Non-vacuity guard for the property above, and it matters more here than
    /// anywhere else in this file.
    ///
    /// A phase whose fragments carry no modules, no views and no dim lists
    /// hands the NEXT phase a `ctx_base` of all zeros, and with a zero base the
    /// phase split is trivially the all-phases merge -- the property passes
    /// without testing anything. The base strategy produces that shape often.
    /// `forced_rich_specs` guarantees each fragment carries all three
    /// resources, and `check_phase_split(.., require_nonzero_base = true)`
    /// asserts the derived bases really are non-zero before checking anything.
    ///
    /// Worth the extra property specifically because this is the one obligation
    /// whose violation nothing else in the default (`--lib`) test suite
    /// notices: dropping `ctx_base` from `absorb_non_gf` leaves all 5363 other
    /// tests green.
    #[test]
    fn forced_rich_phase_split_uses_non_zero_bases(specs in forced_rich_specs()) {
        check_phase_split(&build_fragments(&specs), true)?;
    }
}

/// M8, stated the way the VM experiences it. Shared by the arbitrary and the
/// forced-rich properties.
///
/// `assemble_module` renumbers initials, flows and stocks SEPARATELY (each
/// against a `ctx_base` summed from the preceding phases) but builds the
/// module's `module_decls` / `static_views` / `dim_lists` / `temp_offsets` from
/// ONE all-phases merge. So this derefs a phase's renumbered opcodes against
/// the ALL-PHASES tables and requires them to name what the fragment named --
/// which is literally what happens at run time.
///
/// Literals are the deliberate exception, and the check is written to show why
/// rather than to skip them: each phase's bytecode carries its own literal pool
/// and the all-phases pool is discarded, so a phase's literal ids are resolved
/// against the PHASE's pool.
fn check_phase_split(
    frags: &[PerVarBytecodes],
    require_nonzero_base: bool,
) -> Result<(), TestCaseError> {
    let refs = as_refs(frags);
    // Split into the three phases assembly uses. An empty phase is a real
    // shape (a model with no stocks), so the split is allowed to produce
    // one.
    let n = refs.len();
    let a = n / 3;
    let b = a + (n - a) / 2;
    let phases: [&[&PerVarBytecodes]; 3] = [&refs[..a], &refs[a..b], &refs[b..]];

    let counts0 = ContextResourceCounts::from_fragments(phases[0]);
    let counts1 = ContextResourceCounts::from_fragments(phases[1]);
    // Exactly `assemble_module`'s bases: modules / views / dim-lists sum
    // across preceding phases, temps stay at 0 because the temp pool is
    // shared by every phase rather than partitioned.
    let bases = [
        ContextResourceCounts::default(),
        ContextResourceCounts {
            temps: 0,
            ..counts0.clone()
        },
        ContextResourceCounts {
            modules: counts0.modules + counts1.modules,
            views: counts0.views + counts1.views,
            temps: 0,
            dim_lists: counts0.dim_lists + counts1.dim_lists,
        },
    ];

    if require_nonzero_base {
        prop_assert!(
            bases[1].modules > 0 && bases[1].views > 0 && bases[1].dim_lists > 0,
            "the second phase's base must be non-zero or the split is \
                 trivially the all-phases merge: {:?}",
            bases[1]
        );
        prop_assert!(
            bases[2].modules > bases[1].modules,
            "the third phase's base must exceed the second's: {:?} vs {:?}",
            bases[2],
            bases[1]
        );
    }

    let dedup = GfDedup::build(&refs).map_err(|e| TestCaseError::fail(format!("dedup: {e}")))?;
    let all = concatenate_fragments_with_gf(&refs, &ContextResourceCounts::default(), &dedup, 0)
        .map_err(|e| TestCaseError::fail(format!("all-phases merge: {e}")))?;

    // The INITIALS phase does not go through a concat: `eval_initials` runs
    // each initial separately, so assembly renumbers them one at a time through
    // `renumber_initials_phase`, each at literal offset 0. Drive the REAL
    // function and deref its output against the same all-phases tables, so this
    // property covers the path assembly actually takes rather than a concat
    // standing in for it. As an inline loop that function was unreachable from
    // a test, and freezing two of its three accumulators left the whole
    // repository green.
    //
    // Run it over EVERY fragment rather than over `phases[0]`: the thirds
    // split leaves the initials phase with at most one fragment, and with one
    // initial the running offsets are never used at all -- freezing two of the
    // three would still pass. A model in which every variable has an initial is
    // both a real shape and the one that exercises the accumulators, and it
    // keeps the all-phases tables the correct thing to deref against (the
    // fragment list and its order are the same).
    let named_initials: Vec<(String, &PerVarBytecodes)> = refs
        .iter()
        .enumerate()
        .map(|(i, f)| (format!("init{i}"), *f))
        .collect();
    if require_nonzero_base {
        prop_assert!(
            named_initials.len() >= 2,
            "one initial never advances the per-initial offsets, so the check \
             below would be vacuous"
        );
    }
    let initials = crate::db::renumber_initials_phase(&named_initials, &dedup)
        .map_err(|e| TestCaseError::fail(format!("initials renumber: {e}")))?;
    for (frag, compiled) in refs.iter().zip(initials.iter()) {
        let frag_tables = ResourceTables::of_fragment(frag);
        let tables = ResourceTables {
            literals: &compiled.bytecode.literals,
            graphical_functions: &all.graphical_functions,
            module_decls: &all.module_decls,
            static_views: &all.static_views,
            dim_lists: all
                .dim_lists
                .iter()
                .map(|(n, arr)| arr[..(*n as usize)].to_vec())
                .collect(),
            base: ContextResourceCounts::default(),
        };
        let n = ret_stripped_len(frag);
        for k in 0..n {
            let want = denote_shaped(&frag.symbolic.code[k], &frag_tables)
                .map_err(|e| TestCaseError::fail(format!("initial fragment: {e}")))?;
            let have = denote_shaped(&compiled.bytecode.code[k], &tables).map_err(|e| {
                TestCaseError::fail(format!(
                    "initial opcode {k} against the all-phases tables: {e}"
                ))
            })?;
            prop_assert_eq!(
                &want,
                &have,
                "initial opcode {} does not name its fragment's resource in the \
                 module's all-phases tables",
                k
            );
        }
    }

    let mut gf_index_base = 0usize;
    let mut frag_index = 0usize;
    for (p, phase) in phases.iter().enumerate() {
        let concat = concatenate_fragments_with_gf(phase, &bases[p], &dedup, gf_index_base)
            .map_err(|e| TestCaseError::fail(format!("phase {p} merge: {e}")))?;
        // The tables the VM would consult: the phase's OWN literal pool,
        // every other table from the all-phases merge.
        let tables = ResourceTables {
            literals: &concat.bytecode.literals,
            graphical_functions: &all.graphical_functions,
            module_decls: &all.module_decls,
            static_views: &all.static_views,
            dim_lists: all
                .dim_lists
                .iter()
                .map(|(n, arr)| arr[..(*n as usize)].to_vec())
                .collect(),
            base: ContextResourceCounts::default(),
        };

        let mut pos = 0usize;
        for frag in phase.iter() {
            let n = ret_stripped_len(frag);
            let frag_tables = ResourceTables::of_fragment(frag);
            for k in 0..n {
                let want = denote_shaped(&frag.symbolic.code[k], &frag_tables)
                    .map_err(|e| TestCaseError::fail(format!("fragment: {e}")))?;
                let have = denote_shaped(&concat.bytecode.code[pos + k], &tables).map_err(|e| {
                    TestCaseError::fail(format!(
                        "phase {p} opcode {} against the all-phases tables: {e}",
                        pos + k
                    ))
                })?;
                prop_assert_eq!(
                    &want,
                    &have,
                    "phase {} opcode {} does not name its fragment's resource in the \
                         module's all-phases tables",
                    p,
                    pos + k
                );
            }
            pos += n;
            frag_index += 1;
        }
        gf_index_base = frag_index;
    }

    Ok(())
}
