# TypeScript/React Development Standards

## Code Style

- Use TypeScript with strict mode enabled.
- Prefer function components with hooks. Wrap render-hot components (e.g. per-element diagram pieces) in `React.memo`, and `useCallback` any handler passed to a memo'd child so the memoization actually holds. Class components remain only where there is a concrete reason: `ErrorBoundary` (React has no hook equivalent), and large imperative shells like `Canvas`/`Editor` that are scheduled for incremental migration.
- Use proper TypeScript types, avoid `any`.
- NEVER manually copy files around to get builds or tests passing. Identify the root cause and fix the build scripts.

## Testing

- Target 95%+ code coverage for new code.
- Follow the functional core, imperative shell pattern to ensure as much logic as possible is in easily testable pure functions.
