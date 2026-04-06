# Server Rewrite Design

## Summary

This design replaces Simlin's current Node.js/Firebase server with a single Rust binary built on Axum, backed by SQLite and deployed to fly.io. The rewrite consolidates the entire backend into the same language as the simulation engine (simlin-engine), eliminating the current WASM-based FFI boundary for server-side operations like thumbnail rendering. Firebase Auth is replaced with self-managed Google OAuth and email/password authentication using encrypted session cookies, and Firestore is replaced with a SQLite database running in WAL mode with continuous backups via Litestream.

The architecture introduces AI agent integration as a first-class capability through fly.io Sprites -- lightweight, per-user microVMs that run Claude Code with access to simlin's MCP tools and Python bindings. Concurrent edits from both human users and AI agents are merged using Loro CRDTs on the server side, with the CRDT state lazily initialized only when a project first enters an agent session. Three new standalone Rust libraries (seshcookie-rs, sprites-rs, claude-agent-rs) are required before the server can be built. The work is phased across eight stages, starting with a deployable scaffold and culminating in production hardening, with each phase building on the previous one's infrastructure.

## Definition of Done

A design document that specifies the new Simlin server architecture -- server language (Go vs Rust), database (Postgres, SQLite, etc.), and hosting platform (GCP, AWS, or fly.io) -- with justified rationale for each choice.

**Success criteria:**
- Each choice is evaluated from first principles against the others, not just incremental improvement
- Architecture supports: Google OAuth + email/password auth, AI assistant integration as a core feature, small public service scale
- Cold start problem is structurally eliminated
- Firebase is fully replaced (both Auth and Firestore)
- No design choices that would block future data migration from the current system
- Clear enough to serve as the basis for an implementation plan

**Out of scope:** Data migration plan (separate future work), frontend changes, CI/CD details, detailed implementation tasks.

## Acceptance Criteria

### server-rewrite.AC1: First-principles architecture evaluation
- **server-rewrite.AC1.1 Success:** Design document evaluates Go vs Rust with explicit trade-offs and justified selection
- **server-rewrite.AC1.2 Success:** Design document evaluates SQLite vs PostgreSQL with explicit trade-offs and justified selection
- **server-rewrite.AC1.3 Success:** Design document evaluates GCP vs AWS vs fly.io with explicit trade-offs and justified selection

### server-rewrite.AC2: Authentication
- **server-rewrite.AC2.1 Success:** Google OAuth login round-trip completes and creates user session
- **server-rewrite.AC2.2 Success:** Email/password registration creates user with hashed password
- **server-rewrite.AC2.3 Success:** Email/password login returns encrypted session cookie
- **server-rewrite.AC2.4 Failure:** Invalid Google OAuth token returns 401
- **server-rewrite.AC2.5 Failure:** Wrong password returns 401 with generic error (no password hint)
- **server-rewrite.AC2.6 Failure:** Requests to protected endpoints without session cookie return 401
- **server-rewrite.AC2.7 Edge:** Temp user (temp-uuid) can claim a username; claimed username cannot be re-claimed

### server-rewrite.AC3: AI assistant integration
- **server-rewrite.AC3.1 Success:** Creating an agent session provisions a sprite with Claude Code, Python venv + pysimlin, and simlin-mcp
- **server-rewrite.AC3.2 Success:** User can send prompts and receive Claude Code responses through the browser WebSocket
- **server-rewrite.AC3.3 Success:** Claude Code can use simlin MCP tools to read and edit the model inside the sprite
- **server-rewrite.AC3.4 Success:** Agent model edits propagate to browser via Loro sync within seconds
- **server-rewrite.AC3.5 Success:** Sprite auto-sleeps when idle and wakes on reconnection with state preserved
- **server-rewrite.AC3.6 Failure:** Non-owner cannot connect to another user's agent session

### server-rewrite.AC4: Cold start elimination
- **server-rewrite.AC4.1 Success:** Server responds to first request within 200ms (no cold start)
- **server-rewrite.AC4.2 Success:** Server process is always running on the Fly Machine (not scale-to-zero)

### server-rewrite.AC5: Firebase fully replaced
- **server-rewrite.AC5.1 Success:** No Firebase SDK or Google Cloud SDK dependencies in the server
- **server-rewrite.AC5.2 Success:** All data persisted in SQLite (users, projects, previews, agent sessions)
- **server-rewrite.AC5.3 Success:** Authentication is self-managed (OAuth + email/password, not Firebase Auth)

