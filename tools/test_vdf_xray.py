import unittest
from contextlib import redirect_stdout
from io import StringIO
import math
from pathlib import Path

from tools import vdf_xray


REPO_ROOT = Path(__file__).resolve().parents[1]


def parse_fixture(relpath: str) -> vdf_xray.VdfFile:
    path = REPO_ROOT / relpath
    return vdf_xray.parse_vdf(path.read_bytes())


def parse_mdl_fixture(relpath: str) -> vdf_xray.MdlModel:
    path = REPO_ROOT / relpath
    return vdf_xray.parse_mdl_model(path.read_text(errors="replace"))


class VdfXrayModelEditingTests(unittest.TestCase):
    def test_parse_mdl_model_preserves_definition_order_and_dimensions(self) -> None:
        model = parse_mdl_fixture("test/bobby/vdf/model_editing/8_change_subscript.mdl")

        self.assertEqual(list(model.dimensions), ["sub1", "sub2", "sub3"])
        self.assertEqual(model.dimensions["sub2"].elements, ["i", "j"])
        self.assertEqual(
            [(definition.source_index, definition.kind, definition.name, definition.dimensions)
             for definition in model.definitions],
            [
                (1, "var", "constant", []),
                (2, "var", "flow", ["sub2"]),
                (3, "stock", "stock", ["sub2"]),
                (4, "var", "v", []),
            ],
        )
        self.assertEqual(model.sketch_names[:4], ["v", "constant", "stock", "flow"])
        self.assertEqual(model.definitions[0].expression, "3.1415")
        self.assertEqual(model.definitions[1].expression, "v * sub2")

    def test_section_scan_finds_expected_eight_sections(self) -> None:
        for relpath in [
            "test/bobby/vdf/model_editing/run_8.vdf",
            "test/bobby/vdf/subscripts/subscripts.vdf",
            "test/xmutil_test_models/Ref.vdf",
        ]:
            vdf = parse_fixture(relpath)
            self.assertEqual(len(vdf.sections), 8, relpath)

    def test_slot_table_layout_is_contiguous_for_small_edit_chain_files(self) -> None:
        for relpath in [
            "test/bobby/vdf/model_editing/run_8.vdf",
            "test/bobby/vdf/subscripts/subscripts.vdf",
            "test/bobby/vdf/water/Current.vdf",
        ]:
            vdf = parse_fixture(relpath)
            layout = vdf_xray.analyze_slot_table_offsets(vdf.slot_table)
            self.assertIsNotNone(layout, relpath)
            self.assertTrue(layout.contiguous_16, relpath)
            self.assertEqual(layout.base, 44, relpath)
            self.assertEqual(layout.distinct_strides, [16], relpath)
            self.assertEqual(layout.missing_16_slots, 0, relpath)

    def test_slot_table_layout_keeps_ref_fixture_even_when_not_contiguous(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")
        layout = vdf_xray.analyze_slot_table_offsets(ref.slot_table)

        self.assertIsNotNone(layout)
        self.assertEqual(len(ref.slot_table), 1153)
        self.assertFalse(layout.contiguous_16)
        self.assertEqual(layout.base, 44)
        self.assertIn(20, layout.distinct_strides)
        self.assertGreater(layout.irregular_stride_count, 0)

    def test_run7_and_run8_keep_same_raw_section6_refs(self) -> None:
        run7 = parse_fixture("test/bobby/vdf/model_editing/run_7.vdf")
        run8 = parse_fixture("test/bobby/vdf/model_editing/run_8.vdf")

        refs7 = [entry.refs for entry in run7.parse_section6_ref_stream()[1]]
        refs8 = [entry.refs for entry in run8.parse_section6_ref_stream()[1]]

        self.assertEqual(refs7, refs8)
        self.assertEqual(refs7, [[316], [44], [332, 316], [284], [332]])

    def test_empty_section6_ref_stream_does_not_walk_past_section_end(self) -> None:
        for relpath in [
            "test/bobby/vdf/model_editing/run_1.vdf",
            "test/bobby/vdf/model_editing/run_2.vdf",
        ]:
            vdf = parse_fixture(relpath)
            skip, entries, stop = vdf.parse_section6_ref_stream()

            self.assertEqual(skip, 0, relpath)
            self.assertEqual(entries, [], relpath)
            self.assertLessEqual(stop, vdf.sections[6].region_end, relpath)

    def test_run8_section3_captures_dimension_switch(self) -> None:
        run8 = parse_fixture("test/bobby/vdf/model_editing/run_8.vdf")
        sec3 = run8.parse_section3_directory()

        self.assertIsNotNone(sec3)
        self.assertEqual(len(sec3.entries), 2)

        self.assertEqual(sec3.entries[0].words, [
            59, 2, 2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 348, 0, 0, 0, 0, 0, 0, 0, 1,
        ])
        self.assertEqual(sec3.entries[1].words, [
            0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 236, 0, 0, 0, 0, 0, 0, 0, 1,
        ])

    def test_run6_run7_run8_keep_same_signatures_for_most_section6_refs(self) -> None:
        run6 = parse_fixture("test/bobby/vdf/model_editing/run_6.vdf")
        run7 = parse_fixture("test/bobby/vdf/model_editing/run_7.vdf")
        run8 = parse_fixture("test/bobby/vdf/model_editing/run_8.vdf")

        for slot_ref in [284, 316, 332]:
            sig6 = vdf_xray.slot_words(run6, slot_ref)
            sig7 = vdf_xray.slot_words(run7, slot_ref)
            sig8 = vdf_xray.slot_words(run8, slot_ref)
            self.assertEqual(sig6, sig7)
            self.assertEqual(sig7, sig8)

        self.assertNotEqual(
            vdf_xray.slot_words(run7, 44),
            vdf_xray.slot_words(run8, 44),
        )

    def test_slot_reference_inventory_tracks_section3_section5_and_section6_uses(self) -> None:
        run8 = parse_fixture("test/bobby/vdf/model_editing/run_8.vdf")
        inventory = vdf_xray.collect_slot_reference_inventory(run8)

        self.assertEqual(
            inventory[348].uses,
            ["sec3[0].axis[0]", "sec5[0].ref[3]"],
        )
        self.assertEqual(
            inventory[236].uses,
            ["sec3[1].axis[0]", "sec5[1].ref[2]"],
        )
        self.assertEqual(
            inventory[332].uses,
            ["sec6[2].ref[0]", "sec6[4].ref[0]"],
        )

    def test_section35_bridge_finds_exact_matches_for_run8(self) -> None:
        run8 = parse_fixture("test/bobby/vdf/model_editing/run_8.vdf")
        sec3 = run8.parse_section3_directory()
        sec5 = run8.parse_section5_sets()

        self.assertIsNotNone(sec3)
        self.assertIsNotNone(sec5)

        matches0 = vdf_xray.classify_section5_bridge_matches(sec3.entries[0], sec5)
        matches1 = vdf_xray.classify_section5_bridge_matches(sec3.entries[1], sec5)

        self.assertEqual(matches0.exact, [0])
        self.assertEqual(matches0.partial, [])
        self.assertEqual(matches0.null_trailing, [2])

        self.assertEqual(matches1.exact, [1])
        self.assertEqual(matches1.partial, [])
        self.assertEqual(matches1.null_trailing, [2])

    def test_section35_bridge_marks_subscripts_as_null_trailing_only(self) -> None:
        subscripts = parse_fixture("test/bobby/vdf/subscripts/subscripts.vdf")
        sec3 = subscripts.parse_section3_directory()
        sec5 = subscripts.parse_section5_sets()

        self.assertIsNotNone(sec3)
        self.assertIsNotNone(sec5)

        matches = vdf_xray.classify_section5_bridge_matches(sec3.entries[0], sec5)
        self.assertEqual(matches.exact, [])
        self.assertEqual(matches.partial, [])
        self.assertEqual(matches.null_trailing, [0])

    def test_run6_section5_payload_and_sec3_axis_size_diverge(self) -> None:
        run6 = parse_fixture("test/bobby/vdf/model_editing/run_6.vdf")
        sec3 = run6.parse_section3_directory()
        sec5 = run6.parse_section5_sets()

        self.assertIsNotNone(sec3)
        self.assertIsNotNone(sec5)

        entry = sec5[0]
        self.assertEqual(entry.n, 3)
        self.assertEqual(vdf_xray.section5_payload_refs(entry), [188, 204, 220])

        matches = vdf_xray.classify_section5_shape_matches(entry, sec3.entries)
        self.assertEqual(matches.exact, [0])
        self.assertEqual(vdf_xray.section5_exact_axis_sizes(entry, sec3.entries), [[2]])
        self.assertNotEqual(entry.n, sec3.entries[0].axis_sizes()[0])

    def test_run8_section5_exact_matches_recover_axis_sizes_from_sec3(self) -> None:
        run8 = parse_fixture("test/bobby/vdf/model_editing/run_8.vdf")
        sec3 = run8.parse_section3_directory()
        sec5 = run8.parse_section5_sets()

        self.assertIsNotNone(sec3)
        self.assertIsNotNone(sec5)

        matches0 = vdf_xray.classify_section5_shape_matches(sec5[0], sec3.entries)
        matches1 = vdf_xray.classify_section5_shape_matches(sec5[1], sec3.entries)
        matches2 = vdf_xray.classify_section5_shape_matches(sec5[2], sec3.entries)

        self.assertEqual(matches0.exact, [0])
        self.assertEqual(matches1.exact, [1])
        self.assertEqual(matches2.exact, [])
        self.assertEqual(vdf_xray.section5_exact_axis_sizes(sec5[0], sec3.entries), [[2]])
        self.assertEqual(vdf_xray.section5_exact_axis_sizes(sec5[1], sec3.entries), [[2]])
        self.assertEqual(matches2.null_trailing, [0])

    def test_run6_composite_section6_entry_has_structural_signature_fingerprint(self) -> None:
        run6 = parse_fixture("test/bobby/vdf/model_editing/run_6.vdf")
        sec6 = run6.parse_section6_ref_stream()

        self.assertIsNotNone(sec6)
        fingerprint = vdf_xray.ref_signature_fingerprint(run6, sec6[1][2].refs)
        self.assertEqual(fingerprint, [[32, 23, 17, 55], [140, 0, 0, 0]])

    def test_run6_field6_zero_uses_active_sec3_index0_shape(self) -> None:
        run6 = parse_fixture("test/bobby/vdf/model_editing/run_6.vdf")
        rec = next(rec for rec in run6.records if rec.ot_index() == 1 and rec.fields[10] == 11)

        self.assertEqual(rec.shape_code(), 0)
        self.assertEqual(vdf_xray.record_shape_length(run6, rec), 2)

    def test_run8_field6_zero_does_not_use_placeholder_sec3_index0_shape(self) -> None:
        run8 = parse_fixture("test/bobby/vdf/model_editing/run_8.vdf")
        rec = next(rec for rec in run8.records if rec.ot_index() == 1 and rec.fields[10] == 11)

        self.assertEqual(rec.shape_code(), 0)
        self.assertIsNone(vdf_xray.record_shape_length(run8, rec))

    def test_run7_record_shape_blocks_expose_overlapping_idx0_candidates(self) -> None:
        run7 = parse_fixture("test/bobby/vdf/model_editing/run_7.vdf")
        blocks = vdf_xray.build_record_shape_blocks(run7)

        stock_block = next(block for block in blocks if block.start == 1 and block.end == 3)
        overlap_block = next(block for block in blocks if block.start == 2 and block.end == 4)

        self.assertEqual(stock_block.ot_codes, [vdf_xray.OT_CODE_STOCK, vdf_xray.OT_CODE_STOCK])
        self.assertEqual(stock_block.sort_keys, [])
        self.assertIn(13, stock_block.shape_record_indices)

        self.assertEqual(
            overlap_block.ot_codes,
            [vdf_xray.OT_CODE_STOCK, 0x17],
        )
        self.assertEqual(overlap_block.sort_keys, [11])

    def test_run7_mdl_alignment_is_precise_for_arrayed_owners_only(self) -> None:
        run7 = parse_fixture("test/bobby/vdf/model_editing/run_7.vdf")
        model = parse_mdl_fixture("test/bobby/vdf/model_editing/7_add_new_subscript.mdl")

        matches = {
            match.definition.name: match.candidate_block_indices
            for match in vdf_xray.match_mdl_definitions_to_blocks(run7, model)
        }

        # Pre-fix, SAVEPER's OT[8..9) system block appeared as a shape block
        # and matched the "constant"/"v" definitions as a spurious candidate.
        # The record-region fix plus decoded system-record filtering drop
        # that block before shape-block construction, so MDL alignment now
        # only proposes genuine model-variable candidates. `flow` and `stock`
        # were unaffected because their previous indices matched model blocks.
        self.assertEqual(matches["flow"], [3])
        self.assertEqual(matches["stock"], [0])
        self.assertEqual(matches["constant"], [2, 4])
        self.assertEqual(matches["v"], [2, 4])

    def test_run8_mdl_alignment_finds_unique_arrayed_block_owners(self) -> None:
        run8 = parse_fixture("test/bobby/vdf/model_editing/run_8.vdf")
        model = parse_mdl_fixture("test/bobby/vdf/model_editing/8_change_subscript.mdl")

        matches = {
            match.definition.name: match.candidate_block_indices
            for match in vdf_xray.match_mdl_definitions_to_blocks(run8, model)
        }

        # Same dynamic as run_7: SAVEPER's OT[8..9) is filtered out at the
        # shape-block layer, so "constant" and "v" no longer pick up the
        # system block as a match candidate.
        self.assertEqual(matches["flow"], [2])
        self.assertEqual(matches["stock"], [0])
        self.assertEqual(matches["constant"], [1, 3])
        self.assertEqual(matches["v"], [1, 3])

    def test_run9_record_shape_blocks_split_hidden_and_visible_stock_regions(self) -> None:
        run9 = parse_fixture("test/bobby/vdf/model_editing/run_9.vdf")
        blocks = vdf_xray.build_record_shape_blocks(run9)

        hidden_block = next(block for block in blocks if block.start == 1 and block.end == 2)
        visible_stock_block = next(block for block in blocks if block.start == 2 and block.end == 4)

        self.assertEqual(hidden_block.ot_codes, [vdf_xray.OT_CODE_STOCK])
        self.assertIn(412, hidden_block.slot_refs)
        self.assertEqual(hidden_block.sort_keys, [5, 13])

        self.assertEqual(
            visible_stock_block.ot_codes,
            [vdf_xray.OT_CODE_STOCK, vdf_xray.OT_CODE_STOCK],
        )
        self.assertEqual(visible_stock_block.sort_keys, [])

    def test_run9_mdl_alignment_excludes_hidden_smooth_stock_from_visible_stock(self) -> None:
        run9 = parse_fixture("test/bobby/vdf/model_editing/run_9.vdf")
        model = parse_mdl_fixture("test/bobby/vdf/model_editing/9_smooth_time.mdl")

        matches = {
            match.definition.name: match.candidate_block_indices
            for match in vdf_xray.match_mdl_definitions_to_blocks(run9, model)
        }

        self.assertEqual(matches["flow"], [3])
        self.assertEqual(matches["stock"], [1])
        self.assertEqual(matches["constant"], [2, 4])
        self.assertEqual(matches["v"], [2, 4])

    def test_run7_owner_blocks_drop_overlapping_non_sentinel_stock_candidate(self) -> None:
        run7 = parse_fixture("test/bobby/vdf/model_editing/run_7.vdf")
        blocks = vdf_xray.build_owner_record_blocks(run7)

        self.assertEqual([(block.start, block.end) for block in blocks], [
            (1, 3),
            (3, 4),
            (5, 7),
            (10, 11),
        ])

        stock_block = blocks[0]
        self.assertFalse(stock_block.hidden)
        # Record indices shifted by +3 after the record-region fix exposed
        # the three previously-missed header-region records; sort_anchors
        # moved from [8] to [11] for the same reason.
        self.assertEqual(stock_block.sentinel_record_indices, [16])
        self.assertEqual(stock_block.direct_sort_keys, [])
        self.assertEqual(stock_block.attached_sort_keys, [11])
        self.assertEqual(stock_block.sort_anchor_record_indices, [11])

    def test_run9_owner_blocks_separate_hidden_helper_and_transfer_visible_stock_sort(self) -> None:
        run9 = parse_fixture("test/bobby/vdf/model_editing/run_9.vdf")
        blocks = vdf_xray.build_owner_record_blocks(run9)

        self.assertEqual([(block.start, block.end, block.hidden) for block in blocks], [
            (1, 2, True),
            (2, 4, False),
            (4, 5, False),
            (6, 8, False),
            (11, 12, False),
        ])

        # Record indices shifted by +4 after the record-region fix exposed
        # four previously-missed leading records (INITIAL TIME sentinel
        # plus three header-region blocks).
        hidden_block = blocks[0]
        self.assertEqual(hidden_block.sentinel_record_indices, [22])
        self.assertEqual(hidden_block.direct_sort_keys, [5])
        self.assertEqual(hidden_block.attached_sort_keys, [5])
        self.assertEqual(hidden_block.slot_refs, [412])
        self.assertEqual(hidden_block.hidden_slot_refs, [412])

        visible_stock_block = blocks[1]
        self.assertEqual(visible_stock_block.sentinel_record_indices, [16])
        self.assertEqual(visible_stock_block.direct_sort_keys, [])
        self.assertEqual(visible_stock_block.attached_sort_keys, [13])
        self.assertEqual(visible_stock_block.sort_anchor_record_indices, [18])

    def test_run9_owner_mdl_alignment_keeps_hidden_helper_out_of_visible_candidates(self) -> None:
        run9 = parse_fixture("test/bobby/vdf/model_editing/run_9.vdf")
        model = parse_mdl_fixture("test/bobby/vdf/model_editing/9_smooth_time.mdl")

        matches = {
            match.definition.name: match.candidate_block_indices
            for match in vdf_xray.match_mdl_definitions_to_owner_blocks(run9, model)
        }

        self.assertEqual(matches["stock"], [1])
        self.assertEqual(matches["flow"], [3])
        self.assertEqual(matches["constant"], [2, 4])
        self.assertEqual(matches["v"], [2, 4])

    def test_water_owner_blocks_drop_system_records_at_source(self) -> None:
        water = parse_fixture("test/bobby/vdf/water/Current.vdf")
        blocks = vdf_xray.build_owner_record_blocks(water)

        # INITIAL TIME (f[2]=9) and FINAL TIME (f[2]=13) records carry
        # sentinels but point at system OT slots (OT[7] and OT[4] here).
        # `sentinel_model_record_indices` filters them out by f[2] so the
        # owner-block set contains only the five visible model variables:
        # water_level (stock), two dynamics, two consts. The system OT
        # slots they would have anchored are still reachable through
        # other VDF-native paths (section-6 class codes, name-table
        # filtering), so there is no loss of information.
        self.assertEqual(
            [(block.start, block.end, block.hidden) for block in blocks],
            [
                (1, 2, False),
                (2, 3, False),
                (3, 4, False),
                (5, 6, False),
                (6, 7, False),
            ],
        )
        visible = [b for b in blocks if not b.hidden]
        self.assertEqual([(b.start, b.end) for b in visible], [
            (1, 2),
            (2, 3),
            (3, 4),
            (5, 6),
            (6, 7),
        ])

    def test_run10_owner_blocks_keep_visible_blocks_and_hide_helper_structurally(self) -> None:
        run10 = parse_fixture("test/bobby/vdf/model_editing/run_10.vdf")
        blocks = vdf_xray.build_owner_record_blocks(run10)

        self.assertEqual([(block.start, block.end, block.hidden) for block in blocks], [
            (1, 2, True),
            (2, 4, False),
            (4, 5, False),
            (6, 8, False),
            (11, 12, False),
        ])

        hidden_block = blocks[0]
        self.assertEqual(hidden_block.direct_sort_keys, [5])
        self.assertEqual(hidden_block.attached_sort_keys, [5, 13])

        visible_stock_block = blocks[1]
        self.assertEqual(visible_stock_block.direct_sort_keys, [])
        self.assertEqual(visible_stock_block.attached_sort_keys, [13])
        # Record index shifted by +4 after the record-region fix exposed the
        # three previously-missed header-region records plus the leading
        # INITIAL TIME sentinel record.
        self.assertEqual(visible_stock_block.sort_anchor_record_indices, [7])

        self.assertEqual(blocks[2].attached_sort_keys, [7])
        self.assertEqual(blocks[3].attached_sort_keys, [9])
        self.assertEqual(blocks[4].attached_sort_keys, [18])

    def test_run10_owner_mdl_alignment_recovers_same_visible_candidates_as_run9(self) -> None:
        run10 = parse_fixture("test/bobby/vdf/model_editing/run_10.vdf")
        model = parse_mdl_fixture("test/bobby/vdf/model_editing/10_reformat.mdl")

        matches = {
            match.definition.name: match.candidate_block_indices
            for match in vdf_xray.match_mdl_definitions_to_owner_blocks(run10, model)
        }

        self.assertEqual(matches["stock"], [1])
        self.assertEqual(matches["flow"], [3])
        self.assertEqual(matches["constant"], [2, 4])
        self.assertEqual(matches["v"], [2, 4])

    def test_water_owner_blocks_in_sentinel_order_match_mdl_sketch_classes(self) -> None:
        water = parse_fixture("test/bobby/vdf/water/Current.vdf")
        model = parse_mdl_fixture("test/bobby/vdf/water/water.mdl")

        sketch_defs = vdf_xray.mdl_sketch_definitions(model)
        blocks = vdf_xray.owner_blocks_in_sentinel_order(water)

        self.assertEqual(
            [definition.name for definition in sketch_defs],
            ["water level", "inflow", "gap", "desired water level", "adjustment time"],
        )
        self.assertEqual(
            [vdf_xray.mdl_definition_runtime_class(definition) for definition in sketch_defs],
            ["stock", "dynamic", "dynamic", "const", "const"],
        )
        self.assertEqual(
            [vdf_xray.owner_block_runtime_class(block) for block in blocks],
            ["stock", "dynamic", "dynamic", "const", "const"],
        )
        self.assertEqual(
            [(block.start, block.end) for block in blocks],
            [(1, 2), (6, 7), (5, 6), (3, 4), (2, 3)],
        )

    def test_pop_owner_blocks_in_sentinel_order_match_mdl_sketch_classes(self) -> None:
        pop = parse_fixture("test/bobby/vdf/pop/Current.vdf")
        model = parse_mdl_fixture("test/bobby/vdf/pop/pop.mdl")

        sketch_defs = vdf_xray.mdl_sketch_definitions(model)
        blocks = vdf_xray.owner_blocks_in_sentinel_order(pop)

        self.assertEqual(
            [definition.name for definition in sketch_defs],
            [
                "young population",
                "producing population",
                "starting",
                "age when first child",
                "births per person",
                "years giving birth",
                "births",
                "ending",
            ],
        )
        self.assertEqual(
            [vdf_xray.mdl_definition_runtime_class(definition) for definition in sketch_defs],
            ["stock", "stock", "dynamic", "const", "const", "const", "dynamic", "dynamic"],
        )
        self.assertEqual(
            [vdf_xray.owner_block_runtime_class(block) for block in blocks],
            ["stock", "stock", "dynamic", "const", "const", "const", "dynamic", "dynamic"],
        )

    def test_run9_owner_blocks_in_sentinel_order_match_mdl_sketch_order(self) -> None:
        run9 = parse_fixture("test/bobby/vdf/model_editing/run_9.vdf")
        model = parse_mdl_fixture("test/bobby/vdf/model_editing/9_smooth_time.mdl")

        sketch_defs = vdf_xray.mdl_sketch_definitions(model)
        blocks = vdf_xray.owner_blocks_in_sentinel_order(run9)
        hidden = vdf_xray.owner_blocks_in_sentinel_order(run9, include_hidden=True)
        hidden = [block for block in hidden if block.hidden]

        self.assertEqual([definition.name for definition in sketch_defs], ["v", "constant", "stock", "flow"])
        self.assertEqual(
            [vdf_xray.mdl_definition_runtime_class(definition) for definition in sketch_defs],
            ["dynamic", "const", "stock", "dynamic"],
        )
        self.assertEqual(
            [vdf_xray.owner_block_runtime_class(block) for block in blocks],
            ["dynamic", "const", "stock", "dynamic"],
        )
        self.assertEqual(
            [(block.start, block.end) for block in blocks],
            [(11, 12), (4, 5), (2, 4), (6, 8)],
        )
        self.assertEqual([(block.start, block.end) for block in hidden], [(1, 2)])

    def test_run10_owner_blocks_in_sentinel_order_tracks_vdf_file_order_not_mdl_sketch_order(self) -> None:
        run10 = parse_fixture("test/bobby/vdf/model_editing/run_10.vdf")
        model = parse_mdl_fixture("test/bobby/vdf/model_editing/10_reformat.mdl")

        sketch_defs = vdf_xray.mdl_sketch_definitions(model)
        blocks = vdf_xray.owner_blocks_in_sentinel_order(run10)
        hidden = vdf_xray.owner_blocks_in_sentinel_order(run10, include_hidden=True)
        hidden = [block for block in hidden if block.hidden]

        self.assertEqual([definition.name for definition in sketch_defs], ["v", "constant", "stock", "flow"])
        self.assertEqual(
            [vdf_xray.mdl_definition_runtime_class(definition) for definition in sketch_defs],
            ["dynamic", "const", "stock", "dynamic"],
        )
        self.assertEqual(
            [vdf_xray.owner_block_runtime_class(block) for block in blocks],
            ["const", "dynamic", "dynamic", "stock"],
        )
        self.assertEqual(
            [(block.start, block.end) for block in blocks],
            [(4, 5), (6, 8), (11, 12), (2, 4)],
        )
        self.assertNotEqual(
            [vdf_xray.mdl_definition_runtime_class(definition) for definition in sketch_defs],
            [vdf_xray.owner_block_runtime_class(block) for block in blocks],
        )
        self.assertEqual([(block.start, block.end) for block in hidden], [(1, 2)])

    def test_run10_owner_sketch_alignment_reports_order_mismatch(self) -> None:
        run10 = parse_fixture("test/bobby/vdf/model_editing/run_10.vdf")
        model = parse_mdl_fixture("test/bobby/vdf/model_editing/10_reformat.mdl")

        buf = StringIO()
        with redirect_stdout(buf):
            vdf_xray.print_owner_sketch_alignment(run10, model, "10_reformat.mdl")
        output = buf.getvalue()

        self.assertIn(
            "note: sentinel/file owner order does not match mdl sketch order in this fixture",
            output,
        )

    def test_print_compare_includes_record_shape_block_diffs_and_mdl_alignment(self) -> None:
        run8 = parse_fixture("test/bobby/vdf/model_editing/run_8.vdf")
        run9 = parse_fixture("test/bobby/vdf/model_editing/run_9.vdf")
        mdl8 = parse_mdl_fixture("test/bobby/vdf/model_editing/8_change_subscript.mdl")
        mdl9 = parse_mdl_fixture("test/bobby/vdf/model_editing/9_smooth_time.mdl")

        buf = StringIO()
        with redirect_stdout(buf):
            vdf_xray.print_compare(
                run8,
                "run_8.vdf",
                run9,
                "run_9.vdf",
                left_mdl=(mdl8, "8_change_subscript.mdl"),
                right_mdl=(mdl9, "9_smooth_time.mdl"),
            )
        output = buf.getvalue()

        self.assertIn("=== Record Shape Block Diffs ===", output)
        self.assertIn("=== Owner Block Diffs ===", output)
        self.assertIn("=== MDL Alignment ===", output)
        self.assertIn("=== Owner MDL Alignment ===", output)
        self.assertIn("=== Owner Sketch Alignment ===", output)
        self.assertIn("src[ 3] stock  stock[sub2] flat=2", output)
        self.assertIn("unmatched blocks:", output)

    def test_run9_best_slot_alignment_prefers_display_shift_for_leading_helpers(self) -> None:
        run9 = parse_fixture("test/bobby/vdf/model_editing/run_9.vdf")

        default_alignment = vdf_xray.score_slot_name_alignment(run9, 0)
        best_alignment = vdf_xray.best_slot_name_alignment(run9)

        self.assertEqual(best_alignment.leading_extra_slots, 2)
        self.assertEqual(best_alignment.hidden_slots, [8, 412])
        self.assertGreater(best_alignment.score, default_alignment.score)

    def test_run9_display_alignment_restores_time_and_sec5_metadata_refs(self) -> None:
        run9 = parse_fixture("test/bobby/vdf/model_editing/run_9.vdf")
        slot_to_names = vdf_xray.build_display_slot_to_names(run9)

        self.assertEqual(slot_to_names[156], ["Time"])
        self.assertEqual(slot_to_names[188], ["TIME STEP"])
        self.assertEqual(slot_to_names[204], ["SAVEPER"])
        self.assertEqual(slot_to_names[220], ["sub1"])

    def test_ref_direct_slot_mapping_is_structural_even_when_display_alignment_shifts(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        alignment = vdf_xray.preferred_slot_name_alignment(ref)
        direct = vdf_xray.build_direct_slot_to_names(ref)
        display = vdf_xray.build_display_slot_to_names(ref)

        self.assertEqual(alignment.leading_extra_slots, 1)
        self.assertEqual(alignment.hidden_slots, [17552])
        self.assertEqual(direct[1724], ["Aggregated Regions"])
        self.assertEqual(direct[316], ["COP"])
        self.assertEqual(direct[16784], ["HFC type"])
        self.assertEqual(direct[5164], ["layers"])
        self.assertEqual(direct[13868], ["Semi Agg"])
        self.assertEqual(direct[1052], ["Target"])
        self.assertNotEqual(display[5164], ["layers"])

    def test_decoded_record_shape_length_excludes_ambiguous_zero_shape(self) -> None:
        run6 = parse_fixture("test/bobby/vdf/model_editing/run_6.vdf")
        rec = next(rec for rec in run6.records if rec.ot_index() == 1 and rec.fields[10] == 11)

        self.assertEqual(rec.shape_code(), 0)
        self.assertEqual(vdf_xray.record_shape_length(run6, rec), 2)
        self.assertIsNone(vdf_xray.decoded_record_shape_length(run6, rec))

    def test_decoded_record_spans_are_direct_pre_reconstruction_facts(self) -> None:
        subscripts = parse_fixture("test/bobby/vdf/subscripts/subscripts.vdf")

        spans = vdf_xray.decoded_record_spans(subscripts)
        by_name = {span.name: span for span in spans}

        self.assertEqual((by_name["a stock"].start, by_name["a stock"].end), (1, 4))
        self.assertEqual((by_name["net flow"].start, by_name["net flow"].end), (6, 9))
        self.assertEqual((by_name["other const"].start, by_name["other const"].end), (9, 12))
        self.assertNotIn("a", by_name)
        self.assertNotIn("b", by_name)
        self.assertNotIn("c", by_name)
        self.assertEqual(vdf_xray.record_span_overlaps(spans), {})

    def test_ref_decoded_record_spans_still_expose_owner_descriptor_overlaps(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        spans = vdf_xray.decoded_record_spans(ref)
        overlaps = vdf_xray.record_span_overlaps(spans)
        names_at_113 = {span.name for span in overlaps[113]}

        self.assertNotIn(0, {span.shape_code for span in spans})
        self.assertGreater(len(overlaps), 0)
        self.assertIn("RS N2O", names_at_113)
        self.assertIn("C AF Sequestered", names_at_113)

    def test_risk_sparse_blocks_use_full_bitmap_grid_but_saved_time_suffix(self) -> None:
        risk = parse_fixture("test/bobby/vdf/econ/risk.vdf")

        self.assertEqual(risk.time_point_count, 213)
        self.assertEqual(risk.bitmap_size, 27)
        self.assertEqual(risk.block_time_point_count, 225)
        self.assertEqual(risk.block_bitmap_size, 29)

        time_values = risk.extract_time_values()
        self.assertIsNotNone(time_values)
        assert time_values is not None
        self.assertEqual((time_values[0], time_values[-1]), (13.0, 225.0))

        final_values = risk.section6_final_values()
        self.assertIsNotNone(final_values)
        assert final_values is not None
        federal_funds_rate = risk.extract_ot_series(42, time_values, risk.section6_ot_class_codes(), final_values)
        self.assertIsNotNone(federal_funds_rate)
        assert federal_funds_rate is not None
        self.assertAlmostEqual(federal_funds_rate[0], 6.909999847412109)
        self.assertAlmostEqual(federal_funds_rate[-1], final_values[42])

    def test_ref_raw_zero_dynamic_ot_entries_are_missing_not_zero_constants(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        time_values = ref.extract_time_values()
        self.assertIsNotNone(time_values)
        assert time_values is not None
        series = ref.extract_ot_series(
            611,
            time_values,
            ref.section6_ot_class_codes(),
            ref.section6_final_values(),
        )

        self.assertIsNotNone(series)
        assert series is not None
        self.assertTrue(all(math.isnan(value) for value in series))

    def test_run2_name_mapping_emits_empty_mapping_when_no_model_records(self) -> None:
        run2 = parse_fixture("test/bobby/vdf/model_editing/run_2.vdf")

        mapping = vdf_xray.map_names_to_owner_blocks(run2)

        # run_2 has only system and padding records; after the record-region
        # fix the two sentinel system records (INITIAL TIME, FINAL TIME) are
        # filtered out of `sentinel_model_record_indices` after decoding f[2]
        # to system names, and the remaining records all have f[6]=0 so no
        # model-variable owner blocks survive.
        self.assertIsNotNone(mapping)
        assert mapping is not None
        self.assertEqual(mapping.variable_names, [])
        self.assertEqual(mapping.system_ot_indices, set())
        self.assertEqual(mapping.unmapped_blocks, [])

    def test_run3_name_mapping_uses_record_name_key_and_recovers_v(self) -> None:
        run3 = parse_fixture("test/bobby/vdf/model_editing/run_3.vdf")

        mapping = vdf_xray.map_names_to_owner_blocks(run3)

        # `v` still maps to OT[5..6). The previous expectation
        # `system_ot_indices == {1}` came from a visible FINAL TIME block
        # at OT[1..2) that is now filtered out of owner blocks at source
        # (decoded system records never become model-variable
        # owners after the record-region fix).
        self.assertIsNotNone(mapping)
        assert mapping is not None
        self.assertEqual(mapping.variable_names, ["v"])
        self.assertEqual(mapping.name_to_block["v"].start, 5)
        self.assertEqual(mapping.system_ot_indices, set())

    def test_mark2_name_mapping_uses_record_name_keys_and_leaves_inner_lookup_wiring_unresolved(self) -> None:
        # The direct f[2] string key recovers the large contiguous block of
        # visible model variables in mark2, but record ownership still leaves
        # some lookup/alias wiring to the model-guided path. This checks the
        # broad coverage and one known block without pinning every alias edge.
        mark2 = parse_fixture("test/bobby/vdf/econ/mark2.vdf")

        mapping = vdf_xray.map_names_to_owner_blocks(mark2)

        self.assertIsNotNone(mapping)
        assert mapping is not None
        self.assertGreaterEqual(len(mapping.variable_names), 55)
        self.assertIn("perceived mortgage balance", mapping.name_to_block)

    def test_lookup_ex_name_mapping_keeps_inline_lookup_variable_and_excludes_definition(self) -> None:
        # The direct f[2] string key lets the stock and evaluated inline
        # lookup variable claim their own OT blocks. The graphical-function
        # definition should not steal the evaluated output's block.
        lookup_ex = parse_fixture("test/bobby/vdf/lookups/lookup_ex.vdf")

        mapping = vdf_xray.map_names_to_owner_blocks(lookup_ex)

        self.assertIsNotNone(mapping)
        assert mapping is not None
        self.assertIn("stock", mapping.name_to_block)
        self.assertIn("inline lookup table", mapping.name_to_block)
        self.assertIn("net change", mapping.name_to_block)
        self.assertNotIn("lookup table 1", mapping.name_to_block)

    def test_run3_extract_named_results_assigns_system_slots_from_records(self) -> None:
        run3 = parse_fixture("test/bobby/vdf/model_editing/run_3.vdf")

        results = vdf_xray.extract_named_results(run3)

        self.assertIsNotNone(results)
        assert results is not None
        by_name = {result.name: result.ot_index for result in results}
        self.assertEqual(by_name["Time"], 0)
        self.assertEqual(by_name["FINAL TIME"], 1)
        self.assertEqual(by_name["INITIAL TIME"], 2)
        self.assertEqual(by_name["SAVEPER"], 3)
        self.assertEqual(by_name["TIME STEP"], 4)
        self.assertEqual(by_name["v"], 5)

    def test_system_records_use_decoded_name_keys_not_canonical_numbers(self) -> None:
        scen01 = parse_fixture("test/metasd/WRLD3-03/SCEN01.VDF")

        self.assertEqual(
            vdf_xray.system_record_name_keys(scen01),
            {17, 21, 25, 29},
        )
        self.assertEqual(
            vdf_xray.system_ot_indices_from_records(scen01),
            {
                "FINAL TIME": 105,
                "INITIAL TIME": 179,
                "SAVEPER": 272,
                "TIME STEP": 286,
            },
        )

    def test_ref_extract_named_results_uses_direct_system_record_slots(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        results = vdf_xray.extract_named_results(ref)

        self.assertIsNotNone(results)
        assert results is not None
        by_name = {result.name: result for result in results}
        expected = {
            "FINAL TIME": (1463, 2100.0),
            "INITIAL TIME": (2001, 1850.0),
            "SAVEPER": (3287, 1.0),
            "TIME STEP": (3793, 0.25),
        }
        for name, (ot_idx, value) in expected.items():
            with self.subTest(name=name):
                result = by_name[name]
                self.assertEqual(result.ot_index, ot_idx)
                self.assertAlmostEqual(result.values[0], value)
                self.assertAlmostEqual(result.values[-1], value)

    def test_lookup_ex_extract_named_results_resolves_stock_from_record_mapping(self) -> None:
        # Record f[2] is a direct name-table string key, so the mapper no
        # longer shifts `stock` onto the dynamic `net change` OT slot.
        lookup_ex = parse_fixture("test/bobby/vdf/lookups/lookup_ex.vdf")

        results = vdf_xray.extract_named_results(lookup_ex)

        self.assertIsNotNone(results)
        assert results is not None
        by_name = {result.name: result.ot_index for result in results}
        self.assertEqual(by_name["stock"], 1)
        self.assertEqual(by_name["inline lookup table"], 4)
        self.assertEqual(by_name["net change"], 5)

    def test_mark2_extract_named_results_keeps_lookup_call_outputs(self) -> None:
        # Graphical-function definitions have section-6 lookup records, but
        # their OT slots are already claimed by evaluated model variables.
        # The record-key mapping should keep the evaluated outputs without
        # duplicating the lookup-definition names onto the same OTs.
        mark2 = parse_fixture("test/bobby/vdf/econ/mark2.vdf")

        results = vdf_xray.extract_named_results(mark2)

        self.assertIsNotNone(results)
        assert results is not None
        names = {result.name for result in results}
        self.assertIn("historical federal funds rate", names)
        self.assertIn("inflation rate", names)
        self.assertNotIn("federal funds rate lookup", names)
        self.assertNotIn("inflation rate lookup", names)

    def test_subscripts_extract_named_results_uses_dimension_element_names(self) -> None:
        subscripts = parse_fixture("test/bobby/vdf/subscripts/subscripts.vdf")

        results = vdf_xray.extract_named_results(subscripts)

        self.assertIsNotNone(results)
        assert results is not None
        names = [result.name for result in results]
        self.assertEqual(
            names[:15],
            [
                "Time",
                "a stock[a]",
                "a stock[b]",
                "a stock[c]",
                "net flow[a]",
                "net flow[b]",
                "net flow[c]",
                "other const[a]",
                "other const[b]",
                "other const[c]",
                "some rate",
                "FINAL TIME",
                "INITIAL TIME",
                "SAVEPER",
                "TIME STEP",
            ],
        )
        self.assertNotIn("a stock[0]", names)

    def test_record_field8_recovers_dimension_element_groups(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        dims = {
            dim.name: dim.elements
            for dim in vdf_xray._recover_dimension_sets(ref)
        }

        self.assertEqual(
            dims["COP"],
            [
                "OECD US",
                "OECD EU",
                "G77 China",
                "G77 India",
                "Remaining Developed",
                "Remaining Developing A",
                "COP Developing B",
            ],
        )
        self.assertEqual(
            dims["HFC type"],
            [
                "HFC134a",
                "HFC23",
                "HFC32",
                "HFC125",
                "HFC143a",
                "HFC152a",
                "HFC227ea",
                "HFC245ca",
                "HFC4310mee",
            ],
        )
        self.assertEqual(dims["layers"], ["layer1", "layer2", "layer3", "layer4"])
        self.assertEqual(
            dims["Semi Agg"],
            ["US", "EU", "China", "India", "Other Developed", "Other Developing"],
        )
        self.assertEqual(dims["Target"], ["t1", "t2", "t3"])
        self.assertEqual(
            dims["Aggregated Regions"],
            ["Developed Countries", "Developing A Countries", "Developing B Countries"],
        )

    def test_run8_dimension_set_recovery_uses_record_groups_and_stock_sort_anchor(self) -> None:
        run8 = parse_fixture("test/bobby/vdf/model_editing/run_8.vdf")

        dims = vdf_xray._recover_dimension_sets(run8)

        self.assertEqual(
            {dim.name: dim.elements for dim in dims},
            {
                "sub1": ["a", "b", "c"],
                "sub2": ["i", "j"],
                "sub3": ["x", "y"],
            },
        )

        results = vdf_xray.extract_named_results(run8)
        self.assertIsNotNone(results)
        assert results is not None
        names = {result.name for result in results}
        self.assertIn("stock[i]", names)
        self.assertIn("stock[j]", names)
        self.assertNotIn("stock[0]", names)
        self.assertNotIn("stock[1]", names)
        # The stock's attached dimension anchor binds the reusable section-3
        # shape template, so same-template sibling owners inherit those labels.
        self.assertIn("flow[i]", names)
        self.assertIn("flow[j]", names)
        self.assertNotIn("flow[0]", names)
        self.assertNotIn("flow[x]", names)

    def test_run7_stock_sort_anchor_selects_sub3_elements(self) -> None:
        run7 = parse_fixture("test/bobby/vdf/model_editing/run_7.vdf")

        results = vdf_xray.extract_named_results(run7)

        self.assertIsNotNone(results)
        assert results is not None
        names = {result.name for result in results}
        self.assertIn("stock[x]", names)
        self.assertIn("stock[y]", names)
        self.assertNotIn("stock[i]", names)
        self.assertNotIn("stock[0]", names)
        self.assertIn("flow[x]", names)
        self.assertIn("flow[y]", names)
        self.assertNotIn("flow[0]", names)

    def test_same_shape_siblings_inherit_anchor_bound_dimension_labels(self) -> None:
        expectations = [
            ("test/bobby/vdf/model_editing/run_8.vdf", ["i", "j"]),
            ("test/bobby/vdf/model_editing/run_9.vdf", ["i", "j"]),
            ("test/bobby/vdf/model_editing/run_10.vdf", ["i", "j"]),
        ]

        for relpath, labels in expectations:
            with self.subTest(relpath=relpath):
                vdf = parse_fixture(relpath)
                results = vdf_xray.extract_named_results(vdf)

                self.assertIsNotNone(results)
                assert results is not None
                names = {result.name for result in results}
                self.assertIn(f"stock[{labels[0]}]", names)
                self.assertIn(f"stock[{labels[1]}]", names)
                self.assertIn(f"flow[{labels[0]}]", names)
                self.assertIn(f"flow[{labels[1]}]", names)
                self.assertNotIn("flow[0]", names)
                self.assertNotIn("flow[1]", names)

    def test_ref_multidim_labels_use_unique_record_group_cardinalities(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")
        block = vdf_xray.OwnerRecordBlock(
            start=0,
            end=63,
            ot_codes=[],
            shape_codes=[221],
        )
        labels = vdf_xray._array_element_labels_for_block(
            ref,
            block,
            vdf_xray._recover_dimension_sets(ref),
        )

        self.assertIsNotNone(labels)
        assert labels is not None
        self.assertEqual(len(labels), 63)
        self.assertEqual(
            labels[:3],
            ["OECD US,HFC134a", "OECD US,HFC23", "OECD US,HFC32"],
        )
        self.assertEqual(labels[-1], "COP Developing B,HFC4310mee")

    def test_ref_dimension_anchors_are_not_mapped_as_series_owners(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        mapping = vdf_xray.map_names_to_owner_blocks(ref)

        self.assertIsNotNone(mapping)
        assert mapping is not None
        for name in [
            "Aggregated Regions",
            "COP",
            "HFC type",
            "layers",
            "Semi Agg",
            "Target",
        ]:
            self.assertNotIn(name, mapping.name_to_block)

    def test_ref_owner_mapping_prunes_lookup_descriptor_overlaps(self) -> None:
        # Several Ref graphical-function descriptor records carry sentinel
        # owner-looking fields and start inside the same OT ranges as real
        # saved variables. The VDF-native extraction path should keep the
        # non-overlapping owner partition, not emit duplicate series for both
        # descriptor and variable names.
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        mapping = vdf_xray.map_names_to_owner_blocks(ref)

        self.assertIsNotNone(mapping)
        assert mapping is not None
        ot_to_names: dict[int, list[str]] = {}
        for name, block in mapping.name_to_block.items():
            for ot_idx in range(block.start, block.end):
                ot_to_names.setdefault(ot_idx, []).append(name)

        duplicates = {
            ot_idx: names
            for ot_idx, names in ot_to_names.items()
            if len(names) > 1
        }
        self.assertEqual(duplicates, {})

        self.assertEqual(mapping.name_to_block["C in Mixed Layer"].start, 137)
        self.assertEqual(mapping.name_to_block["C in Mixed Layer"].length(), 3)
        self.assertEqual(mapping.name_to_block["Cum CO2 at start"].start, 146)
        self.assertEqual(mapping.name_to_block["Cum CO2eq at start"].start, 153)
        self.assertEqual(mapping.name_to_block["Cumulative CO2"].start, 160)

        for descriptor_name in [
            "RS N2O",
            "RS PFC",
            "RS SF6",
            "UN population HIGH LOOKUP",
            "UN population LOW LOOKUP",
            "UN population MED LOOKUP",
            "Specified CO2eq emissions scenario in CO2",
            "Specified Developed CO2eq emissions",
            "Specified Developing A CO2eq emissions",
            "Specified Developing B CO2eq emissions",
            "Specified Global CH4",
        ]:
            self.assertNotIn(descriptor_name, mapping.name_to_block)

    def test_ref_explicit_shape_codes_use_following_section3_entry(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")
        key_to_name_idx = vdf_xray.build_record_name_key_to_name_index(ref)
        records_by_name = {
            ref.names[name_idx]: rec
            for rec in ref.records
            if (name_idx := key_to_name_idx.get(rec.fields[2])) is not None
        }

        expected_lengths = {
            "CO2 FF emissions": 7,
            "Intensity RS target": 21,
            "Semi Agg Population in billions": 6,
            "Diffusion Flux": 12,
            "Proportion of COP to global HFC134a eq": 63,
            "Layer Depth": 4,
            "GWP of HFC": 9,
            "Semi Agg Definition": 42,
        }

        for name, expected_length in expected_lengths.items():
            with self.subTest(name=name):
                self.assertEqual(
                    vdf_xray.record_shape_length(ref, records_by_name[name]),
                    expected_length,
                )

    def test_ref_extraction_uses_corrected_explicit_shape_spans(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        results = vdf_xray.extract_named_results(ref)

        self.assertIsNotNone(results)
        assert results is not None
        names = {result.name for result in results}
        self.assertIn("GWP of HFC[HFC4310mee]", names)
        self.assertNotIn("GWP of HFC[0]", names)
        self.assertIn("Layer Depth[layer4]", names)
        self.assertNotIn("Layer Depth[OECD US,HFC134a]", names)

    def test_record_f2_is_name_table_string_start_word_offset_plus_seven(self) -> None:
        run8 = parse_fixture("test/bobby/vdf/model_editing/run_8.vdf")
        run8_keys = vdf_xray.build_record_name_key_to_name_index(run8)

        self.assertEqual(run8.names[run8_keys[49]], "v")
        self.assertEqual(run8.names[run8_keys[51]], "constant")
        self.assertEqual(run8.names[run8_keys[54]], "stock")
        self.assertEqual(run8.names[run8_keys[57]], "flow")

        lookup = parse_fixture("test/bobby/vdf/lookups/lookup_ex.vdf")
        lookup_keys = vdf_xray.build_record_name_key_to_name_index(lookup)

        self.assertEqual(lookup.names[lookup_keys[32]], "lookup table 1")
        self.assertEqual(lookup.names[lookup_keys[37]], "inline lookup table")
        self.assertEqual(lookup.names[lookup_keys[43]], "stock")
        self.assertEqual(lookup.names[lookup_keys[46]], "net change")

    def test_direct_record_name_key_mapping_recovers_edited_array_owners(self) -> None:
        expected_blocks = {
            "constant": (4, 5),
            "flow": (6, 8),
            "stock": (2, 4),
            "v": (11, 12),
        }

        for relpath in [
            "test/bobby/vdf/model_editing/run_9.vdf",
            "test/bobby/vdf/model_editing/run_10.vdf",
        ]:
            vdf = parse_fixture(relpath)
            mapping = vdf_xray.map_names_to_owner_blocks(vdf)

            self.assertIsNotNone(mapping, relpath)
            assert mapping is not None
            self.assertEqual(set(mapping.name_to_block), set(expected_blocks), relpath)
            self.assertEqual(
                {
                    name: (block.start, block.end)
                    for name, block in mapping.name_to_block.items()
                },
                expected_blocks,
                relpath,
            )

    def test_direct_record_name_key_mapping_recovers_single_variable_files(self) -> None:
        for relpath, expected_start in [
            ("test/bobby/vdf/level_vs_aux/x_is_stock.vdf", 1),
            ("test/bobby/vdf/level_vs_aux/x_is_aux.vdf", 5),
        ]:
            vdf = parse_fixture(relpath)
            mapping = vdf_xray.map_names_to_owner_blocks(vdf)

            self.assertIsNotNone(mapping, relpath)
            assert mapping is not None
            self.assertEqual(mapping.variable_names, ["x"], relpath)
            self.assertEqual(mapping.name_to_block["x"].start, expected_start, relpath)

    def test_direct_record_name_key_mapping_separates_lookup_definition_from_output(self) -> None:
        lookup = parse_fixture("test/bobby/vdf/lookups/lookup_ex.vdf")

        results = vdf_xray.extract_named_results(lookup)

        self.assertIsNotNone(results)
        assert results is not None
        by_name = {result.name: result.ot_index for result in results}
        self.assertEqual(by_name["stock"], 1)
        self.assertEqual(by_name["inline lookup table"], 4)
        self.assertEqual(by_name["net change"], 5)
        self.assertNotEqual(by_name.get("lookup table 1"), 4)


if __name__ == "__main__":
    unittest.main()
