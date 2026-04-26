# simlin-serve

Local HTTP server that discovers system-dynamics models in a directory tree and
serves them to a browser-based viewer/editor. Distributed as the `@simlin/serve`
npm package, intended to be invoked as `npx @simlin/serve` from any project
directory.

`simlin-serve` also exposes an MCP server at `/mcp` on port `7878` (override
with `--mcp-port`), so AI assistants can read, edit, simulate, and create
models in the same working directory the browser is editing. The MCP and
HTTP/UI handlers share one in-memory Loro document, so edits from either side
converge and the browser remounts the editor within ~1s of an AI edit.

## Configuring AI clients

Both clients below assume `simlin-serve` is already running in the directory
that holds your models. Start it first (e.g. `npx @simlin/serve`) and leave
it running while the AI client is connected. The MCP URL is stable across
launches because the port defaults to `7878`, so this configuration is
one-time — you don't need to update it whenever you restart the server.

### Claude Code CLI

Claude Code speaks MCP-over-HTTP natively, so no proxy is needed. Add the
server in user scope (available in every project):

```bash
claude mcp add --transport http --scope user simlin-serve http://127.0.0.1:7878/mcp
```

Or, to scope the configuration to a single project (and check it into git so
collaborators inherit the same setup), drop the equivalent into `.mcp.json`
at the repository root:

```json
{
  "mcpServers": {
    "simlin-serve": {
      "type": "http",
      "url": "http://127.0.0.1:7878/mcp"
    }
  }
}
```

### Claude Desktop

