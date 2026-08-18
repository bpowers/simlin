"""The notebook widget's pure decisions (``simlin._widget_core``).

Every function here is an N-way decision; each test enumerates the arms.
The shell that executes them is covered in ``tests/test_widget.py``.
"""

from __future__ import annotations

import base64
import json
from typing import TYPE_CHECKING, Any

import pytest

from simlin._sync import ChangeEvent

if TYPE_CHECKING:
    from collections.abc import Callable
from simlin._widget_core import (
    ASSET_ENV,
    INLINE_WASM_GLOBAL,
    MAX_SNAPSHOT_BYTES,
    TORNADO_DEFAULT_MAX_MESSAGE_SIZE,
    WASM_FILE,
    AssetMode,
    MalformedSnapshot,
    OversizeReport,
    SnapshotOutcome,
    SnapshotRequest,
    Unrecognised,
    WasmRequest,
    dispatch_for_shell,
    format_size,
    inline_esm,
    is_own_change,
    missing_asset_message,
    notice_for_change,
    oversize_notice,
    oversize_report_warning,
    oversize_warning,
    parse_asset_mode,
    parse_incoming,
    plan_snapshot_reply,
    snapshot_wire_size,
    user_stacklevel,
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

    def test_oversize(self) -> None:
        assert parse_incoming({"type": "oversize", "bytes": 9_000_000}) == OversizeReport(
            bytes=9_000_000
        )
        assert parse_incoming({"type": "oversize", "bytes": 0}) == OversizeReport(bytes=0)

    @pytest.mark.parametrize(
        ("content", "reason"),
        [
            ("nope", "expected a JSON object"),
            (None, "expected a JSON object"),
            ({}, "unknown message type None"),
            ({"type": "saved"}, "unknown message type 'saved'"),
            ({"type": "oversize"}, "oversize 'bytes' must be a non-negative integer"),
            ({"type": "oversize", "bytes": "9"}, "oversize 'bytes' must be"),
            ({"type": "oversize", "bytes": True}, "oversize 'bytes' must be"),
            ({"type": "oversize", "bytes": -1}, "oversize 'bytes' must be"),
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


class TestSnapshotSize:
    """The snapshot cap and its three texts (seed/push warning, report
    warning, notice)."""

    def test_default_cap_leaves_room_under_the_tornado_default(self) -> None:
        # The cap is measured on the escaped snapshot text, so the only
        # other bytes in the frame are the envelope's other fields and the
        # jupyter message header/parent/metadata -- under 2 KiB in practice.
        # Pin a whole MiB of headroom so a future bump of the cap has to
        # argue with this line, not discover 1009 closes in a notebook.
        assert MAX_SNAPSHOT_BYTES == 8 * 1024 * 1024
        assert TORNADO_DEFAULT_MAX_MESSAGE_SIZE == 10 * 1024 * 1024
        assert MAX_SNAPSHOT_BYTES + 1024 * 1024 <= TORNADO_DEFAULT_MAX_MESSAGE_SIZE

    @pytest.mark.parametrize(
        ("text", "expected"),
        [
            ("", 2),  # the two quotes
            ("abc", 5),
            ('{"a":1}', 11),  # two escaped quotes -> +2, plus the outer quotes
            ("back\\slash", 13),  # one escaped backslash
            ("Ünï", 7),  # UTF-8, not \\uXXXX escapes (ensure_ascii=False)
            ("line\nbreak", 13),  # control character escaped to \\n
        ],
    )
    def test_snapshot_wire_size_is_the_escaped_utf8_length(self, text: str, expected: int) -> None:
        assert snapshot_wire_size(text) == expected
        assert snapshot_wire_size(text) == len(json.dumps(text, ensure_ascii=False).encode())

    # Pinned identically in src/notebook-widget/src/widget-core.test.ts
    # (`SIZE_FIXTURE`): both sides must print the same figure so the
    # kernel's notice and the browser's toast collapse into one message.
    @pytest.mark.parametrize(
        ("nbytes", "text"),
        [
            (0, "0 KiB"),
            (1, "0 KiB"),
            (512, "0 KiB"),  # 0.5: a tie, rounds to even
            (1536, "2 KiB"),  # 1.5: a tie, rounds to even
            (1537, "2 KiB"),
            (1024, "1 KiB"),
            (16, "0 KiB"),
            (262144, "256 KiB"),
            (1024 * 1024 - 1, "1024 KiB"),
            (1024 * 1024, "1 MiB"),
            (1_357_590, "1.3 MiB"),
            (8 * 1024 * 1024, "8 MiB"),
            (8 * 1024 * 1024 + 256 * 1024, "8.2 MiB"),  # 8.25: a tie, rounds to even
            (8 * 1024 * 1024 + 768 * 1024, "8.8 MiB"),  # 8.75: a tie, rounds to even
            (9_000_000, "8.6 MiB"),
            (10 * 1024 * 1024, "10 MiB"),
            (int(12.3 * 1024 * 1024), "12.3 MiB"),
            (104857600, "100 MiB"),
        ],
    )
    def test_format_size(self, nbytes: int, text: str) -> None:
        assert format_size(nbytes) == text

    @pytest.mark.parametrize("nbytes", [0, 1, MAX_SNAPSHOT_BYTES - 1, MAX_SNAPSHOT_BYTES])
    def test_at_or_below_the_cap_is_no_warning(self, nbytes: int) -> None:
        assert oversize_warning(nbytes, MAX_SNAPSHOT_BYTES) is None

    def test_above_the_cap_warns_with_the_cause_and_both_fixes(self) -> None:
        text = oversize_warning(MAX_SNAPSHOT_BYTES + 1, MAX_SNAPSHOT_BYTES)
        assert text is not None
        assert text.startswith("simlin: ")
        assert "8 MiB" in text
        assert "JSON-escaped" in text
        assert "will not be saved" in text
        assert "model.edit()" in text
        assert "websocket_max_message_size" in text
        assert "10 MiB" in text
        assert "--ServerApp.tornado_settings" in text
        assert "max_snapshot_bytes" in text
        # Hosts: only those behind a Jupyter server are subject to tornado's
        # limit; VS Code's local kernels are not, and must not be named.
        assert "through a Jupyter server" in text
        assert "VS Code" not in text

    def test_custom_limit_is_what_the_texts_report(self) -> None:
        limit = 32 * 1024 * 1024
        assert oversize_warning(limit, limit) is None
        text = oversize_warning(limit + 1, limit)
        assert text is not None
        assert "32 MiB" in text
        assert "32 MiB" in oversize_report_warning(limit + 1, limit)
        assert "32 MiB" in oversize_notice(limit + 1, limit)["text"]

    def test_report_warning_names_the_python_alternative_and_the_knob(self) -> None:
        text = oversize_report_warning(9_000_000, MAX_SNAPSHOT_BYTES)
        assert text.startswith("simlin: ")
        assert "was not saved" in text
        assert "8.6 MiB" in text
        assert "8 MiB" in text
        assert "from Python" in text
        assert "max_snapshot_bytes" in text
        assert "websocket_max_message_size" in text

    def test_notice_is_a_warn_level_notice_with_both_sizes(self) -> None:
        notice = oversize_notice(9_000_000, MAX_SNAPSHOT_BYTES)
        assert notice == {
            "type": "notice",
            "level": "warn",
            "text": (
                "Edit not saved: the model is too large for the notebook connection "
                "(8.6 MiB > 8 MiB limit); edit it from Python instead."
            ),
        }
        # Small caps (tests, tiny hosts) read in KiB, never "0.0 MiB > 0.0 MiB".
        assert "(1 KiB > 0 KiB limit)" in oversize_notice(1024, 16)["text"]


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
            self.request,
            SnapshotOutcome(applied=True, revision=5, error="disk full", write_failed=True),
        )
        assert plan.push == ('{"the":"snapshot"}', 5)
        assert plan.messages[0] == {"type": "saved", "revision": 5}
        notice = plan.messages[1]
        assert notice["type"] == "notice"
        assert notice["level"] == "warn"
        assert "disk full" in notice["text"]
        assert "model.save()" in notice["text"]
        assert len(plan.messages) == 2

    def test_applied_then_failed_later_is_saved_plus_a_warning_without_the_save_hint(
        self,
    ) -> None:
        # Applied AND written; what failed came after (notifying subscribers).
        # Still an accept, and the notice must not claim the file lags or
        # send the user to model.save(): there is nothing to retry.
        plan = plan_snapshot_reply(
            self.request, SnapshotOutcome(applied=True, revision=5, error="IOLoop is closed")
        )
        assert plan.push == ('{"the":"snapshot"}', 5)
        assert plan.messages[0] == {"type": "saved", "revision": 5}
        notice = plan.messages[1]
        assert notice["type"] == "notice"
        assert notice["level"] == "warn"
        assert "IOLoop is closed" in notice["text"]
        assert "applied" in notice["text"]
        assert "model.save()" not in notice["text"]
        assert "written" not in notice["text"]
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
            SnapshotOutcome(applied=True, revision=5, error="disk full", write_failed=True),
            SnapshotOutcome(applied=True, revision=5, error="IOLoop is closed"),
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
            (ChangeEvent("widget", 3), {3}, True),
            (ChangeEvent("widget", 3), {2, 3, 4}, True),  # several accepts pending
            (ChangeEvent("widget", 3), {4}, False),
            (ChangeEvent("widget", 3), set(), False),
            (ChangeEvent("edit", 3), {3}, False),
            (ChangeEvent("disk", 3), {3}, False),
            (ChangeEvent("reload", 3), {3}, False),
        ],
    )
    def test_arms(self, event: ChangeEvent, own: set[int], expected: bool) -> None:
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


