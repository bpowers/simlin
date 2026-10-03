"""The engine's agent tools through pysimlin: the catalog, a session's calls,
its runs, and an edit a tool makes committed as the project's other edits are.

What each tool answers is pinned by the engine's own tests
(``simlin_engine::tools``); these pin the binding.
"""

import json
import shutil
import threading
import time
from pathlib import Path
from typing import TYPE_CHECKING, Any

import pytest

import simlin
from simlin import tools as tools_module
from simlin._ffi import apply_patch_json
from simlin.errors import SimlinRuntimeError, SimlinWriteError
from simlin.json_converter import converter
from simlin.json_types import JsonProjectPatch
from simlin.model import ModelPatchBuilder
from simlin.tools import ToolOutput, ToolSession, catalog
from simlin.types import Aux

if TYPE_CHECKING:
    from simlin._sync import ChangeEvent

LOGISTIC = Path(__file__).parent / "logistic-growth.sd.json"
# A model whose battery runs for ten seconds or more: long enough for another
# thread to cancel it, queue behind it, or edit the model while it runs. Each
# test stops the battery within a second, so none pays for the whole of it.
LARGE = Path(__file__).parents[3] / "test" / "xmutil_test_models" / "C-LEARN v77 for Vensim.mdl"
WORLD3 = Path(__file__).parents[3] / "test" / "metasd" / "WRLD3-03" / "wrld3-03.mdl"
# How long a thread is given to enter the engine before the test acts on it.
ENTERED = 0.5


@pytest.fixture
def model() -> simlin.Model:
    return simlin.load(LOGISTIC)


def test_the_catalog_lists_every_tool_with_its_schemas() -> None:
    tools = catalog()["tools"]
    names = [tool["name"] for tool in tools]
    for name in ("read_model", "run_experiment", "analyze_loops", "edit_model", "verify_findings"):
        assert name in names
    for tool in tools:
        assert tool["inputSchema"]["type"] == "object", tool["name"]
        assert tool["effect"] in ("read", "edit"), tool["name"]
    effects = {tool["name"]: tool["effect"] for tool in tools}
    assert effects["edit_model"] == "edit"
    assert effects["read_model"] == "read"


def test_a_call_answers_with_json_and_a_refusal_is_output(model: simlin.Model) -> None:
    session = ToolSession(model)
    outline = session.call("read_model")
    assert not outline.is_error
    assert outline.data["revision"] == 0
    assert outline.data["stocks"]

    refusal = session.call("read_variables", {"names": ["no such variable"]})
    assert not refusal.is_error, "an unknown name is reported, not refused"
    assert refusal.data["notFound"][0]["name"] == "no such variable"

    refusal = session.call("read_variables", {"notAField": 1})
    assert refusal.is_error
    assert "notAField" in refusal.data["error"]

    with pytest.raises(SimlinRuntimeError):
        session.call("read_everything")


def test_a_run_reaches_python_as_a_dataframe(model: simlin.Model) -> None:
    session = ToolSession(model)
    stock = session.call("read_model").data["stocks"][0]["name"]
    current = session.run()
    assert current.index.name == "time"
    assert any(column.lower() == stock.lower().replace(" ", "_") for column in current.columns)
    with pytest.raises(SimlinRuntimeError):
        session.run("nowhere")


def test_a_sessions_runs_are_listed_with_their_revision_and_staleness(
    model: simlin.Model,
) -> None:
    session = ToolSession(model)
    assert session.runs() == []
    outline = session.call("read_model").data
    constant = outline["constants"][0]["name"]
    made = session.call(
        "run_experiment",
        {"name": "doubled", "set": [{"variable": constant, "multiply": 2}]},
    )
    assert not made.is_error, made.data
    (listed,) = session.runs()
    assert (listed.name, listed.revision, listed.stale, listed.gone) == ("doubled", 0, False, False)
    assert listed.from_run == "current"
    (change,) = listed.changes
    assert change["variable"] == made.data["applied"][0]["variable"]
    assert change["value"] == pytest.approx(made.data["applied"][0]["value"], rel=1e-4)
    assert listed.specs == {}
    assert not session.run("doubled").empty
    assert session.forget("doubled")
    assert session.runs() == []
    assert not session.forget("doubled")
    with pytest.raises(SimlinRuntimeError):
        session.forget("current")


