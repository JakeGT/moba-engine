//! Implementation of the scoped work-stealing pool.

use core::any::Any;
use core::fmt;
use core::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_deque::{Injector, Steal, Stealer, Worker};

/// A lifetime-erased job.
///
/// # Safety invariant
/// `data` is a `Box<F>` created by [`Scope::spawn`] where `F: FnOnce() + Send + 'scope`.
/// The scope's pending counter is incremented before the job is published and decremented
/// only after the closure has run. The scope state is reference-counted (`Arc`), so even a
/// late/incorrect decrement can never dereference a dead stack frame; the scope still waits
/// for the counter before returning so every job sees valid borrows.
struct JobRef {
    invoke: unsafe fn(*const ()),
    data: *const (),
    shared: Arc<ScopeShared>,
}

// SAFETY: `JobRef` is only sent to workers as part of `scope`, which requires the erased
// closure to be `Send`. The `data` pointer is only dereferenced by the job's `invoke`
// function, and the scope keeps all borrowed data alive until its pending counter reaches
// zero. `Arc` keeps the counter/panic slot memory safe independently of the scope frame.
unsafe impl Send for JobRef {}

/// Shared state between a scope and its jobs, kept alive by `Arc`.
struct ScopeShared {
    pending: AtomicUsize,
    panic: Mutex<Option<Box<dyn Any + Send>>>,
}

impl ScopeShared {
    fn new() -> Self {
        Self {
            pending: AtomicUsize::new(0),
            panic: Mutex::new(None),
        }
    }
}

struct PendingGuard(Arc<ScopeShared>);

impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.0.pending.fetch_sub(1, Ordering::AcqRel);
    }
}

unsafe fn invoke_boxed<F: FnOnce()>(data: *const ()) {
    // SAFETY: `data` was produced by `Box::into_raw(Box<F>)` in `Scope::spawn`, is
    // invoked exactly once, and the resulting box is dropped after the call.
    let closure = unsafe { Box::from_raw(data.cast_mut().cast::<F>()) };
    closure();
}

/// Runs a job and records a panic for the scope to re-raise.
#[allow(clippy::needless_pass_by_value)] // Owning the JobRef mirrors the job being consumed.
fn execute(job: JobRef) {
    let _pending = PendingGuard(Arc::clone(&job.shared));
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the function pointer and data pointer form a valid `Box<F>` for the
        // duration of this call, as guaranteed by `JobRef`'s invariant.
        unsafe { (job.invoke)(job.data) };
    }));
    if let Err(payload) = result {
        let mut slot = job
            .shared
            .panic
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(payload);
        }
    }
}

#[derive(Default)]
struct SleepState {
    generation: u64,
}

struct Shared {
    injector: Injector<JobRef>,
    stealers: Vec<Stealer<JobRef>>,
    sleep: Mutex<SleepState>,
    wake: Condvar,
    shutdown: AtomicBool,
}

/// Takes one job from the global queue or another worker.
fn steal_job(shared: &Shared, skip: Option<usize>) -> Option<JobRef> {
    if let Steal::Success(job) = shared.injector.steal() {
        return Some(job);
    }
    for (index, stealer) in shared.stealers.iter().enumerate() {
        if skip == Some(index) {
            continue;
        }
        if let Steal::Success(job) = stealer.steal() {
            return Some(job);
        }
    }
    if let Steal::Success(job) = shared.injector.steal() {
        return Some(job);
    }
    None
}

fn next_job(worker: &Worker<JobRef>, shared: &Shared, index: usize) -> Option<JobRef> {
    if let Some(job) = worker.pop() {
        return Some(job);
    }
    steal_job(shared, Some(index))
}

#[allow(clippy::needless_pass_by_value)] // The worker and Arc are owned for the thread's lifetime.
fn worker_loop(index: usize, worker: Worker<JobRef>, shared: Arc<Shared>) {
    let mut last_generation = 0u64;
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            break;
        }
        if let Some(job) = next_job(&worker, &shared, index) {
            execute(job);
            continue;
        }
        // Nothing to do. Sleep on the condvar, but only after re-checking the generation:
        // a submit that lands between `next_job` and the wait bumps the generation, and
        // the worker must loop and look again instead of missing the wake-up.
        let state = shared.sleep.lock().unwrap_or_else(PoisonError::into_inner);
        if state.generation != last_generation {
            last_generation = state.generation;
            drop(state);
            continue;
        }
        let result = shared.wake.wait_timeout(state, Duration::from_millis(2));
        if let Ok((state, _)) = result {
            last_generation = state.generation;
        }
    }
}

/// A scoped work-stealing executor.
///
/// Create one per process (or per server match shard) and reuse it. Do not create a pool
/// per frame.
pub struct JobSystem {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl JobSystem {
    /// Creates a pool with `worker_threads` background workers.
    ///
    /// The calling thread participates while it waits inside [`JobSystem::scope`], so a
    /// pool with zero workers is still a valid (single-threaded) executor.
    pub fn new(worker_threads: usize) -> Self {
        let mut workers = Vec::with_capacity(worker_threads);
        for _ in 0..worker_threads {
            workers.push(Worker::new_fifo());
        }
        let stealers = workers.iter().map(Worker::stealer).collect();
        let shared = Arc::new(Shared {
            injector: Injector::new(),
            stealers,
            sleep: Mutex::new(SleepState::default()),
            wake: Condvar::new(),
            shutdown: AtomicBool::new(false),
        });
        let mut threads = Vec::with_capacity(worker_threads);
        for (index, worker) in workers.into_iter().enumerate() {
            let shared = Arc::clone(&shared);
            let handle = thread::Builder::new()
                .name(format!("moba-job-{index}"))
                .spawn(move || worker_loop(index, worker, shared))
                .expect("failed to spawn a job worker thread");
            threads.push(handle);
        }
        Self { shared, threads }
    }