def _frame_in(module_name: str) -> Callable[..., Any]:
    """A function whose frame belongs to module ``module_name`` (its
    ``f_globals['__name__']``, which is what ``user_stacklevel`` reads;
    the file the code lives in is not).  ``call(f, *rest)`` calls
    ``f(*rest)`` -- so a chain ``a(b, c, target)`` puts exactly the frames
    a -> b -> c -> target on the stack, with nothing of this module between
    them (a lambda would add a frame of this module)."""
    namespace: dict[str, Any] = {"__name__": module_name}
    exec("def call(f, *rest):\n    return f(*rest)\n", namespace)
    call: Callable[..., Any] = namespace["call"]
    return call


class TestUserStacklevel:
    """The stacklevel that names the user's cell: every ``simlin.*`` frame
    and every IPython/ipykernel frame between the cell and the ``warn``
    call adds one; the first frame of anything else is the target."""

    def test_direct_caller_is_level_one(self) -> None:
        # The function about to warn (this test) is user code: level 1.
        assert user_stacklevel() == 1

    def test_package_frames_are_skipped(self) -> None:
        # warn's caller is simlin.model, called from simlin.widget, called
        # from here -> 3.
        assert _frame_in("simlin.widget")(_frame_in("simlin.model"), user_stacklevel) == 3

    def test_ipython_and_ipykernel_frames_are_skipped(self) -> None:
        # A bare display: cell -> IPython.core.displayhook -> IPython.core.
        # formatters -> simlin.model._repr_mimebundle_ -> warn.  Naming the
        # formatter would put the warning at formatters.py once per kernel
        # session; the cell is what the user sees.
        level = _frame_in("IPython.core.displayhook")(
            _frame_in("IPython.core.formatters"), _frame_in("simlin.model"), user_stacklevel
        )
        assert level == 4
        assert _frame_in("ipykernel.zmqshell")(_frame_in("simlin.widget"), user_stacklevel) == 3

    def test_other_libraries_are_the_target(self) -> None:
        # Only simlin and the IPython machinery are skipped: a caller from
        # any other package (a user's own module, a framework) is the
        # target -- and a name that merely starts with "IPython" is not it.
        assert _frame_in("mylib.views")(_frame_in("simlin.model"), user_stacklevel) == 2
        assert _frame_in("IPythonish")(_frame_in("simlin.model"), user_stacklevel) == 2
        assert _frame_in("__main__")(user_stacklevel) == 1
