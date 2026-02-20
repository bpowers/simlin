# @simlin/server

Express.js backend API. Authentication via Firebase Auth, models persisted in Firestore in protobuf form.

For global development standards, see the root [CLAUDE.md](/CLAUDE.md).
For build/test/lint commands, see [doc/dev/commands.md](/doc/dev/commands.md).

## Key Files

- `app.ts` -- Express app setup and routing
- `api.ts` -- API endpoint handlers
- `authn.ts` -- Firebase authentication middleware
- `authz.ts` -- Authorization logic
- `auth-helpers.ts` -- Auth utility functions
- `project-creation.ts` -- Project creation logic
- `new-user.ts` -- New user handling
- `server-init.ts` -- Server initialization
- `route-handlers.ts` -- Route handler utilities
- `render.ts` -- Server-side PNG rendering (delegates to the engine WASM)
- `models/` -- Database interfaces (Firestore, etc.)
- `schemas/` -- Data validation schemas
