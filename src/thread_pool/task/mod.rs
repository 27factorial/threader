mod raw;
mod waker;

use super::Shared;
use futures::{self, future::Future, task::Waker};
use raw::{Header, InvalidGuard, RawTask};
use std::{sync::Weak, task::Context};

pub fn waker(task: &Task) -> Waker {
    waker::waker(task)
}

pub(crate) trait ExecutorFuture: Future<Output = ()> + Send + 'static {}

impl<F: Future<Output = ()> + Send + 'static> ExecutorFuture for F {}

/// A task that can be run on an executor.
#[derive(Debug)]
pub struct Task {
    raw: RawTask,
}

impl Task {
    /// Creates a new `Task` containing the provided future.
    pub(super) fn new<F>(future: F, shared: Weak<Shared>) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let raw = RawTask::new(future, shared);

        Self { raw }
    }

    pub(super) fn poll(&self, cx: &mut Context) {
        self.raw.poll(cx)
    }

    unsafe fn from_raw(header: *const Header) -> Self {
        let raw = RawTask::from_header(header);
        Self { raw }
    }
}
