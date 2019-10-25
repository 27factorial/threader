use std::ops::{Deref, DerefMut};

/// A type which wraps some value of type
/// `T`. useful for getting a thin pointer
/// to fat pointer objects.
#[derive(Clone, Debug)]
pub struct Wrapper<T>(pub T);

impl<T> Deref for Wrapper<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for Wrapper<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}
