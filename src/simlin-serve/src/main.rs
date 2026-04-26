// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![deny(unsafe_code)]

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

use simlin_serve::build_router;
use simlin_serve::cli::Args;
use simlin_serve::events::EventBus;
use simlin_serve::git::GitProbe;
use simlin_serve::handlers::AppState;
use simlin_serve::launcher::{build_launch_url, open_browser};
use simlin_serve::registry::ProjectRegistry;
use simlin_serve::scan::scan_into_registry;
use simlin_serve::token::generate_launch_token;

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

    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], args.port))).await?;
    let bound = listener.local_addr()?;

    // Generate the one-time launch token after binding so we never log a
    // token associated with a port we failed to acquire.
    let token = generate_launch_token();
    let launch_url = build_launch_url(bound.port(), &token);

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

    // Always print the URL — users with --no-open or running headless still
    // need to see and copy it. stdout (not tracing) so the URL is plain and
    // pipe-friendly.
    println!("simlin-serve listening on {launch_url}");

    if !args.no_open {
        open_browser(&launch_url);
    }

    axum::serve(listener, build_router(state)).await?;
    Ok(())
}
