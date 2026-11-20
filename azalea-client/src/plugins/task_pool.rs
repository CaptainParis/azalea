//! Borrowed from `bevy_core`.

use std::marker::PhantomData;

use bevy_app::{App, Last, Plugin};
use bevy_ecs::prelude::*;
use bevy_tasks::{
    AsyncComputeTaskPool, ComputeTaskPool, IoTaskPool, TaskPoolBuilder,
    tick_global_task_pools_on_main_thread,
};

/// Setup of default task pools: `AsyncComputeTaskPool`, `ComputeTaskPool`,
/// `IoTaskPool`.
#[derive(Default)]
pub struct TaskPoolPlugin {
    /// Options for the [`TaskPool`](bevy_tasks::TaskPool) created at
    /// application start.
    pub task_pool_options: TaskPoolOptions,
}

impl Plugin for TaskPoolPlugin {
    fn build(&self, app: &mut App) {
        // Setup the default bevy task pools
        self.task_pool_options.create_default_pools();

        #[cfg(not(target_arch = "wasm32"))]
        app.add_systems(Last, tick_global_task_pools);
    }
}

pub struct NonSendMarker(PhantomData<*mut ()>);
#[cfg(not(target_arch = "wasm32"))]
fn tick_global_task_pools(_main_thread_marker: Option<NonSend<NonSendMarker>>) {
    tick_global_task_pools_on_main_thread();
}

/// Helper for configuring and creating the default task pools.
///
/// For end-users who want full control, set up [`TaskPoolPlugin`]
#[derive(Clone, Resource)]
pub struct TaskPoolOptions {
    /// If the number of physical cores is less than min_total_threads, force
    /// using min_total_threads
    pub min_total_threads: usize,
    /// If the number of physical cores is greater than max_total_threads, force
    /// using max_total_threads
    pub max_total_threads: usize,

    /// Used to determine number of IO threads to allocate
    pub io: TaskPoolThreadAssignmentPolicy,
    /// Used to determine number of async compute threads to allocate
    pub async_compute: TaskPoolThreadAssignmentPolicy,
    /// Used to determine number of compute threads to allocate
    pub compute: TaskPoolThreadAssignmentPolicy,
}

impl Default for TaskPoolOptions {
    fn default() -> Self {
        TaskPoolOptions {
            // By default, use however many cores are available on the system
            min_total_threads: 1,
            max_total_threads: usize::MAX,

            // Use 25% of cores for IO, at least 1, no more than 4
            io: TaskPoolThreadAssignmentPolicy {
                min_threads: 1,
                max_threads: 4,
                percent: 0.25,
            },

            // Use 25% of cores for async compute, at least 1, no more than 4
            async_compute: TaskPoolThreadAssignmentPolicy {
                min_threads: 1,
                max_threads: 4,
                percent: 0.25,
            },

            // Use all remaining cores for compute (at least 1)
            compute: TaskPoolThreadAssignmentPolicy {
                min_threads: 1,
                max_threads: usize::MAX,
                percent: 1.0, // This 1.0 here means "whatever is left over"
            },
        }
    }
}

impl TaskPoolOptions {
    /// Inserts the default thread pools into the given resource map based on
    /// the configured values
    pub fn create_default_pools(&self) {
        let total_threads = bevy_tasks::available_parallelism()
            .clamp(self.min_total_threads, self.max_total_threads);

        let mut remaining_threads = total_threads;

        {
            // Determine the number of IO threads we will use
            let io_threads = self
                .io
                .get_number_of_threads(remaining_threads, total_threads);

            remaining_threads = remaining_threads.saturating_sub(io_threads);

            IoTaskPool::get_or_init(|| {
                TaskPoolBuilder::default()
                    .num_threads(io_threads)
                    .thread_name("IO Task Pool".to_owned())
                    .build()
            });
        }

        {
            // Determine the number of async compute threads we will use
            let async_compute_threads = self
                .async_compute
                .get_number_of_threads(remaining_threads, total_threads);

            remaining_threads = remaining_threads.saturating_sub(async_compute_threads);

            AsyncComputeTaskPool::get_or_init(|| {
                TaskPoolBuilder::default()
                    .num_threads(async_compute_threads)
                    .thread_name("Async Compute Task Pool".to_owned())
                    .build()
            });
        }

        {
            // Determine the number of compute threads we will use
            // This is intentionally last so that an end user can specify 1.0 as
            // the percent
            let compute_threads = self
                .compute
                .get_number_of_threads(remaining_threads, total_threads);

            ComputeTaskPool::get_or_init(|| {
                TaskPoolBuilder::default()
                    .num_threads(compute_threads)
                    .thread_name("Compute Task Pool".to_owned())
                    .build()
            });
        }
    }
}

/// Defines a simple way to determine how many threads to use given the number
/// of remaining cores and number of total cores
#[derive(Clone)]
pub struct TaskPoolThreadAssignmentPolicy {
    /// Force using at least this many threads
    pub min_threads: usize,
    /// Under no circumstance use more than this many threads for this pool
    pub max_threads: usize,
    /// Target using this percentage of total cores, clamped by min_threads and
    /// max_threads.
    ///
    /// It is permitted to use 1.0 to try to use all remaining
    /// threads
    pub percent: f32,
}

impl TaskPoolThreadAssignmentPolicy {
    /// Determine the number of threads to use for this task pool
    fn get_number_of_threads(&self, remaining_threads: usize, total_threads: usize) -> usize {
        assert!(self.percent >= 0.0);
        let mut desired = (total_threads as f32 * self.percent).round() as usize;

        // Limit ourselves to the number of cores available
        desired = desired.min(remaining_threads);

        // Clamp by min_threads, max_threads. (This may result in us using more
        // threads than are available, this is intended. An example case
        // where this might happen is a device with <= 2 threads.
        desired.clamp(self.min_threads, self.max_threads)
    }
}

