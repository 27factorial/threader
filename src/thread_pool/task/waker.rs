use super::{raw::Header, Task};
use crate::thread_pool::worker;
use futures::task::{RawWaker, RawWakerVTable, Waker};
use std::{mem, sync::atomic::Ordering};

static VTABLE: &'static RawWakerVTable = &RawWakerVTable::new(clone, wake, wake_by_ref, drop_task);

pub fn waker(task: &Task) -> Waker {
    mem::forget(task.raw.clone());
    let header = task.raw.ptr();
    let raw = RawWaker::new(header as *const (), VTABLE);
    unsafe { Waker::from_raw(raw) }
}

unsafe fn cast_header(ptr: *const ()) -> *const Header {
    ptr as *const Header
}

unsafe fn inc_refcount(ptr: *const Header) {
    ((*ptr).vtable.inc_refcount)(ptr);
}

unsafe fn clone(ptr: *const ()) -> RawWaker {
    let header = cast_header(ptr);
    inc_refcount(header);
    RawWaker::new(ptr, VTABLE)
}

unsafe fn wake(ptr: *const ()) {
    let header = cast_header(ptr);
    let task = Task::from_raw(header);

    if let Some(shared) = (*(task.raw.ptr())).shared.upgrade() {
        shared.injector.push(task);

        if !shared.sleep_queue.is_empty() {
            if let Ok(handle) = shared.sleep_queue.pop() {
                handle.state.store(worker::NEW_TASK, Ordering::Release);
                handle.unparker.unpark();
            }
        }
    }
}

unsafe fn wake_by_ref(ptr: *const ()) {
    // Since this waker is not being dropped, we
    // need to increment the refcount in order to
    // ensure the data lives long enough.
    inc_refcount(cast_header(ptr));
    wake(ptr);
}

unsafe fn drop_task(ptr: *const ()) {
    drop(Task::from_raw(cast_header(ptr)));
}
