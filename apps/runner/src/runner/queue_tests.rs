use super::queue::{Enqueued, TaskQueue};

#[test]
fn tasks_leave_in_arrival_order() {
    let mut q = TaskQueue::default();
    assert_eq!(q.enqueue("a", 1, "A1"), Enqueued::Added(1));
    assert_eq!(q.enqueue("b", 1, "B1"), Enqueued::Added(2));
    assert_eq!(q.enqueue("c", 1, "C1"), Enqueued::Added(3));
    assert_eq!(q.len(), 3);
    assert_eq!(q.position("b"), Some(2));
    assert_eq!(q.pop_front().map(|e| e.item), Some("A1"));
    assert_eq!(q.position("b"), Some(1));
    assert_eq!(q.pop_front().map(|e| e.item), Some("B1"));
    assert_eq!(q.pop_front().map(|e| e.item), Some("C1"));
    assert!(q.pop_front().is_none() && q.is_empty());
}

#[test]
fn the_same_task_and_attempt_is_ignored_and_a_new_attempt_replaces_in_place() {
    let mut q = TaskQueue::default();
    q.enqueue("a", 1, "A1");
    q.enqueue("b", 1, "B1");
    assert_eq!(q.enqueue("a", 1, "again"), Enqueued::Duplicate);
    assert_eq!(q.len(), 2);
    assert_eq!(q.front().map(|e| e.item), Some("A1"));
    assert_eq!(q.enqueue("a", 2, "A2"), Enqueued::Replaced(1));
    assert_eq!(q.len(), 2);
    assert_eq!(q.front().map(|e| (e.attempt, e.item)), Some((2, "A2")));
    // An older attempt arriving late replaces too: Sylvode's attempt check is what ends it.
    assert_eq!(q.enqueue("b", 3, "B3"), Enqueued::Replaced(2));
}

#[test]
fn entries_can_be_put_back_removed_and_drained() {
    let mut q = TaskQueue::default();
    q.enqueue("a", 1, 1);
    q.enqueue("b", 1, 2);
    let first = q.pop_front().expect("first");
    q.push_front(first);
    assert_eq!(q.position("a"), Some(1));
    assert_eq!(q.remove("a").map(|e| e.item), Some(1));
    assert!(q.remove("a").is_none());
    q.enqueue("c", 1, 3);
    let all: Vec<i32> = q.drain_all().into_iter().map(|e| e.item).collect();
    assert_eq!(all, [2, 3]);
    assert!(q.is_empty());
    assert_eq!(q.iter().count(), 0);
}

#[test]
fn an_older_attempt_never_replaces_a_newer_one() {
    let mut q = TaskQueue::default();
    q.enqueue("a", 3, "A3");
    assert_eq!(q.enqueue("a", 2, "A2"), Enqueued::Duplicate);
    assert_eq!(q.front().map(|e| (e.attempt, e.item)), Some((3, "A3")));
}
