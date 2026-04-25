// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::path::PathBuf;

use clap::Parser;

/// Command-line arguments for `simlin-serve`. The MCP port is parsed today but
/// not yet honored: it lands here so the CLI surface is stable before the MCP
/// server is wired up in a later phase.
#[derive(Parser, Debug, Clone)]
#[command(name = "simlin-serve", version, about)]
pub struct Args {
    /// Directory to scan for system-dynamics models. Defaults to the current
    /// working directory.
    #[arg(value_name = "ROOT")]
    pub root: Option<PathBuf>,

    /// TCP port to bind on 127.0.0.1. 0 lets the OS choose an ephemeral port.
    #[arg(long, default_value_t = 0)]
    pub port: u16,

    /// TCP port for the MCP server. Parsed today but unused until the MCP
    /// server is added in a later phase; preserving the flag now keeps the CLI
    /// surface stable for users.
    #[arg(long, default_value_t = 7878)]
    pub mcp_port: u16,

    /// When set, do not attempt to open a browser tab on startup.
    #[arg(long, default_value_t = false)]
    pub no_open: bool,
}

impl Args {
    /// Parse from `std::env::args_os()`. Thin wrapper kept here so callers can
    /// substitute `Args::parse_from(...)` in tests without touching `main`.
    pub fn parse_args() -> Self {
        Self::parse()
    }

    /// Resolve the user-supplied root or fall back to the current working
    /// directory. `current_dir` can fail (e.g. the directory was deleted out
    /// from under the process); callers that need the failure surfaced should
    /// inspect `self.root` directly.
    pub fn root_or_cwd(&self) -> std::io::Result<PathBuf> {
        match &self.root {
            Some(p) => Ok(p.clone()),
            None => std::env::current_dir(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec_when_no_args_passed() {
        let args = Args::parse_from(["simlin-serve"]);
        assert!(args.root.is_none());
        assert_eq!(args.port, 0);
        assert_eq!(args.mcp_port, 7878);
        assert!(!args.no_open);
    }

    #[test]
    fn root_falls_back_to_current_dir_when_unspecified() {
        let args = Args::parse_from(["simlin-serve"]);
        let resolved = args.root_or_cwd().expect("current_dir available in tests");
        let cwd = std::env::current_dir().expect("current_dir available in tests");
        assert_eq!(resolved, cwd);
    }

    #[test]
    fn explicit_root_overrides_cwd() {
        let args = Args::parse_from(["simlin-serve", "/tmp/example"]);
        assert_eq!(
            args.root.as_deref(),
            Some(std::path::Path::new("/tmp/example"))
        );
        assert_eq!(args.root_or_cwd().unwrap(), PathBuf::from("/tmp/example"),);
    }

    #[test]
    fn port_flag_is_parsed() {
        let args = Args::parse_from(["simlin-serve", "--port", "8080"]);
        assert_eq!(args.port, 8080);
    }

    #[test]
    fn mcp_port_flag_is_parsed() {
        let args = Args::parse_from(["simlin-serve", "--mcp-port", "9000"]);
        assert_eq!(args.mcp_port, 9000);
    }

    #[test]
    fn no_open_flag_is_parsed() {
        let args = Args::parse_from(["simlin-serve", "--no-open"]);
        assert!(args.no_open);
    }
}
