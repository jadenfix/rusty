/// A fixed-capacity ring buffer. Pushing onto a full buffer overwrites the
/// oldest item. Items come out oldest first.
pub struct RingBuffer<T> {
    buf: Vec<Option<T>>,
    /// Index of the oldest item.
    head: usize,
    len: usize,
}

impl<T> RingBuffer<T> {
    /// Panics if `capacity` is zero.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "capacity must be positive");
        Self { buf: (0..capacity).map(|_| None).collect(), head: 0, len: 0 }
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Adds an item at the newest end. When the buffer is full, the oldest
    /// item is overwritten and returned.
    pub fn push(&mut self, item: T) -> Option<T> {
        let cap = self.capacity();
        let tail = (self.head + self.len) % cap;
        if self.len == cap {
            let old = self.buf[tail].replace(item);
            self.head = (self.head + 1) % cap;
            old
        } else {
            self.buf[tail] = Some(item);
            self.len += 1;
            None
        }
    }

    /// Removes and returns the oldest item.
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let item = self.buf[self.head].take();
        self.head += 1;
        self.len -= 1;
        item
    }

    /// Items from oldest to newest.
    pub fn iter(&self) -> impl Iterator<Item = &T> + '_ {
        (0..self.len).filter_map(move |i| self.buf[(self.head + i) % self.len].as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_newest_items_when_full() {
        let mut r = RingBuffer::new(3);
        for i in 1..=5 {
            r.push(i);
        }
        assert_eq!(r.iter().copied().collect::<Vec<_>>(), vec![3, 4, 5]);
        assert_eq!(r.len(), 3);
    }
}
