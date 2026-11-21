//! The FIFO queue of one session (design section 7.4): arrival order, which is Sylvode's pick-up
//! order. The same task id at a new attempt replaces the old entry (in place); the same id and
//! attempt is ignored.

use std::collections::VecDeque;

/// One entry: the task and the data to run it with.
#[derive(Clone, Debug)]
pub struct Entry<T> {
    pub task_id: String,
    pub attempt: i64,
    pub item: T,
}

/// What happened to an enqueued task.
#[derive(Debug, PartialEq, Eq)]
pub enum Enqueued {
    /// Added at this 1-based position.
    Added(usize),
    /// Replaced an older attempt of the same task, which kept its place.
    Replaced(usize),
    /// The same task and attempt is already queued.
    Duplicate,
}

#[derive(Debug)]
pub struct TaskQueue<T> {
    entries: VecDeque<Entry<T>>,
}

impl<T> Default for TaskQueue<T> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }
}

impl<T> TaskQueue<T> {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Adds `item` unless the same task and attempt is queued.
    pub fn enqueue(&mut self, task_id: &str, attempt: i64, item: T) -> Enqueued {
        if let Some(index) = self.entries.iter().position(|e| e.task_id == task_id) {
            let entry = &mut self.entries[index];
            if entry.attempt == attempt {
                return Enqueued::Duplicate;
            }
            *entry = Entry {
                task_id: task_id.to_string(),
                attempt,
                item,
            };
            return Enqueued::Replaced(index + 1);
        }
        self.entries.push_back(Entry {
            task_id: task_id.to_string(),
            attempt,
            item,
        });
        Enqueued::Added(self.entries.len())
    }

    /// Puts an entry back at the front (a push that could not happen yet).
    pub fn push_front(&mut self, entry: Entry<T>) {
        self.entries.push_front(entry);
    }

    pub fn pop_front(&mut self) -> Option<Entry<T>> {
        self.entries.pop_front()
    }

    pub fn front(&self) -> Option<&Entry<T>> {
        self.entries.front()
    }

    /// The 1-based position of a task.
    pub fn position(&self, task_id: &str) -> Option<usize> {
        self.entries.iter().position(|e| e.task_id == task_id).map(|i| i + 1)
    }

    /// Removes a task, whatever its attempt.
    pub fn remove(&mut self, task_id: &str) -> Option<Entry<T>> {
        let index = self.entries.iter().position(|e| e.task_id == task_id)?;
        self.entries.remove(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Entry<T>> {
        self.entries.iter()
    }

    /// Removes and returns everything, in order.
    pub fn drain_all(&mut self) -> Vec<Entry<T>> {
        self.entries.drain(..).collect()
    }
}
