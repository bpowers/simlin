// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The crate's lock order, stated once and checked where a lock is taken.
//!
//! A thread takes the crate's locks in one order -- a tool session, then a
//! project's contents, then its database, then a simulation's state
//! ([`Rank`]) -- and never two of one rank, so no two threads can each wait
//! for a lock the other holds. Every such lock is an [`OrderedMutex`], whose
//! `lock` and `try_lock` check the order against what the calling thread
//! holds BEFORE they wait: in a build with debug assertions (and in this
//! crate's tests) an out-of-order acquisition panics on the thread that makes
//! it, the first time it runs, where an unchecked one deadlocks only when two
//! threads interleave badly. A release build checks nothing and an
//! `OrderedMutex` is a `Mutex`.

use std::ops::{Deref, DerefMut};
use std::sync::{LockResult, Mutex, MutexGuard, PoisonError, TryLockError, TryLockResult};

/// What a lock guards, in the order a thread takes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rank {
    /// A tool session (`SimlinToolSession`).
    Session,
    /// A project's contents (`SimlinProject::datamodel`).
    Contents,
    /// A project's salsa database.
    Database,
    /// A simulation's state (`SimlinSim::state`).
    SimState,
}

/// A mutex with a place in the crate's lock order.
pub struct OrderedMutex<T> {
    rank: Rank,
    inner: Mutex<T>,
}

impl<T> OrderedMutex<T> {
    pub fn new(rank: Rank, value: T) -> OrderedMutex<T> {
        OrderedMutex {
            rank,
            inner: Mutex::new(value),
        }
    }

    /// `Mutex::lock`, after checking that the calling thread holds no lock
    /// of this rank or a later one.
    pub fn lock(&self) -> LockResult<OrderedGuard<'_, T>> {
        let held = tracking::acquiring(self.rank);
        match self.inner.lock() {
            Ok(guard) => Ok(OrderedGuard { guard, _held: held }),
            Err(poisoned) => Err(PoisonError::new(OrderedGuard {
                guard: poisoned.into_inner(),
                _held: held,
            })),
        }
    }

    /// `Mutex::try_lock`, with the same check.
    pub fn try_lock(&self) -> TryLockResult<OrderedGuard<'_, T>> {
        let held = tracking::acquiring(self.rank);
        match self.inner.try_lock() {
            Ok(guard) => Ok(OrderedGuard { guard, _held: held }),
            Err(TryLockError::WouldBlock) => Err(TryLockError::WouldBlock),
            Err(TryLockError::Poisoned(poisoned)) => {
                Err(TryLockError::Poisoned(PoisonError::new(OrderedGuard {
                    guard: poisoned.into_inner(),
                    _held: held,
                })))
            }
        }
    }
}

/// An [`OrderedMutex`], locked. The fields drop in order, so the lock is
/// released before the thread stops counting it as held.
pub struct OrderedGuard<'a, T> {
    guard: MutexGuard<'a, T>,
    _held: tracking::Held,
}

impl<T> Deref for OrderedGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<T> DerefMut for OrderedGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

#[cfg(any(test, debug_assertions))]
mod tracking {
    use super::Rank;
    use std::cell::RefCell;

    thread_local! {
        /// The ranks of the locks this thread holds or waits for, in the
        /// order it took them.
        static HELD: RefCell<Vec<Rank>> = const { RefCell::new(Vec::new()) };
    }

    /// A lock the thread counts as held, until it drops.
    pub(super) struct Held(Rank);

    /// Count a lock of `rank` as held by this thread from before it waits
    /// for it, panicking if the thread holds one of that rank or a later one.
    pub(super) fn acquiring(rank: Rank) -> Held {
        HELD.with(|held| {
            let mut held = held.borrow_mut();
            if let Some(&last) = held.iter().max() {
                assert!(
                    last < rank,
                    "lock order violation: taking a {rank:?} lock while holding a {last:?} lock \
                     (the order is Session, Contents, Database, SimState, one of each)"
                );
            }
            held.push(rank);
        });
        #[cfg(test)]
        super::trace::record(super::Event::Acquire(rank));
        Held(rank)
    }

    impl Drop for Held {
        fn drop(&mut self) {
            HELD.with(|held| {
                let mut held = held.borrow_mut();
                if let Some(at) = held.iter().rposition(|&rank| rank == self.0) {
                    held.remove(at);
                }
            });
            #[cfg(test)]
            super::trace::record(super::Event::Release(self.0));
        }
    }
}

