//! Delivery lanes: the tasks of one bot, delivered one at a time in pick-up order.
//!
//! Each poll of the delivery loop groups the tasks it picked up by bot and starts one lane per
//! bot. A bot has at most one lane in flight: the loop asks [`Lanes::busy`] which bots to leave
//! out of its next pick-up, so a bot's tasks reach its runner in the order they were picked up and
//! never overlap. Lanes run concurrently up to a cap; a lane beyond the cap waits for a permit
//! while it still counts as in flight.
//!
//! The worker's own `deliver` settles every task on its own and never fails the lane: a task that
//! cannot be settled is logged and keeps its lease (see `ai_tasks::settle_task`), and the lane goes
//! on. A lane that panics is logged when it is reaped; its key is freed on unwind, and its tasks
//! keep their lease and are swept as before.
//!
//! On shutdown a lane delivers no further task. The one it is delivering finishes within the
//! delivery timeout, and the rest keep their lease.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::hash::Hash;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;

/// Shape of a poll's lanes, for the `delivery tick` log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneStats {
    pub lanes: usize,
    pub max_lane_len: usize,
}

/// Groups `tasks` into lanes by `key_of`, each in its original order, lanes in first-seen order.
fn group<T, K: Eq + Hash + Clone>(tasks: Vec<T>, key_of: impl Fn(&T) -> K) -> Vec<Vec<T>> {
    let mut index: HashMap<K, usize> = HashMap::new();
    let mut lanes: Vec<Vec<T>> = Vec::new();
    for task in tasks {
        let key = key_of(&task);
        if let Some(&position) = index.get(&key) {
            if let Some(lane) = lanes.get_mut(position) {
                lane.push(task);
            }
        } else {
            index.insert(key, lanes.len());
            lanes.push(vec![task]);
        }
    }
    lanes
}

/// How many lanes `tasks` form and how long the longest is.
pub fn lane_stats<T, K: Eq + Hash>(tasks: &[T], key_of: impl Fn(&T) -> K) -> LaneStats {
    let mut counts: HashMap<K, usize> = HashMap::new();
    for task in tasks {
        *counts.entry(key_of(task)).or_default() += 1;
    }
    LaneStats {
        lanes: counts.len(),
        max_lane_len: counts.values().copied().max().unwrap_or(0),
    }
}

/// Frees a lane's key when the lane ends, whether it ran to the end, was dropped or panicked.
struct LaneKey<K: Eq + Hash> {
    in_flight: Arc<Mutex<HashSet<K>>>,
    key: K,
}

impl<K: Eq + Hash> Drop for LaneKey<K> {
    fn drop(&mut self) {
        self.in_flight.lock().remove(&self.key);
    }
}

/// The lanes in flight, and the cap on how many deliver at once.
pub struct Lanes<K: Eq + Hash> {
    in_flight: Arc<Mutex<HashSet<K>>>,
    permits: Arc<Semaphore>,
    concurrency: usize,
    stop: watch::Receiver<bool>,
    set: JoinSet<()>,
}

