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
- Tests run on [Rstest](https://rstest.rs/), configured per package in `rstest.config.mts`. `pnpm test` runs them all; `pnpm -C src/<pkg> exec rstest run <filter>` runs a subset.
- **Test globals are off.** Import what you use: `import { describe, it, expect, rs } from '@rstest/core';`. `rs` is the mocking/timer namespace (`rs.fn`, `rs.spyOn`, `rs.mock`, `rs.useFakeTimers`, ...).
- `Mock<T>`'s type argument is the whole *function signature*, not the return type: `rs.fn<(msg: WsMessage) => void>()`. Jest's two-argument `jest.Mock<Return, Args>` has no equivalent.
- A `rs.mock` factory is hoisted above the imports, so it cannot close over one, and async factories are rejected. To keep part of the real module, pull it in with `import * as actual from 'mod' with { rstest: 'importActual' };` -- the synchronous stand-in for `jest.requireActual`. Import attributes need an ES module target, so this cannot be used in `src/server` (its program emits CommonJS and type-checks its own tests); reach for `rs.mock('mod', { spy: true })` there.
- Rstest has no `@jest-environment` docblock. A package that needs both environments declares them as `projects` in its config (see `src/diagram/rstest.config.mts`).
- jsdom + `@testing-library/react` packages need a setup file: RTL only self-registers its `afterEach(cleanup)` when `afterEach` is a global, and `waitFor` only drives fake timers when a global `jest` object exists. See `src/app/tests/setup-testing-library.ts`.
- **Anything a module under test must observe while it is being imported belongs in a `setupFiles` entry, never at the top of the test file.** A module's imports are fully evaluated before its own top-level statements run, so a `globalThis.fetch = ...` or a `TextEncoder` polyfill written above the import lands *after* the imported module already ran. This is a trap carried over from jest, whose CommonJS emit kept `require()` calls in source order and so made the misplaced version work; rspack hoists ES imports, as the spec requires. `src/app/tests/setup-fetch.ts` (App.tsx fetches `/api/user` at module scope) and `src/diagram/tests/setup-text-encoder.ts` (the engine's memory module constructs a `TextEncoder` at import time) are the two live cases.
- Test files are **not** type-checked (`isolatedModules` makes the toolchain transpile-only). Tracked in [#899](https://github.com/bpowers/simlin/issues/899).
