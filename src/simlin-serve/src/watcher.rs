// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
// `WatcherActor` owns the OS-level filesystem watch (via
// `notify-debouncer-full`'s recommended platform watcher) and feeds debounced
// batches into a tokio task that classifies and dispatches them. The
// debouncer runs its tick loop on a dedicated OS thread it spawns
// internally; we bridge it into tokio via an unbounded mpsc channel and a
// closure-based `DebounceEventHandler`. Note 1 in the phase plan called for
// the crate's `tokio` feature, but that feature only exists in 0.8+; on
// 0.7 we use the `FnMut(DebounceEventResult) + Send + 'static` blanket
// impl with a captured `tokio::sync::mpsc::UnboundedSender` to achieve the
// same wiring without the unstable RC.

//! Filesystem watcher actor for the Phase 4 disk -> Loro merge path.
//!
//! Architecture (per docs/design-plans/2026-04-05-server-rewrite.md):
//! a long-lived `WatcherActor` watches `state.root` recursively. The
//! debouncer coalesces bursts of events into 100ms batches; each batch
//! lands on the actor's tokio mpsc receiver. The actor's `run` loop
//! `tokio::select!`s over (a) `rx.recv()` for new batches and
//! (b) `shutdown.notified()` for graceful teardown.
//!
//! The actor's job in Subcomponent A is plumbing only. Subcomponent B
//! will fill in the per-event handlers (`handle_model_change`,
//! `handle_model_removal`, `handle_git_change`) so the watcher actually
//! drives merges; for now those handlers are `tracing::debug!` stubs.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use notify_debouncer_full::notify::{self, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

use crate::handlers::AppState;

/// Cross-task shutdown notifier. `main()` constructs one and passes clones
/// into long-lived actors (the watcher in Phase 4; the websocket loops can
/// adopt the same pattern in later phases). Calling `notify_waiters` on
/// the shared `Notify` wakes all `notified()` futures held inside
/// `tokio::select!` arms so each actor can exit its loop cleanly.
pub type ShutdownSignal = Arc<Notify>;

/// Errors raised while constructing or operating the watcher actor.
///
/// `Debouncer` carries the underlying `notify::Error` from the OS
/// watcher setup (most commonly: the root path doesn't exist or isn't a
/// directory, or the kernel's inotify/FSEvents/ReadDirectoryChangesW
/// resource limits are saturated).
#[derive(Debug)]
pub enum WatcherError {
    Debouncer(notify::Error),
}

impl std::fmt::Display for WatcherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WatcherError::Debouncer(e) => write!(f, "filesystem watcher: {e}"),
        }
    }
}

impl std::error::Error for WatcherError {}

impl From<notify::Error> for WatcherError {
    fn from(value: notify::Error) -> Self {
        WatcherError::Debouncer(value)
    }
}

/// Test-only side channel for observing raw debounced events.
///
/// The watcher's normal path is to classify and dispatch events through
/// the actor's handlers. The smoke test (`watcher_smoke.rs`) needs to
/// observe arrivals before the classification logic exists; rather than
/// teach `handle_batch` about test hooks, we forward each batch to an
/// optional `TestHook` *before* dispatching, so production code is
/// unaffected and the test gets a deterministic observation point.
#[derive(Clone)]
pub struct TestHook {
    sender: UnboundedSender<DebounceEventResult>,
}

impl TestHook {
    pub fn new(sender: UnboundedSender<DebounceEventResult>) -> Self {
        Self { sender }
    }
}

/// Long-lived actor that bridges the OS filesystem watcher into tokio.
pub struct WatcherActor {
    #[allow(dead_code)]
    state: AppState,
    rx: UnboundedReceiver<DebounceEventResult>,
    /// Owned shutdown future captured *synchronously* in `spawn_watcher`
    /// before the actor task starts running. This closes a race that
    /// matters in tests (and could matter in production on a
    /// fast-shutdown path): if `notify_waiters()` runs between the
    /// spawn and the actor's first poll, a future created lazily inside
    /// the loop would miss it. `Notify::notified_owned` captures the
    /// `num_notify_waiters_calls` counter at construction time, so a
    /// notification that arrives before the first poll still wakes us.
    shutdown: tokio::sync::futures::OwnedNotified,
    test_hook: Option<TestHook>,
    /// Hold the `Debouncer` so its OS-level watch and tick thread stay
    /// alive for the actor's lifetime. Dropping the actor drops the
    /// debouncer, which signals its background thread to stop and
    /// releases the kernel-level watch.
    _debouncer: Debouncer<RecommendedWatcher, RecommendedCache>,
}

impl WatcherActor {
    /// Drive the actor until the shutdown signal fires or the channel
    /// closes (which would indicate the debouncer crashed).
    ///
    /// The shutdown `OwnedNotified` future was captured synchronously
    /// in `spawn_watcher` (before this task started running); pinning
    /// it here just lets us poll it via `select!`. The capture-time
    /// counter on `OwnedNotified` is what makes
    /// `notify_waiters()`-then-`spawn_watcher` and
    /// `spawn_watcher`-then-`notify_waiters` both correct.
    async fn run(self) {
        let WatcherActor {
            state,
            mut rx,
            shutdown,
            test_hook,
            _debouncer,
        } = self;
        let mut shutdown = Box::pin(shutdown);
        loop {
            tokio::select! {
                Some(result) = rx.recv() => {
                    if let Some(hook) = test_hook.as_ref() {
                        // Forward an observation copy to the test hook
                        // before consuming `result` for dispatch. The hook
                        // sees the success vec or a re-emitted set of
                        // errors; the underlying `notify::Error` is not
                        // `Clone`, so we rebuild Err entries through their
                        // Display form.
                        let observation: DebounceEventResult = match &result {
                            Ok(events) => Ok(events.clone()),
                            Err(errors) => Err(errors
                                .iter()
                                .map(|e| notify::Error::generic(&format!("{e}")))
                                .collect()),
                        };
                        let _ = hook.sender.send(observation);
                    }
                    Self::handle_batch(&state, result).await;
                }
                _ = &mut shutdown => {
                    tracing::debug!("watcher actor: shutdown signal received");
                    break;
                }
            }
        }
        drop(_debouncer);
    }

