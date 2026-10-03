"""The engine's agent tool surface, for Python hosts and evaluations.

A :class:`ToolSession` is one agent's work on one model: call a tool of the
engine's catalog by name with JSON-shaped input and read its JSON-shaped
output back, exactly as a native host's agent does. What each tool answers is
the engine's decision (``simlin_engine::tools``); this module only carries the
calls. A refusal -- input the tool's schema does not allow, an edit the
engine's gate refuses -- is output the agent reads (``ToolOutput.is_error``,
set exactly when the call did not do what was asked), never an exception;
an exception is the caller's misuse, such as a tool the catalog does not
list.

A tool whose catalog entry says its effect is ``edit`` (``edit_model``) edits
the project inside the call when the engine's gate passes it. The project then
commits the edit as it commits any other: its revision moves, its models'
caches are dropped, a file-backed project writes back to its file, and its
subscribers are told. A refused, interrupted or cancelled edit changes
nothing, and nothing is committed.

Thread-safety: a session holds no lock of its own. The engine serializes a
session's calls (its session is a mutex), and it is the engine that knows
which calls wait: a call takes its cancel ticket as it enters the engine,
before it waits for the session, and an edit counts itself among the work a
read call stops for before it waits. A lock here would make a second call
wait in Python where the engine cannot see it, so :meth:`ToolSession.cancel`
would miss the call, and a read call under way would never stop for an edit.
The handle is set once and released when the session is collected, so nothing
here needs guarding.

An edit tool's call holds the project's locks as any edit does
(``Project._file_lock``, then ``Project._lock``), and waits for them in Python,
before it reaches the engine's ticket. So the session keeps a ticket of its
own, which no lock guards: :meth:`ToolSession.cancel` advances it, an edit
reads it before it waits for the project's locks, and once it holds them an
edit cancelled meanwhile answers the engine's cancelled refusal without
entering the engine.
"""

from __future__ import annotations

import functools
import itertools
import json
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any

from ._ffi import _register_finalizer, check_out_error, ffi, lib, string_to_c
from .vdf import _results_to_dataframe

if TYPE_CHECKING:
    from collections.abc import Mapping

    import pandas as pd

    from .model import Model


@dataclass(frozen=True)
class ToolOutput:
    """What a tool answered: its JSON, parsed, and whether it is a refusal
    for the agent to read and repair rather than a result."""

    data: Any
    is_error: bool

    @property
    def interrupted(self) -> bool:
        """Whether the call stopped for other work on the project, which was
        waiting for it, and kept nothing. Call again once that work is done
        (after an edit, at the next revision), never in a loop."""
        refusal = self.data if isinstance(self.data, dict) else {}
        return self.is_error and refusal.get("interrupted") is True

    @property
    def cancelled(self) -> bool:
        """Whether the call stopped because its host cancelled it
        (:meth:`ToolSession.cancel`), and kept nothing. Not to be called
        again."""
        refusal = self.data if isinstance(self.data, dict) else {}
        return self.is_error and refusal.get("cancelled") is True


@dataclass(frozen=True)
class RunListing:
    """One of a session's named runs: the revision it was made at, whether
    the model has changed since (its diagrams aside), whether it is gone
    (stale, with its series no longer kept), the run it started from, and
    everything it changed from the model, exactly as it ran.

    ``changes`` holds one JSON object per variable: its ``variable`` and its
    ``value``, per-element ``elements`` or replacement ``equation``, with a
    value's ``fromTime``; ``specs`` the run specs it set (``start``,
    ``stop``, ``dt``, ``method``)."""

    name: str
    revision: int
    stale: bool
    gone: bool
    from_run: str = "current"
    changes: tuple[dict[str, Any], ...] = ()
    specs: dict[str, Any] = field(default_factory=dict)


