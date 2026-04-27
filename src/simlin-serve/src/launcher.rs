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
pub fn build_launch_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/")
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
    fn build_launch_url_includes_host_and_port() {
        let url = build_launch_url(54321);
        assert_eq!(url, "http://127.0.0.1:54321/");
    }

    #[test]
    fn build_launch_url_uses_loopback_host() {
        let url = build_launch_url(8080);
        assert!(
            url.starts_with("http://127.0.0.1:"),
            "expected loopback host, got {url:?}"
        );
    }

    /// Smoke-test that `open_browser` does not panic when the platform's
    /// launcher fails or quietly succeeds. The point is to keep the
    /// server running for the user regardless of what `xdg-open` /
    /// `open` / `start` return. The boolean result is not load-bearing:
    ///
    /// - Linux without `$DISPLAY`: `xdg-open` reports I/O failure and we
    ///   get `false`. Asserting `false` here is the historical case.
    /// - macOS: the system `open` command happily hands the URL to
    ///   Launch Services even on a CI runner without a logged-in GUI
    ///   session, so the call typically succeeds (returns `true`).
    /// - Windows: the launcher's behaviour on headless runners is also
    ///   non-deterministic.
    ///
    /// Rather than try to predict the boolean outcome on every host, we
    /// just exercise the call path and assert it returns *some* bool
    /// without panicking. The Linux-headless `false` arm is still the
    /// case the implementation cares about; we keep that assertion
    /// gated on `target_os = "linux"` and the absence of `$DISPLAY`,
    /// where it is genuinely deterministic.
    #[test]
    fn open_browser_does_not_panic() {
        let _ = open_browser("http://127.0.0.1:1/never-opens");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn open_browser_returns_false_in_linux_headless() {
        // `xdg-open` exits non-zero when no display is available; we
        // map that to `false`. On a developer machine with `$DISPLAY`
        // set we cannot predict the outcome, so skip the assertion.
        let has_display = std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty());
        if has_display {
            return;
        }
        let result = open_browser("http://127.0.0.1:1/never-opens");
        assert!(
            !result,
            "expected open_browser to fall through to false in headless Linux"
        );
    }
}
