# Simlin Serve Threat Model

This document describes the V1 threat model for `simlin-serve`, the
local-first HTTP server shipped as `npx @simlin/serve`. It states what
the server does and does not defend against, and explains the design
choices behind each layer.

The audience is anyone evaluating whether the server is safe to run on
their machine. If you're contributing code that touches the bind, the
host allowlist, or the request validation, this is the contract you're
working against -- changes here need a paired update to the threat
model.

## Trust Boundary

The trust boundary is the **OS user account**. Any process running as
the user can already read and write files in the served directory --
the operating system's file-permission model is what stops other users
on the same machine, not `simlin-serve`. The server is therefore
**not** a privilege boundary: a malicious local process that has
inherited the user's privileges can do everything `simlin-serve` can
do, and more, regardless of the server.

`simlin-serve` is designed for **single-user workstations**: a
developer running `npx @simlin/serve` from a terminal on their
laptop. Multi-user shared hosts (school computers, kiosk machines,
build servers shared by multiple humans) are explicitly out of scope
for V1; if that's your environment, do not run this.

What the server does protect:

- **Network reachability.** Only loopback (`127.0.0.1`) is bound.
  Other hosts on the LAN cannot connect.
- **Cross-origin browsers.** A malicious website cannot reach
  `simlin-serve` from a victim's browser -- the host allowlist and
  the WS origin check together close the path.

What the server does **not** protect:

- **Other OS users on the same machine.** They can connect to the
  loopback ports and exercise both the HTTP/UI surface and the MCP
  surface. The OS file-permission model is the only thing that limits
  what they can read or write through the server. If your model
  directory is readable by another local user, they can read it
  through `simlin-serve`; if it's writable, they can write to it.
  This is the same posture as Jupyter notebooks, `vite dev`, and
  every other "loopback-only dev server" in common use.

## Defenses

### 1. Loopback bind (primary boundary)

Both listeners (`--port` for the UI/API, `--mcp-port` for MCP) bind
`127.0.0.1` exclusively. The OS refuses connections from any other
interface, so a network-attached attacker is not in the threat model
at all -- there is no socket for them to reach.

This is not configurable. There is no "listen on 0.0.0.0" mode.

### 2. Host header allowlist (DNS rebinding defense)

A malicious website can persuade the victim's browser to connect to
`127.0.0.1:<port>` by registering a DNS name that resolves to the
loopback IP after a short TTL window. The same-origin policy treats
the request as same-origin with the attacker's page (because the
hostname matches), so the attacker can read responses.

**Mitigation:** every HTTP request is gated on a `Host:` header that
matches one of `127.0.0.1:<ui_port>`, `localhost:<ui_port>`,
`127.0.0.1:<mcp_port>`, or `localhost:<mcp_port>`. Anything else
returns `421 Misdirected Request` before the inner handler runs. The
attacker's DNS name does not match, so the request is rejected.

This was a real attack: **CVE-2025-66414** (December 2025) bit the
official MCP TypeScript SDK's localhost servers via this vector.
`simlin-serve` is not vulnerable because the host check is layered in
front of every route on both the UI and MCP routers (see
`src/middleware.rs`).

### 3. Origin header allowlist (cross-origin WS defense)

WebSocket upgrades carry an `Origin:` header set by the browser. The
upgrade handler compares it against `http://127.0.0.1:<ui_port>` and
`http://localhost:<ui_port>`; mismatches return `403 Forbidden`. This
catches the case where an attacker page somehow crossed the host
allowlist (e.g. via a same-origin compromise) but is still hosted at
a non-allowlisted origin.

The default `--strict-origin true` rejects upgrades with no `Origin:`
header. Browsers always send one; non-browser clients like `wscat`
typically don't, so dev users can pass `--strict-origin false` to
relax that one specific check (a present `Origin:` is still validated
against the allowlist).

### 4. Path traversal rejection

Every `rel_path` segment from the client is validated:

- Null bytes -> 400 Bad Request
- `..` segments -> 400
- Absolute paths (Unix `/`, Windows drive prefix) -> 400
- After canonicalization, the resolved path must descend from the
  scan root -- a symlink that points out of the tree is rejected
  with 403 Forbidden

The same checks apply to MCP-supplied paths via `RegistryAccess`.

### 5. Request body size cap

`/api/projects/{*rel_path}` POST bodies are capped at 16 MiB
(`MAX_BODY_BYTES` in `src/lib.rs`). Larger requests are rejected by
`tower-http`'s `RequestBodyLimitLayer` ahead of any handler.

### 6. Supply-chain posture

The npm packages (`@simlin/serve`, `@simlin/mcp`, and the per-
platform binary shims) are published with `npm publish --provenance`,
attaching an OpenID Connect attestation that the package was built by
GitHub Actions on a known repository commit. There are **no
postinstall scripts** in either npm shim -- a malicious mirror cannot
gain execution at install time. The binary is downloaded as an
`optionalDependencies` entry whose tarball npm verifies against the
registry's integrity hash.

The trust chain is therefore: npm registry + GitHub Actions OIDC. A
compromise of either link is out of scope; we assume the npm signing
key and the OIDC token are not under attacker control.

## Out of Scope

These items are intentionally not defended against. If your threat
model includes one, do not run `simlin-serve` on a hostile host.

| Out-of-scope item | Why |
|---|---|
| **HTTPS/TLS** | The server binds loopback only. There is no plaintext network segment to encrypt. A loopback connection between two processes on the same machine traverses the OS kernel, not a wire. |
| **Bearer-token authentication** | V1 trusts the OS user-account boundary. Any process running as the same user can already read and write the model files directly; gating the server on a token would not raise the privilege bar. Multi-user hosts are out of scope (see Trust Boundary). |
| **OS user namespace** | We assume the OS user is the sole administrator of the user's processes and files. Malware running as the same user is already inside the trust boundary. Other OS users on the same machine are also outside the protected set -- the OS file-permission model is what mediates access to the served files. |
| **File-watcher event content** | The watcher feeds disk-content into the same parser/validator pipeline that browser saves go through; malformed projects are rejected with the same diagnostic surface. There is no eval-on-content path. |
| **Compiler/runtime exploits** | The simulation engine compiles models to a stack-based bytecode VM with checked indexing. A bug in the engine could crash a worker, but is not a privilege escalation -- the worker only has the user's own privileges. |
| **DoS via large models** | Body size is capped; oversize fixtures rejected. There is no rate limit on `/api/*` because the server is single-user. |
| **Frontend-bundle integrity** | The static assets are embedded into the binary at build time (via `include_bytes!`). A compromised binary could ship a malicious SPA, but that's the supply-chain item above, not a runtime concern. |

## Verification

The defenses above are exercised by these tests:

| Defense | Test |
|---|---|
| Host allowlist (UI router) | `tests/middleware_host.rs::*` |
| Host allowlist (MCP router) | `tests/middleware_host.rs::mcp_router_*` |
| Origin allowlist (WS) | `tests/ws_updates.rs::*_origin_*` |
| Path traversal | `src/handlers.rs::tests::sanitize_*` and `tests/api_get_project.rs` |
| Body size cap | `tests/router_layers.rs::oversized_*` |
| Loopback-only bind | `src/serving.rs` (the bind address literal) |

Run the full suite with `cargo test -p simlin-serve`.

## Reporting Security Issues

If you find a vulnerability that this document does not address, open
an issue on the GitHub repository tagged `security`. For sensitive
disclosures, email `bobby@simlin.com`.
