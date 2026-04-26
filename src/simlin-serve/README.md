# @simlin/serve

View, edit, and AI-collaborate on system dynamics models from any
directory on your machine. Distributed as the [`@simlin/serve`][npm]
npm package; runs a local HTTP server plus an in-process MCP server so
your browser and your AI client share one canonical view of every
model in the working tree.

[Simlin](https://simlin.com) is the broader stock-and-flow modelling
toolkit; `@simlin/serve` is the local-first front door.

[npm]: https://www.npmjs.com/package/@simlin/serve

## Quick Start

In any directory containing `.stmx`, `.xmile`, `.mdl`, or `.sd.json`
files:

```bash
npx @simlin/serve@latest
```

You'll see two URLs printed:

```
Simlin Serve
  UI:  http://127.0.0.1:54321/?token=<random>
  MCP: http://127.0.0.1:7878/mcp
```

Your default browser opens the UI automatically. The MCP URL is stable
across launches (port `7878` by default), so configuring an AI client
is a one-time step — you don't need to update it whenever you restart
the server.

The bearer token in the UI URL is fresh on every launch; killing and
restarting the server invalidates all prior tabs.

## CLI flags

| Flag               | Default          | Purpose                                                              |
| ------------------ | ---------------- | -------------------------------------------------------------------- |
| `[ROOT]`           | `$PWD`           | Directory to serve (positional).                                     |
| `--port <N>`       | `0` (ephemeral)  | UI HTTP port. `0` lets the OS pick.                                  |
| `--mcp-port <N>`   | `7878`           | MCP HTTP port. Stable default so AI client configs don't drift.      |
| `--no-open`        | `false`          | Suppress the browser launch; print the URLs and wait.                |
| `--strict-origin`  | `true`           | Reject WebSocket upgrades whose `Origin:` header is missing or wrong. Set to `false` for non-browser WS clients like `wscat`. |

## MCP setup

`simlin-serve` exposes an MCP server at `/mcp` on the MCP port so AI
assistants can read, edit, simulate, and create models in the same
working directory the browser is editing. The MCP and HTTP/UI handlers
share one in-memory Loro document, so edits from either side converge
and the browser remounts the editor within ~1s of an AI edit.

Both clients below assume `simlin-serve` is already running. Start it
first (`npx @simlin/serve`) and leave it running while the AI client
is connected.

### Claude Code CLI

Claude Code speaks MCP-over-HTTP natively, so no proxy is needed:

```bash
claude mcp add --transport http --scope user simlin-serve http://127.0.0.1:7878/mcp
```

Or scope to a single project by dropping the equivalent into
`.mcp.json` at the repository root (so collaborators inherit the same
setup):

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

Claude Desktop (as of April 2026) speaks MCP-over-stdio only, so it
needs the [`mcp-remote`](https://www.npmjs.com/package/mcp-remote)
proxy to bridge to our HTTP server. Install once globally:

```bash
npm install -g mcp-remote
```

Then add an entry to Claude Desktop's config:

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

Restart Claude Desktop after editing the config; new MCP servers are
picked up at app launch, not hot-reloaded.

## Supported file formats

| Format       | Extensions                | Read                | Edit                                          |
| ------------ | ------------------------- | ------------------- | --------------------------------------------- |
| XMILE        | `.stmx`, `.xmile`, `.xml` | yes                 | yes (in-place)                                |
| Simlin JSON  | `.sd.json`                | yes                 | yes (in-place)                                |
| Vensim MDL   | `.mdl`                    | yes (via xmutil)    | yes (writes a `.sd.json` sidecar; `.mdl` untouched) |

The `.mdl` sidecar pattern preserves the source-of-truth Vensim file
verbatim while letting Simlin's editors persist structural changes
into a JSON twin. Subsequent reads prefer the sidecar when both files
are present.

## MCP tool surface

| Tool             | Description                                                                |
| ---------------- | -------------------------------------------------------------------------- |
| `ListProjects`   | Enumerate every model file in the working directory tree, with format and git status. |
| `ReadModel`      | Return the canonical JSON for one model, including loop dominance analysis. |
| `EditModel`      | Apply a list of structural edits and persist the result to disk.            |
| `CreateModel`    | Write a new empty model file and register it with the server.               |
| `Simulate`       | Run a simulation (with optional parameter overrides) and return time-series data. |

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

**Notifications are advisory.** The MCP transport delivers tool
responses and notifications on parallel paths; an AI client may
observe a `simlin/projectChanged` notification *before* the response
to the `EditModel` call that produced it. Treat each notification as a
hint to optionally re-fetch latest state, not as authoritative
delivery of the new state itself. When a notification matters for
your next action, follow it with a fresh `ReadModel` rather than
trusting the notification payload as the canonical view.

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
optimistic-lock version (monotonic per project). `source: "agent"`
notifications fan out to *all* connected MCP clients, including the
one that triggered the edit — your client receives an echo of its own
write.

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
`"simulation"`. `modelName` and `variableName` are omitted (rather
than sent as `null`) when the diagnostic isn't bound to a specific
model or variable. `simlin/diagnosticsChanged` always follows the
corresponding `simlin/projectChanged` for the same path.

### Example wire frame

A complete notification on the wire looks like:

```json
{"jsonrpc":"2.0","method":"simlin/projectChanged","params":{"path":"models/teacup.xmile","version":3,"source":"user"}}
```

There is no `id` field — JSON-RPC notifications are fire-and-forget by
spec, and `simlin-serve` doesn't expect a reply.

### Subscribing

No subscription action is needed: every successfully `initialize`-d
MCP session automatically receives all five notification kinds for the
lifetime of the connection. When the session closes, the server's
per-session forwarder exits cleanly.

The [`mcp-remote`](https://www.npmjs.com/package/mcp-remote) proxy
forwards every server message — including custom-method notifications
— to the stdio client unchanged, so Claude Desktop sessions do receive
these frames on the wire. As of April 2026, however, Claude Desktop's
UI surfaces only the standard MCP notification methods; `simlin/*`
custom methods arrive at the client but aren't visibly rendered. A
future Desktop release that surfaces custom notifications will pick
them up automatically without server-side changes.

## WebSocket protocol

The browser uses a single WebSocket endpoint for live updates:

```
GET /api/updates?token=<launch-token>
```

The connection is authenticated via the `?token=...` query parameter
— browser native `WebSocket` cannot set custom headers on the upgrade
handshake, so the bearer rides as a query string. Token mismatch
returns `401 Unauthorized`; a missing token returns `400 Bad Request`.

Server-to-client frames are JSON text frames carrying a `type`
discriminator (camelCase) plus variant-specific fields:

```json
{
  "type": "projectChanged",
  "path": "models/teacup.stmx",
  "version": 7,
  "source": "user"
}
```

| Field     | Description                                                                 |
| --------- | --------------------------------------------------------------------------- |
| `type`    | Discriminator. Today: `"projectChanged"`, `"projectRemoved"`, `"projectRenamed"`. |
| `path`    | Forward-slash relative path of the project that changed.                    |
| `version` | New optimistic-lock version (monotonic per project).                        |
| `source`  | Provenance: `"user"` (browser save), `"agent"` (MCP), `"disk"` (filesystem watcher). |

Clients should ignore unknown `type` discriminators rather than
erroring. Capacity is bounded at 64 messages per client; a slow
consumer that falls more than 64 messages behind sees the broadcast
channel skip the oldest entries and the server logs `ws: lagged by N`
at warn level.

## Security and threat model

`simlin-serve` binds `127.0.0.1` only and gates `/api/*` plus the
WebSocket upgrade behind a 256-bit per-launch bearer token. A `Host:`
header allowlist on every HTTP route — and an `Origin:` allowlist on
the WebSocket upgrade — defends against DNS-rebinding attacks (see
CVE-2025-66414, which bit the official MCP TypeScript SDK on
December 2025).

For the full V1 threat model — what the server defends against, what
it doesn't, and the design choices behind each layer — see
[/docs/threat-model.md](../../docs/threat-model.md).

## Limitations (V1)

- **Vensim `.mdl` writes are sidecar-only.** Edits to a `.mdl`-backed
  model land in a sibling `.sd.json` file. True `.mdl` round-trip is
  future work; for now treat the `.mdl` as an immutable import
  source and let `.sd.json` be the editable twin.
- **macOS Intel (`darwin-x64`) binaries are not yet published.** The
  shipped `darwin-arm64` binary cannot run on Intel hardware — Rosetta
  only translates x86_64 binaries onto Apple Silicon, never the
  reverse. Intel Mac users can build from source
  (`cargo install --git https://github.com/bpowers/simlin simlin-serve`)
  or wait for the `darwin-x64` binary in a follow-up release.
  Apple Silicon (`darwin-arm64`), Linux x64, Linux arm64, and Windows
  x64 are all shipped today.
- **Claude Desktop requires the `mcp-remote` npm proxy.** Desktop
  speaks stdio-only as of April 2026; `mcp-remote` bridges to the HTTP
  server. Claude Code, Cursor, and other HTTP-native MCP clients
  connect directly.

## License

Apache-2.0
