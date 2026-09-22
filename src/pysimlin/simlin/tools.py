"""The engine's agent tool surface, for Python hosts and evaluations.

A :class:`ToolSession` is one agent's work on one model: call a tool of the
engine's catalog by name with JSON-shaped input and read its JSON-shaped
output back, exactly as a native host's agent does. What each tool answers is
the engine's decision (``simlin_engine::tools``); this module only carries the
calls. A refusal -- an unknown variable, input the tool's schema does not
allow -- is output the agent reads (``ToolOutput.is_error``), never an
exception; an exception is the caller's misuse, such as a tool the catalog
does not list.

``edit_model`` plans an edit and applies nothing. :meth:`ToolSession.land`
lands a plan, as a host does once the person approves it: the engine lands it
on the project as it is (planned again when the project changed since, and
refused, with the reason, when what it writes changed or it no longer passes
the gate), and the project commits the edit as it commits any other.

Thread-safety: a session serializes its calls with a per-instance lock, and
the engine locks the session, the project's contents, and its database for
each call. :meth:`ToolSession.cancel` takes no lock, so another thread can
stop a call under way.
"""

from __future__ import annotations

import json
import threading
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
class Landing:
    """What landing a plan came to: whether it landed, and why not, for the
    agent to plan the edit again."""

    landed: bool
    reason: str | None = None


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


class ToolSession:
    """One agent's work on one model through the engine's tools.

    The session's evidence ids (diagnostics ``D1``, loops ``L1``, checks
    ``T1``, plans ``P1``, findings ``F1``), its last read and its runs are its
    own, and mean nothing to another session.
    """

    def __init__(self, model: Model) -> None:
        self._lock = threading.Lock()
        self._model = model
        err_ptr = ffi.new("SimlinError **")
        ptr = lib.simlin_tool_session_new(model._ptr, err_ptr)
        check_out_error(err_ptr, "Make a tool session")
        self._ptr = ptr
        _register_finalizer(self, lib.simlin_tool_session_unref, ptr)

    @property
    def model(self) -> Model:
        return self._model

    def call(self, tool: str, input: Mapping[str, Any] | None = None) -> ToolOutput:
        """Call ``tool`` with ``input`` (``{}`` when absent)."""
        payload = json.dumps(dict(input or {})).encode("utf-8")
        c_input = ffi.new("uint8_t[]", payload)
        out_buf = ffi.new("uint8_t **")
        out_len = ffi.new("uintptr_t *")
        out_is_error = ffi.new("bool *")
        err_ptr = ffi.new("SimlinError **")
        with self._lock:
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
        with self._lock:
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
        with self._lock:
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
        with self._lock:
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
        gone: each stops at its next checkpoint and answers a refusal whose
        :attr:`ToolOutput.cancelled` is set. A call made after this runs as
        usual. Returns at once, and takes no lock, so any thread may call it.
        """
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
        with self._lock:
            lib.simlin_tool_session_forget_run(self._ptr, string_to_c(name), forgotten, err_ptr)
        check_out_error(err_ptr, f"Forget run '{name}'")
        return bool(forgotten[0])

    def land(self, id: str) -> Landing:
        """Land the plan ``id`` in the model's project, as a host does once
        the person approves it. The engine lands it on the project as it is:
        at the revision it was planned at, as planned; at another, planned
        again, and only when what it writes is as it was, the gate passes
        again, and it would make the changes the person approved. A plan
        that cannot land says why, for the agent to plan the edit again.

        Raises:
            SimlinRuntimeError: If the session has no plan ``id``.
        """
        project = self._model._require_project()
        answer = project._land_tool_plan(self, id)
        return Landing(landed=bool(answer["landed"]), reason=answer.get("reason"))