### server-rewrite.AC6: Migration compatibility
- **server-rewrite.AC6.1 Success:** Database schema preserves field names and semantics from current Firestore collections
- **server-rewrite.AC6.2 Success:** Project data round-trips correctly: current protobuf can be converted to canonical JSON and stored

### server-rewrite.AC7: Concurrent editing via Loro
- **server-rewrite.AC7.1 Success:** Two concurrent saves (browser + agent) merge without data loss
- **server-rewrite.AC7.2 Success:** Loro state serializes to DB and deserializes correctly across server restarts
- **server-rewrite.AC7.3 Edge:** Project without crdt_state (never touched by agent) still serves correctly as plain JSON

### server-rewrite.AC8: Project CRUD and thumbnails
- **server-rewrite.AC8.1 Success:** Authenticated user can create, list, read, and update projects
- **server-rewrite.AC8.2 Success:** Public projects are readable without authentication
- **server-rewrite.AC8.3 Success:** PNG thumbnail is generated on project save via simlin-engine
- **server-rewrite.AC8.4 Failure:** Stale version save (optimistic lock) returns conflict error
- **server-rewrite.AC8.5 Failure:** Non-owner cannot save to another user's project

## Glossary

- **Axum**: A Rust web framework built on top of Tower and Tokio. It provides type-safe routing, middleware composition, and async request handling. Version 0.8 is specified in this design.
- **Tower**: A Rust library of modular, composable middleware components for networking services. Axum's middleware stack (CORS, tracing, body limits, custom layers) is built on Tower.
- **Tokio**: The async runtime for Rust. Provides the event loop, task scheduling, and async I/O that Axum and all async Rust code in this server depend on.
- **WAL mode (Write-Ahead Logging)**: A SQLite journaling mode where writes go to a separate log file before being checkpointed into the main database. Enables concurrent readers alongside a single writer and significantly improves write throughput compared to the default rollback journal.
- **Litestream**: An open-source tool that continuously replicates a SQLite database by streaming its WAL changes to an S3-compatible object store. It runs as a wrapper process around the application, providing point-in-time recovery without application-level backup logic.
- **Tigris**: An S3-compatible object storage service available on fly.io. Used here as the replication target for Litestream backups of the SQLite database.
- **fly.io**: A platform for running applications on lightweight virtual machines (Fly Machines) close to users. Provides persistent volumes, automatic TLS, and the Sprites API used for AI agent sandboxing.
- **Fly Machine**: A single virtual machine instance on fly.io. This design uses a shared-cpu-2x machine with 512MB RAM and a persistent volume, configured to run continuously (no scale-to-zero) to eliminate cold starts.
- **Sprites (fly.io)**: fly.io's API for creating isolated microVMs that can be attached to a parent Fly Machine. Each sprite has its own filesystem, can run arbitrary processes, supports WebSocket-based stdin/stdout streaming, auto-sleeps when idle, and wakes on demand with state preserved. Used here to sandbox per-user Claude Code sessions.
- **Loro**: A CRDT library that enables concurrent edits to structured documents without conflicts. In this design, Loro runs server-side to merge edits from browser users and AI agents, with the CRDT binary state persisted alongside the materialized JSON.
- **CRDT (Conflict-free Replicated Data Type)**: A data structure that can be independently edited by multiple parties and merged deterministically without coordination or conflict resolution logic. Loro implements this for JSON-like document structures.
- **MCP (Model Context Protocol)**: A protocol for exposing tools and resources to AI assistants. Simlin's MCP server (`@simlin/mcp`) gives Claude Code structured access to read, edit, and create simulation models. Uses JSON-RPC 2.0 over stdio.
- **NDJSON (Newline-Delimited JSON)**: A format where each line is a complete JSON object. Used as the message framing protocol for Claude Code's subprocess communication -- the server bridges this stream between the browser WebSocket and the sprite exec API.
- **seshcookie**: A pattern (with existing Node.js and Go implementations) for storing session data entirely in an encrypted cookie. No server-side session store is needed -- the cookie itself contains the encrypted session payload. The Rust implementation (seshcookie-rs) is a new library required by this design.
- **OIDC (OpenID Connect)**: An identity layer on top of OAuth 2.0 that provides standardized user authentication. The `openidconnect` Rust crate implements the relying party side, handling Google's discovery document, token exchange, and ID token verification.
- **Argon2**: A memory-hard password hashing algorithm. Used here via the `argon2` Rust crate for hashing and verifying email/password credentials.
- **rust-embed**: A Rust crate that embeds files (here, the React SPA build output) directly into the compiled binary at build time. Enables single-binary deployment with no external static file dependencies.
- **Optimistic locking**: A concurrency control strategy where writes include a version number. The write succeeds only if the stored version matches, preventing stale overwrites. Used here via the `version` column on projects.
- **Canonical JSON**: A JSON serialization where object keys are sorted deterministically (via `BTreeMap` or custom serializer). Ensures that semantically identical documents produce byte-identical output.
- **simlin-engine**: Simlin's core Rust crate that compiles, type-checks, and simulates system dynamics models. In the current Node.js server it is accessed through a WASM build; in the new Rust server it is linked directly as a crate dependency.
- **pysimlin**: Python bindings to the Simlin simulation engine. Installed inside sprites so that Claude Code can run simulations and analyze model data using Python.
- **Temp user**: A user account with an ID prefixed `temp-` (e.g., `temp-{uuid}`) created when someone first authenticates via OAuth before choosing a username. The user later "claims" a permanent username, which replaces the temp ID.