    /// Stub for Subcomponent A: just log. Subcomponent B replaces this
    /// with per-event classification + dispatch (Tasks 3, 4, 5, 6, 7).
    ///
    /// Takes `&AppState` rather than `&self` because `run` already
    /// destructured `self` into individual fields to keep the partial
    /// borrow checker happy across the `select!` arms.
    async fn handle_batch(_state: &AppState, result: DebounceEventResult) {
        match result {
            Ok(events) => {
                tracing::info!(
                    count = events.len(),
                    "watcher actor: received debounced batch"
                );
            }
            Err(errors) => {
                for err in errors {
                    tracing::warn!(error = %err, "watcher actor: debouncer error");
                }
            }
        }
    }
}

/// Construct and spawn the watcher actor.
///
/// Returns the join handle for the spawned tokio task. The caller must
/// hold onto it (for graceful shutdown / wait-on-exit) — dropping the
/// handle does not abort the task. The debouncer is moved into the actor
/// and dropped when the task exits, releasing the OS-level watch.
///
/// `test_hook` is `None` in production. Smoke tests pass `Some(hook)` to
/// observe raw debounced events without depending on the merge layer.
pub fn spawn_watcher(
    state: AppState,
    shutdown: ShutdownSignal,
    test_hook: Option<TestHook>,
) -> Result<JoinHandle<()>, WatcherError> {
    let (tx, rx) = unbounded_channel::<DebounceEventResult>();

    // Bridge from the debouncer's `DebounceEventHandler` (called on the
    // debouncer's OS thread) into tokio via an unbounded mpsc. The
    // closure satisfies the `FnMut + Send + 'static` blanket impl on
    // `DebounceEventHandler`. `send` drops events if the receiver is
    // gone (i.e. the actor exited); that's the desired behavior.
    let bridge = move |result: DebounceEventResult| {
        let _ = tx.send(result);
    };

    // 100ms debounce window -- plan note 1. tick_rate=None means the
    // debouncer picks 1/4 of the timeout (25ms) automatically.
    let mut debouncer = new_debouncer(Duration::from_millis(100), None, bridge)?;

    // Watch the root recursively (plan note 2: file-level watches die
    // after atomic_write's rename, so we anchor on the parent directory
    // and let the debouncer's file-id cache normalize the final path).
    let root: &Path = state.root.as_ref();
    debouncer.watch(root, RecursiveMode::Recursive)?;

    // Capture the shutdown future synchronously so the
    // `num_notify_waiters_calls` snapshot is taken *before* the spawned
    // task ever runs. This matters when a caller does
    // `spawn_watcher(...); shutdown.notify_waiters();` in close
    // succession (notably in tests): with a lazy-construction approach
    // the actor would register its waiter after the notification was
    // already counted, and the notification would be silently lost.
    let shutdown_future = shutdown.notified_owned();

    let actor = WatcherActor {
        state,
        rx,
        shutdown: shutdown_future,
        test_hook,
        _debouncer: debouncer,
    };

    let handle = tokio::spawn(actor.run());
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper: build a minimal AppState rooted at `dir`.
    fn build_app_state(dir: &Path) -> AppState {
        use crate::events::EventBus;
        use crate::git::GitProbe;
        use crate::registry::ProjectRegistry;

        let canonical = dir.canonicalize().expect("canonicalize");
        AppState {
            registry: Arc::new(ProjectRegistry::new(canonical.clone())),
            git: Arc::new(GitProbe::unavailable_for_tests()),
            root: Arc::new(canonical),
            events: Arc::new(EventBus::new()),
            launch_token: Arc::new("test-token".to_string()),
        }
    }

    #[tokio::test]
    async fn spawn_watcher_returns_join_handle_and_terminates_on_shutdown() {
        // Construct a watcher rooted at a tempdir; verify the spawn returns
        // a join handle, the shutdown signal terminates the actor cleanly,
        // and the join handle resolves promptly.
        let dir = TempDir::new().expect("tempdir");
        let state = build_app_state(dir.path());
        let shutdown: ShutdownSignal = Arc::new(Notify::new());

        let handle =
            spawn_watcher(state, shutdown.clone(), None).expect("watcher spawns successfully");

        shutdown.notify_waiters();

        tokio::time::timeout(Duration::from_millis(500), handle)
            .await
            .expect("actor exited within 500ms")
            .expect("actor task did not panic");
    }

    #[test]
    fn watcher_error_displays_underlying_notify_message() {
        // Smoke check: WatcherError -> Display delegates to the inner
        // notify::Error so server logs show actionable detail.
        let inner = notify::Error::generic("boom");
        let err = WatcherError::from(inner);
        let msg = format!("{err}");
        assert!(msg.contains("filesystem watcher"));
        assert!(msg.contains("boom"));
    }
}
