// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Test-only helpers exposed for integration tests under `tests/`.
//!
//! Mirrors the `test_support` pattern from `simlin-mcp-core`: a
//! `#[doc(hidden)]` module so integration tests can import helpers without
//! polluting the public library API.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::watch;

use crate::git::GitProbe;
use crate::watcher::PROBE_FILE_PREFIX;

/// Return a `GitProbe` that behaves as if git is unavailable.
///
/// Use this in integration tests to exercise the AC2.5 degraded-state path
/// (every file reports `GitState::Unavailable`) without requiring the host
/// to have git installed or a working repository at hand.
pub fn unavailable_git_probe() -> GitProbe {
    GitProbe::new_unavailable()
}

/// Source of unique probe nonces. Nonces are process-global (tests run as
/// threads of one binary) and start at 1, so the actor's initial 0 is an
/// unambiguous "nothing seen yet".
static NEXT_PROBE_NONCE: AtomicU64 = AtomicU64::new(1);

/// How long to wait for one probe write to come back before rewriting it.
///
/// This is *not* a substitute for a readiness check -- it is the retry period
/// of the readiness check itself. A probe write that lands before the OS watch
/// is registered is lost forever, so a single write can never be enough; we
/// have to keep making events until one is reported back to us.
const PROBE_RETRY_INTERVAL: Duration = Duration::from_millis(250);

/// Total budget for a probe round trip before we declare the watch broken.
///
/// Generous because it bounds an OS-level latency we do not control: a
/// saturated macOS `fseventsd` has been measured taking >3s to register a
/// stream and a further ~1.5s to deliver the first event.
const PROBE_BUDGET: Duration = Duration::from_secs(30);

/// Upper bound on how long a *live* watch may take to report a change.
///
/// This bounds an OS latency, not a race: the race is closed by
/// [`wait_for_watcher_ready`]. Once the watch is proven live the event is
/// coming, and the only question is how far behind the OS is running -- we
/// have measured ~1.7s on a macOS host whose `fseventsd` was pegged. A test
/// that passes returns the instant its event lands, so a roomy bound costs
/// nothing except on genuine failure, where it buys a truthful diagnosis
/// instead of a flake.
pub const OS_EVENT_TIMEOUT: Duration = Duration::from_secs(10);

/// Drive one probe round trip against a watcher rooted at `root`: create an
/// inert probe file, rewriting it until the actor reports having seen it, then
/// delete it.
///
/// On return, the OS-level watch is proven live, and every filesystem event
/// that happened before the probe file's final write has been dispatched by
/// the actor (the actor reports probe sightings in event order).
///
/// The probe file is inert: it has no model extension, so it never enters the
/// registry and never produces an event-bus message. Callers therefore do not
/// need to drain anything afterwards.
///
/// Prefer the intention-revealing wrappers [`wait_for_watcher_ready`] and
/// [`watcher_barrier`] at call sites.
async fn probe_round_trip(root: &Path, sightings: &mut watch::Receiver<u64>) -> Result<(), String> {
    let nonce = NEXT_PROBE_NONCE.fetch_add(1, Ordering::Relaxed);
    let path = root.join(format!("{PROBE_FILE_PREFIX}{nonce}"));

    // Ignore sightings from any earlier round; we only care about our nonce.
    sightings.borrow_and_update();

    let deadline = tokio::time::Instant::now() + PROBE_BUDGET;
    let mut attempt: u64 = 0;
    let observed = loop {
        attempt += 1;
        // Distinct content per attempt so a rewrite is always a real change,
        // never a no-op the OS could legitimately decline to report.
        if let Err(err) = tokio::fs::write(&path, attempt.to_string().as_bytes()).await {
            return Err(format!("probe write to {}: {err}", path.display()));
        }

        if *sightings.borrow_and_update() >= nonce {
            break true;
        }
        match tokio::time::timeout(PROBE_RETRY_INTERVAL, sightings.changed()).await {
            // Sender dropped: the actor exited, so no probe will ever land.
            Ok(Err(_)) => break false,
            Ok(Ok(())) => {
                if *sightings.borrow_and_update() >= nonce {
                    break true;
                }
            }
            Err(_) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            break false;
        }
    };

    // Remove the probe whether or not it was seen: leaving it behind would
    // make a later `git status` in the same tree report an untracked file.
    let _ = tokio::fs::remove_file(&path).await;

    if observed {
        Ok(())
    } else {
        Err(format!(
            "watcher never reported probe {nonce} under {} after {attempt} attempts in {PROBE_BUDGET:?}",
            root.display()
        ))
    }
}

/// Block until the OS-level watch is live, then return immediately.
///
/// Call this after `spawn_watcher` and **immediately before** the action whose
/// event the test is waiting for. Do not sleep in between.
///
/// Two distinct hazards make this necessary, and a fixed sleep addresses
/// neither:
///
/// 1. `Watcher::watch` returns before the watch is registered with the OS
///    (on macOS, before `fseventsd` has accepted the stream). Any change made
///    in that window is never reported -- the test then waits out its full
///    timeout for an event that was never generated.
/// 2. On a loaded macOS host an FSEvent stream that has gone quiet for a
///    second or two appears to drop the *next* event it should have reported.
///    We measured a single write after a 2s idle gap being lost 9 times out of
///    10, while the same write issued straight after an observed probe landed
///    10 times out of 10. Keeping the trigger adjacent to the probe is what
///    makes the trigger observable.
///
/// # Panics
/// Never; returns `Err` describing the failure so the caller can `.expect()`
/// with test-specific context.
pub async fn wait_for_watcher_ready(
    root: &Path,
    sightings: &mut watch::Receiver<u64>,
) -> Result<(), String> {
    probe_round_trip(root, sightings).await
}

/// Ordering barrier: block until the watcher has dispatched every filesystem
/// event that occurred before this call.
///
/// Call this after a trigger action and before asserting that some event was
/// *not* emitted. Without it, "no event arrived within N milliseconds" is a
/// statement about the machine's speed, not about the watcher's behaviour, and
/// the assertion passes vacuously whenever the OS is slower than N.
///
/// Soundness rests on two properties: the OS reports events for a single watch
/// in the order they happened, and the actor reports a probe sighting only
/// after dispatching the events that preceded it in the same batch. So once
/// our probe's nonce is visible, the trigger's event -- if the OS produced one
/// at all -- has already been classified, handled, and broadcast.
pub async fn watcher_barrier(
    root: &Path,
    sightings: &mut watch::Receiver<u64>,
) -> Result<(), String> {
    probe_round_trip(root, sightings).await
}
