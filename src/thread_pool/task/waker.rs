use super::{ExecutorFuture, Inner, Task};
use futures::task::{RawWaker, RawWakerVTable, Waker};
use std::{
    mem,
    ptr::{self, NonNull},
    sync::{atomic::Ordering, Arc},
};

static VTABLE: &'static RawWakerVTable = &RawWakerVTable::new(clone, wake, wake_by_ref, drop);

pub fn waker(task: &Task) -> Waker {
    let ptr = Arc::into_raw(Arc::clone(&task.inner));
    let raw = RawWaker::new(ptr as *const (), VTABLE);

    unsafe { Waker::from_raw(raw) }
}

unsafe fn inc_refcount(ptr: *const Inner<ExecutorFuture>) {
    let arc = Arc::from_raw(ptr);
    mem::forget(Arc::clone(&arc));
    mem::forget(arc);
}

unsafe fn clone(ptr: *const ()) -> RawWaker {
    let cloned = (ptr as *const Task).as_ref().unwrap().clone();
    let raw = RawWaker::new(&cloned as *const Task as *const (), VTABLE);
    mem::forget(cloned);
    raw
}

unsafe fn wake(ptr: *const ()) {
    let task = (ptr as *const Task).read_volatile();

    if let Some(shared) = task.inner.shared.upgrade() {
        shared.injector.push(task);
    }
}

unsafe fn wake_by_ref(ptr: *const ()) {
    wake(ptr);
}

unsafe fn drop(ptr: *const ()) {
    ptr::drop_in_place(ptr as *const Task as *mut Task);
}
