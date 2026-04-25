// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Browser-launch shim. The HTTP URL is always printed to stdout from
//! `main.rs` so users in headless environments (or with `--no-open`) still see
//! it; this module's only job is the optional auto-open and the human-friendly
//! fallback message when that fails.

/// Build the launch URL the SPA should open. Pulled out of `main.rs` so we can
/// unit-test the formatting without binding a TCP port. The 127.0.0.1 host is
/// hard-coded because the listener is similarly bound to loopback in `main`.
pub fn build_launch_url(port: u16, token: &str) -> String {
    format!("http://127.0.0.1:{port}/?token={token}")
}

/// Try to open the user's default browser at `url`. Returns `true` on success
/// and `false` on any I/O failure (missing `xdg-open` / `open` / `start`,
/// missing `$DISPLAY`, sandboxed environment, etc.).
///
/// On failure we print a single user-facing line to stderr so a CLI user who
/// is staring at the stdout URL print also sees an explanation; we
/// deliberately do not crash, since the server itself is still healthy and
/// the user can copy the URL manually.
pub fn open_browser(url: &str) -> bool {
    match open::that(url) {
        Ok(()) => true,
        Err(_) => {
            eprintln!("could not open browser automatically; visit: {url}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_launch_url_includes_host_port_and_token() {
        let url = build_launch_url(54321, "abc123");
        assert_eq!(url, "http://127.0.0.1:54321/?token=abc123");
    }

    #[test]
    fn build_launch_url_uses_loopback_host() {
        let url = build_launch_url(8080, "tok");
        assert!(
            url.starts_with("http://127.0.0.1:"),
            "expected loopback host, got {url:?}"
        );
    }

    /// Smoke-test the failure path under the assumption that our test
    /// environment has no GUI: in CI / headless dev shells, `xdg-open`
    /// (Linux) and the equivalent on macOS/Windows have no display to open
    /// against and report I/O failure. The point of this test is to make
    /// sure that failure is downgraded to a `false` return rather than a
    /// panic so the server keeps running for the user. If `DISPLAY` is set
    /// we can't reliably predict the outcome, so skip the assertion in that
    /// case rather than spuriously failing on a developer's desktop.
    #[test]
    fn open_browser_returns_false_when_launcher_fails() {
        let has_display = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty());
        if has_display {
            return;
        }
        let result = open_browser("http://127.0.0.1:1/never-opens");
        assert!(
            !result,
            "expected open_browser to fall through to false in headless env"
        );
    }
}
