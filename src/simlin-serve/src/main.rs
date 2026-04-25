// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![deny(unsafe_code)]

use std::net::SocketAddr;

use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

use simlin_serve::build_router;
use simlin_serve::cli::Args;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse_args();

    // Default to "simlin_serve=info" but let RUST_LOG override; the directive
    // additions are appended only when no env filter is set.
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("simlin_serve=info"));
    fmt().with_env_filter(env_filter).init();

    // Resolve the root early so a missing/inaccessible cwd surfaces before we
    // bind a port. Subsequent phases plumb this into ProjectRegistry.
    let _root = args.root_or_cwd()?;

    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], args.port))).await?;
    let bound = listener.local_addr()?;
    println!("simlin-serve listening on http://{bound}");

    if !args.no_open {
        // Phase 1 leaves the actual browser-launch path for a later task; we
        // honor --no-open by emitting nothing here so the flag has consistent
        // semantics from day one.
    }

    axum::serve(listener, build_router()).await?;
    Ok(())
}
