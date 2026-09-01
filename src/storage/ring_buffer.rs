use crate::collector::snapshot::SystemSnapshot;
use std::collections::VecDeque;

pub struct RingBuffer {
    buffer: VecDeque<SystemSnapshot>,
    capacity: usize,
}

impl RingBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            buffer: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, snapshot: SystemSnapshot) {
        if self.buffer.len() >= self.capacity {
            self.buffer.pop_front();
        }
        self.buffer.push_back(snapshot);
    }

    pub fn get_all(&self) -> Vec<SystemSnapshot> {
        self.buffer.iter().cloned().collect()
    }

    pub fn latest(&self) -> Option<&SystemSnapshot> {
        self.buffer.back()
    }

    pub fn get_since(&self, since_timestamp: u64) -> Vec<SystemSnapshot> {
        self.buffer
            .iter()
            .filter(|s| s.timestamp >= since_timestamp)
            .cloned()
            .collect()
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }
}
