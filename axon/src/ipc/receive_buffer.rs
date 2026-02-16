use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::message::{Envelope, MessageKind};

use super::protocol::BufferedMessage;

// ---------------------------------------------------------------------------
// Receive buffer (IPC.md §4)
// ---------------------------------------------------------------------------

const DEFAULT_BYTE_CAP: usize = 4 * 1024 * 1024; // 4 MB

fn system_now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Approximate byte-size estimate for receive-buffer byte-cap enforcement.
/// Intentionally not exact serialization accounting — the `buffer_byte_cap`
/// eviction logic uses this as a soft safety limit.
fn envelope_byte_size(envelope: &Envelope) -> usize {
    // Estimate without full serialization: fixed overhead + variable-length fields.
    // This is intentionally an approximation — the byte cap is a soft safety limit,
    // not a precise accounting.
    let base = 128; // JSON envelope boilerplate (braces, field names, punctuation)
    let id_len = 36; // UUID string length
    let from_len = envelope.from.as_ref().map_or(0, |id| id.as_str().len());
    let to_len = envelope.to.as_ref().map_or(0, |id| id.as_str().len());
    let kind_len = 12; // max kind string length
    let payload_len = envelope.payload.get().len();
    let ref_len = if envelope.ref_id.is_some() { 36 } else { 0 };
    base + id_len + from_len + to_len + kind_len + payload_len + ref_len
}

#[derive(Debug, Clone)]
struct BufferedEntry {
    seq: u64,
    buffered_at_ms: u64,
    envelope: Envelope,
    byte_size: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ConsumerState {
    pub acked_seq: u64,
    pub highest_delivered_seq: u64,
    pub last_used_ms: u64,
}

pub struct ReceiveBuffer {
    entries: VecDeque<BufferedEntry>,
    capacity: usize,
    ttl_secs: u64,
    byte_cap: usize,
    total_bytes: usize,
    next_seq: u64,
    consumers: HashMap<String, ConsumerState>,
    max_consumers: usize,
    now_millis: Arc<dyn Fn() -> u64 + Send + Sync>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AckError {
    OutOfRange,
}

impl ReceiveBuffer {
    pub fn new(capacity: usize, ttl_secs: u64) -> Self {
        Self {
            entries: VecDeque::with_capacity(capacity.min(1024)),
            capacity,
            ttl_secs,
            byte_cap: DEFAULT_BYTE_CAP,
            total_bytes: 0,
            next_seq: 1,
            consumers: HashMap::new(),
            max_consumers: 1024,
            now_millis: Arc::new(system_now_millis),
        }
    }

    pub fn with_clock(mut self, clock: Arc<dyn Fn() -> u64 + Send + Sync>) -> Self {
        self.now_millis = clock;
        self
    }

    pub fn with_byte_cap(mut self, byte_cap: usize) -> Self {
        self.byte_cap = byte_cap;
        self
    }

    #[cfg(test)]
    pub fn with_max_consumers(mut self, max: usize) -> Self {
        self.max_consumers = max;
        self
    }

    #[cfg(test)]
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    pub fn push(&mut self, envelope: Envelope) -> (u64, u64) {
        self.evict_expired();

        let buffered_at_ms = (self.now_millis)();
        let seq = self.next_seq;
        self.next_seq += 1;

        // If capacity is 0, buffering is disabled — don't store
        if self.capacity == 0 {
            return (seq, buffered_at_ms);
        }

        let byte_size = envelope_byte_size(&envelope);

        // Evict oldest if capacity exceeded
        while self.entries.len() >= self.capacity && !self.entries.is_empty() {
            if let Some(evicted) = self.entries.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(evicted.byte_size);
                tracing::debug!(
                    evicted_seq = evicted.seq,
                    evicted_id = %evicted.envelope.id,
                    "receive buffer full, evicting oldest"
                );
            }
        }

        // Evict oldest if byte cap exceeded
        while self.total_bytes + byte_size > self.byte_cap && !self.entries.is_empty() {
            if let Some(evicted) = self.entries.pop_front() {
                self.total_bytes = self.total_bytes.saturating_sub(evicted.byte_size);
                tracing::debug!(
                    evicted_seq = evicted.seq,
                    "receive buffer byte cap, evicting oldest"
                );
            }
        }

        self.total_bytes += byte_size;
        self.entries.push_back(BufferedEntry {
            seq,
            buffered_at_ms,
            envelope,
            byte_size,
        });

        (seq, buffered_at_ms)
    }

