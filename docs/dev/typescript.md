# TypeScript/React Development Standards

## Code Style

- Use TypeScript with strict mode enabled.
- **Always write new React components as function components with hooks. Do not add class components.** The single permitted exception is an error boundary (`getDerivedStateFromError` / `componentDidCatch` have no hook equivalent); `src/diagram/ErrorBoundary.tsx` is the one class component in the codebase and new error boundaries may subclass it or follow the same shape. Everything else -- including imperative shells with subscriptions, timers, and async lifecycles -- is a function component.
- Wrap render-hot components (e.g. per-element diagram pieces) in `React.memo`, and `useCallback` any handler passed to a memo'd child so the memoization actually holds.
- **Converting a class component (or writing a function-component imperative shell), follow the established pattern** used by `Canvas.tsx` and `Editor.tsx`: instance fields move into a single mutable `refs` object (`useRef`); state that the old constructor derived from props uses a *lazy* `useState(() => ...)` initializer so it runs once per mount; escaped callbacks (listeners, timers, async continuations) read current props/state through a `latest` ref refreshed synchronously each render, never a stale render closure; `componentDidMount`/`componentWillUnmount` become one symmetric empty-deps mount effect whose cleanup undoes everything the body did, so a React 18 StrictMode mount/unmount/mount cycle leaks nothing; and `componentDidUpdate` prev-value comparisons become post-commit effects guarded by a prev-value ref so they keep "fire on change, not on mount" semantics. Objects that StrictMode's double-invoked `useState` initializer must not construct twice (e.g. a resource that opens a handle) are built in a `refs`-init guard instead.
- Use proper TypeScript types, avoid `any`.
- NEVER manually copy files around to get builds or tests passing. Identify the root cause and fix the build scripts.

## Testing

- Target 95%+ code coverage for new code.
- Follow the functional core, imperative shell pattern to ensure as much logic as possible is in easily testable pure functions.
