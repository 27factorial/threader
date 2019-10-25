use futures::future;
use std::time::Instant;
use threader::executor::Executor;
use threader::thread_pool::ThreadPool;

const TIMES: usize = 50;

pub fn main() {
    let mut executor = ThreadPool::with_threads(12).unwrap();
    let mut results = Vec::with_capacity(TIMES);
    eprintln!("threader time test starting...");
    let total_start = Instant::now();
    for _ in 0..TIMES {
        let start = Instant::now();

        for _ in 0..50_000 {
            executor.spawn(async {});
        }

        let end = start.elapsed();
        results.push(end.as_millis());
    }
    let shutdown_start = Instant::now();
    executor.shutdown_on_idle();
    eprintln!("threader shutdown: {:?}", shutdown_start.elapsed());
    eprintln!("threader total: {:?}", total_start.elapsed());
    let average = {
        let sum: u128 = results.into_iter().sum();
        (sum as f64) / (TIMES as f64)
    };
    eprintln!("threader average: {:?} ms", average);
}
