//! Delivery lanes: the tasks of one tick, grouped by bot.
//!
//! A lane delivers its tasks one at a time in pick-up order, so a bot's tasks reach its runner in
//! the order they were picked up. Lanes run concurrently up to a cap. With a cap of 1 there is no
//! lane machinery: the tasks are delivered one after the other in pick-up order and the first
//! error ends the batch. The worker's own `deliver` settles every task on its own and never
//! returns an error (see `ai_tasks::settle_task`), so one failing task does not end a batch there.
//!
//! With a cap above 1 a failed delivery ends only its own lane (its remaining tasks keep their
//! lease and are swept as before); other lanes finish, and the first error is returned after all
//! lanes have ended. A lane that panics is logged and does not end the others.

use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// Shape of a tick's lanes, for the `delivery tick` log line.
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

/// Delivers `tasks` with `deliver`, at most `concurrency` lanes at a time (see the module text).
pub async fn run_lanes<T, K, F, Fut>(
    tasks: Vec<T>,
    key_of: impl Fn(&T) -> K,
    concurrency: usize,
    deliver: F,
) -> anyhow::Result<()>
where
    T: Send + 'static,
    K: Eq + Hash + Clone,
    F: Fn(T) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    if concurrency <= 1 {
        for task in tasks {
            deliver(task).await?;
        }
        return Ok(());
    }
    let permits = Arc::new(Semaphore::new(concurrency));
    let mut set = JoinSet::new();
    for lane in group(tasks, key_of) {
        let (permits, deliver) = (Arc::clone(&permits), deliver.clone());
        set.spawn(async move {
            let _permit = permits
                .acquire_owned()
                .await
                .map_err(|_| anyhow::anyhow!("the delivery lane limit is closed"))?;
            for task in lane {
                deliver(task).await?;
            }
            Ok::<(), anyhow::Error>(())
        });
    }
    let mut first_error = None;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                first_error.get_or_insert(err);
            }
            Err(join) => tracing::error!(error = %join, "a delivery lane ended abnormally"),
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::{LaneStats, lane_stats, run_lanes};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// `(bot, sequence)`
    type Task = (u8, u32);

    fn tasks() -> Vec<Task> {
        vec![(1, 1), (2, 1), (1, 2), (3, 1), (2, 2), (1, 3)]
    }

    async fn run(concurrency: usize) -> (Vec<Task>, usize) {
        let done = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let (active, peak) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (done_in, active_in, peak_in) = (done.clone(), active.clone(), peak.clone());
        run_lanes(
            tasks(),
            |task: &Task| task.0,
            concurrency,
            move |task: Task| {
                let (done, active, peak) = (done_in.clone(), active_in.clone(), peak_in.clone());
                async move {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    done.lock().await.push(task);
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                }
            },
        )
        .await
        .expect("all delivered");
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
    async fn concurrency_one_is_the_serial_pick_up_order() {
        let (done, peak) = run(1).await;
        assert_eq!(done, tasks());
        assert_eq!(peak, 1);
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
    }

    #[tokio::test]
    async fn the_serial_path_stops_the_batch_at_the_first_error() {
        let delivered = Arc::new(AtomicUsize::new(0));
        let delivered_in = delivered.clone();
        let outcome = run_lanes(
            tasks(),
            |task: &Task| task.0,
            1,
            move |task: Task| {
                let delivered = delivered_in.clone();
                async move {
                    delivered.fetch_add(1, Ordering::SeqCst);
                    if task == (2, 1) {
                        anyhow::bail!("down");
                    }
                    Ok(())
                }
            },
        )
        .await;
        assert!(outcome.is_err());
        assert_eq!(delivered.load(Ordering::SeqCst), 2, "the rest of the batch is dropped");
    }

    #[tokio::test]
    async fn a_failure_ends_only_its_own_lane_when_lanes_run() {
        let done = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let done_in = done.clone();
        let outcome = run_lanes(
            tasks(),
            |task: &Task| task.0,
            3,
            move |task: Task| {
                let done = done_in.clone();
                async move {
                    if task == (1, 2) {
                        anyhow::bail!("down");
                    }
                    done.lock().await.push(task);
                    Ok(())
                }
            },
        )
        .await;
        assert!(outcome.is_err());
        let mut finished = done.lock().await.clone();
        finished.sort_unstable();
        assert_eq!(
            finished,
            vec![(1, 1), (2, 1), (2, 2), (3, 1)],
            "bot 1 stopped, the others finished"
        );
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
