# Plans

This directory contains implementation plans for significant work items.

- `active/` -- Plans currently being implemented
- `completed/` -- Finished plans

For MDL parser design history, see [doc/design/mdl-parser.md](/doc/design/mdl-parser.md).

## Template

When creating a new plan, use this template:

```markdown
# Plan: [Title]

**Status**: active | completed | abandoned
**Created**: YYYY-MM-DD
**Owner**: [name]
**Last reviewed**: YYYY-MM-DD

## Goal

## Context

## Approach

## Open Questions

## Outcome (filled in when completed)
```

## Conventions

- Plans start as `active/plan-name.md`
- When completed, move to `completed/` and fill in the Outcome section
- Review active plans monthly; mark stale plans as abandoned with an explanation
- Link to plans from `doc/tech-debt.md` when they address known debt items