def test_a_cancel_stops_only_the_calls_under_way(model: simlin.Model) -> None:
    session = ToolSession(model)
    session.cancel()
    outline = session.call("read_model")
    assert not outline.is_error, outline.data
    assert not outline.cancelled


def _call_in_thread(session: ToolSession, tool: str, answers: list[ToolOutput]) -> threading.Thread:
    """Start ``tool`` on another thread and give it time to enter the engine."""
    thread = threading.Thread(target=lambda: answers.append(session.call(tool)))
    thread.start()
    time.sleep(ENTERED)
    return thread


def test_a_cancel_from_another_thread_stops_the_call_under_way() -> None:
    session = ToolSession(simlin.load(LARGE))
    session.call("read_model")
    answers: list[ToolOutput] = []
    running = _call_in_thread(session, "run_tests", answers)
    session.cancel()
    running.join(timeout=120)
    assert not running.is_alive()
    assert answers[0].cancelled, str(answers[0].data)[:300]
    assert not answers[0].interrupted
    # The binding's own cancelled answer, for an edit it cancels before the
    # engine sees it, is the engine's.
    assert answers[0].data == tools_module._cancelled().data
    after = session.call("read_model")
    assert not after.is_error, "a call made after the cancel answers"


def test_a_cancel_covers_a_call_waiting_for_the_session() -> None:
    """A second call waits for the session inside the engine, where it holds
    a ticket the cancel marks; a lock in the binding would keep it waiting in
    Python with no ticket, and it would run whole once the first stopped."""
    session = ToolSession(simlin.load(LARGE))
    session.call("read_model")
    first: list[ToolOutput] = []
    second: list[ToolOutput] = []
    running = _call_in_thread(session, "run_tests", first)
    waiting = _call_in_thread(session, "analyze_loops", second)
    session.cancel()
    running.join(timeout=120)
    waiting.join(timeout=120)
    assert first[0].cancelled, str(first[0].data)[:300]
    assert second[0].cancelled, str(second[0].data)[:300]


ADD_SPARE = {
    "summary": "Add a named constant.",
    "operations": [{"op": "add_variable", "name": "spare", "equation": "3"}],
}


def test_an_edit_stops_the_read_call_it_waits_for() -> None:
    """The engine counts an edit among the work a read call stops for before
    the edit waits for the session, so an edit waits at most one unit of the
    read call's work, not for the whole call."""
    model = simlin.load(LARGE)
    session = ToolSession(model)
    session.call("read_model")
    answers: list[ToolOutput] = []
    running = _call_in_thread(session, "run_tests", answers)
    edit = session.call(
        "edit_model",
        {
            "summary": "A lower goal.",
            "operations": [
                {
                    "op": "set_equation",
                    "variable": "goal for temperature",
                    "equation": "1.5",
                }
            ],
        },
    )
    running.join(timeout=120)
    assert not edit.is_error, edit.data
    assert answers[0].interrupted, str(answers[0].data)[:300]
    assert not answers[0].cancelled


def test_an_edit_is_made_in_the_call_and_the_project_commits_it(model: simlin.Model) -> None:
    project = model.project
    assert project is not None
    heard: list[ChangeEvent] = []
    project.on_change(heard.append)
    session = ToolSession(model)
    session.call("read_model")
    assert model.get_variable("spare") is None
    revision = project.revision

    edit = session.call("edit_model", ADD_SPARE)
    assert not edit.is_error, edit.data
    assert not edit.is_error, edit.data
    assert model.get_variable("spare") is not None, "the model reads as edited"
    assert project.revision == revision + 1, "an edit is one change of the project"
    assert [(event.source, event.revision) for event in heard] == [("edit", revision + 1)]

    # The session knows its own edit: its next edit needs no new read.
    again = session.call(
        "edit_model",
        {
            "summary": "Change it.",
            "operations": [{"op": "set_equation", "variable": "spare", "equation": "4"}],
        },
    )
    assert not again.is_error, again.data
    assert project.revision == revision + 2


