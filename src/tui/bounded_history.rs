use std::collections::VecDeque;

pub(super) struct BoundedHistory<T> {
    entries: VecDeque<T>,
    capacity: usize,
}

impl<T> BoundedHistory<T> {
    pub(super) fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "history capacity must be greater than zero");
        Self {
            entries: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(super) fn pop(&mut self) -> Option<T> {
        self.entries.pop_back()
    }
}

impl<T: PartialEq> BoundedHistory<T> {
    pub(super) fn push(&mut self, entry: T) {
        if self.entries.back() == Some(&entry) {
            return;
        }
        if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_discards_adjacent_duplicates() {
        let mut history = BoundedHistory::new(3);

        history.push("first");
        history.push("first");
        history.push("second");

        assert_eq!(history.pop(), Some("second"));
        assert_eq!(history.pop(), Some("first"));
        assert_eq!(history.pop(), None);
    }

    #[test]
    fn push_discards_oldest_entry_at_capacity() {
        let mut history = BoundedHistory::new(2);

        history.push("first");
        history.push("second");
        history.push("third");

        assert_eq!(history.pop(), Some("third"));
        assert_eq!(history.pop(), Some("second"));
        assert_eq!(history.pop(), None);
    }
}
