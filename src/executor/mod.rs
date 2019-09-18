mod task;

use {
    crossbeam::{deque::*, utils::Backoff},
    futures::{
        task::{waker_ref, Context, Poll},
        Future,
    },
    num_cpus,
    once_cell::sync::Lazy,
    std::{
        cell::Cell,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread::{self, JoinHandle},
    },
    task::Task,
};

/// The injector used as the point of entry for new tasks. This will
/// be lazily initialized on its first access.
static INJECTOR: Lazy<Injector<Arc<Task>>> = Lazy::new(|| Injector::new());

thread_local! {
    static RESCHEDULE: Cell<bool> = Cell::new(false);
}

/// Signals to the current executor to immediately push this task back onto
/// the injector queue. Useful for splitting up expensive computations in a
/// future.
pub fn reschedule() {
    RESCHEDULE.with(|c| c.set(true));
}

fn create_thread(
    handle: Arc<ExecutorHandle>,
    worker: Worker<Arc<Task>>,
    injector: &'static Injector<Arc<Task>>,
) -> JoinHandle<()> {
    // helper for this function, not used anywhere else.
    fn steal(stealers: &Vec<Stealer<Arc<Task>>>, worker: &Worker<Arc<Task>>) {
        for stealer in stealers.iter() {
            loop {
                match stealer.steal_batch(worker) {
                    Steal::Success(_) => return,
                    Steal::Empty => break,
                    Steal::Retry => (),
                }
            }
        }
    }

    thread::spawn(move || loop {
        // First, the worker queue is checked.
        let mut task = worker.pop().or_else(|| {
            // Then we check the global injector queue if it's not empty.
            if !injector.is_empty() {
                let mut result = injector.steal_batch(&worker);
                loop {
                    if result.is_retry() {
                        result = injector.steal_batch(&worker);
                    } else {
                        break;
                    }
                }

                // The injector may have been emptied between when we checked it and tried to steal
                // from it. This accounts for that possibilty.
                if result.is_empty() {
                    steal(&handle.stealers, &worker);
                }
            } else {
                // If it was empty, then we check the stealers instead.
                steal(&handle.stealers, &worker);
            }

            worker.pop()
        });

        if handle.shutdown.load(Ordering::Acquire) {
            return;
        }

        match task.take() {
            Some(task) => {
                let waker = waker_ref(&task);
                let mut cx = Context::from_waker(&*waker);

                if let Poll::Ready(()) = task.future().as_mut().poll(&mut cx) {
                    // prevents the task from being rescheduled, in case of
                    // bad future implementations.
                    task.complete();
                }

                let reschedule = RESCHEDULE.with(|c| c.get());

                if !task.is_complete() && reschedule {
                    injector.push(task);
                    RESCHEDULE.with(|c| c.set(false));
                }
            }
            None => {
                let backoff = Backoff::new();
                loop {
                    if handle.shutdown.load(Ordering::Acquire) {
                        // we've been ordered to shut down.
                        return;
                    } else if !injector.is_empty() {
                        // a new task is available.
                        break;
                    } else {
                        // nothing new has happened, so wait.
                        backoff.snooze();
                    }
                }
            }
        }
    })
}

/// The executor. This is the part of the thread pool that actually
/// executes futures. It holds many threads which will call `Future::poll()`
/// on the spawned futures. The injector is a global queue that is accessible
/// to all tasks and threads, and is used for spawning new tasks.
#[derive(Debug)]
pub struct Executor {
    threads: Vec<JoinHandle<()>>,
    handle: Arc<ExecutorHandle>,
    injector: &'static Injector<Arc<Task>>,
}

impl Executor {
    /// Creates a new instance of an Executor.
    pub fn new(count: Option<usize>) -> Executor {
        if let Some(0) = count {
            panic!("An executor can not be created with 0 threads.");
        }

        // default to this PC's number of cores.
        let count = count.unwrap_or(num_cpus::get());

        let workers = {
            let mut vec = Vec::with_capacity(count);
            for _ in 0..count {
                vec.push(Worker::new_fifo());
            }
            vec
        };

        let stealers = workers.iter().map(|worker| worker.stealer());

        let handle = Arc::new(ExecutorHandle {
            stealers: stealers.collect(),
            shutdown: AtomicBool::new(false),
        });

        let threads = {
            let mut vec = Vec::with_capacity(count);
            for worker in workers {
                let thread = create_thread(Arc::clone(&handle), worker, &INJECTOR);
                vec.push(thread);
            }
            vec
        };

        Executor {
            threads,
            handle,
            injector: &INJECTOR,
        }
    }

    /// Spawns a future on this executor.
    pub fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let task = Task::arc(future, &self.injector);
        self.injector.push(task);
    }
}

impl Drop for Executor {
    fn drop(&mut self) {
        // Notify threads that may be in the middle of searching for tasks
        // or executing a future that they should shut down.
        self.handle.shutdown.store(true, Ordering::Release);

        while !self.injector.is_empty() {
            let _ = self.injector.steal();
        }

        while let Some(thread) = self.threads.pop() {
            thread.join().ok();
        }
    }
}

/// A handle to the current executor. Used for threads to access other
/// threads' stealers, and for signalling shutdown.
#[derive(Debug)]
pub(crate) struct ExecutorHandle {
    stealers: Vec<Stealer<Arc<Task>>>,
    shutdown: AtomicBool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam::channel;
    use futures::future;
    use std::time::Instant;

    #[test]
    fn simple() {
        let executor = Executor::new(None);

        executor.spawn(async {
            println!("Hello, world!");
        });
    }

    #[test]
    fn reschedule() {
        let (tx, rx) = channel::unbounded();
        let executor = Executor::new(None);

        executor.spawn(async move {
            future::ready(()).await;
            tx.send(0).unwrap();
        });

        assert_eq!(rx.recv(), Ok(0));
    }

    #[test]
    #[should_panic]
    fn zero_threads() {
        let executor = Executor::new(0.into());
        executor.spawn(async {});
    }

    #[test]
    fn custom_thread_count() {
        let executor = Executor::new(32.into());
        executor.spawn(async {});
    }

    #[test]
    fn time() {
        let executor = Executor::new(16384.into());
        eprintln!("threader time test starting...");
        let start = Instant::now();

        for _ in 0..50_000 {
            executor.spawn(async {
                future::ready(()).await;
            });
        }

        let end = start.elapsed();
        eprintln!("threader: {:?}", end);
    }
}