#[cfg(not(any(test, debug_assertions)))]
mod tracking {
    use super::Rank;

    pub(super) struct Held;

    #[inline(always)]
    pub(super) fn acquiring(_rank: Rank) -> Held {
        Held
    }
}

/// What a thread did with the crate's locks, for a test to read back.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Event {
    /// The thread took a lock, or began to wait for it.
    Acquire(Rank),
    Release(Rank),
    /// The thread used the project's database through its lock.
    UseDatabase,
}

/// A thread's record of its lock events over a traced call: how a test reads
/// which locks an entry point takes, in what order, and whether it works on
/// the database while it holds the contents -- from the call itself, on one
/// thread, with no timing.
#[cfg(test)]
pub(crate) mod trace {
    use super::Event;
    use std::cell::RefCell;

    thread_local! {
        static TRACE: RefCell<Option<Vec<Event>>> = const { RefCell::new(None) };
    }

    pub(crate) fn record(event: Event) {
        TRACE.with(|trace| {
            if let Some(events) = trace.borrow_mut().as_mut() {
                // A run of uses reads as one.
                if event != Event::UseDatabase || events.last() != Some(&event) {
                    events.push(event);
                }
            }
        });
    }

    /// Run `f` and return the lock events it made on this thread.
    pub(crate) fn of<T>(f: impl FnOnce() -> T) -> (T, Vec<Event>) {
        TRACE.with(|trace| *trace.borrow_mut() = Some(Vec::new()));
        let value = f();
        let events = TRACE.with(|trace| trace.borrow_mut().take().unwrap_or_default());
        (value, events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panics(f: impl FnOnce() + std::panic::UnwindSafe) -> Option<String> {
        std::panic::catch_unwind(f).err().map(|payload| {
            payload
                .downcast_ref::<String>()
                .cloned()
                .unwrap_or_default()
        })
    }

    /// The order's every pair: a later rank after an earlier one is taken,
    /// and the same rank or an earlier one is refused before the thread waits
    /// for it.
    #[test]
    fn a_lock_is_taken_only_after_the_ranks_before_it() {
        const ALL: [Rank; 4] = [
            Rank::Session,
            Rank::Contents,
            Rank::Database,
            Rank::SimState,
        ];
        for first in ALL {
            for second in ALL {
                let message = panics(move || {
                    let (a, b) = (OrderedMutex::new(first, ()), OrderedMutex::new(second, ()));
                    let _a = a.lock().unwrap();
                    let _b = b.lock().unwrap();
                });
                if first < second {
                    assert_eq!(message, None, "{second:?} after {first:?}");
                } else {
                    let message = message
                        .unwrap_or_else(|| panic!("{second:?} while holding {first:?} was taken"));
                    assert!(message.contains("lock order violation"), "{message}");
                }
            }
        }
    }

    /// A lock released is no longer held: the same rank can be taken again,
    /// and a lock a `try_lock` did not get was never held.
    #[test]
    fn a_released_lock_and_a_failed_try_are_not_held() {
        let contents = OrderedMutex::new(Rank::Contents, ());
        drop(contents.lock().unwrap());
        drop(contents.lock().unwrap());

        let database = OrderedMutex::new(Rank::Database, ());
        let held = database.lock().unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                assert!(matches!(database.try_lock(), Err(TryLockError::WouldBlock)));
                // Had the failed try stayed counted, this would be refused.
                drop(contents.lock().unwrap());
            });
        });
        drop(held);
    }

    /// The trace a test reads: each acquisition and release, in order.
    #[test]
    fn a_trace_lists_what_the_thread_took_and_released() {
        let (contents, database) = (
            OrderedMutex::new(Rank::Contents, ()),
            OrderedMutex::new(Rank::Database, ()),
        );
        let ((), events) = trace::of(|| {
            let c = contents.lock().unwrap();
            let d = database.lock().unwrap();
            drop(c);
            drop(d);
        });
        assert_eq!(
            events,
            [
                Event::Acquire(Rank::Contents),
                Event::Acquire(Rank::Database),
                Event::Release(Rank::Contents),
                Event::Release(Rank::Database),
            ]
        );
    }
}