Claude Desktop (as of April 2026) speaks MCP-over-stdio only, so it needs the
[`mcp-remote`](https://www.npmjs.com/package/mcp-remote) proxy to bridge to
our HTTP server. Install it once globally:

```bash
npm install -g mcp-remote
```

Then add an entry to Claude Desktop's config file:

- macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`
- Windows: `%APPDATA%\Claude\claude_desktop_config.json`

```json
{
  "mcpServers": {
    "simlin-serve": {
      "command": "npx",
      "args": ["mcp-remote", "http://127.0.0.1:7878/mcp"]
    }
  }
}
```

Restart Claude Desktop after editing the config; new MCP servers are picked
up at app launch, not hot-reloaded.

### Available tools

Once connected, the AI sees five tools:

| Tool             | Purpose                                                      |
| ---------------- | ------------------------------------------------------------ |
| `ListProjects`   | Enumerate every model file in the working directory tree.   |
| `ReadModel`      | Return the canonical JSON for one model.                     |
| `EditModel`      | Apply a list of structural edits and persist the result.    |
| `CreateModel`    | Write a new model file and register it with the server.     |
| `Simulate`       | Run a simulation (with optional overrides) and return time-series data. |

Edits made through `EditModel` and `CreateModel` flow through the same
merge primitive as browser saves, so concurrent edits from both sides
converge instead of clobbering each other.

## Notifications

`simlin-serve` pushes JSON-RPC notifications to every active MCP session
whenever the underlying state changes. Five notification methods are
defined, each in the `simlin/` namespace to avoid collision with future
MCP-standard methods:

| Method                       | When it fires                                                                            |
| ---------------------------- | ---------------------------------------------------------------------------------------- |
| `simlin/projectChanged`      | A project moved to a new version (browser save, MCP edit, or filesystem change).         |
| `simlin/projectRemoved`      | A project file was deleted from disk.                                                    |
| `simlin/projectFocused`      | The browser opened or switched to a project.                                             |
| `simlin/selectionChanged`    | The browser's variable selection changed inside the focused project.                     |
| `simlin/diagnosticsChanged`  | The set of validation errors for a project changed (errors introduced, fixed, or both).  |

### Payload shapes

`simlin/projectChanged`:

```json
{
  "path": "models/teacup.xmile",
  "version": 3,
  "source": "user"
}
```

`source` is `"user"` for browser saves, `"agent"` for MCP edits, or
`"disk"` for filesystem-watcher reloads. `version` is the new
optimistic-lock version (monotonic per project). Note that `source:
"agent"` notifications fan out to *all* connected MCP clients, including
the one that triggered the edit — your client receives an echo of its
own write.

`simlin/projectRemoved`:

```json
{ "path": "models/teacup.xmile" }
```

`simlin/projectFocused`:

```json
{ "path": "models/teacup.xmile" }
```

`simlin/selectionChanged`:

```json
{
  "path": "models/teacup.xmile",
  "variableIdents": ["teacup_temperature", "ambient_temperature"]
}
```

`variableIdents` is the list of canonical idents currently selected.
An empty array means nothing is selected. The browser debounces these
events (150ms) so rapid selection changes coalesce into a single frame.

`simlin/diagnosticsChanged`:

```json
{
  "path": "models/teacup.xmile",
  "errors": [
    {
      "code": "unknown_dependency",
      "message": "variable 'x' references unknown 'bogus'",
      "modelName": "main",
      "variableName": "x",
      "kind": "variable"
    }
  ]
}
```

The full error list is sent on every change (not a delta), so an empty
`errors` array means all previously known errors are now fixed. `kind`
is one of `"project"`, `"model"`, `"variable"`, `"units"`, or
`"simulation"`. `modelName` and `variableName` are omitted (rather than
sent as `null`) when the diagnostic isn't bound to a specific model or
variable.

### Example wire frame

A complete notification on the wire looks like:

```json
{"jsonrpc":"2.0","method":"simlin/projectChanged","params":{"path":"models/teacup.xmile","version":3,"source":"user"}}
```

There is no `id` field — JSON-RPC notifications are fire-and-forget by
spec, and `simlin-serve` doesn't expect a reply.

### Ordering and delivery semantics

**Notifications are advisory and may arrive before or after a tool
response that triggered them.** The MCP transport delivers tool
responses and notifications on parallel paths; on Streamable HTTP the
client may observe a `simlin/projectChanged` notification *before* the
response to the `EditModel` call that produced it. AI clients should
treat each notification as a hint to optionally re-fetch latest state,
not as authoritative delivery of the new state itself. Concretely, when
a notification matters for your next action, follow it with a fresh
`ReadModel` (or whichever tool reads the data you need) rather than
trusting the notification payload as the canonical view.

This is the same design the browser frontend uses: it treats
`projectChanged` over the WebSocket as a remount trigger, then
re-reads the project state via the HTTP API.

`simlin/diagnosticsChanged` always follows the corresponding
`simlin/projectChanged` for the same path, because both are published
sequentially from the same task after a successful merge.

### Subscribing

No subscription action is needed: every successfully `initialize`-d MCP
session automatically receives all five notification kinds for the
lifetime of the connection. When the session closes, the server's
per-session forwarder exits cleanly and releases its bus subscription.

### Claude Desktop via `mcp-remote`

The [`mcp-remote`](https://www.npmjs.com/package/mcp-remote) proxy
forwards every server message — including custom-method notifications
— to the stdio client unchanged, so Claude Desktop sessions do receive
these frames on the wire. As of April 2026, however, Claude Desktop's
UI surfaces only the standard MCP notification methods; `simlin/*`
custom methods arrive at the client but aren't visibly rendered.
Programmatic access (logs, debugging tools) still sees them, and a
future Desktop release that surfaces custom notifications will pick
them up automatically without server-side changes.

## WebSocket Protocol

The server exposes a single WebSocket endpoint for live updates:

```
GET /api/updates?token=<launch-token>
```

The connection is authenticated via the `?token=...` query parameter — browser
native `WebSocket` cannot set custom headers on the upgrade handshake, so the
bearer rides as a query string. The token value is the same one embedded in the
launch URL printed at startup. Token mismatch returns `401 Unauthorized`; a
missing token returns `400 Bad Request`.

### Message envelope

Server-to-client frames are JSON text frames. Every message carries a `type`
discriminator (camelCase) plus variant-specific fields. The Phase 3 wire shape
defines a single variant:

```json
{
  "type": "projectChanged",
  "path": "models/teacup.stmx",
  "version": 7,
  "source": "user"
}
```

| field     | description                                                                 |
| --------- | --------------------------------------------------------------------------- |
| `type`    | Always `"projectChanged"` for now. Future variants will add new strings.    |
| `path`    | Forward-slash relative path of the project that changed.                    |
| `version` | New optimistic-lock version (monotonic per project).                        |
| `source`  | Provenance: `"user"` (browser save), `"agent"` (MCP), `"disk"` (filesystem watcher). |

Phase 3 only emits `source: "user"`; `agent` and `disk` arrive in later phases.
Clients should ignore unknown `type` discriminators rather than erroring.

### Client-to-server messages

Phase 3 ignores all incoming frames except `Close`. Future phases will define an
upstream variant for `selectionChanged` (collaborative awareness).

### Minimal browser client

```html
<script>
const token = new URLSearchParams(location.search).get('token');
const ws = new WebSocket(
  `ws://${location.host}/api/updates?token=${encodeURIComponent(token)}`
);

ws.addEventListener('message', (event) => {
  const msg = JSON.parse(event.data);
  if (msg.type === 'projectChanged') {
    console.log(`project ${msg.path} -> v${msg.version} (${msg.source})`);
    // Refetch via GET /api/projects/<path> and remount the editor.
  }
});

ws.addEventListener('close', () => {
  // Browser closes when the server exits or the token rotates.
  // Frontends typically reconnect with exponential backoff (cap 5s).
});
</script>
```

### Operational notes

- Capacity is bounded at 64 messages per client; a slow consumer that falls
  more than 64 messages behind sees the broadcast channel skip the oldest
  entries. The server logs `ws: lagged by N` at warn level.
- The connection accept and disconnect each log a single `info` line; every
  outgoing message logs at `debug`. Set `RUST_LOG=simlin_serve=debug` to see
  per-frame traffic.
- The endpoint is loopback-only (`127.0.0.1` bind), so the token gate is a
  defense-in-depth measure rather than a primary authn boundary.
