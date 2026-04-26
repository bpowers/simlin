# simlin-serve

Local HTTP server that discovers system-dynamics models in a directory tree and
serves them to a browser-based viewer/editor. Distributed as the `@simlin/serve`
npm package, intended to be invoked as `npx @simlin/serve` from any project
directory.

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
