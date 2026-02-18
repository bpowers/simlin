---
name: address-feedback-github
description: Improve the current branch's changes by getting code reviews from GitHub-based reviewers in a loop until all feedback is addressed
---

You are performing iterative code review and improvement using GitHub-based reviewers. This skill takes no arguments (if any were given, ignore). You are operating on the local checkout.

## Prerequisites (verify before starting)

1. **Feature branch**: Confirm you're NOT on `main`. If you are, checkout a new branch with an appropriate name.
2. **Clean working tree**: All changes must be committed. If there are uncommitted changes, commit them first with an appropriate message.
3. **Pushed to remote**: The branch must be pushed. Run `git push -u origin HEAD` if needed.
4. **PR exists**: You might already know the PR number (e.g. from system-reminder or in context). If not, create a PR with `gh pr create` and note the returned PR number.

## Reviewers

There are two GitHub-based reviewers:

1. **Claude auto-review** (`claude-code-review.yml`): Triggered automatically on every push. Posts a PR **comment** from the `claude` user. The check run is named `claude-review`.
2. **Codex review** (`chatgpt-codex-connector[bot]`): Triggered by posting a PR comment `@codex review`. Posts a PR **review** (with inline comments) from the `chatgpt-codex-connector[bot]` user.

## Main Loop

Execute this loop until BOTH reviewers report no actionable feedback in the same iteration:

### Step 0: Sync with origin/main

Run at the START of every iteration:
```bash
git fetch origin && git merge-base --is-ancestor origin/main HEAD && echo "origin/main is ancestor" || echo "diverged or behind"
```

If the current branch has diverged from or is behind `origin/main`, rebase, carefully resolving merge conflicts. After a successful rebase make sure the remote branch is updated:
```bash
git push --force-with-lease
```

### Step 1: Push and record baseline

Push the current state and record the HEAD commit SHA and current timestamp. These are used to identify NEW review feedback (reviews posted after this push, referencing this commit or later).

```bash
git push
PUSH_SHA=$(git rev-parse HEAD)
PUSH_TIME=$(date -u +%Y-%m-%dT%H:%M:%SZ)
```

### Step 2: Trigger Codex review

Post a comment to trigger Codex:
```bash
gh pr comment <PR_NUMBER> --body "@codex review"
```

### Step 3: Wait for Claude auto-review

The Claude auto-review is triggered by the push. Poll the `claude-review` check run until it completes:

```bash
gh pr checks <PR_NUMBER> --watch --fail-fast
```

If `--watch` is unavailable, poll manually every 60 seconds:
```bash
gh pr checks <PR_NUMBER> --json name,state | jq '.[] | select(.name == "claude-review")'
```

Wait until the `claude-review` check reaches state `SUCCESS` or `FAILURE` (both mean the review comment has been posted; `FAILURE` sometimes occurs due to non-zero exit codes even when a review was posted).

### Step 4: Collect Claude feedback

Fetch the most recent comment from the `claude` user that was posted after `$PUSH_TIME`:

```bash
gh pr view <PR_NUMBER> --comments --json comments | jq '[.comments[] | select(.author.login == "claude")] | last'
```

Parse the review body. Look for:
- A "Findings" section with `[P0]`, `[P1]`, `[P2]`, `[P3]` tagged issues
- An "Overall correctness" verdict

If the review contains NO findings (or explicitly states something like "no blocking bugs found", "no problems detected", "no findings"), Claude feedback is clean.

### Step 5: Wait for and collect Codex feedback

Poll for a new Codex review submitted after `$PUSH_TIME`:

```bash
gh api repos/{owner}/{repo}/pulls/<PR_NUMBER>/reviews | jq '[.[] | select(.user.login == "chatgpt-codex-connector[bot]")] | last'
```

If `submitted_at` is before `$PUSH_TIME`, the review hasn't arrived yet -- wait 60 seconds and retry.

Once the review arrives, fetch its inline comments:

```bash
REVIEW_ID=$(gh api repos/{owner}/{repo}/pulls/<PR_NUMBER>/reviews | jq '[.[] | select(.user.login == "chatgpt-codex-connector[bot]")] | last | .id')
gh api repos/{owner}/{repo}/pulls/<PR_NUMBER>/reviews/$REVIEW_ID/comments | jq '[.[] | {path: .path, line: .line, body: .body}]'
```

If there are ZERO inline comments, Codex feedback is clean.

Codex may also signal "no issues" by reacting with a thumbs-up emoji and posting no inline comments.

### Step 6: Evaluate feedback

Think CRITICALLY about ALL collected feedback from both reviewers.

Implement feedback that:
- Improves correctness, robustness, or edge case handling
- Improves test coverage or test quality
- Improves code clarity or maintainability
- Fixes actual bugs or issues

Ignore suggestions that:
- Are based on misunderstanding the code or requirements
- Would add unnecessary complexity
- The reviewer convinced itself weren't actually problems
- Are stylistic nits that don't improve the code

If EITHER reviewer has actionable feedback:
- Think deeply about each piece of feedback
- Identify the ROOT CAUSE, not just the symptom
- Follow Test-Driven Development:
  1. Write failing test(s) that capture the expected behavior. If there is refactoring needed to enable writing good tests that is ok -- this improves the codebase.
  2. Implement the fix to make the tests pass
  3. Refactor if needed while keeping tests green
- Create ONE commit for all feedback from this iteration
- **Go back to Step 0** (both reviewers must re-verify after changes)

Only if there is ZERO actionable feedback from BOTH reviewers should you proceed to Step 7.

### Step 7: Complete

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
- If Claude and Codex give conflicting feedback, prefer Codex's guidance
- There is no iteration limit - continue until all feedback is exhausted
- The review loop requires BOTH reviewers to be clean in the SAME iteration before completing
