use parking_lot::{MappedRwLockReadGuard, Mutex, RwLock, RwLockReadGuard};

/// A Reader/Writer stack. Similar to `RwLock<Vec<T>>`, except that it doesn't
/// lock the entire Vec when being read from or written to. Instead, it only
/// locks the individual element that is being read or written, which can improve
/// performance under contention.
#[derive(Debug)]
pub struct RwStack<T> {
    inner: Vec<RwLock<Option<T>>>,
    idx: Mutex<usize>,
}

impl<T> RwStack<T> {
    /// Creates a new RwStack.
    pub fn new(cap: usize) -> RwStack<T> {
        let inner = {
            let mut vec = Vec::with_capacity(cap);
            for _ in 0..cap {
                vec.push(RwLock::new(None));
            }
            vec
        };

        RwStack {
            inner,
            idx: Mutex::new(0),
        }
    }

    /// Returns the length of the stack. This temporarily
    /// locks the idx of the stack.
    pub fn len(&self) -> usize {
        *self.idx.lock()
    }

    /// Attmepts to push an element onto the stack, returning
    /// it if the stack is currently full.
    pub fn push(&self, element: T) -> Option<T> {
        let mut index = self.idx.lock();
        if *index == self.inner.len() {
            return Some(element);
        }

        let mut guard = self.inner[*index].write();
        debug_assert!(guard.as_ref().is_none());
        *index += 1;
        guard.replace(element)
    }

    /// Attempts to pop an element from the stack, returning
    /// `None` if the stack is currently empty.
    pub fn pop(&self) -> Option<T> {
        let mut index = self.idx.lock();
        if *index == 0 {
            return None;
        }

        *index -= 1;
        let mut guard = self.inner[*index].write();
        debug_assert!(guard.as_ref().is_some());
        guard.take()
    }

    /// Creates an iterator over the elements of the stack.
    /// Note that this does not iterate over references of
    /// the items in the stack, but `MappredRwLockReadGuard`s.
    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            inner: &self.inner,
            complete: false,
            idx: 0,
        }
    }
}

#[derive(Debug)]
pub struct Iter<'a, T> {
    inner: &'a Vec<RwLock<Option<T>>>,
    complete: bool,
    idx: usize,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = MappedRwLockReadGuard<'a, T>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.inner.get(self.idx) {
            Some(rwlock) => {
                if self.complete {
                    return None;
                }

                self.idx += 1;
                let ret = RwLockReadGuard::try_map(rwlock.read(), |opt| opt.as_ref()).ok();

                if let None = ret {
                    self.complete = true;
                }
                ret
            }
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn fill() {
        let rwstack = RwStack::new(1);

        rwstack.push(0);

        assert_eq!(rwstack.push(1), Some(1));
    }

    #[test]
    fn fill_then_pop_all() {
        let rwstack = RwStack::new(10);

        for x in 0..10 {
            assert!(
                rwstack.push(x).is_none(),
                "Pushing, should all return is_none()"
            );
        }

        for _ in 0..10 {
            assert!(
                rwstack.pop().is_some(),
                "Popping, should all return is_some()"
            );
        }

        assert_eq!(rwstack.pop(), None, "At index 0 popping returns None");
    }

    #[test]
    fn iterator() {
        let rwstack = RwStack::new(10);

        for _ in 0..10 {
            rwstack.push(0);
        }

        assert_eq!(rwstack.iter().count(), 10);
    }

    #[test]
    fn partially_full_iterator() {
        let rwstack = RwStack::new(10);

        for _ in 0..5 {
            rwstack.push(0);
        }

        assert_eq!(rwstack.iter().count(), 5);
    }

    #[test]
    fn timing() {
        const ELEMENTS: usize = 2048;
        eprintln!("Running timing test with {} elements.", ELEMENTS);

        eprintln!("Creating RwStack of {} elements...", ELEMENTS);
        let create_start = Instant::now();
        let rwstack = RwStack::new(ELEMENTS);
        let create_end = create_start.elapsed();

        eprintln!("Pushing {} elements to the stack...", ELEMENTS);
        let push_start = Instant::now();
        for x in 0..ELEMENTS {
            rwstack.push(x);
        }
        let push_end = push_start.elapsed();

        eprintln!("Popping {} elements from the stack...", ELEMENTS);
        let pop_start = Instant::now();
        for _ in 0..ELEMENTS {
            rwstack.pop();
        }
        let pop_end = pop_start.elapsed();

        eprintln!("Exhausting an iterator of {} elements", ELEMENTS);
        // refill the stack, this isn't counted for the final results.
        for x in 0..ELEMENTS {
            rwstack.push(x);
        }

        let iter_start = Instant::now();
        rwstack.iter().count();
        let iter_end = iter_start.elapsed();

        eprintln!("\nResults:");
        eprintln!(
            "Creation: {:?}\nPush: {:?}\nPop: {:?}\nIter: {:?}",
            create_end, push_end, pop_end, iter_end,
        )
    }
}
