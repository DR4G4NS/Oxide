//! Small deterministic parallel primitives for read-only network hot paths.
//!
//! Rayon indexed iterators preserve input order when collected into a `Vec`.
//! Callers must still snapshot lock-backed state before invoking these helpers:
//! worker closures should receive owned values or immutable standard-library
//! collections, never `DashMap` guards.

use rayon::prelude::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// Below this many independent entries, allocation/scheduling usually costs
/// more than their snapshot codecs. The threshold is deliberately applied to
/// the parallel-safe subset, not to the total number of world tiles.
pub(crate) const SNAPSHOT_PARALLEL_THRESHOLD: usize = 64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ParallelExecution {
    pub(crate) items: usize,
    pub(crate) workers_used: usize,
    pub(crate) parallel: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ParallelMetrics {
    pub(crate) calls: u64,
    pub(crate) parallel_calls: u64,
    pub(crate) items: u64,
    pub(crate) parallel_items: u64,
    pub(crate) max_workers_used: u64,
}

#[derive(Debug)]
pub(crate) struct OrderedMap<T> {
    pub(crate) values: Vec<T>,
    pub(crate) execution: ParallelExecution,
}

static CALLS: AtomicU64 = AtomicU64::new(0);
static PARALLEL_CALLS: AtomicU64 = AtomicU64::new(0);
static ITEMS: AtomicU64 = AtomicU64::new(0);
static PARALLEL_ITEMS: AtomicU64 = AtomicU64::new(0);
static MAX_WORKERS_USED: AtomicU64 = AtomicU64::new(0);

/// Ordered read-only map with a sequential fast path.
///
/// `par_iter().collect::<Vec<_>>()` is an indexed Rayon collection, so output
/// index `n` always corresponds to input index `n`, regardless of scheduling.
/// `workers_used` is exact for pools up to 64 threads (larger pools are capped
/// to a 64-bit observation mask to keep per-item instrumentation lock-free).
pub(crate) fn map_ordered<T, R, F>(items: &[T], threshold: usize, map: F) -> OrderedMap<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync + Send,
{
    let parallel = items.len() >= threshold.max(2) && rayon::current_num_threads() > 1;
    let (values, workers_used) = if parallel {
        let worker_mask = AtomicU64::new(0);
        let values = items
            .par_iter()
            .map(|item| {
                // A Rayon worker has an index in its current pool. Saturating
                // at bit 63 only affects the diagnostic count for >64 threads.
                let worker = rayon::current_thread_index().unwrap_or(0).min(63);
                worker_mask.fetch_or(1u64 << worker, Ordering::Relaxed);
                map(item)
            })
            .collect();
        (
            values,
            worker_mask.load(Ordering::Relaxed).count_ones() as usize,
        )
    } else {
        (
            items.iter().map(map).collect(),
            usize::from(!items.is_empty()),
        )
    };

    let execution = ParallelExecution {
        items: items.len(),
        workers_used,
        parallel,
    };
    CALLS.fetch_add(1, Ordering::Relaxed);
    ITEMS.fetch_add(items.len() as u64, Ordering::Relaxed);
    if parallel {
        PARALLEL_CALLS.fetch_add(1, Ordering::Relaxed);
        PARALLEL_ITEMS.fetch_add(items.len() as u64, Ordering::Relaxed);
    }
    MAX_WORKERS_USED.fetch_max(workers_used as u64, Ordering::Relaxed);
    OrderedMap { values, execution }
}

pub(crate) fn metrics() -> ParallelMetrics {
    ParallelMetrics {
        calls: CALLS.load(Ordering::Relaxed),
        parallel_calls: PARALLEL_CALLS.load(Ordering::Relaxed),
        items: ITEMS.load(Ordering::Relaxed),
        parallel_items: PARALLEL_ITEMS.load(Ordering::Relaxed),
        max_workers_used: MAX_WORKERS_USED.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_map_uses_multiple_pool_threads_and_preserves_order() {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        let input: Vec<u32> = (0..256).collect();
        let before = metrics();
        let output = pool.install(|| {
            map_ordered(&input, 2, |value| {
                // Give work stealing enough useful lifetime to make worker
                // participation observable even on a single-core CI host.
                std::thread::sleep(std::time::Duration::from_micros(50));
                value.wrapping_mul(17).wrapping_add(3)
            })
        });
        let after = metrics();

        assert!(output.execution.parallel);
        assert!(
            output.execution.workers_used > 1,
            "four-thread pool must execute snapshot work on multiple workers: {:?}",
            output.execution
        );
        assert_eq!(
            output.values,
            input
                .iter()
                .map(|value| value.wrapping_mul(17).wrapping_add(3))
                .collect::<Vec<_>>()
        );
        assert!(after.calls > before.calls);
        assert!(after.parallel_calls > before.parallel_calls);
        assert!(after.parallel_items >= before.parallel_items + input.len() as u64);
        assert!(after.max_workers_used >= output.execution.workers_used as u64);
    }

    #[test]
    fn ordered_map_stays_sequential_below_threshold() {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        let input = [3, 1, 4, 1, 5];
        let output = pool.install(|| map_ordered(&input, 64, |value| value * 2));
        assert_eq!(output.values, vec![6, 2, 8, 2, 10]);
        assert_eq!(
            output.execution,
            ParallelExecution {
                items: input.len(),
                workers_used: 1,
                parallel: false,
            }
        );
    }
}
