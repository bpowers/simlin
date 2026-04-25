// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#![deny(unsafe_code)]

use std::net::SocketAddr;

use clap::Parser;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

use simlin_serve::build_router;

/// Local HTTP server for browsing system-dynamics models.
#[derive(Parser, Debug)]
#[command(name = "simlin-serve", version, about)]
struct Args {
    /// TCP port to bind on 127.0.0.1. 0 lets the OS choose an ephemeral port.
    #[arg(long, default_value_t = 0)]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Default to "simlin_serve=info" but let RUST_LOG override; the directive
    // additions are appended only when no env filter is set.
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("simlin_serve=info"));
    fmt().with_env_filter(env_filter).init();

    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], args.port))).await?;
    let bound = listener.local_addr()?;
    println!("simlin-serve listening on http://{bound}");

    axum::serve(listener, build_router()).await?;
    Ok(())
}
