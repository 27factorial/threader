mod task;
mod worker;

use crate::executor::Executor;
use crossbeam::{
    deque::{Injector, Stealer, Worker},
    queue::ArrayQueue,
};
use futures::Future;
use std::{
    io, mem,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};
use task::Task;

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

pub struct ThreadPool {
    workers: Vec<(JoinHandle<()>, Arc<worker::Handle>)>,
    count: usize,
    shutdown: bool,
    shared: Arc<Shared>,
}

impl ThreadPool {
    pub fn new() -> io::Result<ThreadPool> {
        new_priv(None)
    }

    pub fn with_threads(count: usize) -> io::Result<ThreadPool> {
        new_priv(Some(count))
    }

    pub fn shutdown_on_idle(&mut self) {
        self.shutdown_priv(worker::SHUTDOWN_IDLE);
    }

    pub fn shutdown_now(&mut self) {
        self.shutdown_priv(worker::SHUTDOWN_NOW);
    }

    fn startup(&mut self) -> io::Result<()> {
        if self.shutdown {
            mem::replace(self, new_priv(Some(self.count))?);
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Other,
                "ThreadPool is already running.",
            ))
        }
    }

    fn shutdown_priv(&mut self, shutdown: usize) {
        self.shutdown = true;

        while let Some((thread, handle)) = self.workers.pop() {
            handle.state.store(shutdown, Ordering::Release);
            handle.unparker.unpark();
            let _ = thread.join();
        }
    }
}

impl<F> Executor<F> for ThreadPool
where
    F: Future<Output = ()> + Send + 'static,
{
    fn spawn(&self, future: F) {
        let shared = Arc::downgrade(&self.shared);
        let task = Task::arc(future, shared);
        self.shared.injector.push(task);

        if let Ok(handle) = self.shared.sleep_queue.pop() {
            handle.state.store(worker::NEW_TASK, Ordering::Release);
            handle.unparker.unpark();
        }
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        self.shutdown_now();
    }
}

pub(crate) struct Shared {
    injector: Injector<Arc<Task>>,
    sleep_queue: ArrayQueue<Arc<worker::Handle>>,
    stealers: Vec<Stealer<Arc<Task>>>,
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
        let (tx, rx) = channel::unbounded();
        let executor = ThreadPool::new().unwrap();

        executor.spawn(async move {
            future::ready(()).await;
            tx.send(0).unwrap();
        });

        thread::sleep(Duration::from_secs(1));
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
        let mut executor = ThreadPool::new().unwrap();
        let mut results = Vec::with_capacity(TIMES);
        eprintln!("threader time test starting...");
        let total_start = Instant::now();
        for _ in 0..TIMES {
            let start = Instant::now();

            for _ in 0..50_000 {
                executor.spawn(async {
                    future::ready(()).await;
                });
            }

            let end = start.elapsed();
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
        eprintln!("threader average: {:?} ms", average);
    }

//    #[test]
//    #[ignore]
//    fn time_tokio() {
//        let executor = tokio::runtime::Runtime::new().unwrap();
//        let mut results = Vec::with_capacity(TIMES);
//        eprintln!("tokio time test starting...");
//        let total_start = Instant::now();
//        for _ in 0..TIMES {
//            let start = Instant::now();
//
//            for _ in 0..50_000 {
//                executor.spawn(async {
//                    future::ready(()).await;
//                });
//            }
//
//            let end = start.elapsed();
//            results.push(end.as_millis());
//        }
//        let shutdown_start = Instant::now();
//        executor.shutdown_on_idle();
//        eprintln!("tokio shutdown: {:?}", shutdown_start.elapsed());
//        eprintln!("tokio total: {:?}", total_start.elapsed());
//        let average = {
//            let sum: u128 = results.into_iter().sum();
//            (sum as f64) / (TIMES as f64)
//        };
//        eprintln!("tokio average: {:?} ms", average);
//    }
}
