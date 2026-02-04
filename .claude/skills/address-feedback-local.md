---
name: address-feedback-local
description: Improve the current branch's changes by getting code reviews in a loop until all feedback is addressed
---

You are performing iterative code review and improvement using local review tools.

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

### Step 1: Codex Review

Run with a 20-minute timeout:
```bash
timeout 1200 codex review --base origin/main 2>/dev/null
```

Parse the output. If the output is empty or explicitly states something like "no problems detected", proceed to Step 2.

If codex provides ANY suggestions or identifies ANY issues:
- Think deeply about each piece of feedback
- Identify the ROOT CAUSE, not just the symptom
- Follow Test-Driven Development:
  1. Write failing test(s) that capture the expected behavior.  If there is refactoring needed to enable writing good tests that is ok -- this improves the codebase.
  2. Implement the fix to make the tests pass
  3. Refactor if needed while keeping tests green
- Create ONE commit for all feedback from this codex review cycle
- Push to remote
- **Go back to Step 0**

### Step 2: Claude Review

Run claude review using the PR number (from system-reminder or from when you created the PR):
```bash
claude --dangerously-skip-permissions --model 'claude-opus-4-5-20251101' -p '/review <PR_NUMBER>'
```

Think CRITICALLY about the output. Claude's response will include:
- Positive observations about the code
- Potential concerns that it may talk itself out of
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
- Claude convinced itself weren't actually problems

If ANY feedback would genuinely improve the code:
- Follow the same TDD approach as Step 1
- Create ONE commit for all feedback from this claude review cycle
- Push to remote
- **Go back to Step 0** (codex must re-verify after changes)

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
