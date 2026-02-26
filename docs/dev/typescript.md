# TypeScript/React Development Standards

## Code Style

- Use TypeScript with strict mode enabled.
- Prefer class components by default. Hooks are allowed when wrapping/integrating with components that only support hook-based APIs; in all other cases, prefer classes.
- Use proper TypeScript types, avoid `any`.
- NEVER manually copy files around to get builds or tests passing. Identify the root cause and fix the build scripts.

## Testing

- Target 95%+ code coverage for new code.
- Follow the functional core, imperative shell pattern to ensure as much logic as possible is in easily testable pure functions.
