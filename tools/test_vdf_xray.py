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

    def test_section_header_field1_decodes_section6_and_section7_pointers(self) -> None:
        for relpath in [
            "test/bobby/vdf/model_editing/run_8.vdf",
            "test/bobby/vdf/econ/risk2.vdf",
            "test/metasd/WRLD3-03/SCEN01.VDF",
            "test/xmutil_test_models/Ref.vdf",
        ]:
            with self.subTest(relpath=relpath):
                vdf = parse_fixture(relpath)

                self.assertEqual(
                    vdf.section6_class_code_start_from_field1(),
                    vdf.section6_class_code_start(),
                )
                self.assertEqual(
                    vdf.section7_offset_table_start_from_field1(),
                    vdf.offset_table_start,
                )

    def test_section5_stream_starts_at_data_offset_and_field1_points_to_tail(self) -> None:
        cases = {
            "test/bobby/vdf/model_editing/run_2.vdf": 2,
            "test/bobby/vdf/model_editing/run_7.vdf": 3,
            "test/bobby/vdf/model_editing/run_8.vdf": 3,
            "test/bobby/vdf/model_editing/run_9.vdf": 3,
            "test/bobby/vdf/model_editing/run_10.vdf": 3,
            "test/bobby/vdf/subscripts/subscripts.vdf": 1,
            "test/xmutil_test_models/Ref.vdf": 18,
        }
        for relpath, expected_count in cases.items():
            with self.subTest(relpath=relpath):
                vdf = parse_fixture(relpath)
                sec5 = vdf.sections[5]
                entries = vdf.parse_section5_sets()

                self.assertIsNotNone(entries)
                assert entries is not None
                self.assertEqual(len(entries), expected_count)
                self.assertEqual(entries[0].file_offset, sec5.data_offset())
                self.assertEqual(
                    vdf.section5_region_last_word_from_field1(),
                    sec5.region_end - 4,
                )

        for relpath in [
            "test/bobby/vdf/model_editing/run_1.vdf",
            "test/bobby/vdf/econ/base.vdf",
            "test/metasd/WRLD3-03/SCEN01.VDF",
        ]:
            with self.subTest(relpath=relpath):
                vdf = parse_fixture(relpath)
                entries = vdf.parse_section5_sets()

                self.assertEqual(entries, [])
                self.assertEqual(
                    vdf.section5_region_last_word_from_field1(),
                    vdf.sections[5].region_end - 4,
                )

    def test_section1_records_start_at_fixed_offset_with_short_trailer(self) -> None:
        cases = {
            "test/bobby/vdf/model_editing/run_8.vdf": (8, [32, 131]),
            "test/bobby/vdf/econ/base.vdf": (4, [13352]),
            "test/xmutil_test_models/Ref.vdf": (24, [124, 0, 0, 0, 12320, 26]),
            "test/metasd/WRLD3-03/experiment.vdf": (0, []),
            "test/metasd/WRLD3-03/SCEN01.VDF": (4, [12328]),
            "test/bobby/vdf/lookups/lookup_ex.vdf": (8, [12320, 26]),
        }
        for relpath, (residual_bytes, residual_words) in cases.items():
            with self.subTest(relpath=relpath):
                vdf = parse_fixture(relpath)
                self.assertGreater(len(vdf.records), 0)

                expected_start = vdf.sections[1].data_offset() + vdf_xray.RECORD_REGION_START_OFFSET
                self.assertEqual(vdf.records[0].file_offset, expected_start)

                record_end = vdf.records[-1].file_offset + vdf_xray.RECORD_SIZE
                self.assertEqual(vdf.slot_table_offset - record_end, residual_bytes)
                self.assertLess(vdf.slot_table_offset - record_end, vdf_xray.RECORD_SIZE)
                self.assertEqual(
                    [
                        vdf_xray.u32(vdf.data, offset)
                        for offset in range(record_end, vdf.slot_table_offset, 4)
                    ],
                    residual_words,
                )

    def test_section7_lookup_point_stream_includes_header_payload_words(self) -> None:
        cases = {
            "test/bobby/vdf/lookups/lookup_ex.vdf": [0.0, 30.0, 100.0, 6.0],
            "test/xmutil_test_models/Ref.vdf": [1850.0, 1851.0, 1852.0, 1853.0],
        }
        for relpath, expected_points in cases.items():
            with self.subTest(relpath=relpath):
                vdf = parse_fixture(relpath)
                sec7 = vdf.sections[7]
                stream_start = sec7.file_offset + 16

                self.assertEqual(
                    [
                        vdf_xray.f32(vdf.data, stream_start + i * 4)
                        for i in range(len(expected_points))
                    ],
                    expected_points,
                )
                self.assertEqual(sec7.data_offset(), stream_start + 8)

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

    def test_name_table_skips_declared_deleted_entries_and_resumes(self) -> None:
        risk2 = parse_fixture("test/bobby/vdf/econ/risk2.vdf")

        self.assertEqual(len(risk2.names), 113)
        self.assertEqual(len(risk2.slot_table), 106)
        self.assertEqual(
            risk2.section1_slot_area_offset_from_field1(),
            risk2.slot_table_offset,
        )
        self.assertEqual(risk2.names[45], "max risk")
        self.assertEqual(risk2.names[46], "perceived inflation rate")
        self.assertIn("effect of hud policies on risk taking behavior", risk2.names)

        risk = parse_fixture("test/bobby/vdf/econ/risk.vdf")
        self.assertEqual(risk.names[104], "desired risk taking behavior")
        self.assertIn(
            "#SMOOTH(interestearnedfromderivatives-investmentslostinderivitivedefaults,timedelayininvestmentearnings)#",
            risk.names,
        )

        scen01 = parse_fixture("test/metasd/WRLD3-03/SCEN01.VDF")
        self.assertIn(
            "#LV1<SMOOTH3(ResourceConservationTechnology,technologydevelopmentdelay)#",
            scen01.names,
        )

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
            self.assertEqual(stop, vdf.section6_class_code_start(), relpath)
            self.assertLessEqual(stop, vdf.sections[6].region_end, relpath)

    def test_parse_vdf_accepts_0x53_result_family_magic_for_xray(self) -> None:
        data = bytearray((REPO_ROOT / "test/bobby/vdf/water/Current.vdf").read_bytes())
        data[3] = 0x53

        vdf = vdf_xray.parse_vdf(bytes(data))

        self.assertEqual(vdf.data[:4], vdf_xray.VDF_ALT_RESULT_MAGIC)
        self.assertEqual(len(vdf.sections), 8)
        self.assertEqual(vdf.names[0], "Time")

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

    def test_section3_axis_refs_point_to_dimension_anchor_field9_words(self) -> None:
        cases = {
            "test/bobby/vdf/subscripts/subscripts.vdf": {
                172: "sub1",
            },
            "test/bobby/vdf/model_editing/run_8.vdf": {
                236: "sub3",
                348: "sub2",
            },
            "test/xmutil_test_models/Ref.vdf": {
                540: "scenario",
                636: "COP",
                1852: "HFC type",
                2412: "Aggregated Regions",
                3436: "layers",
                4508: "Semi Agg",
                7036: "Target",
            },
        }

        for relpath, expected in cases.items():
            with self.subTest(relpath=relpath):
                vdf = parse_fixture(relpath)
                anchors_by_ref = vdf_xray.section3_axis_ref_to_dimension_anchor(vdf)

                self.assertEqual(
                    {ref: anchors_by_ref[ref].name for ref in expected},
                    expected,
                )
                sec1_data_offset = vdf.sections[1].data_offset()
                for axis_ref, expected_name in expected.items():
                    anchor = anchors_by_ref[axis_ref]
                    record = vdf.records[anchor.record_index]
                    self.assertEqual(
                        sec1_data_offset + 4 * axis_ref,
                        record.file_offset + 9 * 4,
                        expected_name,
                    )

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
        key_to_name_idx = vdf_xray.build_record_name_key_to_name_index(run10)

        self.assertEqual([(block.start, block.end, block.hidden) for block in blocks], [
            (1, 2, True),
            (2, 4, False),
            (4, 5, False),
            (6, 8, False),
            (11, 12, False),
        ])

        hidden_block = blocks[0]
        hidden_record = run10.records[hidden_block.sentinel_record_indices[0]]
        hidden_name_idx = key_to_name_idx[hidden_record.fields[2]]
        self.assertEqual(run10.names[hidden_name_idx], "#v>SMOOTH#")
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
        self.assertEqual(vdf_xray.u32(risk.data, 0x74), 225)
        self.assertEqual(vdf_xray.u32(risk.data, 0x7C), 225)
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

    def test_risk2_header_splits_saved_suffix_from_full_block_grid(self) -> None:
        risk2 = parse_fixture("test/bobby/vdf/econ/risk2.vdf")

        self.assertEqual(risk2.time_point_count, 213)
        self.assertEqual(vdf_xray.u32(risk2.data, 0x74), 225)
        self.assertEqual(vdf_xray.u32(risk2.data, 0x7C), 225)
        self.assertEqual(risk2.bitmap_size, 27)
        self.assertEqual(risk2.block_time_point_count, 225)
        self.assertEqual(risk2.block_bitmap_size, 29)

    def test_risk_sparse_blocks_choose_bitmap_width_per_block(self) -> None:
        for relpath, stock_ot, full_grid_ot, expected_first in [
            ("test/bobby/vdf/econ/risk.vdf", 10, 42, 70.0),
            ("test/bobby/vdf/econ/risk2.vdf", 10, 43, 75.0),
        ]:
            with self.subTest(relpath=relpath):
                vdf = parse_fixture(relpath)
                time_values = vdf.extract_time_values()
                codes = vdf.section6_ot_class_codes()
                final_values = vdf.section6_final_values()

                self.assertIsNotNone(time_values)
                self.assertIsNotNone(codes)
                self.assertIsNotNone(final_values)
                assert time_values is not None
                assert codes is not None
                assert final_values is not None

                stock_raw = vdf.offset_table_entry(stock_ot)
                full_grid_raw = vdf.offset_table_entry(full_grid_ot)
                self.assertIsNotNone(stock_raw)
                self.assertIsNotNone(full_grid_raw)
                assert stock_raw is not None
                assert full_grid_raw is not None

                stock_count = vdf_xray.u16(vdf.data, stock_raw)
                full_grid_count = vdf_xray.u16(vdf.data, full_grid_raw)
                self.assertEqual(
                    vdf._block_bitmap_layout(stock_raw, stock_count),
                    (vdf.bitmap_size, vdf.time_point_count),
                )
                self.assertEqual(
                    vdf._block_bitmap_layout(full_grid_raw, full_grid_count),
                    (vdf.block_bitmap_size, vdf.block_time_point_count),
                )

                stock_series = vdf.extract_ot_series(stock_ot, time_values, codes, final_values)
                full_grid_series = vdf.extract_ot_series(full_grid_ot, time_values, codes, final_values)
                self.assertIsNotNone(stock_series)
                self.assertIsNotNone(full_grid_series)
                assert stock_series is not None
                assert full_grid_series is not None
                self.assertAlmostEqual(stock_series[0], expected_first)
                self.assertAlmostEqual(stock_series[-1], final_values[stock_ot])
                self.assertAlmostEqual(full_grid_series[-1], final_values[full_grid_ot])

                for ot_idx in range(vdf.offset_table_count):
                    raw = vdf.offset_table_entry(ot_idx)
                    if raw is None or not vdf.is_data_block_offset(raw):
                        continue
                    series = vdf.extract_ot_series(ot_idx, time_values, codes, final_values)
                    self.assertIsNotNone(series, ot_idx)
                    assert series is not None
                    self.assertAlmostEqual(series[-1], final_values[ot_idx], places=5)

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

    def test_ref_section6_post_ref_records_are_16_byte_ot_width_records(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        records = ref.parse_section6_post_ref_records()

        self.assertIsNotNone(records)
        assert records is not None
        self.assertEqual(len(records), 226)
        self.assertEqual(records[0].file_offset, 0x2020C)
        self.assertEqual(records[0].words, [0x05EA9ED0, 1817, 7, 0])
        self.assertEqual(ref.section6_class_code_start(), 0x2102C)

        codes = ref.section6_ot_class_codes()
        self.assertIsNotNone(codes)
        assert codes is not None
        self.assertTrue(all(0 <= record.maybe_ot_index() < ref.offset_table_count for record in records))
        self.assertEqual(
            {codes[record.maybe_ot_index()] for record in records},
            {0x11, 0x16},
        )
        self.assertEqual(
            {record.maybe_block_width() for record in records},
            {1, 3, 7},
        )

    def test_ref_section6_post_ref_records_form_lookup_rooted_linked_lists(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        records = ref.parse_section6_post_ref_records()
        chains = ref.parse_section6_post_ref_chains()

        self.assertIsNotNone(records)
        self.assertIsNotNone(chains)
        assert records is not None
        assert chains is not None
        self.assertEqual(len(chains), 72)
        self.assertEqual(sum(len(chain.records) for chain in chains), len(records))

        length_counts: dict[int, int] = {}
        for chain in chains:
            length_counts[len(chain.records)] = length_counts.get(len(chain.records), 0) + 1
        self.assertEqual(length_counts, {1: 30, 2: 7, 3: 28, 14: 7})

        record_refs = {
            ref.section6_offset_to_word_ref(record.file_offset)
            for record in records
        }
        chain_refs = {
            ref.section6_offset_to_word_ref(record.file_offset)
            for chain in chains
            for record in chain.records
        }
        self.assertEqual(chain_refs, record_refs)
        self.assertEqual(min(record_refs), 3657)
        self.assertEqual(max(record_refs) + 4, ref.section6_offset_to_word_ref(ref.section6_class_code_start()))

        first = chains[0]
        self.assertEqual(first.lookup_record_index, 9)
        self.assertEqual(first.root_ref_word, 3661)
        self.assertEqual([record.maybe_ot_index() for record in first.records], [3060, 1817])

    def test_record_field11_is_lookup_record_index_for_graphical_descriptors(self) -> None:
        lookup = parse_fixture("test/bobby/vdf/lookups/lookup_ex.vdf")
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        def record_by_name(vdf: vdf_xray.VdfFile, name: str) -> vdf_xray.VdfRecord:
            key_to_name = vdf_xray.build_record_name_key_to_name_index(vdf)
            for record in vdf.records:
                name_idx = key_to_name.get(record.fields[2])
                if name_idx is not None and vdf.names[name_idx] == name:
                    return record
            raise AssertionError(f"missing record for {name}")

        lookup_records = lookup.section6_lookup_records()
        self.assertIsNotNone(lookup_records)
        assert lookup_records is not None
        table_record = record_by_name(lookup, "lookup table 1")
        stock_record = record_by_name(lookup, "stock")
        self.assertEqual(table_record.fields[11], 1)
        self.assertEqual(stock_record.fields[11], 1)
        self.assertEqual(lookup_records[table_record.fields[11]].ot_index(), 5)

        ref_lookup_records = ref.section6_lookup_records()
        self.assertIsNotNone(ref_lookup_records)
        assert ref_lookup_records is not None
        rs_n2o = record_by_name(ref, "RS N2O")
        self.assertEqual(rs_n2o.fields[11], 113)
        self.assertEqual(ref_lookup_records[rs_n2o.fields[11]].ot_index(), 2278)

    def test_record_display_keeps_field11_as_unresolved_union(self) -> None:
        lookup = parse_fixture("test/bobby/vdf/lookups/lookup_ex.vdf")

        out = StringIO()
        with redirect_stdout(out):
            vdf_xray.print_records(lookup)
        text = out.getvalue()

        self.assertIn("f[11]=raw owner/lookup union", text)
        self.assertNotIn("f[11]=ot_idx", text)
        self.assertNotIn("model sort=", text)
        self.assertIn("owner?=1", text)
        self.assertIn("lookup?=1", text)

    def test_field11_union_facts_expose_owner_and_lookup_interpretations(self) -> None:
        lookup = parse_fixture("test/bobby/vdf/lookups/lookup_ex.vdf")
        facts = {
            fact.name: fact
            for fact in vdf_xray.decoded_field11_union_facts(lookup)
        }

        table = facts["lookup table 1"]
        stock = facts["stock"]
        self.assertEqual((table.owner_start, table.owner_end), (1, 2))
        self.assertEqual((stock.owner_start, stock.owner_end), (1, 2))
        self.assertEqual(table.lookup_index, 1)
        self.assertEqual(stock.lookup_index, 1)
        self.assertEqual(table.lookup_ot_index, 5)
        self.assertEqual(stock.lookup_ot_index, 5)
        self.assertEqual(table.lookup_width, 1)
        self.assertEqual(stock.lookup_width, 1)
        self.assertTrue(table.lookup_width_matches_shape)
        self.assertTrue(stock.lookup_width_matches_shape)

    def test_ref_field11_union_facts_pin_lookup_descriptor_payload(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")
        facts = {
            fact.name: fact
            for fact in vdf_xray.decoded_field11_union_facts(ref)
        }

        rs_n2o = facts["RS N2O"]
        self.assertEqual((rs_n2o.owner_start, rs_n2o.owner_end), (113, 120))
        self.assertEqual(rs_n2o.lookup_index, 113)
        self.assertEqual(rs_n2o.lookup_ot_index, 2278)
        self.assertEqual(rs_n2o.lookup_width, 7)
        self.assertEqual(rs_n2o.lookup_dependency_ref_word, 4229)
        self.assertTrue(rs_n2o.lookup_width_matches_shape)

        c_af = facts["C AF Sequestered"]
        self.assertEqual((c_af.owner_start, c_af.owner_end), (113, 116))
        self.assertEqual(c_af.lookup_index, 113)
        self.assertEqual(c_af.lookup_ot_index, 2278)
        self.assertEqual(c_af.lookup_width, 7)
        self.assertFalse(c_af.lookup_width_matches_shape)

    def test_field11_union_facts_treat_zero_as_valid_lookup_index(self) -> None:
        mark2 = parse_fixture("test/bobby/vdf/econ/mark2.vdf")
        facts = {
            fact.name: fact
            for fact in vdf_xray.decoded_field11_union_facts(mark2)
        }

        federal = facts["federal funds rate lookup"]
        self.assertIsNone(federal.owner_start)
        self.assertEqual(federal.raw_field11, 0)
        self.assertEqual(federal.lookup_index, 0)
        self.assertEqual(federal.lookup_ot_index, 39)
        self.assertEqual(federal.lookup_width, 1)
        self.assertTrue(federal.lookup_width_matches_shape)

    def test_field11_union_correlations_link_records_to_lookup_outputs(self) -> None:
        lookup = parse_fixture("test/bobby/vdf/lookups/lookup_ex.vdf")
        rows = {
            row.fact.name: row
            for row in vdf_xray.decoded_field11_union_correlations(lookup)
        }

        table = rows["lookup table 1"]
        stock = rows["stock"]
        self.assertEqual(table.closest_output_span.name, "net change")
        self.assertEqual(stock.closest_output_span.name, "net change")
        self.assertEqual(table.output_sort_delta, 2)
        self.assertEqual(stock.output_sort_delta, 13)
        self.assertEqual(table.overlap_component_id, stock.overlap_component_id)
        self.assertEqual(
            {span.name for span in table.overlap_component_spans},
            {"lookup table 1", "stock"},
        )

    def test_field11_union_correlations_surface_ref_sort_proximity_counterexample(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")
        rows = {
            row.fact.name: row
            for row in vdf_xray.decoded_field11_union_correlations(ref)
        }

        solar = rows["Solar and albedo forcings"]
        humus = rows["C in Humus"]
        self.assertEqual(solar.fact.lookup_index, humus.fact.lookup_index)
        self.assertEqual(solar.closest_output_span.name, "Adjusted Other Forcings")
        self.assertEqual(humus.closest_output_span.name, "Adjusted Other Forcings")
        self.assertEqual(solar.output_sort_delta, 2778)
        self.assertEqual(humus.output_sort_delta, 224)
        self.assertEqual(solar.overlap_component_id, humus.overlap_component_id)

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

    def test_precision_report_marks_easy_array_file_exact_by_xray(self) -> None:
        subscripts = parse_fixture("test/bobby/vdf/subscripts/subscripts.vdf")

        report = vdf_xray.precision_report(subscripts)

        self.assertEqual(report.status, "exact-by-xray")
        self.assertEqual(report.reasons, [])
        self.assertGreater(report.array_result_count, 0)
        self.assertEqual(report.duplicate_result_name_count, 0)
        self.assertEqual(report.duplicate_result_ot_count, 0)
        self.assertEqual(report.numeric_array_label_count, 0)
        self.assertEqual(report.record_span_overlap_slots, 0)
        self.assertEqual(report.unmapped_block_count, 0)
        self.assertEqual(report.data_block_tail_mismatches, 0)

    def test_precision_report_surfaces_not_proven_blockers(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        report = vdf_xray.precision_report(ref)

        self.assertEqual(report.status, "not-proven")
        # Ref.vdf is the corpus's one fixture whose graphical-function
        # descriptor names (e.g. `RS N2O`, `Solar and albedo forcings`) do not
        # carry the lexical "lookup"/"table" keyword, so descriptor
        # identification falls through to the highest-`f[10]` heuristic. The
        # heuristic resolves Ref's overlaps cleanly (no remaining
        # owner-vs-owner span conflict), but its use is the documented
        # `not-proven` blocker (the file genuinely does not store the
        # discriminator -- see vdf.md appendix).
        self.assertIn("used-descriptor-f10-fallback", report.reasons)
        self.assertNotIn("record-span-overlap", report.reasons)
        self.assertEqual(report.record_span_overlap_slots, 0)
        # Dimension labels are decoded through section-3 axis refs plus the
        # alternate scenario element records.
        self.assertNotIn("incomplete-dimension-anchors", report.reasons)
        self.assertNotIn("numeric-array-labels", report.reasons)
        self.assertEqual(report.unmapped_block_count, 0)
        self.assertEqual(report.numeric_array_label_count, 0)
        self.assertEqual(report.duplicate_result_name_count, 0)
        self.assertEqual(report.duplicate_result_ot_count, 0)
        self.assertEqual(report.data_block_tail_mismatches, 0)

    def test_precision_report_tracks_mixed_bitmap_width_without_data_mismatch(self) -> None:
        risk2 = parse_fixture("test/bobby/vdf/econ/risk2.vdf")

        report = vdf_xray.precision_report(risk2)

        # `risk2.vdf` was previously `not-proven` due to the
        # field[11] union; after the direct-record-map promotion the
        # lexical lookup-def-name test cleanly identifies its descriptor
        # (`loan standards impact on insolvency table`), so the residual
        # owner partition is clean and the file is `exact-by-xray`. The
        # mixed-bitmap-width property is independent and still pinned here.
        self.assertEqual(report.status, "exact-by-xray")
        self.assertNotIn("record-span-overlap", report.reasons)
        self.assertEqual(report.unmapped_block_count, 0)
        self.assertEqual(report.bitmap_widths, [27, 29])
        self.assertEqual(report.data_block_decode_failures, 0)
        self.assertEqual(report.data_block_tail_mismatches, 0)

    def test_corpus_precision_rows_cover_tracked_result_and_dataset_vdfs(self) -> None:
        rows = vdf_xray.corpus_precision_rows(REPO_ROOT)
        by_path = {row.path: row for row in rows}

        self.assertEqual(len(rows), 41)
        self.assertEqual(
            by_path["test/bobby/vdf/econ/data.vdf"].status,
            "dataset/not-implemented",
        )
        self.assertEqual(
            by_path["test/bobby/vdf/subscripts/subscripts.vdf"].status,
            "exact-by-xray",
        )
        self.assertEqual(
            by_path["test/xmutil_test_models/Ref.vdf"].status,
            "not-proven",
        )

        status_counts: dict[str, int] = {}
        reason_counts: dict[str, int] = {}
        for row in rows:
            status_counts[row.status] = status_counts.get(row.status, 0) + 1
            for reason in row.reasons:
                reason_counts[reason] = reason_counts.get(reason, 0) + 1
        # After the direct-record-map extraction promotion, the lookup-vs-helper
        # field[11] union is resolved on every fixture except `Ref.vdf` (whose
        # descriptor names are abbreviations -- the f[10]-highest fallback fires
        # there). 31 (originally exact-by-xray) + 8 lookup-vs-helper fixtures
        # (lookup_ex, econ ×5, WRLD3 ×2) become exact-by-xray; only Ref.vdf
        # remains `not-proven`, with its f[10]-fallback reason.
        self.assertEqual(status_counts["exact-by-xray"], 39)
        self.assertEqual(status_counts["not-proven"], 1)
        self.assertEqual(status_counts["dataset/not-implemented"], 1)
        self.assertEqual(reason_counts.get("record-span-overlap", 0), 0)
        self.assertEqual(reason_counts.get("used-descriptor-f10-fallback", 0), 1)

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

    def test_record_field8_exposes_incomplete_dimension_anchors(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        anchors = {
            anchor.name: anchor
            for anchor in vdf_xray.decoded_record_dimension_anchors(ref)
        }

        self.assertEqual(anchors["COP"].status, "complete")
        self.assertEqual(anchors["COP"].dimension_id, 3)
        self.assertEqual([name for _, _, name in anchors["COP"].elements], [
            "OECD US",
            "OECD EU",
            "G77 China",
            "G77 India",
            "Remaining Developed",
            "Remaining Developing A",
            "COP Developing B",
        ])

        # These are real dimension/subrange anchors, but the VDF records do
        # not provide their element catalogs through the decoded field[8]
        # element-record shape. They must stay visible as facts without being
        # promoted into labels.
        self.assertEqual(anchors["COP Developed"].status, "no-elements")
        self.assertEqual(anchors["COP Developed"].dimension_id, 4)
        self.assertEqual(anchors["lower"].status, "no-elements")
        self.assertEqual(anchors["upper"].dimension_id, 22)

        # `scenario` mixes the ordinary field[8]/field[11] element layout for
        # Deterministic with a compact late-record layout for the other two
        # elements. The merged catalog is complete.
        self.assertEqual(anchors["scenario"].status, "complete")
        self.assertEqual(anchors["scenario"].dimension_id, 13)
        self.assertEqual(
            [name for _, _, name in anchors["scenario"].elements],
            ["Deterministic", "Low 2xCO2 sensitivity", "High 2xCO2 sensitivity"],
        )

        # Subrange dims whose element records are missing are nonetheless
        # recovered through the sec5-payload-subsequence rule: their payload
        # is an in-order subseq of a root dim's payload, so the subseq
        # positions yield element indices into the root's element list.
        recovered = {
            dim.name: dim.elements
            for dim in vdf_xray._recover_dimension_sets(ref)
        }
        self.assertIn("COP", recovered)
        self.assertEqual(
            recovered["COP Developed"],
            ["OECD US", "OECD EU", "Remaining Developed"],
        )
        self.assertEqual(
            recovered["scenario"],
            ["Deterministic", "Low 2xCO2 sensitivity", "High 2xCO2 sensitivity"],
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

    def test_ref_owner_mapping_is_non_overlapping_but_not_owner_proven(self) -> None:
        # Several Ref graphical-function descriptor records carry owner-looking
        # fields and overlap real saved variables. The current xray mapping is
        # a non-overlap diagnostic partition, not a decoded owner/descriptor
        # rule; pin both the no-duplicate invariant and a known wrong-side
        # selection so callers keep treating Ref as not-proven.
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
        self.assertNotIn("CH4 in Atm", mapping.name_to_block)
        self.assertEqual(mapping.name_to_block["Specified Global HFC134a eq"].start, 140)
        self.assertEqual(mapping.name_to_block["Specified Global N2O"].start, 141)
        self.assertEqual(mapping.name_to_block["Specified Global PFC"].start, 142)

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

    def test_ref_extraction_labels_same_cardinality_axes_from_section3_refs(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        results = vdf_xray.extract_named_results(ref)

        self.assertIsNotNone(results)
        assert results is not None
        names = {result.name for result in results}
        for expected in [
            "C AF Sequestered[Deterministic]",
            "C AF Sequestered[Low 2xCO2 sensitivity]",
            "C AF Sequestered[High 2xCO2 sensitivity]",
            "C in Deep Ocean[Deterministic,layer1]",
            "C in Deep Ocean[High 2xCO2 sensitivity,layer4]",
            "Intensity RS target[OECD US,t1]",
            "Intensity RS target[COP Developing B,t3]",
            "Aggregated Definition[OECD US,Developed Countries]",
            "Aggregated Definition[COP Developing B,Developing B Countries]",
        ]:
            self.assertIn(expected, names)

        self.assertFalse(
            any(vdf_xray.NUMERIC_ARRAY_LABEL_RE.search(name) for name in names)
        )

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

    def test_record_key_mapping_keeps_runtime_signature_names(self) -> None:
        # Direct f[2] keys can point at saved runtime helper signatures. These
        # are structural time-series owners, not display metadata to filter out.
        base = parse_fixture("test/bobby/vdf/econ/base.vdf")

        mapping = vdf_xray.map_names_to_owner_blocks(base)

        self.assertIsNotNone(mapping)
        assert mapping is not None
        self.assertEqual(mapping.unmapped_blocks, [])
        self.assertIn(
            "#LV1<DELAY1(insolvencyrisk,averagetimebeforedefault)#",
            mapping.name_to_block,
        )
        self.assertEqual(
            mapping.name_to_block[
                "#LV1<DELAY1(insolvencyrisk,averagetimebeforedefault)#"
            ].start,
            1,
        )

    def test_sec5_entries_align_with_anchor_f8_ascending(self) -> None:
        # The breakthrough: sorting record-field[8] dimension anchors by
        # f[8] ascending produces a sequence whose cardinalities match
        # section-5 entries in file order. Validated here across every
        # fixture known to carry section-5 dimension metadata.
        cases = [
            "test/xmutil_test_models/Ref.vdf",
            "test/bobby/vdf/subscripts/subscripts.vdf",
            "test/bobby/vdf/model_editing/run_7.vdf",
            "test/bobby/vdf/model_editing/run_8.vdf",
            "test/bobby/vdf/model_editing/run_9.vdf",
            "test/bobby/vdf/model_editing/run_10.vdf",
        ]
        for relpath in cases:
            with self.subTest(relpath=relpath):
                vdf = parse_fixture(relpath)
                pairings = vdf_xray.sec5_anchor_binding(vdf)
                anchors = vdf_xray.decoded_record_dimension_anchors(vdf)
                sec5 = vdf.parse_section5_sets() or []

                # Counts must align before pairing is meaningful.
                self.assertEqual(len(anchors), len(sec5), relpath)
                self.assertEqual(len(pairings), len(sec5), relpath)

                # After sorting anchors by group_id asc, cardinalities
                # (sec5.n) must match the element-record cardinality for
                # every anchor whose element catalog is complete.
                sorted_anchors = sorted(anchors, key=lambda a: a.group_id)
                for rank, (entry, anchor) in enumerate(zip(sec5, sorted_anchors)):
                    if anchor.status == "complete":
                        self.assertEqual(
                            entry.n, len(anchor.elements),
                            f"{relpath}: sec5[{rank}].n mismatch for {anchor.name!r}",
                        )
                # The pairings tuple reports the same sort positions.
                for rank, (anchor, entry, rank_in_tuple) in enumerate(pairings):
                    self.assertEqual(rank, rank_in_tuple, relpath)
                    # Compare by identity-safe attributes: anchor identity is
                    # group_id, sec5 entry identity is file offset.
                    self.assertEqual(
                        anchor.group_id, sorted_anchors[rank].group_id, relpath,
                    )
                    self.assertEqual(entry.file_offset, sec5[rank].file_offset, relpath)

    def test_subrange_payload_is_parent_subseq_on_ref_vdf(self) -> None:
        # Every Ref.vdf subrange dim's section-5 payload is an in-order
        # subseq of its parent root dim's payload, and the subseq positions
        # are the MDL element indices. Locking these in pins the subrange
        # decoder against future accidental regressions.
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")
        pairings = vdf_xray.sec5_anchor_binding(ref)
        payloads_by_name = {
            anchor.name: tuple(vdf_xray.section5_payload_refs(entry))
            for anchor, entry, _ in pairings
        }

        expected = {
            # (subrange name, parent root, expected subseq positions)
            "bottom": ("layers", [3]),
            "lower": ("layers", [1, 2, 3]),
            "upper": ("layers", [0, 1, 2]),
            "COP Developed": ("COP", [0, 1, 4]),
            "COP Developing A": ("COP", [2, 3, 5]),
            "COP Remaining Developing": ("COP", [5, 6]),
            "Developing A": ("Semi Agg", [2, 3]),
            "Developing B": ("Semi Agg", [5]),
            "set targets": ("Target", [0, 1]),
            "tNext": ("Target", [1, 2]),
            "tPrev": ("Target", [0, 1]),
        }

        for subrange, (parent, expected_positions) in expected.items():
            with self.subTest(subrange=subrange):
                sub_payload = payloads_by_name[subrange]
                parent_payload = payloads_by_name[parent]
                positions = vdf_xray._subsequence_positions(sub_payload, parent_payload)
                self.assertIsNotNone(positions, subrange)
                assert positions is not None
                self.assertEqual(positions, expected_positions, subrange)

    def test_dimension_element_recovery_closes_ref_vdf_numeric_array_labels(self) -> None:
        # Ref.vdf now has decoded labels for every dimension used by saved
        # arrays: roots come from element records, scenario adds its compact
        # late-record elements, and subranges come from parent-root sec5
        # subsequence projection. The `incomplete-dimension-anchors` and
        # `numeric-array-labels` blockers therefore no longer fire.
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        recovered = {
            dim.name: dim.elements
            for dim in vdf_xray._recover_dimension_sets(ref)
        }
        # All fully-recoverable dims: 7 roots plus 11 subranges.
        for expected_name in [
            "Aggregated Regions",
            "COP", "COP Developed", "COP Developing A", "COP Remaining Developing",
            "Developing A", "Developing B",
            "HFC type",
            "layers", "bottom", "lower", "upper",
            "scenario", "Semi Agg",
            "Target", "set targets", "tNext", "tPrev",
        ]:
            self.assertIn(expected_name, recovered, expected_name)

        self.assertEqual(recovered["bottom"], ["layer4"])
        self.assertEqual(recovered["lower"], ["layer2", "layer3", "layer4"])
        self.assertEqual(recovered["upper"], ["layer1", "layer2", "layer3"])
        self.assertEqual(
            recovered["COP Developed"],
            ["OECD US", "OECD EU", "Remaining Developed"],
        )
        self.assertEqual(
            recovered["COP Developing A"],
            ["G77 China", "G77 India", "Remaining Developing A"],
        )
        self.assertEqual(
            recovered["COP Remaining Developing"],
            ["Remaining Developing A", "COP Developing B"],
        )
        self.assertEqual(recovered["Developing A"], ["China", "India"])
        self.assertEqual(recovered["Developing B"], ["Other Developing"])
        self.assertEqual(
            recovered["scenario"],
            ["Deterministic", "Low 2xCO2 sensitivity", "High 2xCO2 sensitivity"],
        )
        self.assertEqual(recovered["set targets"], ["t1", "t2"])
        self.assertEqual(recovered["tNext"], ["t2", "t3"])
        self.assertEqual(recovered["tPrev"], ["t1", "t2"])

        # And finally: precision_report must drop the blocker.
        report = vdf_xray.precision_report(ref)
        self.assertNotIn("incomplete-dimension-anchors", report.reasons)
        self.assertNotIn("numeric-array-labels", report.reasons)


class CorpusDecodedRecordSpanCoverageTests(unittest.TestCase):
    """
    Pin the corpus-wide property that motivates the direct-record-map
    extraction path: on every `exact-by-xray` fixture, `decoded_record_spans`
    (with the class-code guard) covers OT[1..N) exactly once and produces zero
    overlapping span-claims. The 9 `not-proven` fixtures all share one
    failure mode -- record-span overlap from the field[11] owner/descriptor
    union (see docs/design/vdf.md appendix).
    """

    EXACT_BY_XRAY = [
        "test/bobby/vdf/bact/Current.vdf",
        "test/bobby/vdf/bact/euler-1.vdf",
        "test/bobby/vdf/bact/euler-10.vdf",
        "test/bobby/vdf/bact/euler-2.vdf",
        "test/bobby/vdf/bact/euler-5.vdf",
        "test/bobby/vdf/bact/euler.vdf",
        "test/bobby/vdf/bact/rk4.vdf",
        "test/bobby/vdf/bact/rk4auto-1.vdf",
        "test/bobby/vdf/consts/b_is_3.vdf",
        "test/bobby/vdf/consts/b_is_4.vdf",
        "test/bobby/vdf/econ/risk.vdf",
        "test/bobby/vdf/level_vs_aux/x_is_aux.vdf",
        "test/bobby/vdf/level_vs_aux/x_is_stock.vdf",
        "test/bobby/vdf/model_editing/run_1.vdf",
        "test/bobby/vdf/model_editing/run_10.vdf",
        "test/bobby/vdf/model_editing/run_2.vdf",
        "test/bobby/vdf/model_editing/run_3.vdf",
        "test/bobby/vdf/model_editing/run_4.vdf",
        "test/bobby/vdf/model_editing/run_5.vdf",
        "test/bobby/vdf/model_editing/run_6.vdf",
        "test/bobby/vdf/model_editing/run_7.vdf",
        "test/bobby/vdf/model_editing/run_8.vdf",
        "test/bobby/vdf/model_editing/run_9.vdf",
        "test/bobby/vdf/pop/Current.vdf",
        "test/bobby/vdf/pop/pop.vdf",
        "test/bobby/vdf/sd202_a2/Current.vdf",
        "test/bobby/vdf/subscripts/subscripts.vdf",
        "test/bobby/vdf/water/Current.vdf",
        "test/bobby/vdf/water/base.vdf",
        "test/bobby/vdf/water/limited.vdf",
        "test/bobby/vdf/water/water.vdf",
    ]

    # Each fixture here exhibits raw descriptor/owner span overlap in
    # `decoded_record_spans` (before `identify_descriptor_records` runs).
    # The pair-equivalent count is a structural fingerprint of the file
    # and changes only when the format itself changes; descriptor
    # identification then resolves the conflict via the decoded forward
    # link, so these fixtures still classify as `exact-by-xray` in the
    # corpus precision report.
    DESCRIPTOR_CONFLICT_PAIRS = {
        "test/bobby/vdf/lookups/lookup_ex.vdf": 1,
        "test/bobby/vdf/econ/base.vdf": 3,
        "test/bobby/vdf/econ/mark2.vdf": 3,
        "test/bobby/vdf/econ/policy.vdf": 3,
        "test/bobby/vdf/econ/risk2.vdf": 1,
        "test/bobby/vdf/econ/rk.vdf": 3,
        "test/metasd/WRLD3-03/SCEN01.VDF": 54,
        "test/metasd/WRLD3-03/experiment.vdf": 54,
    }

    def test_decoded_record_spans_partition_ot_on_every_exact_by_xray_fixture(self) -> None:
        for relpath in self.EXACT_BY_XRAY:
            with self.subTest(fixture=relpath):
                vdf = parse_fixture(relpath)
                spans = vdf_xray.decoded_record_spans(vdf)
                covered: set[int] = set()
                overlap_slots = 0
                for span in spans:
                    for ot in range(span.start, span.end):
                        if ot in covered:
                            overlap_slots += 1
                        covered.add(ot)
                expected = set(range(1, vdf.offset_table_count))
                self.assertEqual(
                    overlap_slots, 0,
                    f"{relpath}: expected zero overlapping span-claims, got {overlap_slots}",
                )
                self.assertEqual(
                    covered, expected,
                    f"{relpath}: spans should cover OT[1..{vdf.offset_table_count}) exactly; "
                    f"missing={sorted(expected - covered)} extra={sorted(covered - expected)}",
                )

    def test_decoded_record_spans_overlap_count_pinned_on_descriptor_conflict_fixtures(self) -> None:
        """
        Pin the raw descriptor/owner overlap count surfaced by
        `decoded_record_spans` *before* `identify_descriptor_records`
        runs. The count is the field[11] owner/descriptor union expressed
        as slot-level pair-equivalents and is a structural fingerprint of
        the file. These fixtures still classify as `exact-by-xray` in the
        precision report -- descriptor identification cleans the spans up
        -- but the pre-resolution overlap shape must stay stable.
        """
        for relpath, expected_pairs in self.DESCRIPTOR_CONFLICT_PAIRS.items():
            with self.subTest(fixture=relpath):
                vdf = parse_fixture(relpath)
                spans = vdf_xray.decoded_record_spans(vdf)
                slot_to_records: dict[int, list[int]] = {}
                for span in spans:
                    for ot in range(span.start, span.end):
                        slot_to_records.setdefault(ot, []).append(span.rec_idx)
                # Count overlap PAIRS at the slot level (each slot with k>=2 spans
                # contributes k-1 pair-equivalents; on the documented fixtures every
                # overlap is a clean pair so this == the conflict-pair count).
                pair_equiv = sum(len(recs) - 1 for recs in slot_to_records.values() if len(recs) >= 2)
                # Coalesce contiguous overlap-pair-equivalents to per-conflict-pair count:
                # all documented overlaps are (descriptor, owner) where the overlap is
                # exactly one shared OT slot per pair (lookup_ex/econ) or a width-N
                # descriptor over a width-1 or width-N owner (Ref/WRLD3 collapse to N).
                # Use the slot-level pair-equivalent count as the pinning quantity --
                # it is what `record-span-overlap` measures.
                self.assertEqual(
                    pair_equiv, expected_pairs,
                    f"{relpath}: expected {expected_pairs} slot-level overlap "
                    f"pair-equivalents, got {pair_equiv}",
                )


class DecodedRecordSpanClassCodeGuardTests(unittest.TestCase):
    """The class-code guard added to `decoded_record_spans` rejects records
    whose f[11]-as-OT span lands on any non-real-data class code (anything
    outside {0x08, 0x11, 0x16, 0x17, 0x18}). On the current corpus this is a
    no-op for the 31 exact-by-xray fixtures (their owner-record f[11]s all
    point to real-data slots). The test pins that no `exact-by-xray` fixture
    loses any spans to the guard."""

    def test_class_code_guard_does_not_drop_spans_on_clean_fixtures(self) -> None:
        # Re-do `decoded_record_spans`'s logic without the class-code check,
        # and assert the count matches.
        for relpath in CorpusDecodedRecordSpanCoverageTests.EXACT_BY_XRAY:
            with self.subTest(fixture=relpath):
                vdf = parse_fixture(relpath)
                guarded = vdf_xray.decoded_record_spans(vdf)

                # Replicate the unguarded path inline.
                key_to_name_idx = vdf_xray.build_record_name_key_to_name_index(vdf)
                unguarded_count = 0
                for rec in vdf.records:
                    if key_to_name_idx.get(rec.fields[2]) is None:
                        continue
                    start = rec.ot_index()
                    if start <= 0 or start >= vdf.offset_table_count:
                        continue
                    length = vdf_xray.decoded_record_shape_length(vdf, rec)
                    if length is None or length <= 0:
                        continue
                    if start + length > vdf.offset_table_count:
                        continue
                    unguarded_count += 1

                self.assertEqual(
                    len(guarded), unguarded_count,
                    f"{relpath}: class-code guard unexpectedly dropped spans "
                    f"({unguarded_count} -> {len(guarded)})",
                )


if __name__ == "__main__":
    unittest.main()
