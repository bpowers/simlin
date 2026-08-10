# Improving the Harness

The harness is everything around the model that shapes what an agent produces here: routed context (the root `CLAUDE.md`, per-package `CLAUDE.md` files, `docs/`), executable checks (`scripts/pre-commit`, CI, guard tests, repo lints), tools and scripts, skills (`.claude/skills/`, with a sibling tree for other agents in `.agents/skills/` -- a sweep that touches one must cover both), and the review loop. A harness change alters every future trajectory, so it gets the same rigor as a code change: identify the real gap, make the smallest fix at the owning boundary, and verify the fix actually fires.

This document owns two procedures: how an observed failure becomes a durable harness change, and how existing harness machinery justifies its carrying cost.

The vocabulary follows the harness-engineering practice described in [OpenAI's harness-engineering essay](https://openai.com/index/harness-engineering/) and Ryan Lopopolo's writing at [hyperbo.la](https://hyperbo.la/).

## The improvement loop

One bounded loop per change:

1. **Observe a baseline.** Start from a real trajectory -- a session transcript, a PR review cycle, a failed run. Record what actually happened, not a summary: which context was retrieved and which was missed, which tool output was misread, where a human had to relay facts or steer.
2. **Find the earliest failed handoff.** Trace the symptom upstream to the first point where the trajectory lacked something the job required, and name the boundary that should have supplied it: a routing line in a `CLAUDE.md`, a doc, a check, a tool, an API. Fixing a late symptom (adding a rule where the mistake surfaced) leaves the upstream gap for the next run.
3. **Classify the gap before choosing a fix.** A missing fact is different from a fact that exists but was not routed to; a missing capability is different from a tool that exists but is hard to discover or whose output is hard to interpret; and both are different from ordinary model variance. One failed trajectory is a lead, not a diagnosis -- do not add a permanent rule for a one-off stochastic miss.
4. **Make the smallest intervention at the owning boundary.** Prefer changes that remove a human relay or make an invariant legible at its source, and pick the strongest rung on the promotion ladder (below) that the lesson supports.
5. **Rerun fresh.** Verify in a fresh session that the intervention is retrieved and applied (next section). A successful outcome proves nothing about guidance the trajectory never used.
6. **Retain, revise, or remove.** Keep the change if the fresh run shows it firing and the gain justifies its carrying cost; revise it if the gap was real but the change is hard to retrieve or apply; remove it if it adds noise or duplicates a better owner.

## Fresh-session testing

The presence of a document proves only that it was written. New guidance is validated the way it will be consumed: a fresh session, given a task of the class the guidance targets, with no hints beyond what every future session will have. Check three things separately -- was the guidance *found* (routing works), was it *applied* (placement and wording work), and did it *change the outcome* (the gap it targets is the gap that existed).

A failure at each stage has a different fix: not found means the route from the root map is missing or the doc is in a place no task classifier reaches; found-but-ignored usually means the guidance is placed where it competes with more salient context, or states a policy without the failure it prevents; applied-but-outcome-unchanged means the diagnosis in step 3 was wrong.

## The promotion ladder

When the same correction recurs, its underlying principle needs a durable owner. Match the owner to how settled the lesson is:

| Owner | Use when |
| --- | --- |
| prompt adjustment, reroll | the task framing is still being discovered; nothing durable yet |
| routing line, doc, runbook, or skill | stable knowledge must appear at a particular decision point |
| review guidance | the judgment is qualitative or still gaining nuance |
| type, API, or tool | the system can make correct use natural and misuse hard |
| lint, test, or pre-commit/CI check | a deterministic invariant should block recurrence |
| architecture change or migration | repeated defects show the wrong owner or dependency direction |

Two rules govern movement on the ladder:

- **Promote settled lessons upward.** Prose competes for attention on every task; an executable check costs attention only when it fires, and it cannot be skimmed past. When a `CLAUDE.md` rule can be stated mechanically, turn it into a check and shrink the prose to a pointer. Several of the root `CLAUDE.md` hard rules are mid-ladder artifacts: they earn their place today as prose because the judgment is not yet mechanical, and each becomes a promotion candidate the moment it is.
- **Retire what a stronger owner makes redundant.** When a type or check now enforces an invariant, delete the prose rule and any downstream defensive checks that guarded the same thing. Layering a validator over a missing domain model preserves the incoherence and adds carrying cost.

## Fix the class, not the instance

A correction usually encodes a principle broader than the line it names. Before landing it, search the discoverable population for sibling instances governed by the same principle and fix them in the same change; then install the ratchet (lint, test, or check) that keeps the old form from returning. This is the same standard [workflow.md](workflow.md) sets for migrations: complete them. A half-migrated pattern is worse than either endpoint, because everything an agent reads is prompt material -- each surviving counterexample teaches the next trajectory the wrong continuation.

## Carrying cost and ablation

Every global instruction spends attention on every task; every check spends maintenance and loop latency on every run. A harness component is justified by a concrete failure class, and it should be possible to say what that class is -- when adding one, name it.

The harness only ever growing is itself a failure mode. When touching a doc or check, ask whether each nearby rule still earns its place: the failure class may now be covered by an executable check, impossible after a refactor, or specific to a worker limitation that no longer exists. When the answer is unclear, ablate: remove the rule on a branch, run a fresh session on a task from its failure class, and see whether the failure returns. A rule that no one can connect to a failure class is a removal candidate, not a keepsake.

## Session learnings are telemetry, not policy

Agents record self-observations while they work: session memory, notes about mistakes made, facts discovered about the environment, tools they wished existed. These are leads for the harness improver, not facts. An agent can miss its most important error, misattribute a cause, or request the wrong capability -- so corroborate a self-report against the trajectory's observable evidence (the diff, test output, review findings, the accepted or rejected outcome) before promoting it into a doc, rule, or check.

Promotion assigns ownership: a stable boundary belongs in an architecture doc, a hazardous operation in a runbook, a known failure in a test, a repeatedly rediscovered fact in a routing line near where the search started. Raw observations stay with the run that produced them. When a remembered fact contradicts the repository, the repository wins -- memory entries describe what was true when written, and they are not maintained the way checked-in docs are.
