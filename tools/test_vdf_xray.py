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


class VdfXrayModelEditingTests(unittest.TestCase):

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
        # Residuals are measured against the deterministic slot-table start
        # (sec1 field1), so files where the old backward-scan heuristic
        # swallowed 1-2 leading words into its table now show those words in
        # the trailer instead.
        cases = {
            "test/bobby/vdf/model_editing/run_8.vdf": (8, [32, 131]),
            "test/bobby/vdf/econ/base.vdf": (8, [13352, 8]),
            "test/xmutil_test_models/Ref.vdf": (24, [124, 0, 0, 0, 12320, 26]),
            "test/metasd/WRLD3-03/experiment.vdf": (8, [12328, 8]),
            "test/metasd/WRLD3-03/SCEN01.VDF": (8, [12328, 8]),
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

    def test_slot_table_deterministic_decode_matches_header(self) -> None:
        # The slot table is decoded deterministically from the section-1
        # header: it starts at the field1 1-based word pointer, has block1[7]
        # entries, and is followed by a single 0x00430000 terminator word
        # before the name table. The deleted backward-scan heuristic
        # under-counted on edited files and over-counted by one elsewhere;
        # these fixtures are the ones it used to mishandle (mirrors the Rust
        # test_slot_table_deterministic_matches_header).
        for relpath in [
            "test/bobby/vdf/econ/risk2.vdf",
            "test/metasd/WRLD3-03/SCEN01.VDF",
            "test/metasd/social-network-valuation/optimistic.vdf",
            "test/bobby/vdf/econ/risk.vdf",
            "test/bobby/vdf/water/Current.vdf",
        ]:
            with self.subTest(relpath=relpath):
                vdf = parse_fixture(relpath)
                sec1 = vdf.sections[1]

                block1_word7 = vdf_xray.u32(
                    vdf.data, sec1.data_offset() + vdf_xray.SLOT_COUNT_WORD_OFFSET)
                field1_start = sec1.file_offset + 4 * (sec1.field1 - 1)

                self.assertEqual(len(vdf.slot_table), block1_word7)
                self.assertEqual(vdf.slot_table_offset, field1_start)
                terminator = vdf_xray.u32(
                    vdf.data, field1_start + len(vdf.slot_table) * 4)
                self.assertEqual(terminator, vdf_xray.SLOT_TABLE_TERMINATOR)
                # The terminator word is the last word before the name table.
                name_sec = vdf.sections[vdf.name_section_idx]
                self.assertEqual(
                    field1_start + (len(vdf.slot_table) + 1) * 4,
                    name_sec.file_offset,
                )

    def test_slot_table_decode_returns_empty_on_cross_check_mismatch(self) -> None:
        # Corrupt block1[7] so the count no longer lands the terminator just
        # before the name table: the decode must bail to (0, []) rather than
        # emit a mis-decoded table.
        path = REPO_ROOT / "test/bobby/vdf/water/Current.vdf"
        data = bytearray(path.read_bytes())
        vdf = vdf_xray.parse_vdf(bytes(data))
        sec1 = vdf.sections[1]
        count_off = sec1.data_offset() + vdf_xray.SLOT_COUNT_WORD_OFFSET
        original_count = vdf_xray.u32(vdf.data, count_off)
        data[count_off:count_off + 4] = (original_count + 1).to_bytes(4, "little")

        corrupted = vdf_xray.parse_vdf(bytes(data))

        self.assertEqual(corrupted.slot_table_offset, 0)
        self.assertEqual(corrupted.slot_table, [])

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

    def test_field6_zero_records_have_no_decoded_shape(self) -> None:
        # f[6]=0 records (dimension anchors, elements, builtins, descriptors)
        # never carry a decoded shape span, even when a section-3 entry with
        # index_word=0 is active (run_6) or a placeholder (run_8).
        for relpath in [
            "test/bobby/vdf/model_editing/run_6.vdf",
            "test/bobby/vdf/model_editing/run_8.vdf",
        ]:
            with self.subTest(relpath=relpath):
                vdf = parse_fixture(relpath)
                rec = next(
                    rec for rec in vdf.records
                    if rec.ot_index() == 1 and rec.fields[10] == 11
                )

                self.assertEqual(rec.shape_code(), 0)
                self.assertIsNone(vdf_xray.decoded_record_shape_length(vdf, rec))

    def test_water_extraction_recovers_model_and_system_series(self) -> None:
        # Replaces the deleted owner-block test for water/Current.vdf at the
        # extraction level: the five visible model variables map onto their
        # record-decoded OT slots and the system records land on their own
        # slots -- system sentinel records never masquerade as model owners.
        water = parse_fixture("test/bobby/vdf/water/Current.vdf")

        results = vdf_xray.extract_named_results(water)

        self.assertIsNotNone(results)
        assert results is not None
        by_name = {result.name: result.ot_index for result in results}
        self.assertEqual(
            by_name,
            {
                "Time": 0,
                "water level": 1,
                "adjustment time": 2,
                "desired water level": 3,
                "FINAL TIME": 4,
                "gap": 5,
                "inflow": 6,
                "INITIAL TIME": 7,
                "SAVEPER": 8,
                "TIME STEP": 9,
            },
        )

    def test_run9_slot_table_keeps_stale_leading_entry_from_edit_chain(self) -> None:
        # run_9 is an edited fixture whose actively-written slot table retains
        # a stale leading helper entry (412), so the naive slot_table[i] ->
        # names[i] pairing is shifted by one on this file. That is a fact
        # about the file, not a decode error: the deterministic header decode
        # (field1 + block1[7] + terminator) pins exactly this table.
        # Extraction does not depend on the slot/name pairing.
        run9 = parse_fixture("test/bobby/vdf/model_editing/run_9.vdf")

        self.assertEqual(len(run9.slot_table), 24)
        self.assertEqual(run9.slot_table[:5], [412, 156, 124, 140, 188])

    def test_ref_direct_slot_mapping_pairs_dimension_names(self) -> None:
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        direct = vdf_xray.build_direct_slot_to_names(ref)

        self.assertEqual(direct[1724], ["Aggregated Regions"])
        self.assertEqual(direct[316], ["COP"])
        self.assertEqual(direct[16784], ["HFC type"])
        self.assertEqual(direct[5164], ["layers"])
        self.assertEqual(direct[13868], ["Semi Agg"])
        self.assertEqual(direct[1052], ["Target"])

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
                    (vdf.bitmap_size, vdf.time_point_count, "saved"),
                )
                # risk's data grid (0x74=225) coincides with its block grid
                # (0x7C=225), so the block candidate wins by order.
                self.assertEqual(
                    vdf._block_bitmap_layout(full_grid_raw, full_grid_count),
                    (vdf.block_bitmap_size, vdf.block_time_point_count, "block"),
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


    def test_run2_extraction_emits_only_time_and_system_series(self) -> None:
        # run_2 has only system and padding records: the two sentinel system
        # records (INITIAL TIME, FINAL TIME) decode to system names via f[2],
        # and the remaining records all have f[6]=0, so no model-variable
        # spans survive. Extraction must emit exactly Time plus the four
        # system variables.
        run2 = parse_fixture("test/bobby/vdf/model_editing/run_2.vdf")

        results = vdf_xray.extract_named_results(run2)

        self.assertIsNotNone(results)
        assert results is not None
        self.assertEqual(
            [result.name for result in results],
            ["Time", "FINAL TIME", "INITIAL TIME", "SAVEPER", "TIME STEP"],
        )

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
        # Broad-coverage floor: the direct record map recovers the large
        # contiguous block of visible model variables.
        self.assertGreaterEqual(len(names), 55)
        self.assertIn("perceived mortgage balance", names)

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
        span = _span(0, "Proportion of COP to global HFC134a eq", 0, 63)
        span.shape_code = 221
        labels = vdf_xray._array_element_labels_for_span(
            ref,
            span,
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

    def test_ref_dimension_anchors_are_not_extracted_as_series_owners(self) -> None:
        # Dimension-anchor records have f[6]=0, so they never yield a decoded
        # span; no dimension name may appear as an extracted series (bare or
        # element-subscripted).
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        results = vdf_xray.extract_named_results(ref)

        self.assertIsNotNone(results)
        assert results is not None
        for name in [
            "Aggregated Regions",
            "COP",
            "HFC type",
            "layers",
            "Semi Agg",
            "Target",
        ]:
            for result in results:
                self.assertFalse(
                    result.name == name or result.name.startswith(name + "["),
                    f"dimension {name!r} emitted as series {result.name!r}",
                )

    def test_ref_owner_spans_are_non_overlapping_after_descriptor_pruning(self) -> None:
        # Several Ref graphical-function descriptor records carry owner-looking
        # fields and overlap real saved variables. After descriptor
        # identification (overlap peeling + the standalone drop) the remaining
        # owner spans must partition their OT slots with no duplicates, and
        # the known descriptor names must be on the descriptor side.
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        spans = vdf_xray.decoded_record_spans(ref)
        descriptor_indices = vdf_xray.identify_descriptor_records(
            ref, spans).descriptor_indices
        owner_spans = [s for s in spans if s.rec_idx not in descriptor_indices]

        ot_to_names: dict[int, list[str]] = {}
        for span in owner_spans:
            for ot_idx in range(span.start, span.end):
                ot_to_names.setdefault(ot_idx, []).append(span.name)
        duplicates = {
            ot_idx: names
            for ot_idx, names in ot_to_names.items()
            if len(names) > 1
        }
        self.assertEqual(duplicates, {})

        owner_by_name = {span.name: span for span in owner_spans}
        self.assertEqual(owner_by_name["C in Mixed Layer"].start, 137)
        self.assertEqual(owner_by_name["C in Mixed Layer"].length(), 3)
        self.assertEqual(owner_by_name["Cum CO2 at start"].start, 146)
        self.assertEqual(owner_by_name["Cum CO2eq at start"].start, 153)
        self.assertEqual(owner_by_name["Cumulative CO2"].start, 160)

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
            self.assertNotIn(descriptor_name, owner_by_name)

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
                    vdf_xray.decoded_record_shape_length(ref, records_by_name[name]),
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

    def test_decoded_record_spans_recover_edited_array_owners(self) -> None:
        # The direct record map (f[2] name key, f[11] OT start, f[6] shape)
        # recovers the model-variable spans on the edited array fixtures.
        expected_spans = {
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
            spans = vdf_xray.decoded_record_spans(vdf)

            model_spans = {
                span.name: (span.start, span.end)
                for span in spans
                if span.name in expected_spans
            }
            self.assertEqual(model_spans, expected_spans, relpath)

    def test_extraction_recovers_single_variable_files(self) -> None:
        for relpath, expected_start in [
            ("test/bobby/vdf/level_vs_aux/x_is_stock.vdf", 1),
            ("test/bobby/vdf/level_vs_aux/x_is_aux.vdf", 5),
        ]:
            vdf = parse_fixture(relpath)
            results = vdf_xray.extract_named_results(vdf)

            self.assertIsNotNone(results, relpath)
            assert results is not None
            by_name = {result.name: result.ot_index for result in results}
            self.assertEqual(by_name["x"], expected_start, relpath)

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

    def test_extraction_keeps_runtime_signature_names(self) -> None:
        # Direct f[2] keys can point at saved runtime helper signatures. These
        # are structural time-series owners, not display metadata to filter out.
        base = parse_fixture("test/bobby/vdf/econ/base.vdf")

        results = vdf_xray.extract_named_results(base)

        self.assertIsNotNone(results)
        assert results is not None
        by_name = {result.name: result.ot_index for result in results}
        self.assertEqual(
            by_name["#LV1<DELAY1(insolvencyrisk,averagetimebeforedefault)#"],
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


def _span(rec_idx: int, name: str, start: int, length: int = 1) -> vdf_xray.DecodedRecordSpan:
    return vdf_xray.DecodedRecordSpan(
        rec_idx=rec_idx,
        name_idx=0,
        name=name,
        start=start,
        end=start + length,
        shape_code=5,
        sort_key=0,
        slot_ref=0,
        group_id=0,
        has_sentinel=True,
        ot_codes=[],
    )


class StandaloneLookupOnlyDescriptorTests(unittest.TestCase):
    """
    Port of the Rust `standalone_descriptor_tests` module
    (src/simlin-engine/src/vdf/record_results.rs): the pure standalone
    lookup-only descriptor detection over synthetic inputs.
    """

    OT_TIME = vdf_xray.OT_CODE_TIME
    OT_STOCK = vdf_xray.OT_CODE_STOCK
    OT_DYNAMIC = vdf_xray.OT_CODE_DYNAMIC

    def test_standalone_lookup_descriptor_is_dropped(self) -> None:
        # OT layout: 0=Time, 1=dynamic owner (the real GF output the
        # descriptor's forward link resolves to), 2=stock-coded GHOST slot the
        # descriptor's f[11]-as-OT-start spuriously lands on.
        class_codes = [self.OT_TIME, self.OT_DYNAMIC, self.OT_STOCK]
        lookup_word10 = [9, 1]

        # The consumer span at the forward link (OT 1) corroborates the drop;
        # it is never a candidate itself because its slot is DYNAMIC, so the
        # ghost-stock gate rejects it first (its f[11] of 0 would fail the
        # forward-link gate too).
        outcome = vdf_xray.standalone_lookup_only_descriptors(
            spans=[
                _span(0, "Some Forcing graph", 2),
                _span(1, "Some Forcing consumer", 1),
            ],
            f11_by_span=[1, 0],
            overlapping=set(),
            peeled_descriptors=set(),
            n_lookups=len(lookup_word10),
            lookup_word10=lookup_word10,
            lookup_word11=[1, 1],
            class_codes=class_codes,
            ot_count=len(class_codes),
        )

        self.assertEqual(outcome.dropped, {0})
        self.assertFalse(outcome.veto_fired)

    def test_legit_dynamic_owner_with_small_f11_is_not_dropped(self) -> None:
        # f[11] == 1 is both the owner's OT start (dynamic, holds its data)
        # AND coincidentally a valid lookup index. It must stay an owner.
        class_codes = [self.OT_TIME, self.OT_DYNAMIC]
        lookup_word10 = [9, 9]

        outcome = vdf_xray.standalone_lookup_only_descriptors(
            spans=[_span(0, "Some Concentration", 1)],
            f11_by_span=[1],
            overlapping=set(),
            peeled_descriptors=set(),
            n_lookups=len(lookup_word10),
            lookup_word10=lookup_word10,
            lookup_word11=[1, 1],
            class_codes=class_codes,
            ot_count=len(class_codes),
        )

        self.assertEqual(outcome.dropped, set())

    def test_legit_dynamic_owner_blocked_only_by_stock_slot_guard(self) -> None:
        # Every other precondition passes (non-overlapping, scalar, valid
        # lookup index, in-range owner-coded forward link); the ONLY condition
        # standing between the owner and a (wrong) drop is the STOCK-slot
        # requirement: its f[11]-as-OT-start lands on a DYNAMIC slot.
        class_codes = [self.OT_TIME, self.OT_DYNAMIC, self.OT_DYNAMIC]
        lookup_word10 = [9, 2]

        # A consumer span at the forward link (OT 2) satisfies consumer
        # corroboration, so ONLY the STOCK-slot guard can reject.
        outcome = vdf_xray.standalone_lookup_only_descriptors(
            spans=[
                _span(0, "Some Dynamic Owner", 1),
                _span(1, "Forward Consumer", 2),
            ],
            f11_by_span=[1, 0],
            overlapping=set(),
            peeled_descriptors=set(),
            n_lookups=len(lookup_word10),
            lookup_word10=lookup_word10,
            lookup_word11=[1, 1],
            class_codes=class_codes,
            ot_count=len(class_codes),
        )

        self.assertEqual(outcome.dropped, set())

    def test_standalone_descriptor_with_time_forward_link_is_not_dropped(self) -> None:
        # lookup record[1].word[10] == 0 -> Time, an invalid evaluated output.
        class_codes = [self.OT_TIME, self.OT_STOCK]
        lookup_word10 = [9, 0]

        outcome = vdf_xray.standalone_lookup_only_descriptors(
            spans=[_span(0, "Ref graph LOOKUP", 1)],
            f11_by_span=[1],
            overlapping=set(),
            peeled_descriptors=set(),
            n_lookups=len(lookup_word10),
            lookup_word10=lookup_word10,
            lookup_word11=[1, 1],
            class_codes=class_codes,
            ot_count=len(class_codes),
        )

        self.assertEqual(outcome.dropped, set())

    def test_overlapping_descriptor_is_left_to_the_component_path(self) -> None:
        class_codes = [self.OT_TIME, self.OT_STOCK]
        lookup_word10 = [9, 1]

        outcome = vdf_xray.standalone_lookup_only_descriptors(
            spans=[_span(0, "Overlapping graph", 1)],
            f11_by_span=[1],
            overlapping={0},
            peeled_descriptors=set(),
            n_lookups=len(lookup_word10),
            lookup_word10=lookup_word10,
            lookup_word11=[1, 1],
            class_codes=class_codes,
            ot_count=len(class_codes),
        )

        self.assertEqual(outcome.dropped, set())

    def test_arrayed_standalone_descriptor_is_dropped_when_width_matches(self) -> None:
        # OT layout: 0=Time, [1,4) = 3 dynamic owners (the forward block),
        # [4,7) = 3 stock GHOST slots the descriptor's f[11]-as-OT-start
        # spuriously covers. Forward width word[11] == 3 == element count.
        class_codes = [
            self.OT_TIME,
            self.OT_DYNAMIC, self.OT_DYNAMIC, self.OT_DYNAMIC,
            self.OT_STOCK, self.OT_STOCK, self.OT_STOCK,
        ]
        lookup_word10 = [9, 1]
        lookup_word11 = [0, 3]

        # The 3-wide consumer span at the forward link (OT 1) corroborates.
        outcome = vdf_xray.standalone_lookup_only_descriptors(
            spans=[
                _span(0, "RS arrayed graph", 4, 3),
                _span(1, "RS arrayed consumer", 1, 3),
            ],
            f11_by_span=[1, 0],
            overlapping=set(),
            peeled_descriptors=set(),
            n_lookups=len(lookup_word10),
            lookup_word10=lookup_word10,
            lookup_word11=lookup_word11,
            class_codes=class_codes,
            ot_count=len(class_codes),
        )

        self.assertEqual(outcome.dropped, {0})
        self.assertFalse(outcome.veto_fired)

    def test_arrayed_standalone_descriptor_with_wider_shared_consumer_is_not_dropped(self) -> None:
        # Mirrors the Ref.vdf `rs_hfc*` family: eight 7-element descriptors
        # all forward-link to one 63-wide consumer block. ONLY the width
        # mismatch (5 != 3) rejects the drop here.
        class_codes = [
            self.OT_TIME,
            self.OT_DYNAMIC, self.OT_DYNAMIC, self.OT_DYNAMIC,
            self.OT_STOCK, self.OT_STOCK, self.OT_STOCK,
        ]
        lookup_word10 = [9, 1]
        lookup_word11 = [0, 5]

        # A 3-wide span at the forward link keeps consumer corroboration
        # satisfiable; ONLY the width mismatch (5 != 3) rejects the drop.
        outcome = vdf_xray.standalone_lookup_only_descriptors(
            spans=[
                _span(0, "RS HFC arrayed graph", 4, 3),
                _span(1, "RS HFC shared consumer", 1, 3),
            ],
            f11_by_span=[1, 0],
            overlapping=set(),
            peeled_descriptors=set(),
            n_lookups=len(lookup_word10),
            lookup_word10=lookup_word10,
            lookup_word11=lookup_word11,
            class_codes=class_codes,
            ot_count=len(class_codes),
        )

        self.assertEqual(outcome.dropped, set())

    def test_plain_stock_owner_without_consumer_span_is_not_dropped(self) -> None:
        # A real stock whose own OT start is coincidentally a valid lookup
        # index passes every physical gate (it IS a stock; scalars have no
        # width gate; the unrelated lookup's forward link is owner-coded) --
        # consumer corroboration is the SOLE veto: no decoded span starts at
        # the forward OT. Mirrors SimService/Base.vdf's Agriculture
        # Employment, which the pre-gate code silently dropped.
        class_codes = [self.OT_TIME, self.OT_STOCK, self.OT_DYNAMIC]
        lookup_word10 = [9, 2]

        outcome = vdf_xray.standalone_lookup_only_descriptors(
            spans=[_span(0, "Agriculture Employment", 1)],
            f11_by_span=[1],
            overlapping=set(),
            peeled_descriptors=set(),
            n_lookups=len(lookup_word10),
            lookup_word10=lookup_word10,
            lookup_word11=[1, 1],
            class_codes=class_codes,
            ot_count=len(class_codes),
        )

        self.assertEqual(outcome.dropped, set())
        # The veto is observable, never silent.
        self.assertTrue(outcome.veto_fired)
        self.assertEqual(outcome.vetoed_candidates, 1)

    def test_one_uncorroborated_candidate_vetoes_all_standalone_drops(self) -> None:
        # Per-file coherence: two candidates pass the physical gates; one
        # corroborates (a decoded span starts at its forward link), one does
        # not -- NEITHER is dropped. Mirrors SimService/Base.vdf, where Alaska
        # Oil Discovered Reserves happened to corroborate off an unrelated
        # consumer span while its three sibling stocks did not.
        class_codes = [
            self.OT_TIME,
            self.OT_STOCK,    # OT 1: candidate 0's slot
            self.OT_STOCK,    # OT 2: candidate 1's slot
            self.OT_DYNAMIC,  # OT 3: corroborated forward link
            self.OT_DYNAMIC,  # OT 4: uncorroborated forward link
        ]
        lookup_word10 = [9, 3, 4]

        outcome = vdf_xray.standalone_lookup_only_descriptors(
            spans=[
                _span(0, "Alaska Oil Discovered Reserves", 1),
                _span(1, "Atmos UOcean Temp", 2),
                _span(2, "land allocated for ethanol", 3),
            ],
            f11_by_span=[1, 2, 0],
            overlapping=set(),
            peeled_descriptors=set(),
            n_lookups=len(lookup_word10),
            lookup_word10=lookup_word10,
            lookup_word11=[1, 1, 1],
            class_codes=class_codes,
            ot_count=len(class_codes),
        )

        self.assertEqual(outcome.dropped, set())
        # Both physically-gated candidates were withheld, observably.
        self.assertTrue(outcome.veto_fired)
        self.assertEqual(outcome.vetoed_candidates, 2)

    def test_candidates_do_not_mutually_corroborate(self) -> None:
        # Two candidates must not mutually corroborate each other's drop: a
        # corroborator has to be a span that can actually be an emitted
        # owner, and a standalone candidate might itself be dropped. Here
        # candidate 0's forward link lands exactly on candidate 1's span and
        # vice versa -- without the corroborator restriction both real
        # stocks would be dropped and the veto would never fire.
        class_codes = [self.OT_TIME, self.OT_STOCK, self.OT_STOCK]
        lookup_word10 = [9, 2, 1]

        outcome = vdf_xray.standalone_lookup_only_descriptors(
            spans=[
                _span(0, "Stock A", 1),
                _span(1, "Stock B", 2),
            ],
            f11_by_span=[1, 2],
            overlapping=set(),
            peeled_descriptors=set(),
            n_lookups=len(lookup_word10),
            lookup_word10=lookup_word10,
            lookup_word11=[1, 1, 1],
            class_codes=class_codes,
            ot_count=len(class_codes),
        )

        self.assertEqual(outcome.dropped, set())
        self.assertTrue(outcome.veto_fired)
        self.assertEqual(outcome.vetoed_candidates, 2)

    def test_peeled_descriptor_does_not_corroborate(self) -> None:
        # An overlap-peeled descriptor cannot corroborate a standalone drop:
        # a peeled ghost is not an emitted owner. The forward link here lands
        # on a span in `peeled_descriptors`, so the candidate is
        # uncorroborated and the veto fires.
        class_codes = [self.OT_TIME, self.OT_STOCK, self.OT_DYNAMIC]
        lookup_word10 = [9, 2]

        outcome = vdf_xray.standalone_lookup_only_descriptors(
            spans=[
                _span(0, "Some Stock", 1),
                _span(7, "Peeled ghost", 2),
            ],
            f11_by_span=[1, 0],
            overlapping=set(),
            peeled_descriptors={7},
            n_lookups=len(lookup_word10),
            lookup_word10=lookup_word10,
            lookup_word11=[1, 1],
            class_codes=class_codes,
            ot_count=len(class_codes),
        )

        self.assertEqual(outcome.dropped, set())
        self.assertTrue(outcome.veto_fired)
        self.assertEqual(outcome.vetoed_candidates, 1)

    def test_standalone_drop_veto_diagnostics_on_fixtures(self) -> None:
        # The coherence veto must be observable at the extraction surface:
        # Ref.vdf's ten standalone candidates all corroborate (no veto),
        # while SimService's four coincidental stock candidates trip it.
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")
        _, diagnostics = vdf_xray.extract_named_results_with_diagnostics(ref)
        self.assertFalse(diagnostics.standalone_drop_veto_fired)
        self.assertEqual(diagnostics.standalone_drop_vetoed_candidates, 0)

        relpath = ("third_party/uib_sd/spring_2008/SimService_BUENO/"
                   "SimService/Model Files/Base.vdf")
        if not (REPO_ROOT / relpath).exists():
            self.skipTest("third_party SimService fixtures not available")
        sim = parse_fixture(relpath)
        _, diagnostics = vdf_xray.extract_named_results_with_diagnostics(sim)
        self.assertTrue(diagnostics.standalone_drop_veto_fired)
        self.assertEqual(diagnostics.standalone_drop_vetoed_candidates, 4)

    def test_residual_overlap_recovery_on_fixtures(self) -> None:
        # Stage 2 recovery for GH #841. Ref.vdf (and every file whose overlaps
        # the peel already resolves) has NO residual components, so the whole
        # re-resolution is a provable no-op there. SimService/Base.vdf's
        # stale-f[11] unsaved-variable records survive the peel still in
        # owner-vs-owner conflict; the re-resolution recovers every real owner
        # (dropping only the ghosts) and, because every conflict is
        # adjudicated, leaves NOTHING honest-dropped -- so the residual
        # diagnostics come back empty, no ghost column is emitted, and no OT
        # slot is claimed twice.
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")
        _, ref_diag = vdf_xray.extract_named_results_with_diagnostics(ref)
        self.assertEqual(ref_diag.residual_overlap, [])

        relpaths = [
            "third_party/uib_sd/spring_2008/SimService_BUENO/SimService/Model Files/Base.vdf",
            "third_party/uib_sd/spring_2008/SimService_BUENO/SimService/Model Files/ctxt0001/Base.vdf",
        ]
        if not (REPO_ROOT / relpaths[0]).exists():
            self.skipTest("third_party SimService fixtures not available")

        # The EXACT 41 ghosts the re-resolution must drop on both fixtures
        # (lowercased). Pinning the full set plus the exact emitted-column count
        # below means a wrongly-recovered unlisted ghost cannot slip past.
        ghosts = [
            "age specific fertility distribution function",
            "china future gdp growth rate",
            "coal capacity utilisation in production table",
            "coal fraction discoverable table",
            "dice ipcc other rad forcing table",
            "effect of technology on productivity of investment in coal production table",
            "maize fraction table",
            "nuclear generation efficiency table",
            "oil substitutability effect on oil import",
            "pc meat demand function",
            "pc fish demand data",
            "pc fish demand function",
            "qdbtu to mb",
            "row coal demand function",
            "renewable energy consumer real price",
            "renewable resource price per mbtu",
            "wood value added per ton",
            "c total population table",
            "elasticity of fdi to fiscal pressure",
            "electricity net export table",
            "extra heavy table",
            "forestry production in cubic meters",
            "gas generation efficiency table",
            "gas to liquids table",
            "hydro electricity generation in bkwh table",
            "meat value added per ton table",
            "nuclear electricity demand in bkwh",
            "overall carbon tax table",
            "pc bushel domestic consumption non ethanol",
            "petroleum generation efficiency table",
            "private capital transfers over gdp table",
            "private factor income table",
            "private transfers table",
            "relative china technology",
            "renewable electricity price",
            "renewable portfolio standard table",
            "residential energy conservation",
            "share of electricity for freight transportation",
            "share of electricity for urban and commuter transportation",
            "total fertility rate",
            "value added agriculture products table",
        ]
        self.assertEqual(len(ghosts), 41)
        reals = [
            "Indicated China GDP",
            "indicated per capita fish demand",
            "indicated row Coal demand",
            "industrial electricity demand in BKWH",
        ]
        for relpath in relpaths:
            with self.subTest(relpath=relpath):
                vdf = parse_fixture(relpath)
                results, diag = vdf_xray.extract_named_results_with_diagnostics(vdf)
                self.assertIsNotNone(results)
                assert results is not None

                # Every conflict is adjudicated, so nothing is honest-dropped.
                self.assertEqual(diag.residual_overlap, [])

                names = {result.name for result in results}
                lower = {name.lower() for name in names}
                for ghost in ghosts:
                    self.assertFalse(
                        any(name == ghost or name.startswith(ghost + "[") for name in lower),
                        f"ghost {ghost!r} must not be emitted")
                for real in reals:
                    self.assertTrue(
                        real in names or any(n.startswith(real + "[") for n in names),
                        f"real owner {real!r} must be recovered")

                # OT-122 orphaned (its ghost `c total population table` dropped),
                # Population fully recovered, no OT slot claimed twice.
                ot_indices = [result.ot_index for result in results]
                self.assertEqual(
                    len(ot_indices), len(set(ot_indices)),
                    "no OT slot may be claimed by two emitted columns")
                # Exact emitted-column count: a wrongly-recovered ghost would push
                # it to 1236, a wrongly-dropped real below 1235.
                self.assertEqual(
                    len(results), 1235,
                    "exact emitted-column count changed (residual recovery regressed)")
                self.assertEqual(
                    sum(1 for name in names if name.startswith("Population[")), 164,
                    "recovered Population must expose all 164 array elements")
                oil = sorted(
                    result.values[0]
                    for result in results
                    if result.name.lower().startswith("c identified oil reserve["))
                self.assertEqual(len(oil), 2)
                # Initial reserves are context-independent stock initial values.
                self.assertAlmostEqual(oil[0], 7900.0, delta=0.01)
                self.assertAlmostEqual(oil[1], 51000.0, delta=0.01)

    def test_simservice_stocks_are_not_dropped_as_lookup_only(self) -> None:
        # Regression for the SimService false positives: four real dynamic
        # stocks whose OT starts collide with the lookup-index range must
        # keep their series with sane values.
        relpaths = [
            "third_party/uib_sd/spring_2008/SimService_BUENO/SimService/Model Files/Base.vdf",
            "third_party/uib_sd/spring_2008/SimService_BUENO/SimService/Model Files/ctxt0001/Base.vdf",
        ]
        if not (REPO_ROOT / relpaths[0]).exists():
            self.skipTest("third_party SimService fixtures not available")

        for relpath in relpaths:
            with self.subTest(relpath=relpath):
                vdf = parse_fixture(relpath)
                results = vdf_xray.extract_named_results(vdf)

                self.assertIsNotNone(results)
                assert results is not None
                by_name = {result.name: result for result in results}
                for name, expected_first in [
                    ("Agriculture Employment", 3.4e6),
                    ("Alaska Oil Discovered Reserves", None),
                    ("Alaska Oil Undiscovered Resources", None),
                    ("Atmos UOcean Temp", None),
                ]:
                    self.assertIn(name, by_name)
                    values = by_name[name].values
                    self.assertTrue(values)
                    self.assertTrue(
                        all(not math.isnan(value) for value in values), name)
                    if expected_first is not None:
                        self.assertAlmostEqual(
                            values[0], expected_first, delta=expected_first * 1e-3)

    def test_ref_extraction_drops_standalone_lookup_only_series_but_keeps_consumers(self) -> None:
        # Ref.vdf-level check: bare graphical functions (tables) no longer
        # emit ghost stock-slot series, while their consumer variables (real
        # owners) keep their series.
        ref = parse_fixture("test/xmutil_test_models/Ref.vdf")

        results = vdf_xray.extract_named_results(ref)

        self.assertIsNotNone(results)
        assert results is not None
        names = {result.name for result in results}
        for dropped_name in [
            "Historical GDP LOOKUP[OECD US]",
            "Historical GDP LOOKUP[COP Developing B]",
            "Historical forestry LOOKUP[OECD US]",
            "RS CH4[OECD US]",
            "RS CO2 FF[OECD US]",
            "RS GDP in trillions[COP Developing B]",
            "Global Emissions from graph LOOKUP",
            "Ozone precursor forcings",
        ]:
            self.assertNotIn(dropped_name, names)
        for consumer in [
            "Historical GDP[OECD US]",
            "Historical GDP[COP Developing B]",
            "CH4 anthro emissions[OECD US]",
        ]:
            self.assertIn(consumer, names)


class CorpusDecodedRecordSpanCoverageTests(unittest.TestCase):
    """
    Pin the corpus-wide property that motivates the direct-record-map
    extraction path: on every `exact-by-xray` fixture, `decoded_record_spans`
    (with the class-code guard) covers OT[1..N) exactly once and produces zero
    overlapping span-claims. The 9 `not-proven` fixtures all share one
    failure mode -- record-span overlap from the field[11] owner/descriptor
    union (see docs/design/vdf.md "Appendix: the owner/descriptor
    discriminator").
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


class ExtractJsonTests(unittest.TestCase):
    """
    Tests for the machine-readable `--extract-json` mode consumed by the Rust
    differential parity harness (tests/integration/vdf_parity.rs).
    """

    def _payload(self, relpaths: list[str]) -> dict:
        return vdf_xray.extract_results_json_payload(
            [REPO_ROOT / relpath for relpath in relpaths]
        )

    def test_encode_series_value_nan_and_infinity(self) -> None:
        # NaN encodes as null (None); infinities as strings; finite values
        # pass through unchanged. JSON has neither NaN nor Infinity literals,
        # and the Rust side decodes exactly these three shapes back.
        self.assertIsNone(vdf_xray._encode_series_value(float("nan")))
        self.assertEqual(vdf_xray._encode_series_value(float("inf")), "Infinity")
        self.assertEqual(vdf_xray._encode_series_value(float("-inf")), "-Infinity")
        self.assertEqual(vdf_xray._encode_series_value(1.5), 1.5)
        self.assertEqual(vdf_xray._encode_series_value(0.0), 0.0)

    def test_payload_shape_single_file(self) -> None:
        relpath = "test/bobby/vdf/water/Current.vdf"
        payload = self._payload([relpath])

        self.assertEqual(list(payload.keys()), [str(REPO_ROOT / relpath)])
        entries = payload[str(REPO_ROOT / relpath)]
        self.assertGreater(len(entries), 1)
        names = [e["name"] for e in entries]
        self.assertEqual(names[0], "Time")
        self.assertIn("water level", names)
        for entry in entries:
            self.assertEqual(sorted(entry.keys()), ["name", "ot_index", "values"])
            self.assertIsInstance(entry["name"], str)
            self.assertIsInstance(entry["ot_index"], int)
            self.assertIsInstance(entry["values"], list)
            # Every series covers the full saved grid.
            self.assertEqual(len(entry["values"]), len(entries[0]["values"]))

    def test_payload_multi_file_keys_are_paths_as_given(self) -> None:
        relpaths = [
            "test/bobby/vdf/water/Current.vdf",
            "test/bobby/vdf/subscripts/subscripts.vdf",
        ]
        payload = self._payload(relpaths)
        self.assertEqual(
            list(payload.keys()),
            [str(REPO_ROOT / relpath) for relpath in relpaths],
        )
        # The arrayed fixture emits element-labelled columns.
        sub_names = [e["name"] for e in payload[str(REPO_ROOT / relpaths[1])]]
        self.assertIn("a stock[a]", sub_names)

    def test_nan_values_encode_as_null_and_payload_is_strict_json(self) -> None:
        import json as json_mod
        import math as math_mod

        # Ref.vdf carries series with genuine missing-data NaN runs
        # (e.g. "Annual rate of emissions to target[OECD US]").
        relpath = "test/xmutil_test_models/Ref.vdf"
        payload = self._payload([relpath])
        entries = payload[str(REPO_ROOT / relpath)]

        nulls = sum(
            1 for e in entries for v in e["values"] if v is None
        )
        self.assertGreater(nulls, 0, "expected NaN-bearing series in Ref.vdf")
        for entry in entries:
            for value in entry["values"]:
                if value is None:
                    continue
                self.assertIsInstance(value, float)
                self.assertTrue(math_mod.isfinite(value))

        # allow_nan=False proves no non-finite float leaked into the payload;
        # a leak would raise ValueError here instead of emitting the
        # nonstandard NaN/Infinity tokens strict parsers reject.
        text = json_mod.dumps(payload, allow_nan=False)
        self.assertEqual(json_mod.loads(text).keys(), payload.keys())

    def test_dataset_vdf_is_rejected_loudly(self) -> None:
        with self.assertRaises(ValueError):
            self._payload(["test/bobby/vdf/econ/data.vdf"])


class DataGridBitmapTests(unittest.TestCase):
    """
    Header-0x74 data-grid bitmap coverage (GH #842), mirroring the Rust
    `vdf_data_grid.rs` integration tests: exogenous-data blocks (class codes
    0x05/0x06/0x0c) carry bitmaps over the external data file's time grid,
    whose point count is header word 0x74.
    """

    GROUPON = "test/metasd/social-network-valuation/groupon3mid.vdf"

    def test_data_grid_positions_span_mapping(self) -> None:
        # 71-point yearly grid inside an eighth-year-step run (the zambaqui
        # "old runs" shape, where the mapping is exact).
        saved = [1980.0 + 0.125 * i for i in range(561)]
        pos = vdf_xray.VdfFile._data_grid_positions(saved, 71)
        self.assertEqual(pos[0], 0)
        self.assertEqual(pos[7], 0)
        self.assertEqual(pos[8], 1)
        self.assertEqual(pos[560], 70)

        # 6-point grid over a 121-point monthly run (the groupon shape).
        saved = [float(i) for i in range(121)]
        pos = vdf_xray.VdfFile._data_grid_positions(saved, 6)
        self.assertEqual((pos[0], pos[23], pos[24], pos[120]), (0, 0, 1, 5))

        # Degenerate grids and axes.
        self.assertEqual(vdf_xray.VdfFile._data_grid_positions([3.0, 4.0], 1), [0, 0])
        self.assertEqual(vdf_xray.VdfFile._data_grid_positions([2.0, 2.0], 4), [0, 0])
        self.assertEqual(vdf_xray.VdfFile._data_grid_positions([], 5), [])

    def test_groupon_data_grid_blocks_decode(self) -> None:
        vdf = parse_fixture(self.GROUPON)
        self.assertEqual(vdf.data_time_point_count, 6)
        self.assertEqual(vdf.data_bitmap_size, 1)
        self.assertEqual(vdf.unreconciled_data_blocks(), [])

        time_values = vdf.extract_time_values()
        codes = vdf.section6_ot_class_codes()
        final_values = vdf.section6_final_values()
        assert time_values is not None
        assert codes is not None
        assert final_values is not None

        # Every data block reconciles, and every block series ends at the
        # recorded final value; count the data-grid population.
        data_grid_ots = []
        for ot in range(vdf.offset_table_count):
            raw = vdf.offset_table_entry(ot)
            if raw is None or not vdf.is_data_block_offset(raw):
                continue
            count = vdf_xray.u16(vdf.data, raw)
            _bm, _grid, kind = vdf._block_bitmap_layout(raw, count)
            self.assertIsNotNone(kind, f"OT[{ot}] unreconciled")
            if kind == "data":
                data_grid_ots.append(ot)
            series = vdf.extract_ot_series(ot, time_values, codes, final_values)
            assert series is not None
            self.assertAlmostEqual(
                series[-1],
                final_values[ot],
                places=5,
                msg=f"OT[{ot}] final-value oracle",
            )
        self.assertEqual(len(data_grid_ots), 21)

        # Value-level pin: OT[53] stores 4 values at data-grid points
        # {1,2,3,5} (bitmap 0x2e); the saved series starts NaN (no data
        # before the first stored point), ends at the recorded final value,
        # and surfaces exactly the four stored values.
        raw = vdf.offset_table_entry(53)
        assert raw is not None
        count = vdf_xray.u16(vdf.data, raw)
        self.assertEqual(count, 4)
        self.assertEqual(vdf._block_bitmap_layout(raw, count), (1, 6, "data"))
        series = vdf.extract_block_series(raw, time_values)
        self.assertTrue(math.isnan(series[0]))
        self.assertAlmostEqual(series[-1], 29504314.0, places=0)
        distinct: list[float] = []
        for value in series:
            if not math.isnan(value) and (not distinct or distinct[-1] != value):
                distinct.append(value)
        self.assertEqual(len(distinct), 4)
        self.assertAlmostEqual(distinct[0], 375099.0, places=0)

    def test_named_results_diagnostics_report_no_unreconciled_blocks(self) -> None:
        vdf = parse_fixture(self.GROUPON)
        _results, diagnostics = vdf_xray.extract_named_results_with_diagnostics(vdf)
        self.assertEqual(diagnostics.bitmap_unreconciled_ots, [])

    def test_unreconciled_block_is_nan_filled_and_reported(self) -> None:
        # Zero out a real dynamic block's bitmap in a small fixture: popcount
        # 0 can never equal a nonzero count, so no candidate grid reconciles
        # (water has no data grid, and its saved/block grids coincide).
        data = bytearray((REPO_ROOT / "test/bobby/vdf/water/Current.vdf").read_bytes())
        vdf = vdf_xray.parse_vdf(bytes(data))
        self.assertEqual(vdf.data_time_point_count, 0)
        raw = vdf.offset_table_entry(5)
        assert raw is not None
        self.assertTrue(vdf.is_data_block_offset(raw))
        count = vdf_xray.u16(vdf.data, raw)
        self.assertGreater(count, 0)
        for i in range(vdf.bitmap_size):
            data[raw + 2 + i] = 0

        corrupted = vdf_xray.parse_vdf(bytes(data))
        self.assertEqual(
            corrupted._block_bitmap_layout(raw, count),
            (corrupted.block_bitmap_size, corrupted.block_time_point_count, None),
        )
        self.assertEqual(corrupted.unreconciled_data_blocks(), [5])

        time_values = corrupted.extract_time_values()
        assert time_values is not None
        series = corrupted.extract_block_series(raw, time_values)
        self.assertTrue(all(math.isnan(v) for v in series))

        _results, diagnostics = vdf_xray.extract_named_results_with_diagnostics(corrupted)
        self.assertEqual(diagnostics.bitmap_unreconciled_ots, [5])

    def test_zambaqui_gdp_deflator_decodes_on_data_grid(self) -> None:
        # third_party corpora are optional checkouts; skip when absent
        # (matching the Rust existence-continue convention).
        relpath = "third_party/uib_sd/zambaqui/baserun.vdf"
        if not (REPO_ROOT / relpath).exists():
            self.skipTest("zambaqui corpus not checked out")
        vdf = parse_fixture(relpath)
        self.assertEqual(vdf.data_time_point_count, 26)
        self.assertEqual(vdf.unreconciled_data_blocks(), [])

        raw = vdf.offset_table_entry(696)
        assert raw is not None
        count = vdf_xray.u16(vdf.data, raw)
        self.assertEqual(count, 26)
        self.assertEqual(vdf._block_bitmap_layout(raw, count), (4, 26, "data"))

        time_values = vdf.extract_time_values()
        assert time_values is not None
        series = vdf.extract_block_series(raw, time_values)
        # Ground truth from the byte-identical block in the sibling
        # Data.vdf dataset (gdp deflator, 1980..2005 yearly). Extraction
        # only widens the stored f32s, so the pins are exact-equality.
        self.assertEqual(series[0], 0.6202020049095154)
        self.assertEqual(series[-1], 2.086980104446411)


class ResolveResidualComponentsTests(unittest.TestCase):
    """Stage 2 re-resolution ordering oracle (`resolve_residual_components`),
    mirroring the Rust `resolve_residual_tests` module span-for-span so the two
    readers stay in lockstep on synthetic traps as well as on the corpus."""

    @staticmethod
    def _span(rec_idx: int, name: str, start: int, length: int) -> vdf_xray.DecodedRecordSpan:
        return vdf_xray.DecodedRecordSpan(
            rec_idx=rec_idx, name_idx=rec_idx, name=name, start=start,
            end=start + length, shape_code=0, sort_key=0, slot_ref=0,
            group_id=0, has_sentinel=False, ot_codes=[])

    def _dropped_names(self, res, spans):
        return sorted(s.name for s in spans if s.rec_idx in res.dropped)

    def test_empty_components_is_noop(self):
        spans = [self._span(0, "a", 1, 1), self._span(1, "b", 2, 1)]
        res = vdf_xray.resolve_residual_components(
            spans, [], set(), vdf_xray.RESIDUAL_ORDERING_GATE,
            vdf_xray.RESIDUAL_ORDERING_MIN_PAIRS)
        self.assertFalse(res.dropped)
        self.assertFalse(res.readmitted)
        self.assertEqual(res.unresolved_components, [])

    def test_run_boundary_prev_only_drops_wide_ghost(self):
        spans = [
            self._span(0, "c gas", 1, 1),
            self._span(1, "agri", 30, 1),
            self._span(2, "age ghost", 3, 4),
            self._span(3, "c oil", 3, 1),
            self._span(4, "c pig", 4, 1),
            self._span(5, "c rat", 5, 1),
            self._span(6, "c sun", 6, 1),
        ]
        comps = vdf_xray.residual_overlap_components(spans, set())
        res = vdf_xray.resolve_residual_components(spans, comps, set(), 0.0, 0)
        self.assertEqual(self._dropped_names(res, spans), ["age ghost"])
        self.assertFalse(res.readmitted)
        self.assertEqual(res.unresolved_components, [])

    def test_recovered_anchor_bracket_resolves_scalar_pair(self):
        spans = [
            self._span(0, "tbl x lookup", 9, 2),
            self._span(1, "ind a", 10, 1),
            self._span(2, "ind b", 12, 1),
            self._span(3, "cat food", 12, 1),
            self._span(4, "tbl y lookup", 13, 2),
            self._span(5, "ind c", 14, 1),
            self._span(6, "a1", 11, 1),
            self._span(7, "z1", 16, 1),
        ]
        comps = vdf_xray.residual_overlap_components(spans, set())
        res = vdf_xray.resolve_residual_components(spans, comps, set(), 0.0, 0)
        self.assertEqual(
            self._dropped_names(res, spans),
            ["cat food", "tbl x lookup", "tbl y lookup"])
        self.assertEqual(res.unresolved_components, [])

    def test_inconclusive_conflict_is_honest_dropped(self):
        spans = [
            self._span(0, "a", 1, 1),
            self._span(1, "b", 9, 1),
            self._span(2, "yyy", 5, 1),
            self._span(3, "zzz", 5, 1),
        ]
        comps = vdf_xray.residual_overlap_components(spans, set())
        res = vdf_xray.resolve_residual_components(spans, comps, set(), 0.0, 0)
        self.assertEqual(self._dropped_names(res, spans), ["yyy", "zzz"])
        self.assertEqual(len(res.unresolved_components), 1)
        self.assertEqual(res.unresolved_components[0].contested_ots, [5])

    def test_unpeeled_descriptor_is_readmitted_when_kept(self):
        spans = [
            self._span(0, "c gas", 1, 1),
            self._span(1, "agri", 20, 1),
            self._span(2, "age ghost", 8, 4),
            self._span(3, "c pig", 9, 1),
            self._span(4, "c rat", 10, 1),
            self._span(5, "c sun", 11, 1),
            self._span(6, "c oil", 7, 2),
        ]
        phase1 = {6}
        comps = vdf_xray.residual_overlap_components(spans, phase1)
        res = vdf_xray.resolve_residual_components(spans, comps, phase1, 0.0, 0)
        self.assertEqual(self._dropped_names(res, spans), ["age ghost"])
        self.assertEqual(res.readmitted, {6})
        self.assertEqual(res.unresolved_components, [])

    def test_lexical_peel_drops_lookupish_names(self):
        spans = [
            self._span(0, "real var", 1, 1),
            self._span(1, "real var2", 2, 1),
            self._span(2, "foo lookup", 1, 2),
        ]
        comps = vdf_xray.residual_overlap_components(spans, set())
        res = vdf_xray.resolve_residual_components(spans, comps, set(), 0.0, 0)
        self.assertEqual(self._dropped_names(res, spans), ["foo lookup"])
        self.assertEqual(res.unresolved_components, [])

    def test_chained_descriptor_is_not_transitively_unpeeled(self):
        spans = [
            self._span(0, "c gas", 1, 1),
            self._span(1, "zz", 30, 1),
            self._span(2, "age ghost", 5, 5),
            self._span(3, "d1", 5, 1),
            self._span(4, "d2", 6, 1),
            self._span(5, "d3", 7, 1),
            self._span(6, "d4", 8, 1),
            self._span(7, "d oil", 9, 3),
            self._span(8, "d tar", 11, 2),
        ]
        phase1 = {7, 8}
        comps = vdf_xray.residual_overlap_components(spans, phase1)
        res = vdf_xray.resolve_residual_components(spans, comps, phase1, 0.0, 0)
        self.assertEqual(self._dropped_names(res, spans), ["age ghost"])
        self.assertEqual(res.readmitted, {7})
        # d tar overlaps only an un-peeled descriptor, so it stays untouched.
        self.assertNotIn(8, res.dropped)
        self.assertNotIn(8, res.readmitted)
        self.assertEqual(res.unresolved_components, [])

    def test_residual_pass_runs_on_lookup_free_file(self):
        # GH #844: identify_descriptor_records must reach the residual pass even
        # when the file has zero lookup records. Before the fix it early-returned
        # on n_lookups == 0, so a differently-named owner-vs-owner OT conflict on
        # such a file emitted BOTH names. The n_lookups == 0 path touches only
        # `section6_lookup_records`, so a tiny stub file suffices.
        class _NoLookupVdf:
            def section6_lookup_records(self):
                return []

        # Nine alphabetically-ordered uncontested owners (>= MIN_PAIRS pairs,
        # ratio 1.0) so the gate passes on real evidence; prev `e own`@4 / next
        # `n own`@6 bracket the slot-5 conflict, keeping `m real`, dropping the
        # ghost `z ghost` (which sorts past `n own`).
        spans = [
            self._span(0, "b own", 1, 1),
            self._span(1, "c own", 2, 1),
            self._span(2, "d own", 3, 1),
            self._span(3, "e own", 4, 1),
            self._span(4, "m real", 5, 1),   # fits [e own, n own] -> recovered
            self._span(5, "z ghost", 5, 1),  # sorts past n own -> dropped
            self._span(6, "n own", 6, 1),
            self._span(7, "o own", 7, 1),
            self._span(8, "p own", 8, 1),
            self._span(9, "q own", 9, 1),
            self._span(10, "r own", 10, 1),
        ]
        di = vdf_xray.identify_descriptor_records(_NoLookupVdf(), spans)
        self.assertIn(5, di.descriptor_indices, "ghost must be dropped with no lookups")
        self.assertNotIn(4, di.descriptor_indices, "real owner must be recovered")

    def test_same_name_duplicate_pair_survives_when_ghost_dropped(self):
        # GH #844: a differently-named ghost drags TWO same-name duplicate
        # records into a component. The conflict predicate is name-aware, so
        # once the ghost is dropped the same-name pair is no longer conflicting:
        # BOTH survive re-resolution and flow to the per-name emission dedup. A
        # name-blind predicate would honest-drop both, losing the variable.
        spans = [
            self._span(0, "a", 1, 1),
            self._span(1, "z", 9, 1),
            self._span(2, "m dup", 5, 1),
            self._span(3, "m dup", 5, 1),
            self._span(4, "zzz ghost", 5, 1),
        ]
        comps = vdf_xray.residual_overlap_components(spans, set())
        self.assertEqual(len(comps), 1)
        self.assertEqual(comps[0].span_indices, [2, 3, 4])
        res = vdf_xray.resolve_residual_components(spans, comps, set(), 0.0, 0)
        self.assertEqual(self._dropped_names(res, spans), ["zzz ghost"])
        self.assertNotIn(2, res.dropped)
        self.assertNotIn(3, res.dropped)
        self.assertEqual(res.unresolved_components, [])

    def test_gate_abstains_when_owners_not_alphabetical(self):
        # RATIO gate: low alphabetical consistency -> abstain. min_pairs=0
        # isolates the ratio side.
        spans = [
            self._span(0, "z", 1, 1),
            self._span(1, "a", 2, 1),
            self._span(2, "m", 3, 1),
            self._span(3, "b", 4, 1),
            self._span(4, "ghost", 10, 1),
            self._span(5, "owner", 10, 1),
        ]
        comps = vdf_xray.residual_overlap_components(spans, set())
        res = vdf_xray.resolve_residual_components(spans, comps, set(), 0.95, 0)
        self.assertEqual(self._dropped_names(res, spans), ["ghost", "owner"])
        self.assertEqual(len(res.unresolved_components), 1)

    def test_gate_abstains_on_too_few_measured_pairs(self):
        # MIN_PAIRS gate (GH #844, codex round 3): one uncontested anchor `m`
        # (0 adjacent pairs) is too little evidence, so the oracle must NOT run
        # (which would silently drop the real `a real` while keeping the ghost
        # `z ghost`). With the production floor the gate abstains: BOTH contested
        # spans honest-drop and the component surfaces on diagnostics.
        # gate_threshold=0.0 proves the pair-count floor drives it, not the ratio.
        spans = [
            self._span(0, "m", 3, 1),
            self._span(1, "a real", 5, 1),
            self._span(2, "z ghost", 5, 1),
        ]
        comps = vdf_xray.residual_overlap_components(spans, set())
        res = vdf_xray.resolve_residual_components(
            spans, comps, set(), 0.0, vdf_xray.RESIDUAL_ORDERING_MIN_PAIRS)
        self.assertEqual(self._dropped_names(res, spans), ["a real", "z ghost"])
        self.assertEqual(len(res.unresolved_components), 1)


if __name__ == "__main__":
    unittest.main()