def test_a_refused_edit_commits_nothing(model: simlin.Model) -> None:
    project = model.project
    assert project is not None
    heard: list[ChangeEvent] = []
    project.on_change(heard.append)
    session = ToolSession(model)
    outline = session.call("read_model").data
    constant = outline["constants"][0]["name"]
    revision = project.revision
    before = model.get_variable(constant)

    refused = session.call(
        "edit_model",
        {
            "summary": "Break it.",
            "operations": [{"op": "set_equation", "variable": constant, "equation": "1 +"}],
        },
    )
    assert refused.is_error, refused.data
    assert refused.data["refusedEdit"]["rule"] == "errors", refused.data
    malformed = session.call("edit_model", {"notAField": 1})
    assert malformed.is_error
    assert project.revision == revision
    assert heard == []
    assert model.get_variable(constant) == before


def test_an_edit_of_a_file_backed_model_is_written_to_its_file(tmp_path: Path) -> None:
    path = tmp_path / "logistic-growth.sd.json"
    shutil.copy(LOGISTIC, path)
    model = simlin.open(path, watch=False)
    session = ToolSession(model)
    session.call("read_model")
    edit = session.call("edit_model", ADD_SPARE)
    assert not edit.is_error, edit.data
    assert not model.dirty
    assert simlin.open(path, watch=False).get_variable("spare") is not None


def test_an_edit_of_what_the_person_changed_since_the_read_is_refused(
    model: simlin.Model,
) -> None:
    session = ToolSession(model)
    outline = session.call("read_model").data
    constant = outline["constants"][0]["name"]
    with model.edit() as (current, patch):
        theirs = current[constant.lower().replace(" ", "_")]
        assert isinstance(theirs, Aux)
        patch.upsert(Aux(name=theirs.name, equation="9", units=theirs.units))
    revision = model.revision
    refusal = session.call(
        "edit_model",
        {
            "summary": "Set the constant.",
            "operations": [{"op": "set_equation", "variable": constant, "equation": "7"}],
        },
    )
    assert refusal.is_error, refusal.data
    assert "changed" in refusal.data["error"]
    assert model.revision == revision


def test_a_second_sessions_edit_appears_in_the_first_sessions_change_report(
    model: simlin.Model,
) -> None:
    session = ToolSession(model)
    assert session.changes() is None
    session.call("read_model")
    other = ToolSession(model)
    other.call("read_model")
    edit = other.call("edit_model", ADD_SPARE)
    assert not edit.is_error, edit.data
    changes = session.changes()
    assert changes is not None
    assert changes["added"] == ["spare"]
    assert other.changes() is None, "a session's own edit is not news to it"


def _patch_adding(model: simlin.Model, name: str) -> bytes:
    """The JSON of a patch that adds the constant ``name`` to ``model``."""
    builder = ModelPatchBuilder(model._name)
    builder.upsert(Aux(name=name, equation="3"))
    patch = JsonProjectPatch(models=[builder.build()])
    return json.dumps(converter.unstructure(patch)).encode("utf-8")


