import unittest
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


if __name__ == "__main__":
    unittest.main()
