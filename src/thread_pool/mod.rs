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
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
};
use task::Task;

fn new_priv(count: Option<usize>) -> io::Result<ThreadPool> {
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

    Ok(ThreadPool {
        workers,
        shutdown: false,
        shared,
    })
}

pub struct ThreadPool {
    workers: Vec<(JoinHandle<()>, Arc<worker::Handle>)>,
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

pub(crate) struct Shared {
    injector: Injector<Arc<Task>>,
    sleep_queue: ArrayQueue<Arc<worker::Handle>>,
    stealers: Vec<Stealer<Arc<Task>>>,
}