def test_a_call_that_changed_the_project_in_the_engine_is_committed(tmp_path: Path) -> None:
    """``Project._commit_tool_edit`` commits exactly when the engine's
    contents revision moved during the call, whatever the call answered: the
    revision moves once, the models' caches are dropped, a file-backed
    project is written, subscribers hear of it, and the answer comes back."""
    path = tmp_path / "logistic-growth.sd.json"
    shutil.copy(LOGISTIC, path)
    model = simlin.open(path, watch=False)
    project = model.project
    assert project is not None
    heard: list[ChangeEvent] = []
    project.on_change(heard.append)
    assert model.get_variable("spare") is None
    revision = project.revision

    def engine_edit() -> str:
        # The engine's own patch entry point, with none of the project's
        # bookkeeping: what an edit tool's call does inside the FFI.
        apply_patch_json(project._ptr, _patch_adding(model, "spare"), False, False)
        return "answered"

    assert project._commit_tool_edit(engine_edit) == "answered"
    assert project.revision == revision + 1
    assert [(event.source, event.revision) for event in heard] == [("edit", revision + 1)]
    assert model.get_variable("spare") is not None
    assert not project.dirty
    assert simlin.open(path, watch=False).get_variable("spare") is not None


def test_a_call_that_changed_nothing_in_the_engine_commits_nothing(model: simlin.Model) -> None:
    project = model.project
    assert project is not None
    heard: list[ChangeEvent] = []
    project.on_change(heard.append)
    revision = project.revision
    assert project._commit_tool_edit(lambda: "answered") == "answered"

    def raising() -> None:
        raise SimlinRuntimeError("the call failed")

    with pytest.raises(SimlinRuntimeError):
        project._commit_tool_edit(raising)
    assert project.revision == revision
    assert heard == []


def test_a_records_inputs_hold_every_incoming_link() -> None:
    """``Model.get_incoming_links`` and the tools' ``inputs`` are two readings
    of one dependency set: every incoming link is an input, and the inputs
    beyond them are a stock's flows (which move it, and are in no equation)
    and the tables a variable looks up."""
    model = simlin.load(WORLD3)
    session = ToolSession(model)
    outline = session.call("read_model").data
    assert outline["counts"]["lookups"] > 0
    assert outline["counts"]["stocks"] > 0
    names = model.get_var_names()
    kinds: dict[str, str] = {}
    inputs: dict[str, set[str]] = {}
    flows: dict[str, set[str]] = {}
    for start in range(0, len(names), 12):
        answer = session.call("read_variables", {"names": names[start : start + 12]}).data
        assert not answer.get("omitted"), answer
        assert not answer.get("notFound"), answer
        for record in answer["variables"]:
            name = _canonical(record["name"])
            assert "moreInputs" not in record, name
            kinds[name] = record["kind"]
            inputs[name] = {_canonical(link["name"]) for link in record.get("inputs", [])}
            flows[name] = {
                _canonical(flow) for flow in record.get("inflows", []) + record.get("outflows", [])
            }
    assert set(inputs) == set(names)
    beyond = 0
    for name in names:
        incoming = set(model.get_incoming_links(name))
        assert incoming <= inputs[name], (name, incoming - inputs[name])
        for extra in inputs[name] - incoming:
            beyond += 1
            assert extra in flows[name] or kinds[extra] == "lookup", (name, extra)
    assert beyond > 0, "World3 has stocks with flows and variables that look tables up"


def _canonical(name: str) -> str:
    return name.lower().replace(" ", "_")


def test_an_interrupted_call_is_told_from_a_refusal() -> None:
    stopped = ToolOutput(data={"error": "the call stopped", "interrupted": True}, is_error=True)
    refused = ToolOutput(data={"error": "no variable 'x'"}, is_error=True)
    answered = ToolOutput(data={"interrupted": True}, is_error=False)
    assert stopped.interrupted
    assert not refused.interrupted
    assert not answered.interrupted


def test_a_cancelled_call_is_told_from_an_interrupted_one() -> None:
    cancelled = ToolOutput(data={"error": "the host cancelled", "cancelled": True}, is_error=True)
    interrupted = ToolOutput(data={"error": "the call stopped", "interrupted": True}, is_error=True)
    assert cancelled.cancelled
    assert not cancelled.interrupted
    assert not interrupted.cancelled


def _file_backed(tmp_path: Path) -> tuple[simlin.Model, simlin.Project, Path]:
    path = tmp_path / "logistic-growth.sd.json"
    shutil.copy(LOGISTIC, path)
    model = simlin.open(path, watch=False)
    project = model.project
    assert project is not None
    return model, project, path