## Architecture

### Chosen Stack

**Rust (Axum) + SQLite + fly.io + Sprites**

| Dimension | Choice | Rationale |
|-----------|--------|-----------|
| Language | Rust (Axum 0.8) | Language consolidation with the rest of the monorepo. Direct `simlin-engine` integration as a crate dependency -- no FFI, no JSON round-trips, no vendored copy maintenance. |
| Database | SQLite (WAL mode) | Operational simplicity for single-machine deployment. Zero separate service to manage. Handles thousands of writes/sec in WAL mode -- far beyond the needs of tens to hundreds of users. Litestream provides continuous backup to Tigris with point-in-time recovery. |
| Hosting | fly.io (Fly Machine) | Eliminates cold starts (always-on machine). Sprites provide purpose-built AI agent sandboxing -- no other platform offers an equivalent. Persistent volumes for SQLite, automatic TLS, simple deployment. |
| AI Sandboxing | fly.io Sprites | Per-user isolated microVMs for Claude Code sessions. Auto-sleep when idle (no cost). Persistent filesystem across sessions. WebSocket exec API provides transparent stdin/stdout streaming to processes inside the sprite. |
| Collaboration | Loro CRDTs (server-side, Phase 1) | Resolves concurrent edits between browser users and AI agents without conflict. Browser sends full JSON saves; server diffs against Loro state. Sets foundation for real-time multi-user collaboration later. |

### System Components

```
                         ┌─────────────────────┐
                         │   React SPA (client) │
                         └──────────┬──────────┘
                                    │ HTTPS / WebSocket
                         ┌──────────▼──────────┐
                         │   fly.io edge (TLS)  │
                         └──────────┬──────────┘
                                    │
              ┌─────────────────────▼─────────────────────┐
              │            Fly Machine (shared-cpu-2x)     │
              │  ┌──────────────────────────────────────┐  │
              │  │         simlin-server (Rust)          │  │
              │  │  Axum routes, auth, Loro, engine      │  │
              │  └──────────────┬───────────────────────┘  │
              │                 │                           │
              │  ┌──────────────▼───────────────────────┐  │
              │  │  SQLite (WAL) on persistent volume    │  │
              │  └──────────────┬───────────────────────┘  │
              │                 │                           │
              │  ┌──────────────▼───────────────────────┐  │
              │  │  Litestream → Tigris (S3 backups)     │  │
              │  └──────────────────────────────────────┘  │
              └────────────────────────────────────────────┘
                                    │
                     sprites HTTP/WebSocket API
                                    │
              ┌─────────────────────▼─────────────────────┐
              │          Sprites (per-user microVMs)        │
              │  Claude Code CLI + simlin MCP + pysimlin   │
              │  Python venv, project files, session state  │
              └────────────────────────────────────────────┘
```

### Data Flow

