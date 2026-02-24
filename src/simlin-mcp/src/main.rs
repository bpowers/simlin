// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MCP server binary for Simlin.
//!
//! Exposes the Simlin simulation engine as MCP tools over stdio,
//! allowing AI assistants to read, create, and edit system dynamics
//! models.
//!
//! # Usage
//!
//! ```sh
//! simlin-mcp              # start the MCP server on stdin/stdout
//! simlin-mcp --version    # print version
//! ```

mod protocol;
mod tool;
mod tools;
mod transport;

use transport::StdioTransport;

#[tokio::main]
async fn main() {
    // Handle --version
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("simlin-mcp {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let config = protocol::ServerConfig {
        name: "simlin-mcp".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        instructions: Some(
            "Simlin MCP server for system dynamics modeling. \
             Use read_model to inspect models, edit_model to apply patches, \
             and create_model to create new model files."
                .to_string(),
        ),
    };

    let mut registry = tool::Registry::new();
    tools::register_all(&mut registry);

    let mut transport = StdioTransport::new();

    if let Err(e) = protocol::serve_async(&mut transport, &config, &registry).await {
        eprintln!("simlin-mcp: fatal error: {e}");
        std::process::exit(1);
    }
}
