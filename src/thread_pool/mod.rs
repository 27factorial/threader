mod task;
mod worker;

use crate::executor::Executor;
use crossbeam::{
    deque::{Injector, Stealer, Worker},
    queue::ArrayQueue,
};
use futures::Future;
use std::{
    io,
    sync::{
        atomic::{Ordering},
        Arc,
    },
    thread::JoinHandle,
};
use task::Task;

/// An executor which distributes tasks across multiple threads using a work-stealing
/// scheduler. Tasks can be spawned on it by calling the [`spawn`][`Executor::spawn`]
/// method on the `ThreadPool`. Note that since this executor moves futures between different
/// threads, the future in question *must* be [`Send`].
///
/// # Examples
/// ```
/// use std::io;
/// use threader::{
///     executor::Executor,
///     thread_pool::ThreadPool,
///     net::tcp::TcpStream,
/// };
///
/// fn main() -> io::Result<()> {
///     let mut pool = ThreadPool::new()?;
///     let addr = "10.0.0.1:80".parse().unwrap();
///
///     pool.spawn(async move {
///         let _stream = TcpStream::connect(&addr);
///     });
///
///     pool.shutdown_on_idle();
///     Ok(())
/// }
/// ```
pub struct ThreadPool {
    workers: Vec<(JoinHandle<()>, Arc<worker::Handle>)>,
    count: usize,
    shutdown: bool,
    shared: Arc<Shared>,
}

impl ThreadPool {
    /// Creates a new `ThreadPool` instance with a number of threads
    /// equal to the number of logical CPU cores in a given machine.
    /// Returns any errors that may have occurred in creating the
    /// thread pool.
    ///
    /// # Examples
    /// ```
    /// use std::io;
    /// use threader::thread_pool::ThreadPool;
    ///
    /// fn main() -> io::Result<()> {
    ///     let pool = ThreadPool::new()?;
    ///     Ok(())
    /// }
    /// ```
    pub fn new() -> io::Result<ThreadPool> {
        ThreadPool::new_priv(None)
    }

    /// Creates a new `ThreadPool` instance with a number of threads
    /// equal to `count`. `count` must not be zero, or this method
    /// will panic. Like [`ThreadPool::new`], this method returns
    /// any errors that occurred when creating the thread pool.
    ///
    /// # Panics
    /// Panics if `count` is equal to zero.
    ///
    /// # Examples
    /// ```
    /// use std::io;
    /// use threader::thread_pool::ThreadPool;
    ///
    /// fn main() -> io::Result<()> {
    ///     let pool = ThreadPool::with_threads(1)?;
    ///     Ok(())
    /// }
    /// ```
    pub fn with_threads(count: usize) -> io::Result<ThreadPool> {
        ThreadPool::new_priv(Some(count))
    }

    /// Shuts down the `ThreadPool` when all worker threads are idle. This method
    /// blocks the current thread until all of the worker threads have been joined.
    pub fn shutdown_on_idle(&mut self) {
        self.shutdown_priv(worker::SHUTDOWN_IDLE);
    }

    /// Shuts down the `ThreadPool` immediately, sending a message to all worker
    /// threads to shut down. This emethod blocks the current thread until all
    /// worker threads have been joined, but this blocking shouldn't be noticeable.
    pub fn shutdown_now(&mut self) {
        self.shutdown_priv(worker::SHUTDOWN_NOW);
    }

    // Private method used to reduce code duplication.
    fn shutdown_priv(&mut self, shutdown: usize) {
        self.shutdown = true;

        for (_, handle) in &self.workers {
            handle.state.store(shutdown, Ordering::Release);
            handle.unparker.unpark();
        }

        while let Some((thread, _)) = self.workers.pop() {
            let _ = thread.join();
        }
    }

    // Private method used to reduce code duplication.
    fn new_priv(count: Option<usize>) -> io::Result<ThreadPool> {
        if let Some(0) = count {
            panic!("Can not create a thread pool with 0 threads.");
        }

        let count = count.unwrap_or(num_cpus::get());

        let queues = {
            let mut vec = Vec::with_capacity(count);
            for _ in 0..count {
                vec.push(Worker::new_fifo());
            }
            vec
        };
        let stealers: Vec<_> = queues.iter().map(|queue| queue.stealer()).collect();
        let shared = Arc::new(Shared {
            injector: Injector::new(),
            sleep_queue: ArrayQueue::new(count),
            stealers,
        });

        let workers = {
            let mut vec = Vec::with_capacity(count);
            for queue in queues {
                let thread = worker::create_worker(Arc::clone(&shared), queue)?;
                vec.push(thread);
            }
            vec
        };

        for (_, handle) in &workers {
            let handle = Arc::clone(handle);
            // Unwrap here since this is a programmer error
            // if this fails.
            shared.sleep_queue.push(handle).unwrap();
        }

        Ok(ThreadPool {
            workers,
            shutdown: false,
            count,
            shared,
        })
    }
}