**Model editing (browser)**:
1. User edits model in React SPA
2. Client POSTs canonical JSON to `/api/projects/:owner/:name`
3. Server diffs incoming JSON against current Loro document state, applies as Loro operations
4. Server materializes updated canonical JSON from Loro, writes to `projects.contents` and `projects.crdt_state`
5. If a sprite session is active, server writes updated model JSON to the sprite's filesystem via sprites API

**Model editing (AI agent)**:
1. Agent calls simlin MCP tool (e.g., `edit_model`) inside the sprite
2. MCP server applies edit, writes updated JSON to sprite filesystem
3. MCP server POSTs updated model to server API (callback)
4. Server applies changes as Loro operations, updates DB
5. Server pushes updated state to browser via WebSocket

**Thumbnail generation**:
1. On project save, server calls `simlin-engine` directly (crate dependency) to render PNG
2. Stores PNG in `previews` table
3. Served via `GET /api/preview/:owner/:name`

### Database Schema

```sql
CREATE TABLE users (
    id              TEXT PRIMARY KEY,       -- username (e.g., "bobby")
    email           TEXT UNIQUE NOT NULL,
    display_name    TEXT,
    photo_url       TEXT,
    provider        TEXT,                   -- "google", "password"
    password_hash   TEXT,                   -- NULL for OAuth-only users
    is_admin        INTEGER NOT NULL DEFAULT 0,
    can_create_projects INTEGER NOT NULL DEFAULT 1,
    created_at      TEXT NOT NULL,          -- RFC 3339
    updated_at      TEXT NOT NULL
);

CREATE TABLE projects (
    id              TEXT PRIMARY KEY,       -- "owner/project-slug"
    owner_id        TEXT NOT NULL REFERENCES users(id),
    display_name    TEXT NOT NULL,
    is_public       INTEGER NOT NULL DEFAULT 0,
    description     TEXT,
    version         INTEGER NOT NULL DEFAULT 1,
    contents        TEXT NOT NULL,           -- materialized canonical simlin JSON
    crdt_state      BLOB,                   -- Loro binary (NULL until first agent session)
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE TABLE project_snapshots (
    project_id      TEXT NOT NULL REFERENCES projects(id),
    version         INTEGER NOT NULL,
    contents        TEXT NOT NULL,
    user_id         TEXT NOT NULL,
    created_at      TEXT NOT NULL,
    PRIMARY KEY (project_id, version)
);

CREATE TABLE previews (
    project_id      TEXT PRIMARY KEY REFERENCES projects(id),
    png             BLOB NOT NULL,
    created_at      TEXT NOT NULL
);

CREATE TABLE agent_sessions (
    id              TEXT PRIMARY KEY,
    user_id         TEXT NOT NULL REFERENCES users(id),
    sprite_name     TEXT,
    title           TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
```

Key design decisions:
- **Mutable documents, not content-addressable blobs.** Projects store current state directly. `project_snapshots` provides explicit version history. This aligns with CRDTs (a document evolving over time) rather than git's immutable-snapshot model.
- **`crdt_state` is nullable.** Projects start as plain JSON. Loro state is initialized when an agent session first touches the project.
- **`version` remains as a monotonic counter** for cache invalidation and client polling, even though Loro handles merge semantics.
- **Canonical JSON as materialized view.** `contents` is always derivable from `crdt_state` (when present). It exists for fast API responses and for projects that haven't entered the CRDT world yet.

### Server Structure

**Framework**: Axum 0.8 on Tokio.

**Middleware stack** (outer to inner):
1. Security headers (custom Tower layer)
2. Request logging (`tracing` + `tower-http::TraceLayer`)
3. Session handling (seshcookie-rs)
4. CORS (`tower-http::CorsLayer`)
5. Body size limit (`tower-http::RequestBodyLimit`, 10MB)
6. Auth extraction (reads session, resolves user, injects into request extensions)

**Route structure**:
```
POST   /api/session                     -- login (OAuth callback or email/password)
DELETE /api/session                     -- logout
GET    /api/user                        -- current user profile
PATCH  /api/user                        -- set username (temp users)
GET    /api/projects                    -- list user's projects
POST   /api/projects                    -- create project
GET    /api/projects/:owner/:name       -- get project + contents
POST   /api/projects/:owner/:name       -- save project
GET    /api/preview/:owner/:name        -- PNG thumbnail
POST   /api/agent/sessions              -- create sprite + start Claude session
GET    /api/agent/sessions              -- list user's agent sessions
WS     /api/agent/sessions/:id          -- WebSocket bridge to sprite NDJSON
GET    /healthz                         -- health check (no auth)
/*                                      -- static file serving (SPA fallback)
```

