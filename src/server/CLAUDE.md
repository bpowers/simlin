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
- `project-creation.ts` -- Project creation logic
- `new-user.ts` -- New user handling
- `server-init.ts` -- Server initialization
- `route-handlers.ts` -- Route handler utilities
- `render.ts` -- Server-side PNG rendering (delegates to the engine WASM)
- `models/` -- Database interfaces (Firestore, etc.)
- `schemas/` -- Data validation schemas
