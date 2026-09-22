"""The engine's agent tools through pysimlin: the catalog, a session's calls,
its runs, and a plan landed and committed as the project's other edits are.

What each tool answers is pinned by the engine's own tests
(``simlin_engine::tools``); these pin the binding.
"""

import threading
from pathlib import Path

import pytest

import simlin
from simlin.errors import SimlinRuntimeError
from simlin.tools import ToolOutput, ToolSession, catalog

LOGISTIC = Path(__file__).parent / "logistic-growth.sd.json"


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
        assert tool["effect"] in ("read", "plan_edit"), tool["name"]


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

    # A cancel takes no lock: it returns while a call holds the session.
    cancelling = threading.Thread(target=session.cancel)
    with session._lock:
        cancelling.start()
        cancelling.join(timeout=10)
        assert not cancelling.is_alive()


def test_a_plan_lands_and_the_project_commits_it(model: simlin.Model) -> None:
    session = ToolSession(model)
    session.call("read_model")
    plan = session.call(
        "edit_model",
        {
            "summary": "Add a named constant.",
            "operations": [{"op": "add_variable", "name": "spare", "equation": "3"}],
        },
    )
    assert not plan.is_error, plan.data
    assert plan.data["verdict"] == "ready"
    assert model.get_variable("spare") is None

    revision = model.project.revision
    landing = session.land(plan.data["plan"])
    assert landing.landed, landing.reason
    assert model.get_variable("spare") is not None
    assert model.project.revision > revision, "landing is an edit of the project"
    again = session.land(plan.data["plan"])
    assert not again.landed
    assert again.reason is not None
    assert "landed already" in again.reason
    with pytest.raises(SimlinRuntimeError):
        session.land("P99")


def test_a_plan_the_person_overtook_does_not_land(model: simlin.Model) -> None:
    session = ToolSession(model)
    outline = session.call("read_model").data
    constant = outline["constants"][0]["name"]
    plan = session.call(
        "edit_model",
        {
            "summary": "Set the constant.",
            "operations": [{"op": "set_equation", "variable": constant, "equation": "7"}],
        },
    )
    assert plan.data["verdict"] == "ready", plan.data
    person = ToolSession(model)
    person.call("read_model")
    theirs = person.call(
        "edit_model",
        {
            "summary": "The person's own value.",
            "operations": [{"op": "set_equation", "variable": constant, "equation": "9"}],
        },
    )
    assert person.land(theirs.data["plan"]).landed
    landing = session.land(plan.data["plan"])
    assert not landing.landed
    assert landing.reason is not None
    assert "changed since the plan" in landing.reason


def test_changes_since_the_last_read_follow_an_edit(model: simlin.Model) -> None:
    session = ToolSession(model)
    assert session.changes() is None
    session.call("read_model")
    other = ToolSession(model)
    other.call("read_model")
    planned = other.call(
        "edit_model",
        {
            "summary": "Add a named constant.",
            "operations": [{"op": "add_variable", "name": "spare", "equation": "3"}],
        },
    )
    other.land(planned.data["plan"])
    changes = session.changes()
    assert changes is not None
    assert changes["added"] == ["spare"]


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
