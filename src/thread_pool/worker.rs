#![allow(unused)]

use super::{
    task::{self, Task},
    Shared,
};
use crate::utils::{
    debug_utils::debug_unreachable,
    thread_parker::{self, ThreadParker, ThreadUnparker},
};
use crossbeam::{
    deque::{Injector, Steal, Stealer, Worker as WorkerQueue},
    queue::ArrayQueue,
    utils::Backoff,
};
use futures::task::{waker_ref, Context, Poll};
use std::{
    io,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Instant,
};

// The worker is idle and ready to be woken up.
pub(super) const IDLE: usize = 0;

// The worker is currently running a task.
pub(super) const RUNNING: usize = 1;

// The worker has a new task waiting on the injector.
pub(super) const NEW_TASK: usize = 2;

// The worker should be shut down whenever it is next idle.
pub(super) const SHUTDOWN_IDLE: usize = 3;

// The worker should be shut down immediately.
// This is similar to SHUTDOWN_IDLE, except that
// it will be checked each time the thread tries
// to poll a task, so the effect is more immediate.
pub(super) const SHUTDOWN_NOW: usize = 4;

pub(super) fn create_worker(
    shared: Arc<Shared>,
    task_queue: WorkerQueue<Task>,
) -> io::Result<(JoinHandle<()>, Arc<Handle>)> {
    let (parker, unparker) = thread_parker::new2();
    let state = AtomicUsize::new(RUNNING);
    let handle = Arc::new(Handle { unparker, state });
    let thread_handle = Arc::clone(&handle);

    let join_handle = thread::Builder::new().spawn(move || loop {
        if thread_handle
            .state
            .compare_and_swap(RUNNING, IDLE, Ordering::AcqRel)
            == RUNNING
        {
            parker.park();
        }

        match thread_handle.state.swap(RUNNING, Ordering::AcqRel) {
            NEW_TASK => {
                run_tasks(&task_queue, &shared, &thread_handle.state);
                // The sleep queue is exactly large enough to store each thread handle once,
                // so if this fails, it means we were already on the sleep queue.
                shared.sleep_queue.push(Arc::clone(&thread_handle));
            }
            SHUTDOWN_IDLE | SHUTDOWN_NOW => return,
            IDLE => (),
            _ => unsafe { debug_unreachable() },
        }
    })?;

    Ok((join_handle, handle))
}

fn is_shutdown(state: &AtomicUsize) -> bool {
    let state = state.load(Ordering::Acquire);
    state == SHUTDOWN_IDLE || state == SHUTDOWN_NOW
}

fn run_tasks(task_queue: &WorkerQueue<Task>, shared: &Shared, state: &AtomicUsize) {
    loop {
        while let Some(task) = get_task(task_queue, shared) {
            if state.load(Ordering::Acquire) == SHUTDOWN_NOW {
                return;
            }

            let guard = task.guard();
            let waker = task::waker(&task);
            let mut cx = Context::from_waker(&waker);

            task.poll(&mut cx, &guard).expect("Invalid guard supplied.");
        }

        let backoff = Backoff::new();
        loop {
            if !shared.injector.is_empty() {
                break;
            } else if is_shutdown(state) || backoff.is_completed() {
                return;
            } else {
                backoff.snooze();
            }
        }
    }
}

fn get_task(task_queue: &WorkerQueue<Task>, shared: &Shared) -> Option<Task> {
    task_queue.pop().or_else(|| {
        // Then we check the global injector queue if it's not empty.
        if !shared.injector.is_empty() {
            let mut result = shared.injector.steal_batch(task_queue);
            loop {
                if result.is_retry() {
                    result = shared.injector.steal_batch(task_queue);
                } else {
                    break;
                }
            }

            // The injector may have been emptied between when we checked it and tried to steal
            // from it. This accounts for that possibilty.
            if result.is_empty() {
                steal(task_queue, &shared.stealers);
            }
        } else {
            // If it was empty, then we check the stealers instead.
            steal(task_queue, &shared.stealers);
        }

        task_queue.pop()
    })
}

fn steal(worker: &WorkerQueue<Task>, stealers: &[Stealer<Task>]) {
    for stealer in stealers {
        loop {
            match stealer.steal_batch(worker) {
                Steal::Success(_) => return,
                Steal::Empty => break,
                Steal::Retry => (),
            }
        }
    }
}

pub(super) struct Handle {
    pub(super) unparker: ThreadUnparker,
    pub(super) state: AtomicUsize,
}
