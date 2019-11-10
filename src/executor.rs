use futures::Future;

/// A trait which includes a spawning method common to all
/// future executors.
pub trait Executor<F: Future> {
    /// Spawns a future on this executor. In general, this
    /// method should not fail or panic, but this is left up
    /// to whatever is implementing the trait.
    fn spawn(&self, future: F);
}
