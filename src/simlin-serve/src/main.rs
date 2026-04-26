// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![deny(unsafe_code)]

use std::sync::Arc;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

use simlin_serve::build_router;
use simlin_serve::cli::Args;
use simlin_serve::events::EventBus;
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::launcher::{build_launch_url, open_browser};
use simlin_serve::mcp::build_mcp_router;
use simlin_serve::registry::ProjectRegistry;
use simlin_serve::scan::scan_into_registry;
use simlin_serve::serving::bind_or_die;
use simlin_serve::token::generate_launch_token;
use simlin_serve::watcher::{ShutdownSignal, spawn_watcher};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse_args();

    // Default to "simlin_serve=info" but let RUST_LOG override; the directive
    // additions are appended only when no env filter is set.
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("simlin_serve=info"));
    fmt().with_env_filter(env_filter).init();

    // Resolve the root early so a missing/inaccessible cwd surfaces before we
    // bind a port. Canonicalize so registry keys and traversal checks share
    // the same absolute anchor.
    let resolved_root = args.root_or_cwd()?;
    let canonical_root = resolved_root.canonicalize()?;

    let registry = Arc::new(ProjectRegistry::new(canonical_root.clone()));
    let git = Arc::new(GitProbe::detect());
    if let Err(err) = scan_into_registry(&canonical_root, &registry, &git) {
        tracing::warn!(error = %err, "initial scan failed; registry starts empty");
    }

    // Bind both listeners up front so a port-conflict diagnosis surfaces
    // before we generate any tokens or open a browser. Order matters:
    // bind UI first so a successful UI bind doesn't leak when the MCP
    // bind subsequently fails (the OS releases the UI listener when the
    // returned `TcpListener` is dropped on early return).
    let ui_listener = bind_or_die(("127.0.0.1", args.port), "HTTP/UI server", None).await?;
    let mcp_listener = bind_or_die(
        ("127.0.0.1", args.mcp_port),
        "MCP server",
        Some("--mcp-port"),
    )
    .await?;

    let ui_addr = ui_listener.local_addr()?;
    let mcp_addr = mcp_listener.local_addr()?;

    // Generate the one-time launch token after both binds so we never log a
    // token associated with a port we failed to acquire.
    let token = generate_launch_token();
    let launch_url = build_launch_url(ui_addr.port(), &token);

    // Token is shared into the AppState so the WebSocket upgrade handler
    // can validate the `?token=...` query param against the same value
    // that ended up in the launch URL.
    let state = AppState {
        registry,
        git,
        root: Arc::new(canonical_root),
        events: Arc::new(EventBus::new()),
        launch_token: Arc::new(token.clone()),
    };

    // Single println! per stable line so subprocess-based smoke tests can
    // parse both URLs deterministically with a single regex pass.
    println!("Simlin Serve");
    println!("  UI:  {launch_url}");
    println!("  MCP: http://127.0.0.1:{}/mcp", mcp_addr.port());

    if !args.no_open {
        open_browser(&launch_url);
    }

    let state_arc = Arc::new(state.clone());

    // Spawn the file watcher (Phase 4). The shutdown notifier is shared
    // with the Ctrl-C path below: a single signal stops both servers
    // (via axum's with_graceful_shutdown) and the watcher actor (via
    // the Notify).
    let watcher_shutdown: ShutdownSignal = Arc::new(tokio::sync::Notify::new());
    let watcher_handle = match spawn_watcher(state.clone(), watcher_shutdown.clone()) {
        Ok(h) => Some(h),
        Err(err) => {
            // Failing to set up the watcher is non-fatal: the server still
            // serves the directory snapshot taken at startup. Surfacing
            // the error keeps the operator aware that disk edits won't
            // trigger live updates until the cause is fixed.
            tracing::warn!(error = %err, "failed to spawn file watcher; disk edits will not be observed");
            None
        }
    };

    // Both axum servers share a single Ctrl-C signal via a `Notify`
    // fan-out: ctrl_c() is consumed once in a small task that publishes
    // to the Notify, and each `with_graceful_shutdown` future waits on
    // its own `notified()` slot. This keeps the teardown deterministic
    // (both servers begin draining at the same time) and avoids racing
    // two ctrl_c handlers against each other.
    //
    // Order of teardown on Ctrl-C:
    //   1. ctrl_c() resolves -> notify_waiters() fires both shutdown
    //      futures simultaneously, so axum stops accepting new
    //      connections on each port and drains in-flight requests.
    //   2. After both axum::serve futures return (joined via
    //      tokio::try_join!), fire the watcher shutdown so the actor
    //      breaks out of its select! loop and drops the Debouncer
    //      (which releases the OS-level watch).
    //   3. Await the watcher's JoinHandle so the binary doesn't exit
    //      while the actor is mid-shutdown.
    let shutdown_notify = Arc::new(tokio::sync::Notify::new());
    {
        let shutdown_notify = shutdown_notify.clone();
        tokio::spawn(async move {
            match tokio::signal::ctrl_c().await {
                Ok(()) => {
                    tracing::info!("ctrl-c received; shutting down");
                }
                Err(err) => {
                    // Failing to install the handler typically means we're
                    // running under a supervisor that strips signals; the
                    // shutdown task simply never fires, leaving the
                    // servers running until the parent kills the process.
                    tracing::error!(error = %err, "failed to install ctrl-c handler");
                    return;
                }
            }
            shutdown_notify.notify_waiters();
        });
    }

    let ui_shutdown = {
        let n = shutdown_notify.clone();
        async move {
            n.notified().await;
        }
    };
    let mcp_shutdown = {
        let n = shutdown_notify.clone();
        async move {
            n.notified().await;
        }
    };

    let ui_serve =
        axum::serve(ui_listener, build_router(state)).with_graceful_shutdown(ui_shutdown);
    let mcp_serve =
        axum::serve(mcp_listener, build_mcp_router(state_arc)).with_graceful_shutdown(mcp_shutdown);

    tokio::try_join!(ui_serve, mcp_serve)?;

    watcher_shutdown.notify_waiters();
    if let Some(handle) = watcher_handle {
        // Drop the join handle's result silently; a panicking watcher
        // task is logged at the actor's source, not here.
        let _ = handle.await;
    }

    Ok(())
}
