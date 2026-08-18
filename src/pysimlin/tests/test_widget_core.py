"""The notebook widget's pure decisions (``simlin._widget_core``).

Every function here is an N-way decision; each test enumerates the arms.
The shell that executes them is covered in ``tests/test_widget.py``.
"""

from __future__ import annotations

import base64

import pytest

from simlin._sync import ChangeEvent
from simlin._widget_core import (
    ASSET_ENV,
    INLINE_WASM_GLOBAL,
    WASM_FILE,
    AssetMode,
    MalformedSnapshot,
    SnapshotOutcome,
    SnapshotRequest,
    Unrecognised,
    WasmRequest,
    dispatch_for_shell,
    inline_esm,
    is_own_change,
    missing_asset_message,
    notice_for_change,
    parse_asset_mode,
    parse_incoming,
    plan_snapshot_reply,
)


class TestAssetMode:
    @pytest.mark.parametrize("value", [None, "", "  ", "bundled", " bundled "])
    def test_default_is_bundled(self, value: str | None) -> None:
        assert parse_asset_mode(value) == AssetMode("bundled")

    def test_inline(self) -> None:
        assert parse_asset_mode("inline") == AssetMode("inline")

    @pytest.mark.parametrize(
        "url", ["http://localhost:5173/widget.js", "https://cdn.example.com/w.js"]
    )
    def test_url(self, url: str) -> None:
        assert parse_asset_mode(url) == AssetMode("url", url=url)

    @pytest.mark.parametrize("value", ["serve", "file:///tmp/widget.js", "INLINE", "yes"])
    def test_unknown_value_raises_naming_the_variable(self, value: str) -> None:
        with pytest.raises(ValueError, match=ASSET_ENV) as excinfo:
            parse_asset_mode(value)
        assert repr(value) in str(excinfo.value)

    def test_inline_esm_is_the_documented_shim_then_the_module(self) -> None:
        wasm = bytes(range(256))
        module = "export default { render() {} };\n"
        esm = inline_esm(module, wasm)
        shim, rest = esm.split("\n", 1)
        assert shim == f'globalThis.{INLINE_WASM_GLOBAL} = "{base64.b64encode(wasm).decode()}";'
        assert rest == module
        assert INLINE_WASM_GLOBAL == "__simlinWidgetInlineWasm"

    def test_missing_asset_message_is_actionable(self) -> None:
        text = missing_asset_message(WASM_FILE, "/site-packages/simlin/_widget")
        assert WASM_FILE in text
        assert "/site-packages/simlin/_widget" in text
        assert "reinstall" in text
        assert "pnpm" in text
        assert ASSET_ENV in text
        assert "README.md" in text  # where the staging step is described


class TestParseIncoming:
    def test_wasm(self) -> None:
        assert parse_incoming({"type": "wasm"}) == WasmRequest()

    def test_snapshot(self) -> None:
        assert parse_incoming({"type": "snapshot", "base": 3, "json": "{}"}) == SnapshotRequest(
            base=3, json="{}"
        )

    @pytest.mark.parametrize(
        ("content", "reason"),
        [
            ("nope", "expected a JSON object"),
            (None, "expected a JSON object"),
            ({}, "unknown message type None"),
            ({"type": "saved"}, "unknown message type 'saved'"),
        ],
    )
    def test_unrecognised(self, content: object, reason: str) -> None:
        message = parse_incoming(content)
        assert isinstance(message, Unrecognised)
        assert reason in message.reason

    @pytest.mark.parametrize(
        ("content", "reason"),
        [
            ({"type": "snapshot", "json": "{}"}, "'base' must be an integer"),
            ({"type": "snapshot", "base": "3", "json": "{}"}, "'base' must be an integer"),
            ({"type": "snapshot", "base": True, "json": "{}"}, "'base' must be an integer"),
            ({"type": "snapshot", "base": 3}, "'json' must be the project as a JSON string"),
            ({"type": "snapshot", "base": 3, "json": {"a": 1}}, "'json' must be"),
        ],
    )
    def test_malformed_snapshot_is_distinct_so_it_gets_its_reply(
        self, content: object, reason: str
    ) -> None:
        message = parse_incoming(content)
        assert isinstance(message, MalformedSnapshot)
        assert reason in message.reason


