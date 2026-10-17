//! Integration tests for the scoped job system.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};

use moba_jobs::JobSystem;

#[test]
#[cfg_attr(miri, ignore = "too slow under Miri")]
fn scoped_jobs_run_exactly_once() {
    let system = JobSystem::new(3);
    let counter = AtomicUsize::new(0);
    system.scope(|scope| {
        for _ in 0..10_000 {
            scope.spawn(|| {
                counter.fetch_add(1, Ordering::SeqCst);
            });
        }
    });
    assert_eq!(counter.load(Ordering::SeqCst), 10_000);
}

#[test]
fn scoped_jobs_can_borrow_caller_data() {
    let system = JobSystem::new(2);
    let mut left = vec![0u32; 4_096];
    let mut right = vec![0u32; 4_096];
    {
        let left = &mut left;
        let right = &mut right;
        system.scope(|scope| {
            scope.spawn(move || left.fill(1));
            scope.spawn(move || right.fill(2));
        });
    }
    assert!(left.iter().all(|&value| value == 1));
    assert!(right.iter().all(|&value| value == 2));
}

#[test]
#[cfg_attr(miri, ignore = "too slow under Miri")]
fn nested_scopes_complete() {
    let system = JobSystem::new(2);
    let counter = AtomicUsize::new(0);
    system.scope(|scope| {
        for _ in 0..32 {
            scope.spawn(|| {
                system.scope(|inner| {
                    for _ in 0..32 {
                        inner.spawn(|| {
                            counter.fetch_add(1, Ordering::SeqCst);
                        });
                    }
                });
            });
        }
    });
    assert_eq!(counter.load(Ordering::SeqCst), 32 * 32);
}

#[test]
fn zero_worker_pool_still_runs_jobs() {
    let system = JobSystem::new(0);
    let counter = AtomicUsize::new(0);
    system.scope(|scope| {
        for _ in 0..1_000 {
            scope.spawn(|| {
                counter.fetch_add(1, Ordering::SeqCst);
            });
        }
    });
    assert_eq!(counter.load(Ordering::SeqCst), 1_000);
}

#[test]
fn job_panics_are_reported_after_all_work() {
    let system = JobSystem::new(2);
    let completed = AtomicUsize::new(0);
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        system.scope(|scope| {
            for _ in 0..100 {
                scope.spawn(|| {
                    completed.fetch_add(1, Ordering::SeqCst);
                });
            }
            scope.spawn(|| panic!("job exploded"));
        });
    }));
    assert!(result.is_err(), "the scope must re-raise the job panic");
    assert_eq!(
        completed.load(Ordering::SeqCst),
        100,
        "every other job must still finish"
    );
}

#[test]
#[should_panic(expected = "scope body exploded")]
fn panics_in_the_scope_body_still_wait_for_jobs() {
    let system = JobSystem::new(2);
    let completed = AtomicUsize::new(0);
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        system.scope(|scope| {
            for _ in 0..100 {
                scope.spawn(|| {
                    std::thread::sleep(std::time::Duration::from_micros(50));
                    completed.fetch_add(1, Ordering::SeqCst);
                });
            }
            panic!("scope body exploded");
        });
    }));
    // Re-raise the body panic to satisfy `should_panic` while proving jobs finished.
    assert_eq!(completed.load(Ordering::SeqCst), 100);
    panic!("scope body exploded");
}

#[test]
#[cfg_attr(miri, ignore = "too slow under Miri")]
fn parallel_for_covers_disjoint_chunks() {
    let system = JobSystem::new(3);
    let mut data = vec![0u32; 10_000];
    system.parallel_for(&mut data, 256, &|chunk_index, chunk| {
        for (offset, value) in chunk.iter_mut().enumerate() {
            *value = (chunk_index * 1_000 + offset) as u32;
        }
    });
    for (chunk_index, chunk) in data.chunks(256).enumerate() {
        for (offset, value) in chunk.iter().enumerate() {
            assert_eq!(*value, (chunk_index * 1_000 + offset) as u32);
        }
    }
}

#[test]
fn parallel_for_handles_empty_and_small_inputs() {
    let system = JobSystem::new(2);
    let mut empty: Vec<u32> = Vec::new();
    system.parallel_for(&mut empty, 64, &|_, _| panic!("must not run"));
    let mut one = [7u32];
    system.parallel_for(&mut one, 64, &|_, chunk| {
        assert_eq!(chunk, &mut [7]);
        chunk[0] = 9;
    });
    assert_eq!(one, [9]);
}

#[test]
fn nested_parallel_for_is_safe() {
    let system = JobSystem::new(2);
    let mut data = vec![0u32; 1_000];
    system.parallel_for(&mut data, 100, &|_, chunk| {
        system.parallel_for(chunk, 10, &|_, inner| {
            for value in inner {
                *value += 1;
            }
        });
    });
    assert!(data.iter().all(|&value| value == 1));
}

#[test]
#[cfg_attr(miri, ignore = "too slow under Miri")]
fn repeated_scopes_do_not_leak_or_deadlock() {
    let system = JobSystem::new(2);
    let counter = AtomicUsize::new(0);
    for _ in 0..500 {
        system.scope(|scope| {
            for _ in 0..64 {
                scope.spawn(|| {
                    counter.fetch_add(1, Ordering::SeqCst);
                });
            }
        });
    }
    assert_eq!(counter.load(Ordering::SeqCst), 500 * 64);
}