def catalog() -> dict[str, Any]:
    """The tool catalog: every tool's name, description, effect, and the JSON
    Schema of its input and output."""
    out_buf = ffi.new("uint8_t **")
    out_len = ffi.new("uintptr_t *")
    err_ptr = ffi.new("SimlinError **")
    lib.simlin_tools_describe(out_buf, out_len, err_ptr)
    check_out_error(err_ptr, "Describe the tools")
    try:
        result: dict[str, Any] = json.loads(bytes(ffi.buffer(out_buf[0], out_len[0])))
        return result
    finally:
        lib.simlin_free(out_buf[0])


def _cancelled() -> ToolOutput:
    """The engine's answer to a call its host cancelled before it began its
    work (``simlin_engine::tools::ToolOutput::cancelled``), for an edit
    cancelled while it waited for the project's locks: the same refusal, so
    a host tells the two apart by nothing. ``tests/test_tools.py`` holds it
    equal to the engine's."""
    return ToolOutput(
        data={
            "error": "the host cancelled the call, which stopped before it finished "
            "and kept nothing",
            "cancelled": True,
        },
        is_error=True,
    )


@functools.cache
def _tool_effects() -> dict[str, str]:
    """Each tool's effect, as the catalog states it: the one place the binding
    learns which tools edit the project."""
    return {tool["name"]: tool["effect"] for tool in catalog()["tools"]}


