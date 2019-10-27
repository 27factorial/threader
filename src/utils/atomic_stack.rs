use std::{
    cell::UnsafeCell,
    sync::atomic::{AtomicUsize, Ordering},
};

pub struct AtomicStack<T> {
    index: AtomicUsize,
    stack: Vec<Option<T>>,
}

impl<T> AtomicStack<T> {}