class TestPlanSnapshotReply:
    request = SnapshotRequest(base=4, json='{"the":"snapshot"}')

    def test_applied_pushes_exact_json_at_base_plus_one_then_saved(self) -> None:
        # The revision read back (here deliberately further along, as if a
        # concurrent change landed) is not what the accept pushes: the
        # accept produced exactly base + 1.
        plan = plan_snapshot_reply(self.request, SnapshotOutcome(applied=True, revision=6))
        assert plan.push == ('{"the":"snapshot"}', 5)
        assert plan.messages == ({"type": "saved", "revision": 5},)

    def test_applied_but_unwritten_is_saved_plus_a_warning(self) -> None:
        plan = plan_snapshot_reply(
            self.request, SnapshotOutcome(applied=True, revision=5, error="disk full")
        )
        assert plan.push == ('{"the":"snapshot"}', 5)
        assert plan.messages[0] == {"type": "saved", "revision": 5}
        notice = plan.messages[1]
        assert notice["type"] == "notice"
        assert notice["level"] == "warn"
        assert "disk full" in notice["text"]
        assert "model.save()" in notice["text"]
        assert len(plan.messages) == 2

    def test_stale_touches_no_trait_and_rejects_with_notice(self) -> None:
        plan = plan_snapshot_reply(self.request, SnapshotOutcome(applied=False, revision=7))
        assert plan.push is None
        assert plan.messages[0] == {"type": "rejected", "revision": 7}
        notice = plan.messages[1]
        assert notice["type"] == "notice"
        assert notice["level"] == "warn"
        assert "revision 4" in notice["text"]
        assert "at 7" in notice["text"]
        assert len(plan.messages) == 2

    def test_unapplied_error_touches_no_trait_and_rejects_with_the_error(self) -> None:
        plan = plan_snapshot_reply(
            self.request, SnapshotOutcome(applied=False, revision=4, error="bad json")
        )
        assert plan.push is None
        assert plan.messages[0] == {"type": "rejected", "revision": 4}
        assert plan.messages[1]["level"] == "warn"
        assert "bad json" in plan.messages[1]["text"]

    @pytest.mark.parametrize(
        "outcome",
        [
            SnapshotOutcome(applied=True, revision=5),
            SnapshotOutcome(applied=True, revision=5, error="disk full"),
            SnapshotOutcome(applied=False, revision=7),
            SnapshotOutcome(applied=False, revision=4, error="bad json"),
        ],
    )
    def test_every_arm_sends_exactly_one_integer_reply(self, outcome: SnapshotOutcome) -> None:
        plan = plan_snapshot_reply(self.request, outcome)
        replies = [m for m in plan.messages if m["type"] in ("saved", "rejected")]
        assert len(replies) == 1
        assert type(replies[0]["revision"]) is int


class TestIsOwnChange:
    @pytest.mark.parametrize(
        ("event", "own", "expected"),
        [
            (ChangeEvent("widget", 3), 3, True),
            (ChangeEvent("widget", 3), 4, False),
            (ChangeEvent("widget", 3), None, False),
            (ChangeEvent("edit", 3), 3, False),
            (ChangeEvent("disk", 3), 3, False),
            (ChangeEvent("reload", 3), 3, False),
        ],
    )
    def test_arms(self, event: ChangeEvent, own: int | None, expected: bool) -> None:
        assert is_own_change(event, own) is expected


class TestNoticeForChange:
    @pytest.mark.parametrize(
        ("source", "text"),
        [
            ("disk", "Updated on disk"),
            ("reload", "Reloaded from disk"),
            ("edit", "Updated from Python"),
            ("widget", "Updated in another view"),
        ],
    )
    def test_every_source_has_an_info_notice(self, source: str, text: str) -> None:
        assert notice_for_change(source) == (text, "info")  # type: ignore[arg-type]

    def test_unknown_source_raises(self) -> None:
        with pytest.raises(ValueError, match="unknown change source"):
            notice_for_change("mystery")  # type: ignore[arg-type]


class TestDispatchForShell:
    def test_no_shell(self) -> None:
        assert dispatch_for_shell(None) is None

    def test_shell_without_kernel(self) -> None:
        class Shell:
            pass

        assert dispatch_for_shell(Shell()) is None

    def test_kernel_without_loop(self) -> None:
        class Kernel:
            io_loop = None

        class Shell:
            kernel = Kernel()

        assert dispatch_for_shell(Shell()) is None

    def test_loop_with_add_callback(self) -> None:
        calls: list[object] = []

        class Loop:
            def add_callback(self, fn: object) -> None:
                calls.append(fn)

        class Kernel:
            io_loop = Loop()

        class Shell:
            kernel = Kernel()

        dispatch = dispatch_for_shell(Shell())
        assert dispatch is not None
        dispatch(lambda: None)
        assert len(calls) == 1