class ToolSession:
    """One agent's work on one model through the engine's tools.

    The session's evidence ids (diagnostics ``D1``, loops ``L1``, checks
    ``T1``, findings ``F1``), its last read and its runs are its own, and
    mean nothing to another session.
    """

    def __init__(self, model: Model) -> None:
        self._model = model
        err_ptr = ffi.new("SimlinError **")
        ptr = lib.simlin_tool_session_new(model._ptr, err_ptr)
        check_out_error(err_ptr, "Make a tool session")
        self._ptr = ptr
        _register_finalizer(self, lib.simlin_tool_session_unref, ptr)
        # The session's own cancel ticket (see the module docstring): the
        # number of the latest cancel. ``next`` on an ``itertools.count`` is
        # one step under the GIL, so concurrent cancels each get a number.
        self._cancels = itertools.count(1)
        self._cancelled = 0

    @property
    def model(self) -> Model:
        return self._model

    def call(self, tool: str, input: Mapping[str, Any] | None = None) -> ToolOutput:
        """Call ``tool`` with ``input`` (``{}`` when absent).

        A tool whose effect is ``edit`` changes the project when its gate
        passes; the project commits that edit before this returns (see
        ``Project._commit_tool_edit``), so the model reads as edited, a
        file-backed project is written, and subscribers are told.

        Raises:
            SimlinRuntimeError: For a tool the catalog does not list.
            SimlinWriteError: When an edit was made and committed in memory
                but a file-backed project's autosave failed; its ``answer``
                is the tool's answer, and ``__cause__`` the failure.
        """
        payload = json.dumps(dict(input or {})).encode("utf-8")
        if _tool_effects().get(tool) == "edit":
            project = self._model._require_project()
            ticket = self._cancelled

            def edit() -> ToolOutput:
                if self._cancelled != ticket:
                    return _cancelled()
                return self._call(tool, payload)

            return project._commit_tool_edit(edit)
        return self._call(tool, payload)

    def _call(self, tool: str, payload: bytes) -> ToolOutput:
        """The engine's answer to ``tool`` with the JSON ``payload``."""
        c_input = ffi.new("uint8_t[]", payload)
        out_buf = ffi.new("uint8_t **")
        out_len = ffi.new("uintptr_t *")
        out_is_error = ffi.new("bool *")
        err_ptr = ffi.new("SimlinError **")
        lib.simlin_tool_session_call(
            self._ptr,
            string_to_c(tool),
            c_input,
            len(payload),
            out_buf,
            out_len,
            out_is_error,
            err_ptr,
        )
        check_out_error(err_ptr, f"Call {tool}")
        try:
            data = json.loads(bytes(ffi.buffer(out_buf[0], out_len[0])))
        finally:
            lib.simlin_free(out_buf[0])
        return ToolOutput(data=data, is_error=bool(out_is_error[0]))

    def changes(self) -> dict[str, Any] | None:
        """What changed in the model's variables and sim specs since the
        session's last ``read_model``; ``None`` before the first read and when
        nothing did."""
        out_buf = ffi.new("uint8_t **")
        out_len = ffi.new("uintptr_t *")
        err_ptr = ffi.new("SimlinError **")
        lib.simlin_tool_session_get_changes(self._ptr, out_buf, out_len, err_ptr)
        check_out_error(err_ptr, "Read the changes")
        try:
            result: dict[str, Any] | None = json.loads(bytes(ffi.buffer(out_buf[0], out_len[0])))
            return result
        finally:
            lib.simlin_free(out_buf[0])

    def run(self, name: str = "current") -> pd.DataFrame:
        """Every saved series of the session's run ``name`` ("current" for
        the model as it is, or a run ``run_experiment`` made), as a DataFrame
        shaped like :attr:`Run.results <simlin.Run.results>`.

        Raises:
            SimlinRuntimeError: For a run the session lacks; with code
                ``ErrorCode.INTERRUPTED`` when the read had to simulate,
                stopped for other work on the project and kept nothing: read
                it again once that work is done.
        """
        err_ptr = ffi.new("SimlinError **")
        results = lib.simlin_tool_session_get_run(
            self._ptr, string_to_c(name), ffi.NULL, ffi.NULL, err_ptr
        )
        check_out_error(err_ptr, f"Read run '{name}'")
        try:
            return _results_to_dataframe(results)
        finally:
            lib.simlin_results_unref(results)

    def runs(self) -> list[RunListing]:
        """The session's named runs, oldest first. "current", the model as it
        is, is always there and is not listed."""
        out_buf = ffi.new("uint8_t **")
        out_len = ffi.new("uintptr_t *")
        err_ptr = ffi.new("SimlinError **")
        lib.simlin_tool_session_list_runs(self._ptr, out_buf, out_len, err_ptr)
        check_out_error(err_ptr, "List the runs")
        try:
            listed = json.loads(bytes(ffi.buffer(out_buf[0], out_len[0])))
        finally:
            lib.simlin_free(out_buf[0])
        return [
            RunListing(
                name=run["name"],
                revision=int(run["revision"]),
                stale=bool(run["stale"]),
                gone=bool(run["gone"]),
                from_run=run["from"],
                changes=tuple(run["changes"]),
                specs=dict(run["specs"]),
            )
            for run in listed
        ]

    def cancel(self) -> None:
        """Cancel the session's calls under way, the one answering and any
        waiting for the session, as a host does when what they were for is
        gone: each stops at its next checkpoint (one still waiting for the
        session, as soon as it has it) and answers a refusal whose
        :attr:`ToolOutput.cancelled` is set. An edit still waiting for the
        project's locks in Python is cancelled too, and answers as one the
        engine cancelled. A call made after this runs as usual. Returns at
        once, and takes no lock, so any thread may call it.
        """
        self._cancelled = next(self._cancels)
        lib.simlin_tool_session_cancel(self._ptr)

    def forget(self, name: str) -> bool:
        """Forget the run ``name``, its series and its plan, as a host does
        when the person discards it: no tool reads it again, and a run made
        from it keeps what it changed. Whether the session had it.

        Raises:
            SimlinRuntimeError: For "current", the model as it is.
        """
        forgotten = ffi.new("bool *")
        err_ptr = ffi.new("SimlinError **")
        lib.simlin_tool_session_forget_run(self._ptr, string_to_c(name), forgotten, err_ptr)
        check_out_error(err_ptr, f"Forget run '{name}'")
        return bool(forgotten[0])
