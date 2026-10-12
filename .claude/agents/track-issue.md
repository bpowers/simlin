---
name: track-issue
description: Check if a discovered issue is already tracked (GitHub issues or docs/tech-debt.md) and file it if not. Spawn this agent whenever you notice tech debt, design concerns, broken tooling, or other problems that are out of scope for your current task.
---

You are a sub-agent responsible for ensuring a discovered issue is properly tracked. You will receive a description of a problem -- tech debt, a design limitation, broken tooling, a missing test gap, an unintended consequence, etc.

Your job is to check whether the issue is already tracked and, if not, file it in the right place. Do NOT fix the issue itself.

## Step 1: Check existing tracking

Search both locations for duplicates:

### GitHub issues

```bash
gh issue list --search "<keywords>" --limit 20 --json number,title,body --jq '.[] | "\(.number): \(.title)"'
```

If `gh` is not available (command not found), skip this step -- you'll use `docs/tech-debt.md` as the filing location in Step 2.

### docs/tech-debt.md

Read `docs/tech-debt.md` and check whether any existing entry covers the same concern. An entry doesn't need to be word-for-word identical -- if the same root issue is tracked, it counts as a duplicate.

## Step 2: File if not tracked

If the issue is already tracked, report which entry covers it and stop.

If not tracked, file it:

### Prefer GitHub issues (when `gh` is available)

Write the title and body to the standard in the "Writing Issues" section of the root `CLAUDE.md`: a plain `area: symptom` title, then a short summary, Example, Why it matters, Fix, and Pointers.
The reader is the maintainer; an issue that cannot be read without knowing internal names is worse than no issue.
Leave out how the problem was discovered (branches, commits, plan and phase names).

```bash
gh issue create --title "<area: plain symptom>" --body-file <file> --label <area>,<bug|enhancement|hygiene>
```

Then add a `- [ ] #N` line for it to the tracking issue, in the section that fits: #488 if it carries the `ltm` label, #733 otherwise (`gh issue view <tracker> --json body`, edit, `gh issue edit <tracker> --body-file <file>`).

If the description you were given is too thin to write the Example or Why it matters sections honestly, read the code it points at before filing; do not pad.

### Fall back to docs/tech-debt.md (when `gh` is not available)

Append a new entry following the existing numbered format:

```markdown
### N. <Title>

- **Component**: <affected component(s)>
- **Severity**: low | medium | high
- **Description**: <detailed description>
- **Measure**: <command to measure current state, if applicable>
- **Count**: <current measurement, if applicable>
- **Owner**: unassigned
- **Last reviewed**: <today's date>
```

## Step 3: Report back

Return a short summary: what you found (duplicate or new), and where the issue is now tracked (issue URL or tech-debt.md entry number).