    /// Creates a pool with one fewer worker than the machine has logical cores.
    ///
    /// Reserving one core for the main thread keeps input and presentation responsive
    /// while simulation jobs run.
    pub fn new_auto() -> Self {
        let cores = thread::available_parallelism().map_or(1, core::num::NonZeroUsize::get);
        Self::new(cores.saturating_sub(1))
    }

    /// Number of background worker threads.
    #[must_use]
    pub fn worker_count(&self) -> usize {
        self.threads.len()
    }

    /// Runs `f` with a scope, then waits for every job spawned into it.
    ///
    /// Spawned closures may borrow data that lives for the duration of this call. Panics
    /// from jobs are re-raised on the calling thread once all jobs have finished.
    pub fn scope<'scope, F>(&'scope self, f: F)
    where
        F: FnOnce(&Scope<'scope>) + 'scope,
    {
        let shared = Arc::new(ScopeShared::new());
        let scope = Scope {
            system: self,
            shared: Arc::clone(&shared),
            marker: PhantomData,
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| f(&scope)));
        while shared.pending.load(Ordering::Acquire) != 0 {
            if let Some(job) = steal_job(&self.shared, None) {
                execute(job);
            } else {
                std::hint::spin_loop();
                thread::yield_now();
            }
        }
        let job_panic = shared
            .panic
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Err(payload) = outcome {
            resume_unwind(payload);
        }
        if let Some(payload) = job_panic {
            resume_unwind(payload);
        }
    }

    /// Splits `data` into chunks and runs `f(chunk_index, chunk)` for each chunk in parallel.
    ///
    /// Chunks are disjoint, so mutating them is safe. The closure is taken by reference so
    /// it can be shared across jobs without a lock.
    ///
    /// # Panics
    /// Panics if `chunk_size` exceeds `isize::MAX` (impossible on 64-bit in practice) or if
    /// a job panics.
    pub fn parallel_for<'s, T, F>(&'s self, data: &'s mut [T], chunk_size: usize, f: &'s F)
    where
        T: Send + 's,
        F: Fn(usize, &mut [T]) + Sync + 's,
    {
        if data.is_empty() {
            return;
        }
        let chunk_size = chunk_size.max(1);
        let len = data.len();
        let chunks = len.div_ceil(chunk_size);
        let base = data.as_mut_ptr() as usize;
        self.scope(|scope| {
            for chunk_index in 0..chunks {
                let start = chunk_index * chunk_size;
                let count = (len - start).min(chunk_size);
                scope.spawn(move || {
                    // SAFETY: each job owns the disjoint half-open range
                    // `[start, start + count)`, so no two jobs alias. The pointer was
                    // derived from `data`, which is borrowed for the whole scope, and the
                    // scope waits for every job before returning.
                    let base_ptr = base as *mut T;
                    // SAFETY: `start + count <= len`, so the offset stays in bounds.
                    let start_ptr = unsafe { base_ptr.add(start) };
                    // SAFETY: the range is disjoint from every other job's range and outlives
                    // the scope; the caller's borrow prevents any other access.
                    let slice = unsafe { std::slice::from_raw_parts_mut(start_ptr, count) };
                    f(chunk_index, slice);
                });
            }
        });
    }

    fn submit(&self, job: JobRef) {
        self.shared.injector.push(job);
        {
            let mut state = self
                .shared
                .sleep
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.generation = state.generation.wrapping_add(1);
        }
        self.shared.wake.notify_all();
    }
}

impl Drop for JobSystem {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        {
            let mut state = self
                .shared
                .sleep
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.generation = state.generation.wrapping_add(1);
        }
        self.shared.wake.notify_all();
        for handle in self.threads.drain(..) {
            // A worker should never panic (job panics are captured), but shutting down
            // must not itself panic if one did.
            let _ = handle.join();
        }
    }
}

impl fmt::Debug for JobSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JobSystem")
            .field("workers", &self.threads.len())
            .finish_non_exhaustive()
    }
}

/// A handle to a running [`JobSystem::scope`].
///
/// Jobs spawned through a scope are guaranteed to finish before `scope` returns.
pub struct Scope<'scope> {
    system: &'scope JobSystem,
    shared: Arc<ScopeShared>,
    marker: PhantomData<&'scope mut &'scope ()>,
}

impl<'scope> Scope<'scope> {
    /// Spawns a job into this scope.
    ///
    /// The closure may borrow any data that outlives the scope (for example, locals in the
    /// function that called `scope`). It must be `Send` because a worker thread may run it.
    pub fn spawn<F>(&self, job: F)
    where
        F: FnOnce() + Send + 'scope,
    {
        let data = Box::into_raw(Box::new(job)).cast::<()>();
        // `Arc` keeps the scope counter and panic slot alive even if a future refactor
        // lets the scope frame return early. The scope still waits for every job before
        // returning, so the erased closure's borrows remain valid.
        self.shared.pending.fetch_add(1, Ordering::Relaxed);
        self.system.submit(JobRef {
            invoke: invoke_boxed::<F>,
            data,
            shared: Arc::clone(&self.shared),
        });
    }
}

impl fmt::Debug for Scope<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scope").finish_non_exhaustive()
    }
}
