import unittest
from contextlib import redirect_stdout
from io import StringIO
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

        self.assertEqual(matches["flow"], [3])
        self.assertEqual(matches["stock"], [0])
        self.assertEqual(matches["constant"], [2, 4, 5])
        self.assertEqual(matches["v"], [2, 4, 5])

    def test_run8_mdl_alignment_finds_unique_arrayed_block_owners(self) -> None:
        run8 = parse_fixture("test/bobby/vdf/model_editing/run_8.vdf")
        model = parse_mdl_fixture("test/bobby/vdf/model_editing/8_change_subscript.mdl")

        matches = {
            match.definition.name: match.candidate_block_indices
            for match in vdf_xray.match_mdl_definitions_to_blocks(run8, model)
        }

        self.assertEqual(matches["flow"], [2])
        self.assertEqual(matches["stock"], [0])
        self.assertEqual(matches["constant"], [1, 3, 4])
        self.assertEqual(matches["v"], [1, 3, 4])

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
        self.assertEqual(stock_block.sentinel_record_indices, [13])
        self.assertEqual(stock_block.direct_sort_keys, [])
        self.assertEqual(stock_block.attached_sort_keys, [11])
        self.assertEqual(stock_block.sort_anchor_record_indices, [8])

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

        hidden_block = blocks[0]
        self.assertEqual(hidden_block.sentinel_record_indices, [18])
        self.assertEqual(hidden_block.direct_sort_keys, [5])
        self.assertEqual(hidden_block.attached_sort_keys, [5])
        self.assertEqual(hidden_block.slot_refs, [412])
        self.assertEqual(hidden_block.hidden_slot_refs, [412])

        visible_stock_block = blocks[1]
        self.assertEqual(visible_stock_block.sentinel_record_indices, [12])
        self.assertEqual(visible_stock_block.direct_sort_keys, [])
        self.assertEqual(visible_stock_block.attached_sort_keys, [13])
        self.assertEqual(visible_stock_block.sort_anchor_record_indices, [14])

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

    def test_run9_best_slot_alignment_detects_hidden_leading_slots(self) -> None:
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


if __name__ == "__main__":
    unittest.main()
