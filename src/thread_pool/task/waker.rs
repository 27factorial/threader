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

    // The specific reason we need this double pointer
    // is a bit complicated, but essentially, if we just
    // used a single *const Inner<ExecutorFuture>, we couldn't
    // cast back directly from *const (). Okay, then why not
    // use &ptr as *const *const Inner<ExecutorFuture>? The answer
    // is that ptr is stored on the stack, and is not guaranteed
    // to stay valid, since other variable and such that get pushed
    // onto the stack may overwrite it, so we would be pointing to a
    // pointer which points to invalid memory. Therefore, we use the
    // ptr that is in the Inner object itself, so that the second
    // pointer has a stable location in memory, since Inner does
    // not move until it is dropped. It's a hack, but it works.
    let waker_ptr = unsafe { (&(*ptr).self_ptr) as *const *const Inner<ExecutorFuture> };

    let raw = RawWaker::new(waker_ptr as *const (), VTABLE);
    unsafe { Waker::from_raw(raw) }
}

unsafe fn cast_inner(ptr: *const ()) -> *const Inner<ExecutorFuture> {
    *(ptr as *const *const Inner<ExecutorFuture>)
}

unsafe fn inc_refcount(ptr: *const Inner<ExecutorFuture>) {
    let arc = Arc::from_raw(ptr);
    mem::forget(Arc::clone(&arc));
    mem::forget(arc);
}

unsafe fn clone(ptr: *const ()) -> RawWaker {
    let inner = cast_inner(ptr);
    inc_refcount(inner);
    RawWaker::new(ptr, VTABLE)
}

unsafe fn wake(ptr: *const ()) {
    let inner = Arc::from_raw(cast_inner(ptr));
    let task = Task::from_inner(inner);

    if let Some(shared) = task.inner.shared.upgrade() {
        shared.injector.push(task);
    }
}

unsafe fn wake_by_ref(ptr: *const ()) {
    // Since this waker is not being dropped, we
    // need to increment the refcount in order to
    // ensure the data in the Arc lives long enough.
    inc_refcount(cast_inner(ptr));
    wake(ptr);
}

unsafe fn drop(ptr: *const ()) {
    let inner = cast_inner(ptr);
    let arc = Arc::from_raw(inner);
    mem::drop(arc);
}