impl<K: Eq + Hash + Clone + Send + 'static> Lanes<K> {
    /// At most `concurrency` lanes deliver at once; `stop` set to `true` ends every lane after the
    /// delivery it is making.
    pub fn new(concurrency: usize, stop: watch::Receiver<bool>) -> Self {
        let concurrency = concurrency.max(1);
        Self {
            in_flight: Arc::new(Mutex::new(HashSet::new())),
            permits: Arc::new(Semaphore::new(concurrency)),
            concurrency,
            stop,
            set: JoinSet::new(),
        }
    }

    /// The keys that have a lane in flight: the bots the next pick-up leaves out.
    pub fn busy(&self) -> Vec<K> {
        self.in_flight.lock().iter().cloned().collect()
    }

    /// How many lanes are in flight (delivering or waiting for a permit).
    pub fn in_flight(&self) -> usize {
        self.in_flight.lock().len()
    }

    /// How many more lanes the cap allows.
    pub fn free(&self) -> usize {
        self.concurrency.saturating_sub(self.in_flight())
    }

    /// Starts one lane per key of `tasks`, each delivering its tasks in the order given. A key that
    /// already has a lane in flight is never given a second one: its tasks are handed back.
    pub fn start<T, F, Fut>(&mut self, tasks: Vec<T>, key_of: impl Fn(&T) -> K, deliver: F) -> Vec<T>
    where
        T: Send + 'static,
        F: Fn(T) -> Fut + Clone + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut refused = Vec::new();
        for lane in group(tasks, &key_of) {
            let Some(key) = lane.first().map(&key_of) else { continue };
            if !self.in_flight.lock().insert(key.clone()) {
                refused.extend(lane);
                continue;
            }
            let guard = LaneKey {
                in_flight: Arc::clone(&self.in_flight),
                key,
            };
            let (permits, deliver, stop) = (Arc::clone(&self.permits), deliver.clone(), self.stop.clone());
            self.set.spawn(async move {
                let _guard = guard;
                let Ok(_permit) = permits.acquire_owned().await else {
                    return;
                };
                for task in lane {
                    if *stop.borrow() {
                        break;
                    }
                    deliver(task).await;
                }
            });
        }
        refused
    }

    /// Collects the lanes that have ended, without waiting; a lane that panicked is logged.
    pub fn reap(&mut self) {
        while let Some(joined) = self.set.try_join_next() {
            if let Err(join) = joined {
                tracing::error!(error = %join, "a delivery lane ended abnormally");
            }
        }
    }

    /// Waits for every lane to end (after `stop` is set they end after their current delivery).
    pub async fn drain(&mut self) {
        while let Some(joined) = self.set.join_next().await {
            if let Err(join) = joined {
                tracing::error!(error = %join, "a delivery lane ended abnormally");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LaneStats, Lanes, lane_stats};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::watch;

    /// `(bot, sequence)`
    type Task = (u8, u32);

    fn tasks() -> Vec<Task> {
        vec![(1, 1), (2, 1), (1, 2), (3, 1), (2, 2), (1, 3)]
    }

    async fn run(concurrency: usize) -> (Vec<Task>, usize) {
        let done = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let (active, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (done_in, active_in, peak_in) = (done.clone(), active.clone(), peak.clone());
        let (_stop_tx, stop_rx) = watch::channel(false);
        let mut lanes = Lanes::new(concurrency, stop_rx);
        let refused = lanes.start(
            tasks(),
            |task: &Task| task.0,
            move |task: Task| {
                let (done, active, peak) = (done_in.clone(), active_in.clone(), peak_in.clone());
                async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    done.lock().await.push(task);
                    active.fetch_sub(1, Ordering::SeqCst);
                }
            },
        );
        assert!(refused.is_empty());
        lanes.drain().await;
        let finished = done.lock().await.clone();
        (finished, peak.load(Ordering::SeqCst))
    }

    fn per_bot_order_holds(done: &[Task]) -> bool {
        (1..=3_u8).all(|bot| {
            let sequence: Vec<u32> = done.iter().filter(|task| task.0 == bot).map(|task| task.1).collect();
            sequence.windows(2).all(|pair| matches!(pair, [a, b] if a < b))
        })
    }

    #[tokio::test]
    async fn lanes_keep_each_bots_order_and_run_bots_concurrently() {
        let (done, peak) = run(3).await;
        assert_eq!(done.len(), 6);
        assert!(per_bot_order_holds(&done), "{done:?}");
        assert_eq!(peak, 3);
        let (done, peak) = run(2).await;
        assert!(per_bot_order_holds(&done), "{done:?}");
        assert_eq!(peak, 2, "the cap holds with three lanes");
        let (done, peak) = run(1).await;
        assert!(per_bot_order_holds(&done), "{done:?}");
        assert_eq!(peak, 1);
    }

    #[test]
    fn lane_stats_count_lanes_and_the_longest() {
        assert_eq!(
            lane_stats(&tasks(), |task: &Task| task.0),
            LaneStats {
                lanes: 3,
                max_lane_len: 3
            }
        );
        assert_eq!(
            lane_stats(&Vec::<Task>::new(), |task: &Task| task.0),
            LaneStats {
                lanes: 0,
                max_lane_len: 0
            }
        );
    }
}