/// Which bot the work on the current thread belongs to, and spawning tasks on
/// the shared pools so that a panic in one is charged to the bot that
/// spawned it.
///
/// Bevy's IO and async compute pools are shared by every app in the process.
/// A task Azalea spawns there is wrapped in an [`OwnedTask`] that carries the
/// owner (a bot id set on the bot's thread with [`set_thread_owner`]): while
/// the task is polled the pool thread runs as that owner (so a panic hook can
/// tell whose panic it is), and if the task panics the reporter set with
/// [`set_task_panic_reporter`] is told which owner it was before the panic
/// goes on to whoever awaits the task.
pub mod owner {
    use std::{
        any::Any,
        cell::Cell,
        future::Future,
        panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
        pin::Pin,
        sync::OnceLock,
        task::{Context, Poll},
    };

    use bevy_tasks::{AsyncComputeTaskPool, IoTaskPool, Task};

    use std::time::Duration;

    /// Time cap of the TCP connect and handshake task (WP3's `Connecting`).
    pub const CONNECT_CAP: Duration = Duration::from_secs(30);
    /// Time cap of the session server join task (WP3's `Authenticating`).
    pub const AUTH_CAP: Duration = Duration::from_secs(60);
    /// Time cap of the chat signing certificate fetch.
    pub const CERTS_CAP: Duration = Duration::from_secs(60);
    /// Time cap of one write of the packet writer task; past it the server
    /// isn't reading (vanilla kicks a silent client after 30 s) and the
    /// connection ends.
    pub const WRITE_CAP: Duration = Duration::from_secs(60);

    thread_local! {
        static OWNER: Cell<Option<u64>> = const { Cell::new(None) };
    }

    type Reporter = Box<dyn Fn(Option<u64>, &(dyn Any + Send)) + Send + Sync>;
    static REPORTER: OnceLock<Reporter> = OnceLock::new();

    /// Set the owner of the work on this thread, returning the previous one.
    pub fn set_thread_owner(owner: Option<u64>) -> Option<u64> {
        OWNER.with(|o| o.replace(owner))
    }

    /// The owner of the work on this thread, if any.
    pub fn thread_owner() -> Option<u64> {
        OWNER.with(Cell::get)
    }

    /// Set the function told about every panic in an [`OwnedTask`] (with the
    /// task's owner, `None` if it was spawned on a thread without one). Only
    /// the first call has an effect; returns whether this one did.
    pub fn set_task_panic_reporter(
        reporter: impl Fn(Option<u64>, &(dyn Any + Send)) + Send + Sync + 'static,
    ) -> bool {
        REPORTER.set(Box::new(reporter)).is_ok()
    }

    /// A future that runs as its owner while it's polled and reports a panic
    /// to the task panic reporter, then lets the panic continue.
    pub struct OwnedTask<F> {
        owner: Option<u64>,
        inner: Pin<Box<F>>,
    }

    impl<F: Future> OwnedTask<F> {
        /// Wrap `inner`, owned by the owner of the current thread.
        pub fn new(inner: F) -> Self {
            Self {
                owner: thread_owner(),
                inner: Box::pin(inner),
            }
        }

        pub fn owner(&self) -> Option<u64> {
            self.owner
        }
    }

    impl<F: Future> Future for OwnedTask<F> {
        type Output = F::Output;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            let owner = self.owner;
            let previous = set_thread_owner(owner);
            let res = catch_unwind(AssertUnwindSafe(|| self.inner.as_mut().poll(cx)));
            set_thread_owner(previous);
            match res {
                Ok(poll) => poll,
                Err(payload) => {
                    if let Some(report) = REPORTER.get() {
                        report(owner, &*payload);
                    }
                    resume_unwind(payload)
                }
            }
        }
    }

    /// Spawn on the IO pool, owned by the owner of the current thread.
    pub fn spawn_io<T: Send + 'static>(
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T> {
        IoTaskPool::get().spawn(OwnedTask::new(future))
    }

    /// Spawn on the async compute pool, owned by the owner of the current
    /// thread.
    pub fn spawn_async_compute<T: Send + 'static>(
        future: impl Future<Output = T> + Send + 'static,
    ) -> Task<T> {
        AsyncComputeTaskPool::get().spawn(OwnedTask::new(future))
    }

    #[cfg(test)]
    mod tests {
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        };

        use bevy_tasks::{TaskPoolBuilder, futures_lite::future};

        use super::*;

        #[test]
        fn owner_is_set_while_polled_and_a_panic_is_reported_with_it() {
            IoTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(1).build());
            static REPORTED: Mutex<Vec<Option<u64>>> = Mutex::new(Vec::new());
            set_task_panic_reporter(|owner, _| REPORTED.lock().unwrap().push(owner));

            let seen = Arc::new(Mutex::new(None));
            set_thread_owner(Some(7));
            let s = seen.clone();
            let ok = spawn_io(async move {
                *s.lock().unwrap() = Some(thread_owner());
                5
            });
            assert_eq!(future::block_on(ok), 5);
            assert_eq!(*seen.lock().unwrap(), Some(Some(7)));

            let polled = Arc::new(AtomicBool::new(false));
            let p = polled.clone();
            let bad = spawn_io(async move {
                p.store(true, Ordering::SeqCst);
                panic!("injected task panic");
            });
            set_thread_owner(None);
            // the panic reaches whoever awaits the task, after the report
            let joined = std::thread::spawn(move || future::block_on(bad)).join();
            assert!(joined.is_err());
            assert!(polled.load(Ordering::SeqCst));
            assert_eq!(*REPORTED.lock().unwrap(), vec![Some(7)]);
        }
    }
}