**Authentication**:
- Google OAuth via `openidconnect` crate (OIDC relying party with discovery)
- Email/password via `argon2` crate for password hashing
- Sessions via seshcookie-rs (encrypted session data in cookie, stateless)

**Static file serving**: React SPA embedded in binary via `rust-embed`. Single binary deployment. Dev mode proxies to Vite dev server.

### AI Agent Integration

**Sprite lifecycle**:
1. **Create**: Server calls sprites-rs to create sprite. Provisions Python venv with pysimlin, registers `@simlin/mcp` as MCP server for Claude Code, writes project model JSON to sprite filesystem. Can checkpoint this setup for fast future creation.
2. **Connect**: Client opens WebSocket to `/api/agent/sessions/:id`. Server opens WebSocket to sprite exec endpoint, starts Claude Code CLI. Server bridges the two connections.
3. **Sync**: Browser saves propagate to sprite via filesystem API. Agent edits propagate to server via MCP callback. Loro CRDTs handle merge.
4. **Idle/Resume**: Sprite auto-sleeps after ~30s inactivity. Wakes automatically on next API call. Filesystem persists across sleep/wake.
5. **Destroy**: User ends session, or cleanup job reaps long-idle sprites.

**What lives inside the sprite**: Claude Code CLI, Python venv with pysimlin, `@simlin/mcp`, project files, any tools/packages Claude installs during the session.

**What the server handles**: Sprite CRUD, WebSocket bridging, auth enforcement, session metadata, model sync via Loro.

### Missing Libraries (to be built separately)

Three Rust crates need to be created. Each is a standalone library:

**1. `seshcookie-rs`** -- Encrypted session cookies for Tower/Axum
- Port of the seshcookie pattern (Node.js and Go versions exist)
- AES-GCM or ChaCha20-Poly1305 encryption of session data into cookie value
- Tower middleware layer: deserialize from request cookie, serialize on response
- Configurable: cookie name, max age, secure, same-site, http-only, domain
- No server-side state -- session lives entirely in the cookie

**2. `sprites-rs`** -- Rust SDK for fly.io Sprites API
- Port of `sprites-go` (`github.com/superfly/sprites-go`)
- Sprite lifecycle (create, get, list, delete, sleep, wake)
- Exec with stdin/stdout streaming via WebSocket binary framing protocol
- `Cmd` type with `stdin()`/`stdout()`/`stderr()` as `AsyncRead`/`AsyncWrite`
- Checkpoints, services, filesystem API, TCP proxy
- Built on `reqwest` + `tokio-tungstenite`