def test_an_edit_the_engine_made_is_committed_whatever_the_call_raised(tmp_path: Path) -> None:
    """A KeyboardInterrupt surfaces as the FFI call returns, after the engine
    made the edit: the project still commits it (revision, caches, file,
    subscribers), then lets the interrupt continue, so a widget snapshot made
    at the old revision is stale rather than accepted over the edit."""
    model, project, path = _file_backed(tmp_path)
    session = ToolSession(model)
    session.call("read_model")
    heard: list[ChangeEvent] = []
    project.on_change(heard.append)
    cached = model.base_case
    on_disk = path.read_bytes()
    revision = project.revision
    engine_call = session._call

    def interrupted_as_it_returns(tool: str, payload: bytes) -> ToolOutput:
        engine_call(tool, payload)
        raise KeyboardInterrupt

    session._call = interrupted_as_it_returns  # type: ignore[method-assign]
    with pytest.raises(KeyboardInterrupt):
        session.call("edit_model", ADD_SPARE)
    assert model.get_variable("spare") is not None
    assert project.revision == revision + 1
    assert [(event.source, event.revision) for event in heard] == [("edit", revision + 1)]
    assert not project.dirty
    assert path.read_bytes() != on_disk
    assert model.base_case is not cached, "the run made before the edit is dropped"
    assert not project._apply_snapshot(on_disk, revision), "an old snapshot is stale"
    assert model.get_variable("spare") is not None


