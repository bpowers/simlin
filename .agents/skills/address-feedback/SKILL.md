---
name: address-feedback-local
description: Improve the current branch's changes by getting code reviews in a loop until all feedback is addressed
---

You are performing iterative code review and improvement using local review tools.  This skill takes no arguments (if any were given, ignore).  You are operating on the local checkout.  If the current branch is `main`, your task is reviewing the changes between `origin/main..HEAD` (or `master` and `origin/master` respectively if that is the convention in the repo).

## Prerequisites (verify before starting)

1. **Feature branch**: Confirm you're NOT on `main`. If you are, checkout a new branch with an appropriate name.
2. **Clean working tree**: All changes must be committed. If there are uncommitted changes, commit them first with an appropriate message.
3. **Pushed to remote**: The branch must be pushed. Run `git push -u origin HEAD` if needed.
4. **PR exists**: You might already know the PR number (e.g. from system-reminder or in context). If not, create a PR with `gh pr create` and note the returned PR number.

## Pre-PR self-review (run BEFORE the first review iteration)

Before kicking off the review loop, read your own diff with reviewer eyes. Reviewer rounds tend to find the same patterns repeatedly; getting them in your first commit saves an iteration cycle. Run:

```bash
git diff origin/main...HEAD
```

and walk it with these questions in mind:

1. **Contract changes**: For every new function, ask "what is its contract? what edge violates it?" Then write a test for that edge if one doesn't exist. Specifically:
   - Path-resolution / canonicalization functions: probe symlink escapes, `..` traversals, case-folding, missing leaves vs missing intermediate components.
   - Validation gates: probe the case where the gate's input *is* the to-be-validated state (a real shape of bug — "validate against new content" is a no-op).
   - Race-sensitive primitives (atomic create, optimistic lock, echo suppression): probe N concurrent callers.

2. **Consumer audit on contract changes**: If the PR changes a contract that has multiple consumers (e.g. a path-resolution rule, a registry primitive, an auth check), grep for every other consumer of the affected primitive. Apply the same change everywhere — DO NOT fix one site and ship.
   ```bash
   # Example: changed how `.mdl` paths resolve. Find every consumer.
   git grep -nE 'mdl|sidecar' src/
   ```
   The test for whether you've finished: every consumer should be calling the same primitive, not reimplementing the rule inline.

3. **Threat-model alignment**: If the PR touches auth, transport, path validation, file-creation, or anything described in `docs/threat-model.md`, cross-reference each promise against your code. The threat model is a contract; if your PR's code doesn't match it, either fix the code or update the threat model — but don't ship divergence silently.

4. **N≥3 duplication smell**: If the same conditional / helper / rule appears in 3+ places, refactor before merging. This is the source of the "next round of bugs" — every duplicated rule eventually drifts.

If you find issues during this pass, fix them with TDD before pushing. The review loop below should be addressing things you genuinely missed, not things visible to anyone reading the diff.

## Main Loop

Execute this loop until the reviewer reports no actionable feedback in the same iteration:

### Step 0: Sync with origin/main

Run at the START of every iteration:
```bash
git fetch origin && git merge-base --is-ancestor origin/main HEAD && echo "origin/main is ancestor" || echo "diverged or behind"
```

If the current branch has diverged from or is behind `origin/main`, rebase, carefully resolving merge conflicts.  After a successful rebase make sure the remote branch is updated:
```bash
git push --force-with-lease
```

### Step 1: Run review

Push the current state:
```bash
git push
```

Then run the review command and collect its output:

**Codex review** -- run with a 30-minute timeout:
```bash
codex -c 'model="gpt-5.3-codex"' -c 'model_reasoning_effort="xhigh"' exec review --json --base origin/main | tee /tmp/codex.stdout | jq -r 'select(.type=="item.completed" and .item.type=="agent_message") | .item.text'
```

### Step 2: Evaluate feedback

Think CRITICALLY about the output. Your job is to extract feedback that would GENUINELY IMPROVE the work. Implement feedback that:
- Improves correctness, robustness, or edge case handling
- Improves test coverage or test quality
- Improves code clarity or maintainability
- Fixes actual bugs or issues

Ignore suggestions that:
- Are based on misunderstanding the code or requirements
- Would add significant unnecessary complexity

If ANY feedback would genuinely improve the code:
- Think deeply about each piece of feedback
- Identify the ROOT CAUSE, not just the symptom
- **Audit consumers of the affected contract.** If the fix changes a primitive's contract (e.g. "rename now also updates format", "save now applies sidecar preference"), grep for every other call site of that primitive and verify they all behave correctly. Reviewers will find the next-leaked site if you don't — this is the single biggest source of "still finding P1s after N iterations." When fixing one consumer, find them all.
- Follow Test-Driven Development:
  1. Write failing test(s) that capture the expected behavior.  If there is refactoring needed to enable writing good tests that is ok -- this improves the codebase.  Test the CONTRACT, not the call site: the test should probe the edge where the contract leaks (symlink escape, format mismatch, race window, …) rather than just exercising the change.
  2. Implement the fix to make the tests pass
  3. Refactor if needed while keeping tests green
- **Re-run the pre-PR self-review pass** above against the new diff before pushing. Each iteration is an opportunity to apply the same scrutiny to the new code.
- Create ONE commit for all feedback from this review cycle
- **Go back to Step 0** (both reviewers must re-verify after changes)

**Deferred feedback**: Suggestions that are valid but out of scope for this PR (like a pre-existing design limitation) must NOT be silently dropped. For each such item, use `gh` to create a new issue with a detailed description of the concern.

Only if there is ZERO actionable feedback should you proceed to Step 3.

### Step 3: Complete

Reviewer found no actionable issues in the same iteration. The review cycle is complete.

1. Ensure all changes are pushed:
   ```bash
   git push
   ```

2. Post a PR comment summarizing the improvements made during this review cycle. Use `gh pr comment <PR_NUMBER> --body "..."`. The summary should be:
   - 1-2 paragraphs describing the high-level changes and improvements
   - NOT a concatenation of commit messages
   - Focus on what was improved and why it matters
   - Mention the number of review iterations if more than one


## Important Guidelines

- NEVER skip feedback because it seems minor - if it improves the code, address it
- NEVER implement fixes without corresponding tests
- Each commit should be atomic: all fixes from one review batch together
- If codex and claude give conflicting feedback, prefer codex's guidance
- There is no iteration limit - continue until all feedback is exhausted
- The review loop requires BOTH automated reviewers to be clean in the SAME iteration before completing
