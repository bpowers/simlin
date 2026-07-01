# @simlin/server

Express.js backend API. Authentication via Firebase Auth, models persisted in Firestore in protobuf form.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [docs/dev/commands.md](/docs/dev/commands.md).

## Key Files

- `app.ts` -- Express app setup and routing
- `api.ts` -- API endpoint handlers
- `authn.ts` -- Firebase authentication middleware (login route + session wiring)
- `authz.ts` -- Authorization logic
- `auth-helpers.ts` -- Auth utility functions
- `session-auth.ts` -- Cookie-session helpers: reads/writes the seshcookie-backed session (which keeps the historic `session.passport.user.id` wire shape) and deserializes `req.user` per request
- `seshcookie/` -- Vendored copy of the seshcookie encrypted-cookie-session library (github.com/bpowers/seshcookie-js, relicensed to Apache-2.0 with its author's permission). simlin is its only consumer, so it lives in-tree instead of going through npm publishes; keep diffs against upstream minimal
- `logger.ts` -- Minimal structured logger (one `{level, message, timestamp}` JSON line per entry on stdout)
- `favicon.ts` -- In-memory favicon middleware
- `healthz.ts` -- Unauthenticated healthz GET route for uptime checks (200 when the WASM engine is ready; never touches Firestore). A preload failure aborts boot before the route mounts, so a broken instance surfaces as a connection failure, not a 503 -- the 503 branch is defense-in-depth
- `project-creation.ts` -- Project creation logic
- `new-user.ts` -- New user handling
- `server-init.ts` -- Server initialization
- `route-handlers.ts` -- Route handler utilities
- `render.ts` -- Server-side PNG preview orchestration: spawns a per-request `worker_threads` worker under a total wall-clock budget (`RENDER_TIMEOUT_MS`, queue wait included) and a small FIFO concurrency cap, so a slow/pathological model can't pin the Express event loop (issue #694)
- `render-worker.ts` -- Worker entry that runs the actual render pipeline (protobuf -> SVG -> PNG) on its own engine WASM instance; `renderProjectToPng` is exported for in-process tests
- `preview-geometry.ts` -- Pure preview sizing/viewBox helpers shared by render.ts (re-exported) and the worker
- `models/` -- Database interfaces (Firestore, etc.)
- `schemas/` -- Data validation schemas