def test_a_patch_the_engine_applied_is_committed_whatever_the_call_raised(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The same rule for ``model.edit()``: the engine's patch entry point
    applies the patch and the interrupt surfaces as it returns."""
    import simlin.project as project_module

    model, project, path = _file_backed(tmp_path)
    heard: list[ChangeEvent] = []
    project.on_change(heard.append)
    cached = model.base_case
    on_disk = path.read_bytes()
    revision = project.revision
    applied = project_module._ffi_apply_patch_json

    def interrupted_as_it_returns(*args: Any) -> Any:
        applied(*args)
        raise KeyboardInterrupt

    monkeypatch.setattr(project_module, "_ffi_apply_patch_json", interrupted_as_it_returns)
    with pytest.raises(KeyboardInterrupt), model.edit() as (_, patch):
        patch.upsert(Aux(name="spare", equation="3"))
    assert model.get_variable("spare") is not None
    assert project.revision == revision + 1
    assert [(event.source, event.revision) for event in heard] == [("edit", revision + 1)]
    assert not project.dirty
    assert path.read_bytes() != on_disk
    assert model.base_case is not cached
    assert not project._apply_snapshot(on_disk, revision)


def test_a_rejected_patch_that_raises_commits_nothing(model: simlin.Model) -> None:
    project = model.project
    assert project is not None
    heard: list[ChangeEvent] = []
    project.on_change(heard.append)
    revision = project.revision
    with pytest.raises(SimlinRuntimeError), model.edit() as (_, patch):
        patch.upsert(Aux(name="broken", equation="nowhere * 2"))
    assert project.revision == revision
    assert heard == []


class _WatchedLock:
    """``Project._file_lock`` with an event set when a thread other than the
    one that made it starts to wait for it: how a test knows an edit is
    queued on the project's locks without sleeping."""

    def __init__(self, lock: Any) -> None:
        self._lock = lock
        self._owner = threading.get_ident()
        self.waited = threading.Event()

    def __enter__(self) -> bool:
        if threading.get_ident() != self._owner:
            self.waited.set()
        return bool(self._lock.__enter__())

    def __exit__(self, *exc: object) -> None:
        self._lock.__exit__(*exc)

    def acquire(self, blocking: bool = True, timeout: float = -1) -> bool:
        return bool(self._lock.acquire(blocking, timeout))

    def release(self) -> None:
        self._lock.release()


def test_a_cancel_covers_an_edit_waiting_for_the_projects_locks(model: simlin.Model) -> None:
    """An edit waits for the project's locks in Python, before the engine's
    ticket: the session's own ticket covers it, and it answers the engine's
    cancelled refusal without being made."""
    project = model.project
    assert project is not None
    session = ToolSession(model)
    session.call("read_model")
    revision = project.revision
    watched = _WatchedLock(project._file_lock)
    project._file_lock = watched  # type: ignore[assignment]
    answers: list[ToolOutput] = []
    with watched:
        # Another of the project's edits, a save or a reload holds the lock.
        queued = threading.Thread(
            target=lambda: answers.append(session.call("edit_model", ADD_SPARE))
        )
        queued.start()
        assert watched.waited.wait(timeout=30), "the edit waits for the project's locks"
        session.cancel()
    queued.join(timeout=60)
    (answer,) = answers
    assert answer.cancelled, answer.data
    assert answer.is_error, answer.data
    assert answer.data == tools_module._cancelled().data
    assert project.revision == revision
    assert model.get_variable("spare") is None
    after = session.call("edit_model", ADD_SPARE)
    assert not after.is_error, "an edit made after the cancel is made"


def test_an_edit_holds_the_projects_locks_across_the_engine_call(model: simlin.Model) -> None:
    """Nothing else edits the project between the engine's edit and its
    commit here: during the engine call both of the project's locks are
    held, so another thread can take neither."""
    project = model.project
    assert project is not None
    session = ToolSession(model)
    session.call("read_model")
    engine_call = session._call
    free: dict[str, bool] = {}

    def observed(tool: str, payload: bytes) -> ToolOutput:
        def try_locks() -> None:
            for name in ("_file_lock", "_lock"):
                lock = getattr(project, name)
                got = lock.acquire(blocking=False)
                if got:
                    lock.release()
                free[name] = got

        probe = threading.Thread(target=try_locks)
        probe.start()
        probe.join()
        return engine_call(tool, payload)

    session._call = observed  # type: ignore[method-assign]
    assert not session.call("edit_model", ADD_SPARE).is_error
    assert free == {"_file_lock": False, "_lock": False}


def test_an_edit_whose_autosave_fails_raises_a_write_error_with_the_answer(
    tmp_path: Path,
) -> None:
    model, project, _ = _file_backed(tmp_path)
    session = ToolSession(model)
    session.call("read_model")
    heard: list[ChangeEvent] = []
    project.on_change(heard.append)
    revision = project.revision

    def failing_write(*args: Any, **kwargs: Any) -> None:
        raise OSError("disk full")

    project._write_to = failing_write  # type: ignore[method-assign]
    with pytest.raises(SimlinWriteError) as raised:
        session.call("edit_model", ADD_SPARE)
    error = raised.value
    assert error.revision == revision + 1
    assert error.write_failed
    assert isinstance(error.__cause__, OSError)
    assert isinstance(error.answer, ToolOutput)
    assert not error.answer.is_error
    assert error.answer.data["changes"][0]["variable"] == "spare"
    assert project.dirty, "save() retries the write"
    assert model.get_variable("spare") is not None
    assert [(event.source, event.revision) for event in heard] == [("edit", revision + 1)]


@pytest.mark.parametrize("effect", ["edit", "read"])
def test_a_tool_is_committed_as_an_edit_by_its_catalog_effect_not_its_name(
    model: simlin.Model, monkeypatch: pytest.MonkeyPatch, effect: str
) -> None:
    project = model.project
    assert project is not None
    routed: list[str] = []
    commit = project._commit_tool_edit

    def spy(call: Any) -> Any:
        routed.append("commit")
        return commit(call)

    monkeypatch.setattr(project, "_commit_tool_edit", spy)
    monkeypatch.setattr(tools_module, "_tool_effects", lambda: {"edit_model": effect})
    session = ToolSession(model)
    session.call("read_model")
    session.call("edit_model", ADD_SPARE)
    assert routed == (["commit"] if effect == "edit" else [])
