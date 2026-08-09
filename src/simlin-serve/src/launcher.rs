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
///
/// The launch command is one fixed per-OS invocation rather than a general
/// URL-opening library: the only argument we ever pass is our own
/// `http://127.0.0.1:<port>/` URL (no spaces, no shell metacharacters), so
/// the detection logic libraries add (WSL, Flatpak, `$BROWSER`, ...) is
/// surface area without benefit here.
pub fn open_browser(url: &str) -> bool {
    if launch_browser_command(url) {
        return true;
    }
    eprintln!("could not open browser automatically; visit: {url}");
    false
}

#[cfg(target_os = "macos")]
fn launch_browser_command(url: &str) -> bool {
    spawn_quiet(std::process::Command::new("open").arg(url))
}

#[cfg(target_os = "windows")]
fn launch_browser_command(url: &str) -> bool {
    // `start` is a cmd.exe builtin; the empty string is the window-title
    // positional that keeps `start` from treating the URL as a title.
    spawn_quiet(std::process::Command::new("cmd").args(["/C", "start", "", url]))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn launch_browser_command(url: &str) -> bool {
    spawn_quiet(std::process::Command::new("xdg-open").arg(url))
}

/// Run the launcher with stdio detached, mapping spawn failure or a non-zero
/// exit to `false`. Waits for exit: `xdg-open`/`open`/`start` all hand off to
/// the desktop session and return promptly, and the exit status is the only
/// failure signal we get (e.g. `xdg-open` without `$DISPLAY`).
fn spawn_quiet(cmd: &mut std::process::Command) -> bool {
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

// `open_browser` and `spawn_quiet` are deliberately untested: every
// observable outcome is a property of the host's `xdg-open` / `open` /
// `start`, not of our three lines around it, and exercising them spawns a
// real browser launch during `cargo test` on any developer machine with a
// display. The tests that used to live here either asserted nothing or
// skipped themselves on every host where the launcher succeeds.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_launch_url_includes_host_and_port() {
        let url = build_launch_url(54321);
        assert_eq!(url, "http://127.0.0.1:54321/");
    }
}
