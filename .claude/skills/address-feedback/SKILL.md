---
name: address-feedback
description: Improve the current branch's changes by getting code reviews in a loop until all feedback is addressed
---

You are performing iterative code review and improvement using local review tools.  This skill takes no arguments (if any were given, ignore).  You are operating on the local checkout.  If the current branch is `main`, your task is reviewing the changes between `origin/main..HEAD` (or `master` and `origin/master` respectively if that is the convention in the repo).

## Prerequisites (verify before starting)

1. **Feature branch**: Confirm you're NOT on `main`. If you are, checkout a new branch with an appropriate name.
2. **Clean working tree**: All changes must be committed. If there are uncommitted changes, commit them first with an appropriate message.
3. **Pushed to remote**: The branch must be pushed. Run `git push -u origin HEAD` if needed.
4. **PR exists**: You might already know the PR number (e.g. from system-reminder or in context). If not, create a PR with `gh pr create` and note the returned PR number.

## Main Loop

Execute this loop until BOTH reviewers report no actionable feedback in the same iteration:

### Step 0: Sync with origin/main

Run at the START of every iteration:
```bash
git fetch origin && git merge-base --is-ancestor origin/main HEAD && echo "origin/main is ancestor" || echo "diverged or behind"
```

If the current branch has diverged from or is behind `origin/main`, rebase, carefully resolving merge conflicts.  After a successful rebase make sure the remote branch is updated:
```bash
git push --force-with-lease
```

### Step 1: Run reviews

Push the current state:
```bash
git push
```

Then run both reviews and collect their output:

**Codex review** -- run with a 30-minute timeout:
```bash
./scripts/codex-review
```

**Claude review** -- launch a sub-agent that invokes the `/review` skill.  Use the Task tool with `subagent_type: "general-purpose"` and a prompt like:

> Use the Skill tool to invoke the "review" skill with argument "<PR_NUMBER>" to review PR #<PR_NUMBER>.

Wait for the sub-agent to complete and use its returned review output.

### Step 2: Evaluate feedback

Think CRITICALLY about the output. Reviewers' responses will include:
- Positive observations about the code
- Potential concerns that they may talk themselves out of
- Suggestions marked as optional or nice-to-have
- Stream-of-consciousness reasoning

Your job is to extract feedback that would GENUINELY IMPROVE the work. Implement feedback that:
- Improves correctness, robustness, or edge case handling
- Improves test coverage or test quality
- Improves code clarity or maintainability
- Fixes actual bugs or issues

Ignore suggestions that:
- Are based on misunderstanding the code or requirements
- Would add unnecessary complexity
- The reviewer convinced itself weren't actually problems

**Deferred feedback**: Only defer feedback that is genuinely unrelated to this PR's changes -- pre-existing issues in untouched code, future feature requests, or theoretical concerns about code paths this PR does not introduce or modify. For each deferred item, spawn the `track-issue` agent (via the Task tool with `subagent_type: "track-issue"`) with a detailed description.

**CRITICAL**: P0/P1/P2 feedback about code introduced or modified by THIS PR must NEVER be deferred. If a reviewer flags a correctness, data-loss, or behavioral bug in code that this branch touches, fix it in this review cycle. Deferring P1 feedback on your own changes is not acceptable -- it means shipping a known bug. When in doubt about whether feedback is "in scope", err on the side of fixing it.

If ANY feedback would genuinely improve the code:
- Think deeply about each piece of feedback
- Identify the ROOT CAUSE, not just the symptom
- Follow Test-Driven Development:
  1. Write failing test(s) that capture the expected behavior.  If there is refactoring needed to enable writing good tests that is ok -- this improves the codebase.
  2. Implement the fix to make the tests pass
  3. Refactor if needed while keeping tests green
- Create ONE commit for all feedback from this review cycle
- **Go back to Step 0** (both reviewers must re-verify after changes)

Only if there is ZERO actionable feedback should you proceed to Step 3.

### Step 3: Complete

Both reviewers found no actionable issues in the same iteration. The review cycle is complete.

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
