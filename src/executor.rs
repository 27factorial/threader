use futures::Future;

pub trait Executor<F: Future> {
    fn spawn(&self, future: F);
}
