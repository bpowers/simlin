# @simlin/server

Express.js backend API. Authentication via Firebase Auth, models persisted in Firestore in protobuf form.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [docs/dev/commands.md](/docs/dev/commands.md).

## Key Files

- `app.ts` -- Express app setup and routing
- `api.ts` -- API endpoint handlers. The save and create paths fast-fail before persisting anything (stale version / taken name), write the File before the conditional project update, and share a two-guard orphan cleanup on both conflict and transport-failure outcomes: never delete a File another request wrote, and never one the winning project row references (identical same-millisecond saves share a file id). A rejected conditional update is a transport failure and surfaces as a 500, never a 409
- `authn.ts` -- Firebase authentication middleware (login route + session wiring). Every POST /session failure answers the API's JSON `{error}` envelope with a user-renderable message (raw Firebase/DB detail goes only to the log); the previous plain-text `sendStatus(500)` made the client's `response.json()` throw and masked the real failure (issue #927)
- `authz.ts` -- Authorization middleware: authentication evidence is the deserialized `req.user` via `isAuthenticated`, never the raw session shape (issue #930). The anonymous carve-out is an anchored regex admitting exactly GET /projects/:username/:projectName, mirroring Express's dispatch semantics so the carve-out and the router can't disagree (a bare prefix match also admitted the trailing-slash alias of the authenticated list route)
- `auth-helpers.ts` -- Auth utility functions
- `session-auth.ts` -- Cookie-session helpers: reads/writes the seshcookie-backed session (which keeps the historic `session.passport.user.id` wire shape) and deserializes `req.user` per request. A session naming a since-deleted user is treated as unauthenticated and emptied, so seshcookie expires the dead cookie instead of it coming back on every request (issue #930); `isAuthenticated(req)` -- true only when the session resolved to a live user record -- is the authentication predicate authz consumes
- `seshcookie/` -- Vendored copy of the seshcookie encrypted-cookie-session library (github.com/bpowers/seshcookie-js, relicensed to Apache-2.0 with its author's permission). simlin is its only consumer, so it lives in-tree instead of going through npm publishes; keep diffs against upstream minimal
- `logger.ts` -- Minimal structured logger (one `{level, message, timestamp}` JSON line per entry on stdout)
- `favicon.ts` -- In-memory favicon middleware
- `healthz.ts` -- Unauthenticated healthz GET route for uptime checks (200 when the WASM engine is ready; never touches Firestore). A preload failure aborts boot before the route mounts, so a broken instance surfaces as a connection failure, not a 503 -- the 503 branch is defense-in-depth
- `project-creation.ts` -- Project creation logic
- `new-user.ts` -- New user handling
- `server-init.ts` -- Server initialization
- `route-handlers.ts` -- Route handler utilities
- `render.ts` -- Server-side PNG preview orchestration: spawns a per-request `worker_threads` worker under a total wall-clock budget (`RENDER_TIMEOUT_MS`, queue wait included) and a small FIFO concurrency cap, so a slow/pathological model can't pin the Express event loop (issue #694). A queued waiter's deadline is enforced by its own timer, which removes it from the queue at expiry (issue #929): the request rejects promptly and a later freed slot goes to a live waiter instead of being burned on a dead one
- `render-worker.ts` -- Worker entry that runs the actual render pipeline (protobuf -> SVG -> PNG) on its own engine WASM instance; `renderProjectToPng` is exported for in-process tests
- `preview-geometry.ts` -- Pure preview sizing/viewBox helpers shared by render.ts (re-exported) and the worker
- `models/` -- Database interfaces (Firestore, etc.). The `Table` boundary normalizes backend errors: `create` rejects duplicate ids as `AlreadyExistsError`, and `update` returns `null` for exactly "precondition failed" (`PreconditionFailedError` is the internal sentinel) while transport failures propagate as rejections -- mapping outages to null made them read as version conflicts
- `schemas/` -- Data validation schemas
- `tests/wire-harness.ts` -- Wire-level test harness: the real middleware chain in app.ts order (seshcookie -> sessionAuth -> authz -> apiRouter, plus the HTML project route) over a Map-backed database, so tests exercise the interaction between the auth and save pieces (stale cookies, the public carve-out, version races) rather than any one unit
