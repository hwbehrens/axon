use std::collections::VecDeque;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use uuid::Uuid;

use super::protocol::BufferedMessage;
use crate::message::{Envelope, MessageKind};

pub struct ReceiveBuffer {
    messages: VecDeque<BufferedMessage>,
    capacity: usize,
    ttl_secs: u64,
}

impl ReceiveBuffer {
    pub fn new(capacity: usize, ttl_secs: u64) -> Self {
        Self {
            messages: VecDeque::with_capacity(capacity),
            capacity,
            ttl_secs,
        }
    }

    pub fn push(&mut self, envelope: Envelope) {
        self.evict_expired();

        let buffered_at = now_iso8601();
        let msg = BufferedMessage {
            envelope,
            buffered_at,
        };

        if self.messages.len() >= self.capacity {
            // Buffer full, evict oldest
            if let Some(evicted) = self.messages.pop_front() {
                tracing::debug!(evicted_id = %evicted.envelope.id, "receive buffer full, evicting oldest");
            }
        }

        self.messages.push_back(msg);
    }

    pub fn fetch(
        &mut self,
        limit: usize,
        since: Option<&str>,
        kinds: Option<&[MessageKind]>,
    ) -> (Vec<BufferedMessage>, bool) {
        self.evict_expired();

        let mut results = Vec::new();
        let mut has_more = false;
        let limit = limit.clamp(1, 1000);

        // Determine skip position
        let mut skip_count = 0;
        if let Some(since_val) = since {
            // Try parsing as UUID first (message ID)
            if let Ok(uuid) = Uuid::parse_str(since_val) {
                for (i, msg) in self.messages.iter().enumerate() {
                    if msg.envelope.id == uuid {
                        skip_count = i + 1;
                        break;
                    }
                }
            } else if let Ok(since_millis) = parse_iso8601_millis(since_val) {
                // Parse as ISO timestamp and compare milliseconds
                for (i, msg) in self.messages.iter().enumerate() {
                    if let Ok(msg_millis) = parse_iso8601_millis(&msg.buffered_at)
                        && msg_millis > since_millis
                    {
                        skip_count = i;
                        break;
                    }
                }
            }
        }

        for msg in self.messages.iter().skip(skip_count) {
            if let Some(filter_kinds) = kinds
                && !filter_kinds.contains(&msg.envelope.kind)
            {
                continue;
            }

            if results.len() < limit {
                results.push(msg.clone());
            } else {
                has_more = true;
                break;
            }
        }

        (results, has_more)
    }

    pub fn ack(&mut self, ids: &[Uuid]) -> usize {
        let mut acked = 0;
        self.messages.retain(|msg| {
            if ids.contains(&msg.envelope.id) {
                acked += 1;
                false
            } else {
                true
            }
        });
        acked
    }

    fn evict_expired(&mut self) {
        let now_millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let ttl_millis = (self.ttl_secs as i64) * 1000;

        let initial_len = self.messages.len();
        self.messages.retain(|msg| {
            // Parse the ISO timestamp and check TTL
            if let Ok(buffered_millis) = parse_iso8601_millis(&msg.buffered_at) {
                let age = now_millis.saturating_sub(buffered_millis);
                age < ttl_millis
            } else {
                true // Keep if we can't parse timestamp
            }
        });

        let expired_count = initial_len - self.messages.len();
        if expired_count > 0 {
            tracing::debug!(expired_count, "evicted expired messages from buffer");
        }
    }
}

fn now_iso8601() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let secs = now.as_secs();
    let millis = now.subsec_millis();

    // Simple ISO 8601 UTC format: YYYY-MM-DDTHH:MM:SS.sssZ
    let dt = chrono::DateTime::from_timestamp(secs as i64, millis * 1_000_000)
        .unwrap_or_else(chrono::Utc::now);
    dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn parse_iso8601_millis(s: &str) -> Result<i64> {
    use chrono::DateTime;
    let dt =
        DateTime::parse_from_rfc3339(s).with_context(|| format!("invalid ISO timestamp: {}", s))?;
    Ok(dt.timestamp_millis())
}

#[cfg(test)]
#[path = "receive_buffer_tests.rs"]
mod tests;