**3. `claude-agent-rs`** -- Rust Claude Code subprocess/transport client
- Port of `go-claudecode` protocol and transport layers
- `Transport` trait abstracting over subprocess or sprite exec
- `SubprocessTransport` for local development (spawn Claude CLI directly)
- `SpriteTransport` using sprites-rs exec WebSocket (production)
- NDJSON message parsing (all message types from go-claudecode's protocol)
- Control router for hooks, MCP messages, permission requests
- Client API: one-shot `query()`/`query_sync()` and multi-turn `Client`
- In-process MCP server support

### Deployment

- **Fly Machine**: `shared-cpu-2x`, 512MB RAM, persistent volume at `/data`
- **Litestream**: Runs as wrapper process (`litestream replicate -exec "simlin-server"`), streams WAL to Tigris
- **Build**: `cargo build --release` targeting `x86_64-unknown-linux-musl` (static binary). Dockerfile: `FROM scratch` or `FROM alpine` with binary + Litestream
- **Deploy**: `fly deploy` (builds image, rolling restart)
- **DNS**: Point `app.simlin.com` to fly.io via CNAME. Automatic TLS via Let's Encrypt.
- **Sprite provisioning**: Base sprite checkpointed with Python venv + pysimlin + simlin-mcp. New sprites clone from checkpoint.
- **Monitoring**: fly.io built-in metrics + Rust `tracing` for structured logging via `fly logs`

## Existing Patterns

Investigation of the current server (`src/server/`) and praxis (`../praxis/`) revealed patterns this design follows and diverges from.

**Patterns followed:**
- **seshcookie for sessions**: Both the current Node.js server and praxis use stateless encrypted session cookies. This design continues that pattern (via seshcookie-rs).
- **Protobuf-compatible data model**: The database schema maps directly from the current Firestore collections (users, projects, files/contents, previews). Field names and semantics are preserved to avoid blocking future migration.
- **Optimistic locking via version counter**: Same pattern as the current server's `version` field on projects.
- **Content served from a single process**: Both the current server and praxis serve the SPA and API from a single binary. This design continues that.

**Patterns diverged from:**
- **Content-addressable file storage** (current server): Replaced with mutable document storage + explicit version history. The indirection (project -> file_id -> contents) added complexity without proportional benefit for a server-authoritative web app, and conflicted with the CRDT collaboration model.
- **Firebase Auth** (current server): Replaced with self-managed Google OAuth + email/password. Eliminates Firebase dependency entirely.
- **Firestore** (current server): Replaced with SQLite. Simpler operations, zero-cost database tier, better dev/prod parity, natural fit for single-machine fly.io deployment.
- **WASM engine integration** (current server's `render.ts`): Replaced with direct crate dependency. The current server uses the WASM build of simlin-engine for server-side rendering. The Rust server links the engine natively -- no WASM overhead, no separate build artifact.
- **Go for server** (praxis): Despite praxis providing a proven Go server architecture, Rust was chosen for language consolidation across the monorepo and to eliminate the FFI maintenance burden of libsimlin Go bindings.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Server Scaffold and Static Serving

**Goal:** Minimal Axum server that builds, runs, serves a placeholder page, and deploys to fly.io.

**Components:**
- New crate `simlin-server` in `src/simlin-server/` added to workspace
- Axum app with health check endpoint and static file serving via `rust-embed`
- `fly.toml` configuration for Fly Machine (shared-cpu-2x, 512MB, persistent volume)
- Dockerfile for the server binary
- Litestream configuration (`litestream.yml`) with Tigris as replication target

**Dependencies:** None (first phase)

**Done when:** `cargo build --release` produces a binary that serves a placeholder page; `fly deploy` succeeds; health check responds; Litestream replicates an empty SQLite DB to Tigris.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Database Layer and Core Data Model

**Goal:** SQLite database with schema migrations, connection pooling, and CRUD operations for users, projects, and previews.

**Components:**
- SQLite setup with WAL mode, connection pool (`r2d2` or `deadpool-sqlite`)
- Schema migration runner (embedded SQL files, applied at startup in a transaction)
- Data access layer: typed query functions for users, projects, project_snapshots, previews, agent_sessions
- Canonical JSON serialization for project contents (sorted keys via `BTreeMap` or custom serializer)

**Dependencies:** Phase 1 (server scaffold exists)

**Done when:** All tables created on startup; CRUD operations for users and projects work with tests verifying insert, read, update, optimistic lock rejection, and snapshot creation.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Authentication (seshcookie-rs + OAuth + Email/Password)

**Goal:** Users can register, log in (via Google OAuth or email/password), and maintain sessions.

**Components:**
- `seshcookie-rs` crate (standalone library, see Missing Libraries above)
- Google OAuth flow using `openidconnect` crate: redirect, callback, ID token verification, user creation/lookup
- Email/password registration and login with `argon2` password hashing
- Session middleware integration: encrypted cookie contains user ID
- Auth extraction middleware: resolves user from session, injects into request extensions
- Temp user flow: new OAuth users get `temp-{uuid}` ID until they claim a username

**Dependencies:** Phase 2 (user table exists)

**Done when:** Google OAuth login round-trip works; email/password registration and login work; sessions persist across requests; unauthenticated requests to protected endpoints return 401; temp user → username claim works.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Project API and Thumbnail Generation

**Goal:** Full project CRUD API with thumbnail generation via simlin-engine.

**Components:**
- API route handlers for all project endpoints (list, create, get, save)
- Authorization middleware (owner check, public project read access)
- Thumbnail rendering: call `simlin-engine` (crate dependency) to render PNG from model JSON
- Preview caching in `previews` table, invalidated on project save

**Dependencies:** Phase 3 (auth works)

**Done when:** Authenticated users can create, read, update projects; public projects are readable without auth; thumbnails are generated on save and served via the preview endpoint; optimistic locking rejects stale saves.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Loro Integration for Model Sync

**Goal:** Server maintains Loro CRDT documents for projects, enabling conflict-free merging of concurrent edits.

**Components:**
- Loro document lifecycle: initialize from JSON when agent session starts, maintain alongside `contents`
- JSON-to-Loro diff/apply layer: given current Loro state and incoming JSON, compute and apply Loro operations
- Loro-to-JSON materialization: produce canonical JSON from Loro document state
- `crdt_state` persistence in `projects` table (serialized Loro binary)
- WebSocket endpoint for pushing model updates to connected browser clients

**Dependencies:** Phase 4 (project save API exists)

**Done when:** Two concurrent saves (simulating browser + agent) merge correctly without data loss; Loro state round-trips through DB correctly; connected WebSocket clients receive model updates.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Sprites SDK and Agent Session Management

**Goal:** Server can create, manage, and communicate with fly.io sprites for AI agent sessions.

**Components:**
- `sprites-rs` crate (standalone library, see Missing Libraries above)
- Agent session API endpoints (create, list, connect via WebSocket)
- Sprite provisioning: create sprite, install Python venv + pysimlin, write model JSON, register simlin-mcp
- Sprite checkpointing for fast subsequent creation
- Model sync integration: browser saves push to sprite filesystem; sprite MCP callbacks push through Loro

**Dependencies:** Phase 5 (Loro sync works)

**Done when:** Creating an agent session provisions a sprite with the correct environment; model files are synced bidirectionally between server and sprite; sprites sleep/wake correctly.
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: Claude Code Integration

**Goal:** Full AI assistant integration where users can chat with Claude Code through the browser, with Claude Code having structured access to the model via MCP tools.

**Components:**
- `claude-agent-rs` crate (standalone library, see Missing Libraries above)
- WebSocket bridge: client ↔ server ↔ sprite exec (Claude Code NDJSON)
- Claude Code launched inside sprite with simlin-mcp registered and Python venv available
- Protocol translation between client-facing WebSocket format and Claude Code NDJSON
- Session reconnection: client can disconnect and reconnect to a running sprite session

**Dependencies:** Phase 6 (sprites work)

**Done when:** User can start an AI session from the browser; Claude Code responds to prompts; Claude can use simlin MCP tools to read/edit the model; model changes from the agent appear in the browser via Loro sync; sessions survive client disconnect/reconnect.
<!-- END_PHASE_7 -->

<!-- START_PHASE_8 -->
### Phase 8: Production Hardening

**Goal:** Production-ready server with security headers, rate limiting, error handling, and operational tooling.

**Components:**
- Security headers middleware (CSP, HSTS, X-Frame-Options, etc.)
- Rate limiting on auth endpoints (prevent brute force)
- Graceful shutdown (drain connections, flush Litestream)
- Error handling: structured error responses, no internal details leaked
- Example project loading for new users (port of current `new-user.ts` logic)
- DNS cutover documentation and runbook

**Dependencies:** Phase 7 (all features working)

**Done when:** Security headers present on all responses; auth endpoints rate-limited; server shuts down gracefully; new users get example projects; deployment to production fly.io succeeds with `app.simlin.com` serving traffic.
<!-- END_PHASE_8 -->

## Additional Considerations

**Data migration (out of scope, separate plan):** The current Firestore data (users, projects, files) will need a migration path. This design preserves compatible field names and semantics to minimize friction. The migration plan should be designed after the new server is functional, allowing side-by-side testing. No design choices here block migration -- the schema is a superset of the current data model.

**Future real-time collaboration (Phase 2 of Loro adoption):** The server-side Loro integration in Phase 5 is deliberately designed as a foundation. Adding browser-side Loro peers (via WASM) and real-time WebSocket sync would enable Google Docs-style collaboration. This requires no architectural changes -- only adding a Loro peer in the browser and a sync protocol over the existing WebSocket connection.

**Sprite cost management:** Sprites bill per-second of active time and auto-sleep when idle. For cost control, implement a maximum active sprites limit per user and a cleanup job that destroys sprites idle for more than N days. Sprite checkpoints reduce creation cost by avoiding redundant environment setup.
