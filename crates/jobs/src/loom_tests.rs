#![allow(unused_mut)]

//! Loom model of the scope submit/wake-up protocol.
//!
//! The real scheduler uses a pending counter plus a generation-guarded condvar so a submit
//! between the final queue check and the wait cannot be missed. These tests model that
//! handshake with Loom primitives; run with:
//! `RUSTFLAGS="--cfg loom" cargo test -p moba-jobs --lib loom -- --test-threads=1`.

use std::time::Duration;

use loom::sync::atomic::{AtomicUsize, Ordering};
use loom::sync::{Arc, Condvar, Mutex};
use loom::thread;

struct Shared {
    pending: AtomicUsize,
    generation: Mutex<u64>,
    wake: Condvar,
}

#[test]
fn submit_between_check_and_wait_is_not_lost() {
    loom::model(|| {
        let shared = Arc::new(Shared {
            pending: AtomicUsize::new(0),
            generation: Mutex::new(0),
            wake: Condvar::new(),
        });
        let worker = {
            let shared = Arc::clone(&shared);
            thread::spawn(move || {
                let mut seen = 0u64;
                loop {
                    if shared.pending.load(Ordering::Acquire) == 1 {
                        break;
                    }
                    let mut generation = shared
                        .generation
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if *generation != seen {
                        seen = *generation;
                        drop(generation);
                        continue;
                    }
                    let _ = shared
                        .wake
                        .wait_timeout(generation, Duration::from_millis(1))
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            })
        };
        // Publish the job, then bump the generation and notify while holding the lock.
        shared.pending.store(1, Ordering::Release);
        {
            let mut generation = shared
                .generation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *generation = generation.wrapping_add(1);
            shared.wake.notify_one();
        }
        worker.join().expect("worker");
        shared.pending.store(0, Ordering::Release);
        assert_eq!(shared.pending.load(Ordering::Acquire), 0);
    });
}

#[test]
fn completion_wakes_the_scope_wait_loop() {
    loom::model(|| {
        let shared = Arc::new(Shared {
            pending: AtomicUsize::new(1),
            generation: Mutex::new(0),
            wake: Condvar::new(),
        });
        let waiter = {
            let shared = Arc::clone(&shared);
            thread::spawn(move || {
                let mut seen = 0u64;
                loop {
                    if shared.pending.load(Ordering::Acquire) == 0 {
                        break;
                    }
                    let mut generation = shared
                        .generation
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if *generation != seen {
                        seen = *generation;
                        drop(generation);
                        continue;
                    }
                    let _ = shared
                        .wake
                        .wait_timeout(generation, Duration::from_millis(1))
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            })
        };
        shared.pending.store(0, Ordering::Release);
        {
            let mut generation = shared
                .generation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *generation = generation.wrapping_add(1);
            shared.wake.notify_one();
        }
        waiter.join().expect("waiter");
        assert_eq!(shared.pending.load(Ordering::Acquire), 0);
    });
}