impl<F> Executor<F> for ThreadPool
where
    F: Future<Output = ()> + Send + 'static,
{
    fn spawn(&self, future: F) {
        let shared = Arc::downgrade(&self.shared);
        let task = Task::new(future, shared);
        self.shared.injector.push(task);

        if !self.shared.sleep_queue.is_empty() {
            if let Ok(handle) = self.shared.sleep_queue.pop() {
                handle.state.store(worker::NEW_TASK, Ordering::Release);
                handle.unparker.unpark();
            }
        }
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        self.shutdown_now();
    }
}

pub(crate) struct Shared {
    injector: Injector<Task>,
    sleep_queue: ArrayQueue<Arc<worker::Handle>>,
    stealers: Vec<Stealer<Task>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam::channel;
    use futures::task::{Context, Waker};
    use futures::{future, Poll};
    use parking_lot::Mutex;
    use std::pin::Pin;
    use std::sync::atomic::AtomicBool;
    use std::time::{Duration, Instant};
    use std::thread;

    static TIMES: usize = 100;

    #[test]
    fn simple() {
        let executor = ThreadPool::new().unwrap();

        executor.spawn(async {
            println!("Hello, world!");
        });

        thread::sleep(Duration::from_secs(1));
    }

    #[test]
    fn reschedule() {
        struct CustomFuture {
            waker: Arc<Mutex<Option<Waker>>>,
            shared: Arc<AtomicBool>,
        }

        impl CustomFuture {
            fn new() -> CustomFuture {
                let waker: Arc<Mutex<Option<Waker>>> = Arc::new(Mutex::new(None));
                let waker_thread = Arc::clone(&waker);
                let shared = Arc::new(AtomicBool::new(false));
                let shared_thread = Arc::clone(&shared);

                thread::spawn(move || {
                    thread::sleep(Duration::from_secs(1));

                    if let Some(waker) = waker_thread.lock().take() {
                        waker.wake();
                        shared_thread.store(true, Ordering::SeqCst);
                    }
                });

                CustomFuture {
                    waker,
                    shared,
                }
            }
        }

        impl Future for CustomFuture {
            type Output = ();

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                if self.shared.load(Ordering::SeqCst) {
                    Poll::Ready(())
                } else {
                    *(self.waker.lock()) = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }

        let (tx, rx) = channel::unbounded();
        let executor = ThreadPool::with_threads(12).unwrap();

        executor.spawn(async move {
            CustomFuture::new().await;
            tx.send(0).unwrap();
        });

        thread::sleep(Duration::from_secs(4));
        assert_eq!(rx.try_recv(), Ok(0));
    }

    #[test]
    #[should_panic]
    fn zero_threads() {
        let executor = ThreadPool::with_threads(0).unwrap();
        executor.spawn(async {});
    }

    #[test]
    fn custom_thread_count() {
        let executor = ThreadPool::with_threads(32).unwrap();
        executor.spawn(async {});
    }

    #[test]
    fn bad_future() {
        // A future that spawns a thread, returns Poll::Ready(()), and
        // keeps trying to reschedule itself on the thread_pool.
        struct BadFuture {
            shared: Arc<Mutex<Option<Waker>>>,
        }

        impl BadFuture {
            fn new() -> BadFuture {
                let shared: Arc<Mutex<Option<Waker>>> = Arc::new(Mutex::new(None));
                let thread_shared = Arc::clone(&shared);
                thread::spawn(move || loop {
                    let guard = thread_shared.lock();

                    if let Some(waker) = guard.as_ref() {
                        waker.clone().wake();
                    }
                });

                BadFuture { shared }
            }
        }

        impl Future for BadFuture {
            type Output = ();

            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                let mut guard = self.shared.lock();
                *guard = Some(cx.waker().clone());
                Poll::Ready(())
            }
        }

        let executor = ThreadPool::new().unwrap();

        for _ in 0..50 {
            executor.spawn(BadFuture::new());
        }
    }

    #[test]
    #[ignore]
    fn time_threader() {
        let mut executor = ThreadPool::with_threads(1).unwrap();
        let mut results = Vec::with_capacity(TIMES);
        eprintln!("\nthreader time test starting...");
        let total_start = Instant::now();
        for _ in 0..TIMES {
            let start = Instant::now();

            for _ in 0..50_000 {
                executor.spawn(async {
                    future::ready(()).await;
                });
            }

            let end = start.elapsed();
            // eprintln!("threader: {:?}", end);
            results.push(end.as_millis());
        }
        let shutdown_start = Instant::now();
        executor.shutdown_on_idle();
        eprintln!("threader shutdown: {:?}", shutdown_start.elapsed());
        eprintln!("threader total: {:?}", total_start.elapsed());
        let average = {
            let sum: u128 = results.into_iter().sum();
            (sum as f64) / (TIMES as f64)
        };
        eprintln!("threader average: {:?}ms", average);
    }
}