    pub fn fetch(
        &mut self,
        consumer: &str,
        limit: usize,
        kinds: Option<&[MessageKind]>,
    ) -> (Vec<BufferedMessage>, Option<u64>, bool) {
        self.evict_expired();

        let consumer_state = self.consumers.entry(consumer.to_string()).or_default();
        consumer_state.last_used_ms = (self.now_millis)();
        let acked_seq = consumer_state.acked_seq;

        let mut results = Vec::new();
        let mut has_more = false;
        let mut highest_seq = None;

        for entry in &self.entries {
            if entry.seq <= acked_seq {
                continue;
            }

            // When a kinds filter is active, stop at the first non-matching
            // message rather than skipping it. This prevents the ack cursor
            // from jumping past messages that were never delivered.
            if let Some(filter_kinds) = kinds
                && !filter_kinds.contains(&entry.envelope.kind)
            {
                has_more = true;
                break;
            }

            if results.len() < limit {
                highest_seq = Some(entry.seq);
                results.push(BufferedMessage {
                    seq: entry.seq,
                    buffered_at_ms: entry.buffered_at_ms,
                    envelope: entry.envelope.clone(),
                });
            } else {
                has_more = true;
                break;
            }
        }

        self.gc_consumers();

        (results, highest_seq, has_more)
    }

    pub fn ack(&mut self, consumer: &str, up_to_seq: u64) -> Result<u64, AckError> {
        let consumer_state = self.consumers.entry(consumer.to_string()).or_default();
        consumer_state.last_used_ms = (self.now_millis)();

        if up_to_seq > consumer_state.highest_delivered_seq {
            return Err(AckError::OutOfRange);
        }

        consumer_state.acked_seq = up_to_seq;

        self.gc_consumers();

        Ok(up_to_seq)
    }

    pub fn highest_seq(&self) -> u64 {
        self.entries.back().map(|e| e.seq).unwrap_or(0)
    }

    pub fn replay_messages(
        &mut self,
        consumer: &str,
        replay_to_seq: u64,
        kinds: Option<&[MessageKind]>,
    ) -> Vec<BufferedMessage> {
        self.evict_expired();

        let consumer_state = self.consumers.entry(consumer.to_string()).or_default();
        consumer_state.last_used_ms = (self.now_millis)();
        let acked_seq = consumer_state.acked_seq;
        let mut results = Vec::new();

        for entry in &self.entries {
            if entry.seq <= acked_seq {
                continue;
            }
            if entry.seq > replay_to_seq {
                break;
            }
            // Stop at non-matching kinds to preserve ack cursor safety
            if let Some(filter_kinds) = kinds
                && !filter_kinds.contains(&entry.envelope.kind)
            {
                break;
            }
            results.push(BufferedMessage {
                seq: entry.seq,
                buffered_at_ms: entry.buffered_at_ms,
                envelope: entry.envelope.clone(),
            });
        }

        self.gc_consumers();

        results
    }

    pub fn update_delivered_seq(&mut self, consumer: &str, seq: u64) {
        let cs = self.consumers.entry(consumer.to_string()).or_default();
        cs.last_used_ms = (self.now_millis)();
        if seq > cs.highest_delivered_seq {
            cs.highest_delivered_seq = seq;
        }
    }

    fn gc_consumers(&mut self) {
        if self.max_consumers == 0 || self.consumers.len() <= self.max_consumers {
            return;
        }
        let mut entries: Vec<(String, u64)> = self
            .consumers
            .iter()
            .map(|(k, v)| (k.clone(), v.last_used_ms))
            .collect();
        entries.sort_by_key(|(_, ts)| *ts);
        let to_remove = self.consumers.len() - self.max_consumers;
        for (key, _) in entries.into_iter().take(to_remove) {
            self.consumers.remove(&key);
        }
    }

    fn evict_expired(&mut self) {
        if self.ttl_secs == 0 {
            return;
        }

        let now_ms = (self.now_millis)();
        let ttl_ms = self.ttl_secs * 1000;

        let initial_len = self.entries.len();
        while let Some(front) = self.entries.front() {
            let age = now_ms.saturating_sub(front.buffered_at_ms);
            if age >= ttl_ms {
                if let Some(evicted) = self.entries.pop_front() {
                    self.total_bytes = self.total_bytes.saturating_sub(evicted.byte_size);
                }
            } else {
                break;
            }
        }

        let expired_count = initial_len - self.entries.len();
        if expired_count > 0 {
            tracing::debug!(expired_count, "evicted expired messages from buffer");
        }
    }
}

#[cfg(test)]
#[path = "receive_buffer_tests.rs"]
mod tests;
